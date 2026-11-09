# Ontology objects and profiles, PR 2

The original results below describe head 6232c41c. The [review fixes and replacement evidence](ontology-2-review-fixes.md) supersede its paging, formatting, latest-value and timing details.

## What was exercised

The production Next.js build talks to this branch's real Axum router and an isolated embedded SurrealDB fixture on ports 13760 and 18760. The original Banksia services on 3600, 8600 and database port 8000 were not modified. No real login or production data was used.

The fixture has 18,878 entities: 300 CSV-imported products, 12,000 stock records, 6,240 weekly sales records and 338 suppliers. Products have money, percentages, dates and unit counts. Both large record kinds have typed unit values, including zero. A product has a source CSV row, incoming relation, related memory and approved metrics. The fixture can be started from `backend/tests/ontology_objects.rs` with the ignored `serve_banksia_sized_fixture` test. It writes non-secret IDs to `/private/tmp/eunomia-ontology2/fixture.json`.

## API behaviour

- `GET /api/vaults/{vault_id}/ontology/objects`: without a kind, returns lightweight kind counts. With a kind, returns approved property columns, typed latest values, named approved links and batched source counts. Search, typed filters and stable sorting run on the server before the page is sliced. Default page size is 25, maximum 100. Missing values remain null and sort last.
- `GET /api/vaults/{vault_id}/ontology/objects/{id}`: returns approved properties, entity-scoped approved metrics, source records/documents, incoming/outgoing link counts, recorded writes and related memories. History, links and memories use bounded pages of 100 with an offset and explicit continuation flags.

Both reads use the existing membership, action scope and token vault restrictions. Cross-space object IDs return 404. No migration was needed. The shared latest-value loader is reused for page values and scoped metric inputs.

The list reads candidate identities and only the sort/filter property histories in batches, then loads visible values for the returned page. It avoids detail HTTP requests per row. It is not a precomputed database projection; timings below describe the measured fixture size.

## Checks

Frontend commands, from `frontend/`:

```sh
bun install --frozen-lockfile
bun run lint
npx next typegen
npx tsc --noEmit
BACKEND_INTERNAL_URL=http://127.0.0.1:18760 bun run build
E2E_BASE_URL=http://localhost:13760 ONTOLOGY_OBJECTS_FIXTURE=/private/tmp/eunomia-ontology2/fixture.json npx playwright test tests/ontology-objects.spec.ts --workers=1
```

Before every Cargo command, available disk space and other Cargo processes were checked. Commands waited in three-minute intervals when another Cargo process existed or space was below 7 GiB. Every command used:

```sh
CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0 RUST_MIN_STACK=8388608 nice -n 19 cargo clippy --all-targets -- -D warnings
```

The successful regression command, with the environment and `nice` prefix above, was:

```sh
cargo test --test ontology_objects --test store --test openapi --test openapi_responses --test ontology --test chat_ui -- --test-threads=1
```

After the final conflict-query and timestamp changes, the endpoint, store and contract cases were rerun:

```sh
cargo test --test ontology_objects --test store --test openapi --test openapi_responses -- --test-threads=1
```

The touched suites are `ontology_objects`, `ontology`, `chat_ui`, `store`, `openapi` and `openapi_responses`. Clippy uses `clippy --all-targets -- -D warnings`. No `cargo fmt` was run.

Endpoint tests cover paging, stable sorting, typed equality/ranges, fractional timestamps, missing versus zero/false, exact source records, property states, object-only metrics, approved links, a 5,002-link hub, cross-space IDs, non-members, wrong scopes and wrong-vault tokens. The browser journey covers paging/search reset, typed formatting and sorting, refreshed values, profile inputs, sources, related objects and the People & topics return action with the same space.

### Permission guard mutations

The membership test temporarily removes `user = $user` from `vaults.membership_active`, proves all three aggregate reads then expose the victim space, and restores the query using a drop guard.

A separate local mutation removes `check_vault(vault)?` from `authz::authorize`. The restricted-token test fails with `[403, 403, 403, 200, 200, 200]` instead of six 403 responses. The source guard is restored after the run. Mutation code is never committed. Removing only the action-scope call in `authz::authorize` left all six reads refused, because the HTTP gate independently checks scopes. The separate shared scope-predicate mutation is recorded with the final results below.

## Provenance limits and PR 3

A kind badge approves its definition, not each value. Missing, conflicting, stale and unavailable values have separate states. Stale means the retained source record changed after the value was written; there is no invented age threshold.

