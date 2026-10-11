//! Generic read-only tools over the cache. The queries themselves live in
//! `cache::search` (one read path shared with recall); this module only
//! validates arguments and shapes the JSON.
//!
//! Every lookup is scoped by vault (`cache::search::RecordScope`): the caller's own records kept in
//! their personal vault, plus records a connector routed to a vault the caller is an active member
//! of. Each `cache_record`'s key is `"{owner_key}:{record_id}"`; the caller's own records are named
//! without the owner key, another owner's (in a shared vault) with it.
//! Tombstoned records (`deleted = true`) are invisible here: not returned by
//! `get`, not matched by `search`/`list`, and never a `links` endpoint.
//!
//! All strings may contain `[eunomia:*]` tokens -- callers treat them as
//! opaque handles.

use serde_json::{json, Value};
use surrealdb::types::RecordId;

use crate::cache::search::{self as cs, CacheRecord, RecordFilter, RecordScope, SearchParams};
use crate::error::AppResult;
use crate::pool::OrgDb;
use crate::vaults::service as vaults_service;

/// An id lookup (`get`, `links`) covers every vault the caller is an active member of.
async fn every_vault(db: &OrgDb, owner: &RecordId) -> AppResult<RecordScope> {
    cs::scope_for(db, owner, &vaults_service::accessible_vault_ids(db, owner).await?).await
}

const SNIPPET_LEN: usize = 200;

fn hit(rec: &CacheRecord) -> Value {
    let snippet: String = if rec.body_text.chars().count() > SNIPPET_LEN {
        let truncated: String = rec.body_text.chars().take(SNIPPET_LEN).collect();
        format!("{truncated}\u{2026}")
    } else {
        rec.body_text.clone()
    };
    let mut out = json!({
        "id": rec.id,
        "source": rec.source,
        "type": rec.type_,
        "title": rec.title,
        "snippet": snippet,
        "occurred_at": &rec.occurred_at,
        "url": if rec.url.is_empty() { Value::Null } else { Value::String(rec.url.clone()) },
    });
    if let Some(doc) = crate::documents::chunk_ref(&rec.id, &rec.source, &rec.payload) {
        out["document"] = doc;
    }
    out
}

#[allow(clippy::too_many_arguments)]
pub async fn search(
    db: &OrgDb,
    settings: &crate::config::Settings,
    owner: &RecordId,
    query: &str,
    sources: Option<&[String]>,
    types: Option<&[String]>,
    since: Option<&str>,
    until: Option<&str>,
    mode: &str,
    limit: i64,
    offset: i64,
    vault_ids: &[RecordId],
) -> AppResult<Value> {
    let (since, until) = cs::parse_range(since, until)?;
    let rs = vaults_service::read_scope(db, owner, vault_ids).await?;
    let scope = cs::RecordScope::vaults(owner, &rs.personal, &rs.vaults);
    let non_empty = |v: Option<&[String]>| v.filter(|v| !v.is_empty()).map(<[String]>::to_vec);
    let params = SearchParams {
        filter: RecordFilter {
            sources: non_empty(sources),
            types: non_empty(types),
            since: since.map(Into::into),
            until: until.map(Into::into),
        },
        mode: mode.to_string(),
        limit: limit.clamp(0, 100) as usize,
        offset: offset.max(0) as usize,
    };
    let (recs, has_more) = cs::search(db, settings, &scope, query, &params).await?;
    let results: Vec<Value> = recs.iter().map(hit).collect();
    Ok(json!({ "results": results, "has_more": has_more }))
}

pub async fn get(db: &OrgDb, owner: &RecordId, id: &str) -> AppResult<Value> {
    let scope = every_vault(db, owner).await?;
    let Some(rec) = cs::get(db, &scope, id).await? else {
        return Ok(json!({ "error": "not found" }));
    };
    let links = cs::links(db, &scope, &rec.id, None).await?;
    // A Pocket recording or transcript chunk: attach the whole stored recording (full transcript,
    // summary, action items, tags), its owner's (the record is in a vault the caller reads).
    let recording = match (rec.source.as_str(), &rec.owner) {
        ("heypocket", Some(record_owner)) => {
            let rid = rec.payload.get("recording_id").and_then(Value::as_str).unwrap_or(&rec.external_id);
            crate::sources::heypocket::stored(db, record_owner, rid).await?
        }
        _ => None,
    };
    let mut out = json!({
        "id": rec.id,
        "source": rec.source,
        "type": rec.type_,
        "external_id": rec.external_id,
        "title": rec.title,
        "body_text": rec.body_text,
        "occurred_at": &rec.occurred_at,
        "url": if rec.url.is_empty() { Value::Null } else { Value::String(rec.url.clone()) },
        "payload": rec.payload,
        "links": { "links": links },
    });
    if let Some(recording) = recording {
        out["recording"] = recording;
    }
    // A chunk of an uploaded document: which document, where in it, and how to fetch the original.
    if let Some(mut doc) = crate::documents::chunk_ref(&rec.id, &rec.source, &out["payload"]) {
        if let Some(d) = crate::documents::summary(db, owner, doc["document_id"].as_str().unwrap_or_default()).await? {
            doc["status"] = d.status.into();
            doc["current_revision"] = d.revision.into();
            doc["media_type"] = d.media_type.into();
            doc["size_bytes"] = d.size_bytes.into();
            doc["sha256"] = d.sha256.into();
        }
        out["document"] = doc;
    }
    Ok(out)
}

pub async fn list(
    db: &OrgDb,
    owner: &RecordId,
    type_: Option<&str>,
    filters: Option<&serde_json::Map<String, Value>>,
    sort: &str,
    limit: i64,
    offset: i64,
) -> AppResult<Value> {
    let filters = match filters {
        Some(f) => cs::parse_filters(f)?,
        None => Vec::new(),
    };
    let limit = limit.clamp(0, 200) as usize;
    let offset = offset.max(0) as usize;
    let rs = vaults_service::read_scope(db, owner, &[]).await?;
    let scope = cs::RecordScope::vaults(owner, &rs.personal, &rs.vaults);
    let (recs, total) = cs::list(db, &scope, type_, &filters, sort, limit, offset).await?;
    let results: Vec<Value> = recs.iter().map(hit).collect();
    let has_more = (offset + results.len()) < total as usize;
    Ok(json!({ "results": results, "total": total, "has_more": has_more }))
}

pub async fn links(db: &OrgDb, owner: &RecordId, id: &str, rel: Option<&str>) -> AppResult<Value> {
    Ok(json!({ "links": cs::links(db, &every_vault(db, owner).await?, id, rel).await? }))
}

/// The tool names these four functions are registered under in the
/// REST/MCP catalogue (see `tools::registry`): `"search"`, `"get"`,
/// `"list"`, `"links"`. All four are read-only.
pub const TOOL_NAMES: &[&str] = &["search", "get", "list", "links"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_truncates_long_bodies_into_a_snippet() {
        let rec = CacheRecord { id: "demo:x:1".into(), body_text: "y".repeat(500), ..Default::default() };
        let h = hit(&rec);
        assert_eq!(h["snippet"].as_str().unwrap().chars().count(), SNIPPET_LEN + 1);
        assert_eq!(h["url"], Value::Null);
    }

    #[test]
    fn tool_names_are_the_four_generic_cache_tools() {
        assert_eq!(TOOL_NAMES, &["search", "get", "list", "links"]);
    }
}
