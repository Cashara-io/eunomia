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
    let ((status, e), _) = post(app, "/api/auth/mfa/enrol", json!({ "password": common::PASSWORD }), Some(cookie)).await;
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
    let (status, _) = app.http("POST", "/api/auth/mfa/enrol", Some(json!({ "password": common::PASSWORD })), true).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // a session alone is not enough: setup needs the password (a 403, so the session itself stays signed in)
    for body in [json!({}), json!({ "password": "not-the-password" })] {
        let ((status, err), _) = post(&app, "/api/auth/mfa/enrol", body, Some(&cookie)).await;
        assert_eq!((status, err["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "{err}");
    }
    assert_eq!(common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&cookie)).await.0 .0, StatusCode::OK);

    // a secret shown but not confirmed changes nothing: sign-in still needs only the password
    let ((_, shown), _) = post(&app, "/api/auth/mfa/enrol", json!({ "password": common::PASSWORD }), Some(&cookie)).await;
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
    let ((status, _), _) = post(&app, "/api/auth/mfa/enrol", json!({ "password": common::PASSWORD }), Some(&cookie)).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // the password alone now gives a pending second step, not a session
    let ((status, body), set) = login(&app, &app.user.email).await;
    assert_eq!((status, body["mfa_required"].as_str()), (StatusCode::OK, Some("verify")), "{body}");
    let set = set.unwrap();
    assert!(set.starts_with("eunomia_mfa=") && set.contains("HttpOnly") && set.contains("Path=/api/auth/mfa") && set.contains("Max-Age=300"), "{set}");
    let pending = set.split(';').next().unwrap().to_string();
    // the pending token is not a session anywhere: not under its own cookie, not presented as the session
    // cookie, not as a bearer token, on routes that answer any signed-in caller
    let raw_token = pending.trim_start_matches("eunomia_mfa=").to_string();
    let as_session = format!("eunomia_session={raw_token}");
    for path in ["/api/auth/me", "/api/auth/tokens", "/api/auth/mfa", "/api/vaults", "/api/settings"] {
        for cookie in [pending.as_str(), as_session.as_str()] {
            let ((status, _), _) = common::http(&app.router, "GET", path, None, None, Some(cookie)).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{path} with {}", cookie.split('=').next().unwrap());
        }
        let ((status, _), _) = common::http(&app.router, "GET", path, None, Some(&raw_token), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path} as a bearer token");
    }
    // and enrolment is closed to it too
    let ((status, _), _) = post(&app, "/api/auth/mfa/enrol", json!({ "password": common::PASSWORD }), Some(&as_session)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

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

    // existing personal access tokens keep working, but cannot mint another token once the account has MFA
    assert_eq!(app.http("GET", "/api/auth/me", None, true).await.0, StatusCode::OK);
    let (status, body) = app.http("POST", "/api/auth/tokens", Some(json!({ "name": "by token" })), true).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.step_up_required")), "{body}");
    let (status, body) = app.http("POST", "/api/auth/tokens", Some(json!({ "name": "by token", "mfa_code": recovery[4] })), true).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "not even with a code: a token is not a browser session: {body}");

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

/// Counts the `auth.mfa_failed` rows: one per code actually evaluated and found wrong.
async fn failed_evaluations(app: &common::TestApp) -> usize {
    audit_actions(app).await.iter().filter(|a| *a == "auth.mfa_failed").count()
}

/// Many wrong codes at once against one sign-in, then against the account through every step-up route: the
/// attempts are reserved before any code is checked, so no more than the limits are ever evaluated.
#[tokio::test]
async fn parallel_wrong_codes_never_exceed_the_limits() {
    let app = common::TestApp::new().await;
    let cookie = app.session_cookie().await;
    enrol(&app, &cookie).await;

    // one challenge, 30 wrong codes at once: at most CHALLENGE_MAX_FAILS are checked
    let pending = pair(&login(&app, &app.user.email).await.1);
    let calls = (0..30).map(|i| {
        let (router, pending) = (app.router.clone(), pending.clone());
        async move { common::http(&router, "POST", "/api/auth/mfa/challenge/verify", Some(json!({ "code": format!("{:06}", 100_000 + i) })), None, Some(&pending)).await.0 }
    });
    let answers = futures::future::join_all(calls).await;
    let invalid = answers.iter().filter(|(_, b)| b["code"] == "auth.mfa_invalid").count();
    let evaluated = failed_evaluations(&app).await;
    assert!(evaluated <= mfa::CHALLENGE_MAX_FAILS as usize, "{evaluated} codes checked on one challenge");
    assert_eq!(evaluated, mfa::CHALLENGE_MAX_FAILS as usize, "the challenge's attempts were all used");
    assert!(invalid < mfa::CHALLENGE_MAX_FAILS as usize, "the last one ends the challenge: {answers:?}");
    assert!(answers.iter().all(|(s, _)| *s == StatusCode::UNAUTHORIZED || *s == StatusCode::TOO_MANY_REQUESTS), "{answers:?}");

    // the account's remaining budget, spent at once across token creation, the admin switch, disable and regenerate
    app.control().test_raw().query("UPDATE session SET mfa_at = time::now() - 1h WHERE owner = $u").bind(("u", app.user.id.clone())).await.unwrap().check().unwrap();
    app.state.credentials.clear();
    let routes: [(&str, &str, Value); 4] = [
        ("POST", "/api/auth/tokens", json!({ "name": "x", "mfa_code": "222222" })),
        ("PUT", "/api/admin/security", json!({ "require_mfa": true, "mfa_code": "222222" })),
        ("POST", "/api/auth/mfa/disable", json!({ "code": "222222" })),
        ("POST", "/api/auth/mfa/recovery-codes", json!({ "code": "222222" })),
    ];
    let calls = (0..40).map(|i| {
        let (router, cookie) = (app.router.clone(), cookie.clone());
        let (method, path, body) = routes[i % 4].clone();
        async move { (path, common::http(&router, method, path, Some(body), None, Some(&cookie)).await.0) }
    });
    let answers = futures::future::join_all(calls).await;
    let evaluated = failed_evaluations(&app).await;
    assert_eq!(evaluated, mfa::MFA_FAILS as usize, "the account's window allows exactly MFA_FAILS codes: {answers:?}");
    for (path, (status, body)) in &answers {
        assert!(*status == StatusCode::UNAUTHORIZED || *status == StatusCode::TOO_MANY_REQUESTS, "{path}: {status} {body}");
    }
    // nothing was created or changed by any of them
    let tokens: Vec<Value> = app.control().test_raw().query("SELECT * FROM api_token WHERE name = 'x'").await.unwrap().take(0).unwrap();
    assert!(tokens.is_empty());
    assert!(!mfa::require_mfa(app.control()).await.unwrap());
    assert!(mfa::enabled(app.control(), &app.user.id).await.unwrap());
}

/// The same code on two sign-ins at once, and the same recovery code twice at once: one wins, one does not.
#[tokio::test]
async fn a_code_used_twice_at_once_signs_in_once() {
    let app = common::TestApp::new().await;
    let cookie = app.session_cookie().await;
    let (secret, recovery, t0) = enrol_at(&app, &cookie).await;
    let code = mfa::code_at(&secret, t0 + 30).unwrap();
    for c in [code.as_str(), recovery[0].as_str()] {
        let a = pair(&login(&app, &app.user.email).await.1);
        let b = pair(&login(&app, &app.user.email).await.1);
        let (ra, rb) = tokio::join!(
            post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": c }), Some(&a)),
            post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": c }), Some(&b)),
        );
        let ok = [&ra, &rb].iter().filter(|r| r.0 .0 == StatusCode::OK).count();
        assert_eq!(ok, 1, "{c}: {:?} {:?}", ra.0, rb.0);
    }
    // and two answers on one challenge at once with two different recovery codes: one session, one code spent
    let p = pair(&login(&app, &app.user.email).await.1);
    let (ra, rb) = tokio::join!(
        post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": recovery[1] }), Some(&p)),
        post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": recovery[2] }), Some(&p)),
    );
    assert_eq!([&ra, &rb].iter().filter(|r| r.0 .0 == StatusCode::OK).count(), 1);
    let ((_, st), _) = common::http(&app.router, "GET", "/api/auth/mfa", None, None, Some(&cookie)).await;
    assert_eq!(st["recovery_codes_left"], 8, "the losing request spent no recovery code: {st}");
}

/// Turning MFA on signs out the account's other browser sessions that never passed a second factor.
#[tokio::test]
async fn enrolling_signs_out_unverified_sessions() {
    let app = common::TestApp::new().await;
    let other = app.session_cookie().await;
    let cookie = app.session_cookie().await;
    enrol(&app, &cookie).await;
    assert_eq!(common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&cookie)).await.0 .0, StatusCode::OK, "this one passed it");
    assert_eq!(common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&other)).await.0 .0, StatusCode::UNAUTHORIZED, "the other one ends");
    assert_eq!(app.http("GET", "/api/auth/me", None, true).await.0, StatusCode::OK, "API tokens are separate credentials");
}

