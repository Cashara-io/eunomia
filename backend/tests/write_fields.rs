//! `memory_write` / `memory_write_batch` accept the field names agents tend to send (`content`,
//! `subject`, `memory_type`), reject conflicting duplicates, and a batch that stores nothing is an error.

mod common;

use common::TestApp;
use serde_json::{json, Value};

async fn texts(app: &TestApp, entity: &str) -> Vec<(String, String)> {
    let e = app.tool("entities_get", json!({"id": entity})).await;
    e["memory"].as_array().unwrap().iter().map(|m| (m["type"].as_str().unwrap().to_string(), m["text"].as_str().unwrap().to_string())).collect()
}

async fn entity_count(app: &TestApp) -> usize {
    app.tool("entities_search", json!({"query": "Zed Aliased"})).await["results"].as_array().map(|a| a.len()).unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn aliased_fields_are_stored() {
    let app = TestApp::new().await;
    let w = app.tool("memory_write", json!({"subject": "Zed Aliased", "subject_kind": "person", "content": "Zed likes tea"})).await;
    assert!(w.get("error").is_none(), "{w}");
    let id = w["entity"]["id"].as_str().unwrap();
    assert_eq!(w["entity"]["name"], "Zed Aliased");
    assert_eq!(entity_count(&app).await, 1, "entity_count sees a stored subject");
    assert_eq!(texts(&app, id).await, vec![("world".to_string(), "Zed likes tea".to_string())]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn memory_type_alias_sets_the_type() {
    let app = TestApp::new().await;
    let w = app.tool("memory_write", json!({"subject_name": "Zed Aliased", "subject_kind": "person", "text": "Zed moved", "memory_type": "experience"})).await;
    assert!(w.get("error").is_none(), "{w}");
    assert_eq!(texts(&app, w["entity"]["id"].as_str().unwrap()).await, vec![("experience".to_string(), "Zed moved".to_string())]);
    // equal duplicates are fine
    let w = app.tool("memory_write", json!({"subject_name": "Zed Aliased", "subject_kind": "person", "text": "Zed moved again", "content": "Zed moved again"})).await;
    assert!(w.get("error").is_none(), "{w}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conflicting_duplicate_is_rejected() {
    let app = TestApp::new().await;
    let w = app.tool("memory_write", json!({"subject_name": "Zed Aliased", "subject_kind": "person", "text": "one", "content": "two"})).await;
    let err = w["error"].as_str().unwrap_or_else(|| panic!("expected error: {w}"));
    assert!(err.contains("`text`") && err.contains("`content`"), "{err}");
    assert_eq!(w["code"], "validation.invalid");
    assert_eq!(entity_count(&app).await, 0, "nothing stored");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn batch_partial_success_reports_the_bad_item() {
    let app = TestApp::new().await;
    let r = app
        .tool(
            "memory_write_batch",
            json!({"items": [
                {"subject": "Zed Aliased", "subject_kind": "person", "content": "Zed likes tea", "memory_type": "experience"},
                {"subject_name": "Zed Aliased", "subject_kind": "person"},
            ]}),
        )
        .await;
    assert!(r.get("error").is_none(), "{r}");
    assert_eq!((r["written"].as_i64(), r["failed"].as_i64()), (Some(1), Some(1)), "{r}");
    assert_eq!(r["results"][0]["ok"], true, "{r}");
    assert_eq!(r["results"][1]["ok"], false);
    assert!(r["results"][1]["error"].as_str().unwrap().contains("`text`"), "{r}");
    let id = r["results"][0]["entity_id"].as_str().unwrap();
    assert_eq!(texts(&app, id).await, vec![("experience".to_string(), "Zed likes tea".to_string())]);
}

fn all_bad() -> Value {
    json!({"items": [
        {"subject_name": "Zed Aliased", "subject_kind": "person"},
        {"subject": "Zed Aliased", "subject_kind": "person", "text": "a", "content": "b"},
    ]})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn batch_that_stores_nothing_is_an_error() {
    let app = TestApp::new().await;
    let r = app.tool("memory_write_batch", all_bad()).await;
    assert!(r["error"].as_str().unwrap().contains("no item was stored"), "{r}");
    assert_eq!(r["code"], "validation.invalid");
    assert_eq!((r["written"].as_i64(), r["failed"].as_i64()), (Some(0), Some(2)));
    assert_eq!(r["results"].as_array().unwrap().len(), 2);
    assert_eq!(entity_count(&app).await, 0, "nothing stored");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_marks_a_nothing_stored_batch_as_error() {
    let app = TestApp::new().await;
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "memory_write_batch", "arguments": all_bad()}});
    let ((_, reply), _) = common::http(&app.router, "POST", "/mcp", Some(body), Some(&app.token), None).await;
    assert_eq!(reply["result"]["isError"], json!(true), "{reply}");
}
