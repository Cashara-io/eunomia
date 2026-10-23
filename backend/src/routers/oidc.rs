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
//!    Linking signs the account's other browser sessions out, so a session somebody planted by pre-registering
//!    the address does not survive it.
//! 3. otherwise a new account with no password, through the same signup gate and setup (org membership,
//!    personal vault) as password signup. On an empty install the first sign-in is the first user, the admin.

use axum::{
    extract::{Extension, Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::{AppendHeaders, IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use chrono::Utc;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

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
    Router::new().route("/auth/oidc/start", get(start)).route("/auth/oidc/callback", get(callback))
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

fn flow_cookie(token: &str, state: &AppState, https: Option<Extension<ForwardedHttps>>) -> String {
    format!("{FLOW_COOKIE}={token}; HttpOnly; SameSite=Lax; Path=/api/auth/oidc; Max-Age={FLOW_TTL_SECS}{}", cookie_flags(&state.settings.public_url, https))
}

fn clear_flow_cookie(state: &AppState, https: Option<Extension<ForwardedHttps>>) -> String {
    format!("{FLOW_COOKIE}=; HttpOnly; SameSite=Lax; Path=/api/auth/oidc; Max-Age=0{}", cookie_flags(&state.settings.public_url, https))
}

fn read_flow_cookie(headers: &HeaderMap) -> Option<String> {
    let all = headers.get(header::COOKIE)?.to_str().ok()?;
    all.split(';').find_map(|p| p.trim().strip_prefix(&format!("{FLOW_COOKIE}=")).map(str::to_string))
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
fn login_error(state: &AppState, code: &str, next: &str, clear: String) -> Response {
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
    let cfg = configured(&state)?;
    let next = safe_next(q.next.as_deref());
    let disc = match state.oidc.discovery(cfg).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "OIDC discovery failed");
            record_failure(&state, "provider", &e.to_string()).await;
            return Ok(login_error(&state, "provider", &next, clear_flow_cookie(&state, https)));
        }
    };
    let flow = Flow {
        purpose: FLOW_PURPOSE.into(),
        state: oidc::random_token(32),
        nonce: oidc::random_token(32),
        verifier: oidc::random_token(48),
        next,
        exp: Utc::now().timestamp() + FLOW_TTL_SECS,
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

/// Why the callback did not sign anyone in: the `oidc_error` code, an audit outcome and a reason for the log.
enum Fail {
    State(&'static str),
    Denied(String),
    Provider(String),
    Token(String),
    NoEmail,
    EmailUnverified,
    SignupClosed,
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
            Fail::Internal(_) => "unavailable",
        }
    }

    fn outcome(&self) -> ErrorCode {
        match self {
            Fail::State(_) | Fail::Denied(_) | Fail::Token(_) => ErrorCode::AuthUnauthorized,
            Fail::NoEmail | Fail::EmailUnverified | Fail::SignupClosed => ErrorCode::AuthForbidden,
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
    let flow = read_flow_cookie(&headers).and_then(|t| decode_flow(&state.settings.jwt_secret, &t));
    let next = flow.as_ref().map_or_else(|| "/".to_string(), |f| f.next.clone());
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
            let target = if mapped == Mapped::Created && next != "/" {
                format!("/onboarding?{}", serde_urlencoded::to_string([("next", next.as_str())]).unwrap_or_default())
            } else if mapped == Mapped::Created {
                "/onboarding".to_string()
            } else {
                next
            };
            let session = session_cookie_header(&token, &state.settings.public_url, https);
            Ok(no_store((AppendHeaders([(header::SET_COOKIE, session), (header::SET_COOKIE, clear)]), Redirect::to(&target)).into_response()))
        }
        Err(fail) => {
            if let Fail::Internal(e) = &fail {
                tracing::error!(error = %fail.detail(), code = e.code.as_str(), "OIDC sign-in failed");
            } else {
                tracing::info!(reason = %fail.detail(), "OIDC sign-in refused");
            }
            audit::record(&state.control, Event { user: None, actor: &audit::anonymous(), action: "auth.oidc_login", target: "", outcome: fail.outcome().as_str(), detail: &fail.detail() }).await;
            Ok(login_error(&state, fail.code(), &next, clear))
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
        ident.email_verified = verified;
    }
    map_identity(state, &ident).await
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

async fn map_identity(state: &AppState, id: &Identity) -> Result<(User, Mapped), Fail> {
    let db = &state.control;
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
        let email = id.email.as_deref().map(models_user::normalize_email).filter(|e| models_user::validate_email(e).is_ok()).ok_or(Fail::NoEmail)?;
        let raced = |e: &AppError| matches!(e.code, ErrorCode::DbDuplicate | ErrorCode::AuthEmailTaken) && attempt == 0;

        if let Some(user) = models_user::user_by_email(db, &email).await? {
            if !id.email_verified {
                return Err(Fail::EmailUnverified);
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

        let has_users = crate::capsules::first_user(db).await?.is_some();
        let allowed = signup_decision(
            has_users,
            id.email_verified,
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

#[derive(utoipa::OpenApi)]
#[openapi(paths(start, callback))]
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
    fn flow_cookie_roundtrips_and_rejects_other_tokens() {
        let flow = Flow { purpose: FLOW_PURPOSE.into(), state: "s".into(), nonce: "n".into(), verifier: "v".into(), next: "/x".into(), exp: Utc::now().timestamp() + 60 };
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
