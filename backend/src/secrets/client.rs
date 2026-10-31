//! The backend's client for the `secrets-helper` sidecar's internal API, version 1
//! (docs/architecture/specs/broker.md, "Sidecar internal API"). The sidecar is reached over the internal
//! compose network (`SECRETS_HELPER_URL`, default `http://secrets-helper:8080`) with the shared key
//! (`SECRETS_HELPER_KEY`, when set). The provider token goes in each request body and nowhere else: not a
//! header, not a URL, not a log line. Short timeouts: a slow provider fails the call, it never hangs it.

use std::sync::LazyLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub const DEFAULT_URL: &str = "http://secrets-helper:8080";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const TIMEOUT: Duration = Duration::from_secs(12);

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TIMEOUT)
        // the sidecar never redirects; a redirect would be someone else answering
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("secrets-helper http client")
});

/// What the sidecar needs to reach one provider. Debug never prints the token.
#[derive(Clone, Serialize)]
pub struct ProviderConfig {
    pub kind: String,
    #[serde(serialize_with = "plain")]
    pub token: Zeroizing<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

fn plain<S: serde::Serializer>(t: &Zeroizing<String>, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(t.as_str())
}

impl std::fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfig").field("kind", &self.kind).field("host", &self.host).field("token", &"[token]").finish()
    }
}

/// One reference's answer. `value` is zeroised when dropped.
#[derive(Deserialize)]
pub struct Resolved {
    #[serde(rename = "ref")]
    pub reference: String,
    pub ok: bool,
    #[serde(default)]
    pub value: Option<Zeroizing<String>>,
    /// `not_found`, `denied`, `invalid_ref` or `provider_error` when not ok.
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

impl std::fmt::Debug for Resolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resolved").field("ref", &self.reference).field("ok", &self.ok).field("error", &self.error).field("message", &self.message).finish()
    }
}

#[derive(Deserialize)]
struct ResolveOut {
    results: Vec<Resolved>,
}

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema)]
pub struct CheckOut {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// `denied`, `provider_error`, `bad_config` or `unreachable` (the sidecar itself) when not ok.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// The sidecar could not be asked (down, slow, refused the key, answered nonsense). Never holds a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreachable(pub String);

#[derive(Clone)]
pub struct Client {
    url: String,
    key: Zeroizing<String>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").field("url", &self.url).finish_non_exhaustive()
    }
}

impl Client {
    pub fn new(url: &str, key: &str) -> Self {
        Client { url: url.trim_end_matches('/').to_string(), key: Zeroizing::new(key.to_string()) }
    }

    pub fn from_settings(s: &crate::config::Settings) -> Self {
        Client::new(&s.secrets_helper_url, &s.secrets_helper_key)
    }

    async fn post<B: Serialize, T: for<'de> Deserialize<'de>>(&self, path: &str, body: &B) -> Result<T, Unreachable> {
        let mut req = HTTP.post(format!("{}{path}", self.url)).json(body);
        if !self.key.is_empty() {
            req = req.bearer_auth(self.key.as_str());
        }
        // reqwest's error text names the URL, never the body; still, keep only its kind
        let res = req.send().await.map_err(|e| Unreachable(if e.is_timeout() { "the secrets helper timed out".into() } else { "the secrets helper is not reachable".into() }))?;
        let status = res.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Unreachable("the secrets helper refused the internal key (SECRETS_HELPER_KEY differs)".into()));
        }
        if !status.is_success() {
            return Err(Unreachable(format!("the secrets helper answered HTTP {}", status.as_u16())));
        }
        res.json::<T>().await.map_err(|_| Unreachable("the secrets helper sent an unexpected answer".into()))
    }

    /// `POST /v1/resolve`: one answer per reference, in order.
    pub async fn resolve(&self, provider: &ProviderConfig, refs: &[String]) -> Result<Vec<Resolved>, Unreachable> {
        #[derive(Serialize)]
        struct Body<'a> {
            provider: &'a ProviderConfig,
            refs: &'a [String],
        }
        let out: ResolveOut = self.post("/v1/resolve", &Body { provider, refs }).await?;
        if out.results.len() != refs.len() || out.results.iter().zip(refs).any(|(r, want)| &r.reference != want) {
            return Err(Unreachable("the secrets helper answered for other references".into()));
        }
        Ok(out.results)
    }

    /// `POST /v1/check`: "Test connection".
    pub async fn check(&self, provider: &ProviderConfig) -> CheckOut {
        #[derive(Serialize)]
        struct Body<'a> {
            provider: &'a ProviderConfig,
        }
        match self.post::<_, CheckOut>("/v1/check", &Body { provider }).await {
            Ok(c) => c,
            Err(Unreachable(m)) => CheckOut { ok: false, detail: None, error: Some("unreachable".into()), message: Some(m) },
        }
    }
}
