//! Cache write + query API. The only module that knows SurrealDB's BM25/MTREE
//! indexes exist -- this module's external signatures (`upsert`,
//! `set_embedding`, `search`, `get`, `list`, `links`) are the swap point for any future backend.
//!

use surrealdb::types::SurrealValue;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use surrealdb::types::{Datetime, RecordId};
use crate::rid::RecordIdExt;

use crate::pool::OrgDb;
use crate::store;
use crate::embeddings::service::DIM;
use crate::error::{AppError, AppResult};

const RRF_K: f64 = 60.0;

/// One `linked_to` relation to create/keep when upserting a record: `{"target": ..., "rel": ...}`.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct LinkSpec {
    pub target: String,
    pub rel: String,
}

/// The ingest envelope a mapper produces for one raw source record.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Envelope {
    pub id: String,
    #[serde(default)]
    pub source: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub external_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body_text: String,
    #[serde(default)]
    pub occurred_at: Option<Datetime>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub links: Vec<LinkSpec>,
    /// Where routing files the record (`sources::routing`); never set by a mapper. `None`: no routing
    /// (uploaded documents, the demo source), so a new record goes to the owner's personal vault.
    #[serde(skip)]
    pub placement: Option<Placement>,
}

/// Where a sync files one record (`sources::routing::Router::placement`). Routing places a record when
/// it is first stored; a stored record stays in its vault on later syncs (a changed destination applies
/// to future records only), unless its owner is no longer a member of that vault: then it is re-filed
/// to `vault`, so a sync never writes into a vault its owner has left.
#[derive(Debug, Clone, Default)]
pub struct Placement {
    /// `None`: the owner's personal vault.
    pub vault: Option<RecordId>,
    /// What the rules matched on, kept on the record for "Move existing data".
    pub route_keys: Vec<String>,
    /// The vaults the owner is an active member of.
    pub writable: std::sync::Arc<Vec<RecordId>>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct CacheRecord {
    /// literal `"{source}:{type}:{external_id}"` (no table prefix, no owner
    /// prefix -- see `literal`).
    pub id: String,
    pub source: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub external_id: String,
    pub title: String,
    pub body_text: String,
    pub occurred_at: Option<Datetime>,
    pub url: String,
    pub payload: Value,
    pub content_hash: String,
    pub ingested_at: Option<Datetime>,
    pub updated_at: Option<Datetime>,
    pub deleted: bool,
    pub embedding: Option<Vec<f32>>,
    /// The `cache_record` id.
    #[serde(skip)]
    pub rid: Option<RecordId>,
    /// Whose synced record it is.
    #[serde(skip)]
    pub owner: Option<RecordId>,
    /// The vault it is filed in (`None`: its owner's personal vault).
    #[serde(skip)]
    pub vault: Option<RecordId>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct Row {
    id: RecordId,
    #[serde(default)]
    #[surreal(default)]
    owner: Option<RecordId>,
    #[serde(default)]
    #[surreal(default)]
    vault: Option<RecordId>,
    #[serde(default)]
    #[surreal(default)]
    route_keys: Option<Vec<String>>,
    #[serde(default)]
    #[surreal(default)]
    source: String,
    #[serde(rename = "type", default)]
    #[surreal(rename = "type", default)]
    type_: String,
    #[serde(default)]
    #[surreal(default)]
    external_id: String,
    #[serde(default)]
    #[surreal(default)]
    title: String,
    #[serde(default)]
    #[surreal(default)]
    body_text: String,
    #[serde(default)]
    #[surreal(default)]
    occurred_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    url: String,
    #[serde(default)]
    #[surreal(default)]
    payload: Value,
    #[serde(default)]
    #[surreal(default)]
    content_hash: String,
    #[serde(default)]
    #[surreal(default)]
    ingested_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    updated_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    deleted: bool,
    #[serde(default)]
    #[surreal(default)]
    embedding: Option<Vec<f32>>,
}

/// The owner-id prefix baked into the internal `cache_record` key -- callers
/// never see or pass it.
pub(crate) fn owner_key(owner: &RecordId) -> String {
    crate::rid::key_string(owner.key()).unwrap_or_else(|| owner.to_string())
}

pub(crate) fn rid(owner: &RecordId, record_id: &str) -> RecordId {
    RecordId::from_table_key("cache_record", format!("{}:{record_id}", owner_key(owner)))
}

/// The caller-facing record id.
pub(crate) fn literal(key: &RecordId) -> String {
    let raw = crate::rid::key_string(key.key()).unwrap_or_default();
    raw.split_once(':').map(|(_, rest)| rest.to_string()).unwrap_or(raw)
}

/// The id `reader` uses for a record: their own records without the owner prefix (as always), another
/// owner's (routed to a vault they share) with it, so the id stays unambiguous in a shared vault.
pub(crate) fn caller_id(key: &RecordId, reader: &RecordId) -> String {
    let raw = crate::rid::key_string(key.key()).unwrap_or_default();
    match raw.strip_prefix(&format!("{}:", owner_key(reader))) {
        Some(own) => own.to_string(),
        None => raw,
    }
}

/// The internal key text of a record (`{owner_key}:{source}:{type}:{external_id}`): what recall passes
/// between its arms, whoever owns the record.
pub(crate) fn key_of(key: &RecordId) -> String {
    crate::rid::key_string(key.key()).unwrap_or_default()
}

/// The records a caller-facing `id` can name: the reader's own, or (with the owner prefix) another owner's.
fn candidates(reader: &RecordId, id: &str) -> [RecordId; 2] {
    [rid(reader, id), RecordId::from_table_key("cache_record", id.to_string())]
}

/// Which synced records a read sees. Routing files a record in a vault (`vault`); a record it did not
/// route stays in its owner's personal vault (`vault = NONE`) and is the owner's alone, as every record
/// was before routing. A read sees the records filed in `vaults` (each one authorized for the caller
/// first), plus the caller's own personal-vault records when their personal vault is among them.
#[derive(Debug, Clone)]
pub struct RecordScope {
    owner: RecordId,
    own: bool,
    vaults: Vec<RecordId>,
    personal: Option<RecordId>,
}

impl RecordScope {
    /// The predicate on `cache_record`; [`RecordScope::bind`] binds `$vaults`, `$own` and `$owner`. Both
    /// branches are index reads (`cache_record_vault_idx`, `cache_record_owner_idx`).
    pub(crate) const SQL: &'static str = "(vault IN $vaults OR ($own AND vault = NONE AND owner = $owner))";

    /// Only `owner`'s own personal-vault records.
    pub fn own(owner: &RecordId) -> Self {
        RecordScope { owner: owner.clone(), own: true, vaults: Vec::new(), personal: None }
    }

    /// What `owner` reads across `vaults`, already authorized; `personal` is their personal vault.
    pub fn vaults(owner: &RecordId, personal: &RecordId, vaults: &[RecordId]) -> Self {
        RecordScope { owner: owner.clone(), own: vaults.contains(personal), vaults: vaults.to_vec(), personal: Some(personal.clone()) }
    }

    pub fn owner(&self) -> &RecordId {
        &self.owner
    }

    /// This scope narrowed to one of its vaults.
    pub(crate) fn narrow(&self, vault: &RecordId) -> Self {
        RecordScope {
            owner: self.owner.clone(),
            own: self.own && self.personal.as_ref() == Some(vault),
            vaults: vec![vault.clone()],
            personal: self.personal.clone(),
        }
    }

    pub(crate) fn bind<'a>(&self, q: store::Q<'a>) -> store::Q<'a> {
        q.bind(("vaults", self.vaults.clone())).bind(("own", self.own)).bind(("owner", self.owner.clone()))
    }

