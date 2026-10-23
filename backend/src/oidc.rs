//! OpenID Connect sign-in for an install that delegates login to an external provider (AuthKit,
//! Okta, Entra ID, Google, Keycloak, ...). This module is the protocol half: configuration, discovery,
//! the authorization-code + PKCE exchange and ID-token validation. `routers/oidc.rs` owns the HTTP
//! routes and the account mapping.
//!
//! Provider calls: discovery (`/.well-known/openid-configuration`) and the JWKS are cached; every
//! outbound request has a timeout, never follows redirects and has its body capped. The endpoints a
//! discovery document names must be https (http only for a loopback host, for local development).
//!
//! ID tokens are checked in full: the signature against the provider's JWKS (an unknown `kid`
//! refetches the set, rate limited, so key rotation works), only asymmetric algorithms are accepted
//! (`none` and `HS*` never are), then `iss`, `aud` (and `azp`), `exp`/`nbf` and the `nonce` the flow
//! stored. SAML would be a second provider kind that ends in the same verified-claims shape
//! ([`Identity`]), so the account mapping would not change.

use std::sync::RwLock;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// `AUTH_MODE`: which sign-in methods exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AuthMode {
    /// Email and password only (the default, and what every install did before OIDC).
    #[default]
    Password,
    /// Email and password, plus "Continue with {provider}".
    Both,
    /// Provider only: password login and signup answer 403 `auth.password_disabled`.
    Oidc,
}

impl AuthMode {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthMode::Password => "password",
            AuthMode::Both => "both",
            AuthMode::Oidc => "oidc",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct OidcConfig {
    pub issuer: String,
    pub client_id: String,
    /// `None` for a public client (PKCE only).
    pub client_secret: Option<String>,
    pub scopes: String,
    /// Shown on the button: "Continue with {provider_name}".
    pub provider_name: String,
}

impl std::fmt::Debug for OidcConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcConfig")
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.as_ref().map(|_| "<redacted>"))
            .field("scopes", &self.scopes)
            .field("provider_name", &self.provider_name)
            .finish()
    }
}

#[derive(Debug, Clone, Default)]
pub struct AuthConfig {
    pub mode: AuthMode,
    /// Set when the mode is `both` or `oidc`.
    pub oidc: Option<OidcConfig>,
}

impl AuthConfig {
    pub fn password_enabled(&self) -> bool {
        self.mode != AuthMode::Oidc
    }

    /// Reads `AUTH_MODE` and `OIDC_*` through `get`. An invalid combination is an error, never a fallback:
    /// a lock-down setting that is mistyped must not quietly leave passwords on.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let var = |k: &str| get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let mode = match var("AUTH_MODE").map(|m| m.to_ascii_lowercase()).as_deref() {
            None | Some("password") => AuthMode::Password,
            Some("both") => AuthMode::Both,
            Some("oidc") => AuthMode::Oidc,
            Some(other) => return Err(format!("AUTH_MODE={other:?} is not one of password, both, oidc")),
        };
        if mode == AuthMode::Password {
            if var("OIDC_ISSUER").is_some() {
                tracing::warn!("OIDC_ISSUER is set but AUTH_MODE=password: OIDC sign-in is off. Set AUTH_MODE=both or oidc to use it.");
            }
            return Ok(AuthConfig { mode, oidc: None });
        }
        let need = |k: &str| var(k).ok_or_else(|| format!("AUTH_MODE={} needs {k}", mode.as_str()));
        let issuer = need("OIDC_ISSUER")?;
        let client_id = need("OIDC_CLIENT_ID")?;
        if !endpoint_ok(&issuer) {
            return Err("OIDC_ISSUER must be an https URL (http only for localhost)".into());
        }
        let scopes = var("OIDC_SCOPES").unwrap_or_else(|| "openid email profile".into());
        let provider_name = var("OIDC_PROVIDER_NAME").unwrap_or_else(|| "single sign-on".into());
        Ok(AuthConfig {
            mode,
            oidc: Some(OidcConfig { issuer, client_id, client_secret: var("OIDC_CLIENT_SECRET"), scopes, provider_name }),
        })
    }
}

