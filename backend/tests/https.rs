//! Settings, HTTPS: the routes only drop a request file for the updater. Reading the status is
//! open to any signed-in user, changing it is for instance admins, and bad input never reaches the file.

mod common;

use axum::http::StatusCode;
use common::{http, TestApp};
use serde_json::json;

async fn app_with_status_dir() -> (TestApp, std::path::PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("eunomia-https-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut settings = common::test_settings();
    settings.update_status_dir = dir.to_string_lossy().into_owned();
    let app = TestApp::with_settings(settings).await; // the first user is the admin
    let member = common::register(&app.state, "member@example.com").await;
    let member_token = eunomia_backend::models_user::create_api_token_with(&app.state.control, &member.id, "m", &eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect::<Vec<_>>(), None, None).await.unwrap().token;
    (app, dir, member_token)
}

#[tokio::test]
async fn an_admin_requests_https_and_the_status_follows_the_updater() {
    let (app, dir, member_token) = app_with_status_dir().await;

    let (status, body) = app.http("GET", "/api/https/status", None, true).await;
    assert_eq!((status, &body["configured"], &body["state"]), (StatusCode::OK, &json!(true), &json!("off")), "{body}");

    let req = json!({"enabled": true, "domain": " Eunomia.Example.com ", "email": "me@example.com"});
    let (status, body) = app.http("POST", "/api/https", Some(req), true).await;
    assert_eq!((status, &body["requested"]), (StatusCode::OK, &json!(true)), "{body}");
    let file = std::fs::read_to_string(dir.join("https.json")).unwrap();
    assert!(file.contains("\"domain\": \"eunomia.example.com\""), "{file}");

    // a request the updater has not picked up yet already reads as pending, for every signed-in user
    for bearer in [app.token.as_str(), member_token.as_str()] {
        let (status, body) = http(&app.router, "GET", "/api/https/status", None, Some(bearer), None).await.0;
        assert_eq!((status, &body["state"], &body["domain"]), (StatusCode::OK, &json!("pending"), &json!("eunomia.example.com")), "{body}");
    }

    // the updater consumed the request and reported its result
    std::fs::remove_file(dir.join("https.json")).unwrap();
    std::fs::write(dir.join("https-status.json"), r#"{"state": "active", "domain": "eunomia.example.com", "message": null}"#).unwrap();
    let (_, body) = app.http("GET", "/api/https/status", None, true).await;
    assert_eq!((&body["state"], &body["configured"]), (&json!("active"), &json!(true)), "{body}");

    let (status, body) = app.http("POST", "/api/https", Some(json!({"enabled": false})), true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = app.http("GET", "/api/https/status", None, true).await;
    assert_eq!(body["state"], "off", "{body}");
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn only_an_instance_admin_can_change_https() {
    let (app, dir, member_token) = app_with_status_dir().await;
    let req = json!({"enabled": true, "domain": "eunomia.example.com", "email": "me@example.com"});
    let (status, body) = http(&app.router, "POST", "/api/https", Some(req), Some(&member_token), None).await.0;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("auth.forbidden")), "{body}");
    assert!(!dir.join("https.json").exists(), "a refused request leaves no file");
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn bad_input_is_refused_before_anything_is_written() {
    let (app, dir, _) = app_with_status_dir().await;
    for (domain, email) in [("evil.com;reboot", "me@example.com"), ("127.0.0.1", "me@example.com"), ("localhost", "me@example.com"), ("eunomia.example.com", "nope"), ("", "")] {
        let (status, body) = app.http("POST", "/api/https", Some(json!({"enabled": true, "domain": domain, "email": email})), true).await;
        assert_eq!((status, body["code"].as_str()), (StatusCode::BAD_REQUEST, Some("validation.invalid")), "{domain:?} {body}");
    }
    assert!(!dir.join("https.json").exists());
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn without_an_updater_https_is_not_configured() {
    let app = TestApp::new().await; // update_status_dir does not exist
    let (_, body) = app.http("GET", "/api/https/status", None, true).await;
    assert_eq!(body, json!({"configured": false}));
    let req = json!({"enabled": true, "domain": "eunomia.example.com", "email": "me@example.com"});
    let (status, body) = app.http("POST", "/api/https", Some(req), true).await;
    assert_eq!((status, body), (StatusCode::OK, json!({"configured": false})));
}
