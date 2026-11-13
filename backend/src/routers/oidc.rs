//! Sign-in through an OpenID Connect provider (`AUTH_MODE=both|oidc`).
//!
//! `GET /api/auth/oidc/start?next=/path` sends the browser to the provider with a fresh `state`, `nonce` and
//! PKCE challenge. Those three, and where to go afterwards, ride in a short-lived signed cookie
//! (`eunomia_oidc`, HttpOnly, SameSite=Lax, scoped to `/api/auth/oidc`, 10 minutes): nothing is stored
//! server-side, and the cookie is what binds the callback to the browser that started the flow, so a
//! callback somebody else produced (login CSRF) has no matching cookie and is refused.
//!
//! `GET /api/auth/oidc/callback` is where the provider sends the browser back. It checks `state`, exchanges the
//! code (with the PKCE verifier), validates the ID token (`oidc.rs`), maps the identity to an account and starts
//! the same session cookie password login does. Every failure is a redirect back to the login page with a short
//! `oidc_error` code, never a page of JSON, because the user is in the middle of a browser navigation.
//!
//! Mapping an identity `(issuer, subject)` to a user:
//! 1. already linked: that user.
//! 2. an account with the same email exists: link to it only when the provider says the email is verified
//!    (otherwise anyone who can claim someone's address at a lax provider would take over their account).
//!    - The account has no password (made by single sign-on): link it, signing its other browser sessions out.
//!    - The account has a password: ask first ([`ask_before_linking`]). Anyone could have registered the
//!      address with a password of their own, and linking silently would hand the real owner an account
//!      that someone else still holds credentials and connections for. The person either proves the
//!      password (link, nothing removed) or starts a new account ([`start_fresh`]): the existing one is
//!      set aside (suspended, its address replaced by a placeholder, its data left for an operator) and a
//!      new, empty account is made exactly as a first sign-in would make one. Nothing of the old account
//!      is inherited, so nothing its holder set up (tokens, app connections, shared-space links, extension
//!      secrets, connectors) reaches the new one. `POST /api/auth/oidc/link` does either; the provider
//!      sign-in that started it rides in a short-lived signed cookie, bound to that identity and account
//!      and spent by either choice.
//!    - An account is linked to at most one identity per provider: a different identity at the same
//!      provider is refused (an operator relinks it).
//! 3. otherwise a new account with no password, through the same signup gate and setup (org membership,
//!    personal vault) as password signup. On an empty install the first sign-in is the first user, the admin.
//!    When the sign-in started from an organisation invitation link (`POST /api/auth/oidc/start` from the
//!    invitation page, same-site only), the flow cookie carries that invitation's token hash and only that
//!    invitation is used: still open, for this email, verified by the provider (`OIDC_INVITE_ACCEPT_UNVERIFIED_EMAIL`
//!    decides a missing claim). It is then the permission (whatever `SIGNUP` says) and the account joins the
//!    inviting org (`invitations.rs`). An invitation is never found by email.

use axum::{
    extract::{Extension, Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::{AppendHeaders, IntoResponse, Redirect, Response},
    routing::get,
    Form, Json, Router,
};
use chrono::Utc;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

use crate::audit::{self, Event};
use crate::auth;
use crate::rid::RecordIdExt;
use crate::authz::{self, Actor};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::gate::ForwardedHttps;
use crate::models_user::{self, User};
use crate::oidc::{self, Identity, OidcConfig};
use crate::routers::auth::{cookie_flags, session_cookie_header, signup_allowed, user_agent_from};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/oidc/start", get(start).post(start_invited))
        .route("/auth/oidc/callback", get(callback))
        .route("/auth/oidc/link", get(link_info).post(link))
}

const FLOW_COOKIE: &str = "eunomia_oidc";
const FLOW_TTL_SECS: i64 = 600;
const FLOW_PURPOSE: &str = "oidc-flow";

/// What the start leg remembers for the callback leg.
#[derive(Serialize, Deserialize)]
struct Flow {
    purpose: String,
    state: String,
    nonce: String,
    verifier: String,
    next: String,
    exp: i64,
    /// The sha256 of the invitation token this sign-in started from (the invitation page's form post), if
    /// any. Only that invitation may be used by this sign-in; never the token itself, never in a URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    invite: Option<String>,
}

fn encode_flow(secret: &str, flow: &Flow) -> AppResult<String> {
    encode(&Header::new(Algorithm::HS256), flow, &EncodingKey::from_secret(secret.as_bytes())).map_err(|e| AppError::internal(e.to_string()))
}

fn decode_flow(secret: &str, token: &str) -> Option<Flow> {
    let mut v = Validation::new(Algorithm::HS256);
    v.set_required_spec_claims(&["exp"]);
    v.leeway = 0;
    let flow = decode::<Flow>(token, &DecodingKey::from_secret(secret.as_bytes()), &v).ok()?.claims;
    (flow.purpose == FLOW_PURPOSE).then_some(flow)
}

/// The name and path of a sign-in cookie. Served over https (`COOKIE_SECURE`, as the session cookie
/// decides it), it is `__Host-<base>` with `Path=/`: the browser then refuses one set by any other host,
/// such as a sibling subdomain, so nobody can plant a sign-in in someone else's browser. Plain http (a
/// local install) keeps the plain name, scoped to `/api/auth/oidc`.
fn cookie_name(base: &str, secure: bool) -> (String, &'static str) {
    if secure { (format!("__Host-{base}"), "/") } else { (base.to_string(), "/api/auth/oidc") }
}

fn is_secure(state: &AppState, https: Option<Extension<ForwardedHttps>>) -> bool {
    !cookie_flags(&state.settings.public_url, https).is_empty()
}

fn set_cookie(base: &str, value: &str, max_age: i64, state: &AppState, https: Option<Extension<ForwardedHttps>>) -> String {
    let secure = is_secure(state, https);
    let (name, path) = cookie_name(base, secure);
    format!("{name}={value}; HttpOnly; SameSite=Lax; Path={path}; Max-Age={max_age}{}", cookie_flags(&state.settings.public_url, https))
}

