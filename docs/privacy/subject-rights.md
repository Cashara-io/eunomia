# Data-subject export and erasure

How Eunomia answers "give this person all their data" and "delete this person, and prove it". Readiness catalogue rows C21 and C22 ([controls.json](../compliance/controls.json)). Code: `backend/src/subject/`, `backend/src/routers/subject.rs`, `backend/src/store/subject.rs`. Proof: `backend/tests/subject_rights.rs`, `scripts/tests/backup.test.sh`.

An erasure is only as good as the inventory of where a person's data lives. The inventory below is the contract: every row names what the export includes and what the erasure does. A new table that can name a user must add a row here, a case in `store::subject::RESIDUE` or `CONTROL_RESIDUE` (the post-check), and a seed in `tests/subject_rights.rs`.

## Who can do what

"Server operator" (operator, for short) means one thing, decided in one function (`authz::is_server_operator`): an account whose email is listed in `EUNOMIA_ADMIN_EMAILS`, or the install's first account (the one that set up the server). The in-app admin flag (Settings > People, "Make admin") does not make anyone an operator. "Workspace owner" is the one `owner` membership of the workspace.

| Action | Who | Where | Scope and confirmation |
| --- | --- | --- | --- |
| Export my data | The person only | Settings > Data, `POST /api/export/subject` | Browser session (not an API token or app). Everything about them in every workspace they belong to. Only they can download it, never an admin, suspended or not |
| Delete my account and data | The person | Settings > Data, `POST /api/account/erasure` | The whole account, every workspace. Browser session and the password typed again; an account with single sign-on only needs a sign-in from the last 10 minutes (`subject.reauthenticate` otherwise). The only member of a workspace also confirms that it closes with them (`close_workspace`). Refused while they own a workspace that has other members, until ownership is handed over |
| Export a workspace's data about a member | Any admin of the workspace (operator or not) | Settings > People, `POST /api/admin/people/{id}/export` | Browser session. Only that workspace's data about the person (see "Export scope"). The admin downloads it from their Settings > Data. The person's own activity (Settings > Data) shows `subject.export_request_admin` and, when the admin downloads it, `subject.export_download_admin`, each naming the admin. Refused for an account that belongs to more than one workspace (see "Limits") |
| Place a legal hold | Any admin of the workspace (operator or not) | Settings > People, `PUT /api/admin/people/{id}/legal-hold` | Browser session; records who placed it (`user.legal_hold_by`) |
| Erase someone's account | An operator | Settings > People, `POST /api/admin/people/{id}/erasure` | The whole account in every workspace, exactly as if the person had asked. Browser session and **the operator's own password** (single sign-on only: a sign-in from the last 10 minutes). `close_workspace` confirms closing a workspace the person is the only member of. Not for the operator's own account (use Settings > Data) |
| Lift a legal hold | An operator, never the person the hold is on | the same route as placing it | The lift is audited with who lifted it and who placed it |
| Cancel an erasure | An operator | Settings > People, `POST /api/admin/erasures/{id}/cancel` | Only before it has taken its first irreversible step (`deleting_at` unset). Clears its working record and the person's id and leaves the account exactly as it was (unsuspended, unless it was suspended before the request). Once deletion has begun it is refused (`db.conflict`): the erasure completes, or waits (`held`, or `failed` and retried) |
| Hand the workspace over | Its current owner, or an operator | Settings > People, `PUT /api/admin/people/{id}/workspace-owner` | To an active member who is not suspended, not held and not being erased. One transaction promotes exactly one membership or changes nothing, so the workspace always has an owner. Audited (`org.owner_transfer`) under the actor and the previous owner, naming the previous owner |
| Erasure records | Any admin of the workspace | Settings > People, `GET /api/admin/erasures` | Read only |
| Re-apply erasures after a restore | Runs when the scheduler leader starts; an operator can run it | `POST /api/admin/erasures/reapply` | Browser session |

An admin who is not an operator can export a workspace's data about a member and place a legal hold, and nothing else here.

The first account and every operator account are erased by nobody but themselves (and the first account not even by itself, see `subject.last_owner`). The check runs on every request path, including a request that joins an erasure already open.

**Erasure by an organisation's own admin is not in this release.** An earlier draft let a workspace admin erase a member from their workspace only; it was removed in review so that this release has one erasure path, always the whole account. Cancelling an erasure that has started deleting was removed too: it could leave an account half erased, with nothing in the product to finish or undo it. Until it returns, a customer who needs a departed member erased asks the server operator, who erases the whole account (Settings > People, "Delete account and data"). If the member also belongs to another workspace, that is the whole account too; the operator agrees it with that workspace's owner first.

