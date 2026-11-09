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
