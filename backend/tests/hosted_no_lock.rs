//! The seat limit under racing writers when the in-process lock is off, so the writers meet at the transaction
//! the way writers on two replicas do (`tx::LOCKS_OFF`, like `no_lock.rs`). The seat guard row (`store/plan.rs`) must
//! make one commit and the other retry. Run in its own binary because the switch is process-global.

mod common;

use axum::http::StatusCode;
use eunomia_backend::config::Settings;
use eunomia_backend::mail::MailBackend;
use eunomia_backend::plan::HostedConfig;
use serde_json::{json, Value};

const TOKEN: &str = "operator-token-for-tests-0123456789abcdef";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_invitations_for_the_last_seat_meet_at_the_database_not_the_lock() {
    // SAFETY: set before any test code reads them, to the values the hosted tests use
    unsafe {
        std::env::set_var("EUNOMIA_SIGNUP_ORG", "personal");
        std::env::set_var("SIGNUP", "closed");
        std::env::set_var("RATE_LIMIT_AUTH_PER_MIN", "1000");
    }
    let hosted = HostedConfig { operator_token: Some(TOKEN.into()), ..Default::default() };
    let app = common::TestApp::with_settings(Settings { mail: MailBackend::Capture, hosted, ..common::test_settings() }).await;
    let ((status, made), _) = common::http(
        &app.router,
        "POST",
        "/api/operator/v1/orgs",
        Some(json!({ "name": "Racers", "owner_email": "owner@race.test", "plan": "team", "seat_limit": 4, "external_ref": "race" })),
        Some(TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made}");
    let token = made["claim_link"].as_str().unwrap().split_once("/invite#").unwrap().1.to_string();
    let ((status, body), cookie) = common::http(
        &app.router,
        "POST",
        "/api/auth/register",
        Some(json!({ "email": "owner@race.test", "password": common::PASSWORD, "invitation": token })),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let cookie = cookie.unwrap().split(';').next().unwrap().to_string();

    eunomia_backend::tx::LOCKS_OFF.store(true, std::sync::atomic::Ordering::SeqCst);
    // owner + 0 pending = 1 of 4: three seats, eight invitations at once
    let mut tasks = Vec::new();
    for i in 0..8 {
        let (router, cookie) = (app.router.clone(), cookie.clone());
        tasks.push(tokio::spawn(async move {
            common::http(&router, "POST", "/api/org/invitations", Some(json!({ "email": format!("racer{i}@race.test"), "role": "member" })), None, Some(&cookie)).await.0
        }));
    }
    let mut results: Vec<(StatusCode, Value)> = Vec::new();
    for t in tasks {
        results.push(t.await.unwrap());
    }
    eunomia_backend::tx::LOCKS_OFF.store(false, std::sync::atomic::Ordering::SeqCst);
    let won = results.iter().filter(|(s, _)| *s == StatusCode::OK).count();
    let seats: Vec<Value> = app.control().test_raw().query("SELECT count() AS n FROM org_invitation WHERE role = 'member' AND accepted_at IS NONE GROUP ALL").await.unwrap().take(0).unwrap();
    let pending = seats.first().and_then(|r| r["n"].as_i64()).unwrap_or(0);
    // never more than the three seats; every refusal is the seat limit (a lost commit race is retried inside, not shown)
    assert!(won <= 3 && pending <= 3, "{won} won, {pending} pending: {results:?}");
    assert_eq!(won, 3, "{results:?}");
    assert!(results.iter().filter(|(s, _)| *s != StatusCode::OK).all(|(s, b)| *s == StatusCode::CONFLICT && b["code"] == "org.seat_limit"), "{results:?}");
}