Refusals, all before anything is touched (see [errors.md](../errors.md)):

- `subject.confirmation_failed`: a wrong password (the person's own, or for an operator the operator's own).
- `auth.forbidden`: the caller is not an operator (erasing someone, cancelling, lifting a hold, re-applying), the target is an operator's account, or a hold is lifted by the person it holds.
- `subject.reauthenticate`: an account with no password whose sign-in is older than 10 minutes. Sign in again, then confirm.
- `subject.legal_hold`: the account is held.
- `subject.workspace_closes`: the person is the only member of a workspace and has not confirmed that it closes.
- `subject.last_owner`: the person owns a workspace that has other members and nobody else owns it (the owner or an operator hands it over first), or it is the account that set up the server. That account is always an admin (`authz::is_admin`); erasing it would silently make the next-oldest account an admin, so the app refuses and the operator removes it by hand.

Once confirmed, a whole-account erasure suspends the account and signs it out everywhere at once (sessions revoked, cached credentials dropped, tokens and apps refused by `load_user`), so nothing new is written while the job runs, and **access cannot be restored while it is open** (pending, running, held or failed): unsuspending answers `auth.forbidden` until it finishes or an operator cancels it before it starts deleting. The `erasure` row and its job are created in one transaction, and `open_slot` (unique per person, matched under the current and previous keys) makes two requests for one person one erasure. A repeat request joins the open one, adds a close confirmation it carries, and gives it a job if it has none; the scheduler leader does the same for every open erasure when it starts. The workspace's activity log keeps who asked: an operator's request is filed under them (and under the person, so it shows in their own activity), a person's own under someone who stays in the workspace, and the person appears only as the first characters of the opaque reference.

### A workspace whose only member is the person

It closes with them, after the explicit confirmation above. Everything in it is erased: every space and its rows, every document and its file in the bucket, connector data, then every org table is emptied (the post-check counts every table and requires 0). The workspace is then marked closed (`tenant.status = closed`, `org.closed_at`): nothing is served from it again (`Pool::for_org` answers "This workspace was closed.") and sign-up never joins it. There is no path in the app that removes an org database, so **the empty database remains for an operator to drop** (`REMOVE DATABASE org_<key>;` as the SurrealDB root user, after a backup if wanted).

### Two erasures at once

A running erasure holds a lock (`erasure_lock`, one row per workspace, taken in one transaction) on **every workspace it touches**; another erasure that needs any of them waits and retries a few seconds later without spending an attempt. Someone with an erasure open (pending or held) or under a legal hold **still counts as a reader** of every space they are in: those spaces stay, and so does what they wrote. They are never chosen to inherit rows or ownership. When everyone left in a workspace is being erased, each goes in turn (ownership passing to one who is still there), and the last of them closes the workspace. A dead job's locks are released when the next job for that erasure starts (the start-up heal gives every open erasure a job).

## The rule for shared spaces

**What the person wrote in a space other people can still read stays, reassigned to an owner who remains; the person is removed from it as an author.** "Other people" includes anyone under a legal hold or with an erasure of their own open. If every other reader is one of those (so nobody may inherit), the space and everyone else's content stay and the person's own rows there are deleted instead of reassigned. Facts, entities, relations, documents (with their files), skills, sharing links, password-manager references, held secrets and secret requests the person created move to an active owner of that space. Two exceptions, because their record ids embed the person's key: records synced from the person's own sources and filed in the space are deleted (the sources go with the account; facts drawn from them stay, without the link), and the person's documents there are indexed again as the new owner's, so their chunks get new ids. Relations the person wrote between other people's entities in the space are reassigned like the rest. If the person was its only owner, the longest-standing remaining member is made an owner first (counted as `ownership_handed_over`). The reasoning: a shared space is the team's record; deleting a colleague's contributions when they leave would damage everyone else's data, while the authorship link is what makes it the person's personal data.

Everything in a space only the person could read (their personal space, or a team space nobody else is left in) is deleted, whoever wrote it, then the space itself.

