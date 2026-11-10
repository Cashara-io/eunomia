import { test, expect, request as pwRequest, type APIRequestContext } from "@playwright/test";
import http from "node:http";
import type { AddressInfo } from "node:net";
import { uniqueEmail, MOCK_BIND, mockBase } from "./helpers";

// Vault segregation, attacked over the real API/MCP with several users:
//   alice   - owns her personal vault and the "Shared" vault
//   bob     - accepted member of Shared (and of alice's + carol's personal vaults)
//   pat     - has PENDING invitations to Shared and alice's personal vault
//   mallory - an outsider who only knows ids
// Each test is one attack; every one must come back empty or refused.

// Tool results are arbitrary JSON that the tests index into freely.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Json = any;

type User = {
  email: string;
  ctx: APIRequestContext;
  call: (name: string, args?: object) => Promise<{ isError: boolean; data: Json }>;
  ok: (name: string, args?: object) => Promise<Json>;
  denied: (name: string, args?: object) => Promise<Json>;
};

const PASSWORD = "correct-horse-battery-staple";
const ALICE_SECRET = "ALICESECRET kiwi";
const SHARED_FACT = "SHAREDFACT mango";
const BOB_SECRET = "BOBSECRET papaya";

let rpcId = 0;
async function newUser(baseURL: string, prefix: string): Promise<User> {
  const ctx = await pwRequest.newContext({ baseURL });
  const email = uniqueEmail(prefix);
  expect((await ctx.post("/api/auth/register", { data: { email, password: PASSWORD } })).ok()).toBeTruthy();
  const token = (await (await ctx.post("/api/auth/tokens", { data: { name: prefix, vault_id: null, scopes: ["memory:read", "memory:write", "vaults:admin", "connectors"] } })).json()).token as string;
  const call = async (name: string, args: object = {}): Promise<{ isError: boolean; data: Json }> => {
    for (let attempt = 0; ; attempt++) {
      const res = await ctx.post("/mcp", {
        headers: { Authorization: `Bearer ${token}`, Accept: "application/json, text/event-stream" },
        data: { jsonrpc: "2.0", id: ++rpcId, method: "tools/call", params: { name, arguments: args } },
      });
      expect(res.status(), name).toBe(200);
      const body = await res.json();
      expect(body.error, `${name}: protocol error`).toBeUndefined();
      const data = JSON.parse(body.result.content[0].text);
      // SurrealDB's optimistic transactions can clash with background sync work
      if (attempt < 3 && String(data?.error ?? "").includes("can be retried")) continue;
      return { isError: body.result.isError as boolean, data };
    }
  };
  const ok = async (name: string, args: object = {}) => {
    const r = await call(name, args);
    expect(r.isError, `${name} failed: ${JSON.stringify(r.data)}`).toBe(false);
    return r.data;
  };
  const denied = async (name: string, args: object = {}) => {
    const r = await call(name, args);
    expect(r.isError, `${name} should have been refused but returned ${JSON.stringify(r.data)}`).toBe(true);
    return r.data;
  };
  return { email, ctx, call, ok, denied };
}

const text = (v: unknown) => JSON.stringify(v);
const enc = encodeURIComponent;

// A stand-in OpenAI-compatible endpoint that records every prompt it is sent.
let llm: http.Server;
const llmPrompts: string[] = [];

let alice: User, bob: User, carol: User, pat: User, mallory: User;
const ids: Record<string, string> = {};

