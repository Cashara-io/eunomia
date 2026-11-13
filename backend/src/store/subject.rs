//! Statements for data-subject rights (`subject/`): the full export, erasure, its post-check, legal
//! holds and the erasure tombstones. See docs/privacy/subject-rights.md for the inventory these follow.
//!
//! Erasure statements are idempotent: a retried or re-applied erasure re-derives what to do from the
//! rows that are still there, so the vault memberships go last (they decide which spaces are the
//! subject's alone).

use super::{ControlStmt, Stmt};

// ------------------------------------------------------------------ org database: export

/// Every vault membership of the subject (active, pending or left), with the space's name and kind.
pub const MEMBERSHIPS: Stmt = Stmt::new(
    "subject.memberships",
    "SELECT <string> vault AS vault, vault.name AS name, vault.kind AS kind, role, status, created_at FROM vault_member WHERE user = $u ORDER BY created_at",
);

/// What the subject authored in the spaces they can still read: entities of every kind, facts,
/// relations, skills and documents. One statement, one result slot per table.
pub const AUTHORED: Stmt = Stmt::new(
    "subject.authored",
    "SELECT <string> id AS id, 'person' AS kind, name, aliases, summary, <string> vault AS vault, created_at FROM person WHERE owner = $u AND vault IN $readable; \
     SELECT <string> id AS id, 'organisation' AS kind, name, aliases, summary, <string> vault AS vault, created_at FROM organisation WHERE owner = $u AND vault IN $readable; \
     SELECT <string> id AS id, 'location' AS kind, name, aliases, summary, <string> vault AS vault, created_at FROM location WHERE owner = $u AND vault IN $readable; \
     SELECT <string> id AS id, 'repository' AS kind, name, aliases, summary, <string> vault AS vault, created_at FROM repository WHERE owner = $u AND vault IN $readable; \
     SELECT <string> id AS id, 'file' AS kind, name, aliases, summary, <string> vault AS vault, created_at FROM file WHERE owner = $u AND vault IN $readable; \
     SELECT <string> id AS id, 'symbol' AS kind, name, aliases, summary, <string> vault AS vault, created_at FROM symbol WHERE owner = $u AND vault IN $readable; \
     SELECT <string> id AS id, kind, name, aliases, summary, <string> vault AS vault, created_at FROM entity WHERE owner = $u AND vault IN $readable; \
     SELECT <string> id AS id, <string> subject AS subject, text, type, status, proof_count, <string> vault AS vault, occurred_at, created_at, updated_at FROM memory WHERE owner = $u AND vault IN $readable ORDER BY created_at; \
     SELECT <string> id AS id, <string> in AS from, <string> out AS to, label, created_at FROM relates_to WHERE owner = $u AND in.vault IN $readable AND out.vault IN $readable; \
     SELECT <string> id AS id, name, description, body, <string> vault AS vault, created_at FROM skill WHERE owner = $u AND vault IN $readable; \
     SELECT <string> id AS id, filename, media_type, size_bytes, sha256, object_path, status, <string> vault AS vault, created_at FROM document WHERE owner = $u AND vault IN $readable AND status != 'deleted'",
);

/// The subject's own records with no space, or in a space they can still read: synced records (not
/// document chunks, which the documents cover), chat, the per-user tool log, sources, recordings and
/// extensions. Never credentials or connector configuration.
pub const OWNED: Stmt = Stmt::new(
    "subject.owned",
    "SELECT <string> id AS id, source, type, external_id, title, body_text, url, occurred_at, payload, (IF vault THEN <string> vault ELSE NONE END) AS vault FROM cache_record \
       WHERE owner = $u AND source != 'documents' AND deleted = false AND (vault IS NONE OR vault IN $readable) ORDER BY occurred_at; \
     SELECT <string> id AS id, title, created_at, updated_at FROM chat_thread WHERE owner = $u ORDER BY created_at; \
     SELECT <string> id AS id, <string> thread_id AS thread_id, role, content, tool_calls, tool_call_id, created_at FROM chat_message WHERE owner = $u ORDER BY created_at; \
     SELECT tool_name, args_summary, outcome, created_at FROM audit_log WHERE owner = $u ORDER BY created_at; \
     SELECT <string> id AS id, kind, label, instance, enabled, updated_at FROM connector WHERE owner = $u; \
     SELECT <string> id AS id, recording_id, title, summary, transcript, speakers, tags, action_items, recorded_at, duration_seconds FROM pocket_recording WHERE owner = $u; \
     SELECT <string> id AS id, name, created_at FROM extension WHERE owner = $u",
);

