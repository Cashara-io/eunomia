// The real sidecar server against mock providers: 1Password Connect and Bitwarden over HTTP, the service
// account SDK as a fake (it talks to 1Password's own servers, which no mock stands in for).
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import type { AddressInfo } from "node:net";
import { makeServer } from "../src/server.js";
import type { SdkFactory } from "../src/providers/service_account.js";
import { mockConnect, CONNECT_TOKEN, VALUES } from "./mocks/connect.js";
import { mockBitwarden, ACCESS_TOKEN, SECRET_ID, DENIED_ID, VALUE } from "./mocks/bitwarden.js";

const KEY = "internal-key-for-tests";
const SA_TOKEN = "ops_" + "eyJmYWtlIjoidG9rZW4ifQ".repeat(5);
const fakeSdk: SdkFactory = async (token) => {
  if (token !== SA_TOKEN) throw new Error(`invalid service account token ${token}`);
  return {
    vaults: { list: async () => [{ id: "v1" }, { id: "v2" }] },
    secrets: {
      resolveAll: async (refs: string[]) => ({
        individualResponses: Object.fromEntries(refs.map((r) => [r, r === "op://Ops/Square/password" ? { content: { secret: "sa-canary-value" } } : { error: { type: "itemNotFound" } }])),
      }),
    },
  };
};

const logs: string[] = [];
let base = "";
let connect: Awaited<ReturnType<typeof mockConnect>>;
let bw: Awaited<ReturnType<typeof mockBitwarden>>;
const server = makeServer({ key: KEY, sdk: fakeSdk, log: (l) => logs.push(l), timeoutMs: 2000 });

before(async () => {
  connect = await mockConnect();
  bw = await mockBitwarden();
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});
after(async () => {
  await new Promise<void>((r) => server.close(() => r()));
  await connect.close();
  await bw.close();
});

const post = async (path: string, body: unknown, key: string | null = KEY) => {
  const res = await fetch(base + path, { method: "POST", headers: { "content-type": "application/json", ...(key ? { authorization: `Bearer ${key}` } : {}) }, body: typeof body === "string" ? body : JSON.stringify(body) });
  return { status: res.status, body: (await res.json()) as any, cache: res.headers.get("cache-control") };
};
const op = (token = CONNECT_TOKEN) => ({ kind: "onepassword_connect", token, host: connect.base });

test("healthz needs no key", async () => {
  const res = await fetch(base + "/healthz");
  assert.equal(res.status, 200);
  assert.deepEqual(((await res.json()) as any).api, 1);
});

test("the internal key is required", async () => {
  assert.equal((await post("/v1/resolve", { provider: op(), refs: ["op://Ops/Square/access_token"] }, null)).status, 401);
  assert.equal((await post("/v1/resolve", { provider: op(), refs: ["op://Ops/Square/access_token"] }, "wrong")).status, 401);
});

test("1Password Connect: top-level and section fields, per-reference errors, in order", async () => {
  const r = await post("/v1/resolve", {
    provider: op(),
    refs: ["op://Ops/Square/access_token", "op://ops/square/API/secret", "op://Ops/Square/nope", "op://Ops/Twin/x", "op://Nope/Square/access_token", "op://Ops/Square/empty", "bws://3f1c2b4a-5d6e-4f70-8a91-b2c3d4e5f607", "op://bad"],
  });
  assert.equal(r.status, 200);
  assert.equal(r.cache, "no-store");
  const res = r.body.results;
  assert.deepEqual(res[0], { ref: "op://Ops/Square/access_token", ok: true, value: VALUES.access });
  assert.deepEqual(res[1], { ref: "op://ops/square/API/secret", ok: true, value: VALUES.secret });
  assert.deepEqual(res.slice(2).map((x: any) => x.error), ["not_found", "not_found", "not_found", "not_found", "invalid_ref", "invalid_ref"]);
});

test("1Password Connect: a refused token is denied for every reference, and the token is in no message or log", async () => {
  const bad = "connect-token-that-is-wrong-xyz";
  const r = await post("/v1/resolve", { provider: op(bad), refs: ["op://Ops/Square/access_token", "op://Ops/Square/api/secret"] });
  assert.deepEqual(r.body.results.map((x: any) => x.error), ["denied", "denied"]);
  const check = await post("/v1/check", { provider: op(bad) });
  assert.equal(check.body.ok, false);
  assert.equal(check.body.error, "denied");
  for (const text of [JSON.stringify(r.body), JSON.stringify(check.body), logs.join("\n")]) assert.ok(!text.includes(bad), text);
});

test("1Password Connect: test connection", async () => {
  assert.deepEqual((await post("/v1/check", { provider: op() })).body, { ok: true, detail: "2 vaults visible" });
});