After that, the person's email address is replaced by `[erased person]`, matched **without regard to case** (`Alice@Example.COM` counts), wherever the system holds it: the text of facts, the names, summaries and aliases of every entity kind, the tool log's argument summaries (`audit_log`, other people's too), failure capsules' stored arguments, audit event details, and the actor names of decision trails. The facts lose their vector and are embedded again.

### Sync clients, peers and the change log

The erasure writes the same content-free change rows the product's normal paths write: `deleted` for the person's facts, entities, relations, skills and documents it deletes outside the spaces that go whole, `updated` for every fact and entity whose text the email scrub rewrites, and a space-deleted event for each space that goes whole. A client syncing with `changes_since` therefore drops or refetches them. **Copies already held by another Eunomia server a space was shared with (peer vaults) are outside this server's reach**: that server's operator erases them there.

### What is not searched, and content other people wrote

- **Names are not searched.** Only the email address and the account's ids (user id, token ids, ids that embed the user key) are matched. A name or a description of the person in prose ("Alice from finance") is not recognised: an admin who must remove that deletes the entity or fact by hand.
- **Content other people wrote that mentions the person is theirs**: their chat messages, skills, synced records (an email in their mailbox), recordings and property values. It is not deleted or rewritten. The post-check counts mentions of the person's email in it, case-insensitively, and the completion record keeps them as `review.mentions_left_in_others_content` (per kind), shown on People as "mentions left in others' content, for review". These counts do not fail the erasure.

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
| `job` | `owner` | | deleted (the erasure job itself is owned by `user:erasure-job`, an account that never exists) |
| `failure_capsule` | `user` (as text), stored arguments | | the person's own: `user` cleared and stored arguments emptied; others': the email in their arguments replaced (any case) |
| `org`, `tenant` | nobody | | untouched, except a workspace that closes with its only member: `tenant.status = closed`, `org.closed_at` set |
| `job_leader` | nobody | | untouched |
| `erasure` | `subject` and `work` (the working record: email, token ids, key, file paths, the plan) only while open; `requester` (an admin) | | the completion record: kept, see below; `requester` cleared if that admin is erased |

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

The erasure job (`subject_erase`, `backend/src/subject/erase.rs`) works from a **working record** saved on the `erasure` row before anything is deleted: the email (lowercase), the person's token ids and key, every bucket file path that must go, and the plan per workspace (who inherits, which spaces stay and which are theirs alone, whether the workspace closes). **Every run rebuilds the plan from live state and merges it with the saved record** (identifiers, files and workspaces from both), so a run resumed after a hold sees files, tokens and workspaces added meanwhile, and a retry after steps that already ran still checks everything the first run knew.

**Where the working record lives, and for how long.** It holds personal data (the email, token ids, file paths). It is on the `erasure` row while the erasure is open: pending, running, **held** (a hold can keep it for months) or **failed** (an erasure that keeps failing keeps it until it passes). It is cleared when the erasure finishes (`done`) and when an operator cancels it before it starts deleting (`cancelled`). While it is open it is also in every nightly backup of the control database taken meanwhile. A database error logged by a failed step can quote a row in the server log (warn level), so keep server logs on the same retention as the data. To stop an erasure that should not finish, an operator cancels it before it starts deleting, which clears the record; after that, a legal hold parks it.

Order: legal hold check; a lock on every workspace touched; the working record (rebuilt and merged); the hold checked again; `deleting_at` set (from here it can no longer be cancelled); every workspace's steps; the **org post-checks**; the hold checked again; the control database (the account goes here, last); the control post-check; closing the workspaces that close; the completion record. A post-check counts every table in the inventory for the person (by id, token ids, key-bearing ids and the email, case-insensitively, where a table can hold it) and checks that every captured bucket file is gone. **Any residue fails the job before the account is deleted**: the record is marked `failed` with the tables still naming the person, the account stays (suspended), and the job retries with the same working record. `done` is never written while residue exists.

When the post-check is clean the `erasure` row becomes the completion record: status `done`, per-category counts, the post-check (every value 0), the review counts (mentions in other people's content), and `subject` and `work` cleared. It keeps only `subject_ref`, a keyed hash (HMAC-SHA256 under `ENCRYPTION_KEY`) of the user id, which names nobody without the key. The same record is written to the audit trail as a `subject.erase` event (actor `system`, filed under someone who stays in the home workspace so only that workspace's log shows it). The `erasure` table is never pruned, so the record outlives the person and the audit retention window.

Tests (`backend/tests/subject_rights.rs`):

- `erasure_removes_every_category_and_proves_it` seeds a person in every category above, including their email in another case in a colleague's fact and entity name, a relation they wrote in a shared space and a colleague's chat that mentions them; erases them through the route and the job; then scans every row of every table in both databases for their key or email (a colleague's own chat may keep its words, and is counted for review), checks the bucket, the reassignments, the workspace log, the resync stand-down, and the two restore re-applies (control and org restored; only the org restored).
- `a_failing_post_check_keeps_the_account_and_every_retry_checks_again` removes the case-insensitive scrub of facts (`isolation::mutate`): the first run reports `memory`, every retry reports it again with the captured email, the account is kept and `done` is never written; with the scrub back, the next retry passes.
- `someone_held_or_being_erased_still_reads_a_shared_space_and_keeps_their_content`: A is held and B shares a space only with A; C has a held erasure and D shares a space only with C. B and D are erased; both spaces stay with A's and C's content, owned by them, and neither inherits.
- `only_an_operator_erases_someone_and_always_the_whole_account`, `single_sign_on_needs_a_recent_sign_in_and_an_operator_can_cancel` (cancel before deletion leaves the account as it was; after `deleting_at` it is refused), `ownership_hand_over_holds_and_space_names_and_the_change_log` (who placed a hold; an admin cannot lift it; an operator cannot lift a hold on themselves).
- `a_hold_placed_after_the_request_parks_the_erasure_until_it_is_lifted`, `two_erasures_in_one_workspace_keep_it_owned_and_never_hand_rows_to_each_other`, `a_request_left_without_a_job_is_finished_by_the_next_request_or_the_next_start`, `refusals_admin_reauthentication_and_closing_a_workspace` and `with_one_control_removed_the_detectors_go_red` (the relation reassignment and the org-only restore sweep, each removed in turn).

## Resync and restore suppression

The `erasure` rows are the tombstone list.

- **Resync.** `sources::scheduler::sync_source` stands down with `{"status": "erased"}` for an account that has an erasure on record (requested, running, held or done), so a sync job queued before the erasure, or retried after it, cannot bring the person's records back as theirs. The erasure deletes the person's connectors, so nothing schedules a new one. Not covered: another person's connector importing records that mention the erased person (an email they sent, say); that is the other person's data, counted for review.
- **Restore.** Every restore that replaces `control` reads the live `erasure` table first and merges it back (`INSERT IGNORE`) after `control` is imported: a backup directory, and a single `control.surql.enc` file (a single `control.surql.enc` or `org_<key>.surql.enc` goes back into its own database). If the live records cannot be read, or the server has no control database to read them from (a fresh server in a disaster recovery), the restore **stops before touching anything**, saying why. `--erasures FILE` supplies an off-box copy made with `eunomia-backup erasures FILE` (keep one off the machine, for example after each erasure), and `--without-erasures` restores anyway. `backend/scripts/restore.sh` passes both through.
- **Re-apply.** When the scheduler leader next starts (the backend restarts after a restore), `subject::reapply_on_start` gives every open erasure a job and runs `subject::reapply_erasures`: (a) every live account whose reference is on the list is erased again (`requested_by: reapply`); (b) in every workspace, rows whose owner the control database does not have, but whose reference is on the list (a restore of only an org database), are erased by owner id (the email is not known then, so only ids are matched). The re-apply keeps the "workspace keeps an owner" rule: if the restored account owns a workspace with other members, ownership is handed to the longest-standing member who stays before the account goes; a workspace whose only member it is closes again. Holds are not consulted (the person was already erased). One failure is logged and the rest go on. An admin can also run it from `POST /api/admin/erasures/reapply`. A workspace that was closed stays closed when only its org database is restored (control still says closed); the data then sits unreachable in that database until an operator drops it.
- **Key change.** A reference is one-way, so tombstones cannot be re-keyed. After changing `ENCRYPTION_KEY` (see [deployment.md](../deployment.md#rotating-encryption_key)), put the old key in `ENCRYPTION_KEY_PREVIOUS` (comma separated for several) for as long as backups from before the change exist: matching then tries the current and every previous key. New tombstones use the current key.

## Legal hold

`user.legal_hold_at` (control migration 0011) is set by any admin from Settings > People, and `user.legal_hold_by` records who; only an operator lifts it, and never the person it holds (an operator cannot lift a hold on their own account). A held person still counts as a reader of their spaces in everyone else's erasure (see "Two erasures at once"). While set, both erasure routes refuse with `subject.legal_hold`. A hold placed **after** the request still stops it: the job checks the hold at its start, again just before its first irreversible step, and again before the account goes; it parks as `held` (nothing more is deleted) and resumes when the hold is lifted. The hold route reports `erasure_open` so the admin knows. Placing and lifting are audited. A hold does not pause the retention windows (`AUDIT_RETENTION_DAYS`, `SESSION_RETENTION_DAYS`, `TOOL_USAGE_RETENTION_DAYS`) or item deletes by the person; it only stops the subject erasure. A hold placed while a job is already between its org steps and its control step stops it before the account goes, but what was already erased in the workspaces stays erased.

## Retention and erasure

Retention (the `prune_auth` job, [deployment.md](../deployment.md)) and erasure do not fight: retention deletes audit events, sessions and usage counters by age; erasure anonymises the person's audit events in place and deletes the rest. The erasure's own `subject.erase` audit event is pruned by the audit window like any other, but the `erasure` row is not, so the proof survives. Export archives are kept for seven days and pruned by the scheduler.

## Export scope and redaction

**The person's own export** (`eunomia-data-export.zip`) covers every workspace they belong to. It holds `manifest.json`; once, the account-level files `account.json` (account, sign-in identities, organisations), `api_tokens.json`, `sessions.json`, `connected_apps.json`, `audit_events.json`, `usage.json`; and per workspace, under `workspaces/<key>/`, `spaces.json`, `entities.json`, `memories.json`, `relations.json`, `skills.json`, `documents.json` and `documents/<id>/<name>` (the files), `synced_records.json`, `chat.json`, `tool_log.json`, `sources.json`, `recordings.json`, `extensions.json`. **Each workspace's part is stored in that workspace's own database** (a `subject_export` row with the same id; the home workspace's row also holds the account part and the manifest), so no workspace's database holds another's data; the download assembles the parts for the person. Only the person starts it and only the person downloads it.

**An admin's export** holds only the admin's workspace's data about the person: what they wrote in its shared spaces (with the document files), their spaces there, their account record (email, creation date), API tokens and connected apps by name, and audit events about them (admin actions that name them, and their own space and data-rights events; these events are account-wide in the ledger, not tagged by workspace). It excludes their personal space, private chat, synced records, sources, recordings, settings and every other workspace. It is stored in that workspace's database and the admin downloads it.

Both kinds start and download only from a browser session, expire after **seven days** and are then deleted by the scheduler. At rest they are ordinary rows in SurrealDB: a server operator with database access can read them, and they are in the nightly backups taken while they exist. The largest archive kept is `EUNOMIA_EXPORT_MAX_MB` (default 64); a bigger one fails with a message saying so.

Redacted (third-party protection):

- Content in a space the person can no longer read is left out, including what they wrote there.
- In a shared space, only what the person wrote is included; other members' facts, entities and documents are theirs.
- A chat message whose stored text names a space the person can no longer read is replaced by a notice (the decision for #230: redact on export, with a clear indication). Assistant prose that repeats a name from such a space without its id is not detected.
- Other people's email addresses in the person's audit events are replaced by `[another person]`.

Not redacted: text the person wrote, or that was synced into their own account, that mentions other people (their own notes about a colleague, an email in their mailbox). That is the person's own data, and the export returns it as stored.

Left out: secret values of any kind (password and token hashes, connector credentials, password-manager references, provider keys), connector configuration and settings, embeddings, recording audio, and backups.

The previous `GET /api/export` (personal space as JSON) is unchanged for scripts that use it.

## Limits

- Erasure and export cover every org the person belongs to; the confirmation and the owner checks cover them all.
- The account that set up the server cannot be erased in the app.
- Accounts with single sign-on only confirm with a sign-in from the last 10 minutes. Eunomia's OIDC sign-in does not send `prompt=login` or `max_age`, so an identity provider that still has a session signs the person in again without asking for credentials; the check proves a recent sign-in, not a recent password.
- An admin's export reads the person's API tokens, connected apps and audit events account-wide (they are not tagged by workspace). That is right only while an account belongs to one workspace, as every account does today; an admin export of an account with more than one membership is refused (`auth.forbidden`) rather than show one workspace's admin another's activity.
- An organisation's own admin cannot erase a member (see "Who can do what"); the operator does it, always for the whole account.
- An erasure that has started deleting cannot be cancelled; a legal hold parks it, and it completes when the hold is lifted.
- A self-hosted install with replicas runs the re-apply on whichever process becomes the scheduler leader.
- Free-text mentions of the person by name are not found (see the shared-space rule).
- Backups are not rewritten; they age out by rotation.
- **A closed workspace restored on its own** (one `org_<key>.surql.enc`, control untouched) brings its content back into a database nothing serves (control still says closed) and the re-apply does not sweep it. An operator drops that database again.
- An org-only re-apply knows the person only by id (the email is gone), so restored mentions of their email in shared content are neither scrubbed nor checked.
- A space whose remaining readers are all held or being erased keeps no owner from the erased person (they may not inherit); its other content stays as it is.
