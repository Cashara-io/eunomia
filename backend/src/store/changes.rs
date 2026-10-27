//! Statements for the per-vault change log (`changes.rs`). See the module docs in store/mod.rs.
//!
//! The caller authorizes the vault; the statements filter by it.

use super::Stmt;

pub const ALL: &[&Stmt] = &[&APPEND, &AFTER];

/// Takes the vault's next seq from its counter row and writes the change, in one transaction: two
/// writers to one vault conflict on the counter and one retries, so seqs never repeat or skip, and a
/// row commits before any row with a higher seq (a reader that has seen `n` never misses one below it).
pub const APPEND: Stmt = Stmt::at(
    "changes.append",
    r#"BEGIN TRANSACTION;
        LET $seq = (UPSERT ONLY $counter SET n += 1 RETURN VALUE n);
        CREATE change SET vault = $vault, seq = $seq, op = $op, kind = $kind, item = $item, actor = $actor RETURN seq, op, kind, item AS id, actor, at;
        COMMIT TRANSACTION;"#,
    2, // BEGIN 0, LET 1, CREATE 2
);

/// The vault's changes after `$after`, oldest first.
pub const AFTER: Stmt = Stmt::new(
    "changes.after",
    "SELECT seq, op, kind, item AS id, actor, at FROM change WHERE vault = $vault AND seq > $after ORDER BY seq LIMIT $limit",
);
