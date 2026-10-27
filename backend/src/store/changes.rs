//! Statements for the per-vault change log (`changes.rs`). See the module docs in store/mod.rs.
//!
//! The caller authorizes the vault; the statements filter by it.

use super::Stmt;

pub const ALL: &[&Stmt] = &[&APPEND, &AFTER, &IDS_IN_VAULT];

/// Takes the next `len($items)` seqs from the vault's counter row and writes the changes, in one
/// transaction: two writers to one vault conflict on the counter and one retries, so seqs never repeat
/// or skip, and rows commit before any row with a higher seq (a reader that has seen `n` never misses
/// one below it). `$items` is `[{i, op, kind, item}]` with `i` counting from 1.
pub const APPEND: Stmt = Stmt::at(
    "changes.append",
    r#"BEGIN TRANSACTION;
        LET $last = (UPSERT ONLY $counter SET n += array::len($items) RETURN VALUE n);
        LET $base = $last - array::len($items);
        INSERT INTO change (SELECT $vault AS vault, $base + i AS seq, op, kind, item, $actor AS actor FROM $items)
            RETURN seq, op, kind, item AS id, actor, at;
        COMMIT TRANSACTION;"#,
    3, // BEGIN 0, two LETs 1 and 2, INSERT 3
);

/// The vault's changes after `$after`, oldest first.
pub const AFTER: Stmt = Stmt::new(
    "changes.after",
    "SELECT seq, op, kind, item AS id, actor, at FROM change WHERE vault = $vault AND seq > $after ORDER BY seq LIMIT $limit",
);

/// What a vault holds that the log names, for a new vault a clone or merge filled: memories (slot 0),
/// relations (slot 1) and documents (slot 2). Entities are listed per table (`vaults/service.rs`).
pub const IDS_IN_VAULT: Stmt = Stmt::new(
    "changes.ids_in_vault",
    "SELECT VALUE id FROM memory WHERE vault = $vault; SELECT VALUE id FROM relates_to WHERE in.vault = $vault; \
     SELECT VALUE id FROM document WHERE vault = $vault",
);
