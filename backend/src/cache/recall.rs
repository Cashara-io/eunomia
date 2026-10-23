//! Recall pipeline: 6 arms (semantic over records, keyword, graph, temporal, memory text, semantic
//! over memories), fused via Reciprocal Rank Fusion, then boosted by recency + "proof" (how many arms
//! agreed on an item), then truncated to a token budget.
//!
//! The query is embedded once, concurrently with the database arms, and both semantic arms share the
//! vector. A memory is only compared with vectors made by the same provider and model
//! (`memory.embedding_spec`), and a semantic hit must clear [`MIN_SIMILARITY`], so an unrelated
//! nearest neighbour never rides in on rank alone. The graph arm follows the matched entities' own
//! memories and then one bounded `relates_to` hop: the relations themselves (kind `relation`) and the
//! neighbours' newest facts, only where both ends are in the vault being searched.
//!
//! Pipeline, matching the reference diagram:
//!
//! ```text
//!     [semantic | keyword, graph, temporal, memory text, memory semantic]  (query embedded alongside the DB arms)
//!                      |
//!                 RRF fusion (k=60, reusing cache::search's rrf_scores)
//!                      |
//!          (cross-encoder step -- intentionally omitted, see below)
//!                      |
//!                 boosts (recency x proof)
//!                      |
//!               token-budget truncation
//! ```
//!
//! Cross-encoder: the diagram has a cross-encoder re-ranking step between RRF
//! fusion and boosts. This is skipped on purpose, for the same reason: OpenAI has no cross-encoder/rerank
//! API endpoint, and `EMBEDDINGS_BACKEND` is OpenAI-only for the user-facing
//! path. We go straight from RRF fusion to boosts rather than fake a
//! cross-encoder with a cheap heuristic pretending to be one.
//!
//! Natural-language date parsing ("last week", "in March") is also out of
//! scope for v1 -- the temporal arm only activates when an explicit
//! `time_range` is given. A memory is dated by when it happened if its writer said so
//! (`memory.occurred_at`), else by when it was written.
//!
//! The graph arm's entity matching queries the
//! `person`/`organisation`/`location`/`repository`/`file`/`symbol` tables
//! directly rather than calling `entities::service`.
//!

use surrealdb::types::SurrealValue;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use surrealdb::types::{Datetime, RecordId};
use crate::rid::RecordIdExt;

use crate::cache::search as cs;
use crate::config::Settings;
use crate::pool::OrgDb;
use crate::store;
use crate::error::{AppError, AppResult};
use crate::vaults::service as vaults_service;

/// Input bounds: anything outside them is a 400 (see `validate`).
const MAX_LIMIT: usize = 100;
const MAX_TOKENS: usize = 100_000;
const MAX_QUERY_CHARS: usize = 2_000;
/// Each arm's deadline: past it the arm contributes nothing and the others'
/// results are returned (see `arm`).
const ARM_DEADLINE: Duration = Duration::from_secs(4);
/// Graph arm bounds: index hits kept per (kind, name/alias), how many
/// matched entities it follows, and the query phrases it looks names up by
/// (runs of up to `NAME_WORDS` of the first `MAX_NAME_WORDS` words).
const GRAPH_CANDIDATES: usize = 20;
const GRAPH_ENTITIES: usize = 10;
/// The one `relates_to` hop: edges read from the matched entities, neighbours followed, and how many
/// of each neighbour's newest facts come along.
const GRAPH_EDGES: usize = 40;
const GRAPH_NEIGHBOURS: usize = 10;
const NEIGHBOUR_MEMORIES: usize = 5;
/// Cosine similarity a semantic hit needs to count at all. Nearest-neighbour search always returns
/// K rows; without a floor the K-th unrelated fact would still earn a fused score.
const MIN_SIMILARITY: f64 = 0.15;
const NAME_WORDS: usize = 6;
const MAX_NAME_WORDS: usize = 64;

// recency boost floor/ceiling; see `boost` below.
const RECENCY_FLOOR: f64 = 0.5;
const RECENCY_WINDOW_DAYS: f64 = 365.0;
const RECENCY_SPAN: f64 = 0.5;
// proof boost: +5% per extra arm that surfaced the same item.
const PROOF_STEP: f64 = 0.05;

/// `surrealdb::types::Datetime` only converts *from* `chrono::DateTime<Utc>`
/// (`Datetime::from`); the reverse direction isn't exposed on the wrapper
/// type directly, only on the inner core type it wraps, so go through
/// `into_inner()`.
fn to_chrono(dt: surrealdb::types::Datetime) -> DateTime<Utc> {
    dt.into_inner()
}

#[derive(Debug, Clone, Serialize)]
pub struct RecallItem {
    pub id: String,
    pub kind: &'static str,
    pub text: String,
    pub source: Option<String>,
    /// A record's date; for a memory, when it happened if its writer said so, else when it was last
    /// written or edited; for a relation, when it was recorded.
    pub occurred_at: Option<String>,
    /// 0..1, relative to the best hit of this recall.
    pub score: f64,
    pub arms_hit: usize,
    /// The vault the hit came from (its id), and that vault's name.
    pub vault: String,
    pub vault_name: String,
    /// For a chunk of an uploaded document: which document, revision, part and character range, and
    /// where to download the original (`documents::chunk_ref`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document: Option<serde_json::Value>,
    /// `superseded` for a fact a later one replaced; only returned when history was asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryType {
    World,
    Experience,
    Observation,
}

impl MemoryType {
    fn as_str(&self) -> &'static str {
        match self {
            MemoryType::World => "world",
            MemoryType::Experience => "experience",
            MemoryType::Observation => "observation",
        }
    }
}

#[derive(Debug, Deserialize, SurrealValue)]
struct EntityRow {
    id: RecordId,
    #[serde(default)]
    #[surreal(default)]
    name: String,
    #[serde(default)]
    #[surreal(default)]
    aliases: Vec<String>,
}

