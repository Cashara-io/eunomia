//! Extensions, phase 1 (docs/extensions.md): the registry (`/api/extensions`), the consumer API that
//! claims and acks buffered events (`/api/extensions/{id}/events/...`), and the public inbox
//! (`POST /hooks/ext/{key}`, [`hook_router`]).
//!
//! The inbox is deliberately a *separate* router with no `User` extractor: the sender holds the
//! extension's secret, not a session or API token. Its URL key names the owner (the same reason a
//! source webhook carries the owner id), which is how the org database is found. A delivery to an
//! unknown extension, a malformed key and a wrong secret all answer the same 401.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use surrealdb::types::{Datetime, RecordId};

use crate::audit;
use crate::authz;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::extensions::{self as svc, Extension, Stats};
use crate::models_user::User;
use crate::rid::RecordIdExt;
use crate::state::{AppState, OrgState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/extensions", get(list_extensions).post(create_extension))
        .route("/extensions/{id}", get(get_extension).patch(update_extension).delete(delete_extension))
        .route("/extensions/{id}/rotate-secret", post(rotate_secret))
        .route("/extensions/{id}/events", get(list_events))
        .route("/extensions/{id}/events/claim", post(claim_events))
        .route("/extensions/{id}/events/ack", post(ack_events))
        .route("/extensions/{id}/events/nack", post(nack_events))
}

/// No `User` extractor: see this module's doc comment.
pub fn hook_router() -> Router<AppState> {
    Router::new().route("/hooks/ext/{key}", post(receive))
}

// ---------------------------------------------------------------- shapes

#[derive(Serialize, utoipa::ToSchema)]
struct ExtensionOut {
    /// `extension:...`
    id: String,
    name: String,
    description: String,
    /// The vault the script is expected to write to; `null` is your personal vault. Informational:
    /// the script passes it as `vault_id` to the tools.
    vault_id: Option<String>,
    enabled: bool,
    /// Where senders POST: `{PUBLIC_URL}/hooks/ext/{key}`.
    webhook_url: String,
    /// The last four characters of the current secret.
    secret_hint: String,
    #[schema(value_type = String)]
    secret_rotated_at: Datetime,
    #[schema(value_type = String)]
    created_at: Datetime,
    #[schema(value_type = String)]
    updated_at: Datetime,
    /// Waiting to be claimed, counting events whose lease ran out.
    pending: i64,
    /// Leased to a consumer right now.
    claimed: i64,
    /// Acked, until they are pruned (7 days).
    done: i64,
    /// Handed out five times without an ack, or nacked at the last attempt (kept 30 days).
    dead: i64,
    #[schema(value_type = Option<String>)]
    last_delivery_at: Option<Datetime>,
}

fn extension_out(state: &AppState, e: &Extension, stats: Option<&Stats>, last: Option<Datetime>) -> ExtensionOut {
    let s = stats.cloned().unwrap_or_default();
    ExtensionOut {
        id: e.id.to_string(),
        name: e.name.clone(),
        description: e.description.clone(),
        vault_id: e.vault.as_ref().map(|v| v.to_string()),
        enabled: e.enabled,
        webhook_url: format!("{}/hooks/ext/{}", state.settings.public_url, e.hook_key()),
        secret_hint: e.secret_hint.clone(),
        secret_rotated_at: e.secret_rotated_at,
        created_at: e.created_at,
        updated_at: e.updated_at,
        pending: s.pending,
        claimed: s.claimed,
        done: s.done,
        dead: s.dead,
        last_delivery_at: last,
    }
}

