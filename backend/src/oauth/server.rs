//! The OAuth endpoints: authorize, token, register, revoke, plus the shared
//! request validation the frontend consent API reuses (`routers/oauth.rs`).

use surrealdb::types::SurrealValue;
use axum::{
    Form, Json,
    extract::{Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use surrealdb::types::{Datetime, RecordId};
use crate::rid::RecordIdExt;

use super::{ACCESS_TTL_SECS, cimd, frontend_url, issue_tokens, mcp_url};
use crate::pool::ControlDb;
use crate::error::AppError;
use crate::models_user::{User, generate_token, hash_token};
use crate::scopes;
use crate::state::AppState;
use crate::store::control as q;

// -- errors (RFC 6749 section 5.2 shape, not problem+json) -----------------

pub struct OAuthError {
    status: StatusCode,
    error: &'static str,
    description: String,
}

impl OAuthError {
    fn new(status: StatusCode, error: &'static str, description: impl Into<String>) -> Self {
        OAuthError { status, error, description: description.into() }
    }
    fn bad(error: &'static str, description: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, error, description)
    }
    fn grant(description: impl Into<String>) -> Self {
        Self::bad("invalid_grant", description)
    }
    fn server(e: impl std::fmt::Display) -> Self {
        tracing::error!(error = %e, "oauth storage error");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "server_error", "internal error")
    }
}

impl IntoResponse for OAuthError {
    fn into_response(self) -> Response {
        let mut res = (self.status, Json(json!({ "error": self.error, "error_description": self.description }))).into_response();
        no_store(&mut res);
        res
    }
}

fn no_store(res: &mut Response) {
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res.headers_mut().insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
}

// -- clients ---------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, SurrealValue)]
pub struct Client {
    pub client_id: String,
    pub name: String,
    pub logo_uri: Option<String>,
    pub client_uri: Option<String>,
    pub redirect_uris: Vec<String>,
    pub kind: String,
    expires_at: Option<Datetime>,
}

async fn find_client(db: &ControlDb, client_id: &str) -> Result<Option<Client>, String> {
    let mut res = q::OAUTH_CLIENT_GET.on(db).bind(("client_id", client_id.to_string())).await.map_err(|e| e.to_string())?;
    let rows: Vec<Client> = res.take(0).map_err(|e| e.to_string())?;
    Ok(rows.into_iter().next())
}

async fn save_client(db: &ControlDb, client_id: &str, meta: &cimd::ClientMeta, kind: &str, ttl: Option<std::time::Duration>) -> Result<(), String> {
    let expires_at = ttl.map(|t| Datetime::from(chrono::Utc::now() + t));
    q::OAUTH_CLIENT_UPSERT
        .on(db)
        .bind(("id", RecordId::from_table_key("oauth_client", hash_token(client_id))))
        .bind(("client_id", client_id.to_string()))
        .bind(("name", meta.name.clone()))
        .bind(("logo_uri", meta.logo_uri.clone()))
        .bind(("client_uri", meta.client_uri.clone()))
        .bind(("redirect_uris", meta.redirect_uris.clone()))
        .bind(("kind", kind.to_string()))
        .bind(("expires_at", expires_at))
        .await
        .map_err(|e| e.to_string())?
        .check()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Find a registered client, or fetch (and cache) its Client ID Metadata Document.
pub async fn resolve_client(state: &AppState, client_id: &str) -> Result<Client, String> {
    let cached = find_client(&state.control, client_id).await?;
    if cimd::is_cimd_client_id(client_id) {
        if let Some(c) = cached.filter(|c| c.expires_at.as_ref().is_some_and(|e| e.into_inner() > chrono::Utc::now())) {
            return Ok(c);
        }
        let (meta, ttl) = cimd::fetch(client_id).await?;
        save_client(&state.control, client_id, &meta, "cimd", Some(ttl)).await?;
        return find_client(&state.control, client_id).await?.ok_or_else(|| "client vanished".into());
    }
    cached.ok_or_else(|| "unknown client_id".into())
}

/// Exact match against the registered list; for a registered loopback URI the
/// port may differ (RFC 8252 section 7.3), nothing else may.
pub fn redirect_matches(registered: &[String], presented: &str) -> bool {
    if registered.iter().any(|r| r == presented) {
        return true;
    }
    let Ok(p) = reqwest::Url::parse(presented) else { return false };
    registered.iter().any(|r| {
        let Ok(mut r) = reqwest::Url::parse(r) else { return false };
        if !cimd::is_loopback_host(&r) || !cimd::is_loopback_host(&p) {
            return false;
        }
        let _ = r.set_port(p.port());
        r == p
    })
}

// -- authorization requests ------------------------------------------------

#[derive(Debug, Default, Clone, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(default)]
pub struct AuthzParams {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    pub state: Option<String>,
    pub scope: Option<String>,
    pub resource: Option<String>,
}

pub enum AuthzFail {
    /// The client or redirect URI cannot be trusted: show the error, never redirect.
    Fatal(String),
    Redirect { error: &'static str, description: String },
}

impl AuthzFail {
    fn redirect(error: &'static str, description: &str) -> Self {
        AuthzFail::Redirect { error, description: description.into() }
    }
    pub fn message(&self) -> &str {
        match self {
            AuthzFail::Fatal(m) => m,
            AuthzFail::Redirect { description, .. } => description,
        }
    }
}

pub struct Validated {
    pub client: Client,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
    pub resource: String,
    pub code_challenge: String,
    pub state: Option<String>,
}

fn same_resource(presented: &str, mcp: &str) -> bool {
    let norm = |s: &str| reqwest::Url::parse(s).ok().filter(|u| u.fragment().is_none()).map(|u| u.as_str().trim_end_matches('/').to_string());
    matches!((norm(presented), norm(mcp)), (Some(a), Some(b)) if a == b)
}

fn pkce_charset_ok(s: &str) -> bool {
    (43..=128).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~'))
}

pub async fn validate_authz(state: &AppState, p: &AuthzParams) -> Result<Validated, AuthzFail> {
    if p.client_id.is_empty() {
        return Err(AuthzFail::Fatal("client_id is required".into()));
    }
    let client = resolve_client(state, &p.client_id).await.map_err(AuthzFail::Fatal)?;
    if p.redirect_uri.is_empty() || !redirect_matches(&client.redirect_uris, &p.redirect_uri) {
        return Err(AuthzFail::Fatal("redirect_uri does not match any URI registered for this client".into()));
    }
    if p.response_type != "code" {
        return Err(AuthzFail::redirect("unsupported_response_type", "only response_type=code is supported"));
    }
    if p.code_challenge_method != "S256" || !pkce_charset_ok(&p.code_challenge) {
        return Err(AuthzFail::redirect("invalid_request", "PKCE with code_challenge_method=S256 is required"));
    }
    let requested: Vec<String> = match p.scope.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => s.split_whitespace().map(String::from).collect(),
        None => scopes::DEFAULT.iter().map(|s| s.to_string()).collect(),
    };
    if let Some(bad) = requested.iter().find(|s| !scopes::is_known(s)) {
        return Err(AuthzFail::redirect("invalid_scope", &format!("unknown scope {bad:?}")));
    }
    let mcp = mcp_url(&state.settings);
    if let Some(r) = p.resource.as_deref()
        && !same_resource(r, &mcp)
    {
        return Err(AuthzFail::redirect("invalid_target", &format!("this server only issues tokens for the resource {mcp}")));
    }
    let mut scopes = requested;
    scopes.dedup();
    Ok(Validated {
        client,
        redirect_uri: p.redirect_uri.clone(),
        scopes,
        resource: mcp,
        code_challenge: p.code_challenge.clone(),
        state: p.state.clone(),
    })
}

/// `redirect_uri` plus response parameters, always with the RFC 9207 `iss`.
pub fn redirect_url(state: &AppState, redirect_uri: &str, authz_state: Option<&str>, params: &[(&str, &str)]) -> String {
    let mut url = reqwest::Url::parse(redirect_uri).expect("redirect_uri was validated");
    {
        let mut q = url.query_pairs_mut();
        for (k, v) in params {
            q.append_pair(k, v);
        }
        if let Some(s) = authz_state {
            q.append_pair("state", s);
        }
        q.append_pair("iss", &state.settings.public_url);
    }
    url.to_string()
}

pub fn fail_redirect(state: &AppState, p: &AuthzParams, error: &str, description: &str) -> String {
    redirect_url(state, &p.redirect_uri, p.state.as_deref(), &[("error", error), ("error_description", description)])
}

/// Mint the single-use authorization code (60 seconds) for an approved request.
pub async fn create_code(state: &AppState, v: &Validated, owner: &RecordId) -> Result<String, AppError> {
    let code = generate_token();
    q::OAUTH_CODE_CREATE
        .on(&state.control)
        .bind(("code_hash", hash_token(&code)))
        .bind(("owner", owner.clone()))
        .bind(("client_id", v.client.client_id.clone()))
        .bind(("redirect_uri", v.redirect_uri.clone()))
        .bind(("code_challenge", v.code_challenge.clone()))
        .bind(("scope", v.scopes.clone()))
        .bind(("resource", v.resource.clone()))
        .await?
        .check()?;
    Ok(code)
}

fn plain_error(message: &str) -> Response {
    OAuthError::bad("invalid_request", message).into_response()
}

/// GET /oauth/authorize: validate, then hand the browser to the consent page
/// (via login first when there is no session).
pub async fn authorize(
    State(state): State<AppState>,
    user: Result<User, AppError>,
    Query(p): Query<AuthzParams>,
) -> Response {
    match validate_authz(&state, &p).await {
        Err(AuthzFail::Fatal(m)) => plain_error(&m),
        Err(AuthzFail::Redirect { error, description }) => Redirect::to(&fail_redirect(&state, &p, error, &description)).into_response(),
        Ok(_) => {
            let front = frontend_url(&state.settings);
            let consent = format!("/consent?{}", serde_urlencoded::to_string(&p).unwrap_or_default());
            let target = if user.is_ok() {
                format!("{front}{consent}")
            } else {
                let next = serde_urlencoded::to_string([("next", consent.as_str())]).unwrap_or_default();
                format!("{front}/login?{next}")
            };
            Redirect::to(&target).into_response()
        }
    }
}

// -- token endpoint --------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct TokenForm {
    grant_type: Option<String>,
    code: Option<String>,
    redirect_uri: Option<String>,
    client_id: Option<String>,
    code_verifier: Option<String>,
    refresh_token: Option<String>,
    resource: Option<String>,
    scope: Option<String>,
}

#[derive(Deserialize, SurrealValue)]
pub struct TokenRow {
    pub id: RecordId,
    pub kind: String,
    pub family: RecordId,
    pub expired: bool,
    #[serde(default)]
    #[surreal(default)]
    pub expires_at: Option<Datetime>,
    pub used_at: Option<Datetime>,
    pub owner: Option<RecordId>,
    pub client_id: Option<String>,
    pub scope: Option<Vec<String>>,
    pub resource: Option<String>,
}

/// A row we only need to know exists.
#[derive(Deserialize, SurrealValue)]
pub struct Gone {
    #[allow(dead_code)]
    id: RecordId,
}

pub async fn token_row(db: &ControlDb, token: &str) -> Option<TokenRow> {
    let mut res = q::OAUTH_TOKEN_BY_HASH.on(db).bind(("token_hash", hash_token(token))).await.ok()?;
    res.take::<Vec<TokenRow>>(0).ok()?.into_iter().next()
}

fn required<'a>(v: &'a Option<String>, name: &str) -> Result<&'a str, OAuthError> {
    v.as_deref().filter(|s| !s.is_empty()).ok_or_else(|| OAuthError::bad("invalid_request", format!("{name} is required")))
}

