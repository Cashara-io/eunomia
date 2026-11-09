# Chat generative UI verification

The implementation uses json-render 0.22.0 and visx 4.0.0. The catalog has 53 components, including 28 chart types. Sankey is included through `@visx/sankey` 4.0.0. The source contract is in [Chat generative UI](../architecture/chat-generative-ui.md).

## Local checks

[Exact commands](chat-json-render-commands.txt) record the environment and flags. Each cargo command used two jobs, no development debug information, no incremental compilation, an 8 MiB Rust stack and niceness 19. Disk space was checked before builds and remained above 6 GiB. No `cargo fmt` command was run.

- Clippy on all targets with warnings denied: passed.
- View validation and shared number-formatting unit tests: 12 passed.
- Dataset aggregation and time-bucket unit tests: 2 passed.
- Hardened `chat_ui` and `round_trips` integration tests: both passed. These cover approved money and percent properties, filters, link grouping, proposed metric status, cross-org refusal, strict mode, forged data, output limits, MCP schema exposure and the round-trip ceiling.
- Existing ontology integration tests: 110 passed, one pre-existing test ignored.
- Frontend lint, typecheck and production build: passed.
- Catalog and number-formatting Playwright checks: 11 passed. The catalog checks all components and chart types in light and dark at 1280 px and 390 px, including empty, one-row and null states. They check visible chart dimensions and fallback notices for unknown components, bad props and missing bindings.
- Every MCP tool Playwright check: passed with server model access disabled, matching CI. With a real server key enabled, it failed at the existing ontology-coverage assertion before reaching the added UI call. The implementation does not change ontology coverage.

The full backend suite was not run locally because of the disk constraint. CI runs it. No SurrealDB migration is needed. Docker image builds were not run locally; both generated catalog files are inside their respective build contexts and their equality is tested.

## Bundle measurement

The same script measures the unique JavaScript chunks referenced by the chat route's client-reference manifest. It sums raw bytes and independently gzipped bytes for those chunks.

| Build | Raw bytes | Gzipped bytes |
| --- | ---: | ---: |
| `origin/main` at `6116a00f` | 474737 | 139065 |
| Feature branch | 481963 | 141076 |

The change adds 7226 raw bytes and about 2 KiB gzipped to the route references. The json-render UI and visx charts are dynamic imports. Recharts remains only in Settings usage charts.

## Real chat proof

A fresh Docker SurrealDB 3.3.5 ran on `127.0.0.1:8580`, the branch backend on `8581` and frontend on `3580`. The shared database on `8000` was not used. A disposable org imported and approved a 12-row CSV through the ontology. Its properties were date, supplier, money in AUD, quantity and percent. The real in-app chat used `gpt-6-luna`.

The six questions requested a monthly trend, supplier breakdown, distribution, relationship, what-if and detail table with totals. The resulting blocks were Line, Donut, Histogram, Scatter, Scenario and Table. The server supplied the monthly totals, supplier totals, histogram bins, row values, scenario values and table totals.

`python3 -I scripts/chat-json-render/verify_numbers.py CSV ANSWER_DIRECTORY` recomputes the numeric fields from the CSV, including aggregate totals, counts, histogram bins, quartiles and hypothetical changes. The proof branch contains the CSV, six chat histories and screenshots under `chat-json-render/`. No keys or session cookies are included. Every screenshot was inspected. Chart sizing, axis label alignment and table footer labels were corrected during that review.

The approved total amount is AUD 1588.00 and total quantity is 49. Increasing amounts by ten percent gives AUD 1746.80, with a change of AUD 158.80. All six answers matched the CSV recompute within 0.01, covering 335 numeric fields.

