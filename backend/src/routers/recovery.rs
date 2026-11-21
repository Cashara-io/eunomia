//! Password recovery and change (`recovery.rs`).
//!
//! - `POST /api/auth/password/forgot` and `/reset`: no session, limited per client address like login.
//! - `POST /api/account/password`: a browser session (never a token) changes its own password.
//!
//! All three are refused when the server signs in through single sign-on only (`AUTH_MODE=oidc`).

use axum::{
    extract::{Extension, State},
    http::HeaderMap,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

use crate::authz;
use crate::error::AppResult;
use crate::models_user::User;
use crate::recovery;
use crate::routers::auth::require_password_enabled;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/password/forgot", post(forgot))
        .route("/auth/password/reset", post(reset))
        .route("/account/password", post(change))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct ForgotBody {
    email: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct ResetBody {
    /// The token from the reset link (the part after `#`).
    token: String,
    /// The new password: the same rules as sign-up (8 characters, at most 72 bytes).
    password: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct ChangeBody {
    current_password: String,
    /// The same rules as sign-up (8 characters, at most 72 bytes).
    new_password: String,
    /// A current authenticator or recovery code. Needed from an account with two-step verification when its
    /// last second factor is older than 10 minutes (`auth.step_up_required`).
    #[serde(default)]
    mfa_code: Option<String>,
}

fn ip_of(ip: Option<Extension<crate::gate::ClientIp>>) -> String {
    ip.map(|Extension(i)| i.0).unwrap_or_default()
}

#[utoipa::path(
    operation_id = "forgotPassword",
    post,
    path = "/api/auth/password/forgot",
    tag = "auth",
    summary = "Email a password reset link",
    description = "Always answers `{\"ok\": true}` for a well-formed address, whether or not an account exists, so it cannot be used to find out who has one. An account that has a password and is not suspended is emailed a link that works once and expires after 30 minutes; asking again replaces it, and a new one is not sent within a minute of the last. A single sign-on account has no password and is sent nothing. Limited per client address (429 `rate.limited`) and per email (no more links, same answer). `email.unavailable` (503) when this server sends no email: check `email_enabled` from `/api/auth/bootstrap` first.",
    request_body = ForgotBody,
    responses((status = 200, body = crate::openapi::OkBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(()),
)]
async fn forgot(State(state): State<AppState>, ip: Option<Extension<crate::gate::ClientIp>>, Json(body): Json<ForgotBody>) -> AppResult<Json<serde_json::Value>> {
    require_password_enabled(&state)?;
    recovery::request_reset(&state, &ip_of(ip), &body.email).await?;
    Ok(Json(json!({ "ok": true })))
}

#[utoipa::path(
    operation_id = "resetPassword",
    post,
    path = "/api/auth/password/reset",
    tag = "auth",
    summary = "Set a new password with a reset link",
    description = "The token works once and for 30 minutes (`auth.reset_invalid`, `auth.reset_expired`). On success every browser session of the account ends and no session starts: sign in with the new password. Two-step verification is untouched, so the next sign-in still asks for the second factor. API tokens and app connections are not revoked. An account with no password (single sign-on) can't be given one here.",
    request_body = ResetBody,
    responses((status = 200, body = crate::openapi::OkBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(()),
)]
async fn reset(State(state): State<AppState>, ip: Option<Extension<crate::gate::ClientIp>>, Json(body): Json<ResetBody>) -> AppResult<Json<serde_json::Value>> {
    require_password_enabled(&state)?;
    recovery::reset_password(&state, &ip_of(ip), &body.token, &body.password).await?;
    Ok(Json(json!({ "ok": true })))
}

#[utoipa::path(
    operation_id = "changePassword",
    post,
    path = "/api/account/password",
    tag = "auth",
    summary = "Change your own password (browser session)",
    description = "Needs the current password (10 wrong passwords per account in 15 minutes and it is refused, shared with setting up two-step verification) and, for an account with two-step verification, a recent second factor or `mfa_code` (`auth.step_up_required`). Every other session of the account ends; this one stays. A wrong current password is 403 `auth.forbidden`. Accounts that sign in only through single sign-on have no password to change.",
    request_body = ChangeBody,
    responses((status = 200, body = crate::openapi::OkBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn change(State(state): State<AppState>, user: User, headers: HeaderMap, Json(body): Json<ChangeBody>) -> AppResult<Json<serde_json::Value>> {
    authz::require_session()?;
    require_password_enabled(&state)?;
    recovery::change_password(&state, &user, &headers, &body.current_password, &body.new_password, body.mfa_code.as_deref()).await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(forgot, reset, change))]
pub struct Doc;
