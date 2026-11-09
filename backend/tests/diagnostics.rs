mod common;
use std::{io::Read, time::Duration};
use axum::{body::Body, http::{Request, StatusCode}};
use http_body_util::BodyExt;
use tower::ServiceExt;
use common::TestApp;
use serde_json::json;

async fn bundle(app: &TestApp, cookie: &str) -> axum::response::Response {
    app.router.clone().oneshot(Request::post("/api/diagnostics").header("cookie", cookie).body(Body::empty()).unwrap()).await.unwrap()
}

#[tokio::test]
async fn diagnostics_admin_zip_is_content_free_and_member_is_forbidden() {
    let status_dir = tempfile::tempdir().unwrap();
    let mut settings = common::test_settings();
    settings.update_status_dir = status_dir.path().join("status").to_string_lossy().into_owned();
    settings.openai_api_key = Some("SECRET_CANARY".into());
    settings.openai_base_url = "https://user:SECRET_CANARY@models.example.test/private?token=SECRET_CANARY".into();
    let app = TestApp::with_settings(settings).await;
    let cookie = app.session_cookie().await;
    let db = app.db().await;
    db.test_raw().query("CREATE connector SET enabled = true, owner = $owner, kind = 'gmail', credentials_encrypted = 'SECRET_CANARY', label = 'EMAIL_CANARY@example.com', config = { private: 'MEMORY_CANARY' }; CREATE memory SET owner = $owner, subject = person:canary, vault = vault:canary, text = 'MEMORY_CANARY DOCUMENT_CANARY CHAT_CANARY', type = 'world';")
        .bind(("owner", app.user.id.clone())).await.unwrap().check().unwrap();
    let sync_id = surrealdb::types::RecordId::new("sync_status", format!("{}:gmail", eunomia_backend::sources::base::owner_key_str(&app.user.id)));
    db.test_raw().query("CREATE $sync SET owner = $owner, cursor = 'CURSOR_CANARY', last_run = time::now(), last_error = '429 MEMORY_CANARY password=SECRET_CANARY', last_report = {written: 3, skipped: 2, failed: 1, status: 'partial', provider_skipped: 5, retry_after_secs: 60, warnings: ['DOCUMENT_CANARY'], errors: ['DOCUMENT_CANARY']};")
        .bind(("sync", sync_id)).bind(("owner", app.user.id.clone())).await.unwrap().check().unwrap();
    app.control().test_raw().query("CREATE job SET org = $org, owner = $owner, kind = 'sync', payload = {source: 'gmail', private: 'CHAT_CANARY'}, idempotency_key = 'diagnostics-fixture', status = 'dead', attempts = 1, last_error_code = 'internal';")
        .bind(("org", app.user.org.key())).bind(("owner", app.user.id.clone())).await.unwrap().check().unwrap();
    let member = common::register(&app.state, "member@example.com").await;
    let ((_, _), set_cookie) = common::http(&app.router, "POST", "/api/auth/login", Some(json!({"email": member.email, "password": common::PASSWORD})), None, None).await;
    let member_cookie = set_cookie.unwrap().split(';').next().unwrap().to_string();
    assert_eq!(bundle(&app, &member_cookie).await.status(), StatusCode::FORBIDDEN);
    // Browser session requirement is independent of admin rights.
    let (status, _) = app.http("POST", "/api/diagnostics", None, true).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Real tracing boundary, with content in both fields and formatted messages.
    use tracing_subscriber::prelude::*;
    let subscriber = tracing_subscriber::registry().with(eunomia_backend::diagnostics::logs::SafeLogs {
        ring: eunomia_backend::diagnostics::logs::LOGS.clone(), stdout: false,
    });
    tracing::subscriber::with_default(subscriber, || {
        tracing::warn!(target: "eunomia_backend::diagnostics_test", memory = "MEMORY_CANARY", document = "DOCUMENT_CANARY", count = 7, "SECRET_CANARY CHAT_CANARY");
    });
    let root = status_dir.path().join("status");
    std::fs::create_dir(&root).unwrap();
    let updater = tokio::spawn(async move {
        let request = root.join("diagnostics-requested");
        while !request.exists() { tokio::task::yield_now().await; }
        let nonce = std::fs::read_to_string(request).unwrap();
        let out = root.join("diagnostics");
        std::fs::create_dir(&out).unwrap();
        std::fs::write(out.join("compose-ps.json"), r#"[{"service":"backend","state":"running","image":"ghcr.io/cashara-io/eunomia-backend:v2.2.0"}]"#).unwrap();
        std::fs::write(out.join("collection.json"), r#"{"data_volume_free_bytes":1048576}"#).unwrap();
        assert!(std::process::Command::new("mkfifo").arg(out.join("frontend.log")).status().unwrap().success());
        for service in ["backend", "surrealdb", "updater", "backup", "caddy", "secrets-helper", "surreal-upgrade"] {
            std::fs::write(out.join(format!("{service}.log")), "provider error: {\"count\":314159265358979323846}\n{\"level\":\"WARN\",\"fields\":{\"count\":8675309,\"status\":61412345678,\"duration_ms\":4111111111111111}}\n").unwrap();
        }
        std::fs::write(out.join("ready"), nonce).unwrap();
    });
    let response = bundle(&app, &cookie).await;
    updater.await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/zip");
    assert_eq!(response.headers()["cache-control"], "no-store");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
    let mut names = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).unwrap();
        names.push(entry.name().to_string());
        let mut text = String::new();
        entry.read_to_string(&mut text).unwrap();
        for secret in ["314159265358979323846", "8675309", "61412345678", "4111111111111111"] { assert!(!text.contains(secret), "numeric secret leaked in {}", entry.name()); }
        assert!(!text.contains("CANARY"), "customer data leaked in {}", entry.name());
        if entry.name() == "connectors.json" {
            let value: serde_json::Value = serde_json::from_str(&text).unwrap();
            let connection = &value["connections"][0];
            assert_eq!(connection["source_type"], "gmail");
            assert_eq!(connection["sync"]["last_error"], "rate_limited");
            assert_eq!(connection["sync"]["written"], 3);
            assert_eq!(connection["sync"]["skipped"], 2);
            assert_eq!(connection["sync"]["partial"], true);
            assert_eq!(connection["sync"]["provider_skipped"], 5);
            assert_eq!(connection["sync"]["retry_after_secs"], 60);
            assert!(!connection["recent_jobs"].as_array().unwrap().is_empty());
        }
        if entry.name() == "summary.json" {
            let summary: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(summary["model_endpoint_host"], "models.example.test");
            assert_eq!(summary["data_volume_free_bytes"], 1048576);
            assert!(!summary["tenant_migrations"].as_array().unwrap().is_empty());
        }
    }
    for expected in ["summary.json", "summary.md", "connectors.json", "backend.log", "health.json", "README.txt", "containers/README.txt"] { assert!(names.iter().any(|n| n == expected), "missing {expected}"); }
    let mut audit = app.control().test_raw().query("SELECT action FROM audit_event WHERE action = 'server.diagnostics'").await.unwrap();
    let rows: Vec<serde_json::Value> = audit.take(0).unwrap();
    assert_eq!(rows.len(), 1);
    if let Ok(path) = std::env::var("DIAGNOSTICS_SAMPLE_ZIP") { std::fs::write(path, &bytes).unwrap(); }
    assert!(!names.iter().any(|n| n == "containers/frontend.log"), "FIFO entered bundle");
    let mut note = String::new();
    zip.by_name("containers/README.txt").unwrap().read_to_string(&mut note).unwrap();
    assert!(note.contains("frontend.log") && note.contains("omitted"));
    std::fs::remove_dir_all(status_dir.path().join("status")).unwrap();
    db.test_raw().query("UPDATE sync_status SET last_error = '', consecutive_failures = 0, last_report.retry_after_secs = 61412345678 WHERE owner = $owner")
        .bind(("owner", app.user.id.clone())).await.unwrap().check().unwrap();
    let second = bundle(&app, &cookie).await;
    assert_eq!(second.status(), StatusCode::OK, "second request must not see a leaked bundle lock");
    let bytes = second.into_body().collect().await.unwrap().to_bytes();
    let mut second_zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
    let mut connectors = String::new();
    second_zip.by_name("connectors.json").unwrap().read_to_string(&mut connectors).unwrap();
    assert!(!connectors.contains("61412345678"));
    let connectors: serde_json::Value = serde_json::from_str(&connectors).unwrap();
    assert_eq!(connectors["connections"][0]["status"], "partial");
    assert_eq!(connectors["connections"][0]["sync"]["provider_skipped"], 5);
    assert!(connectors["connections"][0]["sync"]["retry_after_secs"].is_null());
}

