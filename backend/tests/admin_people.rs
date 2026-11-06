//! Settings > People actions: an admin makes someone an admin and takes it back, the first account
//! keeps its rights, and a suspended account loses its sessions, its tokens and its sign-in at once.

mod common;

use axum::http::StatusCode;
use eunomia_backend::models_user;
use eunomia_backend::rid::RecordIdExt;
use serde_json::{json, Value};

fn person<'a>(report: &'a Value, email: &str) -> &'a Value {
    report["people"].as_array().unwrap().iter().find(|p| p["email"] == email).unwrap_or_else(|| panic!("{email} in {report}"))
}

async fn login(app: &common::TestApp, email: &str) -> ((StatusCode, Value), Option<String>) {
    common::http(&app.router, "POST", "/api/auth/login", Some(json!({ "email": email, "password": common::PASSWORD })), None, None).await
}

fn cookie_of(set_cookie: Option<String>) -> String {
    set_cookie.expect("login sets a cookie").split(';').next().unwrap().to_string()
}

async fn audit_summaries(app: &common::TestApp, cookie: &str) -> Vec<String> {
    let ((status, page), _) = common::http(&app.router, "GET", "/api/admin/audit?category=admin", None, None, Some(cookie)).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    page["results"].as_array().unwrap().iter().map(|r| r["summary"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn grant_and_revoke_admin_with_the_first_account_protected() {
    let app = common::TestApp::new().await;
    let bob = common::register(&app.state, "bob@example.com").await;
    let owner_cookie = app.session_cookie().await;
    let bob_path = |what: &str| format!("/api/admin/people/{}/{what}", bob.id.to_string());

    // bob is a member: the people list says so and the admin routes refuse him
    let ((_, report), _) = common::http(&app.router, "GET", "/api/admin/people", None, None, Some(&owner_cookie)).await;
    assert_eq!(person(&report, "bob@example.com")["role"], "member");
    assert_eq!(person(&report, app.user.email.as_str())["admin"], "creator");
    assert_eq!(person(&report, app.user.email.as_str())["is_you"], true);
    let bob_cookie = cookie_of(login(&app, &bob.email).await.1);
    let ((status, _), _) = common::http(&app.router, "GET", "/api/admin/people", None, None, Some(&bob_cookie)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // a token cannot change admin rights, even the admin's own
    let (status, _) = app.http("PUT", &bob_path("admin"), Some(json!({ "admin": true })), true).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // grant
    let ((status, report), _) = common::http(&app.router, "PUT", &bob_path("admin"), Some(json!({ "admin": true })), None, Some(&owner_cookie)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!((person(&report, "bob@example.com")["role"].as_str(), person(&report, "bob@example.com")["admin"].as_str()), (Some("admin"), Some("granted")));
    // bob is an admin now: his session sees the people list and /me says so
    let ((status, _), _) = common::http(&app.router, "GET", "/api/admin/people", None, None, Some(&bob_cookie)).await;
    assert_eq!(status, StatusCode::OK);
    let ((_, me), _) = common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&bob_cookie)).await;
    assert_eq!(me["is_admin"], true, "{me}");

    // the first account cannot be demoted, not even by another admin
    let creator_path = format!("/api/admin/people/{}/admin", app.user.id.to_string());
    let ((status, body), _) = common::http(&app.router, "PUT", &creator_path, Some(json!({ "admin": false })), None, Some(&bob_cookie)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // revoke
    let ((status, report), _) = common::http(&app.router, "PUT", &bob_path("admin"), Some(json!({ "admin": false })), None, Some(&owner_cookie)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(person(&report, "bob@example.com")["role"], "member");
    let ((status, _), _) = common::http(&app.router, "GET", "/api/admin/people", None, None, Some(&bob_cookie)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // an account outside the workspace is not found
    let ((status, _), _) = common::http(&app.router, "PUT", "/api/admin/people/user:nobody/admin", Some(json!({ "admin": true })), None, Some(&owner_cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let summaries = audit_summaries(&app, &owner_cookie).await;
    assert_eq!(summaries, ["Took admin rights away from bob@example.com", "Made bob@example.com an admin"]);
    // the "changes" filter shows them and leaves the sign-ins out
    let ((_, changes), _) = common::http(&app.router, "GET", "/api/admin/audit?category=changes", None, None, Some(&owner_cookie)).await;
    let changes = changes["results"].as_array().unwrap();
    assert!(changes.iter().any(|r| r["action"] == "admin.grant"));
    assert!(changes.iter().all(|r| r["category"] != "signins"), "{changes:?}");
}

#[tokio::test]
async fn a_suspended_account_loses_sessions_tokens_and_sign_in() {
    let app = common::TestApp::new().await;
    let bob = common::register(&app.state, "bob@example.com").await;
    let owner_cookie = app.session_cookie().await;
    let bob_token = models_user::create_api_token(&app.state.control, &bob.id, "laptop").await.unwrap().token;
    let bob_cookie = cookie_of(login(&app, &bob.email).await.1);
    // both work, and are now cached by the gate
    let ((status, _), _) = common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&bob_cookie)).await;
    assert_eq!(status, StatusCode::OK);
    let ((status, _), _) = common::http(&app.router, "GET", "/api/vaults", None, Some(&bob_token), None).await;
    assert_eq!(status, StatusCode::OK);

    // nobody can suspend themselves or the first account
    let own = format!("/api/admin/people/{}/suspended", app.user.id.to_string());
    let ((status, body), _) = common::http(&app.router, "PUT", &own, Some(json!({ "suspended": true })), None, Some(&owner_cookie)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let path = format!("/api/admin/people/{}/suspended", bob.id.to_string());
    let ((status, report), _) = common::http(&app.router, "PUT", &path, Some(json!({ "suspended": true })), None, Some(&owner_cookie)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert!(person(&report, "bob@example.com")["suspended_at"].is_string(), "{report}");

    // at once: the session is gone, the token is refused, and signing in again is refused
    let ((status, _), _) = common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&bob_cookie)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let ((status, body), _) = common::http(&app.router, "GET", "/api/vaults", None, Some(&bob_token), None).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.suspended")), "{body}");
    let ((status, body), cookie) = login(&app, &bob.email).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.suspended")), "{body}");
    assert!(cookie.is_none());
    // a wrong password says nothing about the suspension
    let ((status, body), _) =
        common::http(&app.router, "POST", "/api/auth/login", Some(json!({ "email": bob.email, "password": "wrong-password" })), None, None).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::UNAUTHORIZED, Some("auth.unauthorized")));

    // restore: the token works again and bob can sign in
    let ((status, report), _) = common::http(&app.router, "PUT", &path, Some(json!({ "suspended": false })), None, Some(&owner_cookie)).await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert!(person(&report, "bob@example.com")["suspended_at"].is_null());
    let ((status, _), _) = common::http(&app.router, "GET", "/api/vaults", None, Some(&bob_token), None).await;
    assert_eq!(status, StatusCode::OK);
    let ((status, _), cookie) = login(&app, &bob.email).await;
    assert_eq!(status, StatusCode::OK);
    assert!(cookie.is_some());

    assert_eq!(audit_summaries(&app, &owner_cookie).await, ["Restored access for bob@example.com", "Suspended bob@example.com"]);
}

#[tokio::test]
async fn an_admin_sees_and_revokes_someone_elses_api_tokens() {
    let app = common::TestApp::new().await;
    let bob = common::register(&app.state, "bob@example.com").await;
    let carol = common::register(&app.state, "carol@example.com").await;
    let owner_cookie = app.session_cookie().await;
    let laptop = models_user::create_api_token(&app.state.control, &bob.id, "work laptop").await.unwrap();
    models_user::create_api_token(&app.state.control, &bob.id, "ci").await.unwrap();
    let bob_cookie = cookie_of(login(&app, &bob.email).await.1);
    let tokens_path = format!("/api/admin/people/{}/tokens", bob.id.to_string());
    let revoke_path = format!("{tokens_path}/{}", laptop.id.to_string());

    // the people list counts them
    let ((_, report), _) = common::http(&app.router, "GET", "/api/admin/people", None, None, Some(&owner_cookie)).await;
    assert_eq!(person(&report, "bob@example.com")["tokens"], 2, "{report}");
    assert_eq!(person(&report, "carol@example.com")["tokens"], 0);

    // the admin sees names and dates, never a secret
    let ((status, list), _) = common::http(&app.router, "GET", &tokens_path, None, None, Some(&owner_cookie)).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let names: Vec<&str> = list.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"work laptop") && names.contains(&"ci"), "{list}");
    assert!(list[0].get("token").is_none() && list[0].get("token_hash").is_none(), "{list}");

    // a member can neither list nor revoke
    let ((status, _), _) = common::http(&app.router, "GET", &tokens_path, None, None, Some(&bob_cookie)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let ((status, _), _) = common::http(&app.router, "DELETE", &revoke_path, None, None, Some(&bob_cookie)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // an admin's token cannot revoke: it takes the browser session
    let (status, _) = app.http("DELETE", &revoke_path, None, true).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // a token id under the wrong person is not found
    let ((status, _), _) =
        common::http(&app.router, "DELETE", &format!("/api/admin/people/{}/tokens/{}", carol.id.to_string(), laptop.id.to_string()), None, None, Some(&owner_cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // the token works until revoked, then stops at once
    let ((status, _), _) = common::http(&app.router, "GET", "/api/vaults", None, Some(&laptop.token), None).await;
    assert_eq!(status, StatusCode::OK);
    let ((status, left), _) = common::http(&app.router, "DELETE", &revoke_path, None, None, Some(&owner_cookie)).await;
    assert_eq!(status, StatusCode::OK, "{left}");
    assert_eq!(left.as_array().unwrap().len(), 1);
    assert_eq!(left[0]["name"], "ci");
    let ((status, _), _) = common::http(&app.router, "GET", "/api/vaults", None, Some(&laptop.token), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let ((status, _), _) = common::http(&app.router, "DELETE", &revoke_path, None, None, Some(&owner_cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    assert_eq!(audit_summaries(&app, &owner_cookie).await, ["Revoked bob@example.com\u{2019}s API token \u{201c}work laptop\u{201d}"]);
}
