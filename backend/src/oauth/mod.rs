//! Built-in OAuth 2.1 authorization server for MCP clients, implementing the
//! MCP authorization spec revision 2026-07-28 (RFC 9728 protected resource
//! metadata, RFC 8414 server metadata, Client ID Metadata Documents, RFC 7591
//! dynamic registration, RFC 8707 resource indicators, PKCE S256 only).
//!
//! Access tokens are opaque (`eoa_...`), stored as sha256 hashes, valid 15
//! minutes, and bound to user, client and this server's MCP URL. Refresh
//! tokens (`eor_...`) rotate on every use; replaying a spent one revokes the
//! whole grant. Tokens are accepted on `/mcp` only, never on `/api`.

pub mod cimd;
pub mod server;

use surrealdb::types::SurrealValue;
use axum::{
    Json, Router,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};

use crate::config::Settings;
use crate::models_user::{User, generate_token, hash_token};
use crate::scopes;
use crate::state::AppState;

pub const ACCESS_PREFIX: &str = "eoa_";
pub const REFRESH_PREFIX: &str = "eor_";
pub const ACCESS_TTL_SECS: u64 = 15 * 60;
const REFRESH_TTL: &str = "30d";

/// The canonical MCP resource URI: what tokens are audience-bound to.
pub fn mcp_url(s: &Settings) -> String {
    format!("{}/mcp", s.public_url)
}

pub fn resource_metadata_url(s: &Settings) -> String {
    format!("{}/.well-known/oauth-protected-resource/mcp", s.public_url)
}

/// Where the browser consent and login pages live: the first allowed frontend origin.
pub fn frontend_url(s: &Settings) -> String {
    s.cors_allowed_origins
        .split(',')
        .map(|o| o.trim().trim_end_matches('/'))
        .find(|o| !o.is_empty())
        .unwrap_or(&s.public_url)
        .to_string()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/.well-known/oauth-protected-resource", get(protected_resource))
        .route("/.well-known/oauth-protected-resource/mcp", get(protected_resource))
        .route("/.well-known/oauth-authorization-server", get(authorization_server))
        .route("/oauth/authorize", get(server::authorize))
        .route("/oauth/token", post(server::token))
        .route("/oauth/register", post(server::register))
        .route("/oauth/revoke", post(server::revoke))
}

async fn protected_resource(axum::extract::State(state): axum::extract::State<AppState>) -> Json<Value> {
    Json(json!({
        "resource": mcp_url(&state.settings),
        "authorization_servers": [state.settings.public_url],
        "scopes_supported": scopes::ALL,
        "bearer_methods_supported": ["header"],
        "resource_name": "Eunomia",
    }))
}

async fn authorization_server(axum::extract::State(state): axum::extract::State<AppState>) -> Json<Value> {
    let base = &state.settings.public_url;
    Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "registration_endpoint": format!("{base}/oauth/register"),
        "revocation_endpoint": format!("{base}/oauth/revoke"),
        "scopes_supported": scopes::ALL,
        "response_types_supported": ["code"],
        "response_modes_supported": ["query"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_methods_supported": ["none"],
        "revocation_endpoint_auth_methods_supported": ["none"],
        "code_challenge_methods_supported": ["S256"],
        "client_id_metadata_document_supported": true,
        "authorization_response_iss_parameter_supported": true,
    }))
}

// -- /mcp integration ------------------------------------------------------

