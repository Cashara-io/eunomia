//! Round 2 hardening: admin-only updater, signup policy, cookie flags, capsule arguments off by
//! default, retention of audit rows and sessions, and no authorization check without a caller.
//! Tests that set environment variables share one binary and run in one test each, in sequence.

mod common;

use axum::http::{header, StatusCode};
use common::{http, TestApp, PASSWORD};
use eunomia_backend::telemetry::with_trace_id;
use eunomia_backend::tools::registry;
use serde_json::json;

fn set(key: &str, value: &str) {
    // SAFETY: each env-changing test is the only test in this binary touching that variable
    unsafe { std::env::set_var(key, value) };
}

fn unset(key: &str) {
    // SAFETY: as above
    unsafe { std::env::remove_var(key) };
}

#[tokio::test]
async fn an_authorization_check_without_a_caller_fails_closed() {
    let app = TestApp::new().await;
    let err = registry::call(&app.state, &app.user, "recall", json!({"query": "x"})).await.unwrap_err();
    assert_eq!(err.code.as_str(), "internal", "{err:?}");
    let ok = common::sys(registry::call(&app.state, &app.user, "recall", json!({"query": "x"}))).await;
    assert!(ok.is_ok());
}

#[tokio::test]
async fn only_an_instance_admin_can_trigger_the_updater() {
    let app = TestApp::new().await; // the first user is the admin
    let member = common::register(&app.state, "member@example.com").await;
    let member_token = eunomia_backend::models_user::create_api_token(&app.state.control, &member.id, "m").await.unwrap().token;
    for path in ["/api/update/request", "/api/update/check"] {
        let (status, body) = http(&app.router, "POST", path, None, Some(&member_token), None).await.0;
        assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "{path} {body}");
        let (status, body) = app.http("POST", path, None, true).await;
        assert_eq!(status, StatusCode::OK, "{path} {body}");
    }
    // reading the status stays open to every signed-in user
    let (status, _) = http(&app.router, "GET", "/api/update/status", None, Some(&member_token), None).await.0;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn signup_policy_and_cookie_flags_follow_the_environment() {
    let app = TestApp::new().await; // one user exists already
    let register = |email: &'static str| {
        let router = app.router.clone();
        async move { http(&router, "POST", "/api/auth/register", Some(json!({"email": email, "password": PASSWORD})), None, None).await }
    };

    set("SIGNUP", "closed");
    let ((status, body), _) = register("late@example.com").await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "{body}");
    assert!(body["detail"].as_str().unwrap().contains("closed"), "the refusal says why: {body}");

    set("SIGNUP", "invite");
    set("SIGNUP_ALLOWLIST", "invited@example.com");
    let ((status, body), _) = register("stranger@example.com").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(body["detail"].as_str().unwrap().contains("SIGNUP_ALLOWLIST"), "{body}");
    assert_eq!(register("Invited@example.com").await.0.0, StatusCode::OK);

    unset("SIGNUP");
    unset("SIGNUP_ALLOWLIST");
    assert_eq!(register("anyone@example.com").await.0.0, StatusCode::OK, "default is open");

    // cookie Secure flag: auto honours X-Forwarded-Proto from a trusted proxy peer, not from anyone else
    let login = |proto: Option<&'static str>, peer: [u8; 4]| {
        let router = app.router.clone();
        let body = json!({"email": "tester@example.com", "password": PASSWORD}).to_string();
        async move {
            let mut req = axum::http::Request::builder().method("POST").uri("/api/auth/login").header(header::CONTENT_TYPE, "application/json");
            if let Some(p) = proto {
                req = req.header("x-forwarded-proto", p);
            }
            let mut req = req.body(axum::body::Body::from(body)).unwrap();
            req.extensions_mut().insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((peer, 4000))));
            let resp = tower::ServiceExt::oneshot(router, req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            resp.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap().to_string()
        }
    };
    unset("COOKIE_SECURE");
    assert!(login(Some("https"), [127, 0, 0, 1]).await.ends_with("; Secure"), "trusted proxy over https");
    assert!(!login(None, [127, 0, 0, 1]).await.contains("Secure"));
    assert!(!login(Some("https"), [203, 0, 113, 9]).await.contains("Secure"), "an untrusted peer cannot claim https");
    set("COOKIE_SECURE", "true");
    assert!(login(None, [127, 0, 0, 1]).await.ends_with("; Secure"));
    set("COOKIE_SECURE", "false");
    assert!(!login(Some("https"), [127, 0, 0, 1]).await.contains("Secure"));
    unset("COOKIE_SECURE");
}

