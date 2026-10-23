//! MCP server over the Streamable HTTP transport, at `/mcp`.
//!
//! Stateless: every POST carries one JSON-RPC message (or a batch, for
//! 2025-03-26 clients) and gets a plain `application/json` reply -- no
//! sessions and no server-initiated stream, so GET/DELETE are 405. Exposes
//! the same tool registry the chat agent uses (`tools::registry`), so every
//! call is owner-scoped and audit-logged exactly like REST/chat calls. The user's memory skill goes
//! out as the `initialize` instructions, the `eunomia-memory` prompt and the `eunomia://skill` resource.
//!
//! Auth is a personal API token or an OAuth access token (`Authorization: Bearer ...`), never the
//! browser session cookie -- a page the user visits can't drive tools. A batch is capped at
//! [`MAX_BATCH`] messages and charged to the rate limit once per message.

use crate::rid::RecordIdExt;
use axum::{
    extract::{Extension, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};

use crate::error::{AppError, ErrorCode};
use crate::auth::Authn;
use crate::models_user::User;
use crate::state::AppState;
use crate::tools::registry;

/// Largest JSON-RPC batch accepted in one request.
const MAX_BATCH: usize = 32;
const SUPPORTED_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];
const DEFAULT_VERSION: &str = "2025-06-18";

const INSTRUCTIONS: &str = "Eunomia is the user's long-term memory: entities (people, organisations, \
locations, code) with remembered facts, plus records synced from their connected sources. \
Use `recall` before answering questions about the user's world, `search`/`get` for raw synced records, \
and `memory_write` to remember new facts. You are the model: if the server has no OpenAI key, `reflect` \
returns the recalled memories for you to answer from (cite them by index) instead of a synthesized answer. \
Writes go to the user's personal vault unless a `vault_id` is given; `recall`, `reflect` and `entities_search` \
cover the personal vault and every org vault the user belongs to unless one is.";

/// The memory skill is also served as a prompt and a resource: `instructions` reach only the
/// clients that read them, and Claude Code offers MCP prompts as slash commands.
const SKILL_PROMPT: &str = "eunomia-memory";
const SKILL_URI: &str = "eunomia://skill";
const SKILL_DESCRIPTION: &str = "The user's Eunomia memory skill: when to recall and what to remember.";

pub fn router() -> Router<AppState> {
    Router::new().route("/mcp", post(handle).get(method_not_allowed).delete(method_not_allowed))
}

async fn method_not_allowed() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")]).into_response()
}

async fn handle(State(state): State<AppState>, authn: Option<Extension<Authn>>, meter: Option<Extension<crate::gate::Meter>>, headers: HeaderMap, body: String) -> Response {
    if !origin_allowed(&headers, &state.settings.cors_allowed_origins) {
        return AppError::coded(ErrorCode::AuthForbidden, "origin not allowed").into_response();
    }
    if let Some(v) = headers.get("mcp-protocol-version").and_then(|v| v.to_str().ok())
        && !SUPPORTED_VERSIONS.contains(&v) {
            return AppError::bad_request(format!("unsupported MCP-Protocol-Version {v}")).into_response();
        }
    let Some((user, granted)) = gate_user(authn) else {
        let presented = headers.contains_key(header::AUTHORIZATION);
        let err = AppError::unauthorized("missing or invalid access token: connect with OAuth or create a personal API token on the Eunomia dashboard");
        return ([(header::WWW_AUTHENTICATE, crate::oauth::www_authenticate(&state.settings, presented))], err).into_response();
    };

    let Ok(message) = serde_json::from_str::<Value>(&body) else {
        return (StatusCode::BAD_REQUEST, Json(error_response(Value::Null, -32700, ErrorCode::ValidationInvalid, "Parse error"))).into_response();
    };

    if let Value::Array(batch) = &message {
        if batch.len() > MAX_BATCH {
            let msg = format!("Batch too large: at most {MAX_BATCH} messages per request.");
            return (StatusCode::BAD_REQUEST, Json(error_response(Value::Null, -32600, ErrorCode::ValidationInvalid, &msg))).into_response();
        }
        // the gate charged one request; a batch costs one per message
        if let Some(Extension(m)) = &meter
            && let Err(wait) = m.charge(batch.len().saturating_sub(1))
        {
            return crate::gate::rate_limited(wait);
        }
    }

    // OAuth tokens carry scopes; a personal API token is limited by its own scopes (the gate and the registry check them).
    if let Some(granted) = &granted
        && let Some(denied) = crate::oauth::scope_challenge(&state.settings, &message, granted)
    {
        return denied;
    }

    // tool calls in this request are counted under the client's MCP session, when it sent one
    let session = crate::usage::session_from_header(headers.get("mcp-session-id").and_then(|v| v.to_str().ok()));
    crate::usage::with_context(crate::usage::Channel::Mcp, session, respond(&state, &user, &message)).await
}