    /// The vault a record in this scope is shown in: its own, or the caller's personal vault.
    pub(crate) fn vault_of(&self, vault: Option<&RecordId>) -> Option<RecordId> {
        vault.or(self.personal.as_ref()).cloned()
    }
}

/// The [`RecordScope`] of a read by `owner` across `vaults` (already authorized). A user without a
/// personal vault (none is ever made without one) sees only what is filed in `vaults`.
pub async fn scope_for(db: &OrgDb, owner: &RecordId, vaults: &[RecordId]) -> AppResult<RecordScope> {
    Ok(match crate::vaults::service::personal_vault_id(db, owner).await {
        Ok(personal) => RecordScope::vaults(owner, &personal, vaults),
        Err(_) => RecordScope { owner: owner.clone(), own: false, vaults: vaults.to_vec(), personal: None },
    })
}

fn row_to_record(row: Row, reader: &RecordId) -> CacheRecord {
    CacheRecord {
        id: caller_id(&row.id, reader),
        rid: Some(row.id.clone()),
        source: row.source,
        type_: row.type_,
        external_id: row.external_id,
        title: row.title,
        body_text: row.body_text,
        occurred_at: row.occurred_at,
        url: row.url,
        payload: row.payload,
        content_hash: row.content_hash,
        ingested_at: row.ingested_at,
        updated_at: row.updated_at,
        deleted: row.deleted,
        embedding: row.embedding,
        owner: row.owner,
        vault: row.vault,
    }
}

/// `json!({...})`'s default `Map` is a `BTreeMap` (no `preserve_order`
/// feature enabled), so keys come out sorted. The exact bytes only need to be stable within this
/// implementation, for dedup.
fn hash_envelope(env: &Envelope) -> String {
    // Stored hashes come from SurrealDB 2.x, whose `Datetime` displayed as `d'...'`; keep that
    // text so an upgraded install does not see every record as changed (and re-embed it).
    let occurred_at_str = env.occurred_at.as_ref().map(|d| format!("d'{d}'"));
    let blob = json!({
        "title": env.title,
        "body_text": env.body_text,
        "url": env.url,
        "occurred_at": occurred_at_str,
        "payload": env.payload,
        "deleted": env.deleted,
    });
    let s = serde_json::to_string(&blob).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    format!("{:x}", hasher.finalize())
}

async fn reconcile_links(db: &OrgDb, owner: &RecordId, record_rid: &RecordId, links_spec: &[LinkSpec]) -> AppResult<()> {
    store::cache::DELETE_SYNC_LINKS.on(db).bind(("id", record_rid.clone())).await?;
    for link in links_spec {
        // (in, out, rel) has a unique index -- an error here means the edge
        // already exists; idempotent no-op.
        let _ = store::cache::RELATE_SYNC_LINK
            .on(db)
            .bind(("in", record_rid.clone()))
            .bind(("out", rid(owner, &link.target)))
            .bind(("rel", link.rel.clone()))
            .await;
    }
    Ok(())
}

/// Whether the record's stored sync links are exactly `links_spec`.
async fn links_unchanged(db: &OrgDb, owner: &RecordId, record_rid: &RecordId, links_spec: &[LinkSpec]) -> AppResult<bool> {
    #[derive(Deserialize, SurrealValue)]
    struct LinkRow {
        rel: String,
        out: RecordId,
    }
    let stored: Vec<LinkRow> = store::cache::SYNC_LINKS_OF.on(db).bind(("id", record_rid.clone())).await?.take(0)?;
    let stored: std::collections::HashSet<(String, String)> = stored.into_iter().map(|l| (l.out.to_string(), l.rel)).collect();
    let wanted: std::collections::HashSet<(String, String)> =
        links_spec.iter().map(|l| (rid(owner, &l.target).to_string(), l.rel.clone())).collect();
    Ok(stored == wanted)
}

