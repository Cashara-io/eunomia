//! Secret requests and the secrets Eunomia holds (docs/architecture/specs/secret-requests.md): the routes
//! the pop-up and the Connectors page use. Account-level (`gate.rs`, `/api/secrets/...`), so a
//! vault-restricted token never gets here; filling, declining, replacing and deleting also need a browser
//! session, so an agent's own credential can never answer its own request. No route returns a value.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use surrealdb::types::{Datetime, RecordId, SurrealValue};
use zeroize::Zeroizing;

use crate::authz;
use crate::error::{AppError, AppResult};
use crate::models_user::User;
use crate::openapi::OkBody;
use crate::rid::RecordIdExt;
use crate::secrets::requests::{self, FillBody, FillOut, HeldOut, RequestOut};
use crate::state::AppState;
use crate::store;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/secrets/requests", get(list_requests))
        .route("/secrets/requests/{id}", get(get_request))
        .route("/secrets/requests/{id}/fill", post(fill_request))
        .route("/secrets/requests/{id}/decline", post(decline_request))
        .route("/secrets/held", get(list_held))
        .route("/secrets/held/{id}", axum::routing::put(replace_held).delete(delete_held))
        .route("/secrets/activity", get(activity))
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(list_requests, get_request, fill_request, decline_request, list_held, replace_held, delete_held, activity),
    components(schemas(RequestOut, FillBody, FillOut, DeclineOut, HeldOut, ReplaceBody, ReplaceOut, ActivityEntry, ActivityPage))
)]
pub struct Doc;

/// An org owner or instance admin (who manages every vault's requests and secrets they belong to).
async fn org_manager(state: &AppState, user: &User) -> AppResult<bool> {
    if authz::is_admin(&state.control, user).await? {
        return Ok(true);
    }
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        role: String,
    }
    let rows: Vec<Row> =
        store::control::MEMBERSHIP_ROLE.on(&state.control).bind(("user", user.id.clone())).bind(("org", user.org.record())).await?.check()?.take(0)?;
    Ok(rows.iter().any(|r| r.role == "owner"))
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    /// `pending`, `filled`, `declined` or `expired`; absent for all.
    status: Option<String>,
}

