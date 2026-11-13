//! Statements for the fact-date backfill (`jobs::fact_dates`). See the module docs in store/mod.rs.
//!
//! Both walks are record-id range scans (`type::record(table, $key>..)`), so each batch starts at the
//! cursor instead of scanning the table from its start (`WHERE id > $cursor` plans as a table scan with
//! a filter on SurrealDB 3.3.5). Each re-date is a compare-and-set on the fact still being undated,
//! unedited, tied to the same record and never re-dated before, written in one transaction with its
//! ledger row (`fact_dates_redated`).

use super::Stmt;

pub const ALL: &[&Stmt] = &[
    &MARKER,
    &SAVE_MARKER,
    &MEMORY_FIRST,
    &MEMORY_AFTER,
    &REDATE,
    &LEDGER_FIRST,
    &LEDGER_AFTER,
    &REPLACED_SUSPECT,
    &REPLACER_SUSPECT,
    &SET_SUSPECT,
    &LEDGER_PENDING,
    &UNDO,
];

pub const MARKER: Stmt = Stmt::new("fact_dates.marker", "SELECT * FROM ONLY $id");

pub const SAVE_MARKER: Stmt = Stmt::new(
    "fact_dates.save_marker",
    "UPSERT $id SET stage = $stage, cursor = $cursor, cutoff = $cutoff, changed = $changed, suspect_replaced = $suspect_replaced, \
     suspect_replacer = $suspect_replacer, done_at = $done_at, updated_at = time::now()",
);

/// What the job reads of each memory: its dates, whether it was edited, and its record's dates (the date
/// it stores now, and a creation date where the connector keeps one in the payload: Notion
/// `created_time`, GitHub and Linear `created_at`, Todoist `added_at`).
macro_rules! memory_row {
    () => {
        "id, vault, subject, type, status, source, occurred_at, asserted_at, created_at, updated_at, version, \
         source.source AS source_kind, source.occurred_at AS source_at, \
         (source.payload.created_time ?? source.payload.created_at ?? source.payload.added_at) AS source_created"
    };
}
// Every row of the batch comes back (the cursor must move past it); the job picks the candidates.
pub const MEMORY_FIRST: Stmt = Stmt::new("fact_dates.memory_first", concat!("SELECT ", memory_row!(), " FROM memory:.. LIMIT $limit"));
pub const MEMORY_AFTER: Stmt =
    Stmt::new("fact_dates.memory_after", concat!("SELECT ", memory_row!(), " FROM type::record('memory', $key>..) LIMIT $limit"));

/// Dates one fact and writes its ledger row, all or nothing. Nothing happens when the fact was dated
/// (by its writer, by hand or by an edit) or re-pointed since it was read, or was re-dated by any run.
pub const REDATE: Stmt = Stmt::at(
    "fact_dates.redate",
    r#"BEGIN TRANSACTION;
        LET $seen = (SELECT VALUE id FROM $ledger);
        LET $done = IF array::len($seen) > 0 { [] } ELSE {
            (UPDATE $id SET occurred_at = $at, asserted_at = $told
             WHERE occurred_at = NONE AND asserted_at = NONE AND source = $source AND version = $version RETURN VALUE id)
        };
        IF array::len($done) > 0 {
            CREATE $ledger SET memory = $id, previous = NONE, previous_asserted = NONE, occurred_at = $at, asserted_at = $told,
                source = $source, version = $version
        };
        RETURN $done;
        COMMIT TRANSACTION;"#,
    4, // BEGIN 0, LET 1, LET 2, IF 3, RETURN 4
);

// The suspect pass walks the ledger once every fact is re-dated, so it compares final dates.
pub const LEDGER_FIRST: Stmt = Stmt::new(
    "fact_dates.ledger_first",
    "SELECT id, memory, memory.subject AS subject, memory.status AS status, asserted_at FROM fact_dates_redated:.. LIMIT $limit",
);
pub const LEDGER_AFTER: Stmt = Stmt::new(
    "fact_dates.ledger_after",
    "SELECT id, memory, memory.subject AS subject, memory.status AS status, asserted_at FROM type::record('fact_dates_redated', $key>..) \
     LIMIT $limit",
);
/// A superseded re-dated fact: is a live raw fact of its subject now told before it? Then that fact may
/// be the one that replaced it, wrongly (it was taken as newer only because of its old job-run date).
pub const REPLACED_SUSPECT: Stmt = Stmt::new(
    "fact_dates.replaced_suspect",
    r#"SELECT VALUE id FROM memory WHERE subject = $subject AND id != $id AND type IN ["world","experience"]
       AND status != "superseded" AND (asserted_at ?? updated_at ?? created_at) < $told LIMIT 1"#,
);
/// A live re-dated fact: is a superseded raw fact of its subject now told after it? Then this fact may
/// have replaced it, wrongly.
pub const REPLACER_SUSPECT: Stmt = Stmt::new(
    "fact_dates.replacer_suspect",
    r#"SELECT VALUE id FROM memory WHERE subject = $subject AND id != $id AND type IN ["world","experience"]
       AND status = "superseded" AND (asserted_at ?? updated_at ?? created_at) > $told LIMIT 1"#,
);
pub const SET_SUSPECT: Stmt = Stmt::new("fact_dates.set_suspect", "UPDATE $id SET suspect = $suspect");

/// The ledger rows not undone yet, a batch at a time.
pub const LEDGER_PENDING: Stmt = Stmt::new(
    "fact_dates.ledger_pending",
    "SELECT id, memory, memory.vault AS vault, previous, previous_asserted, occurred_at, asserted_at FROM fact_dates_redated \
     WHERE undone_at = NONE LIMIT $limit",
);

/// Undoes one re-date: the fact gets its previous dates back unless they changed since (then that change
/// stands). The ledger row is kept, marked undone, so no later run dates the fact again. Returns the fact
/// when it was restored.
pub const UNDO: Stmt = Stmt::at(
    "fact_dates.undo",
    r#"BEGIN TRANSACTION;
        LET $restored = (UPDATE $memory SET occurred_at = $previous, asserted_at = $previous_asserted
                         WHERE occurred_at = $at AND asserted_at = $told RETURN VALUE id);
        UPDATE $id SET undone_at = time::now();
        RETURN $restored;
        COMMIT TRANSACTION;"#,
    3, // BEGIN 0, LET 1, UPDATE 2, RETURN 3
);
