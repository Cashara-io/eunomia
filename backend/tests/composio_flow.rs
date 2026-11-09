//! Full consumer flow. The test starts its own consumer MCP fixture.
mod common;
use common::{PASSWORD, TestApp, http, sys};
use eunomia_backend::{
    connectors::{
        composio::{account, storage},
        service,
    },
    sources::{registry, scheduler},
};
use serde_json::{Value, json};

#[test]
fn consumer_oauth_apps_refresh_ingest_partial_and_disconnect() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let child = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/composio_mock.py"
        ))
        .arg(port.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    struct Mock(std::process::Child);
    impl Drop for Mock {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _mock = Mock(child);
    let origin = format!("http://127.0.0.1:{port}");
    // This integration binary has one test. Set the operator-only override before starting threads.
    unsafe {
        std::env::set_var("EUNOMIA_COMPOSIO_TEST_ORIGIN", &origin);
        std::env::set_var("EUNOMIA_ALLOW_CONNECTOR_BASE_URL", "1");
    }
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            for _ in 0..100 {
                if reqwest::get(format!("{origin}/stats")).await.is_ok() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            let app = TestApp::new().await;
            let ((status, _), cookie) = http(
                &app.router,
                "POST",
                "/api/auth/login",
                Some(json!({"email":app.user.email,"password":PASSWORD})),
                None,
                None,
            )
            .await;
            assert!(status.is_success());
            let cookie = cookie.unwrap();
            async fn call(
                app: &TestApp,
                cookie: &str,
                path: &str,
                body: Value,
            ) -> ((axum::http::StatusCode, Value), Option<String>) {
                http(&app.router, "POST", path, Some(body), None, Some(cookie)).await
            }
            let ((status, begin), _) =
                call(&app, &cookie, "/api/connectors/composio/begin", json!({})).await;
            assert!(status.is_success(), "{begin}");
            let link = url::Url::parse(begin["url"].as_str().unwrap()).unwrap();
            let state = link
                .query_pairs()
                .find(|(k, _)| k == "state")
                .unwrap()
                .1
                .to_string();
            let ((status, _), _) = call(
                &app,
                &cookie,
                "/api/connectors/composio/callback",
                json!({"state":"wrong","code":"one-use-mock-code"}),
            )
            .await;
            assert!(!status.is_success());
            let ((_, _), second_cookie) = http(
                &app.router,
                "POST",
                "/api/auth/login",
                Some(json!({"email":app.user.email,"password":PASSWORD})),
                None,
                None,
            )
            .await;
            let ((status, _), _) = call(
                &app,
                second_cookie.as_deref().unwrap(),
                "/api/connectors/composio/callback",
                json!({"state":state,"code":"one-use-mock-code"}),
            )
            .await;
            assert!(
                !status.is_success(),
                "a different browser session consumed pending OAuth state"
            );
            let ((status, _), _) = call(
                &app,
                &cookie,
                "/api/connectors/composio/callback",
                json!({"state":state,"code":"one-use-mock-code"}),
            )
            .await;
            assert!(status.is_success());
            let ((status, _), _) = call(
                &app,
                &cookie,
                "/api/connectors/composio/callback",
                json!({"state":state,"code":"one-use-mock-code"}),
            )
            .await;
            assert!(!status.is_success(), "callback replay succeeded");
            let (_, status) = app
                .http("GET", "/api/connectors/composio/status", None, true)
                .await;
            assert_eq!(status["account"], "mock-person@example.test");
            assert_eq!(status["accounts"].as_array().unwrap().len(), 7);
            assert!(!status.to_string().contains("canary"));
            let apps = vec![
                json!({"toolkit":"gmail","account":"gmail-mine"}),
                json!({"toolkit":"googledrive","account":"googledrive-mine"}),
                json!({"toolkit":"googledrive","account":"googledrive-second"}),
                json!({"toolkit":"googlecalendar","account":"googlecalendar-mine"}),
                json!({"toolkit":"timelinesai","account":"timelinesai-mine"}),
                json!({"toolkit":"whatsapp","account":"whatsapp-mine","phone_number_id":"phone1"}),
                json!({"toolkit":"github","account":"github-mine","repo":"fixture/vault"}),
            ];
            let ((status, out), _) = call(
                &app,
                &cookie,
                "/api/connectors/composio/selection",
                json!({"apps":apps,"vault":null}),
            )
            .await;
            assert!(status.is_success(), "{out}");
            let state = app.state.org(&app.user.org).await.unwrap();
            sys(async {
                let first = scheduler::sync_source(&state, &app.user.id, "composio", None).await;
                assert_eq!(first["status"], "partial", "{first}");
                assert!(
                    first["message"]
                        .as_str()
                        .unwrap()
                        .starts_with("rate limited"),
                    "{first}"
                );
                let second = scheduler::sync_source(&state, &app.user.id, "composio", None).await;
                assert_ne!(second["status"], "error", "{second}");
                let mut r = eunomia_backend::store::dynamic(
                    &state.db,
                    "test.composio_records",
                    "SELECT * FROM cache_record WHERE owner = $owner AND source = 'composio'",
                )
                .bind(("owner", app.user.id.clone()))
                .await
                .unwrap();
                let records: Vec<Value> = r.take(0).unwrap();
                assert!(records.len() >= 8, "{records:?}");
                assert!(!json!(records).to_string().contains("canary"));
                assert_eq!(
                    records
                        .iter()
                        .filter(|r| r["type"] == "composio.file")
                        .count(),
                    2
                );
                assert!(records.iter().any(|r| {
                    r["type"] == "composio.note"
                        && r["payload"]["frontmatter"]
                            .as_str()
                            .unwrap()
                            .contains("parent")
                }));
                let docs =
                    eunomia_backend::documents::list(&state, &app.user.id, None, false, 50, 0)
                        .await
                        .unwrap();
                assert_eq!(docs.results.len(), 2);
                let other = common::register(&app.state, "another@example.test").await;
                let row = storage::load(&state.db, &other.id).await.unwrap();
                assert!(row.credentials_encrypted.is_empty());
                assert!(account::access_token(&state, &other.id).await.is_err());
                let current = storage::load(&state.db, &app.user.id).await.unwrap();
                assert!(current.credentials_encrypted.starts_with("enc:v1:"));
                assert!(account::disconnect(&state, &app.user.id).await.unwrap());
                let row = storage::load(&state.db, &app.user.id).await.unwrap();
                assert_eq!(
                    service::credentials_of(&state.settings.encryption_key, &row).unwrap(),
                    json!({})
                );
                assert!(
                    storage::replace(
                        &state.db,
                        &state.settings.encryption_key,
                        &app.user.id,
                        &current,
                        &json!({"access_token":"stale"})
                    )
                    .await
                    .unwrap()
                    .is_none()
                );
                assert!(
                    registry::connection(&state.db, &app.user.id, "composio")
                        .await
                        .unwrap()
                        .is_some()
                );
            })
            .await;
            let stats: Value = reqwest::get(format!("{origin}/stats"))
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert!(
                stats["schema_requests"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|s| *s == "GOOGLEDRIVE_FIND_FILE")
                    .count()
                    >= 4,
                "tool preparation was reused across different accounts"
            );
            assert!(stats["refreshes"].as_u64().unwrap() >= 1);
            assert!(stats["revokes"].as_u64().unwrap() >= 1);
            let (_, export) = app.http("GET", "/api/export", None, true).await;
            assert!(!export.to_string().contains("canary"));
        });
}
