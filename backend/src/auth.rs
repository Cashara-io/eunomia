//! Authentication: a browser JWT cookie or a personal API token header, both
//! resolving to the same [`User`].

use surrealdb::types::SurrealValue;
use axum::{
    extract::{FromRef, FromRequestParts},
    http::{header, request::Parts, HeaderMap},
};
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration as StdDuration, Instant};

use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, errors::ErrorKind, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use surrealdb::types::{Datetime, RecordId};
use crate::rid::RecordIdExt;

use crate::authz::{Actor, Caller};
use crate::pool::ControlDb;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::{self, TokenCheck, User};
use crate::state::AppState;
use crate::store;

pub const SESSION_COOKIE: &str = "eunomia_session";

/// A session lives this long past its last use (`SESSION_TTL_DAYS` overrides),
/// extended at most once an hour.
const DEFAULT_SESSION_TTL_DAYS: i64 = 30;
/// Hard ceiling in the JWT itself: even a session used every day is re-issued
/// by a new login after this long.
const JWT_MAX_DAYS: i64 = 90;

fn session_ttl() -> Duration {
    let days = std::env::var("SESSION_TTL_DAYS").ok().and_then(|v| v.parse::<i64>().ok()).filter(|d| *d > 0);
    Duration::days(days.unwrap_or(DEFAULT_SESSION_TTL_DAYS))
}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    email: String,
    sid: String,
    /// Absent on sessions issued before expiry existed; those are still accepted
    /// (the server-side `expires_at` is authoritative).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exp: Option<i64>,
}

/// `exp` is checked when present but not required, so cookies issued before it
/// existed keep working until the server-side expiry catches them.
fn validation() -> Validation {
    let mut v = Validation::new(Algorithm::HS256);
    v.required_spec_claims.clear();
    v.validate_exp = true;
    v
}

pub fn create_session_jwt(user: &User, sid: &str, secret: &str) -> Result<String, AppError> {
    let exp = (Utc::now() + Duration::days(JWT_MAX_DAYS)).timestamp();
    let claims = Claims { sub: user.id.to_string(), email: user.email.clone(), sid: sid.to_string(), exp: Some(exp) };
    encode(&Header::default(), &claims, &EncodingKey::from_secret(secret.as_bytes()))
        .map_err(|e| AppError::internal(e.to_string()))
}

pub async fn start_session(
    db: &ControlDb,
    secret: &str,
    user: &User,
    user_agent: Option<&str>,
) -> Result<String, AppError> {
    let sid = models_user::generate_token();
    let ua: String = user_agent.unwrap_or("").chars().take(300).collect();
    store::control::AUTH_SESSION_CREATE.on(db)
        .bind(("owner", user.id.clone()))
        .bind(("sid", sid.clone()))
        .bind(("user_agent", ua))
        .bind(("expires_at", Datetime::from(Utc::now() + session_ttl())))
        .await?;
    create_session_jwt(user, &sid, secret)
}

/// The authenticated request: the user plus what the credential may do.
#[derive(Debug, Clone)]
pub struct Authn {
    pub user: User,
    pub caller: Caller,
}

/// How long a verified credential is trusted without asking the database again.
const CREDENTIAL_TTL: StdDuration = StdDuration::from_secs(20);
/// Most credentials kept; past it the aged-out ones go, and if none had, all of them.
const CREDENTIAL_CAP: usize = 10_000;

/// Verified credentials (bearer tokens and session cookies) by the SHA-256 of what was presented,
/// so a busy credential costs no database round trip per request. Each entry lives at most
/// [`CREDENTIAL_TTL`] and never past the credential's own expiry. A credential revoked through this
/// process is dropped at once ([`Credentials::forget`]); one revoked through another replica, or
/// changed in the database by hand, keeps working here until its entry ages out (at most the TTL).
#[derive(Default)]
pub struct Credentials {
    map: RwLock<HashMap<String, CachedAuthn>>,
}

struct CachedAuthn {
    authn: Authn,
    /// What revoking it names: the token id, the OAuth grant, or the session row.
    tag: String,
    until: Instant,
    expires_at: Option<chrono::DateTime<Utc>>,
}

impl Credentials {
    fn get(&self, key: &str) -> Option<Authn> {
        let fresh = |c: &CachedAuthn| c.until > Instant::now() && c.expires_at.is_none_or(|e| e > Utc::now());
        {
            let map = self.map.read().unwrap_or_else(|e| e.into_inner());
            match map.get(key) {
                Some(c) if fresh(c) => return Some(c.authn.clone()),
                None => return None,
                Some(_) => {}
            }
        }
        self.map.write().unwrap_or_else(|e| e.into_inner()).remove(key);
        None
    }

