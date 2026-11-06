import { test } from "node:test";
import assert from "node:assert/strict";
import { createHmac, hkdfSync, randomBytes } from "node:crypto";
import { decrypt, encrypt, hkdfExpand, keyFromB64, parseAccessToken, tokenKey, DecryptError, TokenError } from "../src/providers/bitwarden_crypto.js";

test("access token parsing", () => {
  const seed = randomBytes(16);
  const t = parseAccessToken(`0.6b2f0a8e-1c3d-4e5f-8a9b-0c1d2e3f4a5b.secret123:${seed.toString("base64")}`);
  assert.equal(t.id, "6b2f0a8e-1c3d-4e5f-8a9b-0c1d2e3f4a5b");
  assert.equal(t.secret, "secret123");
  assert.ok(t.seed.equals(seed));
  for (const bad of ["", "0.id.secret", "1.6b2f0a8e-1c3d-4e5f-8a9b-0c1d2e3f4a5b.s:" + seed.toString("base64"), "0.nope.s:" + seed.toString("base64"), "0.6b2f0a8e-1c3d-4e5f-8a9b-0c1d2e3f4a5b.s:AAAA"]) {
    assert.throws(() => parseAccessToken(bad), TokenError, bad);
  }
});

test("the token key is HKDF with salt bitwarden-accesstoken and info sm-access-token", () => {
  // Node's hkdfSync(sha256, ikm, salt, info) = Expand(Extract(salt, ikm)); Extract(salt, ikm) = HMAC(salt, ikm)
  const ikm = randomBytes(16);
  const salt = Buffer.from("bitwarden-accesstoken");
  const prk = createHmac("sha256", salt).update(ikm).digest();
  const node = Buffer.from(hkdfSync("sha256", ikm, salt, Buffer.from("sm-access-token"), 64));
  assert.ok(hkdfExpand(prk, Buffer.from("sm-access-token"), 64).equals(node));
  const k = tokenKey(ikm);
  assert.ok(Buffer.concat([k.enc, k.mac]).equals(node));
});

test("EncString round trip, and a wrong key fails the MAC", () => {
  const key = keyFromB64(randomBytes(64).toString("base64"));
  const s = encrypt("hello secret", key);
  assert.match(s, /^2\.[^|]+\|[^|]+\|[^|]+$/);
  assert.equal(decrypt(s, key).toString(), "hello secret");
  assert.throws(() => decrypt(s, keyFromB64(randomBytes(64).toString("base64"))), DecryptError);
  assert.throws(() => decrypt("0.abc|def", key), DecryptError);
});
