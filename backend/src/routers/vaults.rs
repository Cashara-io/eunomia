//! Vault REST routes: thin wrappers over `vaults::service`, same pattern as
//! `routers/auth.rs`.

use axum::{
    extract::{Path, State},
    routing::{get, patch, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use surrealdb::types::RecordId;

use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::User;
use crate::state::AppState;
use crate::vaults::service;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/vaults", get(list_vaults).post(create_vault))
        .route("/vaults/invitations", get(list_invitations))
        .route("/vaults/merge", post(merge_vaults))
        .route("/vaults/{vault_id}", patch(rename_vault).delete(delete_vault))
        .route("/vaults/{vault_id}/clone", post(clone_vault))
        .route("/vaults/{vault_id}/members", get(list_members).post(invite_member))
        .route("/vaults/{vault_id}/members/{email}", axum::routing::delete(remove_member))
        .route("/vaults/{vault_id}/members/{email}/role", axum::routing::put(set_member_role))
        .route("/vaults/{vault_id}/transfer", post(transfer_ownership))
        .route("/vaults/{vault_id}/leave", post(leave_vault))
        .route("/vaults/{vault_id}/integrity", get(check_integrity).post(repair_integrity))
        .route("/vaults/{vault_id}/invitations", get(list_pending_invites))
        .route("/vaults/{vault_id}/invitations/accept", post(accept_invitation))
        .route("/vaults/{vault_id}/invitations/decline", post(decline_invitation))
}

fn default_kind() -> String {
    "org".to_string()
}

fn default_role() -> String {
    "member".to_string()
}

#[derive(Deserialize, utoipa::ToSchema)]
struct VaultCreate {
    name: String,
    #[serde(default = "default_kind")]
    kind: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct VaultClone {
    #[serde(default)]
    name: Option<String>,
    #[serde(default = "default_kind")]
    kind: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct VaultRename {
    name: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct InviteRequest {
    email: String,
    #[serde(default = "default_role")]
    role: String,
}

#[derive(Serialize, utoipa::ToSchema)]
struct VaultList {
    results: Vec<service::VaultWithRole>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct MemberList {
    results: Vec<service::MemberOut>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct InvitationList {
    results: Vec<service::InvitationOut>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct PendingInviteList {
    results: Vec<service::PendingInviteOut>,
}

/// Vault kinds a client may create or clone into.
fn valid_vault_kind(kind: &str) -> bool {
    matches!(kind, "org" | "personal")
}

/// Path params are plain strings; parse here, same convention as `routers/auth.rs`'s token/session
/// id parsing.
fn parse_vault_id(vault_id: &str) -> AppResult<RecordId> {
    crate::rid::parse(vault_id).map_err(|_| AppError::coded(ErrorCode::VaultNotFound, "Vault not found."))
}

#[utoipa::path(
    operation_id = "listVaults",
    get,
    path = "/api/vaults",
    tag = "vaults",
    summary = "List vaults the caller belongs to",
    responses((status = 200, body = VaultList), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_vaults(State(state): State<AppState>, user: User) -> AppResult<Json<VaultList>> {
    let state = state.org(&user.org).await?;
    let results = service::list_my_vaults(&state.db, &user.id).await?;
    Ok(Json(VaultList { results }))
}

#[utoipa::path(
    operation_id = "createVault",
    post,
    path = "/api/vaults",
    tag = "vaults",
    summary = "Create a vault",
    request_body = VaultCreate,
    responses((status = 200, body = service::VaultOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn create_vault(
    State(state): State<AppState>,
    user: User,
    Json(body): Json<VaultCreate>,
) -> AppResult<Json<service::VaultOut>> {
    let state = state.org(&user.org).await?;
    if !valid_vault_kind(&body.kind) {
        return Err(AppError::bad_request("kind must be 'org' or 'personal'"));
    }
    Ok(Json(service::create_vault(&state.db, &state.control, &user.id, &body.name, &body.kind).await?))
}

#[utoipa::path(
    operation_id = "cloneVault",
    post,
    path = "/api/vaults/{vault_id}/clone",
    tag = "vaults",
    summary = "Clone a vault",
    params(("vault_id" = String, Path)),
    request_body = VaultClone,
    responses((status = 200, body = service::CloneOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn clone_vault(
    State(state): State<AppState>,
    user: User,
    Path(vault_id): Path<String>,
    Json(body): Json<VaultClone>,
) -> AppResult<Json<service::CloneOut>> {
    let state = state.org(&user.org).await?;
    if !valid_vault_kind(&body.kind) {
        return Err(AppError::bad_request("kind must be 'org' or 'personal'"));
    }
    let rid = parse_vault_id(&vault_id)?;
    Ok(Json(service::clone_vault(&state.db, &state.control, &user.id, &rid, body.name.as_deref(), &body.kind).await?))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct VaultMerge {
    vault_ids: [String; 2],
    #[serde(default)]
    name: Option<String>,
    #[serde(default = "default_kind")]
    kind: String,
}

#[utoipa::path(
    operation_id = "mergeVaults",
    post,
    path = "/api/vaults/merge",
    tag = "vaults",
    summary = "Merge two vaults",
    request_body = VaultMerge,
    responses((status = 200, body = service::MergeOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn merge_vaults(
    State(state): State<AppState>,
    user: User,
    Json(body): Json<VaultMerge>,
) -> AppResult<Json<service::MergeOut>> {
    let state = state.org(&user.org).await?;
    if !valid_vault_kind(&body.kind) {
        return Err(AppError::bad_request("kind must be 'org' or 'personal'"));
    }
    let a = parse_vault_id(&body.vault_ids[0])?;
    let b = parse_vault_id(&body.vault_ids[1])?;
    Ok(Json(service::merge_vaults(&state.db, &state.control, &user.id, &a, &b, body.name.as_deref(), &body.kind).await?))
}

#[utoipa::path(
    operation_id = "renameVault",
    patch,
    path = "/api/vaults/{vault_id}",
    tag = "vaults",
    summary = "Rename a vault",
    params(("vault_id" = String, Path)),
    request_body = VaultRename,
    responses((status = 200, body = service::VaultOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn rename_vault(
    State(state): State<AppState>,
    user: User,
    Path(vault_id): Path<String>,
    Json(body): Json<VaultRename>,
) -> AppResult<Json<service::VaultOut>> {
    let state = state.org(&user.org).await?;
    let rid = parse_vault_id(&vault_id)?;
    Ok(Json(service::rename_vault(&state.db, &state.control, &user.id, &rid, &body.name).await?))
}

#[utoipa::path(
    operation_id = "deleteVault",
    delete,
    path = "/api/vaults/{vault_id}",
    tag = "vaults",
    summary = "Delete a vault",
    params(("vault_id" = String, Path)),
    responses((status = 200, body = crate::openapi::DeletedBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn delete_vault(
    State(state): State<AppState>,
    user: User,
    Path(vault_id): Path<String>,
) -> AppResult<Json<Value>> {
    let state = state.org(&user.org).await?;
    let rid = parse_vault_id(&vault_id)?;
    service::delete_vault(&state.db, &state.control, &user.id, &rid).await?;
    Ok(Json(json!({ "deleted": true })))
}

#[utoipa::path(
    operation_id = "listMembers",
    get,
    path = "/api/vaults/{vault_id}/members",
    tag = "vaults",
    summary = "List vault members",
    params(("vault_id" = String, Path)),
    responses((status = 200, body = MemberList), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_members(
    State(state): State<AppState>,
    user: User,
    Path(vault_id): Path<String>,
) -> AppResult<Json<MemberList>> {
    let state = state.org(&user.org).await?;
    let rid = parse_vault_id(&vault_id)?;
    let results = service::list_members(&state.db, &state.control, &user.id, &rid).await?;
    Ok(Json(MemberList { results }))
}

#[utoipa::path(
    operation_id = "inviteMember",
    post,
    path = "/api/vaults/{vault_id}/members",
    tag = "vaults",
    summary = "Invite someone to a vault",
    params(("vault_id" = String, Path)),
    request_body = InviteRequest,
    responses((status = 200, body = service::InviteOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn invite_member(
    State(state): State<AppState>,
    user: User,
    Path(vault_id): Path<String>,
    Json(body): Json<InviteRequest>,
) -> AppResult<Json<service::InviteOut>> {
    let state = state.org(&user.org).await?;
    let rid = parse_vault_id(&vault_id)?;
    Ok(Json(service::invite_member(&state.db, &state.control, &user.id, &rid, &body.email, &body.role).await?))
}

#[utoipa::path(
    operation_id = "removeMember",
    delete,
    path = "/api/vaults/{vault_id}/members/{email}",
    tag = "vaults",
    summary = "Remove a vault member",
    params(("vault_id" = String, Path), ("email" = String, Path)),
    responses((status = 200, body = crate::openapi::RemovedBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn remove_member(
    State(state): State<AppState>,
    user: User,
    Path((vault_id, email)): Path<(String, String)>,
) -> AppResult<Json<Value>> {
    let state = state.org(&user.org).await?;
    let rid = parse_vault_id(&vault_id)?;
    service::remove_member(&state.db, &state.control, &user.id, &rid, &email).await?;
    Ok(Json(json!({ "removed": true })))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct RoleRequest {
    /// `owner` or `member`
    role: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct TransferRequest {
    /// An active member who becomes an owner; the caller becomes a member.
    email: String,
}

#[utoipa::path(
    operation_id = "setMemberRole",
    put,
    path = "/api/vaults/{vault_id}/members/{email}/role",
    tag = "vaults",
    summary = "Promote or demote a vault member",
    params(("vault_id" = String, Path), ("email" = String, Path)),
    request_body = RoleRequest,
    responses((status = 200, body = service::MemberOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn set_member_role(
    State(state): State<AppState>,
    user: User,
    Path((vault_id, email)): Path<(String, String)>,
    Json(body): Json<RoleRequest>,
) -> AppResult<Json<service::MemberOut>> {
    let state = state.org(&user.org).await?;
    let rid = parse_vault_id(&vault_id)?;
    Ok(Json(service::set_member_role(&state.db, &state.control, &user.id, &rid, &email, &body.role).await?))
}

#[utoipa::path(
    operation_id = "transferOwnership",
    post,
    path = "/api/vaults/{vault_id}/transfer",
    tag = "vaults",
    summary = "Hand a vault to another member",
    params(("vault_id" = String, Path)),
    request_body = TransferRequest,
    responses((status = 200, body = service::MemberOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn transfer_ownership(
    State(state): State<AppState>,
    user: User,
    Path(vault_id): Path<String>,
    Json(body): Json<TransferRequest>,
) -> AppResult<Json<service::MemberOut>> {
    let state = state.org(&user.org).await?;
    let rid = parse_vault_id(&vault_id)?;
    Ok(Json(service::transfer_ownership(&state.db, &state.control, &user.id, &rid, &body.email).await?))
}

#[utoipa::path(
    operation_id = "leaveVault",
    post,
    path = "/api/vaults/{vault_id}/leave",
    tag = "vaults",
    summary = "Leave a vault",
    params(("vault_id" = String, Path)),
    responses((status = 200, body = crate::openapi::LeftBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn leave_vault(
    State(state): State<AppState>,
    user: User,
    Path(vault_id): Path<String>,
) -> AppResult<Json<Value>> {
    let state = state.org(&user.org).await?;
    let rid = parse_vault_id(&vault_id)?;
    service::leave_vault(&state.db, &user.id, &rid).await?;
    Ok(Json(json!({ "left": true })))
}

#[utoipa::path(
    operation_id = "listInvitations",
    get,
    path = "/api/vaults/invitations",
    tag = "vaults",
    summary = "Pending vault invitations",
    responses((status = 200, body = InvitationList), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_invitations(State(state): State<AppState>, user: User) -> AppResult<Json<InvitationList>> {
    let state = state.org(&user.org).await?;
    let mut results = service::list_my_invitations(&state.db, &user.id).await?;
    service::add_inviters(&state.db, &state.control, &mut results).await?;
    Ok(Json(InvitationList { results }))
}

#[utoipa::path(
    operation_id = "listVaultPendingInvitations",
    get,
    path = "/api/vaults/{vault_id}/invitations",
    tag = "vaults",
    summary = "Invitations into a vault not answered yet (owners only)",
    description = "Withdraw one with `DELETE /api/vaults/{vault_id}/members/{email}`.",
    params(("vault_id" = String, Path)),
    responses((status = 200, body = PendingInviteList), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_pending_invites(
    State(state): State<AppState>,
    user: User,
    Path(vault_id): Path<String>,
) -> AppResult<Json<PendingInviteList>> {
    let state = state.org(&user.org).await?;
    let rid = parse_vault_id(&vault_id)?;
    let results = service::list_pending_invites(&state.db, &state.control, &user.id, &rid).await?;
    Ok(Json(PendingInviteList { results }))
}

#[utoipa::path(
    operation_id = "acceptInvitation",
    post,
    path = "/api/vaults/{vault_id}/invitations/accept",
    tag = "vaults",
    summary = "Accept an invitation",
    params(("vault_id" = String, Path)),
    responses((status = 200, body = service::VaultWithRole), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn accept_invitation(
    State(state): State<AppState>,
    user: User,
    Path(vault_id): Path<String>,
) -> AppResult<Json<service::VaultWithRole>> {
    let state = state.org(&user.org).await?;
    let rid = parse_vault_id(&vault_id)?;
    Ok(Json(service::accept_invitation(&state.db, &state.control, &user.id, &rid).await?))
}

#[utoipa::path(
    operation_id = "declineInvitation",
    post,
    path = "/api/vaults/{vault_id}/invitations/decline",
    tag = "vaults",
    summary = "Decline an invitation",
    params(("vault_id" = String, Path)),
    responses((status = 200, body = crate::openapi::DeclinedBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn decline_invitation(
    State(state): State<AppState>,
    user: User,
    Path(vault_id): Path<String>,
) -> AppResult<Json<Value>> {
    let state = state.org(&user.org).await?;
    let rid = parse_vault_id(&vault_id)?;
    service::decline_invitation(&state.db, &state.control, &user.id, &rid).await?;
    Ok(Json(json!({ "declined": true })))
}

#[utoipa::path(
    operation_id = "checkVaultIntegrity",
    get,
    path = "/api/vaults/{vault_id}/integrity",
    tag = "vaults",
    summary = "Check a vault for broken or leftover data",
    params(("vault_id" = String, Path)),
    responses((status = 200, body = crate::integrity::IntegrityReport), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn check_integrity(State(state): State<AppState>, user: User, Path(vault_id): Path<String>) -> AppResult<Json<crate::integrity::IntegrityReport>> {
    let state = state.org(&user.org).await?;
    let rid = parse_vault_id(&vault_id)?;
    Ok(Json(crate::integrity::check(&state.db, &user.id, &rid).await?))
}

#[utoipa::path(
    operation_id = "repairVaultIntegrity",
    post,
    path = "/api/vaults/{vault_id}/integrity",
    tag = "vaults",
    summary = "Repair what an integrity check found (owner only)",
    params(("vault_id" = String, Path)),
    responses((status = 200, body = crate::integrity::IntegrityReport), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn repair_integrity(State(state): State<AppState>, user: User, Path(vault_id): Path<String>) -> AppResult<Json<crate::integrity::IntegrityReport>> {
    let state = state.org(&user.org).await?;
    let rid = parse_vault_id(&vault_id)?;
    Ok(Json(crate::integrity::repair(&state.db, &user.id, &rid).await?))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    list_vaults,
    create_vault,
    list_invitations,
    list_pending_invites,
    merge_vaults,
    rename_vault,
    delete_vault,
    clone_vault,
    list_members,
    invite_member,
    remove_member,
    set_member_role,
    transfer_ownership,
    leave_vault,
    check_integrity,
    repair_integrity,
    accept_invitation,
    decline_invitation,
))]
pub struct Doc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_vault_kind_accepts_only_org_and_personal() {
        assert!(valid_vault_kind("org"));
        assert!(valid_vault_kind("personal"));
        assert!(!valid_vault_kind("team"));
        assert!(!valid_vault_kind(""));
    }

    #[test]
    fn request_defaults() {
        assert_eq!(default_kind(), "org");
        assert_eq!(default_role(), "member");
    }

    #[test]
    fn parse_vault_id_rejects_garbage() {
        assert!(parse_vault_id("not-a-record-id!!").is_err());
    }

    #[test]
    fn parse_vault_id_accepts_well_formed_id() {
        assert!(parse_vault_id("vault:abc123").is_ok());
    }
}
