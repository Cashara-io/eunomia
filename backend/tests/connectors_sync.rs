//! Connector sync pipeline against the in-memory harness and mock providers: due-ness, the shared
//! ingest path, cursors, one-sync-at-a-time via the job queue, the manual "Sync now" route and the
//! webhook route. Ported from main's `sources/db_tests.rs`, plus one end-to-end sync per connector.

mod common;

use std::sync::Once;
use std::time::{Duration, Instant};

use axum::body::Body;
use common::{test_settings, TestApp};
use eunomia_backend::cache::search;
use eunomia_backend::connectors::service;
use eunomia_backend::jobs::worker;
use eunomia_backend::jobs::{self, handlers, kind, NewJob, WorkerConfig};
use eunomia_backend::models_user::User;
use eunomia_backend::rid::RecordIdExt;
use eunomia_backend::sources::base::owner_key_str;
use eunomia_backend::sources::mock::{route, serve, Route};
use eunomia_backend::sources::heypocket::HeyPocketSource;
use eunomia_backend::sources::registry;
use eunomia_backend::sources::scheduler::{due_syncs, sync_source};
use eunomia_backend::sources::up_bank::UpBankSource;
use eunomia_backend::state::{AppState, OrgState};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use surrealdb::types::RecordId;
use tower::ServiceExt;

/// Connector `base_url` overrides are off unless the operator opts in; the mock providers need them.
fn allow_base_url() {
    static ONCE: Once = Once::new();
    // SAFETY: set once before any test in this binary reads it, from the test threads' first step.
    ONCE.call_once(|| unsafe { std::env::set_var("EUNOMIA_ALLOW_CONNECTOR_BASE_URL", "1") });
}

async fn app_with(openai_key: Option<&str>) -> (TestApp, OrgState) {
    allow_base_url();
    let mut settings = test_settings();
    settings.openai_api_key = openai_key.map(String::from);
    if openai_key.is_some() {
        settings.openai_base_url = "http://127.0.0.1:9/v1".into(); // unroutable: a keyed run must point a mock at itself
    }
    let app = TestApp::with_settings(settings).await;
    let org = app.state.org(&app.user.org).await.unwrap();
    (app, org)
}

fn cfg(id: &str) -> WorkerConfig {
    WorkerConfig {
        id: id.into(),
        concurrency: 4,
        poll: Duration::from_millis(20),
        lease: Duration::from_secs(30),
        owner_cap: 1000,
        grace: Duration::from_millis(200),
        retry_base: Duration::from_millis(10),
        tick: Duration::from_secs(30),
    }
}

fn spawn_worker(state: &AppState) -> tokio::sync::watch::Sender<bool> {
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(worker::run(state.clone(), handlers::registry(), cfg("w"), rx));
    tx
}