/// The cookie's value, under the one name this mode uses (on https only the `__Host-` one counts).
fn read_cookie(base: &str, headers: &HeaderMap, secure: bool) -> Option<String> {
    let (name, _) = cookie_name(base, secure);
    let all = headers.get(header::COOKIE)?.to_str().ok()?;
    all.split(';').find_map(|p| p.trim().strip_prefix(&format!("{name}=")).map(str::to_string))
}

fn flow_cookie(token: &str, state: &AppState, https: Option<Extension<ForwardedHttps>>) -> String {
    set_cookie(FLOW_COOKIE, token, FLOW_TTL_SECS, state, https)
}

fn clear_flow_cookie(state: &AppState, https: Option<Extension<ForwardedHttps>>) -> String {
    set_cookie(FLOW_COOKIE, "", 0, state, https)
}

fn read_flow_cookie(state: &AppState, https: Option<Extension<ForwardedHttps>>, headers: &HeaderMap) -> Option<String> {
    read_cookie(FLOW_COOKIE, headers, is_secure(state, https))
}

/// A same-site path to land on after sign-in: a single leading slash, no backslash or control character
/// (the same rule as the frontend's `nextPath`). Anything else is the dashboard.
pub fn safe_next(raw: Option<&str>) -> String {
    match raw {
        Some(n) if n.starts_with('/') && !n.starts_with("//") && !n.contains('\\') && !n.chars().any(char::is_control) && n.len() <= 2048 => n.to_string(),
        _ => "/".to_string(),
    }
}

fn redirect_uri(state: &AppState) -> String {
    format!("{}/api/auth/oidc/callback", state.settings.public_url)
}

fn no_store(mut r: Response) -> Response {
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

/// Back to the login page with a short code the page turns into a message.
fn login_error(code: &str, next: &str, clear: String) -> Response {
    let mut q = vec![("oidc_error", code)];
    if next != "/" {
        q.push(("next", next));
    }
    // relative: the callback was served on the origin the browser uses, which is where the login page is too
    let target = format!("/login?{}", serde_urlencoded::to_string(&q).unwrap_or_default());
    no_store((AppendHeaders([(header::SET_COOKIE, clear)]), Redirect::to(&target)).into_response())
}

fn configured(state: &AppState) -> AppResult<&OidcConfig> {
    state.settings.auth.oidc.as_ref().ok_or_else(|| AppError::coded(ErrorCode::AuthNotFound, "Single sign-on is not configured on this server."))
}

#[derive(Deserialize)]
struct StartQuery {
    next: Option<String>,
}

#[utoipa::path(
    operation_id = "oidcStart",
    get,
    path = "/api/auth/oidc/start",
    tag = "auth",
    summary = "Begin signing in with the configured OpenID Connect provider",
    description = "Redirects the browser to the provider. `next` is a same-site path to return to afterwards (for example the OAuth consent page).",
    params(("next" = Option<String>, Query, description = "Same-site path to open after sign-in")),
    responses((status = 303, description = "Redirect to the provider"), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(()),
)]
async fn start(State(state): State<AppState>, https: Option<Extension<ForwardedHttps>>, Query(q): Query<StartQuery>) -> AppResult<Response> {
    begin(&state, https, safe_next(q.next.as_deref()), None).await
}

#[derive(Deserialize, utoipa::ToSchema)]
struct StartForm {
    /// Same-site path to open after sign-in (the invitation page).
    #[serde(default)]
    next: Option<String>,
    /// The token from the organisation invitation link.
    #[serde(default)]
    invitation: Option<String>,
}

/// Whether a browser form post came from this site: its `Origin` is one this server serves (the frontend
/// origins in `CORS_ALLOWED_ORIGINS`, or `PUBLIC_URL`). Browsers always send `Origin` on a form post.
fn from_this_site(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()).map(|o| o.trim_end_matches('/')) else { return false };
    let public = url::Url::parse(&state.settings.public_url).ok().map(|u| u.origin().ascii_serialization());
    state.settings.cors_allowed_origins.split(',').map(|o| o.trim().trim_end_matches('/')).any(|o| !o.is_empty() && o == origin)
        || public.as_deref() == Some(origin)
}

#[utoipa::path(
    operation_id = "oidcStartInvited",
    post,
    path = "/api/auth/oidc/start",
    tag = "auth",
    summary = "Begin signing in with the provider from an organisation invitation link",
    description = "The invitation page's form post (same-site `Origin` only). The invitation travels in the body and is kept, hashed, in the signed sign-in cookie: it is never in a URL, so the provider never sees it. Only this invitation can be used by the sign-in that follows, and only for the invited, verified email.",
    request_body(content = StartForm, content_type = "application/x-www-form-urlencoded"),
    responses((status = 303, description = "Redirect to the provider"), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(()),
)]
async fn start_invited(State(state): State<AppState>, https: Option<Extension<ForwardedHttps>>, headers: HeaderMap, Form(f): Form<StartForm>) -> AppResult<Response> {
    // another site could otherwise start a sign-in that uses its own invitation in this browser
    if !from_this_site(&state, &headers) {
        return Err(AppError::coded(ErrorCode::AuthForbidden, "This sign-in has to start from the invitation page."));
    }
    let token = f.invitation.as_deref().map(str::trim).filter(|t| !t.is_empty() && t.len() <= 128);
    begin(&state, https, safe_next(f.next.as_deref()), token.map(crate::invitations::token_hash)).await
}

