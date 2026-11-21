//! Password recovery by email, changing a password while signed in, and invitation emails
//! (`recovery.rs`, `mail.rs`, control migration 0013). Real router, in-memory SurrealDB, and the capture
//! transport, which keeps messages in memory so a test reads the link out of the message.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use eunomia_backend::config::Settings;
use eunomia_backend::mail::{Mail, MailBackend};
use eunomia_backend::mfa;
use eunomia_backend::models_user;
use eunomia_backend::rid::RecordIdExt;
use serde_json::{json, Value};

const NEW_PASSWORD: &str = "a-brand-new-passphrase";

fn env_once() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: set once, before any test reads it, to the same value for every test in this binary
        unsafe {
            // every request in a test comes from one address ("unknown", no peer in a oneshot), and the gate's
            // own per-address login limit (20 a minute) would meet the flows here first
            std::env::set_var("RATE_LIMIT_AUTH_PER_MIN", "1000");
        }
    });
}

async fn app_with_mail() -> common::TestApp {
    env_once();
    common::TestApp::with_settings(Settings { mail: MailBackend::Capture, ..common::test_settings() }).await
}

async fn app_without_mail() -> common::TestApp {
    env_once();
    common::TestApp::new().await
}

async fn post(app: &common::TestApp, path: &str, body: Value, cookie: Option<&str>) -> (StatusCode, Value) {
    common::http(&app.router, "POST", path, Some(body), None, cookie).await.0
}

async fn forgot(app: &common::TestApp, email: &str) -> (StatusCode, Value) {
    post(app, "/api/auth/password/forgot", json!({ "email": email }), None).await
}

async fn reset(app: &common::TestApp, token: &str, password: &str) -> (StatusCode, Value) {
    post(app, "/api/auth/password/reset", json!({ "token": token, "password": password }), None).await
}

async fn login_with(app: &common::TestApp, email: &str, password: &str) -> ((StatusCode, Value), Option<String>) {
    common::http(&app.router, "POST", "/api/auth/login", Some(json!({ "email": email, "password": password })), None, None).await
}

fn cookie_of(set_cookie: Option<String>) -> String {
    set_cookie.expect("a session cookie").split(';').next().unwrap().to_string()
}

