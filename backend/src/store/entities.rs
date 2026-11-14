//! Statements for the entities area. See the module docs in store/mod.rs.

use super::{dynamic, ControlStmt, Q, Stmt};
use crate::pool::OrgDb;

pub const EMAILS_FOR: ControlStmt = ControlStmt::new("entities.emails_for", "SELECT id, email FROM user WHERE id IN $ids");

pub const MERGE_ALIASES: Stmt = Stmt::new(
    "entities.merge_aliases",
    "UPDATE $id SET aliases = array::sort(array::union(aliases, $aliases)), updated_at = time::now() RETURN AFTER",
);

// One round trip, all-or-nothing; see add_memory in entities/service.rs.
pub const WRITE_OBSERVATION: Stmt = Stmt::at(
    "entities.write_observation",
    r#"BEGIN TRANSACTION;
        LET $cur = (SELECT VALUE id FROM $obs_id);
        LET $legacy = IF array::len($cur) > 0 { [] } ELSE {
            (SELECT VALUE id FROM memory WHERE subject = $subject AND type = "observation" LIMIT 1)
        };
        LET $target = array::concat($cur, $legacy);
        LET $row = IF array::len($target) > 0 {
            (UPDATE $target[0] SET text = $text, version = version + 1, status = "fresh", updated_at = time::now(), occurred_at = $occurred_at ?? occurred_at RETURN *, vault.name AS vault_name)
        } ELSE {
            (CREATE $obs_id SET owner = $owner, vault = $vault, subject = $subject, text = $text, type = "observation", status = "fresh", source = $source, occurred_at = $occurred_at RETURN *, vault.name AS vault_name)
        };
        RETURN $row;
        COMMIT TRANSACTION;"#,
    5, // BEGIN 0, four LETs 1 to 4, RETURN 5
);

/// `$occurred_at` (when the fact happened; NONE: unknown) is written with the fact, never after it.
/// `asserted_at` (when we were told it) is `$told_at`, a source record's date, but never later than
/// now; with no `$told_at` (a person or agent wrote it) it is now. See docs/concepts.md.
pub const WRITE_FACT: Stmt = Stmt::at(
    "entities.write_fact",
    r#"BEGIN TRANSACTION;
        CREATE memory SET owner = $owner, vault = $vault, subject = $subject, text = $text, type = $type, source = $source, occurred_at = $occurred_at,
            asserted_at = IF $told_at != NONE AND $told_at < time::now() THEN $told_at ELSE time::now() END RETURN *, vault.name AS vault_name;
        LET $cur = (SELECT VALUE id FROM $obs_id);
        IF array::len($cur) > 0 {
            UPDATE $obs_id SET status = "stale", version = version + 1
        } ELSE {
            UPDATE memory SET status = "stale", version = version + 1 WHERE subject = $subject AND type = "observation"
        };
        COMMIT TRANSACTION;"#,
    1, // BEGIN 0, CREATE 1
);

pub const RELATE_RETURNING: Stmt = Stmt::new(
    "entities.relate_returning",
    "RELATE $in->relates_to->$out SET label = $label, owner = $owner, source = $source RETURN AFTER",
);

pub const RELATE_WITH_REASON: Stmt = Stmt::at(
    "entities.relate_with_reason",
    r#"BEGIN TRANSACTION;
        LET $existing = (SELECT * FROM relates_to WHERE in = $in AND out = $out AND (label = $label OR string::trim(label) = $label) LIMIT 1);
        LET $edge = IF array::len($existing) > 0 { $existing } ELSE {
            (RELATE $in->relates_to->$out SET label = $label, owner = $owner RETURN AFTER)
        };
        LET $memory = IF array::len((SELECT VALUE id FROM $reason_id)) > 0 { [] } ELSE {
            (CREATE $reason_id SET owner = $owner, vault = $vault, subject = $in, text = $text, type = "world" RETURN AFTER)
        };
        IF array::len($memory) > 0 {
            IF array::len((SELECT VALUE id FROM $obs_id)) > 0 {
                UPDATE $obs_id SET status = "stale", version = version + 1
            } ELSE {
                UPDATE memory SET status = "stale", version = version + 1 WHERE subject = $in AND type = "observation"
            }
        };
        RETURN { relation: $edge[0], memory: $memory[0] ?? NONE, created: array::len($existing) = 0 };
        COMMIT TRANSACTION;"#,
    5,
);

pub const RELATE: Stmt =
    Stmt::new("entities.relate", "RELATE $in->relates_to->$out SET label = $label, owner = $owner, source = $source");

