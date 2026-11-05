//! The vault integrity check (`integrity.rs`): finds a memory whose entity is gone and a change-log
//! counter that fell behind, and `vault_repair` fixes both.

mod common;

use common::TestApp;
use serde_json::{json, Value};

fn personal(list: &Value) -> String {
    list["results"].as_array().unwrap().iter().find(|v| v["kind"] == "personal").expect("a personal vault")["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn check_finds_and_repair_fixes_dangling_memories_and_a_counter_behind() {
    let app = TestApp::new().await;
    let w = app.tool("memory_write", json!({"subject_name": "Alice", "subject_kind": "person", "text": "Alice likes green tea"})).await;
    assert!(w.get("error").is_none(), "{w}");
    let entity = w["entity"]["id"].as_str().unwrap().to_string();
    let vault = personal(&app.tool("vault_list", json!({})).await);

    let clean = app.tool("vault_check", json!({"vault_id": vault})).await;
    assert_eq!(clean["ok"], true, "{clean}");

    // what a crash between two writes could leave: the entity gone under its memory, the counter behind
    let org = app.state.org(&app.user.org).await.unwrap();
    let raw = org.db.test_raw();
    raw.query(format!("DELETE {entity}")).await.unwrap().check().unwrap();
    raw.query("UPDATE change_seq SET n = 0").await.unwrap().check().unwrap();

    let found = app.tool("vault_check", json!({"vault_id": vault})).await;
    assert_eq!(found["ok"], false, "{found}");
    assert_eq!(found["dangling_memories"].as_array().unwrap().len(), 1, "{found}");
    assert_eq!(found["change_log_behind"], true, "{found}");

    let fixed = app.tool("vault_repair", json!({"vault_id": vault})).await;
    assert_eq!(fixed["ok"], true, "{fixed}");
    assert_eq!(fixed["repaired"]["memories_removed"], 1, "{fixed}");
    assert_eq!(fixed["repaired"]["counter_fixed"], true, "{fixed}");

    // the log keeps working after the repair: a new write appends past the old top
    let again = app.tool("memory_write", json!({"subject_name": "Bob", "subject_kind": "person", "text": "Bob drinks coffee"})).await;
    assert!(again.get("error").is_none(), "{again}");
    assert_eq!(app.tool("vault_check", json!({"vault_id": vault})).await["ok"], true);
}
