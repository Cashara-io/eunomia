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

/// Who else can read each space: every active membership that is not the subject's nor that of
/// someone with an erasure open (`$pending`), owners first, then the longest-standing.
pub const OTHER_MEMBERS: Stmt = Stmt::new(
    "subject.other_members",
    "SELECT <string> vault AS vault, user, role, created_at FROM vault_member WHERE status = 'active' AND user != $u AND user NOTINSIDE $pending \
     ORDER BY role DESC, created_at, id",
);

/// Makes `$to` an owner of `$vault` (the subject was its only owner).
pub const PROMOTE: Stmt = Stmt::new(
    "subject.promote",
    "UPDATE vault_member SET role = 'owner' WHERE vault = $vault AND user = $to AND status = 'active' RETURN NONE",
);

/// The rule for a space other people can still read: what the subject wrote there stays, owned by
/// `$to` (an owner who remains). Their synced records filed there are deleted instead (their ids carry
/// the subject's key, and the sources they came from are deleted with the account); facts and
/// relations drawn from them stay, without the link. Documents keep their file and are indexed again
/// as `$to`'s, so the chunks get ids of the new owner. One transaction.
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
     LET $recs = (SELECT VALUE id FROM cache_record WHERE owner = $u AND vault = $vault); \
     UPDATE memory SET source = NONE WHERE source IN $recs RETURN NONE; \
     UPDATE relates_to SET source = NONE WHERE source IN $recs RETURN NONE; \
     UPDATE entity_property SET source = NONE WHERE source IN $recs RETURN NONE; \
     DELETE linked_to WHERE in IN $recs OR out IN $recs RETURN NONE; \
     DELETE $recs RETURN NONE; \
     LET $docs = (UPDATE document SET owner = $to WHERE owner = $u AND vault = $vault AND status != 'deleted' RETURN VALUE <string> id); \
     UPDATE skill SET owner = $to WHERE owner = $u AND vault = $vault RETURN NONE; \
     UPDATE peer_share SET created_by = $to WHERE created_by = $u AND vault = $vault RETURN NONE; \
     UPDATE peer_link SET user = $to WHERE user = $u AND vault = $vault RETURN NONE; \
     UPDATE credential_ref SET created_by = $to WHERE created_by = $u AND vault = $vault RETURN NONE; \
     UPDATE vault_secret SET created_by = $to WHERE created_by = $u AND vault = $vault RETURN NONE; \
     UPDATE secret_request SET requested_by = $to WHERE requested_by = $u AND vault = $vault RETURN NONE; \
     RETURN { n: $n, documents: $docs }; \
     COMMIT TRANSACTION;",
    24, // BEGIN 0, LET 1, the updates, deletes and LETs 2 to 23, RETURN 24
);