/// The vault and route keys a write gives the record: a stored record keeps its vault (and, without
/// routing, its keys); see [`Placement`].
fn place(owner: &RecordId, existing: Option<&Row>, placement: Option<&Placement>) -> (Option<RecordId>, Option<Vec<String>>) {
    match (existing, placement) {
        (Some(e), Some(p)) => {
            let keep = e.vault.as_ref().is_none_or(|v| p.writable.contains(v));
            if !keep {
                tracing::info!(
                    owner = %owner.to_string(),
                    record = %e.id.to_string(),
                    "routing: the owner is no longer a member of the record's vault; re-filed where the rules route it now"
                );
            }
            (if keep { e.vault.clone() } else { p.vault.clone() }, Some(p.route_keys.clone()))
        }
        (Some(e), None) => (e.vault.clone(), e.route_keys.clone()),
        (None, Some(p)) => (p.vault.clone(), Some(p.route_keys.clone())),
        (None, None) => (None, None),
    }
}

/// Insert or update one envelope, scoped to `owner`. Returns `(record,
/// changed)`; `changed` is about the record's content only (it drives embedding and extraction), so a
/// replay that changes only `links` reconciles them and still reports unchanged.
pub async fn upsert(db: &OrgDb, owner: &RecordId, env: &Envelope) -> AppResult<(CacheRecord, bool)> {
    let record_rid = rid(owner, &env.id);
    let h = hash_envelope(env);

    let existing: Option<Row> = store::get(db, &record_rid).await?;
    let (vault, route_keys) = place(owner, existing.as_ref(), env.placement.as_ref());
    if let Some(existing) = existing
        && existing.content_hash == h && !existing.deleted {
            store::cache::TOUCH_INGESTED
                .on(db)
                .bind(("id", record_rid.clone()))
                .bind(("vault", vault.clone()))
                .bind(("route_keys", route_keys))
                .await?;
            // links are not in the content hash (a re-categorised transaction changes only them)
            if !links_unchanged(db, owner, &record_rid, &env.links).await? {
                reconcile_links(db, owner, &record_rid, &env.links).await?;
            }
            let mut rec = row_to_record(existing, owner);
            rec.ingested_at = Some(Datetime::from(chrono::Utc::now()));
            rec.vault = vault;
            return Ok((rec, false));
        }

    // Concurrent writers (other syncs, other users) can conflict on the shared indexes; the loser is
    // rejected as retryable, so retry it.
    let (now, payload) = (Datetime::from(chrono::Utc::now()), if env.payload.is_null() { json!({}) } else { env.payload.clone() });
    let rows: Vec<Row> = crate::tx::with_retry(|| async {
        let mut res = store::cache::UPSERT_RECORD
            .on(db)
            .bind(("id", record_rid.clone()))
            .bind(("owner", owner.clone()))
            .bind(("source", env.source.clone()))
            .bind(("type", env.type_.clone()))
            .bind(("external_id", env.external_id.clone()))
            .bind(("title", env.title.clone()))
            .bind(("body_text", env.body_text.clone()))
            .bind(("occurred_at", env.occurred_at))
            .bind(("url", env.url.clone()))
            // 3.x rejects NULL for the `object` field; an envelope without a payload means "none".
            .bind(("payload", payload.clone()))
            .bind(("content_hash", h.clone()))
            .bind(("ingested_at", now))
            .bind(("updated_at", now))
            .bind(("deleted", env.deleted))
            .bind(("vault", vault.clone()))
            .bind(("route_keys", route_keys.clone()))
            .await?;
        res.take(0)
    })
    .await?;
    let row = rows.into_iter().next().ok_or_else(|| AppError::internal("cache_record upsert returned no row"))?;
    let rec = row_to_record(row, owner);

    reconcile_links(db, owner, &record_rid, &env.links).await?;
    Ok((rec, true))
}

pub async fn set_embedding(db: &OrgDb, owner: &RecordId, record_id: &str, vector: Vec<f32>) -> AppResult<()> {
    if vector.len() != DIM {
        return Err(AppError::bad_request(format!("embedding dim {} != {DIM}", vector.len())));
    }
    // The embedding index is shared with concurrent syncs; a commit conflict is retryable.
    crate::tx::with_retry(|| async {
        store::cache::SET_EMBEDDING
            .on(db)
            .bind(("id", rid(owner, record_id)))
            .bind(("embedding", vector.clone()))
            .await?
            .check()
    })
    .await?;
    Ok(())
}

/// Words worth matching on, from a natural-language query: lowercase, split
/// on non-alphanumerics, minus filler words, deduped, at most 8. SurrealDB's
/// `@@` full-text match (and a whole-query substring scan) only hit when
/// EVERY word matches, so a question like "how do updates work?" found
/// nothing; callers instead match each term and rank with [`rank_term_hits`].
pub(crate) fn search_terms(q: &str) -> Vec<String> {
    const FILLER: &[&str] = &[
        "a", "an", "the", "and", "or", "but", "if", "of", "to", "in", "on", "at", "by", "for", "from", "with", "about",
        "into", "as", "is", "are", "was", "were", "be", "been", "am", "do", "does", "did", "done", "have", "has", "had",
        "i", "me", "my", "we", "us", "our", "you", "your", "he", "she", "it", "its", "they", "them", "their", "this",
        "that", "these", "those", "what", "which", "who", "whom", "whose", "when", "where", "why", "how", "can",
        "could", "would", "should", "will", "shall", "may", "might", "must", "so", "than", "then", "there", "here",
        "any", "some", "all", "not", "no", "yes", "just", "also", "please", "tell", "know", "remember", "recall",
        "anything", "something", "thing", "things", "get", "got", "like", "up", "out", "s", "t",
    ];
    let mut out: Vec<String> = Vec::new();
    for w in q.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
        if !w.is_empty() && !FILLER.contains(&w) && !out.iter().any(|o| o == w) {
            out.push(w.to_string());
        }
    }
    out.truncate(8);
    out
}

