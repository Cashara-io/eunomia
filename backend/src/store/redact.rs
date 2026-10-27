//! Statements for the secret backfill (`jobs::redact_backfill`). See the module docs in store/mod.rs.
//!
//! Each walk reads one batch in id order after the cursor, and each rewrite is a compare-and-set on the
//! text it read: a row edited since is left alone (its edit went through the redacting write path).

use super::Stmt;

pub const ALL: &[&Stmt] = &[&MARKER, &SAVE_MARKER, &MEMORY_BATCH, &SET_MEMORY_TEXT, &SKILL_BATCH, &SET_SKILL_TEXT, &AUDIT_BATCH, &SET_AUDIT_SUMMARY];

pub const MARKER: Stmt = Stmt::new("redact.marker", "SELECT * FROM ONLY $id");

pub const SAVE_MARKER: Stmt = Stmt::new(
    "redact.save_marker",
    "UPSERT $id SET stage = $stage, cursor = $cursor, changed_memory = $changed_memory, changed_skill = $changed_skill, \
     changed_audit_log = $changed_audit_log, done_at = $done_at, updated_at = time::now()",
);

// ponytail: `id > $cursor ORDER BY id` may scan the table per batch (O(rows^2 / BATCH) reads for a one-off
// job); a record range (`memory:$cursor>..`) is the upgrade if a vault ever gets big enough to notice.
pub const MEMORY_BATCH: Stmt =
    Stmt::new("redact.memory_batch", "SELECT id, owner, text FROM memory WHERE $cursor = NONE OR id > $cursor ORDER BY id LIMIT $limit");

/// The `memory_text_changed` event drops the vector; the embed job makes a new one from the redacted text.
pub const SET_MEMORY_TEXT: Stmt = Stmt::new("redact.set_memory_text", "UPDATE $id SET text = $text WHERE text = $old RETURN VALUE id");

pub const SKILL_BATCH: Stmt = Stmt::new(
    "redact.skill_batch",
    "SELECT id, owner, name, description, body FROM skill WHERE $cursor = NONE OR id > $cursor ORDER BY id LIMIT $limit",
);

/// The `skill_text_changed` event drops the vector when the description changed.
pub const SET_SKILL_TEXT: Stmt = Stmt::new(
    "redact.set_skill_text",
    "UPDATE $id SET description = $description, body = $body WHERE description = $old_description AND body = $old_body RETURN VALUE id",
);

pub const AUDIT_BATCH: Stmt =
    Stmt::new("redact.audit_batch", "SELECT id, args_summary FROM audit_log WHERE $cursor = NONE OR id > $cursor ORDER BY id LIMIT $limit");

pub const SET_AUDIT_SUMMARY: Stmt =
    Stmt::new("redact.set_audit_summary", "UPDATE $id SET args_summary = $summary WHERE args_summary = $old RETURN VALUE id");
