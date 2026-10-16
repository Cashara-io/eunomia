//! Agent-defined entity kinds (#101): an agent writes under a kind of its own ("db", "decision"),
//! the kind lives in the vault it was written to, and its entities behave like built-in ones in
//! search, recall, get, graph, list, merge and vault copies, without leaking into another vault.

mod common;

use common::TestApp;
use eunomia_backend::tools::registry;
use serde_json::{json, Value};

async fn call(app: &TestApp, user: &eunomia_backend::models_user::User, name: &str, args: Value) -> Value {
    match common::sys(registry::call(&app.state, user, name, args)).await {
        Ok(v) => v,
        Err(e) => json!({ "error": e.message }),
    }
}

fn ok(v: Value) -> Value {
    assert!(v.get("error").is_none(), "{v}");
    v
}

fn texts(v: &Value) -> Vec<String> {
    v["results"].as_array().unwrap_or_else(|| panic!("results in {v}")).iter().map(|r| r["text"].as_str().unwrap_or_default().to_string()).collect()
}

fn names(v: &Value) -> Vec<String> {
    v["results"].as_array().unwrap_or_else(|| panic!("results in {v}")).iter().map(|r| r["name"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn an_agent_writes_under_its_own_kind_and_finds_it_again() {
    let app = TestApp::new().await;

    // a new kind name creates the kind with the entity
    let out = ok(app.tool("memory_write", json!({"subject_name": "orders_db", "subject_kind": "db", "text": "orders_db runs Postgres 16 on the billing host"})).await);
    assert_eq!(out["entity"]["kind"], "db", "{out}");
    let id = out["entity"]["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("entity:"), "{id}");

    // the same name and kind (any case) is the same entity; the same name under another kind is not
    let again = ok(app.tool("memory_write", json!({"subject_name": "ORDERS_DB", "subject_kind": "db", "text": "orders_db is backed up nightly"})).await);
    assert_eq!(again["entity"]["id"], id.as_str());
    let table = ok(app.tool("memory_write", json!({"subject_name": "orders_db", "subject_kind": "table", "text": "a table also called orders_db"})).await);
    assert_ne!(table["entity"]["id"], id.as_str());
    assert_eq!(table["entity"]["kind"], "table");

    let types = ok(app.tool("entity_types", json!({})).await);
    assert_eq!(types["built_in"], json!(["person", "organisation", "location", "repository", "file", "symbol"]));
    assert_eq!(types["custom"], json!([{"kind": "db", "entities": 1}, {"kind": "table", "entities": 1}]));

    // get, search (by name, by kind, by fact text), recall and the graph all see it
    let got = ok(app.tool("entities_get", json!({"id": id})).await);
    assert_eq!(got["kind"], "db");
    assert_eq!(got["memory"].as_array().unwrap().len(), 2, "{got}");
    assert_eq!(names(&ok(app.tool("entities_search", json!({"query": "orders"})).await)).len(), 2);
    let only_db = ok(app.tool("entities_search", json!({"query": "orders", "kind": "db"})).await);
    assert_eq!(only_db["results"].as_array().unwrap().len(), 1, "{only_db}");
    assert_eq!(only_db["results"][0]["kind"], "db");
    assert_eq!(names(&ok(app.tool("entities_search", json!({"query": "postgres"})).await)), ["orders_db"]);
    let recalled = ok(app.tool("recall", json!({"query": "what do we know about orders_db?"})).await);
    assert!(texts(&recalled).iter().any(|t| t.contains("Postgres 16")), "{recalled}");
    let graph = ok(app.tool("entities_graph", json!({"kinds": ["db"]})).await);
    assert_eq!(graph["nodes"].as_array().unwrap().len(), 1, "{graph}");
    assert_eq!(graph["nodes"][0]["kind"], "db");

    // relations and code-style linking work across built-in and custom kinds
    let person = ok(app.tool("memory_write", json!({"subject_name": "Ada", "subject_kind": "person", "text": "Ada owns the billing data"})).await);
    ok(app.tool("code_relate", json!({"from_id": person["entity"]["id"], "to_id": id, "label": "owns"})).await);
    let graph = ok(app.tool("entities_graph", json!({})).await);
    assert_eq!(graph["edges"].as_array().unwrap().len(), 1, "{graph}");

    // REST: list by a custom kind, create one
    let (status, body) = app.http("GET", "/api/entities?kind=db", None, true).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["total"], 1);
    let (status, body) = app.http("POST", "/api/entities", Some(json!({"kind": "decision", "name": "Use Postgres"})), true).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["kind"], "decision");

    // merging stays within one kind
    let other = ok(app.tool("memory_write", json!({"subject_name": "orders database", "subject_kind": "db", "text": "the orders database"})).await);
    let refused = app.tool("entity_merge", json!({"winner_id": id, "loser_id": table["entity"]["id"]})).await;
    assert!(refused.get("error").is_some(), "{refused}");
    let merged = ok(app.tool("entity_merge", json!({"winner_id": id, "loser_id": other["entity"]["id"]})).await);
    assert_eq!(merged["kind"], "db");
    assert!(merged["aliases"].as_array().unwrap().contains(&json!("orders database")), "{merged}");

    // vault clone keeps custom kinds
    let cloned = ok(app.tool("vault_clone", json!({"vault_id": "Personal", "name": "Copy"})).await);
    let types = ok(app.tool("entity_types", json!({"vault_id": cloned["id"]})).await);
    assert_eq!(types["custom"], json!([{"kind": "db", "entities": 1}, {"kind": "decision", "entities": 1}, {"kind": "table", "entities": 1}]));
}

#[tokio::test]
async fn kind_names_are_checked_and_capped_per_vault() {
    let app = TestApp::new().await;
    for bad in ["DB", "1db", "db-table", "", "x = 1 OR true; DELETE user; --", &"a".repeat(33)] {
        let out = app.tool("memory_write", json!({"subject_name": "thing", "subject_kind": bad, "text": "t"})).await;
        assert!(out.get("error").is_some(), "{bad:?}: {out}");
    }
    let max = eunomia_backend::entities::service::MAX_CUSTOM_KINDS;
    for i in 0..max {
        ok(app.tool("memory_write", json!({"subject_name": "thing", "subject_kind": format!("k{i}"), "text": "t"})).await);
    }
    let over = app.tool("memory_write", json!({"subject_name": "thing", "subject_kind": "one_too_many", "text": "t"})).await;
    assert!(over["error"].as_str().is_some_and(|e| e.contains("custom entity kinds")), "{over}");
    // a kind the vault already has still takes new entities, and another vault has its own room
    ok(app.tool("memory_write", json!({"subject_name": "other thing", "subject_kind": "k0", "text": "t"})).await);
    ok(app.tool("vault_create", json!({"name": "Fresh"})).await);
    ok(app.tool("memory_write", json!({"subject_name": "thing", "subject_kind": "one_too_many", "text": "t", "vault_id": "Fresh"})).await);
}

/// Schema N-1 (before 0016) is still served while an org's migration job runs, and has no custom-kind
/// table: every read across the entity tables still works there, and only a custom-kind write fails.
#[tokio::test]
async fn reads_work_on_a_database_from_before_custom_kinds() {
    let app = TestApp::new().await;
    ok(app.tool("memory_write", json!({"subject_name": "Ada", "subject_kind": "person", "text": "Ada drinks tea"})).await);
    app.db().await.test_raw().query("REMOVE TABLE entity").await.unwrap().check().unwrap();

    assert_eq!(names(&ok(app.tool("entities_search", json!({"query": "ada"})).await)), ["Ada"]);
    assert!(texts(&ok(app.tool("recall", json!({"query": "what does Ada drink?"})).await)).iter().any(|t| t.contains("tea")));
    assert_eq!(ok(app.tool("entities_graph", json!({})).await)["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(ok(app.tool("entity_types", json!({})).await)["custom"], json!([]));
    let (status, body) = app.http("GET", "/api/entities", None, true).await;
    assert_eq!((status.as_u16(), body["total"].clone()), (200, json!(1)), "{body}");
    let (status, body) = app.http("GET", "/api/export", None, true).await;
    assert_eq!(status.as_u16(), 200, "{body}");
    ok(app.tool("vault_clone", json!({"vault_id": "Personal", "name": "Copy"})).await);
    let refused = app.tool("memory_write", json!({"subject_name": "orders_db", "subject_kind": "db", "text": "t"})).await;
    assert!(refused.get("error").is_some(), "{refused}");
}

#[tokio::test]
async fn a_custom_kind_stays_in_its_vault() {
    let app = TestApp::new().await;
    let alice = app.user.clone();
    let bob = common::register(&app.state, "bob@example.com").await;

    ok(call(&app, &alice, "vault_create", json!({"name": "A"})).await);
    ok(call(&app, &alice, "vault_create", json!({"name": "B"})).await);
    let written = ok(call(&app, &alice, "memory_write", json!({"subject_name": "ledger_db", "subject_kind": "db", "text": "ledger_db holds SECRETLEDGER rows", "vault_id": "A"})).await);
    let id = written["entity"]["id"].as_str().unwrap().to_string();
    ok(call(&app, &alice, "memory_write", json!({"subject_name": "notes", "subject_kind": "person", "text": "unrelated", "vault_id": "B"})).await);

    // vault B: no trace of the kind, the entity or its fact
    let types = ok(call(&app, &alice, "entity_types", json!({"vault_id": "B"})).await);
    assert_eq!(types["custom"], json!([]), "{types}");
    assert!(names(&ok(call(&app, &alice, "entities_search", json!({"query": "ledger", "vault_id": "B"})).await)).is_empty());
    assert!(names(&ok(call(&app, &alice, "entities_search", json!({"query": "", "kind": "db", "vault_id": "B"})).await)).is_empty());
    let recalled = ok(call(&app, &alice, "recall", json!({"query": "ledger_db", "vault_id": "B"})).await);
    assert!(!texts(&recalled).iter().any(|t| t.contains("SECRETLEDGER")), "{recalled}");
    let graph = ok(call(&app, &alice, "entities_graph", json!({"vault_id": "B"})).await);
    assert!(graph["nodes"].as_array().unwrap().iter().all(|n| n["kind"] != "db"), "{graph}");

    // vault A: all there
    let types = ok(call(&app, &alice, "entity_types", json!({"vault_id": "A"})).await);
    assert_eq!(types["custom"], json!([{"kind": "db", "entities": 1}]));
    assert_eq!(names(&ok(call(&app, &alice, "entities_search", json!({"query": "ledger", "vault_id": "A"})).await)), ["ledger_db"]);

    // bob, not a member of A: nothing by default, refused when he names it, not found by id
    let types = ok(call(&app, &bob, "entity_types", json!({})).await);
    assert_eq!(types["custom"], json!([]), "{types}");
    assert!(names(&ok(call(&app, &bob, "entities_search", json!({"query": "ledger"})).await)).is_empty());
    assert!(call(&app, &bob, "entity_types", json!({"vault_id": "A"})).await.get("error").is_some());
    assert!(call(&app, &bob, "entities_get", json!({"id": id})).await.get("error").is_some());
    let recalled = ok(call(&app, &bob, "recall", json!({"query": "ledger_db"})).await);
    assert!(!texts(&recalled).iter().any(|t| t.contains("SECRETLEDGER")), "{recalled}");
}
