//! Sources dashboard data: the registered sources, each one's sync health,
//! and whether the caller has an enabled connector backing it.
//!
//! Also the webhook intake route ([`webhook_router`]) -- deliberately a
//! *separate* router with no `User` extractor, since an external provider
//! can't send our session cookie or bearer token. Owner is resolved from the
//! URL itself (`{owner_id}`, a `user:...` record id) and the request is only
//! trusted once `Source::webhook()` verifies it against that owner's stored
//! connector credentials, which is why the owner id lives in the URL. With several connections of one
//! kind, the delivery goes to the one whose own secret verifies it. Matches
//! `routers::connectors`'s dual-router pattern (`router()` + a second
//! function the caller mounts separately).

use surrealdb::types::SurrealValue;
use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use surrealdb::types::RecordId;
use crate::rid::RecordIdExt;

use crate::connectors::service::Connector;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::User;
use crate::sources::registry;
use crate::sources::scheduler;
use crate::state::{AppState, OrgState};
use crate::store;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/sources", get(list_sources))
        .route("/sources/status", get(sources_status))
        .route("/sources/{key}/sync", post(sync_now))
        .route("/sources/{key}/reextract", post(reextract))
}

/// No `User` extractor: see this module's doc comment.
pub fn webhook_router() -> Router<AppState> {
    Router::new().route("/sources/{key}/webhook/{owner_id}", post(source_webhook))
}

#[derive(Debug, Deserialize, SurrealValue)]
struct SyncStatusRow {
    id: RecordId,
    #[serde(default)]
    #[surreal(default)]
    cursor: String,
    #[serde(default)]
    #[surreal(default)]
    last_run: Option<surrealdb::types::Datetime>,
    #[serde(default)]
    #[surreal(default)]
    last_ok: Option<surrealdb::types::Datetime>,
    #[serde(default)]
    #[surreal(default)]
    last_error: String,
    #[serde(default)]
    #[surreal(default)]
    consecutive_failures: i64,
}

async fn sync_status_rows(state: &OrgState, owner: &RecordId) -> AppResult<std::collections::HashMap<String, SyncStatusRow>> {
    let mut res = store::app::SOURCES_SYNC_STATUS_LIST
        .on(&state.db)
        .bind(("owner", owner.clone()))
        .await?;
    let rows: Vec<SyncStatusRow> = res.take(0)?;
    Ok(rows
        .into_iter()
        .map(|r| {
            // The record key is `{owner_key}:{source_key}`; partition off the
            // owner-key prefix to recover the source key.
            let raw_key = crate::rid::key_string(r.id.key()).unwrap_or_default();
            let source_key = raw_key.split_once(':').map(|(_, k)| k.to_string()).unwrap_or(raw_key);
            (source_key, r)
        })
        .collect())
}

#[derive(Deserialize, SurrealValue)]
struct CountRow {
    source: String,
    #[serde(default)]
    #[surreal(default)]
    connector: Option<RecordId>,
    count: i64,
}

/// Cached-record counts per source and connection, for the dashboard's totals -- a single grouped
/// count, not a per-source query.
async fn record_counts(state: &OrgState, owner: &RecordId) -> AppResult<Vec<CountRow>> {
    let mut res = store::app::SOURCES_RECORD_COUNTS
        .on(&state.db)
        .bind(("owner", owner.clone()))
        .await?;
    Ok(res.take(0)?)
}

#[derive(Serialize, utoipa::ToSchema)]
struct SyncStatusOut {
    cursor: String,
    #[schema(value_type = Option<String>)]
    last_run: Option<surrealdb::types::Datetime>,
    #[schema(value_type = Option<String>)]
    last_ok: Option<surrealdb::types::Datetime>,
    last_error: String,
    consecutive_failures: i64,
}

/// One connection of a source. A kind's first connection keeps the source key as its `key`; each one
/// added after it is `{source}~{instance}`.
#[derive(Serialize, utoipa::ToSchema)]
struct SourceOut {
    key: String,
    label: &'static str,
    /// The connection's id (`connector:...`); absent for a source never set up.
    connector: Option<String>,
    /// The name the user gave the connection; "" if none.
    account: String,
    provider: &'static str,
    record_types: &'static [&'static str],
    connected: bool,
    sync_status: SyncStatusOut,
    record_count: i64,
}

/// Outcome of a sync or webhook ingest: an ingest report, `source` + `error` when the sync failed, or
/// `status: "already_running"` when a sync of that source is already queued or running, or
/// `status: "queued"` when it was queued but no worker finished it within the wait. Schema only:
/// the handlers pass the report through.
#[derive(Serialize, utoipa::ToSchema)]
#[allow(dead_code)]
struct SyncReport {
    source: String,
    /// `already_running`, `queued`, or `offline` (the connectivity check found no internet; nothing was attempted).
    status: Option<String>,
    written: Option<i64>,
    skipped: Option<i64>,
    failed: Option<i64>,
    errors: Option<Vec<String>>,
    error: Option<String>,
}

