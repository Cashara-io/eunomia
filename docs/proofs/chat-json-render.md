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

## Second review fixes

Verified on 2026-11-09. This completes verification of `e5dad770f5707957e9f174f27754e06b8d128ba7` and fixes the gaps found during that verification. [Exact commands](chat-json-render-second-review-commands.txt) and the two committed verification scripts reproduce the checks.

### Gaps closed

- Aggregate aliases were still model supplied. The input now accepts only an operation and optional property. The server generates `op_property` keys, or `count` without a property, and rejects model aliases. The tool schema, prompt, architecture example, integration tests and independent numeric audit agree on this contract.
- Model formatting could override a plain numeric column. The renderer now takes formats from server column metadata, preserves unitless counts, and uses each column's own currency in chart labels and tooltips. Explicit table cells and totals retain the server format. StatGroup retains missing typed measurements as `none` rather than silently dropping their tiles.
- The compatibility fixture did not exercise nanosecond differences. It now includes timestamps one nanosecond apart, plus the original subsecond cases.
- CI on `e5dad770` failed `committed_spec_matches_generated`. The OpenAPI document and generated frontend client are regenerated and the drift check is rerun without the update flag.

### Main compatibility provenance

`python3 scripts/chat-json-render/check_main_source.py` extracts `compute` directly from `origin/main`'s pinned commit `6116a00ff86fed992a917863e9a3515f70373b84`. It verifies the included Rust oracle against that source, ignoring whitespace. The only code changes are the function name and an unreachable arm for this branch's new median operation. Property parsing is unchanged from that main commit.

`CAPTURE_MAIN_METRICS=1` reruns that actual Rust implementation and recaptures `backend/tests/fixtures/pr192-main-metrics.json`. All 72 legacy aggregate results across twelve populated and empty cases match main. Numeric results are compared by `f64::to_bits`, with exact missing-value equality. These cases cover number, money, percent, date, datetime and case-insensitive text, including order-sensitive sums and nanosecond distinct counts. Median has separate assertions because main does not implement it.

### Guard removal evidence

`check_second_review_guards.py` removes one guard at a time, requires the named regression test to fail, rejects compilation failures as evidence, and restores the exact original source in `finally`. There are thirteen mutations, four backend and nine frontend. All thirteen failed their intended tests, and the restored checks passed.

| Item | Regression test | Guard removed | Mutation result |
| --- | --- | --- | --- |
| Words and aliases | `model_words_grammar_covers_all_prose`; `aggregate_names_are_server_generated` | Prose validation; rejection of model aggregate aliases | Failed as expected; restored |
| Missing numbers | Numeric accessor, chart-y, Meter minimum, zero recurrence and StatGroup renderer tests | Null preservation; required y with series; required minimum; ban on zero coercions; retention of missing tiles | Failed as expected; restored |
| Server formatting | Explicit table metadata and server-type renderer tests | Table format inheritance; unitless `total_rows`; server scalar format precedence | Failed as expected; restored |
| Datetime precision | `main_metric_fixture_preserves_existing_results` | Full stored datetime distinct key replaced with display JSON | Failed as expected; restored |
| Summation order | `main_metric_fixture_preserves_existing_results` | Sorting restored before sum and average | Failed as expected; restored |
| Drawing range | Unsafe finite chart renderer test | Safe drawing-range check | Failed as expected; restored |

The first count mutation exposed a weak assertion that checked only `A$` although the fixture used USD. The assertion now rejects USD too, and the same mutation fails. This survived-mutation attempt was not counted as a pass.

### Final results

- Clippy on all targets with warnings denied: passed.
- Final hardened backend run: 517 library tests, seven chat UI tests, 110 ontology tests, one golden-tools test and one round-trip test passed. The opt-in scale fixture and expensive ontology import fixture remain ignored, two tests total.
- OpenAPI drift and route coverage: two tests passed without `UPDATE_OPENAPI`.
- Frontend lint, `bunx tsc --noEmit`, production build and generated catalogue sync: passed.
- Focused renderer suite: eight tests, 111 assertions passed.
- Both requested Playwright files: nine tests passed on the final production build, with model access disabled. Catalogue checks cover light and dark at 1280 px and 390 px; the typed-values case also checks USD and EUR in the same tooltip.
- Main oracle recapture and exact parity: passed, including nanosecond distinct values. All thirteen guard mutations failed the intended regression and were restored before the final runs.
- CI for `e5dad770`: twelve checks passed; `backend` and `backend-hardened` failed only the stale OpenAPI check. The missing median enum value and generated client are now corrected, and the drift check passes locally. CI for the follow-up push has not yet been verified.