async fn begin(state: &AppState, https: Option<Extension<ForwardedHttps>>, next: String, invite: Option<String>) -> AppResult<Response> {
    let state = state.clone();
    let cfg = configured(&state)?;
    let disc = match state.oidc.discovery(cfg).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "OIDC discovery failed");
            record_failure(&state, "provider", &e.to_string()).await;
            return Ok(login_error("provider", &next, clear_flow_cookie(&state, https)));
        }
    };
    let flow = Flow {
        purpose: FLOW_PURPOSE.into(),
        state: oidc::random_token(32),
        nonce: oidc::random_token(32),
        verifier: oidc::random_token(48),
        next,
        exp: Utc::now().timestamp() + FLOW_TTL_SECS,
        invite,
    };
    let mut scopes: Vec<&str> = cfg.scopes.split_whitespace().collect();
    if !scopes.contains(&"openid") {
        scopes.insert(0, "openid");
    }
    let mut url = url::Url::parse(&disc.authorization_endpoint).map_err(|e| AppError::internal(e.to_string()))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &cfg.client_id)
        .append_pair("redirect_uri", &redirect_uri(&state))
        .append_pair("scope", &scopes.join(" "))
        .append_pair("state", &flow.state)
        .append_pair("nonce", &flow.nonce)
        .append_pair("code_challenge", &oidc::pkce_challenge(&flow.verifier))
        .append_pair("code_challenge_method", "S256");
    let cookie = flow_cookie(&encode_flow(&state.settings.jwt_secret, &flow)?, &state, https);
    Ok(no_store((AppendHeaders([(header::SET_COOKIE, cookie)]), Redirect::to(url.as_str())).into_response()))
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    /// RFC 9207: the issuer that produced this response (a defence against a malicious provider mixing up flows).
    iss: Option<String>,
}

/// The existing account a verified sign-in would link to, while the person is asked first.
struct AskToLink {
    user: surrealdb::types::RecordId,
    email: String,
    issuer: String,
    subject: String,
    /// The invitation token hash this sign-in started from, if any: starting fresh accepts it.
    invite: Option<String>,
}

/// Why the callback did not sign anyone in: the `oidc_error` code, an audit outcome and a reason for the log.
enum Fail {
    State(&'static str),
    Denied(String),
    Provider(String),
    Token(String),
    NoEmail,
    EmailUnverified,
    SignupClosed,
    /// The invitation the sign-in started from cannot be used: (the `oidc_error` code, why).
    Invitation(&'static str, String),
    /// Not a failure: the verified email belongs to an account with a password, so the person is asked
    /// before linking ([`ask_before_linking`]).
    AskToLink(Box<AskToLink>),
    /// The account is already linked to a different identity at this provider.
    IdentityTaken,
    Internal(AppError),
}

impl Fail {
    fn code(&self) -> &'static str {
        match self {
            Fail::State(_) => "state",
            Fail::Denied(_) => "denied",
            Fail::Provider(_) => "provider",
            Fail::Token(_) => "token",
            Fail::NoEmail => "no_email",
            Fail::EmailUnverified => "email_unverified",
            Fail::SignupClosed => "signup_closed",
            Fail::Invitation(code, _) => code,
            Fail::AskToLink(_) => "link",
            Fail::IdentityTaken => "identity_taken",
            Fail::Internal(_) => "unavailable",
        }
    }

    fn outcome(&self) -> ErrorCode {
        match self {
            Fail::State(_) | Fail::Denied(_) | Fail::Token(_) => ErrorCode::AuthUnauthorized,
            Fail::NoEmail | Fail::EmailUnverified | Fail::SignupClosed | Fail::Invitation(..) | Fail::AskToLink(_) | Fail::IdentityTaken => ErrorCode::AuthForbidden,
            Fail::Provider(_) | Fail::Internal(_) => ErrorCode::Internal,
        }
    }

    fn detail(&self) -> String {
        match self {
            Fail::State(m) => (*m).to_string(),
            Fail::Denied(m) | Fail::Provider(m) | Fail::Token(m) => m.clone(),
            Fail::NoEmail => "the provider returned no email".into(),
            Fail::EmailUnverified => "an account has this email but the provider did not verify it".into(),
            Fail::SignupClosed => "signup is not open to this email".into(),
            Fail::Invitation(_, why) => why.clone(),
            Fail::AskToLink(_) => "an account with a password has this email: asking before linking".into(),
            Fail::IdentityTaken => "the account with this email is linked to a different identity at this provider".into(),
            Fail::Internal(e) => e.source.clone().unwrap_or_else(|| e.message.clone()),
        }
    }
}

impl From<AppError> for Fail {
    fn from(e: AppError) -> Self {
        Fail::Internal(e)
    }
}

async fn record_failure(state: &AppState, code: &str, detail: &str) {
    let outcome = if code == "provider" || code == "unavailable" { ErrorCode::Internal } else { ErrorCode::AuthUnauthorized };
    audit::record(&state.control, Event { user: None, actor: &audit::anonymous(), action: "auth.oidc_login", target: "", outcome: outcome.as_str(), detail }).await;
}

/// How the identity became a user.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mapped {
    Existing,
    Linked,
    Created,
}