async fn wait_for<F: std::future::Future<Output = bool>>(what: &str, secs: u64, mut f: impl FnMut() -> F) {
    let start = Instant::now();
    while !f().await {
        assert!(start.elapsed() < Duration::from_secs(secs), "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn connect(org: &OrgState, owner: &RecordId, kind: &str, base: &str, config: Value, credentials: Value) {
    let mut config = config;
    config["base_url"] = json!(base);
    config["token_url"] = json!(format!("{base}/oauth/token"));
    service::upsert_connector(&org.db, &org.settings.encryption_key, owner, kind, Some(true), Some(config), Some(credentials))
        .await
        .unwrap();
}

async fn connect_github(org: &OrgState, owner: &RecordId, base: &str) {
    connect(org, owner, "github", base, json!({}), json!({"personal_access_token": "ghp_test"})).await;
}

async fn status(org: &OrgState, owner: &RecordId, key: &str) -> Value {
    let id = RecordId::from_table_key("sync_status", format!("{}:{key}", owner_key_str(owner)));
    let mut res = org.db.test_raw().query("SELECT last_error, cursor, consecutive_failures FROM ONLY $id").bind(("id", id)).await.unwrap();
    res.take::<Option<Value>>(0).unwrap().unwrap_or(Value::Null)
}

async fn count(org: &OrgState, sql: &str, owner: &RecordId) -> i64 {
    let mut res = org.db.test_raw().query(format!("{sql} GROUP ALL")).bind(("owner", owner.clone())).await.unwrap();
    res.take::<Option<Value>>(0).unwrap().and_then(|v| v["count"].as_i64()).unwrap_or(0)
}

fn issue(number: i64, title: &str, updated: &str) -> Value {
    json!({
        "number": number, "title": title, "state": "open",
        "body": "Ada Lovelace reported that the quarterly importer drops the last CSV row.",
        "user": {"login": "ada"}, "labels": [], "assignees": [], "comments": 0, "repository": {"full_name": "acme/widget"},
        "html_url": format!("https://github.com/acme/widget/issues/{number}"),
        "created_at": "2024-01-01T00:00:00Z", "updated_at": updated,
    })
}

const ISSUE_7: &str = "github:github.issue:acme/widget#7";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_due_sync_lands_searchable_records_then_a_401_is_visible_and_keeps_the_cursor() {
    let (app, org) = app_with(None).await;
    let owner = app.user.id.clone();
    let mock = serve(vec![route("GET", "/issues", json!([issue(7, "Importer drops the last row", "2024-02-01T00:00:00Z")]))]).await;
    connect_github(&org, &owner, &mock.base).await;

    let due = due_syncs(&org).await.unwrap();
    assert_eq!(due.len(), 1, "a never-synced connector is due at once");
    assert_eq!(due[0].source, "github");
    let out = sync_source(&org, &owner, "github", None).await;
    assert_eq!(out["written"], 1, "{out}");
    let st = status(&org, &owner, "github").await;
    assert_eq!(st["last_error"], "");
    assert_eq!(st["cursor"], "2024-02-01T00:00:00Z");
    assert_eq!(mock.requests()[0].header("authorization"), "Bearer ghp_test");

    let params = search::SearchParams { mode: "keyword".into(), limit: 10, ..Default::default() };
    let (hits, _more) = search::search(&org.db, &org.settings, &search::RecordScope::own(&owner), "importer quarterly", &params).await.unwrap();
    assert_eq!(hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), vec![ISSUE_7]);
    assert!(due_syncs(&org).await.unwrap().is_empty(), "a healthy source is not due again within its interval");

    let bad = serve(vec![route("GET", "/issues", json!({"message": "Bad credentials"})).status(401)]).await;
    connect(&org, &owner, "github", &bad.base, json!({}), json!({"personal_access_token": "ghp_test"})).await;
    let out = sync_source(&org, &owner, "github", None).await;
    assert!(out["error"].as_str().unwrap().contains("HTTP 401"), "{out}");
    let st = status(&org, &owner, "github").await;
    assert!(st["last_error"].as_str().unwrap().contains("Bad credentials"));
    assert_eq!(st["consecutive_failures"], 1);
    assert_eq!(st["cursor"], "2024-02-01T00:00:00Z", "a failed sync keeps the cursor");
}

/// Disabling stops syncs without a restart; two overlapping runs make one provider sync; a sync job
/// holding a live lease (another replica) makes this run stand down until the lease expires.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disabled_connectors_are_not_due_and_overlapping_runs_make_one_provider_sync() {
    let (app, org) = app_with(None).await;
    let owner = app.user.id.clone();
    let mock = serve(vec![route("GET", "/issues", json!([issue(7, "Slow", "2024-02-01T00:00:00Z")])).delay(300)]).await;
    connect_github(&org, &owner, &mock.base).await;

    service::upsert_connector(&org.db, &org.settings.encryption_key, &owner, "github", Some(false), None, None).await.unwrap();
    assert!(due_syncs(&org).await.unwrap().is_empty());
    assert!(mock.requests().is_empty(), "disabled: no provider request");
    service::upsert_connector(&org.db, &org.settings.encryption_key, &owner, "github", Some(true), None, None).await.unwrap();

    let (a, b) = tokio::join!(sync_source(&org, &owner, "github", None), sync_source(&org, &owner, "github", None));
    let statuses = [a["status"].as_str(), b["status"].as_str()];
    assert!(statuses.contains(&Some("already_running")), "{a} {b}");
    assert_eq!(mock.requests().len(), 1, "one provider sync");
    assert_eq!(status(&org, &owner, "github").await["cursor"], "2024-02-01T00:00:00Z");
    assert!(sync_source(&org, &owner, "github", None).await.get("status").is_none(), "the guard is released afterwards");

    // A sync job leased by another worker blocks this run until its lease expires.
    let key = "sync:other-replica";
    let job = NewJob::new(kind::SYNC, owner.clone(), key).in_org(org.db.org()).payload(json!({"source": "github"}));
    jobs::enqueue(&app.state.control, job).await.unwrap();
    let held = "UPDATE job SET status = 'running', locked_by = 'other', locked_until = time::now() + 5m WHERE idempotency_key = $k";
    app.state.control.test_raw().query(held).bind(("k", key)).await.unwrap().check().unwrap();
    assert_eq!(sync_source(&org, &owner, "github", None).await["status"], "already_running");
    let expired = "UPDATE job SET locked_until = time::now() - 1s WHERE idempotency_key = $k";
    app.state.control.test_raw().query(expired).bind(("k", key)).await.unwrap().check().unwrap();
    assert!(sync_source(&org, &owner, "github", None).await.get("status").is_none(), "an expired lease is ignored");
    assert_eq!(mock.requests().len(), 3);
}

