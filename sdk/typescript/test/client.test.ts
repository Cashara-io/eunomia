import assert from "node:assert/strict";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { after, before, beforeEach, describe, it } from "node:test";
import { randomBytes } from "node:crypto";
import { Eunomia, EunomiaError } from "../src/index.js";

type Reply = [number, unknown, Record<string, string>?];
type Call = { method: string; path: string; body: any; auth?: string };

const routes = new Map<string, (body: any) => Reply>();
let calls: Call[] = [];

const server = createServer(async (req: IncomingMessage, res: ServerResponse) => {
  let raw = "";
  for await (const c of req) raw += c;
  const body = raw ? JSON.parse(raw) : undefined;
  calls.push({ method: req.method!, path: req.url!, body, auth: req.headers.authorization });
  const fn = routes.get(`${req.method} ${req.url}`);
  const [status, payload, headers] = fn ? fn(body) : [404, { code: "x.none", detail: "no route" }];
  res.writeHead(status, { "content-type": "application/json", ...headers });
  res.end(JSON.stringify(payload));
});

let eu: Eunomia;
before(async () => {
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  const port = (server.address() as { port: number }).port;
  eu = new Eunomia(`http://127.0.0.1:${port}`, { token: "tok123" });
});
after(() => server.close());
beforeEach(() => {
  routes.clear();
  calls = [];
});

describe("vaults", () => {
  it("lists with bearer auth", async () => {
    routes.set("GET /api/vaults", () => [200, { results: [{ id: "vault:1", name: "P", kind: "personal", role: "owner" }] }]);
    const v = await eu.listVaults();
    assert.equal(v[0].role, "owner");
    assert.equal(calls[0].auth, "Bearer tok123");
  });

  it("getVault finds by name and throws vault.not_found", async () => {
    routes.set("GET /api/vaults", () => [200, { results: [{ id: "vault:1", name: "P", kind: "org" }] }]);
    assert.equal((await eu.getVault("P")).id, "vault:1");
    await assert.rejects(eu.getVault("nope"), (e: EunomiaError) => e.code === "vault.not_found");
  });

  it("merge sends both ids", async () => {
    routes.set("POST /api/vaults/merge", (b) => [200, { id: "vault:3", name: b.name, kind: "org", entities: 4, merged_from: b.vault_ids }]);
    const m = await eu.mergeVaults("vault:1", "vault:2", { name: "All" });
    assert.equal(m.entities, 4);
    assert.deepEqual(calls[0].body, { vault_ids: ["vault:1", "vault:2"], kind: "org", name: "All" });
  });

  it("create, rename, clone, delete use the right routes", async () => {
    routes.set("POST /api/vaults", (b) => [200, { id: "vault:9", name: b.name, kind: b.kind }]);
    routes.set("PATCH /api/vaults/vault%3A9", (b) => [200, { id: "vault:9", name: b.name, kind: "org" }]);
    routes.set("POST /api/vaults/vault%3A9/clone", () => [200, { id: "vault:10", name: "c", kind: "org", entities_copied: 2 }]);
    routes.set("DELETE /api/vaults/vault%3A9", () => [200, { deleted: true }]);
    assert.equal((await eu.createVault("x")).id, "vault:9");
    assert.equal((await eu.renameVault("vault:9", "y")).name, "y");
    assert.equal((await eu.cloneVault("vault:9")).entities_copied, 2);
    assert.equal(await eu.deleteVault("vault:9"), undefined);
  });

  it("members, invite, remove, leave", async () => {
    routes.set("GET /api/vaults/v/members", () => [200, { results: [{ email: "a@b.c", role: "owner" }] }]);
    routes.set("POST /api/vaults/v/members", (b) => [200, { vault_id: "v", user_email: b.email, role: b.role }]);
    routes.set("DELETE /api/vaults/v/members/a%40b.c", () => [200, { removed: true }]);
    routes.set("POST /api/vaults/v/leave", () => [200, { left: true }]);
    assert.equal((await eu.listMembers("v"))[0].email, "a@b.c");
    assert.equal((await eu.inviteMember("v", "d@e.f", "editor")).role, "editor");
    await eu.removeMember("v", "a@b.c");
    await eu.leaveVault("v");
  });
});

describe("errors", () => {
  it("carries code, detail and trace id", async () => {
    routes.set("GET /api/vaults", () => [403, { code: "auth.forbidden", detail: "nope", trace_id: "abc" }]);
    await assert.rejects(eu.listVaults(), (e: EunomiaError) => {
      assert.ok(e instanceof EunomiaError);
      assert.deepEqual([e.status, e.code, e.detail, e.traceId], [403, "auth.forbidden", "nope", "abc"]);
      return true;
    });
  });

  it("reports transport failures as status 0", async () => {
    const dead = new Eunomia("http://127.0.0.1:1", { token: "t" });
    await assert.rejects(dead.listVaults(), (e: EunomiaError) => e.status === 0 && e.code === "client.transport");
  });
});

