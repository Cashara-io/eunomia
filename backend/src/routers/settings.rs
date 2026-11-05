//! Per-user app settings: embedding model, sync intervals, whether syncs wait for an internet
//! connection (`sync_check_connectivity`, default on), theme, and the
//! OpenAI(-compatible) base URL + optional API key.
//!
//! `openai_api_key` is stored AES-GCM-encrypted (`connectors::crypto`) in
//! `openai_api_key_encrypted`; it is never returned, only `openai_api_key_set`.
//! An empty `openai_base_url` means the server's `OPENAI_BASE_URL`; which key
//! goes where is decided by `embeddings::provider`.

use surrealdb::types::SurrealValue;
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use surrealdb::types::RecordId;
use crate::rid::RecordIdExt;

use crate::config::Settings;
use crate::embeddings::{provider, service as embeddings};
use crate::pool::OrgDb;
use crate::store;
use crate::error::{AppError, AppResult};
use crate::models_user::User;
use crate::state::AppState;

const DEFAULT_OBSERVATIONS_MISSION: &str = "Observations are stable facts about people and relationships: preferences, skills, roles, recurring patterns, and how they change over time. Ignore ephemeral or one-off details.";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/settings", get(read_settings).patch(patch_settings))
        .route("/settings/complete-onboarding", post(complete_onboarding))
        .route("/settings/openai-models", get(openai_models))
}

#[derive(Debug, Deserialize, SurrealValue)]
struct AppSettingsRow {
    #[serde(default)]
    #[surreal(default)]
    embedding_model: String,
    #[serde(default)]
    #[surreal(default)]
    sync_intervals: Value,
    /// NONE on rows from before the field existed: treated as on.
    #[serde(default)]
    #[surreal(default)]
    sync_check_connectivity: Option<bool>,
    #[serde(default)]
    #[surreal(default)]
    theme: Value,
    #[serde(default)]
    #[surreal(default)]
    openai_api_key_encrypted: String,
    #[serde(default)]
    #[surreal(default)]
    openai_base_url: String,
    #[serde(default)]
    #[surreal(default)]
    observations_mission: String,
    #[serde(default)]
    #[surreal(default)]
    memory_skill: String,
    #[serde(default)]
    #[surreal(default)]
    chat_model: String,
    #[serde(default)]
    #[surreal(default)]
    owner_name: String,
}

#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
struct SettingsUpdate {
    embedding_model: Option<String>,
    sync_intervals: Option<Value>,
    /// Skip scheduled syncs while there is no internet connection (default true).
    sync_check_connectivity: Option<bool>,
    theme: Option<Value>,
    openai_api_key: Option<String>,
    openai_base_url: Option<String>,
    observations_mission: Option<String>,
    /// "" resets to the built-in skill
    memory_skill: Option<String>,
    /// "" = automatic (see embeddings::provider::chat_model)
    chat_model: Option<String>,
    /// The user's own name, used by email extraction so facts about the mailbox owner have a subject ("" = unknown)
    owner_name: Option<String>,
}

fn app_settings_id(owner: &RecordId) -> RecordId {
    RecordId::from_table_key("app_settings", owner.key().clone())
}

#[derive(Serialize, utoipa::ToSchema)]
struct SettingsOut {
    embedding_model: String,
    chat_model: String,
    owner_name: String,
    /// Open map of source key to sync interval seconds.
    #[schema(value_type = Object)]
    sync_intervals: Value,
    /// Probe for an internet connection before a scheduled sync and skip it quietly while offline.
    /// Default true. "Sync now" reports `status: "offline"` instead of failing.
    sync_check_connectivity: bool,
    /// Open UI theme preferences.
    #[schema(value_type = Object)]
    theme: Value,
    openai_api_key_set: bool,
    /// A usable model endpoint resolves for this user (their key, the server's, or a keyless local endpoint): chat can answer.
    model_configured: bool,
    openai_base_url: String,
    observations_mission: String,
    memory_skill: String,
    memory_skill_custom: bool,
}

