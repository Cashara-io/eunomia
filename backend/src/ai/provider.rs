//! Where a model request goes and which key it carries (`ai/mod.rs` decides *whether* it may be made at all):
//! the one resolver behind settings' model list, embeddings, chat, reflect, entity extraction, observation
//! consolidation and the ontology helpers.
//!
//! Only `crate::ai` can make a request with a [`Provider`]: `client`, `url`, `bearer`, `resolve`, `server` and
//! `chat_model` are visible to nothing else, so a call site cannot reach a model without the decision point.
//!
//! Credentials are bound to a destination:
//!   - the server-wide `OPENAI_API_KEY` is only ever sent to the
//!     server-configured `OPENAI_BASE_URL`;
//!   - a user's own key goes to the base URL they configured;
//!   - a user-chosen base URL without a key of their own gets no key.
//!
//! Every call goes through [`Provider::client`] (`llm_net::client`), which
//! applies the address rules and never follows redirects, so a 3xx can't
//! forward the `Authorization` header to a host this policy didn't pick.

use serde::Deserialize;
use surrealdb::types::{RecordId, SurrealValue};

use crate::config::Settings;
use crate::connectors::crypto;
use crate::error::AppResult;
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::store;

pub const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

#[derive(Clone, PartialEq)]
pub struct Provider {
    pub base_url: String,
    pub api_key: String,
    /// The user's `embedding_model` setting ("" = default).
    pub embedding_model: String,
    /// The user's `chat_model` setting, else OPENAI_CHAT_MODEL ("" = auto, see [`chat_model`]).
    pub chat_model: String,
}

// never the key
impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider")
            .field("base_url", &self.base_url)
            .field("api_key", &if self.api_key.is_empty() { "" } else { "<set>" })
            .field("embedding_model", &self.embedding_model)
            .field("chat_model", &self.chat_model)
            .finish()
    }
}

impl Provider {
    pub fn new(base_url: String, api_key: String, embedding_model: String, chat_model: String) -> Self {
        Provider { base_url, api_key, embedding_model, chat_model }
    }

    /// Enough to make a real call: a key, or a non-OpenAI base URL (a
    /// self-hosted server may not need one).
    pub fn configured(&self) -> bool {
        !self.api_key.is_empty() || !same_url(&self.base_url, OPENAI_BASE_URL)
    }

    pub(in crate::ai) fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.base_url.trim_end_matches('/'))
    }

    pub(in crate::ai) fn bearer(&self) -> &str {
        if self.api_key.is_empty() { "not-needed" } else { &self.api_key }
    }

    /// The guarded HTTP client for this provider's base URL.
    pub(in crate::ai) async fn client(&self) -> AppResult<reqwest::Client> {
        crate::llm_net::client(&self.base_url).await
    }
}

/// Same endpoint, ignoring a trailing slash and host case.
pub fn same_url(a: &str, b: &str) -> bool {
    match (url::Url::parse(a.trim_end_matches('/')), url::Url::parse(b.trim_end_matches('/'))) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The base URL a user's settings point at ("" = the server's).
pub fn effective_base_url<'a>(server_base: &'a str, user_base: &'a str) -> &'a str {
    if user_base.trim().is_empty() { server_base } else { user_base.trim() }
}

/// `(base_url, api_key)`: the credential-binding rule from the module docs.
pub(in crate::ai) fn pick(server_base: &str, server_key: Option<&str>, user_base: &str, user_key: &str) -> (String, String) {
    let base = effective_base_url(server_base, user_base).to_string();
    if !user_key.is_empty() {
        (base, user_key.to_string())
    } else if same_url(&base, server_base) {
        (server_base.to_string(), server_key.unwrap_or_default().to_string())
    } else {
        (base, String::new())
    }
}

#[derive(Debug, Default, Deserialize, SurrealValue)]
struct Row {
    #[serde(default)]
    #[surreal(default)]
    openai_base_url: String,
    #[serde(default)]
    #[surreal(default)]
    openai_api_key_encrypted: String,
    #[serde(default)]
    #[surreal(default)]
    embedding_model: String,
    #[serde(default)]
    #[surreal(default)]
    chat_model: String,
}

/// A person's own provider settings, and whether they chose one.
pub(in crate::ai) struct UserProvider {
    pub provider: Provider,
    /// The person set a key of their own, or an endpoint other than the server's: calls go to what they chose
    /// (and at their cost), never on the server's key.
    pub own: bool,
}

/// `owner`'s provider. A stored key that can't be decrypted is an error,
/// never sent anywhere.
pub(in crate::ai) async fn resolve(db: &OrgDb, settings: &Settings, owner: &RecordId) -> AppResult<UserProvider> {
    let row: Option<Row> = store::get(db, &RecordId::from_table_key("app_settings", owner.key().clone())).await?;
    let row = row.unwrap_or_default();
    let user_key = crypto::decrypt_or_plaintext(&settings.encryption_key, &row.openai_api_key_encrypted)?;
    let (base_url, api_key) =
        pick(&settings.openai_base_url, settings.openai_api_key.as_deref(), &row.openai_base_url, &user_key);
    let own = !user_key.is_empty() || !same_url(&base_url, &settings.openai_base_url);
    let chat_model = if row.chat_model.trim().is_empty() { settings.openai_chat_model.clone() } else { row.chat_model };
    Ok(UserProvider { provider: Provider { base_url, api_key, embedding_model: row.embedding_model, chat_model }, own })
}

