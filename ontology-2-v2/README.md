# Ontology 2 review fixes

Runtime source: `3630896e94ad88eec5b51450e9c7c8b8959d85c8`. The [source hash manifest](source-sha256.json) covers 487 files.

Latest values share the metric/chat loader, with native datetime ordering and one cutoff per request. Enum filters validate against approved options. Both object pages use shared decimal formatting: two places for percent and ordinary money, extra precision for non-zero sub-cent money. The captures show 58.50% and AUD 0.0049. Directory sorting, filtering and limits execute in the database.

## Screenshots

All 16 PNGs were opened and inspected. The table and Properties capture demonstrate sub-cent money. Numbers and Sources keep the independent Barista example: Decimal arithmetic `(25.90 - 14.90) * 100 = AUD 1100.00`, one object, source row 3. Properties uses Basket range 002, whose price was explicitly updated to AUD 0.0049 without a retained source, and says so.

| View | Theme | Width | PNG dimensions | Screenshot |
| --- | --- | ---: | --- | --- |
| objects | light | 1440 | 1440 x 1586 | [objects](objects-light-1440.png) |
| profile | light | 1440 | 1440 x 1070 | [profile](profile-light-1440.png) |
| numbers | light | 1440 | 1440 x 990 | [numbers](numbers-light-1440.png) |
| sources | light | 1440 | 1440 x 934 | [sources](sources-light-1440.png) |
| objects | light | 390 | 390 x 1739 | [objects](objects-light-390.png) |
| profile | light | 390 | 390 x 1219 | [profile](profile-light-390.png) |
| numbers | light | 390 | 390 x 1139 | [numbers](numbers-light-390.png) |
| sources | light | 390 | 390 x 1082 | [sources](sources-light-390.png) |
| objects | dark | 1440 | 1440 x 1586 | [objects](objects-dark-1440.png) |
| profile | dark | 1440 | 1440 x 1070 | [profile](profile-dark-1440.png) |
| numbers | dark | 1440 | 1440 x 990 | [numbers](numbers-dark-1440.png) |
| sources | dark | 1440 | 1440 x 934 | [sources](sources-dark-1440.png) |
| objects | dark | 390 | 390 x 1739 | [objects](objects-dark-390.png) |
| profile | dark | 390 | 390 x 1219 | [profile](profile-dark-390.png) |
| numbers | dark | 390 | 390 x 1139 | [numbers](numbers-dark-390.png) |
| sources | dark | 390 | 390 x 1082 | [sources](sources-dark-390.png) |

## EXPLAIN

The actual 18878-object fixture uses `entity_vault_kind_name_idx` with 25 entries for a 25-row name page. Numeric sorting uses `entity_property_num_page_idx`, 400 entries in fixed engine batches, then 25 returned rows. Filtering uses its `>=11900` range, 100 candidate entries, then 25 returned rows. Latest-point selection uses `entity_property_entity_idx` and native `SortTopK`, one history row for the selected fixture point. There are no TableScan operators in these captured plans. [Full plans and runtime metrics](explain.json).

The existing latest-point ordering remains native datetime. Datetime covering indexes are deliberately absent because SurrealDB 3.3.5 returned fractional seconds in the wrong order through that index path. Date-property ordering uses native database sorting. Deep offsets, substring searches, missing-value filters, historical duplication and different sort/filter properties can require more candidate work. Exact totals remain database aggregates. This is not a constant-time arbitrary-search claim.

## Timings

Serial loopback HTTP including response read, JSON decode and exact counts. SurrealDB 3.3.5 with production hardened capabilities and RocksDB on a disposable Docker container. Cold: first object endpoint after independently restarting both the database and backend for every case/sample, reopening the same 18878-object fixture without reseeding. Warm: one backend/database process, one priming request per case then 50 reads. No OS disk-cache flush.

18,878 objects in the measured space: 300 products, 12,000 stock records, 6,240 weekly sales, 338 suppliers. Each case has 50 cold and 50 warm samples. Includes HTTP, response reads, JSON decode and exact counts. p50 is the median; p95 is nearest-rank. Both were independently recomputed from [all 1000 raw samples](performance.json). The shared Mac and the Docker boundary differ from the original embedded-memory measurements, so these numbers should not be used as a direct speedup ratio.

| Read | Cold p50 ms | Cold p95 ms | Warm p50 ms | Warm p95 ms |
| --- | ---: | ---: | ---: | ---: |
| kinds | 96.22 | 107.52 | 41.77 | 43.04 |
| products | 54.89 | 63.35 | 19.50 | 22.05 |
| stock-name | 84.00 | 91.31 | 39.09 | 40.73 |
| stock-page-2 | 81.14 | 92.86 | 39.49 | 42.80 |
| stock-deep | 107.57 | 118.31 | 63.42 | 66.39 |
| stock-sort | 88.35 | 101.06 | 46.51 | 48.57 |
| stock-filter | 58.61 | 69.53 | 28.82 | 30.23 |
| sales | 63.31 | 74.69 | 27.31 | 29.53 |
| sales-sort | 71.03 | 81.13 | 34.70 | 36.77 |
| profile | 48.00 | 58.93 | 28.23 | 38.92 |

## Checks and hardened capabilities

Source: 3630896e94ad88eec5b51450e9c7c8b8959d85c8
Backend: 162 passed, 5 explicitly ignored across eight touched suites (ontology_objects, ontology, chat_ui, migrations, dynamic_queries, store, openapi, openapi_responses).
Hardened: all 8 object cases passed on a real SurrealDB 3.3.5 server, including original CI failure and built-in typed pages. Two ignored cases are the deliberate guard and fixture server.
Latest-order guard: deliberately ascending as_of selects 10 instead of 40; expected panic detected. Restored.
Enum allowed-values guard: omission makes valid blue return 400; regression fails. Restored.
Numeric guard: legacy variable precision/sub-cent rounding fails both new formatter tests. Restored.
Frontend lint: 10 numeric/UI cases; next typegen before typecheck; production build passed.
Playwright: 2 real-backend tests passed; 16 full-page PNGs captured and all inspected (light/dark,1440/390).
Clippy: all-targets with -D warnings passed.
Cargo guard: df -h / and pgrep -x cargo before every step; wait 180s while another Cargo exists or free space<7GiB, max 2h. CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 RUST_MIN_STACK=8388608 nice -n 19. No cargo fmt.
Performance: 10 cases, each 50 cold+50 warm; p50/p95 independently recomputed from raw samples. Correctness checked on every response. Cold restarts DB+backend, no OS disk-cache flush.
Source manifest: 487 source/manifest/migration/harness files match the runtime revision.
Protected service ports were not modified.

The original hardened failure was reproduced: `type::record` is refused. Test IDs are now built in Rust and bound. The new product path also avoids disallowed `record::tb`, using allowed string operations. A built-in-kind regression exercises this path. Changed object queries add functions only from the allowed array, string and math families. The real server used:

```
surrealdb/surrealdb:v3.3.5 start --user root --pass root --deny-all --allow-rpc --allow-http --allow-arbitrary-query=system --allow-funcs=time,string,search,count,array,vector,math,file,type::file --allow-experimental=files --query-timeout=60s --transaction-timeout=60s rocksdb:/data/proof
```

These are disposable test credentials. The production stacks on 3600, 8600, 3700 and 18470 to 18473 were not modified. Local proof servers used 13860, 18860, 18643 and 18644.