/// Phrases of `query` that an entity name or alias could be: every run of up
/// to [`NAME_WORDS`] consecutive words, lowercased, each word with its
/// surrounding punctuation and a possessive "'s" trimmed ("Ada's" -> "ada",
/// "main.rs?" -> "main.rs"). Looked up exactly, these replace a scan of every
/// entity for names that occur in the question.
fn name_phrases(query: &str) -> Vec<String> {
    let words: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(|w| {
            let w = w.trim_matches(|c: char| !c.is_alphanumeric());
            w.strip_suffix("'s").or_else(|| w.strip_suffix("\u{2019}s")).unwrap_or(w).to_string()
        })
        .filter(|w| !w.is_empty())
        .take(MAX_NAME_WORDS)
        .collect();
    let mut out: Vec<String> = Vec::new();
    for start in 0..words.len() {
        for end in start + 1..=(start + NAME_WORDS).min(words.len()) {
            let phrase = words[start..end].join(" ");
            if !out.contains(&phrase) {
                out.push(phrase);
            }
        }
    }
    out
}

/// Candidate entities: the vault's person/organisation/location/.../symbol
/// whose name or an alias appears in `query` (no NER/LLM for v1), ranked by
/// match specificity (longer matched name first). Found by exact lookup of
/// the query's [`name_phrases`] on the `(vault, name_key)` and `alias_keys`
/// indexes: one query, a bounded number of rows, however large the vault.
///
/// For each matched entity, pulls its newest memories, each memory's source
/// `cache_record`, and the records linked to that source -- the "follow the
/// entity graph" hop the diagram's Graph arm describes. A tombstoned source
/// is not followed (the memory itself stays recallable). Three queries in total.
///
/// A followed record shows only if it is filed in this vault (or, in the caller's personal vault, is
/// the caller's own unrouted record): [`hydrate`] drops the rest.
async fn graph_arm(db: &OrgDb, vault: &RecordId, query: &str, limit: usize) -> AppResult<Vec<String>> {
    let phrases = name_phrases(query);
    if phrases.is_empty() {
        return Ok(Vec::new());
    }
    let lookup = |tables: &[&str]| {
        let lookups: Vec<String> = tables
            .iter()
            .flat_map(|kind| {
                [
                    format!("(SELECT id, name, aliases FROM {kind} WHERE vault = $vault AND name_key IN $phrases LIMIT {GRAPH_CANDIDATES})"),
                    format!(
                        "(SELECT id, name, aliases FROM {kind} WITH INDEX {kind}_alias_keys_idx \
                         WHERE alias_keys CONTAINSANY $phrases AND vault = $vault LIMIT {GRAPH_CANDIDATES})"
                    ),
                ]
            })
            .collect();
        // dynamic: one name lookup and one alias lookup per entity table, in one round trip.
        store::dynamic(db, "cache.graph_entities", format!("RETURN array::flatten([{}]);", lookups.join(", ")))
            .bind(("vault", vault.clone()))
            .bind(("phrases", phrases.clone()))
    };
    let mut res = match lookup(crate::entities::service::ENTITY_TABLES).await {
        // schema N-1: no custom-kind table yet
        Err(e) if crate::entities::service::no_custom_table(&e) => lookup(crate::entities::service::KINDS).await?,
        res => res?,
    };
    let rows: Vec<EntityRow> = res.take(0)?;

    let q_lower = query.to_lowercase();
    #[allow(clippy::mutable_key_type)] // RecordId hashes by value; the interior mutability is never touched
    let mut seen = HashSet::new();
    let mut matches: Vec<(usize, RecordId)> = Vec::new();
    for row in rows {
        let best = std::iter::once(row.name.as_str())
            .chain(row.aliases.iter().map(String::as_str))
            .filter(|n| !n.is_empty() && q_lower.contains(&n.to_lowercase()))
            .map(str::len)
            .max();
        if let Some(best) = best
            && seen.insert(row.id.clone())
        {
            matches.push((best, row.id));
        }
    }
    matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_string().cmp(&b.1.to_string())));
    matches.truncate(GRAPH_ENTITIES);
    if matches.is_empty() {
        return Ok(Vec::new());
    }

    #[derive(Deserialize, SurrealValue)]
    struct MemRow {
        id: RecordId,
        #[serde(default)]
        #[surreal(default)]
        source: Option<RecordId>,
    }
    // WITH INDEX: left to itself the planner walks the whole vault's memories (memory_vault_idx)
    // for each entity
    let mut sql = String::new();
    for i in 0..matches.len() {
        sql.push_str(&format!(
            "SELECT id, source, created_at FROM memory WITH INDEX memory_subject_idx \
             WHERE subject = $e{i} AND vault = $vault ORDER BY created_at DESC LIMIT $limit;"
        ));
    }
    // dynamic: one statement per matched entity.
    let mut q = store::dynamic(db, "cache.graph_memories", sql).bind(("vault", vault.clone())).bind(("limit", limit as i64));
    for (i, (_, id)) in matches.iter().enumerate() {
        q = q.bind((format!("e{i}"), id.clone()));
    }
    let mut res = q.await?;
    let mut memories: Vec<MemRow> = Vec::new();
    for i in 0..matches.len() {
        memories.extend(res.take::<Vec<MemRow>>(i)?);
    }

    let sources: Vec<RecordId> = memories.iter().filter_map(|m| m.source.clone()).collect();
    #[allow(clippy::mutable_key_type)]
    let neighbours = cs::live_neighbours(db, &sources).await?;

    let mut keys: Vec<String> = Vec::new();
    for mem in memories {
        keys.push(format!("memory:{}", mem.id.to_string()));
        if let Some((source, links)) = mem.source.as_ref().and_then(|s| neighbours.get(s).map(|l| (s, l))) {
            keys.push(record_key(source));
            keys.extend(links.iter().map(record_key));
        }
        if keys.len() >= limit {
            break;
        }
    }

    let matched: Vec<RecordId> = matches.into_iter().map(|(_, id)| id).collect();
    keys.extend(relation_hop(db, vault, &matched).await?);

    let mut seen = HashSet::new();
    keys.retain(|k| seen.insert(k.clone()));
    keys.truncate(limit);
    Ok(keys)
}

