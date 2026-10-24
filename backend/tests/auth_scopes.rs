//! Token scopes, vault restriction, expiry, audit events and rate limits,
//! end to end through the real router and an in-memory SurrealDB.

mod common;

use axum::body::Body;
use axum::http::{header, HeaderMap, Request, StatusCode};
use common::{TestApp, PASSWORD};
use eunomia_backend::models_user::create_api_token_with;
use eunomia_backend::ratelimit::RateConfig;
use eunomia_backend::scopes;
use eunomia_backend::vaults::service as vaults;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use eunomia_backend::rid::{self, RecordIdExt};
use surrealdb::types::{Datetime, RecordId};
use tower::ServiceExt;

async fn send(
    router: &axum::Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    headers: &[(&str, String)],
) -> (StatusCode, HeaderMap, Value) {
    let mut req = Request::builder().method(method).uri(path);
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let req = match body {
        Some(b) => req.header(header::CONTENT_TYPE, "application/json").body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    // the socket peer is a trusted proxy (the default trusts loopback only)
    let mut req = req;
    req.extensions_mut().insert(axum::extract::ConnectInfo(std::net::SocketAddr::from(([127, 0, 0, 1], 4000))));
    let resp = router.clone().oneshot(req).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn bearer(token: &str) -> Vec<(&'static str, String)> {
    vec![("authorization", format!("Bearer {token}"))]
}

async fn token(app: &TestApp, granted: &[&str], vault: Option<&RecordId>) -> String {
    let granted: Vec<String> = granted.iter().map(|s| s.to_string()).collect();
    create_api_token_with(&app.state.control, &app.user.id, "t", &granted, vault, None).await.unwrap().token
}

async fn org_vault(app: &TestApp) -> RecordId {
    rid::parse(&common::sys(vaults::create_vault(&app.db().await, &app.user.id, "Team", "org")).await.unwrap().id).unwrap()
}

/// A JSON-RPC `tools/call` over `/mcp`; returns (isError, tool value).
async fn mcp_call(app: &TestApp, token: &str, name: &str, args: Value) -> (bool, Value) {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}});
    let (status, _, v) = send(&app.router, "POST", "/mcp", Some(body), &bearer(token)).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let text = v["result"]["content"][0]["text"].as_str().expect("tool text");
    (v["result"]["isError"].as_bool().unwrap(), serde_json::from_str(text).unwrap())
}

async fn events(app: &TestApp, filter: &str) -> Vec<Value> {
    let mut res = app.state.control.test_raw().query(format!(
        "SELECT action, actor_kind, actor_id, target, outcome, trace_id, detail, (IF user != NONE THEN <string> user ELSE '' END) AS user, <string> created_at AS created_at \
         FROM audit_event WHERE {filter} ORDER BY created_at"
    )).await.unwrap();
    res.take::<Vec<Value>>(0).unwrap()
}

fn all() -> Vec<String> {
    scopes::ALL.iter().map(|s| s.to_string()).collect()
}

// ---------------------------------------------------------------- scopes