pub const FIND_RELATION: Stmt = Stmt::new(
    "entities.find_relation",
    // `string::trim`: an edge stored before labels were trimmed is still the same relation
    "SELECT * FROM relates_to WHERE in = $in AND out = $out AND (label = $label OR string::trim(label) = $label) LIMIT 1",
);

pub const DELETE_RECORD: Stmt = Stmt::new("entities.delete_record", "DELETE $id");

// Edit a memory and, for a raw fact, make its subject's observation stale -- one transaction, like
// WRITE_FACT and DELETE_MEMORY. Advancing the observation version also rejects an in-flight
// rebuild from before this edit, even if the observation was already stale.
//
// An edit keeps the fact's status: a replaced fact stays replaced, with its `superseded_by` (bringing
// one back is an explicit action, not a side effect of fixing a typo). Only an edit that changes the
// text of a current raw fact is being told it again: `asserted_at` moves to now (computed before the
// text is assigned) and the caller queues a supersede check. A type-only edit, an edit of a replaced
// fact, or one that leaves the text as it was moves nothing. See docs/concepts.md.
pub const UPDATE_MEMORY_TEXT: Stmt = Stmt::at(
    "entities.update_memory_text",
    r#"BEGIN TRANSACTION;
        UPDATE $id SET asserted_at = IF type != "observation" AND status != "superseded" AND text != $text THEN time::now() ELSE asserted_at END,
            version = version + 1, updated_at = time::now(), text = $text RETURN AFTER;
        IF $raw { UPDATE memory SET status = "stale", version = version + 1 WHERE subject = $subject AND type = "observation" };
        COMMIT TRANSACTION;"#,
    1, // BEGIN 0, UPDATE 1
);
pub const UPDATE_MEMORY_TYPE: Stmt = Stmt::at(
    "entities.update_memory_type",
    r#"BEGIN TRANSACTION;
        UPDATE $id SET version = version + 1, updated_at = time::now(), type = $type RETURN AFTER;
        IF $raw { UPDATE memory SET status = "stale", version = version + 1 WHERE subject = $subject AND type = "observation" };
        COMMIT TRANSACTION;"#,
    1, // BEGIN 0, UPDATE 1
);
pub const UPDATE_MEMORY_TEXT_TYPE: Stmt = Stmt::at(
    "entities.update_memory_text_type",
    r#"BEGIN TRANSACTION;
        UPDATE $id SET asserted_at = IF $type != "observation" AND status != "superseded" AND text != $text THEN time::now() ELSE asserted_at END,
            version = version + 1, updated_at = time::now(), text = $text, type = $type RETURN AFTER;
        IF $raw { UPDATE memory SET status = "stale", version = version + 1 WHERE subject = $subject AND type = "observation" };
        COMMIT TRANSACTION;"#,
    1, // BEGIN 0, UPDATE 1
);


/// Deleting a memory. A deleted raw fact leaves its subject's observation's lineage and makes the
/// observation stale (rebuilt from the surviving facts on the next consolidation); an observation
/// built from nothing but this fact goes with it. One transaction, so a failure leaves all of it.
/// Deletes a memory. The facts it replaced directly (`superseded_by`) become current again and are
/// returned, for the caller to queue a supersede check on each (a fact further down a chain stays
/// replaced by the one now restored).
pub const DELETE_MEMORY: Stmt = Stmt::at(
    "entities.delete_memory",
    r#"BEGIN TRANSACTION;
        DELETE $id;
        LET $restored = (UPDATE memory SET status = NONE, superseded_by = NONE WHERE superseded_by = $id RETURN subject, id);
        IF $raw {
            DELETE memory WHERE subject = $subject AND type = "observation" AND source_memories = [$id];
            UPDATE memory SET status = "stale", version = version + 1, updated_at = time::now(),
                source_memories = IF source_memories THEN array::complement(source_memories, [$id]) ELSE NONE END
                WHERE subject = $subject AND type = "observation";
        };
        RETURN $restored;
        COMMIT TRANSACTION;"#,
    4, // BEGIN 0, DELETE 1, LET 2, IF 3, RETURN 4
);

/// The memory half of a merge, for a caller's transaction: observations folded into the oldest one
/// (texts joined, lineages unioned, marked stale so the next consolidation rebuilds it from the merged
/// facts), then every memory of `$loser` moved to `$winner`, and the winner's observation marked stale.
macro_rules! merge_memories {
    () => {
        r#"
    LET $obs = (SELECT id, text, source_memories, created_at FROM memory
        WHERE subject IN [$winner, $loser] AND type = "observation" ORDER BY created_at, id);
    IF array::len($obs) > 1 {
        DELETE array::slice($obs.id, 1);
        UPDATE $obs[0].id SET subject = $winner, text = array::join($obs.text, "\n\n"), version += 1,
            source_memories = array::distinct(array::flatten($obs.map(|$o| $o.source_memories ?? []))),
            status = "stale", updated_at = time::now();
    };
    UPDATE memory SET subject = $winner WHERE subject = $loser;
    UPDATE memory SET status = "stale", updated_at = time::now() WHERE subject = $winner AND type = "observation";
"#
    };
}