#[utoipa::path(
    operation_id = "oidcCallback",
    get,
    path = "/api/auth/oidc/callback",
    tag = "auth",
    summary = "Finish signing in with the OpenID Connect provider",
    description = "The provider redirects the browser here with `code` and `state`. Starts a session and redirects to the page the sign-in began from, or to the login page with an `oidc_error` code.",
    params(
        ("code" = Option<String>, Query, description = "Authorization code"),
        ("state" = Option<String>, Query, description = "Must match the flow cookie"),
        ("error" = Option<String>, Query, description = "Set when the provider refused"),
        ("iss" = Option<String>, Query, description = "Issuer (RFC 9207)"),
    ),
    responses((status = 303, description = "Redirect to the app or back to the login page"), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(()),
)]
async fn callback(State(state): State<AppState>, https: Option<Extension<ForwardedHttps>>, headers: HeaderMap, Query(q): Query<CallbackQuery>) -> AppResult<Response> {
    let cfg = configured(&state)?;
    let clear = clear_flow_cookie(&state, https);
    let flow = read_flow_cookie(&state, https, &headers).and_then(|t| decode_flow(&state.settings.jwt_secret, &t));
    let next = flow.as_ref().map_or_else(|| "/".to_string(), |f| f.next.clone());
    let from_invitation = flow.as_ref().is_some_and(|f| f.invite.is_some());
    match finish(&state, cfg, flow, &q, &headers).await {
        Ok((user, mapped)) => {
            let token = auth::start_session(&state.control, &state.settings.jwt_secret, &user, user_agent_from(&headers).as_deref()).await?;
            let actor = Actor { kind: "user", id: user.id.to_string() };
            let rec = |action: &'static str, target: &'static str, detail: &'static str| {
                audit::record(&state.control, Event { user: Some(&user.id), actor: &actor, action, target, outcome: "ok", detail })
            };
            // the signup row is the one password signup leaves, so the ledger reads the same either way
            match mapped {
                Mapped::Created => {
                    rec("auth.register", "", "oidc").await;
                    rec("auth.oidc_login", "", "new account").await;
                }
                Mapped::Linked => {
                    rec("auth.oidc_link", "", "linked to the existing account with this email").await;
                    rec("auth.oidc_login", "", "").await;
                }
                Mapped::Existing => rec("auth.oidc_login", "", "").await,
            }
            // a new account made through the invitation has joined: onboarding, not back to the used link
            let target = if mapped == Mapped::Created && from_invitation {
                "/onboarding".to_string()
            } else if mapped == Mapped::Created && next != "/" {
                format!("/onboarding?{}", serde_urlencoded::to_string([("next", next.as_str())]).unwrap_or_default())
            } else if mapped == Mapped::Created {
                "/onboarding".to_string()
            } else {
                next
            };
            let session = session_cookie_header(&token, &state.settings.public_url, https);
            Ok(no_store((AppendHeaders([(header::SET_COOKIE, session), (header::SET_COOKIE, clear)]), Redirect::to(&target)).into_response()))
        }
        Err(Fail::AskToLink(ask)) => {
            let AskToLink { user, email, issuer, subject, invite } = *ask;
            let pending = PendingLink { purpose: LINK_PURPOSE.into(), user: user.to_string(), email, issuer, subject, next, exp: Utc::now().timestamp() + FLOW_TTL_SECS, invite };
            let token = encode(&Header::new(Algorithm::HS256), &pending, &EncodingKey::from_secret(state.settings.jwt_secret.as_bytes())).map_err(|e| AppError::internal(e.to_string()))?;
            let cookie = set_cookie(LINK_COOKIE, &token, FLOW_TTL_SECS, &state, https);
            Ok(no_store((AppendHeaders([(header::SET_COOKIE, cookie), (header::SET_COOKIE, clear)]), Redirect::to("/login/link")).into_response()))
        }
        Err(fail) => {
            if let Fail::Internal(e) = &fail {
                tracing::error!(error = %fail.detail(), code = e.code.as_str(), "OIDC sign-in failed");
            } else {
                tracing::info!(reason = %fail.detail(), "OIDC sign-in refused");
            }
            audit::record(&state.control, Event { user: None, actor: &audit::anonymous(), action: "auth.oidc_login", target: "", outcome: fail.outcome().as_str(), detail: &fail.detail() }).await;
            Ok(login_error(fail.code(), &next, clear))
        }
    }
}

async fn finish(state: &AppState, cfg: &OidcConfig, flow: Option<Flow>, q: &CallbackQuery, _headers: &HeaderMap) -> Result<(User, Mapped), Fail> {
    let flow = flow.ok_or(Fail::State("no valid flow cookie (expired, missing, or started in another browser)"))?;
    if let Some(e) = &q.error {
        return Err(Fail::Denied(format!("the provider returned error {:?}", e.chars().take(64).collect::<String>())));
    }
    let given = q.state.as_deref().unwrap_or("");
    if given.is_empty() || !oidc::constant_time_eq(given, &flow.state) {
        return Err(Fail::State("state does not match"));
    }
    if q.iss.as_deref().is_some_and(|i| i != cfg.issuer) {
        return Err(Fail::Provider("the response names a different issuer".into()));
    }
    let code = q.code.as_deref().filter(|c| !c.is_empty() && c.len() <= 4096).ok_or_else(|| Fail::Provider("no authorization code".into()))?;
    let disc = state.oidc.discovery(cfg).await.map_err(|e| Fail::Provider(e.to_string()))?;
    let (id_token, access_token) = state
        .oidc
        .exchange_code(cfg, &disc, code, &redirect_uri(state), &flow.verifier)
        .await
        .map_err(|e| Fail::Provider(e.to_string()))?;
    let mut ident = state.oidc.verify_id_token(cfg, &disc, &id_token, &flow.nonce).await.map_err(|e| match e {
        oidc::OidcError::Token(m) => Fail::Token(m),
        oidc::OidcError::Provider(m) => Fail::Provider(m),
    })?;
    // some providers keep the email out of the ID token; the userinfo endpoint (same subject, same access token) has it
    if ident.email.is_none()
        && let Some(at) = access_token
        && let Some((email, verified)) = state.oidc.userinfo(&disc, &at, &ident.subject).await
    {
        ident.email = email;
        // an explicit claim in the ID token stands (a missing one in userinfo must not turn a false into "absent")
        if ident.email_verified_absent {
            ident.email_verified = verified.unwrap_or(false);
            ident.email_verified_absent = verified.is_none();
        }
    }
    map_identity(state, cfg, &ident, flow.invite.as_deref()).await
}

/// `EUNOMIA_ADMIN_EMAILS` lists at least one address.
fn operators_configured() -> bool {
    std::env::var("EUNOMIA_ADMIN_EMAILS").unwrap_or_default().split(',').any(|e| !e.trim().is_empty())
}

