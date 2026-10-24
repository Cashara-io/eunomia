//! Tool-call usage counters: every call through the registry (REST, MCP, chat; reads and writes;
//! successes and errors) is counted per person, credential, channel, MCP session and tool, and
//! `GET /api/audit/tool-usage` shows a user their own usage and an admin the whole org's.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::TestApp;
use eunomia_backend::models_user::{create_api_token, create_api_token_with};
use eunomia_backend::rid::RecordIdExt;
use eunomia_backend::scopes;
use eunomia_backend::usage;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

async fn send(router: &axum::Router, method: &str, path: &str, body: Option<Value>, token: &str, extra: &[(&str, &str)]) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {token}"));
    for (k, v) in extra {
        req = req.header(*k, *v);
    }
    let mut req = match body {
        Some(b) => req.header(header::CONTENT_TYPE, "application/json").body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    req.extensions_mut().insert(axum::extract::ConnectInfo(std::net::SocketAddr::from(([127, 0, 0, 1], 4000))));
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn rest(app: &TestApp, token: &str, tool: &str, args: Value) -> (StatusCode, Value) {
    send(&app.router, "POST", &format!("/api/tools/{tool}"), Some(args), token, &[]).await
}

/// A JSON-RPC `tools/call` over `/mcp`, optionally with an `Mcp-Session-Id`.
async fn mcp(app: &TestApp, token: &str, session: Option<&str>, tool: &str, args: Value) -> bool {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": args}});
    let extra: Vec<(&str, &str)> = session.map(|s| ("mcp-session-id", s)).into_iter().collect();
    let (status, v) = send(&app.router, "POST", "/mcp", Some(body), token, &extra).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    v["result"]["isError"].as_bool().unwrap()
}

/// A real MCP session id: what `initialize` hands out (a made-up one is refused as stale, #90).
async fn mcp_session(app: &TestApp, token: &str) -> String {
    let body = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}});
    let mut req = Request::builder().method("POST").uri("/mcp").header(header::AUTHORIZATION, format!("Bearer {token}")).header(header::CONTENT_TYPE, "application/json");
    req = req.header("accept", "application/json");
    let mut req = req.body(Body::from(body.to_string())).unwrap();
    req.extensions_mut().insert(axum::extract::ConnectInfo(std::net::SocketAddr::from(([127, 0, 0, 1], 4000))));
    let resp = app.router.clone().oneshot(req).await.unwrap();
    resp.headers().get("mcp-session-id").expect("initialize issues a session id").to_str().unwrap().to_string()
}

async fn report(app: &TestApp, token: &str, query: &str) -> (StatusCode, Value) {
    usage::settle().await;
    send(&app.router, "GET", &format!("/api/audit/tool-usage{query}"), None, token, &[]).await
}

fn row<'a>(rows: &'a Value, field: &str, value: &str) -> &'a Value {
    rows.as_array().unwrap().iter().find(|r| r[field] == value).unwrap_or_else(|| panic!("no row with {field}={value} in {rows}"))
}

