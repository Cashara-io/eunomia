# Ontology PR 3 verification

- Fresh schema, schema snapshot and 0041 to 0042 upgrade preserve existing definitions and add no invented history.
- Two browser contexts: MCP proposal, live Review notice, stale revision conflict with changed description, preserved reason, re-review, approval and Activity in the other context.
- Shared socket replays only missed events and filters a token restricted to another vault.
- Activity filters use static parameterised predicates, with both orderings and 64 filter combinations exercised in the existing query suite.
- The 6 CI update-path release rows gain the new event table, migration ledger check and durable approval reason check.

## Stored events and live delivery

Tenant migration 0042 adds `ontology_event` and additive revision/actor fields. Each event has a per-vault sequence, time, credential identity, display name, action, definition type/key/revision, optional human reason and revision metadata. Existing definitions start at revision 1, while definitions only observed in data use revision 0. Existing history is not invented.

Synchronous database events append definition and strict-policy changes inside the transaction that changes the row. The same transaction advances the vault's sequence. Single typed-value writes append a count. Each completed CSV import appends one summary with entity, value and link counts. There is no dependency on the security audit ledger's best-effort writes or the content change log's post-write append.

The existing `/api/events/ws` connection adds `ontology` messages. The existing event bus wakes the durable outbox reader after commits. The shared socket catch-up tick also sees writes made by another backend replica. A reconnect supplies a bounded `ontology_after` map. First-time vaults start at the head; explicit resume cursors replay bounded pages in order, with HTTP resync for large backlogs. Authorisation runs before each delivered event, including replay. The client remembers the last processed sequence and invalidates only that vault's ontology, activity and object queries. Existing content/vault messages remain supported.

Decision endpoints require a non-negative `revision` and accept an optional reason of up to 500 characters. Each transaction compares the stored revision before changing the definition. A mismatch returns 409 `ontology.stale_revision` and writes nothing. Batch review sends each selected definition's captured revision and records each successful result independently.

During an N-1 rolling update, ontology reads continue to work. Mutating statements that require 0042 refuse with `tenant.schema_behind` until the tenant schema is ready. This avoids silently dropping audit fields on an older SCHEMAFULL table.

## Boundaries for PR 4

Connections, selective graph expansion and the table equivalent remain for PR 4. No synthetic mapping stages, graph overlays or animated run progress are added here. An import summary is appended only after the full import returns successfully. Previously committed chunks of a failed import have no completion summary.

## Browser proof

A real MCP proposal landed in an already-open Review page. The second context kept its old revision open, received a 409 after another proposal edit, displayed the changed description, preserved the person's reason, and saved the subsequent approval. The first context displayed the stored reason in Activity. Batch merge stored the same reason on both selected definitions. A stale batch result also shows the changed fields and opens a fresh review with the batch reason preserved.

