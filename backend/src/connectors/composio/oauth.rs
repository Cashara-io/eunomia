//! MCP OAuth discovery and public-client registration, with PKCE and resource binding.
//! Endpoints are constrained to the consumer service's documented hosts. Redirects are disabled.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use url::Url;
use zeroize::Zeroizing;

use crate::error::{AppError, AppResult};

pub const RESOURCE: &str = "https://connect.composio.dev/mcp";
const ISSUER: &str = "https://connect.composio.dev";
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

#[derive(Clone, Deserialize, Serialize)]
pub struct Metadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: String,
    #[serde(default)]
    pub revocation_endpoint: Option<String>,
    pub code_challenge_methods_supported: Vec<String>,
    pub token_endpoint_auth_methods_supported: Vec<String>,
}

#[derive(Deserialize)]
struct ResourceMetadata {
    resource: String,
    authorization_servers: Vec<String>,
}

// Secret-bearing types deliberately do not implement Debug.
#[derive(Serialize, Deserialize)]
pub struct Pending {
    pub metadata: Metadata,
    pub client_id: String,
    pub redirect_uri: String,
    pub state: String,
    pub verifier: String,
    pub expires_at: i64,
}

#[derive(Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    pub expires_in: u64,
    pub token_type: String,
}

pub struct OAuthClient {
    http: reqwest::Client,
    issuer: Url,
    resource: String,
    test_origin: bool,
}

fn failure() -> AppError {
    // Never include a provider response, a URL, a reqwest error or an OAuth code.
    AppError::bad_request("Composio sign-in failed. Please reconnect Composio.")
}