#[tokio::test]
async fn every_call_is_counted_per_person_credential_channel_session_and_tool() {
    let app = TestApp::new().await;
    let admin = &app.user;
    let laptop = create_api_token(app.control(), &admin.id, "laptop").await.unwrap().token;
    let member = common::register(&app.state, "member@example.com").await;
    let member_token = create_api_token(app.control(), &member.id, "member-key").await.unwrap().token;

    // admin, token "test": two reads over REST
    for _ in 0..2 {
        assert_eq!(rest(&app, &app.token, "recall", json!({"query": "anything"})).await.0, StatusCode::OK);
    }
    // admin, token "laptop": a read, a write and a failing read over MCP session s1, one read over session s2
    let (s1, s2) = (mcp_session(&app, &laptop).await, mcp_session(&app, &laptop).await);
    assert!(!mcp(&app, &laptop, Some(&s1), "recall", json!({"query": "x"})).await);
    assert!(!mcp(&app, &laptop, Some(&s1), "memory_write", json!({"subject_name": "Ann", "subject_kind": "person", "text": "Ann likes tea"})).await);
    assert!(mcp(&app, &laptop, Some(&s1), "entities_get", json!({"id": "not-an-id"})).await);
    assert!(!mcp(&app, &laptop, Some(&s2), "recall", json!({"query": "y"})).await);
    // the member: a read over REST and an erroring read over REST, one MCP call with no session id
    assert_eq!(rest(&app, &member_token, "recall", json!({"query": "z"})).await.0, StatusCode::OK);
    assert_eq!(rest(&app, &member_token, "entities_get", json!({"id": "nope"})).await.0, StatusCode::OK);
    assert!(!mcp(&app, &member_token, None, "recall", json!({"query": "z"})).await);

    let (status, r) = report(&app, &app.token, "").await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(r["scope"], "org");
    assert_eq!(r["totals"]["calls"], 9);
    assert_eq!(r["totals"]["errors"], 2);

    // per person
    let admin_id = admin.id.to_string();
    let member_id = member.id.to_string();
    let a = row(&r["by_person"], "user_id", &admin_id);
    let m = row(&r["by_person"], "user_id", &member_id);
    assert_eq!((a["calls"].as_i64(), a["errors"].as_i64(), a["email"].as_str()), (Some(6), Some(1), Some("tester@example.com")));
    assert_eq!((m["calls"].as_i64(), m["errors"].as_i64(), m["email"].as_str()), (Some(3), Some(1), Some("member@example.com")));
    assert_eq!(r["by_person"].as_array().unwrap().len(), 2);

    // per credential: name resolved from the token
    let creds = &r["by_credential"];
    let by = |label: &str| row(creds, "label", label);
    assert_eq!((by("test")["calls"].as_i64(), by("test")["kind"].as_str()), (Some(2), Some("token")));
    assert_eq!((by("laptop")["calls"].as_i64(), by("laptop")["errors"].as_i64()), (Some(4), Some(1)));
    assert_eq!((by("member-key")["calls"].as_i64(), by("member-key")["user_id"].as_str()), (Some(3), Some(member_id.as_str())));

    // per channel
    assert_eq!(row(&r["by_channel"], "channel", "rest")["calls"], 4);
    assert_eq!(row(&r["by_channel"], "channel", "mcp")["calls"], 5);

    // per MCP session (calls with no session id are not listed)
    let sessions = &r["by_session"];
    assert_eq!(sessions.as_array().unwrap().len(), 2, "{sessions}");
    assert_eq!((row(sessions, "session", &s1)["calls"].as_i64(), row(sessions, "session", &s1)["errors"].as_i64()), (Some(3), Some(1)));
    assert_eq!(row(sessions, "session", &s2)["calls"], 1);
    assert_eq!(row(sessions, "session", &s1)["channel"], "mcp");

    // per tool, reads and writes
    assert_eq!(row(&r["by_tool"], "tool", "recall")["calls"], 6);
    assert_eq!(row(&r["by_tool"], "tool", "recall")["read_only"], true);
    assert_eq!(row(&r["by_tool"], "tool", "memory_write")["calls"], 1);
    assert_eq!(row(&r["by_tool"], "tool", "memory_write")["read_only"], false);
    assert_eq!(row(&r["by_tool"], "tool", "entities_get")["errors"], 2);

    // the time series covers the range and adds up
    let total: i64 = r["series"].as_array().unwrap().iter().map(|p| p["calls"].as_i64().unwrap()).sum();
    assert_eq!(total, 9);
    assert_eq!(r["series_step"], "day");
    let (_, day) = report(&app, &app.token, "?hours=24").await;
    assert_eq!(day["series_step"], "hour");
    assert_eq!(day["totals"]["calls"], 9);
}

#[tokio::test]
async fn a_member_sees_only_their_own_usage_and_an_admin_sees_everyone() {
    let app = TestApp::new().await;
    let member = common::register(&app.state, "member@example.com").await;
    let member_token = create_api_token(app.control(), &member.id, "member-key").await.unwrap().token;
    rest(&app, &app.token, "recall", json!({"query": "a"})).await;
    rest(&app, &member_token, "recall", json!({"query": "b"})).await;
    rest(&app, &member_token, "recall", json!({"query": "c"})).await;

    // the member: own calls only, no other person in any table
    let (status, mine) = report(&app, &member_token, "").await;
    assert_eq!(status, StatusCode::OK, "{mine}");
    assert_eq!(mine["scope"], "self");
    assert_eq!(mine["totals"]["calls"], 2);
    assert_eq!(mine["by_person"].as_array().unwrap().len(), 1);
    assert_eq!(mine["by_person"][0]["user_id"], member.id.to_string());
    assert!(mine["by_credential"].as_array().unwrap().iter().all(|c| c["user_id"] == member.id.to_string()));
    assert!(!mine.to_string().contains("tester@example.com"), "{mine}");

    // asking for someone else is refused; asking for oneself is fine
    let (status, body) = report(&app, &member_token, &format!("?user={}", app.user.id.to_string())).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "{body}");
    let (status, _) = report(&app, &member_token, &format!("?user={}", member.id.to_string())).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = report(&app, &member_token, "?user=not-an-id").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // the admin sees both, and can narrow to one person
    let (_, all) = report(&app, &app.token, "").await;
    assert_eq!((all["scope"].as_str(), all["totals"]["calls"].as_i64()), (Some("org"), Some(3)));
    let (_, one) = report(&app, &app.token, &format!("?user={}", member.id.to_string())).await;
    assert_eq!((one["scope"].as_str(), one["totals"]["calls"].as_i64()), (Some("self"), Some(2)));

    // a token without the vaults:admin scope cannot read the report at all
    let read_only = create_api_token_with(app.control(), &app.user.id, "ro", &[scopes::MEMORY_READ.to_string()], None, None).await.unwrap().token;
    let (status, body) = report(&app, &read_only, "").await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.scope")));
}

