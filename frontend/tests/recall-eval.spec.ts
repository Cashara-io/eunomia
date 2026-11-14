import { test, expect, request as pwRequest } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";
import { uniqueEmail } from "./helpers";

// A small, fixed retrieval evaluation: seeds tests/fixtures/recall-eval.json
// through the real MCP API, runs every query through `recall`, and reports
// Recall@5 and MRR. Run retrieval experiments against this same corpus so the
// numbers stay comparable. The floors below are what the code achieved when
// they were set (lexical + graph arms only: no embeddings in CI) -- raise them
// when retrieval improves, never lower them to make a change pass.
// Measured: Recall@5 0.917, MRR 0.875 (only the "extended their agreement"
// paraphrase misses); one query losing a rank drops MRR below 0.85.
const FLOOR = { recallAt5: 0.9, mrr: 0.85 };

type Memory = { key: string; vault: "personal" | "shared" | "outsider"; subject: string; kind: string; text: string; occurred_at?: string };
type Query = { kind: string; vault: "personal" | "shared"; query: string; expected: string[]; time_range?: [string, string] };
const fixture: { memories: Memory[]; queries: Query[] } = JSON.parse(
  fs.readFileSync(path.join(__dirname, "fixtures", "recall-eval.json"), "utf8"),
);

// Tool results are arbitrary JSON.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Json = any;

let rpcId = 0;
async function newUser(baseURL: string, prefix: string) {
  const ctx = await pwRequest.newContext({ baseURL });
  const email = uniqueEmail(prefix);
  expect((await ctx.post("/api/auth/register", { data: { email, password: "correct-horse-battery-staple" } })).ok()).toBeTruthy();
  const token = (await (await ctx.post("/api/auth/tokens", { data: { name: prefix, vault_id: null, scopes: ["memory:read", "memory:write", "vaults:admin"] } })).json()).token as string;
  return async (name: string, args: object = {}): Promise<Json> => {
    for (let attempt = 0; ; attempt++) {
      const res = await ctx.post("/mcp", {
        headers: { Authorization: `Bearer ${token}`, Accept: "application/json, text/event-stream" },
        data: { jsonrpc: "2.0", id: ++rpcId, method: "tools/call", params: { name, arguments: args } },
      });
      expect(res.status(), name).toBe(200);
      const body = await res.json();
      const data = JSON.parse(body.result.content[0].text);
      // SurrealDB's optimistic transactions can clash with background work
      if (attempt < 3 && String(data?.error ?? "").includes("can be retried")) continue;
      expect(body.result.isError, `${name} failed: ${JSON.stringify(data)}`).toBe(false);
      return data;
    }
  };
}

test("recall evaluation on the fixed corpus", async ({}, testInfo) => {
  test.setTimeout(120_000);
  const baseURL = testInfo.project.use.baseURL as string;
  const owner = await newUser(baseURL, "eval-owner");
  const outsider = await newUser(baseURL, "eval-outsider");
  const shared = (await owner("vault_create", { name: "Eval Team" })).id as string;
  // Each query is scoped to its vault explicitly (no vault_id would search both).
  const personal = (await owner("vault_list")).results.find((v: Json) => v.kind === "personal").id as string;

  for (const m of fixture.memories) {
    const call = m.vault === "outsider" ? outsider : owner;
    await call("memory_write", {
      subject_name: m.subject,
      subject_kind: m.kind,
      text: m.text,
      ...(m.occurred_at ? { occurred_at: m.occurred_at } : {}),
      ...(m.vault === "shared" ? { vault_id: shared } : {}),
    });
  }
  const textOf = new Map(fixture.memories.map((m) => [m.key, m.text]));
  const vaultTexts = (v: string) => new Set(fixture.memories.filter((m) => m.vault === v).map((m) => m.text));

  const perQuery = [];
  for (const q of fixture.queries) {
    const started = Date.now();
    const { results } = await owner("recall", { query: q.query, limit: 10, vault_id: q.vault === "shared" ? shared : personal, ...(q.time_range ? { time_range: q.time_range } : {}) });
    const ms = Date.now() - started;
    const texts: string[] = results.map((r: Json) => r.text);
    const allowed = vaultTexts(q.vault);
    // a memory from another vault or another user; synced records belong only to the personal vault
    const unauthorized = results.filter((r: Json) => (r.kind === "memory" ? !allowed.has(r.text) : q.vault !== "personal"));
    const expected = q.expected.map((k) => textOf.get(k)!);
    const top5 = texts.slice(0, 5);
    const firstHit = texts.findIndex((t) => expected.includes(t));
    perQuery.push({
      ...q,
      recallAt5: expected.length ? expected.filter((t) => top5.includes(t)).length / expected.length : null,
      rr: expected.length ? (firstHit < 0 ? 0 : 1 / (firstHit + 1)) : null,
      returned: texts.length,
      unauthorized: unauthorized.map((r: Json) => r.text),
      ms,
    });
  }

  const scored = perQuery.filter((r) => r.recallAt5 !== null);
  const mean = (xs: number[]) => xs.reduce((a, b) => a + b, 0) / xs.length;
  const metrics = {
    corpus: { memories: fixture.memories.length, queries: fixture.queries.length, scoredQueries: scored.length },
    recallAt5: mean(scored.map((r) => r.recallAt5!)),
    mrr: mean(scored.map((r) => r.rr!)),
    unauthorizedResults: perQuery.reduce((n, r) => n + r.unauthorized.length, 0),
    // results for queries whose answer is not in the vault: not asserted, just tracked
    negativeQueryResults: perQuery.filter((r) => r.recallAt5 === null).reduce((n, r) => n + r.returned, 0),
    maxLatencyMs: Math.max(...perQuery.map((r) => r.ms)),
    perQuery,
  };

  const out = testInfo.outputPath("recall-eval.json");
  fs.writeFileSync(out, JSON.stringify(metrics, null, 2));
  await testInfo.attach("recall-eval.json", { path: out });
  console.log(
    `recall-eval: Recall@5=${metrics.recallAt5.toFixed(3)} MRR=${metrics.mrr.toFixed(3)} ` +
      `unauthorized=${metrics.unauthorizedResults} negativeQueryResults=${metrics.negativeQueryResults} maxLatencyMs=${metrics.maxLatencyMs}`,
  );
  for (const r of perQuery) {
    console.log(`  [${r.kind}/${r.vault}] ${JSON.stringify(r.query)} R@5=${r.recallAt5 ?? "-"} RR=${r.rr?.toFixed(2) ?? "-"} n=${r.returned}`);
  }

  expect(perQuery.filter((r) => r.unauthorized.length), "cross-vault / other-user results").toEqual([]);
  expect(metrics.recallAt5).toBeGreaterThanOrEqual(FLOOR.recallAt5);
  expect(metrics.mrr).toBeGreaterThanOrEqual(FLOOR.mrr);
});

