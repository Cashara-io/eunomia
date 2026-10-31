//! Password managers for the credential broker (docs/architecture/specs/broker.md, "Settings API"):
//! provider settings (the bootstrap token is write-only), registered references, the resolution log and
//! per-reference counts. Org owners and instance admins only; account-level (`gate.rs`), so a
//! vault-restricted token never gets here. Saving, removing or testing a provider token also needs a
//! browser session, as the other routes that manage credentials do.

use std::collections::BTreeMap;

use axum::{
    extract::{Path, Query, State},
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use crate::audit;
use crate::authz;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::User;
use crate::openapi::OkBody;
use crate::rid::RecordIdExt;
use crate::secrets::{self, client::CheckOut, CredentialRef, RefFields};
use crate::state::{AppState, OrgState};
use crate::store;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/secrets/providers", get(list_providers))
        .route("/secrets/providers/{kind}", put(put_provider).delete(delete_provider))
        .route("/secrets/providers/{kind}/test", post(test_provider))
        .route("/secrets/refs", get(list_refs).post(create_ref))
        .route("/secrets/refs/{id}", put(update_ref).delete(delete_ref))
        .route("/secrets/log", get(resolution_log))
        .route("/secrets/counts", get(resolution_counts))
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(list_providers, put_provider, delete_provider, test_provider, list_refs, create_ref, update_ref, delete_ref, resolution_log, resolution_counts),
    components(schemas(ProviderOut, ProviderBody, CheckOut, RefOut, RefBody, LogEntry, LogPage, RefCount))
)]
pub struct Doc;

// ---------------------------------------------------------------- who may

/// An instance admin, or an owner of the caller's org.
async fn require_owner(state: &AppState, user: &User) -> AppResult<()> {
    authz::require_unrestricted()?;
    if authz::is_admin(&state.control, user).await? {
        return Ok(());
    }
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        role: String,
    }
    let rows: Vec<Row> =
        store::control::MEMBERSHIP_ROLE.on(&state.control).bind(("user", user.id.clone())).bind(("org", user.org.record())).await?.check()?.take(0)?;
    if rows.iter().any(|r| r.role == "owner") {
        Ok(())
    } else {
        Err(AppError::coded(ErrorCode::AuthForbidden, "Only an owner of this organisation or an instance admin can manage password managers."))
    }
}

async fn owner_org(state: &AppState, user: &User) -> AppResult<OrgState> {
    require_owner(state, user).await?;
    state.org(&user.org).await
}

// ---------------------------------------------------------------- providers

/// A configured provider. Never the token: only that one is stored.
#[derive(Serialize, utoipa::ToSchema)]
struct ProviderOut {
    /// `onepassword_connect`, `onepassword_service_account` or `bitwarden_sm`.
    kind: String,
    host: Option<String>,
    /// Always `true` in a listed row: a token is stored (encrypted). It is never returned.
    configured: bool,
    #[schema(value_type = String)]
    updated_at: Datetime,
    /// `user:...`
    updated_by: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct ProviderBody {
    /// Connect: the Connect server URL (required). Bitwarden: the server URL (optional; the US cloud when
    /// empty, `https://vault.bitwarden.eu` for the EU cloud). Service accounts: none.
    #[serde(default)]
    host: Option<String>,
    /// The bootstrap token. Required when none is stored; omit to keep the stored one. Write-only.
    #[serde(default)]
    token: Option<String>,
}

#[utoipa::path(
    operation_id = "listSecretProviders",
    get,
    path = "/api/secrets/providers",
    tag = "secrets",
    summary = "The configured password managers (never their tokens)",
    responses((status = 200, body = Vec<ProviderOut>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_providers(State(state): State<AppState>, user: User) -> AppResult<Json<Vec<ProviderOut>>> {
    let org = owner_org(&state, &user).await?;
    let rows = secrets::providers(&org.db).await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| ProviderOut { kind: r.kind, host: r.host, configured: true, updated_at: r.updated_at, updated_by: r.updated_by.to_string() })
            .collect(),
    ))
}

