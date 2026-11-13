//! Agent-debugging surface: fetch a failure capsule by trace id. Admin only (docs/debugging.md).

use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};

use crate::capsules::{self, Capsule};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::User;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/debug/capsules/{trace_id}", get(get_capsule)).route("/debug/metrics", get(get_metrics))
}

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct Metrics {
    /// Queries that ran without an org context since the process started: an unknown or unready org
    /// asked for, or a statement that reached for the other database's tables. Must stay 0; each one
    /// is also logged at error level.
    queries_without_org_context: u64,
    /// Org database handles currently open in this process (the pool is capped).
    open_org_handles: usize,
}

#[utoipa::path(
    operation_id = "getMetrics",
    get,
    path = "/api/debug/metrics",
    tag = "debug",
    summary = "Tenancy counters for this process (instance admin only)",
    responses((status = 200, body = Metrics), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_metrics(State(state): State<AppState>, user: User) -> AppResult<Json<Metrics>> {
    if !crate::authz::is_admin(&state.control, &user).await? {
        return Err(AppError::coded(ErrorCode::AuthForbidden, "Only an instance admin can read metrics."));
    }
    Ok(Json(Metrics {
        queries_without_org_context: crate::pool::NO_ORG_CONTEXT.load(std::sync::atomic::Ordering::Relaxed),
        open_org_handles: state.pool.open_handles(),
    }))
}

#[utoipa::path(
    operation_id = "getFailureCapsule",
    get,
    path = "/api/debug/capsules/{trace_id}",
    tag = "debug",
    summary = "Fetch the redacted failure capsules for a trace id, oldest first (instance admin only)",
    params(("trace_id" = String, Path)),
    responses((status = 200, body = Vec<Capsule>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_capsule(State(state): State<AppState>, user: User, Path(trace_id): Path<String>) -> AppResult<Json<Vec<Capsule>>> {
    if !crate::authz::is_admin(&state.control, &user).await? {
        return Err(AppError::coded(ErrorCode::AuthForbidden, "Only an instance admin can read failure capsules."));
    }
    // another org's capsule does not exist as far as this admin can tell (instance operators see all)
    let operator = crate::authz::is_operator(&state.control, &user).await?;
    let found: Vec<Capsule> = capsules::get_all(&state.control, &trace_id)
        .await?
        .into_iter()
        .filter(|c| operator || c.org.as_deref().is_none_or(|o| o == user.org.key()))
        .collect();
    if found.is_empty() {
        return Err(AppError::not_found(format!("No failure capsule for trace {trace_id}.")));
    }
    Ok(Json(found))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(get_capsule, get_metrics), components(schemas(Capsule, Metrics)))]
pub struct Doc;
