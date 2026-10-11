// Hundreds of agents over /mcp sharing a few tokens: recall, entities_search, memory_write, search and
// tools/list against an org seeded with a few thousand entities and memories. Measures throughput and
// latency under saturation (no think time), so turn the per-token and per-user limits off on the backend
// (RATE_LIMIT_TOKEN_PER_MIN=0 RATE_LIMIT_USER_PER_MIN=0): with a few shared tokens they would refuse most calls.
//   k6 run -e VUS=250 -e DURATION=60s loadtest/k6/mcp-mix.js
import http from "k6/http";
import { check, fail } from "k6";
import { Rate } from "k6/metrics";

const BASE_URL = __ENV.BASE_URL || "http://localhost:8101";
const TOKENS = parseInt(__ENV.TOKENS || "4", 10); // users, one token each, shared by every VU
const ENTITIES = parseInt(__ENV.ENTITIES || "750", 10); // per user
const MEMORIES = parseInt(__ENV.MEMORIES || "1500", 10); // per user
const OPS = ["recall", "entities_search", "memory_write", "search", "tools_list"];

const FIRST = ["ada", "bruno", "chen", "dara", "emil", "farah", "gita", "hugo", "ines", "jonas", "kofi", "lena", "milo", "nadia", "omar", "priya", "quinn", "rosa", "sami", "tara"];
const LAST = ["okafor", "smith", "garcia", "ito", "novak", "haddad", "kim", "silva", "berg", "rossi", "patel", "moreau", "lund", "costa", "weber"];
const TOPICS = ["project deadline", "favourite food", "budget owner", "meeting notes", "travel plans", "access to the build server", "on-call rota", "preferred editor"];
const KINDS = ["person", "organisation", "location"];

export const mcpErrors = new Rate("mcp_errors");

export const options = {
  setupTimeout: "30m",
  scenarios: {
    mix: { executor: "constant-vus", vus: parseInt(__ENV.VUS || "250", 10), duration: __ENV.DURATION || "60s", exec: "mix" },
  },
  // one threshold per op so k6 keeps a per-op latency series for the summary; the bounds are not targets
  thresholds: Object.fromEntries([
    ...OPS.map((op) => [`http_req_duration{op:${op}}`, ["p(99)<600000"]]),
    ...OPS.map((op) => [`mcp_errors{op:${op}}`, ["rate<=1"]]),
    ["http_req_duration{scenario:mix}", ["p(99)<600000"]],
    ["mcp_errors", ["rate<=1"]],
  ]),
  summaryTrendStats: ["avg", "med", "p(95)", "p(99)", "max", "count"],
};

const headers = (token) => ({ "Content-Type": "application/json", Accept: "application/json, text/event-stream", Authorization: `Bearer ${token}` });
const name = (n) => `${FIRST[n % FIRST.length]} ${LAST[Math.floor(n / FIRST.length) % LAST.length]} ${n}`;
const pick = (a) => a[Math.floor(Math.random() * a.length)];
const toolCall = (id, tool, args) => ({ jsonrpc: "2.0", id, method: "tools/call", params: { name: tool, arguments: args } });

function writeArgs(n, entities, salt) {
  const e = n % entities;
  return { subject_name: name(e), subject_kind: KINDS[e % KINDS.length], text: `${name(e)}: ${TOPICS[n % TOPICS.length]} is item ${n} (${salt}).` };
}

export function setup() {
  const run = `${Date.now().toString(36)}${Math.floor(Math.random() * 1e6).toString(36)}`;
  const tokens = [];
  for (let i = 0; i < TOKENS; i++) {
    const jar = new http.CookieJar();
    const email = `k6-mix-${run}-${i}@loadtest.invalid`;
    const post = (path, body) => http.post(`${BASE_URL}${path}`, JSON.stringify(body), { headers: { "Content-Type": "application/json" }, jar });
    const reg = post("/api/auth/register", { email, password: "k6-load-test-password-1" });
    if (reg.status !== 200) fail(`register ${email}: ${reg.status} ${reg.body}`);
    const tok = post("/api/auth/tokens", { name: "k6-mix" });
    if (tok.status !== 200) fail(`token ${email}: ${tok.status} ${tok.body}`);
    tokens.push(tok.json("token"));
  }
  // MEMORIES facts over ENTITIES subjects per user, in JSON-RPC batches of 32, 4 requests at a time
  const started = Date.now();
  for (const token of tokens) {
    const reqs = [];
    for (let from = 0; from < MEMORIES; from += 32) {
      const batch = [];
      for (let n = from; n < Math.min(MEMORIES, from + 32); n++) batch.push(toolCall(n + 1, "memory_write", writeArgs(n, ENTITIES, "seed")));
      reqs.push(["POST", `${BASE_URL}/mcp`, JSON.stringify(batch), { headers: headers(token), timeout: "300s" }]);
    }
    for (let i = 0; i < reqs.length; i += 4) {
      for (const res of http.batch(reqs.slice(i, i + 4))) if (res.status !== 200) fail(`seed: ${res.status} ${res.body}`);
    }
  }
  console.log(`seeded ${TOKENS} x (${ENTITIES} entities, ${MEMORIES} memories) in ${((Date.now() - started) / 1000).toFixed(0)}s`);
  return { tokens };
}

