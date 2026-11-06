// The small part of Bitwarden's client-side crypto a Secrets Manager machine account needs, written
// from Bitwarden's public description and its open-source clients (not linked: the SDK's licence is the
// Bitwarden SDK License Agreement, see docs/architecture/specs/broker.md, "Licence").
//
// Access token: "0.<access token id>.<client secret>:<base64 of a 16-byte seed>".
// Token key: HMAC-SHA256(key = "bitwarden-accesstoken", data = seed) as the PRK, HKDF-Expand with info
//   "sm-access-token" to 64 bytes: AES key (first 32), MAC key (last 32).
// EncString type 2: "2.<iv b64>|<ciphertext b64>|<mac b64>", AES-256-CBC with PKCS#7 padding, MAC =
//   HMAC-SHA256(mac key, iv || ciphertext), checked in constant time before decrypting.

import { createCipheriv, createDecipheriv, createHmac, randomBytes, timingSafeEqual } from "node:crypto";

export type SymKey = { enc: Buffer; mac: Buffer };

export type AccessToken = { id: string; secret: string; seed: Buffer };

const UUID = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;

export class TokenError extends Error {}

export function parseAccessToken(token: string): AccessToken {
  const colon = token.indexOf(":");
  if (colon < 0) throw new TokenError("a Bitwarden access token is 0.<id>.<secret>:<key>");
  const [version, id, secret, ...extra] = token.slice(0, colon).split(".");
  if (version !== "0" || extra.length || !id || !secret) throw new TokenError("a Bitwarden access token is 0.<id>.<secret>:<key>");
  if (!UUID.test(id)) throw new TokenError("the access token id is not a UUID");
  const seed = Buffer.from(token.slice(colon + 1), "base64");
  if (seed.length !== 16) throw new TokenError("the access token key is not 16 bytes");
  return { id, secret, seed };
}

export function tokenKey(seed: Buffer): SymKey {
  const prk = createHmac("sha256", "bitwarden-accesstoken").update(seed).digest();
  // HKDF-Expand only (the HMAC above is the extract step; Node's hkdfSync always extracts again)
  const okm = hkdfExpand(prk, Buffer.from("sm-access-token"), 64);
  return { enc: okm.subarray(0, 32), mac: okm.subarray(32, 64) };
}

/** RFC 5869 HKDF-Expand with SHA-256. */
export function hkdfExpand(prk: Buffer, info: Buffer, length: number): Buffer {
  const blocks: Buffer[] = [];
  let prev = Buffer.alloc(0);
  for (let i = 1; Buffer.concat(blocks).length < length; i++) {
    prev = createHmac("sha256", prk).update(Buffer.concat([prev, info, Buffer.from([i])])).digest();
    blocks.push(prev);
  }
  return Buffer.concat(blocks).subarray(0, length);
}

export function keyFromB64(b64: string): SymKey {
  const raw = Buffer.from(b64, "base64");
  if (raw.length !== 64) throw new Error("a Bitwarden symmetric key is 64 bytes");
  return { enc: raw.subarray(0, 32), mac: raw.subarray(32, 64) };
}

export class DecryptError extends Error {}

export function decrypt(encString: string, key: SymKey): Buffer {
  const dot = encString.indexOf(".");
  if (dot < 0 || encString.slice(0, dot) !== "2") throw new DecryptError("only type 2 (AES-256-CBC, HMAC-SHA256) is supported");
  const parts = encString.slice(dot + 1).split("|");
  if (parts.length !== 3) throw new DecryptError("a type 2 value has three parts");
  const [iv, data, mac] = parts.map((p) => Buffer.from(p, "base64")) as [Buffer, Buffer, Buffer];
  if (iv.length !== 16 || mac.length !== 32) throw new DecryptError("bad IV or MAC length");
  const want = createHmac("sha256", key.mac).update(Buffer.concat([iv, data])).digest();
  if (!timingSafeEqual(want, mac)) throw new DecryptError("the MAC does not match (wrong key)");
  const d = createDecipheriv("aes-256-cbc", key.enc, iv);
  return Buffer.concat([d.update(data), d.final()]);
}

/** The inverse, for the mock Bitwarden server in the tests. */
export function encrypt(plain: Buffer | string, key: SymKey): string {
  const iv = randomBytes(16);
  const c = createCipheriv("aes-256-cbc", key.enc, iv);
  const data = Buffer.concat([c.update(typeof plain === "string" ? Buffer.from(plain) : plain), c.final()]);
  const mac = createHmac("sha256", key.mac).update(Buffer.concat([iv, data])).digest();
  return `2.${iv.toString("base64")}|${data.toString("base64")}|${mac.toString("base64")}`;
}