#[tokio::test]
async fn capsules_keep_only_a_hash_and_shape_of_arguments_by_default() {
    unset("CAPSULE_ARGS");
    let app = TestApp::new().await;
    let args = json!({ "entity_id": "person:nobody", "summary": "my private diagnosis", "n": 3 });
    let trace = format!("{:032x}", 77);
    let out = with_trace_id(trace.clone(), common::sys(registry::call(&app.state, &app.user, "entity_update", args))).await.unwrap();
    assert_eq!(out["code"], "entity.not_found");

    let c = eunomia_backend::capsules::get(&app.state.control, &trace).await.unwrap().expect("capsule recorded");
    assert!(!c.args_stored());
    let stored = c.args.to_string();
    assert!(!stored.contains("diagnosis") && !stored.contains("person:nobody"), "{stored}");
    assert_eq!(c.args["shape"], json!({ "entity_id": "string", "summary": "string", "n": "number" }));
    assert_eq!(c.args["hash"].as_str().map(str::len), Some(64));

    let err = eunomia_backend::replay::replay(&app.state, &app.state.settings, &trace, 0).await.unwrap_err();
    assert!(err.message.contains("CAPSULE_ARGS"), "{}", err.message);
    let test = eunomia_backend::replay::emit_test(&c);
    assert!(test.contains("NOT stored") && test.contains("\"summary\": \"string\"") && !test.contains("diagnosis"), "{test}");
}

#[tokio::test]
async fn audit_rows_and_long_expired_sessions_are_pruned() {
    let app = TestApp::new().await;
    let control = app.state.control.clone();
    let raw = control.test_raw();
    raw.query(
        "CREATE audit_event SET actor_kind = 'user', action = 'old', outcome = 'ok', created_at = time::now() - 400d;
         CREATE audit_event SET actor_kind = 'system', action = 'recent', outcome = 'ok', created_at = time::now() - 10d;
         CREATE session SET owner = $u, sid = 'long-gone', expires_at = time::now() - 8d;
         CREATE session SET owner = $u, sid = 'just-expired', expires_at = time::now() - 1d;
         CREATE session SET owner = $u, sid = 'live', expires_at = time::now() + 1d;",
    )
    .bind(("u", app.user.id.clone()))
    .await
    .unwrap()
    .check()
    .unwrap();
    eunomia_backend::audit::prune(&control, 365, 7).await.unwrap();
    let mut res = raw.query("SELECT VALUE action FROM audit_event WHERE action IN ['old', 'recent'];  SELECT VALUE sid FROM session WHERE sid IN ['long-gone', 'just-expired', 'live']").await.unwrap();
    let (audit, sessions): (Vec<String>, Vec<String>) = (res.take(0).unwrap(), res.take(1).unwrap());
    assert_eq!(audit, vec!["recent"]);
    assert!(!sessions.contains(&"long-gone".to_string()) && sessions.contains(&"just-expired".to_string()) && sessions.contains(&"live".to_string()), "{sessions:?}");

    // 0 days keeps the audit ledger forever
    raw.query("CREATE audit_event SET actor_kind = 'user', action = 'ancient', outcome = 'ok', created_at = time::now() - 4000d").await.unwrap().check().unwrap();
    eunomia_backend::audit::prune(&control, 0, 7).await.unwrap();
    let mut res = raw.query("SELECT VALUE action FROM audit_event WHERE action = 'ancient'").await.unwrap();
    assert_eq!(res.take::<Vec<String>>(0).unwrap().len(), 1);
}