pub const MERGE_MEMORIES_SQL: &str = merge_memories!();

/// Moves everything of `$loser` onto `$winner` (compatible kinds and same vault, checked) and
/// deletes `$loser`, in ONE transaction: memories (see [`MERGE_MEMORIES_SQL`]), `relates_to` edges in
/// both directions (re-created on the winner unless it already has that exact edge; edges between
/// the two are dropped), and `$aliases` onto the winner. Any failing statement rolls all of it back.
pub const MERGE_ENTITIES: Stmt = Stmt::new(
    "entities.merge_entities",
    concat!(
        "BEGIN TRANSACTION;",
        merge_memories!(),
        r#"
    FOR $e IN (SELECT * FROM relates_to WHERE in = $loser AND out NOT IN [$winner, $loser]) {
        IF array::len(SELECT id FROM relates_to WHERE in = $winner AND out = $e.out AND label = $e.label) = 0 {
            LET $o = $e.out;
            RELATE $winner->relates_to->$o SET label = $e.label, owner = $e.owner, source = $e.source, created_at = $e.created_at;
        };
    };
    FOR $e IN (SELECT * FROM relates_to WHERE out = $loser AND in NOT IN [$winner, $loser]) {
        IF array::len(SELECT id FROM relates_to WHERE in = $e.in AND out = $winner AND label = $e.label) = 0 {
            LET $i = $e.in;
            RELATE $i->relates_to->$winner SET label = $e.label, owner = $e.owner, source = $e.source, created_at = $e.created_at;
        };
    };
    DELETE relates_to WHERE in = $loser OR out = $loser;
    UPDATE $winner SET aliases = array::sort(array::union(aliases, $aliases)), updated_at = time::now();
    FOR $v IN (SELECT * FROM entity_property WHERE entity = $loser) {
        IF $target_kind != NONE {
            LET $p = (SELECT * FROM $v.property)[0];
            LET $targets = (SELECT id FROM ontology_property WHERE vault = $p.vault AND kind = $target_kind
                AND name = $p.name AND status = "approved" AND datatype = $p.datatype
                AND (unit ?? NONE) = ($p.unit ?? NONE)
                AND ($v.unit = NONE OR unit = $v.unit)
                AND (datatype != "enum" OR $v.value_text IN enum_values));
            IF array::len($targets) != 1 { THROW "unmappable property during entity merge"; };
            UPDATE $v.id SET entity = $winner, property = $targets[0].id;
        } ELSE {
            UPDATE $v.id SET entity = $winner;
        };
    };
    DELETE $loser;
"#,
        "COMMIT TRANSACTION;"
    ),
);

pub const DELETE_ENTITY: Stmt = Stmt::new(
    "entities.delete_entity",
    "BEGIN TRANSACTION; DELETE memory WHERE subject = $id; \
     DELETE relates_to WHERE in = $id OR out = $id; DELETE $id; COMMIT TRANSACTION;",
);


// the other endpoint must be in the entity's own vault: an edge left over from before relations were confined to one vault stays hidden
pub const EDGES_OUT_IN_VAULT: Stmt =
    Stmt::new("entities.edges_out_in_vault", "SELECT * FROM relates_to WHERE in = $id AND out.vault = $vault");
pub const EDGES_IN_IN_VAULT: Stmt =
    Stmt::new("entities.edges_in_in_vault", "SELECT * FROM relates_to WHERE out = $id AND in.vault = $vault");


pub const MEMORIES_OF: Stmt =
    Stmt::new("entities.memories_of", "SELECT * FROM memory WHERE subject = $id AND vault = $vault ORDER BY created_at DESC");

pub const EDGES_AMONG: Stmt = Stmt::new(
    "entities.edges_among",
    "SELECT * FROM relates_to WHERE $ids CONTAINS in AND $ids CONTAINS out",
);

