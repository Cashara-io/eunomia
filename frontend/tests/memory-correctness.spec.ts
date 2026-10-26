import { test, expect, request as pwRequest, type APIRequestContext } from "@playwright/test";
import http from "node:http";
import type { AddressInfo } from "node:net";
import { uniqueEmail, MOCK_BIND, mockBase } from "./helpers";

// Memory correctness over the real API/MCP against a live SurrealDB:
// tombstoned records stay gone (#64), observations follow their evidence
// (#65), recall inputs are bounded and a hanging provider can't stall it
// (#68), search/list filters and paging are exact (#69), and merges and
// concurrent writes keep one entity / one observation (#72).
//
// Fixtures the public API can't produce (records from several sources, a
// tombstone, a link, an injected failure) are set up with direct SurrealDB
// queries; everything asserted goes through the app. Org databases are not
// reachable over HTTP on an in-memory backend, so those tests run only when
// E2E_SURREAL_HTTP names a SurrealDB endpoint and E2E_SURREAL_NS / E2E_SURREAL_DB
// the org database (org_<uuid>) holding the new user; otherwise they skip.
// backend/tests/memory_correctness.rs covers the same scenarios without a server.

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Json = any;

const DB_URL = process.env.E2E_SURREAL_HTTP;
const DB_NS = process.env.E2E_SURREAL_NS ?? "eunomia";
const DB_DB = process.env.E2E_SURREAL_DB ?? "eunomia";
const DB_AUTH = Buffer.from(`${process.env.E2E_SURREAL_USER ?? "root"}:${process.env.E2E_SURREAL_PASS ?? "root"}`).toString("base64");
const NO_SQL = "needs direct SurrealDB access (E2E_SURREAL_HTTP, E2E_SURREAL_NS, E2E_SURREAL_DB)";

async function sql(query: string, attempt = 0): Promise<Json[]> {
  if (!DB_URL) throw new Error(NO_SQL);
  const res = await fetch(`${DB_URL}/sql`, {
    method: "POST",
    headers: {
      Accept: "application/json",
      Authorization: `Basic ${DB_AUTH}`,
      "surreal-ns": DB_NS,
      "surreal-db": DB_DB,
    },
    body: query,
  });
  const out: Json = await res.json();
  if (!Array.isArray(out)) throw new Error(`${query}\n-> ${JSON.stringify(out)}`);
  // optimistic transactions can clash with other tests' writes
  if (attempt < 5 && out.some((r: Json) => String(r.result).includes("can be retried"))) return sql(query, attempt + 1);
  for (const r of out) if (r.status !== "OK") throw new Error(`${query}\n-> ${JSON.stringify(r.result)}`);
  return out.map((r) => r.result);
}

let rpcId = 0;
let ctx: APIRequestContext;
let token = "";
let userId = ""; // "user:xyz"
let ownerKey = ""; // "xyz"
let llmMode = false; // the server runs its own consolidation (EMBEDDINGS_BACKEND=openai)

async function call(name: string, args: object = {}): Promise<{ isError: boolean; data: Json }> {
  for (let attempt = 0; ; attempt++) {
    const res = await ctx.post("/mcp", {
      headers: { Authorization: `Bearer ${token}`, Accept: "application/json, text/event-stream" },
      data: { jsonrpc: "2.0", id: ++rpcId, method: "tools/call", params: { name, arguments: args } },
    });
    expect(res.status(), name).toBe(200);
    const body = await res.json();
    expect(body.error, `${name}: protocol error`).toBeUndefined();
    const data = JSON.parse(body.result.content[0].text);
    if (attempt < 3 && String(data?.error ?? "").includes("can be retried")) continue;
    return { isError: body.result.isError as boolean, data };
  }
}
async function ok(name: string, args: object = {}) {
  const r = await call(name, args);
  expect(r.isError, `${name} failed: ${JSON.stringify(r.data)}`).toBe(false);
  return r.data;
}
async function refused(name: string, args: object = {}) {
  const r = await call(name, args);
  expect(r.isError, `${name} should fail but returned ${JSON.stringify(r.data)}`).toBe(true);
  return r.data;
}