pub const EXPORT_CREATE: Stmt = Stmt::new(
    "subject.export_create",
    "CREATE subject_export SET subject = $u, requested_by = $by, status = 'pending' RETURN <string> id AS id, status, created_at, expires_at",
);
pub const EXPORT_FINISH: Stmt = Stmt::new(
    "subject.export_finish",
    "UPDATE $id SET status = $status, manifest = $manifest, archive = $archive, size_bytes = $size, error = $error WHERE status = 'pending' RETURN NONE",
);
/// The exports `$u` asked for or that are about them (an admin's request for them included), newest first, without the archive.
pub const EXPORT_LIST: Stmt = Stmt::new(
    "subject.export_list",
    "SELECT <string> id AS id, <string> subject AS subject, <string> requested_by AS requested_by, status, manifest, size_bytes, error, created_at, expires_at FROM subject_export \
     WHERE (subject = $u OR requested_by = $u) AND expires_at > time::now() ORDER BY created_at DESC LIMIT 20",
);
pub const EXPORT_GET: Stmt = Stmt::new(
    "subject.export_get",
    "SELECT <string> id AS id, subject, requested_by, status, archive, expires_at FROM $id WHERE (subject = $u OR requested_by = $u) AND expires_at > time::now()",
);
/// The export job's row (no caller filter: the job checked the requester when it was queued).
pub const EXPORT_JOB_ROW: Stmt = Stmt::new("subject.export_job_row", "SELECT subject, requested_by, status FROM $id");
/// Retention: an archive is kept for seven days.
pub const EXPORT_PRUNE: Stmt = Stmt::new("subject.export_prune", "DELETE subject_export WHERE expires_at < time::now()");

// ------------------------------------------------------------------ org database: erasure

/// Who else can read each space: every active membership that is not the subject's, owners first,
/// then the longest-standing.
pub const OTHER_MEMBERS: Stmt = Stmt::new(
    "subject.other_members",
    "SELECT <string> vault AS vault, user, role, created_at FROM vault_member WHERE status = 'active' AND user != $u ORDER BY role DESC, created_at, id",
);

/// Makes `$to` an owner of `$vault` (the subject was its only owner).
pub const PROMOTE: Stmt = Stmt::new(
    "subject.promote",
    "UPDATE vault_member SET role = 'owner' WHERE vault = $vault AND user = $to AND status = 'active' RETURN NONE",
);

/// The rule for a space other people can still read: what the subject wrote there stays, owned by
/// `$to` (an owner who remains), and the subject's name goes from its decision trail. One transaction.
pub const REASSIGN: Stmt = Stmt::at(
    "subject.reassign",
    "BEGIN TRANSACTION; \
     LET $n = array::len((SELECT VALUE id FROM memory WHERE owner = $u AND vault = $vault)) \
        + array::len((SELECT VALUE id FROM person WHERE owner = $u AND vault = $vault)) + array::len((SELECT VALUE id FROM organisation WHERE owner = $u AND vault = $vault)) \
        + array::len((SELECT VALUE id FROM location WHERE owner = $u AND vault = $vault)) + array::len((SELECT VALUE id FROM repository WHERE owner = $u AND vault = $vault)) \
        + array::len((SELECT VALUE id FROM file WHERE owner = $u AND vault = $vault)) + array::len((SELECT VALUE id FROM symbol WHERE owner = $u AND vault = $vault)) \
        + array::len((SELECT VALUE id FROM entity WHERE owner = $u AND vault = $vault)) + array::len((SELECT VALUE id FROM document WHERE owner = $u AND vault = $vault)) \
        + array::len((SELECT VALUE id FROM skill WHERE owner = $u AND vault = $vault)); \
     UPDATE person SET owner = $to WHERE owner = $u AND vault = $vault RETURN NONE; \
     UPDATE organisation SET owner = $to WHERE owner = $u AND vault = $vault RETURN NONE; \
     UPDATE location SET owner = $to WHERE owner = $u AND vault = $vault RETURN NONE; \
     UPDATE repository SET owner = $to WHERE owner = $u AND vault = $vault RETURN NONE; \
     UPDATE file SET owner = $to WHERE owner = $u AND vault = $vault RETURN NONE; \
     UPDATE symbol SET owner = $to WHERE owner = $u AND vault = $vault RETURN NONE; \
     UPDATE entity SET owner = $to WHERE owner = $u AND vault = $vault RETURN NONE; \
     UPDATE memory SET owner = $to WHERE owner = $u AND vault = $vault RETURN NONE; \
     UPDATE relates_to SET owner = $to WHERE owner = $u AND (in.vault = $vault OR out.vault = $vault) RETURN NONE; \
     UPDATE cache_record SET owner = $to WHERE owner = $u AND vault = $vault RETURN NONE; \
     UPDATE document SET owner = $to WHERE owner = $u AND vault = $vault RETURN NONE; \
     UPDATE skill SET owner = $to WHERE owner = $u AND vault = $vault RETURN NONE; \
     UPDATE peer_share SET created_by = $to WHERE created_by = $u AND vault = $vault RETURN NONE; \
     UPDATE peer_link SET user = $to WHERE user = $u AND vault = $vault RETURN NONE; \
     UPDATE credential_ref SET created_by = $to WHERE created_by = $u AND vault = $vault RETURN NONE; \
     UPDATE vault_secret SET created_by = $to WHERE created_by = $u AND vault = $vault RETURN NONE; \
     UPDATE secret_request SET requested_by = $to WHERE requested_by = $u AND vault = $vault RETURN NONE; \
     RETURN $n; \
     COMMIT TRANSACTION;",
    20, // BEGIN 0, LET 1, eighteen updates 2 to 19, RETURN 20
);