/// Fuses per-term hit lists (`(id, score)`, one list per term) into one
/// ranking: items matching more of the terms first, then by summed score.
pub(crate) fn rank_term_hits(per_term: Vec<Vec<(String, f64)>>, limit: usize) -> Vec<String> {
    let mut acc: HashMap<String, (usize, f64)> = HashMap::new();
    for hits in per_term {
        let mut seen = std::collections::HashSet::new();
        for (id, score) in hits {
            if seen.insert(id.clone()) {
                let e = acc.entry(id).or_insert((0, 0.0));
                e.0 += 1;
                e.1 += score;
            }
        }
    }
    let mut ranked: Vec<(String, (usize, f64))> = acc.into_iter().collect();
    ranked.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(b.1 .1.partial_cmp(&a.1 .1).unwrap_or(std::cmp::Ordering::Equal)).then(a.0.cmp(&b.0)));
    ranked.into_iter().take(limit).map(|(id, _)| id).collect()
}

/// Most results one `search`/`list` call may page through (`offset + limit`);
/// past that, narrow the query rather than page further.
pub(crate) const MAX_WINDOW: usize = 1000;

/// An ISO 8601 / RFC 3339 timestamp, or a bare `YYYY-MM-DD` date (midnight
/// UTC). Anything else is a 400 naming `field`, not a silent string compare.
pub(crate) fn parse_datetime(field: &str, s: &str) -> AppResult<chrono::DateTime<chrono::Utc>> {
    let s = s.trim();
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&chrono::Utc));
    }
    if let Some(dt) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().and_then(|d| d.and_hms_opt(0, 0, 0)) {
        return Ok(dt.and_utc());
    }
    Err(AppError::bad_request(format!("{field}: {s:?} is not an ISO 8601 date or datetime")))
}

type Utc = chrono::DateTime<chrono::Utc>;

/// A `since`/`until` pair, each optional; a reversed range is an error.
pub(crate) fn parse_range(
    since: Option<&str>,
    until: Option<&str>,
) -> AppResult<(Option<Utc>, Option<Utc>)> {
    let since = since.map(|s| parse_datetime("since", s)).transpose()?;
    let until = until.map(|s| parse_datetime("until", s)).transpose()?;
    if let (Some(a), Some(b)) = (since, until)
        && a > b
    {
        return Err(AppError::bad_request("since must not be after until"));
    }
    Ok((since, until))
}

/// Source/type/date restrictions, applied inside candidate generation rather
/// than to an already-truncated candidate list, so a match in a small source
/// can't be crowded out by other sources' hits. [`RecordFilter::sql`] only
/// references the set fields; [`RecordFilter::bind`] binds them.
#[derive(Debug, Clone, Default)]
pub struct RecordFilter {
    pub sources: Option<Vec<String>>,
    pub types: Option<Vec<String>>,
    pub since: Option<Datetime>,
    pub until: Option<Datetime>,
}

impl RecordFilter {
    fn sql(&self) -> String {
        let mut s = String::new();
        if self.sources.is_some() {
            s.push_str(" AND source IN $sources");
        }
        if self.types.is_some() {
            s.push_str(" AND type IN $types");
        }
        if self.since.is_some() {
            s.push_str(" AND occurred_at >= $since");
        }
        if self.until.is_some() {
            s.push_str(" AND occurred_at <= $until");
        }
        s
    }

    fn bind<'a>(&self, mut q: store::Q<'a>) -> store::Q<'a> {
        if let Some(v) = &self.sources {
            q = q.bind(("sources", v.clone()));
        }
        if let Some(v) = &self.types {
            q = q.bind(("types", v.clone()));
        }
        if let Some(v) = self.since {
            q = q.bind(("since", v));
        }
        if let Some(v) = self.until {
            q = q.bind(("until", v));
        }
        q
    }
}

/// `title` and `body_text` each have their own FULLTEXT index (a two-field index only resolves
/// the first field). Per search term: the title index (BM25-scored, +1 so a title hit beats a
/// body-only one) and the body index (its own BM25 score), both pre-filtered by `scope` and `filter`,
/// every term's two lookups in one round trip; fused by [`rank_term_hits`].
pub(crate) async fn keyword_ids(
    db: &OrgDb,
    scope: &RecordScope,
    q: &str,
    filter: &RecordFilter,
    limit: usize,
) -> AppResult<Vec<RecordId>> {
    #[derive(Deserialize, SurrealValue)]
    struct ScoredRow {
        id: RecordId,
        #[serde(default)]
        #[surreal(default)]
        score: f64,
    }

    let terms = search_terms(q);
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let cond = filter.sql();
    let scoped = RecordScope::SQL;
    let mut sql = String::new();
    for i in 0..terms.len() {
        sql.push_str(&format!(
            "SELECT id, search::score(1) AS score FROM cache_record \
             WHERE {scoped} AND title @1@ $t{i} AND deleted = false{cond} ORDER BY score DESC, id LIMIT $limit; \
             SELECT id, search::score(2) AS score FROM cache_record \
             WHERE {scoped} AND body_text @2@ $t{i} AND deleted = false{cond} ORDER BY score DESC, id LIMIT $limit;"
        ));
    }
    // dynamic: one pair of lookups per term, each with the filters that are set.
    let mut query = scope.bind(filter.bind(store::dynamic(db, "cache.keyword_ids", sql))).bind(("limit", limit as i64));
    for (i, t) in terms.into_iter().enumerate() {
        query = query.bind((format!("t{i}"), t));
    }
    let mut res = query.await?;
    let mut per_term = Vec::new();
    for i in 0..res.num_statements() / 2 {
        let title: Vec<ScoredRow> = res.take(2 * i)?;
        let body: Vec<ScoredRow> = res.take(2 * i + 1)?;
        per_term.push(
            title
                .into_iter()
                .map(|r| (key_of(&r.id), r.score + 1.0))
                .chain(body.into_iter().map(|r| (key_of(&r.id), r.score)))
                .collect(),
        );
    }
    Ok(rank_term_hits(per_term, limit).into_iter().map(|k| RecordId::from_table_key("cache_record", k)).collect())
}