#[tokio::test]
async fn read_only_token_reads_but_cannot_write_or_administer() {
    let app = TestApp::new().await;
    let t = token(&app, &[scopes::MEMORY_READ], None).await;
    let h = bearer(&t);

    let (status, _, _) = send(&app.router, "GET", "/api/entities", None, &h).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, body) = send(&app.router, "POST", "/api/entities", Some(json!({"kind": "person", "name": "Ann"})), &h).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.scope")));
    let (status, _, body) = send(&app.router, "POST", "/api/vaults", Some(json!({"name": "X"})), &h).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.scope")));
    for path in ["/api/settings", "/api/auth/tokens", "/api/connectors", "/api/audit", "/api/export"] {
        let (status, _, body) = send(&app.router, "GET", path, None, &h).await;
        assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.scope")), "{path}");
    }
    // the same call with a fully scoped token passes the gate
    let (status, _, _) = send(&app.router, "GET", "/api/settings", None, &bearer(&app.token)).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn connectors_scope_gates_connector_routes() {
    let app = TestApp::new().await;
    let only = token(&app, &[scopes::CONNECTORS], None).await;
    let (status, _, _) = send(&app.router, "GET", "/api/connectors", None, &bearer(&only)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = send(&app.router, "GET", "/api/entities", None, &bearer(&only)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

/// Deleting a connector's data is account-level: it needs the `connectors` scope, is closed to a
/// vault-restricted token, answers an unknown kind with a problem, and leaves an audit row.
#[tokio::test]
async fn deleting_connector_data_is_account_level_and_audited() {
    let app = TestApp::new().await;
    let path = "/api/connectors/pocketai/data";
    let read = token(&app, &[scopes::MEMORY_READ], None).await;
    let (status, _, body) = send(&app.router, "DELETE", path, None, &bearer(&read)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.scope")));
    let org = org_vault(&app).await;
    let restricted = token(&app, scopes::ALL, Some(&org)).await;
    let (status, _, _) = send(&app.router, "DELETE", path, None, &bearer(&restricted)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a vault-restricted token cannot reach account routes");

    let (status, _, body) = send(&app.router, "DELETE", "/api/connectors/nope/data", None, &bearer(&app.token)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::NOT_FOUND, Some("connector.not_found")), "{body}");

    let only = token(&app, &[scopes::CONNECTORS], None).await;
    let (status, _, body) = send(&app.router, "DELETE", path, None, &bearer(&only)).await;
    assert_eq!((status, body["records"].as_u64(), body["memories"].as_u64()), (StatusCode::OK, Some(0), Some(0)), "{body}");
    assert_eq!(events(&app, "action = 'connector.delete_data' AND target = 'pocketai' AND outcome = 'ok'").await.len(), 1);
}

#[tokio::test]
async fn tool_scopes_hold_over_rest_and_mcp() {
    let app = TestApp::new().await;
    let read = token(&app, &[scopes::MEMORY_READ], None).await;
    let write = token(&app, &[scopes::MEMORY_WRITE], None).await;
    let write_args = json!({"subject_name": "Ann", "subject_kind": "person", "text": "likes tea"});

    // REST
    let (status, _, body) = send(&app.router, "POST", "/api/tools/memory_write", Some(write_args.clone()), &bearer(&read)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.scope")));
    // memory:write implies memory:read, as for OAuth tokens; connectors alone grants neither
    let (status, _, _) = send(&app.router, "POST", "/api/tools/recall", Some(json!({"query": "tea"})), &bearer(&write)).await;
    assert_eq!(status, StatusCode::OK);
    let conn = token(&app, &[scopes::CONNECTORS], None).await;
    let (status, _, body) = send(&app.router, "POST", "/api/tools/recall", Some(json!({"query": "tea"})), &bearer(&conn)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.scope")));
    let (status, _, _) = send(&app.router, "POST", "/api/tools/memory_write", Some(write_args.clone()), &bearer(&write)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = send(&app.router, "POST", "/api/tools/entities_search", Some(json!({"query": "Ann"})), &bearer(&read)).await;
    assert_eq!(status, StatusCode::OK);

    // MCP: a refusal is a tool result with isError and the same code
    let (is_error, v) = mcp_call(&app, &read, "memory_write", write_args).await;
    assert!(is_error);
    assert_eq!(v["code"], "auth.scope");
    let (is_error, v) = mcp_call(&app, &read, "vault_create", json!({"name": "X"})).await;
    assert!((is_error, v["code"].as_str()) == (true, Some("auth.scope")), "{v}");
    let (is_error, v) = mcp_call(&app, &read, "entities_search", json!({"query": "Ann"})).await;
    assert!(!is_error, "{v}");
}

#[tokio::test]
async fn token_creation_validates_and_cannot_escalate() {
    let app = TestApp::new().await;
    let org = org_vault(&app).await;
    let (status, _, made) = send(
        &app.router,
        "POST",
        "/api/auth/tokens",
        Some(json!({"name": "ci", "scopes": ["memory:read"], "vault_id": org.to_string(), "expires_at": "2999-01-01T00:00:00Z"})),
        &bearer(&app.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made}");
    assert_eq!(made["scopes"], json!(["memory:read"]));
    assert_eq!(made["vault_id"], org.to_string());
    assert!(made["expires_at"].is_string());

    let (_, _, list) = send(&app.router, "GET", "/api/auth/tokens", None, &bearer(&app.token)).await;
    let row = list.as_array().unwrap().iter().find(|t| t["name"] == "ci").unwrap();
    assert_eq!((&row["scopes"], &row["vault_id"]), (&json!(["memory:read"]), &json!(org.to_string())));
    let legacy = list.as_array().unwrap().iter().find(|t| t["name"] == "test").unwrap();
    assert_eq!(legacy["scopes"].as_array().unwrap().len(), 4, "tokens made without scopes are full-scope");
    assert!(legacy["expires_at"].is_null());

    for bad in [
        json!({"name": "x", "scopes": []}),
        json!({"name": "x", "scopes": ["root"]}),
        json!({"name": "x", "expires_at": "yesterday"}),
        json!({"name": "x", "expires_at": "2001-01-01T00:00:00Z"}),
        json!({"name": "x", "vault_id": "person:nope"}),
    ] {
        let (status, _, _) = send(&app.router, "POST", "/api/auth/tokens", Some(bad.clone()), &bearer(&app.token)).await;
        assert!(status.is_client_error(), "{bad}");
    }
    // a vault the creator is not in
    let other = common::register(&app.state, "other@example.com").await;
    let theirs = common::sys(vaults::default_vault_id(&app.db().await, &other.id)).await.unwrap();
    let (status, _, body) = send(
        &app.router,
        "POST",
        "/api/auth/tokens",
        Some(json!({"name": "x", "vault_id": theirs.to_string()})),
        &bearer(&app.token),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("vault.forbidden")));

    // vaults:admin alone cannot mint a memory:read token
    let admin = token(&app, &[scopes::VAULTS_ADMIN], None).await;
    let (status, _, body) =
        send(&app.router, "POST", "/api/auth/tokens", Some(json!({"name": "x", "scopes": ["memory:read"]})), &bearer(&admin)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.scope")));
}

#[tokio::test]
async fn tokens_are_renamed_by_their_owner_only_and_audited() {
    let app = TestApp::new().await;
    let (_, _, made) = send(&app.router, "POST", "/api/auth/tokens", Some(json!({"name": "laptop"})), &bearer(&app.token)).await;
    let id = made["id"].as_str().unwrap().to_string();
    let path = format!("/api/auth/tokens/{id}");

    let (status, _, renamed) = send(&app.router, "PATCH", &path, Some(json!({"name": "  work laptop  "})), &bearer(&app.token)).await;
    assert_eq!(status, StatusCode::OK, "{renamed}");
    assert_eq!((renamed["id"].as_str(), renamed["name"].as_str()), (Some(id.as_str()), Some("work laptop")));
    let (_, _, list) = send(&app.router, "GET", "/api/auth/tokens", None, &bearer(&app.token)).await;
    let row = list.as_array().unwrap().iter().find(|t| t["id"] == id.as_str()).unwrap();
    assert_eq!(row["name"], "work laptop");
    assert_eq!(events(&app, "action = 'auth.token_rename'").await.len(), 1);

    // empty (after trimming) and over-long names are refused, and change nothing
    for bad in [json!({"name": "   "}), json!({"name": "x".repeat(81)})] {
        let (status, _, body) = send(&app.router, "PATCH", &path, Some(bad.clone()), &bearer(&app.token)).await;
        assert_eq!((status, body["code"].as_str()), (StatusCode::BAD_REQUEST, Some("validation.invalid")), "{bad}");
    }
    let (status, _, _) = send(&app.router, "POST", "/api/auth/tokens", Some(json!({"name": "x".repeat(81)})), &bearer(&app.token)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "creation has the same limit");

    // another user's token is not found, and keeps its name
    let other = common::register(&app.state, "other@example.com").await;
    let theirs = create_api_token_with(&app.state.control, &other.id, "theirs", &all(), None, None).await.unwrap();
    let their_path = format!("/api/auth/tokens/{}", RecordIdExt::to_string(&theirs.id));
    let (status, _, body) = send(&app.router, "PATCH", &their_path, Some(json!({"name": "mine now"})), &bearer(&app.token)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::NOT_FOUND, Some("auth.not_found")));
    let (_, _, list) = send(&app.router, "GET", "/api/auth/tokens", None, &bearer(&theirs.token)).await;
    assert_eq!(list[0]["name"], "theirs");
    // not a record id at all
    let (status, _, _) = send(&app.router, "PATCH", "/api/auth/tokens/nope", Some(json!({"name": "y"})), &bearer(&app.token)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(events(&app, "action = 'auth.token_rename'").await.len(), 1, "failures are not recorded as renames");
}

#[tokio::test]
async fn the_memory_skill_prompt_follows_token_scopes() {
    let app = TestApp::new().await;
    let rpc = |method: &str, params: Value| json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let reader = token(&app, &[scopes::MEMORY_READ], None).await;
    let (status, _, list) = send(&app.router, "POST", "/mcp", Some(rpc("prompts/list", json!({}))), &bearer(&reader)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["result"]["prompts"][0]["name"], "eunomia-memory");
    let (_, _, got) = send(&app.router, "POST", "/mcp", Some(rpc("prompts/get", json!({"name": "eunomia-memory"}))), &bearer(&reader)).await;
    assert!(got["result"]["messages"][0]["content"]["text"].as_str().unwrap().contains("## Before you finish"), "{got}");
    let (_, _, read) = send(&app.router, "POST", "/mcp", Some(rpc("resources/read", json!({"uri": "eunomia://skill"}))), &bearer(&reader)).await;
    assert!(read["result"]["contents"][0]["text"].as_str().unwrap().starts_with("# Eunomia memory"), "{read}");

    // a token that cannot read memory is offered no prompt and cannot fetch it
    let connectors = token(&app, &[scopes::CONNECTORS], None).await;
    let (_, _, list) = send(&app.router, "POST", "/mcp", Some(rpc("prompts/list", json!({}))), &bearer(&connectors)).await;
    assert_eq!(list["result"]["prompts"], json!([]));
    let (_, _, got) = send(&app.router, "POST", "/mcp", Some(rpc("prompts/get", json!({"name": "eunomia-memory"}))), &bearer(&connectors)).await;
    assert_eq!(got["error"]["data"]["code"], "auth.scope", "{got}");
}

// ------------------------------------------------------- vault restriction

#[tokio::test]
async fn vault_restricted_token_is_confined_to_its_vault() {
    let app = TestApp::new().await;
    let org = org_vault(&app).await;
    let personal = common::sys(vaults::default_vault_id(&app.db().await, &app.user.id)).await.unwrap();
    // personal data the restricted token must never see
    app.tool("memory_write", json!({"subject_name": "Secret", "subject_kind": "person", "text": "in personal"})).await;

    let t = token(&app, scopes::ALL, Some(&org)).await;
    let h = bearer(&t);

    // no vault_id means its own vault, not the personal one
    let (status, _, body) = send(
        &app.router,
        "POST",
        "/api/tools/memory_write",
        Some(json!({"subject_name": "Team Fact", "subject_kind": "person", "text": "in org"})),
        &h,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, _, listing) = send(&app.router, "GET", &format!("/api/entities?vault_id={}", org.to_string()), None, &h).await;
    let names: Vec<&str> = listing["results"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Team Fact"]);
    let (_, _, default_listing) = send(&app.router, "GET", "/api/entities", None, &h).await;
    assert_eq!(default_listing["results"].as_array().unwrap().len(), 1, "the default scope is the restricted vault");

    // the personal vault is a 403 auth.scope, over REST and MCP, read and write
    let (status, _, body) = send(&app.router, "GET", &format!("/api/entities?vault_id={}", personal.to_string()), None, &h).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.scope")), "{body}");
    let (status, _, body) = send(&app.router, "GET", &format!("/api/vaults/{}/members", personal.to_string()), None, &h).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.scope")), "{body}");
    let (is_error, v) = mcp_call(&app, &t, "recall", json!({"query": "personal", "vault_id": personal.to_string()})).await;
    assert!((is_error, v["code"].as_str()) == (true, Some("auth.scope")), "{v}");
    let (is_error, v) = mcp_call(
        &app,
        &t,
        "memory_write",
        json!({"subject_name": "X", "subject_kind": "person", "text": "t", "vault_id": personal.to_string()}),
    )
    .await;
    assert!((is_error, v["code"].as_str()) == (true, Some("auth.scope")), "{v}");

    // nothing from the personal vault leaks through the default scope
    let (is_error, v) = mcp_call(&app, &t, "entities_search", json!({"query": "Secret"})).await;
    assert!(!is_error);
    assert!(!v.to_string().contains("Secret"), "{v}");
    let (_, v) = mcp_call(&app, &t, "recall", json!({"query": "personal"})).await;
    assert!(!v.to_string().contains("in personal"), "{v}");

    // it sees only its vault, cannot make new ones, cannot read raw source records
    let (_, _, vs) = send(&app.router, "GET", "/api/vaults", None, &h).await;
    let ids: Vec<&str> = vs.as_array().or(vs["results"].as_array()).unwrap().iter().map(|v| v["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [org.to_string()]);
    for (m, p, b) in [
        ("POST", "/api/vaults", Some(json!({"name": "new"}))),
        ("POST", &*format!("/api/vaults/{}/clone", org.to_string()), Some(json!({}))),
        ("GET", "/api/settings", None),
        ("GET", "/api/auth/tokens", None),
        ("POST", "/api/chat/threads", Some(json!({}))),
    ] {
        let (status, _, body) = send(&app.router, m, p, b, &h).await;
        assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.scope")), "{m} {p}");
    }
    let (is_error, v) = mcp_call(&app, &t, "search", json!({"query": "x"})).await;
    assert!((is_error, v["code"].as_str()) == (true, Some("auth.scope")), "{v}");

    // and the unrestricted token still reaches both
    let (status, _, _) = send(&app.router, "GET", &format!("/api/entities?vault_id={}", personal.to_string()), None, &bearer(&app.token)).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn restricted_token_loses_access_when_removed_from_its_vault() {
    let app = TestApp::new().await;
    let other = common::register(&app.state, "o@example.com").await;
    let theirs: RecordId = rid::parse(&common::sys(vaults::create_vault(&app.db().await, &other.id, "Theirs", "org")).await.unwrap().id).unwrap();
    common::sys(vaults::invite_member(&app.db().await, app.control(), &other.id, &theirs, &app.user.email, "member")).await.unwrap();
    common::sys(vaults::accept_invitation(&app.db().await, &app.user.id, &theirs)).await.unwrap();
    let t = token(&app, scopes::ALL, Some(&theirs)).await;

    let (status, _, _) = send(&app.router, "GET", "/api/entities", None, &bearer(&t)).await;
    assert_eq!(status, StatusCode::OK);
    common::sys(vaults::remove_member(&app.db().await, app.control(), &other.id, &theirs, &app.user.email)).await.unwrap();
    let (status, _, body) = send(&app.router, "GET", "/api/entities", None, &bearer(&t)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("vault.forbidden")));
}

// ----------------------------------------------------------------- expiry

#[tokio::test]
async fn expired_token_is_401_token_expired() {
    let app = TestApp::new().await;
    let past = Datetime::from(chrono::Utc::now() - chrono::Duration::minutes(1));
    let expired = create_api_token_with(&app.state.control, &app.user.id, "old", &all(), None, Some(past)).await.unwrap().token;
    let future = Datetime::from(chrono::Utc::now() + chrono::Duration::days(1));
    let live = create_api_token_with(&app.state.control, &app.user.id, "new", &all(), None, Some(future)).await.unwrap().token;

    let (status, _, body) = send(&app.router, "GET", "/api/auth/me", None, &bearer(&expired)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::UNAUTHORIZED, Some("auth.token_expired")));
    let (status, _, _) = send(&app.router, "GET", "/api/auth/me", None, &bearer(&live)).await;
    assert_eq!(status, StatusCode::OK);

    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
    let (status, headers, v) = send(&app.router, "POST", "/mcp", Some(body), &bearer(&expired)).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::UNAUTHORIZED, Some("auth.token_expired")));
    let challenge = headers[header::WWW_AUTHENTICATE].to_str().unwrap();
    assert!(challenge.starts_with("Bearer error=\"invalid_token\"") && challenge.contains("resource_metadata="), "{challenge}");
}

async fn login_cookie(app: &TestApp) -> String {
    let creds = json!({"email": "tester@example.com", "password": PASSWORD});
    let (status, headers, _) = send(&app.router, "POST", "/api/auth/login", Some(creds), &[]).await;
    assert_eq!(status, StatusCode::OK);
    headers[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string()
}

async fn session_expiry_secs_from_now(app: &TestApp) -> i64 {
    let mut res = app.state.control.test_raw().query("SELECT VALUE expires_at FROM session").await.unwrap();
    (res.take::<Vec<Datetime>>(0).unwrap()[0].into_inner() - chrono::Utc::now()).num_seconds()
}

#[tokio::test]
async fn expired_session_is_401_and_live_one_slides_at_most_hourly() {
    let app = TestApp::new().await;
    let cookie = login_cookie(&app).await;
    let c = vec![("cookie", cookie.clone())];

    let (status, _, _) = send(&app.router, "GET", "/api/auth/me", None, &c).await;
    assert_eq!(status, StatusCode::OK);
    let day = 24 * 3600;
    let secs = session_expiry_secs_from_now(&app).await;
    assert!((29 * day..=30 * day).contains(&secs), "default 30 day window, got {secs}s");

    // used within the hour: not extended
    app.state.control.test_raw().query("UPDATE session SET expires_at = time::now() + 29d + 23h + 30m").await.unwrap().check().unwrap();
    app.state.credentials.clear(); // a hand edit reaches a cached session only once its entry ages out
    send(&app.router, "GET", "/api/auth/me", None, &c).await;
    let secs = session_expiry_secs_from_now(&app).await;
    assert!(secs < 29 * day + 23 * 3600 + 31 * 60, "not extended, got {secs}s");

    // idle for a while: extended back to the full window
    app.state.control.test_raw().query("UPDATE session SET expires_at = time::now() + 10d").await.unwrap().check().unwrap();
    app.state.credentials.clear(); // a hand edit reaches a cached session only once its entry ages out
    send(&app.router, "GET", "/api/auth/me", None, &c).await;
    let secs = session_expiry_secs_from_now(&app).await;
    assert!(secs > 29 * day, "slid forward, got {secs}s");

    // past expiry: rejected
    app.state.control.test_raw().query("UPDATE session SET expires_at = time::now() - 1m").await.unwrap().check().unwrap();
    app.state.credentials.clear(); // a hand edit reaches a cached session only once its entry ages out
    let (status, _, body) = send(&app.router, "GET", "/api/auth/me", None, &c).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::UNAUTHORIZED, Some("auth.session_expired")));
}

#[tokio::test]
async fn jwt_exp_is_enforced_and_legacy_cookies_without_exp_still_work() {
    let app = TestApp::new().await;
    let sid = eunomia_backend::models_user::generate_token();
    app.state.control.test_raw().query("CREATE session SET owner = $o, sid = $sid, expires_at = time::now() + 1d")
        .bind(("o", app.user.id.clone()))
        .bind(("sid", sid.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();
    let jwt = |claims: Value| {
        jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(app.state.settings.jwt_secret.as_bytes()),
        )
        .unwrap()
    };
    let base = json!({"sub": app.user.id.to_string(), "email": app.user.email, "sid": sid});

    let legacy = jwt(base.clone());
    let (status, _, _) = send(&app.router, "GET", "/api/auth/me", None, &[("cookie", format!("eunomia_session={legacy}"))]).await;
    assert_eq!(status, StatusCode::OK, "cookies issued before exp existed keep working");

    let mut expired = base;
    expired["exp"] = json!(chrono::Utc::now().timestamp() - 3600);
    let (status, _, body) =
        send(&app.router, "GET", "/api/auth/me", None, &[("cookie", format!("eunomia_session={}", jwt(expired)))]).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::UNAUTHORIZED, Some("auth.session_expired")));
}

// ------------------------------------------------------------------ audit

/// An OAuth access token for `scope`, minted straight into the database.
async fn oauth_token(app: &TestApp, scope: &[&str]) -> String {
    let token = format!("eoa_{}", eunomia_backend::models_user::generate_token());
    let grant: Option<RecordId> = app.state.control.test_raw().query(
            "CREATE oauth_grant SET owner = $o, client_id = 'client-x', client_name = 'X', scope = $scope, \
             resource = 'http://localhost:8001/mcp' RETURN VALUE id",
        )
        .bind(("o", app.user.id.clone()))
        .bind(("scope", scope.iter().map(|s| s.to_string()).collect::<Vec<_>>()))
        .await
        .unwrap()
        .take(0)
        .unwrap();
    app.state.control.test_raw().query("CREATE oauth_token SET kind = 'access', token_hash = $h, family = $g, expires_at = time::now() + 15m")
        .bind(("h", eunomia_backend::models_user::hash_token(&token)))
        .bind(("g", grant.unwrap()))
        .await
        .unwrap()
        .check()
        .unwrap();
    token
}

#[tokio::test]
async fn oauth_tokens_work_on_mcp_only_and_are_audited_as_the_client() {
    let app = TestApp::new().await;
    let t = oauth_token(&app, &["memory:write"]).await;
    let (is_error, v) =
        mcp_call(&app, &t, "memory_write", json!({"subject_name": "Ann", "subject_kind": "person", "text": "x"})).await;
    assert!(!is_error, "{v}");
    let rows = events(&app, "action = 'tool.memory_write'").await;
    assert_eq!((rows[0]["actor_kind"].as_str(), rows[0]["actor_id"].as_str()), (Some("oauth"), Some("client-x")));
    // never accepted on the REST API
    let (status, _, _) = send(&app.router, "GET", "/api/entities", None, &bearer(&t)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn audit_events_cover_auth_and_tool_calls_with_trace_ids() {
    let app = TestApp::new().await;

    // login (failed, then ok)
    let bad = json!({"email": "tester@example.com", "password": "wrong-password"});
    let (status, headers, _) = send(&app.router, "POST", "/api/auth/login", Some(bad), &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let failed_trace = headers["x-trace-id"].to_str().unwrap().to_string();
    login_cookie(&app).await;

    // token create + revoke
    let (_, _, made) = send(&app.router, "POST", "/api/auth/tokens", Some(json!({"name": "k"})), &bearer(&app.token)).await;
    let id = made["id"].as_str().unwrap().to_string();
    send(&app.router, "DELETE", &format!("/api/auth/tokens/{id}"), None, &bearer(&app.token)).await;

    // a mutating tool call over REST, and over MCP
    let (_, headers, _) = send(
        &app.router,
        "POST",
        "/api/tools/memory_write",
        Some(json!({"subject_name": "Ann", "subject_kind": "person", "text": "x"})),
        &bearer(&app.token),
    )
    .await;
    let tool_trace = headers["x-trace-id"].to_str().unwrap().to_string();
    mcp_call(&app, &app.token, "memory_write", json!({"subject_name": "Bob", "subject_kind": "person", "text": "y"})).await;

    // failed auth, and a scope denial
    let (_, headers, _) = send(&app.router, "GET", "/api/auth/me", None, &bearer("not-a-token")).await;
    let auth_trace = headers["x-trace-id"].to_str().unwrap().to_string();
    let read = token(&app, &[scopes::MEMORY_READ], None).await;
    send(&app.router, "POST", "/api/tools/memory_write", Some(json!({"subject_name": "Z", "subject_kind": "person", "text": "z"})), &bearer(&read)).await;
    send(&app.router, "GET", "/api/settings", None, &bearer(&read)).await;

    let login = events(&app, "action = 'auth.login'").await;
    assert_eq!(login.len(), 2);
    assert_eq!((login[0]["outcome"].as_str(), login[0]["actor_kind"].as_str()), (Some("auth.unauthorized"), Some("anonymous")));
    assert_eq!(login[0]["trace_id"], failed_trace);
    assert_eq!((login[1]["outcome"].as_str(), login[1]["actor_kind"].as_str()), (Some("ok"), Some("user")));

    let created = events(&app, "action = 'auth.token_create'").await;
    assert_eq!((created.len(), created[0]["actor_kind"].as_str(), created[0]["target"].as_str()), (1, Some("token"), Some(id.as_str())));
    assert_eq!(events(&app, "action = 'auth.token_revoke'").await.len(), 1);

    let tools = events(&app, "action = 'tool.memory_write' AND outcome = 'ok'").await;
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0]["trace_id"], tool_trace);
    assert!(tools.iter().all(|e| e["actor_kind"] == "token" && e["user"] == app.user.id.to_string()));
    // argument names and a hash, never the values (the control database is shared across orgs)
    let detail = tools[0]["detail"].as_str().unwrap();
    assert!(detail.starts_with("keys=") && detail.contains("subject_name") && !detail.contains("Ann"), "{detail}");

    let failed = events(&app, "action = 'auth.failed'").await;
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["trace_id"], auth_trace);
    assert_eq!(failed[0]["actor_kind"], "anonymous");

    let denied = events(&app, "outcome = 'auth.scope'").await;
    assert!(denied.iter().any(|e| e["action"] == "authz.denied" && e["actor_kind"] == "token"), "route denial: {denied:?}");
    assert!(denied.iter().any(|e| e["action"] == "tool.memory_write" && e["actor_kind"] == "token"), "tool denial: {denied:?}");

    for e in events(&app, "true").await {
        let t = e["trace_id"].as_str().unwrap();
        assert!(t.len() == 32 && t.bytes().all(|b| b.is_ascii_hexdigit()), "{e}");
    }
    // the owner-facing history still lists the mutating tool calls
    let (_, _, log) = send(&app.router, "GET", "/api/audit", None, &bearer(&app.token)).await;
    assert!(log["total"].as_i64().unwrap() >= 2, "{log}");
}

#[test]
fn audit_event_is_append_only_in_the_store() {
    for stmt in eunomia_backend::store::all() {
        let sql = stmt.sql.to_lowercase();
        // retention is the one deletion: a plain age cut-off, nothing else
        if stmt.name == "app.audit_prune" {
            assert!(sql.starts_with("delete audit_event where created_at < time::now()"), "{}", stmt.sql);
            continue;
        }
        if sql.contains("audit_event") {
            assert!(sql.trim_start().starts_with("create audit_event"), "{} touches audit_event: {}", stmt.name, stmt.sql);
        }
    }
    // and no other code path writes to it
    for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/src")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).unwrap().to_lowercase();
            for verb in ["update audit_event", "delete audit_event", "delete from audit_event", "upsert audit_event"] {
                assert!(!text.contains(verb), "{} contains {verb:?}", path.display());
            }
        }
    }
}

// ------------------------------------------------------------ rate limits

fn app_with_limits(app: &TestApp, limits: RateConfig) -> axum::Router {
    eunomia_backend::app_with(app.state.clone(), limits)
}

#[tokio::test]
async fn login_is_limited_per_client_address_with_retry_after() {
    let app = TestApp::new().await;
    let router = app_with_limits(&app, RateConfig { auth_per_min: 3, ..RateConfig::default() });
    let creds = json!({"email": "tester@example.com", "password": "wrong-password"});
    let from = |ip: &str| vec![("x-forwarded-for", ip.to_string())];

    for _ in 0..3 {
        let (status, _, _) = send(&router, "POST", "/api/auth/login", Some(creds.clone()), &from("203.0.113.1")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, headers, body) = send(&router, "POST", "/api/auth/login", Some(creds.clone()), &from("203.0.113.1")).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate.limited")));
    assert_eq!(headers[header::CONTENT_TYPE], "application/problem+json");
    assert!(headers[header::RETRY_AFTER].to_str().unwrap().parse::<u64>().unwrap() >= 1);
    assert!(body["trace_id"].is_string());
    // registration shares the auth limit; another address is unaffected
    let (status, _, _) = send(&router, "POST", "/api/auth/register", Some(creds.clone()), &from("203.0.113.1")).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let (status, _, _) = send(&router, "POST", "/api/auth/login", Some(creds), &from("203.0.113.2")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn forwarded_for_from_an_untrusted_peer_cannot_dodge_the_limit() {
    let app = TestApp::new().await;
    let router = app_with_limits(&app, RateConfig { auth_per_min: 2, ..RateConfig::default() });
    let creds = json!({"email": "tester@example.com", "password": "wrong-password"});
    let mut last = StatusCode::OK;
    for i in 0..3 {
        let mut req = Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-forwarded-for", format!("10.1.1.{i}"))
            .body(Body::from(creds.to_string()))
            .unwrap();
        req.extensions_mut().insert(axum::extract::ConnectInfo(std::net::SocketAddr::from(([203, 0, 113, 7], 4000))));
        last = router.clone().oneshot(req).await.unwrap().status();
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn guessing_tokens_is_throttled_by_address() {
    let app = TestApp::new().await;
    let router = app_with_limits(&app, RateConfig { auth_per_min: 2, ..RateConfig::default() });
    let h = |t: &str| vec![("authorization", format!("Bearer {t}")), ("x-forwarded-for", "203.0.113.9".to_string())];
    for _ in 0..2 {
        let (status, _, _) = send(&router, "GET", "/api/auth/me", None, &h("guess")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, _, body) = send(&router, "GET", "/api/auth/me", None, &h("guess")).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate.limited")));
    // a valid token from that address is not affected by the failed-credential bucket
    let (status, _, _) = send(&router, "GET", "/api/auth/me", None, &h(&app.token)).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn requests_are_limited_per_token_and_per_user() {
    let app = TestApp::new().await;
    let router = app_with_limits(&app, RateConfig { token_per_min: 3, user_per_min: 5, auth_per_min: 100, ..RateConfig::default() });
    let other = token(&app, scopes::ALL, None).await;

    for _ in 0..3 {
        let (status, _, _) = send(&router, "GET", "/api/auth/me", None, &bearer(&app.token)).await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, headers, body) = send(&router, "GET", "/api/auth/me", None, &bearer(&app.token)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate.limited")));
    assert!(headers.contains_key(header::RETRY_AFTER));

    // a second token has its own bucket, but both draw on the user's
    let mut ok = 0;
    for _ in 0..4 {
        let (status, _, _) = send(&router, "GET", "/api/auth/me", None, &bearer(&other)).await;
        if status == StatusCode::OK {
            ok += 1;
        }
    }
    assert!((1..=2).contains(&ok), "user bucket (5) minus the 3+1 already spent, got {ok} ok");
}

#[tokio::test]
async fn limits_come_from_the_environment_with_self_host_defaults() {
    let d = RateConfig::default();
    assert!(d.auth_per_min >= 5 && d.auth_per_min <= d.token_per_min && d.token_per_min <= d.user_per_min);
}

// ------------------------------------------------------ route classification

#[test]
fn every_documented_route_is_classified_and_tools_have_a_scope() {
    use axum::http::Method;
    let doc = serde_json::to_value(eunomia_backend::openapi::spec()).unwrap();
    let mut unclassified = Vec::new();
    for (path, item) in doc["paths"].as_object().unwrap() {
        let concrete = path.replace(['{', '}'], "").replace("kind", "up_bank");
        for m in ["get", "post", "put", "patch", "delete"] {
            if item.get(m).is_some() {
                let method = Method::from_bytes(m.to_uppercase().as_bytes()).unwrap();
                if eunomia_backend::gate::explicit_class(&method, &concrete).is_none() {
                    unclassified.push(format!("{m} {path}"));
                }
            }
        }
    }
    assert!(unclassified.is_empty(), "add these routes to gate::explicit_class: {unclassified:?}");
}

// ------------------------------------------------------ hardening

#[tokio::test]
async fn a_replayed_bad_credential_leaves_one_audit_row_per_minute_and_is_throttled() {
    let app = TestApp::new().await;
    let router = app_with_limits(&app, RateConfig { auth_per_min: 5, ..RateConfig::default() });
    let past = Datetime::from(chrono::Utc::now() - chrono::Duration::minutes(1));
    let expired = create_api_token_with(&app.state.control, &app.user.id, "old", &all(), None, Some(past)).await.unwrap().token;
    let h = |t: &str| vec![("authorization", format!("Bearer {t}")), ("x-forwarded-for", "203.0.113.50".to_string())];

    let mut statuses = Vec::new();
    for _ in 0..8 {
        statuses.push(send(&router, "GET", "/api/auth/me", None, &h(&expired)).await.0);
    }
    // the first five fail as expired, then the address bucket answers 429 before any audit write
    assert_eq!(statuses.iter().filter(|s| **s == StatusCode::UNAUTHORIZED).count(), 5, "{statuses:?}");
    assert_eq!(statuses.iter().filter(|s| **s == StatusCode::TOO_MANY_REQUESTS).count(), 3, "{statuses:?}");
    // five attempts, one credential: one row
    assert_eq!(events(&app, "action = 'auth.failed'").await.len(), 1);
}

#[tokio::test]
async fn webhooks_are_limited_per_address_and_unknown_owners_look_like_bad_signatures() {
    let app = TestApp::new().await;
    let router = app_with_limits(&app, RateConfig { webhook_per_min: 3, ..RateConfig::default() });
    let from = vec![("x-forwarded-for", "203.0.113.60".to_string())];
    let real = format!("/api/sources/github/webhook/{}", app.user.id.to_string());
    let (s1, _, b1) = send(&router, "POST", &real, Some(json!({"zen": "x"})), &from).await;
    let (s2, _, b2) = send(&router, "POST", "/api/sources/github/webhook/user:nobody", Some(json!({"zen": "x"})), &from).await;
    let (s3, _, b3) = send(&router, "POST", "/api/sources/github/webhook/not-an-id", Some(json!({"zen": "x"})), &from).await;
    // no secret is configured for the real user, the others do not exist: all answer alike
    assert_eq!((s1, s2, s3), (StatusCode::OK, StatusCode::OK, StatusCode::OK));
    assert_eq!((&b1, &b2), (&b2, &b3));
    assert_eq!(b1["status"], "ignored");
    let (s4, _, b4) = send(&router, "POST", &real, Some(json!({})), &from).await;
    assert_eq!((s4, b4["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate.limited")));
}

#[tokio::test]
async fn oversized_bodies_are_refused_before_any_handler_reads_them() {
    let app = TestApp::new().await;
    let big = "x".repeat(2 * 1024 * 1024);
    for path in ["/api/auth/login", "/api/sources/github/webhook/user:nobody"] {
        let mut req = Request::builder().method("POST").uri(path).header(header::CONTENT_TYPE, "application/json").body(Body::from(big.clone())).unwrap();
        req.extensions_mut().insert(axum::extract::ConnectInfo(std::net::SocketAddr::from(([127, 0, 0, 1], 4000))));
        let status = app.router.clone().oneshot(req).await.unwrap().status();
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{path}");
    }
}

/// A delivery whose body never finishes is cut off after the 10 s read timeout with 408 (takes that long).
#[tokio::test]
async fn a_stalled_webhook_body_times_out_with_408() {
    let app = TestApp::new().await;
    let body = Body::from_stream(futures::stream::pending::<Result<axum::body::Bytes, std::io::Error>>());
    let mut req = Request::builder().method("POST").uri("/api/sources/github/webhook/user:nobody").header(header::CONTENT_TYPE, "application/json").body(body).unwrap();
    req.extensions_mut().insert(axum::extract::ConnectInfo(std::net::SocketAddr::from(([127, 0, 0, 1], 4000))));
    let status = app.router.clone().oneshot(req).await.unwrap().status();
    assert_eq!(status, StatusCode::REQUEST_TIMEOUT);
}

#[tokio::test]
async fn forwarded_for_from_a_lan_peer_is_not_believed_by_default() {
    let app = TestApp::new().await;
    let router = app_with_limits(&app, RateConfig { auth_per_min: 2, ..RateConfig::default() });
    let creds = json!({"email": "tester@example.com", "password": "wrong-password"});
    let mut last = StatusCode::OK;
    for i in 0..3 {
        let mut req = Request::builder()
            .method("POST")
            .uri("/api/auth/login")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-forwarded-for", format!("198.51.100.{i}"))
            .body(Body::from(creds.to_string()))
            .unwrap();
        req.extensions_mut().insert(axum::extract::ConnectInfo(std::net::SocketAddr::from(([192, 168, 1, 20], 4000))));
        last = router.clone().oneshot(req).await.unwrap().status();
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}

// ------------------------------------------------------------ session-only OAuth routes

#[tokio::test]
async fn oauth_consent_and_grants_need_a_browser_session_not_a_token() {
    let app = TestApp::new().await;
    let admin_pat = token(&app, &[scopes::VAULTS_ADMIN], None).await;
    let body = json!({
        "response_type": "code", "client_id": "nope", "redirect_uri": "http://127.0.0.1:7777/cb",
        "code_challenge": "x", "code_challenge_method": "S256", "approve": true,
    });
    // a vaults:admin token is not enough to approve a grant (it would mint a longer-lived credential)
    let (status, _, v) = send(&app.router, "POST", "/api/oauth/consent", Some(body.clone()), &bearer(&admin_pat)).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.scope")), "{v}");
    let (status, _, v) = send(&app.router, "GET", "/api/oauth/grants", None, &bearer(&admin_pat)).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.scope")), "{v}");
    let (status, _, v) = send(&app.router, "DELETE", "/api/oauth/grants/x", None, &bearer(&admin_pat)).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.scope")), "{v}");

    // the browser session gets past the check (the unknown client then fails validation, not scope)
    let (status, v) = app.http_session("POST", "/api/oauth/consent", Some(body)).await;
    assert_ne!(status, StatusCode::FORBIDDEN, "{v}");
    let (status, _) = app.http_session("GET", "/api/oauth/grants", None).await;
    assert_eq!(status, StatusCode::OK);
}

// ------------------------------------------------------------ a token cannot mint a longer-lived or wider token

#[tokio::test]
async fn an_expiring_or_vault_restricted_token_cannot_mint_a_longer_lived_one() {
    let app = TestApp::new().await;
    let org = org_vault(&app).await;
    let soon = chrono::Utc::now() + chrono::Duration::hours(1);
    let soon_ts = Datetime::from(soon);
    let granted: Vec<String> = vec![scopes::VAULTS_ADMIN.into(), scopes::MEMORY_READ.into()];
    let pat = create_api_token_with(&app.state.control, &app.user.id, "short", &granted, None, Some(soon_ts)).await.unwrap().token;
    let mint = |body: Value, tok: String| {
        let router = app.router.clone();
        async move { send(&router, "POST", "/api/auth/tokens", Some(body), &bearer(&tok)).await }
    };

    // no expiry, or one past its own: refused
    for body in [
        json!({"name": "forever", "scopes": ["memory:read"]}),
        json!({"name": "later", "scopes": ["memory:read"], "expires_at": (soon + chrono::Duration::hours(1)).to_rfc3339()}),
    ] {
        let (status, _, v) = mint(body, pat.clone()).await;
        assert_eq!((status, v["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.scope")), "{v}");
    }
    // sooner, and within its scopes: fine
    let body = json!({"name": "sooner", "scopes": ["memory:read"], "expires_at": (soon - chrono::Duration::minutes(10)).to_rfc3339()});
    let (status, _, v) = mint(body, pat.clone()).await;
    assert_eq!(status, StatusCode::OK, "{v}");

    // a vault-restricted token cannot mint at all (the gate keeps account routes from it)
    let restricted = create_api_token_with(&app.state.control, &app.user.id, "r", &granted, Some(&org), None).await.unwrap().token;
    let (status, _, v) = mint(json!({"name": "wide", "scopes": ["memory:read"]}), restricted).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.scope")), "{v}");

    // a browser session keeps full ability
    let (status, v) = app.http_session("POST", "/api/auth/tokens", Some(json!({"name": "forever"}))).await;
    assert_eq!(status, StatusCode::OK, "{v}");
}

// ------------------------------------------------------------ MCP batches

#[tokio::test]
async fn mcp_batches_are_capped_and_charged_per_message() {
    let app = TestApp::new().await;
    let ping = |id: u32| json!({"jsonrpc": "2.0", "id": id, "method": "ping"});
    let batch = |n: u32| Value::Array((0..n).map(ping).collect());

    let (status, _, v) = send(&app.router, "POST", "/mcp", Some(batch(33)), &bearer(&app.token)).await;
    assert_eq!((status, v["error"]["code"].as_i64()), (StatusCode::BAD_REQUEST, Some(-32600)), "{v}");
    let (status, _, v) = send(&app.router, "POST", "/mcp", Some(batch(32)), &bearer(&app.token)).await;
    assert_eq!((status, v.as_array().map(Vec::len)), (StatusCode::OK, Some(32)));

    // 10 requests a minute: a 6 message batch uses 6, so a second one is refused
    let limits = RateConfig { user_per_min: 10, token_per_min: 0, ..RateConfig::default() };
    let limited = app_with_limits(&app, limits);
    let (status, _, v) = send(&limited, "POST", "/mcp", Some(batch(6)), &bearer(&app.token)).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let (status, _, v) = send(&limited, "POST", "/mcp", Some(batch(6)), &bearer(&app.token)).await;
    assert_eq!((status, v["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate.limited")), "{v}");
}

/// `initialize` hands out a session id naming the tool surface; a request carrying one from an older
/// surface (the server was upgraded under a connected client) gets 404 so the client re-initializes
/// and re-fetches `tools/list` (issue #90). No session id at all keeps working, as before.
#[tokio::test]
async fn mcp_sessions_from_an_older_tool_surface_are_sent_to_reinitialize() {
    let app = TestApp::new().await;
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}});
    let (status, headers, v) = send(&app.router, "POST", "/mcp", Some(init.clone()), &bearer(&app.token)).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let session = headers.get("mcp-session-id").expect("initialize issues a session id").to_str().unwrap().to_string();
    assert!(session.is_ascii() && session.contains('.'), "{session}");

    let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
    let with = |id: &str| {
        let mut h = bearer(&app.token);
        h.push(("mcp-session-id", id.to_string()));
        h
    };
    let (status, _, v) = send(&app.router, "POST", "/mcp", Some(list.clone()), &with(&session)).await;
    assert_eq!(status, StatusCode::OK, "the current session is accepted: {v}");
    let (status, _, _) = send(&app.router, "POST", "/mcp", Some(list.clone()), &bearer(&app.token)).await;
    assert_eq!(status, StatusCode::OK, "no session id: stateless clients keep working");

    let (status, _, v) = send(&app.router, "POST", "/mcp", Some(list), &with("0000000000000000.made-up-before-restart")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{v}");
    // re-initializing with the stale id is how the client recovers
    let (status, headers, _) = send(&app.router, "POST", "/mcp", Some(init), &with("0000000000000000.old")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("mcp-session-id").unwrap().to_str().unwrap().split('.').next(), session.split('.').next());
}

// ------------------------------------------------------------ bulk writes (#87)

fn registry_cost(n: usize) -> usize {
    eunomia_backend::tools::registry::extra_cost("memory_write_batch", &json!({"items": facts(n, 0)}))
}

fn facts(n: usize, from: usize) -> Value {
    Value::Array((from..from + n).map(|i| json!({"subject_name": format!("Page {i}"), "subject_kind": "file", "text": format!("page {i} says hello")})).collect())
}

/// Hundreds of facts go in as batches of 100: each item is written like `memory_write`, a bad item
/// fails alone, and a batch costs one request per 10 facts -- cheap enough for a 900-page import,
/// still inside the rate limit.
#[tokio::test]
async fn bulk_writes_go_in_batches_charged_per_ten_facts() {
    let app = TestApp::new().await;
    let tok = bearer(&app.token);
    let (status, _, v) = send(&app.router, "POST", "/api/tools/memory_write_batch", Some(json!({"items": facts(100, 0)})), &tok).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!((v["written"].as_u64(), v["failed"].as_u64()), (Some(100), Some(0)), "{v}");
    assert_eq!(v["results"][99]["index"], 99);
    assert!(v["results"][0]["memory_id"].as_str().unwrap().starts_with("memory:"));

    let mut mixed = facts(2, 100);
    mixed.as_array_mut().unwrap().insert(1, json!({"subject_name": "No text", "subject_kind": "file"}));
    let (_, _, v) = send(&app.router, "POST", "/api/tools/memory_write_batch", Some(json!({"items": mixed})), &tok).await;
    assert_eq!((v["written"].as_u64(), v["failed"].as_u64()), (Some(2), Some(1)), "{v}");
    assert_eq!((v["results"][1]["ok"].as_bool(), v["results"][1]["code"].as_str()), (Some(false), Some("validation.invalid")), "{v}");

    let (_, _, v) = send(&app.router, "POST", "/api/tools/memory_write_batch", Some(json!({"items": facts(101, 0)})), &tok).await;
    assert!(v["error"].as_str().unwrap_or_default().contains("100"), "{v}");

    // A batch costs one request per 10 facts, over REST and MCP alike. Wall-clock refill makes exact
    // budgets flaky under load, so: a batch fits a fresh budget, and once the budget is drained by
    // plain requests, a batch is refused at once. (That a refused surcharge takes nothing is the
    // limiter's own unit test, `ratelimit::a_refused_multi_token_charge_takes_nothing`.)
    let limited = app_with_limits(&app, RateConfig { token_per_min: 10, user_per_min: 0, ..RateConfig::default() });
    let batch = |n: usize, from: usize| Some(json!({"items": facts(n, from)}));
    let (status, _, v) = send(&limited, "POST", "/api/tools/memory_write_batch", batch(50, 200), &tok).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let mut drained = false;
    for _ in 0..30 {
        if send(&limited, "GET", "/api/vaults", None, &tok).await.0 == StatusCode::TOO_MANY_REQUESTS {
            drained = true;
            break;
        }
    }
    assert!(drained, "a 10/min token runs dry");
    let (status, headers, _) = send(&limited, "POST", "/api/tools/memory_write_batch", batch(50, 500), &tok).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(headers.contains_key("retry-after"));
    let call = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "memory_write_batch", "arguments": {"items": facts(50, 600)}}});
    let (status, _, _) = send(&limited, "POST", "/mcp", Some(call), &tok).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    // the batch spent 5 of the 10: about 5 plain requests (plus a little refill) drained the rest
    assert_eq!(registry_cost(50), 4);
    assert_eq!(memory_count(&app).await, 100 + 2 + 50, "refused batches wrote nothing");
}

// ------------------------------------------------------------ the chat agent runs as the caller

/// A stand-in for the OpenAI endpoint: asks for a `memory_write` first, then answers in text.
/// With `fail` it answers every request with a 500 instead.
async fn stub_openai(fail: bool) -> String {
    use axum::{routing::post, Router};
    async fn completions(axum::extract::State(fail): axum::extract::State<bool>, body: String) -> axum::response::Response {
        use axum::response::IntoResponse;
        if fail {
            return (StatusCode::INTERNAL_SERVER_ERROR, "boom").into_response();
        }
        let sse = |chunk: Value| format!("data: {chunk}\n\ndata: [DONE]\n\n");
        let text = if body.contains("\"role\":\"tool\"") {
            sse(json!({"choices": [{"delta": {"content": "done"}}]}))
        } else {
            let args = json!({"subject_name": "Chat", "subject_kind": "person", "text": "written by the model"}).to_string();
            sse(json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c1", "function": {"name": "memory_write", "arguments": args}}]}}]}))
        };
        ([(header::CONTENT_TYPE, "text/event-stream")], text).into_response()
    }
    let router = Router::new().route("/v1/chat/completions", post(completions)).with_state(fail);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}/v1")
}

/// Sends one chat message with `tok` and returns the SSE events.
async fn chat_events(app: &TestApp, tok: &str, base_url: &str) -> (String, Vec<Value>) {
    let h = bearer(tok);
    let (status, _, _) = send(&app.router, "PATCH", "/api/settings", Some(json!({"openai_base_url": base_url})), &h).await;
    assert_eq!(status, StatusCode::OK);
    let (_, _, thread) = send(&app.router, "POST", "/api/chat/threads", Some(json!({})), &h).await;
    let thread_id = thread["id"].as_str().unwrap().to_string();
    let mut req = Request::builder().method("POST").uri(format!("/api/chat/threads/{thread_id}")).header(header::CONTENT_TYPE, "application/json");
    for (k, v) in &h {
        req = req.header(*k, v);
    }
    let req = req.body(Body::from(json!({"message": "remember something"}).to_string())).unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let text = String::from_utf8(resp.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    let events = text.lines().filter_map(|l| l.strip_prefix("data:")).filter_map(|d| serde_json::from_str(d.trim()).ok()).collect();
    (thread_id, events)
}

async fn memory_count(app: &TestApp) -> i64 {
    let mut res = app.db().await.test_raw().query("SELECT count() FROM memory GROUP ALL").await.unwrap();
    let rows: Vec<Value> = res.take(0).unwrap();
    rows.first().and_then(|r| r["count"].as_i64()).unwrap_or(0)
}

#[tokio::test]
async fn model_tool_calls_in_chat_carry_the_callers_scopes() {
    let app = TestApp::new().await;
    let base = stub_openai(false).await;

    // vaults:admin reaches the chat route but has no memory:write, so the model's write is refused
    let admin_only = token(&app, &[scopes::VAULTS_ADMIN], None).await;
    let (thread_id, events) = chat_events(&app, &admin_only, &base).await;
    assert!(events.iter().any(|e| e["type"] == "tool_result"), "{events:?}");
    assert_eq!(memory_count(&app).await, 0, "the model wrote memory for a token without memory:write");
    let (_, _, hist) = send(&app.router, "GET", &format!("/api/chat/threads/{thread_id}/history"), None, &bearer(&admin_only)).await;
    assert!(hist.to_string().contains("auth.scope"), "{hist}");

    // with memory:write the same flow writes
    let both = token(&app, &[scopes::VAULTS_ADMIN, scopes::MEMORY_WRITE], None).await;
    chat_events(&app, &both, &base).await;
    assert_eq!(memory_count(&app).await, 1);
}

#[tokio::test]
async fn chat_error_events_carry_a_code_and_trace_id() {
    let app = TestApp::new().await;
    let base = stub_openai(true).await;
    let tok = token(&app, &[scopes::VAULTS_ADMIN], None).await;
    let (_, events) = chat_events(&app, &tok, &base).await;
    let err = events.iter().find(|e| e["type"] == "error").expect("an error event");
    assert_eq!(err["code"], "internal");
    assert_eq!(err["trace_id"].as_str().map(str::len), Some(32), "{err}");
    assert!(err["message"].as_str().unwrap().contains("OpenAI request failed"));
}