const rec = (id: string) => `cache_record:⟨${ownerKey}:${id}⟩`;
/** A synced record for the test user, written straight to the cache. */
function recordSql(id: string, f: { source: string; type: string; title: string; body: string; occurred_at?: string; payload?: object }) {
  const occurred = f.occurred_at ? `d"${f.occurred_at}"` : "NONE";
  return `CREATE ${rec(id)} SET owner = ${userId}, source = ${JSON.stringify(f.source)}, type = ${JSON.stringify(f.type)}, \
external_id = ${JSON.stringify(id)}, title = ${JSON.stringify(f.title)}, body_text = ${JSON.stringify(f.body)}, \
occurred_at = ${occurred}, payload = ${JSON.stringify(f.payload ?? {})}, content_hash = "fx", ingested_at = time::now(), \
updated_at = time::now(), deleted = false;`;
}

/** Statements as one transaction, so a retried conflict re-runs all or nothing. */
const tx = (statements: string[]) => `BEGIN TRANSACTION;\n${statements.join("\n")}\nCOMMIT TRANSACTION;`;

// A stand-in OpenAI-compatible endpoint: consolidation returns the current
// belief plus the new facts verbatim, so tests can see exactly what a rebuild
// was built from. Embeddings can be made to hang.
let llm: http.Server;
let hangEmbeddings = false;
function startLlm() {
  llm = http.createServer((req, res) => {
    let body = "";
    req.on("data", (c) => (body += c));
    req.on("end", () => {
      res.setHeader("Content-Type", "application/json");
      if (req.url?.endsWith("/models")) {
        // chat-model discovery: nothing listed, so the default model is used
        res.statusCode = 404;
        return res.end("{}");
      }
      if (req.url?.endsWith("/embeddings")) {
        if (hangEmbeddings) return; // never answers
        const n = (JSON.parse(body).input as string[]).length;
        res.end(JSON.stringify({ data: Array.from({ length: n }, (_, i) => ({ index: i, embedding: Array(1536).fill(0.01) })) }));
        return;
      }
      const prompt: string = JSON.parse(body).messages[0].content;
      if (!prompt.includes('{"belief": str}')) {
        // record-extraction prompts: extract nothing
        res.end(JSON.stringify({ choices: [{ message: { content: JSON.stringify({ entities: [], facts: [], relations: [] }) } }] }));
        return;
      }
      const current = /Current belief: (.*)\n/.exec(prompt)?.[1] ?? "(none yet)";
      const facts = prompt.split("New raw facts:\n")[1].split("\n").filter((l) => l.startsWith("- ")).map((l) => l.slice(2));
      const belief = [...(current === "(none yet)" ? [] : [current]), ...facts].join("; ");
      res.end(JSON.stringify({ choices: [{ message: { content: JSON.stringify({ belief }) } }] }));
    });
  });
  return new Promise<void>((r) => llm.listen(0, MOCK_BIND, r));
}

const observationOf = (entity: Json) => entity.memory.filter((m: Json) => m.type === "observation");