/// The `WWW-Authenticate` challenge for a 401 on `/mcp` (RFC 9728 section 5.1, RFC 6750).
pub fn www_authenticate(s: &Settings, invalid_token: bool) -> String {
    let error = if invalid_token { r#"error="invalid_token", "# } else { "" };
    format!(
        r#"Bearer {error}resource_metadata="{}", scope="{}""#,
        resource_metadata_url(s),
        scopes::DEFAULT.join(" ")
    )
}

pub fn is_access_token(token: &str) -> bool {
    token.starts_with(ACCESS_PREFIX)
}

/// A verified OAuth access token: whose it is, what it may do, and which client and grant it belongs to.
pub struct VerifiedAccess {
    pub user: User,
    pub scopes: Vec<String>,
    pub client_id: String,
    pub grant: surrealdb::types::RecordId,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Verify an OAuth access token presented on `/mcp`: known, unexpired, grant
/// still present, and issued for this server's MCP URL.
pub async fn verify_access_token(state: &AppState, token: &str) -> Option<VerifiedAccess> {
    let row = server::token_row(&state.control, token).await?;
    if row.kind != "access" || row.expired || row.resource.as_deref() != Some(mcp_url(&state.settings).as_str()) {
        return None;
    }
    let owner = row.owner?;
    #[derive(serde::Deserialize, SurrealValue)]
    struct UserRow {
        id: surrealdb::types::RecordId,
        email: String,
    }
    let user: UserRow = crate::store::get_control(&state.control, &owner).await.ok()??;
    // at most once a minute (the statement checks), and the request does not wait for it
    let (control, grant) = (state.control.clone(), row.family.clone());
    tokio::spawn(async move {
        let _ = crate::store::control::OAUTH_GRANT_TOUCH.on(&control).bind(("id", grant)).await;
    });
    let user = crate::models_user::load_user(&state.control, user.id, user.email).await.ok()?;
    Some(VerifiedAccess {
        user,
        scopes: row.scope.unwrap_or_default(),
        client_id: row.client_id.unwrap_or_default(),
        grant: row.family,
        expires_at: row.expires_at.map(chrono::DateTime::from),
    })
}

/// 403 `insufficient_scope` when a `tools/call` in `message` (single or batch)
/// needs a scope the token lacks. All missing scopes go in one challenge.
pub fn scope_challenge(s: &Settings, message: &Value, granted: &[String]) -> Option<Response> {
    let calls: Vec<&Value> = match message {
        Value::Array(batch) => batch.iter().collect(),
        m => vec![m],
    };
    let mut missing: Vec<&'static str> = Vec::new();
    for m in calls {
        if m.get("method").and_then(Value::as_str) != Some("tools/call") {
            continue;
        }
        let Some(spec) = m.pointer("/params/name").and_then(Value::as_str).and_then(|n| {
            crate::tools::registry::all_tools().get(n).map(|spec| (n, spec))
        }) else {
            continue;
        };
        let need = scopes::for_tool(spec.0, spec.1.read_only);
        if !scopes::allows(granted, need) && !missing.contains(&need) {
            missing.push(need);
        }
    }
    if missing.is_empty() {
        return None;
    }
    let scope = missing.join(" ");
    let challenge = format!(
        r#"Bearer error="insufficient_scope", scope="{scope}", resource_metadata="{}", error_description="This token lacks the scope needed for this tool""#,
        resource_metadata_url(s)
    );
    Some((StatusCode::FORBIDDEN, [(header::WWW_AUTHENTICATE, challenge)], Json(json!({
        "error": "insufficient_scope",
        "error_description": format!("This token lacks the scope needed for this tool: {scope}"),
    }))).into_response())
}

// -- token minting ---------------------------------------------------------

pub struct IssuedTokens {
    pub access: String,
    pub refresh: String,
}

/// Mint an access and a refresh token in `family` (a grant id). `scope` narrows them to a subset
/// of the grant's scope (a refresh that asked for less); `None` carries the grant's scope.
pub async fn issue_tokens(state: &AppState, family: &surrealdb::types::RecordId, scope: Option<&[String]>) -> Result<IssuedTokens, surrealdb::Error> {
    let access = format!("{ACCESS_PREFIX}{}", generate_token());
    let refresh = format!("{REFRESH_PREFIX}{}", generate_token());
    let ttl = format!("{ACCESS_TTL_SECS}s");
    for (kind, tok, ttl) in [("access", &access, ttl.as_str()), ("refresh", &refresh, REFRESH_TTL)] {
        crate::store::control::OAUTH_TOKEN_CREATE
            .on(&state.control)
            .bind(("kind", kind))
            .bind(("token_hash", hash_token(tok)))
            .bind(("family", family.clone()))
            .bind(("scope", scope.map(<[String]>::to_vec)))
            .bind(("ttl", ttl.to_string()))
            .await?
            .check()?;
    }
    // ponytail: pruning rides on issuance, so an idle server never accumulates garbage it did not create
    let _ = crate::store::control::OAUTH_PRUNE.on(&state.control).await;
    Ok(IssuedTokens { access, refresh })
}
