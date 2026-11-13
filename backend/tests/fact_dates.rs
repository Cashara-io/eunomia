//! Facts carry the date they happened (audit T1: findings F1, F3, F15), against a real in-memory
//! SurrealDB through the public paths: extraction from a synced record, `memory_write` with an explicit
//! `occurred_at`, the supersede check, and recall's date range.
//!
//! The model and embeddings are a local stub: every text gets the same vector (so every pair of facts is a
//! candidate) and the supersede "model" answers a fixed list of `{fact, by}` pairs, including the wrong
//! ones, so what gets marked is decided by the code's own ordering rules, not by the stub.

mod common;

use std::sync::{Arc, Mutex};

use common::TestApp;
use eunomia_backend::entities::{extract, supersede};
use serde_json::{json, Value};
use surrealdb::types::RecordId;

/// What the stub's chat endpoint answers: the supersede pairs, and the extraction JSON.
#[derive(Clone, Default)]
struct Answers {
    pairs: Arc<Mutex<Value>>,
    extraction: Arc<Mutex<Value>>,
}

/// An OpenAI-compatible `/v1/embeddings` (one constant vector per input) and `/v1/chat/completions`.
async fn stub(answers: Answers) -> String {
    use axum::{extract::State, routing::post, Json, Router};
    async fn embed(Json(body): Json<Value>) -> Json<Value> {
        let n = match &body["input"] {
            Value::Array(a) => a.len(),
            _ => 1,
        };
        let v = vec![0.01_f32; eunomia_backend::embeddings::service::dim()];
        Json(json!({ "data": (0..n).map(|i| json!({"index": i, "embedding": v})).collect::<Vec<_>>() }))
    }
    async fn chat(State(a): State<Answers>, Json(body): Json<Value>) -> Json<Value> {
        let prompt = body["messages"][0]["content"].as_str().unwrap_or_default();
        let content = if prompt.contains("\"superseded\"") {
            json!({ "superseded": a.pairs.lock().unwrap().clone() })
        } else {
            a.extraction.lock().unwrap().clone()
        };
        Json(json!({"choices": [{"message": {"content": content.to_string()}}]}))
    }
    let router = Router::new().route("/v1/embeddings", post(embed)).route("/v1/chat/completions", post(chat)).with_state(answers);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}/v1")
}

async fn app(answers: Answers) -> TestApp {
    let mut settings = common::test_settings();
    settings.openai_api_key = Some("sk-test".into());
    settings.openai_base_url = stub(answers).await;
    settings.openai_chat_model = "test-model".into();
    TestApp::with_settings(settings).await
}

fn rid(v: &Value) -> RecordId {
    eunomia_backend::rid::parse(v.as_str().unwrap_or_else(|| panic!("an id: {v}"))).unwrap()
}

async fn write(app: &TestApp, subject: &str, text: &str, occurred_at: Option<&str>) -> (RecordId, RecordId) {
    let mut args = json!({"subject_name": subject, "subject_kind": "person", "text": text});
    if let Some(at) = occurred_at {
        args["occurred_at"] = json!(at);
    }
    let out = app.tool("memory_write", args).await;
    assert!(out.get("error").is_none(), "{out}");
    (rid(&out["entity"]["id"]), rid(&out["memory"]["id"]))
}

/// `(status, occurred_at)` of one memory, read straight from the database.
async fn row(app: &TestApp, id: &RecordId) -> (Option<String>, Option<String>) {
    let mut res = app.db().await.test_raw().query("SELECT status, occurred_at AS at FROM ONLY $id").bind(("id", id.clone())).await.unwrap();
    let v: Option<Value> = res.take(0).unwrap();
    let v = v.unwrap_or_else(|| panic!("{id:?} is gone"));
    (v["status"].as_str().map(String::from), v["at"].as_str().map(String::from))
}

async fn superseded(app: &TestApp, id: &RecordId) -> bool {
    row(app, id).await.0.as_deref() == Some("superseded")
}

async fn check(app: &TestApp, subject: &RecordId, new: &[RecordId]) -> Vec<RecordId> {
    common::sys(supersede::check(&app.db().await, &app.state.settings, &app.user.id, subject, new)).await
}

