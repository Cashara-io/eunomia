//! Every call to a model provider goes through here. For one organisation and one person, right now: may a
//! model be called at all, and with whose key? (docs/deployment.md, "Model spend and the AI switch".)
//!
//! ```text
//! route()      who would pay, and where would it go (no network, no refusal): the user's own key, else the
//!              organisation's, else the server's
//! authorize()  route() + may it happen at all: the organisation's AI switch, a configured provider, and for the
//!              server's key the month's allowance  ->  a Grant, or a Refusal
//! Grant        the only value that can reach a provider: complete(), embeddings(), stream_chat(), list_models()
//! ```
//!
//! **Order of precedence** (first match wins): the person's own key or endpoint (`app_settings`, as before); the
//! organisation's own provider (`org_ai`, set by an owner or admin); the server's key, if the organisation's
//! allowance (set by the billing service) has room. An organisation with no allowance, no provider of its own and
//! AI left on is every organisation on a self-hosted install: it uses the server's key as before.
//!
//! **The AI switch** (`org_ai.enabled = false`, capability C31) is checked first and applies to every key, a
//! person's own included: nothing about the organisation's data is sent to any model.
//!
//! **A call site cannot skip any of this.** A [`provider::Provider`] can make no request outside this module (its
//! `client`, `url` and `bearer` are visible only here), and the only way to get one is [`Route`] / [`Grant`].
//! `tests/store.rs` (`no_model_call_outside_the_ai_module`) also fails if a source file other than this
//! module names a provider endpoint.
//!
//! **Metering** (`hub.rs`): a call on the server's key adds its tokens and cost to a counter per organisation,
//! day, purpose and model; a call on anyone's own key adds calls and tokens and no cost. Tokens are what the
//! provider reported; where it reported none they are counted here, conservatively ([`estimate_tokens`]).

pub mod hub;
pub mod orgcfg;
pub mod prices;
pub mod provider;

use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use surrealdb::types::RecordId;

use crate::rid::RecordIdExt;

use self::hub::{Counters, Hub, OrgAi, Reservation, Reserve};
use self::prices::Priced;
use self::provider::Provider;
use crate::config::Settings;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::pool::{OrgDb, OrgId};

/// What a model call is for. The counters are kept by purpose, so the operator can see what the spend is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Purpose {
    /// Facts out of what people save and sync.
    Extraction,
    /// Vectors for search and recall.
    Embedding,
    /// The in-app chat.
    Chat,
    /// `reflect`: an answer from recalled memories.
    Reflect,
    /// Rewriting an entity's observation from its facts.
    Consolidation,
    /// Marking facts a newer one makes untrue.
    Supersede,
    /// Ontology suggestions, mapping and coverage.
    Ontology,
    /// Listing a provider's models (Settings): no tokens, so never counted or limited by the allowance.
    Models,
}

impl Purpose {
    pub const ALL: [Purpose; 8] = [
        Purpose::Extraction,
        Purpose::Embedding,
        Purpose::Chat,
        Purpose::Reflect,
        Purpose::Consolidation,
        Purpose::Supersede,
        Purpose::Ontology,
        Purpose::Models,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Purpose::Extraction => "extraction",
            Purpose::Embedding => "embedding",
            Purpose::Chat => "chat",
            Purpose::Reflect => "reflect",
            Purpose::Consolidation => "consolidation",
            Purpose::Supersede => "supersede",
            Purpose::Ontology => "ontology",
            Purpose::Models => "models",
        }
    }
}

/// Whose key a call carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeySource {
    /// The server's (`OPENAI_API_KEY`): costs us money and counts against the allowance.
    Server,
    /// The organisation's own provider.
    Org,
    /// The person's own key or endpoint.
    User,
}

impl KeySource {
    pub fn as_str(self) -> &'static str {
        match self {
            KeySource::Server => "server",
            KeySource::Org => "org",
            KeySource::User => "user",
        }
    }
}

/// The most output a call may produce. Sent to api.openai.com as `max_completion_tokens` when the organisation has an
/// allowance, and the output a call's reservation assumes.
pub const MAX_OUTPUT_TOKENS: u64 = 4096;

