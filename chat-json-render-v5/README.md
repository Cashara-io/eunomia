# Bound prose placeholder proof

Initial source: 679d69e57ac48fbadab5a41947c90be3b70a0d35. Final runtime source: 12f0f63e. Source trees are recorded in source-trees.json. The source PR records the final documentation head and validation results.

The server resolves numeric and date placeholders against one exact typed path, formats with server units/currency and returns substituted UI props and terminal text. Raw digits require complete normalised server string tokens or matching parsed applied dates/bucket labels. Number words stay unchecked. The generated catalogue and chat prompt both describe this rule. Rejection errors list concrete valid numeric paths.

## Final live run

The original six questions use gpt-6-luna and request Line, Donut, Histogram, Scatter, Scenario and Table. Each additionally requests a measured count or total in its title through a bound placeholder. Table retains the prior proof's explicit whole-dataset pointer hint. Receipts are authentic persisted chat histories.

All six final answers use placeholders. verify_numbers.py independently recomputes 436 numeric fields from the CSV within 0.01. verify_placeholders.py independently checks nine exact formatted substitutions, including currency and inclusion in terminal text. It checks every substitution and requires each answer's final view to contain a placeholder; an intermediate plain view is allowed.

Monthly, supplier and Scatter titles bind the overall 1588.00 AUD. Histogram and Table bind twelve rows/samples. Scenario binds its entity count and shows 1588.00 to 1746.80 AUD, delta 158.80 AUD. Histogram automatically repairs its tool calls and produces two views. Its ordinary chat reply still prints unresolved placeholder syntax, while the UI and server text are correctly substituted. One follow-up corrects that ordinary reply. All calls and the correction are retained. Scenario also repairs source/schema, raw headline and binding errors. This does not claim first-attempt success or semantic correctness of every model sentence.

All six final screenshots were captured and inspected in native Helium after repeated T3 snapshot failures: "Preview automation snapshot timed out after 15000ms." The table screenshot includes its overall totals footer, 1588.00 AUD and quantity 49, on the first of two pages. Native JPEG captures were encoded as real PNG without resizing or cropping; decoded pixel bytes and dimensions were compared for equality. The original dimensions are 3024 by 1898. Browser proof tabs were closed and the original view restored.

## Isolation, checks and retained attempts

Throwaway SurrealDB 3.3.5 uses 8597 with --allow-all --allow-experimental=files memory; backend 8598 and frontend 3597. The shared database 8000 and owner ports 3600/8600/3700 are never used. Twelve CSV entities and sixty typed property values import with no bad cells. Documents use a memory bucket. Seeding and deterministic browser checks use the stub provider. Live chat uses the real OpenAI provider and gpt-6-luna. No keys, launchers, cookies or runtime logs are uploaded.

Earlier attempts are separate under attempts/. They exposed missing file-bucket configuration, the stub-provider setting disabling chat, confusing aggregate paths, a false month coverage gap and stale generated catalogue wording. The first unit date-range fixture failure is retained in unit.log; corrected UI tests are in unit-restored.log. Completed guard mutations must fail their named regression assertion, not compilation, and restore exact source bytes.

Every Cargo step checks `df -h /` and `pgrep -x cargo` before running, waits three minutes while blocked for up to two hours, and uses two jobs, debug zero, no incremental compilation, an 8 MiB Rust stack, serial tests and niceness 19. The owned target was transparently compressed with hash equality checks, then moved to a disposable 4 GiB RAM disk. The owned Next output was moved there too. Completed test executables are removed only after all six backend suites pass, to make room for clippy. Full logs are retained. No other worktree's artefacts are removed. Final cleanup removes owned target, node_modules, .next, runtime processes, disposable container and RAM disk. No cargo fmt or merge is run.

See results.txt for exact final check results and source/proof heads.

Final validation: all requested backend suites, strict clippy, frontend checks and nine Playwright tests pass. All thirteen mutation experiments fail their intended assertions and restore source. See results.txt and the individual logs.