/// The subject's email in what stays in the workspace (shared spaces, other people's own spaces and
/// tool logs), matched without regard to case (`$email_re` is `(?i)` and the escaped address): replaced
/// in the text of facts, the names, aliases and summaries of every entity kind and the tool log's
/// argument summaries (the facts lose their vector and are embedded again).
pub const SCRUB_EMAIL: Stmt = Stmt::at(
    "subject.scrub_email",
    "BEGIN TRANSACTION; \
     LET $m = (UPDATE memory SET text = string::replace(text, <regex> $email_re, '[erased person]'), embedding = NONE, embedding_spec = NONE \
        WHERE string::contains(string::lowercase(text), $email_lc) RETURN VALUE id); \
     LET $e = array::flatten([ \
        (UPDATE person SET name = string::replace(name, <regex> $email_re, '[erased person]'), summary = string::replace(summary ?? '', <regex> $email_re, '[erased person]'), aliases = (aliases ?? []).filter(|$a| !string::contains(string::lowercase($a), $email_lc)) \
           WHERE string::contains(string::lowercase(name), $email_lc) OR string::contains(string::lowercase(summary ?? ''), $email_lc) OR (aliases ?? []).any(|$a| string::contains(string::lowercase($a), $email_lc)) RETURN VALUE id), \
        (UPDATE organisation SET name = string::replace(name, <regex> $email_re, '[erased person]'), summary = string::replace(summary ?? '', <regex> $email_re, '[erased person]'), aliases = (aliases ?? []).filter(|$a| !string::contains(string::lowercase($a), $email_lc)) \
           WHERE string::contains(string::lowercase(name), $email_lc) OR string::contains(string::lowercase(summary ?? ''), $email_lc) OR (aliases ?? []).any(|$a| string::contains(string::lowercase($a), $email_lc)) RETURN VALUE id), \
        (UPDATE location SET name = string::replace(name, <regex> $email_re, '[erased person]'), summary = string::replace(summary ?? '', <regex> $email_re, '[erased person]'), aliases = (aliases ?? []).filter(|$a| !string::contains(string::lowercase($a), $email_lc)) \
           WHERE string::contains(string::lowercase(name), $email_lc) OR string::contains(string::lowercase(summary ?? ''), $email_lc) OR (aliases ?? []).any(|$a| string::contains(string::lowercase($a), $email_lc)) RETURN VALUE id), \
        (UPDATE repository SET name = string::replace(name, <regex> $email_re, '[erased person]'), summary = string::replace(summary ?? '', <regex> $email_re, '[erased person]'), aliases = (aliases ?? []).filter(|$a| !string::contains(string::lowercase($a), $email_lc)) \
           WHERE string::contains(string::lowercase(name), $email_lc) OR string::contains(string::lowercase(summary ?? ''), $email_lc) OR (aliases ?? []).any(|$a| string::contains(string::lowercase($a), $email_lc)) RETURN VALUE id), \
        (UPDATE file SET name = string::replace(name, <regex> $email_re, '[erased person]'), summary = string::replace(summary ?? '', <regex> $email_re, '[erased person]'), aliases = (aliases ?? []).filter(|$a| !string::contains(string::lowercase($a), $email_lc)) \
           WHERE string::contains(string::lowercase(name), $email_lc) OR string::contains(string::lowercase(summary ?? ''), $email_lc) OR (aliases ?? []).any(|$a| string::contains(string::lowercase($a), $email_lc)) RETURN VALUE id), \
        (UPDATE symbol SET name = string::replace(name, <regex> $email_re, '[erased person]'), summary = string::replace(summary ?? '', <regex> $email_re, '[erased person]'), aliases = (aliases ?? []).filter(|$a| !string::contains(string::lowercase($a), $email_lc)) \
           WHERE string::contains(string::lowercase(name), $email_lc) OR string::contains(string::lowercase(summary ?? ''), $email_lc) OR (aliases ?? []).any(|$a| string::contains(string::lowercase($a), $email_lc)) RETURN VALUE id), \
        (UPDATE entity SET name = string::replace(name, <regex> $email_re, '[erased person]'), summary = string::replace(summary ?? '', <regex> $email_re, '[erased person]'), aliases = (aliases ?? []).filter(|$a| !string::contains(string::lowercase($a), $email_lc)) \
           WHERE string::contains(string::lowercase(name), $email_lc) OR string::contains(string::lowercase(summary ?? ''), $email_lc) OR (aliases ?? []).any(|$a| string::contains(string::lowercase($a), $email_lc)) RETURN VALUE id) ]); \
     LET $t = (UPDATE audit_log SET args_summary = string::replace(args_summary, <regex> $email_re, '[erased person]') \
        WHERE string::contains(string::lowercase(args_summary), $email_lc) RETURN VALUE id); \
     RETURN array::len($m) + array::len($e) + array::len($t); \
     COMMIT TRANSACTION;",
    4,
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
    10, // BEGIN 0, nine LETs 1 to 9, RETURN 10
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
     UPDATE ontology_kind SET activity_actor = 'erased', activity_actor_name = 'Erased person' WHERE activity_actor IN $actors OR string::lowercase(activity_actor_name ?? '') = $email_lc RETURN NONE; \
     UPDATE ontology_link SET activity_actor = 'erased', activity_actor_name = 'Erased person' WHERE activity_actor IN $actors OR string::lowercase(activity_actor_name ?? '') = $email_lc RETURN NONE; \
     UPDATE ontology_property SET activity_actor = 'erased', activity_actor_name = 'Erased person' WHERE activity_actor IN $actors OR string::lowercase(activity_actor_name ?? '') = $email_lc RETURN NONE; \
     UPDATE ontology_metric SET activity_actor = 'erased', activity_actor_name = 'Erased person' WHERE activity_actor IN $actors OR string::lowercase(activity_actor_name ?? '') = $email_lc RETURN NONE; \
     UPDATE ontology_event SET actor = 'erased', actor_name = 'Erased person' WHERE actor IN $actors OR string::lowercase(actor_name ?? '') = $email_lc RETURN NONE; \
     UPDATE vault SET ontology_actor = 'erased', ontology_actor_name = 'Erased person' WHERE ontology_actor IN $actors OR string::lowercase(ontology_actor_name ?? '') = $email_lc RETURN NONE; \
     UPDATE change SET actor = 'erased' WHERE actor IN $actors RETURN NONE; \
     LET $xp = (DELETE subject_export WHERE subject = $u OR requested_by = $u RETURN BEFORE); \
     RETURN { relations: array::len($r) + array::len($l), memories: array::len($m), entities: array::len($e), synced_records: array::len($c), documents: array::len($d), \
        skills: array::len($s), chat_threads: array::len($ct), chat_messages: array::len($cm), tool_log: array::len($al), sources: array::len($src), recordings: array::len($pr), \
        extensions: array::len($ex), settings: array::len($st), sharing_and_secrets: array::len($sec), exports: array::len($xp) }; \
     COMMIT TRANSACTION;",
    33, // BEGIN 0, the LETs and updates 1 to 32, RETURN 33
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
       person: count((SELECT id FROM person WHERE owner = $u OR string::contains(string::lowercase(name), $email_lc) OR string::contains(string::lowercase(summary ?? ''), $email_lc) OR (aliases ?? []).any(|$a| string::contains(string::lowercase($a), $email_lc)))), \
       organisation: count((SELECT id FROM organisation WHERE owner = $u OR string::contains(string::lowercase(name), $email_lc) OR string::contains(string::lowercase(summary ?? ''), $email_lc) OR (aliases ?? []).any(|$a| string::contains(string::lowercase($a), $email_lc)))), \
       location: count((SELECT id FROM location WHERE owner = $u OR string::contains(string::lowercase(name), $email_lc) OR string::contains(string::lowercase(summary ?? ''), $email_lc) OR (aliases ?? []).any(|$a| string::contains(string::lowercase($a), $email_lc)))), \
       repository: count((SELECT id FROM repository WHERE owner = $u OR string::contains(string::lowercase(name), $email_lc) OR string::contains(string::lowercase(summary ?? ''), $email_lc) OR (aliases ?? []).any(|$a| string::contains(string::lowercase($a), $email_lc)))), \
       file: count((SELECT id FROM file WHERE owner = $u OR string::contains(string::lowercase(name), $email_lc) OR string::contains(string::lowercase(summary ?? ''), $email_lc) OR (aliases ?? []).any(|$a| string::contains(string::lowercase($a), $email_lc)))), \
       symbol: count((SELECT id FROM symbol WHERE owner = $u OR string::contains(string::lowercase(name), $email_lc) OR string::contains(string::lowercase(summary ?? ''), $email_lc) OR (aliases ?? []).any(|$a| string::contains(string::lowercase($a), $email_lc)))), \
       entity: count((SELECT id FROM entity WHERE owner = $u OR string::contains(string::lowercase(name), $email_lc) OR string::contains(string::lowercase(summary ?? ''), $email_lc) OR (aliases ?? []).any(|$a| string::contains(string::lowercase($a), $email_lc)))), \
       memory: count((SELECT id FROM memory WHERE owner = $u OR string::contains(string::lowercase(text), $email_lc))), \
       relates_to: count((SELECT id FROM relates_to WHERE owner = $u)), \
       chat_thread: count((SELECT id FROM chat_thread WHERE owner = $u)), \
       chat_message: count((SELECT id FROM chat_message WHERE owner = $u)), \
       audit_log: count((SELECT id FROM audit_log WHERE owner = $u OR string::contains(string::lowercase(args_summary), $email_lc))), \
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
       ontology_kind: count((SELECT id FROM ontology_kind WHERE decided_by = $u OR activity_actor IN $actors OR string::lowercase(activity_actor_name ?? '') = $email_lc)), \
       ontology_link: count((SELECT id FROM ontology_link WHERE decided_by = $u OR activity_actor IN $actors OR string::lowercase(activity_actor_name ?? '') = $email_lc)), \
       ontology_property: count((SELECT id FROM ontology_property WHERE decided_by = $u OR activity_actor IN $actors OR string::lowercase(activity_actor_name ?? '') = $email_lc)), \
       ontology_metric: count((SELECT id FROM ontology_metric WHERE decided_by = $u OR activity_actor IN $actors OR string::lowercase(activity_actor_name ?? '') = $email_lc)), \
       ontology_event: count((SELECT id FROM ontology_event WHERE actor IN $actors OR string::lowercase(actor_name ?? '') = $email_lc)), \
       vault: count((SELECT id FROM vault WHERE id IN $solo OR (string::lowercase(name) CONTAINS $email_lc) OR ontology_actor IN $actors OR string::lowercase(ontology_actor_name ?? '') = $email_lc)), \
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
       (SELECT VALUE <string> vault FROM secret_request WHERE requested_by = $u), \
       (SELECT VALUE <string> in.vault FROM relates_to WHERE owner = $u AND in.vault IS NOT NONE), \
       (SELECT VALUE <string> out.vault FROM relates_to WHERE owner = $u AND out.vault IS NOT NONE) ]))",
);