/// Whether a call's output is capped at [`MAX_OUTPUT_TOKENS`] (`max_completion_tokens`): only on the server's key, only
/// when the organisation has an allowance to protect, and only on api.openai.com, which accepts the field (another
/// server may reject it). Everything else is sent exactly as before.
pub fn caps_output(source: KeySource, has_allowance: bool, base_url: &str) -> bool {
    source == KeySource::Server && has_allowance && provider::same_url(base_url, provider::OPENAI_BASE_URL)
}

/// Tokens in `bytes` of text, counted conservatively: one token per three bytes. English averages about four
/// bytes a token, so this overcounts typical text by a third; for Chinese, Japanese and Korean text (three bytes
/// a character, one to two tokens a character) it is close. Used where the provider reports nothing, and for
/// what a call may cost before it is made.
pub fn estimate_tokens(bytes: usize) -> u64 {
    (bytes as u64).div_ceil(3)
}

// ---------------------------------------------------------------------------------------------- refusals

/// Why a model may not be called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The organisation switched AI off (`ai.disabled`).
    Disabled,
    /// The month's allowance on the server's key is used up (`ai.allowance_used`).
    AllowanceUsed,
    /// No provider is configured for this person.
    NotConfigured,
}

impl Refusal {
    pub fn into_error(self) -> AppError {
        match self {
            Refusal::Disabled => AppError::coded(
                ErrorCode::AiDisabled,
                "AI is switched off for this workspace, so no model is used on its content. An owner or admin can switch it on in Settings > Organisation.",
            ),
            Refusal::AllowanceUsed => AppError::coded(
                ErrorCode::AiAllowanceUsed,
                "This workspace has used its AI allowance for this month. An owner can add the organisation's own provider key in Settings > Organisation to keep going now; otherwise it renews at the start of next month (UTC). Saving, reading and keyword search still work.",
            ),
            Refusal::NotConfigured => AppError::bad_request("OpenAI API key not configured -- add one in Settings"),
        }
    }

    /// The refusals a background job waits out, rather than fails on.
    pub fn is_hold(code: ErrorCode) -> bool {
        matches!(code, ErrorCode::AiDisabled | ErrorCode::AiAllowanceUsed)
    }
}

// ---------------------------------------------------------------------------------------------- the decision

/// Who would pay for a call by `user`, and where it would go. Decides nothing about whether it may happen.
pub struct Route {
    hub: Arc<Hub>,
    org: OrgId,
    source: KeySource,
    provider: Provider,
    cfg: Arc<OrgAi>,
}

/// The route for `user` in the organisation `db` belongs to.
pub async fn route(db: &OrgDb, settings: &Settings, user: &RecordId) -> AppResult<Route> {
    let hub = db.ai().clone();
    let org = db.org();
    let cfg = hub.org_config(&org).await?;
    let own = provider::resolve(db, settings, user).await?;
    let (source, provider) = if own.own {
        (KeySource::User, own.provider)
    } else if cfg.has_own_provider() {
        (KeySource::Org, orgcfg::provider_of(&cfg, settings)?)
    } else {
        (KeySource::Server, own.provider)
    };
    Ok(Route { hub, org, source, provider, cfg })
}

/// Whether a model may be called now, and if not why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Ready,
    NotConfigured,
    Refused(Refusal),
}

impl Route {
    pub fn source(&self) -> KeySource {
        self.source
    }

    /// Where its calls would go and with which models (for the embedding provenance of a vector).
    pub fn provider(&self) -> &Provider {
        &self.provider
    }

    /// May a model be called for this person now? The AI switch first; then a configured provider; then, for
    /// the server's key, an allowance with room left. Not a reservation.
    pub async fn availability(&self, purpose: Purpose) -> AppResult<Availability> {
        if !self.cfg.enabled {
            return Ok(Availability::Refused(Refusal::Disabled));
        }
        if !self.provider.configured() {
            return Ok(Availability::NotConfigured);
        }
        if self.source == KeySource::Server
            && purpose != Purpose::Models
            && let Some(allowance) = self.cfg.allowance_micros
            && self.hub.remaining(&self.org, allowance).await? <= 0
        {
            return Ok(Availability::Refused(Refusal::AllowanceUsed));
        }
        Ok(Availability::Ready)
    }