// ---------------------------------------------------------------------------------------------
// Recall ranking (date filter, cross-vault fusion): the fixture's "ranking" block, on users of its
// own so the corpus and metrics above are unchanged. Kept as a separate block at the end on purpose.
type RankingMemory = Omit<Memory, "vault"> & { vault: "personal" | "shared"; occurred_at?: string };
type RankingQuery = { kind: string; vault: "personal" | "shared" | "all"; query: string; expected: string[]; time_range?: [string, string]; excluded?: string[] };
const ranking: { memories: RankingMemory[]; queries: RankingQuery[] } = JSON.parse(
  fs.readFileSync(path.join(__dirname, "fixtures", "recall-eval.json"), "utf8"),
).ranking;

test("recall evaluation: date ranges and cross-vault ranking", async ({}, testInfo) => {
  test.setTimeout(120_000);
  const baseURL = testInfo.project.use.baseURL as string;
  const owner = await newUser(baseURL, "eval-rank");
  const shared = (await owner("vault_create", { name: "Eval Rank Team" })).id as string;
  const personal = (await owner("vault_list")).results.find((v: Json) => v.kind === "personal").id as string;
  for (const m of ranking.memories) {
    await owner("memory_write", {
      subject_name: m.subject,
      subject_kind: m.kind,
      text: m.text,
      ...(m.occurred_at ? { occurred_at: m.occurred_at } : {}),
      ...(m.vault === "shared" ? { vault_id: shared } : {}),
    });
  }
  const textOf = new Map(ranking.memories.map((m) => [m.key, m.text]));
  const perQuery = [];
  for (const q of ranking.queries) {
    const vault = q.vault === "all" ? {} : { vault_id: q.vault === "shared" ? shared : personal };
    const { results } = await owner("recall", { query: q.query, limit: 10, ...vault, ...(q.time_range ? { time_range: q.time_range } : {}) });
    const texts: string[] = results.map((r: Json) => r.text);
    const expected = q.expected.map((k) => textOf.get(k)!);
    const top5 = texts.slice(0, 5);
    const firstHit = texts.findIndex((t) => expected.includes(t));
    perQuery.push({
      ...q,
      recallAt5: expected.length ? expected.filter((t) => top5.includes(t)).length / expected.length : null,
      rr: expected.length ? (firstHit < 0 ? 0 : 1 / (firstHit + 1)) : null,
      returned: texts.length,
      excludedReturned: (q.excluded ?? []).map((k) => textOf.get(k)!).filter((t) => texts.includes(t)),
    });
  }
  const scored = perQuery.filter((r) => r.recallAt5 !== null);
  const mean = (xs: number[]) => xs.reduce((a, b) => a + b, 0) / xs.length;
  console.log(`recall-eval ranking: Recall@5=${mean(scored.map((r) => r.recallAt5!)).toFixed(3)} MRR=${mean(scored.map((r) => r.rr!)).toFixed(3)}`);
  for (const r of perQuery) console.log(`  [${r.kind}/${r.vault}] ${JSON.stringify(r.query)} R@5=${r.recallAt5 ?? "-"} RR=${r.rr?.toFixed(2) ?? "-"} n=${r.returned}`);

  expect(perQuery.filter((r) => r.excludedReturned.length), "results outside the time range").toEqual([]);
  expect(perQuery.filter((r) => r.kind === "dated-no-match" && r.returned), "a query matching nothing in the range").toEqual([]);
  expect(perQuery.filter((r) => r.rr !== null && r.rr < 1), "the expected answer ranks first").toEqual([]);
});

