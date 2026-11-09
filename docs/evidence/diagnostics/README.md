# Diagnostics evidence

`settings-card.png` shows the actual Settings > Support card running in Next,
with a local, synthetic admin API fixture. It is UI evidence, not a live sign-in
or a production-server diagnostics collection.

The Playwright spec also captures desktop and 375 px layouts in both themes and
checks download, waiting, errors, cap messaging and the member explanation.

`log-privacy.md` records the audit and the output policy. The sample ZIP listing
is produced by the backend integration test using an in-memory database and a
synthetic updater response. It is not a zip from a customer's installation.

Verified locally on 9 November 2026:

- Four backend integration tests, both normal and hardened engine settings.
- Five diagnostics unit tests, covering both ring limits, privacy patterns,
  content-bearing tracing fields, archive limits and sync history.
- Eight store tests, both normal and hardened, including every static query and
  the raw-database access boundary.
- Focused Clippy (`--lib --test diagnostics -- -D warnings`) passed.
- Two OpenAPI tests, regenerated API client and TypeScript check.
- Seven Playwright tests with API fixtures. Four layout captures inspected.
- Both guard mutations killed by test assertions: removing the admin check
  changes the member response to 200; bypassing scrubbing fails a secret canary.
- Diagnostics updater tests, 105 existing auto-update checks, 79 HTTPS checks,
  migration numbering and update-path row guards.

The Rust tests were built with the requested low-priority Cargo settings, then
the produced test binaries were run directly with `RUST_MIN_STACK=8388608` and
`nice -n 19`. No formatter was run. The full Docker update-path matrix remains a
CI check, it was not run locally on the shared Mac.

After both mutations were restored, a fresh build and serial run passed all
31 selected Rust test executions, including both hardened checks.