/// Handles the (already validated) message or batch and builds the HTTP reply.
async fn respond(state: &AppState, user: &User, message: &Value) -> Response {
    let replies: Vec<Value> = match message {
        Value::Array(batch) if !batch.is_empty() => {
            let mut out = Vec::new();
            for m in batch {
                if let Some(r) = handle_message(state, user, m).await {
                    out.push(r);
                }
            }
            if out.is_empty() {
                return StatusCode::ACCEPTED.into_response();
            }
            return Json(Value::Array(out)).into_response();
        }
        m => handle_message(state, user, m).await.into_iter().collect(),
    };
    match replies.into_iter().next() {
        Some(reply) => Json(reply).into_response(),
        // Notifications and client responses get no body.
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// The caller the gate already authenticated, plus the scopes an OAuth token was limited to
/// (`None` for a personal API token). No second token lookup.
fn gate_user(authn: Option<Extension<Authn>>) -> Option<(User, Option<Vec<String>>)> {
    let Extension(a) = authn?;
    let granted = (a.caller.actor.kind == "oauth").then(|| a.caller.scopes.clone());
    crate::telemetry::record_user(&a.user.id.to_string());
    Some((a.user, granted))
}

/// DNS-rebinding guard. The MCP transport spec says servers MUST validate
/// `Origin`; the attack it prevents is a web page borrowing a victim's ambient
/// authority (cookies, or being on a trusted network) through a rebound DNS name.
/// `/mcp` is bearer-only, and a page cannot know the token, so a request that
/// presents `Authorization` and no `Cookie` has no ambient authority to abuse and
/// any origin is fine (this is what lets browser clients such as MCP Inspector
/// connect). Any request that carries a cookie, or no bearer, must come from no
/// browser at all, the same origin (directly or via the frontend's proxy, which
/// forwards the original host) or an allowed CORS origin.
fn origin_allowed(headers: &HeaderMap, cors_allowed_origins: &str) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    if headers.contains_key(header::AUTHORIZATION) && !headers.contains_key(header::COOKIE) {
        return true;
    }
    if cors_allowed_origins.split(',').map(str::trim).any(|o| o == origin) {
        return true;
    }
    let origin_host = origin.split_once("://").map(|(_, h)| h).unwrap_or(origin);
    ["x-forwarded-host", "host"]
        .iter()
        .filter_map(|h| headers.get(*h).and_then(|v| v.to_str().ok()))
        .any(|host| host == origin_host)
}

/// One JSON-RPC message in, `Some(response)` for a request, `None` for a
/// notification or a response from the client.
async fn handle_message(state: &AppState, user: &User, message: &Value) -> Option<Value> {
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        // A client response (result/error) needs no reply; anything else is malformed.
        if message.get("result").is_some() || message.get("error").is_some() {
            return None;
        }
        return Some(error_response(message.get("id").cloned().unwrap_or(Value::Null), -32600, ErrorCode::ValidationInvalid, "Invalid Request"));
    };
    let id = message.get("id").cloned()?; // notification: nothing to send back
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    Some(match method {
        "initialize" => {
            // the user's memory skill rides along, so edits in the UI reach every client on connect
            let skill = user_skill(state, user).await.unwrap_or_default();
            result_response(id, initialize_result(&params, &skill))
        }
        "ping" => result_response(id, json!({})),
        "tools/list" => result_response(id, json!({ "tools": tool_list() })),
        "tools/call" => call_tool(state, user, id, &params).await,
        "prompts/list" => {
            let prompts: Vec<Value> = may_read_skill()
                .then(|| json!({ "name": SKILL_PROMPT, "title": "Eunomia memory", "description": SKILL_DESCRIPTION, "arguments": [] }))
                .into_iter()
                .collect();
            result_response(id, json!({ "prompts": prompts }))
        }
        "resources/list" => {
            let resources: Vec<Value> = may_read_skill()
                .then(|| json!({ "uri": SKILL_URI, "name": SKILL_PROMPT, "title": "Eunomia memory skill", "description": SKILL_DESCRIPTION, "mimeType": "text/markdown" }))
                .into_iter()
                .collect();
            result_response(id, json!({ "resources": resources }))
        }
        "prompts/get" | "resources/read" => {
            let (key, wanted, unknown) = if method == "prompts/get" { ("name", SKILL_PROMPT, -32602) } else { ("uri", SKILL_URI, -32002) };
            let asked = params.get(key).and_then(Value::as_str).unwrap_or_default();
            if asked != wanted {
                return Some(error_response(id, unknown, ErrorCode::ResourceNotFound, &format!("Unknown {key}: {asked}")));
            }
            if !may_read_skill() {
                return Some(error_response(id, -32602, ErrorCode::AuthScope, "this token lacks the memory:read scope"));
            }
            let Some(skill) = user_skill(state, user).await else {
                return Some(error_response(id, -32603, ErrorCode::Internal, "could not load the memory skill"));
            };
            let body = crate::docs::skill_body(&skill);
            result_response(id, if method == "prompts/get" { prompt_result(body) } else { resource_result(body) })
        }
        _ => error_response(id, -32601, ErrorCode::ValidationInvalid, &format!("Method not found: {method}")),
    })
}