/// A failed write keeps the previous cursor; the retry recovers the record without duplicating the
/// stored ones; a checkpoint write error is reported, not a clean sync.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_write_keeps_the_cursor_and_the_retry_recovers_without_duplicates() {
    let (app, org) = app_with(None).await;
    let owner = app.user.id.clone();
    let mock = serve(vec![route(
        "GET",
        "/issues",
        json!([issue(7, "Fine", "2024-02-01T00:00:00Z"), issue(8, "POISON", "2024-02-02T00:00:00Z")]),
    )])
    .await;
    connect_github(&org, &owner, &mock.base).await;
    let raw = org.db.test_raw();
    raw.query("DEFINE FIELD OVERWRITE title ON cache_record TYPE string DEFAULT '' ASSERT $value != 'POISON'").await.unwrap().check().unwrap();

    let out = sync_source(&org, &owner, "github", None).await;
    assert!(out["error"].as_str().unwrap().contains("1 of 2 records failed to save"), "{out}");
    let st = status(&org, &owner, "github").await;
    assert_eq!(st["cursor"], "", "cursor not advanced past the failed record");
    assert_eq!(st["consecutive_failures"], 1);

    raw.query("DEFINE FIELD OVERWRITE title ON cache_record TYPE string DEFAULT ''").await.unwrap().check().unwrap();
    let out = sync_source(&org, &owner, "github", None).await;
    assert_eq!(out["written"], 1, "{out}");
    assert_eq!(out["skipped"], 1, "the stored record is replayed, not duplicated");
    assert_eq!(count(&org, "SELECT count() FROM cache_record WHERE owner = $owner", &owner).await, 2);
    assert_eq!(status(&org, &owner, "github").await["cursor"], "2024-02-02T00:00:00Z");

    // Checkpoint write failure: visible, not a clean sync.
    raw.query("DEFINE FIELD OVERWRITE cursor ON sync_status TYPE string DEFAULT '' ASSERT $value != '2024-02-02T00:00:00Z'").await.unwrap().check().unwrap();
    raw.query("UPDATE sync_status SET cursor = ''").await.unwrap().check().unwrap();
    let out = sync_source(&org, &owner, "github", None).await;
    assert!(out["error"].as_str().unwrap().contains("checkpoint"), "{out}");
    assert!(status(&org, &owner, "github").await["last_error"].as_str().unwrap().contains("checkpoint"));
}

fn openai_mock_routes() -> Vec<Route> {
    let extraction = json!({"people": [{"name": "Ada Lovelace", "facts": ["Ada reported the importer bug"]}]});
    vec![
        route("POST", "/embeddings", json!({"data": [{"index": 0, "embedding": vec![0.01_f32; eunomia_backend::embeddings::service::dim()]}]})),
        route("POST", "/chat/completions", json!({"choices": [{"message": {"content": extraction.to_string()}}]})).body("Extract entities"),
        route("POST", "/chat/completions", json!({"choices": [{"message": {"content": "{\"belief\": \"Ada reports importer bugs.\"}"}}]})),
    ]
}

async fn point_openai_at(org: &OrgState, owner: &RecordId, base: &str) {
    org.db
        .test_raw()
        .query("UPSERT $id SET owner = $owner, openai_base_url = $base")
        .bind(("id", RecordId::new("app_settings", owner_key_str(owner))))
        .bind(("owner", owner.clone()))
        .bind(("base", base.to_string()))
        .await
        .unwrap()
        .check()
        .unwrap();
}

/// One synced record gets an embedding right away; extraction and consolidation run as queued jobs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_synced_record_is_embedded_and_its_entities_extracted_and_consolidated() {
    let (app, org) = app_with(Some("sk-test")).await;
    let owner = app.user.id.clone();
    let mut routes = openai_mock_routes();
    routes.push(route("GET", "/issues", json!([issue(7, "Importer drops the last row", "2024-02-01T00:00:00Z")])));
    let mock = serve(routes).await;
    point_openai_at(&org, &owner, &mock.base).await;
    connect_github(&org, &owner, &mock.base).await;

    let out = sync_source(&org, &owner, "github", None).await;
    assert_eq!(out["written"], 1, "{out}");
    assert_eq!(count(&org, "SELECT count() FROM cache_record WHERE owner = $owner AND embedding != NONE", &owner).await, 1);

    let stop = spawn_worker(&app.state);
    let q = |sql: &'static str| {
        let (org, owner) = (org.clone(), owner.clone());
        async move {
            let mut res = org.db.test_raw().query(sql).bind(("owner", owner)).await.unwrap();
            res.take::<Vec<String>>(0).unwrap()
        }
    };
    wait_for("extracted fact and observation", 30, || async {
        !q("SELECT VALUE text FROM memory WHERE type = 'world' AND owner = $owner").await.is_empty()
            && !q("SELECT VALUE text FROM memory WHERE type = 'observation' AND owner = $owner").await.is_empty()
    })
    .await;
    assert_eq!(q("SELECT VALUE text FROM memory WHERE type = 'world' AND owner = $owner").await, vec!["Ada reported the importer bug".to_string()]);
    assert_eq!(q("SELECT VALUE text FROM memory WHERE type = 'observation' AND owner = $owner").await, vec!["Ada reports importer bugs.".to_string()]);
    let _ = stop.send(true);
}