#[utoipa::path(
    operation_id = "listSecretRequests",
    get,
    path = "/api/secrets/requests",
    tag = "secrets",
    summary = "Secret requests you asked for or manage, newest first (never a value)",
    params(ListQuery),
    responses((status = 200, body = Vec<RequestOut>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_requests(State(state): State<AppState>, user: User, Query(q): Query<ListQuery>) -> AppResult<Json<Vec<RequestOut>>> {
    let status = match q.status.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(s @ ("pending" | "filled" | "declined" | "expired")) => Some(s.to_string()),
        Some(_) => return Err(AppError::bad_request("status is pending, filled, declined or expired")),
    };
    let manager = org_manager(&state, &user).await?;
    let org = state.org(&user.org).await?;
    Ok(Json(requests::list(&org, &user.id, manager, status).await?))
}

#[utoipa::path(
    operation_id = "getSecretRequest",
    get,
    path = "/api/secrets/requests/{id}",
    tag = "secrets",
    summary = "One secret request (the standalone fill page)",
    params(("id" = String, Path, description = "`secret_request:...` or its key")),
    responses((status = 200, body = RequestOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_request(State(state): State<AppState>, user: User, Path(id): Path<String>) -> AppResult<Json<RequestOut>> {
    let manager = org_manager(&state, &user).await?;
    let org = state.org(&user.org).await?;
    Ok(Json(requests::get(&org, &user.id, manager, &id).await?))
}

#[utoipa::path(
    operation_id = "fillSecretRequest",
    post,
    path = "/api/secrets/requests/{id}/fill",
    tag = "secrets",
    summary = "Paste the secret for a request (the pop-up's Save)",
    description = "Only the person whose agent asked, in a browser session. The value is encrypted (AES-256-GCM under ENCRYPTION_KEY) before it is stored and is never returned; the answer holds the handle only.",
    params(("id" = String, Path)),
    request_body = FillBody,
    responses((status = 200, body = FillOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn fill_request(State(state): State<AppState>, user: User, Path(id): Path<String>, Json(body): Json<FillBody>) -> AppResult<Json<FillOut>> {
    let manager = org_manager(&state, &user).await?;
    let org = state.org(&user.org).await?;
    Ok(Json(requests::fill(&org, &user.id, manager, &id, body).await?))
}

#[derive(Serialize, utoipa::ToSchema)]
struct DeclineOut {
    status: String,
}

#[utoipa::path(
    operation_id = "declineSecretRequest",
    post,
    path = "/api/secrets/requests/{id}/decline",
    tag = "secrets",
    summary = "Decline a secret request",
    params(("id" = String, Path)),
    responses((status = 200, body = DeclineOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn decline_request(State(state): State<AppState>, user: User, Path(id): Path<String>) -> AppResult<Json<DeclineOut>> {
    let manager = org_manager(&state, &user).await?;
    let org = state.org(&user.org).await?;
    Ok(Json(DeclineOut { status: requests::decline(&org, &user.id, manager, &id).await? }))
}

#[utoipa::path(
    operation_id = "listHeldSecrets",
    get,
    path = "/api/secrets/held",
    tag = "secrets",
    summary = "Secrets Eunomia holds that you filled or manage (never a value)",
    responses((status = 200, body = Vec<HeldOut>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_held(State(state): State<AppState>, user: User) -> AppResult<Json<Vec<HeldOut>>> {
    let manager = org_manager(&state, &user).await?;
    let org = state.org(&user.org).await?;
    Ok(Json(requests::list_held(&org, &user.id, manager).await?))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct ReplaceBody {
    /// The new value. Write-only.
    #[schema(value_type = String)]
    value: Zeroizing<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct ReplaceOut {
    ok: bool,
    /// How many secrets derived from the old value were deleted.
    removed_derived: usize,
}

#[utoipa::path(
    operation_id = "replaceHeldSecret",
    put,
    path = "/api/secrets/held/{id}",
    tag = "secrets",
    summary = "Replace a held secret's value (its derived secrets are deleted)",
    params(("id" = String, Path)),
    request_body = ReplaceBody,
    responses((status = 200, body = ReplaceOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn replace_held(State(state): State<AppState>, user: User, Path(id): Path<String>, Json(body): Json<ReplaceBody>) -> AppResult<Json<ReplaceOut>> {
    let manager = org_manager(&state, &user).await?;
    let org = state.org(&user.org).await?;
    let removed_derived = requests::replace_held(&org, &user.id, manager, &id, &body.value).await?;
    Ok(Json(ReplaceOut { ok: true, removed_derived }))
}

#[utoipa::path(
    operation_id = "deleteHeldSecret",
    delete,
    path = "/api/secrets/held/{id}",
    tag = "secrets",
    summary = "Delete a held secret and every secret derived from it",
    params(("id" = String, Path)),
    responses((status = 200, body = OkBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn delete_held(State(state): State<AppState>, user: User, Path(id): Path<String>) -> AppResult<Json<OkBody>> {
    let manager = org_manager(&state, &user).await?;
    let org = state.org(&user.org).await?;
    let removed = requests::delete_held(&org, &user.id, manager, &id).await?;
    let warning = (removed > 0).then(|| format!("{removed} derived secret(s) were deleted with it."));
    Ok(Json(OkBody { ok: true, warning }))
}

// ---------------------------------------------------------------- activity (the resolution log)

/// Every `secret.*` action the log shows.
const ACTIONS: &[&str] = &["secret.resolve", "secret.request", "secret.fill", "secret.decline", "secret.transform", "secret.http", "secret.replace", "secret.delete"];

#[derive(Deserialize, SurrealValue)]
struct EventRow {
    id: RecordId,
    action: String,
    actor_kind: String,
    actor_id: String,
    outcome: String,
    trace_id: String,
    detail: String,
    created_at: Datetime,
}

#[derive(Serialize, utoipa::ToSchema)]
struct ActivityEntry {
    id: String,
    #[schema(value_type = String)]
    at: Datetime,
    /// `secret.resolve`, `secret.request`, `secret.fill`, `secret.decline`, `secret.transform`, `secret.http`, `secret.replace` or `secret.delete`.
    action: String,
    actor_kind: String,
    actor_id: String,
    /// The reference or handle concerned (for a request, the handle it will fill).
    reference: String,
    ref_id: String,
    tool: String,
    channel: String,
    session: String,
    vault_id: String,
    outcome: String,
    reason: String,
    /// For a transform: the held secrets it was derived from; for an outbound call: the host.
    lineage: Vec<String>,
    host: String,
    trace_id: String,
}

#[derive(Serialize, utoipa::ToSchema)]
struct ActivityPage {
    entries: Vec<ActivityEntry>,
    next_before: Option<String>,
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ActivityQuery {
    /// 1 to 200, default 50.
    limit: Option<i64>,
    /// An RFC 3339 time: entries strictly older.
    before: Option<String>,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn entry(r: EventRow) -> ActivityEntry {
    let d: Value = serde_json::from_str(&r.detail).unwrap_or(Value::Null);
    let reference = [s(&d, "ref"), s(&d, "handle")].into_iter().find(|x| !x.is_empty()).unwrap_or_else(|| {
        // a request names its slot by vault and name, not by handle
        let (v, n) = (s(&d, "vault"), s(&d, "name"));
        crate::rid::parse(&v).ok().filter(|_| !n.is_empty()).map(|v| crate::secrets::held::handle_for(&v, &n)).unwrap_or_default()
    });
    let lineage = d.get("lineage").or_else(|| d.get("handles")).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    ActivityEntry {
        id: r.id.to_string(),
        at: r.created_at,
        tool: match r.action.as_str() {
            "secret.transform" => "secret_transform".into(),
            "secret.http" => "http_request".into(),
            _ => s(&d, "tool"),
        },
        action: r.action,
        actor_kind: r.actor_kind,
        actor_id: r.actor_id,
        reference,
        ref_id: [s(&d, "ref_id"), s(&d, "secret"), s(&d, "request")].into_iter().find(|x| !x.is_empty()).unwrap_or_default(),
        channel: s(&d, "channel"),
        session: s(&d, "session"),
        vault_id: s(&d, "vault"),
        outcome: r.outcome,
        reason: s(&d, "reason"),
        lineage,
        host: s(&d, "host"),
        trace_id: r.trace_id,
    }
}

#[utoipa::path(
    operation_id = "listSecretActivity",
    get,
    path = "/api/secrets/activity",
    tag = "secrets",
    summary = "Every secret request, fill, decline, use, transform and outbound call in this organisation, newest first (never a value)",
    params(ActivityQuery),
    responses((status = 200, body = ActivityPage), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn activity(State(state): State<AppState>, user: User, Query(q): Query<ActivityQuery>) -> AppResult<Json<ActivityPage>> {
    authz::require_unrestricted()?;
    if !org_manager(&state, &user).await? {
        return Err(AppError::coded(crate::error::ErrorCode::AuthForbidden, "Only an owner of this organisation or an instance admin can read the secret activity log."));
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let before: Option<Datetime> = match q.before.as_deref() {
        None | Some("") => None,
        Some(b) => Some(Datetime::from(chrono::DateTime::parse_from_rfc3339(b).map_err(|_| AppError::bad_request("before is an RFC 3339 time"))?.with_timezone(&chrono::Utc))),
    };
    let rows: Vec<EventRow> = store::secret_requests::ACTIVITY
        .on(&state.control)
        .bind(("actions", ACTIONS.iter().map(|a| a.to_string()).collect::<Vec<_>>()))
        .bind(("org", user.org.key()))
        .bind(("before", before))
        .bind(("limit", limit + 1))
        .await?
        .check()?
        .take(0)?;
    let more = rows.len() as i64 > limit;
    let entries: Vec<ActivityEntry> = rows.into_iter().take(limit as usize).map(entry).collect();
    let next_before = if more { entries.last().map(|e| chrono::DateTime::<chrono::Utc>::from(e.at.clone()).to_rfc3339()) } else { None };
    Ok(Json(ActivityPage { entries, next_before }))
}