/// https, or http for a loopback host (local development and tests).
pub fn endpoint_ok(raw: &str) -> bool {
    let Ok(u) = url::Url::parse(raw) else { return false };
    if !u.username().is_empty() || u.password().is_some() || u.fragment().is_some() {
        return false;
    }
    match (u.scheme(), u.host()) {
        ("https", Some(_)) => true,
        ("http", Some(url::Host::Domain(d))) => d.eq_ignore_ascii_case("localhost") || d.to_ascii_lowercase().ends_with(".localhost"),
        ("http", Some(url::Host::Ipv4(ip))) => ip.is_loopback(),
        ("http", Some(url::Host::Ipv6(ip))) => ip.is_loopback(),
        _ => false,
    }
}

// -- discovery, JWKS, token endpoint ------------------------------------------------------------

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
/// No provider document, key set or token response is anywhere near this.
const MAX_BODY: usize = 1 << 20;
const DISCOVERY_TTL: Duration = Duration::from_secs(3600);
const JWKS_TTL: Duration = Duration::from_secs(3600);
/// An ID token naming a `kid` we do not hold refetches the JWKS, but never more often than this.
const DEFAULT_REFETCH_COOLDOWN: Duration = Duration::from_secs(30);
const CLOCK_LEEWAY_SECS: u64 = 30;

/// Why a sign-in attempt failed.
#[derive(Debug)]
pub enum OidcError {
    /// The provider could not be reached or answered with something unusable.
    Provider(String),
    /// The ID token did not validate.
    Token(String),
}

impl std::fmt::Display for OidcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OidcError::Provider(m) | OidcError::Token(m) => f.write_str(m),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Discovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    #[serde(default)]
    pub userinfo_endpoint: Option<String>,
    #[serde(default)]
    pub token_endpoint_auth_methods_supported: Option<Vec<String>>,
}

struct Cached<T> {
    value: T,
    fetched: Instant,
}

/// The verified claims of an ID token (or the userinfo fallback): what the account mapping needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub issuer: String,
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
}

pub struct Client {
    http: reqwest::Client,
    discovery: RwLock<Option<Cached<Discovery>>>,
    jwks: RwLock<Option<Cached<JwkSet>>>,
    refetch_cooldown_ms: std::sync::atomic::AtomicU64,
}

impl Default for Client {
    fn default() -> Self {
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("eunomia/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        Client {
            http,
            discovery: RwLock::new(None),
            jwks: RwLock::new(None),
            refetch_cooldown_ms: std::sync::atomic::AtomicU64::new(DEFAULT_REFETCH_COOLDOWN.as_millis() as u64),
        }
    }
}

fn provider_err(what: &str, e: impl std::fmt::Display) -> OidcError {
    OidcError::Provider(format!("{what}: {e}"))
}