#[utoipa::path(
    operation_id = "putSecretProvider",
    put,
    path = "/api/secrets/providers/{kind}",
    tag = "secrets",
    summary = "Save a password manager's host and token",
    description = "The token is encrypted (AES-256-GCM under ENCRYPTION_KEY) before it is stored and is never returned. Needs a browser session.",
    params(("kind" = String, Path)),
    request_body = ProviderBody,
    responses((status = 200, body = OkBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn put_provider(State(state): State<AppState>, user: User, Path(kind): Path<String>, Json(body): Json<ProviderBody>) -> AppResult<Json<OkBody>> {
    authz::require_session()?;
    let org = owner_org(&state, &user).await?;
    let host = secrets::check_host(body.host.as_deref())?;
    let token = body.token.as_deref().filter(|t| !t.trim().is_empty());
    secrets::put_provider(&org.db, &state.settings.encryption_key, &user.id, &kind, host, token).await?;
    let action = if token.is_some() { "secret.provider.token_set" } else { "secret.provider.update" };
    audit::record_as_caller(&state.control, &user.id, action, &format!("{}:{kind}", org.db.org().key()), "ok").await;
    Ok(Json(OkBody { ok: true, warning: None }))
}

#[utoipa::path(
    operation_id = "deleteSecretProvider",
    delete,
    path = "/api/secrets/providers/{kind}",
    tag = "secrets",
    summary = "Remove a password manager and its stored token",
    params(("kind" = String, Path)),
    responses((status = 200, body = OkBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn delete_provider(State(state): State<AppState>, user: User, Path(kind): Path<String>) -> AppResult<Json<OkBody>> {
    authz::require_session()?;
    let org = owner_org(&state, &user).await?;
    if !secrets::delete_provider(&org.db, &kind).await? {
        return Err(AppError::not_found("That password manager is not configured."));
    }
    audit::record_as_caller(&state.control, &user.id, "secret.provider.delete", &format!("{}:{kind}", org.db.org().key()), "ok").await;
    Ok(Json(OkBody { ok: true, warning: None }))
}

#[utoipa::path(
    operation_id = "testSecretProvider",
    post,
    path = "/api/secrets/providers/{kind}/test",
    tag = "secrets",
    summary = "Test connection: the secrets helper signs in with the stored token",
    params(("kind" = String, Path)),
    responses((status = 200, body = CheckOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn test_provider(State(state): State<AppState>, user: User, Path(kind): Path<String>) -> AppResult<Json<CheckOut>> {
    authz::require_session()?;
    let org = owner_org(&state, &user).await?;
    let kind = secrets::valid_kind(&kind)?;
    let Some(config) = secrets::provider_config(&org.db, &state.settings.encryption_key, kind).await? else {
        return Err(AppError::not_found("That password manager is not configured."));
    };
    let out = secrets::client::Client::from_settings(&state.settings).check(&config).await;
    let outcome = if out.ok { "ok" } else { out.error.as_deref().unwrap_or("provider_error") };
    audit::record_as_caller(&state.control, &user.id, "secret.provider.test", &format!("{}:{kind}", org.db.org().key()), outcome).await;
    Ok(Json(out))
}

// ---------------------------------------------------------------- references

#[derive(Serialize, utoipa::ToSchema)]
struct RefOut {
    /// `credential_ref:...`
    id: String,
    reference: String,
    provider: String,
    vault_id: String,
    label: String,
    allowed_tools: Vec<String>,
    allowed_actors: Vec<String>,
    created_by: String,
    #[schema(value_type = String)]
    created_at: Datetime,
    #[schema(value_type = String)]
    updated_at: Datetime,
}

impl From<CredentialRef> for RefOut {
    fn from(r: CredentialRef) -> Self {
        RefOut {
            id: r.id.to_string(),
            reference: r.reference,
            provider: r.provider,
            vault_id: r.vault.to_string(),
            label: r.label,
            allowed_tools: r.allowed_tools,
            allowed_actors: r.allowed_actors,
            created_by: r.created_by.to_string(),
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

#[derive(Deserialize, utoipa::ToSchema)]
struct RefBody {
    /// `op://<vault>/<item>/[<section>/]<field>` or `bws://<secret-id>`. Ignored on update.
    #[serde(default)]
    reference: Option<String>,
    /// The provider kind that resolves it. Ignored on update.
    #[serde(default)]
    provider: Option<String>,
    /// The vault the reference is bound to: only its members may use it.
    vault_id: String,
    #[serde(default)]
    label: Option<String>,
    /// Tools that may resolve it; `*` is any tool that takes secrets.
    #[serde(default)]
    allowed_tools: Vec<String>,
    /// `user:...`, `api_token:...` or `oauth_client:...`; empty: any member of the vault.
    #[serde(default)]
    allowed_actors: Vec<String>,
}

/// The vault must be one the admin belongs to (as for an extension's vault).
async fn fields(org: &OrgState, user: &User, body: &RefBody) -> AppResult<RefFields> {
    let vault: RecordId = crate::rid::parse(body.vault_id.trim())
        .ok()
        .filter(|r: &RecordId| r.table() == "vault")
        .ok_or_else(|| AppError::coded(ErrorCode::VaultNotFound, "Vault not found."))?;
    authz::ensure_member(&org.db, &user.id, &vault).await?;
    Ok(RefFields { vault, label: body.label.clone().unwrap_or_default(), allowed_tools: body.allowed_tools.clone(), allowed_actors: body.allowed_actors.clone() })
}

fn ref_id(id: &str) -> AppResult<RecordId> {
    crate::rid::parse(id).ok().filter(|r: &RecordId| r.table() == "credential_ref").ok_or_else(|| AppError::not_found("Reference not found."))
}

#[utoipa::path(
    operation_id = "listSecretRefs",
    get,
    path = "/api/secrets/refs",
    tag = "secrets",
    summary = "Registered secret references (never values)",
    responses((status = 200, body = Vec<RefOut>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_refs(State(state): State<AppState>, user: User) -> AppResult<Json<Vec<RefOut>>> {
    let org = owner_org(&state, &user).await?;
    Ok(Json(secrets::refs(&org.db, &user.id).await?.into_iter().map(RefOut::from).collect()))
}

#[utoipa::path(
    operation_id = "createSecretRef",
    post,
    path = "/api/secrets/refs",
    tag = "secrets",
    summary = "Register a secret reference",
    request_body = RefBody,
    responses((status = 200, body = RefOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn create_ref(State(state): State<AppState>, user: User, Json(body): Json<RefBody>) -> AppResult<Json<RefOut>> {
    let org = owner_org(&state, &user).await?;
    let f = fields(&org, &user, &body).await?;
    let reference = body.reference.as_deref().ok_or_else(|| AppError::bad_request("reference is required"))?;
    let provider = body.provider.as_deref().ok_or_else(|| AppError::bad_request("provider is required"))?;
    let row = secrets::create_ref(&org.db, &user.id, reference, provider, f).await?;
    audit::record_as_caller(&state.control, &user.id, "secret.ref.create", &row.id.to_string(), "ok").await;
    Ok(Json(row.into()))
}

#[utoipa::path(
    operation_id = "updateSecretRef",
    put,
    path = "/api/secrets/refs/{id}",
    tag = "secrets",
    summary = "Change a reference's vault, label, allowed tools or allowed actors",
    params(("id" = String, Path)),
    request_body = RefBody,
    responses((status = 200, body = RefOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn update_ref(State(state): State<AppState>, user: User, Path(id): Path<String>, Json(body): Json<RefBody>) -> AppResult<Json<RefOut>> {
    let org = owner_org(&state, &user).await?;
    let id = ref_id(&id)?;
    let f = fields(&org, &user, &body).await?;
    let row = secrets::update_ref(&org.db, &user.id, &id, f).await?.ok_or_else(|| AppError::not_found("Reference not found."))?;
    audit::record_as_caller(&state.control, &user.id, "secret.ref.update", &row.id.to_string(), "ok").await;
    Ok(Json(row.into()))
}

#[utoipa::path(
    operation_id = "deleteSecretRef",
    delete,
    path = "/api/secrets/refs/{id}",
    tag = "secrets",
    summary = "Remove a reference (the secret in the password manager is untouched)",
    params(("id" = String, Path)),
    responses((status = 200, body = OkBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn delete_ref(State(state): State<AppState>, user: User, Path(id): Path<String>) -> AppResult<Json<OkBody>> {
    let org = owner_org(&state, &user).await?;
    let id = ref_id(&id)?;
    if !secrets::delete_ref(&org.db, &user.id, &id).await? {
        return Err(AppError::not_found("Reference not found."));
    }
    audit::record_as_caller(&state.control, &user.id, "secret.ref.delete", &id.to_string(), "ok").await;
    Ok(Json(OkBody { ok: true, warning: None }))
}

// ---------------------------------------------------------------- the resolution log

#[derive(Deserialize, SurrealValue)]
struct EventRow {
    id: RecordId,
    actor_kind: String,
    actor_id: String,
    outcome: String,
    trace_id: String,
    detail: String,
    created_at: Datetime,
}

#[derive(Serialize, utoipa::ToSchema)]
struct LogEntry {
    /// `audit_event:...`
    id: String,
    #[schema(value_type = String)]
    at: Datetime,
    /// `user`, `token` or `oauth`.
    actor_kind: String,
    actor_id: String,
    reference: String,
    /// `credential_ref:...`, empty when the reference was not registered.
    ref_id: String,
    tool: String,
    /// `rest`, `mcp` or `chat`.
    channel: String,
    session: String,
    vault_id: String,
    /// `ok`, `denied`, `not_found` or `provider_error`.
    outcome: String,
    reason: String,
    trace_id: String,
}

#[derive(Serialize, utoipa::ToSchema)]
struct LogPage {
    entries: Vec<LogEntry>,
    /// Pass as `before` for the next (older) page; absent on the last page.
    next_before: Option<String>,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn entry(r: EventRow) -> LogEntry {
    let d: Value = serde_json::from_str(&r.detail).unwrap_or(Value::Null);
    LogEntry {
        id: r.id.to_string(),
        at: r.created_at,
        actor_kind: r.actor_kind,
        actor_id: r.actor_id,
        reference: s(&d, "ref"),
        ref_id: s(&d, "ref_id"),
        tool: s(&d, "tool"),
        channel: s(&d, "channel"),
        session: s(&d, "session"),
        vault_id: s(&d, "vault"),
        outcome: r.outcome,
        reason: s(&d, "reason"),
        trace_id: r.trace_id,
    }
}

#[derive(Deserialize, utoipa::IntoParams)]
struct LogQuery {
    /// 1 to 200, default 50.
    limit: Option<i64>,
    /// An RFC 3339 time: entries strictly older (the `next_before` of the previous page).
    before: Option<String>,
}

#[utoipa::path(
    operation_id = "listSecretResolutions",
    get,
    path = "/api/secrets/log",
    tag = "secrets",
    summary = "Every secret resolution in this organisation, newest first (never a value)",
    params(LogQuery),
    responses((status = 200, body = LogPage), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn resolution_log(State(state): State<AppState>, user: User, Query(q): Query<LogQuery>) -> AppResult<Json<LogPage>> {
    require_owner(&state, &user).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let before: Option<Datetime> = match q.before.as_deref() {
        None | Some("") => None,
        Some(b) => Some(Datetime::from(chrono::DateTime::parse_from_rfc3339(b).map_err(|_| AppError::bad_request("before is an RFC 3339 time"))?.with_timezone(&chrono::Utc))),
    };
    let rows: Vec<EventRow> = store::control::SECRET_LOG
        .on(&state.control)
        .bind(("org", user.org.key()))
        .bind(("before", before))
        .bind(("limit", limit + 1))
        .await?
        .check()?
        .take(0)?;
    let more = rows.len() as i64 > limit;
    let entries: Vec<LogEntry> = rows.into_iter().take(limit as usize).map(entry).collect();
    let next_before = if more { entries.last().map(|e| chrono::DateTime::<chrono::Utc>::from(e.at).to_rfc3339()) } else { None };
    Ok(Json(LogPage { entries, next_before }))
}

#[derive(Serialize, Default, utoipa::ToSchema)]
struct RefCount {
    reference: String,
    ref_id: String,
    ok: i64,
    denied: i64,
    not_found: i64,
    provider_error: i64,
    #[schema(value_type = Option<String>)]
    last_at: Option<Datetime>,
}

#[derive(Deserialize, utoipa::IntoParams)]
struct CountQuery {
    /// Look-back window in hours, 1 to 2160 (90 days), default 168.
    hours: Option<i64>,
}

#[utoipa::path(
    operation_id = "getSecretCounts",
    get,
    path = "/api/secrets/counts",
    tag = "secrets",
    summary = "Resolutions per reference and outcome over a window",
    params(CountQuery),
    responses((status = 200, body = Vec<RefCount>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn resolution_counts(State(state): State<AppState>, user: User, Query(q): Query<CountQuery>) -> AppResult<Json<Vec<RefCount>>> {
    require_owner(&state, &user).await?;
    let hours = q.hours.unwrap_or(168).clamp(1, 2160);
    let since = Datetime::from(chrono::Utc::now() - chrono::TimeDelta::hours(hours));
    let rows: Vec<EventRow> = store::control::SECRET_SINCE.on(&state.control).bind(("org", user.org.key())).bind(("since", since)).await?.check()?.take(0)?;
    let mut by: BTreeMap<String, RefCount> = BTreeMap::new();
    for r in rows {
        let d: Value = serde_json::from_str(&r.detail).unwrap_or(Value::Null);
        let reference = s(&d, "ref");
        let c = by.entry(reference.clone()).or_insert_with(|| RefCount { reference, ..Default::default() });
        if c.ref_id.is_empty() {
            c.ref_id = s(&d, "ref_id");
        }
        match r.outcome.as_str() {
            "ok" => c.ok += 1,
            "denied" => c.denied += 1,
            "not_found" => c.not_found += 1,
            _ => c.provider_error += 1,
        }
        if c.last_at.is_none_or(|t| t < r.created_at) {
            c.last_at = Some(r.created_at);
        }
    }
    Ok(Json(by.into_values().collect()))
}
