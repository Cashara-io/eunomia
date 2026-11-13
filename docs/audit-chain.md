# Security log integrity and access reviews

The security log (`audit_event` in the control database) records sign-ins, tokens, app connections, space changes, admin actions and every mutating tool call: who, how (browser, API token, app or system), what, on which record, the outcome code and the request's trace id. This page covers how an event is delivered so it is not lost, how the log is hash-chained so an edit or a deletion shows, how to verify and export it, and access reviews. Code: `backend/src/audit.rs`, `backend/src/audit/chain.rs`, `backend/src/routers/audit_chain.rs`, `backend/src/routers/access_reviews.rs`. Schema: control migration 0010, tenant migration 0044. Tests: `backend/tests/audit_chain.rs`. Catalogue: C07 and C08 in [compliance/controls.json](compliance/controls.json).

## Delivery

An event is written in two steps:

1. **Write.** The row is created `pending`, with an id and a time the writer chose. This is one small statement that no other writer competes for, and it is the only thing a request waits for: at most two quick retries, never a lock, a seal or the replay queue.
2. **Seal.** A background sealer, one per ledger per process however many events arrive, reads the ledger's head (`audit_head`). It gives the oldest pending rows the next sequence numbers and hashes, and commits them together with the new head in one transaction. That transaction first checks that the head has not moved since it was read; if it has, nothing is written and the sealer reads again. Two sealers, in one process or in several replicas, therefore never extend the same head: the chain stays one gapless sequence.

What can go wrong, and what happens:

