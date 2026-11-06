import { test } from "node:test";
import assert from "node:assert/strict";
import { parseRef, RefError } from "../src/refs.js";

test("op:// with and without a section, and an attribute", () => {
  assert.deepEqual(parseRef("op://Ops/Square/access_token"), { scheme: "op", vault: "Ops", item: "Square", field: "access_token", attribute: undefined });
  assert.deepEqual(parseRef("op://Ops/Square/api/secret"), { scheme: "op", vault: "Ops", item: "Square", section: "api", field: "secret", attribute: undefined });
  assert.equal((parseRef("OP://Ops/Square/password?attribute=OTP") as { attribute?: string }).attribute, "otp");
  assert.equal(parseRef("op://My Vault/My Item/my field").scheme, "op");
});

test("bws:// takes a UUID and nothing else", () => {
  assert.deepEqual(parseRef("bws://3F1C2B4A-5D6E-4F70-8A91-B2C3D4E5F607"), { scheme: "bws", id: "3f1c2b4a-5d6e-4f70-8a91-b2c3d4e5f607" });
});

test("malformed references", () => {
  for (const bad of [
    "",
    "op://",
    "op://Ops",
    "op://Ops/Square",
    "op://Ops/Square/a/b/c",
    "op:///Square/field",
    "op://Ops//field",
    "op://Ops/Square/ ",
    "op://Ops/Square/field?attribute=",
    "op://Ops/Square/field?foo=bar",
    "op://Ops/Square/field?attribute=otp&x=1",
    "op://Ops/Sq\nuare/field",
    `op://${"v".repeat(257)}/i/f`,
    "bws://not-a-uuid",
    "bws://3f1c2b4a-5d6e-4f70-8a91-b2c3d4e5f607/extra",
    "https://example.com/a/b",
    "Ops/Square/field",
    "x".repeat(1025),
  ]) {
    assert.throws(() => parseRef(bad), RefError, JSON.stringify(bad));
  }
});
