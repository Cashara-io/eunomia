# Security log integrity and access reviews

The security log (`audit_event` in the control database) records sign-ins, tokens, app connections, space changes, admin actions and every mutating tool call: who, how (browser, API token, app or system), what, on which record, the outcome code and the request's trace id. This page covers how an event is delivered so it is not lost, how the log is hash-chained so an edit or a deletion shows, how to verify and export it, and access reviews. Code: `backend/src/audit.rs`, `backend/src/audit/chain.rs`, `backend/src/routers/audit_chain.rs`, `backend/src/routers/access_reviews.rs`. Schema: control migration 0010, tenant migration 0044. Tests: `backend/tests/audit_chain.rs`. Catalogue: C07 and C08 in [compliance/controls.json](compliance/controls.json).

## Delivery

An event is written in two steps:

1. **Write.** The row is created `pending`, with an id the writer chose and its time set by the writer. This is one small statement that no other writer competes for.
2. **Seal.** The writer reads its ledger's head (`audit_head`), gives the oldest pending rows the next sequence numbers and hashes, and commits them together with the new head in one transaction. The transaction first checks that the head has not moved since it was read; if it has, nothing is written and the writer reads again. Two writers, in one process or in several replicas, therefore never extend the same head: the chain stays one gapless sequence.

What can go wrong, and what happens:

| Failure | Result |
|---|---|
| The caller is cancelled while the event is being written (a client disconnects mid-request; issue #219) | The write and seal run in a task of their own, which the cancellation does not stop. |
| The write fails (the database refuses or is unreachable) | Retried twice, then queued in memory. The scheduler leader's replay (`chain::replay`, every tick) writes the queue. The event keeps its id and time, so a replay of an event that did land is a duplicate, never a second row. Up to 10,000 events are queued; past that the oldest is dropped, counted (`chain::DROPPED`) and logged at error level. |
| The process dies between the write and the seal | The row is in the database, `pending`. The next replay seals it, on any replica. |
| The seal fails | The same: the row stays pending until the replay seals it. |

What it does not cover: an event that only ever existed in the memory queue is lost if the process also dies before the database comes back. The action it describes is usually lost too in that case, since the action's own data lives in the same SurrealDB server. A write is never part of the action's own transaction: the action runs in the org database and the log lives in the control database, and SurrealDB has no transaction across databases.

## Ledgers

Each org has its own chain, `org:<32 hex>`. An event with no org (a sign-in attempt with an email that has no account) goes to the `instance` ledger. Tenants never share a chain. An event with a user goes to that user's home org (their oldest membership); a job or the self-host move names the org itself (`audit::record_in_org`).

## Hash

For each event, `hash = sha256_hex(canonical)` where `canonical` is this compact JSON array:

```
["eunomia.audit.v1", ledger, seq, prev_hash, id, created_at, user, actor_kind, actor_id, action, target, outcome, trace_id, detail_sha256]
```

- `seq` is an integer; the first event of a ledger is 1.
- `prev_hash` is the previous event's `hash`. For event 1 it is the genesis hash, `sha256_hex(["eunomia.audit.v1","genesis",ledger,genesis_at])`, kept as checkpoint 0.
- `created_at` and `genesis_at` are RFC 3339 in UTC with exactly six fractional digits and `Z`, for example `2026-11-13T01:02:03.000004Z`.
- `id` and `user` are `table:key` (`audit_event:...`, `user:...`); `user` is `null` when there is none.
- `detail_sha256` is the SHA-256 of the free-text detail. The hash commits to the detail without the export having to carry it.
- Strings are JSON with minimal escaping and no spaces: `serde_json`'s output, and the same bytes as Python's `json.dumps(value, ensure_ascii=False, separators=(",", ":"))`.

`backend/src/audit/chain.rs` (`canonical_form_is_pinned`) pins the format. Changing it would break every chain already written, so a change needs a new version string.

## Rows written before the chain

Rows written before control migration 0010 have no ledger and no hash. They are not rewritten: they stay readable in the activity log, the verify report counts them as `before_chain` on the instance ledger, and the retention window deletes them by age as before. Each ledger's chain starts at its genesis checkpoint, written when its first new event is sealed.

## Retention

`AUDIT_RETENTION_DAYS` (default 365, `0` keeps everything) still applies. For a chained ledger the daily prune finds the newest sealed event older than the window, writes a `retention` checkpoint with its sequence number, hash and the number of events deleted, and deletes it and everything before it, all in one transaction. Verification then starts from that checkpoint. Deleting the oldest events any other way leaves no checkpoint and is reported as missing events.

## Checkpoints and anchoring

`audit_checkpoint` keeps the points a verifier can start from or compare against: `genesis` (seq 0), `retention` (the last pruned event) and `periodic`, a copy of the head that the scheduler writes at most once an hour when the head has moved. Verification checks every checkpoint against the event at its sequence number, and a checkpoint past the head means the newest events were deleted.

Checkpoints in the same database only help against partial tampering. Someone who can write to the database can rewrite events, recompute every hash and move the head and checkpoints to match. Defending against that means keeping checkpoints somewhere they cannot reach. This release does not do that automatically (C07 stays Partial). To do it by hand: copy the newest `periodic` checkpoint (`seq` and `hash`) from an export into a place you control (a ticket, a signed email, an object-lock bucket), and pass it to the offline verifier later with `--checkpoint SEQ:HASH`. The planned design for the automatic version is in the PR03 description: sign each periodic checkpoint with a key derived from `ENCRYPTION_KEY` and optionally publish it to a configured write-once destination.

## Verifying

- **In the app:** Settings > Activity log > Check integrity, or `GET /api/admin/audit/verify` (admin). It seals anything pending, then walks the workspace's ledger and the instance ledger from their anchor to their head and reports the first break: `altered` (content no longer matches its hash), `link` (an event does not follow the one before it), `missing`, `duplicate`, `head`, `checkpoint` or `genesis`, with the sequence number and a sentence.
- **Offline:** `python3 scripts/audit/verify-audit-export.py export.json [--checkpoint SEQ:HASH ...]`. Standard library only. Exit 0 when it verifies, 1 at the first break (printed), 2 when the file cannot be read.

## Exporting

Settings > Activity log > Download, or `GET /api/admin/audit/export?from=YYYY-MM-DD&to=YYYY-MM-DD&ledger=org|instance` (admin). The range is by time (`from` inclusive, `to` exclusive, either may be left out), and the file holds the contiguous run of sequence numbers from the first event in the range to the last. Each event carries its canonical fields and its hash. The file also carries the anchor the first event hangs from, the stored checkpoints inside the range, an `export` checkpoint every 1,000 events and one at the last event, the head at export time, and the server's own check. At most 50,000 events per file: `truncated` and `next_seq` say where to continue.

The export is metadata only: who, what, when, on which record and the outcome. The free-text detail (which can hold an email address, a token name or a count) is replaced by its SHA-256, so the file holds no memory text, chat content, prompts or secrets and still verifies. The test `the_export_holds_metadata_only_and_verifies_offline` writes tenant content through a real tool and a detail canary and checks neither appears in the file. Each export is itself recorded in the log (`audit.export`).

## Access reviews

Settings > Access reviews (admin). Starting a review takes a snapshot of who has access to what in the workspace: every person, their role and why they are an admin, whether they are suspended, their spaces and role in each, their API tokens (name, scopes, restricted space, dates), connected apps (name, client id, scopes, dates) and the number of live browser sessions. Never a token, password or other secret. One review can be open at a time.

Admins then record a decision per person: keep, remove or suspend, with an optional note. A decision is a record, not an action: suspending or removing someone is still done from People. An admin cannot decide about their own access (`access_review.self_review`). Completing the review needs a decision for everyone except the admin completing it; their own access is recorded as not reviewed when no other admin decided it. This PR adds no separate reviewer role or separation-of-duties rules (planned in PR04 and PR09).

A completed review cannot change: the API refuses (`access_review.completed`) and so does the database (events in tenant migration 0044 refuse any update or delete of the review and its decisions). Its start, every decision and its completion are in the hash-chained log (`access_review.start`, `access_review.decide`, `access_review.complete`). The completion event's detail carries `result_hash`, the SHA-256 of the review's canonical JSON (keys sorted, no spaces): the review id, the snapshot's hash, who started and completed it and when, every decision and who was not reviewed. Download a completed review from its row or with `GET /api/admin/access-reviews/{id}/export`: the file has that document, both hashes, whether they still match, and the completion event's sequence number and hash in the chain. Reviews live in the org's own database, since they name people and spaces.