// entities/neighbourhood.rs and views/ (`entities_neighbourhood`, `show_view`)
/// Up to `$limit` relations from each of `$ids`, and up to `$limit` into each, with both ends' vaults:
/// one list per id and direction. Each id's relations come through the `in` and `out` indexes, all in one
/// round trip.
// The caller filters on the vaults: `out = $id AND in.vault = $vault` inside the closure matched nothing
// on SurrealDB 3.3 (outside one it works).
pub const EDGES_TOUCHING: Stmt = Stmt::at(
    "entities.edges_touching",
    r#"LET $outs = $ids.map(|$id| (SELECT id, in, out, label, in.vault AS from_vault, out.vault AS to_vault FROM relates_to WHERE in = $id LIMIT $limit));
       LET $ins = $ids.map(|$id| (SELECT id, in, out, label, in.vault AS from_vault, out.vault AS to_vault FROM relates_to WHERE out = $id LIMIT $limit));
       RETURN array::concat($outs, $ins);"#,
    2, // two LETs, then the RETURN
);
/// One indexed direction at a time, so sparse lists leave the budget for busier nodes.
pub const EDGES_IN_DIRECTION: Stmt = Stmt::new(
    "entities.edges_in_direction",
    r#"RETURN IF $outgoing {
        (SELECT id, in, out, label, in.vault AS from_vault, out.vault AS to_vault FROM relates_to WHERE in = $id LIMIT $limit)
    } ELSE {
        (SELECT id, in, out, label, in.vault AS from_vault, out.vault AS to_vault FROM relates_to WHERE out = $id LIMIT $limit)
    };"#,
);
/// Diagram search reads one indexed direction at a time, with a small caller row budget.
pub const DIAGRAM_EDGES: Stmt = Stmt::new(
    "entities.diagram_edges",
    r#"RETURN IF $outgoing {
        (SELECT id, in, out, label, in.vault AS from_vault, out.vault AS to_vault FROM relates_to WHERE in = $id LIMIT $limit START $start TIMEOUT 1s)
    } ELSE {
        (SELECT id, in, out, label, in.vault AS from_vault, out.vault AS to_vault FROM relates_to WHERE out = $id LIMIT $limit START $start TIMEOUT 1s)
    };"#,
);
pub const DIAGRAM_NODES: Stmt = Stmt::new(
    "entities.diagram_nodes", "SELECT id, vault, kind, name FROM $ids LIMIT $limit TIMEOUT 1s",
);
/// Whether `$id` has any relation, either way, to an entity in its own vault `$vault`.
pub const HAS_RELATION: Stmt = Stmt::at(
    "entities.has_relation",
    r#"LET $outs = (SELECT VALUE id FROM relates_to WHERE in = $id AND out.vault = $vault LIMIT 1);
       LET $ins = (SELECT VALUE id FROM relates_to WHERE out = $id AND in.vault = $vault LIMIT 1);
       RETURN array::len($outs) + array::len($ins) > 0;"#,
    2, // two LETs, then the RETURN
);
/// Entities by id, in any vault: the caller checks each one's vault.
pub const NODES_BY_ID: Stmt = Stmt::new("entities.nodes_by_id", "SELECT id, vault, kind, name, summary FROM $ids");
/// Relations by id, in any vault: the caller checks both ends.
pub const RELATIONS_BY_ID: Stmt = Stmt::new("entities.relations_by_id", "SELECT id, in, out, label FROM $ids");
/// The texts of up to three most recent live raw facts of each of `$ids` in `$vault`, in `$ids` order.
pub const RECENT_FACTS: Stmt = Stmt::new(
    "entities.recent_facts",
    r#"RETURN $ids.map(|$id| (SELECT text, created_at FROM memory WHERE subject = $id AND vault = $vault
       AND type IN ["world","experience"] AND status != "superseded" ORDER BY created_at DESC LIMIT 3).text)"#,
);

pub const SET_SUMMARY: Stmt = Stmt::new(
    "entities.set_summary",
    "UPDATE $id SET summary = $summary, updated_at = time::now() RETURN AFTER",
);