/// Embed the query, then [`nearest_ids`].
pub(crate) async fn semantic_ids(
    db: &OrgDb,
    settings: &crate::config::Settings,
    scope: &RecordScope,
    q: &str,
    filter: &RecordFilter,
    limit: usize,
) -> AppResult<Vec<RecordId>> {
    let vec = crate::embeddings::service::embed(db, settings, &[q.to_string()], Some(scope.owner()))
        .await?
        .into_iter()
        .next()
        .unwrap_or_default();
    nearest_ids(db, scope, vec, filter, limit).await
}

/// The `limit` live records in `scope` matching `filter` closest to `vec`, best first. HNSW KNN
/// (`<|K,EF|>`, both literal integers: they cannot be bound parameters) answers first. The index is
/// shared by every owner and vault and the scope is not part of the ANN walk, so a small scope among big
/// ones (or a narrow filter) can get fewer than `limit` rows back; then an exact cosine scan over just
/// the scope's matching records is the answer (it is ground truth, and cheap exactly when the set is small).
pub async fn nearest_ids(db: &OrgDb, scope: &RecordScope, vec: Vec<f32>, filter: &RecordFilter, limit: usize) -> AppResult<Vec<RecordId>> {
    #[derive(Deserialize, SurrealValue)]
    struct IdRow {
        id: RecordId,
    }
    let ef = (limit * 2).max(64);
    let query = format!(
        "SELECT id FROM cache_record WHERE {} AND embedding <|{limit},{ef}|> $vec AND deleted = false{}",
        RecordScope::SQL,
        filter.sql()
    );
    // dynamic: the KNN `<|K,EF|>` operator needs literal integers, one SQL text per (K, EF).
    let mut res = scope.bind(filter.bind(store::dynamic(db, "cache.semantic_ids", query))).bind(("vec", vec.clone())).await?;
    let rows: Vec<IdRow> = res.take(0)?;
    if rows.len() >= limit {
        return Ok(rows.into_iter().map(|r| r.id).collect());
    }
    let exact = format!(
        "SELECT id, vector::similarity::cosine(embedding, $vec) AS sim FROM cache_record \
         WHERE {} AND deleted = false AND embedding != NONE{} ORDER BY sim DESC LIMIT $limit",
        RecordScope::SQL,
        filter.sql()
    );
    // dynamic: the exact fallback carries the same optional filters.
    let mut res =
        scope.bind(filter.bind(store::dynamic(db, "cache.semantic_exact", exact))).bind(("vec", vec)).bind(("limit", limit as i64)).await?;
    let rows: Vec<IdRow> = res.take(0)?;
    Ok(rows.into_iter().map(|r| r.id).collect())
}

/// Reciprocal Rank Fusion, k=60: sum of `1/(60+rank+1)` per id across any
/// number of ranked lists. Exposed (not just the fused order) so callers
/// that need the raw fused score to apply further boosts on top -- e.g.
/// `cache::recall`'s 5-arm pipeline -- don't reimplement this formula.
pub(crate) fn rrf_scores(ranked_lists: &[Vec<String>]) -> HashMap<String, f64> {
    let mut scores: HashMap<String, f64> = HashMap::new();
    for list in ranked_lists {
        for (rank, id) in list.iter().enumerate() {
            *scores.entry(id.clone()).or_insert(0.0) += 1.0 / (RRF_K + rank as f64 + 1.0);
        }
    }
    scores
}

/// [`rrf_scores`] as an order; ties by id so paging is stable.
fn rrf(ranked_lists: &[Vec<String>]) -> Vec<String> {
    let scores = rrf_scores(ranked_lists);
    let mut ids: Vec<String> = scores.keys().cloned().collect();
    ids.sort_by(|a, b| scores[b].partial_cmp(&scores[a]).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(b)));
    ids
}

/// Options for [`search`].
#[derive(Debug, Clone, Default)]
pub struct SearchParams {
    pub filter: RecordFilter,
    pub mode: String,
    pub limit: usize,
    pub offset: usize,
}

impl SearchParams {
    pub fn new() -> Self {
        SearchParams { mode: "hybrid".to_string(), limit: 20, offset: 0, ..Default::default() }
    }
}

/// One page of records in `scope` matching `q`, plus whether more exist. Filters are
/// pushed into every candidate query and candidates are fetched up to
/// `offset + limit + 1`, so later pages are reachable and `has_more` is
/// exact for the fused candidate list.
pub async fn search(
    db: &OrgDb,
    settings: &crate::config::Settings,
    scope: &RecordScope,
    q: &str,
    p: &SearchParams,
) -> AppResult<(Vec<CacheRecord>, bool)> {
    let window = p
        .offset
        .checked_add(p.limit)
        .filter(|w| *w <= MAX_WINDOW)
        .ok_or_else(|| AppError::bad_request(format!("offset + limit must be at most {MAX_WINDOW}")))?;
    let pool = window + 1;
    // Keyword candidates always come from the full window, so a page's ranking never depends on
    // `offset` and paging returns each match exactly once (the term-hit fusion reorders by pool).
    let kw_pool = MAX_WINDOW + 1;
    let f = &p.filter;
    let keys = |ids: Vec<RecordId>| ids.iter().map(key_of).collect::<Vec<_>>();
    // Without embeddings (no OpenAI key) every mode degrades to keyword search.
    let semantic_ok = crate::embeddings::service::available(db, settings, scope.owner()).await;
    let ids = match p.mode.as_str() {
        "semantic" if semantic_ok => keys(semantic_ids(db, settings, scope, q, f, pool).await?),
        "keyword" | "semantic" => keys(keyword_ids(db, scope, q, f, kw_pool).await?),
        _ if semantic_ok => {
            let kw = keys(keyword_ids(db, scope, q, f, kw_pool).await?);
            rrf(&[kw, keys(semantic_ids(db, settings, scope, q, f, pool).await.unwrap_or_default())])
        }
        _ => keys(keyword_ids(db, scope, q, f, kw_pool).await?),
    };

    let has_more = ids.len() > window;
    let page: Vec<RecordId> = ids.into_iter().skip(p.offset).take(p.limit).map(|k| RecordId::from_table_key("cache_record", k)).collect();
    Ok((get_many(db, scope, &page).await?, has_more))
}