/// Waits (a background task sends the mail) until `n` messages are in the outbox.
async fn outbox(app: &common::TestApp, n: usize) -> Vec<Mail> {
    for _ in 0..100 {
        let sent = app.state.mailer.captured();
        if sent.len() >= n {
            return sent;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("expected {n} message(s), got {}", app.state.mailer.captured().len());
}

/// Gives a background send time it would need, then says how many messages exist.
async fn outbox_after_a_moment(app: &common::TestApp) -> usize {
    tokio::time::sleep(Duration::from_millis(300)).await;
    app.state.mailer.captured().len()
}

/// The token after `<marker>#` in a message.
fn token_in(mail: &Mail, marker: &str) -> String {
    let at = mail.body.find(&format!("{marker}#")).unwrap_or_else(|| panic!("no {marker} link in {}", mail.body));
    mail.body[at + marker.len() + 1..].split_whitespace().next().unwrap().to_string()
}

async fn user_row(app: &common::TestApp, email: &str) -> Value {
    let mut res = app
        .control()
        .test_raw()
        .query("SELECT reset_token_hash, reset_expires_at, reset_sent_at, password_hash FROM user WHERE email_lc = $e")
        .bind(("e", email.to_string()))
        .await
        .unwrap();
    res.take::<Vec<Value>>(0).unwrap().into_iter().next().expect("the account")
}

async fn exec(app: &common::TestApp, sql: &str) {
    app.control().test_raw().query(sql).await.unwrap().check().unwrap();
}

async fn audit(app: &common::TestApp, action: &str) -> Vec<Value> {
    let mut res = app
        .control()
        .test_raw()
        .query("SELECT user, actor_kind, action, target, outcome, detail FROM audit_event WHERE action = $a ORDER BY created_at")
        .bind(("a", action.to_string()))
        .await
        .unwrap();
    res.take(0).unwrap()
}

/// Everything the ledger holds, as one string, to look for a secret in.
async fn whole_ledger(app: &common::TestApp) -> String {
    let mut res = app.control().test_raw().query("SELECT * FROM audit_event").await.unwrap();
    serde_json::to_string(&res.take::<Vec<Value>>(0).unwrap()).unwrap()
}

#[tokio::test]
async fn a_reset_link_sets_a_new_password_once_and_ends_the_sessions() {
    let app = app_with_mail().await;
    let email = app.user.email.clone();
    let session = app.session_cookie().await;
    assert_eq!(common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&session)).await.0 .0, StatusCode::OK);

    let (status, body) = forgot(&app, &email).await;
    assert_eq!((status, &body), (StatusCode::OK, &json!({ "ok": true })));
    let mail = outbox(&app, 1).await.remove(0);
    assert_eq!(mail.to, email);
    assert!(mail.subject.contains("Reset"), "{}", mail.subject);
    assert!(mail.body.contains("http://localhost:8001/reset-password#"), "built from PUBLIC_URL: {}", mail.body);
    let token = token_in(&mail, "/reset-password");

    // only the token's hash is stored, on the account
    let row = user_row(&app, &email).await;
    assert_eq!(row["reset_token_hash"], json!(models_user::hash_token(&token)));
    assert!(!row.to_string().contains(&token));

    // a password that breaks the sign-up rules is refused and the link is not spent on it
    for weak in ["short", &"x".repeat(73)] {
        let (status, body) = reset(&app, &token, weak).await;
        assert_eq!((status, body["code"].as_str()), (StatusCode::BAD_REQUEST, Some("validation.invalid")), "{body}");
    }
    assert!(!user_row(&app, &email).await["reset_token_hash"].is_null(), "the link survives a refused password");

    let (status, body) = reset(&app, &token, NEW_PASSWORD).await;
    assert_eq!((status, &body), (StatusCode::OK, &json!({ "ok": true })));
    let row = user_row(&app, &email).await;
    assert!(row["reset_token_hash"].is_null() && row["reset_expires_at"].is_null(), "spent: {row}");

    // no session starts; the old password is dead, the new one works
    assert_eq!(login_with(&app, &email, common::PASSWORD).await.0 .0, StatusCode::UNAUTHORIZED);
    let ((status, me), set) = login_with(&app, &email, NEW_PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert!(set.is_some());
    // the session from before the reset has ended, in this process's cache too
    let ((status, body), _) = common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&session)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    // single use: the same link again is refused, with the same answer as one that never existed
    let (status, again) = reset(&app, &token, "another-passphrase-1").await;
    assert_eq!((status, again["code"].as_str()), (StatusCode::BAD_REQUEST, Some("auth.reset_invalid")), "{again}");
    let (status, never) = reset(&app, "not-a-real-token", "another-passphrase-1").await;
    assert_eq!((status, never["code"].as_str()), (StatusCode::BAD_REQUEST, Some("auth.reset_invalid")));
    assert_eq!(again["detail"], never["detail"]);
    assert_eq!(login_with(&app, &email, NEW_PASSWORD).await.0 .0, StatusCode::OK, "the second attempt changed nothing");

    // audited, and no secret is anywhere in the ledger or the mail log
    let asked = audit(&app, "auth.password_forgot").await;
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["outcome"], "ok");
    assert!(asked[0]["detail"].as_str().unwrap().contains("link=sent"), "{}", asked[0]);
    let done = audit(&app, "auth.password_reset").await;
    assert_eq!((done.len(), done[0]["outcome"].as_str()), (1, Some("ok")), "{done:?}");
    assert_eq!(done[0]["user"].as_str(), Some(app.user.id.to_string().as_str()));
    let ledger = whole_ledger(&app).await;
    for secret in [token.as_str(), NEW_PASSWORD, common::PASSWORD] {
        assert!(!ledger.contains(secret), "a secret reached the audit ledger");
    }
    common::assert_chained(app.control(), &["auth.password_forgot", "auth.password_reset"]).await;
}

