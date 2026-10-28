//! Statements for the secret backfill (`jobs::redact_backfill`). See the module docs in store/mod.rs.
//!
//! Each walk reads one batch in id order after the cursor, and each rewrite is a compare-and-set on the
//! text it read: a row edited since is left alone (its edit went through the redacting write path).

use super::Stmt;

pub const ALL: &[&Stmt] = &[
    &MARKER,
    &SAVE_MARKER,
    &MEMORY_FIRST,
    &MEMORY_AFTER,
    &SET_MEMORY_TEXT,
    &SKILL_FIRST,
    &SKILL_AFTER,
    &SET_SKILL_TEXT,
    &AUDIT_FIRST,
    &AUDIT_AFTER,
    &SET_AUDIT_SUMMARY,
];

pub const MARKER: Stmt = Stmt::new("redact.marker", "SELECT * FROM ONLY $id");

pub const SAVE_MARKER: Stmt = Stmt::new(
    "redact.save_marker",
    "UPSERT $id SET stage = $stage, cursor = $cursor, changed_memory = $changed_memory, changed_skill = $changed_skill, \
     changed_audit_log = $changed_audit_log, done_at = $done_at, updated_at = time::now()",
);

// Each walk is a first-batch statement and an `id > $cursor` one, so the cursor is a plain id range.
pub const MEMORY_FIRST: Stmt = Stmt::new("redact.memory_first", "SELECT id, owner, text FROM memory ORDER BY id LIMIT $limit");
pub const MEMORY_AFTER: Stmt = Stmt::new("redact.memory_after", "SELECT id, owner, text FROM memory WHERE id > $cursor ORDER BY id LIMIT $limit");

/// The `memory_text_changed` event drops the vector; the embed job makes a new one from the redacted text.
pub const SET_MEMORY_TEXT: Stmt = Stmt::new("redact.set_memory_text", "UPDATE $id SET text = $text WHERE text = $old RETURN VALUE id");

pub const SKILL_FIRST: Stmt = Stmt::new("redact.skill_first", "SELECT id, owner, name, description, body FROM skill ORDER BY id LIMIT $limit");
pub const SKILL_AFTER: Stmt =
    Stmt::new("redact.skill_after", "SELECT id, owner, name, description, body FROM skill WHERE id > $cursor ORDER BY id LIMIT $limit");

/// The `skill_text_changed` event drops the vector when the description changed; the embed job's skill
/// backlog makes a new one.
pub const SET_SKILL_TEXT: Stmt = Stmt::new(
    "redact.set_skill_text",
    "UPDATE $id SET description = $description, body = $body WHERE description = $old_description AND body = $old_body RETURN VALUE id",
);

pub const AUDIT_FIRST: Stmt = Stmt::new("redact.audit_first", "SELECT id, args_summary FROM audit_log ORDER BY id LIMIT $limit");
pub const AUDIT_AFTER: Stmt = Stmt::new("redact.audit_after", "SELECT id, args_summary FROM audit_log WHERE id > $cursor ORDER BY id LIMIT $limit");

pub const SET_AUDIT_SUMMARY: Stmt =
    Stmt::new("redact.set_audit_summary", "UPDATE $id SET args_summary = $summary WHERE args_summary = $old RETURN VALUE id");
