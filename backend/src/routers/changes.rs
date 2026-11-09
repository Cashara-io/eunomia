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
use surrealdb::types::{RecordId, SurrealValue};

use crate::rid::RecordIdExt;
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
    Router::new()
        .route("/vaults/{vault_id}/changes", get(list_changes))
        .route("/vaults/{vault_id}/changes/ws", get(follow_changes))
        .route("/events/ws", get(follow_events))
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

// --- The per-user live stream (#132) --------------------------------------------------------------
// One socket per browser tab instead of one per vault: what changed in any vault the user can read
// (the change log, `change` messages) and what happened to the vaults themselves (`vault` messages,
// `changes::announce`), so an open page refetches only what an agent, another tab or another member
// just changed. Messages are JSON text with a `type`:
//
// - `hello` {vaults: {vault id: last seq}}: first, and after every reconnect. A client that remembers
//   an older seq for a vault missed changes there and refetches that vault's views (catch-up without
//   replaying the log); one that was connected before refetches its vault list (vault events are not
//   kept).
// - `change` {vault, seq, op, kind, id, actor, at}: a change-log row, as it commits in this process.
// - `seq` {vault, seq}: the log moved past what this socket sent (another replica, a missed
//   broadcast), found by the periodic check; refetch that vault's views.
// - `vault` {vault, op, actor}: something happened to the vault itself (`changes::VAULT_*` ops).
// - `vaults`: the set of vaults the user can read changed without an event this process saw.
// - `resync`: this socket fell behind the in-process bus; refetch everything.
// - `heartbeat` {at}: every few seconds, so a client can tell a quiet socket from a dead one (a
//   browser never sees WebSocket pings).

/// How often the per-user stream sends a heartbeat and re-reads the vaults' counters.
const EVENTS_TICK: Duration = Duration::from_secs(10);

#[utoipa::path(
    operation_id = "followEvents",
    get,
    path = "/api/events/ws",
    tag = "vaults",
    summary = "Follow everything that changes in the caller's vaults over one WebSocket",
    params(OntologyResume),
    description = "Upgrades to a WebSocket that sends JSON text messages with a `type`: `hello` (each vault's last change seq, first and on every reconnect), `change` (a change-log row of any vault the caller can read), `seq` (a vault's log moved on; refetch it), `vault` (a vault was created, renamed, deleted, cloned, merged, or its members changed), `vaults` (the caller's vault set changed), `resync` (refetch everything) and `heartbeat`. No content: fetch what changed. A browser authenticates with its session cookie from an allowed origin; other clients send a Bearer token.",
    responses((status = 101, description = "Switching to the WebSocket protocol"), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn follow_events(State(app): State<AppState>, user: User, Query(args): Query<OntologyResume>, headers: HeaderMap, ws: WebSocketUpgrade) -> AppResult<Response> {
    if !super::mcp::origin_allowed(&headers, &app.settings.cors_allowed_origins) {
        return Err(AppError::coded(ErrorCode::AuthScope, "this origin may not follow changes with a session cookie"));
    }
    authz::require_scope(Action::ReadMemories.scope())?;
    let state = app.org(&user.org).await?;
    let resume: std::collections::HashMap<String, i64> = match args.ontology_after {
        Some(raw) if raw.len() <= 32000 => serde_json::from_str(&raw).map_err(|_| AppError::bad_request("invalid ontology cursor"))?,
        None => Default::default(),
        _ => return Err(AppError::bad_request("ontology cursor too large")),
    };
    if resume.len() > 500 || resume.values().any(|n| *n < 0) { return Err(AppError::bad_request("invalid ontology cursor")); }
    Ok(ws.on_upgrade(move |socket| follow_user(socket, state, headers, user.id, resume)))
}

/// The vaults the credential in `headers` may read now, as a sorted list of ids; `Denied` once it no
/// longer verifies (signed out, token revoked, scope gone).
async fn readable_vaults(state: &OrgState, headers: &HeaderMap) -> Result<Vec<RecordId>, Stop> {
    let authn = match crate::auth::authenticate(&state.app, headers, false).await {
        Ok(Some(a)) => a,
        _ => return Err(Stop::Denied("credential no longer valid".into())),
    };
    let user = authn.user.id.clone();
    let vaults = authz::with_caller(authn.caller, async {
        authz::require_scope(Action::ReadMemories.scope())?;
        crate::vaults::service::accessible_vault_ids(&state.db, &user).await
    })
    .await;
    match vaults {
        Ok(mut v) => {
            v.sort_by_key(|a| a.to_string());
            v.dedup();
            Ok(v)
        }
        Err(e) if e.code == ErrorCode::AuthScope || e.code == ErrorCode::AuthUnauthorized => Err(Stop::Denied(e.message)),
        Err(_) => Err(Stop::Gone),
    }
}

/// Each vault's last seq (0 for one with no changes yet), by vault id.
async fn counters(state: &OrgState, vaults: &[RecordId]) -> Result<std::collections::HashMap<String, i64>, Stop> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        id: RecordId,
        n: i64,
    }
    let ids: Vec<RecordId> = vaults.iter().map(changes::counter).collect();
    let rows: Vec<Row> = crate::store::changes::COUNTERS
        .on(&state.db)
        .bind(("ids", ids))
        .await
        .and_then(|mut r| r.take(0))
        .map_err(|_| Stop::Gone)?;
    let mut out: std::collections::HashMap<String, i64> = vaults.iter().map(|v| (v.to_string(), 0)).collect();
    for row in rows {
        out.insert(RecordId::from_table_key("vault", row.id.key().clone()).to_string(), row.n);
    }
    Ok(out)
}