/// The subject's email in what stays in the workspace (shared spaces and other people's own): replaced in the text of facts and in the
/// names, aliases and summaries of entities (the facts lose their vector and are embedded again).
pub const SCRUB_EMAIL: Stmt = Stmt::at(
    "subject.scrub_email",
    "BEGIN TRANSACTION; \
     LET $m = (UPDATE memory SET text = string::replace(string::replace(text, $email, '[erased person]'), $email_lc, '[erased person]'), embedding = NONE, embedding_spec = NONE \
        WHERE (string::contains(text, $email) OR string::contains(string::lowercase(text), $email_lc)) RETURN VALUE id); \
     LET $e = array::concat( \
        (UPDATE person SET name = string::replace(name, $email_lc, '[erased person]'), summary = string::replace(summary, $email_lc, '[erased person]'), aliases = aliases.filter(|$a| !string::contains(string::lowercase($a), $email_lc)) \
           WHERE (string::contains(string::lowercase(name), $email_lc) OR string::contains(string::lowercase(summary), $email_lc) OR aliases.any(|$a| string::contains(string::lowercase($a), $email_lc))) RETURN VALUE id), \
        (UPDATE entity SET name = string::replace(name, $email_lc, '[erased person]'), summary = string::replace(summary, $email_lc, '[erased person]'), aliases = aliases.filter(|$a| !string::contains(string::lowercase($a), $email_lc)) \
           WHERE (string::contains(string::lowercase(name), $email_lc) OR string::contains(string::lowercase(summary), $email_lc) OR aliases.any(|$a| string::contains(string::lowercase($a), $email_lc))) RETURN VALUE id)); \
     RETURN array::len($m) + array::len($e); \
     COMMIT TRANSACTION;",
    3,
);

/// Spaces only the subject could read: every row in them, whoever wrote it (their documents' files
/// were deleted first, see `documents::purge_vault_documents`). The space rows themselves go last.
pub const ERASE_SPACES: Stmt = Stmt::at(
    "subject.erase_spaces",
    "BEGIN TRANSACTION; \
     LET $records = (SELECT VALUE id FROM cache_record WHERE vault IN $solo); \
     LET $r = (DELETE relates_to WHERE in.vault IN $solo OR out.vault IN $solo RETURN BEFORE); \
     LET $l = (DELETE linked_to WHERE in IN $records OR out.vault IN $solo RETURN BEFORE); \
     LET $m = (DELETE memory WHERE vault IN $solo RETURN BEFORE); \
     LET $p = (DELETE entity_property WHERE vault IN $solo RETURN BEFORE); \
     LET $e = array::concat((DELETE person WHERE vault IN $solo RETURN BEFORE), (DELETE organisation WHERE vault IN $solo RETURN BEFORE), \
        (DELETE location WHERE vault IN $solo RETURN BEFORE), (DELETE repository WHERE vault IN $solo RETURN BEFORE), (DELETE file WHERE vault IN $solo RETURN BEFORE), \
        (DELETE symbol WHERE vault IN $solo RETURN BEFORE), (DELETE entity WHERE vault IN $solo RETURN BEFORE)); \
     LET $c = (DELETE cache_record WHERE vault IN $solo RETURN BEFORE); \
     LET $s = (DELETE skill WHERE vault IN $solo RETURN BEFORE); \
     LET $x = array::concat((DELETE peer_share WHERE vault IN $solo RETURN BEFORE), (DELETE peer_link WHERE vault IN $solo RETURN BEFORE), (DELETE peer_item WHERE vault IN $solo RETURN BEFORE), \
        (DELETE credential_ref WHERE vault IN $solo RETURN BEFORE), (DELETE vault_secret WHERE vault IN $solo RETURN BEFORE), (DELETE secret_request WHERE vault IN $solo RETURN BEFORE), \
        (DELETE ontology_kind WHERE vault IN $solo RETURN BEFORE), (DELETE ontology_link WHERE vault IN $solo RETURN BEFORE), (DELETE ontology_property WHERE vault IN $solo RETURN BEFORE), \
        (DELETE ontology_metric WHERE vault IN $solo RETURN BEFORE), (DELETE ontology_event WHERE vault IN $solo RETURN BEFORE), (DELETE extension WHERE vault IN $solo RETURN BEFORE)); \
     RETURN { relations: array::len($r) + array::len($l), memories: array::len($m), entities: array::len($e) + array::len($p), synced_records: array::len($c), skills: array::len($s), space_settings: array::len($x) }; \
     COMMIT TRANSACTION;",
    12,
);

