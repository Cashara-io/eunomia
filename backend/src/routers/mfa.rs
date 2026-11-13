//! Two-step verification routes (see `mfa.rs` and docs/security/mfa.md).
//!
//! - `/auth/mfa/challenge/*`: the second step of a password sign-in. No session yet: these read the
//!   short-lived `eunomia_mfa` cookie that login and register set, and are limited per client address like login.
//! - `/auth/mfa*`: the signed-in person's own settings, from a browser session only.

use axum::{
    extract::{Extension, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::auth::Via;
use crate::authz;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::gate::ForwardedHttps;
use crate::mfa::{self, Enrolment, MfaStatus};
use crate::models_user::{self, User};
use crate::routers::auth::{auth_out, session_response, AuthOut};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/mfa", get(get_status))
        .route("/auth/mfa/enrol", post(enrol))
        .route("/auth/mfa/enrol/confirm", post(enrol_confirm))
        .route("/auth/mfa/disable", post(disable))
        .route("/auth/mfa/recovery-codes", post(regenerate))
        .route("/auth/mfa/challenge/verify", post(challenge_verify))
        .route("/auth/mfa/challenge/enrol", post(challenge_enrol))
        .route("/auth/mfa/challenge/enrol/confirm", post(challenge_enrol_confirm))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct CodeIn {
    /// A six-digit authenticator code, or (where a recovery code is accepted) a recovery code.
    code: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct EnrolIn {
    /// Your account password: setting up an authenticator from Settings needs it, so a stolen session alone
    /// cannot put an attacker's authenticator on the account.
    #[serde(default)]
    password: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct RecoveryCodesOut {
    /// Shown once. Each works once, in place of an authenticator code.
    recovery_codes: Vec<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct EnrolledOut {
    /// The signed-in user (the session cookie is set with this answer).
    me: AuthOut,
    /// Shown once. Each works once, in place of an authenticator code.
    recovery_codes: Vec<String>,
}

const PROBLEM: &str = "application/problem+json";

#[utoipa::path(
    operation_id = "getMfa",
    get,
    path = "/api/auth/mfa",
    tag = "auth",
    summary = "Two-step verification for your account",
    responses((status = 200, body = StatusOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = PROBLEM)),
    security(("cookie" = [])),
)]
async fn get_status(State(state): State<AppState>, user: User) -> AppResult<Json<StatusOut>> {
    authz::require_session()?;
    let mut status = mfa::status(&state.control, &user.id).await?;
    status.unreadable = mfa::secret_unreadable(&state, &user.id).await?;
    Ok(Json(StatusOut { status, can_require: authz::is_instance_operator(&state.control, &user).await? }))
}

#[derive(Serialize, utoipa::ToSchema)]
struct StatusOut {
    #[serde(flatten)]
    status: MfaStatus,
    /// You may require two-step verification for the whole server (`/api/admin/security`): the server's operator.
    can_require: bool,
}

#[utoipa::path(
    operation_id = "startMfaEnrolment",
    post,
    path = "/api/auth/mfa/enrol",
    tag = "auth",
    summary = "Start setting up an authenticator app",
    description = "Needs the account password (10 wrong passwords per account in 15 minutes and it is refused, like login). Returns a new secret, its `otpauth://` URI and a QR code. Nothing changes until a code from the app confirms it (`/api/auth/mfa/enrol/confirm`). The secret is never shown again. Accounts that sign in only through single sign-on cannot enrol here: their provider handles the second factor.",
    request_body = EnrolIn,
    responses((status = 200, body = Enrolment), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = PROBLEM)),
    security(("cookie" = [])),
)]
async fn enrol(State(state): State<AppState>, user: User, Json(body): Json<EnrolIn>) -> AppResult<Json<Enrolment>> {
    authz::require_session()?;
    let status = mfa::status(&state.control, &user.id).await?;
    if !status.available {
        return Err(AppError::bad_request("This account signs in through single sign-on. Your identity provider handles two-step verification."));
    }
    // the password again, counted before it is checked (database-backed, every replica)
    mfa::reserve_reauth(&state, &user.id).await?;
    let password = body.password.unwrap_or_default();
    if password.is_empty() || !models_user::password_matches(&state.control, &user.id, &password).await? {
        let actor = authz::current().map(|c| c.actor).unwrap_or(authz::Actor { kind: "user", id: crate::rid::RecordIdExt::to_string(&user.id) });
        crate::audit::record(&state.control, crate::audit::Event { user: Some(&user.id), actor: &actor, action: "auth.reauth", target: "", outcome: ErrorCode::AuthForbidden.as_str(), detail: "where=mfa-setup" }).await;
        // 403, not 401: the session is fine, only the password typed again was wrong
        return Err(AppError::coded(ErrorCode::AuthForbidden, "That password isn't right."));
    }
    mfa::reauth_passed(&state, &user.id).await?;
    Ok(Json(mfa::begin_enrolment(&state, &user.id).await?))
}

