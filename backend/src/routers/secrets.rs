//! Password managers for the credential broker (docs/architecture/specs/broker.md, "Settings API"):
//! provider settings (the bootstrap token is write-only), registered references, the resolution log and
//! per-reference counts, the provider kinds (with their stability), and the secrets helper's status and
//! the request to switch it on or off.
//!
//! Who may (decided by Avi on 2026-11-01):
//! - Org owners and instance admins: everything, for every vault they belong to. Only they set up a
//!   provider (its token is org-wide); saving, removing or testing one also needs a browser session.
//! - Vault owners: the references bound to a vault they own, and that vault's resolution log and counts.
//!   Nothing else: not another vault's references, not a provider's settings or token.
//! - Instance admins only: switching the secrets helper on or off (it changes the server's compose stack).
//! - Anyone signed in: the provider kinds and the helper's status.
//!
//! Account-level routes (`gate.rs`): a vault-restricted token gets `auth.scope` on every one, even for the
//! vault it is restricted to (an agent's token must not widen what secrets it may resolve).

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
use crate::secrets::{self, client::CheckOut, CredentialRef, Kind, RefFields, Stability};
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
        .route("/secrets/kinds", get(list_kinds))
        .route("/secrets/me", get(my_access))
        .route("/secrets/helper", get(helper_status).post(request_helper))
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        list_providers, put_provider, delete_provider, test_provider, list_refs, create_ref, update_ref, delete_ref, resolution_log, resolution_counts,
        list_kinds, my_access, helper_status, request_helper
    ),
    components(schemas(ProviderOut, ProviderBody, CheckOut, RefOut, RefBody, LogEntry, LogPage, RefCount, Kind, Stability, AccessOut, VaultRef, HelperStatus, HelperRequest, HelperAck))
)]
pub struct Doc;

// ---------------------------------------------------------------- who may

/// What the caller may manage here.
struct Access {
    org: OrgState,
    /// An org owner or an instance admin: providers, and the references of every vault they belong to.
    org_manager: bool,
    /// An instance admin (may switch the secrets helper on or off).
    admin: bool,
    /// The vaults whose references the caller may manage: every vault they belong to for an org manager,
    /// the vaults they own otherwise. With names, for Settings.
    vaults: Vec<(RecordId, String)>,
}

impl Access {
    fn vault_ids(&self) -> Vec<RecordId> {
        self.vaults.iter().map(|(v, _)| v.clone()).collect()
    }

    fn manages(&self, vault: &RecordId) -> bool {
        self.vaults.iter().any(|(v, _)| v == vault)
    }

    /// References and the log: an org manager, or the owner of at least one vault.
    fn require_refs(&self) -> AppResult<()> {
        if self.org_manager || !self.vaults.is_empty() {
            Ok(())
        } else {
            Err(AppError::coded(ErrorCode::AuthForbidden, "Only an owner of this organisation, an instance admin or a vault owner can manage secret references."))
        }
    }

    /// A provider's settings and token: an org manager only.
    fn require_org(&self) -> AppResult<()> {
        if self.org_manager {
            Ok(())
        } else {
            Err(AppError::coded(ErrorCode::AuthForbidden, "Only an owner of this organisation or an instance admin can manage password managers."))
        }
    }
}

/// Who the caller is here. A vault-restricted token gets `auth.scope` (the gate already refuses it; this
/// keeps the rule when the function is reached another way).
async fn access(state: &AppState, user: &User) -> AppResult<Access> {
    authz::require_unrestricted()?;
    let org = state.org(&user.org).await?;
    let admin = authz::is_admin(&state.control, user).await?;
    let org_manager = admin || {
        #[derive(Deserialize, SurrealValue)]
        struct Row {
            role: String,
        }
        let rows: Vec<Row> =
            store::control::MEMBERSHIP_ROLE.on(&state.control).bind(("user", user.id.clone())).bind(("org", user.org.record())).await?.check()?.take(0)?;
        rows.iter().any(|r| r.role == "owner")
    };
    let vaults = crate::vaults::service::list_my_vaults(&org.db, &user.id)
        .await?
        .into_iter()
        .filter(|v| org_manager || v.role == "owner")
        .filter_map(|v| crate::rid::parse(&v.id).ok().map(|id| (id, v.name)))
        .collect();
    Ok(Access { org, org_manager, admin, vaults })
}