function call(token, op, body) {
  const res = http.post(`${BASE_URL}/mcp`, JSON.stringify(body), { headers: headers(token), tags: { op }, timeout: "120s" });
  let ok = res.status === 200;
  if (ok) {
    try {
      const b = res.json();
      ok = !b.error && b.result && (op === "tools_list" ? Array.isArray(b.result.tools) : b.result.isError === false);
    } catch (_) {
      ok = false;
    }
  }
  check(res, { "mcp call ok": () => ok });
  mcpErrors.add(!ok, { op });
}

// recall 30, entities_search 20, memory_write 15, search 15, tools/list 20 (percent)
// ONLY=<op> runs just that op (to see what one costs under the same concurrency)
const ONLY = __ENV.ONLY ? [0, 0.3, 0.5, 0.65, 0.8][OPS.indexOf(__ENV.ONLY)] : undefined;

export function mix(data) {
  const token = pick(data.tokens);
  const r = ONLY === undefined ? Math.random() : ONLY;
  if (r < 0.3) call(token, "recall", toolCall(1, "recall", { query: `${pick(FIRST)} ${pick(TOPICS)}`, limit: 10 }));
  else if (r < 0.5) call(token, "entities_search", toolCall(1, "entities_search", { query: Math.random() < 0.5 ? pick(FIRST) : pick(LAST), limit: 20 }));
  else if (r < 0.65) call(token, "memory_write", toolCall(1, "memory_write", writeArgs(Math.floor(Math.random() * 1e6), ENTITIES, `vu${__VU}`)));
  else if (r < 0.8) call(token, "search", toolCall(1, "search", { query: pick(TOPICS) }));
  else call(token, "tools_list", { jsonrpc: "2.0", id: 1, method: "tools/list" });
}

export function handleSummary(data) {
  const m = data.metrics;
  // the measured phase only (setup's seeding is not in the scenario's series)
  const secs = parseFloat(__ENV.DURATION || "60s");
  const row = (label, trend, count, errs) =>
    `${label.padEnd(16)} ${String(count).padStart(7)} ${(count / secs).toFixed(1).padStart(8)} ${trend.med.toFixed(1).padStart(9)} ${trend["p(95)"].toFixed(1).padStart(9)} ${trend["p(99)"].toFixed(1).padStart(9)} ${(100 * errs).toFixed(2).padStart(7)}%`;
  const lines = [`${"op".padEnd(16)} ${"reqs".padStart(7)} ${"req/s".padStart(8)} ${"p50 ms".padStart(9)} ${"p95 ms".padStart(9)} ${"p99 ms".padStart(9)} ${"errors".padStart(8)}`];
  const all = m["http_req_duration{scenario:mix}"];
  for (const op of OPS) {
    const t = m[`http_req_duration{op:${op}}`];
    if (t) lines.push(row(op, t.values, t.values.count, m[`mcp_errors{op:${op}}`]?.values.rate || 0));
  }
  lines.push(row("all", all.values, all.values.count, m.mcp_errors ? m.mcp_errors.values.rate : 0));
  const out = { stdout: `\nduration ${secs.toFixed(0)}s, ${__ENV.VUS || 250} VUs, ${TOKENS} tokens\n${lines.join("\n")}\n` };
  if (__ENV.SUMMARY_OUT) out[__ENV.SUMMARY_OUT] = JSON.stringify(data, null, 1);
  return out;
}