/// The instance switch is the server operator's: an admin granted in the app cannot read or change it, and
/// the count before switching covers every organisation.
#[tokio::test]
async fn only_the_server_operator_controls_the_switch_and_sees_every_org() {
    let app = common::TestApp::new().await;
    let owner = app.session_cookie().await;
    enrol(&app, &owner).await;
    let bob = common::register(&app.state, "bob@example.com").await;
    let ((status, _), _) = common::http(&app.router, "PUT", &format!("/api/admin/people/{}/admin", eunomia_backend::rid::RecordIdExt::to_string(&bob.id)), Some(json!({ "admin": true })), None, Some(&owner)).await;
    assert_eq!(status, StatusCode::OK);
    // an account in another organisation on the same host
    common::register_personal(&app.state, "carol@other.example").await;

    let bob_cookie = pair(&login(&app, "bob@example.com").await.1);
    for (method, body) in [("GET", None), ("PUT", Some(json!({ "require_mfa": true })))] {
        let ((status, body), _) = common::http(&app.router, method, "/api/admin/security", body, None, Some(&bob_cookie)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "a granted admin: {body}");
        assert!(!body.to_string().contains(app.user.email.as_str()), "no operator email leaks: {body}");
    }
    let ((_, st), _) = common::http(&app.router, "GET", "/api/auth/mfa", None, None, Some(&bob_cookie)).await;
    assert_eq!(st["can_require"], false);
    let ((_, st), _) = common::http(&app.router, "GET", "/api/auth/mfa", None, None, Some(&owner)).await;
    assert_eq!(st["can_require"], true);

    let ((status, s), _) = common::http(&app.router, "GET", "/api/admin/security", None, None, Some(&owner)).await;
    assert_eq!(status, StatusCode::OK, "{s}");
    assert_eq!(s["without_mfa"], 2, "bob and carol, across both organisations: {s}");
}

