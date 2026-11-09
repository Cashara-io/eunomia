# Diagnostics review fixes

Base: `db7cb98a4c80f9339bf78cf25e5843c29995624d` on `feat/diagnostics-zip`.

## Design

- The collector parses whole JSON lines only. Python, backend stdout, the ring and
  imported container logs share `backend/src/diagnostics/events.json`. An exact
  event-name and target pair selects a fixed message template and bounded integer
  fields. Unknown messages become `<redacted>` and unknown fields disappear.
  Booleans, strings, floating point numbers and out-of-range integers are dropped.
- Container reads use `O_NONBLOCK | O_NOFOLLOW`, inspect the opened descriptor and
  require a regular file no larger than 1 MiB. Reads, including the ready marker,
  run in `spawn_blocking` under the common collection deadline. An outer timeout
  also bounds directory checks, marker publication and polling. The request's
  mutex guard stays in the async caller, so a detached slow reader cannot retain
  it. Marker cleanup is dispatched off the async worker, including on timeout.
- Logging builds the safe record before taking `try_lock`. Busy or poisoned rings
  drop events and increment an atomic counter. Snapshots clone `Arc<str>` entries
  under the lock and copy their bytes afterwards. A bounded stdout queue uses
  `try_send`; a separate thread performs potentially blocking writes.
- Settings, support documentation and the archive README describe these limits.
  Container collection has a 60-second deadline after system checks, whose
  timeout is 30 seconds. Archive creation and audit recording follow.

## Verification

Run Rust checks with `CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0
CARGO_INCREMENTAL=0 RUST_MIN_STACK=8388608 nice -n 19`. Before every Cargo
invocation, check `df -h /` and `pgrep -x cargo`. Wait three minutes between
checks if another Cargo process is running or free space is below 7 GiB, for at
most two hours. Do not run `cargo fmt`.

- `bash scripts/tests/diagnostics.test.sh`: passed, including numeric ZIP canaries.
- Python mutation: restoring the reviewed `safe_logs` parser caused the numeric
  ZIP privacy assertion to fail. Restored code passed again.
- `bunx playwright test tests/diagnostics.spec.ts` against a dedicated frontend on
  port 3336: 7 passed. The initial attempt used an unrelated existing port 3000
  server and failed; that run was discarded and the server was isolated.
- `bunx tsc --noEmit` and changed-file ESLint: passed.
- Inspected light and dark screenshots at 375 px and 1440 px. No horizontal overflow.
- Ran all 18 `scripts/tests/*.test.sh` suites. Compliance needed its declared
  Python `jsonschema` dependency; all 21 rejection fixtures then passed. The
  SurrealDB upgrade suite passed all 46 checks. Backup reported 46 passed and one
  failed legacy-format restore fixture in unchanged code. Two Docker build suites
  initially stopped at the lockfile check after adding `libc`; the lock entry
  was subsequently added.

- `cargo clippy --locked --lib --bin eunomia-backend --test diagnostics -- -D warnings`: passed.
- `cargo test --locked --lib diagnostics::`: 8 passed.
- Integration diagnostics tests and Rust mutations remain pending the resource
  gate. Completed Cargo checks followed the requested environment and priority.
  The task-owned target directory was removed after the unit tests to recover
  space before the next Cargo step.