#[tokio::test]
async fn an_unknown_email_gets_the_same_answer_and_no_mail() {
    let app = app_with_mail().await;
    let (known_status, known) = forgot(&app, &app.user.email).await;
    let (unknown_status, unknown) = forgot(&app, "nobody@example.com").await;
    assert_eq!((known_status, &known), (unknown_status, &unknown), "the answer does not say whether the account exists");
    assert_eq!(unknown, json!({ "ok": true }));
    // case and spacing are the same address
    let (status, _) = forgot(&app, "  Nobody@Example.com ").await;
    assert_eq!(status, StatusCode::OK);

    let sent = outbox(&app, 1).await;
    assert_eq!(sent.len(), 1, "only the account that exists is mailed");
    assert_eq!(outbox_after_a_moment(&app).await, 1);
    // each request left one row; the unknown address is on the instance ledger with nobody behind it
    let rows = audit(&app, "auth.password_forgot").await;
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().any(|r| r["detail"].as_str().unwrap().contains("nobody@example.com link=no_link") && r["user"].is_null()), "{rows:?}");
    // a malformed address is a plain validation error for everyone
    let (status, body) = forgot(&app, "not-an-address").await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::BAD_REQUEST, Some("validation.invalid")));
}

#[tokio::test]
async fn an_expired_link_is_refused_and_changes_nothing() {
    let app = app_with_mail().await;
    forgot(&app, &app.user.email).await;
    let token = token_in(&outbox(&app, 1).await[0], "/reset-password");
    // the link lives 30 minutes
    let row = user_row(&app, &app.user.email).await;
    let (sent, expires) = (row["reset_sent_at"].as_str().unwrap().to_string(), row["reset_expires_at"].as_str().unwrap().to_string());
    let seconds = (chrono::DateTime::parse_from_rfc3339(&expires).unwrap() - chrono::DateTime::parse_from_rfc3339(&sent).unwrap()).num_seconds();
    assert!((1799..=1800).contains(&seconds), "{seconds}");

    exec(&app, "UPDATE user SET reset_expires_at = time::now() - 1s").await;
    let (status, body) = reset(&app, &token, NEW_PASSWORD).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::GONE, Some("auth.reset_expired")), "{body}");
    assert_eq!(login_with(&app, &app.user.email, common::PASSWORD).await.0 .0, StatusCode::OK, "the password is unchanged");
    let rows = audit(&app, "auth.password_reset").await;
    assert_eq!((rows.len(), rows[0]["outcome"].as_str()), (1, Some("auth.reset_expired")));
}

#[tokio::test]
async fn asking_again_replaces_the_link_and_is_rate_limited_per_address_and_per_email() {
    let app = app_with_mail().await;
    let email = app.user.email.clone();
    forgot(&app, &email).await;
    let first = token_in(&outbox(&app, 1).await[0], "/reset-password");
    // within a minute of the last link no new one is issued (any replica: it is kept in the database)
    forgot(&app, &email).await;
    assert_eq!(outbox_after_a_moment(&app).await, 1);
    exec(&app, "UPDATE user SET reset_sent_at = time::now() - 2m").await;
    forgot(&app, &email).await;
    let second = token_in(&outbox(&app, 2).await[1], "/reset-password");
    assert_ne!(first, second);
    // the newer link replaced the older one
    let (status, body) = reset(&app, &first, NEW_PASSWORD).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::BAD_REQUEST, Some("auth.reset_invalid")));

    // three requests for one email in the window (the one inside the minute counted): the fourth sends
    // nothing, even with the minute long past, and the answer does not change
    exec(&app, "UPDATE user SET reset_sent_at = time::now() - 2m").await;
    let (status, body) = forgot(&app, &email).await;
    assert_eq!((status, body), (StatusCode::OK, json!({ "ok": true })));
    assert_eq!(outbox_after_a_moment(&app).await, 2, "past the per-email limit nothing is sent");
    let why: Vec<String> = audit(&app, "auth.password_forgot").await.iter().map(|r| r["detail"].as_str().unwrap().to_string()).collect();
    assert!(why.last().unwrap().contains("link=too_often"), "{why:?}");

    // per client address: the tenth request in the window is the last, then 429 with Retry-After
    let mut last = StatusCode::OK;
    for i in 0..12 {
        last = forgot(&app, &format!("someone{i}@example.com")).await.0;
        if last == StatusCode::TOO_MANY_REQUESTS {
            assert!(i >= 5, "limited too early at {i}");
            break;
        }
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
    let (status, body) = forgot(&app, "another@example.com").await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate.limited")), "{body}");
}

