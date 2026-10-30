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
            (UPDATE $target[0] SET text = $text, version = version + 1, status = "fresh", updated_at = time::now() RETURN AFTER)
        } ELSE {
            (CREATE $obs_id SET owner = $owner, vault = $vault, subject = $subject, text = $text, type = "observation", status = "fresh", source = $source RETURN AFTER)
        };
        RETURN $row;
        COMMIT TRANSACTION;"#,
    5, // BEGIN 0, four LETs 1 to 4, RETURN 5
);

pub const WRITE_FACT: Stmt = Stmt::at(
    "entities.write_fact",
    r#"BEGIN TRANSACTION;
        CREATE memory SET owner = $owner, vault = $vault, subject = $subject, text = $text, type = $type, source = $source RETURN AFTER;
        LET $cur = (SELECT VALUE id FROM $obs_id);
        IF array::len($cur) > 0 {
            UPDATE $obs_id SET status = "stale" WHERE status != "stale"
        } ELSE {
            UPDATE memory SET status = "stale" WHERE subject = $subject AND type = "observation" AND status != "stale"
        };
        COMMIT TRANSACTION;"#,
    1, // BEGIN 0, CREATE 1
);

pub const RELATE_RETURNING: Stmt = Stmt::new(
    "entities.relate_returning",
    "RELATE $in->relates_to->$out SET label = $label, owner = $owner, source = $source RETURN AFTER",
);

pub const RELATE: Stmt =
    Stmt::new("entities.relate", "RELATE $in->relates_to->$out SET label = $label, owner = $owner, source = $source");

pub const FIND_RELATION: Stmt = Stmt::new(
    "entities.find_relation",
    "SELECT * FROM relates_to WHERE in = $in AND out = $out AND label = $label LIMIT 1",
);

pub const DELETE_RECORD: Stmt = Stmt::new("entities.delete_record", "DELETE $id");

// Edit a memory and, for a raw fact, make its subject's observation stale -- one transaction, like
// WRITE_FACT and DELETE_MEMORY, so an edit never leaves an observation summarising the old text as
// fresh. An edited raw fact is current again (no longer superseded); an observation keeps its status.
pub const UPDATE_MEMORY_TEXT: Stmt = Stmt::at(
    "entities.update_memory_text",
    r#"BEGIN TRANSACTION;
        UPDATE $id SET version = version + 1, updated_at = time::now(), text = $text, status = IF type = "observation" THEN status ELSE NONE END RETURN AFTER;
        IF $raw { UPDATE memory SET status = "stale" WHERE subject = $subject AND type = "observation" };
        COMMIT TRANSACTION;"#,
    1, // BEGIN 0, UPDATE 1
);
pub const UPDATE_MEMORY_TYPE: Stmt = Stmt::at(
    "entities.update_memory_type",
    r#"BEGIN TRANSACTION;
        UPDATE $id SET version = version + 1, updated_at = time::now(), type = $type RETURN AFTER;
        IF $raw { UPDATE memory SET status = "stale" WHERE subject = $subject AND type = "observation" };
        COMMIT TRANSACTION;"#,
    1, // BEGIN 0, UPDATE 1
);
pub const UPDATE_MEMORY_TEXT_TYPE: Stmt = Stmt::at(
    "entities.update_memory_text_type",
    r#"BEGIN TRANSACTION;
        UPDATE $id SET version = version + 1, updated_at = time::now(), text = $text, type = $type, status = IF $type = "observation" THEN status ELSE NONE END RETURN AFTER;
        IF $raw { UPDATE memory SET status = "stale" WHERE subject = $subject AND type = "observation" };
        COMMIT TRANSACTION;"#,
    1, // BEGIN 0, UPDATE 1
);