    /// The grant to call a model for `purpose`, or the reason not.
    pub async fn authorize(self, purpose: Purpose) -> AppResult<Result<Grant, Refusal>> {
        Ok(match self.availability(purpose).await? {
            Availability::Ready => Ok(Grant { hub: self.hub, org: self.org, purpose, source: self.source, provider: self.provider, allowance: self.cfg.allowance_micros }),
            Availability::NotConfigured => Err(Refusal::NotConfigured),
            Availability::Refused(r) => Err(r),
        })
    }
}

/// The one question every call site asks first: may `user` call a model for `purpose`?
pub async fn authorize(db: &OrgDb, settings: &Settings, user: &RecordId, purpose: Purpose) -> AppResult<Result<Grant, Refusal>> {
    route(db, settings, user).await?.authorize(purpose).await
}

/// [`authorize`] for a caller that has nothing to fall back on: every refusal is the error to show.
pub async fn grant(db: &OrgDb, settings: &Settings, user: &RecordId, purpose: Purpose) -> AppResult<Grant> {
    authorize(db, settings, user, purpose).await?.map_err(Refusal::into_error)
}

/// Whether a call for `purpose` could be made now. For a feature that has a path without a model: it takes that
/// path when this is false.
pub async fn available(db: &OrgDb, settings: &Settings, user: &RecordId, purpose: Purpose) -> bool {
    match route(db, settings, user).await {
        Ok(r) => matches!(r.availability(purpose).await, Ok(Availability::Ready)),
        Err(e) => {
            tracing::warn!("model provider unavailable for {}: {}", user.to_string(), e.message);
            false
        }
    }
}

/// [`available`] for work that must not be lost (a background job): a refusal is returned, so the job can wait
/// for the reason to pass instead of finishing without its work.
pub async fn readiness(db: &OrgDb, settings: &Settings, user: &RecordId, purpose: Purpose) -> AppResult<Availability> {
    route(db, settings, user).await?.availability(purpose).await
}

// ---------------------------------------------------------------------------------------------- background work

/// The job kinds that call a model. The claim leaves these alone for an organisation that is on hold
/// (`store::jobs::CLAIM` names the same list; a test keeps them equal).
pub const MODEL_JOB_KINDS: &[&str] = &["embed", "extract", "consolidate", "supersede"];

/// Whether `job` must wait because its organisation may not call a model now: AI switched off, or the allowance
/// used up with no key of the organisation's or the person's own to carry on with. The same mechanism as a
/// suspension (`jobs::paused_by_plan`): the job stays queued, is handed back without using an attempt, and the claim
/// leaves the organisation's model jobs alone (`org_ai.hold_until`) until the reason passes. It passes when the
/// month rolls over, the allowance is raised, a key is added or AI is switched on.
pub async fn job_hold(state: &crate::state::AppState, job: &crate::jobs::Job) -> bool {
    if !MODEL_JOB_KINDS.contains(&job.kind.as_str()) {
        return false;
    }
    let Ok(org) = crate::models_user::org_of(&state.control, &job.owner).await else { return false };
    let hub = state.pool.ai();
    let Ok(cfg) = hub.org_config(&org).await else { return false };
    // the common case, an organisation with nothing to hold for, costs no database read
    if cfg.enabled && cfg.allowance_micros.is_none() {
        return false;
    }
    let held = if !cfg.enabled {
        true
    } else {
        match state.org(&org).await {
            Ok(os) => matches!(readiness(&os.db, &state.settings, &job.owner, Purpose::Embedding).await, Ok(Availability::Refused(_))),
            Err(_) => false,
        }
    };
    if held {
        hub.mark_hold(&org).await;
    }
    held
}

// ---------------------------------------------------------------------------------------------- the grant

/// A reported or estimated token count.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
struct RawUsage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
    #[serde(default)]
    total_tokens: Option<u64>,
}

impl RawUsage {
    /// `(input, output)` if the provider reported them.
    fn tokens(&self) -> Option<(u64, u64)> {
        match (self.prompt_tokens, self.completion_tokens, self.total_tokens) {
            (Some(i), Some(o), _) => Some((i, o)),
            (Some(i), None, Some(t)) => Some((i, t.saturating_sub(i))),
            (Some(i), None, None) => Some((i, 0)),
            (None, None, Some(t)) => Some((t, 0)),
            _ => None,
        }
    }
}