/// May a sign-in with no account yet create one?
/// - Empty install: the first sign-in is the first user and the admin. When `EUNOMIA_ADMIN_EMAILS` names operators, only
///   one of them (provider-verified) may be that first user, so a stranger who reaches a shared provider first cannot claim the instance.
/// - A provider-verified operator address may always sign in (this is how an admin exists in provider-only mode).
///   An unverified one is refused, as password signup refuses an operator address once the install has users.
/// - Otherwise the `SIGNUP` mode decides, and an unverified email can never satisfy `SIGNUP=invite`'s allow list.
fn signup_decision(has_users: bool, verified: bool, listed: bool, operators_configured: bool, mode: &str, allowlist: &str, email: &str) -> bool {
    if !has_users {
        return !operators_configured || (listed && verified);
    }
    if listed {
        return verified;
    }
    signup_allowed(mode, true, if verified { email } else { "" }, allowlist)
}

/// Maps a verified identity to an account. `invite` is the hash of the invitation token this sign-in
/// started from (the signed flow cookie): only that invitation can place a new account, never one found by
/// email.
async fn map_identity(state: &AppState, cfg: &OidcConfig, id: &Identity, invite: Option<&str>) -> Result<(User, Mapped), Fail> {
    let db = &state.control;
    // linking, take-over, operator addresses and the allowlist: only the provider's explicit claim counts
    let verified = id.email_verified;
    // two attempts: a concurrent first sign-in of the same person loses the unique index, then finds the winner
    for attempt in 0..2 {
        if let Some(user) = models_user::user_by_identity(db, &id.issuer, &id.subject).await? {
            let _ = crate::store::control::IDENTITY_TOUCH
                .on(db)
                .bind(("issuer", id.issuer.clone()))
                .bind(("subject", id.subject.clone()))
                .bind(("email", id.email.clone()))
                .await;
            return Ok((user, Mapped::Existing));
        }
        let email = id.email.as_deref().and_then(|e| models_user::normalize_new_email(e).ok()).ok_or(Fail::NoEmail)?;
        let raced = |e: &AppError| matches!(e.code, ErrorCode::DbDuplicate | ErrorCode::AuthEmailTaken) && attempt == 0;

        // an existing account: linked (verified email only) and signed in; an invitation is accepted, if at
        // all, on the invitation page afterwards, as a separate confirmed step
        if let Some(user) = models_user::user_by_email(db, &email).await? {
            if !verified {
                return Err(Fail::EmailUnverified);
            }
            // one account, one identity per provider: a second one is not added, whatever the account holds
            if linked_at_issuer(db, &user.id, &id.issuer).await? {
                return Err(Fail::IdentityTaken);
            }
            if ask_before_linking(db, &user.id).await? {
                return Err(Fail::AskToLink(Box::new(AskToLink { user: user.id, email, issuer: id.issuer.clone(), subject: id.subject.clone(), invite: invite.map(str::to_string) })));
            }
            match models_user::link_identity(db, &user.id, &id.issuer, &id.subject, &email).await {
                Ok(()) => {
                    // whoever pre-registered this address may hold a session on the account: end every one
                    crate::store::control::AUTH_SESSION_REVOKE_OWNER.on(db).bind(("owner", user.id.clone())).await.map_err(AppError::from)?;
                    state.credentials.clear();
                    return Ok((user, Mapped::Linked));
                }
                Err(e) if raced(&e) => continue,
                Err(e) => return Err(e.into()),
            }
        }

        // a new account from an invitation link: that invitation, and only for its own, verified email
        if let Some(hash) = invite {
            let p = invited(state, hash, &email, id.verified_for_invitation(cfg)).await?;
            return match models_user::register_oidc_user_invited(state, &email, &id.issuer, &id.subject, &p).await {
                Ok(user) => {
                    crate::invitations::record_accepted(db, &user.id, &p, &p.role, "oidc").await;
                    Ok((user, Mapped::Created))
                }
                Err(e) if raced(&e) => continue,
                Err(e) if e.code == ErrorCode::InvitationNotFound => Err(Fail::Invitation("invitation", "the invitation was used or withdrawn meanwhile".into())),
                Err(e) => Err(e.into()),
            };
        }

        // no link: the host's ordinary signup rules, exactly as without invitations
        let has_users = crate::capsules::first_user(db).await?.is_some();
        let allowed = signup_decision(
            has_users,
            verified,
            authz::operator_email_listed(&email),
            operators_configured(),
            &std::env::var("SIGNUP").unwrap_or_default(),
            &std::env::var("SIGNUP_ALLOWLIST").unwrap_or_default(),
            &email,
        );
        if !allowed {
            return Err(Fail::SignupClosed);
        }
        return match models_user::register_oidc_user(state, &email, &id.issuer, &id.subject).await {
            Ok(user) => Ok((user, Mapped::Created)),
            Err(e) if raced(&e) => continue,
            Err(e) => Err(e.into()),
        };
    }
    Err(Fail::Internal(AppError::internal("could not settle a concurrent sign-in")))
}

/// The invitation a sign-in started from, if this identity may use it: still open (an operator's address
/// never is), not expired, for this email, and the email verified.
async fn invited(state: &AppState, hash: &str, email: &str, verified: bool) -> Result<crate::invitations::Pending, Fail> {
    let db = &state.control;
    let p = crate::invitations::by_hash(db, hash).await?.ok_or_else(|| Fail::Invitation("invitation", "the invitation is unknown".into()))?;
    let p = match crate::invitations::check_usable(db, p).await {
        Ok(p) => p,
        Err(e) if e.code == ErrorCode::InvitationExpired => return Err(Fail::Invitation("invitation_expired", "the invitation has expired".into())),
        Err(e) if e.code == ErrorCode::InvitationNotFound => return Err(Fail::Invitation("invitation", "the invitation is used, withdrawn or not usable".into())),
        Err(e) => return Err(e.into()),
    };
    if crate::invitations::require_email(db, &p, email).await.is_err() {
        return Err(Fail::Invitation("invitation_email", "the signed-in email is not the invited one".into()));
    }
    if !verified {
        crate::invitations::record_not_accepted(db, &p).await;
        return Err(Fail::Invitation("invitation_unverified", "the provider has not verified the email".into()));
    }
    Ok(p)
}