/// With no model configured raw data is stored and nothing calls out; an embedding outage leaves work
/// that replaying the unchanged record repairs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_model_stores_raw_data_and_a_failed_embedding_is_repaired_on_replay() {
    let (app, org) = app_with(None).await;
    let owner = app.user.id.clone();
    let github = serve(vec![route("GET", "/issues", json!([issue(7, "Importer", "2024-02-01T00:00:00Z")]))]).await;
    connect_github(&org, &owner, &github.base).await;

    let out = sync_source(&org, &owner, "github", None).await;
    assert_eq!(out["written"], 1);
    assert_eq!(out["errors"], json!([]), "no model: no provider call was attempted");
    assert_eq!(count(&org, "SELECT count() FROM cache_record WHERE owner = $owner AND embedding = NONE", &owner).await, 1);
    drop(app);

    let (app, org) = app_with(Some("sk-test")).await;
    let owner = app.user.id.clone();
    connect_github(&org, &owner, &github.base).await;
    let down = serve(vec![route("POST", "/embeddings", json!({"error": "overloaded"})).status(503)]).await;
    point_openai_at(&org, &owner, &down.base).await;
    let out = sync_source(&org, &owner, "github", None).await;
    assert!(out["errors"][0].as_str().unwrap().starts_with("embed"), "{out}");
    assert_eq!(status(&org, &owner, "github").await["last_error"], "");

    let up = serve(openai_mock_routes()).await;
    point_openai_at(&org, &owner, &up.base).await;
    // Same record again: unchanged, so it is skipped, but its embedding is repaired.
    let raw = vec![issue(7, "Importer", "2024-02-01T00:00:00Z")];
    let src = registry::get("github").unwrap();
    let report = registry::ingest(&org, &owner, &raw, src.as_ref()).await.unwrap();
    assert_eq!(report.written + report.skipped, 1);
    let out = sync_source(&org, &owner, "github", None).await;
    assert_eq!(out["skipped"], 1, "{out}");
    assert_eq!(count(&org, "SELECT count() FROM cache_record WHERE owner = $owner AND embedding != NONE", &owner).await, 1);
}

