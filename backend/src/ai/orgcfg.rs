//! An organisation's say over model use: its own provider key and the AI switch, read and written by its
//! owner or admin (`routers/org_ai.rs`), and what its members and the operator are shown of the month's spend.
//!
//! * **Who.** Every operation starts with [`crate::org_admin::may_manage_org_ai`] and acts on the caller's own
//!   organisation, never one named in the request.
//! * **The key is never returned.** It is stored encrypted like a person's own key (`connectors::crypto`), shown
//!   as `key_set`, replaced or removed, and logged nowhere (the ledger row names the host and what happened to
//!   the key, never the key).
//! * **The address is checked** exactly as a person's own base URL is (`llm_net::check_base_url`, with
//!   `ALLOW_PRIVATE_LLM_URL`), when it is saved and again on every call.
//! * **Changing the embedding model** is allowed, as it is for a person: vectors carry the provider and model they
//!   were made with (`memory.embedding_spec`), a query only meets vectors of its own spec, and the embed job
//!   re-embeds what is stale in the background, on the organisation's key. Until it has caught up, search for
//!   memories made under the old model falls back to keywords for them. The refusal is only for what could not be
//!   searched at all: a model that is not [`crate::embeddings::service::DIM`]-dimensional on api.openai.com.
//! * **Spend shown to members** is a percentage and a state, never a price or an amount.

use serde::{Deserialize, Serialize};
use surrealdb::types::{RecordId, SurrealValue};

use super::hub::{Hub, OrgAi};
use super::provider::Provider;
use crate::config::Settings;
use crate::connectors::crypto;
use crate::error::{AppError, AppResult};
use crate::models_user::User;
use crate::pool::OrgId;
use crate::state::AppState;
use crate::store::ai as q;

/// The organisation's provider as a [`Provider`] (the key decrypted, for a call; never for a response).
pub fn provider_of(cfg: &OrgAi, settings: &Settings) -> AppResult<Provider> {
    let key = crypto::decrypt(&settings.encryption_key, &cfg.api_key_enc)?;
    let base = if cfg.base_url.trim().is_empty() { settings.openai_base_url.clone() } else { cfg.base_url.trim().to_string() };
    Ok(Provider::new(base, key, cfg.embedding_model.clone(), cfg.chat_model.clone()))
}

fn config_id(org: &OrgId) -> RecordId {
    RecordId::new("org_ai", org.key())
}

// ---------------------------------------------------------------------------------------------- spend, as shown

/// Used share of the allowance at or above which the state is `nearly_used`.
pub const NEARLY_USED_PERCENT: i64 = 80;

/// This month's use of the allowance in words and a percentage: what members see. No prices, no amounts.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct AiUsage {
    /// `ok`, `nearly_used` (80% or more) or `used_up`; null when the organisation has no allowance (unlimited).
    pub state: Option<String>,
    /// Share of this month's allowance used, 0 to 100; null when there is no allowance.
    pub percent: Option<i64>,
}

pub fn usage_view(spend_micros: i64, allowance: Option<i64>) -> AiUsage {
    let Some(allowance) = allowance else { return AiUsage { state: None, percent: None } };
    let percent = if allowance <= 0 { 100 } else { (spend_micros.max(0).saturating_mul(100) / allowance).min(100) };
    let state = if spend_micros >= allowance {
        "used_up"
    } else if percent >= NEARLY_USED_PERCENT {
        "nearly_used"
    } else {
        "ok"
    };
    AiUsage { state: Some(state.to_string()), percent: Some(percent) }
}

/// The organisation's usage of this month's allowance.
pub async fn usage(hub: &Hub, org: &OrgId, cfg: &OrgAi) -> AppResult<AiUsage> {
    match cfg.allowance_micros {
        None => Ok(usage_view(0, None)),
        Some(a) => Ok(usage_view(hub.month_spend(org, &hub.month()).await?, Some(a))),
    }
}

/// What the plan status tells every member about model use.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct AiStatus {
    /// An owner or admin has not switched AI off. When false, no model is used on this workspace's content.
    pub enabled: bool,
    /// The organisation has its own provider key, so its model use does not count against the allowance.
    pub own_key: bool,
    /// This month's use of the allowance: a percentage and a state, no amounts.
    pub usage: AiUsage,
}

pub async fn status_for_members(state: &AppState, org: &OrgId) -> AppResult<AiStatus> {
    let hub = state.pool.ai();
    let cfg = hub.org_config(org).await?;
    Ok(AiStatus { enabled: cfg.enabled, own_key: cfg.has_key(), usage: usage(hub, org, &cfg).await? })
}

