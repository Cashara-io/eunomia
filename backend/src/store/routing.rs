//! Statements for connector routing ("Move existing data", `sources::routing`). See the module docs in
//! store/mod.rs.

use super::Stmt;

/// One page of `$owner`'s records from one connection of `$source` (see `cache::DELETE_SOURCE_DATA`
/// for `$connector` and `$first`), whatever vault they are filed in.
pub const RECORDS_OF_SOURCE: Stmt = Stmt::new(
    "routing.records_of_source",
    "SELECT id, vault, title, route_keys FROM cache_record WHERE owner = $owner AND source = $source \
     AND (connector = $connector OR ($first AND connector = NONE)) ORDER BY id LIMIT $limit START $start",
);

/// The facts drawn from `$ids` (records), by the `memory_source_idx` index.
pub const FACTS_OF: Stmt =
    Stmt::new("routing.facts_of", "SELECT id, subject, vault, text, type, source FROM memory WHERE source IN $ids");

/// The relations drawn from `$ids` (records), by the `relates_to_source_idx` index.
pub const EDGES_OF: Stmt =
    Stmt::new("routing.edges_of", "SELECT id, in, out, label, source FROM relates_to WHERE source IN $ids");

/// Re-files one fact `$id` from `$old` (an entity) to `$subject` (the same entity in `$vault`), unless
/// `$subject` already has the same fact: then the moved one is dropped, so nothing is duplicated. Like
/// deleting a fact, `$old`'s observations lose it from their lineage and go stale (or go, when it was
/// all they had); like writing one, `$subject`'s observation goes stale. One transaction.
pub const MOVE_FACT: Stmt = Stmt::new(
    "routing.move_fact",
    r#"BEGIN TRANSACTION;
        LET $same = (SELECT VALUE id FROM memory WHERE subject = $subject AND type = $type AND text = $text LIMIT 1);
        IF array::len($same) > 0 { DELETE $id } ELSE { UPDATE $id SET subject = $subject, vault = $vault, updated_at = time::now() };
        DELETE memory WHERE type = "observation" AND subject = $old
            AND array::len(source_memories ?? []) > 0 AND array::len(array::complement(source_memories ?? [], [$id])) = 0;
        UPDATE memory SET status = "stale", updated_at = time::now(),
            source_memories = IF source_memories THEN array::complement(source_memories, [$id]) ELSE NONE END
            WHERE type = "observation" AND subject = $old;
        UPDATE memory SET status = "stale" WHERE type = "observation" AND subject = $subject AND status != "stale";
        COMMIT TRANSACTION;"#,
);

/// Re-creates relation `$edge` between `$in` and `$out` (the same entities in the destination vault),
/// unless they already have it, and drops the original. One transaction.
pub const MOVE_EDGE: Stmt = Stmt::new(
    "routing.move_edge",
    r#"BEGIN TRANSACTION;
        IF array::len((SELECT VALUE id FROM relates_to WHERE in = $in AND out = $out AND label = $label LIMIT 1)) = 0 {
            RELATE $in->relates_to->$out SET label = $label, owner = $owner, source = $source
        };
        DELETE $edge;
        COMMIT TRANSACTION;"#,
);

/// Re-files records `$ids` to `$vault` (NONE: the owner's personal vault).
pub const MOVE_RECORDS: Stmt = Stmt::new("routing.move_records", "UPDATE $ids SET vault = $vault");

/// Every connector of `$owner` with routing set, for re-pointing after a vault merge.
pub const ROUTED_CONNECTORS: Stmt =
    Stmt::new("routing.routed_connectors", "SELECT id, routing FROM connector WHERE owner = $owner AND routing != {} AND routing != NONE");

pub const SET_ROUTING: Stmt =
    Stmt::new("routing.set_routing", "UPDATE $id SET routing = $routing, updated_at = time::now() RETURN AFTER");

pub const ALL: &[&Stmt] = &[&RECORDS_OF_SOURCE, &FACTS_OF, &EDGES_OF, &MOVE_FACT, &MOVE_EDGE, &MOVE_RECORDS, &ROUTED_CONNECTORS, &SET_ROUTING];
