//! Statements for the cache area. See the module docs in store/mod.rs.
//!
//! Not here, built at runtime with `store::dynamic` (the variant count
//! explodes, the SQL embeds a literal, or one statement is repeated per term or entity):
//! `cache.keyword_ids` (per term, filtered), `cache.semantic_ids` (KNN `<|k,ef|>` needs literal
//! integers), `cache.semantic_exact`, `cache.list_records`, `cache.graph_entities`,
//! `cache.graph_memories`, `cache.memory_text_terms`.

use super::Stmt;

pub const MEMORY_FOR_EMBED: Stmt = Stmt::new("cache.memory_for_embed", "SELECT id, text, type FROM memory WHERE vault = $vault LIMIT $limit");
/// Records in a scope (`cache::search::RecordScope`), for the vector cloud.
pub const RECORDS_FOR_EMBED: Stmt = Stmt::new(
    "cache.records_for_embed",
    "SELECT id, title, body_text, embedding FROM cache_record \
     WHERE (vault IN $vaults OR ($own AND vault = NONE AND owner = $owner)) AND deleted = false LIMIT $limit",
);

// newest first, limited in the database, so a broad range never loads the whole range
pub const CACHE_RECORDS_IN_RANGE: Stmt = Stmt::new(
    "cache.records_in_range",
    "SELECT id, occurred_at FROM cache_record WHERE (vault IN $vaults OR ($own AND vault = NONE AND owner = $owner)) \
     AND deleted = false AND occurred_at >= $since AND occurred_at <= $until ORDER BY occurred_at DESC LIMIT $limit",
);
/// Dated by when the fact happened (`occurred_at`) if its writer said, else when it was written.
pub const MEMORIES_IN_RANGE: Stmt = Stmt::new(
    "cache.memories_in_range",
    "SELECT id, occurred_at ?? created_at AS at FROM memory WHERE vault = $vault \
     AND (occurred_at ?? created_at) >= $since AND (occurred_at ?? created_at) <= $until ORDER BY at DESC LIMIT $limit",
);

/// A record's sync links, to tell whether a replay changed only them.
pub const SYNC_LINKS_OF: Stmt = Stmt::new("cache.sync_links_of", "SELECT rel, out FROM linked_to WHERE in = $id AND origin = 'sync'");
pub const DELETE_SYNC_LINKS: Stmt = Stmt::new("cache.delete_sync_links", "DELETE linked_to WHERE in = $id AND origin = 'sync'");
pub const RELATE_SYNC_LINK: Stmt = Stmt::new("cache.relate_sync_link", "RELATE $in->linked_to->$out SET rel = $rel, origin = 'sync'");
/// An unchanged replayed record: when it was last seen, plus its vault and route keys (see
/// `cache::search::Placement`).
pub const TOUCH_INGESTED: Stmt =
    Stmt::new("cache.touch_ingested", "UPDATE $id SET ingested_at = time::now(), vault = $vault, route_keys = $route_keys");
pub const UPSERT_RECORD: Stmt = Stmt::new(
    "cache.upsert_record",
    "UPSERT $id SET owner = $owner, source = $source, type = $type, external_id = $external_id, \
     title = $title, body_text = $body_text, occurred_at = $occurred_at, url = $url, \
     payload = $payload, content_hash = $content_hash, ingested_at = $ingested_at, \
     updated_at = $updated_at, deleted = $deleted, vault = $vault, route_keys = $route_keys, connector = $connector RETURN AFTER",
);
/// Whether a stored record already has its embedding (an unchanged replayed record is re-embedded only if not).
pub const HAS_EMBEDDING: Stmt = Stmt::new("cache.has_embedding", "SELECT VALUE embedding != NONE FROM ONLY $id");
pub const SET_EMBEDDING: Stmt = Stmt::new("cache.set_embedding", "UPDATE $id SET embedding = $embedding");

/// The owner's own live records by id, whatever vault they are filed in (work done on the owner's
/// behalf, never a read); a tombstone reads as absent.
pub const RECORDS_BY_IDS: Stmt =
    Stmt::new("cache.records_by_ids", "SELECT * FROM $ids WHERE owner = $owner AND deleted = false");
