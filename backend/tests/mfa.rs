//! Two-step verification end to end (compliance PR04, capability C02): enrolment with a confirming code,
//! the encrypted secret, the second sign-in step with replay refusal, single-use recovery codes, lockout,
//! the admin's "require two-step verification" switch (no session without it, old sessions refused, signup
//! enrols first), step-up for new tokens, and the audit trail. Real router, in-memory SurrealDB.

mod common;

use axum::http::StatusCode;
use eunomia_backend::mfa;
use serde_json::{json, Value};

type Reply = ((StatusCode, Value), Option<String>);

fn now() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp()).unwrap()
}

/// `name=value` of a Set-Cookie header.
fn pair(set_cookie: &Option<String>) -> String {
    set_cookie.as_deref().expect("a cookie").split(';').next().unwrap().to_string()
}

async fn post(app: &common::TestApp, path: &str, body: Value, cookie: Option<&str>) -> Reply {
    common::http(&app.router, "POST", path, Some(body), None, cookie).await
}

async fn login(app: &common::TestApp, email: &str) -> Reply {
    post(app, "/api/auth/login", json!({ "email": email, "password": common::PASSWORD }), None).await
}

/// Turns MFA on for the session's account; returns (secret, recovery codes). The confirming code is the
/// one for the current step.
async fn enrol(app: &common::TestApp, cookie: &str) -> (String, Vec<String>) {
    let (secret, codes, _) = enrol_at(app, cookie).await;
    (secret, codes)
}