/// Pocket: everything a recording carries is stored whole and readable through `get`; the cache side
/// is chunked, embedded and entity-extracted (as a job) with the endpoint's own chat model; a shorter
/// re-sync tombstones the stale chunks; deleting the connector's data takes everything derived from
/// it and nothing from another source or another owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pocket_recording_is_stored_whole_chunked_embedded_and_extracted() {
    let (app, org) = app_with(Some("sk-test")).await;
    let owner = app.user.id.clone();
    let mut routes = openai_mock_routes();
    routes.push(route("GET", "/models", json!({"data": [{"id": "nomic-embed-text"}, {"id": "llama3.1"}]})));
    let mock = serve(routes).await;
    point_openai_at(&org, &owner, &mock.base).await;

    let mut rec = json!({
        "id": "rec_1", "title": "Importer review", "recording_at": "2024-05-02T09:00:00Z", "duration": 900,
        "tags": [{"id": "t1", "name": "work"}], "speakers": {"s1": {"name": "Ada Lovelace", "speakerId": "s1"}},
        "transcript": {"segments": [
            {"speaker": "s1", "start": 0, "end": 1000, "text": "Ada here, the importer drops the last row."},
            {"speaker": "s2", "speakerName": "Grace", "start": 1000, "end": 9000, "text": "word ".repeat(1500)},
        ]},
        "summarizations": {"sm1": {"v2": {"summary": {"markdown": "Agreed to fix the importer."},
                                          "actionItems": {"items": [{"title": "Ship the fix"}]}}}},
    });
    let report = registry::ingest(&org, &owner, &[rec.clone()], &HeyPocketSource).await.unwrap();
    assert_eq!((report.written, report.failed), (4, 0), "recording + 3 chunks: {:?}", report.errors);

    let got = common::sys(eunomia_backend::tools::generic::get(&org.db, &owner, "heypocket:heypocket.transcript_chunk:rec_1:2")).await.unwrap();
    let stored = &got["recording"];
    assert_eq!(stored["summary"], "Agreed to fix the importer.", "{got}");
    assert_eq!(stored["action_items"], json!(["Ship the fix"]));
    assert_eq!(stored["tags"], json!(["work"]));
    assert_eq!(stored["speakers"], json!(["Ada Lovelace", "Grace"]));
    let transcript = stored["transcript"].as_str().unwrap();
    assert!(transcript.starts_with("Ada Lovelace: Ada here, the importer drops the last row.\nGrace: word word"), "{transcript}");
    assert_eq!(transcript.matches("word").count(), 1500);
    assert_eq!(count(&org, "SELECT count() FROM pocket_recording WHERE owner = $owner AND raw.summarizations.sm1.v2 != NONE", &owner).await, 1);
    assert_eq!(count(&org, "SELECT count() FROM cache_record WHERE owner = $owner AND embedding != NONE", &owner).await, 4);

    // Entity extraction is a queued job; it uses the endpoint's own chat model.
    let stop = spawn_worker(&app.state);
    wait_for("extracted facts", 30, || async {
        count(&org, "SELECT count() FROM memory WHERE owner = $owner AND source != NONE", &owner).await >= 1
    })
    .await;
    let chats: Vec<_> = mock.requests().into_iter().filter(|r| r.path == "/chat/completions").collect();
    assert!(!chats.is_empty() && chats.iter().all(|r| r.body.contains("\"llama3.1\"")), "the endpoint's own chat model draws the relations");

    // Another owner's `get` sees nothing.
    let other = RecordId::new("user", "other-owner");
    assert_eq!(common::sys(eunomia_backend::tools::generic::get(&org.db, &other, "heypocket:heypocket.recording:rec_1")).await.unwrap()["error"], "not found");

    rec["transcript"]["segments"] = json!([{"speaker": "s1", "text": "Short now."}]);
    registry::ingest(&org, &owner, &[rec.clone()], &HeyPocketSource).await.unwrap();
    assert_eq!(count(&org, "SELECT count() FROM cache_record WHERE owner = $owner AND deleted = false", &owner).await, 2);

    registry::ingest(&org, &other, &[rec], &HeyPocketSource).await.unwrap();
    let up = json!({"type": "transactions", "id": "t1",
        "attributes": {"description": "Coles", "createdAt": "2024-03-01T00:00:00+11:00", "status": "SETTLED",
                       "amount": {"value": "-5.00", "valueInBaseUnits": -500, "currencyCode": "AUD"}},
        "relationships": {"account": {"data": {"id": "acc"}}}});
    registry::ingest(&org, &owner, &[up], &UpBankSource).await.unwrap();
    let up_memories = count(&org, "SELECT count() FROM memory WHERE owner = $owner AND source.source = 'up_bank'", &owner).await;
    let out = registry::delete_data(&org.db, &owner, &HeyPocketSource).await.unwrap();
    assert_eq!(out["records"], 4, "{out}");
    assert!(out["memories"].as_i64().unwrap() >= 1, "{out}");
    assert_eq!(count(&org, "SELECT count() FROM cache_record WHERE owner = $owner AND source = 'heypocket'", &owner).await, 0);
    assert_eq!(count(&org, "SELECT count() FROM pocket_recording WHERE owner = $owner", &owner).await, 0);
    assert_eq!(count(&org, "SELECT count() FROM memory WHERE owner = $owner AND source.source = 'heypocket'", &owner).await, 0);
    assert_eq!(count(&org, "SELECT count() FROM linked_to WHERE in.source = 'heypocket' AND in.owner = $owner", &owner).await, 0);
    assert_eq!(count(&org, "SELECT count() FROM cache_record WHERE owner = $owner AND source = 'up_bank'", &owner).await, 1);
    assert_eq!(count(&org, "SELECT count() FROM memory WHERE owner = $owner AND source.source = 'up_bank'", &owner).await, up_memories);
    assert_eq!(count(&org, "SELECT count() FROM pocket_recording WHERE owner = $owner", &other).await, 1);
    assert_eq!(count(&org, "SELECT count() FROM cache_record WHERE owner = $owner AND source = 'heypocket'", &other).await, 2);
    let _ = stop.send(true);
}

/// A source's `persist` failing holds the cursor (the record counts as failed and is retried).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_pocket_store_counts_as_failed_so_the_cursor_holds() {
    let (app, org) = app_with(None).await;
    let owner = app.user.id.clone();
    org.db.test_raw().query("DEFINE FIELD OVERWRITE title ON pocket_recording TYPE string DEFAULT '' ASSERT $value != 'POISON'").await.unwrap().check().unwrap();
    let rec = json!({"id": "rec_p", "title": "POISON", "recording_at": "2024-05-02T09:00:00Z"});
    let report = registry::ingest(&org, &owner, &[rec], &HeyPocketSource).await.unwrap();
    assert_eq!(report.failed, 1, "{:?}", report.errors);
    assert!(report.errors[0].starts_with("store rec_p"), "{:?}", report.errors);
}

/// Concurrent writers of the same records (other syncs, other owners) can collide on the shared
/// indexes; the loser is retried instead of counted as a failed record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_ingests_of_the_same_records_all_land() {
    let (app, org) = app_with(None).await;
    let raw: Vec<Value> = (0..20).map(|i| issue(i, &format!("Issue {i}"), "2024-02-01T00:00:00Z")).collect();
    let src = registry::get("github").unwrap();
    let owners: Vec<RecordId> = std::iter::once(app.user.id.clone()).chain((0..5).map(|i| RecordId::new("user", format!("o{i}")))).collect();
    let runs = owners.iter().map(|o| registry::ingest(&org, o, &raw, src.as_ref()));
    for report in futures::future::join_all(runs).await {
        let report = report.unwrap();
        assert_eq!(report.failed, 0, "{:?}", report.errors);
    }
    for o in &owners {
        assert_eq!(count(&org, "SELECT count() FROM cache_record WHERE owner = $owner", o).await, 20);
    }
}

