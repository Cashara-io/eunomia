# SurrealQL functions used by the backend

Scope: every SurrealQL query string in `backend/src`. Rust calls (for example `search::upsert` and `search::set_embedding` in `cache/search.rs`, or `chrono::Utc::now()`) are not counted. Counts are occurrences in SQL, found with grep on 2026-10-08. Comments are not counted.

## Functions

| Function | Occurrences | Files | Example (file:line) |
|---|---|---|---|
| `time::now()` | 48 | 10 | `backend/src/db.rs:19` |
| `string::lowercase()` | 8 | 4 | `backend/src/models_user.rs:65` |
| `string::contains()` | 3 | 2 | `backend/src/cache/search.rs:305` |
| `search::score()` | 2 | 2 | `backend/src/cache/search.rs:302` |
| `count()` (aggregate, `GROUP ALL`) | 10 | 7 | `backend/src/routers/audit.rs:69` |

Files for `time::now()`: `models_user.rs`, `db.rs`, `auth.rs`, `routers/settings.rs`, `connectors/service.rs`, `cache/search.rs`, `chat/service.rs`, `vaults/service.rs`, `entities/consolidate.rs`, `entities/service.rs`.

Files for `string::lowercase()`: `models_user.rs`, `sources/heypocket.rs` (2 calls on line 248), `vaults/service.rs`, `cache/search.rs`.

Files for `count()`: `routers/audit.rs`, `routers/auth.rs`, `routers/sources.rs`, `tools/generic.rs`, `tools/registry.rs`, `cache/search.rs`, `vaults/service.rs`.

## Related SQL syntax (operators, not functions)

- Full-text match `@1@`: `backend/src/cache/recall.rs:193`, `backend/src/cache/search.rs:303`.
- KNN `<|K|>`: `backend/src/cache/search.rs:345`.

## Function families

## Allow list for `surreal start`

Use this with `--deny-all`, plus `--allow-rpc --allow-http --allow-arbitrary-query=system` (RPC for the backend and the `isready` healthcheck, which must use `ws://` because over `http://` it fails with "Method not allowed", HTTP for the CLI, arbitrary queries for root; flag names from the 3.3 `surreal start` docs). `--deny-all` with this set has since been run on live 3.3.1 (2026-10-09) and 3.3.5 (2026-10-31) servers; see the last section. The flag syntax is documented at https://surrealdb.com/docs/surrealdb/cli/start: `--allow-funcs` takes a comma-separated list, and each entry is `<family>` or `<family>::<name>`.

```
surreal start --deny-all --allow-funcs time,string,search,count [other flags]
```

Or with the env var: `SURREAL_CAPS_ALLOW_FUNC=time,string,search,count`.

Checks before you rely on it:
- The docs say a bare family name (`http`) and a wildcard (`http::*`) both include the whole family. Test that `count` alone covers `count()`. If not, add `count::*`.
- `DEFINE FIELD ... DEFAULT time::now()` in `db.rs` runs at write time, so the allow list must cover it. Test on a throwaway container.
- `docker-compose.yml` pins SurrealDB `v2.3`. These flags come from the 3.x docs. Confirm they exist in the version you run.

## Update for SurrealDB 3.3.1 (the shipped allow list)

The grep above missed some families. The list `docker-compose.yml` ships, and that the whole test suite runs under (`TEST_HARDENED=1 cargo nextest run`, which reads the `--allow-funcs=` value out of the compose file; CI runs it as its own step after the plain `cargo nextest run`, plus `ISOLATION_TEST_NO_APP_FILTERS=1 cargo nextest run --test isolation`), is:

```
--deny-all --allow-funcs=time,string,search,count,array,vector,math --query-timeout=60s --transaction-timeout=60s
```

- `array::` (len, concat, slice, sort, union): store/vaults.rs, store/entities.rs, store/jobs.rs.
- `vector::similarity::cosine`: the exact-scan fallback of semantic search (`cache.semantic_exact`).
- `math::max`: `jobs.release_worker`.
- `string::len/trim/concat` are covered by `string`. The HNSW `<|K,EF|>` operator and `search::score` need nothing beyond `search`.
- Removing `vector` from the list makes the suite fail with "Function 'vector::similarity::cosine' is not allowed", so the list is both necessary and sufficient for the code today.
- `--query-timeout=60s --transaction-timeout=60s` are for the running app only. A dump import or restore runs on a server without them (`docker-compose.import.yml`, used by `scripts/upgrade-surreal-v3.sh` and `backend/scripts/restore.sh`), then the hardened server is started again. The tests do not set the timeouts.
- Not verified against a running 3.3.1 server (Docker was unavailable): `--deny-all` may also deny the RPC/HTTP surfaces and arbitrary queries for the root user. If the backend cannot connect, add `--allow-rpc --allow-http --allow-arbitrary-query=system` (all exist in 3.3.1 `start --help`). The flag names, not their interaction, were checked.

## Re-checked on SurrealDB 3.3.5 (2026-10-31, release 2.0.8)

The `docker-compose.yml` flags, unchanged, on a throwaway `surrealdb/surrealdb:v3.3.5` container on RocksDB (`--deny-all --allow-rpc --allow-http --allow-arbitrary-query=system --allow-funcs=time,string,search,count,array,vector,math,file,type::file --allow-experimental=files --query-timeout=60s --transaction-timeout=60s`, `SURREAL_BUCKET_FOLDER_ALLOWLIST=/data/documents`):

- The server starts. Adding `--deny-guests` is still a startup error (a usage error, as on 3.3.1).
- Root queries over HTTP `/sql` work; `/health` and `/status` answer 200.
- Every allowed family runs (`time::now`, `string::lowercase`, `math::max`, `array::len`, `vector::similarity::cosine`, `count`). Families outside the list are refused with "Function '...' is not allowed to be executed": `crypto`, `rand`, `http`, `object`, `meta`, `record`, `type::string`, `parse`, `duration`, `geo`.
- `--allow-experimental=files`: `DEFINE BUCKET ... BACKEND 'file:/data/documents'`, `.put()` and `.get()` work; a bucket outside the allow list fails with "File access denied".
- `surreal isready` passes over `ws://` and, on this throwaway container, also over `http://` (empty, with 2,000 rows, and after a restart; 3.3.1 behaved the same here). The `http://` failure ("Method not allowed") was seen on the live hardened 3.3.1 server and is not reproduced by this setup, so the healthcheck and the backup keep using `ws://`.
- The whole backend suite under this list (`TEST_HARDENED=1 cargo nextest run`) passes on the 3.3.5 crate.