/// The response body, refusing more than [`MAX_BODY`].
async fn read_capped(mut resp: reqwest::Response) -> Result<Vec<u8>, OidcError> {
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| provider_err("reading the response", e))? {
        if out.len() + chunk.len() > MAX_BODY {
            return Err(OidcError::Provider("the response is too large".into()));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

impl Client {
    /// Tests only: how soon an unknown `kid` may refetch the key set again.
    #[cfg(feature = "test-support")]
    pub fn set_refetch_cooldown(&self, d: Duration) {
        self.refetch_cooldown_ms.store(d.as_millis() as u64, std::sync::atomic::Ordering::Relaxed);
    }

    fn cooldown(&self) -> Duration {
        Duration::from_millis(self.refetch_cooldown_ms.load(std::sync::atomic::Ordering::Relaxed))
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str, what: &str) -> Result<T, OidcError> {
        if !endpoint_ok(url) {
            return Err(OidcError::Provider(format!("{what} is not an https URL")));
        }
        let resp = self.http.get(url).header("accept", "application/json").send().await.map_err(|e| provider_err(what, e))?;
        if !resp.status().is_success() {
            return Err(OidcError::Provider(format!("{what} answered {}", resp.status())));
        }
        let bytes = read_capped(resp).await?;
        serde_json::from_slice(&bytes).map_err(|e| provider_err(what, e))
    }

    /// The provider's discovery document, cached for an hour. A failed refresh keeps serving the old one.
    pub async fn discovery(&self, cfg: &OidcConfig) -> Result<Discovery, OidcError> {
        let fresh = |c: &Cached<Discovery>| c.fetched.elapsed() < DISCOVERY_TTL;
        if let Some(c) = self.discovery.read().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|c| fresh(c)) {
            return Ok(c.value.clone());
        }
        let url = format!("{}/.well-known/openid-configuration", cfg.issuer.trim_end_matches('/'));
        let fetched: Result<Discovery, OidcError> = async {
            let d: Discovery = self.get_json(&url, "discovery").await?;
            // the document must describe the issuer we were configured with (OIDC Discovery 4.3)
            if d.issuer != cfg.issuer {
                return Err(OidcError::Provider("the discovery document names a different issuer".into()));
            }
            for (name, u) in [("authorization_endpoint", &d.authorization_endpoint), ("token_endpoint", &d.token_endpoint), ("jwks_uri", &d.jwks_uri)] {
                if !endpoint_ok(u) {
                    return Err(OidcError::Provider(format!("{name} is not an https URL")));
                }
            }
            if d.userinfo_endpoint.as_deref().is_some_and(|u| !endpoint_ok(u)) {
                return Err(OidcError::Provider("userinfo_endpoint is not an https URL".into()));
            }
            Ok(d)
        }
        .await;
        match fetched {
            Ok(d) => {
                *self.discovery.write().unwrap_or_else(|e| e.into_inner()) = Some(Cached { value: d.clone(), fetched: Instant::now() });
                Ok(d)
            }
            Err(e) => match self.discovery.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
                Some(stale) => {
                    tracing::warn!(error = %e, "OIDC discovery refresh failed, using the cached document");
                    Ok(stale.value.clone())
                }
                None => Err(e),
            },
        }
    }

    async fn fetch_jwks(&self, uri: &str) -> Result<JwkSet, OidcError> {
        let set: JwkSet = self.get_json(uri, "jwks").await?;
        *self.jwks.write().unwrap_or_else(|e| e.into_inner()) = Some(Cached { value: set.clone(), fetched: Instant::now() });
        Ok(set)
    }

    /// The key set: cached, refetched when stale, and refetched (at most once per cooldown) when the
    /// token names a `kid` it does not contain, which is how a provider rotates keys.
    async fn key_set(&self, uri: &str, wanted_kid: Option<&str>) -> Result<JwkSet, OidcError> {
        let cached = self.jwks.read().unwrap_or_else(|e| e.into_inner()).as_ref().map(|c| (c.value.clone(), c.fetched));
        match cached {
            None => self.fetch_jwks(uri).await,
            Some((set, fetched)) if fetched.elapsed() >= JWKS_TTL => self.fetch_jwks(uri).await.or(Ok(set)),
            Some((set, fetched)) => {
                let missing = wanted_kid.is_some_and(|k| !set.keys.iter().any(|j| j.common.key_id.as_deref() == Some(k)));
                if missing && fetched.elapsed() >= self.cooldown() { self.fetch_jwks(uri).await.or(Ok(set)) } else { Ok(set) }
            }
        }
    }

    /// Exchanges the authorization code. Returns the ID token and the access token.
    pub async fn exchange_code(&self, cfg: &OidcConfig, disc: &Discovery, code: &str, redirect_uri: &str, verifier: &str) -> Result<(String, Option<String>), OidcError> {
        let mut form: Vec<(&str, &str)> = vec![("grant_type", "authorization_code"), ("code", code), ("redirect_uri", redirect_uri), ("code_verifier", verifier)];
        let mut req = self.http.post(&disc.token_endpoint).header("accept", "application/json");
        match &cfg.client_secret {
            // client_secret_basic is the default; use _post only when the provider lists it and not basic
            Some(secret) => {
                let methods = disc.token_endpoint_auth_methods_supported.as_deref().unwrap_or(&[]);
                if !methods.is_empty() && !methods.iter().any(|m| m == "client_secret_basic") && methods.iter().any(|m| m == "client_secret_post") {
                    form.push(("client_id", &cfg.client_id));
                    form.push(("client_secret", secret));
                } else {
                    // RFC 6749 2.3.1: id and secret are form-urlencoded before they are base64'd
                    let enc = |s: &str| serde_urlencoded::to_string([("", s)]).unwrap_or_default().trim_start_matches('=').to_string();
                    req = req.basic_auth(enc(&cfg.client_id), Some(enc(secret)));
                }
            }
            None => form.push(("client_id", &cfg.client_id)),
        }
        let resp = req.form(&form).send().await.map_err(|e| provider_err("token request", e))?;
        let status = resp.status();
        let bytes = read_capped(resp).await?;
        #[derive(Deserialize)]
        struct Tok {
            id_token: Option<String>,
            access_token: Option<String>,
            error: Option<String>,
        }
        let tok: Tok = serde_json::from_slice(&bytes).map_err(|_| OidcError::Provider(format!("token endpoint answered {status} with a body that is not JSON")))?;
        if !status.is_success() {
            return Err(OidcError::Provider(format!("token endpoint answered {status} ({})", tok.error.unwrap_or_default())));
        }
        let id_token = tok.id_token.ok_or_else(|| OidcError::Provider("the token response has no id_token".into()))?;
        Ok((id_token, tok.access_token))
    }

    /// Validates an ID token and returns its claims as an [`Identity`]. `nonce` is the value the flow stored.
    pub async fn verify_id_token(&self, cfg: &OidcConfig, disc: &Discovery, token: &str, nonce: &str) -> Result<Identity, OidcError> {
        let bad = |m: &str| OidcError::Token(m.to_string());
        let header = decode_header(token).map_err(|_| bad("not a signed JWT"))?;
        // an allow-list, not a deny-list: `none` does not parse into an Algorithm at all, and HS* would let a
        // public key act as an HMAC secret
        let alg = header.alg;
        if !matches!(
            alg,
            Algorithm::RS256 | Algorithm::RS384 | Algorithm::RS512 | Algorithm::PS256 | Algorithm::PS384 | Algorithm::PS512 | Algorithm::ES256 | Algorithm::ES384 | Algorithm::EdDSA
        ) {
            return Err(bad("unsupported signing algorithm"));
        }
        let set = self.key_set(&disc.jwks_uri, header.kid.as_deref()).await?;
        let keys: Vec<_> = set
            .keys
            .iter()
            .filter(|j| header.kid.is_none() || j.common.key_id == header.kid)
            .filter(|j| j.common.public_key_use.as_ref().is_none_or(|u| matches!(u, jsonwebtoken::jwk::PublicKeyUse::Signature)))
            .filter(|j| j.common.key_algorithm.is_none_or(|a| format!("{a:?}") == format!("{alg:?}")))
            .collect();
        if keys.is_empty() {
            return Err(bad("no matching signing key"));
        }
        let mut validation = Validation::new(alg);
        validation.set_issuer(&[cfg.issuer.as_str()]);
        validation.set_audience(&[cfg.client_id.as_str()]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.validate_nbf = true;
        validation.leeway = CLOCK_LEEWAY_SECS;
        #[derive(Deserialize)]
        struct Claims {
            sub: String,
            aud: serde_json::Value,
            #[serde(default)]
            azp: Option<String>,
            #[serde(default)]
            nonce: Option<String>,
            #[serde(default)]
            email: Option<String>,
            #[serde(default)]
            email_verified: Option<serde_json::Value>,
        }
        let mut last = None;
        let mut claims = None;
        for jwk in keys.iter().take(8) {
            let Ok(key) = DecodingKey::from_jwk(jwk) else { continue };
            match decode::<Claims>(token, &key, &validation) {
                Ok(d) => {
                    claims = Some(d.claims);
                    break;
                }
                Err(e) => last = Some(e),
            }
        }
        let Some(c) = claims else {
            use jsonwebtoken::errors::ErrorKind::*;
            return Err(bad(match last.map(|e| e.into_kind()) {
                Some(ExpiredSignature) => "the ID token has expired",
                Some(InvalidIssuer) => "the ID token has the wrong issuer",
                Some(InvalidAudience) => "the ID token was issued for a different client",
                Some(InvalidSignature) => "the ID token signature is invalid",
                _ => "the ID token is invalid",
            }));
        };
        // several audiences: the authorized party must be us (OIDC Core 3.1.3.7); a lone azp must be us too
        let multiple = c.aud.as_array().is_some_and(|a| a.len() > 1);
        if (multiple && c.azp.is_none()) || c.azp.as_deref().is_some_and(|a| a != cfg.client_id) {
            return Err(bad("the ID token's authorized party is not this client"));
        }
        if nonce.is_empty() || !constant_time_eq(c.nonce.as_deref().unwrap_or(""), nonce) {
            return Err(bad("the ID token nonce does not match"));
        }
        if c.sub.is_empty() || c.sub.len() > 255 {
            return Err(bad("the ID token has no usable subject"));
        }
        Ok(Identity { issuer: cfg.issuer.clone(), subject: c.sub, email: c.email, email_verified: c.email_verified.as_ref().is_some_and(truthy) })
    }

    /// The userinfo claims, for a provider whose ID token carries no email. The subject must match the verified one.
    pub async fn userinfo(&self, disc: &Discovery, access_token: &str, subject: &str) -> Option<(Option<String>, bool)> {
        let url = disc.userinfo_endpoint.as_deref()?;
        let resp = self.http.get(url).bearer_auth(access_token).header("accept", "application/json").send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let v: serde_json::Value = serde_json::from_slice(&read_capped(resp).await.ok()?).ok()?;
        (v.get("sub").and_then(|s| s.as_str()) == Some(subject))
            .then(|| (v.get("email").and_then(|e| e.as_str()).map(String::from), v.get("email_verified").is_some_and(truthy)))
    }
}

/// `email_verified` is a boolean, but some providers send the string "true".
fn truthy(v: &serde_json::Value) -> bool {
    v.as_bool().unwrap_or_else(|| v.as_str().is_some_and(|s| s.eq_ignore_ascii_case("true")))
}

pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// URL-safe random string of `bytes` random bytes.
pub fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

/// PKCE S256 challenge for a verifier.
pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn default_is_password_only() {
        let c = AuthConfig::from_vars(vars(&[])).unwrap();
        assert_eq!(c.mode, AuthMode::Password);
        assert!(c.oidc.is_none() && c.password_enabled());
    }

    #[test]
    fn modes_need_their_settings_and_typos_are_errors() {
        assert!(AuthConfig::from_vars(vars(&[("AUTH_MODE", "oicd")])).is_err());
        assert!(AuthConfig::from_vars(vars(&[("AUTH_MODE", "oidc")])).is_err());
        assert!(AuthConfig::from_vars(vars(&[("AUTH_MODE", "oidc"), ("OIDC_ISSUER", "https://idp.example")])).is_err());
        assert!(AuthConfig::from_vars(vars(&[("AUTH_MODE", "oidc"), ("OIDC_ISSUER", "http://idp.example"), ("OIDC_CLIENT_ID", "x")])).is_err(), "plain http to a remote host");
        let c = AuthConfig::from_vars(vars(&[("AUTH_MODE", " OIDC "), ("OIDC_ISSUER", "https://idp.example"), ("OIDC_CLIENT_ID", "x"), ("OIDC_PROVIDER_NAME", "Okta")])).unwrap();
        assert_eq!(c.mode, AuthMode::Oidc);
        assert!(!c.password_enabled());
        let o = c.oidc.unwrap();
        assert_eq!((o.scopes.as_str(), o.provider_name.as_str(), o.client_secret), ("openid email profile", "Okta", None));
        let c = AuthConfig::from_vars(vars(&[("AUTH_MODE", "both"), ("OIDC_ISSUER", "http://127.0.0.1:9"), ("OIDC_CLIENT_ID", "x"), ("OIDC_CLIENT_SECRET", "hunter2-secret")])).unwrap();
        assert!(c.password_enabled());
        assert!(!format!("{c:?}").contains("hunter2-secret"), "the secret is redacted in Debug");
    }

    #[test]
    fn pkce_matches_the_rfc_example() {
        assert_eq!(pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn only_loopback_may_be_plain_http() {
        for ok in ["https://idp.example", "http://localhost:8040", "http://127.0.0.1:1", "http://[::1]:1"] {
            assert!(endpoint_ok(ok), "{ok}");
        }
        for bad in ["http://idp.example", "ftp://x", "https://u:p@idp.example", "javascript:alert(1)", "https://x#frag"] {
            assert!(!endpoint_ok(bad), "{bad}");
        }
    }
}