Screenshots and receipts: [proof directory](https://github.com/Cashara-io/eunomia/tree/pr-screenshots/chat-json-render), [numeric audit](https://github.com/Cashara-io/eunomia/blob/pr-screenshots/chat-json-render/numeric-audit.txt).

## Review fixes

The 9 November review round starts from `e5b725097f2b6ef820726eda680b0ffcae3e926f` and keeps its content. The earlier sections record earlier runs. This section records the review verification. [Commands](chat-json-render-review-commands.txt) use two cargo jobs, no dev debug information, no incremental compilation, an 8 MiB stack and niceness 19. Disk was checked before builds and stayed above 8 GiB. No `cargo fmt` was run.

The review fixes restrict numeric slots to server-typed numeric fields, reject model-written digits, require chart-specific shapes and show unavailable measurements without substituting zeros. Dataset and metric use the same UTC cutoff and aggregate arithmetic, including case-insensitive distinct and empty sum zero. Currency survives aggregation and rendering; mixed-unit columns are rejected. Dataset filter validation now reuses those scoped property rows rather than reloading every property of the kind. The ten-filter limit runs before metadata queries. The remaining renderer gaps were currency inference when a chart omits `y`, including a `/rows` binding, and money units leaking into scalar counts. Both now have browser assertions. Numeric values use the UI font with tabular figures. StatGroup uses responsive Stat tiles. Sankey labels stay inside the plot.

The generated catalogues were regenerated and both remained identical to their checked-in copies. The catalogue sync test passed.

### Guard mutations

Every experiment changes one guard, runs the named regression and requires a test assertion failure, then restores the file. Compilation failures do not count. The replayable script is `scripts/chat-json-render/check_guards.py`. The combined access test is now split so each refusal has its own test. The strict-kind fixture has approved properties and first succeeds before strict mode is enabled.

| Test in `backend/tests/chat_ui.rs`, unless qualified | Guard disabled | Result |
| --- | --- | --- |
| `dataset_filter_limit` | Ten-filter limit before metadata | Test failed |
| `dataset_denies_same_org_nonmember` | Dataset vault authorisation | Test failed |
| `dataset_denies_vault_restricted_token` | Credential vault restriction | Test failed |
| `dataset_hides_cross_vault_links` | Outgoing target vault filter | Test failed |
| `dataset_strict_kind_guard` | Approved-kind requirement in strict mode | Test failed |
| `views::ui::tests::numeric_bypass_matrix` | Typed Stat scalar | Test failed |
| Same matrix | Typed chart and formatted table columns | Test failed |
| Same matrix | Distribution path and shape | Test failed |
| Same matrix | Model-written digit refusal | Test failed |
| `dataset_cutoff_currency_and_mixed_units` | Mixed-unit refusal | Test failed |
| Same test | Historical cutoff | Test failed |
| `ontology::metric::tests::shared_aggregate_semantics_nulls_empty_and_case` | Case folding | Test failed |
| Same test | Empty sum zero | Test failed |
| `dataset_resolves_real_typed_values_and_mcp_catalog` | Numeric aggregate datatype | Test failed |
| Playwright: `typed money keeps its currency and missing values are unavailable` | Missing-measurement drawing guard | Test failed |

The cutoff regression includes Melbourne midnight on the day after a bare UTC date and excludes a later value. It compares dataset and metric counts at the same cutoff. Currency checks cover USD metadata on rows and aggregates, plus mixed AUD/USD refusal. The aggregate parity matrix compares every supported aggregate on both numeric and text inputs where permitted, including empty selections.

### Scale proof

A fresh namespace and throwaway vault on the isolated SurrealDB 3.3.5 server at `127.0.0.1:8590` contains 5,000 entities and 50,000 property-history rows. Two properties have five versions each. The real `show_view` dataset path took 1,164 ms for grouping and 2,152 ms for monthly bucketing. The grouping produced 5,000 groups and explicitly marked the 500 returned rows as truncated. Monthly bucketing returned all twelve groups. The independently expected sum, 12,517,500, and total count, 5,000, were also asserted. The initial run measured 1,196 ms and 2,121 ms; both logs are retained.

`EXPLAIN` uses `entity_vault_kind_idx` for entity selection with a limit of 5,001. Latest values use a backward `IndexScan` on `entity_property_entity_idx`, constrained by entity, property and cutoff. Neither plan uses a table scan. The plan retains a per-pair `SortTopKByKey` for `as_of, created_at` tie-breaking. This bounds entity/property lookups and avoids unrelated vault history, but does not make cost independent of history length within one selected pair. Metadata reads avoid the old full ontology profiling path. The scale test checks row counts, both named indexes, output groups and absence of table scans.

### Fresh tests and live chat

- Backend integration: seven chat UI tests, 110 ontology tests, one every-tool shape test and the round-trip guard passed. The ontology suite's pre-existing expensive import test is ignored. The scale proof is separately opted in.
- Frontend lint, typecheck and production build passed. Both Playwright files passed, nine tests, with server model access disabled, matching CI.
- Removing the frontend guard failed at the missing-data notice. After restoration, all nine browser tests passed again. A later restart check hit `ECONNREFUSED` before the rebuilt backend was ready; the eight UI tests passed and every-tool passed again after checking `/readyz`. This startup attempt is retained in the check logs.
- Backend library: 514 passed. Hardened integration: seven chat UI tests, 110 ontology tests, one every-tool shape test and the round-trip guard passed. Clippy on all targets with warnings denied passed.

The live check used the disposable database at 8590, backend at 8591 and frontend at 3590. It never used the shared database at port 8000. A fresh disposable account uploaded the exact CSV from `6b4e265861a6a01f362cb1c9041b39b1e44e73cf`, approved the kind, properties, source maps and total metric, then imported twelve rows and sixty property values with no bad cells. All six questions used `gpt-6-luna`, without changing models.

| Question | Actual block | Recomputed answer |
| --- | --- | --- |
| Monthly trend | Line | Jan 200.75, Feb 370.75, Mar 240.50, Apr 225.25, May 280.50, Jun 270.25 AUD |
| Supplier breakdown | Donut | Coastal 510.50, Inland 431.25, Metro 646.25 AUD |
| Distribution | Histogram | Four bins containing 4, 3, 3 and 2 values |
| Relationship | Scatter | All twelve quantity/amount pairs match the CSV |
| Ten percent what-if | Scenario | AUD 1,588.00 to 1,746.80, delta 158.80 |
| Table with totals | Table | AUD 1,588.00 and quantity 49 |

`python3 -I scripts/chat-json-render/verify_numbers.py CSV ANSWER_DIRECTORY` matched 379 numeric fields within 0.01. This covers server rows, aggregates, totals, distributions and scenario fields. All six answer screenshots were opened and inspected, including the formatted scenario changes and table totals. All four regenerated catalogue contact sheets were inspected, with full-size checks of the phone StatGroup and phone/desktop Sankey labels.

[Six answer screenshots, four catalogue sheets, CSV, receipts and numeric audit](https://github.com/Cashara-io/eunomia/tree/pr-screenshots/chat-json-render-v3). Guard failures, scale plans and final check logs are published there too. No keys, launch scripts or session cookies are published. Screenshots are uploaded through a temporary detached worktree. Cleanup removes this worktree's backend target, frontend node_modules, Next output, servers and disposable container.