/// Everything else the subject owns in the org database, wherever it is. What is left in a space with
/// other readers was reassigned first ([`REASSIGN`]), so what still names the subject here is theirs
/// alone. The actor columns (`$actors`: the user id and their API token ids) and the subject's email as
/// an actor name become "erased".
pub const ERASE_OWNED: Stmt = Stmt::at(
    "subject.erase_owned",
    "BEGIN TRANSACTION; \
     LET $records = (SELECT VALUE id FROM cache_record WHERE owner = $u); \
     LET $r = (DELETE relates_to WHERE owner = $u OR in.owner = $u OR out.owner = $u RETURN BEFORE); \
     LET $l = (DELETE linked_to WHERE in IN $records OR out.owner = $u RETURN BEFORE); \
     LET $m = (DELETE memory WHERE owner = $u RETURN BEFORE); \
     LET $e = array::concat((DELETE person WHERE owner = $u RETURN BEFORE), (DELETE organisation WHERE owner = $u RETURN BEFORE), \
        (DELETE location WHERE owner = $u RETURN BEFORE), (DELETE repository WHERE owner = $u RETURN BEFORE), (DELETE file WHERE owner = $u RETURN BEFORE), \
        (DELETE symbol WHERE owner = $u RETURN BEFORE), (DELETE entity WHERE owner = $u RETURN BEFORE)); \
     LET $c = (DELETE cache_record WHERE owner = $u RETURN BEFORE); \
     LET $d = (DELETE document WHERE owner = $u RETURN BEFORE); \
     LET $s = (DELETE skill WHERE owner = $u RETURN BEFORE); \
     LET $ct = (DELETE chat_thread WHERE owner = $u RETURN BEFORE); \
     LET $cm = (DELETE chat_message WHERE owner = $u RETURN BEFORE); \
     LET $al = (DELETE audit_log WHERE owner = $u RETURN BEFORE); \
     LET $src = array::concat((DELETE connector WHERE owner = $u RETURN BEFORE), (DELETE composio_file WHERE owner = $u RETURN BEFORE), (DELETE sync_status WHERE owner = $u RETURN BEFORE)); \
     LET $pr = (DELETE pocket_recording WHERE owner = $u RETURN BEFORE); \
     LET $ex = array::concat((DELETE extension_event WHERE owner = $u RETURN BEFORE), (DELETE extension WHERE owner = $u RETURN BEFORE)); \
     LET $st = (DELETE app_settings WHERE owner = $u RETURN BEFORE); \
     LET $sec = array::concat((DELETE peer_share WHERE created_by = $u RETURN BEFORE), (DELETE peer_link WHERE user = $u RETURN BEFORE), \
        (DELETE credential_ref WHERE created_by = $u RETURN BEFORE), (DELETE vault_secret WHERE created_by = $u RETURN BEFORE), (DELETE secret_request WHERE requested_by = $u RETURN BEFORE)); \
     UPDATE secret_provider SET created_by = $heir WHERE created_by = $u RETURN NONE; \
     UPDATE secret_provider SET updated_by = $heir WHERE updated_by = $u RETURN NONE; \
     UPDATE secret_request SET decided_by = NONE WHERE decided_by = $u RETURN NONE; \
     UPDATE secret_request SET actor_id = 'erased' WHERE actor_id IN $actors RETURN NONE; \
     UPDATE ontology_kind SET decided_by = NONE WHERE decided_by = $u RETURN NONE; \
     UPDATE ontology_link SET decided_by = NONE WHERE decided_by = $u RETURN NONE; \
     UPDATE ontology_property SET decided_by = NONE WHERE decided_by = $u RETURN NONE; \
     UPDATE ontology_metric SET decided_by = NONE WHERE decided_by = $u RETURN NONE; \
     UPDATE ontology_kind SET activity_actor = 'erased', activity_actor_name = 'Erased person' WHERE activity_actor IN $actors OR activity_actor_name = $email RETURN NONE; \
     UPDATE ontology_link SET activity_actor = 'erased', activity_actor_name = 'Erased person' WHERE activity_actor IN $actors OR activity_actor_name = $email RETURN NONE; \
     UPDATE ontology_property SET activity_actor = 'erased', activity_actor_name = 'Erased person' WHERE activity_actor IN $actors OR activity_actor_name = $email RETURN NONE; \
     UPDATE ontology_metric SET activity_actor = 'erased', activity_actor_name = 'Erased person' WHERE activity_actor IN $actors OR activity_actor_name = $email RETURN NONE; \
     UPDATE ontology_event SET actor = 'erased', actor_name = 'Erased person' WHERE actor IN $actors OR actor_name = $email RETURN NONE; \
     UPDATE vault SET ontology_actor = 'erased', ontology_actor_name = 'Erased person' WHERE ontology_actor IN $actors OR ontology_actor_name = $email RETURN NONE; \
     UPDATE change SET actor = 'erased' WHERE actor IN $actors RETURN NONE; \
     LET $xp = (DELETE subject_export WHERE subject = $u OR requested_by = $u RETURN BEFORE); \
     RETURN { relations: array::len($r) + array::len($l), memories: array::len($m), entities: array::len($e), synced_records: array::len($c), documents: array::len($d), \
        skills: array::len($s), chat_threads: array::len($ct), chat_messages: array::len($cm), tool_log: array::len($al), sources: array::len($src), recordings: array::len($pr), \
        extensions: array::len($ex), settings: array::len($st), sharing_and_secrets: array::len($sec), exports: array::len($xp) }; \
     COMMIT TRANSACTION;",
    35,
);