/// The graph arm's one hop along `relates_to` from the entities a query named: the relations
/// themselves (those joining two named entities first, so "how is Ada related to Acme" finds the edge
/// and its evidence), then up to [`GRAPH_NEIGHBOURS`] neighbours' newest facts. Bounded at every step
/// ([`GRAPH_EDGES`], [`NEIGHBOUR_MEMORIES`]); an edge counts only when both its ends are in `vault`,
/// so a hop never leaves the vault being searched. Two queries.
async fn relation_hop(db: &OrgDb, vault: &RecordId, matched: &[RecordId]) -> AppResult<Vec<String>> {
    if matched.is_empty() {
        return Ok(Vec::new());
    }
    #[derive(Deserialize, SurrealValue)]
    struct EdgeRow {
        id: RecordId,
        #[serde(rename = "in")]
        #[surreal(rename = "in")]
        from: RecordId,
        out: RecordId,
        #[serde(default)]
        #[surreal(default)]
        iv: Option<RecordId>,
        #[serde(default)]
        #[surreal(default)]
        ov: Option<RecordId>,
    }
    // dynamic: the edge bound is a literal LIMIT.
    let sql = format!(
        "LET $edges = array::distinct(array::flatten((SELECT VALUE array::concat(->relates_to, <-relates_to) FROM $ents))); \
         SELECT id, in, out, in.vault AS iv, out.vault AS ov FROM $edges LIMIT {GRAPH_EDGES};"
    );
    let mut res = store::dynamic(db, "cache.graph_edges", sql).bind(("ents", matched.to_vec())).await?;
    let edges: Vec<EdgeRow> = res.take(1)?;
    let edges: Vec<EdgeRow> = edges.into_iter().filter(|e| e.iv.as_ref() == Some(vault) && e.ov.as_ref() == Some(vault)).collect();

    let named = |id: &RecordId| matched.contains(id);
    let (between, outward): (Vec<&EdgeRow>, Vec<&EdgeRow>) = edges.iter().partition(|e| named(&e.from) && named(&e.out));
    let mut keys: Vec<String> = between.iter().chain(outward.iter()).map(|e| format!("relates_to:{}", crate::rid::key_string(&e.id.key).unwrap_or_default())).collect();

    let mut neighbours: Vec<RecordId> = Vec::new();
    for e in &outward {
        let far = if named(&e.from) { &e.out } else { &e.from };
        if !neighbours.contains(far) {
            neighbours.push(far.clone());
        }
    }
    neighbours.truncate(GRAPH_NEIGHBOURS);
    if !neighbours.is_empty() {
        let sql: String = (0..neighbours.len())
            .map(|i| {
                format!(
                    "SELECT VALUE id FROM memory WITH INDEX memory_subject_idx \
                     WHERE subject = $n{i} AND vault = $vault ORDER BY created_at DESC LIMIT {NEIGHBOUR_MEMORIES};"
                )
            })
            .collect();
        // dynamic: one statement per neighbour.
        let mut q = store::dynamic(db, "cache.graph_neighbour_memories", sql).bind(("vault", vault.clone()));
        for (i, id) in neighbours.iter().enumerate() {
            q = q.bind((format!("n{i}"), id.clone()));
        }
        let mut res = q.await?;
        for i in 0..neighbours.len() {
            keys.extend(res.take::<Vec<RecordId>>(i)?.iter().map(|id| format!("memory:{}", id.to_string())));
        }
    }
    Ok(keys)
}

/// Semantic arm over this vault's remembered facts: the memories nearest the query vector among those
/// embedded by the same provider and model, best first, each at least [`MIN_SIMILARITY`]. HNSW first;
/// when the shared index underfills for this vault (a small vault among big ones) the exact cosine
/// scan over the vault's vectors is the answer, as for records (`cache::search::nearest_ids`).
async fn memory_semantic_arm(db: &OrgDb, vault: &RecordId, query: Option<&QueryVector>, limit: usize) -> AppResult<Vec<String>> {
    let Some(q) = query else { return Ok(Vec::new()) };
    Ok(nearest_memories(db, vault, &q.vector, &q.spec, limit).await?.into_iter().map(|id| format!("memory:{}", id.to_string())).collect())
}

/// The `limit` memories of `vault` nearest `vector` among those embedded under `spec`, best first,
/// each at least [`MIN_SIMILARITY`] (see [`memory_semantic_arm`]).
pub async fn nearest_memories(db: &OrgDb, vault: &RecordId, vector: &[f32], spec: &str, limit: usize) -> AppResult<Vec<RecordId>> {
    #[derive(Deserialize, SurrealValue)]
    struct SimRow {
        id: RecordId,
        #[serde(default)]
        #[surreal(default)]
        sim: f64,
    }
    let ef = (limit * 2).max(64);
    // dynamic: the KNN `<|K,EF|>` operator needs literal integers.
    let knn = format!(
        "SELECT id, vector::similarity::cosine(embedding, $vec) AS sim FROM memory \
         WHERE vault = $vault AND embedding_spec = $spec AND embedding <|{limit},{ef}|> $vec ORDER BY sim DESC"
    );
    let bind = |sql: String, name: &'static str| {
        store::dynamic(db, name, sql).bind(("vault", vault.clone())).bind(("spec", spec.to_string())).bind(("vec", vector.to_vec()))
    };
    let mut rows: Vec<SimRow> = bind(knn, "cache.memory_semantic").await?.take(0)?;
    if rows.len() < limit {
        let exact = "SELECT id, vector::similarity::cosine(embedding, $vec) AS sim FROM memory \
                     WHERE vault = $vault AND embedding_spec = $spec ORDER BY sim DESC LIMIT $limit"
            .to_string();
        rows = bind(exact, "cache.memory_semantic_exact").bind(("limit", limit as i64)).await?.take(0)?;
    }
    Ok(rows.into_iter().filter(|r| r.sim >= MIN_SIMILARITY).map(|r| r.id).collect())
}

