# SurrealQL 3.x cheat sheet

Use this when you write or review SurrealDB 3.x queries. Most 2.x examples on the web still work in 2.x only, and several now fail in 3.x.

Status: "unverified" means the official docs or release notes I could read did not confirm the fact. Check it before you rely on it.

Repo note: the backend now runs on SurrealDB 3.3.5 (crate and image pinned exactly; 3.3.1 until release 2.0.8). Indexes live in `migrations/tenant/0008_v3_indexes.surql`. Things found while porting that are not in the lists below: `FLEXIBLE` goes after the type (`TYPE object FLEXIBLE`); a two-field `FULLTEXT` index is rejected ("Expected one column"), so one index per field; the namespace and database must exist before `USE`; every statement of a transaction, `BEGIN`, `LET` and `COMMIT` included, has a result slot (`res.take(n)`); NULL is not accepted for an `object` field; `RecordId` has no `Display`, `FromStr`, `key()` or `table()` (see `src/rid.rs`); `Datetime`'s `Display` lost its `d'..'` wrapper (matters for stored hashes, see `hash_envelope`); `serde_json` needs `preserve_order` to keep 2.x key order.

Sources (all official):
- S-MIG: https://surrealdb.com/docs/build/migrating/from-old-surrealdb-versions/2x-to-3x (2.x to 3.x breaking changes)
- S-IDX: https://surrealdb.com/docs/surrealdb/surrealql/statements/define/indexes (HNSW, FULLTEXT, CONCURRENTLY, KNN)
- S-R30 to S-R33: https://surrealdb.com/releases/3.0, /3.1, /3.2, /3.3 (release notes)
- S-CLI: https://surrealdb.com/docs/surrealdb/cli/start (capability flags)
- S-BEGIN: https://surrealdb.com/docs/surrealdb/surrealql/statements/begin
- S-FN: https://surrealdb.com/docs/surrealdb/surrealql/functions

## 1. Vector indexes (MTREE is gone)

Source: https://surrealdb.com/docs/build/migrating/from-old-surrealdb-versions/2x-to-3x item 9, https://surrealdb.com/docs/surrealdb/surrealql/statements/define/indexes.

- 2.x: `DEFINE INDEX i ON cache_record FIELDS embedding MTREE DIMENSION 1536 DIST COSINE TYPE F32;`
- 3.x: `DEFINE INDEX i ON cache_record FIELDS embedding HNSW DIMENSION 1536 DIST COSINE TYPE F32;`
- HNSW options: `DIMENSION n`, `TYPE` (F64, F32, I64, I32, I16, default F32), `DIST`, `EFC`, `M`.
- Build without blocking: add `CONCURRENTLY` at the end, then check progress with `INFO FOR INDEX`. Do not start or leave a `CONCURRENTLY` build or `REBUILD INDEX` running across a version change (S-R32, 3.2.4).
- KNN search: `embedding <|K,EF|> $vec`. K is the number of results. EF bounds the candidate list. Example from S-IDX: `WHERE point <|10,40|> [2,3,4,5]`.
- Repo today: `backend/src/cache/search.rs:345` uses `<|{}|>` with one number. Check which EF it sends (unverified for 3.x).
- DiskANN is a 3.1 index type with the same KNN operator: `DISKANN DIMENSION 4 DIST EUCLIDEAN TYPE F32 DEGREE 16 L_BUILD 64;` (S-R31).

## 2. Full-text search (SEARCH ANALYZER is gone)

Source: https://surrealdb.com/docs/build/migrating/from-old-surrealdb-versions/2x-to-3x item 7, https://surrealdb.com/docs/surrealdb/surrealql/statements/define/indexes.

- 2.x: `DEFINE INDEX i ON memory FIELDS text SEARCH ANALYZER cache_text_analyzer BM25 HIGHLIGHTS;`
- 3.x: `DEFINE INDEX i ON memory FIELDS text FULLTEXT ANALYZER cache_text_analyzer BM25 HIGHLIGHTS;`
- BM25 can take parameters: `BM25(k1, b)`.
- Match operator: `WHERE text @1@ $t`. Score with `search::score(1)` (S-FN confirms `search::score(1)` with `@1@`).
- One field per index: unverified. `db.rs:96` puts two fields (`title, body_text`) in one index. Test it on a throwaway 3.x database.

