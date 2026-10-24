//! HTTP route tests against the real router and an in-memory SurrealDB.

mod common;

use axum::http::StatusCode;
use common::{http, TestApp, PASSWORD};
use serde_json::json;

#[tokio::test]
async fn signup_then_login_issues_a_session_cookie() {
    let state = common::bare_state().await;
    let router = eunomia_backend::app(state);
    let creds = json!({"email": "new@example.com", "password": PASSWORD});

    let ((status, body), cookie) = http(&router, "POST", "/api/auth/register", Some(creds.clone()), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["email"], "new@example.com");
    assert!(cookie.unwrap().starts_with("eunomia_session="));

    let ((status, _), _) = http(&router, "POST", "/api/auth/register", Some(creds.clone()), None, None).await;
    assert_eq!(status, StatusCode::CONFLICT);

    let bad = json!({"email": "new@example.com", "password": "wrong-password"});
    let ((status, _), _) = http(&router, "POST", "/api/auth/login", Some(bad), None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let ((status, body), cookie) = http(&router, "POST", "/api/auth/login", Some(creds), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["onboarded"], false);
    let session = cookie.unwrap();
    let session = session.split(';').next().unwrap().to_string();

    let ((status, body), _) = http(&router, "GET", "/api/auth/me", None, None, Some(&session)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["email"], "new@example.com");
}

/// Issue #76: on a default install a second person signs up (even with the first one's password),
/// gets their own personal vault in the install's org, and the first user is unaffected.
#[tokio::test]
async fn a_second_user_can_sign_up_on_a_default_install() {
    let state = common::bare_state().await;
    let router = eunomia_backend::app(state.clone());
    let signup = |email: &str| json!({"email": email, "password": PASSWORD});
    let ((status, first), _) = http(&router, "POST", "/api/auth/register", Some(signup("root@example.com")), None, None).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let ((status, second), cookie) = http(&router, "POST", "/api/auth/register", Some(signup("second@example.com")), None, None).await;
    assert_eq!(status, StatusCode::OK, "{second}");
    let session = cookie.unwrap().split(';').next().unwrap().to_string();
    let ((status, vaults), _) = http(&router, "GET", "/api/vaults", None, None, Some(&session)).await;
    assert_eq!(status, StatusCode::OK, "{vaults}");
    let personal: Vec<_> = vaults["results"].as_array().unwrap().iter().filter(|v| v["kind"] == "personal").collect();
    assert_eq!(personal.len(), 1, "the second user has exactly their own personal vault: {vaults}");
    let ((status, _), _) = http(&router, "POST", "/api/auth/login", Some(signup("root@example.com")), None, None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn protected_routes_return_401_without_auth() {
    let app = TestApp::new().await;
    for (method, path) in [("GET", "/api/auth/me"), ("GET", "/api/vaults"), ("POST", "/api/tools/vault_list")] {
        let (status, _) = app.http(method, path, None, false).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
    }
    let (status, body) = app.http("GET", "/healthz", None, false).await;
    assert_eq!((status, body["status"].as_str()), (StatusCode::OK, Some("ok")));
}

#[tokio::test]
async fn vault_routes_list_and_create() {
    let app = TestApp::new().await;
    let (status, body) = app.http("GET", "/api/vaults", None, true).await;
    assert_eq!(status, StatusCode::OK);
    let vaults = body.as_array().or(body["results"].as_array()).expect("vault list");
    assert_eq!(vaults.len(), 1);
    assert_eq!(vaults[0]["kind"], "personal");

    let (status, created) = app.http("POST", "/api/vaults", Some(json!({"name": "Team"})), true).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["name"], "Team");

    let (_, body) = app.http("GET", "/api/vaults", None, true).await;
    let n = body.as_array().or(body["results"].as_array()).unwrap().len();
    assert_eq!(n, 2);
}

#[tokio::test]
async fn tool_route_runs_a_registry_tool() {
    let app = TestApp::new().await;
    let (status, body) = app.http("POST", "/api/tools/docs", Some(json!({})), true).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["docs"].as_array().is_some_and(|d| !d.is_empty()));
    let (status, _) = app.http("POST", "/api/tools/not_a_tool", Some(json!({})), true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// One request returning (status, headers, parsed body), for header-level assertions.
async fn raw(
    app: &TestApp,
    path: &str,
    extra: &[(&str, &str)],
    bearer: bool,
) -> (StatusCode, axum::http::HeaderMap, serde_json::Value) {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let mut req = axum::http::Request::builder().method("GET").uri(path);
    if bearer {
        req = req.header("authorization", format!("Bearer {}", app.token));
    }
    for (k, v) in extra {
        req = req.header(*k, *v);
    }
    let resp = app.router.clone().oneshot(req.body(axum::body::Body::empty()).unwrap()).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, headers, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
}

fn is_trace_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[tokio::test]
async fn not_found_is_problem_json_with_code_and_matching_trace_id() {
    let app = TestApp::new().await;
    let (status, headers, body) = raw(&app, "/api/vaults/not-a-vault/members", &[], true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(headers["content-type"], "application/problem+json");
    assert_eq!(body["code"], "vault.not_found");
    assert_eq!(body["type"], "about:blank");
    assert_eq!(body["status"], 404);
    assert!(body["title"].is_string() && body["detail"].is_string());
    let trace_id = body["trace_id"].as_str().unwrap();
    assert!(is_trace_id(trace_id), "{trace_id}");
    assert_eq!(headers["x-trace-id"], trace_id);
}

#[tokio::test]
async fn unauthorized_has_its_code_and_every_response_has_a_trace_id_header() {
    let app = TestApp::new().await;
    let (status, headers, body) = raw(&app, "/api/auth/me", &[], false).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "auth.unauthorized");
    assert_eq!(headers["x-trace-id"], body["trace_id"].as_str().unwrap());

    let (status, headers, _) = raw(&app, "/healthz", &[], false).await;
    assert_eq!(status, StatusCode::OK);
    assert!(is_trace_id(headers["x-trace-id"].to_str().unwrap()));
}

#[tokio::test]
async fn incoming_traceparent_trace_id_is_echoed() {
    let app = TestApp::new().await;
    let tp = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
    let (_, headers, body) = raw(&app, "/api/auth/me", &[("traceparent", tp)], false).await;
    assert_eq!(headers["x-trace-id"], "0af7651916cd43dd8448eb211c80319c");
    assert_eq!(body["trace_id"], "0af7651916cd43dd8448eb211c80319c");

    // a malformed header starts a new trace instead of being echoed
    let (_, headers, _) = raw(&app, "/healthz", &[("traceparent", "garbage")], false).await;
    assert!(is_trace_id(headers["x-trace-id"].to_str().unwrap()));
}

#[tokio::test]
async fn cors_allows_traceparent_and_exposes_trace_id() {
    use tower::ServiceExt;
    let app = TestApp::new().await;
    let req = axum::http::Request::builder()
        .method("OPTIONS")
        .uri("/api/vaults")
        .header("origin", "http://localhost:3000")
        .header("access-control-request-method", "GET")
        .header("access-control-request-headers", "traceparent")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert!(resp.headers()["access-control-allow-headers"].to_str().unwrap().contains("traceparent"));

    let (_, headers, _) = raw(&app, "/healthz", &[("origin", "http://localhost:3000")], false).await;
    assert!(headers["access-control-expose-headers"].to_str().unwrap().contains("x-trace-id"));
}

#[tokio::test]
async fn tool_errors_carry_code_and_trace_id() {
    let app = TestApp::new().await;
    let v = app.tool("entities_get", json!({ "entity_id": "not a record id" })).await;
    assert!(v["error"].is_string());
    assert_eq!(v["code"], "validation.invalid");
    assert!(is_trace_id(v["trace_id"].as_str().unwrap()));
}
