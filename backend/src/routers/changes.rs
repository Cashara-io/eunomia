//! A vault's change log (`changes.rs`): a page after a cursor, or a WebSocket that sends the backlog
//! after the cursor and then each change as it commits. Both read the vault like any vault route:
//! the gate authenticates (Bearer token or session cookie) and `authorize` checks membership and a
//! token's vault restriction.

use std::time::{Duration, Instant};

use axum::{
    extract::{
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::HeaderMap,
    response::Response,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use surrealdb::types::RecordId;
use tokio::sync::broadcast::error::RecvError;

use crate::authz::{self, Action};
use crate::changes::{self, Change, ChangePage};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::User;
use crate::state::{AppState, OrgState};

/// How often a live follower's credential and membership are checked again.
const RECHECK: Duration = Duration::from_secs(30);

pub fn router() -> Router<AppState> {
    Router::new().route("/vaults/{vault_id}/changes", get(list_changes)).route("/vaults/{vault_id}/changes/ws", get(follow_changes))
}

fn default_limit() -> i64 {
    changes::DEFAULT_LIMIT
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ChangesQuery {
    /// Changes with a seq above this one (0, the default, is the whole log).
    #[serde(default)]
    after: i64,
    /// At most this many (default 100, at most 1000).
    #[serde(default = "default_limit")]
    limit: i64,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct FollowQuery {
    /// Send the backlog after this seq first (0, the default, is the whole log).
    #[serde(default)]
    after: i64,
}

fn parse_vault_id(vault_id: &str) -> AppResult<RecordId> {
    crate::rid::parse(vault_id).map_err(|_| AppError::coded(ErrorCode::VaultNotFound, "Vault not found."))
}

#[utoipa::path(
    operation_id = "listVaultChanges",
    get,
    path = "/api/vaults/{vault_id}/changes",
    tag = "vaults",
    summary = "A vault's change log after a cursor",
    description = "One row per write that changed the vault (no content: fetch the item). `next` is the cursor for the following page.",
    params(("vault_id" = String, Path), ChangesQuery),
    responses((status = 200, body = ChangePage), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_changes(State(state): State<AppState>, user: User, Path(vault_id): Path<String>, Query(q): Query<ChangesQuery>) -> AppResult<Json<ChangePage>> {
    let state = state.org(&user.org).await?;
    let vault = parse_vault_id(&vault_id)?;
    authz::authorize(&state.db, &user.id, Action::ReadMemories, &vault).await?;
    Ok(Json(changes::after(&state.db, &vault, q.after, q.limit).await?))
}

#[utoipa::path(
    operation_id = "followVaultChanges",
    get,
    path = "/api/vaults/{vault_id}/changes/ws",
    tag = "vaults",
    summary = "Follow a vault's change log over a WebSocket",
    description = "Upgrades to a WebSocket that sends each change after `after` as a JSON text message (a `Change`), then every new one as it commits. A browser authenticates with its session cookie from an allowed origin; other clients send a Bearer token.",
    params(("vault_id" = String, Path), FollowQuery),
    responses((status = 101, description = "Switching to the WebSocket protocol"), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn follow_changes(
    State(app): State<AppState>,
    user: User,
    Path(vault_id): Path<String>,
    Query(q): Query<FollowQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> AppResult<Response> {
    // a page on another site must not open a socket on the visitor's session cookie
    if !super::mcp::origin_allowed(&headers, &app.settings.cors_allowed_origins) {
        return Err(AppError::coded(ErrorCode::AuthScope, "this origin may not follow changes with a session cookie"));
    }
    let state = app.org(&user.org).await?;
    let vault = parse_vault_id(&vault_id)?;
    authz::authorize(&state.db, &user.id, Action::ReadMemories, &vault).await?;
    Ok(ws.on_upgrade(move |socket| follow(socket, state, headers, vault, q.after)))
}

/// Why a socket stops: the client left, or its credential or membership is gone.
enum Stop {
    Gone,
    Denied(String),
}

async fn follow(mut socket: WebSocket, state: OrgState, headers: HeaderMap, vault: RecordId, mut last: i64) {
    // subscribe before reading the backlog, so a change committed in between is not missed
    let mut rx = changes::subscribe();
    let mut checked = Instant::now();
    let mut result = catch_up(&mut socket, &state, &vault, &mut last).await;
    while result.is_ok() {
        result = tokio::select! {
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => Err(Stop::Gone),
                _ => Ok(()),
            },
            ev = rx.recv() => match ev {
                Ok(ev) if ev.org != state.db.org() || ev.vault != vault || ev.change.seq <= last => Ok(()),
                Ok(ev) => {
                    let mut allowed = Ok(());
                    if checked.elapsed() > RECHECK {
                        checked = Instant::now();
                        allowed = recheck(&state, &headers, &vault).await;
                    }
                    if allowed.is_err() {
                        allowed
                    } else if ev.change.seq == last + 1 {
                        send(&mut socket, &ev.change, &mut last).await
                    } else {
                        // published out of order, or rows this process never saw: the database has them all
                        catch_up(&mut socket, &state, &vault, &mut last).await
                    }
                }
                Err(RecvError::Lagged(_)) => catch_up(&mut socket, &state, &vault, &mut last).await,
                Err(RecvError::Closed) => Err(Stop::Gone),
            },
        };
    }
    if let Err(Stop::Denied(reason)) = result {
        let _ = socket.send(Message::Close(Some(CloseFrame { code: 1008, reason: reason.into() }))).await;
    }
}

/// The credential still verifies and may still read the vault.
async fn recheck(state: &OrgState, headers: &HeaderMap, vault: &RecordId) -> Result<(), Stop> {
    let authn = match crate::auth::authenticate(&state.app, headers, false).await {
        Ok(Some(a)) => a,
        _ => return Err(Stop::Denied("credential no longer valid".into())),
    };
    let allowed = authz::with_caller(authn.caller, authz::authorize(&state.db, &authn.user.id, Action::ReadMemories, vault)).await;
    allowed.map(|_| ()).map_err(|e| Stop::Denied(e.message))
}

async fn catch_up(socket: &mut WebSocket, state: &OrgState, vault: &RecordId, last: &mut i64) -> Result<(), Stop> {
    loop {
        let page = changes::after(&state.db, vault, *last, changes::MAX_LIMIT).await.map_err(|_| Stop::Gone)?;
        for change in &page.changes {
            send(socket, change, last).await?;
        }
        if (page.changes.len() as i64) < changes::MAX_LIMIT {
            return Ok(());
        }
    }
}

async fn send(socket: &mut WebSocket, change: &Change, last: &mut i64) -> Result<(), Stop> {
    let text = serde_json::to_string(change).map_err(|_| Stop::Gone)?;
    socket.send(Message::Text(text.into())).await.map_err(|_| Stop::Gone)?;
    *last = change.seq;
    Ok(())
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(list_changes, follow_changes))]
pub struct Doc;