test.describe.serial("vault isolation", () => {
  test.beforeAll(async ({}, testInfo) => {
    test.setTimeout(120_000);
    const baseURL = testInfo.project.use.baseURL as string;
    llm = http.createServer((req, res) => {
      let body = "";
      req.on("data", (c) => (body += c));
      req.on("end", () => {
        llmPrompts.push(body);
        res.setHeader("Content-Type", "application/json");
        if (req.url?.endsWith("/embeddings")) {
          const n = (JSON.parse(body).input as string[]).length;
          res.end(JSON.stringify({ data: Array.from({ length: n }, (_, i) => ({ index: i, embedding: Array(1536).fill(0.01) })) }));
        } else {
          res.end(JSON.stringify({ choices: [{ message: { content: JSON.stringify({ belief: "stolen" }) } }] }));
        }
      });
    });
    await new Promise<void>((r) => llm.listen(0, MOCK_BIND, r));

    [alice, bob, carol, pat, mallory] = await Promise.all(
      ["alice", "bob", "carol", "pat", "mallory"].map((p) => newUser(baseURL, `iso-${p}`)),
    );
    const personalOf = async (u: User) =>
      (await u.ok("vault_list")).results.find((v: Json) => v.kind === "personal" && v.role === "owner").id as string;
    ids.alicePersonal = await personalOf(alice);
    ids.bobPersonal = await personalOf(bob);
    ids.carolPersonal = await personalOf(carol);
    ids.malloryPersonal = await personalOf(mallory);

    // alice's private fact, and a same-named entity in bob's personal vault
    const zed = await alice.ok("memory_write", { subject_name: "Zed Secretperson", subject_kind: "person", text: ALICE_SECRET });
    ids.aliceZed = zed.entity.id;
    ids.aliceZedMemory = zed.memory.id;
    const bobZed = await bob.ok("memory_write", { subject_name: "Zed Secretperson", subject_kind: "person", text: BOB_SECRET });
    ids.bobZed = bobZed.entity.id;

    // the shared vault, bob accepted, pat only invited
    ids.shared = (await alice.ok("vault_create", { name: "Shared" })).id;
    const quinn = await alice.ok("memory_write", { subject_name: "Quinn Teammate", subject_kind: "person", text: SHARED_FACT, vault_id: ids.shared });
    ids.quinn = quinn.entity.id;
    ids.quinnMemory = quinn.memory.id;
    await alice.ok("vault_invite", { vault_id: ids.shared, email: bob.email });
    expect((await bob.ctx.post(`/api/vaults/${enc(ids.shared)}/invitations/accept`)).ok()).toBeTruthy();
    await alice.ok("vault_invite", { vault_id: ids.shared, email: pat.email });
    await alice.ok("vault_invite", { vault_id: ids.alicePersonal, email: pat.email });

    // bob also joins two other people's personal vaults, so "his personal vault"
    // is ambiguous by kind alone
    await alice.ok("vault_invite", { vault_id: ids.alicePersonal, email: bob.email });
    expect((await bob.ctx.post(`/api/vaults/${enc(ids.alicePersonal)}/invitations/accept`)).ok()).toBeTruthy();
    await carol.ok("vault_invite", { vault_id: ids.carolPersonal, email: bob.email });
    expect((await bob.ctx.post(`/api/vaults/${enc(ids.carolPersonal)}/invitations/accept`)).ok()).toBeTruthy();

    // synced records for alice and bob (same demo ids, different owners)
    for (const u of [alice, bob]) expect((await u.ctx.post("/api/sources/demo/sync")).ok()).toBeTruthy();
  });

  test.afterAll(async () => {
    llm?.close();
    await Promise.all([alice, bob, carol, pat, mallory].filter(Boolean).map((u) => u.ctx.dispose()));
  });

  // -- default scope ---------------------------------------------------------

  test("a member of other people's personal vaults still defaults to their own", async () => {
    for (let i = 0; i < 3; i++) {
      const w = await bob.ok("memory_write", { subject_name: `Default Probe ${i}`, subject_kind: "person", text: "probe" });
      expect(w.memory.vault).toBe(ids.bobPersonal);
    }
    const found = await bob.ok("recall", { query: "Zed Secretperson kiwi papaya" });
    expect(text(found)).not.toContain("ALICESECRET");
    expect(text(found)).toContain("BOBSECRET");
    expect(text(await bob.ok("entities_search", { query: "Zed" }))).toContain(ids.bobZed);
    expect(text(await bob.ok("entities_search", { query: "Zed" }))).not.toContain(ids.aliceZed);
  });

  test("recall without a vault covers the user's org vaults, labelled; a named vault narrows it", async () => {
    const all = await alice.ok("recall", { query: "Quinn Teammate mango" });
    const shared = all.results.find((i: Json) => i.text.includes("SHAREDFACT"));
    expect(shared, "org-vault facts are found without vault_id").toBeTruthy();
    expect(shared.vault).toBe(ids.shared);
    expect(shared.vault_name).toBe("Shared");
    expect(Math.max(...all.results.map((i: Json) => i.score))).toBe(1);
    expect(text(await alice.ok("recall", { query: "Quinn Teammate mango", vault_id: ids.alicePersonal }))).not.toContain("SHAREDFACT");
    expect(text(await alice.ok("entities_search", { query: "Quinn" }))).toContain(ids.quinn);
  });

  // -- cross-vault reads -----------------------------------------------------

  test("recall/reflect over the shared vault never return personal memories", async () => {
    const all = ["2000-01-01T00:00:00Z", "2100-01-01T00:00:00Z"];
    for (const u of [alice, bob]) {
      const r = await u.ok("recall", { query: "Zed Secretperson kiwi papaya Quinn mango", vault_id: ids.shared, time_range: all });
      expect(text(r)).toContain("SHAREDFACT");
      expect(text(r)).not.toContain("ALICESECRET");
      expect(text(r)).not.toContain("BOBSECRET");
      const byName = await u.ok("recall", { query: "kiwi papaya", vault_id: "Shared" });
      expect(text(byName)).not.toContain("SECRET");
      expect(text(await u.ok("reflect", { query: "What about Zed Secretperson? kiwi", vault_id: ids.shared }))).not.toContain("SECRET");
    }
  });

  test("entity search/graph over the shared vault list only its entities", async () => {
    const search = await bob.ok("entities_search", { query: "", vault_id: ids.shared });
    expect(search.results.map((e: Json) => e.id)).toEqual([ids.quinn]);
    const graph = await bob.ok("entities_graph", { vault_id: ids.shared });
    expect(graph.nodes.map((n: Json) => n.id)).toEqual([ids.quinn]);
  });

  test("the vector cloud of the shared vault has no personal memories or synced records", async () => {
    const res = await bob.ctx.get(`/api/entities/cloud?vault_ids=${enc(ids.shared)}`);
    expect(res.ok()).toBeTruthy();
    const cloud = await res.json();
    expect(cloud.points.every((p: Json) => p.vault === ids.shared && p.kind !== "record")).toBe(true);
    expect(text(cloud)).not.toContain("SECRET");
  });

  // -- cross-user, by id -----------------------------------------------------

  test("an outsider can't read another user's entity or memory by id", async () => {
    expect((await mallory.call("entities_get", { id: ids.aliceZed })).data.error).toBeTruthy();
    expect((await mallory.ctx.get(`/api/entities/${enc(ids.aliceZed)}`)).status()).toBe(404);
  });

  test("an outsider can't edit, delete or merge another user's entities and memories", async () => {
    await mallory.denied("memory_update", { memory_id: ids.aliceZedMemory, text: "pwned" });
    expect((await mallory.ok("memory_delete", { memory_id: ids.aliceZedMemory })).deleted).toBe(false);
    await mallory.denied("entity_update", { entity_id: ids.aliceZed, name: "pwned" });
    expect((await mallory.ok("entity_delete", { entity_id: ids.aliceZed })).deleted).toBe(false);
    const mine = (await mallory.ok("memory_write", { subject_name: "Mallory Thing", subject_kind: "person", text: "x" })).entity.id;
    await mallory.denied("entity_merge", { winner_id: mine, loser_id: ids.aliceZed });
    await mallory.denied("entity_merge", { winner_id: ids.aliceZed, loser_id: mine });
    await mallory.denied("code_relate", { from_id: mine, to_id: ids.aliceZed, label: "knows" });
    expect((await mallory.ctx.patch(`/api/entities/memory/${enc(ids.aliceZedMemory)}`, { data: { text: "pwned" } })).status()).toBe(404);
    expect((await mallory.ctx.post(`/api/entities/${enc(ids.aliceZed)}/memory`, { data: { text: "pwned" } })).status()).toBe(404);
    expect((await mallory.ctx.post(`/api/entities/${enc(mine)}/relations`, { data: { to_id: ids.aliceZed, label: "x" } })).status()).toBe(404);
    expect((await mallory.ctx.post(`/api/entities/${enc(mine)}/merge`, { data: { loser_id: ids.aliceZed } })).ok()).toBe(false);
    expect((await mallory.ctx.delete(`/api/entities/${enc(ids.aliceZed)}`)).status()).toBe(404);

    const zed = await alice.ok("entities_get", { id: ids.aliceZed });
    expect(zed.name).toBe("Zed Secretperson");
    expect(zed.memory.map((m: Json) => m.text)).toEqual([ALICE_SECRET]);
    expect(zed.relations).toEqual([]);
  });

  test("an outsider can't use someone else's vault by id or by name", async () => {
    for (const vault_id of [ids.shared, ids.alicePersonal]) {
      await mallory.denied("recall", { query: "kiwi mango", vault_id });
      await mallory.denied("reflect", { query: "kiwi mango", vault_id });
      await mallory.denied("entities_search", { query: "", vault_id });
      await mallory.denied("entities_graph", { vault_id });
      await mallory.denied("memory_write", { subject_name: "X", subject_kind: "person", text: "x", vault_id });
      await mallory.denied("code_entity_upsert", { kind: "repository", name: "x", vault_id });
      await mallory.denied("vault_members", { vault_id });
      await mallory.denied("vault_clone", { vault_id });
      await mallory.denied("vault_merge", { vault_id_a: vault_id, vault_id_b: ids.malloryPersonal });
      await mallory.denied("vault_merge", { vault_id_a: ids.malloryPersonal, vault_id_b: vault_id });
      await mallory.denied("vault_rename", { vault_id, name: "pwned" });
      await mallory.denied("vault_invite", { vault_id, email: mallory.email });
      await mallory.denied("vault_remove_member", { vault_id, email: alice.email });
      await mallory.denied("vault_delete", { vault_id });
      expect((await mallory.ctx.get(`/api/entities/cloud?vault_ids=${enc(vault_id)}`)).status()).toBe(403);
      expect((await mallory.ctx.get(`/api/entities?vault_id=${enc(vault_id)}`)).status()).toBe(403);
      expect((await mallory.ctx.get(`/api/entities/graph?vault_id=${enc(vault_id)}`)).status()).toBe(403);
    }
    expect((await mallory.call("recall", { query: "mango", vault_id: "Shared" })).data.error).toContain("no vault named");
    expect((await alice.ok("vault_members", { vault_id: ids.shared })).results.map((m: Json) => m.email)).not.toContain(mallory.email);
  });

  test("an outsider can't hijack consolidation to send someone else's facts to their own model", async () => {
    expect((await mallory.ctx.patch("/api/settings", { data: { openai_base_url: mockBase((llm.address() as AddressInfo).port) } })).ok()).toBeTruthy();
    const out = await mallory.ok("consolidate_observations", { subject_id: ids.aliceZed });
    expect(out.consolidated ?? []).toEqual([]);
    expect(llmPrompts.join("\n")).not.toContain("ALICESECRET");
    const zed = await alice.ok("entities_get", { id: ids.aliceZed });
    expect(zed.memory.filter((m: Json) => m.type === "observation")).toEqual([]);
  });

  test("a plain member can't administer the vault", async () => {
    await bob.denied("vault_rename", { vault_id: ids.shared, name: "pwned" });
    await bob.denied("vault_invite", { vault_id: ids.shared, email: mallory.email });
    await bob.denied("vault_remove_member", { vault_id: ids.shared, email: alice.email });
    await bob.denied("vault_delete", { vault_id: ids.shared });
    expect((await alice.ok("vault_list")).results.find((v: Json) => v.id === ids.shared).name).toBe("Shared");
  });

  // -- pending invitations ---------------------------------------------------

  test("a pending invitee can't read or write the vault", async () => {
    const mine = (await pat.ok("vault_list")).results.map((v: Json) => v.id);
    expect(mine).not.toContain(ids.shared);
    expect(mine).not.toContain(ids.alicePersonal);
    for (const vault_id of [ids.shared, ids.alicePersonal]) {
      await pat.denied("recall", { query: "kiwi mango", vault_id });
      await pat.denied("entities_search", { query: "", vault_id });
      await pat.denied("memory_write", { subject_name: "X", subject_kind: "person", text: "x", vault_id });
      await pat.denied("vault_members", { vault_id });
      await pat.denied("vault_clone", { vault_id });
      expect((await pat.ctx.get(`/api/entities/cloud?vault_ids=${enc(vault_id)}`)).status()).toBe(403);
    }
    expect((await pat.call("recall", { query: "mango", vault_id: "Shared" })).data.error).toContain("no vault named");
    expect((await pat.call("entities_get", { id: ids.quinn })).data.error).toBeTruthy();
    await pat.denied("memory_update", { memory_id: ids.quinnMemory, text: "pwned" });
  });

  test("a pending invitee's export and default recall stay their own", async () => {
    const exported = await (await pat.ctx.get("/api/export")).text();
    expect(exported).not.toContain("ALICESECRET");
    expect(exported).not.toContain("Zed Secretperson");
    expect(text(await pat.ok("recall", { query: "Zed Secretperson kiwi" }))).not.toContain("ALICESECRET");
  });

  test("export holds only the caller's own personal vault", async () => {
    const exported = await (await bob.ctx.get("/api/export")).text();
    expect(exported).toContain("BOBSECRET");
    expect(exported).not.toContain("ALICESECRET");
    expect(exported).not.toContain("SHAREDFACT");
  });

  // -- writes landing in the wrong vault -------------------------------------

  test("memory_write into a vault never attaches to a same-named entity elsewhere", async () => {
    const w = await bob.ok("memory_write", { subject_name: "Zed Secretperson", subject_kind: "person", text: "shared note", vault_id: ids.shared });
    expect(w.memory.vault).toBe(ids.shared);
    expect([ids.aliceZed, ids.bobZed]).not.toContain(w.entity.id);
    ids.sharedZed = w.entity.id;
    expect((await alice.ok("entities_get", { id: ids.aliceZed })).memory.length).toBe(1);
  });

  test("relations and merges can't bridge two vaults", async () => {
    // bob belongs to both vaults, which used to be enough
    await bob.denied("code_relate", { from_id: ids.sharedZed, to_id: ids.bobZed, label: "same_as" });
    await bob.denied("entity_merge", { winner_id: ids.sharedZed, loser_id: ids.bobZed });
    const repo = await bob.ok("code_entity_upsert", { kind: "repository", name: "private-repo" });
    await bob.denied("code_entity_upsert", { kind: "file", name: "leak.rs", parent_id: repo.id, vault_id: ids.shared });
    expect((await bob.ctx.post(`/api/entities/${enc(ids.sharedZed)}/relations`, { data: { to_id: ids.bobZed, label: "x" } })).ok()).toBe(false);
    expect((await bob.ctx.post(`/api/entities/${enc(ids.sharedZed)}/merge`, { data: { loser_id: ids.bobZed } })).ok()).toBe(false);
    await alice.denied("code_relate", { from_id: ids.quinn, to_id: ids.aliceZed, label: "knows" });

    const seen = await alice.ok("entities_get", { id: ids.sharedZed });
    expect(seen.relations).toEqual([]);
    expect(text(seen)).not.toContain(ids.bobZed);
    expect((await bob.ok("entities_get", { id: ids.bobZed })).memory.map((m: Json) => m.text)).toEqual([BOB_SECRET]);
  });

  test("a memory id is not an entity id", async () => {
    expect((await bob.call("entities_get", { id: ids.quinnMemory })).data.error).toBeTruthy();
    // foundation refuses outright (main answered deleted:false); either way the memory survives
    await bob.denied("entity_delete", { entity_id: ids.quinnMemory });
    expect((await alice.ok("entities_get", { id: ids.quinn })).memory.length).toBe(1);
  });

  // -- synced records (owner-scoped) -----------------------------------------

  test("synced records stay with their owner, never with a vault", async () => {
    const bobRecords = (await bob.ok("list", { limit: 200 })).results;
    expect(bobRecords.length).toBeGreaterThan(0);
    const rec = bobRecords[0];
    // bob cites his own record in a shared-vault memory
    await bob.ok("memory_write", { subject_name: "Quinn Teammate", subject_kind: "person", text: "met about the record", source_record_id: rec.id, vault_id: ids.shared });
    for (const u of [alice, bob]) {
      const r = await u.ok("recall", { query: `Quinn Teammate ${rec.title}`, vault_id: ids.shared });
      expect(r.results.filter((i: Json) => i.kind === "cache_record"), "records in a shared-vault recall").toEqual([]);
    }
    // the list tool's filters can't be bent into an unscoped query
    const injected = await alice.call("list", { filters: { "source OR true OR source": "x" } });
    expect(injected.data.error).toBeTruthy();
    expect((await carol.ok("list", { limit: 200 })).results).toEqual([]);
    expect((await carol.ok("search", { query: rec.title })).results).toEqual([]);
    expect((await carol.call("get", { id: rec.id })).data.error).toBe("not found");
    expect((await carol.ok("links", { id: rec.id })).links).toEqual([]);
  });

  test("the audit log only shows the caller's own calls", async () => {
    const audit = await (await alice.ctx.get("/api/audit?limit=500")).json();
    expect(audit.results.length).toBeGreaterThan(0);
    expect(text(audit)).not.toContain("BOBSECRET");
    expect(text(audit)).not.toContain("pwned");
  });

  // -- losing access ---------------------------------------------------------

  test("a removed member loses access immediately", async () => {
    await alice.ok("vault_remove_member", { vault_id: ids.shared, email: bob.email });
    await bob.denied("recall", { query: "mango", vault_id: ids.shared });
    await bob.denied("memory_write", { subject_name: "X", subject_kind: "person", text: "x", vault_id: ids.shared });
    await bob.denied("memory_update", { memory_id: ids.quinnMemory, text: "pwned" });
    expect((await bob.ok("memory_delete", { memory_id: ids.quinnMemory })).deleted).toBe(false);
    expect((await bob.call("entities_get", { id: ids.quinn })).data.error).toBeTruthy();
    expect((await bob.call("recall", { query: "mango", vault_id: "Shared" })).data.error).toContain("no vault named");
    expect((await bob.ctx.get(`/api/entities/cloud?vault_ids=${enc(ids.shared)}`)).status()).toBe(403);
  });

  test("a member who leaves loses access immediately", async () => {
    await alice.ok("vault_invite", { vault_id: ids.shared, email: carol.email });
    expect((await carol.ctx.post(`/api/vaults/${enc(ids.shared)}/invitations/accept`)).ok()).toBeTruthy();
    expect(text(await carol.ok("recall", { query: "Quinn mango", vault_id: ids.shared }))).toContain("SHAREDFACT");
    await carol.ok("vault_leave", { vault_id: ids.shared });
    await carol.denied("recall", { query: "mango", vault_id: ids.shared });
    expect((await carol.call("entities_get", { id: ids.quinn })).data.error).toBeTruthy();
  });
});
