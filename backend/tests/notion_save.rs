//! Issue #102: saving a Notion integration secret failed with a 500 `internal` on an install upgraded to
//! 2.0.3, because the connector row predated the `routing` field. One test, because it edits the environment.

mod common;

use common::TestApp;
use eunomia_backend::sources::mock::{route, serve};
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saving_and_testing_a_notion_secret_on_a_row_from_before_2_0_3() {
    // SAFETY: this binary has a single test, so nothing else reads the environment concurrently.
    unsafe { std::env::set_var("EUNOMIA_ALLOW_CONNECTOR_BASE_URL", "1") };
    let app = TestApp::new().await;
    // A row created before migration 0015 (the connectors page creates one per kind on first visit)
    // has no `routing`: drop the field, create the row, then define it again exactly as 0015 does.
    let db = app.db().await;
    db.test_raw()
        .query("REMOVE FIELD routing ON connector; CREATE connector SET owner = $owner, kind = 'notion';")
        .query(include_str!("../migrations/tenant/0015_vault_routing.surql"))
        .bind(("owner", app.user.id.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();

    let notion = serve(vec![route("GET", "/users/me", json!({"object": "user", "id": "u1", "type": "bot", "bot": {}}))]).await;
    let save = json!({"enabled": true, "credentials": {"integration_token": "ntn_abc123"}, "config": {"base_url": notion.base}});
    let (status, body) = app.http_session("PUT", "/api/connectors/notion", Some(save)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["credentials_set"], true, "{body}");
    assert_eq!(body["routing"], json!({"rules": []}), "{body}");

    let (status, body) = app.http_session("POST", "/api/connectors/notion/test", None).await;
    assert_eq!((status.as_u16(), &body), (200, &json!({"ok": true})));
    let req = &notion.requests()[0];
    assert_eq!((req.header("authorization"), req.header("notion-version")), ("Bearer ntn_abc123", "2022-06-28"));

    // A secret Notion refuses is a clear answer from the test, not a server error.
    let refused = serve(vec![route("GET", "/users/me", json!({"object": "error", "status": 401, "code": "unauthorized", "message": "API token is invalid."})).status(401)]).await;
    let (status, _) = app.http_session("PUT", "/api/connectors/notion", Some(json!({"config": {"base_url": refused.base}}))).await;
    assert_eq!(status, 200);
    let (status, body) = app.http_session("POST", "/api/connectors/notion/test", None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ok"], false, "{body}");
    let error = body["error"].as_str().unwrap();
    assert!(error.contains("HTTP 401") && error.contains("API token is invalid."), "{error}");
}
