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

Results and evidence links are recorded below after the final runs. Before every Cargo step, `df -h /` and `pgrep -x cargo` are checked. The launcher waits three minutes between checks while another Cargo process exists or less than 7 GiB is free, for at most two hours. Commands use `CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 RUST_MIN_STACK=8388608 nice -n 19`. No `cargo fmt` is run.