async fn revoke_family(state: &AppState, family: &RecordId, owner: &RecordId) -> Result<(), OAuthError> {
    let db = &state.control;
    state.credentials.forget(&family.to_string());
    q::OAUTH_GRANT_DELETE.on(db).bind(("id", family.clone())).bind(("owner", owner.clone())).await.map_err(OAuthError::server)?;
    q::OAUTH_TOKENS_DELETE_FAMILY.on(db).bind(("family", family.clone())).await.map_err(OAuthError::server)?;
    Ok(())
}

fn token_response(t: super::IssuedTokens, scope: &[String]) -> Response {
    let mut res = Json(json!({
        "access_token": t.access,
        "token_type": "Bearer",
        "expires_in": ACCESS_TTL_SECS,
        "refresh_token": t.refresh,
        "scope": scope.join(" "),
    }))
    .into_response();
    no_store(&mut res);
    res
}

pub async fn token(State(state): State<AppState>, ip: Option<axum::Extension<crate::gate::ClientIp>>, Form(f): Form<TokenForm>) -> Result<Response, OAuthError> {
    // failed grants per registered client and address; a success clears the count. Keyed on the address too,
    // because anyone who knows a client id could otherwise lock that client's users out. CIMD client ids
    // are public URLs shared by every user of that app, so those rely on the per-address bucket alone.
    let fail_key = f
        .client_id
        .as_deref()
        .filter(|c| !c.is_empty() && !c.starts_with("https://"))
        .map(|c| format!("oauth-fail:{c}:{}", ip.as_ref().map_or("", |i| i.0 .0.as_str())));
    if let Some(k) = &fail_key
        && let Err(wait) = state.fail_throttle.check(k, crate::ratelimit::OAUTH_CLIENT_FAILS)
    {
        return Ok(crate::gate::rate_limited(wait));
    }
    let out = match f.grant_type.as_deref() {
        Some("authorization_code") => exchange_code(&state, &f).await,
        Some("refresh_token") => refresh(&state, &f).await,
        _ => Err(OAuthError::bad("unsupported_grant_type", "grant_type must be authorization_code or refresh_token")),
    };
    if let Some(k) = &fail_key {
        match &out {
            // a server fault is not the client's failure
            Err(e) if e.status != StatusCode::INTERNAL_SERVER_ERROR => state.fail_throttle.fail(k),
            Ok(_) => state.fail_throttle.clear(k),
            _ => {}
        }
    }
    out
}