fn random_secret() -> String {
    let mut bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

impl OAuthClient {
    pub fn consumer() -> AppResult<Self> {
        Self::new(ISSUER, RESOURCE, false)
    }

    fn new(issuer: &str, resource: &str, test_origin: bool) -> AppResult<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|_| failure())?,
            issuer: Url::parse(issuer).map_err(|_| failure())?,
            resource: resource.to_string(),
            test_origin,
        })
    }

    fn endpoint(&self, raw: &str) -> AppResult<Url> {
        let url = Url::parse(raw).map_err(|_| failure())?;
        if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() || url.query().is_some() {
            return Err(failure());
        }
        if self.test_origin {
            if url.origin() != self.issuer.origin() {
                return Err(failure());
            }
        } else if url.scheme() != "https"
            || url.port_or_known_default() != Some(443)
            || !matches!(url.host_str(), Some("connect.composio.dev" | "login.composio.dev"))
        {
            return Err(failure());
        }
        Ok(url)
    }

    async fn json<T: serde::de::DeserializeOwned>(&self, request: reqwest::RequestBuilder) -> AppResult<T> {
        let mut response = request.send().await.map_err(|_| failure())?;
        if !response.status().is_success() || response.content_length().is_some_and(|n| n > MAX_RESPONSE_BYTES as u64) {
            return Err(failure());
        }
        let mut body = Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| failure())? {
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(failure());
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| failure())
    }

    pub async fn discover(&self) -> AppResult<Metadata> {
        let resource: ResourceMetadata = self.json(self.http.get(self.issuer.join("/.well-known/oauth-protected-resource/mcp").map_err(|_| failure())?)).await?;
        if resource.resource != self.resource || !resource.authorization_servers.contains(&self.issuer.as_str().trim_end_matches('/').to_string()) {
            return Err(failure());
        }
        let metadata: Metadata = self.json(self.http.get(self.issuer.join("/.well-known/oauth-authorization-server").map_err(|_| failure())?)).await?;
        self.validate_metadata(&metadata)?;
        Ok(metadata)
    }

    fn validate_metadata(&self, metadata: &Metadata) -> AppResult<()> {
        if metadata.issuer.trim_end_matches('/') != self.issuer.as_str().trim_end_matches('/')
            || !metadata.code_challenge_methods_supported.iter().any(|s| s == "S256")
            || !metadata.token_endpoint_auth_methods_supported.iter().any(|s| s == "none")
        {
            return Err(failure());
        }
        self.endpoint(&metadata.authorization_endpoint)?;
        self.endpoint(&metadata.token_endpoint)?;
        self.endpoint(&metadata.registration_endpoint)?;
        if let Some(endpoint) = &metadata.revocation_endpoint {
            self.endpoint(endpoint)?;
        }
        Ok(())
    }

    /// Register only Eunomia's callback, derived from its configured public origin by the caller.
    pub async fn begin(&self, redirect_uri: &str) -> AppResult<(Pending, String)> {
        let redirect = Url::parse(redirect_uri).map_err(|_| failure())?;
        if !redirect.username().is_empty() || redirect.password().is_some() || redirect.fragment().is_some() || redirect.query().is_some()
            || !(redirect.scheme() == "https" || (redirect.scheme() == "http" && matches!(redirect.host_str(), Some("localhost" | "127.0.0.1"))))
        {
            return Err(failure());
        }
        let metadata = self.discover().await?;
        #[derive(Deserialize)]
        struct Registration { client_id: String }
        let registration: Registration = self.json(self.http.post(self.endpoint(&metadata.registration_endpoint)?).json(&json!({
            "client_name": "Eunomia",
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none"
        }))).await?;
        if registration.client_id.is_empty() {
            return Err(failure());
        }
        let pending = Pending {
            metadata, client_id: registration.client_id, redirect_uri: redirect_uri.to_string(),
            state: random_secret(), verifier: random_secret(), expires_at: chrono::Utc::now().timestamp() + 600,
        };
        let mut url = self.endpoint(&pending.metadata.authorization_endpoint)?;
        url.query_pairs_mut().extend_pairs([
            ("response_type", "code"), ("client_id", pending.client_id.as_str()),
            ("redirect_uri", redirect_uri), ("state", pending.state.as_str()),
            ("code_challenge", challenge(&pending.verifier).as_str()), ("code_challenge_method", "S256"),
            ("resource", self.resource.as_str()), ("scope", "openid profile email offline_access"),
        ]);
        Ok((pending, url.to_string()))
    }

    pub async fn exchange(&self, pending: &Pending, state: &str, code: &str) -> AppResult<Tokens> {
        // Callers must consume persisted pending state atomically before calling this function.
        if pending.expires_at <= chrono::Utc::now().timestamp() || pending.state != state || code.is_empty() || state.is_empty() {
            return Err(failure());
        }
        self.validate_metadata(&pending.metadata)?;
        self.tokens(&pending.metadata, &[
            ("grant_type", "authorization_code"), ("code", code), ("client_id", &pending.client_id),
            ("redirect_uri", &pending.redirect_uri), ("code_verifier", &pending.verifier), ("resource", &self.resource),
        ]).await
    }

    pub async fn refresh(&self, metadata: &Metadata, client_id: &str, refresh_token: &str) -> AppResult<Tokens> {
        self.validate_metadata(metadata)?;
        self.tokens(metadata, &[("grant_type", "refresh_token"), ("client_id", client_id), ("refresh_token", refresh_token), ("resource", &self.resource)]).await
    }

    async fn tokens(&self, metadata: &Metadata, form: &[(&str, &str)]) -> AppResult<Tokens> {
        let tokens: Tokens = self.json(self.http.post(self.endpoint(&metadata.token_endpoint)?).form(form)).await?;
        if !tokens.token_type.eq_ignore_ascii_case("bearer") || tokens.access_token.is_empty() || tokens.expires_in == 0 {
            return Err(failure());
        }
        Ok(tokens)
    }

    /// No undocumented revocation endpoint is guessed when metadata omits RFC 7009 support.
    pub async fn revoke(&self, metadata: &Metadata, client_id: &str, token: &str) -> AppResult<bool> {
        self.validate_metadata(metadata)?;
        let Some(endpoint) = &metadata.revocation_endpoint else { return Ok(false) };
        let response = self.http.post(self.endpoint(endpoint)?).form(&[("token", token), ("client_id", client_id)]).send().await.map_err(|_| failure())?;
        if !response.status().is_success() { return Err(failure()); }
        Ok(true)
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.verifier.zeroize();
        self.state.zeroize();
    }
}