/// The query's vector, made once per recall and shared by both semantic arms, with the
/// `embedding_spec` it was made under.
struct QueryVector {
    vector: Vec<f32>,
    spec: String,
}

/// Embeds the query under [`ARM_DEADLINE`]. `None` (and the semantic arms contribute nothing) without a
/// working provider, on a provider error, or past the deadline.
async fn query_vector(db: &OrgDb, settings: &Settings, owner: &RecordId, query: &str) -> Option<QueryVector> {
    use crate::embeddings::service as embeddings;
    let fut = async {
        if query.trim().is_empty() || !embeddings::available(db, settings, owner).await {
            return AppResult::Ok(None);
        }
        let spec = embeddings::spec_for(db, settings, owner).await?.tag();
        let vector = embeddings::embed(db, settings, &[query.to_string()], Some(owner)).await?.into_iter().next().unwrap_or_default();
        Ok((vector.len() == embeddings::dim()).then_some(QueryVector { vector, spec }))
    };
    match tokio::time::timeout(ARM_DEADLINE, fut).await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            tracing::warn!("recall: semantic arms skipped: {}", e.message);
            None
        }
        Err(_) => {
            tracing::warn!("recall: query embedding timed out after {ARM_DEADLINE:?}");
            None
        }
    }
}

/// Full-text arm over remembered facts (`memory.text`), BM25-ranked. This is
/// what makes a fact an agent wrote with `memory_write` findable by what it
/// says, not just by its subject's name -- no embeddings required. Works for
/// any vault the caller can read.
async fn memory_text_arm(db: &OrgDb, vault: &RecordId, query: &str, limit: usize) -> AppResult<Vec<String>> {
    #[derive(Deserialize, SurrealValue)]
    struct ScoredRow {
        id: RecordId,
        #[serde(default)]
        #[surreal(default)]
        score: f64,
    }
    // one BM25 match per term (`@@` with a whole question needs every word to
    // match), all in one round trip
    let terms = cs::search_terms(query);
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let sql: String = (0..terms.len())
        .map(|i| {
            format!(
                "SELECT id, search::score(1) AS score FROM memory \
                 WHERE vault = $vault AND text @1@ $t{i} ORDER BY score DESC LIMIT $limit;"
            )
        })
        .collect();
    // dynamic: one statement per term.
    let mut q = store::dynamic(db, "cache.memory_text_terms", sql).bind(("vault", vault.clone())).bind(("limit", limit as i64));
    for (i, t) in terms.iter().enumerate() {
        q = q.bind((format!("t{i}"), t.clone()));
    }
    let mut res = q.await?;
    let mut per_term = Vec::new();
    for i in 0..terms.len() {
        let rows: Vec<ScoredRow> = res.take(i)?;
        per_term.push(rows.into_iter().map(|r| (format!("memory:{}", r.id.to_string()), r.score)).collect());
    }
    Ok(cs::rank_term_hits(per_term, limit))
}

/// A record's candidate key in recall: its internal key, whoever owns it.
fn record_key(id: &RecordId) -> String {
    format!("cache_record:{}", cs::key_of(id))
}

/// The newest `limit` records of `scope` in the range (`occurred_at`), newest first: the records half
/// of the temporal arm, run once for every vault of the recall ([`RecordArms`]).
async fn temporal_records(db: &OrgDb, scope: &cs::RecordScope, time_range: Option<(Datetime, Datetime)>, limit: usize) -> AppResult<Vec<String>> {
    let Some((since, until)) = time_range else {
        return Ok(Vec::new());
    };
    #[derive(Deserialize, SurrealValue)]
    struct CacheRow {
        id: RecordId,
    }
    let q = store::cache::CACHE_RECORDS_IN_RANGE.on(db).bind(("since", since)).bind(("until", until)).bind(("limit", limit as i64));
    let rows: Vec<CacheRow> = scope.bind(q).await?.take(0)?;
    Ok(rows.iter().map(|r| record_key(&r.id)).collect())
}

/// Explicit date-range filter only -- no NL date parsing in v1. Merges the
/// newest `limit` of `records` (this vault's share of [`temporal_records`], dated `occurred_at`) and
/// `memory` rows (vault-scoped, dated when they happened if known, else when written) in the range,
/// ranked by recency. Each side is ordered and
/// limited in the database, so a broad range never loads the whole range.
async fn temporal_ids(
    db: &OrgDb,
    vault: &RecordId,
    time_range: Option<(Datetime, Datetime)>,
    limit: usize,
    records: Vec<(Datetime, String)>,
) -> AppResult<Vec<String>> {
    let Some((since, until)) = time_range else {
        return Ok(Vec::new());
    };

    #[derive(Deserialize, SurrealValue)]
    struct MemRow {
        id: RecordId,
        at: Datetime,
    }

    let mut dated = records;
    let mut res = store::cache::MEMORIES_IN_RANGE
        .on(db)
        .bind(("vault", vault.clone()))
        .bind(("since", since))
        .bind(("until", until))
        .bind(("limit", limit as i64))
        .await?;
    let rows: Vec<MemRow> = res.take(0)?;
    dated.extend(rows.into_iter().map(|r| (r.at, format!("memory:{}", r.id.to_string()))));

    dated.sort_by_key(|d| std::cmp::Reverse(d.0));
    Ok(dated.into_iter().take(limit).map(|(_, k)| k).collect())
}

struct Hydrated {
    id: String,
    kind: &'static str,
    text: String,
    source: Option<String>,
    occurred_at: Option<DateTime<Utc>>,
    mem_type: String,
    document: Option<serde_json::Value>,
    status: Option<String>,
}