/// What the post-check counts but does not fail on: the email in content other people wrote and the
/// system does not rewrite (their chats, skills, synced records, recordings, property values). The
/// completion record keeps the counts as "mentions left in others' content, for review".
pub const REVIEW: Stmt = Stmt::new(
    "subject.review",
    "RETURN { \
       chat_messages: count((SELECT id FROM chat_message WHERE string::contains(string::lowercase(content), $email_lc))), \
       skills: count((SELECT id FROM skill WHERE string::contains(string::lowercase(body), $email_lc) OR string::contains(string::lowercase(description), $email_lc))), \
       synced_records: count((SELECT id FROM cache_record WHERE string::contains(string::lowercase(body_text), $email_lc) OR string::contains(string::lowercase(title), $email_lc))), \
       recordings: count((SELECT id FROM pocket_recording WHERE string::contains(string::lowercase(transcript), $email_lc) OR string::contains(string::lowercase(summary), $email_lc))), \
       entity_properties: count((SELECT id FROM entity_property WHERE string::contains(string::lowercase(value_text ?? ''), $email_lc))) \
     }",
);

/// Every space and every stored file of a workspace that closes with its only member.
pub const ALL_SPACES_AND_FILES: Stmt = Stmt::new(
    "subject.all_spaces_and_files",
    "SELECT VALUE id FROM vault; SELECT VALUE object_path FROM document; SELECT VALUE audio_path FROM pocket_recording WHERE audio_path IS NOT NONE",
);