/// The live records by id that a read's scope sees (`cache::search::RecordScope`).
pub const RECORDS_IN_SCOPE: Stmt = Stmt::new(
    "cache.records_in_scope",
    "SELECT * FROM $ids WHERE (vault IN $vaults OR ($own AND vault = NONE AND owner = $owner)) AND deleted = false",
);

/// A memory page for recall's hydrate: this vault's, and not a stale observation (its facts changed
/// since it was consolidated, so it is not a current fact; the raw facts are still recalled).
pub const MEMORIES_FOR_RECALL: Stmt = Stmt::new(
    "cache.memories_for_recall",
    "SELECT id, text, source, created_at, updated_at, occurred_at, type, status FROM $ids WHERE vault = $vault \
     AND !(type = \"observation\" AND status = \"stale\") AND ($history OR status != \"superseded\")",
);
/// Relations the graph arm surfaced, as names, when both ends are in the vault being searched.
pub const RELATIONS_FOR_RECALL: Stmt = Stmt::new(
    "cache.relations_for_recall",
    "SELECT id, label, in.name AS a, out.name AS b, created_at FROM $ids WHERE in.vault = $vault AND out.vault = $vault",
);
/// When a fact happened, as its writer said (`memory_write`'s `occurred_at`).
pub const SET_MEMORY_OCCURRED_AT: Stmt = Stmt::new("cache.set_memory_occurred_at", "UPDATE $id SET occurred_at = $at");
/// A vector for a memory, unless its text changed since it was read for embedding.
pub const SET_MEMORY_EMBEDDING: Stmt =
    Stmt::new("cache.set_memory_embedding", "UPDATE $id SET embedding = $embedding, embedding_spec = $spec WHERE text = $text");

/// Each source's live linked records in either direction, one traversal for the whole batch (recall's
/// graph arm). Tombstoned sources and targets are skipped.
pub const LIVE_NEIGHBOURS: Stmt = Stmt::new(
    "cache.live_neighbours",
    "SELECT id, ->linked_to->(cache_record WHERE deleted = false) AS fwd, \
     <-linked_to<-(cache_record WHERE deleted = false) AS back FROM $ids WHERE deleted = false",
);

// A tombstoned endpoint hides the link (a not-yet-synced target, like a category placeholder, does not),
// and a tombstoned record has no links at all. The other end must be in the read's scope (or not synced
// yet): a record routed to one vault may link to one its owner kept in another.
pub const LINKS_OUT: Stmt = Stmt::new(
    "cache.links_out",
    "SELECT rel, out FROM linked_to WHERE in = $id AND in.deleted != true AND out.deleted != true \
     AND (out.owner = NONE OR out.vault IN $vaults OR ($own AND out.vault = NONE AND out.owner = $owner))",
);
pub const LINKS_OUT_REL: Stmt = Stmt::new(
    "cache.links_out_rel",
    "SELECT rel, out FROM linked_to WHERE in = $id AND rel = $rel AND in.deleted != true AND out.deleted != true \
     AND (out.owner = NONE OR out.vault IN $vaults OR ($own AND out.vault = NONE AND out.owner = $owner))",
);
pub const LINKS_IN: Stmt = Stmt::new(
    "cache.links_in",
    "SELECT rel, in FROM linked_to WHERE out = $id AND in.deleted != true AND out.deleted != true \
     AND (in.owner = NONE OR in.vault IN $vaults OR ($own AND in.vault = NONE AND in.owner = $owner))",
);
pub const LINKS_IN_REL: Stmt = Stmt::new(
    "cache.links_in_rel",
    "SELECT rel, in FROM linked_to WHERE out = $id AND rel = $rel AND in.deleted != true AND out.deleted != true \
     AND (in.owner = NONE OR in.vault IN $vaults OR ($own AND in.vault = NONE AND in.owner = $owner))",
);

pub const EMBED_CACHE_GET: Stmt = Stmt::new("cache.embed_cache_get", "SELECT text_hmac, vector FROM embed_cache WHERE text_hmac IN $keys");
pub const EMBED_CACHE_PUT: Stmt = Stmt::new("cache.embed_cache_put", "UPSERT $id SET text_hmac = $hmac, vector = $vector");
pub const EMBED_CACHE_DELETE: Stmt = Stmt::new("cache.embed_cache_delete", "DELETE embed_cache WHERE text_hmac IN $keys");