## 3. Schema strictness

Source: https://surrealdb.com/docs/build/migrating/from-old-surrealdb-versions/2x-to-3x items 8 and 19, https://surrealdb.com/releases/3.0.

- SCHEMAFULL extra fields. 2.x silently drops them. 3.x errors: `Found field 'other', but no such field exists`. Select only defined fields with `.{ name }` if you need to ignore extras.
- Missing table. 3.x errors: `The table 'doesnt_exist' does not exist`. 2.x did not error.
- Strict mode. 2.x: `surreal start --strict`. 3.x: `DEFINE DATABASE mydb STRICT;`. Strict can differ per database on one server (S-MIG item 8).
- `DEFINE ... OVERWRITE`: the 3.0 notes say import now overwrites by default (S-R30). The `OVERWRITE` clause on `DEFINE` statements in 3.x: unverified.
- Field counts for arrays: in `DEFINE FIELD`, the number on an array is now the exact length. Use `array<int, 640>` for exact, and `ASSERT $value.len() <= 1000` for a maximum (S-MIG item 30).

## 4. SET and LET

Source: https://surrealdb.com/docs/build/migrating/from-old-surrealdb-versions/2x-to-3x items 4 and 21.

- SET reads the record as it was before the statement. 2.x: `UPDATE t:1 SET a = 10, c = a` gives `c = 10`. 3.x: `c = 1` (the old value of `a`). `CREATE t:2 SET a = 1, b = a + 1` errors.
- Fix with a `LET` or a `COMPUTED` field.
- LET is required for parameters. 2.x: `$val = 10;`. 3.x: `LET $val = 10;`. Without it you get "Parameter declarations without `let` are deprecated."

## 5. Transactions

Source: https://surrealdb.com/docs/surrealdb/surrealql/statements/begin, https://surrealdb.com/releases/3.1, https://surrealdb.com/releases/3.2, https://surrealdb.com/releases/3.0.

- Syntax: `BEGIN [TRANSACTION];` then statements, then `COMMIT TRANSACTION;`. Any error cancels the transaction.
- Commit conflict: 3.1 sends it to clients as `TransactionConflict`, wire code `-32009` (S-R31). Exact error message text: unverified.
- Retry: the SDK `.retry()` helper replays a conflicted transaction (S-R32, 3.2.5). The server does not retry.
- WebSocket limits (3.1): 64 transactions per connection and 64 per session. Extra ones fail with `too_many_transactions`.
- `UPDATE` and `UPSERT` in 3.3 check `WHERE` before the data clause, and both read one copy of the record taken before the write (S-R33).

## 6. Export and upgrade

Source: https://surrealdb.com/docs/build/migrating/from-old-surrealdb-versions/2x-to-3x (CLI section), https://surrealdb.com/releases/3.3, https://surrealdb.com/releases/3.2.

- 2.x export: `surreal export ...` (2.x binary).
- 3.x: first export with the 3.x binary: `surreal v2 export --v3 --namespace <ns> --database <db> --token <token> <file>.surql`. Needs SurrealDB 3.0.3 or later. Then `surreal import`.
- The 3.x binary cannot read 2.x data directly.
- Rollback: a 3.2 binary reads migrated 3.3 data wrongly, so take an export before upgrading (S-R33).
- 3.2.5 and 3.1.6 upgrades are one-way for some data (S-R31, S-R32).

## 7. Functions

Source: https://surrealdb.com/docs/build/migrating/from-old-surrealdb-versions/2x-to-3x item 2, item 3, https://surrealdb.com/docs/surrealdb/surrealql/functions, https://surrealdb.com/releases/3.0, https://surrealdb.com/releases/3.3.

