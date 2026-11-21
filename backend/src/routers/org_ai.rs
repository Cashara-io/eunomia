//! The organisation's say over model use: `/api/org/ai` (`ai/orgcfg.rs`).
//!
//! For an owner or admin of the caller's own organisation (or an instance admin, in their own): the
//! organisation is always the caller's, never a request value, so there is no way to name another. Changes
//! need a browser session (a key is a credential), are audited, and never return the key.

use axum::{extract::State, routing::get, routing::put, Json, Router};
use serde::Deserialize;

use crate::ai::orgcfg::{self, OrgAiView, ProviderUpdate};
use crate::authz;
use crate::error::AppResult;
use crate::models_user::User;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/org/ai", get(get_ai))
        .route("/org/ai/provider", put(set_provider).delete(delete_provider))
        .route("/org/ai/enabled", put(set_enabled))
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct EnabledBody {
    /// `false` switches AI off for the whole organisation: no model is called for its content with any key,
    /// a member's own included.
    enabled: bool,
}

#[utoipa::path(
    operation_id = "getOrgAi",
    get,
    path = "/api/org/ai",
    tag = "org",
    summary = "Your organisation's AI settings, this month's use of its allowance, and whose key pays (owner or admin)",
    description = "Whether AI is on, the organisation's own provider (never its key: `key_set` says whether one is stored), whose key model use goes on by default, this month's use of the allowance as a percentage and a state (no prices or amounts), and this month's call counts by whose key paid.",
    responses((status = 200, body = OrgAiView), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_ai(State(state): State<AppState>, user: User) -> AppResult<Json<OrgAiView>> {
    Ok(Json(orgcfg::view(&state, &user).await?))
}

#[utoipa::path(
    operation_id = "setOrgAiProvider",
    put,
    path = "/api/org/ai/provider",
    tag = "org",
    summary = "Set the organisation's own AI provider: endpoint, key and models (owner or admin, browser session)",
    description = "With a provider of its own the organisation's model use goes to that provider, never on the server's key, so it never counts against the monthly allowance and continues when the allowance is used up. A person's own key still comes first. Only the fields you send change; an `api_key` of \"\" removes the key. The endpoint is checked like a person's own base URL (`ALLOW_PRIVATE_LLM_URL`). The key is stored encrypted and never returned. Changing the endpoint or the embedding model makes stored vectors stale: the embed job re-embeds them in the background, on this key. Audited as `org.ai_provider` (the host and what happened to the key, never the key).",
    request_body = ProviderUpdate,
    responses((status = 200, body = OrgAiView), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn set_provider(State(state): State<AppState>, user: User, Json(body): Json<ProviderUpdate>) -> AppResult<Json<OrgAiView>> {
    authz::require_session()?;
    Ok(Json(orgcfg::set_provider(&state, &user, body).await?))
}

#[utoipa::path(
    operation_id = "deleteOrgAiProvider",
    delete,
    path = "/api/org/ai/provider",
    tag = "org",
    summary = "Remove the organisation's own AI provider and key (owner or admin, browser session)",
    description = "Model use goes back to the server's key and its allowance.",
    responses((status = 200, body = OrgAiView), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn delete_provider(State(state): State<AppState>, user: User) -> AppResult<Json<OrgAiView>> {
    authz::require_session()?;
    Ok(Json(orgcfg::clear_provider(&state, &user).await?))
}

#[utoipa::path(
    operation_id = "setOrgAiEnabled",
    put,
    path = "/api/org/ai/enabled",
    tag = "org",
    summary = "Switch AI on or off for the whole organisation (owner or admin, browser session)",
    description = "While off, no call is made to any model provider for this organisation's content, with any key, a member's own included. Features that have a path without a model (saving, reading, keyword search) keep working; the others answer `ai.disabled` (403); background model work waits and resumes when AI is switched on. Audited as `org.ai_switch`.",
    request_body = EnabledBody,
    responses((status = 200, body = OrgAiView), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn set_enabled(State(state): State<AppState>, user: User, Json(body): Json<EnabledBody>) -> AppResult<Json<OrgAiView>> {
    authz::require_session()?;
    Ok(Json(orgcfg::set_enabled(&state, &user, body.enabled).await?))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(get_ai, set_provider, delete_provider, set_enabled))]
pub struct Doc;
