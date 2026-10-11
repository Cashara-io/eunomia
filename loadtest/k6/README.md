# k6 load tests

Drives Eunomia through its public HTTP and MCP surface only. Point it at a **test stack**, never at a real install: `setup` registers `ORGS` users and writes memories for each.

## Run

```sh
# local k6
k6 run loadtest/k6/steady.js
k6 run loadtest/k6/burst.js
k6 run loadtest/k6/noisy.js

# or without installing k6 (pinned image; --network host so localhost works on Linux,
# on macOS use BASE_URL=http://host.docker.internal:8101)
docker run --rm -i --network host -v "$PWD/loadtest/k6:/k6" -e BASE_URL=http://localhost:8101 \
  grafana/k6:0.54.0 run /k6/noisy.js
```

| Env | Default | Meaning |
| --- | --- | --- |
| `BASE_URL` | `http://localhost:8101` | Backend root (the one serving `/mcp` and `/api`) |
| `ORGS` | `20` | Users to register |
| `WHALE_MEMORIES` | `400` | Memories for user 0; user i gets `WHALE_MEMORIES/(i+1)` (min `MIN_MEMORIES`, default 5) |
| `VUS`, `DURATION` | `10`, `2m` | steady.js only |
| `PEAK_RPS` | `60` | burst.js peak write rate |
| `PHASE_SECONDS`, `WHALE_VUS` | `60`, `20` | noisy.js phase length and whale concurrency |
| `BASELINE_P95_MS` | `75` | noisy.js: small-user baseline p95 the 2x fairness limit is computed from |

User tiers: `whale` (user 0), `mid` (top 15 percent), `small` (the rest). Every request is tagged `op` (`recall`, `search`, `write`) and `size` (tier); noisy.js also tags `phase` (`baseline`, `noisy`).

## Scenarios

- `steady.js`: constant mix of recall (60 percent), search (30), write (10) across all tiers.
- `burst.js`: ramping-arrival-rate of writes, with a recall probe alongside.
- `mcp-mix.js`: saturation over `/mcp` from `VUS` (default 250) agents sharing `TOKENS` (default 4) tokens: recall 30 percent, entities_search 20, memory_write 15, search 15, tools/list 20, no think time, against `ENTITIES`/`MEMORIES` (750/1500) per user seeded in setup. Start the backend with `RATE_LIMIT_TOKEN_PER_MIN=0 RATE_LIMIT_USER_PER_MIN=0` (and `RATE_LIMIT_AUTH_PER_MIN` above `TOKENS`), or the per-token limit refuses most calls. Prints req/s, p50/p95/p99 and the error rate per op; `ONLY=<op>` runs one op alone. Database round trips per request: `cd backend && cargo test --release --test round_trips -- --nocapture`.
- `noisy.js`: phase 1 small users alone (baseline). Phase 2 the same small users while the whale runs 20 VUs of recall/search with no think time.

## Thresholds

- `http_req_duration{op:recall} p(95)<150`: 95 percent of recalls answer within 150 ms.
- `mcp_errors rate<0.01`: under 1 percent of calls fail. A failure is a non-200, a JSON-RPC error, or `result.isError`.
- `http_req_duration{op:write} p(95)<1000` (burst only): writes stay bounded under the burst.
- Fairness (noisy.js): `recall{size:small,phase:noisy} p(95) < 2 x BASELINE_P95_MS`, same for `size:mid`. k6 thresholds are constants, so for a strict check run noisy.js once, read the `phase:baseline` p95, then rerun with `-e BASELINE_P95_MS=<that>`. The end-of-run summary also prints the measured ratio (limit 2.00). The whale's own latency is deliberately not thresholded.
- Default `BASELINE_P95_MS=75` makes the noisy limit equal the absolute 150 ms target.

noisy.js imports `k6-summary` from jslib.k6.io (pinned), so the machine running it needs internet on first run.