fn usage_of(v: &Value) -> Option<RawUsage> {
    v.get("usage").and_then(|u| serde_json::from_value(u.clone()).ok())
}

/// Permission to call a model for one purpose, with the key the decision chose. The only way to reach a provider.
pub struct Grant {
    hub: Arc<Hub>,
    org: OrgId,
    purpose: Purpose,
    source: KeySource,
    provider: Provider,
    allowance: Option<i64>,
}

/// A call in flight: what is reserved for it and what it is counted under. Counted when it finishes; counted
/// conservatively if it is dropped after the request was sent (the client went away, the future was cancelled).
struct Meter {
    hub: Arc<Hub>,
    org: OrgId,
    purpose: Purpose,
    source: KeySource,
    model: String,
    priced: Priced,
    reservation: Option<Reservation>,
    est_in: u64,
    out_bytes: u64,
    sent: bool,
    done: bool,
}

impl Meter {
    fn counters(&self, input: u64, output: u64, estimated: bool) -> Counters {
        let cost = if self.source == KeySource::Server { prices::cost_micros(&self.priced.price, input, output) } else { 0 };
        Counters { calls: 1, input: input as i64, output: output as i64, estimated: i64::from(estimated), cost }
    }

    /// The call ended with an answer: count what the provider reported, or an estimate where it reported nothing.
    fn finish(&mut self, usage: Option<RawUsage>) {
        if self.done {
            return;
        }
        self.done = true;
        let (input, output, estimated) = match usage.and_then(|u| u.tokens()) {
            Some((i, o)) => (i, o, false),
            None => (self.est_in, estimate_tokens(self.out_bytes as usize), true),
        };
        let c = self.counters(input, output, estimated);
        self.hub.record(self.org, self.purpose, &self.model, self.source, c, self.reservation.take());
    }

    /// The provider refused or never got the request: nothing was billed, nothing is counted.
    fn discard(&mut self) {
        self.done = true;
        self.reservation = None;
    }
}

impl Drop for Meter {
    fn drop(&mut self) {
        if !self.done && self.sent {
            // cancelled or failed after the request left: the provider may have billed it
            let (i, o) = (self.est_in, estimate_tokens(self.out_bytes as usize));
            let c = self.counters(i, o, true);
            self.done = true;
            self.hub.record(self.org, self.purpose, &self.model, self.source, c, self.reservation.take());
        }
    }
}

/// A streamed answer being read. Finish it (`done`) when the stream ends; dropping it earlier counts what was
/// seen conservatively.
pub struct Streaming {
    resp: Option<reqwest::Response>,
    meter: Meter,
}

/// Why a streamed call did not start.
pub enum StreamError {
    /// The provider answered with an error status (not billed). The caller may recognise it and retry.
    Http { status: reqwest::StatusCode, body: String },
    Other(AppError),
}

impl Streaming {
    /// The response, once: read its body with `bytes_stream()`. It does not borrow the `Streaming`, so text can
    /// be noted while it is read.
    pub fn take_body(&mut self) -> reqwest::Response {
        self.resp.take().expect("the body is read once")
    }

    /// Text that arrived, for the estimate when the provider reports no usage.
    pub fn note_output(&mut self, bytes: usize) {
        self.meter.out_bytes += bytes as u64;
    }

    /// The stream ended. `usage` is the final chunk's `usage` object when the provider sent one.
    pub fn done(mut self, usage: Option<&Value>) {
        let raw = usage.and_then(|u| serde_json::from_value::<RawUsage>(u.clone()).ok());
        self.meter.finish(raw);
    }
}

impl Grant {
    pub fn source(&self) -> KeySource {
        self.source
    }

    /// The model to ask for chat: the configured one, else the provider's default.
    pub async fn chat_model(&self) -> String {
        provider::chat_model(&self.provider).await
    }

    /// The embedding model of the provider this grant carries ("" = the built-in default).
    pub fn embedding_model(&self) -> &str {
        self.provider.embedding_model.trim()
    }