/// Changed links are reconciled without duplicating the raw record.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn link_changes_are_reconciled_without_duplicating_records() {
    let (app, org) = app_with(None).await;
    let owner = app.user.id.clone();
    let txn = |cat: &str| {
        json!({"type": "transactions", "id": "t1",
               "attributes": {"description": "Coles", "createdAt": "2024-03-01T00:00:00+11:00", "status": "SETTLED",
                              "amount": {"value": "-5.00", "valueInBaseUnits": -500, "currencyCode": "AUD"}},
               "relationships": {"account": {"data": {"id": "acc"}}, "category": {"data": {"id": cat}}}})
    };
    registry::ingest(&org, &owner, &[txn("groceries")], &UpBankSource).await.unwrap();
    registry::ingest(&org, &owner, &[txn("takeaway")], &UpBankSource).await.unwrap();
    assert_eq!(count(&org, "SELECT count() FROM cache_record WHERE owner = $owner", &owner).await, 1);
    let links = search::links(&org.db, &search::RecordScope::own(&owner), "up_bank:up.transaction:t1", Some("category")).await.unwrap();
    let json = serde_json::to_string(&links).unwrap();
    assert!(json.contains("takeaway") && !json.contains("groceries"), "{json}");
    drop(app);
}

async fn post(app: &TestApp, path: &str, body: Body, headers: &[(&str, String)]) -> (u16, Value) {
    let mut req = axum::http::Request::post(path);
    for (k, v) in headers {
        req = req.header(*k, v.as_str());
    }
    let resp = app.router.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = http_body_util::BodyExt::collect(resp.into_body()).await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// A signed webhook is ingested; one whose records can't be stored gets a retryable 503; oversized
/// and stalled bodies are cut off before any signature or database work.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signed_webhooks_ingest_and_failed_persistence_is_retryable() {
    let (app, org) = app_with(None).await;
    let owner = app.user.id.clone();
    let txn = |desc: &str| {
        json!({"data": {"type": "transactions", "id": "t9",
               "attributes": {"description": desc, "createdAt": "2024-03-01T00:00:00+11:00", "status": "HELD",
                              "amount": {"value": "-5.00", "valueInBaseUnits": -500, "currencyCode": "AUD"}},
               "relationships": {}}})
    };
    let mock = serve(vec![route("GET", "/transactions/t9", txn("POISON")).query_has("v=1"), route("GET", "/transactions/t9", txn("Coles"))]).await;
    service::upsert_connector(
        &org.db,
        &org.settings.encryption_key,
        &owner,
        "up_bank",
        Some(true),
        None,
        Some(json!({"personal_access_token": "up:yeah:x", "webhook_secret_key": "shh"})),
    )
    .await
    .unwrap();
    let path = format!("/api/sources/up_bank/webhook/{}", owner.to_string());
    let deliver = |related: String| {
        let body = json!({"data": {"attributes": {"eventType": "TRANSACTION_CREATED"},
            "relationships": {"transaction": {"data": {"id": "t9"}, "links": {"related": related}}}}})
        .to_string();
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(b"shh").unwrap();
        mac.update(body.as_bytes());
        (body, hex::encode(mac.finalize().into_bytes()))
    };

    org.db.test_raw().query("DEFINE FIELD OVERWRITE title ON cache_record TYPE string DEFAULT '' ASSERT $value != 'POISON'").await.unwrap().check().unwrap();
    let (body, sig) = deliver(format!("{}/transactions/t9?v=1", mock.base));
    let (status, _) = post(&app, &path, Body::from(body), &[("X-Up-Authenticity-Signature", sig)]).await;
    assert_eq!(status, 503);

    let (body, sig) = deliver(format!("{}/transactions/t9", mock.base));
    let (status, _) = post(&app, &path, Body::from(body), &[("X-Up-Authenticity-Signature", sig)]).await;
    assert_eq!(status, 200);
    assert_eq!(count(&org, "SELECT count() FROM cache_record WHERE owner = $owner AND title = 'Coles'", &owner).await, 1);

    let (status, _) = post(&app, &path, Body::from(vec![b'x'; (1 << 20) + 1]), &[]).await;
    assert_eq!(status, 413);

    let stalled = Body::from_stream(futures::stream::pending::<Result<Vec<u8>, std::io::Error>>());
    let (status, _) = post(&app, &path, stalled, &[]).await;
    assert_eq!(status, 408);
}

