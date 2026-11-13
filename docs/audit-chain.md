# Security log integrity and access reviews

The security log (`audit_event` in the control database) records sign-ins, tokens, app connections, space changes, admin actions and every mutating tool call: who, how (browser, API token, app or system), what, on which record, the outcome code and the request's trace id. This page covers how an event is delivered so it is not lost, how the log is hash-chained so an edit or a deletion shows, how to verify and export it, and access reviews. Code: `backend/src/audit.rs`, `backend/src/audit/chain.rs`, `backend/src/routers/audit_chain.rs`, `backend/src/routers/access_reviews.rs`. Schema: control migration 0010, tenant migration 0044. Tests: `backend/tests/audit_chain.rs`. Catalogue: C07 and C08 in [compliance/controls.json](compliance/controls.json).

## Delivery

An event is written in two steps:

1. **Write.** The row is created `pending`, with an id and a time the writer chose. This is one small statement that no other writer competes for, and it is the only thing a request waits for: at most two quick retries, never a lock, a seal or the replay queue.
2. **Seal.** A background sealer, one per ledger per process however many events arrive, reads the ledger's head (`audit_head`). It gives the oldest pending rows the next sequence numbers and hashes, and commits them together with the new head in one transaction. That transaction first checks that the head has not moved since it was read; if it has, nothing is written and the sealer reads again. Two sealers, in one process or in several replicas, therefore never extend the same head: the chain stays one gapless sequence.

**Every way an action can succeed while its event is lost**, and what an operator sees. Everything else (a failed write, a failed seal, a crash between write and seal) is retried until it lands: a failed write waits in its process's queue, drained by that process every 5 seconds and at graceful shutdown, and the scheduler leader's replay seals every pending row on any replica. A replay never writes an event twice: the event keeps its own id and time.

| How the event is lost | When | What an operator sees |
|---|---|---|
| The process crashes, or is killed without a graceful shutdown, while events wait in its queue | The database was unreachable (writes failed) and the process died before it came back | Earlier `audit_event write failed; queued for replay` warnings from that process, and no `audit replay queue drained` line after them |
| The queue passes 10,000 events and drops the oldest | A long database outage on one process | `audit replay queue full: oldest event dropped` at error level, with the action and id; `chain::DROPPED` counts them |
| Graceful shutdown cannot write the queue within 10 seconds | The database is still down at shutdown | `audit events still queued at shutdown were lost`, with the count, at error level |
| The delivery task panics | A bug | `audit delivery task failed: event lost`, with the action, at error level |
| A queued event that can never be written blocks the queue behind it | A write the database refuses every time (for example a schema assertion added later that an old event fails). A drain pass stops at the first failure, so the events behind it wait until the queue overflows (then the oldest, this one, is dropped) or the process exits | The same `audit_event write failed` warning for the same action on every drain pass |
| The client disconnects between an older route's action and its event (issue #219) | Any audited route except access reviews and the log export, which use `audit::detached` | Nothing: the action is done and no event or warning is written |
| Graceful shutdown while a detached action is still running | A client left an access-review or export request just before the process stopped; shutdown waits for the job loops and the queue, not for detached request tasks | Nothing: the runtime drops the task |

A write is never part of the action's own transaction: the action runs in the org database and the log lives in the control database, and SurrealDB has no transaction across databases.