fn check_resource(state: &AppState, resource: &Option<String>) -> Result<(), OAuthError> {
    match resource.as_deref() {
        Some(r) if !same_resource(r, &mcp_url(&state.settings)) => {
            Err(OAuthError::bad("invalid_target", "this server only issues tokens for its MCP URL"))
        }
        _ => Ok(()),
    }
}

async fn exchange_code(state: &AppState, f: &TokenForm) -> Result<Response, OAuthError> {
    #[derive(Deserialize, SurrealValue)]
    struct CodeRow {
        owner: RecordId,
        client_id: String,
        redirect_uri: String,
        code_challenge: String,
        scope: Vec<String>,
        resource: String,
        expires_at: Datetime,
    }
    #[derive(Deserialize, SurrealValue)]
    struct IdRow {
        id: RecordId,
    }

    let client_id = required(&f.client_id, "client_id")?;
    let code = required(&f.code, "code")?;
    let verifier = required(&f.code_verifier, "code_verifier")?;
    let client = resolve_client(state, client_id)
        .await
        .map_err(|e| OAuthError::new(StatusCode::UNAUTHORIZED, "invalid_client", e))?;

    // Taking the code marks it redeemed, so a second redemption finds nothing here.
    let _ = q::OAUTH_CLIENT_TOUCH.on(&state.control).bind(("client_id", client.client_id.clone())).await;
    let code_hash = hash_token(code);
    let mut res = q::OAUTH_CODE_TAKE.on(&state.control).bind(("code_hash", code_hash.clone())).await.map_err(OAuthError::server)?;
    let Some(row) = res.take::<Vec<CodeRow>>(0).map_err(OAuthError::server)?.into_iter().next() else {
        // RFC 6749 4.1.2: a replayed code means the first redeemer may be an attacker, so revoke what it produced.
        #[derive(Deserialize, SurrealValue)]
        struct Marker {
            owner: RecordId,
            grant_id: Option<RecordId>,
        }
        let mut res = q::OAUTH_CODE_REDEEMED.on(&state.control).bind(("code_hash", code_hash)).await.map_err(OAuthError::server)?;
        if let Some(m) = res.take::<Vec<Marker>>(0).map_err(OAuthError::server)?.into_iter().next()
            && let Some(grant) = m.grant_id
        {
            revoke_family(state, &grant, &m.owner).await?;
        }
        return Err(OAuthError::grant("unknown or already used authorization code"));
    };

    if row.expires_at.into_inner() < chrono::Utc::now() {
        return Err(OAuthError::grant("authorization code expired"));
    }
    if row.client_id != client.client_id {
        return Err(OAuthError::grant("authorization code was issued to a different client"));
    }
    if f.redirect_uri.as_deref() != Some(row.redirect_uri.as_str()) {
        return Err(OAuthError::grant("redirect_uri does not match the authorization request"));
    }
    if !pkce_charset_ok(verifier) || URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) != row.code_challenge {
        return Err(OAuthError::grant("PKCE verification failed"));
    }
    check_resource(state, &f.resource)?;
    if row.resource != mcp_url(&state.settings) {
        return Err(OAuthError::grant("authorization code is bound to a different resource"));
    }

    let mut res = q::OAUTH_GRANT_CREATE
        .on(&state.control)
        .bind(("owner", row.owner))
        .bind(("client_id", client.client_id))
        .bind(("client_name", client.name))
        .bind(("client_logo", client.logo_uri))
        .bind(("scope", row.scope.clone()))
        .bind(("resource", row.resource))
        .await
        .map_err(OAuthError::server)?;
    let grant: IdRow = res.take::<Vec<IdRow>>(0).map_err(OAuthError::server)?.into_iter().next().ok_or_else(|| OAuthError::server("grant not created"))?;
    q::OAUTH_CODE_LINK_GRANT.on(&state.control).bind(("code_hash", code_hash)).bind(("grant_id", grant.id.clone())).await.map_err(OAuthError::server)?;
    let tokens = issue_tokens(state, &grant.id, None).await.map_err(OAuthError::server)?;
    Ok(token_response(tokens, &row.scope))
}