#[tokio::test]
async fn diagnostics_old_updater_times_out_ignores_stale_results_and_leaves_note() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("diagnostics")).unwrap();
    std::fs::write(dir.path().join("diagnostics/ready"), "old-request").unwrap();
    std::fs::write(dir.path().join("diagnostics/backend.log"), "STALE_CANARY").unwrap();
    let files = eunomia_backend::diagnostics::container_files(dir.path(), Duration::from_millis(10)).await;
    assert_eq!(files.len(), 1);
    assert!(files[0].1.contains("2.2.0 or earlier"));
    assert!(!files[0].1.contains("CANARY"));
    for _ in 0..100 {
        if !dir.path().join("diagnostics-requested").exists() { break; }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert!(!dir.path().join("diagnostics-requested").exists());
}

#[tokio::test]
async fn diagnostics_matching_updater_result_is_collected() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let writer = tokio::spawn(async move {
        let request = root.join("diagnostics-requested");
        while !request.exists() { tokio::task::yield_now().await; }
        let nonce = std::fs::read_to_string(request).unwrap();
        std::fs::create_dir(root.join("diagnostics")).unwrap();
        std::fs::write(root.join("diagnostics/backend.log"), "{\"level\":\"WARN\",\"line\":123}\n").unwrap();
        std::fs::write(root.join("diagnostics/ready"), nonce).unwrap();
    });
    let files = eunomia_backend::diagnostics::container_files(dir.path(), Duration::from_secs(2)).await;
    writer.await.unwrap();
    assert!(files.iter().any(|(name, text)| name == "containers/backend.log" && text.contains("WARN")));
}

#[test]
fn diagnostics_archive_size_cap_is_enforced_and_explained() {
    let (file, capped) = eunomia_backend::diagnostics::test_archive(vec![("summary.json".into(), "{}".into()), ("backend.log".into(), "large".repeat(10000))], 4096).unwrap();
    assert!(capped);
    assert!(file.as_file().metadata().unwrap().len() <= 4096);
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let mut note = String::new();
    zip.by_name("LIMITS.txt").unwrap().read_to_string(&mut note).unwrap();
    assert!(note.contains("backend.log"));
}
