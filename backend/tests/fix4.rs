//! Round 4 review fixes: per-account sign-in throttle, signup cleanup, reserved operator emails,
//! the model base URL guard, the normalized email index, OAuth audit rows.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::{http, TestApp, PASSWORD};
use eunomia_backend::models_user;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

/// Env switches are process-global: tests that set one take this lock.
static ENV: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn login(app: &TestApp, email: &str, password: &str, xff: &str) -> (StatusCode, Option<String>) {
    let req = Request::builder()
        .method("POST")
        .uri("/api/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-forwarded-for", xff)
        .body(Body::from(json!({ "email": email, "password": password }).to_string()))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let retry = resp.headers().get(header::RETRY_AFTER).and_then(|v| v.to_str().ok()).map(str::to_string);
    (resp.status(), retry)
}

#[tokio::test]
async fn failed_logins_are_throttled_per_account_and_peer_despite_spoofed_headers() {
    let app = TestApp::new().await;
    // a different spoofed header every time (the peer is the same, so the lockout key is too: the header is not trusted)
    for i in 0..10 {
        let (status, _) = login(&app, "Tester@Example.com", "wrong-password", &format!("198.51.100.{i}")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    // the account is now locked, even for the right password, and the answer says when to come back
    let (status, retry) = login(&app, "tester@example.com", PASSWORD, "198.51.100.200").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(retry.is_some_and(|r| r.parse::<u64>().unwrap() >= 1));
    // other accounts are unaffected
    let (status, _) = login(&app, "someone-else@example.com", "wrong-password", "198.51.100.201").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_successful_login_clears_the_failure_count() {
    let app = TestApp::new().await;
    for i in 0..5 {
        assert_eq!(login(&app, "tester@example.com", "wrong-password", &format!("203.0.113.{i}")).await.0, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(login(&app, "tester@example.com", PASSWORD, "203.0.113.50").await.0, StatusCode::OK);
    for i in 0..9 {
        assert_eq!(login(&app, "tester@example.com", "wrong-password", &format!("203.0.113.{}", 60 + i)).await.0, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(login(&app, "tester@example.com", PASSWORD, "203.0.113.99").await.0, StatusCode::OK, "5 + 9 failures with a success between is under the limit");
}

#[tokio::test]
async fn failed_token_exchanges_are_throttled_per_client() {
    let app = TestApp::new().await;
    let post_from = |client: &'static str, xff: &'static str| {
        let body = serde_urlencoded::to_string([("grant_type", "refresh_token"), ("refresh_token", "nope"), ("client_id", client)]).unwrap();
        let req = Request::builder().method("POST").uri("/oauth/token").header(header::CONTENT_TYPE, "application/x-www-form-urlencoded").body(Body::from(body)).unwrap();
        let mut req = req;
        // no socket under `oneshot`: say which peer sent it
        req.extensions_mut().insert(axum::extract::ConnectInfo(std::net::SocketAddr::new(xff.parse().unwrap(), 5000)));
        let router = app.router.clone();
        async move {
            let resp = router.oneshot(req).await.unwrap();
            let retry = resp.headers().contains_key(header::RETRY_AFTER);
            let status = resp.status();
            let body: Value = serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap_or(Value::Null);
            (status, retry, body)
        }
    };
    let post = |client: &'static str| post_from(client, "203.0.113.7");
    for i in 0..30 {
        assert_eq!(post("victim-client").await.0, StatusCode::BAD_REQUEST, "attempt {i}");
    }
    let (status, retry, body) = post("victim-client").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert!(retry);
    assert_eq!(body["code"], "rate.limited");
    assert_eq!(post("another-client").await.0, StatusCode::BAD_REQUEST);
    // keyed on the address too: knowing a client id does not let a stranger lock its real users out
    assert_eq!(post_from("victim-client", "203.0.113.8").await.0, StatusCode::BAD_REQUEST);
    // a CIMD client id is a public URL shared by every user of that app: a stranger's failures
    // must not lock it for everyone (the per-address bucket covers abuse instead)
    for i in 0..31 {
        assert_eq!(post("https://app.example/client.json").await.0, StatusCode::BAD_REQUEST, "cimd attempt {i}");
    }
}

#[tokio::test]
async fn a_failed_signup_leaves_no_user_membership_or_vault() {
    let app = TestApp::new().await;
    *models_user::FAIL_SETUP_FOR.lock().unwrap() = Some("doomed@example.com".into());
    let err = models_user::register_user(&app.state, "doomed@example.com", PASSWORD).await.expect_err("injected failure");
    *models_user::FAIL_SETUP_FOR.lock().unwrap() = None;
    assert_eq!(err.code, eunomia_backend::error::ErrorCode::Internal);

    let control = app.control().clone();
    let count = |q: &'static str| {
        let control = control.clone();
        async move {
        let mut res = control.test_raw().query(q).await.unwrap();
        res.take::<Option<i64>>("count").unwrap().unwrap_or(0)
        }
    };
    assert_eq!(count("SELECT count() FROM user GROUP ALL").await, 1, "only the first user");
    assert_eq!(count("SELECT count() FROM membership GROUP ALL").await, 1);
    // and the address can sign up again
    assert!(models_user::register_user(&app.state, "doomed@example.com", PASSWORD).await.is_ok());
}

async fn register_http(app: &TestApp, email: &str) -> StatusCode {
    http(&app.router, "POST", "/api/auth/register", Some(json!({ "email": email, "password": PASSWORD })), None, None).await.0 .0
}

#[tokio::test]
async fn a_listed_operator_address_cannot_be_claimed_by_signup() {
    let _e = ENV.lock().await;
    let app = TestApp::new().await;
    // SAFETY: serialised by ENV; nothing else in this binary reads it
    unsafe { std::env::set_var("EUNOMIA_ADMIN_EMAILS", "boss@example.com, Ops@example.com") };
    assert_eq!(register_http(&app, "Boss@Example.com").await, StatusCode::FORBIDDEN);
    assert_eq!(register_http(&app, "ops@example.com").await, StatusCode::FORBIDDEN);
    assert_eq!(register_http(&app, "ordinary@example.com").await, StatusCode::OK);
    // an account made before the address was listed is the operator
    unsafe { std::env::set_var("EUNOMIA_ADMIN_EMAILS", "ordinary@example.com") };
    let ordinary = models_user::authenticate(app.control(), "ordinary@example.com", PASSWORD).await.unwrap().unwrap();
    assert!(eunomia_backend::authz::is_operator(app.control(), &ordinary).await.unwrap());
    unsafe { std::env::remove_var("EUNOMIA_ADMIN_EMAILS") };
}

async fn set_base_url(app: &TestApp, url: &str) -> StatusCode {
    app.http("PATCH", "/api/settings", Some(json!({ "openai_base_url": url })), true).await.0
}

#[tokio::test]
async fn the_model_base_url_cannot_reach_metadata_or_the_stack() {
    let _e = ENV.lock().await;
    let app = TestApp::new().await;
    for bad in [
        "http://169.254.169.254/latest/meta-data",
        "http://[::ffff:169.254.169.254]/",
        "http://surrealdb:8000/",
        "http://backup/v1",
        "http://metadata.google.internal/",
        "file:///etc/passwd",
        "http://user:pw@127.0.0.1:11434/v1",
        "not a url",
    ] {
        assert_eq!(set_base_url(&app, bad).await, StatusCode::BAD_REQUEST, "{bad}");
    }
    // a model server on this machine is the normal self-host setup
    assert_eq!(set_base_url(&app, "http://127.0.0.1:11434/v1").await, StatusCode::OK);
    assert_eq!(set_base_url(&app, "http://localhost:11434/v1").await, StatusCode::OK);
    // and an operator can close that too
    unsafe { std::env::set_var("ALLOW_PRIVATE_LLM_URL", "0") };
    let (a, b) = (set_base_url(&app, "http://127.0.0.1:11434/v1").await, set_base_url(&app, "http://10.1.2.3/v1").await);
    unsafe { std::env::remove_var("ALLOW_PRIVATE_LLM_URL") };
    assert_eq!((a, b), (StatusCode::BAD_REQUEST, StatusCode::BAD_REQUEST));
}

#[tokio::test]
async fn email_lookup_uses_the_normalized_column_and_its_unique_index() {
    let app = TestApp::new().await;
    models_user::register_user(&app.state, "  Mixed.Case@Example.COM ", PASSWORD).await.unwrap();
    assert!(models_user::authenticate(app.control(), "mixed.case@example.com", PASSWORD).await.unwrap().is_some());
    assert!(models_user::authenticate(app.control(), "MIXED.CASE@example.com", PASSWORD).await.unwrap().is_some());
    let mut res = app.control().test_raw().query("SELECT email, email_lc FROM user WHERE email_lc = 'mixed.case@example.com'").await.unwrap();
    let rows: Vec<Value> = res.take(0).unwrap();
    assert_eq!(rows.len(), 1);
    // the index, not application code, stops a second account for the same address
    let err = app.control().test_raw().query("CREATE user SET email = 'MIXED.case@example.com', email_lc = 'mixed.case@example.com', password_hash = 'x'").await.unwrap().check().expect_err("unique");
    assert!(err.to_string().contains("already contains"), "{err}");
}

#[tokio::test]
async fn the_email_migration_backfills_and_keeps_the_oldest_duplicate() {
    let app = TestApp::new().await;
    let raw = app.control().test_raw();
    raw.query("REMOVE INDEX user_email_lc_unique ON user; REMOVE INDEX membership_slot_unique ON membership;").await.unwrap().check().unwrap();
    raw.query(
        "CREATE user:old SET email = 'Dup@Example.com ', password_hash = 'x', created_at = time::now() - 2d, email_lc = NONE;
         CREATE user:newer SET email = 'dup@example.com', password_hash = 'x', created_at = time::now() - 1d, email_lc = NONE;
         CREATE user:other SET email = 'Other@Example.com', password_hash = 'x', email_lc = NONE;",
    )
    .await
    .unwrap()
    .check()
    .unwrap();
    raw.query(include_str!("../migrations/control/0004_email_lc_and_owner_slot.surql")).await.unwrap().check().unwrap();
    let mut res = raw.query("SELECT VALUE email_lc FROM user WHERE id IN [user:old, user:newer, user:other] ORDER BY id").await.unwrap();
    let got: Vec<String> = res.take(0).unwrap();
    assert!(got.contains(&"dup@example.com".to_string()) && got.contains(&"other@example.com".to_string()), "{got:?}");
    let mut res = raw.query("SELECT VALUE id FROM user WHERE email_lc = 'dup@example.com'").await.unwrap();
    let owner: Vec<surrealdb::types::RecordId> = res.take(0).unwrap();
    assert_eq!(owner.len(), 1);
    assert_eq!(eunomia_backend::rid::key_string(&owner[0].key).unwrap(), "old", "the oldest keeps the address");
    assert!(got.iter().any(|e| e.starts_with("duplicate:")), "{got:?}");
}

async fn audit_actions(app: &TestApp, action: &str) -> Vec<Value> {
    let mut res = app.control().test_raw().query("SELECT action, target, outcome, detail FROM audit_event WHERE action = $a").bind(("a", action.to_string())).await.unwrap();
    res.take(0).unwrap()
}

#[tokio::test]
async fn consent_decisions_and_grant_revocations_leave_audit_rows() {
    let app = TestApp::new().await;
    let redirect = "http://127.0.0.1:7777/callback";
    let (_, reg) = http(&app.router, "POST", "/oauth/register", Some(json!({"client_name": "Agent <b>evil</b>", "redirect_uris": [redirect], "token_endpoint_auth_method": "none"})), None, None).await.0;
    let client_id = reg["client_id"].as_str().unwrap().to_string();
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    use base64::Engine;
    use sha2::Digest;
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()));
    let params = |approve: bool| json!({
        "response_type": "code", "client_id": client_id, "redirect_uri": redirect, "code_challenge": challenge,
        "code_challenge_method": "S256", "state": "free text from the client", "resource": "http://localhost:8001/mcp", "approve": approve,
    });

    let (status, _) = app.http_session("POST", "/api/oauth/consent", Some(params(false))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, res) = app.http_session("POST", "/api/oauth/consent", Some(params(true))).await;
    assert_eq!(status, StatusCode::OK);
    let rows = audit_actions(&app, "oauth.consent").await;
    let outcomes: Vec<&str> = rows.iter().map(|r| r["outcome"].as_str().unwrap()).collect();
    assert!(outcomes.contains(&"ok") && outcomes.contains(&"access_denied"), "{rows:?}");
    assert!(rows.iter().all(|r| r["target"] == client_id.as_str() && r["detail"] == ""), "{rows:?}");
    assert!(!serde_json::to_string(&rows).unwrap().contains("free text"));

    // redeem the code so a grant exists, then disconnect it
    let code = reqwest::Url::parse(res["redirect_to"].as_str().unwrap()).unwrap().query_pairs().find(|(k, _)| k == "code").unwrap().1.into_owned();
    let body = serde_urlencoded::to_string([
        ("grant_type", "authorization_code"), ("code", code.as_str()), ("redirect_uri", redirect), ("client_id", client_id.as_str()),
        ("code_verifier", verifier), ("resource", "http://localhost:8001/mcp"),
    ])
    .unwrap();
    let req = Request::builder().method("POST").uri("/oauth/token").header(header::CONTENT_TYPE, "application/x-www-form-urlencoded").body(Body::from(body)).unwrap();
    assert_eq!(app.router.clone().oneshot(req).await.unwrap().status(), StatusCode::OK);
    let (_, grants) = app.http_session("GET", "/api/oauth/grants", None).await;
    let id = grants[0]["id"].as_str().unwrap();
    let (status, _) = app.http_session("DELETE", &format!("/api/oauth/grants/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    let rows = audit_actions(&app, "oauth.grant_revoke").await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0]["target"].as_str().unwrap().ends_with(id) && rows[0]["outcome"] == "ok");
}
