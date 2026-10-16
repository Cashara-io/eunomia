//! Connector CRUD + per-connector data endpoints (finance summary, PocketAI
//! recordings) + the cross-connector dashboard snapshot. Owner-scoped, no
//! Google/Twenty/demo-mode.
//!
//! This module exposes two routers mounted separately:
//! `router` (prefixed `/connectors`) and `snapshot_router` (mounted bare, so
//! its one route lands at `/snapshot`). This file has that split as
//! [`router`] and [`snapshot_router`] -- the caller is responsible for
//! merging both into the app.

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use chrono::{Datelike, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::connectors::clients::{PocketAIClient, UpBankClient};
use crate::connectors::service::{self, Connector};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::User;
use crate::rid::RecordIdExt;
use crate::sources::registry;
use crate::sources::routing::{self, MoveReport, Routing};
use crate::state::{AppState, OrgState};

use super::schemas::{FinanceSummary, PocketaiSummary, SnapshotOut};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/connectors", get(list_all).post(create_one))
        .route("/connectors/up_bank/finance-summary", get(up_bank_finance_summary))
        .route("/connectors/pocketai/summary", get(pocketai_summary))
        .route("/connectors/pocketai/all", get(pocketai_all))
        .route("/connectors/pocketai/search", get(pocketai_search))
        .route("/connectors/pocketai/detail/{recording_id}", get(pocketai_detail))
        .route("/connectors/{connector}", get(get_one).put(put_one).delete(delete_one))
        .route("/connectors/{connector}/test", axum::routing::post(test_one))
        .route("/connectors/{connector}/data", axum::routing::delete(delete_data))
        .route("/connectors/{connector}/move", axum::routing::post(move_data))
        .route("/connectors/repoint", axum::routing::post(repoint))
}

pub fn snapshot_router() -> Router<AppState> {
    Router::new().route("/snapshot", get(snapshot))
}

/// One connection. A user may connect the same kind more than once (two Gmail accounts); each
/// connection has its own credentials, sync and records.
#[derive(Serialize, utoipa::ToSchema)]
struct ConnectorOut {
    /// The connection's id (`connector:...`). Routes taking `{connector}` accept it.
    id: String,
    kind: String,
    /// The name the user gave this connection (an email address, a device name); "" if none.
    label: String,
    /// This connection's key in `GET /api/sources` and `POST /api/sources/{key}/sync`: the source key for
    /// a kind's first connection, `{source}~{instance}` for each one added after it.
    source_key: String,
    enabled: bool,
    /// Connector-specific settings (open object).
    #[schema(value_type = Object)]
    config: Value,
    credentials_set: bool,
    #[schema(value_type = String)]
    updated_at: surrealdb::types::Datetime,
    /// Which vault its syncs file records in: a default destination and ordered rules.
    routing: Routing,
    /// The fields this connector's rules can match on.
    routing_fields: Vec<&'static str>,
}

fn connector_out(row: &Connector) -> ConnectorOut {
    let src = registry::for_provider(&row.kind);
    ConnectorOut {
        id: row.id.to_string(),
        kind: row.kind.clone(),
        label: row.label.clone(),
        source_key: src.as_ref().map(|s| registry::sync_key(s.as_ref(), row)).unwrap_or_else(|| row.kind.clone()),
        enabled: row.enabled,
        config: row.config.clone(),
        credentials_set: !row.credentials_encrypted.is_empty(),
        updated_at: row.updated_at,
        routing: Routing::of(&row.routing),
        routing_fields: src.map(|s| routing::fields(s.key()).to_vec()).unwrap_or_default(),
    }
}

