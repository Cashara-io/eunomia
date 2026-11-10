//! Peer vaults (`peers/`, docs/peer-vaults.md). Two kinds of route:
//!
//! * For the UI and agents, behind the gate like every vault route: share codes, the list of other
//!   Eunomias a vault is shared with, joining, "Sync now", verify, stop sharing.
//! * For another Eunomia, under `/api/peer/v1/{org}/...`: public (the gate limits them per client
//!   address, like webhooks), and answering nothing of substance without proof of the share secret
//!   (join) or the link key (everything after).

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use surrealdb::types::RecordId;
use crate::rid::RecordIdExt;

use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::User;
use crate::peers;
use crate::pool::OrgId;
use crate::state::{AppState, OrgState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/vaults/{vault_id}/share-codes", post(create_share_code))
        .route("/vaults/{vault_id}/share-codes/{code_id}", delete(delete_share_code))
        .route("/vaults/{vault_id}/peers", get(list_peers))
        .route("/peers/join", post(join_shared))
        .route("/peers/links/{link_id}", delete(stop_sharing))
        .route("/peers/links/{link_id}/sync", post(sync_now))
        .route("/peers/links/{link_id}/verify", get(verify_link))
        .route("/peer/v1/{org}/hello", get(peer_hello))
        .route("/peer/v1/{org}/join", post(peer_join))
        .route("/peer/v1/{org}/links/{link_id}", post(peer_link))
}

fn parse_vault_id(vault_id: &str) -> AppResult<RecordId> {
    crate::rid::parse(vault_id).map_err(|_| AppError::coded(ErrorCode::VaultNotFound, "Vault not found."))
}

fn default_role() -> String {
    peers::ROLE_READ.to_string()
}