/// The owners of rows in this org database, for the restore re-apply: an erased person's rows brought
/// back by restoring only an org database have an owner the control database no longer has.
pub const ROW_OWNERS: Stmt = Stmt::new(
    "subject.row_owners",
    "RETURN array::distinct(array::flatten([ \
       (SELECT VALUE owner FROM memory GROUP BY owner), (SELECT VALUE owner FROM person GROUP BY owner), (SELECT VALUE owner FROM entity GROUP BY owner), \
       (SELECT VALUE owner FROM organisation GROUP BY owner), (SELECT VALUE owner FROM cache_record GROUP BY owner), (SELECT VALUE owner FROM document GROUP BY owner), \
       (SELECT VALUE owner FROM chat_message GROUP BY owner), (SELECT VALUE owner FROM chat_thread GROUP BY owner), (SELECT VALUE owner FROM connector GROUP BY owner), \
       (SELECT VALUE owner FROM app_settings GROUP BY owner), (SELECT VALUE owner FROM audit_log GROUP BY owner), (SELECT VALUE owner FROM pocket_recording GROUP BY owner), \
       (SELECT VALUE owner FROM skill GROUP BY owner), (SELECT VALUE user FROM vault_member GROUP BY user) ]))",
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
    &REVIEW,
    &ALL_SPACES_AND_FILES,
    &ROW_OWNERS,
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

/// Every membership of an org with its role, longest-standing first (the plan: who stays, who owns).
pub const ORG_MEMBERSHIPS: ControlStmt = ControlStmt::new(
    "subject.org_memberships",
    "SELECT user, role FROM membership WHERE org = $org ORDER BY created_at, id",
);

/// The people with an erasure open (they are never chosen to inherit anything).
pub const OPEN_SUBJECTS: ControlStmt = ControlStmt::new(
    "subject.open_subjects",
    "SELECT VALUE subject FROM erasure WHERE status IN ['pending', 'running', 'held', 'failed'] AND subject IS NOT NONE",
);

/// Hands the workspace's ownership from `$from` to `$to` (the owner slot is unique per org, so the old
/// owner gives it up first, in the same transaction).
pub const ORG_OWNER_TRANSFER: ControlStmt = ControlStmt::new(
    "subject.org_owner_transfer",
    "BEGIN TRANSACTION; \
     UPDATE membership SET role = 'member', slot = string::concat(<string> $org, '/', <string> $from) WHERE user = $from AND org = $org RETURN NONE; \
     UPDATE membership SET role = 'owner', slot = string::concat(<string> $org, '/owner') WHERE user = $to AND org = $org RETURN NONE; \
     COMMIT TRANSACTION;",
);

/// The legal hold alone (the erasure job reads it before each irreversible phase).
pub const HOLD_OF: ControlStmt = ControlStmt::new("subject.hold_of", "SELECT VALUE legal_hold_at FROM ONLY $u");

/// Places (`true`, now) or lifts (`false`) a legal hold.
pub const SET_LEGAL_HOLD: ControlStmt = ControlStmt::new(
    "subject.set_legal_hold",
    "UPDATE $u SET legal_hold_at = IF $hold THEN (legal_hold_at ?? time::now()) ELSE NONE END RETURN legal_hold_at",
);

/// The request and its job, in one transaction: a request is never left without a job. `open_slot`
/// is unique, so two requests for one person make one row (the loser reads the winner's).
pub const ERASURE_CREATE: ControlStmt = ControlStmt::at(
    "subject.erasure_create",
    "BEGIN TRANSACTION; \
     CREATE $eid SET org = $org, subject_ref = $ref, subject = $u, requested_by = $by, requester = $requester, status = 'pending', \
        open_slot = $ref, running_slot = <string> $eid, closes = $closes; \
     INSERT IGNORE INTO job { id: $jid, kind: 'subject_erase', owner: $owner, payload: $payload, idempotency_key: $key, run_at: time::now(), max_attempts: 8 }; \
     RETURN (SELECT <string> id AS id, status, requested_at FROM ONLY $eid); \
     COMMIT TRANSACTION;",
    3,
);
/// The open erasure of this person (pending, running, held or failed), if any.
pub const ERASURE_OPEN: ControlStmt = ControlStmt::new(
    "subject.erasure_open",
    "SELECT <string> id AS id, status, requested_at FROM erasure WHERE open_slot = $ref LIMIT 1",
);
/// Open erasures, for the start-up heal (one with no live job gets a new one).
pub const ERASURES_OPEN_ALL: ControlStmt = ControlStmt::new(
    "subject.erasures_open_all",
    "SELECT VALUE <string> id FROM erasure WHERE status IN ['pending', 'running', 'failed']",
);
/// The live job of an erasure, if there is one.
pub const ERASURE_LIVE_JOB: ControlStmt = ControlStmt::new(
    "subject.erasure_live_job",
    "SELECT VALUE id FROM job WHERE kind = 'subject_erase' AND payload.erasure = $eid AND status IN ['ready', 'running']",
);
pub const ERASURE_GET: ControlStmt = ControlStmt::new("subject.erasure_get", "SELECT * FROM ONLY $id");
/// Takes the workspace's running slot (unique): a second erasure in the same workspace fails here
/// with a duplicate and waits for its retry.
pub const ERASURE_RUNNING: ControlStmt = ControlStmt::new(
    "subject.erasure_running",
    "UPDATE $id SET status = 'running', running_slot = org, held_at = NONE, error = NONE WHERE status IN ['pending', 'running', 'failed'] RETURN VALUE <string> id",
);
/// The identifiers the post-check needs, saved before anything is deleted.
pub const ERASURE_WORK: ControlStmt = ControlStmt::new("subject.erasure_work", "UPDATE $id SET work = $work, closes = $closes RETURN NONE");
/// Parked by a legal hold placed after the request; lifting the hold resumes it.
pub const ERASURE_HELD: ControlStmt = ControlStmt::new(
    "subject.erasure_held",
    "UPDATE $id SET status = 'held', held_at = time::now(), running_slot = <string> id RETURN NONE",
);
/// The held erasures of `$u` go back to pending (the hold was lifted).
pub const ERASURES_HELD_OF: ControlStmt = ControlStmt::new(
    "subject.erasures_held_of",
    "UPDATE erasure SET status = 'pending', held_at = NONE WHERE subject = $u AND status = 'held' RETURN VALUE <string> id",
);
/// Whether `$u` has an erasure open (a hold placed now parks it).
pub const ERASURE_OPEN_OF: ControlStmt = ControlStmt::new(
    "subject.erasure_open_of",
    "SELECT VALUE id FROM erasure WHERE subject = $u AND status IN ['pending', 'running', 'failed', 'held']",
);
/// The completion record: counts, the post-check and the review counts; the person's id and the
/// working record are dropped, and both slots become the row's own id.
pub const ERASURE_DONE: ControlStmt = ControlStmt::new(
    "subject.erasure_done",
    "UPDATE $id SET status = 'done', subject = NONE, work = {}, open_slot = <string> id, running_slot = <string> id, counts = $counts, \
     post_check = $post_check, review = $review, error = NONE, completed_at = time::now() RETURN NONE",
);
pub const ERASURE_FAILED: ControlStmt = ControlStmt::new(
    "subject.erasure_failed",
    "UPDATE $id SET status = 'failed', error = $error, post_check = $post_check, running_slot = <string> id RETURN NONE",
);
/// One org's erasures, newest first (Settings > People shows them as proof).
pub const ERASURE_LIST: ControlStmt = ControlStmt::new(
    "subject.erasure_list",
    "SELECT <string> id AS id, subject_ref, requested_by, status, counts, post_check, review, error, requested_at, completed_at FROM erasure \
     WHERE org = $org ORDER BY requested_at DESC LIMIT 50",
);
/// Every finished erasure's reference and the workspaces it closed (the restore re-apply).
pub const ERASURE_REFS: ControlStmt = ControlStmt::new("subject.erasure_refs", "SELECT subject_ref, closes FROM erasure WHERE status = 'done'");
/// Whether any of `$refs` has an erasure on record (resync suppression).
pub const ERASURE_HAS_REF: ControlStmt =
    ControlStmt::new("subject.erasure_has_ref", "SELECT VALUE id FROM erasure WHERE subject_ref IN $refs LIMIT 1");
/// Every account (the restore re-apply).
pub const USERS_WITH_ORG: ControlStmt = ControlStmt::new("subject.users_with_org", "SELECT VALUE id FROM user");
/// Which of `$ids` still have an account.
pub const USERS_EXISTING: ControlStmt = ControlStmt::new("subject.users_existing", "SELECT VALUE id FROM user WHERE id IN $ids");
/// Closes a workspace whose only member was erased: nothing is served from it or joins it again.
pub const CLOSE_ORG: ControlStmt = ControlStmt::new(
    "subject.close_org",
    "UPDATE type::record('tenant', $okey) SET status = 'closed' RETURN NONE; UPDATE type::record('org', $okey) SET closed_at = time::now() RETURN NONE",
);
/// Ready workspaces (the re-apply sweeps each).
pub const READY_ORGS: ControlStmt = ControlStmt::new("subject.ready_orgs", "SELECT VALUE org FROM tenant WHERE status = 'ready'");

/// Every org the subject belongs to (an erasure covers each).
pub const USER_ORGS: ControlStmt = ControlStmt::new("subject.user_orgs", "SELECT VALUE org FROM membership WHERE user = $u ORDER BY created_at, id");

/// The subject's API token ids (actor ids in the audit trail and the change log).
pub const TOKEN_IDS: ControlStmt = ControlStmt::new("subject.token_ids", "SELECT VALUE <string> id FROM api_token WHERE owner = $u");

/// Everything the control database holds about the subject. Their audit events stay (the security
/// ledger is append-only and pruned only by retention) but no longer name them: the actor id, a
/// target that is their id and their email anywhere in the detail become "erased", and a row filed
/// under them gets `user:erased`, an account that never exists, so no workspace's activity log lists
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
     LET $fc = (UPDATE failure_capsule SET user = NONE, args = '' WHERE user = <string> $u RETURN BEFORE); \
     UPDATE failure_capsule SET args = string::replace(args, <regex> $email_re, '[erased person]') WHERE string::contains(string::lowercase(args), $email_lc) RETURN NONE; \
     UPDATE erasure SET requester = NONE WHERE requester = $u RETURN NONE; \
     LET $ae = (UPDATE audit_event SET user = IF user = $u THEN user:erased ELSE user END, actor_id = IF actor_id IN $actors THEN 'erased' ELSE actor_id END, \
        target = IF target IN $actors OR string::contains(target, $key) THEN 'erased' ELSE target END, \
        detail = string::replace(detail, <regex> $email_re, '[erased person]') \
        WHERE user = $u OR actor_id IN $actors OR target IN $actors OR string::contains(target, $key) OR string::contains(string::lowercase(detail), $email_lc) RETURN BEFORE); \
     LET $ms = (DELETE membership WHERE user = $u RETURN BEFORE); \
     LET $us = (DELETE $u RETURN BEFORE); \
     RETURN { app_connections: array::len($og) + array::len($ot) + array::len($oc), api_tokens: array::len($at), sessions: array::len($se), \
        sign_in_identities: array::len($id), usage_counters: array::len($tu), jobs: array::len($jb), failure_capsules: array::len($fc), \
        audit_events_anonymised: array::len($ae), org_memberships: array::len($ms), account: array::len($us) }; \
     COMMIT TRANSACTION;",
    16,
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
       failure_capsule: count((SELECT id FROM failure_capsule WHERE user = <string> $u OR string::contains(string::lowercase(args), $email_lc))), \
       audit_event: count((SELECT id FROM audit_event WHERE user = $u OR actor_id IN $actors OR target IN $actors OR string::contains(target, $key) OR string::contains(string::lowercase(detail), $email_lc))), \
       erasure: count((SELECT id FROM erasure WHERE (subject = $u OR requester = $u) AND id != $erasure)) \
     }",
);