// ---------------------------------------------------------------- linking to an account that has a password

const LINK_COOKIE: &str = "eunomia_oidc_link";
const LINK_PURPOSE: &str = "oidc-link";

/// A verified provider sign-in waiting for the person to prove the account's password or take it over.
#[derive(Serialize, Deserialize)]
struct PendingLink {
    purpose: String,
    /// The account with this email.
    user: String,
    email: String,
    issuer: String,
    subject: String,
    next: String,
    exp: i64,
    /// The invitation token hash the sign-in started from, if any (only that invitation, as at the callback).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    invite: Option<String>,
}

fn read_pending_link(state: &AppState, https: Option<Extension<ForwardedHttps>>, headers: &HeaderMap) -> Option<PendingLink> {
    let token = read_cookie(LINK_COOKIE, headers, is_secure(state, https))?;
    let token = token.as_str();
    let mut v = Validation::new(Algorithm::HS256);
    v.set_required_spec_claims(&["exp"]);
    v.leeway = 0;
    let p = decode::<PendingLink>(token, &DecodingKey::from_secret(state.settings.jwt_secret.as_bytes()), &v).ok()?.claims;
    (p.purpose == LINK_PURPOSE).then_some(p)
}

fn clear_link_cookie(state: &AppState, https: Option<Extension<ForwardedHttps>>) -> String {
    set_cookie(LINK_COOKIE, "", 0, state, https)
}

/// Whether linking a verified provider identity to this existing account must ask first: it has a
/// password, so whoever set that password may not be the person signing in. An account with no password
/// (made by single sign-on) links as before. Accounts already linked to the identity never get here.
async fn ask_before_linking(db: &crate::pool::ControlDb, user: &surrealdb::types::RecordId) -> AppResult<bool> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        #[serde(default)]
        #[surreal(default)]
        password_hash: Option<String>,
    }
    let mut res = crate::store::control::AUTH_USER_BY_ID.on(db).bind(("id", user.clone())).await?;
    Ok(res.take::<Vec<Row>>(0)?.into_iter().next().is_some_and(|r| r.password_hash.is_some()))
}

/// Whether the account already has an identity at `issuer` (the one signing in is not it: that would have
/// been found by `(issuer, subject)` first).
async fn linked_at_issuer(db: &crate::pool::ControlDb, user: &surrealdb::types::RecordId, issuer: &str) -> AppResult<bool> {
    #[derive(Deserialize, SurrealValue)]
    struct Link {
        issuer: String,
    }
    let mut res = crate::store::control::IDENTITY_LIST_FOR_USER.on(db).bind(("user", user.clone())).await?;
    Ok(res.take::<Vec<Link>>(0)?.iter().any(|l| l.issuer == issuer))
}

/// The address and state of the account a pending link names, read again before acting on it.
#[derive(Deserialize, SurrealValue)]
struct LinkTarget {
    email: String,
    #[serde(default)]
    #[surreal(default)]
    email_lc: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    password_hash: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    suspended_at: Option<surrealdb::types::Datetime>,
}

async fn link_target(db: &crate::pool::ControlDb, user: &surrealdb::types::RecordId) -> AppResult<Option<LinkTarget>> {
    let mut res = crate::store::control::AUTH_USER_BY_ID.on(db).bind(("id", user.clone())).await?;
    Ok(res.take::<Vec<LinkTarget>>(0)?.into_iter().next())
}

/// Starts a new account for the person whose provider explicitly verified the address, instead of
/// inheriting the account that already has it. The existing account is set aside in one transaction:
/// suspended (so no session, token, app connection, shared-space link, share code, extension, source
/// webhook or job acts for it), its sign-in address replaced by a `set-aside:` placeholder that matches no
/// address, and its sessions ended. Its data stays where it is, for an operator to review, restore or
/// erase. Then a new account is made exactly as a first sign-in would make one; if that fails, the set-aside
/// is undone. When enforced second factors exist, an enrolled authenticator changes this (see the PR notes).
async fn start_fresh(
    state: &AppState,
    old: &surrealdb::types::RecordId,
    before: &LinkTarget,
    email: &str,
    issuer: &str,
    subject: &str,
    invitation: Option<&crate::invitations::Pending>,
) -> AppResult<User> {
    crate::store::control::AUTH_SET_ASIDE.on(&state.control).bind(("user", old.clone())).await?.check()?;
    state.credentials.forget_user(old);
    // through the invitation the sign-in carried, the new account joins the inviting organisation
    let made = match invitation {
        Some(p) => models_user::register_oidc_user_invited(state, email, issuer, subject, p).await,
        None => models_user::register_oidc_user(state, email, issuer, subject).await,
    };
    let user = match made {
        Ok(u) => u,
        Err(e) => {
            let _ = crate::store::control::AUTH_SET_ASIDE_UNDO
                .on(&state.control)
                .bind(("user", old.clone()))
                .bind(("email", before.email.clone()))
                .bind(("email_lc", before.email_lc.clone()))
                .bind(("suspended_at", before.suspended_at))
                .await;
            return Err(e);
        }
    };
    if let Some(p) = invitation {
        crate::invitations::record_accepted(&state.control, &user.id, p, &p.role, "oidc").await;
    }
    let actor = Actor { kind: "user", id: user.id.to_string() };
    let detail = format!("email={email} set aside when a provider-verified sign-in for the address started a new account ({})", user.id.to_string());
    audit::record(&state.control, Event { user: Some(old), actor: &actor, action: "auth.oidc_set_aside", target: &old.to_string(), outcome: "ok", detail: &detail }).await;
    Ok(user)
}

