# Data-subject export and erasure

How Eunomia answers "give this person all their data" and "delete this person, and prove it". Readiness catalogue rows C21 and C22 ([controls.json](../compliance/controls.json)). Code: `backend/src/subject/`, `backend/src/routers/subject.rs`, `backend/src/store/subject.rs`. Proof: `backend/tests/subject_rights.rs`, `scripts/tests/backup.test.sh`.

An erasure is only as good as the inventory of where a person's data lives. The inventory below is the contract: every row names what the export includes and what the erasure does. A new table that can name a user must add a row here, a case in `store::subject::RESIDUE` or `CONTROL_RESIDUE` (the post-check), and a seed in `tests/subject_rights.rs`.

## Who can do what

| Action | Who | Where | Confirmation |
| --- | --- | --- | --- |
| Export my data | The person | Settings > Data, `POST /api/export/subject` | Signed in (any credential with the account scope) |
| Export someone's data | A workspace admin | Settings > People, `POST /api/admin/people/{id}/export` | Browser session; the person must be in the admin's workspace |
| Delete my account and data | The person | Settings > Data, `POST /api/account/erasure` | Browser session (not a token or app) and the account password typed again; an account with single sign-on only types its email |
| Delete someone's account and data | A workspace admin | Settings > People, `POST /api/admin/people/{id}/erasure` | Browser session and the person's email typed again; not for the admin's own account |
| Legal hold | A workspace admin | Settings > People, `PUT /api/admin/people/{id}/legal-hold` | Browser session; placing and lifting are both audited (`subject.legal_hold`, `subject.legal_hold_lift`) |
| Erasure records | A workspace admin | Settings > People, `GET /api/admin/erasures` | |
| Re-apply erasures after a restore | Runs when the scheduler leader starts; an admin can run it | `POST /api/admin/erasures/reapply` | Browser session |

Refusals, all before anything is touched: `subject.confirmation_failed` (wrong password or email), `subject.legal_hold` (the account is held), `subject.last_owner` (the workspace's last owner, the only account in a workspace, or the account that set up the server). The account that set up the server is always an admin (`authz::is_admin`); erasing it would silently make the next-oldest account an admin, so the app refuses and the operator removes it by hand. See [errors.md](../errors.md).

Once confirmed, the account is suspended and signed out everywhere at once (sessions revoked, cached credentials dropped, tokens and apps refused by `load_user`), so nothing new is written while the erasure job runs. A second request while one is open returns the open one.

## The rule for shared spaces

**What the person wrote in a space other people can still read stays, reassigned to an owner who remains; the person is removed from it as an author.** Facts, entities, relations, documents (with their files), skills, sharing links, password-manager references, held secrets and secret requests the person created move to an active owner of that space. Two exceptions, because their record ids embed the person's key: records synced from the person's own sources and filed in the space are deleted (the sources go with the account; facts drawn from them stay, without the link), and the person's documents there are indexed again as the new owner's, so their chunks get new ids. If the person was its only owner, the longest-standing remaining member is made an owner first (counted as `ownership_handed_over`). The reasoning: a shared space is the team's record; deleting a colleague's contributions when they leave would damage everyone else's data, while the authorship link is what makes it the person's personal data.

Everything in a space only the person could read (their personal space, or a team space nobody else is left in) is deleted, whoever wrote it, then the space itself.

After that, the person's email address is replaced by `[erased person]` wherever it appears in the text of facts, in entity names, summaries and aliases, and in other people's tool log (`audit_log`) anywhere in the workspace (the facts lose their vector and are embedded again). A name or a description of the person written in prose ("Alice from finance") is not recognised: an admin who must remove that deletes the entity or fact by hand.

## Inventory

`U` is the person's user record, `E` their email. "Deleted" means the row is removed; "anonymised" means the row stays but no longer names them.

### Control database

| Table | Names the person by | Export | Erasure |
| --- | --- | --- | --- |
| `user` | the row itself, `email`, `email_lc` | account fields (never the password hash) | deleted (last, in one transaction with the rest) |
| `identity` | `user`, `email`, `subject` | issuer, provider subject, email, dates | deleted |
| `membership` | `user` | org and role | deleted |
| `api_token` | `owner` | name, scopes, dates (never the hash) | deleted |
| `session` | `owner`, `user_agent` | user agent, dates | deleted |
| `oauth_grant`, `oauth_token`, `oauth_code` | `owner` (tokens through the grant) | app name, client id, scope, dates | deleted |
| `oauth_client` | nobody (an instance-wide client registry) | | untouched |
| `audit_event` | `user`, `actor_id` (user or token id), `target`, `detail` (emails) | their events, other people's emails replaced | anonymised: `user` becomes `user:erased` (an account that never exists, so no workspace log lists it), actor ids that were theirs and targets that are or embed their id (an extension id carries the owner's key) become `erased`, `E` in any detail becomes `[erased person]`. The security ledger keeps its rows and is pruned only by `AUDIT_RETENTION_DAYS` |
| `tool_usage` | `user` | per hour, tool and channel | deleted |
| `job` | `owner` | | deleted (the erasure job itself is owned by the workspace heir, see below) |
| `failure_capsule` | `user` (as text) | | anonymised (`user` cleared) |
| `org`, `tenant`, `job_leader` | nobody | | untouched |
| `erasure` | `subject` only while pending or running | | the completion record: kept, see below |

