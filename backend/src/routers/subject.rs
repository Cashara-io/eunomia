//! Data-subject rights over HTTP (docs/privacy/subject-rights.md): the full export of what Eunomia
//! holds about a person and their verified erasure. The person exports and erases their own account
//! (Settings > Data); a server operator erases any account (Settings > People), cancels an erasure that
//! has not started deleting and lifts holds; a workspace admin exports this workspace's data about a
//! member and places a legal hold; the owner hands the workspace over. Starting or downloading an export and every
//! route that changes something need a browser session: no API token or app.

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use surrealdb::types::{Datetime, RecordId};

use crate::authz;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::User;
use crate::rid::RecordIdExt;
use crate::state::AppState;
use crate::subject::{self, export::ExportRow, Confirmation, ErasureRecord, ErasureTicket, Requester};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/export/subject", get(list_my_exports).post(start_my_export))
        .route("/export/subject/{export_id}", get(download_export))
        .route("/account/erasure", post(erase_me))
        .route("/account/activity", get(my_activity))
        .route("/admin/people/{user_id}/export", post(start_person_export))
        .route("/admin/people/{user_id}/erasure", post(erase_person))
        .route("/admin/people/{user_id}/legal-hold", put(set_legal_hold))
        .route("/admin/people/{user_id}/workspace-owner", put(make_workspace_owner))
        .route("/admin/erasures", get(list_erasures))
        .route("/admin/erasures/reapply", post(reapply_erasures))
        .route("/admin/erasures/{erasure_id}/cancel", post(cancel_erasure))
}

#[utoipa::path(
    operation_id = "startSubjectExport",
    post,
    path = "/api/export/subject",
    tag = "export",
    summary = "Prepare a full export of your data (a zip archive with a manifest; browser session)",
    description = "Queues a job that collects everything in scope about you in every workspace you belong to (account, spaces, what you wrote in spaces you can still read, documents and their files, synced records, chat, sources, tokens and connected apps by name, your audit events) into a zip archive kept for seven days. List exports to see when it is ready. Not available to API tokens or apps.",
    responses((status = 200, body = ExportRow), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn start_my_export(State(state): State<AppState>, user: User) -> AppResult<Json<ExportRow>> {
    authz::require_session()?;
    Ok(Json(subject::export::request_export(&state, &user.id, None).await?))
}

#[utoipa::path(
    operation_id = "listSubjectExports",
    get,
    path = "/api/export/subject",
    tag = "export",
    summary = "Your data exports (including ones an admin started for you) and the ones you asked for as an admin, newest first",
    responses((status = 200, body = Vec<ExportRow>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_my_exports(State(state): State<AppState>, user: User) -> AppResult<Json<Vec<ExportRow>>> {
    let org = state.org(&user.org).await?;
    Ok(Json(subject::export::list_exports(&org, &user.id).await?))
}

// open body: a zip download (Content-Disposition attachment), not consumed through the typed client
#[utoipa::path(
    operation_id = "downloadSubjectExport",
    get,
    path = "/api/export/subject/{export_id}",
    tag = "export",
    summary = "Download a ready data export (zip; browser session)",
    description = "Your own export (assembled from its part in every workspace you belong to), or a workspace export you started as an admin (while you are still an admin). Nobody else's own export, ever.",
    params(("export_id" = String, Path, description = "The export's id, as listed")),
    responses((status = 200, description = "The zip archive", content_type = "application/zip"), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn download_export(State(state): State<AppState>, user: User, Path(export_id): Path<String>) -> AppResult<Response> {
    authz::require_session()?;
    let org = state.org(&user.org).await?;
    let (bytes, scope, about) = subject::export::archive(&state, &org, &user.id, &export_id)
        .await?
        .ok_or_else(|| AppError::coded(ErrorCode::ResourceNotFound, "That export doesn't exist or has expired."))?;
    if scope == "organisation" {
        crate::routers::admin::require_admin(&state, &user).await?;
        crate::audit::record_as_caller(&state.control, &user.id, "subject.export_download_admin", &export_id, "ok").await;
        // and in the person's own activity (Settings > Data): an admin downloaded this workspace's data about them
        if let Some(about) = about {
            let detail = format!("by={}", user.email);
            crate::audit::record_as_caller_detail(&state.control, &about, "subject.export_download_admin", &export_id, "ok", &detail).await;
        }
    }
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/zip".to_string()), (header::CONTENT_DISPOSITION, "attachment; filename=\"eunomia-data-export.zip\"".to_string())],
        bytes,
    )
        .into_response())
}