#[tokio::test]
async fn bad_links_are_counted_per_address_and_then_refused() {
    let app = app_with_mail().await;
    for _ in 0..10 {
        assert_eq!(reset(&app, "guess", NEW_PASSWORD).await.0, StatusCode::BAD_REQUEST);
    }
    let (status, body) = reset(&app, "guess", NEW_PASSWORD).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate.limited")), "{body}");
}

#[tokio::test]
async fn a_reset_never_weakens_two_step_verification() {
    let app = app_with_mail().await;
    let email = app.user.email.clone();
    let cookie = app.session_cookie().await;
    let ((status, e), _) = common::http(&app.router, "POST", "/api/auth/mfa/enrol", Some(json!({ "password": common::PASSWORD })), None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{e}");
    let secret = e["secret"].as_str().unwrap().to_string();
    let t = u64::try_from(chrono::Utc::now().timestamp()).unwrap();
    let (status, _) = post(&app, "/api/auth/mfa/enrol/confirm", json!({ "code": mfa::code_at(&secret, t).unwrap() }), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    // a sign-in step that was pending when the reset happens does not survive it
    let ((_, pending), pending_set) = login_with(&app, &email, common::PASSWORD).await;
    assert_eq!(pending["mfa_required"], "verify");
    let pending_cookie = cookie_of(pending_set);

    forgot(&app, &email).await;
    let token = token_in(&outbox(&app, 1).await[0], "/reset-password");
    assert_eq!(reset(&app, &token, NEW_PASSWORD).await.0, StatusCode::OK);

    // enrolment is still there, and the next sign-in still needs the second factor: the reset gave no session
    let ((_, status), _) = common::http(&app.router, "GET", "/api/auth/mfa", None, None, Some(&cookie)).await;
    assert_eq!(status["enabled"], json!(null), "the old session is gone: {status}");
    let mut res = app.control().test_raw().query("SELECT VALUE mfa_secret_enc FROM user WHERE email_lc = $e").bind(("e", email.clone())).await.unwrap();
    assert!(res.take::<Vec<Option<String>>>(0).unwrap()[0].is_some(), "the authenticator is still enrolled");
    let ((status, body), set) = login_with(&app, &email, NEW_PASSWORD).await;
    assert_eq!((status, body["mfa_required"].as_str()), (StatusCode::OK, Some("verify")), "{body}");
    let challenge = cookie_of(set);
    let ((status, body), _) = common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&challenge)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the pending step is not a session: {body}");
    // the step that was pending before the reset is gone
    let (status, body) = post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": "000000" }), Some(&pending_cookie)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::UNAUTHORIZED, Some("auth.mfa_challenge_expired")), "{body}");
    // and the new password plus a code does sign in
    let code = mfa::code_at(&secret, t + 90).unwrap();
    let (status, body) = post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": code }), Some(&challenge)).await;
    assert!(status == StatusCode::OK || body["code"] == "auth.mfa_invalid", "{status} {body}");
}

#[tokio::test]
async fn a_single_sign_on_account_is_given_no_password() {
    let app = app_with_mail().await;
    let sso = common::sys(models_user::register_oidc_user(&app.state, "sso@example.com", "https://idp.example", "subject-1")).await.expect("an SSO account");
    assert_eq!(sso.email, "sso@example.com");
    assert!(user_row(&app, "sso@example.com").await["password_hash"].is_null());

    // asking is answered like any other, and nothing is sent or stored
    let (status, body) = forgot(&app, "sso@example.com").await;
    assert_eq!((status, body), (StatusCode::OK, json!({ "ok": true })));
    assert_eq!(outbox_after_a_moment(&app).await, 0);
    assert!(user_row(&app, "sso@example.com").await["reset_token_hash"].is_null());

    // even a link forged into the row (it cannot be, but the reset must not rely on that) creates no password
    exec(&app, &format!("UPDATE user SET reset_token_hash = '{}', reset_expires_at = time::now() + 30m WHERE email_lc = 'sso@example.com'", models_user::hash_token("forged"))).await;
    let (status, body) = reset(&app, "forged", NEW_PASSWORD).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::BAD_REQUEST, Some("auth.reset_invalid")), "{body}");
    assert!(user_row(&app, "sso@example.com").await["password_hash"].is_null(), "still no password");
    assert_eq!(login_with(&app, "sso@example.com", NEW_PASSWORD).await.0 .0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_suspended_account_is_sent_nothing_and_cannot_use_an_older_link() {
    let app = app_with_mail().await;
    let kate = common::register(&app.state, "kate@example.com").await;
    forgot(&app, "kate@example.com").await;
    let token = token_in(&outbox(&app, 1).await[0], "/reset-password");
    exec(&app, &format!("UPDATE {} SET suspended_at = time::now()", kate.id.to_string())).await;
    let (status, body) = reset(&app, &token, NEW_PASSWORD).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::BAD_REQUEST, Some("auth.reset_invalid")), "{body}");
    exec(&app, "UPDATE user SET reset_sent_at = NONE").await;
    forgot(&app, "kate@example.com").await;
    assert_eq!(outbox_after_a_moment(&app).await, 1, "no new link for a suspended account");
}