/// The live records among `ids` that `scope` sees, in `ids` order: tombstoned (`deleted = true`),
/// missing and out-of-scope ids are dropped. One query for any number of ids; the one read path
/// behind `get`, `search` and recall hydration.
pub(crate) async fn get_many(db: &OrgDb, scope: &RecordScope, ids: &[RecordId]) -> AppResult<Vec<CacheRecord>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let res = scope.bind(store::cache::RECORDS_IN_SCOPE.on(db).bind(("ids", ids.to_vec()))).await;
    let rows: Vec<Row> = res?.take(0)?;
    #[allow(clippy::mutable_key_type)] // RecordId hashes by value; the interior mutability is never touched
    let mut by_id: HashMap<RecordId, Row> = rows.into_iter().map(|r| (r.id.clone(), r)).collect();
    Ok(ids.iter().filter_map(|i| by_id.remove(i)).map(|r| row_to_record(r, scope.owner())).collect())
}

/// One live record by its caller-facing id (see [`caller_id`]), if `scope` sees it; a tombstoned record
/// reads as not found.
pub async fn get(db: &OrgDb, scope: &RecordScope, record_id: &str) -> AppResult<Option<CacheRecord>> {
    Ok(get_many(db, scope, &candidates(scope.owner(), record_id)).await?.into_iter().next())
}

/// `owner`'s own live record, whatever vault it is filed in: for work done on the owner's behalf
/// (entity extraction), never for a read.
pub(crate) async fn get_own(db: &OrgDb, owner: &RecordId, record_id: &str) -> AppResult<Option<CacheRecord>> {
    let mut res = store::cache::RECORDS_BY_IDS.on(db).bind(("ids", vec![rid(owner, record_id)])).bind(("owner", owner.clone())).await?;
    let rows: Vec<Row> = res.take(0)?;
    Ok(rows.into_iter().next().map(|r| row_to_record(r, owner)))
}

/// Columns an agent may sort/filter `list` on -- anything else is a 400.
pub const FIELDS: &[&str] = &["occurred_at", "ingested_at", "updated_at", "title", "type", "source", "id", "external_id"];
const DATETIME_FIELDS: &[&str] = &["occurred_at", "ingested_at", "updated_at"];
/// `field__op` suffixes `list` filters understand; no suffix means equality.
const FILTER_OPS: &[(&str, &str)] = &[("ne", "!="), ("gt", ">"), ("gte", ">="), ("lt", "<"), ("lte", "<=")];

/// One validated `list` filter: `path` is a known column or a `payload.a.b`
/// path made only of `[A-Za-z0-9_]` segments, so it is safe to interpolate.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    path: String,
    op: &'static str,
    value: Value,
    datetime: bool,
}

/// Parses `{"field": v, "field__gte": v, "payload__a__b__lt": v, ...}`.
/// Datetime columns take ISO 8601 values and compare as datetimes;
/// `payload__<path>` targets that nested payload field; an unknown field or
/// operator is an error rather than a different query.
pub fn parse_filters(filters: &serde_json::Map<String, Value>) -> AppResult<Vec<Filter>> {
    let ops: Vec<&str> = FILTER_OPS.iter().map(|(name, _)| *name).collect();
    let mut out = Vec::new();
    for (key, value) in filters {
        let mut parts: Vec<&str> = key.split("__").collect();
        let op = match parts.last() {
            Some(last) if parts.len() > 1 => FILTER_OPS.iter().find(|(name, _)| name == last).map(|(_, sql)| *sql),
            _ => None,
        };
        if op.is_some() {
            parts.pop();
        }
        let (field, rest) = (parts[0], &parts[1..]);
        let path = if field == "payload" {
            let ok = |s: &&str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if rest.is_empty() || !rest.iter().all(ok) {
                return Err(AppError::bad_request(format!(
                    "filter {key:?}: payload filters name a field, e.g. payload__category or payload__amount_cents__lt"
                )));
            }
            format!("payload.{}", rest.join("."))
        } else if !FIELDS.contains(&field) {
            return Err(AppError::bad_request(format!(
                "unknown filter field {field:?}; use one of {FIELDS:?} or payload__<field>"
            )));
        } else if !rest.is_empty() {
            return Err(AppError::bad_request(format!(
                "filter {key:?}: unsupported operator {:?}; use one of {ops:?} (or none for equality)",
                rest.join("__")
            )));
        } else {
            field.to_string()
        };
        let datetime = DATETIME_FIELDS.contains(&field);
        let value = if datetime {
            let s = value
                .as_str()
                .ok_or_else(|| AppError::bad_request(format!("filter {key:?}: expected an ISO 8601 string")))?;
            Value::String(parse_datetime(key, s)?.to_rfc3339())
        } else {
            value.clone()
        };
        out.push(Filter { path, op: op.unwrap_or("="), value, datetime });
    }
    Ok(out)
}