#[tokio::test]
async fn usage_never_crosses_orgs() {
    let app = TestApp::new().await;
    let other = eunomia_backend::models_user::register_user_with(&app.state, "other@example.com", common::PASSWORD, true).await.unwrap();
    assert_ne!(other.org, app.user.org);
    let other_token = create_api_token(app.control(), &other.id, "other-key").await.unwrap().token;
    rest(&app, &app.token, "recall", json!({"query": "a"})).await;
    rest(&app, &other_token, "recall", json!({"query": "b"})).await;
    rest(&app, &other_token, "recall", json!({"query": "c"})).await;

    let (_, r) = report(&app, &app.token, "").await;
    assert_eq!(r["totals"]["calls"], 1);
    assert!(!r.to_string().contains("other@example.com") && !r.to_string().contains("other-key"), "{r}");
    // the other org has no admin (admin is instance-wide): its user sees only their own calls
    let (_, theirs) = report(&app, &other_token, "").await;
    assert_eq!((theirs["scope"].as_str(), theirs["totals"]["calls"].as_i64()), (Some("self"), Some(2)));
}

#[tokio::test]
async fn refused_calls_count_as_errors() {
    let app = TestApp::new().await;
    let read_only = create_api_token_with(app.control(), &app.user.id, "ro", &[scopes::MEMORY_READ.to_string(), scopes::VAULTS_ADMIN.to_string()], None, None)
        .await
        .unwrap()
        .token;
    assert_eq!(rest(&app, &read_only, "recall", json!({"query": "a"})).await.0, StatusCode::OK);
    let (status, _) = rest(&app, &read_only, "memory_write", json!({"subject_name": "A", "subject_kind": "person", "text": "t"})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (_, r) = report(&app, &app.token, "").await;
    assert_eq!((r["totals"]["calls"].as_i64(), r["totals"]["errors"].as_i64()), (Some(2), Some(1)));
    assert_eq!(row(&r["by_tool"], "tool", "memory_write")["errors"], 1);
    assert_eq!(row(&r["by_credential"], "label", "ro")["errors"], 1);
}

/// Many simultaneous calls on one key land on one row and none is lost (the upsert is atomic and
/// a commit conflict is retried).
#[tokio::test]
async fn concurrent_calls_all_count() {
    let app = TestApp::new().await;
    let calls = (0..24).map(|i| {
        let router = app.router.clone();
        let token = app.token.clone();
        async move { send(&router, "POST", "/api/tools/recall", Some(json!({"query": format!("q{i}")})), &token, &[]).await.0 }
    });
    for status in futures::future::join_all(calls).await {
        assert_eq!(status, StatusCode::OK);
    }
    let (_, r) = report(&app, &app.token, "").await;
    assert_eq!(r["totals"]["calls"], 24);
    // one hourly row for the one key, not one per call
    let mut res = app.control().test_raw().query("SELECT count() AS n FROM tool_usage GROUP ALL").await.unwrap();
    let n: Vec<Value> = res.take(0).unwrap();
    assert!(n[0]["n"].as_i64().unwrap() <= 2, "{n:?}"); // 2 only if the clock crossed an hour mid-test
}

#[tokio::test]
async fn old_hours_are_pruned_and_recent_ones_kept() {
    let app = TestApp::new().await;
    rest(&app, &app.token, "recall", json!({"query": "a"})).await;
    usage::settle().await;
    let uid = app.user.id.to_string();
    app.control()
        .test_raw()
        .query(format!(
            "CREATE tool_usage:old SET org = 'x', bucket = time::now() - 200d, user = {uid}, actor_kind = 'user', actor_id = '', channel = 'rest', tool = 'recall', calls = 5"
        ))
        .await
        .unwrap()
        .check()
        .unwrap();
    usage::prune(app.control(), 90).await.unwrap();
    let mut res = app.control().test_raw().query("SELECT VALUE id FROM tool_usage").await.unwrap();
    let ids: Vec<surrealdb::types::RecordId> = res.take(0).unwrap();
    assert_eq!(ids.len(), 1, "{ids:?}");
    assert_ne!(ids[0].to_string(), "tool_usage:old");
    // 0 keeps everything
    usage::prune(app.control(), 0).await.unwrap();
}