/// The subject's own memberships (active, pending or left), after everything they decide is done.
pub const ERASE_MEMBERSHIPS: Stmt =
    Stmt::new("subject.erase_memberships", "DELETE vault_member WHERE user = $u RETURN BEFORE");

/// Cached embeddings are keyed by a keyed hash of the text, not by who wrote it, so the subject's
/// cannot be told apart: the org's whole cache goes (it only saves embedding calls).
pub const ERASE_EMBED_CACHE: Stmt = Stmt::new("subject.erase_embed_cache", "DELETE embed_cache RETURN BEFORE");

/// Files the subject owns in the bucket: their documents in spaces nobody else reads and their
/// recordings' audio.
pub const OWNED_FILES: Stmt = Stmt::new(
    "subject.owned_files",
    "SELECT VALUE object_path FROM document WHERE owner = $u AND vault NOTINSIDE $kept; \
     SELECT VALUE audio_path FROM pocket_recording WHERE owner = $u AND audio_path IS NOT NONE",
);

/// The post-check: every org table that can name a user, counted for the subject. All must be 0.
pub const RESIDUE: Stmt = Stmt::new(
    "subject.residue",
    "RETURN { \
       app_settings: count((SELECT id FROM app_settings WHERE owner = $u)), \
       vault_member: count((SELECT id FROM vault_member WHERE user = $u)), \
       connector: count((SELECT id FROM connector WHERE owner = $u)), \
       composio_file: count((SELECT id FROM composio_file WHERE owner = $u)), \
       sync_status: count((SELECT id FROM sync_status WHERE owner = $u)), \
       cache_record: count((SELECT id FROM cache_record WHERE owner = $u)), \
       person: count((SELECT id FROM person WHERE owner = $u)), \
       organisation: count((SELECT id FROM organisation WHERE owner = $u)), \
       location: count((SELECT id FROM location WHERE owner = $u)), \
       repository: count((SELECT id FROM repository WHERE owner = $u)), \
       file: count((SELECT id FROM file WHERE owner = $u)), \
       symbol: count((SELECT id FROM symbol WHERE owner = $u)), \
       entity: count((SELECT id FROM entity WHERE owner = $u)), \
       memory: count((SELECT id FROM memory WHERE owner = $u OR string::contains(string::lowercase(text), $email_lc))), \
       relates_to: count((SELECT id FROM relates_to WHERE owner = $u)), \
       chat_thread: count((SELECT id FROM chat_thread WHERE owner = $u)), \
       chat_message: count((SELECT id FROM chat_message WHERE owner = $u)), \
       audit_log: count((SELECT id FROM audit_log WHERE owner = $u)), \
       pocket_recording: count((SELECT id FROM pocket_recording WHERE owner = $u)), \
       document: count((SELECT id FROM document WHERE owner = $u)), \
       extension: count((SELECT id FROM extension WHERE owner = $u)), \
       extension_event: count((SELECT id FROM extension_event WHERE owner = $u)), \
       skill: count((SELECT id FROM skill WHERE owner = $u)), \
       peer_share: count((SELECT id FROM peer_share WHERE created_by = $u)), \
       peer_link: count((SELECT id FROM peer_link WHERE user = $u)), \
       credential_ref: count((SELECT id FROM credential_ref WHERE created_by = $u)), \
       secret_provider: count((SELECT id FROM secret_provider WHERE created_by = $u OR updated_by = $u)), \
       secret_request: count((SELECT id FROM secret_request WHERE requested_by = $u OR decided_by = $u OR actor_id IN $actors)), \
       vault_secret: count((SELECT id FROM vault_secret WHERE created_by = $u)), \
       ontology_kind: count((SELECT id FROM ontology_kind WHERE decided_by = $u OR activity_actor IN $actors OR activity_actor_name = $email)), \
       ontology_link: count((SELECT id FROM ontology_link WHERE decided_by = $u OR activity_actor IN $actors OR activity_actor_name = $email)), \
       ontology_property: count((SELECT id FROM ontology_property WHERE decided_by = $u OR activity_actor IN $actors OR activity_actor_name = $email)), \
       ontology_metric: count((SELECT id FROM ontology_metric WHERE decided_by = $u OR activity_actor IN $actors OR activity_actor_name = $email)), \
       ontology_event: count((SELECT id FROM ontology_event WHERE actor IN $actors OR actor_name = $email)), \
       vault: count((SELECT id FROM vault WHERE id IN $solo OR ontology_actor IN $actors OR ontology_actor_name = $email)), \
       change: count((SELECT id FROM change WHERE actor IN $actors)), \
       subject_export: count((SELECT id FROM subject_export WHERE subject = $u OR requested_by = $u)) \
     }",
);