/// One page of the live records in `scope` of `type_` matching `filters`, sorted
/// by `sort` (`-field` for DESC), plus the total match count.
pub async fn list(
    db: &OrgDb,
    scope: &RecordScope,
    type_: Option<&str>,
    filters: &[Filter],
    sort: &str,
    limit: usize,
    offset: usize,
) -> AppResult<(Vec<CacheRecord>, i64)> {
    let field = sort.strip_prefix('-').unwrap_or(sort);
    if !FIELDS.contains(&field) {
        return Err(AppError::bad_request(format!("unknown sort field {sort:?}; use one of {FIELDS:?}")));
    }
    let direction = if sort.starts_with('-') { "DESC" } else { "ASC" };

    let mut conditions = vec![RecordScope::SQL.to_string(), "deleted = false".to_string()];
    if type_.is_some() {
        conditions.push("type = $type".to_string());
    }
    for (i, f) in filters.iter().enumerate() {
        let param = if f.datetime { format!("<datetime>$f{i}") } else { format!("$f{i}") };
        conditions.push(format!("{} {} {param}", f.path, f.op));
    }
    let where_ = conditions.join(" AND ");
    // Ties (same occurred_at, or none) break by id, so offset pages never repeat or skip a record.
    let tiebreak = if field == "id" { String::new() } else { format!(", id {direction}") };
    let sql = format!(
        "SELECT * FROM cache_record WHERE {where_} ORDER BY {field} {direction}{tiebreak} LIMIT $limit START $offset; \
         SELECT count() FROM cache_record WHERE {where_} GROUP ALL"
    );
    // dynamic: caller-chosen filter fields, operators and sort column.
    let mut q = scope
        .bind(store::dynamic(db, "cache.list_records", sql))
        .bind(("type", type_.map(str::to_string)))
        .bind(("limit", limit as i64))
        .bind(("offset", offset as i64));
    for (i, f) in filters.iter().enumerate() {
        q = q.bind((format!("f{i}"), f.value.clone()));
    }
    let mut res = q.await?;
    let rows: Vec<Row> = res.take(0)?;

    #[derive(Deserialize, SurrealValue)]
    struct CountRow {
        count: i64,
    }
    let counts: Vec<CountRow> = res.take(1)?;
    Ok((rows.into_iter().map(|r| row_to_record(r, scope.owner())).collect(), counts.first().map(|r| r.count).unwrap_or(0)))
}

#[derive(Debug, Clone, Serialize)]
pub struct LinkEntry {
    pub rel: String,
    pub direction: &'static str,
    pub target_id: String,
}

/// The links of one record `scope` sees, to and from records it sees (or not yet synced): a tombstoned
/// endpoint hides the link, and a tombstoned record has no links at all.
pub async fn links(db: &OrgDb, scope: &RecordScope, record_id: &str, rel: Option<&str>) -> AppResult<Vec<LinkEntry>> {
    let mut out = Vec::new();
    let Some(record) = get(db, scope, record_id).await? else { return Ok(out) };
    let Some(record_rid) = record.rid else { return Ok(out) };

    #[derive(Deserialize, SurrealValue)]
    struct FwdRow {
        rel: String,
        out: RecordId,
    }
    #[derive(Deserialize, SurrealValue)]
    struct BackRow {
        rel: String,
        #[serde(rename = "in")]
        #[surreal(rename = "in")]
        in_: RecordId,
    }

    let fwd = if rel.is_some() { &store::cache::LINKS_OUT_REL } else { &store::cache::LINKS_OUT };
    let mut q = scope.bind(fwd.on(db).bind(("id", record_rid.clone())));
    if let Some(r) = rel {
        q = q.bind(("rel", r.to_string()));
    }
    let fwd: Vec<FwdRow> = q.await?.take(0)?;
    for row in fwd {
        out.push(LinkEntry { rel: row.rel, direction: "out", target_id: caller_id(&row.out, scope.owner()) });
    }

    let back = if rel.is_some() { &store::cache::LINKS_IN_REL } else { &store::cache::LINKS_IN };
    let mut q = scope.bind(back.on(db).bind(("id", record_rid)));
    if let Some(r) = rel {
        q = q.bind(("rel", r.to_string()));
    }
    let back: Vec<BackRow> = q.await?.take(0)?;
    for row in back {
        out.push(LinkEntry { rel: row.rel, direction: "in", target_id: caller_id(&row.in_, scope.owner()) });
    }

    Ok(out)
}