**Cost of a burst.** A request pays one write (and, for an event with a user, one read of that user's organisation). The seal for a whole burst is one transaction per 200 events, by one background task per ledger per process. Sealers take their own locks, one per ledger, held for one batch at a time and never across a retry pause, apart from the striped `tx::lock` that sign-up and job claims share, so a flood of failed sign-ins on the `instance` ledger does not slow other requests. Across replicas, sealers of one ledger take turns at its head; a sealer that loses retries with a short random pause. **A ledger that fails** is logged, counted (`chain::SEAL_FAILURES`) and left alone for 2, 4, 8 ... seconds (at most five minutes) by the replay, which carries on with the other ledgers; retention skips it the same way.

## Ledgers

Each org has its own chain, `org:<32 hex>`. An event with no org (a sign-in attempt with an email that has no account, a refused request with no credential) goes to the `instance` ledger. An event with a user goes to that user's home org (their oldest membership); a job or the self-host move names the org itself (`audit::record_in_org`).

**What the instance ledger holds.** Every event with no organisation:

- failed password sign-ins, whether or not an account has that email (the password route records no user for a failure), with the email that was tried in the detail;
- `auth.failed`: a request with a missing, expired or unknown credential, with the method and path, once per credential per minute;
- failed OpenID Connect sign-ins, with the reason (which can name the provider's error);
- the events of an account with no organisation membership.

In the database the detail is kept in the clear (the operator's activity log shows the attempted email). In an export it is a keyed digest.

**Who can read it.** On a host with several organisations: the accounts listed in `EUNOMIA_ADMIN_EMAILS` and the install's first account, permanently (`authz::sees_instance`). That covers its verify entry, its export and its rows in the activity log. While the install has a single organisation, any admin of it also sees it, as before; that window does not reopen, because organisations are never deleted. So on a hosted instance the operator must create the first account before sign-up opens (docs/deployment.md). Who is an admin at all (`authz::is_admin`): an operator, the first account, or an account an admin granted admin rights in Settings > People; People only lists the granting admin's own organisation.

## Hash

For each event, `hash = sha256_hex(canonical)` where `canonical` is this compact JSON array:

```
["eunomia.audit.v1", ledger, seq, prev_hash, id, created_at, user, actor_kind, actor_id, action, target, outcome, trace_id, detail_digest]
```

- `seq` is an integer; the first event of a ledger is 1.
- `prev_hash` is the previous event's `hash`. For event 1 it is the genesis hash, `sha256_hex(["eunomia.audit.v1","genesis",ledger,genesis_at])`, kept as checkpoint 0.
- `created_at` and `genesis_at` are RFC 3339 in UTC with exactly six fractional digits and `Z`, for example `2026-11-13T01:02:03.000004Z`.
- `id` and `user` are `table:key` (`audit_event:...`, `user:...`); `user` is `null` when there is none.
- `detail_digest` commits to the free-text detail. For an access review's start and completion (`chain::public_detail`), whose detail is only hashes and counts, it is the plain SHA-256 of the detail, so an auditor can check offline that the completion event names the exported review's `result_hash`. For every other event it is HMAC-SHA256 of the free-text detail under a key derived from `ENCRYPTION_KEY` (label `eunomia/audit-detail`, the way the control database password is derived; nothing is stored). It is computed when the row is written and stored on it with the key's fingerprint (`digest_key`). The hash commits to the detail without the export carrying it, and a digest in an export cannot be reversed by guessing the detail: a failed sign-in's detail is the email that was tried. `verify` checks each detail against its digest; after a change of `ENCRYPTION_KEY` older details cannot be checked and are counted as `detail_unchecked` (their hash and link are still checked).
- Strings are JSON with minimal escaping and no spaces: `serde_json`'s output, and the same bytes as Python's `json.dumps(value, ensure_ascii=False, separators=(",", ":"))`.

`backend/src/audit/chain.rs` (`canonical_form_is_pinned`) pins the format. Changing it would break every chain already written, so a change needs a new version string.

## Rows written before the chain

Rows written before control migration 0010 have no ledger and no hash. They are not rewritten: they stay readable in the activity log, the verify report counts them as `before_chain` on the instance ledger, and the retention window deletes them by age as before. Each ledger's chain starts at its genesis checkpoint, written when its first new event is sealed.

## Retention

`AUDIT_RETENTION_DAYS` (default 365, `0` keeps everything) still applies. For a chained ledger the daily prune finds the first event written inside the window and cuts just below it (found from the newest events down, so one row with an old timestamp sealed late cannot pull newer events into the cut), writes a `retention` checkpoint with its sequence number, hash and the number of events deleted, and deletes it and everything before it, all in one transaction. Verification then starts from that checkpoint. Deleting the oldest events any other way leaves no checkpoint and is reported as missing events.

## Checkpoints and anchoring

`audit_checkpoint` keeps the points a verifier can start from or compare against: `genesis` (seq 0), `retention` (the last pruned event) and `periodic`, a copy of the head that the scheduler writes at most once an hour when the head has moved. Verification checks every checkpoint against the event at its sequence number, and a checkpoint past the head means the newest events were deleted. Before writing a periodic checkpoint the scheduler checks the events since the previous one, so it never endorses a head that does not follow from it. Verify reads the checkpoints before the head and stops at that head, so events sealed while it runs are newer, not errors. If the oldest stored event has no checkpoint under it, it reads once more, so a retention cut committed between its reads is not reported as missing events. A cut that commits while the walk is paging can still show as missing events at the start; run the check again (retention runs once a day).

Checkpoints in the same database only help against partial tampering. Someone who can write to the database can rewrite events, recompute every hash and move the head and checkpoints to match. Defending against that means keeping checkpoints somewhere they cannot reach. This release does not do that automatically (C07 stays Partial). To do it by hand: copy the newest `periodic` checkpoint (`seq` and `hash`) from an export into a place you control (a ticket, a signed email, an object-lock bucket), and pass it to the offline verifier later with `--checkpoint SEQ:HASH`. The planned design for the automatic version is in the PR03 description: sign each periodic checkpoint with a key derived from `ENCRYPTION_KEY` and optionally publish it to a configured write-once destination.

## What this protects against, and what it does not

External anchoring is not built, so this list is what the chain proves on its own.

**Detected by `verify` and by the offline script:**

- an edited event, including only its detail (in the app, while the key is unchanged);
- a deleted, duplicated or reordered event in the middle;
- a deleted head while events or checkpoints remain (the export is marked broken, and an empty export fails against any kept checkpoint);
- a completed access review changed after completion, even with its `result_hash` recomputed (the chained completion event still names the original; `--review` and the review export's `chain_matches`);
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
- **Offline:** `python3 scripts/audit/verify-audit-export.py export.json [--checkpoint SEQ:HASH ...] [--partial] [--expect-empty] [--review review.json]`. Standard library only. Exit 0 when it verifies, 1 at the first break (printed), 2 when a file cannot be read. Each file states its range in a header (first and last event, the ledger's head when it was made, and how many events were still unsealed), and the script checks the header against the events and the head. Checkpoints you kept (`--checkpoint`) are checked like this:
  - inside the file, or at the head the file records: they must match;
  - above that head: always a failure (newer events were deleted, or the head wound back);
  - older than a retention cut the file starts from: reported, not a failure, so a full export of a log the retention window has cut verifies **without** any flag;
  - between the file's last event and its head, or before its first event with no retention cut: a failure unless `--partial`. That flag is only for a file that deliberately covers part of the log (a date range or a continuation file), never the routine path.

  A whole-log file with no events fails when you pass any checkpoint, and otherwise needs `--expect-empty`; a file the server marked broken (for example events with no head) fails. `--review` checks a completed access review export against the log export: its snapshot and result hashes, and that the chained completion event commits to that result. `scripts/tests/verify-audit-export.test.sh` covers each case.

## Exporting

Settings > Activity log > Download, or `GET /api/admin/audit/export?from=YYYY-MM-DD&to=YYYY-MM-DD&ledger=org|instance` (admin). The range is by time (`from` inclusive, `to` exclusive, either may be left out), and the file holds the contiguous run of sequence numbers from the first event in the range to the last. Each event carries its canonical fields and its hash. The file also carries the anchor the first event hangs from, the stored checkpoints inside the range, an `export` checkpoint every 1,000 events and one at the last event, the head at export time, and the server's own check. At most 50,000 events per file: `truncated` and `next_seq` say where to continue. The header (`range`) also says how many events were still unsealed when the file was made; they are not in it, and the verifier prints the count. When a ledger has events or checkpoints but no head, the export carries no events and is marked broken, instead of looking like an empty log.

The export is metadata only: who, what, when, on which record and the outcome. The free-text detail (which can hold an email address, a token name or a count) is replaced by its keyed digest (see Hash), so the file holds no memory text, chat content, prompts or secrets, cannot be dictionary-reversed, and still verifies. The test `the_export_holds_metadata_only_and_verifies_offline` writes tenant content through a real tool and a detail canary and checks neither appears in the file. Each export is itself recorded in the log (`audit.export`).

## Access reviews

Settings > Access reviews (admin). Starting a review takes a snapshot of who has access to what in the workspace: every person, their role and why they are an admin, whether they are suspended, their spaces and role in each, their API tokens (name, scopes, restricted space, dates), connected apps (name, client id, scopes, dates) and the number of live browser sessions. Never a token, password or other secret. One review can be open at a time.

Admins then record a decision per person: keep, remove or suspend, with an optional note. A decision is a record, not an action: suspending or removing someone is still done from People. An admin cannot decide about their own access (`access_review.self_review`). Completing the review needs a decision for everyone except the admin completing it; their own access is recorded as not reviewed when no other admin decided it. This PR adds no separate reviewer role or separation-of-duties rules (planned in PR04 and PR09).

A completed review cannot change: the API refuses (`access_review.completed`) and so does the database (events in tenant migration 0044 refuse any update or delete of the review and its decisions). Its start, every decision and its completion are in the hash-chained log (`access_review.start`, `access_review.decide`, `access_review.complete`). The completion event's detail carries `result_hash`, the SHA-256 of the review's canonical JSON (keys sorted, no spaces): the review id, the snapshot's hash, who started and completed it and when, every decision and who was not reviewed. Download a completed review from its row or with `GET /api/admin/access-reviews/{id}/export`: the file has that document, both hashes, whether they still match, the completion event's sequence number, hash and detail, and `chain_matches`: whether that chained detail still names this document's hash. Check it offline with `verify-audit-export.py LOG.json --review REVIEW.json`. Reviews live in the org's own database, since they name people and spaces.

## Migration numbers

Migrations are numbered in merge order (`backend/migrations/QUEUE.md`). The scheduler queues an org's tenant migration only while its version is below the latest, so a number left out would never be applied once it lands. `scripts/ci/check-migrations.sh` fails CI on a gap, and `migrate::require_contiguous` makes a binary with a gap refuse to boot ("this build registers migrations with a gap"). Test builds (feature `test-support`) and `EUNOMIA_ALLOW_MIGRATION_GAP=1` (a developer running a lane before the numbers ahead of it land) only log it. **A database migrated by such a build is past a number it lacks, for good: throw it away.**

At boot the server also reads every org's migration ledger and looks for a registered migration below the newest one that org applied (`Provisioner::find_migration_gaps`). Such an org is reported by id on stderr and in the log, and is not served (`tenant.schema_behind`, naming the missing numbers) rather than served with a migration silently never applied. To repair: read the missing migration's notes (it will run after newer ones), then restart once with `EUNOMIA_REPAIR_MIGRATIONS=1`, which applies the missing migrations to those orgs at boot and serves them again. Remove the variable afterwards. This adds one read per org to every boot.

## Known limits

- **Cancellation (#219).** Only routes that use `audit::detached` keep an action and its event together when the client disconnects mid-action: access reviews and the log export. Every other audited route still records its event after the action, so a client dropping out between the two can leave a committed action without its event. Moving each route to `audit::detached` is mechanical; it was left out of this change to keep it reviewable.
- **Retention racing verify.** See Checkpoints: a cut that commits mid-walk can read as missing events once; the next check is clean.
- **Rolling upgrade.** Replicas still on the previous release write events with no ledger (they are never chained), and their retention job deletes by age without the checkpoint. If one of them runs the daily prune during the rollout, `verify` reports the cut as missing events. Upgrade all replicas together, or stop the old workers first; a single-process self-host upgrade is not affected.
- **Clocks.** Event times come from the writing process's clock. Sequence order, not time order, is what the chain proves; an export by date range takes the contiguous run from the first event at or after `from` to the last before `to`.
- **Access-review decisions** are records only: suspending or removing a person is still done from People. A lone admin's own access is recorded as not reviewed. There is no reviewer role or separation of duties (PR04 and PR09).