/// The stub model proposes every pair in both directions; only the code decides which may stand.
fn both_ways() -> Value {
    json!([{"fact": 1, "by": 2}, {"fact": 2, "by": 1}])
}

// ---- F1: facts extracted from a synced record carry the record's date -------------------------

#[tokio::test]
async fn an_extracted_fact_is_dated_by_its_record_and_found_by_that_dates_range() {
    let answers = Answers::default();
    *answers.extraction.lock().unwrap() = json!({"people": [{"name": "Ada Lovelace", "facts": ["Ada agreed to lead the Acme pilot"]}]});
    let app = app(answers.clone()).await;
    let db = app.db().await;
    // as the extract job builds it from a cache record (jobs/handlers.rs extract_record)
    let record = |id: &str, occurred_at: Option<&str>| extract::ExtractRecord {
        id: id.into(),
        title: "Pilot kickoff".into(),
        body_text: "Ada Lovelace agreed to lead the Acme pilot starting next month, with weekly check-ins.".into(),
        occurred_at: occurred_at.map(String::from),
        vault: None,
        source: "notion".into(),
        payload: Value::Null,
        owner_name: None,
    };
    common::sys(extract::extract_entities(&db, &app.state.settings, &app.user.id, &record("notion:page:1", Some("2023-04-01T09:30:00+00:00"))))
        .await
        .unwrap();
    let rows: Vec<Value> = db
        .test_raw()
        .query("SELECT occurred_at AS at, created_at FROM memory WHERE text = 'Ada agreed to lead the Acme pilot'")
        .await
        .unwrap()
        .take(0)
        .unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0]["at"].as_str().is_some_and(|a| a.starts_with("2023-04-01T09:30:00")), "dated by the record, not the job: {rows:?}");

    // date-range recall finds it in the record's year, through the date arm, and not in this year's
    let hits = app.tool("recall", json!({"query": "Ada Acme pilot", "time_range": ["2023-01-01", "2023-12-31T23:59:59Z"]})).await;
    let hit = hits["results"].as_array().unwrap().iter().find(|h| h["text"] == "Ada agreed to lead the Acme pilot").cloned();
    let hit = hit.unwrap_or_else(|| panic!("{hits}"));
    assert!(hit["occurred_at"].as_str().unwrap().starts_with("2023-04-01"), "{hit}");
    assert!(hit["arms_hit"].as_u64().unwrap() >= 2, "the date arm counts it in 2023: {hit}");

    // a record with no date keeps today's behaviour: the fact has no occurred_at, and recall dates it by its write
    *answers.extraction.lock().unwrap() = json!({"people": [{"name": "Ada Lovelace", "facts": ["Ada prefers written updates"]}]});
    common::sys(extract::extract_entities(&db, &app.state.settings, &app.user.id, &record("notion:page:2", None))).await.unwrap();
    let rows: Vec<Value> = db.test_raw().query("SELECT occurred_at FROM memory WHERE text = 'Ada prefers written updates'").await.unwrap().take(0).unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].get("occurred_at").is_none_or(Value::is_null), "{rows:?}");
}

// ---- F15: the date is written with the fact -------------------------------------------------

/// A reader polling while facts with an explicit date are written never sees one of them undated.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dated_write_is_never_seen_without_its_date() {
    let app = Arc::new(app(Answers::default()).await);
    let db = app.db().await;
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = {
        let (db, stop) = (db.clone(), stop.clone());
        tokio::spawn(async move {
            let mut undated = 0usize;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let n: Option<i64> = db
                    .test_raw()
                    .query("SELECT VALUE count() FROM memory WHERE string::starts_with(text, 'Dated fact') AND occurred_at = NONE GROUP ALL")
                    .await
                    .unwrap()
                    .take::<Vec<i64>>(0)
                    .unwrap()
                    .into_iter()
                    .next();
                undated += n.unwrap_or(0) as usize;
                tokio::task::yield_now().await;
            }
            undated
        })
    };
    for i in 0..40 {
        write(&app, "Grace Hopper", &format!("Dated fact {i}"), Some("2019-06-01")).await;
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(reader.await.unwrap(), 0, "a reader saw a dated fact before its date was written");
    let n: Vec<i64> =
        db.test_raw().query("SELECT VALUE count() FROM memory WHERE string::starts_with(text, 'Dated fact') AND occurred_at != NONE GROUP ALL").await.unwrap().take(0).unwrap();
    assert_eq!(n, vec![40]);
}

