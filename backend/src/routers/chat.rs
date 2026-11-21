//! Chat REST surface: multiple named threads per user, each backed by
//! `chat::service`'s streaming OpenAI function-calling loop over the shared
//! tool registry. Sending a message returns a Server-Sent Events stream
//! (see `chat::service::ChatEvent` for the event shapes) rather than a
//! single JSON response, so the frontend can render the reply incrementally.
//!

use std::convert::Infallible;

use axum::{
    extract::{Path, State},
    response::sse::{Event, KeepAlive, Sse},
    routing::{delete, get},
    Json, Router,
};
use futures::Stream;
use serde::Deserialize;
use serde_json::{json, Value};
use surrealdb::types::RecordId;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt as _;

use crate::chat::service::{self, ChatEvent};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::User;
use crate::state::AppState;

/// Buffered events in flight between the agent-loop task and the SSE
/// stream -- generous enough that a burst of text deltas never blocks the
/// loop on a slow client.
const EVENT_CHANNEL_CAPACITY: usize = 64;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/chat/threads", get(list_threads).post(create_thread))
        .route("/chat/threads/{thread_id}", delete(delete_thread_route).post(send_message))
        .route("/chat/threads/{thread_id}/history", get(thread_history))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct ChatRequest {
    message: String,
}

#[derive(Deserialize, Default, utoipa::ToSchema)]
struct ThreadCreate {
    #[serde(default)]
    title: Option<String>,
}

/// Path params are plain strings; parse here, same convention as
/// `routers/vaults.rs`'s `parse_vault_id`.
fn parse_thread_id(thread_id: &str) -> AppResult<RecordId> {
    crate::rid::parse(thread_id).map_err(|_| AppError::coded(ErrorCode::ChatThreadNotFound, "not found"))
}

#[utoipa::path(
    operation_id = "listThreads",
    get,
    path = "/api/chat/threads",
    tag = "chat",
    summary = "List chat threads",
    responses((status = 200, body = Vec<crate::chat::service::ThreadOut>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_threads(State(state): State<AppState>, user: User) -> AppResult<Json<Vec<service::ThreadOut>>> {
    let state = state.org(&user.org).await?;
    Ok(Json(service::list_threads(&state.db, &user.id).await?))
}

#[utoipa::path(
    operation_id = "createThread",
    post,
    path = "/api/chat/threads",
    tag = "chat",
    summary = "Create a chat thread",
    request_body = ThreadCreate,
    responses((status = 200, body = crate::chat::service::ThreadOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn create_thread(
    State(state): State<AppState>,
    user: User,
    Json(body): Json<ThreadCreate>,
) -> AppResult<Json<service::ThreadOut>> {
    let state = state.org(&user.org).await?;
    Ok(Json(service::create_thread(&state.db, &user.id, body.title.as_deref()).await?))
}

#[utoipa::path(
    operation_id = "deleteThread",
    delete,
    path = "/api/chat/threads/{thread_id}",
    tag = "chat",
    summary = "Delete a chat thread",
    params(("thread_id" = String, Path)),
    responses((status = 200, body = crate::openapi::OkBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn delete_thread_route(
    State(state): State<AppState>,
    user: User,
    Path(thread_id): Path<String>,
) -> AppResult<Json<Value>> {
    let state = state.org(&user.org).await?;
    let rid = parse_thread_id(&thread_id)?;
    let ok = service::delete_thread(&state.db, &user.id, &rid).await?;
    if !ok {
        return Err(AppError::coded(ErrorCode::ChatThreadNotFound, "not found"));
    }
    Ok(Json(json!({ "ok": true })))
}

#[utoipa::path(
    operation_id = "getThreadHistory",
    get,
    path = "/api/chat/threads/{thread_id}/history",
    tag = "chat",
    summary = "Messages in a thread",
    params(("thread_id" = String, Path)),
    responses((status = 200, body = Vec<crate::chat::service::MessageOut>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn thread_history(
    State(state): State<AppState>,
    user: User,
    Path(thread_id): Path<String>,
) -> AppResult<Json<Vec<service::MessageOut>>> {
    let state = state.org(&user.org).await?;
    let rid = parse_thread_id(&thread_id)?;
    let hist = service::history(&state.db, &user.id, &rid).await?;
    hist.map(Json).ok_or_else(|| AppError::coded(ErrorCode::ChatThreadNotFound, "not found"))
}

#[utoipa::path(
    operation_id = "sendMessage",
    post,
    path = "/api/chat/threads/{thread_id}",
    tag = "chat",
    summary = "Send a message, streamed back as server-sent events",
    params(("thread_id" = String, Path)),
    request_body = ChatRequest,
    responses((status = 200, description = "Server-sent events", content_type = "text/event-stream", body = String), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn send_message(
    State(state): State<AppState>,
    user: User,
    Path(thread_id): Path<String>,
    Json(body): Json<ChatRequest>,
) -> AppResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let state = state.org(&user.org).await?;
    let rid = parse_thread_id(&thread_id)?;
    if service::get_thread(&state.db, &user.id, &rid).await?.is_none() {
        return Err(AppError::coded(ErrorCode::ChatThreadNotFound, "not found"));
    }
    service::ensure_configured(&state.db, &state.settings, &user.id).await?;

    let (tx, rx) = mpsc::channel::<ChatEvent>(EVENT_CHANNEL_CAPACITY);
    let state_for_task = state.clone();
    // a spawned task starts with no task-locals: carry the credential (scope and vault
    // restriction for the model's tool calls) and the trace id across
    let caller = crate::authz::caller()?;
    let trace_id = crate::telemetry::current_trace_id();
    tokio::spawn(crate::authz::with_caller(
        caller,
        crate::telemetry::with_trace_id(trace_id, service::send_stream(state_for_task, user, rid, body.message, tx)),
    ));

    let stream = ReceiverStream::new(rx).map(|event| {
        let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string());
        Ok(Event::default().data(data))
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    list_threads,
    create_thread,
    delete_thread_route,
    send_message,
    thread_history,
))]
pub struct Doc;