| Failure | Result |
|---|---|
| The client disconnects while the event is being written | The write runs in a task of its own, which the cancellation does not stop. |
| The client disconnects while the action itself is still running (issue #219) | Routes that use `audit::detached` (access reviews, the log export) run the action and its event in one task, so they finish together. Other audited routes still record their event after the action in the request's own future: see Known limits. |
| The write fails (the database refuses or is unreachable) | Retried twice, then queued in this process. **Every process drains its own queue on its own 5-second timer** (`chain::spawn_drain`), leader or not, and once more at graceful shutdown (up to 10 seconds). The event keeps its id and time, so a replay of an event that did land is a duplicate, never a second row. Up to 10,000 events are queued per process; past that the oldest is dropped, counted (`chain::DROPPED`) and logged at error level. |
| The process dies between the write and the seal, or a seal fails | The row is in the database, `pending`. The scheduler leader's replay (`chain::replay`, every tick) seals every pending row of every ledger, whichever process wrote it. |
| The delivery task panics | The event is lost, and an error line names its action. |

**The case it does not cover:** an event that only exists in a process's queue is lost if that process crashes, or is killed without a graceful shutdown, before the database accepts it. In practice this means the database was unreachable at the time, and the action the event describes usually failed for the same reason, since its own data lives in the same SurrealDB server. A write is never part of the action's own transaction: the action runs in the org database and the log lives in the control database, and SurrealDB has no transaction across databases.

**Cost of a burst.** A request pays one write. The seal for a whole burst is one transaction per 200 events, by one task per ledger per process, so a burst of failed sign-ins (all on the `instance` ledger) costs real sign-ins nothing beyond that write: sign-ins with a known account go to their org's ledger, and nothing a request does waits for the instance head. Across replicas, sealers of one ledger take turns at its head; a sealer that loses retries with a short random pause, in the background.

## Ledgers

Each org has its own chain, `org:<32 hex>`. An event with no org (a sign-in attempt with an email that has no account, a refused request with no credential) goes to the `instance` ledger. An event with a user goes to that user's home org (their oldest membership); a job or the self-host move names the org itself (`audit::record_in_org`).

**Who can see the instance ledger.** It holds attempts against every tenant, so on a host with several organisations only the server's operators see it: the accounts listed in `EUNOMIA_ADMIN_EMAILS` and the install's first account (`authz::sees_instance`). That covers its verify entry, its export and its rows in the activity log. While the install has a single organisation, any admin of it sees it, as before. Who is an admin at all (`authz::is_admin`): an operator, the first account, or an account an admin granted admin rights in Settings > People. People only lists the granting admin's own organisation, so granted admins exist only in organisations that already have one.

## Hash

For each event, `hash = sha256_hex(canonical)` where `canonical` is this compact JSON array:

```
["eunomia.audit.v1", ledger, seq, prev_hash, id, created_at, user, actor_kind, actor_id, action, target, outcome, trace_id, detail_digest]
```

- `seq` is an integer; the first event of a ledger is 1.
- `prev_hash` is the previous event's `hash`. For event 1 it is the genesis hash, `sha256_hex(["eunomia.audit.v1","genesis",ledger,genesis_at])`, kept as checkpoint 0.
- `created_at` and `genesis_at` are RFC 3339 in UTC with exactly six fractional digits and `Z`, for example `2026-11-13T01:02:03.000004Z`.
- `id` and `user` are `table:key` (`audit_event:...`, `user:...`); `user` is `null` when there is none.
- `detail_digest` is HMAC-SHA256 of the free-text detail under a key derived from `ENCRYPTION_KEY` (label `eunomia/audit-detail`, the way the control database password is derived; nothing is stored). It is computed when the row is written and stored on it with the key's fingerprint (`digest_key`). The hash commits to the detail without the export carrying it, and a digest in an export cannot be reversed by guessing the detail: a failed sign-in's detail is the email that was tried. `verify` checks each detail against its digest; after a change of `ENCRYPTION_KEY` older details cannot be checked and are counted as `detail_unchecked` (their hash and link are still checked).
- Strings are JSON with minimal escaping and no spaces: `serde_json`'s output, and the same bytes as Python's `json.dumps(value, ensure_ascii=False, separators=(",", ":"))`.

`backend/src/audit/chain.rs` (`canonical_form_is_pinned`) pins the format. Changing it would break every chain already written, so a change needs a new version string.

## Rows written before the chain

Rows written before control migration 0010 have no ledger and no hash. They are not rewritten: they stay readable in the activity log, the verify report counts them as `before_chain` on the instance ledger, and the retention window deletes them by age as before. Each ledger's chain starts at its genesis checkpoint, written when its first new event is sealed.

## Retention

`AUDIT_RETENTION_DAYS` (default 365, `0` keeps everything) still applies. For a chained ledger the daily prune finds the first event written inside the window and cuts just below it (found from the newest events down, so one row with an old timestamp sealed late cannot pull newer events into the cut), writes a `retention` checkpoint with its sequence number, hash and the number of events deleted, and deletes it and everything before it, all in one transaction. Verification then starts from that checkpoint. Deleting the oldest events any other way leaves no checkpoint and is reported as missing events.

## Checkpoints and anchoring

`audit_checkpoint` keeps the points a verifier can start from or compare against: `genesis` (seq 0), `retention` (the last pruned event) and `periodic`, a copy of the head that the scheduler writes at most once an hour when the head has moved. Verification checks every checkpoint against the event at its sequence number, and a checkpoint past the head means the newest events were deleted. Before writing a periodic checkpoint the scheduler checks the events since the previous one, so it never endorses a head that does not follow from it. Verify reads the checkpoints before the head and stops at that head, so events sealed while it runs are newer, not errors.

Checkpoints in the same database only help against partial tampering. Someone who can write to the database can rewrite events, recompute every hash and move the head and checkpoints to match. Defending against that means keeping checkpoints somewhere they cannot reach. This release does not do that automatically (C07 stays Partial). To do it by hand: copy the newest `periodic` checkpoint (`seq` and `hash`) from an export into a place you control (a ticket, a signed email, an object-lock bucket), and pass it to the offline verifier later with `--checkpoint SEQ:HASH`. The planned design for the automatic version is in the PR03 description: sign each periodic checkpoint with a key derived from `ENCRYPTION_KEY` and optionally publish it to a configured write-once destination.

## What this protects against, and what it does not

External anchoring is not built, so this list is what the chain proves on its own.

**Detected by `verify` and by the offline script:**

- an edited event, including only its detail (in the app, while the key is unchanged);
- a deleted, duplicated or reordered event in the middle;
- a deleted head while events or checkpoints remain;
- a deleted or changed genesis;
- the oldest events deleted without a retention checkpoint;
- the newest events deleted when a periodic checkpoint taken earlier is still stored.

**Detected only with a checkpoint kept off the server** (pass it to the offline script with `--checkpoint`):

- the newest events deleted together with the periodic checkpoints that covered them, and the head wound back to a stored hash. This needs no hashing.
- a suffix rewritten with freshly computed hashes, head and checkpoints. The format is public and the event hash is not keyed, so someone with database write access can compute it.

**Not detected** by anyone holding only database access, and the reason a deployment that needs it must keep checkpoints elsewhere:

- a forged `retention` checkpoint plus deletion of the events below it (it looks like a normal retention cut);
- a whole ledger wiped: its events, head and checkpoints together;
- events inserted with no ledger (they show in the activity log and count as written before the chain);
- a forged event inserted `pending` with a plausible time (it is sealed in as genuine);
- the database events that freeze a completed access review, removed with `REMOVE EVENT`.

Someone with the application's secrets (`ENCRYPTION_KEY`) as well as the database can also produce matching detail digests.

## Verifying

- **In the app:** Settings > Activity log > Check integrity, or `GET /api/admin/audit/verify` (admin). It seals anything pending, then walks the workspace's ledger and the instance ledger from their anchor to their head and reports the first break: `altered` (content no longer matches its hash), `link` (an event does not follow the one before it), `missing`, `duplicate`, `head`, `checkpoint` or `genesis`, with the sequence number and a sentence.
- **Offline:** `python3 scripts/audit/verify-audit-export.py export.json [--checkpoint SEQ:HASH ...] [--partial]`. Standard library only. Exit 0 when it verifies, 1 at the first break (printed), 2 when the file cannot be read. A checkpoint you kept that the file does not reach fails ("the export ends at event N but you hold a checkpoint for event M"): newer events were deleted or the head wound back. A checkpoint older than the file's first event fails the same way. Pass `--partial` when the file is deliberately a slice (a date range, a continuation file, or a log the retention window has cut since you kept the checkpoint). A file exported with no end date must also end at the head it records. `scripts/tests/verify-audit-export.test.sh` covers each case.

## Exporting

Settings > Activity log > Download, or `GET /api/admin/audit/export?from=YYYY-MM-DD&to=YYYY-MM-DD&ledger=org|instance` (admin). The range is by time (`from` inclusive, `to` exclusive, either may be left out), and the file holds the contiguous run of sequence numbers from the first event in the range to the last. Each event carries its canonical fields and its hash. The file also carries the anchor the first event hangs from, the stored checkpoints inside the range, an `export` checkpoint every 1,000 events and one at the last event, the head at export time, and the server's own check. At most 50,000 events per file: `truncated` and `next_seq` say where to continue.

The export is metadata only: who, what, when, on which record and the outcome. The free-text detail (which can hold an email address, a token name or a count) is replaced by its keyed digest (see Hash), so the file holds no memory text, chat content, prompts or secrets, cannot be dictionary-reversed, and still verifies. The test `the_export_holds_metadata_only_and_verifies_offline` writes tenant content through a real tool and a detail canary and checks neither appears in the file. Each export is itself recorded in the log (`audit.export`).

## Access reviews

Settings > Access reviews (admin). Starting a review takes a snapshot of who has access to what in the workspace: every person, their role and why they are an admin, whether they are suspended, their spaces and role in each, their API tokens (name, scopes, restricted space, dates), connected apps (name, client id, scopes, dates) and the number of live browser sessions. Never a token, password or other secret. One review can be open at a time.

Admins then record a decision per person: keep, remove or suspend, with an optional note. A decision is a record, not an action: suspending or removing someone is still done from People. An admin cannot decide about their own access (`access_review.self_review`). Completing the review needs a decision for everyone except the admin completing it; their own access is recorded as not reviewed when no other admin decided it. This PR adds no separate reviewer role or separation-of-duties rules (planned in PR04 and PR09).

A completed review cannot change: the API refuses (`access_review.completed`) and so does the database (events in tenant migration 0044 refuse any update or delete of the review and its decisions). Its start, every decision and its completion are in the hash-chained log (`access_review.start`, `access_review.decide`, `access_review.complete`). The completion event's detail carries `result_hash`, the SHA-256 of the review's canonical JSON (keys sorted, no spaces): the review id, the snapshot's hash, who started and completed it and when, every decision and who was not reviewed. Download a completed review from its row or with `GET /api/admin/access-reviews/{id}/export`: the file has that document, both hashes, whether they still match, and the completion event's sequence number and hash in the chain. Reviews live in the org's own database, since they name people and spaces.

## Migration numbers

Migrations are numbered in merge order (`backend/migrations/QUEUE.md`). The scheduler queues an org's tenant migration only while its version is below the latest, so a number left out would never be applied once it lands. `scripts/ci/check-migrations.sh` fails CI on a gap, and `migrate::require_contiguous` makes a binary with a gap refuse to boot ("this build registers migrations with a gap"). Test builds (feature `test-support`) and `EUNOMIA_ALLOW_MIGRATION_GAP=1` (a developer running a lane before the numbers ahead of it land) only log it.

## Known limits

- **Cancellation (#219).** Only routes that use `audit::detached` keep an action and its event together when the client disconnects mid-action: access reviews and the log export. Every other audited route still records its event after the action, so a client dropping out between the two can leave a committed action without its event. Moving each route to `audit::detached` is mechanical; it was left out of this change to keep it reviewable.
- **Rolling upgrade.** Replicas still on the previous release write events with no ledger (they are never chained), and their retention job deletes by age without the checkpoint. If one of them runs the daily prune during the rollout, `verify` reports the cut as missing events. Upgrade all replicas together, or stop the old workers first; a single-process self-host upgrade is not affected.
- **Clocks.** Event times come from the writing process's clock. Sequence order, not time order, is what the chain proves; an export by date range takes the contiguous run from the first event at or after `from` to the last before `to`.
- **Access-review decisions** are records only: suspending or removing a person is still done from People. A lone admin's own access is recorded as not reviewed. There is no reviewer role or separation of duties (PR04 and PR09).
