//! Round 3 review fixes: no tenant text in the control database's audit ledger, session ids,
//! readiness, and the batched export.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use eunomia_backend::rid::RecordIdExt;
use eunomia_backend::store::CONTROL_TABLES;
use serde_json::{json, Value};

#[tokio::test]
async fn tool_arguments_never_reach_the_control_database() {
    let app = TestApp::new().await;
    let canary = "CANARY-5d1f-tenant-text";
    app.tool("memory_write", json!({ "subject_name": format!("Name {canary}"), "subject_kind": "person", "text": format!("fact {canary}") })).await;

    // the tenant audit_log keeps the text (that is the owner's own log) ...
    let (_, audit) = app.http("GET", "/api/audit?limit=10", None, true).await;
    assert!(audit.to_string().contains(canary), "{audit}");

    // ... the shared control database keeps only tool name, argument keys and a hash
    for table in CONTROL_TABLES {
        let rows: Vec<Value> = app.control().test_raw().query(format!("SELECT * FROM {table}")).await.unwrap().take(0).unwrap();
        assert!(!rows.iter().any(|r| r.to_string().contains(canary)), "{canary} found in control table {table}: {rows:?}");
    }
    let rows: Vec<Value> = app.control().test_raw().query("SELECT action, detail FROM audit_event WHERE action = 'tool.memory_write'").await.unwrap().take(0).unwrap();
    let detail = rows[0]["detail"].as_str().unwrap();
    assert!(detail.starts_with("keys=") && detail.contains("text") && detail.contains("hash="), "{detail}");
}

#[tokio::test]
async fn revoking_a_session_rejects_ids_of_other_tables() {
    let app = TestApp::new().await;
    let cookie = app.session_cookie().await;
    let user_id = app.user.id.to_string();
    for id in [app.token.as_str(), user_id.as_str(), "api_token:abc", "nonsense"] {
        let ((status, _), _) = common::http(&app.router, "DELETE", &format!("/api/auth/sessions/{id}"), None, None, Some(&cookie)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{id}");
    }
    // an existing user row is still there
    let ((status, _), _) = common::http(&app.router, "GET", "/api/auth/me", None, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn readyz_checks_the_control_database_and_its_migration_version() {
    let app = TestApp::new().await;
    let ((status, body), _) = common::http(&app.router, "GET", "/readyz", None, None, None).await;
    assert_eq!((status, &body["ok"]), (StatusCode::OK, &json!(true)));
    assert!(body.get("warning").is_none(), "{body}");
    let ((status, _), _) = common::http(&app.router, "GET", "/healthz", None, None, None).await;
    assert_eq!(status, StatusCode::OK);

    // the newest control migration, whichever that is, goes missing from the ledger
    let newest = eunomia_backend::migrate::CONTROL_MIGRATIONS.iter().map(|m| m.0).max().unwrap();
    app.control().test_raw().query(format!("DELETE _migration WHERE version = {newest}")).await.unwrap().check().unwrap();
    // the check is cached for 2 seconds: still ready right after the change, down after it expires
    let ((status, _), _) = common::http(&app.router, "GET", "/readyz", None, None, None).await;
    assert_eq!(status, StatusCode::OK);
    tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
    let ((status, body), _) = common::http(&app.router, "GET", "/readyz", None, None, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "internal");
    assert!(body["detail"].as_str().unwrap().contains(&format!("migration {}", newest - 1)), "{body}");
}

/// The old export ran three queries per entity. The batched one must produce the same document.
#[tokio::test]
async fn batched_export_equals_the_per_entity_export() {
    let app = TestApp::new().await;
    for (name, text) in [("Ada", "likes engines"), ("Ada", "writes notes"), ("Bob", "works at Acme")] {
        app.tool("memory_write", json!({ "subject_name": name, "subject_kind": "person", "text": text })).await;
    }
    app.tool("memory_write", json!({ "subject_name": "Acme", "subject_kind": "organisation", "text": "makes anvils" })).await;
    let found = app.tool("entities_search", json!({ "query": "Ada" })).await;
    let ada = found["results"][0]["id"].as_str().unwrap().to_string();
    let found = app.tool("entities_search", json!({ "query": "Acme" })).await;
    let acme = found["results"][0]["id"].as_str().unwrap().to_string();
    app.tool("code_relate", json!({ "from_id": ada, "to_id": acme, "label": "works at" })).await;

    let (status, doc) = app.http("GET", "/api/export", None, true).await;
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["scope"]["vault"], "personal");
    assert!(doc["scope"]["tables"].as_array().unwrap().iter().any(|t| t == "relates_to"));

    // reference: one query set per entity, the way the old code did it
    let db = app.db().await;
    let raw = db.test_raw();
    let mut reference = Vec::new();
    for e in doc["entities"].as_array().unwrap() {
        let id = e["id"].as_str().unwrap();
        let q = |sql: &str| {
            let raw = &raw;
            let sql = sql.to_string();
            let rid = eunomia_backend::rid::parse(id).unwrap();
            async move { raw.query(sql).bind(("id", rid)).await.unwrap().take::<Vec<Value>>(0).unwrap() }
        };
        let ids = |rows: Vec<Value>| -> Vec<String> {
            let mut v: Vec<String> = rows.iter().map(|r| r["id"].as_str().unwrap().to_string()).collect();
            v.sort();
            v
        };
        let mem = q("SELECT <string> id AS id FROM memory WHERE subject = $id ORDER BY created_at DESC").await;
        let out = q("SELECT <string> id AS id FROM relates_to WHERE in = $id").await;
        let inc = q("SELECT <string> id AS id FROM relates_to WHERE out = $id").await;
        let got_mem: Vec<String> = e["memory"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap().to_string()).collect();
        let rel = |dir: &str| {
            let mut v: Vec<String> =
                e["relations"].as_array().unwrap().iter().filter(|r| r["direction"] == dir).map(|r| r["id"].as_str().unwrap().to_string()).collect();
            v.sort();
            v
        };
        assert_eq!(got_mem, mem.iter().map(|r| r["id"].as_str().unwrap().to_string()).collect::<Vec<_>>(), "memories of {id}");
        assert_eq!(rel("out"), ids(out), "outgoing of {id}");
        assert_eq!(rel("in"), ids(inc), "incoming of {id}");
        reference.push(id.to_string());
    }
    assert_eq!(reference.len(), 3, "Ada, Bob, Acme");
    assert!(doc["entities"].to_string().contains("works at"));
}