async fn org_access(state: &AppState, user: &User) -> AppResult<Access> {
    let a = access(state, user).await?;
    a.require_org()?;
    Ok(a)
}

async fn ref_access(state: &AppState, user: &User) -> AppResult<Access> {
    let a = access(state, user).await?;
    a.require_refs()?;
    Ok(a)
}

// ---------------------------------------------------------------- providers

/// A configured provider. Never the token: only that one is stored.
#[derive(Serialize, utoipa::ToSchema)]
struct ProviderOut {
    /// `onepassword_connect`, `onepassword_service_account` or `bitwarden_sm`.
    kind: String,
    /// For people (`GET /api/secrets/kinds`).
    label: String,
    /// `stable` or `preview` (`GET /api/secrets/kinds`).
    stability: Stability,
    /// `true` for a kind in preview (Bitwarden Secrets Manager).
    preview: bool,
    /// The server URL; shown to org managers only (a vault owner sees `null`).
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
    description = "Org owners and instance admins; a vault owner sees which kinds are configured (no host), to pick one for a reference.",
    responses((status = 200, body = Vec<ProviderOut>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_providers(State(state): State<AppState>, user: User) -> AppResult<Json<Vec<ProviderOut>>> {
    let a = ref_access(&state, &user).await?;
    let rows = secrets::providers(&a.org.db).await?;
    Ok(Json(
        rows.into_iter()
            .filter_map(|r| {
                let k = secrets::kind_info(&r.kind)?;
                Some(ProviderOut {
                    kind: r.kind,
                    label: k.label.to_string(),
                    stability: k.stability,
                    preview: k.preview,
                    host: r.host.filter(|_| a.org_manager),
                    configured: true,
                    updated_at: r.updated_at,
                    updated_by: if a.org_manager { r.updated_by.to_string() } else { String::new() },
                })
            })
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
    let org = org_access(&state, &user).await?.org;
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
    let org = org_access(&state, &user).await?.org;
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
    description = "The provider's answer is `{ok, error?, message?}`. When the secrets helper itself is switched off or not answering: `secret.unavailable` (503) with `reason` `helper_disabled` or `helper_unreachable`, at once.",
    params(("kind" = String, Path)),
    responses((status = 200, body = CheckOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn test_provider(State(state): State<AppState>, user: User, Path(kind): Path<String>) -> AppResult<Json<CheckOut>> {
    authz::require_session()?;
    let org = org_access(&state, &user).await?.org;
    let kind = secrets::valid_kind(&kind)?;
    let Some(config) = secrets::provider_config(&org.db, &state.settings.encryption_key, kind).await? else {
        return Err(AppError::not_found("That password manager is not configured."));
    };
    let target = format!("{}:{kind}", org.db.org().key());
    match secrets::client::Client::from_settings(&state.settings).check(&config).await {
        Ok(out) => {
            let outcome = if out.ok { "ok" } else { out.error.as_deref().unwrap_or("provider_error") };
            audit::record_as_caller(&state.control, &user.id, "secret.provider.test", &target, outcome).await;
            Ok(Json(out))
        }
        // the helper itself is off or not answering: secret.unavailable with the reason, at once
        Err(u) => {
            audit::record_as_caller(&state.control, &user.id, "secret.provider.test", &target, u.reason).await;
            Err(u.into_error())
        }
    }
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

/// The vault must be one the caller manages: one they belong to for an org manager, one they own for a
/// vault owner.
fn fields(a: &Access, body: &RefBody) -> AppResult<RefFields> {
    let vault: RecordId = crate::rid::parse(body.vault_id.trim())
        .ok()
        .filter(|r: &RecordId| r.table() == "vault")
        .ok_or_else(|| AppError::coded(ErrorCode::VaultNotFound, "Vault not found."))?;
    if !a.manages(&vault) {
        let need = if a.org_manager { "a member" } else { "an owner" };
        return Err(AppError::coded(ErrorCode::VaultForbidden, format!("You must be {need} of vault {} to bind a reference to it.", vault.to_string())));
    }
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
    let a = ref_access(&state, &user).await?;
    Ok(Json(secrets::refs(&a.org.db, &a.vault_ids()).await?.into_iter().map(RefOut::from).collect()))
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
    let a = ref_access(&state, &user).await?;
    let f = fields(&a, &body)?;
    let reference = body.reference.as_deref().ok_or_else(|| AppError::bad_request("reference is required"))?;
    let provider = body.provider.as_deref().ok_or_else(|| AppError::bad_request("provider is required"))?;
    let row = secrets::create_ref(&a.org.db, &user.id, reference, provider, f).await?;
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
    let a = ref_access(&state, &user).await?;
    let id = ref_id(&id)?;
    let f = fields(&a, &body)?;
    // only a reference bound to a vault the caller manages is found; any other is 404, as another org's is
    let row = secrets::update_ref(&a.org.db, &a.vault_ids(), &id, f).await?.ok_or_else(|| AppError::not_found("Reference not found."))?;
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
    let a = ref_access(&state, &user).await?;
    let id = ref_id(&id)?;
    if !secrets::delete_ref(&a.org.db, &a.vault_ids(), &id).await? {
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
    /// The stable code when the secrets helper itself failed: `helper_disabled` or `helper_unreachable`.
    code: String,
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
        code: s(&d, "code"),
        trace_id: r.trace_id,
    }
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct LogQuery {
    /// 1 to 200, default 50.
    limit: Option<i64>,
    /// An RFC 3339 time: entries strictly older (the `next_before` of the previous page).
    before: Option<String>,
}

/// The most rows one log page reads for a vault owner before it answers (with `next_before` set to carry on).
const LOG_SCAN: usize = 2000;

/// Does a vault owner see this entry? Org managers see every entry.
fn visible(a: &Access, vault_id: &str) -> bool {
    a.org_manager || a.vaults.iter().any(|(v, _)| v.to_string() == vault_id)
}

#[utoipa::path(
    operation_id = "listSecretResolutions",
    get,
    path = "/api/secrets/log",
    tag = "secrets",
    summary = "Every secret resolution in this organisation, newest first (never a value)",
    description = "Org owners and instance admins see the whole organisation; a vault owner sees the resolutions of the vaults they own.",
    params(LogQuery),
    responses((status = 200, body = LogPage), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn resolution_log(State(state): State<AppState>, user: User, Query(q): Query<LogQuery>) -> AppResult<Json<LogPage>> {
    let a = ref_access(&state, &user).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200) as usize;
    let mut cursor: Option<Datetime> = match q.before.as_deref() {
        None | Some("") => None,
        Some(b) => Some(Datetime::from(chrono::DateTime::parse_from_rfc3339(b).map_err(|_| AppError::bad_request("before is an RFC 3339 time"))?.with_timezone(&chrono::Utc))),
    };
    // an org manager keeps every row: one read of limit + 1. A vault owner's rows are picked out of the
    // org's (`detail.vault`), so read in batches until the page is full, the log ends or LOG_SCAN rows.
    let batch = if a.org_manager { limit + 1 } else { 200 };
    let (mut entries, mut scanned, mut ended) = (Vec::new(), 0usize, false);
    while entries.len() <= limit && !ended && scanned < LOG_SCAN {
        let rows: Vec<EventRow> = store::control::SECRET_LOG
            .on(&state.control)
            .bind(("org", user.org.key()))
            .bind(("before", cursor))
            .bind(("limit", batch as i64))
            .await?
            .check()?
            .take(0)?;
        ended = rows.len() < batch;
        for r in rows {
            scanned += 1;
            cursor = Some(r.created_at);
            let e = entry(r);
            if visible(&a, &e.vault_id) {
                entries.push(e);
                if entries.len() > limit {
                    break;
                }
            }
        }
    }
    let next_before = if entries.len() > limit {
        entries.truncate(limit);
        entries.last().map(|e| e.at)
    } else if !ended {
        cursor // the scan stopped early: carry on from the last row read
    } else {
        None
    };
    Ok(Json(LogPage { entries, next_before: next_before.map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()) }))
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
#[into_params(parameter_in = Query)]
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
    description = "Org owners and instance admins: the whole organisation; a vault owner: the vaults they own.",
    params(CountQuery),
    responses((status = 200, body = Vec<RefCount>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn resolution_counts(State(state): State<AppState>, user: User, Query(q): Query<CountQuery>) -> AppResult<Json<Vec<RefCount>>> {
    let a = ref_access(&state, &user).await?;
    let hours = q.hours.unwrap_or(168).clamp(1, 2160);
    let since = Datetime::from(chrono::Utc::now() - chrono::TimeDelta::hours(hours));
    let rows: Vec<EventRow> = store::control::SECRET_SINCE.on(&state.control).bind(("org", user.org.key())).bind(("since", since)).await?.check()?.take(0)?;
    let mut by: BTreeMap<String, RefCount> = BTreeMap::new();
    for r in rows {
        let d: Value = serde_json::from_str(&r.detail).unwrap_or(Value::Null);
        if !visible(&a, &s(&d, "vault")) {
            continue;
        }
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


// ---------------------------------------------------------------- provider kinds

#[utoipa::path(
    operation_id = "listSecretKinds",
    get,
    path = "/api/secrets/kinds",
    tag = "secrets",
    summary = "The password manager kinds Eunomia supports, with their stability (Bitwarden Secrets Manager is a preview)",
    responses((status = 200, body = Vec<Kind>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_kinds(_user: User) -> Json<&'static [Kind]> {
    Json(secrets::PROVIDERS)
}

// ---------------------------------------------------------------- the caller's access

#[derive(Serialize, utoipa::ToSchema)]
struct VaultRef {
    /// `vault:...`
    id: String,
    name: String,
}

#[derive(Serialize, utoipa::ToSchema)]
struct AccessOut {
    /// An org owner or instance admin: may set up providers and manage every vault's references.
    manage_providers: bool,
    /// May manage references at all (an org manager, or the owner of a vault).
    manage_refs: bool,
    /// An instance admin: may switch the secrets helper on or off.
    toggle_helper: bool,
    /// The vaults the caller may bind references to.
    vaults: Vec<VaultRef>,
}

#[utoipa::path(
    operation_id = "getSecretAccess",
    get,
    path = "/api/secrets/me",
    tag = "secrets",
    summary = "What the caller may manage under Password managers",
    responses((status = 200, body = AccessOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn my_access(State(state): State<AppState>, user: User) -> AppResult<Json<AccessOut>> {
    let a = access(&state, &user).await?;
    Ok(Json(AccessOut {
        manage_providers: a.org_manager,
        manage_refs: a.require_refs().is_ok(),
        toggle_helper: a.admin,
        vaults: a.vaults.into_iter().map(|(id, name)| VaultRef { id: id.to_string(), name }).collect(),
    }))
}

// ---------------------------------------------------------------- the secrets helper: status and switch

/// The request the backend drops for the updater (scripts/auto-update.sh, `secrets_apply`).
const HELPER_REQUEST: &str = "secrets.json";
/// The updater's outcome: `{"state": off|starting|running|error, "message", "checked_at"}`.
const HELPER_STATUS: &str = "secrets-status.json";
/// The updater's `starting` counts for this long; after it, a helper that does not answer is unreachable.
const STARTING_FOR_SECS: i64 = 300;

/// The secrets helper's status for Settings.
#[derive(Serialize, utoipa::ToSchema)]
struct HelperStatus {
    /// `off` (the `secrets` profile is not on), `starting` (asked for, or started and not answering yet),
    /// `running` (answers its health check) or `unreachable` (on, but not answering).
    state: String,
    /// `helper_disabled` (off or starting) or `helper_unreachable` (unreachable): what a resolution
    /// gets now; absent when running.
    reason: Option<String>,
    /// The profile is on for this backend (`EUNOMIA_COMPOSE_PROFILES` names `secrets`).
    enabled: bool,
    /// `enable` or `disable` while a request waits for the updater.
    pending: Option<String>,
    /// The updater's request channel is there (`update-status/`): "Enable password managers" works.
    /// When `false`, Settings shows the manual steps instead.
    can_request: bool,
    /// The compose profile to switch on by hand: `secrets`.
    profile: String,
    /// The helper's version when running.
    version: Option<String>,
    /// Why, for people (the updater's error, or why the helper is not answering).
    message: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct HelperRequest {
    enabled: bool,
}

#[derive(Serialize, utoipa::ToSchema)]
struct HelperAck {
    /// `false`: there is no updater on this server (no `update-status/`); switch the profile on by hand.
    configured: bool,
    requested: bool,
}

fn read_json(path: std::path::PathBuf) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Was the updater's `starting` written in the last [`STARTING_FOR_SECS`]?
fn recently(checked_at: &Value) -> bool {
    checked_at
        .as_str()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .is_some_and(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds() < STARTING_FOR_SECS)
}

#[utoipa::path(
    operation_id = "getSecretsHelper",
    get,
    path = "/api/secrets/helper",
    tag = "secrets",
    summary = "The secrets helper's status: off, starting, running or unreachable",
    responses((status = 200, body = HelperStatus), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn helper_status(State(state): State<AppState>, _user: User) -> Json<HelperStatus> {
    let dir = std::path::PathBuf::from(&state.settings.update_status_dir);
    let can_request = dir.is_dir();
    let pending = read_json(dir.join(HELPER_REQUEST)).map(|r| if r["enabled"] == Value::Bool(true) { "enable" } else { "disable" }.to_string());
    let updater = read_json(dir.join(HELPER_STATUS)).unwrap_or(Value::Null);
    let starting = pending.as_deref() == Some("enable") || (updater["state"] == "starting" && recently(&updater["checked_at"]));
    let updater_error = (updater["state"] == "error").then(|| updater["message"].as_str().unwrap_or_default().to_string()).filter(|m| !m.is_empty());
    let client = secrets::client::Client::from_settings(&state.settings);
    let (st, reason, version, message) = match client.health().await {
        Ok(h) => ("running", None, Some(h.version), None),
        Err(_) if starting => ("starting", Some(if client.enabled() { secrets::client::HELPER_UNREACHABLE } else { secrets::client::HELPER_DISABLED }), None, None),
        Err(u) if client.enabled() => ("unreachable", Some(u.reason), None, Some(updater_error.unwrap_or(u.message))),
        Err(u) => ("off", Some(u.reason), None, updater_error),
    };
    Json(HelperStatus {
        state: st.into(),
        reason: reason.map(String::from),
        enabled: client.enabled(),
        pending,
        can_request,
        profile: secrets::client::PROFILE.into(),
        version,
        message,
    })
}

#[utoipa::path(
    operation_id = "requestSecretsHelper",
    post,
    path = "/api/secrets/helper",
    tag = "secrets",
    summary = "Ask the updater to switch the secrets helper on or off",
    description = "Instance admins, browser session. Writes `update-status/secrets.json`; the updater (within 20 seconds) adds `secrets` to COMPOSE_PROFILES in .env and starts the helper, or removes it and stops the helper, restarting the backend either way, and reports in `update-status/secrets-status.json`.",
    request_body = HelperRequest,
    responses((status = 200, body = HelperAck), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn request_helper(State(state): State<AppState>, user: User, Json(req): Json<HelperRequest>) -> AppResult<Json<HelperAck>> {
    authz::require_session()?;
    super::update::require_admin(&state, &user, "switch password managers on or off").await?;
    let dir = std::path::PathBuf::from(&state.settings.update_status_dir);
    if !dir.is_dir() {
        return Ok(Json(HelperAck { configured: false, requested: false }));
    }
    // one key per line: the updater's shell parser (json_field) relies on it
    let body = serde_json::to_string_pretty(&serde_json::json!({ "enabled": req.enabled })).map_err(|e| AppError::internal(e.to_string()))?;
    std::fs::write(dir.join(HELPER_REQUEST), body).map_err(|e| AppError::internal(e.to_string()))?;
    let action = if req.enabled { "secret.helper.enable" } else { "secret.helper.disable" };
    audit::record_as_caller(&state.control, &user.id, action, &user.org.key(), "requested").await;
    Ok(Json(HelperAck { configured: true, requested: true }))
}
