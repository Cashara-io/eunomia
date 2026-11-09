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
        // Keep the full consumer scenario off libtest's default 2 MiB thread stack.
        .block_on(Box::pin(async {
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
            // Visit the authorisation endpoint so the mock retains the actual S256 challenge.
            let response = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap()
                .get(link.clone()).send().await.unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::FOUND);
            let invalid_pkce = reqwest::Client::new().post(format!("{origin}/token"))
                .form(&[("grant_type", "authorization_code"), ("code", "one-use-mock-code"), ("code_verifier", "wrong-verifier")])
                .send().await.unwrap();
            assert_eq!(invalid_pkce.status(), reqwest::StatusCode::BAD_REQUEST);
            let ((status, _), _) = call(
                &app, &cookie, "/api/connectors/composio/callback",
                json!({"state":state,"code":"one-use-mock-code"}),
            ).await;
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
            sys(Box::pin(async {
                let first = scheduler::sync_source(&state, &app.user.id, "composio", None).await;
                assert_eq!(first["status"], "partial", "bad file aborted the batch: {first}");
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
                    8
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
                assert_eq!(docs.results.len(), 8);
                assert_eq!(docs.results.iter().filter(|d| d.media_type == "application/pdf").count(), 4);
                assert_eq!(second["failed"], 0, "{second}");
                let warnings = second["warnings"].as_array().unwrap();
                for reason in ["not valid JSON", "no text could be extracted", "document is empty"] {
                    assert_eq!(warnings.iter().filter(|w| w.as_str().unwrap().contains(reason)).count(), 2,
                        "bad file did not produce a visible skip reason: {reason}: {second}");
                }
                assert_eq!(warnings.len(), 10, "bad files blocked the batch: {second}");
                assert_eq!(second["provider_skipped"], 10, "bad files were not counted: {second}");
                let conn = registry::conn_for(&state.settings.encryption_key, Some(&storage::load(&state.db, &app.user.id).await.unwrap())).unwrap();
                use eunomia_backend::sources::base::Source;
                let fetched = eunomia_backend::sources::composio::ComposioSource.fetch(&conn, None).await.unwrap();
                let checkpoint: Value = serde_json::from_str(fetched.cursor.as_deref().unwrap()).unwrap();
                let mut drive_checkpoints = 0;
                for (key, cp) in checkpoint.as_object().unwrap() {
                    if serde_json::from_str::<Vec<String>>(key).unwrap()[0] == "googledrive" {
                        drive_checkpoints += 1;
                        assert!(cp["pending"].as_array().unwrap().is_empty(), "parse failure blocked checkpoint");
                        assert!(!cp["since"].as_str().unwrap().is_empty());
                    }
                }
                assert_eq!(drive_checkpoints, 2);
                let tail: Vec<_> = fetched.records.iter().filter(|r| r["title"] == "tail.txt").collect();
                assert_eq!(tail.len(), 2);
                assert!(tail.iter().all(|r| r["document"]["media_type"] == "text/plain"), "normalised MIME was discarded");
                let other = common::register(&app.state, "another@example.test").await;
                let row = storage::load(&state.db, &other.id).await.unwrap();
                assert!(row.credentials_encrypted.is_empty());
                assert!(account::access_token(&state, &other.id).await.is_err());
                // A provider sleeps while status is in flight. Disconnect must complete first,
                // including when remote revocation is also slow. Stale status must be rejected.
                let row = storage::replace(&state.db, &state.settings.encryption_key, &other.id, &row,
                    &json!({"access_token":"composio-canary-access","expires_at":chrono::Utc::now().timestamp()+3600})).await.unwrap().unwrap();
                let client = reqwest::Client::new();
                client.post(format!("{origin}/control")).json(&json!({"status_delay":5,"status_started":false,"revoke_delay":5})).send().await.unwrap();
                let pending_status = http(&app.router, "GET", "/api/connectors/composio/status", None, None, Some(&cookie));
                // Use the signed-in owner for the route race. Keep the disposable account for sync below.
                let original = storage::load(&state.db, &app.user.id).await.unwrap();
                let disconnect = async {
                    tokio::time::timeout(std::time::Duration::from_secs(3), async {
                        loop {
                            let stats: Value = client.get(format!("{origin}/stats")).send().await.unwrap().json().await.unwrap();
                            if stats["status_started"] == true { break; }
                            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                        }
                    }).await.expect("status never reached the sleeping provider");
                    let ((status, body), _) = tokio::time::timeout(std::time::Duration::from_secs(3),
                        call(&app, &cookie, "/api/connectors/composio/disconnect", json!({})))
                        .await.expect("disconnect waited for a slow status provider");
                    assert!(status.is_success(), "disconnect did not complete promptly: {body}");
                    assert!(body["message"].as_str().unwrap().starts_with("Disconnected."));
                    client.post(format!("{origin}/control")).json(&json!({"status_delay":0,"revoke_delay":0})).send().await.unwrap();
                    storage::load(&state.db, &app.user.id).await.unwrap()
                };
                let (((status, body), _), disconnected) = tokio::join!(pending_status, disconnect);
                assert!(!status.is_success(), "stale status survived disconnect: {body}");
                let after = storage::load(&state.db, &app.user.id).await.unwrap();
                assert!(after.credentials_encrypted.is_empty(), "status restored credentials after disconnect");
                assert_eq!(after.updated_at, disconnected.updated_at, "status wrote after disconnect");
                // Restore a fresh connected generation for the remaining flow assertions.
                let credentials = service::credentials_of(&state.settings.encryption_key, &original).unwrap();
                let restored = storage::replace(&state.db, &state.settings.encryption_key, &app.user.id, &after, &credentials).await.unwrap().unwrap();
                assert!(storage::select(&state.db, &app.user.id, &restored, original.config, original.routing, original.enabled).await.unwrap());
                // Exercise the actual registry path with a slow file-bucket write.
                let row = storage::replace(&state.db, &state.settings.encryption_key, &other.id, &row,
                    &json!({"access_token":"composio-canary-access","expires_at":chrono::Utc::now().timestamp()+3600})).await.unwrap().unwrap();
                assert!(storage::select(&state.db, &other.id, &row,
                    json!({"apps":[{"toolkit":"googledrive","account":"googledrive-mine"}]}), json!({}), true).await.unwrap());
                let before = eunomia_backend::isolation::rewritten();
                eunomia_backend::isolation::mutate("documents.object_put", "file::put", "SLEEP 500ms; file::put");
                let syncing = registry::run_sync(&state, &other.id, "composio", None);
                let disconnecting = async {
                    tokio::time::timeout(std::time::Duration::from_secs(10), async {
                        while eunomia_backend::isolation::rewritten() == before {
                            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                        }
                    }).await.expect("sync never reached document upload");
                    account::disconnect(&state, &other.id).await.unwrap();
                };
                let (synced, ()) = tokio::join!(syncing, disconnecting);
                eunomia_backend::isolation::clear_mutations();
                assert!(synced.is_err(), "sync published after disconnect during upload");
                let docs = eunomia_backend::documents::list(&state, &other.id, None, false, 50, 0).await.unwrap();
                assert!(docs.results.is_empty(), "documents appeared after disconnect");
                let rows: Vec<Value> = state.db.test_raw().query("SELECT * FROM cache_record WHERE owner = $owner")
                    .bind(("owner", other.id.clone())).await.unwrap().take(0).unwrap();
                assert!(rows.is_empty(), "records appeared after disconnect");
                let http = reqwest::Client::new();
                http.post(format!("{origin}/control")).json(&json!({"pause_refresh":true,"refresh_started":false})).send().await.unwrap();
                let row = storage::load(&state.db, &app.user.id).await.unwrap();
                let mut credentials = service::credentials_of(&state.settings.encryption_key, &row).unwrap();
                let previous_refresh = credentials["refresh_token"].clone();
                credentials["expires_at"] = json!(0);
                storage::replace(&state.db, &state.settings.encryption_key, &app.user.id, &row, &credentials).await.unwrap().unwrap();
                let refresh = account::access_token(&state, &app.user.id);
                let select = async {
                    for _ in 0..100 {
                        let stats: Value = http.get(format!("{origin}/stats")).send().await.unwrap().json().await.unwrap();
                        if stats["refresh_started"] == true { break; }
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                    let leased = storage::load(&state.db, &app.user.id).await.unwrap();
                    assert!(service::credentials_of(&state.settings.encryption_key, &leased).unwrap()["lease_until"].as_i64().unwrap() > 0);
                    assert!(storage::select(&state.db, &app.user.id, &leased, leased.config.clone(), leased.routing.clone(), true).await.unwrap());
                    http.post(format!("{origin}/control")).json(&json!({"pause_refresh":false})).send().await.unwrap();
                };
                let (refreshed, ()) = tokio::join!(refresh, select);
                assert!(refreshed.is_ok(), "selection lost the provider's rotated token");
                let current = storage::load(&state.db, &app.user.id).await.unwrap();
                let mut credentials = service::credentials_of(&state.settings.encryption_key, &current).unwrap();
                let stats: Value = http.get(format!("{origin}/stats")).send().await.unwrap().json().await.unwrap();
                assert_eq!(credentials["refresh_token"], stats["refresh_token"]);
                assert_ne!(credentials["refresh_token"], previous_refresh, "the mock did not rotate the refresh token");
                let stale = http.post(format!("{origin}/token"))
                    .form(&[("grant_type", "refresh_token"), ("refresh_token", previous_refresh.as_str().unwrap())])
                    .send().await.unwrap();
                assert_eq!(stale.status(), reqwest::StatusCode::BAD_REQUEST, "the mock accepted a spent refresh token");
                credentials["expires_at"] = json!(0);
                storage::replace(&state.db, &state.settings.encryption_key, &app.user.id, &current, &credentials).await.unwrap().unwrap();
                http.post(format!("{origin}/control")).json(&json!({"revoke_refresh":true})).send().await.unwrap();
                let (_, health) = app.http("GET", "/api/connectors/composio/status", None, true).await;
                assert!(health["health"].as_str().is_some_and(|status| status.contains("reconnect Composio")), "revoked refresh did not request reconnect: {health}");
                assert!(!health.to_string().contains("canary"));
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
            }))
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
        }));
}
