//! Self-update check/trigger -- reads/writes files in
//! `settings.update_status_dir` rather than running `git`/`docker` itself.
//! The privileged work (git, docker) happens in the separate `updater`
//! service, never in this web-facing process.
//!
//! Not configured (the directory doesn't exist) is a normal, expected
//! state, surfaced as `{"configured": false}` rather than an error.

use std::path::PathBuf;

use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use serde_json::{json, Value};

use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::User;
use crate::state::AppState;

/// Documents the body of `GET /update/status`. The handler merges the on-disk
/// `status.json` written by `scripts/auto-update.sh` verbatim, so every field
/// except `configured` is absent when the updater is not configured.
#[derive(Serialize, utoipa::ToSchema)]
#[allow(dead_code)] // schema only: the handler passes the file's JSON through
struct UpdateStatus {
    configured: bool,
    current_version: Option<String>,
    latest_version: Option<String>,
    update_available: Option<bool>,
    checked_at: Option<String>,
    applying: Option<bool>,
    error: Option<String>,
    /// While `applying`: what the updater is waiting on (e.g. a slow first boot it keeps retrying).
    progress: Option<String>,
}

/// Body of `POST /update/request` and `/update/check`.
#[derive(Serialize, utoipa::ToSchema)]
#[allow(dead_code)] // schema only: built with json! to keep the wire shape in one place
struct UpdateAck {
    configured: bool,
    /// Present only when the updater is configured.
    requested: Option<bool>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/update/status", get(get_status))
        .route("/update/request", post(request_update))
        .route("/update/check", post(check_now))
}

fn status_dir(state: &AppState) -> PathBuf {
    PathBuf::from(&state.settings.update_status_dir)
}

/// Merges the on-disk status JSON into `{"configured": true, ...}`.
fn merge_status(data: Value) -> Value {
    let mut merged = json!({ "configured": true });
    if let (Some(obj), Some(data_obj)) = (merged.as_object_mut(), data.as_object()) {
        for (k, v) in data_obj {
            obj.insert(k.clone(), v.clone());
        }
    }
    merged
}

#[utoipa::path(
    operation_id = "getUpdateStatus",
    get,
    path = "/api/update/status",
    tag = "update",
    summary = "Updater status",
    responses((status = 200, body = UpdateStatus), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_status(State(state): State<AppState>, _user: User) -> AppResult<Json<Value>> {
    let status_file = status_dir(&state).join("status.json");
    if !status_file.exists() {
        return Ok(Json(json!({ "configured": false })));
    }

    let text = match std::fs::read_to_string(&status_file) {
        Ok(t) => t,
        Err(_) => return Ok(Json(json!({ "configured": false }))),
    };
    let data: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Ok(Json(json!({ "configured": false }))),
    };

    Ok(Json(merge_status(data)))
}

/// Drops a marker file for `scripts/auto-update.sh` (run every 20s by the
/// `updater` service) -- acted on at its next run, not instantly.
fn drop_marker(state: &AppState, name: &str) -> AppResult<Json<Value>> {
    let dir = status_dir(state);
    if !dir.exists() {
        return Ok(Json(json!({ "configured": false })));
    }
    std::fs::File::create(dir.join(name)).map_err(|e| crate::error::AppError::internal(e.to_string()))?;
    Ok(Json(json!({ "configured": true, "requested": true })))
}

/// The updater restarts the stack and moves data: instance admins only. `what` completes
/// "Only an instance admin can ...". Shared with the HTTPS routes, which ask the same updater.
pub(super) async fn require_admin(state: &AppState, user: &User, what: &str) -> AppResult<()> {
    if crate::authz::is_admin(&state.control, user).await? {
        Ok(())
    } else {
        Err(AppError::coded(ErrorCode::AuthForbidden, format!("Only an instance admin can {what}.")))
    }
}

/// "Update now": apply the newest release.
#[utoipa::path(
    operation_id = "requestUpdate",
    post,
    path = "/api/update/request",
    tag = "update",
    summary = "Ask the updater to update",
    responses((status = 200, body = UpdateAck), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn request_update(State(state): State<AppState>, user: User) -> AppResult<Json<Value>> {
    require_admin(&state, &user, "update this server").await?;
    drop_marker(&state, "requested")
}

/// "Check now": ask GitHub for the newest release without waiting for the
/// 10-minute throttle.
#[utoipa::path(
    operation_id = "checkForUpdate",
    post,
    path = "/api/update/check",
    tag = "update",
    summary = "Ask the updater to check now",
    responses((status = 200, body = UpdateAck), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn check_now(State(state): State<AppState>, user: User) -> AppResult<Json<Value>> {
    require_admin(&state, &user, "update this server").await?;
    drop_marker(&state, "check")
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    get_status,
    request_update,
    check_now,
))]
pub struct Doc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_status_adds_configured_true_alongside_data() {
        let merged = merge_status(json!({
            "current_version": "v1.0.0",
            "latest_version": "v1.1.0",
            "update_available": true,
            "checked_at": "now"
        }));
        assert_eq!(merged["configured"], json!(true));
        assert_eq!(merged["current_version"], json!("v1.0.0"));
        assert_eq!(merged["update_available"], json!(true));
    }

    #[test]
    fn merge_status_with_empty_object_is_just_configured() {
        let merged = merge_status(json!({}));
        assert_eq!(merged, json!({ "configured": true }));
    }
}