fn out(row: &AppSettingsRow, server_base_url: &str, model_configured: bool) -> SettingsOut {
    SettingsOut {
        embedding_model: row.embedding_model.clone(),
        chat_model: row.chat_model.clone(),
        owner_name: row.owner_name.clone(),
        sync_intervals: row.sync_intervals.clone(),
        sync_check_connectivity: row.sync_check_connectivity.unwrap_or(true),
        theme: row.theme.clone(),
        openai_api_key_set: !row.openai_api_key_encrypted.is_empty(),
        model_configured,
        openai_base_url: provider::effective_base_url(server_base_url, &row.openai_base_url).to_string(),
        observations_mission: row.observations_mission.clone(),
        memory_skill: crate::docs::effective_skill(&row.memory_skill).to_string(),
        memory_skill_custom: !row.memory_skill.trim().is_empty(),
    }
}

/// The `app_settings:<owner_id>` row, creating it with defaults if missing.
/// The skill text for `owner` (theirs, or the built-in default).
pub async fn memory_skill(db: &OrgDb, owner: &RecordId) -> AppResult<String> {
    let row = get_app_settings(db, owner).await?;
    Ok(crate::docs::effective_skill(&row.memory_skill).to_string())
}

async fn get_app_settings(db: &OrgDb, owner: &RecordId) -> AppResult<AppSettingsRow> {
    let rid = app_settings_id(owner);
    let row: Option<AppSettingsRow> = store::get(db, &rid).await?;
    if let Some(row) = row {
        return Ok(row);
    }

    let mut res = store::app::SETTINGS_UPSERT_DEFAULTS
        .on(db)
        .bind(("id", rid))
        .bind(("owner", owner.clone()))
        .bind(("embedding_model", embeddings::DEFAULT_MODEL))
        .bind(("sync_intervals", json!({ "heypocket": 86400 })))
        .bind(("theme", json!({})))
        .bind(("observations_mission", DEFAULT_OBSERVATIONS_MISSION))
        .bind(("openai_base_url", "")) // "" = the server's
        .await?;
    let rows: Vec<AppSettingsRow> = res.take(0)?;
    rows.into_iter()
        .next()
        .ok_or_else(|| AppError::internal("app_settings upsert returned no row"))
}

/// Partial update: each provided field replaces its current value outright
/// (no deep merge).
async fn update_app_settings(db: &OrgDb, owner: &RecordId, body: &SettingsUpdate, encryption_key: &str) -> AppResult<AppSettingsRow> {
    get_app_settings(db, owner).await?; // ensure the row exists

    let mut set_parts: Vec<&str> = Vec::new();
    if body.chat_model.is_some() {
        set_parts.push("chat_model = $chat_model");
    }
    if body.owner_name.is_some() {
        set_parts.push("owner_name = $owner_name");
    }
    if body.embedding_model.is_some() {
        set_parts.push("embedding_model = $embedding_model");
    }
    if body.sync_intervals.is_some() {
        set_parts.push("sync_intervals = $sync_intervals");
    }
    if body.sync_check_connectivity.is_some() {
        set_parts.push("sync_check_connectivity = $sync_check_connectivity");
    }
    if body.theme.is_some() {
        set_parts.push("theme = $theme");
    }
    if body.openai_api_key.is_some() {
        set_parts.push("openai_api_key_encrypted = $openai_api_key_encrypted");
    }
    if body.openai_base_url.is_some() {
        set_parts.push("openai_base_url = $openai_base_url");
    }
    if body.observations_mission.is_some() {
        set_parts.push("observations_mission = $observations_mission");
    }
    if body.memory_skill.is_some() {
        set_parts.push("memory_skill = $memory_skill");
    }

    if set_parts.is_empty() {
        return get_app_settings(db, owner).await;
    }

    let query_str = format!("UPDATE $id SET {}, updated_at = time::now() RETURN AFTER", set_parts.join(", "));
    let rid = app_settings_id(owner);
    // dynamic: the SET clause list depends on which fields the patch carries
    let mut q = store::dynamic(db, "app.settings_update", query_str).bind(("id", rid));
    if let Some(v) = &body.chat_model {
        q = q.bind(("chat_model", v.trim().to_string()));
    }
    if let Some(v) = &body.owner_name {
        q = q.bind(("owner_name", v.trim().to_string()));
    }
    if let Some(v) = &body.embedding_model {
        q = q.bind(("embedding_model", v.clone()));
    }
    if let Some(v) = &body.sync_intervals {
        q = q.bind(("sync_intervals", v.clone()));
    }
    if let Some(v) = body.sync_check_connectivity {
        q = q.bind(("sync_check_connectivity", v));
    }
    if let Some(v) = &body.theme {
        q = q.bind(("theme", v.clone()));
    }
    if let Some(v) = &body.openai_api_key {
        q = q.bind(("openai_api_key_encrypted", crate::connectors::crypto::encrypt(encryption_key, v)));
    }
    if let Some(v) = &body.openai_base_url {
        q = q.bind(("openai_base_url", v.clone()));
    }
    if let Some(v) = &body.observations_mission {
        q = q.bind(("observations_mission", v.clone()));
    }
    if let Some(v) = &body.memory_skill {
        // saving the built-in text unchanged keeps following future defaults
        let v = if v.trim() == crate::docs::DEFAULT_SKILL.trim() { String::new() } else { v.clone() };
        q = q.bind(("memory_skill", v));
    }

    let mut res = q.await?;
    let rows: Vec<AppSettingsRow> = res.take(0)?;
    rows.into_iter()
        .next()
        .ok_or_else(|| AppError::internal("app_settings update returned no row"))
}