async fn enrol_at(app: &common::TestApp, cookie: &str) -> (String, Vec<String>, u64) {
    let ((status, e), _) = post(app, "/api/auth/mfa/enrol", json!({}), Some(cookie)).await;
    assert_eq!(status, StatusCode::OK, "{e}");
    let secret = e["secret"].as_str().unwrap().to_string();
    let t0 = now();
    let code = mfa::code_at(&secret, t0).unwrap();
    let ((status, done), _) = post(app, "/api/auth/mfa/enrol/confirm", json!({ "code": code }), Some(cookie)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    let codes = done["recovery_codes"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect();
    (secret, codes, t0)
}

async fn audit_actions(app: &common::TestApp) -> Vec<String> {
    let mut res = app.control().test_raw().query("SELECT VALUE action FROM audit_event ORDER BY created_at").await.unwrap();
    res.take(0).unwrap()
}

#[tokio::test]
async fn enrol_sign_in_with_a_code_refuse_replay_and_spend_recovery_codes_once() {
    let app = common::TestApp::new().await;
    let cookie = app.session_cookie().await;
    let ((status, st), _) = common::http(&app.router, "GET", "/api/auth/mfa", None, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{st}");
    assert_eq!((st["enabled"].as_bool(), st["available"].as_bool()), (Some(false), Some(true)));

    // an API token cannot manage two-step verification
    let (status, _) = app.http("POST", "/api/auth/mfa/enrol", Some(json!({})), true).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // a secret shown but not confirmed changes nothing: sign-in still needs only the password
    let ((_, shown), _) = post(&app, "/api/auth/mfa/enrol", json!({}), Some(&cookie)).await;
    assert!(shown["otpauth_uri"].as_str().unwrap().starts_with("otpauth://totp/Eunomia:tester%40example.com?"), "{shown}");
    assert!(shown["qr_svg"].as_str().unwrap().contains("<svg"));
    let ((_, body), _) = login(&app, &app.user.email).await;
    assert!(body.get("mfa_required").is_none(), "unconfirmed enrolment does not switch MFA on: {body}");
    // a wrong code does not confirm it
    let ((status, body), _) = post(&app, "/api/auth/mfa/enrol/confirm", json!({ "code": "000000" }), Some(&cookie)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::UNAUTHORIZED, Some("auth.mfa_invalid")), "{body}");

    let (secret, recovery, t0) = enrol_at(&app, &cookie).await;
    assert_eq!(recovery.len(), mfa::RECOVERY_CODES);

    // stored encrypted, never in plain text, and never returned again
    let mut res = app.control().test_raw().query("SELECT VALUE mfa_secret_enc FROM ONLY $id").bind(("id", app.user.id.clone())).await.unwrap();
    let enc: Option<String> = res.take(0).unwrap();
    let enc = enc.expect("a stored secret");
    assert!(enc.starts_with("enc:v1:") && !enc.contains(&secret), "{enc}");
    let mut res = app.control().test_raw().query("SELECT * FROM mfa_recovery_code").await.unwrap();
    let rows: Vec<Value> = res.take(0).unwrap();
    assert_eq!(rows.len(), mfa::RECOVERY_CODES);
    let dump = serde_json::to_string(&rows).unwrap();
    assert!(recovery.iter().all(|c| !dump.contains(c.as_str()) && !dump.contains(&c.replace('-', ""))), "recovery codes are stored hashed");
    let ((_, st), _) = common::http(&app.router, "GET", "/api/auth/mfa", None, None, Some(&cookie)).await;
    assert_eq!((st["enabled"].as_bool(), st["recovery_codes_left"].as_i64()), (Some(true), Some(10)), "{st}");
    assert!(!st.to_string().contains(&secret));
    // and a second enrolment is refused while it is on
    let ((status, _), _) = post(&app, "/api/auth/mfa/enrol", json!({}), Some(&cookie)).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // the password alone now gives a pending second step, not a session
    let ((status, body), set) = login(&app, &app.user.email).await;
    assert_eq!((status, body["mfa_required"].as_str()), (StatusCode::OK, Some("verify")), "{body}");
    let set = set.unwrap();
    assert!(set.starts_with("eunomia_mfa=") && set.contains("HttpOnly") && set.contains("Path=/api/auth/mfa") && set.contains("Max-Age=300"), "{set}");
    let pending = set.split(';').next().unwrap().to_string();
    // the pending cookie is not a session, anywhere
    for path in ["/api/auth/me", "/api/auth/tokens", "/api/auth/mfa"] {
        let ((status, _), _) = common::http(&app.router, "GET", path, None, None, Some(&pending)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }

    // the code that confirmed enrolment was spent in its window: the same one again is a replay
    let same = mfa::code_at(&secret, t0).unwrap();
    let ((status, body), _) = post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": same }), Some(&pending)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::UNAUTHORIZED, Some("auth.mfa_invalid")), "replayed code: {body}");

    // the next step's code (one step of clock skew is allowed) signs in, once
    let next = mfa::code_at(&secret, t0 + 30).unwrap();
    let ((status, me), set) = post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": next }), Some(&pending)).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["email"], app.user.email.as_str());
    let session = pair(&set);
    assert!(session.starts_with("eunomia_session="));
    assert_eq!(common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&session)).await.0 .0, StatusCode::OK);
    // the challenge was spent with it
    let ((_, body), _) = post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": next }), Some(&pending)).await;
    assert_eq!(body["code"], "auth.mfa_challenge_expired", "{body}");
    // and the same code on a fresh challenge is a replay
    let ((_, _), set) = login(&app, &app.user.email).await;
    let pending = pair(&set);
    let ((_, body), _) = post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": next }), Some(&pending)).await;
    assert_eq!(body["code"], "auth.mfa_invalid", "{body}");

    // a recovery code works once, typed any reasonable way
    let typed = recovery[0].to_uppercase().replace('-', " ");
    let ((status, _), set) = post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": typed }), Some(&pending)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(pair(&set).starts_with("eunomia_session="));
    let ((_, _), set) = login(&app, &app.user.email).await;
    let ((status, body), _) = post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": recovery[0] }), Some(&pair(&set))).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::UNAUTHORIZED, Some("auth.mfa_invalid")), "a spent recovery code: {body}");

    let actions = audit_actions(&app).await;
    for a in ["auth.mfa_enrol", "auth.mfa_failed", "auth.mfa_recovery_used"] {
        assert!(actions.iter().any(|x| x == a), "{a} in {actions:?}");
    }
    let ((_, page), _) = common::http(&app.router, "GET", "/api/admin/audit?category=signins", None, None, Some(&session)).await;
    let summaries: Vec<&str> = page["results"].as_array().unwrap().iter().filter_map(|r| r["summary"].as_str()).collect();
    assert!(summaries.contains(&"Signed in with two-step verification"), "{summaries:?}");
    assert!(summaries.iter().any(|s| s.starts_with("Used a recovery code (9 left)")), "{summaries:?}");
}

#[tokio::test]
async fn wrong_codes_end_the_sign_in_and_then_lock_the_account() {
    let app = common::TestApp::new().await;
    let cookie = app.session_cookie().await;
    let (secret, _) = enrol(&app, &cookie).await;

    let ((_, _), set) = login(&app, &app.user.email).await;
    let pending = pair(&set);
    for i in 1..=mfa::CHALLENGE_MAX_FAILS {
        let ((status, body), _) = post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": "000000" }), Some(&pending)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let want = if i < mfa::CHALLENGE_MAX_FAILS { "auth.mfa_invalid" } else { "auth.mfa_challenge_expired" };
        assert_eq!(body["code"], want, "attempt {i}: {body}");
    }
    // that sign-in is over, even with the right code
    let good = mfa::code_at(&secret, now() + 30).unwrap();
    let ((_, body), _) = post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": good }), Some(&pending)).await;
    assert_eq!(body["code"], "auth.mfa_challenge_expired");

    // a new sign-in may try again, but the account allows MFA_FAILS wrong codes per window in all
    let ((_, _), set) = login(&app, &app.user.email).await;
    let pending = pair(&set);
    let left = i64::from(mfa::MFA_FAILS) - mfa::CHALLENGE_MAX_FAILS;
    for _ in 0..left {
        let ((status, _), _) = post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": "111111" }), Some(&pending)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let ((_, _), set) = login(&app, &app.user.email).await;
    let ((status, body), _) = post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": good }), Some(&pair(&set))).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate.limited")), "locked out, even with the right code: {body}");
}