describe("memory", () => {
  it("memoryWrite, recall, entitiesSearch pass vault_id", async () => {
    routes.set("POST /api/tools/memory_write", () => [200, { entity: { id: "e" }, memory: { id: "m" } }]);
    routes.set("POST /api/tools/recall", () => [200, { results: [{ text: "t" }] }]);
    routes.set("POST /api/tools/entities_search", () => [200, { results: [{ name: "Ada" }], total: 1 }]);
    await eu.memoryWrite({ subjectName: "Ada", subjectKind: "person", text: "hi", vaultId: "V" });
    assert.deepEqual(calls[0].body, { subject_name: "Ada", subject_kind: "person", text: "hi", type: "world", vault_id: "V" });
    assert.equal((await eu.recall("q", { vaultId: "V" }))[0].text, "t");
    assert.equal((await eu.entitiesSearch("Ada", { vaultId: "V" }))[0].name, "Ada");
    assert.equal(calls[1].body.vault_id, "V");
    assert.equal(calls[2].body.vault_id, "V");
  });
});

describe("rate limiting", () => {
  it("retries a 429 after Retry-After", async () => {
    let n = 0;
    routes.set("GET /api/vaults", () => (++n === 1 ? [429, { code: "rate.limited", detail: "slow" }, { "retry-after": "0.05" }] : [200, { results: [] }]));
    assert.deepEqual(await eu.listVaults(), []);
    assert.equal(n, 2);
  });

  it("gives up after maxRetries", async () => {
    routes.set("GET /api/vaults", () => [429, { code: "rate.limited", detail: "slow" }, { "retry-after": "0" }]);
    const port = (server.address() as { port: number }).port;
    const c = new Eunomia(`http://127.0.0.1:${port}`, { token: "t", maxRetries: 2 });
    await assert.rejects(c.listVaults(), (e: EunomiaError) => e.code === "rate.limited");
    assert.equal(calls.length, 3);
  });

  it("writeMemories keeps going past failures and a 429, in input order", async () => {
    let limited = false;
    routes.set("POST /api/tools/memory_write", (b) => {
      if (b.text === "bad") return [400, { code: "request.invalid", detail: "bad" }];
      if (b.text === "f3" && !limited) {
        limited = true;
        return [429, { code: "rate.limited", detail: "slow" }, { "retry-after": "0.05" }];
      }
      return [200, { entity: {}, memory: { id: b.text } }];
    });
    const items = [0, 1, 2, 3, 4, 5].map((i) => ({ subjectName: "S", subjectKind: "person", text: `f${i}` }));
    items.splice(2, 0, { subjectName: "S", subjectKind: "person", text: "bad" });
    const out = await eu.writeMemories(items, { vaultId: "V", concurrency: 3 });
    assert.deepEqual(out.map((o) => o.index), items.map((_, i) => i));
    assert.deepEqual(out.map((o) => !o.error), [true, true, false, true, true, true, true]);
    assert.equal(out[2].error?.code, "request.invalid");
    assert.ok(limited);
    assert.ok(calls.every((c) => c.body.vault_id === "V"));
  });
});

const URL_ = process.env.EUNOMIA_URL;
const TOKEN = process.env.EUNOMIA_TOKEN;

describe("integration", { skip: !(URL_ && TOKEN) && "set EUNOMIA_URL and EUNOMIA_TOKEN to run" }, () => {
  it("create, write, merge, recall, clean up", async () => {
    const live = new Eunomia(URL_!, { token: TOKEN! });
    const tag = randomBytes(4).toString("hex");
    const made: string[] = [];
    try {
      const a = await live.createVault(`sdk-ts-a-${tag}`);
      made.push(a.id);
      const b = await live.createVault(`sdk-ts-b-${tag}`);
      made.push(b.id);
      const w = await live.writeMemories([{ subjectName: `Ada${tag}`, subjectKind: "person", text: `Ada${tag} likes green tea` }], { vaultId: a.id });
      w.push(...(await live.writeMemories([{ subjectName: `Grace${tag}`, subjectKind: "person", text: `Grace${tag} likes compilers` }], { vaultId: b.id })));
      assert.ok(w.every((o) => !o.error), JSON.stringify(w));
      const merged = await live.mergeVaults(a.id, b.id, { name: `sdk-ts-merged-${tag}` });
      made.push(merged.id);
      assert.equal(merged.entities, 2);
      assert.ok((await live.listVaults()).some((v) => v.id === merged.id));
      const hits = await live.recall("green tea", { vaultId: merged.id });
      assert.ok(hits.some((h) => h.text.includes(`Ada${tag}`)), JSON.stringify(hits));
      const names = (await live.entitiesSearch(`Grace${tag}`, { vaultId: merged.id })).map((e) => e.name);
      assert.ok(names.includes(`Grace${tag}`));
      assert.equal((await live.renameVault(merged.id, `sdk-ts-renamed-${tag}`)).name, `sdk-ts-renamed-${tag}`);
    } finally {
      for (const id of made) await live.deleteVault(id).catch(() => {});
    }
  });
});