/// The spaces the subject has anything in (the ones a reassignment may have to touch).
pub const VAULTS_WITH_ROWS: Stmt = Stmt::new(
    "subject.vaults_with_rows",
    "RETURN array::distinct(array::flatten([ \
       (SELECT VALUE <string> vault FROM memory WHERE owner = $u), (SELECT VALUE <string> vault FROM person WHERE owner = $u), \
       (SELECT VALUE <string> vault FROM organisation WHERE owner = $u), (SELECT VALUE <string> vault FROM location WHERE owner = $u), \
       (SELECT VALUE <string> vault FROM repository WHERE owner = $u), (SELECT VALUE <string> vault FROM file WHERE owner = $u), \
       (SELECT VALUE <string> vault FROM symbol WHERE owner = $u), (SELECT VALUE <string> vault FROM entity WHERE owner = $u), \
       (SELECT VALUE <string> vault FROM document WHERE owner = $u), (SELECT VALUE <string> vault FROM skill WHERE owner = $u), \
       (SELECT VALUE <string> vault FROM cache_record WHERE owner = $u AND vault IS NOT NONE), \
       (SELECT VALUE <string> vault FROM peer_share WHERE created_by = $u), (SELECT VALUE <string> vault FROM peer_link WHERE user = $u), \
       (SELECT VALUE <string> vault FROM credential_ref WHERE created_by = $u), (SELECT VALUE <string> vault FROM vault_secret WHERE created_by = $u), \
       (SELECT VALUE <string> vault FROM secret_request WHERE requested_by = $u) ]))",
);

/// One bucket object, or NONE once it is gone (the post-check of the files).
pub const OBJECT_EXISTS: Stmt = Stmt::new("subject.object_exists", "RETURN file::exists(type::file(\"documents\", $path))");

pub const ALL: &[&Stmt] = &[
    &MEMBERSHIPS,
    &AUTHORED,
    &OWNED,
    &EXPORT_CREATE,
    &EXPORT_FINISH,
    &EXPORT_LIST,
    &EXPORT_GET,
    &EXPORT_JOB_ROW,
    &EXPORT_PRUNE,
    &OTHER_MEMBERS,
    &PROMOTE,
    &REASSIGN,
    &SCRUB_EMAIL,
    &ERASE_SPACES,
    &ERASE_OWNED,
    &ERASE_MEMBERSHIPS,
    &ERASE_EMBED_CACHE,
    &OWNED_FILES,
    &VAULTS_WITH_ROWS,
    &RESIDUE,
    &OBJECT_EXISTS,
];

// ------------------------------------------------------------------ control database

/// The account as the subject sees it, with how they sign in and where they belong.
pub const ACCOUNT: ControlStmt = ControlStmt::new(
    "subject.account",
    "SELECT email, created_at, onboarded_at, admin, suspended_at, legal_hold_at, (password_hash != NONE) AS has_password FROM ONLY $u; \
     SELECT issuer, subject, email, created_at, last_login_at FROM identity WHERE user = $u; \
     SELECT <string> org AS org, org.name AS org_name, role, created_at FROM membership WHERE user = $u; \
     SELECT name, scopes, created_at, last_used_at, expires_at FROM api_token WHERE owner = $u; \
     SELECT user_agent, created_at, last_seen_at, revoked FROM session WHERE owner = $u; \
     SELECT client_name, client_id, scope, created_at, last_used_at FROM oauth_grant WHERE owner = $u; \
     SELECT action, target, outcome, actor_kind, detail, created_at FROM audit_event WHERE user = $u ORDER BY created_at; \
     SELECT tool, channel, calls, errors, bucket FROM tool_usage WHERE user = $u ORDER BY bucket",
);