/// Approving an OAuth app is a sensitive action like a new token: a fresh second factor first. Denying never asks.
#[tokio::test]
async fn connecting_an_oauth_app_needs_a_fresh_second_factor() {
    let app = common::TestApp::new().await;
    let cookie = app.session_cookie().await;
    let (_, recovery) = enrol(&app, &cookie).await;
    let ((status, client), _) = common::http(&app.router, "POST", "/oauth/register", Some(json!({"client_name": "Agent", "redirect_uris": ["http://127.0.0.1:9/cb"], "token_endpoint_auth_method": "none"})), None, None).await;
    assert_eq!(status, StatusCode::CREATED, "{client}");
    let request = |approve: bool, code: Option<&str>| {
        let mut b = json!({
            "response_type": "code", "client_id": client["client_id"], "redirect_uri": "http://127.0.0.1:9/cb",
            "code_challenge": "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM", "code_challenge_method": "S256",
            "state": "s", "resource": "http://localhost:8001/mcp", "approve": approve,
        });
        if let Some(c) = code {
            b["mfa_code"] = json!(c);
        }
        b
    };
    // just verified: allowed
    let ((status, body), _) = post(&app, "/api/oauth/consent", request(true, None), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    app.control().test_raw().query("UPDATE session SET mfa_at = time::now() - 1h WHERE owner = $u").bind(("u", app.user.id.clone())).await.unwrap().check().unwrap();
    app.state.credentials.clear();
    let ((status, body), _) = post(&app, "/api/oauth/consent", request(true, None), Some(&cookie)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.step_up_required")), "{body}");
    let ((status, _), _) = post(&app, "/api/oauth/consent", request(false, None), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "denying needs no code");
    let ((status, body), _) = post(&app, "/api/oauth/consent", request(true, Some(&recovery[0])), Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["redirect_to"].as_str().unwrap().contains("code="));
}

/// A two-step verification request that fails with a 5xx leaves a capsule without the typed code.
#[tokio::test]
async fn a_failing_mfa_request_leaves_no_code_in_its_capsule() {
    // SAFETY: set before any other thread reads it, to the same value the capsule tests use
    unsafe { std::env::set_var("CAPSULE_ARGS", "redacted") };
    let app = common::TestApp::new().await;
    let cookie = app.session_cookie().await;
    let (_, recovery) = enrol(&app, &cookie).await;
    // a row the typed reader cannot decode: the next MFA read fails with a database error
    app.control()
        .test_raw()
        .query("DEFINE FIELD OVERWRITE mfa_pending_at ON user TYPE any; UPDATE $u SET mfa_pending_at = 'not a date'")
        .bind(("u", app.user.id.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();
    let ((status, body), _) = post(&app, "/api/auth/mfa/recovery-codes", json!({ "code": recovery[0] }), Some(&cookie)).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    let c = eunomia_backend::capsules::get(app.control(), body["trace_id"].as_str().unwrap()).await.unwrap().expect("a capsule");
    let text = serde_json::to_string(&c).unwrap();
    assert!(!text.contains(&recovery[0]) && !text.contains(&recovery[0].replace('-', "")), "{text}");
}

/// A changed ENCRYPTION_KEY: the authenticator answers a clear 409 (not a 500), costs no attempt and is audited;
/// a recovery code still signs in, and turns it off for re-enrolment even while MFA is required.
#[tokio::test]
async fn an_unreadable_secret_points_to_recovery_codes_not_a_500() {
    let app = common::TestApp::new().await;
    let cookie = app.session_cookie().await;
    let (secret, recovery, t0) = enrol_at(&app, &cookie).await;
    mfa::set_require_mfa(app.control(), true, &app.user.id).await.unwrap();
    let other_key = eunomia_backend::connectors::crypto::encrypt("an-older-key-that-is-gone-0123456789", &secret);
    app.control().test_raw().query("UPDATE $u SET mfa_secret_enc = $e").bind(("u", app.user.id.clone())).bind(("e", other_key)).await.unwrap().check().unwrap();

    let pending = pair(&login(&app, &app.user.email).await.1);
    for _ in 0..(mfa::MFA_FAILS + 2) {
        let ((status, body), _) = post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": mfa::code_at(&secret, t0 + 30).unwrap() }), Some(&pending)).await;
        assert_eq!((status, body["code"].as_str()), (StatusCode::CONFLICT, Some("auth.mfa_unreadable")), "no attempt counted, never a 500: {body}");
        assert!(body["detail"].as_str().unwrap().contains("recovery code"), "{body}");
    }
    assert!(audit_actions(&app).await.iter().any(|a| a == "auth.mfa_secret_unreadable"));
    let ((status, _), set) = post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": recovery[0] }), Some(&pending)).await;
    assert_eq!(status, StatusCode::OK);
    let session = pair(&set);
    let ((_, st), _) = common::http(&app.router, "GET", "/api/auth/mfa", None, None, Some(&session)).await;
    assert_eq!(st["unreadable"], true, "{st}");
    let ((status, st), _) = post(&app, "/api/auth/mfa/disable", json!({ "code": recovery[1] }), Some(&session)).await;
    assert_eq!(status, StatusCode::OK, "turning it off is allowed to re-enrol, even while required: {st}");
}


/// Before MFA, an API token may mint another (agent setup scripts rely on it); while MFA is required it may not.
#[tokio::test]
async fn tokens_mint_tokens_only_without_mfa() {
    let app = common::TestApp::new().await;
    let (status, _) = app.http("POST", "/api/auth/tokens", Some(json!({ "name": "agent" })), true).await;
    assert_eq!(status, StatusCode::OK, "no MFA anywhere: unchanged");
    mfa::set_require_mfa(app.control(), true, &app.user.id).await.unwrap();
    let (status, body) = app.http("POST", "/api/auth/tokens", Some(json!({ "name": "agent" })), true).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.step_up_required")), "{body}");
}

/// An authenticator lockout (a password holder spending the 10 tries) never blocks the owner's recovery code,
/// which also clears the lockout. Wrong recovery codes are bounded on their own counter.
#[tokio::test]
async fn a_recovery_code_gets_past_an_authenticator_lockout() {
    let app = common::TestApp::new().await;
    let cookie = app.session_cookie().await;
    let (secret, recovery, t0) = enrol_at(&app, &cookie).await;
    // the attacker uses up the authenticator budget over a few sign-ins
    for _ in 0..(mfa::MFA_FAILS as usize).div_ceil(mfa::CHALLENGE_MAX_FAILS as usize) {
        let pending = pair(&login(&app, &app.user.email).await.1);
        for _ in 0..mfa::CHALLENGE_MAX_FAILS {
            post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": "000000" }), Some(&pending)).await;
        }
    }
    let pending = pair(&login(&app, &app.user.email).await.1);
    let ((status, _), _) = post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": mfa::code_at(&secret, t0 + 30).unwrap() }), Some(&pending)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "the authenticator is locked out");
    // the owner's recovery code still signs in, and ends the lockout
    let ((status, body), set) = post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": recovery[0] }), Some(&pending)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(pair(&set).starts_with("eunomia_session="));
    let pending = pair(&login(&app, &app.user.email).await.1);
    let ((status, body), _) = post(&app, "/api/auth/mfa/challenge/verify", json!({ "code": mfa::code_at(&secret, t0 + 30).unwrap() }), Some(&pending)).await;
    assert_eq!(status, StatusCode::OK, "the authenticator works again: {body}");

    // wrong recovery codes have a budget of their own (through step-up, so no sign-in budget gets in the way)
    let mut refused = None;
    for i in 0..(mfa::RECOVERY_FAILS + 1) {
        let ((status, _), _) = post(&app, "/api/auth/mfa/recovery-codes", json!({ "code": format!("zzzzz-{i:05}") }), Some(&cookie)).await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            refused = Some(i);
            break;
        }
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(refused, Some(mfa::RECOVERY_FAILS), "the {}th wrong recovery code is refused", mfa::RECOVERY_FAILS + 1);
    let ((status, _), _) = post(&app, "/api/auth/mfa/recovery-codes", json!({ "code": recovery[1] }), Some(&cookie)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "even a right one, until the window ends");
}

/// The password typed again to set up an authenticator is limited per account in the database: 10 wrong
/// ones and even the right one is refused for the rest of the window.
#[tokio::test]
async fn the_setup_password_retry_limit_is_shared() {
    let app = common::TestApp::new().await;
    let cookie = app.session_cookie().await;
    for _ in 0..mfa::REAUTH_FAILS {
        let ((status, _), _) = post(&app, "/api/auth/mfa/enrol", json!({ "password": "wrong-password" }), Some(&cookie)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    let ((status, body), _) = post(&app, "/api/auth/mfa/enrol", json!({ "password": common::PASSWORD }), Some(&cookie)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate.limited")), "{body}");
    // counted in the control database, not in this process: clearing the process's throttle changes nothing
    let mut res = app.control().test_raw().query("SELECT VALUE reauth_attempts FROM ONLY $u").bind(("u", app.user.id.clone())).await.unwrap();
    let n: Option<i64> = res.take(0).unwrap();
    assert_eq!(n, Some(i64::from(mfa::REAUTH_FAILS)));
}