pub const DEFAULT_CHAT_MODEL: &str = "gpt-4o-mini";

/// The chat model to ask `p` for: the configured one; else `gpt-4o-mini` on api.openai.com; else the
/// first chat model the endpoint itself lists, so a self-hosted server (Ollama, vLLM, LM Studio, ...)
/// uses what it has. Listings are cached per endpoint for 10 minutes.
pub(in crate::ai) async fn chat_model(p: &Provider) -> String {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, String)>>> = OnceLock::new();

    if !p.chat_model.trim().is_empty() {
        return p.chat_model.trim().to_string();
    }
    if same_url(&p.base_url, OPENAI_BASE_URL) {
        return DEFAULT_CHAT_MODEL.to_string();
    }
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((at, model)) = cache.lock().unwrap().get(&p.base_url)
        && at.elapsed() < Duration::from_secs(600)
    {
        return model.clone();
    }
    let listed = async {
        let v: serde_json::Value = p
            .client()
            .await
            .ok()?
            .get(p.url("models"))
            .bearer_auth(p.bearer())
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .json()
            .await
            .ok()?;
        let ids: Vec<String> = v.get("data")?.as_array()?.iter().filter_map(|m| m.get("id")?.as_str().map(String::from)).collect();
        pick_chat_model(&ids)
    }
    .await;
    let model = listed.unwrap_or_else(|| DEFAULT_CHAT_MODEL.to_string());
    cache.lock().unwrap().insert(p.base_url.clone(), (Instant::now(), model.clone()));
    model
}

/// First model id that isn't an embedding/speech/image/moderation model.
pub fn pick_chat_model(ids: &[String]) -> Option<String> {
    const NOT_CHAT: &[&str] = &["embed", "whisper", "tts", "rerank", "moderation", "dall-e", "image", "transcribe"];
    ids.iter().find(|id| { let l = id.to_lowercase(); !NOT_CHAT.iter().any(|w| l.contains(w)) }).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_first_chat_capable_model() {
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(pick_chat_model(&ids(&["nomic-embed-text", "whisper-1", "llama3.1:8b", "qwen2.5"])).as_deref(), Some("llama3.1:8b"));
        assert_eq!(pick_chat_model(&ids(&["text-embedding-3-small"])), None);
        assert_eq!(pick_chat_model(&[]), None);
    }

    #[tokio::test]
    async fn chat_model_prefers_config_then_openai_default() {
        let mk = |base: &str, model: &str| Provider { base_url: base.into(), api_key: String::new(), embedding_model: String::new(), chat_model: model.into() };
        assert_eq!(chat_model(&mk("http://localhost:1/v1", "my-model")).await, "my-model");
        assert_eq!(chat_model(&mk(OPENAI_BASE_URL, "")).await, DEFAULT_CHAT_MODEL);
        // unreachable self-hosted endpoint: falls back rather than failing
        assert_eq!(chat_model(&mk("http://127.0.0.1:9/v1", "")).await, DEFAULT_CHAT_MODEL);
    }

    const SERVER: &str = "https://api.openai.com/v1";
    const SERVER_KEY: Option<&str> = Some("sk-server");

    #[test]
    fn a_custom_url_without_a_user_key_never_gets_the_server_key() {
        let (base, key) = pick(SERVER, SERVER_KEY, "https://evil.example/v1", "");
        assert_eq!(base, "https://evil.example/v1");
        assert_eq!(key, "");
    }

    #[test]
    fn the_server_key_goes_only_to_the_server_url() {
        assert_eq!(pick(SERVER, SERVER_KEY, "", ""), (SERVER.to_string(), "sk-server".to_string()));
        assert_eq!(pick(SERVER, SERVER_KEY, "https://API.openai.com/v1/", "").1, "sk-server");
        // a lookalike path or host is a different destination
        assert_eq!(pick(SERVER, SERVER_KEY, "https://api.openai.com/v1/../evil", "").1, "");
        assert_eq!(pick(SERVER, SERVER_KEY, "https://api.openai.com.evil.example/v1", "").1, "");
        assert_eq!(pick(SERVER, SERVER_KEY, "http://api.openai.com/v1", "").1, "");
    }

    #[test]
    fn a_custom_server_url_is_honoured() {
        let (base, key) = pick("http://llm.internal:8080/v1", SERVER_KEY, "", "");
        assert_eq!((base.as_str(), key.as_str()), ("http://llm.internal:8080/v1", "sk-server"));
    }

    #[test]
    fn a_user_key_goes_to_the_users_own_url() {
        assert_eq!(
            pick(SERVER, SERVER_KEY, "http://localhost:11434/v1", "sk-user"),
            ("http://localhost:11434/v1".to_string(), "sk-user".to_string())
        );
        assert_eq!(pick(SERVER, SERVER_KEY, "", "sk-user"), (SERVER.to_string(), "sk-user".to_string()));
    }

    #[test]
    fn configured_needs_a_key_only_for_openai() {
        let p = |base: &str, key: &str| Provider { base_url: base.into(), api_key: key.into(), embedding_model: String::new(), chat_model: String::new() };
        assert!(!p(OPENAI_BASE_URL, "").configured());
        assert!(p(OPENAI_BASE_URL, "sk-abc").configured());
        assert!(p("http://localhost:11434/v1", "").configured());
    }
}