impl Drop for Tokens {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.access_token.zeroize();
        if let Some(token) = &mut self.refresh_token { token.zeroize(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::mock::{route, serve};

    fn metadata() -> serde_json::Value {
        json!({
            "issuer": "{base}", "authorization_endpoint": "{base}/authorize",
            "registration_endpoint": "{base}/register", "token_endpoint": "{base}/token",
            "revocation_endpoint": "{base}/revoke",
            "code_challenge_methods_supported": ["S256"],
            "token_endpoint_auth_methods_supported": ["none"]
        })
    }

    #[test]
    fn pkce_matches_rfc7636_example() {
        assert_eq!(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[tokio::test]
    async fn discover_register_exchange_refresh_and_revoke() {
        let mock = serve(vec![
            route("GET", "/.well-known/oauth-protected-resource/mcp", json!({"resource":"{base}/mcp", "authorization_servers":["{base}"]})),
            route("GET", "/.well-known/oauth-authorization-server", metadata()),
            route("POST", "/register", json!({"client_id":"public-client"})),
            route("POST", "/token", json!({"access_token":"canary-access", "refresh_token":"canary-refresh", "expires_in":3600, "token_type":"Bearer"})),
            route("POST", "/revoke", json!({})),
        ]).await;
        let client = OAuthClient::new(&mock.base, &format!("{}/mcp", mock.base), true).unwrap();
        let (pending, link) = client.begin("http://localhost:3000/connectors/composio/callback").await.unwrap();
        let link = Url::parse(&link).unwrap();
        let pairs: std::collections::HashMap<_, _> = link.query_pairs().into_owned().collect();
        assert_eq!(pairs["code_challenge"], challenge(&pending.verifier));
        assert_eq!(pairs["code_challenge_method"], "S256");
        assert_eq!(pairs["resource"], format!("{}/mcp", mock.base));
        assert_eq!(pairs["state"], pending.state);
        assert!(!link.as_str().contains(&pending.verifier));
        let registration = &mock.requests()[2];
        let body: serde_json::Value = serde_json::from_str(&registration.body).unwrap();
        assert_eq!(body["token_endpoint_auth_method"], "none");
        assert_eq!(body["redirect_uris"], json!([pending.redirect_uri]));
        let tokens = client.exchange(&pending, &pending.state, "one-use-code").await.unwrap();
        assert_eq!(tokens.access_token, "canary-access");
        let request = mock.requests().last().unwrap().body.clone();
        assert!(request.contains(&format!("code_verifier={}", pending.verifier)));
        assert!(request.contains("resource="));
        let refreshed = client.refresh(&pending.metadata, &pending.client_id, tokens.refresh_token.as_deref().unwrap()).await.unwrap();
        assert_eq!(refreshed.access_token, "canary-access");
        assert!(mock.requests().last().unwrap().body.contains("grant_type=refresh_token"));
        assert!(client.revoke(&pending.metadata, &pending.client_id, "canary-refresh").await.unwrap());
        assert_eq!(mock.requests().last().unwrap().path, "/revoke");
    }

    #[tokio::test]
    async fn rejects_state_mismatch_expiry_and_provider_errors_without_echoing_secrets() {
        let mock = serve(vec![
            route("GET", "/.well-known/oauth-protected-resource/mcp", json!({"resource":"{base}/mcp", "authorization_servers":["{base}"]})),
            route("GET", "/.well-known/oauth-authorization-server", metadata()),
            route("POST", "/register", json!({"client_id":"c"})),
            route("POST", "/token", json!({"error":"canary-secret-must-never-escape"})).status(400),
        ]).await;
        let client = OAuthClient::new(&mock.base, &format!("{}/mcp", mock.base), true).unwrap();
        let (mut pending, _) = client.begin("http://localhost:3000/callback").await.unwrap();
        let before = mock.requests().len();
        assert!(client.exchange(&pending, "wrong-state", "canary-code").await.is_err());
        assert_eq!(mock.requests().len(), before);
        let error = client.exchange(&pending, &pending.state, "canary-code").await.err().unwrap();
        assert!(!error.message.contains("canary"));
        assert!(error.source.is_none());
        pending.expires_at = 0;
        let before = mock.requests().len();
        assert!(client.exchange(&pending, &pending.state, "code").await.is_err());
        assert_eq!(mock.requests().len(), before);
    }

    #[test]
    fn refuses_untrusted_endpoints_and_credentials_in_urls() {
        let client = OAuthClient::consumer().unwrap();
        for endpoint in ["http://connect.composio.dev/token", "https://evil.example/token", "https://connect.composio.dev.evil.example/token", "https://user:secret@login.composio.dev/token", "https://login.composio.dev:444/token", "https://login.composio.dev/token?key=secret"] {
            assert!(client.endpoint(endpoint).is_err());
        }
        assert!(client.endpoint("https://login.composio.dev/oauth2/token").is_ok());
    }

    #[tokio::test]
    async fn refuses_issuer_substitution_before_registration() {
        let mock = serve(vec![
            route("GET", "/.well-known/oauth-protected-resource/mcp", json!({"resource":"{base}/mcp", "authorization_servers":["https://evil.example"]})),
        ]).await;
        let client = OAuthClient::new(&mock.base, &format!("{}/mcp", mock.base), true).unwrap();
        assert!(client.begin("http://localhost:3000/callback").await.is_err());
        assert_eq!(mock.requests().len(), 1);
    }
}