/// Every candidate key's content, in at most two queries (records, memories) whatever the candidate
/// count. Authorization is re-checked here: a record must be live and filed in `vault` (`scope`, one
/// vault; the ones [`RecordArms`] already read are taken from it), a memory must be in `vault`. A stale
/// observation (its facts changed since it was consolidated) is left out: it is not
/// a current fact; the raw facts it summarised are still recalled. So is a superseded fact (a later
/// fact contradicted it; see `entities::supersede`).
async fn hydrate(
    db: &OrgDb,
    scope: &cs::RecordScope,
    vault: &RecordId,
    keys: &[&String],
    found: &HashMap<String, cs::CacheRecord>,
    history: bool,
) -> AppResult<HashMap<String, Hydrated>> {
    let mut out = HashMap::new();
    let mut records = Vec::new();
    let mut fetch = Vec::new();
    for key in keys.iter().filter(|k| k.starts_with("cache_record:")) {
        match found.get(*key) {
            Some(rec) if scope.vault_of(rec.vault.as_ref()).as_ref() == Some(vault) => records.push(((*key).clone(), rec.clone())),
            Some(_) => {}
            None => fetch.push(RecordId::from_table_key("cache_record", key["cache_record:".len()..].to_string())),
        }
    }
    for rec in cs::get_many(db, scope, &fetch).await? {
        records.push((rec.rid.as_ref().map(record_key).unwrap_or_default(), rec));
    }
    for (key, rec) in records {
        let text = format!("{}\n{}", rec.title, rec.body_text).trim().to_string();
        let document = crate::documents::chunk_ref(&rec.id, &rec.source, &rec.payload);
        out.insert(
            key,
            Hydrated {
                document,
                id: rec.id,
                kind: "cache_record",
                text,
                source: if rec.source.is_empty() { None } else { Some(rec.source) },
                occurred_at: rec.occurred_at.map(to_chrono),
                mem_type: String::new(),
                status: None,
            },
        );
    }

    hydrate_relations(db, vault, keys, &mut out).await?;

    let memory_ids: Vec<RecordId> =
        keys.iter().filter_map(|k| k.strip_prefix("memory:")).filter_map(|r| crate::rid::parse(r).ok()).collect();
    if memory_ids.is_empty() {
        return Ok(out);
    }
    #[derive(Deserialize, SurrealValue)]
    struct MemRow {
        id: RecordId,
        #[serde(default)]
        #[surreal(default)]
        text: String,
        #[serde(default)]
        #[surreal(default)]
        source: Option<RecordId>,
        created_at: Datetime,
        #[serde(default)]
        #[surreal(default)]
        updated_at: Option<Datetime>,
        #[serde(default)]
        #[surreal(default)]
        occurred_at: Option<Datetime>,
        #[serde(default)]
        #[surreal(default)]
        status: Option<String>,
        #[serde(rename = "type", default = "default_memory_type")]
        #[surreal(rename = "type", default = "default_memory_type")]
        mem_type: String,
    }
    fn default_memory_type() -> String {
        "world".to_string()
    }
    let mut res =
        store::cache::MEMORIES_FOR_RECALL.on(db).bind(("ids", memory_ids)).bind(("vault", vault.clone())).bind(("history", history)).await?;
    let rows: Vec<MemRow> = res.take(0)?;
    for row in rows {
        out.insert(
            format!("memory:{}", row.id.to_string()),
            Hydrated {
                id: row.id.to_string(),
                kind: "memory",
                text: row.text,
                source: row.source.map(|s| cs::caller_id(&s, scope.owner())),
                occurred_at: Some(to_chrono(row.occurred_at.or(row.updated_at).unwrap_or(row.created_at))),
                mem_type: row.mem_type,
                document: None,
                status: row.status.filter(|s| s == "superseded"),
            },
        );
    }
    Ok(out)
}

/// The graph arm's `relates_to:` keys as readable items ("Ada Lovelace works at Acme Corp"), one
/// query. Authorization again: both ends must be in `vault`.
async fn hydrate_relations(db: &OrgDb, vault: &RecordId, keys: &[&String], out: &mut HashMap<String, Hydrated>) -> AppResult<()> {
    let ids: Vec<RecordId> =
        keys.iter().filter_map(|k| k.strip_prefix("relates_to:")).map(|k| RecordId::from_table_key("relates_to", k.to_string())).collect();
    if ids.is_empty() {
        return Ok(());
    }
    #[derive(Deserialize, SurrealValue)]
    struct RelRow {
        id: RecordId,
        #[serde(default)]
        #[surreal(default)]
        label: String,
        #[serde(default)]
        #[surreal(default)]
        a: String,
        #[serde(default)]
        #[surreal(default)]
        b: String,
        created_at: Datetime,
    }
    let mut res = store::cache::RELATIONS_FOR_RECALL.on(db).bind(("ids", ids)).bind(("vault", vault.clone())).await?;
    let rows: Vec<RelRow> = res.take(0)?;
    for row in rows {
        let label = if row.label.is_empty() { "related to".to_string() } else { row.label.replace('_', " ") };
        out.insert(
            format!("relates_to:{}", crate::rid::key_string(&row.id.key).unwrap_or_default()),
            Hydrated {
                id: row.id.to_string(),
                kind: "relation",
                text: format!("{} {label} {}", row.a, row.b),
                source: None,
                occurred_at: Some(to_chrono(row.created_at)),
                mem_type: String::new(),
                document: None,
                status: None,
            },
        );
    }
    Ok(())
}