// consolidate.rs
// Supersession places a fact by when we were last told it (`asserted_at`), else its last write
// (facts from before 0046); never by when it happened (`occurred_at`, shown to the model only).
// `written` (last write) breaks ties. See docs/concepts.md, "A fact's two dates".
/// The facts being checked (the new ones).
pub const FACTS_BY_ID: Stmt = Stmt::new(
    "entities.facts_by_id",
    r#"SELECT id, text, asserted_at ?? updated_at ?? created_at AS at, updated_at ?? created_at AS written, occurred_at, asserted_at AS told, source, version FROM $ids
       WHERE type IN ["world","experience"] AND status != "superseded""#,
);
/// The subject's live raw facts with no vector for `$spec` yet (their embed job has not run), newest first.
/// The supersede check embeds these itself, or a contradiction written right after a fact would miss it.
pub const UNEMBEDDED_LIVE_FACTS: Stmt = Stmt::new(
    "entities.unembedded_live_facts",
    r#"SELECT id, text, asserted_at ?? updated_at ?? created_at AS at, updated_at ?? created_at AS written, occurred_at, asserted_at AS told, source, version FROM memory
       WHERE subject = $subject AND id NOTINSIDE $new AND type IN ["world","experience"]
       AND status != "superseded" AND (embedding_spec = NONE OR embedding_spec != $spec)
       ORDER BY at DESC LIMIT $limit"#,
);
/// The subject's live raw facts most similar to one new fact's `$vector` (same embedding `$spec`), best first.
/// Only these can be superseded by it: a fact about something else can't be made untrue by it.
pub const SIMILAR_LIVE_FACTS: Stmt = Stmt::new(
    "entities.similar_live_facts",
    r#"SELECT id, text, asserted_at ?? updated_at ?? created_at AS at, updated_at ?? created_at AS written, occurred_at, asserted_at AS told, source, version, vector::similarity::cosine(embedding, $vector) AS sim
       FROM memory WHERE subject = $subject AND id != $new AND type IN ["world","experience"]
       AND status != "superseded" AND embedding_spec = $spec ORDER BY sim DESC LIMIT $limit"#,
);
/// Marks `$fact` superseded by `$by`, only if neither changed since the check read them (same versions,
/// both still live): a check that started before an edit must not hide it (the edit's own check
/// decides). Makes the subject's observation stale when it marks. Returns the fact when marked.
pub const MARK_SUPERSEDED: Stmt = Stmt::at(
    "entities.mark_superseded",
    r#"BEGIN TRANSACTION;
        LET $m = (UPDATE $fact SET status = "superseded", superseded_by = $by
                  WHERE version = $fact_version AND status != "superseded"
                  AND $by.version = $by_version AND $by.status != "superseded" RETURN VALUE id);
        IF array::len($m) > 0 { UPDATE memory SET status = "stale" WHERE subject = $subject AND type = "observation" };
        RETURN $m;
        COMMIT TRANSACTION;"#,
    3, // BEGIN 0, LET 1, IF 2, RETURN 3
);
/// A subject's raw facts from one source record (`*`, with the vault's name): the extraction path
/// compares their normalised texts so a re-synced record does not tell the same fact again.
pub const SAME_SOURCE_FACTS: Stmt = Stmt::new(
    "entities.same_source_facts",
    r#"SELECT *, vault.name AS vault_name FROM memory WHERE source = $source AND subject = $subject AND type IN ["world","experience"]"#,
);
pub const RAW_MEMORIES: Stmt = Stmt::new(
    "entities.raw_memories",
    r#"SELECT * FROM memory WHERE subject = $id AND type IN ["world","experience"] AND status != "superseded" ORDER BY created_at"#,
);
pub const OBSERVATION_OF: Stmt = Stmt::new(
    "entities.observation_of",
    r#"SELECT * FROM memory WHERE subject = $id AND type = "observation" LIMIT 1"#,
);
pub const CONSOLIDATE_CREATE: Stmt = Stmt::at(
    "entities.consolidate_create",
    r#"BEGIN TRANSACTION;
            LET $cur = (SELECT VALUE id FROM memory WHERE subject = $subject AND type = "observation" LIMIT 1);
            LET $row = IF array::len($cur) > 0 { [] } ELSE {
                (CREATE $obs_id SET owner = $owner, vault = $vault, subject = $subject, text = $text,
                    type = "observation", version = 1, proof_count = $proof_count, status = "fresh",
                    source_memories = $source_memories, updated_at = time::now() RETURN AFTER)
            };
            RETURN $row;
            COMMIT TRANSACTION;"#,
    3, // BEGIN 0, two LETs 1 and 2, RETURN 3
);
pub const CONSOLIDATE_UPDATE: Stmt = Stmt::new(
    "entities.consolidate_update",
    "UPDATE $id SET text = $text, version = version + 1, proof_count = $proof_count, \
             status = \"fresh\", source_memories = $source_memories, updated_at = time::now() \
             WHERE version = $expected_version RETURN AFTER",
);

/// Whether `$kind` is already in `$vault` (slot 1), else every custom kind in it (slot 2). The kinds
/// are read only for a new kind: that read visits every entity of the vault, and a write under a kind
/// already there is the common case.
pub const CUSTOM_KIND_ROOM: Stmt = Stmt::new(
    "entities.custom_kind_room",
    "LET $known = (SELECT VALUE id FROM entity WHERE vault = $vault AND kind = $kind LIMIT 1); \
     RETURN $known; \
     RETURN IF array::len($known) > 0 { [] } ELSE { (SELECT kind FROM entity WHERE vault = $vault GROUP BY kind) };",
);

/// Every custom kind in `$vaults`, with how many entities it has there.
pub const CUSTOM_KINDS_IN: Stmt = Stmt::new(
    "entities.custom_kinds_in",
    "SELECT kind, vault, count() AS count FROM entity WHERE vault IN $vaults GROUP BY kind, vault ORDER BY kind",
);

