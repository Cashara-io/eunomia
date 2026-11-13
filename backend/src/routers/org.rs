//! Organisation invitations and seats (`invitations.rs`).
//!
//! `/api/org/...` is for an owner or admin of the caller's own org (or an instance admin): every id is
//! looked up inside that org only. Creating, resending and withdrawing need a browser session, because
//! an invitation grants access to the org. `/api/invitations/lookup` is public (the link holder may not
//! have an account), limited per client address; `/api/invitations/accept` is for a signed-in account.
//! Signing up through a link is `POST /api/auth/register` with `invitation`.

use axum::{
    extract::{Extension, Path, State},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

use crate::authz;
use crate::error::AppResult;
use crate::invitations::{self, InvitationAccepted, OrgInvitationLink, OrgInvitation, OrgInviteRole, InvitationLookup, OrgSeats, Who};
use crate::models_user::User;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/org/invitations", get(list_invitations).post(create_invitation))
        .route("/org/invitations/{invitation_id}", delete(revoke_invitation))
        .route("/org/invitations/{invitation_id}/resend", post(resend_invitation))
        .route("/org/seats", get(get_seats))
        .route("/invitations/lookup", post(lookup_invitation))
        .route("/invitations/accept", post(accept_invitation))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct OrgInvitationCreate {
    email: String,
    /// `member` (default) or `admin` (can invite people and see seats; not an instance admin).
    #[serde(default)]
    role: OrgInviteRole,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct InvitationToken {
    /// The token from the invitation link (the part after `#`).
    token: String,
}

#[utoipa::path(
    operation_id = "listOrgInvitations",
    get,
    path = "/api/org/invitations",
    tag = "org",
    summary = "Your organisation's open invitations, newest first (owner or admin)",
    description = "Expired invitations are listed too, so they can be resent. Never returns a link or token.",
    responses((status = 200, body = Vec<OrgInvitation>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_invitations(State(state): State<AppState>, user: User) -> AppResult<Json<Vec<OrgInvitation>>> {
    Ok(Json(invitations::list(&state, &user).await?))
}

#[utoipa::path(
    operation_id = "createOrgInvitation",
    post,
    path = "/api/org/invitations",
    tag = "org",
    summary = "Invite someone to your organisation by email (owner or admin, browser session)",
    description = "Returns the link once; only its hash is stored. Eunomia sends no email: send the link yourself. \
                   The link expires after 7 days and works once. It is refused for an email already in this organisation or already invited here.",
    request_body = OrgInvitationCreate,
    responses((status = 200, body = OrgInvitationLink), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn create_invitation(State(state): State<AppState>, user: User, Json(body): Json<OrgInvitationCreate>) -> AppResult<Json<OrgInvitationLink>> {
    authz::require_session()?;
    Ok(Json(invitations::create(&state, &user, &body.email, body.role).await?))
}

#[utoipa::path(
    operation_id = "resendOrgInvitation",
    post,
    path = "/api/org/invitations/{invitation_id}/resend",
    tag = "org",
    summary = "Make a new link for an open invitation (owner or admin, browser session)",
    description = "The old link stops working and the expiry starts again.",
    params(("invitation_id" = String, Path)),
    responses((status = 200, body = OrgInvitationLink), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn resend_invitation(State(state): State<AppState>, user: User, Path(invitation_id): Path<String>) -> AppResult<Json<OrgInvitationLink>> {
    authz::require_session()?;
    Ok(Json(invitations::resend(&state, &user, &invitation_id).await?))
}

#[utoipa::path(
    operation_id = "revokeOrgInvitation",
    delete,
    path = "/api/org/invitations/{invitation_id}",
    tag = "org",
    summary = "Withdraw an open invitation (owner or admin, browser session)",
    params(("invitation_id" = String, Path)),
    responses((status = 200, body = crate::openapi::OkBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn revoke_invitation(State(state): State<AppState>, user: User, Path(invitation_id): Path<String>) -> AppResult<Json<serde_json::Value>> {
    authz::require_session()?;
    invitations::revoke(&state, &user, &invitation_id).await?;
    Ok(Json(json!({ "ok": true })))
}

#[utoipa::path(
    operation_id = "getOrgSeats",
    get,
    path = "/api/org/seats",
    tag = "org",
    summary = "OrgSeats in your organisation: active and suspended members, pending invitations (owner or admin)",
    responses((status = 200, body = OrgSeats), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_seats(State(state): State<AppState>, user: User) -> AppResult<Json<OrgSeats>> {
    Ok(Json(invitations::seats(&state, &user).await?))
}

#[utoipa::path(
    operation_id = "lookupOrgInvitation",
    post,
    path = "/api/invitations/lookup",
    tag = "org",
    summary = "What an invitation link is for: the organisation, the invited email and whether it has expired",
    description = "Public, limited per client address. An unknown, used or withdrawn link is `invitation.not_found`. Says nothing about whether the email has an account.",
    request_body = InvitationToken,
    responses((status = 200, body = InvitationLookup), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(()),
)]
async fn lookup_invitation(State(state): State<AppState>, ip: Option<Extension<crate::gate::ClientIp>>, Json(body): Json<InvitationToken>) -> AppResult<Json<InvitationLookup>> {
    let ip = ip.map(|Extension(i)| i.0).unwrap_or_default();
    Ok(Json(invitations::lookup(&state, &body.token, Who::Address(&ip)).await?))
}

#[utoipa::path(
    operation_id = "acceptOrgInvitation",
    post,
    path = "/api/invitations/accept",
    tag = "org",
    summary = "Accept an organisation invitation with the signed-in account (browser session)",
    description = "The account's email must be the invited one. An account that already belongs to another organisation is refused (`invitation.other_org`): an account is in one organisation for now.",
    request_body = InvitationToken,
    responses((status = 200, body = InvitationAccepted), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn accept_invitation(State(state): State<AppState>, user: User, Json(body): Json<InvitationToken>) -> AppResult<Json<InvitationAccepted>> {
    authz::require_session()?;
    Ok(Json(invitations::accept_signed_in(&state, &user, &body.token).await?))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(list_invitations, create_invitation, resend_invitation, revoke_invitation, get_seats, lookup_invitation, accept_invitation))]
pub struct Doc;
