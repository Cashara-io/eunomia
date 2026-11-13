//! Statements for the fact-date backfill (`jobs::fact_dates`). See the module docs in store/mod.rs.
//!
//! The walk reads one batch of memories in id order after the cursor; each re-date is a compare-and-set
//! on the fact still being undated, still tied to the same record and never re-dated before, written in
//! one transaction with its ledger row (`fact_dates_redated`).

use super::Stmt;

pub const ALL: &[&Stmt] = &[&MARKER, &SAVE_MARKER, &MEMORY_FIRST, &MEMORY_AFTER, &REDATE, &EARLIER_LIVE, &LEDGER_PENDING, &UNDO];

pub const MARKER: Stmt = Stmt::new("fact_dates.marker", "SELECT * FROM ONLY $id");

pub const SAVE_MARKER: Stmt = Stmt::new(
    "fact_dates.save_marker",
    "UPSERT $id SET cursor = $cursor, cutoff = $cutoff, changed = $changed, suspect_superseded = $suspect, done_at = $done_at, \
     updated_at = time::now()",
);

// A first-batch statement and an `id > $cursor` one, so the cursor is a plain id range. Every row of the
// batch comes back (the cursor must move past it); the job picks the candidates.
pub const MEMORY_FIRST: Stmt = Stmt::new(
    "fact_dates.memory_first",
    "SELECT id, vault, subject, type, status, source, occurred_at, created_at, source.occurred_at AS source_at FROM memory \
     ORDER BY id LIMIT $limit",
);
pub const MEMORY_AFTER: Stmt = Stmt::new(
    "fact_dates.memory_after",
    "SELECT id, vault, subject, type, status, source, occurred_at, created_at, source.occurred_at AS source_at FROM memory \
     WHERE id > $cursor ORDER BY id LIMIT $limit",
);

/// Dates one fact by its record and writes its ledger row, all or nothing. Nothing happens when the fact
/// was dated (by its writer or by hand) or re-pointed since it was read, or was re-dated by any earlier run.
pub const REDATE: Stmt = Stmt::at(
    "fact_dates.redate",
    r#"BEGIN TRANSACTION;
        LET $seen = (SELECT VALUE id FROM $ledger);
        LET $done = IF array::len($seen) > 0 { [] } ELSE {
            (UPDATE $id SET occurred_at = $at WHERE occurred_at = NONE AND source = $source RETURN VALUE id)
        };
        IF array::len($done) > 0 {
            CREATE $ledger SET memory = $id, previous = NONE, occurred_at = $at, source = $source, version = $version
        };
        RETURN $done;
        COMMIT TRANSACTION;"#,
    4, // BEGIN 0, LET 1, LET 2, IF 3, RETURN 4
);

/// Live raw facts of `$subject` dated before `$at`: one of them may be what wrongly replaced a fact now dated `$at`.
pub const EARLIER_LIVE: Stmt = Stmt::new(
    "fact_dates.earlier_live",
    r#"SELECT VALUE count() FROM memory WHERE subject = $subject AND id != $id AND type IN ["world","experience"]
       AND status != "superseded" AND (occurred_at ?? created_at) < $at GROUP ALL"#,
);

/// The ledger rows not undone yet, a batch at a time.
pub const LEDGER_PENDING: Stmt =
    Stmt::new("fact_dates.ledger_pending", "SELECT id, memory, memory.vault AS vault, previous, occurred_at FROM fact_dates_redated WHERE undone_at = NONE LIMIT $limit");

/// Undoes one re-date: the fact gets its previous date back unless its date changed since (then that change
/// stands). The ledger row is kept, marked undone, so no later run dates the fact again. Returns the fact
/// when it was restored.
pub const UNDO: Stmt = Stmt::at(
    "fact_dates.undo",
    r#"BEGIN TRANSACTION;
        LET $restored = (UPDATE $memory SET occurred_at = $previous WHERE occurred_at = $at RETURN VALUE id);
        UPDATE $id SET undone_at = time::now();
        RETURN $restored;
        COMMIT TRANSACTION;"#,
    3, // BEGIN 0, LET 1, UPDATE 2, RETURN 3
);