/// Body of the webhook route: `{"status": "ignored"}` or an ingest report with
/// `status: "ok"`. Schema only.
#[derive(Serialize, utoipa::ToSchema)]
#[allow(dead_code)]
struct WebhookOut {
    status: String,
    source: Option<String>,
    written: Option<i64>,
    skipped: Option<i64>,
    failed: Option<i64>,
    errors: Option<Vec<String>>,
}

fn status_out(row: Option<&SyncStatusRow>) -> SyncStatusOut {
    match row {
        None => SyncStatusOut { cursor: String::new(), last_run: None, last_ok: None, last_error: String::new(), consecutive_failures: 0 },
        Some(row) => SyncStatusOut {
            cursor: row.cursor.clone(),
            last_run: row.last_run,
            last_ok: row.last_ok,
            last_error: row.last_error.clone(),
            consecutive_failures: row.consecutive_failures,
        },
    }
}

#[utoipa::path(
    operation_id = "listSources",
    get,
    path = "/api/sources",
    tag = "sources",
    summary = "List sources with sync status",
    responses((status = 200, body = Vec<SourceOut>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_sources(State(state): State<AppState>, user: User) -> AppResult<Json<Vec<SourceOut>>> {
    let state = state.org(&user.org).await?;
    let statuses = sync_status_rows(&state, &user.id).await?;
    let enabled: Vec<RecordId> = registry::enabled(&state.db, &user.id).await?.into_iter().map(|(_, c)| c.id).collect();
    let counts = record_counts(&state, &user.id).await?;
    let mut res = store::app::CONNECTORS_OF_OWNER.on(&state.db).bind(("owner", user.id.clone())).await?;
    let mut connections: Vec<Connector> = res.take(0)?;
    connections.sort_by(|a, b| (&a.label, &a.instance).cmp(&(&b.label, &b.instance)));

    let mut out = Vec::new();
    for src in registry::all() {
        let mine: Vec<&Connector> = connections.iter().filter(|c| c.kind == src.provider_key()).collect();
        // the first connection's row even before it exists (as before 2.0.4), then each added one
        let first = mine.iter().copied().find(|c| c.is_first());
        let rows = std::iter::once(first).chain(mine.iter().copied().filter(|c| !c.is_first()).map(Some));
        for conn in rows {
            let key = conn.map_or_else(|| src.key().to_string(), |c| registry::sync_key(src.as_ref(), c));
            let is_first = conn.is_none_or(Connector::is_first);
            let record_count = counts
                .iter()
                .filter(|r| r.source == src.key())
                .filter(|r| match (&r.connector, conn) {
                    (Some(id), Some(c)) => *id == c.id,
                    (None, _) => is_first,
                    (Some(_), None) => false,
                })
                .map(|r| r.count)
                .sum();
            out.push(SourceOut {
                connected: conn.is_some_and(|c| enabled.contains(&c.id)),
                sync_status: status_out(statuses.get(&key)),
                key,
                label: src.label(),
                connector: conn.map(|c| c.id.to_string()),
                account: conn.map(|c| c.label.clone()).unwrap_or_default(),
                provider: src.provider_key(),
                record_types: src.record_types(),
                record_count,
            });
        }
    }
    Ok(Json(out))
}

#[utoipa::path(
    operation_id = "getSourcesStatus",
    get,
    path = "/api/sources/status",
    tag = "sources",
    summary = "Overall sync status",
    responses((status = 200, body = std::collections::HashMap<String, SyncStatusOut>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn sources_status(State(state): State<AppState>, user: User) -> AppResult<Json<std::collections::HashMap<String, SyncStatusOut>>> {
    let state = state.org(&user.org).await?;
    let statuses = sync_status_rows(&state, &user.id).await?;
    Ok(Json(statuses.iter().map(|(k, v)| (k.clone(), status_out(Some(v)))).collect()))
}

#[utoipa::path(
    operation_id = "syncSource",
    post,
    path = "/api/sources/{key}/sync",
    tag = "sources",
    summary = "Sync a source now",
    description = "`key` is a source key (its first connection) or a connection's key from `GET /api/sources` (`{source}~{instance}`).",
    params(("key" = String, Path)),
    responses((status = 200, body = SyncReport), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn sync_now(State(state): State<AppState>, user: User, Path(key): Path<String>) -> AppResult<Json<Value>> {
    let state = state.org(&user.org).await?;
    if registry::connection(&state.db, &user.id, &key).await?.is_none() {
        return Err(AppError::coded(ErrorCode::SourceNotFound, format!("no source {key:?}")));
    }
    Ok(Json(scheduler::sync_now(&state, &user.id, &key).await?))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct ReextractIn {
    /// Only count what would be replaced (the default); `false` deletes it and queues extraction again.
    #[serde(default = "default_true")]
    dry_run: bool,
}

fn default_true() -> bool {
    true
}

/// What a re-extract run replaces: the connection's live records, the facts and relations drawn from
/// them, and the observations resting only on those facts.
#[derive(Serialize, Deserialize, SurrealValue, utoipa::ToSchema)]
struct ReextractOut {
    records: i64,
    memories: i64,
    relations: i64,
    observations: i64,
}

#[utoipa::path(
    operation_id = "reextractSource",
    post,
    path = "/api/sources/{key}/reextract",
    tag = "sources",
    summary = "Re-run entity extraction over a source's records",
    description = "`key` is a source key or a connection's key, as for sync. Counts the caller's live records of that connection and the facts and relations extracted from them. With `dry_run` false it deletes those facts and relations (their subjects' observations go stale, or go when nothing else backs them) and queues extraction for every record again; it answers 409 while an earlier re-extraction of the source is still queued or running.",
    params(("key" = String, Path)),
    request_body = ReextractIn,
    responses((status = 200, body = ReextractOut), (status = 409, description = "A re-extraction of this source is still queued or running", body = crate::openapi::Problem, content_type = "application/problem+json"), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn reextract(State(state): State<AppState>, user: User, Path(key): Path<String>, Json(body): Json<ReextractIn>) -> AppResult<Json<ReextractOut>> {
    let state = state.org(&user.org).await?;
    let Some((_, conn)) = registry::connection(&state.db, &user.id, &key).await? else {
        return Err(AppError::coded(ErrorCode::SourceNotFound, format!("no source {key:?}")));
    };
    let source = registry::split_key(&key).0.to_string();
    // scoped like a connection's data delete: its own records, plus a first connection's older ones
    let first = conn.as_ref().is_none_or(Connector::is_first);
    let connector = conn.as_ref().map(|c| c.id.clone());
    macro_rules! scope {
        ($stmt:expr) => {
            $stmt.on(&state.db).bind(("owner", user.id.clone())).bind(("source", source.clone())).bind(("connector", connector.clone())).bind(("first", first))
        };
    }

    if body.dry_run {
        let mut res = scope!(store::cache::REEXTRACT_COUNTS).await?;
        let out = res.take::<Option<ReextractOut>>(store::cache::REEXTRACT_COUNTS.slot)?;
        return Ok(Json(out.unwrap_or(ReextractOut { records: 0, memories: 0, relations: 0, observations: 0 })));
    }

    // a run still working through its records would recreate what this one deletes, twice over
    let mut res = store::jobs::REEXTRACT_ACTIVE.on(&state.control).bind(("owner", user.id.clone())).bind(("src", key.clone())).await?;
    if res.take::<Option<RecordId>>(0)?.is_some() {
        return Err(AppError::coded(ErrorCode::DbConflict, format!("a re-extraction of {key:?} is still queued or running; wait for it to finish")));
    }

    #[derive(Deserialize, SurrealValue)]
    struct Rec {
        id: RecordId,
        content_hash: String,
    }
    #[derive(Deserialize, SurrealValue)]
    struct Cleared {
        records: Vec<Rec>,
        memories: i64,
        relations: i64,
        observations: i64,
    }
    let mut res = crate::tx::with_retry(|| async { scope!(store::cache::REEXTRACT_CLEAR).await }).await?;
    let cleared = res.take::<Option<Cleared>>(store::cache::REEXTRACT_CLEAR.slot)?;
    let Some(cleared) = cleared else { return Ok(Json(ReextractOut { records: 0, memories: 0, relations: 0, observations: 0 })) };
    let out = ReextractOut { records: cleared.records.len() as i64, memories: cleared.memories, relations: cleared.relations, observations: cleared.observations };

    // audited before returning, with the counts; the queueing below carries on in the background
    let detail = format!("records={} memories={} relations={} observations={}", out.records, out.memories, out.relations, out.observations);
    crate::audit::record_as_caller_detail(&state.control, &user.id, "source.reextract", &key, "ok", &detail).await;

    let (owner, src, nonce) = (user.id.clone(), key.clone(), uuid::Uuid::new_v4().simple().to_string());
    tokio::spawn(async move {
        for rec in &cleared.records {
            let id = crate::cache::search::literal(&rec.id);
            crate::jobs::handlers::enqueue_reextract(&state, &owner, &id, &rec.content_hash, &nonce, &src).await;
        }
    });
    Ok(Json(out))
}

/// Largest webhook delivery read into memory. Provider deliveries are a few KiB.
const MAX_WEBHOOK_BYTES: usize = 1 << 20;
/// A slow or stalled upload is cut off rather than holding a connection.
const WEBHOOK_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[utoipa::path(
    operation_id = "receiveSourceWebhook",
    post,
    path = "/api/sources/{key}/webhook/{owner_id}",
    tag = "sources",
    summary = "Source webhook (authenticated by the source's own signature)",
    params(("key" = String, Path), ("owner_id" = String, Path)),
    responses((status = 200, body = WebhookOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(()),
)]
async fn source_webhook(
    State(state): State<AppState>,
    Path((key, owner_id)): Path<(String, String)>,
    request: axum::extract::Request,
) -> AppResult<Json<Value>> {
    let Some(src) = registry::get(&key) else {
        return Err(AppError::coded(ErrorCode::SourceNotFound, format!("no source {key:?}")));
    };

    // Read the body (bounded) before any database work. An unknown owner answers exactly like a
    // delivery that does not verify, so the status cannot be used to find out which ids exist. The
    // signing secret lives in the owner's own database, so that database has to be opened to check it;
    // the per-address limit in the gate bounds how often that can be provoked.
    let (parts, body) = request.into_parts();
    let body_bytes = match tokio::time::timeout(WEBHOOK_READ_TIMEOUT, axum::body::to_bytes(body, MAX_WEBHOOK_BYTES)).await {
        Err(_) => {
            return Err(AppError::new(axum::http::StatusCode::REQUEST_TIMEOUT, "webhook body not received in time"));
        }
        Ok(Err(e)) => {
            let mut err = AppError::bad_request(format!("webhook body not readable or over {MAX_WEBHOOK_BYTES} bytes: {e}"));
            err.status = axum::http::StatusCode::PAYLOAD_TOO_LARGE;
            return Err(err);
        }
        Ok(Ok(bytes)) => bytes,
    };

    let ignored = || Ok(Json(json!({"status": "ignored"})));
    if !src.has_webhook() {
        return ignored();
    }
    let Ok(owner) = crate::rid::parse(&owner_id) else { return ignored() };
    let Ok(org) = crate::models_user::org_of(&state.control, &owner).await else { return ignored() };
    let state = state.org(&org).await?;

    // One URL serves every connection of the kind: the delivery belongs to the connection whose own
    // signing secret verifies it.
    let mut res = store::app::CONNECTORS_OF_KIND.on(&state.db).bind(("owner", owner.clone())).bind(("kind", src.provider_key().to_string())).await?;
    let mut connections: Vec<Connector> = res.take(0)?;
    connections.sort_by_key(|c| !c.is_first());
    let mut delivered = None;
    for row in connections {
        let conn = registry::conn_for(&state.settings.encryption_key, Some(&row))?;
        if let Some(records) = src.webhook(&conn, &parts.headers, &body_bytes).await? {
            delivered = Some((records, row));
            break;
        }
    }

    let Some((raw_records, row)) = delivered.filter(|(r, _)| !r.is_empty()) else {
        // Covers both "signature didn't verify" and "nothing worth
        // ingesting" -- `Source::webhook` doesn't distinguish the two (see
        // its doc comment), and responding 200 either way avoids the
        // provider retrying a delivery it has no reason to believe failed.
        tracing::info!(source = %key, owner = %owner.to_string(), "webhook: ignored");
        return Ok(Json(json!({"status": "ignored"})));
    };

    let report = registry::ingest_via(&state, &owner, &raw_records, src.as_ref(), Some(&row)).await?;
    tracing::info!(source = %key, connector = %row.id.to_string(), owner = %owner.to_string(), report = ?report.as_dict(), "webhook: processed");
    if report.failed > 0 {
        // Not stored: a 5xx makes the provider redeliver.
        return Err(AppError::new(axum::http::StatusCode::SERVICE_UNAVAILABLE, format!("{} records failed to save; retry", report.failed)));
    }

    let mut out = report.as_dict();
    out.as_object_mut().unwrap().insert("status".to_string(), json!("ok"));
    Ok(Json(out))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    list_sources,
    sources_status,
    sync_now,
    reextract,
    source_webhook,
))]
pub struct Doc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reextract_defaults_to_a_dry_run() {
        assert!(serde_json::from_str::<ReextractIn>("{}").unwrap().dry_run);
        assert!(!serde_json::from_str::<ReextractIn>(r#"{"dry_run": false}"#).unwrap().dry_run);
    }

    #[test]
    fn status_out_defaults_for_a_missing_row() {
        let v = serde_json::to_value(status_out(None)).unwrap();
        assert_eq!(v["cursor"], "");
        assert_eq!(v["consecutive_failures"], 0);
        assert!(v["last_run"].is_null());
    }
}