async fn send_json(socket: &mut WebSocket, value: serde_json::Value) -> Result<(), Stop> {
    socket.send(Message::Text(value.to_string().into())).await.map_err(|_| Stop::Gone)
}

async fn follow_user(mut socket: WebSocket, state: OrgState, headers: HeaderMap, user: RecordId, mut ontology_last: std::collections::HashMap<String, i64>) {
    use serde_json::json;
    // subscribe before reading anything, so an event committed in between is not missed
    let mut rx = changes::subscribe();
    let mut vrx = changes::subscribe_vaults();
    let mut checked = Instant::now();
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + EVENTS_TICK, EVENTS_TICK);

    let start = async {
        let vaults = readable_vaults(&state, &headers).await?;
        let last = counters(&state, &vaults).await?;
        let mut ontology_heads = std::collections::HashMap::new();
        if state.db.schema_version() >= 41 {
            for vault in &vaults {
                let head = crate::ontology::activity::head(&state.db, vault).await.map_err(|_| Stop::Gone)?;
                let cursor = ontology_last.entry(vault.to_string()).or_insert(head);
                if *cursor > head { *cursor = 0; }
                ontology_heads.insert(vault.to_string(), *cursor);
            }
        }
        ontology_last.retain(|v, _| vaults.iter().any(|id| id.to_string() == *v));
        send_json(&mut socket, json!({"type": "hello", "vaults": last, "ontology": ontology_heads})).await?;
        Ok::<_, Stop>((vaults, last))
    }
    .await;
    let (mut vaults, mut last) = match start {
        Ok(v) => v,
        Err(stop) => return close(socket, stop).await,
    };

    if let Err(stop) = ontology_catch_up(&mut socket, &state, &headers, &mut ontology_last, None).await {
        return close(socket, stop).await;
    }
    let result: Result<(), Stop> = loop {
        // A socket opened during the N-1 window holds its original OrgDb handle.
        // Reconnect once the pool sees 0041, so ontology delivery cannot stay off
        // forever behind an otherwise healthy heartbeat connection.
        if state.db.schema_version() < 41
            && let Ok(db) = state.app.pool.for_org(&state.db.org()).await
            && db.schema_version() >= 41
        {
            break Err(Stop::Resync);
        }
        let step = tokio::select! {
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => Err(Stop::Gone),
                _ => Ok(()),
            },
            _ = tick.tick() => async {
                if checked.elapsed() >= recheck_every() {
                    checked = Instant::now();
                    let now = readable_vaults(&state, &headers).await?;
                    if now != vaults {
                        vaults = now;
                        send_json(&mut socket, json!({"type": "vaults"})).await?;
                    }
                }
                socket.send(Message::Ping(Default::default())).await.map_err(|_| Stop::Gone)?;
                send_json(&mut socket, json!({"type": "heartbeat", "at": chrono::Utc::now().to_rfc3339()})).await?;
                // rows another replica wrote, or a broadcast this socket missed
                for (vault, seq) in counters(&state, &vaults).await? {
                    let known = last.get(&vault).copied().unwrap_or(0);
                    if seq > known {
                        send_json(&mut socket, json!({"type": "seq", "vault": vault, "seq": seq})).await?;
                    }
                    last.insert(vault, seq);
                }
                ontology_catch_up(&mut socket, &state, &headers, &mut ontology_last, None).await
            }.await,
            ev = rx.recv() => match ev {
                Ok(ev) if ev.org != state.db.org() || !vaults.contains(&ev.vault) => Ok(()),
                Ok(ev) => async {
                    recheck(&state, &headers, &ev.vault).await?;
                    let vault = ev.vault.to_string();
                    let known = last.entry(vault.clone()).or_insert(0);
                    if ev.change.seq > *known {
                        *known = ev.change.seq;
                    }
                    let mut msg = serde_json::to_value(&ev.change).map_err(|_| Stop::Gone)?;
                    msg["type"] = json!("change");
                    msg["vault"] = json!(vault);
                    send_json(&mut socket, msg).await
                }.await,
                Err(RecvError::Lagged(_)) => send_json(&mut socket, json!({"type": "resync"})).await,
                Err(RecvError::Closed) => Err(Stop::Gone),
            },
            ev = vrx.recv() => match ev {
                Ok(ev) if ev.org != state.db.org() || !(ev.users.contains(&user) || vaults.contains(&ev.vault)) => Ok(()),
                Ok(ev) if ev.op == changes::ONTOLOGY_COMMITTED => ontology_catch_up(&mut socket, &state, &headers, &mut ontology_last, Some(&ev.vault)).await,
                Ok(ev) => async {
                    send_json(&mut socket, json!({"type": "vault", "vault": ev.vault.to_string(), "op": ev.op, "actor": ev.actor})).await?;
                    // membership may have changed: follow the vaults the user can read now
                    checked = Instant::now();
                    vaults = readable_vaults(&state, &headers).await?;
                    for (vault, seq) in counters(&state, &vaults).await? {
                        last.entry(vault).or_insert(seq);
                    }
                    Ok::<(), Stop>(())
                }.await,
                Err(RecvError::Lagged(_)) => send_json(&mut socket, json!({"type": "resync"})).await,
                Err(RecvError::Closed) => Err(Stop::Gone),
            },
        };
        if let Err(stop) = step {
            break Err(stop);
        }
    };
    if let Err(stop) = result {
        close(socket, stop).await;
    }
}