/// Deleting a memory. A deleted raw fact leaves its subject's observation's lineage and makes the
/// observation stale (rebuilt from the surviving facts on the next consolidation); an observation
/// built from nothing but this fact goes with it. One transaction, so a failure leaves all of it.
pub const DELETE_MEMORY: Stmt = Stmt::new(
    "entities.delete_memory",
    r#"BEGIN TRANSACTION;
        DELETE $id;
        IF $raw {
            DELETE memory WHERE subject = $subject AND type = "observation" AND source_memories = [$id];
            UPDATE memory SET status = "stale", updated_at = time::now(),
                source_memories = IF source_memories THEN array::complement(source_memories, [$id]) ELSE NONE END
                WHERE subject = $subject AND type = "observation";
        };
        COMMIT TRANSACTION;"#,
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

/// Moves everything of `$loser` onto `$winner` (same kind and vault, checked by the caller) and
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

pub const SET_SUMMARY: Stmt = Stmt::new(
    "entities.set_summary",
    "UPDATE $id SET summary = $summary, updated_at = time::now() RETURN AFTER",
);

// consolidate.rs
/// The facts being checked (the new ones), dated by their last write.
pub const FACTS_BY_ID: Stmt = Stmt::new(
    "entities.facts_by_id",
    r#"SELECT id, text, updated_at ?? created_at AS at FROM $ids
       WHERE type IN ["world","experience"] AND status != "superseded""#,
);
/// The subject's live raw facts most similar to one new fact (same embedding model), best first.
/// Only these can be superseded by it: a fact about something else can't be made untrue by it.
pub const SIMILAR_LIVE_FACTS: Stmt = Stmt::at(
    "entities.similar_live_facts",
    r#"LET $n = (SELECT embedding, embedding_spec FROM ONLY $new);
       SELECT id, text, updated_at ?? created_at AS at, vector::similarity::cosine(embedding, $n.embedding) AS sim
       FROM memory WHERE subject = $subject AND id != $new AND type IN ["world","experience"]
       AND status != "superseded" AND embedding_spec = $n.embedding_spec ORDER BY sim DESC LIMIT $limit"#,
    1,
);
/// Marks facts superseded and makes the subject's observation stale.
pub const MARK_SUPERSEDED: Stmt = Stmt::new(
    "entities.mark_superseded",
    r#"UPDATE $ids SET status = "superseded";
       UPDATE memory SET status = "stale" WHERE subject = $subject AND type = "observation";"#,
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

/// Whether `$kind` is already in `$vault` (slot 0), else every custom kind in it (slot 1).
pub const CUSTOM_KIND_ROOM: Stmt = Stmt::new(
    "entities.custom_kind_room",
    "SELECT VALUE id FROM entity WHERE vault = $vault AND kind = $kind LIMIT 1; \
     SELECT kind FROM entity WHERE vault = $vault GROUP BY kind;",
);

/// Every custom kind in `$vaults`, with how many entities it has there.
pub const CUSTOM_KINDS_IN: Stmt = Stmt::new(
    "entities.custom_kinds_in",
    "SELECT kind, count() AS count FROM entity WHERE vault IN $vaults GROUP BY kind ORDER BY kind",
);

// extract.rs
pub const UPSERT_APP_SETTINGS: Stmt =
    Stmt::new("entities.upsert_app_settings", "UPSERT $id SET owner = $owner RETURN AFTER");

pub const ALL: &[&Stmt] = &[
    &MERGE_ALIASES,
    &WRITE_OBSERVATION,
    &WRITE_FACT,
    &RELATE_RETURNING,
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
    &SET_SUMMARY,
    &RAW_MEMORIES,
    &FACTS_BY_ID,
    &SIMILAR_LIVE_FACTS,
    &MARK_SUPERSEDED,
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

/// Entity edit where only the passed fields change (7 combinations).
pub fn update_entity<'a>(db: &'a OrgDb, set: &str) -> Q<'a> {
    dynamic(db, "entities.update_entity", format!("UPDATE $id SET {set} RETURN AFTER"))
}

/// The `table` entity in `vault` whose lowercase name or an alias is `$needle`, a name match first;
/// both lookups are index reads. `table` is always a `kind_table()`-validated entity table.
pub fn find_by_key<'a>(db: &'a OrgDb, table: &str) -> Q<'a> {
    let k = kind_clause(table);
    dynamic(
        db,
        "entities.find_by_key",
        format!(
            "SELECT * FROM {table} WHERE vault = $vault{k} AND name_key = $needle LIMIT 1; \
             SELECT * FROM {table} WITH INDEX {table}_alias_keys_idx WHERE alias_keys CONTAINS $needle AND vault = $vault{k} \
             ORDER BY created_at LIMIT 1;"
        ),
    )
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