#[derive(Serialize, utoipa::ToSchema)]
struct TestOut {
    ok: bool,
    /// Present only when the ping failed with an error.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// The source behind a connection.
fn source_of(row: &Connector) -> AppResult<std::sync::Arc<dyn crate::sources::base::Source>> {
    registry::for_provider(&row.kind).ok_or_else(|| AppError::coded(ErrorCode::ConnectorNotFound, "unknown connector"))
}

/// What `{connector}` in a path may be: a connection's id, or a kind (its first connection, which is
/// what these routes meant before a kind could be connected twice).
const CONNECTOR_PARAM: &str = "A connection id (`connector:...` or its key), or a connector kind for that kind's first connection.";

#[derive(Deserialize, utoipa::ToSchema)]
struct ConnectorCreateBody {
    kind: String,
    /// A name for the new connection, for example the account's email address or the device's name.
    label: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct ConnectorUpdateBody {
    enabled: Option<bool>,
    /// Renames the connection.
    label: Option<String>,
    config: Option<Value>,
    credentials: Option<Value>,
    /// Replaces the routing (destination vault and rules). It applies to records synced from now on;
    /// `POST /api/connectors/{connector}/move` re-files those already synced.
    routing: Option<Routing>,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct DaysQuery {
    #[serde(default = "default_days")]
    days: i64,
}

fn default_days() -> i64 {
    30
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct LimitQuery {
    #[serde(default = "default_limit")]
    limit: i64,
}

fn default_limit() -> i64 {
    50
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct SearchQuery {
    query: String,
}

#[utoipa::path(
    operation_id = "listConnectors",
    get,
    path = "/api/connectors",
    tag = "connectors",
    summary = "List connectors with their status",
    responses((status = 200, body = Vec<ConnectorOut>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_all(State(state): State<AppState>, user: User) -> AppResult<Json<Vec<ConnectorOut>>> {
    let state = state.org(&user.org).await?;
    let rows = service::list_connectors(&state.db, &user.id).await?;
    Ok(Json(rows.iter().map(connector_out).collect()))
}

#[utoipa::path(
    operation_id = "getUpBankFinanceSummary",
    get,
    path = "/api/connectors/up_bank/finance-summary",
    tag = "connectors",
    summary = "Up Bank finance summary",
    params(DaysQuery),
    responses((status = 200, body = FinanceSummary), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn up_bank_finance_summary(
    State(state): State<AppState>,
    user: User,
    Query(q): Query<DaysQuery>,
) -> AppResult<Json<Value>> {
    let state = state.org(&user.org).await?;
    // ponytail: the kind's first connection; take a `connector` query parameter if a second Up account needs its own summary
    let row = service::get_or_create_connector(&state.db, &user.id, "up_bank").await?;
    let creds = service::credentials_of(&state.settings.encryption_key, &row)?;
    let has_token = creds.get("personal_access_token").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
    if !row.enabled || !has_token {
        return Err(AppError::coded(ErrorCode::ConnectorNotConnected, "Up Bank is not connected"));
    }
    let since = Utc::now() - Duration::days(q.days);
    let summary = UpBankClient::new(&creds, &row.config)?.finance_summary(&since.to_rfc3339()).await?;
    Ok(Json(summary))
}

async fn pocketai_client_or_400(state: &OrgState, user: &User) -> AppResult<PocketAIClient> {
    // ponytail: the kind's first connection, as for the Up summary
    let row = service::get_or_create_connector(&state.db, &user.id, "pocketai").await?;
    let creds = service::credentials_of(&state.settings.encryption_key, &row)?;
    let has_key = creds.get("api_key").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
    if !row.enabled || !has_key {
        return Err(AppError::coded(ErrorCode::ConnectorNotConnected, "PocketAI is not connected"));
    }
    PocketAIClient::new(&creds, &row.config)
}

#[utoipa::path(
    operation_id = "getPocketaiSummary",
    get,
    path = "/api/connectors/pocketai/summary",
    tag = "connectors",
    summary = "PocketAI summary",
    params(DaysQuery),
    responses((status = 200, body = PocketaiSummary), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn pocketai_summary(
    State(state): State<AppState>,
    user: User,
    Query(q): Query<DaysQuery>,
) -> AppResult<Json<Value>> {
    let state = state.org(&user.org).await?;
    let client = pocketai_client_or_400(&state, &user).await?;
    let since = (Utc::now() - Duration::days(q.days)).date_naive().to_string();
    Ok(Json(client.summary(&since).await?))
}

#[utoipa::path(
    operation_id = "getPocketaiAll",
    // open body: raw PocketAI API response forwarded as-is
    get,
    path = "/api/connectors/pocketai/all",
    tag = "connectors",
    summary = "All PocketAI recordings",
    params(LimitQuery),
    responses((status = 200, body = Object), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn pocketai_all(
    State(state): State<AppState>,
    user: User,
    Query(q): Query<LimitQuery>,
) -> AppResult<Json<Value>> {
    let state = state.org(&user.org).await?;
    let client = pocketai_client_or_400(&state, &user).await?;
    Ok(Json(client.recordings(&[("limit", q.limit.to_string())]).await?))
}

#[utoipa::path(
    operation_id = "searchPocketai",
    // open body: raw PocketAI API response forwarded as-is
    get,
    path = "/api/connectors/pocketai/search",
    tag = "connectors",
    summary = "Search PocketAI recordings",
    params(SearchQuery),
    responses((status = 200, body = Object), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn pocketai_search(
    State(state): State<AppState>,
    user: User,
    Query(q): Query<SearchQuery>,
) -> AppResult<Json<Value>> {
    let state = state.org(&user.org).await?;
    let client = pocketai_client_or_400(&state, &user).await?;
    Ok(Json(client.search(&q.query).await?))
}

#[utoipa::path(
    operation_id = "getPocketaiDetail",
    // open body: raw PocketAI API response forwarded as-is
    get,
    path = "/api/connectors/pocketai/detail/{recording_id}",
    tag = "connectors",
    summary = "One PocketAI recording",
    params(("recording_id" = String, Path)),
    responses((status = 200, body = Object), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn pocketai_detail(
    State(state): State<AppState>,
    user: User,
    Path(recording_id): Path<String>,
) -> AppResult<Json<Value>> {
    let state = state.org(&user.org).await?;
    let client = pocketai_client_or_400(&state, &user).await?;
    Ok(Json(client.recording(&recording_id).await?))
}

#[utoipa::path(
    operation_id = "getConnector",
    get,
    path = "/api/connectors/{connector}",
    tag = "connectors",
    summary = "One connection",
    params(("connector" = String, Path, description = CONNECTOR_PARAM)),
    responses((status = 200, body = ConnectorOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_one(State(state): State<AppState>, user: User, Path(connector): Path<String>) -> AppResult<Json<ConnectorOut>> {
    let state = state.org(&user.org).await?;
    let row = service::resolve(&state.db, &user.id, &connector).await?;
    Ok(Json(connector_out(&row)))
}

#[utoipa::path(
    operation_id = "createConnector",
    post,
    path = "/api/connectors",
    tag = "connectors",
    summary = "Add another connection of a kind",
    description = "Creates a further, empty and disabled connection of `kind` (a second Gmail account, another Pocket device). Set it up with `PUT /api/connectors/{connector}` using the returned id.",
    request_body = ConnectorCreateBody,
    responses((status = 200, body = ConnectorOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn create_one(State(state): State<AppState>, user: User, Json(body): Json<ConnectorCreateBody>) -> AppResult<Json<ConnectorOut>> {
    let state = state.org(&user.org).await?;
    let row = service::create_connection(&state.db, &user.id, &body.kind, &body.label).await?;
    Ok(Json(connector_out(&row)))
}

#[derive(Serialize, utoipa::ToSchema)]
struct DeleteConnectorOut {
    /// Synced records removed.
    records: u64,
    /// Extracted facts removed.
    memories: u64,
}

/// Removes one connection with everything synced through it. Account-level: the gate refuses
/// vault-restricted tokens.
#[utoipa::path(
    operation_id = "deleteConnector",
    delete,
    path = "/api/connectors/{connector}",
    tag = "connectors",
    summary = "Remove a connection and its data",
    description = "Deletes the connection's credentials, sync status and everything synced through it (as `DELETE /api/connectors/{connector}/data`). Other connections of the same kind are untouched. A kind's first connection comes back empty and disconnected.",
    params(("connector" = String, Path, description = CONNECTOR_PARAM)),
    responses((status = 200, body = DeleteConnectorOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn delete_one(State(app): State<AppState>, user: User, Path(connector): Path<String>) -> AppResult<Json<DeleteConnectorOut>> {
    let state = app.org(&user.org).await?;
    let row = service::resolve(&state.db, &user.id, &connector).await?;
    let out = registry::delete_connection(&state.db, &user.id, &row).await?;
    crate::audit::record_as_caller(&app.control, &user.id, "connector.delete", &connector, "ok").await;
    Ok(Json(DeleteConnectorOut {
        records: out["records"].as_u64().unwrap_or(0),
        memories: out["memories"].as_u64().unwrap_or(0),
    }))
}

#[utoipa::path(
    operation_id = "updateConnector",
    put,
    path = "/api/connectors/{connector}",
    tag = "connectors",
    summary = "Update a connection",
    params(("connector" = String, Path, description = CONNECTOR_PARAM)),
    request_body = ConnectorUpdateBody,
    responses((status = 200, body = ConnectorOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn put_one(
    State(state): State<AppState>,
    user: User,
    Path(connector): Path<String>,
    Json(body): Json<ConnectorUpdateBody>,
) -> AppResult<Json<ConnectorOut>> {
    let state = state.org(&user.org).await?;
    let row = service::resolve(&state.db, &user.id, &connector).await?;
    let routing = match body.routing {
        Some(r) => Some(routing::validate(&state.db, &user.id, source_of(&row)?.key(), r).await?),
        None => None,
    };
    let mut row =
        service::update_connector(&state.db, &state.settings.encryption_key, row, body.enabled, body.config, body.credentials, body.label).await?;
    if let Some(r) = routing {
        routing::set(&state.db, &row.id, &r).await?;
        row.routing = serde_json::to_value(&r).unwrap_or_default();
    }
    Ok(Json(connector_out(&row)))
}

#[utoipa::path(
    operation_id = "testConnector",
    post,
    path = "/api/connectors/{connector}/test",
    tag = "connectors",
    summary = "Test a connection's credentials",
    params(("connector" = String, Path, description = CONNECTOR_PARAM)),
    responses((status = 200, body = TestOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn test_one(State(state): State<AppState>, user: User, Path(connector): Path<String>) -> AppResult<Json<TestOut>> {
    let state = state.org(&user.org).await?;
    let row = service::resolve(&state.db, &user.id, &connector).await?;
    let src = source_of(&row)?;
    let conn = registry::conn_for(&state.settings.encryption_key, Some(&row))?;
    match src.check(&conn).await {
        Ok(()) => Ok(Json(TestOut { ok: true, error: None })),
        Err(err) => Ok(Json(TestOut { ok: false, error: Some(err.message) })),
    }
}

#[derive(Serialize, utoipa::ToSchema)]
struct DeleteDataOut {
    /// Synced records removed.
    records: u64,
    /// Extracted facts removed.
    memories: u64,
}

/// Deletes all of the caller's data from one connection (see `registry::delete_data`); the connection
/// itself stays. Account-level: the gate refuses vault-restricted tokens.
#[utoipa::path(
    operation_id = "deleteConnectorData",
    delete,
    path = "/api/connectors/{connector}/data",
    tag = "connectors",
    summary = "Delete everything synced from a connection",
    description = "Removes the records synced through this connection, their links, the facts and relations extracted from them, and (Pocket) the stored recordings. Observations built from removed facts go stale. Entities, the credentials and the sync cursor stay, and so does everything synced through other connections of the same kind.",
    params(("connector" = String, Path, description = CONNECTOR_PARAM)),
    responses((status = 200, body = DeleteDataOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn delete_data(State(app): State<AppState>, user: User, Path(connector): Path<String>) -> AppResult<Json<DeleteDataOut>> {
    let state = app.org(&user.org).await?;
    let row = service::resolve(&state.db, &user.id, &connector).await?;
    let out = registry::delete_data(&state.db, &user.id, &row).await?;
    crate::audit::record_as_caller(&app.control, &user.id, "connector.delete_data", &connector, "ok").await;
    Ok(Json(DeleteDataOut {
        records: out["records"].as_u64().unwrap_or(0),
        memories: out["memories"].as_u64().unwrap_or(0),
    }))
}

/// "Move existing data": re-files this connector's already-synced records, and the facts and relations
/// drawn from them, to the vault its current routing sends them to. Account-level (the gate refuses
/// vault-restricted tokens).
#[utoipa::path(
    operation_id = "moveConnectorData",
    post,
    path = "/api/connectors/{connector}/move",
    tag = "connectors",
    summary = "Re-file a connection's synced data by its current routing",
    description = "Changing a connector's destination vault or rules applies to records synced afterwards. This moves the records already synced (and the facts and relations extracted from them) to where the current routing sends them. Idempotent: a record already in the right vault is left alone, and a fact the destination already has is not duplicated.",
    params(("connector" = String, Path, description = CONNECTOR_PARAM)),
    responses((status = 200, body = MoveReport), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn move_data(State(app): State<AppState>, user: User, Path(connector): Path<String>) -> AppResult<Json<MoveReport>> {
    let state = app.org(&user.org).await?;
    let row = service::resolve(&state.db, &user.id, &connector).await?;
    let report = routing::move_existing(&state, &user.id, source_of(&row)?.as_ref(), &row).await?;
    crate::audit::record_as_caller(&app.control, &user.id, "connector.move_data", &connector, "ok").await;
    Ok(Json(report))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct RepointBody {
    /// Vaults connectors may name now (e.g. the two vaults just merged).
    from: Vec<String>,
    /// The vault to name instead (e.g. the merged vault).
    to: String,
}

#[derive(Serialize, utoipa::ToSchema)]
struct RepointOut {
    /// Connectors whose default or rules changed.
    connectors: usize,
}

/// After a vault merge: point every connector of the caller that files records in one of `from` at
/// `to` instead. Applies to future syncs, like any routing change.
#[utoipa::path(
    operation_id = "repointConnectors",
    post,
    path = "/api/connectors/repoint",
    tag = "connectors",
    summary = "Point connectors at another vault",
    request_body = RepointBody,
    responses((status = 200, body = RepointOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn repoint(State(app): State<AppState>, user: User, Json(body): Json<RepointBody>) -> AppResult<Json<RepointOut>> {
    let state = app.org(&user.org).await?;
    let to = crate::rid::parse(&body.to).map_err(|_| AppError::bad_request(format!("invalid vault id {:?}", body.to)))?;
    let from: Vec<surrealdb::types::RecordId> = body.from.iter().filter_map(|v| crate::rid::parse(v).ok()).collect();
    let connectors = routing::repoint(&state.db, &user.id, &from, &to).await?;
    Ok(Json(RepointOut { connectors }))
}

/// Best-effort figures for each connected account, read live from each
/// client. A connector that isn't connected, or whose call fails, comes back
/// as null rather than failing the whole request.
#[utoipa::path(
    operation_id = "getSnapshot",
    get,
    path = "/api/snapshot",
    tag = "connectors",
    summary = "Combined snapshot of connector data",
    responses((status = 200, body = SnapshotOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn snapshot(State(state): State<AppState>, user: User) -> AppResult<Json<Value>> {
    let state = state.org(&user.org).await?;
    let mut result = json!({ "up_bank": Value::Null, "pocketai": Value::Null });

    let now = Utc::now();
    let week_start_date = now.date_naive() - Duration::days(now.weekday().num_days_from_monday() as i64);
    let week_start = week_start_date.and_hms_opt(0, 0, 0).expect("midnight is always valid").and_utc();

    let up_bank = service::get_or_create_connector(&state.db, &user.id, "up_bank").await?;
    let up_bank_creds = service::credentials_of(&state.settings.encryption_key, &up_bank)?;
    let has_token =
        up_bank_creds.get("personal_access_token").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
    if up_bank.enabled
        && has_token
        && let Ok(client) = UpBankClient::new(&up_bank_creds, &up_bank.config)
        && let Ok(summary) = client.week_summary(&week_start.to_rfc3339()).await
    {
        result["up_bank"] = summary;
    }

    let pocketai = service::get_or_create_connector(&state.db, &user.id, "pocketai").await?;
    let pocketai_creds = service::credentials_of(&state.settings.encryption_key, &pocketai)?;
    let has_key = pocketai_creds.get("api_key").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
    if pocketai.enabled && has_key {
        let since = (now - Duration::days(7)).date_naive().to_string();
        if let Ok(client) = PocketAIClient::new(&pocketai_creds, &pocketai.config)
            && let Ok(summary) = client.summary(&since).await
        {
            result["pocketai"] = json!({ "recordings_count": summary["recordings_count"] });
        }
    }

    Ok(Json(result))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    list_all,
    up_bank_finance_summary,
    pocketai_summary,
    pocketai_all,
    pocketai_search,
    pocketai_detail,
    get_one,
    create_one,
    delete_one,
    put_one,
    test_one,
    delete_data,
    move_data,
    repoint,
    snapshot,
))]
pub struct Doc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connector_out_reports_credentials_set_without_leaking_them() {
        let row = Connector {
            id: crate::rid::parse("connector:abc").unwrap(),
            owner: crate::rid::parse("user:abc").unwrap(),
            kind: "up_bank".to_string(),
            instance: String::new(),
            label: String::new(),
            enabled: true,
            config: json!({}),
            credentials_encrypted: "ciphertext".to_string(),
            updated_at: chrono::Utc::now().into(),
            routing: json!({}),
        };
        let out = serde_json::to_value(connector_out(&row)).unwrap();
        assert_eq!(out["credentials_set"], true);
        assert!(out.get("credentials_encrypted").is_none());
        assert!(out.get("credentials").is_none());
    }

    #[test]
    fn every_known_kind_has_a_source_to_test_with() {
        for kind in service::CONNECTOR_KINDS {
            assert!(registry::for_provider(kind).is_some(), "{kind}");
        }
    }
}