/// The user's memory skill (their own, or the built-in one). `None` if it could not be loaded.
async fn user_skill(state: &AppState, user: &User) -> Option<String> {
    let org = state.org(&user.org).await.ok()?;
    crate::routers::settings::memory_skill(&org.db, &user.id).await.ok()
}

/// Reading the skill needs `memory:read`, like `recall`; a credential without it is offered nothing.
fn may_read_skill() -> bool {
    crate::authz::current().is_some_and(|c| c.allows(crate::scopes::MEMORY_READ))
}

fn prompt_result(body: &str) -> Value {
    json!({ "description": SKILL_DESCRIPTION, "messages": [{ "role": "user", "content": { "type": "text", "text": body } }] })
}

fn resource_result(body: &str) -> Value {
    json!({ "contents": [{ "uri": SKILL_URI, "mimeType": "text/markdown", "text": body }] })
}

fn negotiate_version(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|r| SUPPORTED_VERSIONS.iter().find(|v| **v == r).copied())
        .unwrap_or(DEFAULT_VERSION)
}

fn initialize_result(params: &Value, skill: &str) -> Value {
    let body = crate::docs::skill_body(skill);
    let instructions = if body.is_empty() { INSTRUCTIONS.to_string() } else { format!("{INSTRUCTIONS}\n\n{body}") };
    json!({
        "protocolVersion": negotiate_version(params.get("protocolVersion").and_then(Value::as_str)),
        "capabilities": { "tools": { "listChanged": false }, "prompts": { "listChanged": false }, "resources": { "listChanged": false } },
        "serverInfo": { "name": "eunomia", "title": "Eunomia", "version": crate::config::APP_VERSION },
        "instructions": instructions,
    })
}