#[utoipa::path(
    operation_id = "confirmMfaEnrolment",
    post,
    path = "/api/auth/mfa/enrol/confirm",
    tag = "auth",
    summary = "Turn on two-step verification with a code from the new authenticator",
    request_body = CodeIn,
    responses((status = 200, body = RecoveryCodesOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = PROBLEM)),
    security(("cookie" = [])),
)]
async fn enrol_confirm(State(state): State<AppState>, user: User, headers: HeaderMap, Json(body): Json<CodeIn>) -> AppResult<Json<RecoveryCodesOut>> {
    authz::require_session()?;
    // the code just proved the factor: this session counts as verified; the account's other unverified sessions end
    let sid = crate::auth::session_sid(&state.settings.jwt_secret, &headers);
    let recovery_codes = mfa::confirm_enrolment(&state, &user.id, &body.code, None, sid.as_deref()).await?;
    Ok(Json(RecoveryCodesOut { recovery_codes }))
}

#[utoipa::path(
    operation_id = "disableMfa",
    post,
    path = "/api/auth/mfa/disable",
    tag = "auth",
    summary = "Turn off two-step verification",
    description = "Needs a current authenticator code or an unused recovery code, every time. Refused while an admin requires two-step verification, unless the stored authenticator can no longer be read (`auth.mfa_unreadable`, after an `ENCRYPTION_KEY` change): then a recovery code turns it off so it can be set up again.",
    request_body = CodeIn,
    responses((status = 200, body = MfaStatus), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = PROBLEM)),
    security(("cookie" = [])),
)]
async fn disable(State(state): State<AppState>, user: User, Json(body): Json<CodeIn>) -> AppResult<Json<MfaStatus>> {
    authz::require_session()?;
    if mfa::require_mfa(&state.control).await? && !mfa::secret_unreadable(&state, &user.id).await? {
        return Err(AppError::coded(ErrorCode::AuthForbidden, "Your admin requires two-step verification on this server, so it can't be turned off."));
    }
    if !mfa::enabled(&state.control, &user.id).await? {
        return Ok(Json(mfa::status(&state.control, &user.id).await?));
    }
    mfa::verify(&state, &user.id, &body.code, "settings").await?;
    mfa::disable(&state.control, &user.id).await?;
    Ok(Json(mfa::status(&state.control, &user.id).await?))
}

#[utoipa::path(
    operation_id = "regenerateRecoveryCodes",
    post,
    path = "/api/auth/mfa/recovery-codes",
    tag = "auth",
    summary = "Replace your recovery codes",
    description = "Needs a current authenticator code or an unused recovery code. Every old recovery code stops working.",
    request_body = CodeIn,
    responses((status = 200, body = RecoveryCodesOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = PROBLEM)),
    security(("cookie" = [])),
)]
async fn regenerate(State(state): State<AppState>, user: User, Json(body): Json<CodeIn>) -> AppResult<Json<RecoveryCodesOut>> {
    authz::require_session()?;
    if !mfa::enabled(&state.control, &user.id).await? {
        return Err(AppError::bad_request("Two-step verification is off, so there are no recovery codes."));
    }
    mfa::verify(&state, &user.id, &body.code, "settings").await?;
    Ok(Json(RecoveryCodesOut { recovery_codes: mfa::regenerate_recovery_codes(&state.control, &user.id).await? }))
}

/// Links the pending single sign-on identity to the account after its second step passed.
async fn link_after_second_step(state: &AppState, c: &mfa::Challenge, issuer: &str, subject: &str) -> AppResult<()> {
    let email = c.link_email.clone().unwrap_or_default();
    match models_user::link_identity(&state.control, &c.user, issuer, subject, &email).await {
        Ok(()) => {}
        // linked meanwhile (a second sign-in raced this one): fine if it is this account's
        Err(e) if matches!(e.code, ErrorCode::DbDuplicate | ErrorCode::AuthEmailTaken) => {
            let owner = models_user::user_by_identity(&state.control, issuer, subject).await?;
            if owner.map(|u| u.id) != Some(c.user.clone()) {
                return Err(AppError::coded(ErrorCode::AuthForbidden, "That single sign-on account is already linked to another Eunomia account."));
            }
            return Ok(());
        }
        Err(e) => return Err(e),
    }
    crate::store::control::AUTH_SESSION_REVOKE_OWNER.on(&state.control).bind(("owner", c.user.clone())).await?.check()?;
    state.credentials.clear();
    let actor = authz::Actor { kind: "user", id: crate::rid::RecordIdExt::to_string(&c.user) };
    crate::audit::record(&state.control, crate::audit::Event { user: Some(&c.user), actor: &actor, action: "auth.oidc_link", target: "", outcome: "ok", detail: "linked to the existing account with this email, after its second step" }).await;
    Ok(())
}

