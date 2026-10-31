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
/// How often a live follower is pinged and catches up from the database, whatever the broadcast says.
const TICK: Duration = Duration::from_secs(5);
/// The close code for a cursor the log can no longer continue from (`changes.resync`; 4000 to 4999 are
/// the application's own): re-read from `after=0`.
pub const CLOSE_RESYNC: u16 = 4410;

/// Test-only: when above 0, [`RECHECK`] in seconds, so a test sees an idle socket closed in seconds.
#[cfg(feature = "test-support")]
pub static TEST_RECHECK_SECS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn recheck_every() -> Duration {
    #[cfg(feature = "test-support")]
    if let n @ 1.. = TEST_RECHECK_SECS.load(std::sync::atomic::Ordering::Relaxed) {
        return Duration::from_secs(n);
    }
    RECHECK
}

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
    description = "One row per write that changed the vault (no content: fetch the item). `next` is the cursor for the following page. `after=0` reads from the oldest row kept; a cursor below what retention keeps, or past the newest change, is 410 `changes.resync`.",
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
    description = "Upgrades to a WebSocket that sends each change after `after` as a JSON text message (a `Change`), then every new one as it commits. A browser authenticates with its session cookie from an allowed origin; other clients send a Bearer token. A cursor outside the kept log is refused with 410 `changes.resync` before the upgrade; a socket that falls behind what is kept closes with code 4410.",
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
    // a cursor outside the log is refused before the upgrade, as over REST (410 changes.resync)
    changes::after(&state.db, &vault, q.after, 1).await?;
    Ok(ws.on_upgrade(move |socket| follow(socket, state, headers, vault, q.after)))
}

/// Why a socket stops: the client left, its credential or membership is gone, or retention removed
/// rows it had not sent yet.
enum Stop {
    Gone,
    Denied(String),
    Resync,
}

async fn follow(mut socket: WebSocket, state: OrgState, headers: HeaderMap, vault: RecordId, mut last: i64) {
    // subscribe before reading the backlog, so a change committed in between is not missed
    let mut rx = changes::subscribe();
    let mut checked = Instant::now();
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + TICK, TICK);
    let mut result = catch_up(&mut socket, &state, &vault, &mut last).await;
    while result.is_ok() {
        result = tokio::select! {
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => Err(Stop::Gone),
                _ => Ok(()),
            },
            // even when nothing arrives: recheck the caller, keep the connection alive, and read rows a
            // missed broadcast or another replica wrote
            _ = tick.tick() => async {
                if checked.elapsed() >= recheck_every() {
                    checked = Instant::now();
                    recheck(&state, &headers, &vault).await?;
                }
                socket.send(Message::Ping(Default::default())).await.map_err(|_| Stop::Gone)?;
                catch_up(&mut socket, &state, &vault, &mut last).await
            }.await,
            ev = rx.recv() => match ev {
                Ok(ev) if ev.org != state.db.org() || ev.vault != vault || ev.change.seq <= last => Ok(()),
                other => async {
                    if checked.elapsed() >= recheck_every() {
                        checked = Instant::now();
                        recheck(&state, &headers, &vault).await?;
                    }
                    match other {
                        Ok(ev) if ev.change.seq == last + 1 => send(&mut socket, &ev.change, &mut last).await,
                        // published out of order, lagged, or rows this process never saw: the database has them all
                        Ok(_) | Err(RecvError::Lagged(_)) => catch_up(&mut socket, &state, &vault, &mut last).await,
                        Err(RecvError::Closed) => Err(Stop::Gone),
                    }
                }.await,
            },
        };
    }
    match result {
        Err(Stop::Denied(reason)) => {
            let _ = socket.send(Message::Close(Some(CloseFrame { code: 1008, reason: reason.into() }))).await;
        }
        Err(Stop::Resync) => {
            let _ = socket.send(Message::Close(Some(CloseFrame { code: CLOSE_RESYNC, reason: ErrorCode::ChangesResync.as_str().into() }))).await;
        }
        _ => {}
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
        let page = changes::after(&state.db, vault, *last, changes::MAX_LIMIT)
            .await
            .map_err(|e| if e.code == ErrorCode::ChangesResync { Stop::Resync } else { Stop::Gone })?;
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