/// The only response that carries a secret in the clear.
#[derive(Serialize, utoipa::ToSchema)]
struct ExtensionWithSecret {
    extension: ExtensionOut,
    /// Shown once. Send it as `Authorization: Bearer <secret>`, or sign the body with it
    /// (`X-Eunomia-Signature: sha256=<hex HMAC-SHA256>`). Rotate to get a new one.
    secret: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct CreateBody {
    /// 1 to 80 characters.
    name: String,
    #[serde(default)]
    description: Option<String>,
    /// A vault you belong to; omit for your personal vault.
    #[serde(default)]
    vault_id: Option<String>,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct PatchBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    /// A vault you belong to; an empty string goes back to your personal vault; omit to keep it.
    #[serde(default)]
    vault_id: Option<String>,
    /// `false` makes the inbox answer 403 `extension.disabled`; buffered events stay.
    #[serde(default)]
    enabled: Option<bool>,
}

// ---------------------------------------------------------------- registry

async fn org(state: &AppState, user: &User) -> AppResult<OrgState> {
    state.org(&user.org).await
}

async fn vault_of(state: &OrgState, user: &User, v: Option<&str>) -> AppResult<Option<RecordId>> {
    let Some(v) = v.map(str::trim).filter(|v| !v.is_empty()) else { return Ok(None) };
    let rid: RecordId = crate::rid::parse(v)
        .ok()
        .filter(|r: &RecordId| r.table() == "vault")
        .ok_or_else(|| AppError::coded(ErrorCode::VaultNotFound, "Vault not found."))?;
    authz::ensure_member(&state.db, &user.id, &rid).await?;
    Ok(Some(rid))
}

fn ext_id(user: &User, id: &str) -> AppResult<RecordId> {
    svc::parse_id(id, &user.id).ok_or_else(|| AppError::coded(ErrorCode::ExtensionNotFound, "Extension not found."))
}

#[utoipa::path(
    operation_id = "listExtensions",
    get,
    path = "/api/extensions",
    tag = "extensions",
    summary = "List your extensions with their inbox counts",
    responses((status = 200, body = Vec<ExtensionOut>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_extensions(State(state): State<AppState>, user: User) -> AppResult<Json<Vec<ExtensionOut>>> {
    let org = org(&state, &user).await?;
    let rows = svc::list(&org.db, &user.id).await?;
    let stats = svc::stats(&org.db, &user.id).await?;
    let mut out = Vec::with_capacity(rows.len());
    for e in &rows {
        let last = svc::last_delivery(&org.db, &user.id, &e.id).await?;
        out.push(extension_out(&state, e, stats.get(&e.id.to_string()), last));
    }
    Ok(Json(out))
}

#[utoipa::path(
    operation_id = "createExtension",
    post,
    path = "/api/extensions",
    tag = "extensions",
    summary = "Create an extension and its inbox",
    description = "The response carries the inbox secret once; it cannot be read again, only rotated.",
    request_body = CreateBody,
    responses((status = 200, body = ExtensionWithSecret), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn create_extension(State(state): State<AppState>, user: User, Json(body): Json<CreateBody>) -> AppResult<Json<ExtensionWithSecret>> {
    let org = org(&state, &user).await?;
    let vault = vault_of(&org, &user, body.vault_id.as_deref()).await?;
    let (ext, secret) =
        svc::create(&org.db, &state.settings.encryption_key, &user.id, &body.name, body.description.as_deref().unwrap_or(""), vault).await?;
    audit::record_as_caller(&state.control, &user.id, "extension.create", &ext.id.to_string(), "ok").await;
    Ok(Json(ExtensionWithSecret { extension: extension_out(&state, &ext, None, None), secret }))
}

#[utoipa::path(
    operation_id = "getExtension",
    get,
    path = "/api/extensions/{id}",
    tag = "extensions",
    summary = "One extension",
    params(("id" = String, Path)),
    responses((status = 200, body = ExtensionOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_extension(State(state): State<AppState>, user: User, Path(id): Path<String>) -> AppResult<Json<ExtensionOut>> {
    let org = org(&state, &user).await?;
    let ext = svc::get(&org.db, &user.id, &ext_id(&user, &id)?).await?;
    let stats = svc::stats(&org.db, &user.id).await?;
    let last = svc::last_delivery(&org.db, &user.id, &ext.id).await?;
    Ok(Json(extension_out(&state, &ext, stats.get(&ext.id.to_string()), last)))
}

#[utoipa::path(
    operation_id = "updateExtension",
    patch,
    path = "/api/extensions/{id}",
    tag = "extensions",
    summary = "Rename, redescribe, retarget, enable or disable an extension",
    params(("id" = String, Path)),
    request_body = PatchBody,
    responses((status = 200, body = ExtensionOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn update_extension(State(state): State<AppState>, user: User, Path(id): Path<String>, Json(body): Json<PatchBody>) -> AppResult<Json<ExtensionOut>> {
    let org = org(&state, &user).await?;
    let current = svc::get(&org.db, &user.id, &ext_id(&user, &id)?).await?;
    let vault = match body.vault_id.as_deref() {
        None => current.vault.clone(),
        Some(v) => vault_of(&org, &user, Some(v)).await?,
    };
    let ext = svc::update(
        &org.db,
        &user.id,
        &current,
        body.name.as_deref().unwrap_or(&current.name),
        body.description.as_deref().unwrap_or(&current.description),
        vault,
        body.enabled.unwrap_or(current.enabled),
    )
    .await?;
    audit::record_as_caller(&state.control, &user.id, "extension.update", &ext.id.to_string(), "ok").await;
    let stats = svc::stats(&org.db, &user.id).await?;
    let last = svc::last_delivery(&org.db, &user.id, &ext.id).await?;
    Ok(Json(extension_out(&state, &ext, stats.get(&ext.id.to_string()), last)))
}

#[utoipa::path(
    operation_id = "deleteExtension",
    delete,
    path = "/api/extensions/{id}",
    tag = "extensions",
    summary = "Delete an extension and every event in its inbox",
    params(("id" = String, Path)),
    responses((status = 200, body = crate::openapi::OkBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn delete_extension(State(state): State<AppState>, user: User, Path(id): Path<String>) -> AppResult<Json<serde_json::Value>> {
    let org = org(&state, &user).await?;
    let id = ext_id(&user, &id)?;
    svc::delete(&org.db, &user.id, &id).await?;
    audit::record_as_caller(&state.control, &user.id, "extension.delete", &id.to_string(), "ok").await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[utoipa::path(
    operation_id = "rotateExtensionSecret",
    post,
    path = "/api/extensions/{id}/rotate-secret",
    tag = "extensions",
    summary = "Issue a new inbox secret; the old one stops working at once",
    params(("id" = String, Path)),
    responses((status = 200, body = ExtensionWithSecret), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn rotate_secret(State(state): State<AppState>, user: User, Path(id): Path<String>) -> AppResult<Json<ExtensionWithSecret>> {
    let org = org(&state, &user).await?;
    let id = ext_id(&user, &id)?;
    let (ext, secret) = svc::rotate(&org.db, &state.settings.encryption_key, &user.id, &id).await?;
    audit::record_as_caller(&state.control, &user.id, "extension.rotate_secret", &id.to_string(), "ok").await;
    let stats = svc::stats(&org.db, &user.id).await?;
    let last = svc::last_delivery(&org.db, &user.id, &ext.id).await?;
    Ok(Json(ExtensionWithSecret { extension: extension_out(&state, &ext, stats.get(&ext.id.to_string()), last), secret }))
}

// ---------------------------------------------------------------- consumer API

/// A claimed event, for the consumer.
#[derive(Serialize, utoipa::ToSchema)]
struct EventOut {
    /// `extension_event:...`; pass it back to ack or nack.
    id: String,
    /// The sender's delivery id (`X-Delivery-Id`, `X-GitHub-Delivery` or `Idempotency-Key`), or one we made up.
    delivery_id: String,
    #[schema(value_type = String)]
    received_at: Datetime,
    /// How many times this event has been handed out, this claim included.
    attempts: i64,
    /// The delivery's JSON body, as sent.
    #[schema(value_type = Object)]
    body: serde_json::Value,
}

fn event_out(e: svc::Event) -> EventOut {
    let body = serde_json::from_str(&e.body).unwrap_or(serde_json::Value::Null);
    EventOut { id: e.id.to_string(), delivery_id: e.delivery_id, received_at: e.received_at, attempts: e.attempts, body }
}

#[derive(Deserialize, utoipa::ToSchema)]
struct ClaimBody {
    /// 1 to 500; default 100.
    #[serde(default)]
    max: Option<i64>,
    /// How long the batch is yours, 5 to 3600 seconds; default 300. Ack or nack before it runs out,
    /// or the events go to the next claim.
    #[serde(default)]
    lease_secs: Option<i64>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct ClaimOut {
    extension_id: String,
    /// The extension's target vault, to pass as `vault_id` to the tools; `null` is your personal vault.
    vault_id: Option<String>,
    lease_secs: i64,
    /// Oldest first. Empty when nothing is waiting.
    events: Vec<EventOut>,
}

#[utoipa::path(
    operation_id = "claimExtensionEvents",
    post,
    path = "/api/extensions/{id}/events/claim",
    tag = "extensions",
    summary = "Lease a batch of buffered events",
    description = "At-least-once: an event whose lease runs out without an ack is handed out again, up to five times, then it is dead. A batch also stops at about 8 MiB of bodies.",
    params(("id" = String, Path)),
    request_body = ClaimBody,
    responses((status = 200, body = ClaimOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn claim_events(State(state): State<AppState>, user: User, Path(id): Path<String>, Json(body): Json<ClaimBody>) -> AppResult<Json<ClaimOut>> {
    let org = org(&state, &user).await?;
    let ext = svc::get(&org.db, &user.id, &ext_id(&user, &id)?).await?;
    let max = body.max.unwrap_or(100);
    if !(1..=svc::CLAIM_MAX).contains(&max) {
        return Err(AppError::bad_request(format!("max must be between 1 and {}.", svc::CLAIM_MAX)));
    }
    let lease = body.lease_secs.unwrap_or(svc::LEASE_DEFAULT_SECS);
    if !(svc::LEASE_MIN_SECS..=svc::LEASE_MAX_SECS).contains(&lease) {
        return Err(AppError::bad_request(format!("lease_secs must be between {} and {}.", svc::LEASE_MIN_SECS, svc::LEASE_MAX_SECS)));
    }
    let events = svc::claim(&org.db, &ext, max, lease).await?;
    Ok(Json(ClaimOut {
        extension_id: ext.id.to_string(),
        vault_id: ext.vault.as_ref().map(|v| v.to_string()),
        lease_secs: lease,
        events: events.into_iter().map(event_out).collect(),
    }))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct IdsBody {
    /// Event ids from a claim, at most 500.
    ids: Vec<String>,
}

fn event_ids(ids: &[String]) -> AppResult<Vec<RecordId>> {
    if ids.is_empty() || ids.len() as i64 > svc::CLAIM_MAX {
        return Err(AppError::bad_request(format!("Send between 1 and {} event ids.", svc::CLAIM_MAX)));
    }
    ids.iter()
        .map(|s| {
            crate::rid::parse(s)
                .ok()
                .filter(|r: &RecordId| r.table() == "extension_event")
                .ok_or_else(|| AppError::bad_request(format!("{s:?} is not an event id.")))
        })
        .collect()
}

#[derive(Serialize, utoipa::ToSchema)]
struct AckOut {
    /// Ids that were leased and are now done. Ids that are unknown, already done or not leased are skipped.
    acked: usize,
}

#[utoipa::path(
    operation_id = "ackExtensionEvents",
    post,
    path = "/api/extensions/{id}/events/ack",
    tag = "extensions",
    summary = "Mark leased events done",
    params(("id" = String, Path)),
    request_body = IdsBody,
    responses((status = 200, body = AckOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn ack_events(State(state): State<AppState>, user: User, Path(id): Path<String>, Json(body): Json<IdsBody>) -> AppResult<Json<AckOut>> {
    let org = org(&state, &user).await?;
    let ext = svc::get(&org.db, &user.id, &ext_id(&user, &id)?).await?;
    let acked = svc::ack(&org.db, &ext, event_ids(&body.ids)?).await?;
    Ok(Json(AckOut { acked }))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct NackBody {
    ids: Vec<String>,
    /// Why, kept on the event (500 characters).
    #[serde(default)]
    error: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct NackOut {
    /// Back to pending.
    requeued: usize,
    /// Handed out five times already, so dead-lettered instead.
    dead: usize,
}

#[utoipa::path(
    operation_id = "nackExtensionEvents",
    post,
    path = "/api/extensions/{id}/events/nack",
    tag = "extensions",
    summary = "Give leased events back",
    description = "They are claimable again at once; after the fifth attempt they are dead-lettered instead.",
    params(("id" = String, Path)),
    request_body = NackBody,
    responses((status = 200, body = NackOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn nack_events(State(state): State<AppState>, user: User, Path(id): Path<String>, Json(body): Json<NackBody>) -> AppResult<Json<NackOut>> {
    let org = org(&state, &user).await?;
    let ext = svc::get(&org.db, &user.id, &ext_id(&user, &id)?).await?;
    let (requeued, dead) = svc::nack(&org.db, &ext, event_ids(&body.ids)?, body.error).await?;
    Ok(Json(NackOut { requeued, dead }))
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct EventsQuery {
    /// `pending`, `claimed`, `done` or `dead`; all when omitted.
    status: Option<String>,
    /// 1 to 100; default 20.
    limit: Option<i64>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct EventDetail {
    id: String,
    delivery_id: String,
    /// `pending`, `claimed`, `done` or `dead`.
    status: String,
    attempts: i64,
    #[schema(value_type = Option<String>)]
    claimed_until: Option<Datetime>,
    last_error: Option<String>,
    #[schema(value_type = String)]
    received_at: Datetime,
    #[schema(value_type = Object)]
    body: serde_json::Value,
}

#[utoipa::path(
    operation_id = "listExtensionEvents",
    get,
    path = "/api/extensions/{id}/events",
    tag = "extensions",
    summary = "Look at the inbox without leasing anything, newest first",
    params(("id" = String, Path), EventsQuery),
    responses((status = 200, body = Vec<EventDetail>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_events(State(state): State<AppState>, user: User, Path(id): Path<String>, Query(q): Query<EventsQuery>) -> AppResult<Json<Vec<EventDetail>>> {
    let org = org(&state, &user).await?;
    let ext = svc::get(&org.db, &user.id, &ext_id(&user, &id)?).await?;
    if let Some(s) = q.status.as_deref()
        && !["pending", "claimed", "done", "dead"].contains(&s)
    {
        return Err(AppError::bad_request("status must be pending, claimed, done or dead."));
    }
    let rows = svc::events(&org.db, &ext, q.status.as_deref(), q.limit.unwrap_or(20)).await?;
    Ok(Json(
        rows.into_iter()
            .map(|e| EventDetail {
                body: serde_json::from_str(&e.body).unwrap_or(serde_json::Value::Null),
                id: e.id.to_string(),
                delivery_id: e.delivery_id,
                status: e.status,
                attempts: e.attempts,
                claimed_until: e.claimed_until,
                last_error: e.last_error,
                received_at: e.received_at,
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------- the inbox

/// A slow or stalled upload is cut off rather than holding a connection.
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Serialize, utoipa::ToSchema)]
struct HookOut {
    /// `accepted`, or `duplicate` when this delivery id was already stored.
    status: String,
    /// The buffered event's id.
    id: String,
}

fn unauthorized() -> AppError {
    AppError::unauthorized("Invalid extension credentials.")
}

/// A delivery id from the sender's own header, so a retry is recognised; made up when there is none.
fn delivery_id(headers: &HeaderMap) -> AppResult<String> {
    for name in ["x-delivery-id", "x-github-delivery", "idempotency-key"] {
        if let Some(v) = headers.get(name) {
            let v = v.to_str().map(str::trim).unwrap_or("");
            if v.is_empty() || v.len() > 128 || v.chars().any(char::is_control) {
                return Err(AppError::bad_request(format!("{name} must be 1 to 128 printable characters.")));
            }
            return Ok(v.to_string());
        }
    }
    Ok(uuid::Uuid::new_v4().to_string())
}

#[utoipa::path(
    operation_id = "receiveExtensionWebhook",
    post,
    path = "/hooks/ext/{key}",
    tag = "extensions",
    summary = "An extension's inbox (authenticated by the extension's secret)",
    description = "The body is JSON, at most 256 KiB. Prove the secret with `Authorization: Bearer <secret>` or \
        `X-Eunomia-Signature: sha256=<hex HMAC-SHA256 of the raw body keyed with the secret>`. A sender's own \
        `X-Delivery-Id`, `X-GitHub-Delivery` or `Idempotency-Key` makes a retry a no-op. 429 `extension.buffer_full` \
        means the inbox holds its maximum of unprocessed events.",
    params(("key" = String, Path)),
    responses(
        (status = 202, body = HookOut),
        (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json"),
    ),
    security(()),
)]
async fn receive(State(state): State<AppState>, Path(key): Path<String>, request: axum::extract::Request) -> AppResult<(StatusCode, Json<HookOut>)> {
    let (parts, body) = request.into_parts();
    // read the body (bounded) before any database work
    let bytes = match tokio::time::timeout(READ_TIMEOUT, axum::body::to_bytes(body, svc::BODY_MAX_BYTES)).await {
        Err(_) => return Err(AppError::new(StatusCode::REQUEST_TIMEOUT, "body not received in time")),
        Ok(Err(_)) => {
            let mut err = AppError::bad_request(format!("The body is unreadable or over {} bytes.", svc::BODY_MAX_BYTES));
            err.status = StatusCode::PAYLOAD_TOO_LARGE;
            return Err(err);
        }
        Ok(Ok(b)) => b,
    };

    // who: unknown owner, unknown extension, and a wrong secret are indistinguishable from outside
    let (owner, id) = svc::parse_hook_key(&key).ok_or_else(unauthorized)?;
    let org = crate::models_user::org_of(&state.control, &owner).await.map_err(|_| unauthorized())?;
    let org = state.org(&org).await?;
    let ext = svc::get(&org.db, &owner, &id).await.map_err(|_| unauthorized())?;
    let secret = svc::secret_of(&state.settings.encryption_key, &ext)?;
    if !svc::authentic(&secret, &parts.headers, &bytes) {
        tracing::info!(extension = %id.to_string(), "extension webhook: bad credentials");
        return Err(unauthorized());
    }
    if !ext.enabled {
        return Err(AppError::coded(ErrorCode::ExtensionDisabled, "This extension is switched off."));
    }

    let text = std::str::from_utf8(&bytes).map_err(|_| AppError::bad_request("The body must be UTF-8 JSON."))?;
    serde_json::from_str::<serde::de::IgnoredAny>(text).map_err(|e| AppError::bad_request(format!("The body is not valid JSON: {e}")))?;
    let delivery = delivery_id(&parts.headers)?;
    let (status, id) = match svc::deliver(&org.db, &ext, &delivery, text).await? {
        svc::Delivery::Accepted(id) => ("accepted", id),
        svc::Delivery::Duplicate(id) => ("duplicate", id),
    };
    Ok((StatusCode::ACCEPTED, Json(HookOut { status: status.to_string(), id: id.to_string() })))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    list_extensions,
    create_extension,
    get_extension,
    update_extension,
    delete_extension,
    rotate_secret,
    claim_events,
    ack_events,
    nack_events,
    list_events,
    receive,
))]
pub struct Doc;