/// The subject's password hash (the self-service erasure asks for the password again) and hold.
pub const CONFIRM_ROW: ControlStmt =
    ControlStmt::new("subject.confirm_row", "SELECT email, password_hash, legal_hold_at FROM ONLY $u");

/// The org's owners other than the subject (an org must keep one).
pub const OTHER_ORG_OWNERS: ControlStmt = ControlStmt::new(
    "subject.other_org_owners",
    "SELECT VALUE user FROM membership WHERE org = $org AND role = 'owner' AND user != $u ORDER BY created_at, id",
);

/// Any other member of the org, longest-standing first (the heir for org-level rows when the subject
/// is not an owner).
pub const OTHER_ORG_MEMBERS: ControlStmt = ControlStmt::new(
    "subject.other_org_members",
    "SELECT VALUE user FROM membership WHERE org = $org AND user != $u ORDER BY created_at, id",
);

/// Places (`true`, now) or lifts (`false`) a legal hold.
pub const SET_LEGAL_HOLD: ControlStmt = ControlStmt::new(
    "subject.set_legal_hold",
    "UPDATE $u SET legal_hold_at = IF $hold THEN (legal_hold_at ?? time::now()) ELSE NONE END RETURN legal_hold_at",
);

/// The request: an erasure row that holds the subject's id only until the job finishes.
pub const ERASURE_CREATE: ControlStmt = ControlStmt::new(
    "subject.erasure_create",
    "CREATE erasure SET org = $org, subject_ref = $ref, subject = $u, requested_by = $by, status = 'pending' RETURN <string> id AS id, status, requested_at",
);
/// An erasure already waiting or running for this subject (a second request joins it).
pub const ERASURE_OPEN: ControlStmt = ControlStmt::new(
    "subject.erasure_open",
    "SELECT <string> id AS id, status, requested_at FROM erasure WHERE subject_ref = $ref AND status IN ['pending', 'running'] LIMIT 1",
);
pub const ERASURE_GET: ControlStmt = ControlStmt::new("subject.erasure_get", "SELECT * FROM ONLY $id");
pub const ERASURE_RUNNING: ControlStmt =
    ControlStmt::new("subject.erasure_running", "UPDATE $id SET status = 'running' WHERE status IN ['pending', 'running', 'failed'] RETURN NONE");
/// The completion record: counts and the post-check, and the subject's id is dropped.
pub const ERASURE_DONE: ControlStmt = ControlStmt::new(
    "subject.erasure_done",
    "UPDATE $id SET status = 'done', subject = NONE, counts = $counts, post_check = $post_check, error = NONE, completed_at = time::now() RETURN NONE",
);
pub const ERASURE_FAILED: ControlStmt =
    ControlStmt::new("subject.erasure_failed", "UPDATE $id SET status = 'failed', error = $error, post_check = $post_check RETURN NONE");
/// One org's erasures, newest first (Settings > People shows them as proof).
pub const ERASURE_LIST: ControlStmt = ControlStmt::new(
    "subject.erasure_list",
    "SELECT <string> id AS id, subject_ref, requested_by, status, counts, post_check, error, requested_at, completed_at FROM erasure \
     WHERE org = $org ORDER BY requested_at DESC LIMIT 50",
);
/// Every finished erasure's reference (the restore re-apply matches live accounts against them).
pub const ERASURE_REFS: ControlStmt = ControlStmt::new("subject.erasure_refs", "SELECT VALUE subject_ref FROM erasure WHERE status = 'done'");
/// Whether a reference has been erased (resync suppression).
pub const ERASURE_HAS_REF: ControlStmt =
    ControlStmt::new("subject.erasure_has_ref", "SELECT VALUE id FROM erasure WHERE subject_ref = $ref LIMIT 1");
/// Every account and its home org (the restore re-apply).
pub const USERS_WITH_ORG: ControlStmt = ControlStmt::new(
    "subject.users_with_org",
    "SELECT id, email, (SELECT VALUE org FROM membership WHERE user = $parent.id ORDER BY created_at, id LIMIT 1)[0] AS org FROM user",
);

/// Every org the subject belongs to (an erasure covers each).
pub const USER_ORGS: ControlStmt = ControlStmt::new("subject.user_orgs", "SELECT VALUE org FROM membership WHERE user = $u ORDER BY created_at, id");