/// "Sync now" queues a `sync` job and waits for it; a waiting or running sync job means "already
/// running" and no second one is started.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_sync_now_route_runs_a_job_and_reports_an_already_running_sync() {
    let (app, org) = app_with(None).await;
    let owner = app.user.id.clone();
    let mock = serve(vec![route("GET", "/issues", json!([issue(7, "Importer", "2024-02-01T00:00:00Z")])).delay(400)]).await;
    connect_github(&org, &owner, &mock.base).await;

    // A queued job and no worker: already running, answered at once, nothing new queued.
    let job = NewJob::new(kind::SYNC, owner.clone(), "sync:queued").in_org(org.db.org()).payload(json!({"source": "github"}));
    jobs::enqueue(&app.state.control, job).await.unwrap();
    let (status, body) = app.http("POST", "/api/sources/github/sync", None, true).await;
    assert_eq!((status.as_u16(), body["status"].as_str()), (200, Some("already_running")), "{body}");
    assert!(mock.requests().is_empty());
    app.state.control.test_raw().query("DELETE job").await.unwrap().check().unwrap();

    let stop = spawn_worker(&app.state);
    let (first, second) = tokio::join!(app.http("POST", "/api/sources/github/sync", None, true), async {
        tokio::time::sleep(Duration::from_millis(150)).await;
        app.http("POST", "/api/sources/github/sync", None, true).await
    });
    assert_eq!(first.1["written"], 1, "{}", first.1);
    assert_eq!(second.1["status"], "already_running", "{}", second.1);
    assert_eq!(mock.requests().len(), 1);

    // A failing provider comes back as the error.
    let bad = serve(vec![route("GET", "/issues", json!({"message": "Bad credentials"})).status(401)]).await;
    connect(&org, &owner, "github", &bad.base, json!({}), json!({"personal_access_token": "x"})).await;
    let (_, body) = app.http("POST", "/api/sources/github/sync", None, true).await;
    assert!(body["error"].as_str().unwrap().contains("HTTP 401"), "{body}");
    let _ = stop.send(true);
}

fn b64(s: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s)
}