/// Monotonic in both arm-count ("proof") and recency, matching the diagram's
/// small multipliers in spirit, not exact values -- the task leaves the
/// exact formula up to the implementation.
///
/// proof:   `1 + 0.05 * (arms_hit - 1)` -> 1.00 .. 1.25 for 1..6 arms
/// recency: 1.0 at age=0, linearly down to a 0.7 floor at 365+ days old;
///          1.0 (neutral) when there's no date to judge recency from.
///
fn boost(arms_hit: usize, occurred_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> f64 {
    let proof = 1.0 + PROOF_STEP * (arms_hit as f64 - 1.0);

    let recency = match occurred_at {
        None => 1.0,
        Some(dt) => {
            let age_days = ((now - dt).num_seconds() as f64 / 86400.0).max(0.0);
            (1.0 - age_days.min(RECENCY_WINDOW_DAYS) / RECENCY_WINDOW_DAYS * RECENCY_SPAN).max(RECENCY_FLOOR)
        }
    };

    proof * recency
}

/// [`boost`], except that with an explicit date range "recent" means "in the range asked about": an
/// item dated inside it gets the full recency factor and one dated outside it the floor, whatever its
/// age (a question about last year must not prefer this week's facts). Undated items stay neutral.
fn boost_in(arms_hit: usize, occurred_at: Option<DateTime<Utc>>, now: DateTime<Utc>, range: Option<(DateTime<Utc>, DateTime<Utc>)>) -> f64 {
    let Some((since, until)) = range else { return boost(arms_hit, occurred_at, now) };
    let proof = boost(arms_hit, None, now);
    match occurred_at {
        Some(at) if at < since || at > until => proof * RECENCY_FLOOR,
        _ => proof,
    }
}

/// Checks recall's inputs up front -- every bound is a clear 400, not a slow
/// query or an arithmetic overflow -- and parses the time range.
fn validate(
    query: &str,
    time_range: Option<(&str, &str)>,
    limit: usize,
    max_tokens: Option<usize>,
) -> AppResult<Option<(Datetime, Datetime)>> {
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(AppError::bad_request(format!("query is longer than {MAX_QUERY_CHARS} characters")));
    }
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(AppError::bad_request(format!("limit must be between 1 and {MAX_LIMIT}")));
    }
    if max_tokens.is_some_and(|t| !(1..=MAX_TOKENS).contains(&t)) {
        return Err(AppError::bad_request(format!("max_tokens must be between 1 and {MAX_TOKENS}")));
    }
    let Some((since, until)) = time_range else { return Ok(None) };
    match cs::parse_range(Some(since), Some(until))? {
        (Some(since), Some(until)) => Ok(Some((since.into(), until.into()))),
        _ => Ok(None),
    }
}

/// Runs one arm under [`ARM_DEADLINE`]. A timeout -- or, for an `optional`
/// arm (semantic: an external provider), an error -- drops just that arm's
/// candidates, so one slow or broken arm never blocks the others' results.
async fn arm(
    name: &str,
    optional: bool,
    fut: impl std::future::Future<Output = AppResult<Vec<String>>>,
) -> AppResult<Vec<String>> {
    let started = std::time::Instant::now();
    match tokio::time::timeout(ARM_DEADLINE, fut).await {
        Ok(Ok(keys)) => {
            tracing::debug!(arm = name, candidates = keys.len(), elapsed_ms = started.elapsed().as_millis() as u64, "recall: arm done");
            Ok(keys)
        }
        Ok(Err(e)) if optional => {
            tracing::warn!("recall: {name} arm skipped: {}", e.message);
            Ok(Vec::new())
        }
        Ok(Err(e)) => Err(e),
        Err(_) => {
            tracing::warn!("recall: {name} arm timed out after {ARM_DEADLINE:?}");
            Ok(Vec::new())
        }
    }
}

/// Recall in `vault_ids`, or -- when there are none -- in the caller's personal
/// vault and every org vault they belong to (facts kept in an org vault must
/// not look absent to a caller that forgot to pass it). Each hit names its vault; scores are scaled so
/// the best hit is 1. `history` also returns facts a later one superseded, marked `status: superseded`
/// (for "what did we think before"); otherwise only current facts.
#[allow(clippy::too_many_arguments)]
pub async fn recall(
    db: &OrgDb,
    settings: &Settings,
    owner: &RecordId,
    query: &str,
    time_range: Option<(&str, &str)>,
    limit: usize,
    max_tokens: Option<usize>,
    types: Option<&[MemoryType]>,
    vault_ids: &[RecordId],
    history: bool,
) -> AppResult<Vec<RecallItem>> {
    let time_range = validate(query, time_range, limit, max_tokens)?;
    let pool = limit.saturating_mul(4).max(40);

    let rs = vaults_service::read_scope(db, owner, vault_ids).await?;
    let scope = cs::RecordScope::vaults(owner, &rs.personal, &rs.vaults);
    let records = record_arms(db, settings, &scope, query, time_range, pool).await?;

    let mut scored = Vec::new();
    for vault in &rs.vaults {
        let mut hits = recall_in(db, &scope.narrow(vault), query, time_range, pool, types, vault, &records, history).await?;
        let (id, name) = (vault.to_string(), rs.names.get(&vault.to_string()).cloned().unwrap_or_default());
        for h in &mut hits {
            (h.vault, h.vault_name) = (id.clone(), name.clone());
        }
        scored.extend(hits);
    }

    scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.id.cmp(&b.id)));
    scored.truncate(limit);
    let top = scored.first().map(|i| i.score).unwrap_or(1.0);
    for item in &mut scored {
        item.score = (item.score / top * 1000.0).round() / 1000.0;
    }

    Ok(match max_tokens {
        Some(budget) => fit_budget(scored, budget),
        None => scored,
    })
}

/// The synced-record arms (semantic, keyword, and the records half of temporal), run once over every
/// vault of a recall rather than once per vault; `found` holds every record they hit, read once, so
/// each vault's pass ([`RecordArms::in_vault`]) keeps the ones filed in it.
struct RecordArms {
    semantic: Vec<String>,
    keyword: Vec<String>,
    temporal: Vec<String>,
    found: HashMap<String, cs::CacheRecord>,
    /// The query's vector, for the memory semantic arm of each vault's pass.
    query: Option<QueryVector>,
}

impl RecordArms {
    /// The keys of `arm` whose records are filed in `scope`'s vault, in order.
    fn in_vault(&self, arm: &[String], scope: &cs::RecordScope, vault: &RecordId) -> Vec<String> {
        arm.iter().filter(|k| self.found.get(*k).is_some_and(|r| scope.vault_of(r.vault.as_ref()).as_ref() == Some(vault))).cloned().collect()
    }
}

