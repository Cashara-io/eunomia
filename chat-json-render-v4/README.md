# Chat UI third review proof

Branch: `fix/chat-ui-review-3`, based on latest main `42a45fc4d14931bd9c558f250d3ba443c29b653c`, which includes PR 192 and the documentation/CI-only PR 199. The final rebase preserved the tested backend, frontend and chat verification script trees exactly. See `source-trees.json`.

Final source head: 679d69e57ac48fbadab5a41947c90be3b70a0d35

The six final receipts are authentic persisted gpt-6-luna chat histories from the real thread/message API. Each produced one resolved UI. All six final images were captured and inspected in native Helium after T3 preview snapshot capture repeatedly timed out. Images include the chart or scenario and table footer. No chart resolver, metric projection or renderer changes followed the live rerun. Final Unicode/date-label edge cases are covered by the backend regression suite. No keys, cookies or launchers are included.

The CSV SHA256 is `db20675d62816b32ba3acb85eb89d15d1fa9eebd0f1b42a6edc56fc96fdea65e`. It imported as twelve entities and sixty property values without bad cells. The throwaway SurrealDB 3.3.5 server used 8595, backend 8596 and production frontend 3595. The shared database at 8000 and occupied ports 3600, 8600 and 3700 were not used. Document storage used the database's memory bucket. Embeddings used the local stub, while real chat used gpt-6-luna.

| Answer | Block | CSV result |
| --- | --- | --- |
| 1 | Line | Monthly AUD sums 200.75, 370.75, 240.50, 225.25, 280.50, 270.25 |
| 2 | Donut | Coastal 510.50, Inland 431.25, Metro 646.25 AUD |
| 3 | Histogram | Bin counts 4, 3, 3, 2 |
| 4 | Scatter | All twelve quantity/amount pairs |
| 5 | Scenario | AUD 1588.00 to 1746.80, delta 158.80 |
| 6 | Table | AUD 1588.00 and quantity 49 |

The final audit matches 389 numeric and nullable fields within 0.01, including the applied dataset parameters. Run the repository's `scripts/chat-json-render/verify_numbers.py source.csv .` from this directory, using the script from the source branch.

`attempts/browser-pass/` retains an earlier six-question browser-submitted run. Its scenario corrected an invalid source argument. Its table needed two follow-up prompts to replace a `/rows` binding with a whole-dataset binding. That run passed 673 field checks because it includes all three table resolutions. The final six-question run uses an explicit pointer hint in its table question and succeeds on the first attempt. Other initial captures and setup failures are retained separately.

- Clippy, all targets, warnings denied: passed.
- Serial hardened backend: 518 library, nine chat UI, 110 ontology, one golden-tools, one round-trip and two OpenAPI tests passed. Two existing opt-in fixtures remain ignored.
- Frontend lint, typecheck, production build, catalogue sync and API generation passed.
- Renderer: eleven tests, 199 assertions passed. Both requested Playwright files: nine passed with model access disabled.
- OpenAPI was regenerated and the final drift check passed without an update flag.
- All guard mutations failed their intended assertions and were restored. Main compatibility: 78 bitwise results matched.

Every requested fix was disabled and its intended regression failed, with sources restored afterward. The mutation logs cover prose digits, metric unit parity, server column resolution, renderer column resolution, and proportional range/total guards. The main compatibility fixture includes the AUD property with a permitted USD value of 13 and compares 78 existing aggregate results bitwise.

An interrupted disk-gated backend run and one parallel query-tracing test failure are retained as incomplete/failed evidence. Final checks are reported separately. Cargo uses two jobs, no dev debug information, no incremental compilation, an 8 MiB Rust stack and niceness 19. The final pre-step gate waits below 7 GiB or while another Cargo runs, at three-minute intervals for up to sixty minutes. No other worktree's build folders were deleted. No cargo fmt or merge was run.