// ---- F3: supersession goes by when facts happened, not when they were written ----------------

#[tokio::test]
async fn an_old_fact_imported_today_never_replaces_the_current_one() {
    let answers = Answers::default();
    *answers.pairs.lock().unwrap() = both_ways();
    let app = app(answers).await;
    let (ada, current) = write(&app, "Ada Lovelace", "Ada is Head of Platform", Some("2026-09-01")).await;
    let (_, imported) = write(&app, "Ada Lovelace", "Ada's title is Staff Engineer", Some("2021-05-01")).await;
    check(&app, &ada, std::slice::from_ref(&imported)).await;
    assert!(!superseded(&app, &current).await, "the 2021 title must not replace the 2026 one");
    // the import is the one out of date: the later-dated live fact supersedes it
    assert!(superseded(&app, &imported).await);
}

#[tokio::test]
async fn an_old_fact_imported_today_never_replaces_an_undated_current_one() {
    let answers = Answers::default();
    *answers.pairs.lock().unwrap() = both_ways();
    let app = app(answers).await;
    // undated: placed at when it was written (now)
    let (ada, current) = write(&app, "Ada Lovelace", "Ada is Head of Platform", None).await;
    let (_, imported) = write(&app, "Ada Lovelace", "Ada's title is Staff Engineer", Some("2021-05-01")).await;
    check(&app, &ada, std::slice::from_ref(&imported)).await;
    assert!(!superseded(&app, &current).await);
    assert!(superseded(&app, &imported).await);
}

#[tokio::test]
async fn an_undated_new_fact_replaces_an_older_dated_one() {
    let answers = Answers::default();
    *answers.pairs.lock().unwrap() = both_ways();
    let app = app(answers).await;
    let (ada, old) = write(&app, "Ada Lovelace", "Ada's title is Staff Engineer", Some("2021-05-01")).await;
    let (_, new) = write(&app, "Ada Lovelace", "Ada is Head of Platform", None).await;
    check(&app, &ada, std::slice::from_ref(&new)).await;
    assert!(superseded(&app, &old).await);
    assert!(!superseded(&app, &new).await);
}

/// Two facts about the same moment: the later write is the correction, never the other way round.
#[tokio::test]
async fn a_correction_about_the_same_moment_replaces_the_first_write() {
    let answers = Answers::default();
    *answers.pairs.lock().unwrap() = both_ways();
    let app = app(answers).await;
    let (ada, first) = write(&app, "Ada Lovelace", "Ada's salary from 2025-01-01 is 120k", Some("2025-01-01")).await;
    let (_, correction) = write(&app, "Ada Lovelace", "Ada's salary from 2025-01-01 is 125k", Some("2025-01-01")).await;
    check(&app, &ada, std::slice::from_ref(&correction)).await;
    assert!(superseded(&app, &first).await);
    assert!(!superseded(&app, &correction).await);
}

/// A history arriving newest first ends as one arriving oldest first: the later-dated fact stands.
#[tokio::test]
async fn a_history_backfilled_in_any_order_keeps_the_latest_dated_fact() {
    let answers = Answers::default();
    *answers.pairs.lock().unwrap() = both_ways();
    let app = app(answers).await;
    let history = [("Ada is a junior engineer", "2023-04-01"), ("Ada is a senior engineer", "2025-06-01"), ("Ada is a staff engineer", "2026-02-01")];
    for (subject, order) in [("Ada Inorder", [0, 1, 2]), ("Ada Reversed", [2, 1, 0]), ("Ada Shuffled", [1, 2, 0])] {
        let mut ids = [None, None, None];
        for i in order {
            let (s, m) = write(&app, subject, history[i].0, Some(history[i].1)).await;
            check(&app, &s, std::slice::from_ref(&m)).await;
            ids[i] = Some(m);
        }
        let mut live = Vec::new();
        for (i, id) in ids.iter().enumerate() {
            if !superseded(&app, id.as_ref().unwrap()).await {
                live.push(history[i].0);
            }
        }
        assert_eq!(live, ["Ada is a staff engineer"], "{subject}: only the latest-dated title stays live");
    }
}
