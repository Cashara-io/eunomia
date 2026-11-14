//! A default single-organisation install (`EUNOMIA_SIGNUP_ORG` and `SIGNUP` unset: `join`, open signup)
//! keeps working with organisation invitations in the build. This binary sets no sign-up env at all.

mod common;

use axum::http::StatusCode;
use eunomia_backend::models_user;
use serde_json::json;

#[tokio::test]
async fn the_default_join_install_keeps_working() {
    assert!(std::env::var("EUNOMIA_SIGNUP_ORG").is_err() && std::env::var("SIGNUP").is_err(), "this binary runs on the defaults");
    let app = common::TestApp::new().await;
    let cookie = app.session_cookie().await;
    let ((status, me), _) = common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
    // the first user created and owns the one org, and is its instance admin
    assert_eq!((me["org_role"].as_str(), me["can_manage_org"].as_bool(), me["is_admin"].as_bool()), (Some("owner"), Some(true), Some(true)), "{me}");

    // an ordinary signup through the open route joins the one org as a member, as before
    let ((status, bob), _) = common::http(&app.router, "POST", "/api/auth/register", Some(json!({"email": "bob@example.com", "password": common::PASSWORD})), None, None).await;
    assert_eq!(status, StatusCode::OK, "{bob}");
    assert_eq!((bob["org_role"].as_str(), bob["can_manage_org"].as_bool()), (Some("member"), Some(false)));
    assert_eq!(models_user::user_by_email(&app.state.control, "bob@example.com").await.unwrap().unwrap().org, app.user.org);

    // and an invitation lands in the same one org
    let ((status, created), _) = common::http(&app.router, "POST", "/api/org/invitations", Some(json!({"email": "cy@example.com"})), None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let ((status, _), _) = common::http(
        &app.router,
        "POST",
        "/api/auth/register",
        Some(json!({"email": "cy@example.com", "password": common::PASSWORD, "invitation": created["token"]})),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(models_user::user_by_email(&app.state.control, "cy@example.com").await.unwrap().unwrap().org, app.user.org);

    // the API token, a session and the vault list are untouched
    let (status, _) = app.http("GET", "/api/vaults", None, true).await;
    assert_eq!(status, StatusCode::OK);
    let ((status, _), _) = common::http(&app.router, "GET", "/api/vaults", None, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
}