#[derive(Serialize, utoipa::ToSchema)]
struct LinkInfo {
    /// The address both the provider and the existing account have.
    email: String,
    /// "Continue with {provider}".
    provider: String,
}

#[utoipa::path(
    operation_id = "oidcLinkInfo",
    get,
    path = "/api/auth/oidc/link",
    tag = "auth",
    summary = "The account a provider sign-in is waiting to link to (it has a password)",
    responses((status = 200, body = LinkInfo), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(()),
)]
async fn link_info(State(state): State<AppState>, https: Option<Extension<ForwardedHttps>>, headers: HeaderMap) -> AppResult<Json<LinkInfo>> {
    let cfg = configured(&state)?;
    let p = read_pending_link(&state, https, &headers).ok_or_else(link_expired)?;
    Ok(Json(LinkInfo { email: p.email, provider: cfg.provider_name.clone() }))
}

fn link_spent() -> AppError {
    AppError::coded(ErrorCode::AuthUnauthorized, "This choice has already been made, or the account changed meanwhile. Sign in with your provider again.")
}

fn link_expired() -> AppError {
    AppError::coded(ErrorCode::AuthUnauthorized, "This sign-in has expired. Sign in with your provider again.")
}

#[derive(Deserialize, utoipa::ToSchema)]
struct LinkChoice {
    /// The existing account's password: proves it is yours, and links without removing anything.
    #[serde(default)]
    password: Option<String>,
    /// `true`: you don't know the password. A new, empty account is made for you; the existing one is set
    /// aside (suspended, nothing of it inherited) for a server operator to review, restore or erase.
    #[serde(default)]
    start_fresh: bool,
}

#[derive(Serialize, utoipa::ToSchema)]
struct LinkDone {
    /// A new account was made and the existing one set aside.
    started_fresh: bool,
    /// Where to go next.
    next: String,
}

#[utoipa::path(
    operation_id = "oidcLink",
    post,
    path = "/api/auth/oidc/link",
    tag = "auth",
    summary = "Link the provider sign-in to the existing account by its password, or start a new account",
    description = "Needs the cookie the provider sign-in left, which names one identity and one account and is spent by either choice. With `password`, the password is checked (throttled like login) and the account is linked; nothing is removed. With `start_fresh: true`, the existing account is set aside (suspended, its address replaced, its data kept for an operator) and a new, empty account is made under this server's sign-up rules. A suspended account can be neither linked nor replaced here. Starts a session.",
    request_body = LinkChoice,
    responses((status = 200, body = LinkDone), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(()),
)]
async fn link(
    State(state): State<AppState>,
    https: Option<Extension<ForwardedHttps>>,
    ip: Option<Extension<crate::gate::ClientIp>>,
    headers: HeaderMap,
    Json(body): Json<LinkChoice>,
) -> AppResult<Response> {
    configured(&state)?;
    let p = read_pending_link(&state, https, &headers).ok_or_else(link_expired)?;
    let user_id: surrealdb::types::RecordId = crate::rid::parse(&p.user).map_err(|_| link_expired())?;
    // bound to the identity and the account it was issued for, and spent by either choice: the identity
    // must still be unlinked, and the account must still hold this address and a password
    let identity_free = models_user::user_by_identity(&state.control, &p.issuer, &p.subject).await.ok().flatten().is_none()
        && !crate::store::control::IDENTITY_FIND.on(&state.control).bind(("issuer", p.issuer.clone())).bind(("subject", p.subject.clone())).await?.take::<Vec<serde_json::Value>>(0).map(|v| !v.is_empty()).unwrap_or(true);
    let target = link_target(&state.control, &user_id).await?.ok_or_else(link_spent)?;
    // a suspended account is an operator's to decide: it can be neither linked nor replaced here
    if target.suspended_at.is_some() {
        return Err(models_user::suspended_error());
    }
    if !identity_free || target.email_lc.as_deref() != Some(models_user::normalize_email(&p.email).as_str()) || target.password_hash.is_none() {
        return Err(link_spent());
    }
    let (user, started_fresh) = if body.start_fresh {
        // a sign-in that came from an invitation link starts fresh as that invitation's acceptance (it is the
        // permission, and the account joins the inviting organisation); checked before anything is set aside
        if let Some(hash) = p.invite.as_deref() {
            let inv = invited(&state, hash, &models_user::normalize_email(&p.email), true).await.map_err(|f| AppError::coded(ErrorCode::InvitationNotFound, format!("The invitation can't be used: {}.", f.detail())))?;
            let user = start_fresh(&state, &user_id, &target, &p.email, &p.issuer, &p.subject, Some(&inv)).await?;
            (user, true)
        } else {
        // the same rules a first sign-in with this verified address meets on this server
        let allowed = signup_decision(
            true,
            true,
            authz::operator_email_listed(&p.email),
            operators_configured(),
            &std::env::var("SIGNUP").unwrap_or_default(),
            &std::env::var("SIGNUP_ALLOWLIST").unwrap_or_default(),
            &p.email,
        );
        if !allowed {
            return Err(AppError::coded(ErrorCode::AuthForbidden, "This server isn't taking new accounts for this address. Ask a server operator for help."));
        }
        (start_fresh(&state, &user_id, &target, &p.email, &p.issuer, &p.subject, None).await?, true)
        }
    } else {
        let password = body.password.as_deref().filter(|p| !p.is_empty()).ok_or_else(|| AppError::bad_request("Enter the account's password, or choose to start a new account."))?;
        // the same throttle as login, on the same keys: guesses here and there add up
        let account_key = format!("login-fail:{}", models_user::normalize_email(&p.email));
        let fail_key = format!("{account_key}:{}", ip.as_ref().map_or("", |i| i.0 .0.as_str()));
        let throttled = state.fail_throttle.check(&fail_key, crate::ratelimit::LOGIN_FAILS).and_then(|()| state.fail_throttle.check(&account_key, crate::ratelimit::LOGIN_ACCOUNT_FAILS));
        if let Err(wait) = throttled {
            return Ok(crate::gate::rate_limited(wait));
        }
        let proved = models_user::authenticate(&state.control, &p.email, password).await?.is_some_and(|u| u.id == user_id);
        if !proved {
            state.fail_throttle.fail(&fail_key);
            state.fail_throttle.fail(&account_key);
            let actor = audit::anonymous();
            audit::record(&state.control, Event { user: Some(&user_id), actor: &actor, action: "auth.oidc_link", target: "", outcome: ErrorCode::AuthUnauthorized.as_str(), detail: "wrong password" }).await;
            return Err(AppError::unauthorized("That isn't the password for this account."));
        }
        state.fail_throttle.clear(&fail_key);
        state.fail_throttle.clear(&account_key);
        models_user::link_identity(&state.control, &user_id, &p.issuer, &p.subject, &p.email).await.map_err(|e| if e.code == ErrorCode::DbDuplicate { link_spent() } else { e })?;
        (models_user::load_user(&state.control, user_id.clone(), target.email.clone()).await?, false)
    };
    let token = auth::start_session(&state.control, &state.settings.jwt_secret, &user, user_agent_from(&headers).as_deref()).await?;
    let actor = Actor { kind: "user", id: user.id.to_string() };
    if started_fresh {
        audit::record(&state.control, Event { user: Some(&user.id), actor: &actor, action: "auth.register", target: "", outcome: "ok", detail: "oidc, a new account instead of the one set aside" }).await;
    } else {
        audit::record(&state.control, Event { user: Some(&user.id), actor: &actor, action: "auth.oidc_link", target: "", outcome: "ok", detail: "linked after the account's password was given" }).await;
    }
    let session = session_cookie_header(&token, &state.settings.public_url, https);
    let next = if started_fresh { "/onboarding".to_string() } else { p.next };
    let body = LinkDone { started_fresh, next };
    Ok(no_store((AppendHeaders([(header::SET_COOKIE, session), (header::SET_COOKIE, clear_link_cookie(&state, https))]), Json(body)).into_response()))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(start, start_invited, callback, link_info, link))]
