//! Embedding service: one interface, swappable backend.
//!
//! `embed(texts) -> Vec<Vec<f32>>` -- always [`DIM`]-dimensional,
//! order-preserving.
//!
//! Backend is selected by `settings.embeddings_backend` (env-only, never a
//! DB-backed setting -- see `config.rs`):
//!   - `"openai"` -- an OpenAI-compatible embeddings API (the owner's
//!     provider, see `embeddings::provider`), with the owner's selected
//!     `embedding_model`, batched.
//!   - `"stub"`   -- deterministic SHA256-derived vector, zero network, for
//!     tests.
//!
//! The vector index is fixed at [`DIM`] dimensions: a model that returns
//! anything else is rejected, never stored.
//!
//! Results are memoized in the `embed_cache` SurrealDB table, keyed by
//! HMAC(ENCRYPTION_KEY, spec + text), where the [`EmbedSpec`] is the
//! provider origin, model and dimension -- so two providers never share a
//! vector for the same text.

use surrealdb::types::SurrealValue;
use std::collections::HashMap;

use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use surrealdb::types::RecordId;

use super::provider::{self, Provider};
use crate::config::Settings;
use crate::error::{AppError, AppResult};
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::store;

pub const DIM: usize = 1536;
const BATCH: usize = 64;
pub const DEFAULT_MODEL: &str = "text-embedding-3-small";
/// api.openai.com models that return [`DIM`]-dimension vectors.
const OPENAI_MODELS: &[&str] = &["text-embedding-3-small", "text-embedding-ada-002"];

type HmacSha256 = Hmac<Sha256>;