#[derive(Deserialize, utoipa::ToSchema)]
struct EraseMe {
    /// Your password, typed again. Required when the account has one.
    #[serde(default)]
    password: Option<String>,
    /// You confirm that a workspace whose only member you are is closed with your account (everything
    /// in it is deleted). Without it such a request is refused with `subject.workspace_closes`.
    #[serde(default)]
    close_workspace: bool,
}

#[utoipa::path(
    operation_id = "eraseMyAccount",
    post,
    path = "/api/account/erasure",
    tag = "export",
    summary = "Delete your account and your data (browser session, confirmed with your password)",
    description = "An account with no password (single sign-on only) confirms with a sign-in from the last 10 minutes (`subject.reauthenticate` otherwise). Checks the confirmation, then suspends the account and signs it out everywhere, and queues the erasure. Refused with `subject.legal_hold` while an admin holds the account, `subject.last_owner` when you own a workspace that has other members and no other owner, and `subject.workspace_closes` until you confirm closing a workspace you are the only member of. What is deleted and what is kept: docs/privacy/subject-rights.md.",
    request_body = EraseMe,
    responses((status = 200, body = ErasureTicket), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn erase_me(State(state): State<AppState>, user: User, headers: HeaderMap, Json(body): Json<EraseMe>) -> AppResult<Json<ErasureTicket>> {
    authz::require_session()?;
    let res = async {
        let signed_in_at = subject::session_signed_in_at(&state.control, &user.id, crate::auth::session_sid(&state.settings.jwt_secret, &headers)).await?;
        subject::confirm(&state.control, &user.id, Confirmation::SelfService { password: body.password.as_deref(), signed_in_at }).await?;
        subject::request_erasure(&state, &user.id, Requester::Myself, body.close_workspace).await
    }
    .await;
    if let Err(e) = &res {
        crate::audit::record_as_caller(&state.control, &user.id, "subject.erase_request", "", e.code.as_str()).await;
    }
    Ok(Json(res?))
}

#[utoipa::path(
    operation_id = "startPersonExport",
    post,
    path = "/api/admin/people/{user_id}/export",
    tag = "admin",
    summary = "Prepare a full data export for someone in the workspace (admin only, browser session)",
    description = "This workspace's data about the person only: what they wrote in its shared spaces, their spaces here, their account record, tokens and connected apps by name, and audit events about them. Not their personal space, private chat, synced records or other workspaces. You download it from Settings > Data; the person's own activity records that it was made.",
    params(("user_id" = String, Path)),
    responses((status = 200, body = ExportRow), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn start_person_export(State(state): State<AppState>, user: User, Path(user_id): Path<String>) -> AppResult<Json<ExportRow>> {
    let (_, target, _) = crate::routers::admin::person_in_workspace(&state, &user, &user_id).await?;
    Ok(Json(subject::export::request_export(&state, &target, Some((&user.id, user.org))).await?))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct ErasePerson {
    /// Your own password, typed again (an operator who signs in only with single sign-on confirms with a
    /// sign-in from the last 10 minutes instead).
    #[serde(default)]
    password: Option<String>,
    /// Confirms closing any workspace whose only member the person is.
    #[serde(default)]
    close_workspace: bool,
}

#[utoipa::path(
    operation_id = "erasePerson",
    post,
    path = "/api/admin/people/{user_id}/erasure",
    tag = "admin",
    summary = "Erase someone's whole account and data (server operators only, browser session, confirmed with your own password)",
    description = "Only a server operator (an address in EUNOMIA_ADMIN_EMAILS, or the server's first account): the whole account in every workspace, as if the person had deleted it themselves. An admin of a workspace who is not an operator can't erase anyone in this release. Refused for your own account (use Settings > Data), for the first account and other operators' accounts, with `subject.legal_hold` while the account is held, with `subject.last_owner` when the person owns a workspace with other members, and with `subject.workspace_closes` until you confirm closing a workspace the person is the only member of.",
    params(("user_id" = String, Path)),
    request_body = ErasePerson,
    responses((status = 200, body = ErasureTicket), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn erase_person(State(state): State<AppState>, user: User, headers: HeaderMap, Path(user_id): Path<String>, Json(body): Json<ErasePerson>) -> AppResult<Json<ErasureTicket>> {
    authz::require_session()?;
    if !authz::is_server_operator(&state.control, &user).await? {
        return Err(AppError::coded(ErrorCode::AuthForbidden, "Only the server operator can erase someone's account. Ask them, or ask the person to delete it in Settings > Data."));
    }
    let target: RecordId = crate::rid::parse(&user_id).map_err(|_| AppError::coded(ErrorCode::AuthNotFound, "That account doesn't exist."))?;
    if target.table() != "user" {
        return Err(AppError::coded(ErrorCode::AuthNotFound, "That account doesn't exist."));
    }
    if target == user.id {
        return Err(AppError::bad_request("To delete your own account, use Settings > Data."));
    }
    let res = async {
        let signed_in_at = subject::session_signed_in_at(&state.control, &user.id, crate::auth::session_sid(&state.settings.jwt_secret, &headers)).await?;
        let how = Confirmation::Admin { admin: &user.id, password: body.password.as_deref(), signed_in_at };
        subject::confirm(&state.control, &target, how).await?;
        subject::request_erasure(&state, &target, Requester::Operator(user.id.clone()), body.close_workspace).await
    }
    .await;
    if let Err(e) = &res {
        crate::audit::record_as_caller(&state.control, &user.id, "subject.erase_request", "", e.code.as_str()).await;
    }
    Ok(Json(res?))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct HoldChange {
    /// `true` places a legal hold (the account cannot be erased), `false` lifts it.
    hold: bool,
}

#[derive(Serialize, utoipa::ToSchema)]
struct HoldState {
    /// When the hold was placed, RFC 3339; none when there is no hold.
    #[schema(value_type = Option<String>)]
    legal_hold_at: Option<Datetime>,
    /// An erasure of this account is open. A hold placed now parks it (`held`) before its next
    /// irreversible step; lifting the hold resumes it.
    erasure_open: bool,
}

#[utoipa::path(
    operation_id = "setPersonLegalHold",
    put,
    path = "/api/admin/people/{user_id}/legal-hold",
    tag = "admin",
    summary = "Place or lift a legal hold on someone's account (admin places, server operator lifts; browser session)",
    description = "Any admin places a hold; only a server operator lifts it, and never a hold on their own account. Who placed it and who lifted it are recorded. While held, erasing the account is refused with `subject.legal_hold`, and an erasure already queued parks as `held` before its next irreversible step and resumes when the hold is lifted. Placing and lifting are both in the activity log, with who did each. A hold does not stop the audit retention window (AUDIT_RETENTION_DAYS).",
    params(("user_id" = String, Path)),
    request_body = HoldChange,
    responses((status = 200, body = HoldState), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn set_legal_hold(State(state): State<AppState>, user: User, Path(user_id): Path<String>, Json(body): Json<HoldChange>) -> AppResult<Json<HoldState>> {
    let (_, target, email) = crate::routers::admin::person_in_workspace(&state, &user, &user_id).await?;
    let placed_by = subject::hold_placed_by(&state.control, &target).await?;
    let (at, erasure_open) = subject::set_legal_hold(&state.control, &user, &target, body.hold).await?;
    let action = if body.hold { "subject.legal_hold" } else { "subject.legal_hold_lift" };
    let mut detail = format!("email={email} by={}", user.email);
    if let (false, Some(p)) = (body.hold, placed_by) {
        detail.push_str(&format!(" placed_by={}", crate::models_user::email_of(&state.control, &p).await.unwrap_or_default()));
    }
    crate::audit::record_as_caller_detail(&state.control, &user.id, action, &target.to_string(), "ok", &detail).await;
    Ok(Json(HoldState { legal_hold_at: at, erasure_open }))
}

#[utoipa::path(
    operation_id = "makeWorkspaceOwner",
    put,
    path = "/api/admin/people/{user_id}/workspace-owner",
    tag = "admin",
    summary = "Hand the workspace over to another member (the current owner or a server operator, browser session)",
    description = "A workspace has one owner. Only the current owner or a server operator hands it over, to an active member who is not suspended, not under a legal hold and not being erased. The change promotes exactly one membership or changes nothing, so the workspace always keeps an owner.",
    params(("user_id" = String, Path)),
    responses((status = 200, body = crate::routers::admin::PeopleReportRef), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn make_workspace_owner(State(state): State<AppState>, user: User, Path(user_id): Path<String>) -> AppResult<Response> {
    let (_, target, email) = crate::routers::admin::person_in_workspace(&state, &user, &user_id).await?;
    let previous = subject::transfer_workspace(&state.control, &user, &user.org, &target).await?;
    let from = crate::models_user::email_of(&state.control, &previous).await.unwrap_or_default();
    let detail = format!("email={email} from={from}");
    crate::audit::record_as_caller_detail(&state.control, &user.id, "org.owner_transfer", &target.to_string(), "ok", &detail).await;
    if previous != user.id {
        crate::audit::record_as_caller_detail(&state.control, &previous, "org.owner_transfer", &target.to_string(), "ok", &detail).await;
    }
    crate::routers::admin::people_response(&state, &user).await
}

#[utoipa::path(
    operation_id = "listErasures",
    get,
    path = "/api/admin/erasures",
    tag = "admin",
    summary = "Erasure records for the workspace, newest first (admin only)",
    description = "Each record keeps only an opaque reference to the person, the counts per category, the post-check (every value 0 when it passed) and the mentions of their email left in other people's content, for review.",
    responses((status = 200, body = Vec<ErasureRecord>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_erasures(State(state): State<AppState>, user: User) -> AppResult<Json<Vec<ErasureRecord>>> {
    crate::routers::admin::require_admin(&state, &user).await?;
    Ok(Json(subject::list_erasures(&state.control, &user.org).await?))
}

#[derive(Serialize, utoipa::ToSchema)]
struct Reapplied {
    /// Erasures queued again: accounts a restore brought back, and restored rows whose owner was erased.
    queued: usize,
}

#[utoipa::path(
    operation_id = "reapplyErasures",
    post,
    path = "/api/admin/erasures/reapply",
    tag = "admin",
    summary = "After restoring a backup: erase again everyone erased after the backup was taken (server operators only, browser session)",
    description = "Also runs by itself when the server starts. Matches live accounts, and the owners of rows in every workspace, against the erasure records; queues an erasure for each match. It acts on the whole server, like a restore.",
    responses((status = 200, body = Reapplied), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn reapply_erasures(State(state): State<AppState>, user: User) -> AppResult<Json<Reapplied>> {
    authz::require_session()?;
    if !authz::is_server_operator(&state.control, &user).await? {
        return Err(AppError::coded(ErrorCode::AuthForbidden, "Only a server operator can re-apply erasures: it acts on the whole server."));
    }
    subject::heal_open(&state).await?;
    let queued = subject::reapply_erasures(&state).await?;
    Ok(Json(Reapplied { queued }))
}

#[utoipa::path(
    operation_id = "cancelErasure",
    post,
    path = "/api/admin/erasures/{erasure_id}/cancel",
    tag = "admin",
    summary = "Cancel an erasure that has not started deleting (server operators only, browser session)",
    description = "Stops an erasure before its first irreversible step, clears its working record (the email, token ids and file paths it held) and leaves the account exactly as it was (unsuspended if the request suspended it). Refused with `db.conflict` once deletion has begun: from then on it completes, or waits while held or failed and retried.",
    params(("erasure_id" = String, Path)),
    responses((status = 200, body = Vec<ErasureRecord>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn cancel_erasure(State(state): State<AppState>, user: User, Path(erasure_id): Path<String>) -> AppResult<Json<Vec<ErasureRecord>>> {
    authz::require_session()?;
    if !authz::is_server_operator(&state.control, &user).await? {
        return Err(AppError::coded(ErrorCode::AuthForbidden, "Only a server operator can cancel an erasure."));
    }
    subject::cancel_erasure(&state.control, &erasure_id).await?;
    crate::audit::record_as_caller(&state.control, &user.id, "subject.erase_cancel", &erasure_id, "ok").await;
    Ok(Json(subject::list_erasures(&state.control, &user.org).await?))
}

#[utoipa::path(
    operation_id = "myDataActivity",
    get,
    path = "/api/account/activity",
    tag = "export",
    summary = "What has been done about your data: exports an admin made of it, erasure requests, legal holds, admin actions on your account",
    responses((status = 200, body = Vec<subject::ActivityRow>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn my_activity(State(state): State<AppState>, user: User) -> AppResult<Json<Vec<subject::ActivityRow>>> {
    Ok(Json(subject::member_activity(&state.control, &user.id).await?))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    start_my_export,
    list_my_exports,
    download_export,
    erase_me,
    start_person_export,
    erase_person,
    set_legal_hold,
    make_workspace_owner,
    list_erasures,
    reapply_erasures,
    cancel_erasure,
    my_activity
))]
pub struct Doc;