test("Bitwarden: login, decrypt, per-secret errors", async () => {
  const provider = { kind: "bitwarden_sm", token: ACCESS_TOKEN, host: bw.base };
  const r = await post("/v1/resolve", { provider, refs: [`bws://${SECRET_ID}`, `bws://${DENIED_ID}`, "bws://00000000-0000-4000-8000-000000000000", "op://Ops/Square/x"] });
  assert.equal(r.status, 200);
  assert.deepEqual(r.body.results[0], { ref: `bws://${SECRET_ID}`, ok: true, value: VALUE });
  assert.deepEqual(r.body.results.slice(1).map((x: any) => x.error), ["denied", "not_found", "invalid_ref"]);
  assert.deepEqual((await post("/v1/check", { provider })).body, { ok: true, detail: "signed in as the machine account" });
});

test("Bitwarden: a wrong client secret or key is denied; a malformed token is bad_config", async () => {
  const wrongSecret = ACCESS_TOKEN.replace("e2eClientSecret", "e2eWrongSecret0");
  const r = await post("/v1/resolve", { provider: { kind: "bitwarden_sm", token: wrongSecret, host: bw.base }, refs: [`bws://${SECRET_ID}`] });
  assert.equal(r.body.results[0].error, "denied");
  const wrongKey = ACCESS_TOKEN.slice(0, ACCESS_TOKEN.indexOf(":") + 1) + Buffer.alloc(16, 7).toString("base64");
  const r2 = await post("/v1/resolve", { provider: { kind: "bitwarden_sm", token: wrongKey, host: bw.base }, refs: [`bws://${SECRET_ID}`] });
  assert.equal(r2.body.results[0].error, "denied");
  const c = await post("/v1/check", { provider: { kind: "bitwarden_sm", token: "not-a-token", host: bw.base } });
  assert.equal(c.body.error, "bad_config");
  for (const text of [JSON.stringify(r.body), JSON.stringify(r2.body), JSON.stringify(c.body), logs.join("\n")]) {
    assert.ok(!text.includes(wrongSecret) && !text.includes("not-a-token") && !text.includes(ACCESS_TOKEN), text);
  }
});

test("service account through the SDK adapter", async () => {
  const provider = { kind: "onepassword_service_account", token: SA_TOKEN };
  const r = await post("/v1/resolve", { provider, refs: ["op://Ops/Square/password", "op://Ops/Gone/password"] });
  assert.deepEqual(r.body.results[0], { ref: "op://Ops/Square/password", ok: true, value: "sa-canary-value" });
  assert.equal(r.body.results[1].error, "not_found");
  assert.deepEqual((await post("/v1/check", { provider })).body, { ok: true, detail: "2 vaults visible" });
  // the fake SDK puts the token in its error text, as a careless SDK might: it must not come back
  const wrong = "ops_" + "d3JvbmctdG9rZW4".repeat(4);
  const bad = await post("/v1/check", { provider: { kind: "onepassword_service_account", token: wrong } });
  assert.equal(bad.body.error, "denied");
  assert.ok(!JSON.stringify(bad.body).includes(wrong));
  const notOps = await post("/v1/check", { provider: { kind: "onepassword_service_account", token: "eyJhbGciOi" } });
  assert.equal(notOps.body.error, "bad_config");
});

test("request-level errors", async () => {
  assert.equal((await post("/v1/resolve", "not json")).status, 400);
  assert.equal((await post("/v1/resolve", { provider: { kind: "vault", token: "x" }, refs: ["op://a/b/c"] })).status, 400);
  assert.equal((await post("/v1/resolve", { provider: { kind: "onepassword_connect", token: "x" }, refs: ["op://a/b/c"] })).status, 400, "Connect needs a host");
  assert.equal((await post("/v1/resolve", { provider: op(), refs: [] })).status, 400);
  assert.equal((await post("/v1/resolve", { provider: op(), refs: Array(17).fill("op://a/b/c") })).status, 400);
  assert.equal((await post("/v1/resolve", { provider: op(), refs: ["x".repeat(70_000)] })).status, 413);
  assert.equal((await fetch(base + "/v1/other", { method: "POST" })).status, 404);
});

test("a provider that does not answer in time is a provider_error, not a hang", async () => {
  const { createServer } = await import("node:http");
  const slow = createServer(() => {}); // never answers
  await new Promise<void>((r) => slow.listen(0, "127.0.0.1", r));
  const t = Date.now();
  const r = await post("/v1/resolve", { provider: { kind: "onepassword_connect", token: CONNECT_TOKEN, host: `http://127.0.0.1:${(slow.address() as AddressInfo).port}` }, refs: ["op://Ops/Square/access_token"] });
  assert.equal(r.body.results[0].error, "provider_error");
  assert.ok(Date.now() - t < 5000);
  slow.closeAllConnections();
  await new Promise<void>((r) => slow.close(() => r()));
});

test("logs carry routes and statuses, never a body, token or value", () => {
  const all = logs.join("\n");
  for (const s of [CONNECT_TOKEN, VALUES.access, VALUES.secret, VALUE, ACCESS_TOKEN, SA_TOKEN, "sa-canary-value"]) assert.ok(!all.includes(s), s);
  assert.match(all, /POST \/v1\/resolve 200/);
});
