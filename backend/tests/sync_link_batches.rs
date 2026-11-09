//! Relation batching has its own process so the global query counter is not shared with other tests.
mod common;

use common::TestApp;
use eunomia_backend::cache::search;
use serde_json::{json, Value};
use surrealdb::types::RecordId;

async fn count(db: &surrealdb::Surreal<surrealdb::engine::any::Any>, sql: &str, owner: &RecordId) -> i64 {
    let mut res = db.query(format!("{sql} GROUP ALL")).bind(("owner", owner.clone())).await.unwrap();
    res.take::<Option<Value>>(0).unwrap().and_then(|v| v["count"].as_i64()).unwrap_or(0)
}

#[tokio::test]
async fn relations_are_deduplicated_and_written_in_bounded_batches() {
    let app = TestApp::new().await;
    let org = app.state.org(&app.user.org).await.unwrap();
    let links: Vec<_> = (0..500).flat_map(|i| {
        let link = json!({"target":format!("demo:note:{i}"),"rel":"wikilink"});
        [link.clone(), link]
    }).collect();
    let env: search::Envelope = serde_json::from_value(json!({
        "id":"demo:note:origin","source":"demo","type":"note","external_id":"origin",
        "title":"Origin","body_text":"links","links":links,
    })).unwrap();
    let before = eunomia_backend::store::SENT.load(std::sync::atomic::Ordering::Relaxed);
    search::upsert(&org.db, &app.user.id, &env).await.unwrap();
    assert_eq!(count(org.db.test_raw(), "SELECT count() FROM linked_to WHERE in.owner = $owner", &app.user.id).await, 500);
    let trips = eunomia_backend::store::SENT.load(std::sync::atomic::Ordering::Relaxed) - before;
    assert!(trips < 20, "relations were written individually: {trips} queries");
}
