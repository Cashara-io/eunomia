//! Statements for the fact-date backfill (`jobs::fact_dates`). See the module docs in store/mod.rs.
//!
//! Both walks (the backfill over `memory`, undo over the ledger) are record-id range scans (`FROM $range`, a bound record-id range `table:<cursor>>..`; `type::record` is outside the
//! hardened function list), so each batch starts at the
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
    &MEMORY_BY_ID,
    &REDATE,
    &LEDGER_FIRST,
    &LEDGER_AFTER,
    &UNDO,
];

pub const MARKER: Stmt = Stmt::new("fact_dates.marker", "SELECT * FROM ONLY $id");

pub const SAVE_MARKER: Stmt = Stmt::new(
    "fact_dates.save_marker",
    "UPSERT $id SET cursor = $cursor, cutoff = $cutoff, changed = $changed, done_at = $done_at, updated_at = time::now()",
);

/// What the job reads of each memory: its dates, whether it was edited, and its record's connector and
/// the date the record stores now.
macro_rules! memory_row {
    () => {
        "id, vault, type, source, occurred_at, asserted_at, created_at, updated_at, version, \
         source.source AS source_kind, source.occurred_at AS source_at"
    };
}
// Every row of the batch comes back (the cursor must move past it); the job picks the candidates.
pub const MEMORY_FIRST: Stmt = Stmt::new("fact_dates.memory_first", concat!("SELECT ", memory_row!(), " FROM memory:.. LIMIT $limit"));
pub const MEMORY_AFTER: Stmt =
    Stmt::new("fact_dates.memory_after", concat!("SELECT ", memory_row!(), " FROM $range LIMIT $limit"));

/// The same rows for given memories (a supersede check dating facts the walk has not reached yet).
pub const MEMORY_BY_ID: Stmt = Stmt::new("fact_dates.memory_by_id", concat!("SELECT ", memory_row!(), " FROM $ids"));

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

// Undo walks the ledger by record-id range, like the backfill walks `memory`.
pub const LEDGER_FIRST: Stmt = Stmt::new(
    "fact_dates.ledger_first",
    "SELECT id, memory, memory.vault AS vault, previous, previous_asserted, occurred_at, asserted_at, undone_at FROM fact_dates_redated:.. \
     LIMIT $limit",
);
pub const LEDGER_AFTER: Stmt = Stmt::new(
    "fact_dates.ledger_after",
    "SELECT id, memory, memory.vault AS vault, previous, previous_asserted, occurred_at, asserted_at, undone_at \
     FROM $range LIMIT $limit",
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