#[tokio::test]
async fn requiring_mfa_means_no_session_without_it() {
    let app = common::TestApp::new().await;
    let admin = app.session_cookie().await;
    common::register(&app.state, "bob@example.com").await;
    // carol signed in before the rule: a password session that never passed a second factor
    common::register(&app.state, "carol@example.com").await;
    let carol = pair(&login(&app, "carol@example.com").await.1);
    assert_eq!(common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&carol)).await.0 .0, StatusCode::OK);

    // only an admin's browser session may turn it on, and only once their own account is protected
    let bob = pair(&login(&app, "bob@example.com").await.1);
    let ((status, _), _) = common::http(&app.router, "PUT", "/api/admin/security", Some(json!({ "require_mfa": true })), None, Some(&bob)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = app.http("PUT", "/api/admin/security", Some(json!({ "require_mfa": true })), true).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "not with an API token");
    let ((status, body), _) = common::http(&app.router, "PUT", "/api/admin/security", Some(json!({ "require_mfa": true })), None, Some(&admin)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    enrol(&app, &admin).await;
    let ((status, body), _) = common::http(&app.router, "PUT", "/api/admin/security", Some(json!({ "require_mfa": true })), None, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!((body["require_mfa"].as_bool(), body["without_mfa"].as_i64()), (Some(true), Some(2)), "{body}");

    // carol's and bob's old sessions stop working; the admin's (verified at enrolment) keeps working; tokens keep working
    for s in [&carol, &bob] {
        let ((status, body), _) = common::http(&app.router, "GET", "/api/auth/me", None, None, Some(s)).await;
        assert_eq!((status, body["code"].as_str()), (StatusCode::UNAUTHORIZED, Some("auth.mfa_required")), "{body}");
    }
    assert_eq!(common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&admin)).await.0 .0, StatusCode::OK);
    assert_eq!(app.http("GET", "/api/auth/me", None, true).await.0, StatusCode::OK, "API tokens are not affected");

    // bob's password alone gives an enrolment step, nothing else
    let ((_, body), set) = login(&app, "bob@example.com").await;
    assert_eq!(body["mfa_required"], "enrol", "{body}");
    let pending = pair(&set);
    assert!(pending.starts_with("eunomia_mfa="));
    assert_eq!(common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&pending)).await.0 .0, StatusCode::UNAUTHORIZED);
    // an enrolment challenge cannot be used as a verification one
    let ((_, body), _) = post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": "123456" }), Some(&pending)).await;
    assert_eq!(body["code"], "auth.mfa_challenge_expired", "{body}");
    let ((status, e), _) = post(&app, "/api/auth/mfa/challenge/enrol", json!({}), Some(&pending)).await;
    assert_eq!(status, StatusCode::OK, "{e}");
    let secret = e["secret"].as_str().unwrap();
    let ((status, done), set) = post(&app, "/api/auth/mfa/challenge/enrol/confirm", json!({ "code": mfa::code_at(secret, now()).unwrap() }), Some(&pending)).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["me"]["email"], "bob@example.com");
    assert_eq!(done["recovery_codes"].as_array().unwrap().len(), mfa::RECOVERY_CODES);
    let bob = pair(&set);
    assert_eq!(common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&bob)).await.0 .0, StatusCode::OK);

    // bob cannot turn it off while it is required
    let code = mfa::code_at(secret, now() + 30).unwrap();
    let ((status, _), _) = post(&app, "/api/auth/mfa/disable", json!({ "code": code }), Some(&bob)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // a new signup enrols before it gets a session
    let ((status, body), set) = post(&app, "/api/auth/register", json!({ "email": "dave@example.com", "password": common::PASSWORD }), None).await;
    assert_eq!((status, body["mfa_required"].as_str()), (StatusCode::OK, Some("enrol")), "{body}");
    assert!(pair(&set).starts_with("eunomia_mfa="), "no session cookie on signup");

    let ((_, page), _) = common::http(&app.router, "GET", "/api/admin/audit?category=admin", None, None, Some(&admin)).await;
    let summaries: Vec<&str> = page["results"].as_array().unwrap().iter().filter_map(|r| r["summary"].as_str()).collect();
    assert_eq!(summaries, ["Required two-step verification for every password account"]);
    let ((_, people), _) = common::http(&app.router, "GET", "/api/admin/people", None, None, Some(&admin)).await;
    let mfa_of = |email: &str| people["people"].as_array().unwrap().iter().find(|p| p["email"] == email).unwrap()["mfa"].clone();
    assert_eq!((mfa_of("bob@example.com"), mfa_of("carol@example.com")), (json!("on"), json!("off")));

    // turning it off lets carol sign in with her password alone again
    let ((status, _), _) = common::http(&app.router, "PUT", "/api/admin/security", Some(json!({ "require_mfa": false })), None, Some(&admin)).await;
    assert_eq!(status, StatusCode::OK);
    let ((_, body), set) = login(&app, "carol@example.com").await;
    assert!(body.get("mfa_required").is_none(), "{body}");
    assert!(pair(&set).starts_with("eunomia_session="));
}

