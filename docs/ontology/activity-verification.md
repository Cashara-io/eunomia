# Ontology PR 3 verification

- Fresh schema, schema snapshot and 0040 to 0041 upgrade preserve existing definitions and add no invented history.
- Two browser contexts: MCP proposal, live Review notice, stale revision conflict with changed description, preserved reason, re-review, approval and Activity in the other context.
- Shared socket replays only missed events and filters a token restricted to another vault.
- Activity filters use static parameterised predicates, with both orderings and 64 filter combinations exercised in the existing query suite.
- The 6 CI update-path release rows gain the new event table, migration ledger check and durable approval reason check.

## Stored events and live delivery

Tenant migration 0041 adds `ontology_event` and additive revision/actor fields. Each event has a per-vault sequence, time, credential identity, display name, action, definition type/key/revision, optional human reason and before/after review fields. Existing definitions start at revision 1, while definitions only observed in data use revision 0. Existing history is not invented.

Synchronous database events append definition and strict-policy changes inside the transaction that changes the row. The same transaction advances the vault's sequence. Single typed-value writes and CSV typed-value batches append their own committed values or counts. There is no dependency on the security audit ledger's best-effort writes or the content change log's post-write append.

The existing `/api/events/ws` connection adds `ontology` messages. Its one-second outbox reader also sees writes made by another backend replica. A reconnect supplies a bounded `ontology_after` map. First-time vaults start at the head; explicit resume cursors replay every later row in order. Authorisation runs before each delivered event, including replay. The client remembers the last processed sequence and invalidates only that vault's ontology, activity and object queries. Existing content/vault messages remain supported.

Decision endpoints require a non-negative `revision` and accept an optional reason of up to 500 characters. Each transaction compares the stored revision before changing the definition. A mismatch returns 409 `ontology.stale_revision` and writes nothing. Batch review sends each selected definition's captured revision and records each successful result independently.

During an N-1 rolling update, ontology reads continue to work. Mutating statements that require 0041 refuse with `tenant.schema_behind` until the tenant schema is ready. This avoids silently dropping audit fields on an older SCHEMAFULL table.

## Boundaries for PR 4

Connections, selective graph expansion and the table equivalent remain for PR 4. No synthetic mapping stages, graph overlays or animated run progress are added here. Committed activity does not claim that an entire mapping run or CSV import completed when only a batch is known to have committed.

## Browser proof

A real MCP proposal landed in an already-open Review page. The second context kept its old revision open, received a 409 after another proposal edit, displayed the changed description, preserved the person's reason, and saved the subsequent approval. The first context displayed the stored reason in Activity. Batch merge stored the same reason on both selected definitions.

- [Desktop light](https://github.com/Cashara-io/eunomia/blob/pr-screenshots/ontology-3/activity-1440-light.png)
- [Desktop dark](https://github.com/Cashara-io/eunomia/blob/pr-screenshots/ontology-3/activity-1440-dark.png)
- [Mobile light](https://github.com/Cashara-io/eunomia/blob/pr-screenshots/ontology-3/activity-390-light.png)
- [Mobile dark](https://github.com/Cashara-io/eunomia/blob/pr-screenshots/ontology-3/activity-390-dark.png)
- [Live proposal frame strip](https://github.com/Cashara-io/eunomia/blob/pr-screenshots/ontology-3/live-proposal-strip.png)
- [Stale revision conflict](https://github.com/Cashara-io/eunomia/blob/pr-screenshots/ontology-3/stale-conflict.png)

## Recorded check results

- Focused Helium Playwright against the isolated hardened SurrealDB: 10 passed, 1 existing large-fixture test skipped.
- Full Helium Playwright run: 158 passed, 16 skipped, 1 signup case hit the fixture's authentication rate limit. That case passed when rerun on its own. The failed full-run result is retained in the verification record.
- Shared preview automation snapshots timed out twice. The real Playwright browser cases and screenshots supplied the browser proof.
- Migration numbering and latest-release update-path row guards passed. Fresh schema, 0040-to-0041 upgrade, replay, membership and stored-reason tests were exercised before the final source checks.
- Local multi-release Compose update runs were not attempted while this shared Mac was close to its disk threshold. The existing six CI update-path rows include the new table, migration and stored-reason assertions.