    pub fn base_url(&self) -> &str {
        &self.provider.base_url
    }

    /// Whether to cap the output of a call (see [`caps_output`]).
    fn caps_output(&self) -> bool {
        caps_output(self.source, self.allowance.is_some(), &self.provider.base_url)
    }

    /// Whether the provider is asked to report usage in a stream (`stream_options`): our key on api.openai.com.
    fn asks_stream_usage(&self) -> bool {
        self.source == KeySource::Server && provider::same_url(&self.provider.base_url, provider::OPENAI_BASE_URL)
    }

    async fn begin(&self, model: &str, est_in: u64, max_out: u64) -> AppResult<Meter> {
        let priced = if self.source == KeySource::Server { self.hub.price(model).await } else { Priced { price: prices::Price::new(0.0, 0.0), known: true } };
        let reservation = match (self.source, self.allowance) {
            (KeySource::Server, Some(allowance)) if self.purpose != Purpose::Models => {
                let cost = prices::cost_micros(&priced.price, est_in, max_out);
                match self.hub.reserve(self.org, allowance, cost).await {
                    Ok(r) => Some(r),
                    Err(Reserve::Used) => return Err(Refusal::AllowanceUsed.into_error()),
                    Err(Reserve::Unavailable(e)) => return Err(e),
                }
            }
            _ => None,
        };
        Ok(Meter { hub: self.hub.clone(), org: self.org, purpose: self.purpose, source: self.source, model: model.to_string(), priced, reservation, est_in, out_bytes: 0, sent: false, done: false })
    }