/// For each live record among `sources`, its live linked records in either
/// direction: one graph-traversal query for the whole batch (recall's graph arm).
/// Tombstoned sources and targets are skipped.
pub(crate) async fn live_neighbours(db: &OrgDb, sources: &[RecordId]) -> AppResult<HashMap<RecordId, Vec<RecordId>>> {
    if sources.is_empty() {
        return Ok(HashMap::new());
    }
    #[derive(Deserialize, SurrealValue)]
    struct NRow {
        id: RecordId,
        #[serde(default)]
        #[surreal(default)]
        fwd: Vec<RecordId>,
        #[serde(default)]
        #[surreal(default)]
        back: Vec<RecordId>,
    }
    let mut res = store::cache::LIVE_NEIGHBOURS.on(db).bind(("ids", sources.to_vec())).await?;
    let rows: Vec<NRow> = res.take(0)?;
    Ok(rows.into_iter().map(|r| (r.id, r.fwd.into_iter().chain(r.back).collect())).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_terms_drop_filler_and_keep_content_words() {
        assert_eq!(search_terms("How do updates work in Eunomia?"), vec!["updates", "work", "eunomia"]);
        assert_eq!(search_terms("What does Ada like?"), vec!["ada"]);
        assert_eq!(search_terms("the the THE"), Vec::<String>::new());
        assert_eq!(search_terms("v1.2.0 release notes"), vec!["v1", "2", "0", "release", "notes"]);
        assert_eq!(search_terms(&"word ".repeat(3)), vec!["word"]);
        assert_eq!(search_terms("a b c d e f g h i j k l m n o p").len(), 8);
    }

    #[test]
    fn rank_term_hits_prefers_items_matching_more_terms() {
        let per_term = vec![
            vec![("a".to_string(), 5.0), ("b".to_string(), 1.0)],
            vec![("b".to_string(), 1.0), ("c".to_string(), 9.0)],
        ];
        // b matched both terms, so it beats higher-scoring single-term hits
        assert_eq!(rank_term_hits(per_term, 10), vec!["b", "c", "a"]);
    }

    #[test]
    fn rank_term_hits_counts_a_term_once_per_item_and_truncates() {
        let per_term = vec![vec![("a".to_string(), 1.0), ("a".to_string(), 1.0), ("b".to_string(), 3.0)]];
        assert_eq!(rank_term_hits(per_term, 1), vec!["b"]);
        assert!(rank_term_hits(vec![], 5).is_empty());
    }

    fn env(title: &str, body: &str) -> Envelope {
        Envelope {
            id: "x:y:z".to_string(),
            source: "demo".to_string(),
            type_: "note".to_string(),
            external_id: "z".to_string(),
            title: title.to_string(),
            body_text: body.to_string(),
            ..Default::default()
        }
    }

    /// The expected value was computed from the 2.x text form, so this fails if the hash input
    /// drifts (datetime format, field order, `serde_json` key order) and would orphan stored hashes.
    #[test]
    fn hash_envelope_matches_the_value_stored_by_2x() {
        let e = Envelope {
            title: "Woolworths".into(),
            body_text: "Groceries $42.50".into(),
            url: "https://x/y".into(),
            occurred_at: Some("2026-01-05T09:00:00Z".parse().unwrap()),
            payload: serde_json::from_str(r#"{"zeta":1,"alpha":{"b":2,"a":1}}"#).unwrap(),
            ..Default::default()
        };
        assert_eq!(hash_envelope(&e), "86d1d37b57ed8b728294c14bb1fad628e4c94df7acac2e2836a152f6bf18ac41");
    }

    #[test]
    fn hash_envelope_is_deterministic() {
        let e = env("hello", "world");
        assert_eq!(hash_envelope(&e), hash_envelope(&e));
    }

    #[test]
    fn hash_envelope_changes_with_content() {
        assert_ne!(hash_envelope(&env("a", "b")), hash_envelope(&env("a", "c")));
    }

    #[test]
    fn hash_envelope_ignores_fields_outside_the_hashed_set() {
        let mut a = env("a", "b");
        let mut b = env("a", "b");
        a.id = "one".to_string();
        b.id = "two".to_string();
        a.source = "s1".to_string();
        b.source = "s2".to_string();
        assert_eq!(hash_envelope(&a), hash_envelope(&b));
    }

    #[test]
    fn rrf_scores_sums_reciprocal_ranks_across_lists() {
        let a = vec!["x".to_string(), "y".to_string()];
        let b = vec!["y".to_string(), "x".to_string()];
        let scores = rrf_scores(&[a, b]);
        // x: rank0 in a (1/61) + rank1 in b (1/62); y: rank1 in a (1/62) + rank0 in b (1/61)
        assert!((scores["x"] - scores["y"]).abs() < 1e-12);
    }

    #[test]
    fn rrf_orders_items_appearing_in_more_lists_first() {
        let a = vec!["x".to_string()];
        let b = vec!["x".to_string(), "y".to_string()];
        let c = vec!["y".to_string()];
        let order = rrf(&[a, b, c]);
        // x appears at rank0 twice; y appears at rank0 once + rank1 once --
        // x's score must be strictly higher.
        assert_eq!(order[0], "x");
    }

    fn filters(v: Value) -> AppResult<Vec<Filter>> {
        parse_filters(v.as_object().unwrap())
    }

    #[test]
    fn parse_filters_maps_operators_and_compares_dates_as_datetimes() {
        let f = filters(json!({"occurred_at__gte": "2024-01-01"})).unwrap();
        assert_eq!(f[0].path, "occurred_at");
        assert_eq!(f[0].op, ">=");
        assert!(f[0].datetime);
        assert_eq!(f[0].value, json!("2024-01-01T00:00:00+00:00"));
        assert_eq!(filters(json!({"title__ne": "x"})).unwrap()[0].op, "!=");
        assert_eq!(filters(json!({"source": "demo"})).unwrap()[0].op, "=");
        assert!(filters(json!({"occurred_at__lt": "last tuesday"})).is_err());
        assert!(filters(json!({"occurred_at": 5})).is_err());
    }

    #[test]
    fn parse_filters_targets_the_named_payload_field() {
        let f = filters(json!({"payload__category": "Groceries", "payload__a__b__lt": 3})).unwrap();
        let paths: Vec<(&str, &str)> = f.iter().map(|f| (f.path.as_str(), f.op)).collect();
        assert!(paths.contains(&("payload.category", "=")));
        assert!(paths.contains(&("payload.a.b", "<")));
        assert!(filters(json!({"payload": {"x": 1}})).is_err());
        assert!(filters(json!({"payload__a-b": 1})).is_err());
        assert!(filters(json!({"payload__x;DELETE": 1})).is_err());
    }

    #[test]
    fn parse_filters_rejects_unknown_fields_and_operators() {
        assert!(filters(json!({"title__contains": "x"})).is_err());
        assert!(filters(json!({"title__gte__lt": "x"})).is_err());
        assert!(filters(json!({"bogus": "x"})).is_err());
        assert!(filters(json!({})).unwrap().is_empty());
    }

    #[test]
    fn parse_range_validates_dates_and_order() {
        assert!(parse_range(Some("2024-01-01"), Some("2024-02-01T10:00:00Z")).is_ok());
        assert!(parse_range(Some("2024-03-01"), Some("2024-02-01")).is_err());
        assert!(parse_range(Some("yesterday"), None).is_err());
        assert_eq!(parse_range(None, None).unwrap(), (None, None));
    }

    #[test]
    fn record_filter_sql_only_mentions_set_fields() {
        assert_eq!(RecordFilter::default().sql(), "");
        let f = RecordFilter { sources: Some(vec!["demo".into()]), until: Some(Datetime::default()), ..Default::default() };
        assert_eq!(f.sql(), " AND source IN $sources AND occurred_at <= $until");
    }

    #[test]
    fn rrf_breaks_ties_by_id_for_stable_pages() {
        let a = vec!["b".to_string(), "a".to_string()];
        let b = vec!["a".to_string(), "b".to_string()];
        assert_eq!(rrf(&[a, b]), vec!["a", "b"]);
    }
}
