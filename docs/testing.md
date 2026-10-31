# Testing

Every test layer, from cheapest to most destructive. Run the first six on every change. The rest need a stack or special tooling.

| Layer | What it proves | Where | Run |
| --- | --- | --- | --- |
| Unit | Pure logic in each module | `#[cfg(test)]` blocks in `backend/src/**` | `cd backend && cargo test --lib` (or `cargo nextest run --lib`) |
| Real-DB harness and goldens | Tools, routes and concurrent writes against a real in-memory SurrealDB; tool output shapes pinned by `insta` snapshots | `backend/tests/` (`common/`, `golden_tools.rs`, `http_routes.rs`, `concurrent_memory.rs`, `snapshots/`) | `cd backend && cargo test` ; review snapshot changes with `cargo insta review` |
| Hardened and filters-off runs | The whole suite under the shipped `--allow-funcs` list, and the isolation proof with the application owner/vault filters off | `backend/tests/common/mod.rs`, `backend/tests/isolation.rs` | `cd backend && TEST_HARDENED=1 cargo nextest run` ; `ISOLATION_TEST_NO_APP_FILTERS=1 cargo nextest run --test isolation` |
| Isolation proof | Two orgs, canaries in every table, every tool and route called as one org with the other's ids: no leak with both layers, with the app filters off, or with a shared database; the harness fails when a filter is removed; credential wall; static gates | `backend/tests/isolation.rs`, `backend/tests/store.rs`, `docs/architecture/tenancy.md` | `cd backend && cargo test --test isolation` |
| Concurrency without the in-process lock | Writers meet at the transaction and retry path: the striped `tx::lock` is switched off (`tx::LOCKS_OFF`, `test-support` builds only) and two memory writers and four job claimers run against the in-memory engine; asserts what the engine guarantees, prints what it does not | `backend/tests/no_lock.rs` | `cd backend && cargo test --test no_lock -- --nocapture` |
| Tenancy and the self-host move | Provisioning, the pool, schema N-1 gating, the verified resumable move of a single-database install (the 2.x export fixture) | `backend/tests/tenancy.rs` | `cd backend && cargo test --test tenancy` |
| Script tests | Shell scripts (auto-update, agent connect, backup and restore of every database) against a mock server | `scripts/tests/` | every `scripts/tests/*.test.sh` except `upgrade-surreal-v3.test.sh` (real containers, own CI job) runs in the CI `scripts` job by glob; locally `for t in scripts/tests/*.test.sh; do bash $t; done` |
| Playwright e2e | The real UI and `/mcp` through the frontend proxy | `frontend/tests/` | Start backend and SurrealDB (`docker compose up -d surrealdb backend`), then `cd frontend && bun run test` (`E2E_BASE_URL=...` to target a running stack; `https-settings.spec.ts` also needs `E2E_UPDATE_STATUS_DIR` set to the backend's `UPDATE_STATUS_DIR`, and skips itself without it; its admin cases also need `E2E_ADMIN_EMAIL` and `E2E_ADMIN_PASSWORD` of the install's first user, because only an instance admin can change HTTPS). `memory-correctness.spec.ts` seeds fixtures with SQL when `E2E_SURREAL_HTTP`, `E2E_SURREAL_NS` and `E2E_SURREAL_DB` (the user's `org_<uuid>` database) are set, and skips those tests otherwise. CI gets all of these from `scripts/ci/e2e-setup.sh` (registers the admin, finds the org database) and `docker-compose.ci.yml` (publishes SurrealDB on 127.0.0.1:8000, CI only). The specs' mock providers and model endpoints listen on the test machine: against a containerised backend set `E2E_MOCK_BIND=0.0.0.0` and `E2E_MOCK_HOST=host.docker.internal` (the CI overlay maps that name to the host gateway); against a backend run natively the defaults (`127.0.0.1`) are right. Against a local (non-compose) stack, `BACKEND_INTERNAL_URL` (the backend address `next.config.ts` rewrites `/api` and `/mcp` to, default `http://localhost:8001`) is baked in when the frontend is built, so set it for `bun run build` (or for `bun run dev`), not for `next start`. `recall-eval.spec.ts` is the retrieval evaluation (Recall@5 and MRR floors on `tests/fixtures/recall-eval.json`) |
| k6 load | Latency, error rate and noisy-neighbour fairness over MCP | `loadtest/k6/` | `k6 run loadtest/k6/noisy.js`, see `loadtest/k6/README.md` |
| Fuzz | Parsers never panic on hostile input (MCP request parsing, every connector `map`, summary functions) | `backend/fuzz/` | `cd backend && cargo +nightly fuzz run source_map` (also `jsonrpc_parse`, `summaries`). Needs `cargo install cargo-fuzz` and a nightly toolchain. Without them, `cd backend/fuzz && cargo check --bins` still type-checks the targets |
| Chaos | Killing the backend and DB mid-load loses no acknowledged records and `/healthz` recovers | `scripts/chaos.sh` | `CHAOS_CONFIRM=yes scripts/chaos.sh` |
| Restore drill | Backups actually restore (seed, back up, wipe, restore, compare counts) | `scripts/restore-drill.sh` | `scripts/restore-drill.sh` (CI job `restore-drill`) |
| Restore drill, tenancy layout | A backup directory with `control` and one `org_<32 hex>` database restores: per-database per-table counts match. Needs Docker. First real run is the CI job `restore-drill`, after the plain drill (written while Docker was unavailable, only `bash -n` checked before that) | `scripts/restore-drill-tenancy.sh` | `scripts/restore-drill-tenancy.sh` |

## What CI runs

- `backend`: `cargo clippy --all-targets -- -D warnings`, `cargo nextest run`, `TEST_HARDENED=1 cargo nextest run`, `ISOLATION_TEST_NO_APP_FILTERS=1 cargo nextest run --test isolation`, `job_claims_across_processes` against a real RocksDB-backed SurrealDB 3.3.5 container (below), `cargo build --release`.
- `frontend`: generated API client check, lint, build, `tsc`. `e2e`: Playwright against the compose stack.
- `scripts`: `scripts/ci/check-surreal-pins.sh` (every SurrealDB pin on the same patch), every `scripts/tests/*.test.sh` except the two that need Docker, plus `bash -n` on the scripts.
- `upgrade-surreal-v3`: `scripts/tests/upgrade-surreal-v3.test.sh` (real 2.x to 3.3 move; the large-table case `UPGRADE_TEST_LARGE=1` is not run in CI).
- `restore-drill`: `scripts/restore-drill.sh` then `scripts/restore-drill-tenancy.sh`.
- `update-path`: `scripts/tests/update-path.test.sh v2.0.7`, an install on the previous release updated to this build through the real updater (below).
- Not in CI: Chaos, k6, fuzz.

## Update path

`bash scripts/tests/update-path.test.sh <from-tag>` (CI job `update-path`, `from: v2.0.7`; needs Docker, `jq` and a clone with the release tags) proves that an install on a released tag takes the current checkout through the real update path:

1. Builds this checkout's backend, frontend and backup images and tags them `v9.9.9-ci`, a local-only tag that sorts above every release. It is never pushed: pushing any `v*` tag publishes images and offers it to every install.
2. A local bare repo stands in for GitHub: the real tags up to `<from-tag>`, plus `v9.9.9-ci` on HEAD.
3. Clones `<from-tag>` from it and starts it with that release's published ghcr images, as compose project `eu-update` on ports 8251 (backend) and 3251 (frontend), `EMBEDDINGS_BACKEND=stub` (so the HNSW indexes hold vectors) and `EUNOMIA_SIGNUP_ORG=personal` (two users, two org databases).
4. Seeds through the API: two users, a PAT each, entities of the built-in kinds and a custom one, facts, an edited fact, a deleted fact, a relation, two extra vaults, skills, a document. One fact is marked superseded with the statement `entities.mark_superseded` runs, because superseding needs a chat model the test does not have.
5. Snapshots per-database, per-table row counts, the `_migration` ledgers and a fixed set of reads (recall, entities_get, skill_get, skill_find, the documents list).
6. `backup now pre-update`.
7. "Update now" as the admin (`POST /api/update/request`), then the OLD release's `scripts/auto-update.sh` on the host with `EUNOMIA_DIR` set. Only `docker compose pull` is shimmed; every other docker call is real.
8. Asserts `status.json` and `history.log`, every service healthy and on the new images, `surrealdb` on the pin in `docker-compose.yml` with the `ws://` healthcheck, `/readyz`, ledgers at the latest migration, row counts (bar the tables that move by themselves: jobs, sessions, audit and usage rows) and reads unchanged.
9. Runs the updater again, restarts the backend and runs `up -d` again: no new history line, no container recreated, no new ledger rows, counts unchanged.
10. Rollback: copies the data volume and reports (information only) whether the old SurrealDB opens what the new one wrote; then checks out `<from-tag>`, starts it on a fresh volume, restores the pre-update backup with `backend/scripts/restore.sh <file> --wipe`, and checks counts, reads and ledgers against the snapshot.

On failure, logs, `docker compose ps`, the snapshots and `update-status/` go to `update-path-artifacts/` (uploaded by CI). `UPDATE_PATH_KEEP=1` leaves the stack up, `UPDATE_PATH_SKIP_BUILD=1` reuses existing `v9.9.9-ci` images. Waits are bounded; the job's timeout is 45 minutes.

## Safety

- k6 registers users and writes memories. Point it at a test stack only.
- Chaos kills containers. It only touches compose project `fw-chaos` (own volumes, port `18101`), refuses ports used by a normal install, and needs `CHAOS_CONFIRM=yes`. Never run it against a real install.
- Both restore drills use the throwaway database container `fw-backup-db`, never the live stack.
- Do not run any of these against the containers or volumes of a real `eunomia` compose project.

## Concurrency ceiling

`tx::lock` (a striped in-process mutex) serialises same-key writers inside one process, so `concurrent_memory.rs` and the job tests in `jobs.rs` run with it on and cannot fail if the transaction and retry path breaks. `tests/no_lock.rs` switches it off and measures. Result on the in-memory engine, one process, debug build (3 runs):

| Case | Lock on | Lock off |
| --- | --- | --- |
| 2 tasks x 100 x (fact + observation) on one entity | 400 of 400 ok, exact counts | 400 of 400 ok, 0 conflicts reached the caller, one person, 200 fact rows, observation version 200 |
| 4 claimers, 200 jobs, claim batches of 5 | each job handed out once | each job handed out once; 4 to 6 `claim` calls per run ended in a 409 after their 5 attempts (the worker loop just polls again) |

What this proves: with the lock off, the transaction and `with_retry` keep two concurrent writers exact, and a lost race is retryable (`db.conflict`) and never a 500. Before `store::surface_root_cause` the same run lost 80 of 400 writes to a 500 (the retry never fired); the test fails on that.

**Several processes on a real server.** `job_claims_across_processes` (`tests/jobs.rs`, CI) starts 4 copies of the test binary as separate OS processes, each with its own connection and one worker loop of 16 slots, all claiming 6,000 jobs from one RocksDB-backed `surrealdb/surrealdb:v3.3.5` server with the hardened flags. `tx::lock` is per process, so only the server's commit-time conflict check keeps claims exclusive; the lease is long and nothing is killed, so a job run twice was claimed twice. It asserts every job ran exactly once and that the server agrees (every row `done` with `attempts = 1`). It skips itself without `TEST_SURREAL_URL`:
`docker run -d --name fw-claim-db --user root -p 127.0.0.1:8231:8000 surrealdb/surrealdb:v3.3.5 start --user root --pass root rocksdb:/data/db` (add the `--deny-all ... --allow-funcs=...` flags of `docker-compose.yml` to match production, as CI does), then `cd backend && TEST_SURREAL_URL=ws://127.0.0.1:8231/rpc cargo test --release --test jobs job_claims_across_processes -- --nocapture`. Result on a laptop (OrbStack, release build, 3 runs): 6,000 of 6,000 jobs run once, 0 claimed twice, 0 lost, 0 dead, every process claimed 1,100 to 2,100 of them, about 1,100 to 1,200 claim-and-complete cycles per second. With the claim's transaction and re-check removed on purpose, the same run reported 1,140 jobs claimed twice, so it does detect a broken claim.

What it does **not** prove: the in-memory and single-process runs above say nothing about several OS processes; that is the run just described. Still open: the memory-write path (`no_lock.rs`) over `ws://`, and worker kills across processes. The in-memory engine has its own commit check; the one in RocksDB, the network between replicas and the client's reconnect behaviour have not been exercised. The assertions deliberately stop at what the engine guarantees: the number of 409s and the observation version counter are printed, not asserted (the engine is documented to miss some bare write-write races; these statements did not hit that in the runs above, but nothing here rules it out).

To close the gap when Docker is back:

1. Single process, real server, lock still off (real commit conflicts over `ws://`):
   `docker run -d --name fw-nolock-db --user root -p 8231:8000 surrealdb/surrealdb:v3.3.5 start --user root --pass root rocksdb:/data/db` (without `--user root` the image cannot create the RocksDB directory), then
   `cd backend && TEST_SURREAL_URL=ws://127.0.0.1:8231/rpc TEST_SURREAL_USER=root TEST_SURREAL_PASS=root cargo test --test no_lock -- --test-threads=2 --nocapture`, and the same with `--test jobs -- --ignored job_claim_throughput` (S3). Remove the container afterwards.
2. Several processes: `docker compose up -d --scale backend=2` with `EUNOMIA_ROLE=api` on two replicas and two `EUNOMIA_ROLE=worker` replicas, then `k6 run loadtest/k6/noisy.js` and check `SELECT kind, status, count() FROM job GROUP BY kind, status` for duplicates and dead jobs (see `docs/architecture/jobs.md`).

## Seeing a failure

For a failure in a running instance (a trace id from the UI or an MCP error), see [debugging.md](debugging.md): `eunomia replay <trace_id>` re-runs it and `--emit-test` writes the failing test.


Fuzz crashes are saved under `backend/fuzz/artifacts/<target>/`; replay with `cargo +nightly fuzz run <target> <file>`. Chaos leaves k6 output in `/tmp/fw-chaos-k6.log`. k6 prints per-threshold pass or fail at the end of every run.

## Spike S1: database per org at scale

`cd backend && S1_N=200 cargo test --test spike_s1 -- --ignored --nocapture` (add `--release` for numbers closer to a server). Provisions N org databases on the in-memory engine, each migrated (HNSW 1536 and two FULLTEXT indexes) and loaded with 100 records, then prints provisioning time per org, resident memory, cold `pool.for_org` and first-query latency, vector-search latency on a cold handle, and how long a one-field migration takes to fan out over every org. A script, not a gate.