// extract.rs
pub const UPSERT_APP_SETTINGS: Stmt =
    Stmt::new("entities.upsert_app_settings", "UPSERT $id SET owner = $owner RETURN AFTER");

pub const ALL: &[&Stmt] = &[
    &MERGE_ALIASES,
    &WRITE_OBSERVATION,
    &WRITE_FACT,
    &RELATE_RETURNING,
    &RELATE_WITH_REASON,
    &RELATE,
    &FIND_RELATION,
    &DELETE_RECORD,
    &DELETE_MEMORY,
    &MERGE_ENTITIES,
    &UPDATE_MEMORY_TEXT,
    &UPDATE_MEMORY_TYPE,
    &UPDATE_MEMORY_TEXT_TYPE,
    &DELETE_ENTITY,
    &EDGES_OUT_IN_VAULT,
    &EDGES_IN_IN_VAULT,
    &MEMORIES_OF,
    &EDGES_AMONG,
    &EDGES_TOUCHING,
    &EDGES_IN_DIRECTION,
    &DIAGRAM_EDGES,
    &DIAGRAM_NODES,
    &HAS_RELATION,
    &NODES_BY_ID,
    &RELATIONS_BY_ID,
    &RECENT_FACTS,
    &SET_SUMMARY,
    &RAW_MEMORIES,
    &FACTS_BY_ID,
    &UNEMBEDDED_LIVE_FACTS,
    &SIMILAR_LIVE_FACTS,
    &MARK_SUPERSEDED,
    &SAME_SOURCE_FACTS,
    &OBSERVATION_OF,
    &CONSOLIDATE_CREATE,
    &CONSOLIDATE_UPDATE,
    &UPSERT_APP_SETTINGS,
    &CUSTOM_KIND_ROOM,
    &CUSTOM_KINDS_IN,
];

// The helpers below build SQL at runtime, so every_query_executes cannot
// cover them. `table` is always a kind_table()-validated entity table name
// (table names cannot be bound parameters); `set` is a join of fixed
// "field = $field" fragments chosen by the caller. A custom kind is never
// spliced: the `entity` table is filtered by a bound `$kind`.

/// ` AND kind = $kind` for the custom-kind table, which holds every custom kind; nothing for a built-in
/// kind's own table.
fn kind_clause(table: &str) -> &'static str {
    if table == "entity" { " AND kind = $kind" } else { "" }
}

/// `SELECT * FROM {table} WHERE vault = $vault` (and `kind = $kind` when `by_kind`), optionally ordered by name.
pub fn select_by_vault<'a>(db: &'a OrgDb, table: &str, ordered: bool, by_kind: bool) -> Q<'a> {
    let k = if by_kind { kind_clause(table) } else { "" };
    if ordered {
        dynamic(db, "entities.list_by_vault", format!("SELECT * FROM {table} WHERE vault = $vault{k} ORDER BY name"))
    } else {
        dynamic(db, "entities.select_by_vault", format!("SELECT * FROM {table} WHERE vault = $vault{k}"))
    }
}

pub fn create_entity<'a>(db: &'a OrgDb, table: &str) -> Q<'a> {
    let k = if table == "entity" { ", kind = $kind" } else { "" };
    dynamic(
        db,
        "entities.create_entity",
        format!("CREATE {table} SET owner = $owner, vault = $vault{k}, name = $name, aliases = $aliases RETURN AFTER"),
    )
}

pub const EDIT_VISIBLE_ALIASES: &str = "aliases = array::union($aliases, aliases.filter(|$a| string::starts_with($a, $merge_prefix)))";

/// Entity edit where only the passed fields change (7 combinations).
pub fn update_entity<'a>(db: &'a OrgDb, set: &str) -> Q<'a> {
    dynamic(db, "entities.update_entity", format!("UPDATE $id SET {set} RETURN AFTER"))
}

/// The `table` entity in `vault` whose lowercase name (slot 1) or else an alias (slot 2) is `$needle`.
/// The name is an index read. On SurrealDB 3.3.5 the alias lookup scans the table (the planner never
/// uses the array index, hint or not), so it runs only when the name matched nothing and the caller
/// did not set `$no_alias` (it knows no entity has `$needle` as an alias). `table` is always a
/// `kind_table()`-validated entity table.
pub fn find_by_key<'a>(db: &'a OrgDb, table: &str) -> Q<'a> {
    let k = kind_clause(table);
    dynamic(
        db,
        "entities.find_by_key",
        format!(
            "LET $by_name = (SELECT * FROM {table} WHERE vault = $vault{k} AND name_key = $needle LIMIT 1); \
             RETURN $by_name; \
             RETURN IF array::len($by_name) > 0 OR $no_alias = true {{ [] }} ELSE {{ \
                (SELECT * FROM {table} WITH INDEX {table}_alias_keys_idx WHERE alias_keys CONTAINS $needle AND vault = $vault{k} \
                ORDER BY created_at LIMIT 1) }};"
        ),
    )
}