pub const CONTROL_ALL: &[&ControlStmt] = &[
    &ACCOUNT,
    &CONFIRM_ROW,
    &ORG_MEMBERSHIPS,
    &OPEN_SUBJECTS,
    &ORG_OWNER_TRANSFER,
    &HOLD_OF,
    &SET_LEGAL_HOLD,
    &ERASURE_CREATE,
    &ERASURE_OPEN,
    &ERASURES_OPEN_ALL,
    &ERASURE_LIVE_JOB,
    &ERASURE_GET,
    &ERASURE_RUNNING,
    &ERASURE_WORK,
    &ERASURE_HELD,
    &ERASURES_HELD_OF,
    &ERASURE_OPEN_OF,
    &ERASURE_DONE,
    &ERASURE_FAILED,
    &ERASURE_LIST,
    &ERASURE_REFS,
    &ERASURE_HAS_REF,
    &USERS_WITH_ORG,
    &USERS_EXISTING,
    &CLOSE_ORG,
    &READY_ORGS,
    &USER_ORGS,
    &TOKEN_IDS,
    &ERASE_CONTROL,
    &CONTROL_RESIDUE,
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The statement index of the RETURN in a transaction (SurrealDB gives BEGIN, LET and COMMIT a slot too).
    fn return_slot(sql: &str) -> usize {
        let (mut depth, mut i, mut start) = (0i32, 0usize, 0usize);
        let b = sql.as_bytes();
        for (pos, c) in sql.char_indices() {
            match c {
                '(' | '{' | '[' => depth += 1,
                ')' | '}' | ']' => depth -= 1,
                ';' if depth == 0 => {
                    if std::str::from_utf8(&b[start..pos]).unwrap().trim_start().starts_with("RETURN") {
                        return i;
                    }
                    i += 1;
                    start = pos + 1;
                }
                _ => {}
            }
        }
        panic!("no RETURN in {sql}");
    }

    #[test]
    fn every_transaction_takes_its_return_slot() {
        for s in [&REASSIGN, &SCRUB_EMAIL, &ERASE_SPACES, &ERASE_OWNED] {
            assert_eq!(s.slot, return_slot(s.sql), "{}", s.name);
        }
        assert_eq!(ERASE_CONTROL.slot, return_slot(ERASE_CONTROL.sql), "{}", ERASE_CONTROL.name);
    }
}