    fn put(&self, key: String, tag: String, authn: &Authn, expires_at: Option<chrono::DateTime<Utc>>) {
        let now = Instant::now();
        let mut map = self.map.write().unwrap_or_else(|e| e.into_inner());
        if map.len() >= CREDENTIAL_CAP {
            map.retain(|_, c| c.until > now);
            if map.len() >= CREDENTIAL_CAP {
                map.clear();
            }
        }
        map.insert(key, CachedAuthn { authn: authn.clone(), tag, until: now + CREDENTIAL_TTL, expires_at });
    }

    /// Drops every credential revoking `tag` ends: a token id, an OAuth grant (all its access tokens)
    /// or a session row id.
    pub fn forget(&self, tag: &str) {
        self.map.write().unwrap_or_else(|e| e.into_inner()).retain(|_, c| c.tag != tag);
    }

    /// Drops one presented credential (a token or a session cookie).
    pub fn forget_presented(&self, presented: &str) {
        self.map.write().unwrap_or_else(|e| e.into_inner()).remove(&models_user::hash_token(presented));
    }

    /// Drops everything (a test that edits credentials in the database directly).
    pub fn clear(&self) {
        self.map.write().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

async fn session_authn(state: &AppState, token: &str) -> AppResult<Option<Authn>> {
    let key = models_user::hash_token(token);
    if let Some(a) = state.credentials.get(&key) {
        return Ok(Some(a));
    }
    let db = &state.control;
    let data = match decode::<Claims>(token, &DecodingKey::from_secret(state.settings.jwt_secret.as_bytes()), &validation()) {
        Ok(d) => d,
        Err(e) if matches!(e.kind(), ErrorKind::ExpiredSignature) => {
            return Err(AppError::coded(ErrorCode::AuthSessionExpired, "Your session has expired. Log in again."));
        }
        Err(_) => return Ok(None),
    };
    let claims = data.claims;
    let Ok(rid) = crate::rid::parse(&claims.sub) else { return Ok(None) };

    #[derive(Deserialize, SurrealValue)]
    struct SessionRow {
        id: RecordId,
        #[serde(default)]
        expired: bool,
        #[serde(default)]
        #[surreal(default)]
        expires_at: Option<Datetime>,
    }
    let mut res = store::control::AUTH_SESSION_FIND
        .on(db)
        .bind(("sid", claims.sid.clone()))
        .bind(("owner", rid.clone()))
        .await?;
    let rows: Vec<SessionRow> = res.take(0)?;
    let Some(row) = rows.into_iter().next() else { return Ok(None) };

    let now = Utc::now();
    if row.expired {
        return Err(AppError::coded(ErrorCode::AuthSessionExpired, "Your session has expired. Log in again."));
    }

    let ttl = session_ttl();
    let _ = store::control::AUTH_SESSION_TOUCH
        .on(db)
        .bind(("id", row.id.clone()))
        .bind(("new_exp", Datetime::from(now + ttl)))
        .bind(("threshold", Datetime::from(now + ttl - Duration::hours(1))))
        .await;

    let user = models_user::load_user(db, rid, claims.email).await?;
    let caller = Caller::session(&user.id);
    let authn = Authn { user, caller };
    // the earlier of the row's expiry (as read, before any slide) and the JWT's own
    let jwt_exp = claims.exp.and_then(|e| chrono::DateTime::from_timestamp(e, 0));
    let expires_at = row.expires_at.map(chrono::DateTime::from).into_iter().chain(jwt_exp).min();
    state.credentials.put(key, row.id.to_string(), &authn, expires_at);
    Ok(Some(authn))
}

/// A Bearer personal access token, or (on `/mcp` only, `allow_oauth`) an OAuth
/// access token. `Ok(None)` means unknown; an expired PAT is an error. This is
/// the one place a bearer credential becomes a [`Caller`].
pub async fn bearer_authn(state: &AppState, token: &str, allow_oauth: bool) -> AppResult<Option<Authn>> {
    let oauth = crate::oauth::is_access_token(token);
    if oauth && !allow_oauth {
        return Ok(None);
    }
    let key = models_user::hash_token(token);
    if let Some(a) = state.credentials.get(&key) {
        return Ok(Some(a));
    }
    if oauth {
        let Some(v) = crate::oauth::verify_access_token(state, token).await else { return Ok(None) };
        let caller = Caller { actor: Actor { kind: "oauth", id: v.client_id }, scopes: v.scopes, vault: None, expires_at: None };
        let authn = Authn { user: v.user, caller };
        state.credentials.put(key, v.grant.to_string(), &authn, v.expires_at);
        return Ok(Some(authn));
    }
    match models_user::check_api_token(&state.control, token).await? {
        TokenCheck::Valid(v) => {
            let actor = Actor { kind: "token", id: v.token_id.to_string() };
            let authn = Authn { user: v.user, caller: Caller { actor, scopes: v.scopes, vault: v.vault, expires_at: v.expires_at } };
            state.credentials.put(key, v.token_id.to_string(), &authn, v.expires_at);
            Ok(Some(authn))
        }
        TokenCheck::Expired => Err(AppError::coded(ErrorCode::AuthTokenExpired, "This API token has expired.")),
        TokenCheck::Unknown => Ok(None),
    }
}

pub fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    value.strip_prefix("Bearer ").or_else(|| value.strip_prefix("bearer ")).map(str::trim)
}

pub fn session_token(headers: &HeaderMap) -> Option<String> {
    extract_cookie(headers.get(header::COOKIE)?.to_str().ok()?, SESSION_COOKIE)
}

/// Resolves the request's credential: a Bearer token first, then (unless
/// `mcp`, which takes tokens only) the session cookie.
pub async fn authenticate(state: &AppState, headers: &HeaderMap, mcp: bool) -> AppResult<Option<Authn>> {
    if let Some(token) = bearer_token(headers)
        && let Some(authn) = bearer_authn(state, token, mcp).await?
    {
        return Ok(Some(authn));
    }
    if !mcp
        && let Some(token) = session_token(headers)
    {
        return session_authn(state, &token).await;
    }
    Ok(None)
}

/// The `sid` of the session cookie on this request, if it carries a well-formed one.
pub fn session_sid(secret: &str, headers: &HeaderMap) -> Option<String> {
    let token = session_token(headers)?;
    decode::<Claims>(&token, &DecodingKey::from_secret(secret.as_bytes()), &validation()).ok().map(|d| d.claims.sid)
}

pub async fn revoke_session_by_jwt(state: &AppState, token: &str) {
    state.credentials.forget_presented(token);
    let Ok(data) = decode::<Claims>(
        token,
        &DecodingKey::from_secret(state.settings.jwt_secret.as_bytes()),
        &validation(),
    ) else {
        return;
    };
    let _ = store::control::AUTH_SESSION_REVOKE_BY_SID
        .on(&state.control)
        .bind(("sid", data.claims.sid))
        .await;
}

/// Axum extractor: the user the gate (`gate.rs`) authenticated for this
/// request. Rejects with 401 if the request carried no valid credential.
impl<S> FromRequestParts<S> for User
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        match parts.extensions.get::<Authn>() {
            Some(a) => {
                crate::telemetry::record_user(&a.user.id.to_string());
                Ok(a.user.clone())
            }
            None => Err(AppError::unauthorized("Not authenticated.")),
        }
    }
}