pub struct Doc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_must_be_a_same_site_path() {
        assert_eq!(safe_next(Some("/consent?client_id=x&state=a")), "/consent?client_id=x&state=a");
        for bad in ["//evil.example", "https://evil.example", "/\\evil.example", "evil", "/a\nb", "/a\tb", ""] {
            assert_eq!(safe_next(Some(bad)), "/", "{bad:?}");
        }
        assert_eq!(safe_next(None), "/");
    }

    #[test]
    fn who_may_create_an_account() {
        // empty install: anyone is the first user, unless operators are named
        assert!(signup_decision(false, false, false, false, "closed", "", "a@x.com"));
        assert!(!signup_decision(false, true, false, true, "open", "", "a@x.com"), "a stranger cannot be first when operators are named");
        assert!(signup_decision(false, true, true, true, "closed", "", "boss@x.com"));
        assert!(!signup_decision(false, false, true, true, "open", "", "boss@x.com"), "the operator's email must be verified");
        // an install with users: SIGNUP decides, operators sign in when verified
        assert!(signup_decision(true, false, false, false, "open", "", "a@x.com"));
        assert!(!signup_decision(true, true, false, false, "closed", "a@x.com", "a@x.com"));
        assert!(signup_decision(true, true, false, false, "invite", "a@x.com", "A@x.com"));
        assert!(!signup_decision(true, false, false, false, "invite", "a@x.com", "a@x.com"), "unverified email cannot use the allow list");
        assert!(signup_decision(true, true, true, true, "closed", "", "boss@x.com"));
        assert!(!signup_decision(true, false, true, true, "open", "", "boss@x.com"));
    }

    #[test]
    fn sign_in_cookies_are_host_locked_on_https() {
        assert_eq!(cookie_name(FLOW_COOKIE, true), ("__Host-eunomia_oidc".to_string(), "/"));
        assert_eq!(cookie_name(LINK_COOKIE, true), ("__Host-eunomia_oidc_link".to_string(), "/"));
        assert_eq!(cookie_name(FLOW_COOKIE, false), ("eunomia_oidc".to_string(), "/api/auth/oidc"));
        // on https a plain-named cookie (which another host could have set) is not read
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("eunomia_oidc=planted"));
        assert_eq!(read_cookie(FLOW_COOKIE, &h, true), None);
        assert_eq!(read_cookie(FLOW_COOKIE, &h, false).as_deref(), Some("planted"));
        h.insert(header::COOKIE, HeaderValue::from_static("eunomia_oidc=planted; __Host-eunomia_oidc=real"));
        assert_eq!(read_cookie(FLOW_COOKIE, &h, true).as_deref(), Some("real"));
    }

    #[test]
    fn flow_cookie_roundtrips_and_rejects_other_tokens() {
        let flow = Flow { purpose: FLOW_PURPOSE.into(), state: "s".into(), nonce: "n".into(), verifier: "v".into(), next: "/x".into(), exp: Utc::now().timestamp() + 60, invite: None };
        let t = encode_flow("secret", &flow).unwrap();
        assert_eq!(decode_flow("secret", &t).unwrap().next, "/x");
        assert!(decode_flow("other", &t).is_none());
        let expired = Flow { exp: Utc::now().timestamp() - 5, ..flow };
        assert!(decode_flow("secret", &encode_flow("secret", &expired).unwrap()).is_none());
        // a session cookie is not a flow
        let user = User { id: crate::rid::parse("user:abc").unwrap(), email: "a@b.co".into(), org: crate::pool::OrgId::new() };
        let session = auth::create_session_jwt(&user, "sid", "secret").unwrap();
        assert!(decode_flow("secret", &session).is_none());
    }
}