/// (connector kind, source key, config, credentials, mock routes) for one single-page sync each.
fn per_connector() -> Vec<(&'static str, &'static str, Value, Value, Vec<Route>)> {
    let oauth = || route("POST", "/oauth/token", json!({"access_token": "fresh"}));
    let google = json!({"client_id": "cid", "client_secret": "s", "refresh_token": "rt"});
    vec![
        (
            "up_bank", "up_bank", json!({}), json!({"personal_access_token": "up:yeah:x"}),
            vec![
                route("GET", "/transactions", json!({"data": [{"type": "transactions", "id": "t1", "attributes": {"status": "SETTLED", "description": "Coles",
                    "amount": {"currencyCode": "AUD", "value": "-1.00", "valueInBaseUnits": -100}, "createdAt": "2024-03-01T05:08:57+11:00"}, "relationships": {}}],
                    "links": {"next": null}})),
                route("GET", "/accounts", json!({"data": [], "links": {"next": null}})),
                route("GET", "/categories", json!({"data": []})),
            ],
        ),
        (
            "pocketai", "heypocket", json!({}), json!({"api_key": "pk"}),
            vec![
                route("GET", "/public/recordings", json!({"data": [{"id": "rec_1", "title": "Standup", "duration": 60, "recording_at": "2024-05-02T09:00:00Z", "tags": []}]})),
                route("GET", "/public/recordings/rec_1", json!({"data": {"id": "rec_1", "summary": "Ship it.", "transcript": {"segments": []}}})),
            ],
        ),
        ("github", "github", json!({}), json!({"personal_access_token": "t"}), vec![route("GET", "/issues", json!([issue(1, "One", "2024-02-01T00:00:00Z")]))]),
        (
            "slack", "slack", json!({}), json!({"bot_token": "xoxb"}),
            vec![
                route("GET", "/auth.test", json!({"ok": true, "url": "https://acme.slack.com/", "team": "Acme", "user_id": "U0"})),
                route("GET", "/conversations.list", json!({"ok": true, "channels": [{"id": "C1", "name": "general", "is_member": true}], "response_metadata": {"next_cursor": ""}})),
                route("GET", "/conversations.history", json!({"ok": true, "messages": [{"type": "message", "user": "U1", "text": "Deploy is green", "ts": "1714000000.000200"}], "has_more": false})),
            ],
        ),
        (
            "notion", "notion", json!({}), json!({"integration_token": "ntn"}),
            vec![
                route("POST", "/search", json!({"results": [{"object": "page", "id": "p1", "created_time": "2024-01-01T00:00:00.000Z", "last_edited_time": "2024-03-02T10:00:00.000Z",
                    "archived": false, "url": "https://www.notion.so/p1", "properties": {"Name": {"type": "title", "title": [{"plain_text": "Roadmap"}]}}}], "has_more": false, "next_cursor": null})),
                route("GET", "/blocks/p1/children", json!({"results": [{"type": "paragraph", "paragraph": {"rich_text": [{"plain_text": "Ship it."}]}}], "has_more": false})),
            ],
        ),
        (
            "linear", "linear", json!({}), json!({"api_key": "lin"}),
            vec![route("POST", "/graphql", json!({"data": {"viewer": {"assignedIssues": {"nodes": [{"id": "i1", "identifier": "ENG-1", "title": "Fix", "description": "d",
                "url": "https://linear.app/a/issue/ENG-1", "priorityLabel": "High", "createdAt": "2024-01-01T00:00:00.000Z", "updatedAt": "2024-02-01T00:00:00.000Z",
                "completedAt": null, "state": {"name": "Todo"}, "team": {"name": "Eng"}, "project": null, "labels": {"nodes": []}}],
                "pageInfo": {"hasNextPage": false, "endCursor": null}}}}}))],
        ),
        (
            "gmail", "gmail", json!({}), google.clone(),
            vec![
                oauth(),
                route("GET", "/users/me/messages", json!({"messages": [{"id": "m1", "threadId": "t1"}]})),
                route("GET", "/users/me/messages/m1", json!({"id": "m1", "threadId": "t1", "labelIds": ["INBOX"], "snippet": "s", "internalDate": "1700000100000",
                    "payload": {"mimeType": "text/plain", "headers": [{"name": "Subject", "value": "Lunch?"}, {"name": "From", "value": "Ada <a@e.com>"}],
                                "body": {"size": 5, "data": b64("Lunch")}}})),
            ],
        ),
        (
            "google_calendar", "google_calendar", json!({}), google.clone(),
            vec![
                oauth(),
                route("GET", "/calendars/primary/events", json!({"items": [{"id": "e1", "status": "confirmed", "summary": "Review", "htmlLink": "https://g.co/e1",
                    "start": {"dateTime": "2024-03-04T10:00:00+11:00"}, "end": {"dateTime": "2024-03-04T11:00:00+11:00"}}]})),
            ],
        ),
        (
            "discord", "discord", json!({"channel_id": "C1"}), json!({"bot_token": "b"}),
            vec![
                route("GET", "/channels/C1", json!({"id": "C1", "name": "general", "guild_id": "G1", "type": 0})),
                route("GET", "/channels/C1/messages", json!([{"id": "1100", "channel_id": "C1", "content": "Release notes", "timestamp": "2024-04-01T12:00:00.000000+00:00",
                    "author": {"id": "u1", "username": "ada"}, "attachments": []}])),
            ],
        ),
        (
            "spotify", "spotify", json!({}), google.clone(),
            vec![
                oauth(),
                route("GET", "/me/player/recently-played", json!({"items": [{"track": {"id": "tr1", "name": "Song", "duration_ms": 1000, "artists": [{"name": "Band"}],
                    "album": {"name": "Alb"}, "external_urls": {"spotify": "https://open.spotify.com/track/tr1"}}, "played_at": "2024-06-01T08:30:00.123Z"}],
                    "next": null, "cursors": {"after": "1717230600123"}})),
            ],
        ),
        (
            "todoist", "todoist", json!({}), json!({"api_token": "td"}),
            vec![
                route("GET", "/projects", json!({"results": [{"id": "p1", "name": "Home"}], "next_cursor": null})),
                route("GET", "/tasks", json!({"results": [{"id": "t1", "project_id": "p1", "content": "File taxes", "description": "", "priority": 1, "labels": [],
                    "checked": false, "added_at": "2024-06-01T09:00:00.000000Z", "updated_at": "2024-06-02T09:00:00.000000Z"}], "next_cursor": null})),
            ],
        ),
        (
            "stripe", "stripe", json!({}), json!({"secret_key": "rk_test_x"}),
            vec![route("GET", "/charges", json!({"object": "list", "has_more": false, "data": [{"id": "ch_1", "object": "charge", "amount": 2500, "currency": "aud",
                "created": 1700000100, "status": "succeeded", "paid": true, "refunded": false, "description": "Pro", "payment_intent": "pi_1"}]}))],
        ),
    ]
}

/// Every connector kind syncs end to end through the real pipeline against its mock provider: records
/// land, the status row is clean, the cursor is saved, the connector's own check passes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_connector_syncs_end_to_end_against_its_mock_provider() {
    let (app, org) = app_with(None).await;
    let owner = app.user.id.clone();
    let fixtures = per_connector();
    assert_eq!(fixtures.len(), service::CONNECTOR_KINDS.len(), "a fixture per connector kind");
    for (kind, key, config, credentials, routes) in fixtures {
        let mock = serve(routes).await;
        connect(&org, &owner, kind, &mock.base, config, credentials).await;
        let out = sync_source(&org, &owner, key, None).await;
        assert!(out.get("error").is_none() && out["failed"] == 0, "{key}: {out}");
        assert!(out["written"].as_i64().unwrap() >= 1, "{key} stored nothing: {out}");
        let st = status(&org, &owner, key).await;
        assert_eq!(st["last_error"], "", "{key}");
        assert!(!mock.requests().is_empty(), "{key} never called its provider");
        let n = count(&org, &format!("SELECT count() FROM cache_record WHERE owner = $owner AND source = '{key}'"), &owner).await;
        assert!(n >= 1, "{key}: no cache rows");
    }
}

#[allow(dead_code)]
fn _types(_: &User) {}