/// [`find_by_key`] for `$needle` (slots 1 and 2), then the same name and alias lookups for the other
/// lowercase keys in `$needles` (slots 3 and 4), all in one round trip.
pub fn find_by_key_or_variants<'a>(db: &'a OrgDb, table: &str) -> Q<'a> {
    let k = kind_clause(table);
    dynamic(
        db,
        "entities.find_by_key_or_variants",
        format!(
            "LET $by_name = (SELECT * FROM {table} WHERE vault = $vault{k} AND name_key = $needle LIMIT 1); \
             RETURN $by_name; \
             RETURN IF array::len($by_name) > 0 OR $no_alias = true {{ [] }} ELSE {{ \
                (SELECT * FROM {table} WITH INDEX {table}_alias_keys_idx WHERE alias_keys CONTAINS $needle AND vault = $vault{k} \
                ORDER BY created_at LIMIT 1) }}; \
             SELECT * FROM {table} WHERE vault = $vault{k} AND name_key IN $needles ORDER BY created_at LIMIT 1; \
             SELECT * FROM {table} WITH INDEX {table}_alias_keys_idx WHERE alias_keys CONTAINSANY $needles AND vault = $vault{k} \
             ORDER BY created_at LIMIT 1;"
        ),
    )
}

/// Every alias key of the `table` entities in `$vault`: an import reads them once instead of looking
/// each new name up as an alias (a scan, see [`find_by_key`]).
pub fn alias_keys_of<'a>(db: &'a OrgDb, table: &str) -> Q<'a> {
    let k = kind_clause(table);
    dynamic(db, "entities.alias_keys_of", format!("SELECT VALUE alias_keys FROM {table} WHERE vault = $vault{k} AND array::len(alias_keys) > 0"))
}

/// Other `table` entities in `$vault` (not `$id`) whose name or an alias shares one of `$words`.
pub fn sharing_a_word<'a>(db: &'a OrgDb, table: &str) -> Q<'a> {
    dynamic(
        db,
        "entities.sharing_a_word",
        format!(
            "SELECT * FROM {table} WHERE vault = $vault{} AND id != $id \
             AND (string::words(name_key) CONTAINSANY $words OR alias_keys CONTAINSANY $words) LIMIT 5",
            kind_clause(table)
        ),
    )
}

/// `entities_search` in one round trip: the `kinds` entities in `$vaults` whose lowercase name or an alias
/// contains `$needle`, then (`with_facts`) the ones a fact in the searched vaults mentions (full text, at
/// most 200 facts per vault), each group by vault order, kind order and name, sliced to `$start..$end`
/// with the total, so only the page leaves the database. One read of each kind's rows serves both groups
/// (a fact's vault is its subject's). Every `kinds` entry is a `kind_table()`-validated table.
// ponytail: no index serves a substring match, so the database still reads every entity of the searched
// kinds in the searched vaults (an index range per vault, `{kind}_vault_name_idx`); only the page is sent.
pub fn search_entities<'a>(db: &'a OrgDb, kinds: &[&str], by_kind: bool, with_facts: bool) -> Q<'a> {
    const MATCH: &str = "(string::contains(name_key, $needle) OR array::any(alias_keys, |$a| string::contains($a, $needle)))";
    let mentioned = if with_facts {
        "array::flatten($vaults.map(|$v| (SELECT VALUE subject FROM memory WHERE vault = $v AND text @@ $q LIMIT 200)))"
    } else {
        "[]"
    };
    let rows: Vec<String> = kinds
        .iter()
        .map(|k| {
            let kc = if by_kind { kind_clause(k) } else { "" };
            format!("(SELECT id, vault, kind, name, aliases, summary, {MATCH} AS hit FROM {k} WHERE vault IN $vaults{kc} AND ({MATCH} OR id IN $m) ORDER BY name)")
        })
        .collect();
    dynamic(
        db,
        "entities.search",
        format!(
            "LET $m = {mentioned}; \
             LET $rows = array::flatten([{}]); \
             LET $all = array::concat(array::flatten($vaults.map(|$v| $rows[WHERE vault = $v AND hit])), \
                 array::flatten($vaults.map(|$v| $rows[WHERE vault = $v AND !hit]))); \
             RETURN {{ total: array::len($all), results: array::slice($all, $start, $end) }};",
            rows.join(", ")
        ),
    )
}