#[tokio::test]
async fn a_reset_leaves_api_tokens_alone_like_the_cases_it_follows() {
    // single sign-on linking and suspension end sessions and nothing else; so does a reset
    let app = app_with_mail().await;
    forgot(&app, &app.user.email).await;
    let token = token_in(&outbox(&app, 1).await[0], "/reset-password");
    assert_eq!(reset(&app, &token, NEW_PASSWORD).await.0, StatusCode::OK);
    let (status, _) = app.http("GET", "/api/auth/me", None, true).await;
    assert_eq!(status, StatusCode::OK, "the API token still works");
}

#[tokio::test]
async fn changing_the_password_needs_the_current_one_and_keeps_only_this_session() {
    let app = app_with_mail().await;
    let email = app.user.email.clone();
    let here = app.session_cookie().await;
    let elsewhere = app.session_cookie().await;
    let change = |current: &str, new: &str, cookie: &str| {
        let body = json!({ "current_password": current, "new_password": new });
        let router = app.router.clone();
        let cookie = cookie.to_string();
        async move { common::http(&router, "POST", "/api/account/password", Some(body), None, Some(&cookie)).await.0 }
    };

    // a token cannot change a password
    let (status, _) = app.http("POST", "/api/account/password", Some(json!({ "current_password": common::PASSWORD, "new_password": NEW_PASSWORD })), true).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // wrong current password: 403 (the session itself is fine), nothing changes, and it is audited
    let (status, body) = change("not-the-password", NEW_PASSWORD, &here).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "{body}");
    assert_eq!(common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&here)).await.0 .0, StatusCode::OK);
    assert_eq!(login_with(&app, &email, common::PASSWORD).await.0 .0, StatusCode::OK);
    // the new password follows the sign-up rules, and must differ
    let (status, body) = change(common::PASSWORD, "short", &here).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::BAD_REQUEST, Some("validation.invalid")), "{body}");
    assert_eq!(change(common::PASSWORD, common::PASSWORD, &here).await.0, StatusCode::BAD_REQUEST);

    let (status, body) = change(common::PASSWORD, NEW_PASSWORD, &here).await;
    assert_eq!((status, body), (StatusCode::OK, json!({ "ok": true })));
    assert_eq!(common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&here)).await.0 .0, StatusCode::OK, "this session stays");
    assert_eq!(common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&elsewhere)).await.0 .0, StatusCode::UNAUTHORIZED, "the others end");
    assert_eq!(login_with(&app, &email, common::PASSWORD).await.0 .0, StatusCode::UNAUTHORIZED);
    assert_eq!(login_with(&app, &email, NEW_PASSWORD).await.0 .0, StatusCode::OK);

    let rows = audit(&app, "auth.password_change").await;
    let outcomes: Vec<&str> = rows.iter().map(|r| r["outcome"].as_str().unwrap()).collect();
    assert_eq!(outcomes, ["auth.forbidden", "ok"], "{rows:?}");
    let ledger = whole_ledger(&app).await;
    assert!(!ledger.contains(NEW_PASSWORD) && !ledger.contains("not-the-password"));
}