#[derive(Deserialize, utoipa::ToSchema)]
struct ShareCodeRequest {
    /// `read` by default. Explicit `edit` lets the other Eunomia add and change.
    #[serde(default = "default_role")]
    #[schema(default = "read")]
    role: String,
    /// How long the code works, in hours (default 168, at most 720).
    #[serde(default)]
    expires_in_hours: Option<i64>,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct JoinRequestBody {
    /// The share code, `eunomia1:...`.
    code: String,
    /// How the other Eunomia will name this one (default: `EUNOMIA_INSTANCE_NAME`, else your email).
    #[serde(default)]
    name: Option<String>,
}

/// What this instance tells the other side it is called.
fn my_name(user: &User, given: Option<&str>) -> String {
    let given = given.map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
    given.or_else(|| Some(peers::instance_name()).filter(|n| !n.is_empty())).unwrap_or_else(|| user.email.clone())
}

/// This instance's addresses, plus the private address the browser opened the app on (the
/// frontend forwards it as `X-Forwarded-Host`), so sharing works without `EUNOMIA_LAN_ADDRS`.
fn my_addrs(state: &OrgState, headers: &HeaderMap) -> Vec<String> {
    let browser = ["x-forwarded-host", "host"]
        .iter()
        .filter_map(|h| headers.get(*h).and_then(|v| v.to_str().ok()))
        .find_map(peers::addrs::browser_lan_addr);
    peers::addrs::advertised_with(&state.settings.public_url, browser)
}

#[utoipa::path(
    operation_id = "createVaultShareCode",
    post,
    path = "/api/vaults/{vault_id}/share-codes",
    tag = "vaults",
    summary = "Make a one-time code to share a vault with another Eunomia (owner only)",
    description = "The code carries this instance's addresses (LAN first), a share id and a secret. It works once and expires (default 7 days). Only the secret's ciphertext is kept here.",
    params(("vault_id" = String, Path)),
    request_body = ShareCodeRequest,
    responses((status = 200, body = peers::ShareCodeOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn create_share_code(State(state): State<AppState>, user: User, headers: HeaderMap, Path(vault_id): Path<String>, Json(body): Json<ShareCodeRequest>) -> AppResult<Json<peers::ShareCodeOut>> {
    let state = state.org(&user.org).await?;
    let vault = parse_vault_id(&vault_id)?;
    let addrs = my_addrs(&state, &headers);
    let res = peers::create_share(&state, &user.id, &vault, &body.role, body.expires_in_hours, addrs).await;
    audit(&state, &user, "peer.share_code", &space_of(&state, &vault).await, &format!("role={}", body.role), &res).await;
    Ok(Json(res?))
}

/// The space's name for the ledger (its id when it can't be read).
async fn space_of(_state: &OrgState, vault: &RecordId) -> String {
    // by id: the audit log reads the space's current name, and never stores what a caller typed
    vault.to_string()
}

/// One ledger row per sharing change: these copy data to, or stop copying it to, another server.
async fn audit<T>(state: &OrgState, user: &User, action: &str, space: &str, detail: &str, res: &AppResult<T>) {
    let outcome = match res {
        Ok(_) => "ok",
        Err(e) => e.code.as_str(),
    };
    // refused access: never name the other side's space or what the attempt carried
    // refused: name nothing; any other failure keeps its detail but not the space id the caller sent
    let (space, detail) = if crate::vaults::service::refused_access(res) {
        (crate::vaults::service::HIDDEN_SPACE, "")
    } else if res.is_err() {
        ("", detail)
    } else {
        (space, detail)
    };
    crate::audit::record_as_caller_detail(&state.control, &user.id, action, space, outcome, detail).await;
}

#[utoipa::path(
    operation_id = "deleteVaultShareCode",
    delete,
    path = "/api/vaults/{vault_id}/share-codes/{code_id}",
    tag = "vaults",
    summary = "Withdraw a share code that has not been used (owner only)",
    params(("vault_id" = String, Path), ("code_id" = String, Path)),
    responses((status = 200, body = crate::openapi::DeletedBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn delete_share_code(State(state): State<AppState>, user: User, Path((vault_id, code_id)): Path<(String, String)>) -> AppResult<Json<crate::openapi::DeletedBody>> {
    let state = state.org(&user.org).await?;
    let vault = parse_vault_id(&vault_id)?;
    let res = peers::delete_share(&state, &user.id, &vault, &code_id).await;
    if !matches!(res, Ok(false)) {
        audit(&state, &user, "peer.share_code_withdraw", &space_of(&state, &vault).await, "", &res).await;
    }
    Ok(Json(crate::openapi::DeletedBody { deleted: res? }))
}

#[utoipa::path(
    operation_id = "listVaultPeers",
    get,
    path = "/api/vaults/{vault_id}/peers",
    tag = "vaults",
    summary = "The other Eunomias a vault is shared with, their sync status, and unused share codes",
    params(("vault_id" = String, Path)),
    responses((status = 200, body = peers::PeersOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_peers(State(state): State<AppState>, user: User, headers: HeaderMap, Path(vault_id): Path<String>) -> AppResult<Json<peers::PeersOut>> {
    let state = state.org(&user.org).await?;
    let vault = parse_vault_id(&vault_id)?;
    let addrs = my_addrs(&state, &headers);
    Ok(Json(peers::list(&state, &user.id, &vault, addrs).await?))
}

#[utoipa::path(
    operation_id = "joinSharedVault",
    post,
    path = "/api/peers/join",
    tag = "vaults",
    summary = "Join a vault another Eunomia shared, with its share code",
    description = "Finds the first address in the code that answers as the right instance (LAN first), proves knowledge of the code's secret, and makes a new vault holding this Eunomia's copy. The first sync runs before this returns.",
    request_body = JoinRequestBody,
    responses((status = 200, body = peers::JoinOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn join_shared(State(state): State<AppState>, user: User, headers: HeaderMap, Json(body): Json<JoinRequestBody>) -> AppResult<Json<peers::JoinOut>> {
    let state = state.org(&user.org).await?;
    let addrs = my_addrs(&state, &headers);
    let name = my_name(&user, body.name.as_deref());
    let res = peers::join(&state, &user.id, &body.code, addrs, &name).await;
    let (space, detail) = match &res {
        Ok(out) => (out.vault.id.to_string(), format!("role={} from={}", out.link.role, out.link.peer_name)),
        // a failed join names nothing: the code came from the caller, and its space isn't theirs
        Err(_) => (String::new(), String::new()),
    };
    audit(&state, &user, "peer.join", &space, &detail, &res).await;
    Ok(Json(res?))
}

#[utoipa::path(
    operation_id = "stopVaultPeer",
    delete,
    path = "/api/peers/links/{link_id}",
    tag = "vaults",
    summary = "Stop sharing with one other Eunomia (owner only); both sides keep their copy",
    params(("link_id" = String, Path)),
    responses((status = 200, body = crate::openapi::DeletedBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn stop_sharing(State(state): State<AppState>, user: User, Path(link_id): Path<String>) -> AppResult<Json<crate::openapi::DeletedBody>> {
    let state = state.org(&user.org).await?;
    let link = if peers::valid_key(&link_id) { peers::load_link(&state.db, &link_id).await.ok().flatten() } else { None };
    let res = peers::stop(&state, &user.id, &link_id).await;
    if let Some(link) = link {
        let peer = if link.peer_name.trim().is_empty() { "another Eunomia" } else { link.peer_name.trim() };
        audit(&state, &user, "peer.stop", &space_of(&state, &link.vault).await, &format!("from={peer}"), &res).await;
    }
    res?;
    Ok(Json(crate::openapi::DeletedBody { deleted: true }))
}

#[utoipa::path(
    operation_id = "syncVaultPeer",
    post,
    path = "/api/peers/links/{link_id}/sync",
    tag = "vaults",
    summary = "Sync a shared vault with the other Eunomia now",
    params(("link_id" = String, Path)),
    responses((status = 200, body = peers::PeerOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn sync_now(State(state): State<AppState>, user: User, Path(link_id): Path<String>) -> AppResult<Json<peers::PeerOut>> {
    let state = state.org(&user.org).await?;
    Ok(Json(peers::sync_now(&state, &user.id, &link_id).await?))
}

#[utoipa::path(
    operation_id = "verifyVaultPeer",
    get,
    path = "/api/peers/links/{link_id}/verify",
    tag = "vaults",
    summary = "Check that both copies of a shared vault hold exactly the same items",
    params(("link_id" = String, Path)),
    responses((status = 200, body = peers::VerifyOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn verify_link(State(state): State<AppState>, user: User, Path(link_id): Path<String>) -> AppResult<Json<peers::VerifyOut>> {
    let state = state.org(&user.org).await?;
    Ok(Json(peers::verify(&state, &user.id, &link_id).await?))
}

// --- for another Eunomia ------------------------------------------------------------------------

/// The org a peer route names, if it is served here.
async fn peer_org(state: &AppState, org: &str) -> Option<OrgState> {
    let org = OrgId::parse(org)?;
    state.org(&org).await.ok()
}

fn refuse(status: u16, reason: &str) -> Response {
    (StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST), Json(json!({ "error": reason }))).into_response()
}

#[utoipa::path(
    operation_id = "peerHello",
    get,
    path = "/api/peer/v1/{org}/hello",
    tag = "peer",
    summary = "For another Eunomia: which instance answers at this address",
    params(("org" = String, Path)),
    responses((status = 200, body = Object), (status = 404, description = "No such org here", body = Object)),
    security(()),
)]
async fn peer_hello(State(state): State<AppState>, Path(org): Path<String>) -> Response {
    let Some(st) = peer_org(&state, &org).await else { return refuse(404, "unknown") };
    match peers::instance_id(&st.db).await {
        Ok(id) => Json(json!({ "v": 1, "instance_id": id, "name": peers::instance_name() })).into_response(),
        Err(_) => refuse(503, "try again"),
    }
}

#[utoipa::path(
    operation_id = "peerJoin",
    post,
    path = "/api/peer/v1/{org}/join",
    tag = "peer",
    summary = "For another Eunomia: redeem a share code (the request carries a MAC keyed by the code's secret)",
    params(("org" = String, Path)),
    request_body(content = Object, description = "share, guest_instance, guest_org, guest_name, guest_addrs, nonce_g, mac"),
    responses((status = 200, body = Object), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(()),
)]
async fn peer_join(State(state): State<AppState>, Path(org): Path<String>, body: Result<Json<Value>, axum::extract::rejection::JsonRejection>) -> Response {
    let Some(st) = peer_org(&state, &org).await else { return refuse(404, "unknown") };
    let body = body.map(|Json(value)| value).unwrap_or(Value::Null);
    match peers::accept_join_json(&st, body).await {
        Ok(out) => Json(out).into_response(),
        Err(e) => e.into_response(),
    }
}

#[utoipa::path(
    operation_id = "peerExchange",
    post,
    path = "/api/peer/v1/{org}/links/{link_id}",
    tag = "peer",
    summary = "For another Eunomia: one sealed request on a link (exchange, verify, leave), one sealed answer",
    description = "Body `{link, ts, nonce, data}`: `data` is AES-256-GCM under the link key, with `POST|path|link|ts|nonce` as associated data. Refused outside a 5 minute clock window or for a nonce already seen.",
    params(("org" = String, Path), ("link_id" = String, Path)),
    request_body(content = Object, description = "{link, ts, nonce, data}"),
    responses((status = 200, body = Object), (status = 401, description = "Not authenticated", body = Object)),
    security(()),
)]
async fn peer_link(State(state): State<AppState>, Path((org, link_id)): Path<(String, String)>, Json(body): Json<Value>) -> Response {
    let Some(st) = peer_org(&state, &org).await else { return refuse(401, "not authenticated") };
    // the guest is not a user here: what the link writes is the link's (`peers::sync::apply`)
    match crate::authz::as_system(peers::sync::host_rpc(&st, &org, &link_id, &body)).await {
        Ok(v) => Json(v).into_response(),
        Err((status, reason)) => refuse(status, reason),
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(create_share_code, delete_share_code, list_peers, join_shared, stop_sharing, sync_now, verify_link, peer_hello, peer_join, peer_link))]
pub struct Doc;
