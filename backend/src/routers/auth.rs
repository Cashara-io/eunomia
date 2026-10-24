//! Auth routes: register/login issue the session cookie, `me`/`token` require
//! it (or a Bearer API token, via the `User` extractor).

use surrealdb::types::SurrealValue;
use axum::{
    extract::{Extension, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use surrealdb::types::{Datetime, RecordId};
use crate::rid::RecordIdExt;

use crate::audit::{self, Event};
use crate::auth::{self, SESSION_COOKIE};
use crate::authz;
use crate::gate::ForwardedHttps;
use crate::scopes;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::store;
use crate::models_user::{self, User};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
        .route("/auth/tokens", post(create_token).get(get_tokens))
        .route("/auth/tokens/{token_id}", delete(delete_token).patch(rename_token))
        .route("/auth/sessions", get(get_sessions))
        .route("/auth/sessions/{session_id}", delete(revoke_session_route))
        .route("/auth/bootstrap", get(bootstrap))
}

#[derive(Serialize, utoipa::ToSchema)]
struct AuthOut {
    id: String,
    email: String,
    onboarded: bool,
    /// False for an account made by an OpenID Connect provider: it has no password to change or use.
    has_password: bool,
    /// The display name of the configured OpenID Connect provider when this account can sign in with it.
    oidc_provider: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct TokenCreated {
    id: String,
    name: String,
    /// Shown once, never retrievable again.
    token: String,
    scopes: Vec<String>,
    vault_id: Option<String>,
    #[schema(value_type = Option<String>)]
    expires_at: Option<Datetime>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct BootstrapOut {
    has_users: bool,
    /// Which sign-in methods the login and register pages offer.
    auth_mode: AuthModeOut,
    /// The button label ("Continue with {oidc_provider}") when an OpenID Connect provider is configured.
    oidc_provider: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
enum AuthModeOut {
    Password,
    Both,
    Oidc,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct Credentials {
    email: String,
    password: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct TokenCreate {
    name: String,
    /// Any of `memory:read`, `memory:write`, `vaults:admin`, `connectors`. Defaults
    /// to all of them. Cannot exceed the creating credential's own scopes.
    #[serde(default)]
    scopes: Option<Vec<String>>,
    /// Restrict the token to one vault you belong to.
    #[serde(default)]
    vault_id: Option<String>,
    /// RFC 3339 timestamp in the future. Omit for a token that never expires.
    #[serde(default)]
    expires_at: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct TokenRename {
    /// The new name: trimmed, 1 to 80 characters.
    name: String,
}

/// Longest token name, in characters (the same limit as a vault name).
const MAX_TOKEN_NAME: usize = 80;

fn check_token_name(name: &str) -> AppResult<()> {
    if name.chars().count() > MAX_TOKEN_NAME {
        return Err(AppError::bad_request(format!("Token names are at most {MAX_TOKEN_NAME} characters.")));
    }
    Ok(())
}

pub(crate) fn user_agent_from(headers: &HeaderMap) -> Option<String> {
    headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()).map(str::to_string)
}

/// The `AuthOut` for a user: onboarding state, whether a password exists, and the linked provider.
async fn auth_out(state: &AppState, user: User) -> AppResult<AuthOut> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        onboarded_at: Option<Datetime>,
        #[serde(default)]
        #[surreal(default)]
        password_hash: Option<String>,
    }
    let row: Option<Row> = store::get_control(&state.control, &user.id).await?;
    let oidc_provider = match &state.settings.auth.oidc {
        Some(cfg) => {
            #[derive(Deserialize, SurrealValue)]
            struct Link {
                issuer: String,
            }
            let mut res = store::control::IDENTITY_LIST_FOR_USER.on(&state.control).bind(("user", user.id.clone())).await?;
            let links: Vec<Link> = res.take(0)?;
            links.iter().any(|l| l.issuer == cfg.issuer).then(|| cfg.provider_name.clone())
        }
        None => None,
    };
    Ok(AuthOut {
        id: user.id.to_string(),
        email: user.email,
        onboarded: row.as_ref().is_some_and(|r| r.onboarded_at.is_some()),
        has_password: row.is_some_and(|r| r.password_hash.is_some()),
        oidc_provider,
    })
}

/// Password sign-in and signup are off when `AUTH_MODE=oidc`.
pub(crate) fn require_password_enabled(state: &AppState) -> AppResult<()> {
    if state.settings.auth.password_enabled() {
        return Ok(());
    }
    Err(AppError::coded(ErrorCode::AuthPasswordDisabled, "This server signs in through a single sign-on provider. Email and password are turned off."))
}

/// `COOKIE_SECURE`: `true`, `false`, or `auto` (default). `auto` marks the cookie `Secure` when the
/// public URL is https or a trusted proxy says the browser connected over https (`X-Forwarded-Proto`).
/// The browser talks to the frontend origin, which may be https while `PUBLIC_URL` (the API) is not.
fn cookie_secure(mode: &str, public_url: &str, forwarded_https: bool) -> bool {
    match mode.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => true,
        "false" | "0" => false,
        _ => forwarded_https || public_url.starts_with("https://"),
    }
}

pub(crate) fn cookie_flags(public_url: &str, https: Option<Extension<ForwardedHttps>>) -> &'static str {
    let mode = std::env::var("COOKIE_SECURE").unwrap_or_default();
    if cookie_secure(&mode, public_url, https.is_some_and(|Extension(h)| h.0)) { "; Secure" } else { "" }
}

pub(crate) fn session_cookie_header(token: &str, public_url: &str, https: Option<Extension<ForwardedHttps>>) -> String {
    format!("{SESSION_COOKIE}={token}; HttpOnly; SameSite=Lax; Path=/{}", cookie_flags(public_url, https))
}

/// `SIGNUP`: who may create an account once the install has a first user.
/// `open` lets anyone who can reach the server sign up; `invite` only emails the operator listed in
/// `SIGNUP_ALLOWLIST` or `EUNOMIA_ADMIN_EMAILS` (a vault invitation needs an existing account, so it
/// cannot be the invitation); `closed` nobody. The first user can always sign up. Unknown values close.
pub(crate) fn signup_allowed(mode: &str, has_users: bool, email: &str, allowlist: &str) -> bool {
    if !has_users {
        return true;
    }
    match mode.trim().to_ascii_lowercase().as_str() {
        "" | "open" => true,
        "invite" => {
            let email = models_user::normalize_email(email);
            allowlist.split(',').map(models_user::normalize_email).any(|e| !e.is_empty() && e == email)
        }
        _ => false,
    }
}

/// Why signup was refused, in words the person and their admin can act on (issue #76: a bare
/// "403 forbidden" left a second user guessing). A reserved operator address reads like an
/// invite-only refusal, so it says nothing about which addresses are operators.
fn signup_refusal(mode: &str, reserved: bool) -> &'static str {
    match mode.trim().to_ascii_lowercase().as_str() {
        _ if reserved => "Signup on this server is by invitation only. Ask the admin to add your email to SIGNUP_ALLOWLIST.",
        "invite" => "Signup on this server is by invitation only. Ask the admin to add your email to SIGNUP_ALLOWLIST.",
        _ => "Signup is closed on this server (SIGNUP). Ask the admin to create your account or to open signup.",
    }
}

#[utoipa::path(
    operation_id = "register",
    post,
    path = "/api/auth/register",
    tag = "auth",
    summary = "Create an account and start a session",
    request_body = Credentials,
    responses((status = 200, body = AuthOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(()),
)]
async fn register(
    State(state): State<AppState>,
    https: Option<Extension<ForwardedHttps>>,
    headers: HeaderMap,
    Json(body): Json<Credentials>,
) -> AppResult<Response> {
    require_password_enabled(&state)?;
    let has_users = crate::capsules::first_user(&state.control).await?.is_some();
    // an operator address is not verified, so whoever signed up with it first would become operator:
    // it must already have an account before it is listed (same refusal text, nothing to probe)
    let reserved = has_users && authz::operator_email_listed(&body.email);
    let mode = std::env::var("SIGNUP").unwrap_or_default();
    if reserved || !signup_allowed(&mode, has_users, &body.email, &std::env::var("SIGNUP_ALLOWLIST").unwrap_or_default()) {
        tracing::info!(signup = %mode, reserved, "signup refused by the SIGNUP policy");
        return Err(AppError::coded(ErrorCode::AuthForbidden, signup_refusal(&mode, reserved)));
    }
    let user = models_user::register_user(&state, &body.email, &body.password)
        .await
        .map_err(|e| {
            if e.code == ErrorCode::DbDuplicate {
                AppError::coded(ErrorCode::AuthEmailTaken, "A user with that email already exists.")
            } else {
                e
            }
        })?;

    let token = auth::start_session(&state.control, &state.settings.jwt_secret, &user, user_agent_from(&headers).as_deref())
        .await?;
    audit::record_as_caller(&state.control, &user.id, "auth.register", "", "ok").await;

    let body = AuthOut { id: user.id.to_string(), email: user.email, onboarded: false, has_password: true, oidc_provider: None };
    Ok((
        StatusCode::OK,
        [(header::SET_COOKIE, session_cookie_header(&token, &state.settings.public_url, https))],
        Json(body),
    )
        .into_response())
}

#[utoipa::path(
    operation_id = "login",
    post,
    path = "/api/auth/login",
    tag = "auth",
    summary = "Log in and start a session",
    request_body = Credentials,
    responses((status = 200, body = AuthOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(()),
)]
async fn login(
    State(state): State<AppState>,
    https: Option<Extension<ForwardedHttps>>,
    ip: Option<Extension<crate::gate::ClientIp>>,
    headers: HeaderMap,
    Json(body): Json<Credentials>,
) -> AppResult<Response> {
    require_password_enabled(&state)?;
    // checked before the bcrypt work, only failures count. Lockout is per account and address, so a stranger
    // cannot lock a known email out; a looser per-account ceiling still stops guessing spread over addresses.
    let account_key = format!("login-fail:{}", models_user::normalize_email(&body.email));
    let fail_key = format!("{account_key}:{}", ip.as_ref().map_or("", |i| i.0 .0.as_str()));
    let throttled = state.fail_throttle.check(&fail_key, crate::ratelimit::LOGIN_FAILS).and_then(|()| state.fail_throttle.check(&account_key, crate::ratelimit::LOGIN_ACCOUNT_FAILS));
    if let Err(wait) = throttled {
        return Ok(crate::gate::rate_limited(wait));
    }
    let Some(user) = models_user::authenticate(&state.control, &body.email, &body.password).await? else {
        state.fail_throttle.fail(&fail_key);
        state.fail_throttle.fail(&account_key);
        let email = models_user::normalize_email(&body.email);
        let event = Event {
            user: None,
            actor: &audit::anonymous(),
            action: "auth.login",
            target: "",
            outcome: ErrorCode::AuthUnauthorized.as_str(),
            detail: &email,
        };
        audit::record(&state.control, event).await;
        return Err(AppError::unauthorized("Invalid email or password."));
    };

    state.fail_throttle.clear(&fail_key);
    state.fail_throttle.clear(&account_key);
    let token = auth::start_session(&state.control, &state.settings.jwt_secret, &user, user_agent_from(&headers).as_deref())
        .await?;
    audit::record_as_caller(&state.control, &user.id, "auth.login", "", "ok").await;
    let body = auth_out(&state, user).await?;
    Ok((
        StatusCode::OK,
        [(header::SET_COOKIE, session_cookie_header(&token, &state.settings.public_url, https))],
        Json(body),
    )
        .into_response())
}

#[utoipa::path(
    operation_id = "logout",
    post,
    path = "/api/auth/logout",
    tag = "auth",
    summary = "End the current session",
    responses((status = 200, body = crate::openapi::OkBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(()),
)]
async fn logout(State(state): State<AppState>, https: Option<Extension<ForwardedHttps>>, headers: HeaderMap) -> Response {
    if let Some(cookie_header) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok())
        && let Some(token) = cookie_header.split(';').find_map(|p| {
            let p = p.trim();
            p.strip_prefix(&format!("{SESSION_COOKIE}=")).map(str::to_string)
        }) {
            auth::revoke_session_by_jwt(&state, &token).await;
        }
    let expired = format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0{}", cookie_flags(&state.settings.public_url, https));
    (StatusCode::OK, [(header::SET_COOKIE, expired)], Json(json!({ "ok": true }))).into_response()
}