/// Only the tools the credential's scopes allow (OAuth token or PAT alike); calls are refused
/// either way, but a client should not be offered what it cannot use. No credential lists none.
fn tool_list() -> Vec<Value> {
    let caller = crate::authz::current();
    let mut names: Vec<&&str> = registry::all_tools().keys().collect();
    names.sort();
    names
        .into_iter()
        .filter(|name| caller.as_ref().is_some_and(|c| c.allows(crate::scopes::for_tool(name, registry::is_read_only(name)))))
        .map(|name| {
            let spec = &registry::all_tools()[*name];
            let schema = if spec.schema.is_object() { spec.schema.clone() } else { json!({ "type": "object", "properties": {} }) };
            json!({
                "name": name,
                "description": registry::description(name),
                "inputSchema": schema,
                "annotations": {
                    "readOnlyHint": spec.read_only,
                    "destructiveHint": registry::is_destructive(name),
                    "openWorldHint": false,
                },
            })
        })
        .collect()
}

async fn call_tool(state: &AppState, user: &User, id: Value, params: &Value) -> Value {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return error_response(id, -32602, ErrorCode::ValidationInvalid, "tools/call needs a tool `name`");
    };
    if !registry::all_tools().contains_key(name) {
        return error_response(id, -32602, ErrorCode::ToolNotFound, &format!("Unknown tool: {name}"));
    }
    let args = match params.get("arguments") {
        None | Some(Value::Null) => json!({}),
        Some(v) => v.clone(),
    };

    // Tool failures are results the model should see (isError), not
    // protocol errors. The value carries `error`, `code` and `trace_id`.
    let (value, is_error) = match registry::call(state, user, name, args).await {
        Ok(v) => {
            let is_error = v.get("error").is_some();
            (v, is_error)
        }
        Err(e) => (e.to_tool_value(), true),
    };
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    result_response(id, json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
}