/// Rejects an embedding model the update would make unusable on the endpoint it would be sent to,
/// before anything is written. (The base URL itself is checked by `llm_net::check_base_url`.)
async fn validate_update(db: &OrgDb, settings: &Settings, owner: &RecordId, body: &SettingsUpdate) -> AppResult<()> {
    if body.openai_base_url.is_none() && body.embedding_model.is_none() {
        return Ok(());
    }
    let row = get_app_settings(db, owner).await?;
    let base = body.openai_base_url.as_deref().unwrap_or(&row.openai_base_url);
    let model = body.embedding_model.as_deref().unwrap_or(&row.embedding_model).trim();
    let model = if model.is_empty() { embeddings::DEFAULT_MODEL } else { model };
    embeddings::check_model(provider::effective_base_url(&settings.openai_base_url, base), model).map_err(AppError::bad_request)
}

async fn respond(state: &crate::state::OrgState, user: &User, row: &AppSettingsRow) -> AppResult<Json<SettingsOut>> {
    let ready = crate::chat::service::ensure_configured(&state.db, &state.settings, &user.id).await.is_ok();
    Ok(Json(out(row, &state.settings.openai_base_url, ready)))
}

#[utoipa::path(
    operation_id = "getSettings",
    get,
    path = "/api/settings",
    tag = "settings",
    summary = "Read app settings",
    responses((status = 200, body = SettingsOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn read_settings(State(state): State<AppState>, user: User) -> AppResult<Json<SettingsOut>> {
    let state = state.org(&user.org).await?;
    let row = get_app_settings(&state.db, &user.id).await?;
    respond(&state, &user, &row).await
}

#[utoipa::path(
    operation_id = "updateSettings",
    patch,
    path = "/api/settings",
    tag = "settings",
    summary = "Update app settings",
    request_body = SettingsUpdate,
    responses((status = 200, body = SettingsOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn patch_settings(
    State(state): State<AppState>,
    user: User,
    Json(body): Json<SettingsUpdate>,
) -> AppResult<Json<SettingsOut>> {
    let state = state.org(&user.org).await?;
    if let Some(u) = &body.openai_base_url {
        crate::llm_net::check_base_url(u, crate::llm_net::allow_private_llm_url()).await?;
    }
    validate_update(&state.db, &state.settings, &user.id, &body).await?;
    let row = update_app_settings(&state.db, &user.id, &body, &state.settings.encryption_key).await?;
    respond(&state, &user, &row).await
}

#[utoipa::path(
    operation_id = "completeOnboarding",
    post,
    path = "/api/settings/complete-onboarding",
    tag = "settings",
    summary = "Mark onboarding done",
    responses((status = 200, body = crate::openapi::OkBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn complete_onboarding(State(state): State<AppState>, user: User) -> AppResult<Json<Value>> {
    store::control::AUTH_USER_ONBOARDED
        .on(&state.control)
        .bind(("id", user.id.clone()))
        .await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
struct ModelsListResponse {
    #[serde(default)]
    data: Vec<ModelEntry>,
}

#[derive(Debug, Deserialize)]
struct ModelEntry {
    id: String,
}

#[derive(Serialize, utoipa::ToSchema)]
struct ModelsOut {
    models: Vec<String>,
    /// Null on success.
    error: Option<String>,
}

/// Lists models from the caller's configured OpenAI-compatible base URL.
/// Never errors out to the caller -- an unreachable base URL or auth
/// failure comes back as `{"models": [], "error": "..."}`.
#[utoipa::path(
    operation_id = "listOpenaiModels",
    get,
    path = "/api/settings/openai-models",
    tag = "settings",
    summary = "List models the configured OpenAI endpoint offers",
    responses((status = 200, body = ModelsOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn openai_models(State(state): State<AppState>, user: User) -> AppResult<Json<ModelsOut>> {
    let state = state.org(&user.org).await?;
    // Generic codes only: upstream error or status text would let a caller probe internal hosts and ports.
    let fail = |code: &str| Json(ModelsOut { models: vec![], error: Some(code.to_string()) });
    let Ok(p) = provider::resolve(&state.db, &state.settings, &user.id).await else { return Ok(fail("credential_unreadable")) };
    let Ok(client) = p.client().await else { return Ok(fail("base_url_not_allowed")) };
    let Ok(resp) = client.get(p.url("models")).bearer_auth(p.bearer()).timeout(std::time::Duration::from_secs(15)).send().await else {
        return Ok(fail("unreachable"));
    };
    if !resp.status().is_success() {
        return Ok(fail("upstream_error"));
    }
    match resp.json::<ModelsListResponse>().await {
        Ok(body) => {
            let mut ids: Vec<String> = body.data.into_iter().map(|m| m.id).collect();
            ids.sort();
            Ok(Json(ModelsOut { models: ids, error: None }))
        }
        Err(_) => Ok(fail("bad_response")),
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    read_settings,
    patch_settings,
    complete_onboarding,
    openai_models,
))]
pub struct Doc;

#[cfg(test)]
mod tests {
    use super::*;

    fn row(key_set: bool) -> AppSettingsRow {
        AppSettingsRow {
            embedding_model: embeddings::DEFAULT_MODEL.to_string(),
            sync_intervals: json!({ "heypocket": 86400 }),
            sync_check_connectivity: None,
            theme: json!({}),
            openai_api_key_encrypted: if key_set { "sk-very-secret-123".to_string() } else { String::new() },
            openai_base_url: String::new(),
            observations_mission: DEFAULT_OBSERVATIONS_MISSION.to_string(),
            memory_skill: String::new(),
            chat_model: String::new(),
            owner_name: String::new(),
        }
    }

    #[test]
    fn out_reports_key_set_true_when_encrypted_value_present() {
        let v = serde_json::to_value(out(&row(true), SERVER, false)).unwrap();
        assert_eq!(v["openai_api_key_set"], json!(true));
        assert!(v.get("openai_api_key_encrypted").is_none());
    }

    #[test]
    fn out_reports_key_set_false_when_empty() {
        let v = serde_json::to_value(out(&row(false), SERVER, false)).unwrap();
        assert_eq!(v["openai_api_key_set"], json!(false));
    }

    const SERVER: &str = "http://llm.internal/v1";

    #[test]
    fn out_reports_the_connectivity_check_on_unless_switched_off() {
        let mut r = row(false);
        assert_eq!(serde_json::to_value(out(&r, SERVER, false)).unwrap()["sync_check_connectivity"], json!(true), "a row from before the field reads as on");
        r.sync_check_connectivity = Some(false);
        assert_eq!(serde_json::to_value(out(&r, SERVER, false)).unwrap()["sync_check_connectivity"], json!(false));
    }

    #[test]
    fn out_shows_the_servers_base_url_when_the_user_has_none() {
        assert_eq!(serde_json::to_value(out(&row(false), SERVER, false)).unwrap()["openai_base_url"], json!(SERVER));
        let mut r = row(false);
        r.openai_base_url = "http://mine/v1".into();
        assert_eq!(serde_json::to_value(out(&r, SERVER, false)).unwrap()["openai_base_url"], json!("http://mine/v1"));
    }

    #[test]
    fn out_never_leaks_the_raw_secret() {
        let v = serde_json::to_value(out(&row(true), SERVER, false)).unwrap();
        let dumped = v.to_string();
        assert!(!dumped.contains("sk-very-secret-123"));
    }
}