#[tokio::test]
async fn changing_the_password_kills_an_open_reset_link() {
    let app = app_with_mail().await;
    forgot(&app, &app.user.email).await;
    let token = token_in(&outbox(&app, 1).await[0], "/reset-password");
    let cookie = app.session_cookie().await;
    let (status, _) = post(&app, "/api/account/password", json!({ "current_password": common::PASSWORD, "new_password": NEW_PASSWORD }), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = reset(&app, &token, "yet-another-passphrase").await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::BAD_REQUEST, Some("auth.reset_invalid")), "{body}");
}

#[tokio::test]
async fn an_account_with_two_step_verification_passes_the_step_up_to_change_its_password() {
    let app = app_with_mail().await;
    let cookie = app.session_cookie().await;
    let ((_, e), _) = common::http(&app.router, "POST", "/api/auth/mfa/enrol", Some(json!({ "password": common::PASSWORD })), None, Some(&cookie)).await;
    let secret = e["secret"].as_str().unwrap().to_string();
    let t = u64::try_from(chrono::Utc::now().timestamp()).unwrap();
    assert_eq!(post(&app, "/api/auth/mfa/enrol/confirm", json!({ "code": mfa::code_at(&secret, t).unwrap() }), Some(&cookie)).await.0, StatusCode::OK);
    // the second factor this session passed is older than 10 minutes
    exec(&app, "UPDATE session SET mfa_at = time::now() - 1h").await;
    app.state.credentials.clear();
    let body = json!({ "current_password": common::PASSWORD, "new_password": NEW_PASSWORD });
    let (status, err) = post(&app, "/api/account/password", body.clone(), Some(&cookie)).await;
    assert_eq!((status, err["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.step_up_required")), "{err}");
    assert_eq!(login_with(&app, &app.user.email, common::PASSWORD).await.0 .0, StatusCode::OK, "nothing changed");

    // a code now (the next step, so it is not the one that confirmed enrolment) is enough
    let mut with_code = body;
    with_code["mfa_code"] = json!(mfa::code_at(&secret, t + 30).unwrap());
    let (status, err) = post(&app, "/api/account/password", with_code, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{err}");
}

#[tokio::test]
async fn a_single_sign_on_account_has_no_password_to_change() {
    let app = app_with_mail().await;
    let sso = common::sys(models_user::register_oidc_user(&app.state, "sso@example.com", "https://idp.example", "subject-1")).await.unwrap();
    let token = models_user::create_api_token_with(app.control(), &sso.id, "t", &[], None, None).await;
    drop(token);
    // a browser session of that account, as single sign-on would have started it
    let session = eunomia_backend::auth::start_session(app.control(), "test-jwt-secret", &sso, None, eunomia_backend::auth::Via::Oidc, false).await.unwrap();
    let cookie = format!("{}={session}", eunomia_backend::auth::SESSION_COOKIE);
    let (status, body) = post(&app, "/api/account/password", json!({ "current_password": "whatever-it-is", "new_password": NEW_PASSWORD }), Some(&cookie)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::BAD_REQUEST, Some("validation.invalid")), "{body}");
    assert!(user_row(&app, "sso@example.com").await["password_hash"].is_null());
}