test.describe.serial("memory correctness", () => {
  test.beforeAll(async ({}, testInfo) => {
    test.setTimeout(120_000);
    ctx = await pwRequest.newContext({ baseURL: testInfo.project.use.baseURL as string });
    const email = uniqueEmail("memfix");
    expect((await ctx.post("/api/auth/register", { data: { email, password: "correct-horse-battery-staple" } })).ok()).toBeTruthy();
    token = (await (await ctx.post("/api/auth/tokens", { data: { name: "memfix" } })).json()).token;
    userId = (await (await ctx.get("/api/auth/me")).json()).id;
    ownerKey = userId.slice("user:".length).replace(/^⟨|⟩$/g, "");
    await startLlm();
    const base = mockBase((llm.address() as AddressInfo).port);
    expect((await ctx.patch("/api/settings", { data: { openai_base_url: base } })).ok()).toBeTruthy();
    expect((await ctx.post("/api/sources/demo/sync")).ok()).toBeTruthy();
    const probe = await ok("memory_write", { subject_name: "Probe Person", subject_kind: "person", text: "Probe exists" });
    llmMode = !(await ok("consolidate_observations", { subject_id: probe.entity.id })).note;
  });

  test.afterAll(async () => {
    llm?.close();
    await ctx?.dispose();
  });

  // -- #64 tombstones --------------------------------------------------------

  test("a tombstoned record is gone from get (REST and MCP), links and recall; others stay", async () => {
    test.skip(!DB_URL, NO_SQL);
    const page = await ok("list", { type: "up.transaction", limit: 3 });
    const [gone, neighbour, other] = page.results.map((r: Json) => r.id as string);
    const goneBody = (await ok("get", { id: gone })).body_text as string;
    await sql(`RELATE ${rec(gone)}->linked_to->${rec(neighbour)} SET rel = "fixture", origin = "sync";`);
    await ok("memory_write", { subject_name: "Tomas Tombstone", subject_kind: "person", text: "Tomas paid for lunch", source_record_id: gone });

    // before: linked, and recall follows the fact to its source record
    expect((await ok("links", { id: neighbour })).links.map((l: Json) => l.target_id)).toContain(gone);
    const before = await ok("recall", { query: "Tomas Tombstone" });
    expect(before.results.map((r: Json) => r.id)).toContain(gone);

    await sql(`UPDATE ${rec(gone)} SET deleted = true;`);

    const mcp = await call("get", { id: gone });
    expect(mcp.data.error).toBe("not found");
    const rest = await ctx.post("/api/tools/get", { data: { id: gone } });
    const restBody = await rest.text();
    expect(restBody).toContain("not found");
    expect(restBody).not.toContain(goneBody);
    expect((await ok("links", { id: neighbour })).links.map((l: Json) => l.target_id)).not.toContain(gone);
    expect((await ok("links", { id: gone })).links).toEqual([]);
    const after = await ok("recall", { query: "Tomas Tombstone" });
    expect(after.results.map((r: Json) => r.id)).not.toContain(gone);
    // (not a text check: demo data repeats merchant text across records, so
    // another, live record may legitimately carry the same body)
    const listed = await ok("list", { type: "up.transaction", limit: 200 });
    expect(listed.results.map((r: Json) => r.id)).not.toContain(gone);

    // documented rule: the derived fact outlives its source, still recallable
    expect(after.results.map((r: Json) => r.text)).toContain("Tomas paid for lunch");

    // unrelated active records are untouched
    expect((await ok("get", { id: other })).id).toBe(other);
    expect((await ok("get", { id: neighbour })).id).toBe(neighbour);
  });

  // -- #65 observations follow their evidence --------------------------------

  test("an edited fact makes the observation stale, recall leaves it out, and the rebuild reflects the edit", async () => {
    const w = await ok("memory_write", { subject_name: "Alice Mover", subject_kind: "person", text: "Alice lives in Paris" });
    const alice = w.entity.id;
    const fact = w.memory.id;
    if (llmMode) {
      expect((await ok("consolidate_observations", { subject_id: alice })).consolidated).toEqual([alice]);
    } else {
      await ok("memory_write", { subject_name: "Alice Mover", subject_kind: "person", type: "observation", text: "Alice lives in Paris" });
    }
    const [obs] = observationOf(await ok("entities_get", { id: alice }));
    expect(obs.text).toContain("Paris");
    expect(obs.status).toBe("fresh");

    await ok("memory_update", { memory_id: fact, text: "Alice lives in Berlin" });
    const stale = observationOf(await ok("entities_get", { id: alice }));
    expect(stale).toHaveLength(1);
    expect(stale[0].status).toBe("stale");
    const recalled = await ok("recall", { query: "Alice Mover Paris" });
    // the stale belief never surfaces; the background reconciler may already have rebuilt the observation (same id, now Berlin)
    expect(recalled.results.map((r: Json) => r.text)).not.toContain(obs.text);
    expect(recalled.results.map((r: Json) => r.text)).toContain("Alice lives in Berlin");

    if (!llmMode) return;
    expect((await ok("consolidate_observations", { subject_id: alice })).consolidated).toEqual([alice]);
    const [rebuilt] = observationOf(await ok("entities_get", { id: alice }));
    expect(rebuilt.text).toBe("Alice lives in Berlin");
    expect(rebuilt.status).toBe("fresh");
    expect(rebuilt.source_memories).toEqual([fact]);
    const again = await ok("recall", { query: "Alice Mover" });
    expect(again.results.map((r: Json) => r.id)).toContain(rebuilt.id);
  });

  test("deleting the only supporting fact removes the observation; lineage keeps only surviving facts", async () => {
    test.skip(!llmMode, "needs server-side consolidation (EMBEDDINGS_BACKEND=openai)");
    const only = await ok("memory_write", { subject_name: "Bea Single", subject_kind: "person", text: "Bea plays cello" });
    await ok("consolidate_observations", { subject_id: only.entity.id });
    expect(observationOf(await ok("entities_get", { id: only.entity.id }))).toHaveLength(1);
    await ok("memory_delete", { memory_id: only.memory.id });
    expect(observationOf(await ok("entities_get", { id: only.entity.id }))).toHaveLength(0);

    const f1 = await ok("memory_write", { subject_name: "Cy Pair", subject_kind: "person", text: "Cy drinks tea" });
    const f2 = await ok("memory_write", { subject_name: "Cy Pair", subject_kind: "person", text: "Cy runs marathons" });
    const cy = f1.entity.id;
    await ok("consolidate_observations", { subject_id: cy });
    expect(observationOf(await ok("entities_get", { id: cy }))[0].source_memories.sort()).toEqual([f1.memory.id, f2.memory.id].sort());

    await ok("memory_delete", { memory_id: f1.memory.id });
    const [pruned] = observationOf(await ok("entities_get", { id: cy }));
    expect(pruned.status).toBe("stale");
    expect(pruned.source_memories).toEqual([f2.memory.id]);
    expect((await ok("recall", { query: "Cy Pair tea" })).results.map((r: Json) => r.id)).not.toContain(pruned.id);

    await ok("consolidate_observations", { subject_id: cy });
    const [rebuilt] = observationOf(await ok("entities_get", { id: cy }));
    expect(rebuilt.text).toBe("Cy runs marathons");
    expect(rebuilt.source_memories).toEqual([f2.memory.id]);
  });

  // -- #68 bounded recall ----------------------------------------------------

  test("recall rejects unbounded or malformed input with a clear error", async () => {
    for (const args of [
      { query: "x", limit: 0 },
      { query: "x", limit: 101 },
      { query: "x", limit: 1e15 },
      { query: "x", max_tokens: 0 },
      { query: "x", max_tokens: 10_000_000 },
      { query: "x".repeat(2001) },
      { query: "x", time_range: ["2024-02-01", "2024-01-01"] },
      { query: "x", time_range: ["last week", "2024-01-01"] },
      { query: "x", time_range: ["2024-01-01"] },
    ]) {
      const data = await refused("recall", args);
      expect(JSON.stringify(data), JSON.stringify(args).slice(0, 80)).toMatch(/limit|max_tokens|query|since|time_range|date|invalid|integer/i);
    }
    const rest = await ctx.post("/api/tools/recall", { data: { query: "x", limit: 5000 } });
    expect(rest.status()).toBe(400);
    expect((await ok("recall", { query: "x", time_range: ["2020-01-01", "2030-01-01"], limit: 100, max_tokens: 100000 })).results).toBeDefined();
  });

  test("a hanging embedding provider does not stop keyword and graph results", async () => {
    test.skip(!llmMode, "semantic arm only calls the provider with EMBEDDINGS_BACKEND=openai");
    await ok("memory_write", { subject_name: "Hana Hangtest", subject_kind: "person", text: "Hana keeps bees" });
    hangEmbeddings = true;
    try {
      const started = Date.now();
      const r = await ok("recall", { query: `Hana Hangtest bees ${Date.now()}` });
      expect(Date.now() - started).toBeLessThan(8_000);
      expect(r.results.map((x: Json) => x.text)).toContain("Hana keeps bees");
    } finally {
      hangEmbeddings = false;
    }
  });

  test("graph recall on a large vault matches through the name index", async () => {
    test.skip(!DB_URL, NO_SQL);
    const vault = (await ok("vault_list")).results.find((v: Json) => v.kind === "personal").id;
    await sql(
      `FOR $i IN 0..2000 { CREATE person SET owner = ${userId}, vault = ${vault}, name = "Filler Person " + <string>$i; };`,
    );
    await ok("memory_write", { subject_name: "Zygmunt Needle", subject_kind: "person", text: "Zygmunt repairs clocks" });
    const started = Date.now();
    const r = await ok("recall", { query: "what does Zygmunt do?" });
    expect(Date.now() - started).toBeLessThan(3_000);
    expect(r.results.map((x: Json) => x.text)).toContain("Zygmunt repairs clocks");
  });

  // -- #69 search filters, paging, payload filters, body index ---------------

  test("search pushes filters into candidates, pages exactly and uses the body index", async () => {
    test.skip(!DB_URL, NO_SQL);
    const statements = [
      ...Array.from({ length: 60 }, (_, i) =>
        recordSql(`fx:crowd:${i}`, { source: "crowd", type: "fx.note", title: `zebrafjord crowd ${i}`, body: "zebrafjord zebrafjord" }),
      ),
      recordSql("fx:rare:1", { source: "rare", type: "fx.note", title: "an unrelated title", body: "a lone zebrafjord mention" }),
    ];
    await sql(tx(statements));

    // the rare source's single, body-only match beats 60 better-ranked crowd hits
    const rare = await ok("search", { query: "zebrafjord", sources: ["rare"], mode: "keyword", limit: 1 });
    expect(rare.results.map((r: Json) => r.id)).toEqual(["fx:rare:1"]);
    expect(rare.has_more).toBe(false);

    // three pages, no repeats, has_more exact
    const seen: string[] = [];
    const more: boolean[] = [];
    for (const offset of [0, 25, 50]) {
      const p = await ok("search", { query: "zebrafjord", sources: ["crowd"], mode: "keyword", limit: 25, offset });
      seen.push(...p.results.map((r: Json) => r.id));
      more.push(p.has_more);
    }
    expect(seen).toHaveLength(60);
    expect(new Set(seen).size).toBe(60);
    expect(more).toEqual([true, true, false]);
    await refused("search", { query: "zebrafjord", limit: 100, offset: 950 });

    // the pinned SurrealDB plans body_text matches on its own BM25 index
    const [plan] = await sql(`SELECT id FROM cache_record WHERE body_text @2@ "zebrafjord" EXPLAIN;`);
    expect(JSON.stringify(plan)).toContain("cache_record_body_fts");
  });

  test("dates compare as datetimes and payload filters target the named field", async () => {
    test.skip(!DB_URL, NO_SQL);
    await sql(
      tx([
        recordSql("fx:d:1", { source: "dated", type: "fx.dated", title: "d1", body: "", occurred_at: "2020-01-01T00:00:00Z", payload: { category: "Groceries", amount: { cents: 500 } } }),
        recordSql("fx:d:2", { source: "dated", type: "fx.dated", title: "d2", body: "", occurred_at: "2020-01-05T12:00:00Z", payload: { category: "Travel", amount: { cents: 9000 } } }),
        recordSql("fx:d:3", { source: "dated", type: "fx.dated", title: "d3", body: "", occurred_at: "2020-01-10T00:00:00Z", payload: { category: "Groceries", amount: { cents: 2500 } } }),
      ]),
    );
    const titles = async (filters: object) =>
      (await ok("list", { type: "fx.dated", filters, sort: "occurred_at" })).results.map((r: Json) => r.title);
    expect(await titles({ occurred_at__gte: "2020-01-05" })).toEqual(["d2", "d3"]);
    expect(await titles({ occurred_at__lt: "2020-01-05T12:00:00Z" })).toEqual(["d1"]);
    expect(await titles({ occurred_at__gte: "2020-01-02", occurred_at__lte: "2020-01-09" })).toEqual(["d2"]);
    expect(await titles({ payload__category: "Groceries" })).toEqual(["d1", "d3"]);
    expect(await titles({ payload__amount__cents__gt: 1000 })).toEqual(["d2", "d3"]);
    for (const filters of [{ title__contains: "d" }, { bogus: 1 }, { occurred_at__gte: "yesterday" }, { payload: { category: "x" } }]) {
      await refused("list", { type: "fx.dated", filters });
    }

    const ranged = await ok("search", { query: "d2", sources: ["dated"], since: "2020-01-05", until: "2020-01-06", mode: "keyword" });
    expect(ranged.results.map((r: Json) => r.title)).toEqual(["d2"]);
    await refused("search", { query: "d2", since: "2020-02-01", until: "2020-01-01" });
    await refused("search", { query: "d2", since: "not a date" });
  });

  // -- #72 merges and concurrent writes --------------------------------------

  test("concurrent equivalent entity and observation writes land on one row", async () => {
    const names = ["Concurrent Carl", "concurrent carl", "CONCURRENT CARL", " Concurrent Carl "];
    const writes = await Promise.all(
      Array.from({ length: 8 }, (_, i) =>
        ok("memory_write", { subject_name: names[i % names.length], subject_kind: "person", text: `Carl fact ${i}` }),
      ),
    );
    const ids = new Set(writes.map((w: Json) => w.entity.id));
    expect(ids.size).toBe(1);
    const [carl] = [...ids];
    expect((await ok("entities_get", { id: carl })).memory.filter((m: Json) => m.type !== "observation")).toHaveLength(8);

    await Promise.all(
      Array.from({ length: 6 }, (_, i) =>
        ok("memory_write", { subject_name: "Concurrent Carl", subject_kind: "person", type: "observation", text: `Carl belief ${i}` }),
      ),
    );
    expect(observationOf(await ok("entities_get", { id: carl }))).toHaveLength(1);

    if (llmMode) {
      await ok("memory_write", { subject_name: "Concurrent Carl", subject_kind: "person", text: "Carl fact late" });
      await Promise.all(Array.from({ length: 4 }, () => ok("consolidate_observations", { subject_id: carl })));
      expect(observationOf(await ok("entities_get", { id: carl }))).toHaveLength(1);
    }
  });

  test("a failing merge rolls back completely; a merge of two observed entities leaves one stale observation", async () => {
    const mk = async (name: string, fact: string) =>
      (await ok("memory_write", { subject_name: name, subject_kind: "person", text: fact })).entity.id as string;
    const winner = await mk("Mergy Winner", "Winner fact");
    const loser = await mk("Mergy Loser", "Loser fact");
    const friend = await mk("Mergy Friend", "Friend fact");
    await ok("memory_write", { subject_name: "Mergy Winner", subject_kind: "person", type: "observation", text: "Winner belief" });
    await ok("memory_write", { subject_name: "Mergy Loser", subject_kind: "person", type: "observation", text: "Loser belief" });
    const boom = `boom${Date.now()}`;
    await ok("code_relate", { from_id: winner, to_id: friend, label: "knows" });
    await ok("code_relate", { from_id: loser, to_id: friend, label: "knows" }); // a duplicate once merged
    await ok("code_relate", { from_id: loser, to_id: friend, label: boom }); // unique to the loser
    await ok("code_relate", { from_id: friend, to_id: loser, label: "mentors" });

    if (DB_URL) {
      // inject a failure into copying the loser's unique edge
      await sql(`DEFINE EVENT inject_${boom} ON relates_to WHEN $event = "CREATE" AND $after.label = "${boom}" THEN { THROW "injected edge-copy failure" };`);
      try {
        await refused("entity_merge", { winner_id: winner, loser_id: loser });
      } finally {
        await sql(`REMOVE EVENT inject_${boom} ON relates_to;`);
      }
      const loserAfter = await ok("entities_get", { id: loser });
      expect(loserAfter.name).toBe("Mergy Loser");
      expect(loserAfter.relations.map((r: Json) => r.label).sort()).toEqual([boom, "knows", "mentors"].sort());
      expect(loserAfter.memory.map((m: Json) => m.text).sort()).toEqual(["Loser belief", "Loser fact"]);
      const winnerAfter = await ok("entities_get", { id: winner });
      expect(winnerAfter.memory.map((m: Json) => m.text).sort()).toEqual(["Winner belief", "Winner fact"]);
      expect(winnerAfter.relations.map((r: Json) => r.label)).toEqual(["knows"]);
    }

    // the real merge
    await ok("entity_merge", { winner_id: winner, loser_id: loser });
    const gone = await call("entities_get", { id: loser });
    expect(gone.isError || !gone.data || gone.data.error, JSON.stringify(gone.data)).toBeTruthy();
    const merged = await ok("entities_get", { id: winner });
    expect(merged.aliases).toContain("Mergy Loser");
    const obs = observationOf(merged);
    expect(obs).toHaveLength(1);
    expect(obs[0].status).toBe("stale");
    expect(obs[0].text).toContain("Winner belief");
    expect(obs[0].text).toContain("Loser belief");
    expect(merged.memory.filter((m: Json) => m.type === "world").map((m: Json) => m.text).sort()).toEqual(["Loser fact", "Winner fact"]);
    const rels = merged.relations.map((r: Json) => `${r.direction}:${r.label}`).sort();
    expect(rels).toEqual([`out:${boom}`, "in:mentors", "out:knows"].sort());
    if (DB_URL) {
      const [[edges], [orphans]] = await sql(
        `SELECT count() FROM relates_to WITH NOINDEX WHERE in = ${loser} OR out = ${loser} GROUP ALL; \
         SELECT count() FROM memory WITH NOINDEX WHERE subject = ${loser} GROUP ALL;`,
      );
      expect(edges?.count ?? 0, "dangling edges").toBe(0);
      expect(orphans?.count ?? 0, "orphaned memories").toBe(0);
    }

    if (llmMode) {
      await ok("consolidate_observations", { subject_id: winner });
      const [rebuilt] = observationOf(await ok("entities_get", { id: winner }));
      expect(rebuilt.status).toBe("fresh");
      expect(rebuilt.text.split("; ").sort()).toEqual(["Loser fact", "Winner fact"]);
    }
  });
});