fn extract_cookie(header: &str, name: &str) -> Option<String> {
    header.split(';').find_map(|part| {
        let part = part.trim();
        let (key, value) = part.split_once('=')?;
        (key == name).then(|| value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_user() -> User {
        User { id: crate::rid::parse("user:abc123").unwrap(), email: "a@example.com".to_string(), org: crate::pool::OrgId::new() }
    }

    #[test]
    fn jwt_roundtrips_claims() {
        let user = test_user();
        let token = create_session_jwt(&user, "sid-1", "test-secret").unwrap();
        let data = decode::<Claims>(
            &token,
            &DecodingKey::from_secret(b"test-secret"),
            &validation(),
        )
        .unwrap();
        assert_eq!(data.claims.sub, user.id.to_string());
        assert_eq!(data.claims.email, user.email);
        assert_eq!(data.claims.sid, "sid-1");
    }

    #[test]
    fn jwt_rejects_wrong_secret() {
        let user = test_user();
        let token = create_session_jwt(&user, "sid-1", "test-secret").unwrap();
        let result = decode::<Claims>(
            &token,
            &DecodingKey::from_secret(b"wrong-secret"),
            &validation(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn extract_cookie_finds_named_cookie_among_several() {
        let header = "foo=bar; eunomia_session=the-token; baz=qux";
        assert_eq!(extract_cookie(header, SESSION_COOKIE), Some("the-token".to_string()));
    }

    #[test]
    fn extract_cookie_returns_none_when_absent() {
        let header = "foo=bar; baz=qux";
        assert_eq!(extract_cookie(header, SESSION_COOKIE), None);
    }
}
