//! Data-subject rights over HTTP (docs/privacy/subject-rights.md): the full export of what Eunomia
//! holds about a person and their verified erasure, for the person themselves (Settings > Data) and
//! for a workspace admin acting for someone in the workspace (Settings > People), plus legal holds
//! and the erasure records. Every route here that changes something needs a browser session.

use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use surrealdb::types::Datetime;

use crate::authz;
use crate::rid::RecordIdExt;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::User;
use crate::state::AppState;
use crate::subject::{self, export::ExportRow, Confirmation, ErasureRecord, ErasureTicket};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/export/subject", get(list_my_exports).post(start_my_export))
        .route("/export/subject/{export_id}", get(download_export))
        .route("/account/erasure", post(erase_me))
        .route("/admin/people/{user_id}/export", post(start_person_export))
        .route("/admin/people/{user_id}/erasure", post(erase_person))
        .route("/admin/people/{user_id}/legal-hold", put(set_legal_hold))
        .route("/admin/erasures", get(list_erasures))
        .route("/admin/erasures/reapply", post(reapply_erasures))
}

#[utoipa::path(
    operation_id = "startSubjectExport",
    post,
    path = "/api/export/subject",
    tag = "export",
    summary = "Prepare a full export of your data (a zip archive with a manifest)",
    description = "Queues a job that collects everything in scope about you (account, spaces, what you wrote in spaces you can still read, documents and their files, synced records, chat, sources, tokens and connected apps by name, your audit events) into a zip archive kept for seven days. List exports to see when it is ready.",
    responses((status = 200, body = ExportRow), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn start_my_export(State(state): State<AppState>, user: User) -> AppResult<Json<ExportRow>> {
    let org = state.org(&user.org).await?;
    Ok(Json(subject::export::request_export(&org, &user.id, &user.id).await?))
}

#[utoipa::path(
    operation_id = "listSubjectExports",
    get,
    path = "/api/export/subject",
    tag = "export",
    summary = "Your data exports, and the ones you asked for as an admin, newest first",
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
    summary = "Download a ready data export (zip)",
    params(("export_id" = String, Path, description = "The export's id, as listed")),
    responses((status = 200, description = "The zip archive", content_type = "application/zip"), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn download_export(State(state): State<AppState>, user: User, Path(export_id): Path<String>) -> AppResult<Response> {
    let org = state.org(&user.org).await?;
    let bytes = subject::export::archive(&org, &user.id, &export_id)
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
}

#[utoipa::path(
    operation_id = "eraseMyAccount",
    post,
    path = "/api/account/erasure",
    tag = "export",
    summary = "Delete your account and your data (browser session, confirmed with your password)",
    description = "Checks the confirmation, then suspends the account and signs it out everywhere, and queues the erasure. Refused with `subject.legal_hold` while an admin holds the account, and with `subject.last_owner` for the workspace's last owner. What is deleted and what is kept: docs/privacy/subject-rights.md.",
    request_body = EraseMe,
    responses((status = 200, body = ErasureTicket), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn erase_me(State(state): State<AppState>, user: User, Json(body): Json<EraseMe>) -> AppResult<Json<ErasureTicket>> {
    authz::require_session()?;
    let res = async {
        subject::confirm(&state.control, &user.id, Confirmation::SelfService { password: body.password.as_deref(), email: body.confirm_email.as_deref() }).await?;
        subject::request_erasure(&state, &user.id, "self").await
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
    description = "The same archive the person gets from Settings > Data. It is listed with your own exports and kept for seven days.",
    params(("user_id" = String, Path)),
    responses((status = 200, body = ExportRow), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn start_person_export(State(state): State<AppState>, user: User, Path(user_id): Path<String>) -> AppResult<Json<ExportRow>> {
    let (_, target, _) = crate::routers::admin::person_in_workspace(&state, &user, &user_id).await?;
    let org = state.org(&user.org).await?;
    Ok(Json(subject::export::request_export(&org, &target, &user.id).await?))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct ErasePerson {
    /// The person's email, typed by the admin to confirm.
    confirm_email: String,
}

#[utoipa::path(
    operation_id = "erasePerson",
    post,
    path = "/api/admin/people/{user_id}/erasure",
    tag = "admin",
    summary = "Delete someone's account and data (admin only, browser session, confirmed with their email)",
    description = "Refused for your own account (use Settings > Data), with `subject.legal_hold` while the account is held, and with `subject.last_owner` for the workspace's last owner or the account that set up the server.",
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
        subject::confirm(&state.control, &target, Confirmation::Admin { email: &body.confirm_email }).await?;
        subject::request_erasure(&state, &target, "admin").await
    }
    .await;
    if let Err(e) = &res {
        crate::audit::record_as_caller(&state.control, &user.id, "subject.erase_request", &target.to_string(), e.code.as_str()).await;
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
}

#[utoipa::path(
    operation_id = "setPersonLegalHold",
    put,
    path = "/api/admin/people/{user_id}/legal-hold",
    tag = "admin",
    summary = "Place or lift a legal hold on someone's account (admin only, browser session)",
    description = "While held, erasing the account is refused with `subject.legal_hold`. Placing and lifting are both in the activity log. A hold does not stop the audit retention window (AUDIT_RETENTION_DAYS).",
    params(("user_id" = String, Path)),
    request_body = HoldChange,
    responses((status = 200, body = HoldState), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn set_legal_hold(State(state): State<AppState>, user: User, Path(user_id): Path<String>, Json(body): Json<HoldChange>) -> AppResult<Json<HoldState>> {
    let (_, target, email) = crate::routers::admin::person_in_workspace(&state, &user, &user_id).await?;
    let at = subject::set_legal_hold(&state.control, &target, body.hold).await?;
    let action = if body.hold { "subject.legal_hold" } else { "subject.legal_hold_lift" };
    crate::audit::record_as_caller_detail(&state.control, &user.id, action, &target.to_string(), "ok", &format!("email={email}")).await;
    Ok(Json(HoldState { legal_hold_at: at }))
}

#[utoipa::path(
    operation_id = "listErasures",
    get,
    path = "/api/admin/erasures",
    tag = "admin",
    summary = "Erasure records for the workspace, newest first (admin only)",
    description = "Each record keeps only an opaque reference to the person, the counts per category and the post-check (every value 0 when it passed).",
    responses((status = 200, body = Vec<ErasureRecord>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_erasures(State(state): State<AppState>, user: User) -> AppResult<Json<Vec<ErasureRecord>>> {
    crate::routers::admin::require_admin(&state, &user).await?;
    Ok(Json(subject::list_erasures(&state.control, &user.org).await?))
}

#[derive(Serialize, utoipa::ToSchema)]
struct Reapplied {
    /// Accounts erased before that a restore brought back, queued to be erased again.
    queued: usize,
}

#[utoipa::path(
    operation_id = "reapplyErasures",
    post,
    path = "/api/admin/erasures/reapply",
    tag = "admin",
    summary = "After restoring a backup: erase again everyone erased after the backup was taken (admin only, browser session)",
    description = "Also runs by itself when the server starts. Matches live accounts against the erasure records; queues an erasure for each match.",
    responses((status = 200, body = Reapplied), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn reapply_erasures(State(state): State<AppState>, user: User) -> AppResult<Json<Reapplied>> {
    authz::require_session()?;
    crate::routers::admin::require_admin(&state, &user).await?;
    let queued = subject::reapply_erasures(&state).await?;
    Ok(Json(Reapplied { queued }))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(start_my_export, list_my_exports, download_export, erase_me, start_person_export, erase_person, set_legal_hold, list_erasures, reapply_erasures))]
pub struct Doc;