Legacy imports retain the document but not the original row number or document revision. Sources show the exact current document row only when its name match is unique, with that limitation stated beside it. They do not claim this is the historical imported revision. Unavailable records remain unavailable. Existing import paths can update a value in place, so Timeline explicitly describes its incomplete recorded-write history and does not invent reviews or import events.

PR 3 remains the revisioned proposal/decision/value event record, durable human reasons, live activity feed, pending counts, reconnection and revision preconditions.

## Final results

138 backend cases passed across the six touched suites, with three explicitly ignored fixture/large-import cases. The final object, store and contract suites passed again after the query refinements. All-target Clippy passed with warnings denied. Frontend lint (including eight numeric formatting cases), route type generation, TypeScript checking and production build passed. Both Playwright tests passed in 13.1 seconds on the final frontend build.

The shared `Caller::allows` scope predicate was then mutated to allow `memory:read`. The endpoint permission test failed with `[200, 200, 200, 403, 403, 403]`, proving it detects the scope bypass as well as the vault restriction bypass. All mutations were restored before the final successful tests and runtime. A hash manifest verified that the screenshot/runtime backend matches all 188 backend source, manifest and harness files used to build it.

### HTTP timings

The process was warm from browser verification. Each read has 12 serial loopback HTTP samples, including response reads and JSON parsing. The first is reported separately. Median and nearest-rank p95 use the next 11 samples, so p95 is their maximum. This is an embedded SurrealDB fixture on a shared Mac, not a production latency guarantee.

| Read | Matching objects | First (ms) | Median (ms) | p95 (ms) |
| --- | ---: | ---: | ---: | ---: |
| kinds | n/a | 305.97 | 303.70 | 305.10 |
| products | 300 | 76.55 | 76.19 | 77.88 |
| stock-name | 12000 | 256.58 | 254.53 | 255.11 |
| stock-page-2 | 12000 | 254.32 | 253.81 | 255.64 |
| stock-deep | 12000 | 255.22 | 254.28 | 256.09 |
| stock-sort | 12000 | 601.68 | 597.98 | 603.01 |
| stock-filter | 100 | 771.42 | 770.22 | 775.35 |
| sales | 6240 | 142.94 | 142.51 | 145.79 |
| sales-sort | 6240 | 322.57 | 322.19 | 324.32 |
| profile | n/a | 29.08 | 27.08 | 28.16 |

The independent source-row calculation is `(25.90 - 14.90) * 100 = AUD 1,100.00`. The evaluator returns exactly that amount, one contributing object and only that object ID. The source row is 3.

Browser navigation to expected content at 1440 px: median 114 ms, maximum 830 ms across eight captures. These include navigation and rendering and are separate from HTTP endpoint timings.

Browser navigation to expected content at 390 px: median 108 ms, maximum 820 ms across eight captures. These include navigation and rendering and are separate from HTTP endpoint timings.

### Uploaded evidence

All 16 PNGs were opened and inspected. Source revision: `4325441c207963d405123c69420f4877ade49a5a`. Upload commit: `299a3e33fcf99ecb185a5929a034017e99fd80c2`. The remote Git tree matches the local hashes of all 19 uploaded files.

- [All 16 screenshots and evidence index](https://github.com/Cashara-io/eunomia/blob/299a3e33fcf99ecb185a5929a034017e99fd80c2/ontology-2/README.md)
- [Directory, light, 1440 px](https://github.com/Cashara-io/eunomia/blob/299a3e33fcf99ecb185a5929a034017e99fd80c2/ontology-2/objects-light-1440.png)
- [Directory, dark, 390 px](https://github.com/Cashara-io/eunomia/blob/299a3e33fcf99ecb185a5929a034017e99fd80c2/ontology-2/objects-dark-390.png)
- [Properties, light, 1440 px](https://github.com/Cashara-io/eunomia/blob/299a3e33fcf99ecb185a5929a034017e99fd80c2/ontology-2/profile-light-1440.png)
- [Numbers, dark, 390 px](https://github.com/Cashara-io/eunomia/blob/299a3e33fcf99ecb185a5929a034017e99fd80c2/ontology-2/numbers-dark-390.png)
- [Sources, light, 390 px](https://github.com/Cashara-io/eunomia/blob/299a3e33fcf99ecb185a5929a034017e99fd80c2/ontology-2/sources-light-390.png)
- [Raw HTTP samples](https://github.com/Cashara-io/eunomia/blob/299a3e33fcf99ecb185a5929a034017e99fd80c2/ontology-2/performance.json)
- [Browser navigation timings](https://github.com/Cashara-io/eunomia/blob/299a3e33fcf99ecb185a5929a034017e99fd80c2/ontology-2/render-timings.json)