async fn refresh(state: &AppState, f: &TokenForm) -> Result<Response, OAuthError> {
    let client_id = required(&f.client_id, "client_id")?;
    let presented = required(&f.refresh_token, "refresh_token")?;
    let row = token_row(&state.control, presented)
        .await
        .filter(|r| r.kind == "refresh")
        .ok_or_else(|| OAuthError::grant("unknown refresh token"))?;
    let (Some(owner), Some(row_client)) = (row.owner.clone(), row.client_id.clone()) else {
        return Err(OAuthError::grant("unknown refresh token"));
    };
    if row_client != client_id {
        return Err(OAuthError::grant("refresh token was issued to a different client"));
    }
    // A spent token coming back means two parties hold it: kill the whole grant.
    if row.used_at.is_some() {
        revoke_family(state, &row.family, &owner).await?;
        return Err(OAuthError::grant("refresh token reuse detected; the grant was revoked"));
    }
    if row.expired {
        return Err(OAuthError::grant("refresh token expired"));
    }
    check_resource(state, &f.resource)?;
    let _ = q::OAUTH_CLIENT_TOUCH.on(&state.control).bind(("client_id", client_id.to_string())).await;
    let mut scope = row.scope.clone().unwrap_or_default();
    // RFC 6749 section 6: the requested scope may be a subset of the granted one, and then that is what is issued.
    let requested: Vec<String> = f.scope.as_deref().unwrap_or_default().split_whitespace().map(String::from).collect();
    if requested.iter().any(|s| !scope.iter().any(|g| g == s)) {
        return Err(OAuthError::bad("invalid_scope", "requested scope exceeds the original grant"));
    }
    if !requested.is_empty() {
        scope = requested;
    }
    let mut res = q::OAUTH_TOKEN_SPEND.on(&state.control).bind(("id", row.id)).await.map_err(OAuthError::server)?;
    if res.take::<Vec<Gone>>(0).map_err(OAuthError::server)?.is_empty() {
        revoke_family(state, &row.family, &owner).await?;
        return Err(OAuthError::grant("refresh token reuse detected; the grant was revoked"));
    }
    let tokens = issue_tokens(state, &row.family, Some(&scope)).await.map_err(OAuthError::server)?;
    Ok(token_response(tokens, &scope))
}