#[utoipa::path(
    operation_id = "getMe",
    get,
    path = "/api/auth/me",
    tag = "auth",
    summary = "The signed in user",
    responses((status = 200, body = AuthOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn me(State(state): State<AppState>, user: User) -> AppResult<Json<AuthOut>> {
    Ok(Json(auth_out(&state, user).await?))
}

#[utoipa::path(
    operation_id = "createToken",
    post,
    path = "/api/auth/tokens",
    tag = "auth",
    summary = "Create an API token",
    request_body = TokenCreate,
    responses((status = 200, body = TokenCreated), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn create_token(
    State(state): State<AppState>,
    user: User,
    Json(body): Json<TokenCreate>,
) -> AppResult<Json<TokenCreated>> {
    let name = {
        let trimmed = body.name.trim();
        if trimmed.is_empty() { "API token".to_string() } else { trimmed.to_string() }
    };
    check_token_name(&name)?;
    let scopes: Vec<String> = match body.scopes {
        None => scopes::ALL.iter().map(|s| s.to_string()).collect(),
        Some(s) if s.is_empty() => return Err(AppError::bad_request("Choose at least one scope.")),
        Some(mut s) => {
            if let Some(bad) = s.iter().find(|n| !scopes::is_known(n)) {
                return Err(AppError::bad_request(format!("Unknown scope {bad:?}.")));
            }
            s.sort();
            s.dedup();
            s
        }
    };
    // a token cannot hand out more than it holds
    for s in &scopes {
        authz::require_scope(s)?;
    }
    let vault = match body.vault_id.as_deref().filter(|v| !v.is_empty()) {
        None => None,
        Some(v) => {
            let rid: RecordId = crate::rid::parse(v)
                .ok()
                .filter(|r: &RecordId| r.table() == "vault")
                .ok_or_else(|| AppError::coded(ErrorCode::VaultNotFound, "Vault not found."))?;
            authz::ensure_member(&state.org(&user.org).await?.db, &user.id, &rid).await?;
            Some(rid)
        }
    };
    let expires = match body.expires_at.as_deref().filter(|v| !v.is_empty()) {
        None => None,
        Some(v) => {
            let at = chrono::DateTime::parse_from_rfc3339(v)
                .map_err(|_| AppError::bad_request("expires_at must be an RFC 3339 timestamp."))?
                .with_timezone(&chrono::Utc);
            if at <= chrono::Utc::now() {
                return Err(AppError::bad_request("expires_at must be in the future."));
            }
            Some(at)
        }
    };
    // a token cannot outlive itself (scopes are checked above; the gate keeps vault-restricted tokens off this route):
    // only a browser session mints a longer-lived token
    let me = authz::caller()?;
    if me.actor.kind == "token"
        && let Some(own) = me.expires_at
        && expires.is_none_or(|at| at > own)
    {
        return Err(AppError::coded(ErrorCode::AuthScope, "this token expires, so it can only mint tokens that expire no later than it does"));
    }
    let expires_at = expires.map(Datetime::from);
    let result = models_user::create_api_token_with(&state.control, &user.id, &name, &scopes, vault.as_ref(), expires_at).await?;
    audit::record_as_caller(&state.control, &user.id, "auth.token_create", &result.id.to_string(), "ok").await;
    Ok(Json(TokenCreated {
        id: result.id.to_string(),
        name: result.name,
        token: result.token,
        scopes: result.scopes,
        vault_id: result.vault.map(|v| v.to_string()),
        expires_at: result.expires_at,
    }))
}

#[derive(Serialize, utoipa::ToSchema)]
struct TokenOut {
    id: String,
    name: String,
    #[schema(value_type = Option<String>)]
    created_at: Option<Datetime>,
    #[schema(value_type = Option<String>)]
    last_used_at: Option<Datetime>,
    scopes: Vec<String>,
    vault_id: Option<String>,
    /// `null` for a token that never expires.
    #[schema(value_type = Option<String>)]
    expires_at: Option<Datetime>,
}

#[utoipa::path(
    operation_id = "listTokens",
    get,
    path = "/api/auth/tokens",
    tag = "auth",
    summary = "List API tokens",
    responses((status = 200, body = Vec<TokenOut>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_tokens(State(state): State<AppState>, user: User) -> AppResult<Json<Vec<TokenOut>>> {
    let rows = models_user::list_api_tokens(&state.control, &user.id).await?;
    Ok(Json(
        rows.into_iter().map(token_out).collect(),
    ))
}

fn token_out(r: models_user::ApiTokenSummary) -> TokenOut {
    TokenOut {
        id: r.id.to_string(),
        name: r.name,
        created_at: Some(r.created_at),
        last_used_at: r.last_used_at,
        scopes: r.scopes.into_iter().filter(|s| scopes::is_known(s)).collect(),
        vault_id: r.vault.map(|v| v.to_string()),
        expires_at: r.expires_at,
    }
}

#[utoipa::path(
    operation_id = "deleteToken",
    delete,
    path = "/api/auth/tokens/{token_id}",
    tag = "auth",
    summary = "Revoke an API token",
    params(("token_id" = String, Path)),
    responses((status = 200, body = crate::openapi::OkBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn delete_token(
    State(state): State<AppState>,
    user: User,
    Path(token_id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let rid: RecordId = crate::rid::parse(&token_id).map_err(|_| AppError::coded(ErrorCode::AuthNotFound, "Token not found."))?;
    let ok = models_user::revoke_api_token(&state.control, &user.id, &rid).await?;
    state.credentials.forget(&rid.to_string());
    if !ok {
        return Err(AppError::coded(ErrorCode::AuthNotFound, "Token not found."));
    }
    audit::record_as_caller(&state.control, &user.id, "auth.token_revoke", &rid.to_string(), "ok").await;
    Ok(Json(json!({ "ok": true })))
}

#[utoipa::path(
    operation_id = "renameToken",
    patch,
    path = "/api/auth/tokens/{token_id}",
    tag = "auth",
    summary = "Rename an API token",
    params(("token_id" = String, Path)),
    request_body = TokenRename,
    responses((status = 200, body = TokenOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn rename_token(
    State(state): State<AppState>,
    user: User,
    Path(token_id): Path<String>,
    Json(body): Json<TokenRename>,
) -> AppResult<Json<TokenOut>> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError::bad_request("Give the token a name."));
    }
    check_token_name(name)?;
    let not_found = || AppError::coded(ErrorCode::AuthNotFound, "Token not found.");
    let rid: RecordId = crate::rid::parse(&token_id).map_err(|_| not_found())?;
    let r = models_user::rename_api_token(&state.control, &user.id, &rid, name).await?.ok_or_else(not_found)?;
    audit::record_as_caller(&state.control, &user.id, "auth.token_rename", &rid.to_string(), "ok").await;
    Ok(Json(token_out(r)))
}

#[derive(Deserialize, SurrealValue)]
struct SessionRow {
    id: RecordId,
    sid: String,
    #[serde(default)]
    #[surreal(default)]
    user_agent: String,
    created_at: Datetime,
    last_seen_at: Datetime,
}

#[derive(Serialize, utoipa::ToSchema)]
struct SessionOut {
    id: String,
    user_agent: String,
    #[schema(value_type = String)]
    created_at: Datetime,
    #[schema(value_type = String)]
    last_seen_at: Datetime,
    /// This is the session the request itself is using.
    current: bool,
}

#[utoipa::path(
    operation_id = "listSessions",
    get,
    path = "/api/auth/sessions",
    tag = "auth",
    summary = "List active sessions",
    responses((status = 200, body = Vec<SessionOut>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_sessions(State(state): State<AppState>, user: User, headers: HeaderMap) -> AppResult<Json<Vec<SessionOut>>> {
    let mine = auth::session_sid(&state.settings.jwt_secret, &headers);
    let mut res = store::control::AUTH_SESSION_LIST
        .on(&state.control)
        .bind(("owner", user.id.clone()))
        .await?;
    let rows: Vec<SessionRow> = res.take(0)?;
    Ok(Json(
        rows.into_iter()
            .map(|r| SessionOut {
                id: r.id.to_string(),
                user_agent: r.user_agent,
                created_at: r.created_at,
                last_seen_at: r.last_seen_at,
                current: mine.as_deref() == Some(r.sid.as_str()),
            })
            .collect(),
    ))
}

#[utoipa::path(
    operation_id = "revokeSession",
    delete,
    path = "/api/auth/sessions/{session_id}",
    tag = "auth",
    summary = "Revoke a session",
    params(("session_id" = String, Path)),
    responses((status = 200, body = crate::openapi::OkBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn revoke_session_route(
    State(state): State<AppState>,
    user: User,
    Path(session_id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        owner: RecordId,
    }
    let rid: RecordId = crate::rid::parse(&session_id).map_err(|_| AppError::bad_request("Not a session id."))?;
    if rid.table() != "session" {
        return Err(AppError::bad_request("Not a session id."));
    }
    let row: Option<Row> = store::get_control(&state.control, &rid).await?;
    match row {
        Some(r) if r.owner == user.id => {}
        _ => return Err(AppError::coded(ErrorCode::AuthNotFound, "Session not found.")),
    }
    store::control::AUTH_SESSION_REVOKE.on(&state.control).bind(("id", rid.clone())).await?;
    state.credentials.forget(&rid.to_string());
    Ok(Json(json!({ "ok": true })))
}

#[utoipa::path(
    operation_id = "getBootstrap",
    get,
    path = "/api/auth/bootstrap",
    tag = "auth",
    summary = "Whether any user exists yet, and how users sign in",
    responses((status = 200, body = BootstrapOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(()),
)]
async fn bootstrap(State(state): State<AppState>) -> AppResult<Json<BootstrapOut>> {
    #[derive(Deserialize, SurrealValue)]
    struct CountRow {
        count: i64,
    }
    let mut res = store::control::AUTH_USER_COUNT.on(&state.control).await?;
    let rows: Vec<CountRow> = res.take(0)?;
    let has_users = rows.first().map(|r| r.count > 0).unwrap_or(false);
    let auth = &state.settings.auth;
    let auth_mode = match auth.mode {
        crate::oidc::AuthMode::Password => AuthModeOut::Password,
        crate::oidc::AuthMode::Both => AuthModeOut::Both,
        crate::oidc::AuthMode::Oidc => AuthModeOut::Oidc,
    };
    Ok(Json(BootstrapOut { has_users, auth_mode, oidc_provider: auth.oidc.as_ref().map(|o| o.provider_name.clone()) }))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    register,
    login,
    logout,
    me,
    create_token,
    get_tokens,
    delete_token,
    rename_token,
    get_sessions,
    revoke_session_route,
    bootstrap,
))]
pub struct Doc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_cookie_header_includes_name_and_flags() {
        let header = session_cookie_header("abc.def.ghi", "http://localhost:8001", None);
        assert!(header.starts_with("eunomia_session=abc.def.ghi;"));
        assert!(header.contains("HttpOnly"));
        assert!(header.contains("SameSite=Lax"));
        assert!(!header.contains("Secure"));
        assert!(session_cookie_header("t", "https://eunomia.example.com", None).ends_with("; Secure"));
    }

    #[test]
    fn empty_token_name_defaults_to_api_token() {
        let name = {
            let trimmed = "   ".trim();
            if trimmed.is_empty() { "API token".to_string() } else { trimmed.to_string() }
        };
        assert_eq!(name, "API token");
    }

    #[test]
    fn a_refusal_says_which_policy_refused() {
        assert!(signup_refusal("invite", false).contains("SIGNUP_ALLOWLIST"));
        assert!(signup_refusal("closed", false).contains("closed"));
        assert!(signup_refusal("junk", false).contains("closed"), "a typo closes, and says so");
        assert_eq!(signup_refusal("", true), signup_refusal("invite", false), "a reserved address reveals nothing more");
    }

    #[test]
    fn signup_modes() {
        // a fresh install always lets the first user in
        for m in ["open", "invite", "closed", "junk"] {
            assert!(signup_allowed(m, false, "a@x.com", ""), "{m}");
        }
        assert!(signup_allowed("", true, "a@x.com", ""), "default is open");
        assert!(signup_allowed("open", true, "a@x.com", ""));
        assert!(!signup_allowed("closed", true, "a@x.com", "a@x.com"));
        assert!(!signup_allowed("junk", true, "a@x.com", "a@x.com"), "a typo closes");
        assert!(signup_allowed("invite", true, "A@x.com", "b@x.com, a@X.com"));
        assert!(!signup_allowed("invite", true, "c@x.com", "b@x.com,"));
    }

    #[test]
    fn cookie_secure_modes() {
        assert!(cookie_secure("auto", "https://m.example", false));
        assert!(cookie_secure("auto", "http://localhost:8001", true), "trusted proxy said https");
        assert!(!cookie_secure("auto", "http://localhost:8001", false));
        assert!(cookie_secure("", "http://localhost:8001", true));
        assert!(cookie_secure("true", "http://localhost:8001", false));
        assert!(!cookie_secure("false", "https://m.example", true));
    }
}
