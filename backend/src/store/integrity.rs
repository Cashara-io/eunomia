//! Statements for the vault integrity check (`integrity.rs`). The caller authorizes the vault; the
//! statements filter by it.

use super::Stmt;

pub const ALL: &[&Stmt] = &[&SCAN, &REPAIR];

/// One vault's integrity problems, one slot each:
/// 0. memories whose subject entity no longer exists (never shown, never recalled by graph);
/// 1. memories filed in this vault whose subject lives in another vault (reported, not repaired);
/// 2. relations with one end in this vault and the other end gone;
/// 3. the vault's highest change-log seq (empty when the log is empty);
/// 4. the vault's change-log counter (empty when there is no counter row yet).
pub const SCAN: Stmt = Stmt::new(
    "integrity.scan",
    "SELECT VALUE id FROM memory WHERE vault = $vault AND subject.id = NONE LIMIT $limit; \
     SELECT VALUE id FROM memory WHERE vault = $vault AND subject.id != NONE AND subject.vault != $vault LIMIT $limit; \
     SELECT VALUE id FROM relates_to WHERE (in.vault = $vault AND out.id = NONE) OR (out.vault = $vault AND in.id = NONE) LIMIT $limit; \
     SELECT VALUE seq FROM change WHERE vault = $vault ORDER BY seq DESC LIMIT 1; \
     SELECT VALUE n FROM $counter",
);

/// Removes the dangling memories and relations a [`SCAN`] found (re-checked here, so a row that
/// became valid in between stays) and lifts the change-log counter to the highest seq written, so
/// the next append can't reuse a seq. One transaction.
pub const REPAIR: Stmt = Stmt::new(
    "integrity.repair",
    "BEGIN TRANSACTION; \
     DELETE memory WHERE id IN $memories AND vault = $vault AND subject.id = NONE; \
     DELETE relates_to WHERE id IN $relations AND (in.id = NONE OR out.id = NONE); \
     IF $top > 0 { UPSERT $counter SET n = math::max([n ?? 0, $top]) }; \
     COMMIT TRANSACTION;",
);