[Recorded result summaries and mutation failures](chat-json-render-second-review-results.txt).

The first browser attempt began before the isolated backend was ready. Every-tool failed its registration request and the first catalogue case timed out. After readiness succeeded through the frontend, the complete rerun passed. The failed startup log was retained locally. No product defect is claimed for that attempt.

The runtime used a disposable SurrealDB 3.3.5 on `8590`, this branch's backend on `8591`, and a production frontend on `3590`. Server model API settings were removed from the runtime environment. Every-tool completed without model access. No shared database or owner stack was used.

Machine free space started at about 14 GiB and stayed above the 6 GiB stop threshold. Every Cargo process used two jobs, no development debug information, no incremental compilation, an 8 MiB Rust stack and niceness 19. Only one Cargo process ran at a time. No `cargo fmt` was run. Teardown removed this checkout's backend target, node_modules and Next output, plus only this run's processes and disposable database.

## Third review fixes

The final branch is rebased onto main `42a45fc4d14931bd9c558f250d3ba443c29b653c`, after PR 192 and the documentation/CI-only PR 199. The last rebase left the backend, frontend and chat verification script trees byte-for-byte identical to the tested version; `source-trees.json` records their Git tree hashes. The newly introduced compliance catalogue check also passed in an isolated Python environment.

This follow-up replaces the prose number-word blocklist. Words such as "twice", "score", "First Nations" and "IV" are ordinary language and are not checked. Unicode compatibility forms, including superscript and circled digits, are normalised first, then decimal digits in every script are converted to ASCII. Digits in alphanumeric identifiers such as Q3 and COVID-19 are allowed. Other digit quantities must match resolved view values or applied server parameters, with thousands separators, currencies, percentages and displayed precision normalised. Errors identify the field. Applied dataset limits, cutoff dates and filters are included in resolved parameters so the check can accept them after resolution.

Metric loading once again omits stored unit overrides, as main did. Dataset loading alone retains units. The main compatibility fixture now includes an AUD property with a permitted stored USD value of 13. The API regression evaluates all six existing aggregates against its captured main results. The oracle comparison covers 78 results bitwise, including the earlier input-order and datetime-precision cases. The pinned main source comparison still passes.

The server returns explicit x, y and series for every chart. Omitted y is accepted only with one unambiguous numeric metadata column. The decision does not depend on whether a row has a measurement. The renderer refuses an unresolved column specification and never substitutes quantity for a missing amount. Shared numeric access rejects unsafe tiny values, and proportional totals reject zero before Pie, Donut, Treemap, Partition, Funnel and stacked charts draw.

### Third review guard mutations

`scripts/chat-json-render/check_third_review_guards.py backend|frontend` disables one fix, requires an assertion failure rather than a compilation failure, and restores the file in `finally`.

| Fix disabled | Regression | Result |
| --- | --- | --- |
| Prose digit matching | `model_digits_match_resolved_values` | Failed as expected, restored |
| Main metric projection without units | `metric_stored_unit_override_matches_main_fixture` | Failed as expected, restored |
| Unambiguous server y selection | `omitted_y_resolves_only_unambiguous_metadata` | Failed as expected, restored |
| Renderer requires resolved columns | `omitted y never substitutes an available measurement` | Failed as expected, restored |
| Shared tiny-value and positive-total guards | `proportional charts reject subnormal and zero totals` | Failed as expected, restored |

### Third review verification and live proof