// -- revocation (RFC 7009) -------------------------------------------------

#[derive(Deserialize)]
pub struct RevokeForm {
    token: Option<String>,
    client_id: Option<String>,
}

pub async fn revoke(State(state): State<AppState>, Form(f): Form<RevokeForm>) -> Result<Response, OAuthError> {
    let token = required(&f.token, "token")?;
    // Unknown tokens, and tokens of another client, are not an error (RFC 7009 section 2.2).
    if let Some(row) = token_row(&state.control, token).await
        && f.client_id.as_deref().is_none_or(|c| row.client_id.as_deref() == Some(c))
    {
        if row.kind == "refresh" {
            if let Some(owner) = &row.owner {
                revoke_family(&state, &row.family, owner).await?;
            }
        } else {
            state.credentials.forget_presented(token);
            q::OAUTH_TOKEN_DELETE.on(&state.control).bind(("id", row.id)).await.map_err(OAuthError::server)?;
        }
    }
    Ok(StatusCode::OK.into_response())
}

// -- dynamic client registration (RFC 7591, deprecated by MCP but still used) --

/// Most dynamically registered clients kept at once (see `OAUTH_CLIENT_PRUNE` for how they expire).
const MAX_DCR_CLIENTS: i64 = 5_000;

pub async fn register(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let err = |msg: &str| {
        let mut r = (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid_client_metadata", "error_description": msg }))).into_response();
        no_store(&mut r);
        r
    };
    let redirect_uris = match cimd::redirect_uris_from(&body) {
        Ok(u) => u,
        Err(m) => return err(&m).into_response(),
    };
    let meta = cimd::ClientMeta {
        name: cimd::clean_name(body.get("client_name").and_then(Value::as_str)).unwrap_or_else(|| "Unnamed app".into()),
        logo_uri: cimd::https_only(body.get("logo_uri")),
        client_uri: cimd::https_only(body.get("client_uri")),
        redirect_uris,
    };
    let _ = q::OAUTH_CLIENT_PRUNE.on(&state.control).await;
    // a hard cap on top of the TTL: a flood of registrations cannot grow the table without bound
    #[derive(Deserialize, SurrealValue)]
    struct Count {
        count: i64,
    }
    let registered = match q::OAUTH_CLIENT_DCR_COUNT.on(&state.control).await {
        Ok(mut r) => r.take::<Vec<Count>>(0).ok().and_then(|v| v.first().map(|c| c.count)).unwrap_or(0),
        Err(e) => return OAuthError::server(e).into_response(),
    };
    if registered >= MAX_DCR_CLIENTS {
        let mut r = (StatusCode::SERVICE_UNAVAILABLE, Json(json!({ "error": "temporarily_unavailable", "error_description": "too many registered clients; try again later" }))).into_response();
        no_store(&mut r);
        return r;
    }
    let client_id = format!("eunomia_{}", uuid::Uuid::new_v4().simple());
    if let Err(e) = save_client(&state.control, &client_id, &meta, "dcr", None).await {
        return OAuthError::server(e).into_response();
    }
    let mut res = (
        StatusCode::CREATED,
        Json(json!({
            "client_id": client_id,
            "client_id_issued_at": chrono::Utc::now().timestamp(),
            "client_name": meta.name,
            "redirect_uris": meta.redirect_uris,
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        })),
    )
        .into_response();
    no_store(&mut res);
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uris(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn redirect_matching_is_exact_except_loopback_port() {
        let reg = uris(&["https://claude.ai/cb", "http://127.0.0.1/callback", "cursor://x.y/cb"]);
        assert!(redirect_matches(&reg, "https://claude.ai/cb"));
        assert!(redirect_matches(&reg, "http://127.0.0.1:53211/callback"));
        assert!(redirect_matches(&reg, "cursor://x.y/cb"));
        assert!(!redirect_matches(&reg, "https://claude.ai/cb/"));
        assert!(!redirect_matches(&reg, "https://claude.ai:444/cb"));
        assert!(!redirect_matches(&reg, "https://claude.ai/cb?x=1"));
        assert!(!redirect_matches(&reg, "http://127.0.0.1:53211/other"));
        assert!(!redirect_matches(&reg, "http://localhost:53211/callback"));
        assert!(!redirect_matches(&reg, "https://127.0.0.1:53211/callback"));
        assert!(!redirect_matches(&uris(&["https://claude.ai:8443/cb"]), "https://claude.ai:9999/cb"));
    }

    #[test]
    fn resource_compare_ignores_trailing_slash_and_host_case() {
        assert!(same_resource("http://LOCALHOST:8001/mcp/", "http://localhost:8001/mcp"));
        assert!(!same_resource("http://localhost:8001", "http://localhost:8001/mcp"));
        assert!(!same_resource("https://evil.example/mcp", "http://localhost:8001/mcp"));
        assert!(!same_resource("http://localhost:8001/mcp#x", "http://localhost:8001/mcp"));
    }
}