fn name_lookup_select(table: &str, prefix: bool) -> String {
    if prefix {
        // 3.3.5 leaves a compound range's upper bound as a residual filter. Limit the ordered
        // lower-bound index read first, so a missing prefix cannot scan the rest of the vault.
        format!("SELECT vault, name FROM (SELECT vault, name, name_key FROM {table} \
                 WHERE vault = $vault AND name_key >= $lower ORDER BY name_key LIMIT 8) WHERE name_key < $upper")
    } else {
        format!("SELECT vault, name FROM {table} WHERE vault = $vault AND name_key = $key LIMIT 8")
    }
}

/// Separate exact and word-prefix index reads, limited per table and vault so Personal cannot crowd
/// out another space. Scalar vault equality lets the compound name_key index use its trailing range.
pub fn names_by_prefix<'a>(db: &'a OrgDb) -> Q<'a> {
    let selects: Vec<String> = crate::entities::service::ENTITY_TABLES.iter()
        // Custom entities only gain a cross-kind name index in migration 0039.
        .filter(|table| **table != "entity" || db.schema_version() >= 39)
        .flat_map(|table| [false, true].map(|prefix| format!("({})", name_lookup_select(table, prefix))))
        .collect();
    dynamic(db, "entities.names_by_prefix", format!(
        "RETURN array::flatten($vaults.map(|$vault| array::concat({})))", selects.join(", ")
    ))
}

#[cfg(feature = "test-support")]
pub fn explain_name_lookup<'a>(db: &'a OrgDb, table: &str, prefix: bool) -> Q<'a> {
    assert!(crate::entities::service::ENTITY_TABLES.contains(&table));
    dynamic(db, "entities.explain_name_lookup", format!("{} EXPLAIN", name_lookup_select(table, prefix)))
}

/// Deterministic rows for the isolated view regression fixtures. The caller must prove mem:// first.
#[cfg(feature = "test-support")]
pub async fn seed_view_fixture(db: &OrgDb, owner: &surrealdb::types::RecordId, vault: &surrealdb::types::RecordId, count: usize) {
    let ids: Vec<_> = (0..count).map(|i| surrealdb::types::RecordId::new("entity", format!("fixture{i}"))).collect();
    db.test_raw().query(r#"
        FOR $id IN $ids {
            CREATE $id SET owner = $owner, vault = $vault, kind = 'sample', name = 'Item ' + <string>$id;
        };
    "#).bind(("owner", owner.clone())).bind(("vault", vault.clone()))
        .bind(("ids", ids)).await.unwrap().check().unwrap();
}

#[cfg(feature = "test-support")]
pub async fn seed_dense_view_links(db: &OrgDb, start: &surrealdb::types::RecordId, count: usize) {
    let ids: Vec<_> = (0..count).map(|i| surrealdb::types::RecordId::new("entity", format!("fixture{i}"))).collect();
    let mut pairs = Vec::new();
    for i in 0..count {
        pairs.push(vec![start.clone(), ids[i].clone()]);
        for j in 1..22 { pairs.push(vec![ids[i].clone(), ids[(i + j) % count].clone()]); }
    }
    db.test_raw().query(r#"
        FOR $ends IN $pairs {
            LET $from = $ends[0];
            LET $to = $ends[1];
            RELATE $from->relates_to->$to SET label = 'touches';
        };
    "#).bind(("pairs", pairs)).await.unwrap().check().unwrap();
}

#[cfg(feature = "test-support")]
pub async fn seed_view_item(db: &OrgDb, owner: &surrealdb::types::RecordId, vault: &surrealdb::types::RecordId, id: &surrealdb::types::RecordId) {
    db.test_raw().query("CREATE $id SET owner = $owner, vault = $vault, kind = 'sample', name = 'AAA inserted'")
        .bind(("id", id.clone())).bind(("owner", owner.clone())).bind(("vault", vault.clone())).await.unwrap().check().unwrap();
}

#[cfg(feature = "test-support")]
pub async fn seed_view_values(db: &OrgDb, vault: &surrealdb::types::RecordId) {
    db.test_raw().query(r#"
        LET $property = (SELECT VALUE id FROM ontology_property WHERE vault = $vault AND name = 'score' LIMIT 1)[0];
        FOR $e IN (SELECT VALUE id FROM entity WHERE vault = $vault AND kind = 'sample') {
            CREATE entity_property SET vault = $vault, entity = $e, property = $property, value_num = 1,
                as_of = d'2026-01-01T00:00:00.987654321Z', origin = 'agent';
        };
    "#).bind(("vault", vault.clone())).await.unwrap().check().unwrap();
}
