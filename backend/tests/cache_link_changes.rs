//! A sync that changes only a record's links (a transaction re-categorised) reconciles them, without
//! rewriting, re-embedding or re-extracting the record; an identical replay changes nothing.

mod common;

use common::TestApp;
use eunomia_backend::cache::{ingest, search};
use eunomia_backend::state::OrgState;
use serde_json::{json, Value};
use surrealdb::types::RecordId;

fn map(raw: &Value) -> Result<Vec<search::Envelope>, String> {
    let env = json!({
        "id": "demo:txn:t1", "source": "demo", "type": "txn", "external_id": "t1",
        "title": "Coles", "body_text": "Coles groceries $5.00",
        "links": [{"target": format!("demo:category:{}", raw["cat"].as_str().unwrap()), "rel": "category"}],
    });
    Ok(vec![serde_json::from_value(env).map_err(|e| e.to_string())?])
}

async fn sync(org: &OrgState, owner: &RecordId, cat: &str) -> ingest::IngestReport {
    let report = ingest::ingest(org, owner, "demo", &[json!({"id": "t1", "cat": cat})], map).await.unwrap();
    assert_eq!(report.failed, 0, "{:?}", report.errors);
    report
}

async fn count(db: &surrealdb::Surreal<surrealdb::engine::any::Any>, sql: &str, owner: &RecordId) -> i64 {
    let mut res = db.query(format!("{sql} GROUP ALL")).bind(("owner", owner.clone())).await.unwrap();
    res.take::<Option<Value>>(0).unwrap().and_then(|v| v["count"].as_i64()).unwrap_or(0)
}

async fn edge_ids(org: &OrgState, owner: &RecordId) -> Vec<String> {
    let mut res = org.db.test_raw().query("SELECT VALUE id FROM linked_to WHERE in.owner = $owner").bind(("owner", owner.clone())).await.unwrap();
    let ids: Vec<RecordId> = res.take(0).unwrap();
    ids.iter().map(|i| format!("{i:?}")).collect()
}

#[tokio::test]
async fn a_link_only_change_is_reconciled_and_an_identical_replay_is_a_no_op() {
    let app = TestApp::new().await;
    let org = app.state.org(&app.user.org).await.unwrap();
    let owner = app.user.id.clone();
    let targets = |links: Vec<search::LinkEntry>| links.into_iter().map(|l| l.target_id).collect::<Vec<_>>();
    let extract_jobs = || count(app.control().test_raw(), "SELECT count() FROM job WHERE kind = 'extract' AND owner = $owner", &owner);

    let first = sync(&org, &owner, "groceries").await;
    assert_eq!((first.written, first.skipped), (1, 0));
    let before = search::get(&org.db, &search::RecordScope::own(&owner), "demo:txn:t1").await.unwrap().unwrap();
    assert_eq!(extract_jobs().await, 1);

    // only the category link changed: the new link replaces the old one, the record is not rewritten
    let second = sync(&org, &owner, "takeaway").await;
    assert_eq!((second.written, second.skipped), (0, 1), "a link-only change is not a content change");
    let links = search::links(&org.db, &search::RecordScope::own(&owner), "demo:txn:t1", Some("category")).await.unwrap();
    assert_eq!(targets(links), ["demo:category:takeaway"]);
    assert_eq!(count(org.db.test_raw(), "SELECT count() FROM cache_record WHERE owner = $owner", &owner).await, 1);
    assert_eq!(count(org.db.test_raw(), "SELECT count() FROM linked_to WHERE in.owner = $owner", &owner).await, 1);
    let after = search::get(&org.db, &search::RecordScope::own(&owner), "demo:txn:t1").await.unwrap().unwrap();
    assert_eq!((after.updated_at, &after.content_hash), (before.updated_at, &before.content_hash), "the record itself is untouched");
    assert_eq!(extract_jobs().await, 1, "no re-extraction for a link-only change");

    // an identical replay changes nothing: same edge, same record
    let edges = edge_ids(&org, &owner).await;
    let third = sync(&org, &owner, "takeaway").await;
    assert_eq!((third.written, third.skipped), (0, 1));
    assert_eq!(edge_ids(&org, &owner).await, edges, "an unchanged link set is not rewritten");
    let replayed = search::get(&org.db, &search::RecordScope::own(&owner), "demo:txn:t1").await.unwrap().unwrap();
    assert_eq!(replayed.updated_at, before.updated_at);
    assert_eq!(extract_jobs().await, 1);
}