/// Records the sign-in after the second step (which already spent the challenge); the caller starts the session.
async fn finish(state: &AppState, c: &mfa::Challenge, detail: &str) -> AppResult<User> {
    let user = models_user::load_user(&state.control, c.user.clone(), c.email.clone()).await?;
    let actor = authz::Actor { kind: "user", id: crate::rid::RecordIdExt::to_string(&user.id) };
    crate::audit::record(&state.control, crate::audit::Event { user: Some(&user.id), actor: &actor, action: "auth.login", target: "", outcome: "ok", detail }).await;
    Ok(user)
}

#[utoipa::path(
    operation_id = "verifyMfaChallenge",
    post,
    path = "/api/auth/mfa/challenge/verify",
    tag = "auth",
    summary = "Finish signing in with an authenticator or recovery code",
    description = "The second step after `/api/auth/login` answered `mfa_required: verify`. Reads the `eunomia_mfa` cookie that answer set. Each code counts against the sign-in (5 and it ends) and the account (10 in 15 minutes and every attempt is refused), reserved before it is checked.",
    request_body = CodeIn,
    responses((status = 200, body = AuthOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = PROBLEM)),
    security(()),
)]
async fn challenge_verify(State(state): State<AppState>, https: Option<Extension<ForwardedHttps>>, headers: HeaderMap, Json(body): Json<CodeIn>) -> AppResult<Response> {
    let c = mfa::find_challenge(&state.control, &headers, "verify").await?;
    let factor = mfa::verify_attempt(&state, &c.user, &body.code, "sign-in", Some(&c)).await?;
    // a single sign-on identity waiting on this step is linked only now, and only now are the account's
    // other sessions ended (as an immediate link would have done)
    if let (Some(issuer), Some(subject)) = (&c.link_issuer, &c.link_subject) {
        link_after_second_step(&state, &c, issuer, subject).await?;
    }
    let user = finish(&state, &c, &format!("second step: {}", factor.words())).await?;
    let clear = mfa::clear_challenge_cookie(&state.settings.public_url, https);
    session_response(&state, user, &headers, https, Via::parse(&c.via), true, Some(clear)).await
}

#[utoipa::path(
    operation_id = "startMfaChallengeEnrolment",
    post,
    path = "/api/auth/mfa/challenge/enrol",
    tag = "auth",
    summary = "Set up an authenticator during sign-in (when the server requires it)",
    description = "After `/api/auth/login` or `/api/auth/register` answered `mfa_required: enrol`. Returns the secret, `otpauth://` URI and QR code; confirm with `/api/auth/mfa/challenge/enrol/confirm`.",
    responses((status = 200, body = Enrolment), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = PROBLEM)),
    security(()),
)]
async fn challenge_enrol(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Json<Enrolment>> {
    let c = mfa::find_challenge(&state.control, &headers, "enrol").await?;
    Ok(Json(mfa::begin_enrolment(&state, &c.user).await?))
}

#[utoipa::path(
    operation_id = "confirmMfaChallengeEnrolment",
    post,
    path = "/api/auth/mfa/challenge/enrol/confirm",
    tag = "auth",
    summary = "Confirm the new authenticator and finish signing in",
    description = "Turns two-step verification on, starts the session and returns the recovery codes (shown once).",
    request_body = CodeIn,
    responses((status = 200, body = EnrolledOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = PROBLEM)),
    security(()),
)]
async fn challenge_enrol_confirm(State(state): State<AppState>, https: Option<Extension<ForwardedHttps>>, headers: HeaderMap, Json(body): Json<CodeIn>) -> AppResult<Response> {
    let c = mfa::find_challenge(&state.control, &headers, "enrol").await?;
    // the password was proved by this sign-in: no second ask
    let recovery_codes = mfa::confirm_enrolment(&state, &c.user, &body.code, Some(&c), None).await?;
    let user = finish(&state, &c, "second step: set up an authenticator").await?;
    let token = crate::auth::start_session(&state.control, &state.settings.jwt_secret, &user, crate::routers::auth::user_agent_from(&headers).as_deref(), Via::parse(&c.via), true).await?;
    let me = auth_out(&state, user).await?;
    let mut resp = (StatusCode::OK, Json(EnrolledOut { me, recovery_codes })).into_response();
    for cookie in [
        crate::routers::auth::session_cookie_header(&token, &state.settings.public_url, https),
        mfa::clear_challenge_cookie(&state.settings.public_url, https),
    ] {
        if let Ok(v) = header::HeaderValue::from_str(&cookie) {
            resp.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    Ok(resp)
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(get_status, enrol, enrol_confirm, disable, regenerate, challenge_verify, challenge_enrol, challenge_enrol_confirm))]
pub struct Doc;