fn result_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// JSON-RPC protocol error; `data` carries the same stable `code` and `trace_id` as REST errors.
fn error_response(id: Value, code: i64, app_code: ErrorCode, message: &str) -> Value {
    let data = json!({ "code": app_code.as_str(), "trace_id": crate::telemetry::current_trace_id() });
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message, "data": data } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn negotiates_supported_versions_and_falls_back() {
        assert_eq!(negotiate_version(Some("2025-03-26")), "2025-03-26");
        assert_eq!(negotiate_version(Some("2025-11-25")), "2025-11-25");
        assert_eq!(negotiate_version(Some("1999-01-01")), DEFAULT_VERSION);
        assert_eq!(negotiate_version(None), DEFAULT_VERSION);
    }

    #[tokio::test]
    async fn tool_list_exposes_every_tool_with_schema_and_annotations() {
        let tools = crate::authz::as_system(async { tool_list() }).await;
        assert_eq!(tools.len(), registry::all_tools().len());
        for t in &tools {
            assert!(!t["description"].as_str().unwrap().is_empty());
            assert_eq!(t["inputSchema"]["type"], "object");
            assert!(t["annotations"]["readOnlyHint"].is_boolean());
        }
        let delete = tools.iter().find(|t| t["name"] == "vault_delete").unwrap();
        assert_eq!(delete["annotations"]["destructiveHint"], true);
        let search = tools.iter().find(|t| t["name"] == "search").unwrap();
        assert_eq!(search["annotations"]["readOnlyHint"], true);
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_static(v));
        }
        h
    }

    #[test]
    fn origin_check_allows_non_browsers_same_origin_and_cors_list() {
        let cors = "http://localhost:3000";
        assert!(origin_allowed(&headers(&[]), cors));
        assert!(origin_allowed(&headers(&[("origin", "http://localhost:3000")]), cors));
        assert!(origin_allowed(&headers(&[("origin", "http://10.0.0.5:3000"), ("x-forwarded-host", "10.0.0.5:3000")]), cors));
        assert!(origin_allowed(&headers(&[("origin", "http://localhost:8001"), ("host", "localhost:8001")]), cors));
        assert!(!origin_allowed(&headers(&[("origin", "https://evil.example"), ("host", "localhost:8001")]), cors));
        // bearer-only browser clients (MCP Inspector) from any origin; a cookie brings the check back
        assert!(origin_allowed(&headers(&[("origin", "https://inspector.example"), ("authorization", "Bearer x")]), cors));
        assert!(!origin_allowed(&headers(&[("origin", "https://evil.example"), ("authorization", "Bearer x"), ("cookie", "a=b")]), cors));
    }

    #[test]
    fn initialize_advertises_tools_and_echoes_version() {
        let r = initialize_result(&json!({ "protocolVersion": "2025-03-26" }), crate::docs::DEFAULT_SKILL);
        assert!(r["instructions"].as_str().unwrap().contains("# Eunomia memory"));
        assert!(!r["instructions"].as_str().unwrap().contains("name: eunomia-memory"));
        assert_eq!(r["protocolVersion"], "2025-03-26");
        assert!(r["capabilities"]["tools"].is_object());
        assert!(r["capabilities"]["prompts"].is_object());
        assert!(r["capabilities"]["resources"].is_object());
        assert_eq!(r["serverInfo"]["name"], "eunomia");
        // reads are no longer described as personal-vault only
        assert!(r["instructions"].as_str().unwrap().contains("every org vault"));
    }

    #[test]
    fn the_skill_prompt_and_resource_carry_the_skill_body() {
        let body = crate::docs::skill_body(crate::docs::DEFAULT_SKILL);
        let p = prompt_result(body);
        assert_eq!(p["messages"][0]["role"], "user");
        assert!(p["messages"][0]["content"]["text"].as_str().unwrap().starts_with("# Eunomia memory"));
        let r = resource_result(body);
        assert_eq!(r["contents"][0]["uri"], SKILL_URI);
        assert!(r["contents"][0]["text"].as_str().unwrap().contains("Before you finish"));
    }

    #[tokio::test]
    async fn prompts_and_resources_are_listed_only_with_memory_read() {
        let list = |method: &'static str| rpc(json!({ "jsonrpc": "2.0", "id": 1, "method": method }));
        let p = crate::authz::as_system(list("prompts/list")).await.unwrap();
        assert_eq!(p["result"]["prompts"][0]["name"], SKILL_PROMPT);
        let r = crate::authz::as_system(list("resources/list")).await.unwrap();
        assert_eq!(r["result"]["resources"][0]["uri"], SKILL_URI);
        // no credential in scope (as for a token without memory:read): nothing offered, and get is refused
        assert_eq!(list("prompts/list").await.unwrap()["result"]["prompts"], json!([]));
        assert_eq!(list("resources/list").await.unwrap()["result"]["resources"], json!([]));
        let get = rpc(json!({ "jsonrpc": "2.0", "id": 1, "method": "prompts/get", "params": { "name": SKILL_PROMPT } })).await.unwrap();
        assert_eq!(get["error"]["data"]["code"], "auth.scope");
        // unknown names: invalid params for a prompt, resource-not-found for a resource
        let code = |r: Option<Value>| r.unwrap()["error"]["code"].as_i64().unwrap();
        let bad_prompt = json!({ "jsonrpc": "2.0", "id": 1, "method": "prompts/get", "params": { "name": "nope" } });
        assert_eq!(code(crate::authz::as_system(rpc(bad_prompt)).await), -32602);
        let bad_uri = json!({ "jsonrpc": "2.0", "id": 1, "method": "resources/read", "params": { "uri": "eunomia://nope" } });
        assert_eq!(code(crate::authz::as_system(rpc(bad_uri)).await), -32002);
    }

    // -- protocol-level tests: no database needed, `Surreal::init()` is an
    // unconnected handle, and none of these paths query it. --------------

    async fn test_state() -> AppState {
        use crate::config::Settings;
        let settings = Settings {
            jwt_secret: "t".into(),
            surreal_url: "mem://".into(),
            surreal_user: "root".into(),
            surreal_pass: "root".into(),
            surreal_ns: "mcp_unit".into(),
            surreal_db: "legacy".into(),
            openai_api_key: None,
            openai_base_url: "https://api.openai.com/v1".into(),
            openai_chat_model: String::new(),
            encryption_key: "mcp-unit-test-encryption-key".into(),
            embeddings_backend: "stub".into(),
            cors_allowed_origins: "http://localhost:3000".into(),
            log_level: "INFO".into(),
            update_status_dir: String::new(),
            bind_addr: String::new(),
            public_url: "http://localhost:8001".into(),
            documents_backend: String::new(),
            documents_max_bytes: 1 << 20,
        };
        AppState::build(&settings, surrealdb::opt::Config::new()).await.unwrap()
    }

    fn user() -> User {
        User { id: crate::rid::parse("user:abc").unwrap(), email: "a@example.com".into(), org: crate::pool::OrgId::new() }
    }

    async fn rpc(message: Value) -> Option<Value> {
        handle_message(&test_state().await, &user(), &message).await
    }

    #[tokio::test]
    async fn initialize_tells_the_agent_it_is_the_model() {
        let r = rpc(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} })).await.unwrap();
        let instructions = r["result"]["instructions"].as_str().unwrap();
        assert!(instructions.contains("You are the model"));
        assert!(instructions.contains("`recall`"));
        assert_eq!(r["id"], 1);
    }

    #[tokio::test]
    async fn ping_tools_list_and_notifications() {
        assert_eq!(rpc(json!({ "jsonrpc": "2.0", "id": 7, "method": "ping" })).await.unwrap()["result"], json!({}));
        assert!(rpc(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await.is_none());
        assert!(rpc(json!({ "jsonrpc": "2.0", "id": 1, "result": {} })).await.is_none());

        let list = crate::authz::as_system(rpc(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }))).await.unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        for expected in ["recall", "reflect", "search", "memory_write", "entities_search", "vault_list"] {
            assert!(names.contains(&expected), "tools/list is missing {expected}");
        }
    }

    #[tokio::test]
    async fn protocol_errors_use_the_right_codes() {
        let code = |r: Option<Value>| r.unwrap()["error"]["code"].as_i64().unwrap();
        assert_eq!(code(rpc(json!({ "jsonrpc": "2.0", "id": 1, "method": "completion/complete" })).await), -32601);
        assert_eq!(code(rpc(json!({ "jsonrpc": "2.0", "id": 1 })).await), -32600);
        assert_eq!(code(rpc(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {} })).await), -32602);
        let unknown = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "nope" } });
        assert_eq!(code(rpc(unknown).await), -32602);
    }

    async fn post(headers: &[(&str, &str)], method: axum::http::Method, body: &str) -> StatusCode {
        use tower::ServiceExt;
        let mut req = axum::http::Request::builder().method(method).uri("/mcp");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let app = router().with_state(test_state().await);
        app.oneshot(req.body(axum::body::Body::from(body.to_string())).unwrap()).await.unwrap().status()
    }

    #[tokio::test]
    async fn http_layer_rejects_unauthenticated_and_malformed_requests() {
        use axum::http::Method;
        let rpc_body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        // no token / a token that can't be verified -> 401, never tool output
        assert_eq!(post(&[], Method::POST, rpc_body).await, StatusCode::UNAUTHORIZED);
        assert_eq!(post(&[("authorization", "Bearer nope")], Method::POST, rpc_body).await, StatusCode::UNAUTHORIZED);
        // the session cookie is not an accepted credential on /mcp
        assert_eq!(post(&[("cookie", "eunomia_session=x")], Method::POST, rpc_body).await, StatusCode::UNAUTHORIZED);
        // stateless server: no GET stream, no DELETE session
        assert_eq!(post(&[], Method::GET, "").await, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(post(&[], Method::DELETE, "").await, StatusCode::METHOD_NOT_ALLOWED);
        // DNS-rebinding guard and protocol-version check come before auth
        assert_eq!(
            post(&[("origin", "https://evil.example"), ("host", "localhost:8001")], Method::POST, rpc_body).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(post(&[("mcp-protocol-version", "1999-01-01")], Method::POST, rpc_body).await, StatusCode::BAD_REQUEST);
    }
}