- Final serial hardened backend run on the rebased implementation: 518 library tests, nine chat UI tests, 110 ontology tests, one golden-tools test, one round-trip test and two OpenAPI tests passed. The existing scale fixture and expensive ontology import fixture remain ignored, two in total.
- Final Clippy on all targets with warnings denied passed.
- Frontend lint, typecheck, production build, API regeneration and catalogue sync passed. The final renderer suite passed eleven tests and 199 assertions. Both requested Playwright files passed all nine tests with server model access disabled.
- All five guard mutations failed their intended assertions and were restored. The main oracle source check and all 78 bitwise compatibility comparisons passed.
- OpenAPI regeneration was run. Its generic UI envelope did not require a generated shape change; the final no-update drift test passed.

[Replay commands](chat-json-render-third-review-commands.txt). Detailed logs and earlier failed or interrupted attempts are included with the screenshots.

The throwaway SurrealDB 3.3.5 database used port 8595, the backend 8596 and frontend 3595. The shared database at 8000 and occupied ports 3600, 8600 and 3700 were not used. File buckets were enabled on the disposable database, with memory-backed document storage and stub embeddings. The CSV import created twelve entities and sixty values with no bad cells. The supplied launcher was copied privately for the real gpt-6-luna key; neither its contents nor session cookies are published.

| Question | Live result | Independently expected values |
| --- | --- | --- |
| Monthly trend | Line | Jan 200.75, Feb 370.75, Mar 240.50, Apr 225.25, May 280.50, Jun 270.25 AUD |
| Supplier breakdown | Donut | Coastal 510.50, Inland 431.25, Metro 646.25 AUD |
| Distribution | Histogram | Four bins with 4, 3, 3 and 2 values |
| Relationship | Scatter | All twelve quantity/amount pairs match |
| Ten percent what-if | Scenario | AUD 1588.00 to 1746.80, delta 158.80 |
| Table with totals | Table | AUD 1588.00 and quantity 49 |

All six final questions ran through the real chat service with gpt-6-luna after rebasing onto main. No chart resolver, metric projection or renderer changes followed this live rerun; the subsequent Unicode/date-label edge cases are covered by the final backend suite. They were submitted through the normal thread/message API, and their persisted histories were opened and captured in Helium. Each final question resolved one UI. The table question explicitly named the whole-dataset pointer, and its first result showed the correct footer.

The earlier browser-submitted six-question run is retained under `attempts/browser-pass/`. That run's scenario corrected an invalid source argument, and the table needed two follow-up prompts to replace its `/rows` binding. Those unsuccessful table displays are retained too. Final screenshots use the second fresh run. T3 preview could read the final answers but repeatedly timed out capturing snapshots, so native Helium captured the PNGs instead. Only proof tabs and windows were opened and closed.

`python3 -I scripts/chat-json-render/verify_numbers.py CSV ANSWER_DIRECTORY` passes 389 numeric and nullable field checks within 0.01. This audits all six final tool results and the applied dataset parameters. The earlier run also passed 673 checks, including its repeated table resolutions. All six final PNGs were opened and inspected. The final scenario image includes every changed row, and the table image includes its footer.

[Six screenshots, authentic receipts, CSV, numeric audit and verification logs](https://github.com/Cashara-io/eunomia/tree/pr-screenshots/chat-json-render-v4).

The first backend build needed test error-message and source-prose serialization corrections. A later run was stopped by the disk monitor when unrelated disk use took free space below 6 GiB. Its library, chat UI and golden-tools portions had passed; its interrupted ontology run is not counted as complete. The owned Rust target was removed. Free space subsequently recovered, and verification restarted. The owner then changed the pre-Cargo gate to wait below 7 GiB or while another Cargo runs, rechecking every three minutes for up to sixty minutes. Only this task's artefacts were removed. Earlier frontend typecheck and document-bucket setup failures are retained separately from the final checks. One restored parallel backend run failed the existing `gate2_a_long_question_caps_name_lookups_and_never_searches_memory_text` tracing assertion with zero spans. The earlier full run passed it; the final run uses one Rust test thread to avoid concurrent tracing collectors. No `cargo fmt` was run. All Cargo processes used two build jobs, no dev debug information, no incremental compilation, an 8 MiB Rust stack and niceness 19. Final cleanup removes this worktree's target, node_modules, .next, runtime processes and disposable container.
