//! An organisation runs itself (`org_admin.rs`): change a member's role, end their sessions, revoke their
//! tokens and apps, and set the organisation's name and its two-step verification policy. Every route is for
//! the caller's own organisation and passes through one capability function of `org_admin.rs`; the
//! organisation is never a parameter, and a person in another organisation is "not found". The people
//! list, suspension, other people's tokens, the audit log and access reviews keep their routes
//! (`routers/admin.rs`, `audit_chain.rs`, `access_reviews.rs`) with their gates changed to the same functions.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::Deserialize;

use crate::authz;
use crate::error::AppResult;
use crate::models_user::User;
use crate::org_admin::{self, AccessRevoked, OrgOut, OrgRole, PersonApp};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/org", get(get_org))
        .route("/admin/org/name", put(put_name))
        .route("/admin/org/security", put(put_security))
        .route("/admin/people/{user_id}/role", put(set_role))
        .route("/admin/people/{user_id}/sessions", delete(end_sessions))
        .route("/admin/people/{user_id}/revoke-access", post(revoke_access))
        .route("/admin/people/{user_id}/apps", get(get_apps))
        .route("/admin/people/{user_id}/apps/{grant_id}", delete(revoke_app))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct NameChange {
    /// The organisation's new display name (1 to 80 characters, no `@`).
    name: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct PolicyChange {
    /// `true` requires two-step verification of every member of this organisation who signs in with a password.
    require_mfa: bool,
    /// A current authenticator or recovery code, when your own account has two-step verification and its
    /// last second factor is older than 10 minutes (`auth.step_up_required`).
    #[serde(default)]
    mfa_code: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct RoleChange {
    /// `member` or `admin`. Only the organisation's owner changes roles; nobody changes the owner's.
    role: OrgRoleChoice,
}

/// The roles a role change may give.
#[derive(Deserialize, utoipa::ToSchema, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum OrgRoleChoice {
    Member,
    Admin,
}

#[derive(serde::Serialize, utoipa::ToSchema)]
struct SessionsEnded {
    /// Browser sessions that were open.
    sessions: i64,
}

#[utoipa::path(
    operation_id = "getOrg",
    get,
    path = "/api/admin/org",
    tag = "admin",
    summary = "Your organisation's name and security policy (organisation owner or admin)",
    responses((status = 200, body = OrgOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_org(State(state): State<AppState>, user: User) -> AppResult<Json<OrgOut>> {
    let auth = org_admin::may_administer_members(&state.control, &user).await?;
    Ok(Json(org_admin::org_out(&state, &user, &auth).await?))
}

#[utoipa::path(
    operation_id = "renameOrg",
    put,
    path = "/api/admin/org/name",
    tag = "admin",
    summary = "Rename your organisation (its owner, browser session)",
    description = "The name is shown to people you invite. It is recorded in the audit log.",
    request_body = NameChange,
    responses((status = 200, body = OrgOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn put_name(State(state): State<AppState>, user: User, Json(body): Json<NameChange>) -> AppResult<Json<OrgOut>> {
    authz::require_session()?;
    Ok(Json(org_admin::rename(&state, &user, &body.name).await?))
}

#[utoipa::path(
    operation_id = "setOrgSecurity",
    put,
    path = "/api/admin/org/security",
    tag = "admin",
    summary = "Require two-step verification of your organisation's members, or stop (organisation owner or admin, browser session)",
    description = "Binds this organisation's members only, on top of the server-wide switch (`/api/admin/security`, the server's operator): a member must use two-step verification when either requires it. Turning it on works as the server-wide switch does: a password sign-in without two-step verification must set it up before it gets a session, and browser sessions that never passed a second factor are signed out. API tokens and app connections keep working. Needs your own account protected first.",
    request_body = PolicyChange,
    responses((status = 200, body = OrgOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn put_security(State(state): State<AppState>, user: User, headers: HeaderMap, Json(body): Json<PolicyChange>) -> AppResult<Json<OrgOut>> {
    authz::require_session()?;
    Ok(Json(org_admin::set_mfa_policy(&state, &user, &headers, body.require_mfa, body.mfa_code.as_deref()).await?))
}

#[utoipa::path(
    operation_id = "setPersonOrgRole",
    put,
    path = "/api/admin/people/{user_id}/role",
    tag = "admin",
    summary = "Make a member an organisation admin, or an ordinary member again (the organisation's owner, browser session)",
    description = "An organisation admin can invite members, suspend members and revoke their access, read the audit log and run access reviews, for this organisation only. Only the owner changes roles. The owner's role never changes here, and an operator of the server is not changed by an organisation's owner.",
    params(("user_id" = String, Path)),
    request_body = RoleChange,
    responses((status = 200, body = crate::routers::admin::PeopleReportRef), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn set_role(State(state): State<AppState>, user: User, Path(user_id): Path<String>, Json(body): Json<RoleChange>) -> AppResult<Json<crate::routers::admin::PeopleReportRef>> {
    authz::require_session()?;
    let role = match body.role {
        OrgRoleChoice::Member => OrgRole::Member,
        OrgRoleChoice::Admin => OrgRole::Admin,
    };
    org_admin::set_role(&state, &user, &user_id, role).await?;
    Ok(Json(crate::routers::admin::people_report(&state, &user).await?))
}

#[utoipa::path(
    operation_id = "endPersonSessions",
    delete,
    path = "/api/admin/people/{user_id}/sessions",
    tag = "admin",
    summary = "Sign a member out everywhere (organisation owner or admin, browser session)",
    description = "Ends their browser sessions. Their API tokens and connected apps keep working: see `revoke-access`.",
    params(("user_id" = String, Path)),
    responses((status = 200, body = SessionsEnded), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn end_sessions(State(state): State<AppState>, user: User, Path(user_id): Path<String>) -> AppResult<Json<SessionsEnded>> {
    authz::require_session()?;
    Ok(Json(SessionsEnded { sessions: org_admin::end_sessions(&state, &user, &user_id).await? }))
}

#[utoipa::path(
    operation_id = "revokePersonAccess",
    post,
    path = "/api/admin/people/{user_id}/revoke-access",
    tag = "admin",
    summary = "End every session, API token and connected app of a member at once (organisation owner or admin, browser session)",
    description = "For a lost laptop or a leaver. The account itself is untouched: it can sign in again unless it is suspended.",
    params(("user_id" = String, Path)),
    responses((status = 200, body = AccessRevoked), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn revoke_access(State(state): State<AppState>, user: User, Path(user_id): Path<String>) -> AppResult<Json<AccessRevoked>> {
    authz::require_session()?;
    Ok(Json(org_admin::revoke_all(&state, &user, &user_id).await?))
}

#[utoipa::path(
    operation_id = "listPersonApps",
    get,
    path = "/api/admin/people/{user_id}/apps",
    tag = "admin",
    summary = "The apps a member connected through OAuth (organisation owner or admin)",
    params(("user_id" = String, Path)),
    responses((status = 200, body = Vec<PersonApp>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_apps(State(state): State<AppState>, user: User, Path(user_id): Path<String>) -> AppResult<Json<Vec<PersonApp>>> {
    Ok(Json(org_admin::list_apps(&state, &user, &user_id).await?))
}

#[utoipa::path(
    operation_id = "revokePersonApp",
    delete,
    path = "/api/admin/people/{user_id}/apps/{grant_id}",
    tag = "admin",
    summary = "Disconnect one app of a member (organisation owner or admin, browser session)",
    description = "The app's tokens stop working at once. Returns the member's remaining apps.",
    params(("user_id" = String, Path), ("grant_id" = String, Path)),
    responses((status = 200, body = Vec<PersonApp>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn revoke_app(State(state): State<AppState>, user: User, Path((user_id, grant_id)): Path<(String, String)>) -> AppResult<Json<Vec<PersonApp>>> {
    authz::require_session()?;
    Ok(Json(org_admin::revoke_app(&state, &user, &user_id, &grant_id).await?))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(get_org, put_name, put_security, set_role, end_sessions, revoke_access, get_apps, revoke_app))]
pub struct Doc;
