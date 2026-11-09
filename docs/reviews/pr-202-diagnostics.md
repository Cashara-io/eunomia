# Diagnostics review proof

Product source validated at `1b5c90ac48458ceda0d8a7b8774c0bd802a8f0fa`.
The final documentation commit changes only this proof record.

## Design

- Python, backend stdout, the ring and imported container logs share the event
  allowlist in `backend/src/diagnostics/events.json`. Whole JSON lines require an
  exact known event and target. Messages use fixed templates. Only event-specific,
  typed, bounded integer fields survive. Other messages are `<redacted>` and
  other fields disappear. Embedded JSON in prose is never parsed.
- Container reads use `O_NONBLOCK | O_NOFOLLOW`, check the opened descriptor is a
  regular file, and omit files larger than 1 MiB. Blocking reads run outside the
  async workers under the common collection deadline. The outer deadline also
  bounds marker publication, directory checks and polling. The bundle guard stays
  in the async caller, so detached slow readers cannot retain it. Marker cleanup
  runs off the async worker.
- Logging constructs safe records before `try_lock`, drops contended events and
  counts drops atomically. Snapshots clone `Arc<str>` references under the lock
  and copy bytes afterwards. A bounded stdout queue uses `try_send`; a separate
  writer performs blocking writes.
- Settings, support documentation and the archive README describe these limits.
  System checks have a 30-second timeout, followed by a 60-second container
  collection deadline. Archive creation and audit recording follow.

## Main merge

Merged `332d96edf116da2af6d4c3a0ff9982f250d46320`, including Gmail quota handling,
Next.js 16.4.5 and the Ontology area. The scheduler conflict retains main's partial
checkpoints and retry semantics and diagnostics' failure counts and history.

Exports include partial-fetch status, ingest skips, retry delays up to seven days
and Gmail's optional cumulative permanent provider skips. Other fetchers leave
provider skips unknown. Cursor contents and warning prose are not parsed to
obtain counts. The OpenAPI document and generated client include the typed field.

## Completed checks

Every Cargo step checked `df -h /` and `pgrep -x cargo`, waiting three minutes
between resource checks when needed, with a two-hour bound per wait. Commands used
`CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0
RUST_MIN_STACK=8388608 nice -n 19`. No `cargo fmt` was run.

- Clippy with `-D warnings`: library, backend binary, diagnostics, connector-sync
  and OpenAPI targets passed. The pushed code also passed CI's all-target clippy.
- Full library suite: 552 passed. Connector-sync integration suite: 28 passed,
  including Gmail partial checkpoints, permanent skip counts and history.
- Final restored diagnostics run: 9 unit tests and 4 integration tests passed.
  It covers real FIFOs, symlinks to regular files, directories, oversized files,
  simulated slow/panicking reads, omission notes and successful second requests.
- Both OpenAPI tests passed without update mode after canonical regeneration.
- Client generation, Next.js 16.4.5 route generation, full typecheck, frontend
  lint and eight UI-value tests passed.
- All 17 Playwright tests passed on an isolated local stack: diagnostics,
  connectors-sync and ontology-area. It used fake credentials, stub embeddings
  and an in-memory database. Both services and the database were stopped.
- Inspected all four diagnostics screenshots, light/dark at 375 px and 1440 px.
- Diagnostics shell tests and both Python privacy tests passed again after
  restoring all guards. ZIP canaries include the reviewer's number, numeric
  environment values, long digit strings, phone numbers and card-like numbers.

## Guard mutations

Each mutation was isolated. Its test failed at an assertion, then the original
source was restored. The final restored diagnostics run passed all 13 tests.

| Disabled guard | Observed failure |
| --- | --- |
| Python whole-record/event filtering, restored reviewed parser | Numeric ZIP canary leaked |
| Rust event-specific numeric allowlist | Numeric secret survived in ZIP |
| Regular-file requirement | FIFO accepted |
| `O_NOFOLLOW` | Symlink to a regular file accepted |
| Blocking-read deadline | Slow read bypassed deadline |
| Nonblocking ring insertion | Logger blocked while ring mutex was held |

The contention test emits 1,000 events while another thread holds the ring lock.
The restored implementation finishes without waiting and counts all 1,000 drops.
The slow-read test injects a 250 ms reader behind a 20 ms timeout.

## Retained attempts and broader checks

- The first browser attempt reached an unrelated existing port 3000 server and
  failed. All reported browser passes used the isolated port 3336 stack.
- Post-merge lint caught Display formatting in two new record-ID assertions.
  They were corrected to RecordIdExt before the passing lint run.
- A new ZIP assertion exposed a fixture connector left disabled. The fixture now
  explicitly enables it; the corrected ZIP suite and final restored suite passed.
- A regular-file mutation build was interrupted for the requested main merge.
  Source was restored; that attempt was not counted. All five Rust mutations
  subsequently ran and failed as expected on the merged source.
- All 17 shell suites were attempted locally. After installing the declared
  Python dependency, 14 passed. The unchanged legacy-backup fixture reported
  46 passed and one failed on macOS. Two Docker suites initially stopped at the
  lockfile check after adding libc; the lock entry was fixed. On the pushed code,
  Linux CI's scripts, secrets-helper-compose and default update-path jobs passed,
  as did backend, hardened backend, frontend, end-to-end, upgrade and restore jobs.
  One additional v2.1.0 update-path matrix job was still queued at the last check.

Task-owned Cargo output, node_modules and .next were deleted at completion.