    /// One non-streaming chat completion: `body` is the request (with its `model`). Returns the provider's JSON.
    pub async fn complete(&self, mut body: Value, timeout: Duration) -> AppResult<Value> {
        let model = body.get("model").and_then(Value::as_str).unwrap_or_default().to_string();
        if self.caps_output() && body.get("max_completion_tokens").is_none() {
            body["max_completion_tokens"] = json!(MAX_OUTPUT_TOKENS);
        }
        let est_in = estimate_tokens(body.to_string().len());
        let mut meter = self.begin(&model, est_in, MAX_OUTPUT_TOKENS).await?;
        let client = self.provider.client().await?;
        meter.sent = true;
        let resp = client
            .post(self.provider.url("chat/completions"))
            .bearer_auth(self.provider.bearer())
            .json(&body)
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() {
                    meter.discard();
                }
                AppError::internal(format!("chat completion request failed: {e}"))
            })?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            meter.discard();
            return Err(AppError::internal(format!("chat completion HTTP {status}: {text}")));
        }
        let v: Value = match resp.json().await {
            Ok(v) => v,
            // the provider answered, so it may have billed: counted by the drop of `meter`
            Err(e) => return Err(AppError::internal(format!("invalid chat completion response: {e}"))),
        };
        meter.out_bytes = v.get("choices").map_or(0, |c| c.to_string().len()) as u64;
        meter.finish(usage_of(&v));
        Ok(v)
    }

    /// One embeddings request for `input`; `T` is the response's `data` array. Counted from the response's
    /// `usage`, or from the input's size where the provider gives none.
    pub async fn embeddings<T: DeserializeOwned + Default>(&self, model: &str, input: &[String]) -> AppResult<T> {
        #[derive(Deserialize)]
        struct Reply<T> {
            #[serde(default)]
            data: T,
            #[serde(default)]
            usage: Option<RawUsage>,
        }
        let est_in = estimate_tokens(input.iter().map(String::len).sum::<usize>() + 8 * input.len());
        let mut meter = self.begin(model, est_in, 0).await?;
        let client = self.provider.client().await?;
        meter.sent = true;
        let resp = client
            .post(self.provider.url("embeddings"))
            .bearer_auth(self.provider.bearer())
            .json(&json!({ "model": model, "input": input }))
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() {
                    meter.discard();
                }
                AppError::internal(format!("OpenAI embeddings request failed: {e}"))
            })?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            meter.discard();
            return Err(AppError::internal(format!("OpenAI embeddings HTTP {status}: {text}")));
        }
        let reply: Reply<T> = resp.json().await.map_err(|e| AppError::internal(format!("invalid OpenAI embeddings response: {e}")))?;
        // an embeddings usage with only `total_tokens` is all input
        meter.finish(reply.usage);
        Ok(reply.data)
    }

    /// Starts a streamed chat completion. The caller reads [`Streaming::bytes_stream`] and ends it with
    /// [`Streaming::done`].
    pub async fn stream_chat(&self, mut body: Value) -> Result<Streaming, StreamError> {
        let model = body.get("model").and_then(Value::as_str).unwrap_or_default().to_string();
        if self.caps_output() && body.get("max_completion_tokens").is_none() {
            body["max_completion_tokens"] = json!(MAX_OUTPUT_TOKENS);
        }
        if self.asks_stream_usage() && body.get("stream_options").is_none() {
            body["stream_options"] = json!({ "include_usage": true });
        }
        let est_in = estimate_tokens(body.to_string().len());
        let mut meter = self.begin(&model, est_in, MAX_OUTPUT_TOKENS).await.map_err(StreamError::Other)?;
        let client = self.provider.client().await.map_err(StreamError::Other)?;
        meter.sent = true;
        let resp = client
            .post(self.provider.url("chat/completions"))
            .bearer_auth(self.provider.bearer())
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() {
                    meter.discard();
                }
                StreamError::Other(AppError::internal(format!("OpenAI request failed: {e}")))
            })?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            meter.discard();
            return Err(StreamError::Http { status, body });
        }
        Ok(Streaming { resp: Some(resp), meter })
    }

    /// The model ids the provider lists, or a short code for why not (never upstream text: a caller could use it
    /// to probe internal hosts). Free and not counted.
    pub async fn list_models(&self, timeout: Duration) -> Result<Vec<String>, &'static str> {
        #[derive(Deserialize)]
        struct List {
            #[serde(default)]
            data: Vec<Entry>,
        }
        #[derive(Deserialize)]
        struct Entry {
            id: String,
        }
        let client = self.provider.client().await.map_err(|_| "base_url_not_allowed")?;
        let resp = client.get(self.provider.url("models")).bearer_auth(self.provider.bearer()).timeout(timeout).send().await.map_err(|_| "unreachable")?;
        if !resp.status().is_success() {
            return Err("upstream_error");
        }
        let list: List = resp.json().await.map_err(|_| "bad_response")?;
        let mut ids: Vec<String> = list.data.into_iter().map(|m| m.id).collect();
        ids.sort();
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_counted_conservatively() {
        assert_eq!(estimate_tokens(0), 0);
        assert_eq!(estimate_tokens(1), 1);
        assert_eq!(estimate_tokens(3), 1);
        assert_eq!(estimate_tokens(4), 2);
        // 4,000 bytes of English is about 1,000 tokens; we count 1,334
        assert_eq!(estimate_tokens(4_000), 1_334);
    }

    #[test]
    fn reported_usage_is_read_in_every_shape_providers_use() {
        let t = |v: Value| usage_of(&json!({ "usage": v })).and_then(|u| u.tokens());
        assert_eq!(t(json!({"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15})), Some((10, 5)));
        assert_eq!(t(json!({"prompt_tokens": 10, "total_tokens": 10})), Some((10, 0)), "embeddings");
        assert_eq!(t(json!({"total_tokens": 7})), Some((7, 0)), "embeddings with only a total");
        assert_eq!(t(json!({})), None);
        assert_eq!(usage_of(&json!({"choices": []})).and_then(|u| u.tokens()), None);
    }

    #[test]
    fn the_refusals_are_the_documented_codes() {
        let e = Refusal::AllowanceUsed.into_error();
        assert_eq!((e.code, e.status.as_u16()), (ErrorCode::AiAllowanceUsed, 402));
        assert!(e.message.contains("allowance") && e.message.contains("owner") && e.message.contains("key"), "{}", e.message);
        let e = Refusal::Disabled.into_error();
        assert_eq!((e.code, e.status.as_u16()), (ErrorCode::AiDisabled, 403));
        assert!(Refusal::is_hold(ErrorCode::AiDisabled) && Refusal::is_hold(ErrorCode::AiAllowanceUsed) && !Refusal::is_hold(ErrorCode::Internal));
    }
}