/// HMAC-SHA256(key, s), hex-encoded.
fn hmac_hex(key: &str, s: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(key.as_bytes()).expect("HMAC accepts a key of any length");
    mac.update(s.as_bytes());
    mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

/// Deterministic SHA256-derived vector: repeat the 32-byte digest to fill
/// `DIM`, then map each byte from `[0, 255]` to `[-1.0, 1.0)`. Mirrors
/// `embeddings/service.py`'s `_stub_vec`.
fn stub_vec(text: &str) -> Vec<f32> {
    let digest = Sha256::digest(text.as_bytes());
    let mut raw = Vec::with_capacity(DIM);
    while raw.len() < DIM {
        raw.extend_from_slice(&digest);
    }
    raw.truncate(DIM);
    raw.into_iter().map(|b| (b as f32 / 255.0) * 2.0 - 1.0).collect()
}

fn embed_stub(texts: &[String]) -> Vec<Vec<f32>> {
    texts.iter().map(|t| stub_vec(t)).collect()
}

/// What a vector was made with. Identical text only shares a cached vector
/// under the same spec; the dimension is always [`DIM`].
#[derive(Debug, Clone, PartialEq)]
pub struct EmbedSpec {
    pub origin: String,
    pub model: String,
}

impl EmbedSpec {
    fn cache_key(&self, secret: &str, text: &str) -> String {
        hmac_hex(secret, &format!("{}\n{}\n{DIM}\n{text}", self.origin, self.model))
    }
}

/// The model `p`'s owner selected ("" = [`DEFAULT_MODEL`]).
pub fn model_for(p: &Provider) -> &str {
    let m = p.embedding_model.trim();
    if m.is_empty() { DEFAULT_MODEL } else { m }
}

/// On api.openai.com only [`DIM`]-dimension models are accepted. Other
/// endpoints' models can't be known up front; a wrong dimension is
/// rejected when the response arrives.
pub fn check_model(base_url: &str, model: &str) -> Result<(), String> {
    if provider::same_url(base_url, provider::OPENAI_BASE_URL) && !OPENAI_MODELS.contains(&model) {
        return Err(format!(
            "embedding model `{model}` isn't supported: Eunomia's vector index is {DIM}-dimensional, \
             so on api.openai.com use {}",
            OPENAI_MODELS.join(" or ")
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct EmbeddingDatum {
    index: usize,
    embedding: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingsResponse {
    #[serde(default)]
    data: Vec<EmbeddingDatum>,
}

/// One vector per input, in input order: exactly `n` items, every index in
/// range and unique, every vector [`DIM`]-long with finite components.
fn order_response(data: Vec<EmbeddingDatum>, n: usize) -> Result<Vec<Vec<f32>>, String> {
    if data.len() != n {
        return Err(format!("embeddings response has {} vectors for {n} inputs", data.len()));
    }
    let mut slots: Vec<Option<Vec<f32>>> = vec![None; n];
    for d in data {
        let slot = slots.get_mut(d.index).ok_or_else(|| format!("embeddings response index {} out of range", d.index))?;
        if slot.is_some() {
            return Err(format!("embeddings response repeats index {}", d.index));
        }
        if d.embedding.len() != DIM {
            return Err(format!(
                "embedding model returned {}-dimension vectors; Eunomia's index needs {DIM} -- choose a {DIM}-dimension model in Settings",
                d.embedding.len()
            ));
        }
        if !d.embedding.iter().all(|x| x.is_finite()) {
            return Err("embeddings response contains non-finite values".to_string());
        }
        *slot = Some(d.embedding);
    }
    Ok(slots.into_iter().flatten().collect())
}

/// Calls `{base_url}/embeddings`, batched `BATCH` at a time. Every batch is
/// validated before anything is returned.
async fn embed_openai(texts: &[String], p: &Provider, model: &str) -> AppResult<Vec<Vec<f32>>> {
    let client = p.client().await?;
    let url = p.url("embeddings");

    let mut out = Vec::with_capacity(texts.len());
    for chunk in texts.chunks(BATCH) {
        let resp = client
            .post(&url)
            .bearer_auth(p.bearer())
            .json(&json!({ "model": model, "input": chunk }))
            .send()
            .await
            .map_err(|e| AppError::internal(format!("OpenAI embeddings request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::internal(format!("OpenAI embeddings HTTP {status}: {body}")));
        }

        let parsed: EmbeddingsResponse = resp
            .json()
            .await
            .map_err(|e| AppError::internal(format!("invalid OpenAI embeddings response: {e}")))?;
        out.extend(order_response(parsed.data, chunk.len()).map_err(AppError::internal)?);
    }
    Ok(out)
}

/// What `owner`'s vectors are made with now: the provider's origin and the model (`stub` for the
/// hermetic test backend). For export provenance; stored vectors do not record it yet.
pub async fn spec_for(db: &OrgDb, settings: &Settings, owner: &RecordId) -> AppResult<EmbedSpec> {
    if settings.embeddings_backend != "openai" {
        return Ok(EmbedSpec { origin: "stub".to_string(), model: "stub".to_string() });
    }
    let p = provider::resolve(db, settings, owner).await?;
    Ok(EmbedSpec { origin: p.base_url.trim_end_matches('/').to_string(), model: model_for(&p).to_string() })
}

pub fn dim() -> usize {
    DIM
}

impl EmbedSpec {
    /// The `memory.embedding_spec` a vector made under this spec carries: a query is only compared
    /// with vectors from the same provider and model.
    pub fn tag(&self) -> String {
        format!("{}|{}|{DIM}", self.origin, self.model)
    }
}

/// Embed up to `limit` of `owner`'s memories that have no vector from the current model (never
/// embedded, edited since, or embedded by another model). Returns how many were written. A memory
/// edited while this ran keeps no vector (`SET_MEMORY_EMBEDDING` checks the text) and is picked up again.
pub async fn embed_memory_backlog(db: &OrgDb, settings: &Settings, owner: &RecordId, limit: usize) -> AppResult<usize> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        id: RecordId,
        text: String,
    }
    let spec = spec_for(db, settings, owner).await?.tag();
    let mut res = store::jobs::MEMORY_EMBED_BACKLOG.on(db).bind(("owner", owner.clone())).bind(("spec", spec.clone())).bind(("limit", limit as i64)).await?;
    let rows: Vec<Row> = res.take(0)?;
    if rows.is_empty() {
        return Ok(0);
    }
    let texts: Vec<String> = rows.iter().map(|r| r.text.clone()).collect();
    let vectors = embed(db, settings, &texts, Some(owner)).await?;
    for (row, vector) in rows.iter().zip(vectors) {
        if vector.len() != DIM {
            return Err(AppError::internal(format!("embedding for {} has dimension {}", row.id.to_string(), vector.len())));
        }
        store::cache::SET_MEMORY_EMBEDDING
            .on(db)
            .bind(("id", row.id.clone()))
            .bind(("embedding", vector))
            .bind(("spec", spec.clone()))
            .bind(("text", row.text.clone()))
            .await?
            .check()?;
    }
    Ok(rows.len())
}

/// Whether embeddings can run for `owner` right now. Without them the
/// semantic arm of search/recall is skipped and the connected MCP agent is
/// the model: keyword + graph + temporal retrieval still work, and the agent
/// does any synthesis itself.
pub async fn available(db: &OrgDb, settings: &Settings, owner: &RecordId) -> bool {
    if settings.embeddings_backend != "openai" {
        return true; // "stub": hermetic tests
    }
    match provider::resolve(db, settings, owner).await {
        Ok(p) => p.configured(),
        Err(e) => {
            tracing::warn!("model provider unavailable for {}: {}", owner.to_string(), e.message);
            false
        }
    }
}

/// Whether the server can run its own chat completions for `owner`
/// (reflect / observation consolidation); if not, the MCP agent does it.
pub async fn chat_available(db: &OrgDb, settings: &Settings, owner: &RecordId) -> bool {
    settings.embeddings_backend == "openai" && available(db, settings, owner).await
}

#[derive(Debug, Deserialize, SurrealValue)]
struct EmbedCacheRow {
    text_hmac: String,
    vector: Vec<f32>,
}

/// Embed `texts`, using the SurrealDB-backed memo (`embed_cache`) for ones
/// seen before under the same [`EmbedSpec`]. Nothing is cached unless the
/// whole response validated.
///
/// `owner`, if given, resolves that user's provider and model; owner-less
/// call sites use the server's provider.
pub async fn embed(db: &OrgDb, settings: &Settings, texts: &[String], owner: Option<&RecordId>) -> AppResult<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }

    let (spec, prov) = if settings.embeddings_backend == "openai" {
        let p = match owner {
            Some(o) => provider::resolve(db, settings, o).await?,
            None => provider::server(settings),
        };
        let model = model_for(&p).to_string();
        check_model(&p.base_url, &model).map_err(AppError::bad_request)?;
        (EmbedSpec { origin: p.base_url.trim_end_matches('/').to_string(), model }, Some(p))
    } else {
        (EmbedSpec { origin: "stub".to_string(), model: "stub".to_string() }, None)
    };
    let keys: Vec<String> = texts.iter().map(|t| spec.cache_key(&settings.encryption_key, t)).collect();

    let mut res = store::cache::EMBED_CACHE_GET.on(db).bind(("keys", keys.clone())).await?;
    let rows: Vec<EmbedCacheRow> = res.take(0)?;
    let mut cached: HashMap<String, Vec<f32>> = rows.into_iter().map(|r| (r.text_hmac, r.vector)).collect();

    let missing_idx: Vec<usize> = keys.iter().enumerate().filter(|(_, k)| !cached.contains_key(*k)).map(|(i, _)| i).collect();
    if !missing_idx.is_empty() {
        let fresh_texts: Vec<String> = missing_idx.iter().map(|&i| texts[i].clone()).collect();
        let vecs = match &prov {
            Some(p) => embed_openai(&fresh_texts, p, &spec.model).await?,
            None => embed_stub(&fresh_texts),
        };

        for (i, v) in missing_idx.into_iter().zip(vecs) {
            let k = keys[i].clone();
            store::cache::EMBED_CACHE_PUT
                .on(db)
                .bind(("id", RecordId::from_table_key("embed_cache", k.clone())))
                .bind(("hmac", k.clone()))
                .bind(("vector", v.clone()))
                .await?;
            cached.insert(k, v);
        }
    }

    Ok(keys.iter().map(|k| cached.get(k).cloned().unwrap_or_default()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn datum(index: usize, embedding: Vec<f32>) -> EmbeddingDatum {
        EmbeddingDatum { index, embedding }
    }

    #[test]
    fn response_is_reordered_by_index() {
        let out = order_response(vec![datum(1, vec![2.0; DIM]), datum(0, vec![1.0; DIM])], 2).unwrap();
        assert_eq!(out[0][0], 1.0);
        assert_eq!(out[1][0], 2.0);
    }

    #[test]
    fn missing_duplicate_and_out_of_range_indexes_are_rejected() {
        assert!(order_response(vec![datum(0, vec![1.0; DIM])], 2).is_err(), "missing");
        assert!(order_response(vec![datum(0, vec![1.0; DIM]), datum(0, vec![1.0; DIM])], 2).is_err(), "duplicate");
        assert!(order_response(vec![datum(0, vec![1.0; DIM]), datum(2, vec![1.0; DIM])], 2).is_err(), "out of range");
        assert!(order_response((0..3).map(|i| datum(i, vec![1.0; DIM])).collect(), 2).is_err(), "too many");
    }

    #[test]
    fn wrong_dimension_and_non_finite_vectors_are_rejected() {
        let e = order_response(vec![datum(0, vec![1.0; 768])], 1).unwrap_err();
        assert!(e.contains("768"), "{e}");
        let mut v = vec![1.0; DIM];
        v[7] = f32::NAN;
        assert!(order_response(vec![datum(0, v)], 1).is_err());
        let mut v = vec![1.0; DIM];
        v[7] = f32::INFINITY;
        assert!(order_response(vec![datum(0, v)], 1).is_err());
    }

    #[test]
    fn the_cache_key_depends_on_provider_and_model() {
        let a = EmbedSpec { origin: "http://a.example/v1".into(), model: "m".into() };
        let b = EmbedSpec { origin: "http://b.example/v1".into(), model: "m".into() };
        let c = EmbedSpec { origin: "http://a.example/v1".into(), model: "other".into() };
        assert_eq!(a.cache_key("k", "same text"), a.cache_key("k", "same text"));
        assert_ne!(a.cache_key("k", "same text"), b.cache_key("k", "same text"));
        assert_ne!(a.cache_key("k", "same text"), c.cache_key("k", "same text"));
    }

    #[test]
    fn selected_model_is_used_and_defaults_when_blank() {
        let p = |m: &str| Provider { base_url: "http://x/v1".into(), api_key: String::new(), embedding_model: m.into(), chat_model: String::new() };
        assert_eq!(model_for(&p("")), DEFAULT_MODEL);
        assert_eq!(model_for(&p(" nomic-embed ")), "nomic-embed");
    }

    #[test]
    fn unsupported_openai_models_are_rejected() {
        assert!(check_model(provider::OPENAI_BASE_URL, "text-embedding-3-small").is_ok());
        assert!(check_model(provider::OPENAI_BASE_URL, "text-embedding-ada-002").is_ok());
        assert!(check_model(provider::OPENAI_BASE_URL, "text-embedding-3-large").is_err());
        assert!(check_model(provider::OPENAI_BASE_URL, "gpt-4o-mini").is_err());
        // other endpoints: checked by dimension when the vectors arrive
        assert!(check_model("http://localhost:11434/v1", "anything").is_ok());
    }

    #[test]
    fn dim_is_1536() {
        assert_eq!(dim(), 1536);
    }

    #[test]
    fn hmac_hex_is_deterministic() {
        assert_eq!(hmac_hex("key", "value"), hmac_hex("key", "value"));
    }

    #[test]
    fn hmac_hex_changes_with_key_or_input() {
        let base = hmac_hex("key", "value");
        assert_ne!(base, hmac_hex("other-key", "value"));
        assert_ne!(base, hmac_hex("key", "other-value"));
    }

    #[test]
    fn hmac_hex_is_64_hex_chars() {
        let h = hmac_hex("key", "value");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn stub_vec_has_correct_dimension() {
        assert_eq!(stub_vec("hello world").len(), DIM);
    }

    #[test]
    fn stub_vec_is_deterministic_and_in_range() {
        let a = stub_vec("same text");
        let b = stub_vec("same text");
        assert_eq!(a, b);
        assert!(a.iter().all(|v| *v >= -1.0 && v < &1.0));
    }

    #[test]
    fn stub_vec_differs_for_different_text() {
        assert_ne!(stub_vec("a"), stub_vec("b"));
    }

    #[test]
    fn embed_stub_preserves_order_and_count() {
        let texts = vec!["one".to_string(), "two".to_string(), "three".to_string()];
        let out = embed_stub(&texts);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], stub_vec("one"));
        assert_eq!(out[1], stub_vec("two"));
        assert_eq!(out[2], stub_vec("three"));
    }
}