/// Embedding the query waits on an external provider: it runs concurrently with the keyword and
/// temporal arms, so a slow or hanging provider costs at most its deadline. Every arm has its own deadline.
async fn record_arms(
    db: &OrgDb,
    settings: &Settings,
    scope: &cs::RecordScope,
    query: &str,
    time_range: Option<(Datetime, Datetime)>,
    pool: usize,
) -> AppResult<RecordArms> {
    let no_filter = cs::RecordFilter::default();
    let keys = |ids: Vec<RecordId>| ids.iter().map(record_key).collect::<Vec<_>>();
    let local = async {
        let keyword = arm("keyword", false, async { Ok(keys(cs::keyword_ids(db, scope, query, &no_filter, pool).await?)) }).await;
        let temporal = arm("temporal", false, temporal_records(db, scope, time_range, pool)).await;
        (keyword, temporal)
    };
    let (qv, (keyword, temporal)) = tokio::join!(query_vector(db, settings, scope.owner(), query), local);
    let (keyword, temporal) = (keyword?, temporal?);
    let semantic = match &qv {
        Some(q) => arm("semantic", true, async { Ok(keys(cs::nearest_ids(db, scope, q.vector.clone(), &no_filter, pool).await?)) }).await?,
        None => Vec::new(),
    };

    let mut seen = HashSet::new();
    let ids: Vec<RecordId> = semantic
        .iter()
        .chain(&keyword)
        .chain(&temporal)
        .filter(|k| seen.insert(*k))
        .map(|k| RecordId::from_table_key("cache_record", k["cache_record:".len()..].to_string()))
        .collect();
    let found = cs::get_many(db, scope, &ids).await?.into_iter().filter_map(|r| Some((record_key(r.rid.as_ref()?), r))).collect();
    Ok(RecordArms { semantic, keyword, temporal, found, query: qv })
}

/// One vault's ranked hits (raw scores, unsorted): its share of the record arms, plus the graph,
/// temporal and memory text arms over its memories. `scope` is the recall's scope narrowed to `vault`.
#[allow(clippy::too_many_arguments)]
async fn recall_in(
    db: &OrgDb,
    scope: &cs::RecordScope,
    query: &str,
    time_range: Option<(Datetime, Datetime)>,
    pool: usize,
    types: Option<&[MemoryType]>,
    vault: &RecordId,
    records: &RecordArms,
    history: bool,
) -> AppResult<Vec<RecallItem>> {
    let started = std::time::Instant::now();
    let semantic_keys = records.in_vault(&records.semantic, scope, vault);
    let keyword_keys = records.in_vault(&records.keyword, scope, vault);
    let dated: Vec<(Datetime, String)> = records
        .in_vault(&records.temporal, scope, vault)
        .into_iter()
        .filter_map(|k| Some((records.found.get(&k)?.occurred_at?, k)))
        .collect();

    // These arms only query SurrealDB and run one after another (running them concurrently made each
    // several times slower, they contend inside the database).
    let graph = arm("graph", false, graph_arm(db, vault, query, pool)).await?;
    let temporal = arm("temporal", false, temporal_ids(db, vault, time_range, pool, dated)).await?;
    let memory = arm("memory_text", false, memory_text_arm(db, vault, query, pool)).await?;
    let memory_semantic = arm("memory_semantic", true, memory_semantic_arm(db, vault, records.query.as_ref(), pool)).await?;
    let arms = [semantic_keys, keyword_keys, graph, temporal, memory, memory_semantic];
    tracing::debug!(
        semantic = arms[0].len(),
        keyword = arms[1].len(),
        graph = arms[2].len(),
        temporal = arms[3].len(),
        memory_text = arms[4].len(),
        memory_semantic = arms[5].len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "recall: arm candidates"
    );

    let rrf = cs::rrf_scores(&arms);
    if rrf.is_empty() {
        return Ok(Vec::new());
    }
    let keys: Vec<&String> = rrf.keys().collect();
    let mut hydrated = hydrate(db, scope, vault, &keys, &records.found, history).await?;

    let now = Utc::now();
    let range = time_range.map(|(a, b)| (to_chrono(a), to_chrono(b)));
    let mut scored: Vec<RecallItem> = Vec::new();
    for (key, base_score) in &rrf {
        let Some(item) = hydrated.remove(key) else {
            continue;
        };
        if let Some(types) = types
            && item.kind == "memory"
        {
            let allowed = types.iter().any(|t| t.as_str() == item.mem_type);
            if !allowed {
                continue;
            }
        }
        let arms_hit = arms.iter().filter(|arm| arm.contains(key)).count();
        let b = boost_in(arms_hit, item.occurred_at, now, range);
        scored.push(RecallItem {
            id: item.id,
            kind: item.kind,
            text: item.text,
            source: item.source,
            occurred_at: item.occurred_at.map(|d| d.to_rfc3339()),
            score: base_score * b,
            arms_hit,
            vault: String::new(),
            vault_name: String::new(),
            document: item.document,
            status: item.status,
        });
    }
    Ok(scored)
}