/// The subject's API token ids (actor ids in the audit trail and the change log).
pub const TOKEN_IDS: ControlStmt = ControlStmt::new("subject.token_ids", "SELECT VALUE <string> id FROM api_token WHERE owner = $u");

/// Everything the control database holds about the subject. Their audit events stay (the security
/// ledger is append-only and pruned only by retention) but no longer name them: the actor id, a
/// target that is their id and their email anywhere in the detail become "erased", and the row's
/// `user` becomes `user:erased`, an account that never exists, so no workspace's activity log lists
/// it (a row with no user would show in every workspace's log, like a failed sign-in).
pub const ERASE_CONTROL: ControlStmt = ControlStmt::at(
    "subject.erase_control",
    "BEGIN TRANSACTION; \
     LET $grants = (SELECT VALUE id FROM oauth_grant WHERE owner = $u); \
     LET $ot = (DELETE oauth_token WHERE family IN $grants RETURN BEFORE); \
     LET $og = (DELETE oauth_grant WHERE owner = $u RETURN BEFORE); \
     LET $oc = (DELETE oauth_code WHERE owner = $u RETURN BEFORE); \
     LET $at = (DELETE api_token WHERE owner = $u RETURN BEFORE); \
     LET $se = (DELETE session WHERE owner = $u RETURN BEFORE); \
     LET $id = (DELETE identity WHERE user = $u RETURN BEFORE); \
     LET $tu = (DELETE tool_usage WHERE user = $u RETURN BEFORE); \
     LET $jb = (DELETE job WHERE owner = $u RETURN BEFORE); \
     LET $fc = (UPDATE failure_capsule SET user = NONE WHERE user = <string> $u RETURN BEFORE); \
     LET $ae = (UPDATE audit_event SET user = user:erased, actor_id = IF actor_id IN $actors THEN 'erased' ELSE actor_id END, \
        target = IF target IN $actors THEN 'erased' ELSE target END, \
        detail = string::replace(string::replace(detail, $email, '[erased person]'), $email_lc, '[erased person]') \
        WHERE user = $u OR actor_id IN $actors OR target IN $actors OR string::contains(string::lowercase(detail), $email_lc) RETURN BEFORE); \
     LET $ms = (DELETE membership WHERE user = $u RETURN BEFORE); \
     LET $us = (DELETE $u RETURN BEFORE); \
     RETURN { app_connections: array::len($og) + array::len($ot) + array::len($oc), api_tokens: array::len($at), sessions: array::len($se), \
        sign_in_identities: array::len($id), usage_counters: array::len($tu), jobs: array::len($jb), failure_capsules: array::len($fc), \
        audit_events_anonymised: array::len($ae), org_memberships: array::len($ms), account: array::len($us) }; \
     COMMIT TRANSACTION;",
    14,
);

/// The control half of the post-check: every control table that can name the subject. All must be 0.
pub const CONTROL_RESIDUE: ControlStmt = ControlStmt::new(
    "subject.control_residue",
    "RETURN { \
       user: count((SELECT id FROM user WHERE id = $u OR email_lc = $email_lc)), \
       identity: count((SELECT id FROM identity WHERE user = $u)), \
       membership: count((SELECT id FROM membership WHERE user = $u)), \
       api_token: count((SELECT id FROM api_token WHERE owner = $u)), \
       session: count((SELECT id FROM session WHERE owner = $u)), \
       oauth_code: count((SELECT id FROM oauth_code WHERE owner = $u)), \
       oauth_grant: count((SELECT id FROM oauth_grant WHERE owner = $u)), \
       tool_usage: count((SELECT id FROM tool_usage WHERE user = $u)), \
       job: count((SELECT id FROM job WHERE owner = $u)), \
       failure_capsule: count((SELECT id FROM failure_capsule WHERE user = <string> $u)), \
       audit_event: count((SELECT id FROM audit_event WHERE user = $u OR actor_id IN $actors OR target IN $actors OR string::contains(string::lowercase(detail), $email_lc))), \
       erasure: count((SELECT id FROM erasure WHERE subject = $u AND id != $erasure)) \
     }",
);

pub const CONTROL_ALL: &[&ControlStmt] = &[
    &ACCOUNT,
    &CONFIRM_ROW,
    &OTHER_ORG_OWNERS,
    &OTHER_ORG_MEMBERS,
    &SET_LEGAL_HOLD,
    &ERASURE_CREATE,
    &ERASURE_OPEN,
    &ERASURE_GET,
    &ERASURE_RUNNING,
    &ERASURE_DONE,
    &ERASURE_FAILED,
    &ERASURE_LIST,
    &ERASURE_REFS,
    &ERASURE_HAS_REF,
    &USERS_WITH_ORG,
    &USER_ORGS,
    &TOKEN_IDS,
    &ERASE_CONTROL,
    &CONTROL_RESIDUE,
];
