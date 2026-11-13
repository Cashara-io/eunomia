//! Data-subject rights over HTTP (docs/privacy/subject-rights.md): the full export of what Eunomia
//! holds about a person and their verified erasure, for the person themselves (Settings > Data) and
//! for a workspace admin acting for someone in the workspace (Settings > People), plus legal holds,
//! the workspace-owner handover and the erasure records. Starting or downloading an export and every
//! route that changes something need a browser session: no API token or app.

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use surrealdb::types::Datetime;

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
        .route("/admin/people/{user_id}/export", post(start_person_export))
        .route("/admin/people/{user_id}/erasure", post(erase_person))
        .route("/admin/people/{user_id}/legal-hold", put(set_legal_hold))
        .route("/admin/people/{user_id}/workspace-owner", put(make_workspace_owner))
        .route("/admin/erasures", get(list_erasures))
        .route("/admin/erasures/reapply", post(reapply_erasures))
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
    Ok(Json(subject::export::request_export(&state, &user.id, &user.id).await?))
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

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct DownloadQuery {
    /// Required when an admin downloads someone else's export (allowed only while that account is
    /// suspended): why. Recorded in both people's activity.
    #[serde(default)]
    reason: Option<String>,
}

// open body: a zip download (Content-Disposition attachment), not consumed through the typed client
#[utoipa::path(
    operation_id = "downloadSubjectExport",
    get,
    path = "/api/export/subject/{export_id}",
    tag = "export",
    summary = "Download a ready data export (zip; browser session)",
    description = "The person downloads their own. An admin who started one for someone downloads it only while that account is suspended, with a reason.",
    params(("export_id" = String, Path, description = "The export's id, as listed"), DownloadQuery),
    responses((status = 200, description = "The zip archive", content_type = "application/zip"), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn download_export(State(state): State<AppState>, user: User, Path(export_id): Path<String>, Query(q): Query<DownloadQuery>) -> AppResult<Response> {
    authz::require_session()?;
    let org = state.org(&user.org).await?;
    let bytes = subject::export::archive(&state, &org, &user.id, &export_id, q.reason.as_deref())
        .await?
        .ok_or_else(|| AppError::coded(ErrorCode::ResourceNotFound, "That export doesn't exist or has expired."))?;
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
    /// Your email, typed again: the confirmation for an account that signs in only with single sign-on.
    #[serde(default)]
    confirm_email: Option<String>,
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
    description = "Checks the confirmation, then suspends the account and signs it out everywhere, and queues the erasure. Refused with `subject.legal_hold` while an admin holds the account, `subject.last_owner` when you own a workspace that has other members and no other owner, and `subject.workspace_closes` until you confirm closing a workspace you are the only member of. What is deleted and what is kept: docs/privacy/subject-rights.md.",
    request_body = EraseMe,
    responses((status = 200, body = ErasureTicket), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn erase_me(State(state): State<AppState>, user: User, Json(body): Json<EraseMe>) -> AppResult<Json<ErasureTicket>> {
    authz::require_session()?;
    let res = async {
        subject::confirm(&state.control, &user.id, Confirmation::SelfService { password: body.password.as_deref(), email: body.confirm_email.as_deref() }).await?;
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
    description = "The same archive the person gets from Settings > Data, delivered to them: it appears in their Settings > Data, and their activity records that an admin started it. You see its status and counts; you can download it only while their account is suspended, with a reason.",
    params(("user_id" = String, Path)),
    responses((status = 200, body = ExportRow), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn start_person_export(State(state): State<AppState>, user: User, Path(user_id): Path<String>) -> AppResult<Json<ExportRow>> {
    let (_, target, _) = crate::routers::admin::person_in_workspace(&state, &user, &user_id).await?;
    Ok(Json(subject::export::request_export(&state, &target, &user.id).await?))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct ErasePerson {
    /// Your own password, typed again (an admin who signs in only with single sign-on types the
    /// person's email in `confirm_email` instead).
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    confirm_email: Option<String>,
    /// Confirms closing any workspace whose only member the person is.
    #[serde(default)]
    close_workspace: bool,
}

#[utoipa::path(
    operation_id = "erasePerson",
    post,
    path = "/api/admin/people/{user_id}/erasure",
    tag = "admin",
    summary = "Delete someone's account and data (admin only, browser session, confirmed with your own password)",
    description = "Refused for your own account (use Settings > Data), with `subject.legal_hold` while the account is held, and with `subject.last_owner` when the person owns a workspace with other members and nobody else owns it, or is the account that set up the server.",
    params(("user_id" = String, Path)),
    request_body = ErasePerson,
    responses((status = 200, body = ErasureTicket), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn erase_person(State(state): State<AppState>, user: User, Path(user_id): Path<String>, Json(body): Json<ErasePerson>) -> AppResult<Json<ErasureTicket>> {
    let (_, target, _) = crate::routers::admin::person_in_workspace(&state, &user, &user_id).await?;
    if target == user.id {
        return Err(AppError::bad_request("To delete your own account, use Settings > Data."));
    }
    let res = async {
        let how = Confirmation::Admin { admin: &user.id, password: body.password.as_deref(), email: body.confirm_email.as_deref() };
        subject::confirm(&state.control, &target, how).await?;
        subject::request_erasure(&state, &target, Requester::Admin(user.id.clone()), body.close_workspace).await
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
    summary = "Place or lift a legal hold on someone's account (admin only, browser session)",
    description = "While held, erasing the account is refused with `subject.legal_hold`, and an erasure already queued parks as `held` before its next irreversible step and resumes when the hold is lifted. Placing and lifting are both in the activity log. A hold does not stop the audit retention window (AUDIT_RETENTION_DAYS).",
    params(("user_id" = String, Path)),
    request_body = HoldChange,
    responses((status = 200, body = HoldState), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn set_legal_hold(State(state): State<AppState>, user: User, Path(user_id): Path<String>, Json(body): Json<HoldChange>) -> AppResult<Json<HoldState>> {
    let (_, target, email) = crate::routers::admin::person_in_workspace(&state, &user, &user_id).await?;
    let (at, erasure_open) = subject::set_legal_hold(&state.control, &target, body.hold).await?;
    let action = if body.hold { "subject.legal_hold" } else { "subject.legal_hold_lift" };
    crate::audit::record_as_caller_detail(&state.control, &user.id, action, &target.to_string(), "ok", &format!("email={email}")).await;
    Ok(Json(HoldState { legal_hold_at: at, erasure_open }))
}

#[utoipa::path(
    operation_id = "makeWorkspaceOwner",
    put,
    path = "/api/admin/people/{user_id}/workspace-owner",
    tag = "admin",
    summary = "Make someone the workspace's owner in place of the current owner (admin only, browser session)",
    description = "A workspace has one owner. Handing it over lets the previous owner's account be erased while the workspace keeps an owner.",
    params(("user_id" = String, Path)),
    responses((status = 200, body = crate::routers::admin::PeopleReportRef), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn make_workspace_owner(State(state): State<AppState>, user: User, Path(user_id): Path<String>) -> AppResult<Response> {
    let (_, target, email) = crate::routers::admin::person_in_workspace(&state, &user, &user_id).await?;
    subject::transfer_workspace(&state.control, &user.org, &target).await?;
    crate::audit::record_as_caller_detail(&state.control, &user.id, "org.owner_transfer", &target.to_string(), "ok", &format!("email={email}")).await;
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
    summary = "After restoring a backup: erase again everyone erased after the backup was taken (admin only, browser session)",
    description = "Also runs by itself when the server starts. Matches live accounts, and the owners of rows in every workspace, against the erasure records; queues an erasure for each match. It acts on the whole server, like a restore.",
    responses((status = 200, body = Reapplied), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn reapply_erasures(State(state): State<AppState>, user: User) -> AppResult<Json<Reapplied>> {
    authz::require_session()?;
    crate::routers::admin::require_admin(&state, &user).await?;
    subject::heal_open(&state).await?;
    let queued = subject::reapply_erasures(&state).await?;
    Ok(Json(Reapplied { queued }))
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
    reapply_erasures
))]
pub struct Doc;