// ---------------------------------------------------------------------------------------------- the owner's view

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OrgProviderView {
    /// The organisation has a provider of its own (a key or an endpoint).
    pub set: bool,
    /// The endpoint ("" = the server's).
    pub base_url: String,
    /// A key is stored. It is never returned.
    pub key_set: bool,
    /// "" = automatic.
    pub chat_model: String,
    /// "" = the default.
    pub embedding_model: String,
}

/// Calls counted this month by whose key paid (no amounts).
#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct CallsBySource {
    /// On the server's key (counted against the allowance).
    pub server: i64,
    /// On the organisation's own key.
    pub organisation: i64,
    /// On people's own keys.
    pub personal: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OrgAiView {
    /// AI is on. When off, no model is called for this workspace's content with any key.
    pub enabled: bool,
    pub provider: OrgProviderView,
    /// Whose key the workspace's model use goes on by default: `off`, `organisation_key` or `server_key`.
    pub serving: String,
    pub usage: AiUsage,
    /// Calls this month, by whose key paid.
    pub calls: CallsBySource,
}

#[derive(Deserialize, SurrealValue)]
struct CallRow {
    source: String,
    calls: i64,
}

pub async fn view(state: &AppState, user: &User) -> AppResult<OrgAiView> {
    crate::org_admin::may_manage_org_ai(&state.control, user).await?;
    let hub = state.pool.ai();
    let cfg = hub.org_config(&user.org).await?;
    let rows: Vec<CallRow> = q::USAGE_MONTH_CALLS.on(&state.control).bind(("org", user.org.record())).bind(("month", hub.month())).await?.take(0)?;
    let mut calls = CallsBySource::default();
    for r in rows {
        match r.source.as_str() {
            "server" => calls.server += r.calls,
            "org" => calls.organisation += r.calls,
            _ => calls.personal += r.calls,
        }
    }
    Ok(OrgAiView {
        enabled: cfg.enabled,
        provider: OrgProviderView { set: cfg.has_own_provider(), base_url: cfg.base_url.clone(), key_set: cfg.has_key(), chat_model: cfg.chat_model.clone(), embedding_model: cfg.embedding_model.clone() },
        serving: if !cfg.enabled {
            "off"
        } else if cfg.has_own_provider() {
            "organisation_key"
        } else {
            "server_key"
        }
        .to_string(),
        usage: usage(hub, &user.org, &cfg).await?,
        calls,
    })
}

// ---------------------------------------------------------------------------------------------- changes

/// What an owner or admin sends to set the organisation's provider. A field left out keeps its value; an
/// `api_key` of "" removes the key.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderUpdate {
    /// The endpoint, `https://...` ("" = the server's). Checked like a person's own base URL.
    pub base_url: Option<String>,
    /// The key. Stored encrypted, never returned.
    pub api_key: Option<String>,
    pub chat_model: Option<String>,
    pub embedding_model: Option<String>,
}

const MAX_FIELD: usize = 300;

fn short(field: &str, v: &str) -> AppResult<String> {
    let v = v.trim();
    if v.chars().count() > MAX_FIELD || v.chars().any(char::is_control) {
        return Err(AppError::bad_request(format!("`{field}` must be at most {MAX_FIELD} printable characters.")));
    }
    Ok(v.to_string())
}