#[tokio::test]
async fn new_tokens_and_mfa_changes_need_a_fresh_second_factor() {
    let app = common::TestApp::new().await;
    let cookie = app.session_cookie().await;
    let (_, recovery) = enrol(&app, &cookie).await;

    // just verified (enrolment counts): a token is created without another code
    let ((status, body), _) = post(&app, "/api/auth/tokens", json!({ "name": "fresh" }), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // ten minutes later it asks again
    app.control().test_raw().query("UPDATE session SET mfa_at = time::now() - 1h WHERE owner = $u").bind(("u", app.user.id.clone())).await.unwrap().check().unwrap();
    app.state.credentials.clear();
    let ((status, body), _) = post(&app, "/api/auth/tokens", json!({ "name": "stale" }), Some(&cookie)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.step_up_required")), "{body}");
    let ((status, body), _) = post(&app, "/api/auth/tokens", json!({ "name": "stale", "mfa_code": "999999" }), Some(&cookie)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::UNAUTHORIZED, Some("auth.mfa_invalid")), "{body}");
    let ((status, body), _) = post(&app, "/api/auth/tokens", json!({ "name": "stale", "mfa_code": recovery[1] }), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // and that code counts for the session for a while
    let ((status, _), _) = post(&app, "/api/auth/tokens", json!({ "name": "after" }), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);

    // existing personal access tokens keep working, and may still mint (bounded by their own scopes and expiry)
    let (status, _) = app.http("POST", "/api/auth/tokens", Some(json!({ "name": "by token" })), true).await;
    assert_eq!(status, StatusCode::OK);

    // new recovery codes and turning MFA off each need a code every time
    let ((status, _), _) = post(&app, "/api/auth/mfa/recovery-codes", json!({ "code": "nope" }), Some(&cookie)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let ((status, fresh), _) = post(&app, "/api/auth/mfa/recovery-codes", json!({ "code": recovery[2] }), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{fresh}");
    let ((status, _), _) = post(&app, "/api/auth/mfa/disable", json!({ "code": recovery[3] }), Some(&cookie)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the old codes stopped working");
    let new_code = fresh["recovery_codes"][0].as_str().unwrap();
    let ((status, st), _) = post(&app, "/api/auth/mfa/disable", json!({ "code": new_code }), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{st}");
    assert_eq!((st["enabled"].as_bool(), st["recovery_codes_left"].as_i64()), (Some(false), Some(0)));
    let ((_, body), set) = login(&app, &app.user.email).await;
    assert!(body.get("mfa_required").is_none() && pair(&set).starts_with("eunomia_session="), "{body}");

    let actions = audit_actions(&app).await;
    for a in ["auth.mfa_recovery_regenerate", "auth.mfa_disable"] {
        assert!(actions.iter().any(|x| x == a), "{a} in {actions:?}");
    }
}

/// An install upgraded from before MFA: its sessions carry no `via` or `mfa_at`, its users no MFA fields.
/// They keep working until an admin requires MFA.
#[tokio::test]
async fn sessions_from_before_the_upgrade_keep_working_until_mfa_is_required() {
    let app = common::TestApp::new().await;
    let cookie = app.session_cookie().await;
    app.control().test_raw().query("UPDATE session SET via = NONE, mfa_at = NONE").await.unwrap().check().unwrap();
    app.state.credentials.clear();
    assert_eq!(common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&cookie)).await.0 .0, StatusCode::OK);
    mfa::set_require_mfa(app.control(), true, &app.user.id).await.unwrap();
    app.state.credentials.clear();
    let ((_, body), _) = common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&cookie)).await;
    assert_eq!(body["code"], "auth.mfa_required", "{body}");
}
