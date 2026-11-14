//! Two-step verification with the in-process lock switched off (`tx::LOCKS_OFF`, `test-support` builds), so
//! concurrent requests meet at the database the way requests on separate replicas do. Proves the attempt
//! budgets and the single-use challenge rest on the database's conditional writes, not on the per-process
//! lock. Its own test binary: the switch is process-wide.

mod common;

use std::sync::atomic::Ordering;

use axum::http::StatusCode;
use eunomia_backend::mfa;
use serde_json::{json, Value};

struct LocksOff;

impl LocksOff {
    fn on() -> Self {
        eunomia_backend::tx::LOCKS_OFF.store(true, Ordering::SeqCst);
        LocksOff
    }
}

impl Drop for LocksOff {
    fn drop(&mut self) {
        eunomia_backend::tx::LOCKS_OFF.store(false, Ordering::SeqCst);
    }
}

fn pair(set_cookie: &Option<String>) -> String {
    set_cookie.as_deref().expect("a cookie").split(';').next().unwrap().to_string()
}

async fn post(app: &common::TestApp, path: &str, body: Value, cookie: Option<&str>) -> ((StatusCode, Value), Option<String>) {
    common::http(&app.router, "POST", path, Some(body), None, cookie).await
}

async fn login_pending(app: &common::TestApp) -> String {
    let ((status, body), set) = post(app, "/api/auth/login", json!({ "email": app.user.email, "password": common::PASSWORD }), None).await;
    assert_eq!((status, body["mfa_required"].as_str()), (StatusCode::OK, Some("verify")), "{body}");
    pair(&set)
}

/// Turns MFA on; returns the recovery codes.
async fn enrol(app: &common::TestApp, cookie: &str) -> Vec<String> {
    let ((status, e), _) = post(app, "/api/auth/mfa/enrol", json!({ "password": common::PASSWORD }), Some(cookie)).await;
    assert_eq!(status, StatusCode::OK, "{e}");
    let code = mfa::code_at(e["secret"].as_str().unwrap(), u64::try_from(chrono::Utc::now().timestamp()).unwrap()).unwrap();
    let ((status, done), _) = post(app, "/api/auth/mfa/enrol/confirm", json!({ "code": code }), Some(cookie)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    done["recovery_codes"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect()
}

async fn failed_evaluations(app: &common::TestApp) -> usize {
    let mut res = app.control().test_raw().query("SELECT count() FROM audit_event WHERE action = 'auth.mfa_failed' GROUP ALL").await.unwrap();
    let rows: Vec<Value> = res.take(0).unwrap();
    rows.first().and_then(|r| r["count"].as_u64()).unwrap_or(0) as usize
}

/// Two answers to one sign-in at once, each with its own valid recovery code, as two replicas would see
/// them: the challenge's conditional delete gives the session to at most one of them.
#[tokio::test]
async fn two_replicas_answering_one_sign_in_give_at_most_one_session() {
    let app = common::TestApp::new().await;
    let cookie = app.session_cookie().await;
    let codes = enrol(&app, &cookie).await;
    let _off = LocksOff::on();
    let mut sessions = 0;
    for pair_of_codes in codes.chunks(2).take(4) {
        let pending = login_pending(&app).await;
        let (a, b) = tokio::join!(
            post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": pair_of_codes[0] }), Some(&pending)),
            post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": pair_of_codes[1] }), Some(&pending)),
        );
        let ok: Vec<_> = [&a, &b].into_iter().filter(|r| r.0 .0 == StatusCode::OK).collect();
        assert!(ok.len() <= 1, "one sign-in, two sessions: {:?} {:?}", a.0, b.0);
        for r in [&a, &b] {
            let s = r.0 .0;
            assert!(s == StatusCode::OK || s == StatusCode::UNAUTHORIZED || s == StatusCode::CONFLICT, "{s} {}", r.0 .1);
        }
        sessions += ok.len();
    }
    assert!(sessions >= 1, "the sign-ins do finish");
}

/// The per-challenge and per-account budgets hold with many parallel wrong codes and no in-process lock:
/// the reservations are conditional UPDATEs in the database. A lost write race is refused (`db.conflict`)
/// before any code is checked, so it can only lower the number of codes evaluated.
#[tokio::test]
async fn attempt_budgets_hold_without_the_in_process_lock() {
    let app = common::TestApp::new().await;
    let cookie = app.session_cookie().await;
    enrol(&app, &cookie).await;
    let _off = LocksOff::on();

    let pending = login_pending(&app).await;
    let calls = (0..30).map(|i| {
        let (router, pending) = (app.router.clone(), pending.clone());
        async move { common::http(&router, "POST", "/api/auth/mfa/challenge/verify", Some(json!({ "code": format!("{:06}", 300_000 + i) })), None, Some(&pending)).await.0 }
    });
    let answers = futures::future::join_all(calls).await;
    let on_challenge = failed_evaluations(&app).await;
    assert!(on_challenge <= mfa::CHALLENGE_MAX_FAILS as usize, "{on_challenge} codes checked on one challenge: {answers:?}");

    app.control().test_raw().query("UPDATE session SET mfa_at = time::now() - 1h WHERE owner = $u").bind(("u", app.user.id.clone())).await.unwrap().check().unwrap();
    app.state.credentials.clear();
    let calls = (0..40).map(|_| {
        let (router, cookie) = (app.router.clone(), cookie.clone());
        async move { common::http(&router, "POST", "/api/auth/tokens", Some(json!({ "name": "x", "mfa_code": "222222" })), None, Some(&cookie)).await.0 }
    });
    let answers = futures::future::join_all(calls).await;
    let total = failed_evaluations(&app).await;
    assert!(total <= mfa::MFA_FAILS as usize, "{total} codes checked against a budget of {}: {answers:?}", mfa::MFA_FAILS);
    for (s, b) in &answers {
        assert!(*s == StatusCode::UNAUTHORIZED || *s == StatusCode::TOO_MANY_REQUESTS || *s == StatusCode::CONFLICT, "{s} {b}");
    }
    let tokens: Vec<Value> = app.control().test_raw().query("SELECT * FROM api_token WHERE name = 'x'").await.unwrap().take(0).unwrap();
    assert!(tokens.is_empty());
}