/// Fits ranked items into a token budget (~4 chars/token). Each item is
/// capped at a third of the budget -- trimmed with "…", its `id` kept so the
/// caller can fetch the full text -- and an item that doesn't fit is skipped
/// rather than ending the list, so one huge memory (an imported transcript,
/// say) can't crowd out everything after it.
fn fit_budget(items: Vec<RecallItem>, budget: usize) -> Vec<RecallItem> {
    let cap_chars = (budget / 3).max(60) * 4;
    let mut out = Vec::new();
    let mut used = 0usize;
    for mut item in items {
        if item.text.chars().count() > cap_chars {
            item.text = item.text.chars().take(cap_chars - 1).collect::<String>() + "\u{2026}";
        }
        let tokens = item.text.chars().count().div_ceil(4);
        if used.saturating_add(tokens) > budget {
            continue;
        }
        used += tokens;
        out.push(item);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn a_date_range_makes_in_range_items_recent_and_the_rest_old() {
        let now = Utc::now();
        let range = Some((now - Duration::days(600), now - Duration::days(300)));
        let inside = boost_in(1, Some(now - Duration::days(400)), now, range);
        let outside = boost_in(1, Some(now), now, range);
        assert!((inside - 1.0).abs() < 1e-9);
        assert!((outside - RECENCY_FLOOR).abs() < 1e-9);
        assert!((boost_in(1, None, now, range) - 1.0).abs() < 1e-9);
        assert_eq!(boost_in(2, Some(now), now, None), boost(2, Some(now), now));
    }

    #[test]
    fn boost_is_neutral_with_one_arm_and_no_date() {
        let now = Utc::now();
        assert!((boost(1, None, now) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn boost_increases_with_more_arms() {
        let now = Utc::now();
        assert!(boost(2, None, now) > boost(1, None, now));
        assert!(boost(4, None, now) > boost(2, None, now));
    }

    #[test]
    fn boost_decreases_with_age_down_to_floor() {
        let now = Utc::now();
        let fresh = boost(1, Some(now), now);
        let old = boost(1, Some(now - Duration::days(400)), now);
        assert!((fresh - 1.0).abs() < 1e-9);
        assert!((old - RECENCY_FLOOR).abs() < 1e-9);
        assert!(old < fresh);
    }

    #[test]
    fn boost_is_monotonic_between_zero_and_window() {
        let now = Utc::now();
        let b0 = boost(1, Some(now), now);
        let b_half = boost(1, Some(now - Duration::days(180)), now);
        let b_full = boost(1, Some(now - Duration::days(365)), now);
        assert!(b0 > b_half);
        assert!(b_half > b_full);
        assert!((b_full - RECENCY_FLOOR).abs() < 1e-9);
    }

    fn item(text: &str) -> RecallItem {
        RecallItem { id: "m".into(), kind: "memory", text: text.into(), source: None, occurred_at: None, score: 1.0, arms_hit: 1, vault: String::new(), vault_name: String::new(), document: None, status: None }
    }

    #[test]
    fn fit_budget_trims_a_huge_item_instead_of_returning_nothing() {
        let out = fit_budget(vec![item(&"x".repeat(40_000)), item("small fact")], 600);
        assert_eq!(out.len(), 2);
        assert!(out[0].text.ends_with('\u{2026}'));
        assert!(out[0].text.chars().count() <= 200 * 4);
        assert_eq!(out[1].text, "small fact");
    }

    #[test]
    fn fit_budget_skips_what_does_not_fit_and_keeps_going() {
        // budget 350 -> items capped at 116 tokens: three fit (348), the fourth is
        // skipped, and the short one after it still fits
        let big = "y".repeat(10_000);
        let out = fit_budget(vec![item(&big), item(&big), item(&big), item(&big), item("tiny")], 350);
        let total: usize = out.iter().map(|i| i.text.chars().count().div_ceil(4)).sum();
        assert!(total <= 350);
        assert_eq!(out.len(), 4);
        assert_eq!(out.last().unwrap().text, "tiny");
        assert!(fit_budget(vec![], 100).is_empty());
    }

    #[test]
    fn memory_type_as_str_matches_schema_values() {
        assert_eq!(MemoryType::World.as_str(), "world");
        assert_eq!(MemoryType::Experience.as_str(), "experience");
        assert_eq!(MemoryType::Observation.as_str(), "observation");
    }

    #[test]
    fn name_phrases_are_trimmed_word_runs() {
        let p = name_phrases("What does Ada Lovelace's team think of main.rs?");
        for want in ["ada lovelace", "ada", "lovelace", "main.rs", "team think of main.rs"] {
            assert!(p.contains(&want.to_string()), "{want} in {p:?}");
        }
        assert!(!p.iter().any(|x| x.contains('?') || x.contains("'s")));
        assert!(name_phrases("  ?! ").is_empty());
        // bounded: at most NAME_WORDS words per phrase, MAX_NAME_WORDS words read
        let long = name_phrases(&"w ".repeat(1000));
        assert_eq!(long, (1..=NAME_WORDS).map(|n| vec!["w"; n].join(" ")).collect::<Vec<_>>());
        let distinct: String = (0..1000).map(|i| format!("w{i} ")).collect();
        assert!(name_phrases(&distinct).len() <= MAX_NAME_WORDS * NAME_WORDS);
    }

    #[test]
    fn validate_bounds_every_input() {
        assert!(validate("q", None, 20, None).unwrap().is_none());
        assert!(validate("q", Some(("2024-01-01", "2024-02-01T00:00:00Z")), 1, Some(500)).unwrap().is_some());
        assert!(validate("q", None, 0, None).is_err());
        assert!(validate("q", None, MAX_LIMIT + 1, None).is_err());
        assert!(validate("q", None, usize::MAX, None).is_err());
        assert!(validate("q", None, 20, Some(0)).is_err());
        assert!(validate("q", None, 20, Some(MAX_TOKENS + 1)).is_err());
        assert!(validate(&"x".repeat(MAX_QUERY_CHARS + 1), None, 20, None).is_err());
        assert!(validate("q", Some(("2024-03-01", "2024-02-01")), 20, None).is_err());
        assert!(validate("q", Some(("last week", "2024-02-01")), 20, None).is_err());
    }

    #[tokio::test]
    async fn a_hanging_arm_is_dropped_at_its_deadline() {
        let started = std::time::Instant::now();
        let hung = arm("semantic", true, std::future::pending::<AppResult<Vec<String>>>()).await.unwrap();
        assert!(hung.is_empty());
        assert!(started.elapsed() < ARM_DEADLINE + std::time::Duration::from_secs(1));
    }

    #[tokio::test]
    async fn only_an_optional_arm_swallows_errors() {
        let failing = || async { Err::<Vec<String>, _>(AppError::internal("provider down")) };
        assert!(arm("semantic", true, failing()).await.unwrap().is_empty());
        assert!(arm("keyword", false, failing()).await.is_err());
        assert_eq!(arm("keyword", false, async { Ok(vec!["k".to_string()]) }).await.unwrap(), vec!["k"]);
    }
}
