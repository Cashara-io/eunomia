# Ontology 2 review fixes

## Changes

The profile, directory values, metric inputs and chat datasets now use the same latest-point selection. The database orders native `as_of`, `created_at` and record IDs descending, applies one cutoff, and only then serialises timestamps. Profiles use that cutoff for their scoped metrics too. The old profile query is removed. A regression fixture includes whole and fractional seconds, equal timestamps, record-ID ties and a future point.

Enum filters pass the approved property's allowed options to the existing parser. A valid option returns its object; an unlisted option returns 400.

Both object pages use `lib/numbers.ts` and its decimal rounding with the server datatype and unit. Percent columns show two decimal places and keep zeros. Money keeps two places, except non-zero sub-cent amounts retain enough decimal places to remain visible. For example, AUD 0.0049 remains AUD 0.0049. Ordinary numbers show two places; integer counts remain integers.

## Paging and indexes

Migration 0040 adds a name-order index and typed property-order indexes. Latest-point probes retain the existing entity/property/as-of index and native sorting by as-of, recorded-at and record ID. The directory queries the database for the page, rather than transferring every candidate and property history into Rust. Typed pages traverse histories in value order and check each candidate against the shared latest-point selection. Missing values form a separate last phase. Typed filters narrow through a typed value index before name sorting. Exact totals stay in database aggregates.

Scalar closure parameters are deliberate. SurrealDB 3.3.5 used a table scan for a subquery bound directly to `$parent.entity`; scalar entity/property parameters make that lookup use the latest-point index. The captured EXPLAIN plans include actual row counts. Datetime covering indexes are deliberately excluded: SurrealDB 3.3.5 returned a whole second after its fractional value through the ordered index path. The existing pair index plus native sorting preserves fractional ordering. Date-property sorting also stays in the database with native datetime comparisons.

Deep offsets still traverse index entries. Substring searches, missing-value filters, large historical duplication and different sort/filter properties can require more candidate work. This does not claim constant-time arbitrary searches. No cached or precomputed value projection is introduced.

## Measurement method

The isolated fixture contains 18,878 objects, including 12,000 stock records and 6,240 weekly sales records. Measurements include HTTP, response reads, JSON decoding and exact totals. Each case has 50 cold and 50 warm samples. A cold sample is the first object endpoint read after restarting both the backend and the hardened SurrealDB 3.3.5 database process. The same persisted RocksDB fixture is reopened without reseeding, so fixture writes do not prime the database caches. A warm sample follows a priming request in one backend/database process. There is no claim of an OS disk-cache flush. The database runs in a disposable Docker container on port 18644. Raw samples and nearest-rank p95 are retained.

The protected services on ports 3600, 8600, 3700 and 18470 to 18473 are outside this work. The new runtime uses 13860 and 18860. A disposable SQL investigation container uses 18643.

## Hardened capability regression

The original hub test constructed record IDs with `type::record`, which production's function allow-list refuses. The original failure was reproduced against a disposable SurrealDB 3.3.5 server with the exact production capabilities. Hub, scale-fixture and fractional-tie setup now bind record IDs built in Rust. The new product query also initially used `record::tb`; it now uses allowed string operations instead. A built-in-kind typed-page regression covers that path. No `type::record`, `type::thing` or `record::tb` calls remain in the changed object query or test files. Product query functions added here belong to the allowed `array`, `string` and `math` families.

## Validation

Before every Cargo step, `df -h /` and `pgrep -x cargo` are checked. The launcher waits three minutes between checks while another Cargo process exists or less than 7 GiB is free, for at most two hours. Commands use `CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 RUST_MIN_STACK=8388608 nice -n 19`. No `cargo fmt` is run.

## Completed checks

- Eight touched backend suites passed: 162 cases, five explicitly ignored. These cover objects, ontology metrics, chat datasets, migrations, dynamic SQL builders, store gates and API contracts.
- All eight object cases also passed on the real hardened SurrealDB 3.3.5 server. The original hub failure was reproduced before its fix. The new built-in-kind case proves the product path uses allowed functions.
- All-target Clippy passed with warnings denied.
- Reversing the shared latest order made the fractional-tie guard detect 10 instead of 40. Omitting enum options made the valid-blue filter test fail with 400. Reverting number formatting failed both new formatter cases. Every mutation was restored.
- Frontend lint passed, including ten numeric/UI cases. Route type generation, TypeScript checking and the production build passed.
- Both real-backend Playwright tests passed in 9.1 seconds. All 16 regenerated PNGs were opened and inspected, in both themes at 1440 and 390 px. The captures show 58.50% and AUD 0.0049.
- A manifest of 487 source, manifest, migration and harness files matches runtime revision `3630896e94ad88eec5b51450e9c7c8b8959d85c8`.

The actual fixture EXPLAIN reports 25 index entries for a 25-row name page. A numeric sort scans 400 entries in fixed engine batches and returns 25 rows. The filter uses the numeric `>= 11900` range, reads its 100 candidate entries and returns 25 rows. Latest-point selection uses the existing pair index and native `SortTopK`, with one history row for that fixture point. None of these captured plans contains a table scan.

## Final timings and uploaded evidence

Milliseconds, 50 cold and 50 warm samples per case. Cold restarts both backend and database processes; it retains the OS disk cache. The same saved 18,878-object fixture is reopened without reseeding. Measurements include HTTP, response reads, JSON decoding and exact totals. p50 and nearest-rank p95 were independently recomputed from all 1,000 raw samples. This hardened Docker/RocksDB run differs from the original embedded-memory run, so a direct speedup ratio is not claimed.

| Read | Cold p50 | Cold p95 | Warm p50 | Warm p95 |
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

[All 16 screenshots and evidence index](https://github.com/Cashara-io/eunomia/blob/b96251a47fc1b27af4a1ceea4ddb70f4ec0134a1/ontology-2-v2/README.md), [raw samples](https://github.com/Cashara-io/eunomia/blob/b96251a47fc1b27af4a1ceea4ddb70f4ec0134a1/ontology-2-v2/performance.json), [EXPLAIN plans](https://github.com/Cashara-io/eunomia/blob/b96251a47fc1b27af4a1ceea4ddb70f4ec0134a1/ontology-2-v2/explain.json), [test and guard summary](https://github.com/Cashara-io/eunomia/blob/b96251a47fc1b27af4a1ceea4ddb70f4ec0134a1/ontology-2-v2/checks.txt).

The upload contains 35 files, including raw test, mutation, lint, build and hardened-run logs. Local Git blob hashes were checked against the uploaded blobs and remote tree. These results supersede the original 12-sample timings. No merge or deployment was performed.

After verification, the temporary services were stopped and this worktree's `backend/target`, `frontend/node_modules` and `frontend/.next` were deleted. The source hash manifest still matched after cleanup. Only this evidence document changed after the runtime revision.