/// Sets the organisation's own provider. Validated before anything is written.
pub async fn set_provider(state: &AppState, user: &User, body: ProviderUpdate) -> AppResult<OrgAiView> {
    crate::org_admin::may_manage_org_ai(&state.control, user).await?;
    let hub = state.pool.ai();
    let org = user.org;
    // read through the database, not the cache, so a change made a moment ago on another replica is not undone
    hub.forget_config(&org);
    let current = hub.org_config(&org).await?;

    let base_url = match &body.base_url {
        Some(u) => short("base_url", u)?,
        None => current.base_url.clone(),
    };
    if !base_url.is_empty() {
        crate::llm_net::check_base_url(&base_url, crate::llm_net::allow_private_llm_url()).await?;
    }
    let chat_model = match &body.chat_model {
        Some(m) => short("chat_model", m)?,
        None => current.chat_model.clone(),
    };
    let embedding_model = match &body.embedding_model {
        Some(m) => short("embedding_model", m)?,
        None => current.embedding_model.clone(),
    };
    let (key_enc, key_note) = match &body.api_key {
        Some(k) if k.trim().is_empty() => (String::new(), "removed"),
        Some(k) => {
            let k = k.trim();
            if k.len() > 500 || k.chars().any(char::is_control) {
                return Err(AppError::bad_request("`api_key` is not a usable key."));
            }
            (crypto::encrypt(&state.settings.encryption_key, k), "set")
        }
        None => (current.api_key_enc.clone(), if current.api_key_enc.is_empty() { "none" } else { "kept" }),
    };
    // what would be called must be able to work: a key, or an endpoint that needs none
    let effective_base = if base_url.is_empty() { state.settings.openai_base_url.as_str() } else { base_url.as_str() };
    let candidate = Provider::new(effective_base.to_string(), if key_enc.is_empty() { String::new() } else { "k".into() }, embedding_model.clone(), chat_model.clone());
    if !candidate.configured() {
        return Err(AppError::bad_request("An API key is needed for this endpoint. (Leave the endpoint empty and set a key to use OpenAI, or give the address of a server that needs none.)"));
    }
    if !embedding_model.is_empty() || base_url != current.base_url {
        let model = if embedding_model.is_empty() { crate::embeddings::service::DEFAULT_MODEL } else { embedding_model.as_str() };
        crate::embeddings::service::check_model(effective_base, model).map_err(AppError::bad_request)?;
    }
    let changed_embedding = embedding_model != current.embedding_model || base_url != current.base_url;
    {
        let id = config_id(&org);
        crate::tx::with_retry(|| async {
            q::CONFIG_SET_PROVIDER
                .on(&state.control)
                .bind(("id", id.clone()))
                .bind(("org", org.record()))
                .bind(("base_url", base_url.clone()))
                .bind(("api_key_enc", key_enc.clone()))
                .bind(("chat_model", chat_model.clone()))
                .bind(("embedding_model", embedding_model.clone()))
                .bind(("by", user.id.clone()))
                .await?
                .check()
        })
        .await?;
    }
    hub.forget_config(&org);
    // a key was added: model jobs the allowance was holding may run now
    hub.clear_hold(&org).await;
    let host = url::Url::parse(effective_base).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
    let detail = format!("host={host} key={key_note} embedding_model_changed={changed_embedding}");
    crate::audit::record_as_caller_detail(&state.control, &user.id, "org.ai_provider", &org.key(), "ok", &detail).await;
    view(state, user).await
}

/// Removes the organisation's own provider: its model use goes back to the server's key (and the allowance).
pub async fn clear_provider(state: &AppState, user: &User) -> AppResult<OrgAiView> {
    crate::org_admin::may_manage_org_ai(&state.control, user).await?;
    let org = user.org;
    let id = config_id(&org);
    crate::tx::with_retry(|| async { q::CONFIG_CLEAR_PROVIDER.on(&state.control).bind(("id", id.clone())).bind(("org", org.record())).bind(("by", user.id.clone())).await?.check() }).await?;
    state.pool.ai().forget_config(&org);
    crate::audit::record_as_caller_detail(&state.control, &user.id, "org.ai_provider", &org.key(), "ok", "removed").await;
    view(state, user).await
}

/// Switches AI on or off for the organisation. Audited.
pub async fn set_enabled(state: &AppState, user: &User, enabled: bool) -> AppResult<OrgAiView> {
    crate::org_admin::may_manage_org_ai(&state.control, user).await?;
    let org = user.org;
    let id = config_id(&org);
    crate::tx::with_retry(|| async {
        q::CONFIG_SET_ENABLED.on(&state.control).bind(("id", id.clone())).bind(("org", org.record())).bind(("enabled", enabled)).bind(("by", user.id.clone())).await?.check()
    })
    .await?;
    let hub = state.pool.ai();
    hub.forget_config(&org);
    if enabled {
        hub.clear_hold(&org).await;
    }
    crate::audit::record_as_caller_detail(&state.control, &user.id, "org.ai_switch", &org.key(), "ok", &format!("enabled={enabled}")).await;
    view(state, user).await
}

/// The allowance the billing service sets, in micro-dollars (`None`: unlimited). Raising it releases held jobs.
pub async fn set_allowance(state: &AppState, org: &OrgId, allowance_micros: Option<i64>) -> AppResult<()> {
    let id = config_id(org);
    crate::tx::with_retry(|| async {
        q::CONFIG_SET_ALLOWANCE.on(&state.control).bind(("id", id.clone())).bind(("org", org.record())).bind(("allowance", allowance_micros)).await?.check()
    })
    .await?;
    let hub = state.pool.ai();
    hub.forget_config(org);
    hub.clear_hold(org).await;
    Ok(())
}

