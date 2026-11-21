// Shared helpers for the Cupel k6 scripts: user setup, MCP calls, metrics.
import http from "k6/http";
import { check, fail } from "k6";
import { Rate } from "k6/metrics";

export const BASE_URL = __ENV.BASE_URL || "http://localhost:8101";
export const ORGS = parseInt(__ENV.ORGS || "20", 10);
// Memories seeded for the biggest user; user i gets WHALE / (i + 1), at least MIN_MEMORIES (Zipf-like).
const WHALE_MEMORIES = parseInt(__ENV.WHALE_MEMORIES || "400", 10);
const MIN_MEMORIES = parseInt(__ENV.MIN_MEMORIES || "5", 10);

export const mcpErrors = new Rate("mcp_errors");

const QUERIES = ["project deadline", "favourite food", "who owns the budget", "meeting notes", "travel plans", "preferences"];
const KINDS = ["person", "organisation", "location"];

function tierFor(i) {
  if (i === 0) return "whale";
  return i < Math.ceil(ORGS * 0.15) ? "mid" : "small";
}

function postJson(path, body, params) {
  return http.post(`${BASE_URL}${path}`, JSON.stringify(body), {
    headers: { "Content-Type": "application/json", Accept: "application/json, text/event-stream" },
    ...params,
  });
}

function toolCall(id, name, args) {
  return { jsonrpc: "2.0", id, method: "tools/call", params: { name, arguments: args } };
}

function memoryWriteArgs(user, n) {
  return {
    subject_name: `${user} subject ${n % 25}`,
    subject_kind: KINDS[n % KINDS.length],
    text: `${user} fact ${n}: ${QUERIES[n % QUERIES.length]} is item ${n}, noted at ${Date.now()}.`,
  };
}

// Register ORGS users, mint a token each, seed memories over MCP in JSON-RPC batches of 20.
export function setupUsers() {
  const run = `${Date.now().toString(36)}${Math.floor(Math.random() * 1e6).toString(36)}`;
  const users = [];
  for (let i = 0; i < ORGS; i++) {
    const jar = new http.CookieJar();
    const email = `k6-${run}-${i}@loadtest.invalid`;
    const reg = postJson("/api/auth/register", { email, password: "k6-load-test-password-1" }, { jar });
    if (reg.status !== 200) fail(`register ${email}: ${reg.status} ${reg.body}`);
    const tok = postJson("/api/auth/tokens", { name: "k6" }, { jar });
    if (tok.status !== 200) fail(`token ${email}: ${tok.status} ${tok.body}`);
    const token = tok.json("token");
    const size = Math.max(MIN_MEMORIES, Math.round(WHALE_MEMORIES / (i + 1)));
    for (let from = 0; from < size; from += 20) {
      const batch = [];
      for (let n = from; n < Math.min(size, from + 20); n++) batch.push(toolCall(n + 1, "memory_write", memoryWriteArgs(email, n)));
      const res = postJson("/mcp", batch, { headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json", Accept: "application/json, text/event-stream" } });
      if (res.status !== 200) fail(`seed ${email}: ${res.status} ${res.body}`);
    }
    users.push({ email, token, size, tier: tierFor(i) });
  }
  return { users };
}

export function pick(data, tier) {
  let pool = data.users.filter((u) => u.tier === tier);
  if (pool.length === 0) pool = data.users; // a tier is empty when ORGS is small
  return pool[Math.floor(Math.random() * pool.length)];
}

// One MCP tools/call. `op` and the user's tier become tags so thresholds can target them.
export function call(user, op, name, args, extraTags) {
  const res = http.post(
    `${BASE_URL}/mcp`,
    JSON.stringify(toolCall(1, name, args)),
    {
      headers: { "Content-Type": "application/json", Accept: "application/json, text/event-stream", Authorization: `Bearer ${user.token}` },
      tags: { op, size: user.tier, ...extraTags },
    },
  );
  let ok = res.status === 200;
  if (ok) {
    try {
      const body = res.json();
      ok = !body.error && body.result && body.result.isError === false;
    } catch (_) {
      ok = false;
    }
  }
  check(res, { "mcp call ok": () => ok });
  mcpErrors.add(!ok);
  return res;
}

export const recall = (u, t) => call(u, "recall", "recall", { query: pickOne(QUERIES), limit: 10 }, t);
export const search = (u, t) => call(u, "search", "search", { query: pickOne(QUERIES) }, t);
export const write = (u, t) => call(u, "write", "memory_write", memoryWriteArgs(u.email, Math.floor(Math.random() * 1e6)), t);

function pickOne(a) {
  return a[Math.floor(Math.random() * a.length)];
}

// Print p95 per tier/phase as a short table in addition to the default summary.
export function fairnessSummary(data, textSummary) {
  const p95 = (k) => {
    const m = data.metrics[k];
    return m && m.values ? m.values["p(95)"] : undefined;
  };
  const base = p95("http_req_duration{op:recall,size:small,phase:baseline}");
  const noisy = p95("http_req_duration{op:recall,size:small,phase:noisy}");
  let line = "";
  if (base !== undefined && noisy !== undefined) {
    line = `\nfairness: small-user recall p95 baseline=${base.toFixed(1)}ms noisy=${noisy.toFixed(1)}ms ratio=${(noisy / base).toFixed(2)} (limit 2.00)\n`;
  }
  return { stdout: textSummary + line };
}