Renamed in 3.x:
- `type::is::record` becomes `type::is_record` (and `is::` becomes `is_`, `from::` becomes `from_`)
- `type::thing` becomes `type::record`
- `rand::guid()` becomes `rand::id()`
- `string::distance::osa_distance` becomes `string::distance::osa`
- `record::is::edge()` becomes `record::is_edge()` (S-R30)

Changed behaviour:
- `array::range(offset, count)` becomes `array::range(start, end)`. Old `array::range(-1, 5)` gave 5 items. New gives 6. Migrate with `offset + count`.
- `math::sqrt(negative)` returns `NaN` (was `NONE`).
- `math::min([])` returns `Infinity`, `math::max([])` returns `-Infinity` (were `NONE`).
- `time::nano` errors outside its range instead of returning 0 (S-R33).
- `vector::similarity::jaccard` uses set semantics (S-R33).
- `record::exists()` always returns a boolean (S-R32).
- `eval::surql` and `eval::gql` are denied by default and are not enabled by `--allow-all` (S-R32).
- `duration::set_*` and `object::matches` are parse errors (S-R33).
- `value::expect` is new (S-R31).

Removed:
- Like operators `~`, `!~`, `?~`, `*~`. Use `string::similarity::jaro(...) > 0.8` or `string::distance::osa(...)`.
- `ANALYZE` statement.
- `VERSION` clause on `CREATE` and `INSERT`.
- Stored closures. `SET closure = |$a| $a + 1` errors.
- Futures (`VALUE <future> {...}`). Use `COMPUTED` fields.

Unchanged in the sources I read (not listed as renamed in S-MIG): `time::now`, `string::lowercase`, `string::contains`, `search::score`, `count()`.

## 8. Other syntax changes

Source: https://surrealdb.com/docs/build/migrating/from-old-surrealdb-versions/2x-to-3x items 12 to 29.

- `GROUP` and `SPLIT` together are no longer allowed in one query.
- Optional chaining: `?` becomes `.?` (for example `$val.?.len()`).
- Identifiers that match keywords in `DEFINE`, `INFO FOR` and `REMOVE` must use backticks: ``DEFINE INDEX `select` ...``.
- `set` type deduplicates and sorts its items.
- `.id` idiom no longer has its old special behaviour. `.id.key_field` reads a field.
- Record IDs with numeric keys: `t:[1]`, `t:[1f]` and `t:[1dec]` map to one key.
- The `/key` HTTP endpoint rejects executable SurrealQL. Send queries to `/sql` or RPC (S-R31).

## 9. Rust SDK (3.x)

Source: https://surrealdb.com/docs/build/migrating/from-old-surrealdb-versions/2x-to-3x, https://surrealdb.com/releases/3.0, https://surrealdb.com/releases/3.3.

- Shared types moved to `surrealdb_types` (for example `surrealdb_types::*`) in 3.0.0.
- `SurrealValue` derive: 3.0.2 made deriving it more convenient in the SDK.
- Query futures are `Send` but no longer `Sync` (3.3.0-beta.1).
- Rust engines implement the `SurrealEngine` trait (3.3.0).
- `Datastore::transaction(..)` drops its lock-type argument. `Datastore::get_capabilities()` returns `Arc<Capabilities>`.
- The `strict` connection option is removed.
- `surrealdb-core` no longer re-exports `surrealdb-rpc`.
- `engine::any`: unverified. I could not find it in the sources I read. Check the 3.x crate docs before you use it.

## 10. Capability flags for `surreal start`

Source: https://surrealdb.com/docs/surrealdb/cli/start.

- `--deny-all` (`-D`) denies everything except what you allow.
- `--allow-funcs time,string,search,count` allows those families. Forms: `time` or `time::*` (family), `time::now` (one function). Comma separated.
- `--allow-net`, `--allow-experimental files,surrealism` (experimental values).
- Env vars: `SURREAL_CAPS_DENY_ALL`, `SURREAL_CAPS_ALLOW_FUNC`, `SURREAL_CAPS_ALLOW_NET`.
- See `docs/architecture/surreal-functions.md` for the function list the backend uses.