- [Desktop light](https://github.com/Cashara-io/eunomia/blob/pr-screenshots/ontology-3/activity-1440-light.png)
- [Desktop dark](https://github.com/Cashara-io/eunomia/blob/pr-screenshots/ontology-3/activity-1440-dark.png)
- [Mobile light](https://github.com/Cashara-io/eunomia/blob/pr-screenshots/ontology-3/activity-390-light.png)
- [Mobile dark](https://github.com/Cashara-io/eunomia/blob/pr-screenshots/ontology-3/activity-390-dark.png)
- [Live proposal frame strip](https://github.com/Cashara-io/eunomia/blob/pr-screenshots/ontology-3/live-proposal-strip.png)
- [Stale revision conflict](https://github.com/Cashara-io/eunomia/blob/pr-screenshots/ontology-3/stale-conflict.png)
- [Batch revision conflict](https://github.com/Cashara-io/eunomia/blob/pr-screenshots/ontology-3/batch-stale-conflict.png)

## Recorded check results

- On the combined branch including main's diagnostics PR: `cargo check --tests` and `cargo clippy --all-targets -- -D warnings` passed. The final library, diagnostics, dynamic-query, event, migration, activity, OpenAPI and store suites passed 600 tests with 1 existing fixture ignored, both normally and with `TEST_HARDENED=1`.
- Expanded OAuth, auth-scope, full ontology, Objects, round-trip and golden-tool suites also passed normally and hardened before the diagnostics merge. The 110 full ontology cases include the owner-only decision boundary with the now-required revision.
- Integrating main while the previous OpenAPI binaries were still running produced a route/spec mismatch for the newly added diagnostics route. The combined spec and client were regenerated, and both final OpenAPI tests passed normally and hardened.
- Focused Helium Playwright against the isolated hardened SurrealDB: 10 passed, 1 existing large-fixture test skipped.
- An earlier focused attempt stopped in cold account setup before reaching Ontology. The batch assertion also needed to await the approval response before reading Activity. Both failed attempts were retained, and the complete focused run then passed.
- Full Helium Playwright run: 158 passed, 16 skipped, 1 signup case hit the fixture's authentication rate limit. That case passed when rerun on its own. The failed full-run result is retained in the verification record.
- Shared preview automation snapshots timed out twice. The real Playwright browser cases and screenshots supplied the browser proof.
- The latest-release update-path row guard passed. Migration numbering depends on #205 landing tenant 0041 before this lane's 0042. Fresh schema, 0041-to-0042 upgrade, replay, membership and stored-reason tests were exercised before the final source checks.
- Local multi-release Compose update runs were not attempted while this shared Mac was close to its disk threshold. The existing six CI update-path rows include the new table, migration and stored-reason assertions.

## Migration dependency

Lead assignment: tenant 0041 is `composio_connector` (#205), and this lane is tenant 0042. If #205 remains unmerged, the branch intentionally has no 0041 file or registration, so `check-migrations.sh` reports the expected contiguity gap. Local schema verification applies the Composio migration from `origin/feat/composio-connector` first.

The combined-schema probe used Composio commit `5b23162bbbdaba949cf24dd67cd55dda75cae5ab`. At that fetch the remote file was still named `0042_composio_connector.surql`; its unchanged SQL payload (`f246029e6d937431e3bf248dc40cda2bfed085a1f5747c5bea3dd986b83cfbc6`) was applied in the lead-assigned 0041 position, before activity 0042. This verified the schema combination on isolated hardened SurrealDB 3.3.5. The numbering guard also passed in a temporary copy with that SQL and the combined 0041/0042 registry. Composio source changes are not included in this branch.

## PR 207 review corrections

Activity contains metadata only. Definition events carry actor, vault, definition key and name,
change type, revision, optional human review reason and time. Value writes carry a count of one.
No constructor copies typed values, entity text, descriptions, expressions, enum choices, synonyms
or source mappings into the activity log. HTTP and socket reads normalise legacy import labels
and discard agent rationale, which may quote source data. They allow only numeric revision
and count fields from older `before` and `after` objects. The confidential-note canary checks stored
rows, live frames, replay and the HTTP page.

For `ontology_propose`, pass `revision` on each existing kind, link, property or metric item.
`ontology_get` returns it on every definition. For example, after reading revision 2, send
`{"kinds":[{"kind":"shop","revision":2,"description":"Reviewed description"}]}`.
Omitted or zero revisions allow creation only. Updates compare the revision inside the write
transaction. `ontology.stale_revision` means fetch `ontology_get`, review what changed and retry
with the new revision. Server suggestions and auto-mapping use the profile captured before the
model call as their compare-and-set snapshot. Decisions, description changes and merges also
compare their observed revision in the write transaction. A proposal that leaves a decided kind,
link or metric completely unchanged can still return that row without a revision.

A completed CSV import emits exactly one summary, including entities handled, values written and
links created. Value batches remain capped at 500. Link-only imports emit the same summary.
The summary is appended after all chunks succeed. A failed import can leave previously committed
chunks, but does not claim completion. The summary uses a fixed label, never a filename or cell text.

Each socket replay pass scans at most 32 vault heads and sends at most one 100-event page, then
returns to live traffic and heartbeats. Empty vaults are skipped, and vault selection rotates on
periodic passes. More than 1,000 pending events produces
`ontology_resync` with the vault and current cursor. The client advances its cursor and refreshes
ontology, Activity and Objects over HTTP. Membership is queried between pages and rechecked before
each frame, including resync. Tests exercise control traffic between pages, revocation without a
local bus notification, large-backlog resync and subsequent live delivery.

### Review correction evidence

The normal library, ontology, activity, events, migrations, dynamic-query, store, golden-tool and
OpenAPI suites passed 712 tests, with two existing fixtures ignored. The schema snapshot matched
without changes. The five new guards passed before mutation testing. Restoring the value leak,
blind proposal update, link-only import omission and draining replay loop made all five guards
fail. A separate mutation that cached replay membership and removed delivery rechecks failed with
`revoked member received another page`. Both mutation runs restored the fixed source afterwards.

Frontend lint and its ten helper tests, Next type generation, TypeScript checking, production build
and the Helium Playwright Activity spec passed. The Playwright run used a disposable in-memory app
on ports 18581 and 3852. Both services were stopped after the run. The expected missing-0041
migration contiguity failure was retained and ignored, as requested for this review.

Local check logs are in `/tmp/eunomia-207-review/`, including the deliberately failed mutation runs.

The first hardened full run passed every suite except one socket resume test. An empty vault could
consume a bounded pass and delay another vault's missed events until the ten-second heartbeat,
which raced the test's ten-second receive timeout. Replay now skips empty vaults, caps head scans
at 32 and retains the one-page limit. The resume fixture puts an empty vault first so this case
is deterministic. The failed first hardened run is retained in `hardened.log`.

After the replay correction, all eight socket tests and nine activity tests passed again both
normally and with `TEST_HARDENED=1`. This includes the deterministic empty-vault resume, canary
checks for legacy import labels and value payloads, bounded replay, resync and membership loss.
The Helium Playwright Activity spec also passed again against the corrected backend. Frontend
lint, helper tests, type generation, TypeScript checking and production build were repeated and
passed. See `recheck-normal.log`, `recheck-hardened.log`, `playwright-final.log` and
`frontend-final.log`.

The backend runs use the shared harness's embedded SurrealDB 3.3.5 engine. `TEST_HARDENED=1`
applies the production deny-by-default capabilities and function allowlist. No shared database
or protected runtime was used.

Agent proposal rationale stays on the definition for review over HTTP. Activity stores and sends
reasons only for human review decisions and description edits. New and legacy proposal events
omit rationale. The canary also supplies confidential text as proposal rationale, and the
Playwright spec checks both proposal events have no reason before checking the durable human
approval reason.

After excluding agent rationale, the socket, activity and migration suites passed 38 tests in each
mode, with one existing migration fixture ignored in each run. The final browser run first timed
out in account setup before reaching Ontology. Its log is retained as `playwright-privacy-final.log`.
The retry on the warmed app passed the full Activity spec, including the absent proposal rationale
and durable human approval reason. The passing retry is in `playwright-privacy-retry.log`. Frontend
lint, ten helper tests, type generation, TypeScript and production build passed again in
`frontend-privacy-final.log`.

Final `cargo clippy --all-targets -- -D warnings` passed after the rationale filter and final
privacy checks. Cargo commands used the disk/process guard and requested resource limits.
No `cargo fmt` command was run.