### Org database (each org the person belongs to)

| Table | Names the person by | Export | Erasure |
| --- | --- | --- | --- |
| `vault`, `vault_member` | membership `user`; `ontology_actor`, `ontology_actor_name` | spaces with role and status | memberships deleted last; spaces only they could read deleted; actor fields anonymised |
| `person`, `organisation`, `location`, `repository`, `file`, `symbol`, `entity` | `owner` | what they authored in spaces they can still read | shared: reassigned; theirs alone: deleted; `E` in names, summaries and aliases replaced |
| `memory` | `owner`, text | what they authored in spaces they can still read | shared: reassigned; theirs alone: deleted; `E` in text replaced |
| `relates_to`, `linked_to` | `owner`; edges to their rows | relations they authored between readable entities | shared: reassigned; otherwise deleted with either end |
| `entity_property` | through the space | | deleted with the space |
| `cache_record` (synced records, document chunks) | `owner`, and the record id (it starts with the owner's key) | their synced records with no space or in readable spaces (chunks are covered by the documents) | deleted with their vectors, also in shared spaces (a reassigned document is indexed again as the new owner's) |
| `document` and its file in the `documents` bucket | `owner` | metadata, and the file itself under `documents/` | shared: reassigned with the file; otherwise the file is deleted from the bucket, then the row and chunks |
| `pocket_recording` and its audio file in the bucket | `owner`, `audio_path` | title, summary, transcript, speakers (not the audio) | file deleted, row deleted |
| `chat_thread`, `chat_message` | `owner` | every thread and message, see redaction | deleted |
| `audit_log` (tool calls) | `owner`; `E` in other people's argument summaries | tool, argument summary, outcome | theirs deleted; `E` in others' replaced |
| `connector`, `sync_status`, `composio_file` | `owner` | kind, label, enabled (never credentials or configuration) | deleted (credentials go with the row) |
| `app_settings` | `owner` | not exported (holds encrypted provider keys) | deleted |
| `extension`, `extension_event` | `owner` | extension names | deleted |
| `skill` | `owner` | skills in readable spaces | shared: reassigned; otherwise deleted |
| `peer_share`, `peer_link`, `peer_item` | `created_by`, `user` | | shared: reassigned; otherwise deleted |
| `credential_ref`, `vault_secret`, `secret_request` | `created_by`, `requested_by`, `decided_by`, `actor_id` | | shared: reassigned; otherwise deleted; `decided_by` cleared; `actor_id` anonymised |
| `secret_provider` | `created_by`, `updated_by` | | reassigned to the workspace heir (it is the workspace's password-manager setup) |
| `ontology_kind`, `_link`, `_property`, `_metric` | `decided_by`, `activity_actor`, `activity_actor_name` (the email) | | `decided_by` cleared, actor fields anonymised; deleted with a space only they could read |
| `ontology_event` | `actor`, `actor_name` | | anonymised |
| `change` | `actor` (user or token id) | | anonymised |
| `embed_cache` | nobody directly: keyed by a keyed hash of the text | | the org's whole cache is cleared (it only saves embedding calls), because the person's entries cannot be told apart |
| `subject_export` | `subject`, `requested_by` | | deleted (archives about or asked for by the person) |
| `change_seq`, `peer_instance`, `redact_backfill` | nobody | | untouched |

### Outside the databases

| Where | Erasure |
| --- | --- |
| The `documents` file bucket (`EUNOMIA_DOCUMENTS_BACKEND`) | Covered above. With an S3 backend the object store's own versioning or retention may keep copies: configure it to match. |
| Backups (`eunomia-backups` volume) | **Not rewritten.** A backup taken before the erasure still holds the person until it rotates out (14 daily and 8 weekly by default). Restoring one is covered by the re-apply below. Copies of backups outside the volume are the operator's to manage. |
| Logs and traces | Request logs carry user ids and trace ids, not content. They are kept by the operator's logging setup, outside Eunomia. |
| Data already sent to an AI provider or a connected service | Outside Eunomia. The erasure deletes the connector and its credentials; it cannot recall what the provider holds. |

## Proof: the completion record and the post-check

The erasure job (`subject_erase`, `backend/src/subject/erase.rs`) is owned by the workspace heir (an owner who remains), not the person, and its payload holds only the erasure id and the org keys. When the steps finish, a post-check counts every table in the inventory for the person (by id, token ids, and email where a table can hold it) and checks that every bucket file that was theirs is gone. Any count above zero fails the job, which marks the record `failed` with the tables still naming them and retries; a retry re-derives what is left.

When the post-check is clean the `erasure` row becomes the completion record: status `done`, per-category counts, the post-check (every value 0), and `subject` cleared. It keeps only `subject_ref`, a keyed hash (HMAC-SHA256 under `ENCRYPTION_KEY`) of the user id, which names nobody without the key. The same record is written to the audit trail as a `subject.erase` event (actor `system`, filed under the workspace heir so only that workspace's log shows it). The `erasure` table is never pruned, so the record outlives the person and the audit retention window.

The test `erasure_removes_every_category_and_proves_it` seeds a person in every category above, erases them through the route and the job, then scans every row of every table in both databases for their id or email and checks the bucket. `the_post_check_fails_when_one_category_is_not_deleted` removes one category's delete statement (`isolation::mutate`) and asserts the post-check names that table.

## Resync and restore suppression

The `erasure` rows are the tombstone list.

- **Resync.** `sources::scheduler::sync_source` stands down with `{"status": "erased"}` for an account that has an erasure on record (requested, running or done), so a sync job queued before the erasure, or retried after it, cannot bring the person's records back as theirs. The erasure deletes the person's connectors, so nothing schedules a new one. Not covered: another person's connector importing records that mention the erased person (an email they sent, say); that is the other person's data.
- **Restore.** `eunomia-backup restore` (and `backend/scripts/restore.sh`) exports the live `erasure` table before restoring a tenancy backup and merges it back (`INSERT IGNORE`) after `control` is imported, so the tombstones survive restoring a backup taken before them. When the scheduler leader next starts (the backend restarts after a restore), `subject::reapply_erasures` matches every live account against the tombstones and queues the erasure again for each match (`requested_by: reapply`). An admin can also run it from `POST /api/admin/erasures/reapply`. Holds are not consulted for a re-apply (the person was already erased). A restored account that is now alone in its workspace is logged and skipped. Changing `ENCRYPTION_KEY` changes every reference, so a key rotation must keep the old key to match old tombstones (not automated).

## Legal hold

`user.legal_hold_at` (control migration 0011) is set by an admin from Settings > People. While set, both erasure routes refuse with `subject.legal_hold`. Placing and lifting are audited. A hold does not pause the retention windows (`AUDIT_RETENTION_DAYS`, `SESSION_RETENTION_DAYS`, `TOOL_USAGE_RETENTION_DAYS`) or item deletes by the person; it only blocks the subject erasure.

## Retention and erasure

Retention (the `prune_auth` job, [deployment.md](../deployment.md)) and erasure do not fight: retention deletes audit events, sessions and usage counters by age; erasure anonymises the person's audit events in place and deletes the rest. The erasure's own `subject.erase` audit event is pruned by the audit window like any other, but the `erasure` row is not, so the proof survives. Export archives are kept for seven days and pruned by the scheduler.

## Export scope and redaction

The archive (`eunomia-data-export.zip`) holds `manifest.json` and one file per category: `account.json` (account, sign-in identities, organisations), `spaces.json`, `entities.json`, `memories.json`, `relations.json`, `skills.json`, `documents.json` and `documents/<id>/<name>` (the files), `synced_records.json`, `chat.json`, `tool_log.json`, `sources.json`, `recordings.json`, `extensions.json`, `api_tokens.json`, `sessions.json`, `connected_apps.json`, `audit_events.json`, `usage.json`. The manifest lists each category with its count, the document files included or missing, the redactions applied and what is left out. It is built by the `subject_export` job and kept for seven days, readable by the person and by the admin who asked for it. The largest archive kept is `EUNOMIA_EXPORT_MAX_MB` (default 64); a bigger one fails with a message saying so.

Redacted (third-party protection):

- Content in a space the person can no longer read is left out, including what they wrote there.
- In a shared space, only what the person wrote is included; other members' facts, entities and documents are theirs.
- A chat message whose stored text names a space the person can no longer read is replaced by a notice (the decision for #230: redact on export, with a clear indication). Assistant prose that repeats a name from such a space without its id is not detected.
- Other people's email addresses in the person's audit events are replaced by `[another person]`.

Not redacted: text the person wrote, or that was synced into their own account, that mentions other people (their own notes about a colleague, an email in their mailbox). That is the person's own data, and the export returns it as stored.

Left out: secret values of any kind (password and token hashes, connector credentials, password-manager references, provider keys), connector configuration and settings, embeddings, recording audio, and backups.

The previous `GET /api/export` (personal space as JSON) is unchanged for scripts that use it.

## Limits

- Erasure covers every org the person belongs to; the confirmation and the last-owner check cover them all.
- A self-hosted install with replicas runs the re-apply on whichever process becomes the scheduler leader.
- Free-text mentions of the person by name are not found (see the shared-space rule).
- Backups are not rewritten; they age out by rotation.