async fn close(mut socket: WebSocket, stop: Stop) {
    if let Stop::Denied(reason) = stop {
        let _ = socket.send(Message::Close(Some(CloseFrame { code: 1008, reason: reason.into() }))).await;
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(list_changes, follow_changes, follow_events))]
pub struct Doc;

#[derive(Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct OntologyResume { ontology_after: Option<String> }

async fn ontology_catch_up(
    socket: &mut WebSocket, state: &OrgState, headers: &HeaderMap,
    last: &mut std::collections::HashMap<String, i64>, only: Option<&RecordId>,
) -> Result<(), Stop> {
    if state.db.schema_version() < 41 { return Ok(()); }
    let allowed = readable_vaults(state, headers).await?;
    last.retain(|v, _| allowed.iter().any(|id| id.to_string() == *v));
    for vault in allowed.iter().filter(|v| only.is_none_or(|id| id == *v)) {
        let cursor = last.entry(vault.to_string()).or_insert(0);
        loop {
            let args = crate::ontology::activity::ActivityQuery { after: *cursor, limit: Some(200), ..Default::default() };
            let page = crate::ontology::activity::page(&state.db, vault, &args).await.map_err(|_| Stop::Gone)?;
            for event in page.events {
                recheck(state, headers, vault).await?;
                send_json(socket, serde_json::json!({"type":"ontology","vault":vault.to_string(),"event":event})).await?;
                *cursor = event.seq;
            }
            if !page.more { break; }
        }
    }
    Ok(())
}