#[tokio::test]
async fn an_invitation_is_emailed_with_the_same_link_the_admin_can_copy() {
    let app = app_with_mail().await;
    let admin = app.session_cookie().await;
    let (status, made) = post(&app, "/api/org/invitations", json!({ "email": "Pat@Example.com", "role": "member" }), Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{made}");
    assert_eq!(made["emailed"], true);

    let mail = outbox(&app, 1).await.remove(0);
    assert_eq!(mail.to, "pat@example.com");
    assert!(mail.body.contains(&app.user.email), "says who invited: {}", mail.body);
    let token = token_in(&mail, "/invite");
    assert_eq!(token, made["token"].as_str().unwrap(), "the emailed link is the copyable one");
    assert!(made["link"].as_str().unwrap().ends_with(&format!("/invite#{token}")));

    // the emailed link works: the page's lookup, then signing up as the invited address
    let (status, seen) = post(&app, "/api/invitations/lookup", json!({ "token": token }), None).await;
    assert_eq!((status, seen["email"].as_str()), (StatusCode::OK, Some("pat@example.com")), "{seen}");
    let ((status, joined), _) =
        common::http(&app.router, "POST", "/api/auth/register", Some(json!({ "email": "pat@example.com", "password": common::PASSWORD, "invitation": token })), None, None).await;
    assert_eq!(status, StatusCode::OK, "{joined}");

    // a resend emails the new link, and the old one stops working
    let (_, second) = post(&app, "/api/org/invitations", json!({ "email": "quinn@example.com" }), Some(&admin)).await;
    let first_token = token_in(&outbox(&app, 2).await[1], "/invite");
    let id = second["invitation"]["id"].as_str().unwrap().to_string();
    let (status, resent) = post(&app, &format!("/api/org/invitations/{id}/resend"), json!({}), Some(&admin)).await;
    assert_eq!((status, resent["emailed"].clone()), (StatusCode::OK, json!(true)), "{resent}");
    let new_token = token_in(&outbox(&app, 3).await[2], "/invite");
    assert_ne!(first_token, new_token);
    assert_eq!(post(&app, "/api/invitations/lookup", json!({ "token": first_token }), None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(post(&app, "/api/invitations/lookup", json!({ "token": new_token }), None).await.0, StatusCode::OK);

    // the audit rows say it was emailed, and carry no token
    let rows = audit(&app, "org.invite").await;
    assert!(rows[0]["detail"].as_str().unwrap().ends_with("emailed=true"), "{rows:?}");
    assert!(!whole_ledger(&app).await.contains(&token));
}

#[tokio::test]
async fn a_mail_server_that_fails_leaves_the_invitation_and_says_so() {
    env_once();
    // nothing listens on port 1
    let smtp = MailBackend::from_vars(|k| match k {
        "SMTP_HOST" => Some("127.0.0.1".into()),
        "SMTP_PORT" => Some("1".into()),
        "SMTP_FROM" => Some("Eunomia <no-reply@example.com>".into()),
        "SMTP_TLS" => Some("none".into()),
        _ => None,
    })
    .unwrap();
    let app = common::TestApp::with_settings(Settings { mail: smtp, ..common::test_settings() }).await;
    let admin = app.session_cookie().await;
    let (status, made) = post(&app, "/api/org/invitations", json!({ "email": "pat@example.com" }), Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{made}");
    assert_eq!(made["emailed"], false, "the admin is told to copy the link");
    assert!(made["token"].as_str().is_some());
    let rows = audit(&app, "org.invite").await;
    assert!(rows[0]["detail"].as_str().unwrap().ends_with("emailed=false"), "{rows:?}");
}

#[tokio::test]
async fn with_email_off_everything_is_as_it_was() {
    let app = app_without_mail().await;
    // the capability flag the login page reads
    let (status, boot) = common::http(&app.router, "GET", "/api/auth/bootstrap", None, None, None).await.0;
    assert_eq!((status, boot["email_enabled"].clone()), (StatusCode::OK, json!(false)), "{boot}");
    // the route that needs email says so, with a documented code
    let (status, body) = forgot(&app, &app.user.email).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::SERVICE_UNAVAILABLE, Some("email.unavailable")), "{body}");
    assert!(audit(&app, "auth.password_forgot").await.is_empty());
    assert_eq!(app.state.mailer.captured().len(), 0);
    // sign-in is untouched
    assert_eq!(login_with(&app, &app.user.email, common::PASSWORD).await.0 .0, StatusCode::OK);
    // invitations: the link to copy, nothing emailed, and the audit row is the one it always was
    let admin = app.session_cookie().await;
    let (status, made) = post(&app, "/api/org/invitations", json!({ "email": "pat@example.com" }), Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{made}");
    assert_eq!(made["emailed"], false);
    assert!(made["link"].as_str().unwrap().contains("/invite#"));
    let rows = audit(&app, "org.invite").await;
    assert_eq!(rows[0]["detail"], "email=pat@example.com role=member");
    // changing a password needs no email
    let (status, _) = post(&app, "/api/account/password", json!({ "current_password": common::PASSWORD, "new_password": NEW_PASSWORD }), Some(&admin)).await;
    assert_eq!(status, StatusCode::OK);

    // and it is on when a mail server is configured
    let on = app_with_mail().await;
    let (_, boot) = common::http(&on.router, "GET", "/api/auth/bootstrap", None, None, None).await.0;
    assert_eq!(boot["email_enabled"], json!(true));
}
