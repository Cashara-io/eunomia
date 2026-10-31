//! Statements for agent skills (`tools/registry/skills.rs`). See the module docs in store/mod.rs.
//!
//! A skill belongs to a vault. The caller authorizes the vault (or reads only the vaults it may read)
//! and the statements filter by it.

use super::Stmt;

pub const ALL: &[&Stmt] = &[&UPSERT, &SET_EMBEDDING, &LIST_META, &PAGE, &BY_NAME, &DELETE];

/// Create or replace by (vault, name). The vector goes; `SET_EMBEDDING` sets it again if one can be made.
pub const UPSERT: Stmt = Stmt::new(
    "skills.upsert",
    "UPSERT skill SET owner = $owner, vault = $vault, name = $name, description = $description, body = $body, \
     embedding = NONE, embedding_spec = NONE, updated_at = time::now() WHERE vault = $vault AND name = $name RETURN AFTER",
);

/// Only if the skill still says what was embedded (an edit since keeps no vector).
pub const SET_EMBEDDING: Stmt = Stmt::new(
    "skills.set_embedding",
    "UPDATE $id SET embedding = $embedding, embedding_spec = $spec \
     WHERE vault = $vault AND name = $name AND description = $description",
);

/// What `skill_find` matches by keyword: no bodies.
pub const LIST_META: Stmt = Stmt::new(
    "skills.list_meta",
    "SELECT id, vault, name, description, updated_at FROM skill WHERE vault IN $vaults ORDER BY updated_at DESC LIMIT $limit",
);

/// One page of a vault's skills by name, after `$after` (`""` for the first): what `skill_list` returns.
/// Keyset paging on the `(vault, name)` unique index (0021): no OFFSET, so later pages cost no more to find.
pub const PAGE: Stmt = Stmt::new(
    "skills.page",
    "SELECT id, vault, name, description, updated_at FROM skill WHERE vault = $vault AND name > $after ORDER BY name LIMIT $limit",
);

pub const BY_NAME: Stmt = Stmt::new("skills.by_name", "SELECT * FROM skill WHERE vault IN $vaults AND name = $name ORDER BY id");

pub const DELETE: Stmt = Stmt::new("skills.delete", "DELETE $id");