pub const RECORD_AUDIT: Stmt = Stmt::new(
    "cache.record_audit",
    "CREATE audit_log SET owner = $owner, tool_name = $tool_name, \
     args_summary = $args_summary, outcome = $outcome",
);
pub const AUDIT_PAGE: Stmt = Stmt::new(
    "cache.audit_page",
    "SELECT * FROM audit_log WHERE owner = $owner ORDER BY created_at DESC LIMIT $limit START $offset",
);
pub const AUDIT_COUNT: Stmt = Stmt::new("cache.audit_count", "SELECT count() FROM audit_log WHERE owner = $owner GROUP ALL");

/// Deletes everything `$owner` has from one connection of `$source` (`sources::registry::delete_data`):
/// its records are those it synced (`connector = $connector`) plus, for a kind's first connection
/// (`$first`), the ones synced before 2.0.4 (`connector = NONE`). Then the facts drawn from them and
/// (like deleting one memory) each affected subject's observations lose those facts from their lineage
/// and go stale, or are deleted when nothing else backs them; relations extracted from them; links
/// either end of which is one of them; the records themselves; and their heypocket `pocket_recording`
/// rows. Entities, the connector's credentials and its sync cursor stay. One transaction.
pub const DELETE_SOURCE_DATA: Stmt = Stmt::at(
    "cache.delete_source_data",
    r#"BEGIN TRANSACTION;
        LET $records = (SELECT VALUE id FROM cache_record WHERE owner = $owner AND source = $source
            AND (connector = $connector OR ($first AND connector = NONE)));
        LET $mems = (SELECT id, subject FROM memory WHERE source IN $records);
        LET $ids = $mems.id;
        LET $subjects = array::distinct($mems.subject);
        DELETE $ids;
        DELETE memory WHERE type = "observation" AND subject IN $subjects
            AND array::len(source_memories ?? []) > 0 AND array::len(array::complement(source_memories ?? [], $ids)) = 0;
        UPDATE memory SET status = "stale", updated_at = time::now(),
            source_memories = IF source_memories THEN array::complement(source_memories, $ids) ELSE NONE END
            WHERE type = "observation" AND subject IN $subjects;
        DELETE relates_to WHERE source IN $records;
        DELETE linked_to WHERE in IN $records OR out IN $records;
        DELETE pocket_recording WHERE owner = $owner
            AND recording_id IN (SELECT VALUE external_id FROM $records WHERE type = "heypocket.recording");
        DELETE $records;
        RETURN { records: array::len($records), memories: array::len($ids) };
        COMMIT TRANSACTION;"#,
    12, // BEGIN 0, four LETs 1 to 4, six writes 5 to 10, DELETE 11, RETURN 12
);

pub const ALL: &[&Stmt] = &[
    &DELETE_SOURCE_DATA,
    &MEMORY_FOR_EMBED,
    &RECORDS_FOR_EMBED,
    &CACHE_RECORDS_IN_RANGE,
    &MEMORIES_IN_RANGE,
    &SYNC_LINKS_OF,
    &DELETE_SYNC_LINKS,
    &RELATE_SYNC_LINK,
    &TOUCH_INGESTED,
    &UPSERT_RECORD,
    &HAS_EMBEDDING,
    &RECORDS_BY_IDS,
    &RECORDS_IN_SCOPE,
    &MEMORIES_FOR_RECALL,
    &LIVE_NEIGHBOURS,
    &SET_EMBEDDING,
    &SET_MEMORY_EMBEDDING,
    &SET_MEMORY_OCCURRED_AT,
    &RELATIONS_FOR_RECALL,
    &LINKS_OUT,
    &LINKS_OUT_REL,
    &LINKS_IN,
    &LINKS_IN_REL,
    &EMBED_CACHE_GET,
    &EMBED_CACHE_PUT,
    &EMBED_CACHE_DELETE,
    &RECORD_AUDIT,
    &AUDIT_PAGE,
    &AUDIT_COUNT,
];
