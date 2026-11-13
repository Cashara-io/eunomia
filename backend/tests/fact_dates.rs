//! Facts carry two dates (audit T1: findings F1, F3, F15; docs/concepts.md "A fact's two dates"),
//! against a real in-memory SurrealDB through the public paths: extraction from a synced record,
//! `memory_write` with an explicit `occurred_at`, `memory_update`, the supersede check, and recall's
//! date range. When a fact happened (`occurred_at`) is for recall and display; when we were last told it
//! (`asserted_at`) decides which fact is current.
//!
//! The model and embeddings are a local stub: every text gets the same vector (so every pair of facts is a
//! candidate) and the supersede "model" answers a fixed list of `{fact, by}` pairs, including the wrong
//! ones, so what gets marked is decided by the code's own ordering rules, not by the stub.

mod common;

use std::sync::{Arc, Mutex};

use common::TestApp;
use eunomia_backend::rid::RecordIdExt;
use eunomia_backend::entities::{extract, supersede};
use serde_json::{json, Value};
use surrealdb::types::RecordId;

/// What the stub's chat endpoint answers: the supersede pairs, and the extraction JSON.
#[derive(Clone, Default)]
struct Answers {
    pairs: Arc<Mutex<Value>>,
    extraction: Arc<Mutex<Value>>,
    /// Every supersede prompt the stub was sent.
    prompts: Arc<Mutex<Vec<String>>>,
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
            a.prompts.lock().unwrap().push(prompt.to_string());
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
    write_kind(app, subject, "person", text, occurred_at).await
}

async fn write_kind(app: &TestApp, subject: &str, kind: &str, text: &str, occurred_at: Option<&str>) -> (RecordId, RecordId) {
    let mut args = json!({"subject_name": subject, "subject_kind": kind, "text": text});
    if let Some(at) = occurred_at {
        args["occurred_at"] = json!(at);
    }
    let out = app.tool("memory_write", args).await;
    assert!(out.get("error").is_none(), "{out}");
    (rid(&out["entity"]["id"]), rid(&out["memory"]["id"]))
}

/// A fact extracted from a synced record dated `date` (`section`: "people" or "organisations"), as the
/// extract job writes it. Returns `(subject, memory)`.
async fn extract_fact(app: &TestApp, answers: &Answers, section: &str, subject: &str, text: &str, date: &str) -> (RecordId, RecordId) {
    // one record per (text, date): the same pair is the same record synced again
    let key = format!("{text}|{date}").bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    extract_record(app, answers, &format!("notes:{key:x}"), date, section, subject, &[text]).await;
    fact_by_text(app, text).await
}

/// Runs extraction of one synced record (`id`, dated `date`) whose model answer gives `facts` about
/// `subject`, as the extract job does on every sync of that record (or a retry of its job).
#[allow(clippy::too_many_arguments)]
async fn extract_record(app: &TestApp, answers: &Answers, id: &str, date: &str, section: &str, subject: &str, facts: &[&str]) {
    *answers.extraction.lock().unwrap() = json!({ section: [{"name": subject, "facts": facts}] });
    let record = extract::ExtractRecord {
        id: id.to_string(),
        title: "Note".into(),
        body_text: format!("{}. This note is long enough to be worth extracting facts from.", facts.join(". ")),
        occurred_at: Some(date.to_string()),
        vault: None,
        source: "notes".into(),
        payload: Value::Null,
        owner_name: None,
    };
    common::sys(extract::extract_entities(&app.db().await, &app.state.settings, &app.user.id, &record)).await.unwrap();
}

/// `(subject, memory)` of the one fact with this text (fails if there are none or several).
async fn fact_by_text(app: &TestApp, text: &str) -> (RecordId, RecordId) {
    let rows: Vec<Value> = app
        .db()
        .await
        .test_raw()
        .query("SELECT <string> id AS id, <string> subject AS subject FROM memory WHERE text = $t")
        .bind(("t", text.to_string()))
        .await
        .unwrap()
        .take(0)
        .unwrap();
    assert_eq!(rows.len(), 1, "{text}: {rows:?}");
    (rid(&rows[0]["subject"]), rid(&rows[0]["id"]))
}

/// The keys of the supersede jobs queued so far.
async fn supersede_jobs(app: &TestApp) -> Vec<String> {
    app.control().test_raw().query("SELECT VALUE idempotency_key FROM job WHERE kind = 'supersede'").await.unwrap().take(0).unwrap()
}

/// Rows with this text.
async fn count_text(app: &TestApp, text: &str) -> usize {
    let rows: Vec<Value> = app.db().await.test_raw().query("SELECT id FROM memory WHERE text = $t").bind(("t", text.to_string())).await.unwrap().take(0).unwrap();
    rows.len()
}

/// A synced record written straight to the cache, for a fact planted in its pre-0046 shape.
async fn plant_record(app: &TestApp, key: &str, date: &str) -> String {
    let rid = format!("cache_record:`{}:{key}`", eunomia_backend::rid::key_string(app.user.id.key()).unwrap());
    app.db()
        .await
        .test_raw()
        .query(format!(
            "CREATE {rid} SET owner = {}, source = 'gmail', type = 'gmail.message', external_id = '{key}', title = 't', body_text = 'b', \
             occurred_at = d'{date}', url = '', payload = {{}}, content_hash = 'fx', ingested_at = time::now(), updated_at = time::now(), deleted = false",
            app.user.id.to_string()
        ))
        .await
        .unwrap()
        .check()
        .unwrap();
    rid
}

/// A memory's dates and replacement link, as strings.
async fn dates(app: &TestApp, id: &RecordId) -> Value {
    let mut res = app
        .db()
        .await
        .test_raw()
        .query("SELECT <string> occurred_at AS occurred, <string> asserted_at AS asserted, <string> superseded_by AS by FROM ONLY $id")
        .bind(("id", id.clone()))
        .await
        .unwrap();
    res.take::<Option<Value>>(0).unwrap().unwrap()
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

    // recall for the record's year returns it, dated by the record (whichever arm finds it)
    let hits = app.tool("recall", json!({"query": "Ada Acme pilot", "time_range": ["2023-01-01", "2023-12-31T23:59:59Z"]})).await;
    let hit = hits["results"].as_array().unwrap().iter().find(|h| h["text"] == "Ada agreed to lead the Acme pilot").cloned();
    let hit = hit.unwrap_or_else(|| panic!("the fact is returned for its record's year: {hits}"));
    assert!(hit["occurred_at"].as_str().unwrap().starts_with("2023-04-01"), "{hit}");

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

// ---- F3: replacement goes by when we were told a fact, never by when it happened ------------

/// (1) An old email imported today does not replace a newer fact: it is told at the email's date.
#[tokio::test]
async fn an_old_email_imported_today_does_not_replace_a_newer_fact() {
    let answers = Answers::default();
    *answers.pairs.lock().unwrap() = both_ways();
    let app = app(answers.clone()).await;
    let (ada, current) = write(&app, "Ada Lovelace", "Ada is Head of Platform", None).await;
    let (_, imported) = extract_fact(&app, &answers, "people", "Ada Lovelace", "Ada's title is Staff Engineer", "2021-05-01T09:00:00+00:00").await;
    assert!(dates(&app, &imported).await["asserted"].as_str().unwrap().starts_with("2021-05-01"), "told at the email's date");
    check(&app, &ada, std::slice::from_ref(&imported)).await;
    assert!(!superseded(&app, &current).await, "the 2021 email must not replace the current fact");
    assert!(superseded(&app, &imported).await, "the import is the one out of date");
    assert_eq!(dates(&app, &imported).await["by"], json!(current.to_string()), "the replacing fact is recorded");
}

/// (2) A correction written today wins even when it says "since March" and the error was dated June.
#[tokio::test]
async fn a_correction_about_an_earlier_moment_beats_the_error_it_corrects() {
    let answers = Answers::default();
    *answers.pairs.lock().unwrap() = both_ways();
    let app = app(answers.clone()).await;
    let (ada, error) = extract_fact(&app, &answers, "people", "Ada Lovelace", "Ada started on 3 March", "2025-06-03T08:00:00+00:00").await;
    let (_, correction) = write(&app, "Ada Lovelace", "Ada started on 10 March 2025", Some("2025-03-10")).await;
    check(&app, &ada, std::slice::from_ref(&correction)).await;
    assert!(superseded(&app, &error).await, "the error is replaced");
    assert!(!superseded(&app, &correction).await, "the correction stands, though it is about March");
    assert_eq!(dates(&app, &error).await["by"], json!(correction.to_string()));
    // the model sees when each fact happened, to tell history from a correction
    let prompt = answers.prompts.lock().unwrap().last().cloned().unwrap();
    assert!(prompt.contains("Ada started on 10 March 2025 (happened 2025-03-10)"), "{prompt}");
    assert!(prompt.contains("Ada started on 3 March (happened 2025-06-03)"), "{prompt}");
}

/// (3) A fact from next month's calendar event does not block or beat a correction made today.
#[tokio::test]
async fn a_future_dated_fact_does_not_beat_a_correction_made_today() {
    let answers = Answers::default();
    *answers.pairs.lock().unwrap() = both_ways();
    let app = app(answers.clone()).await;
    let next_month = (chrono::Utc::now() + chrono::Duration::days(40)).to_rfc3339();
    let (acme, planned) = extract_fact(&app, &answers, "organisations", "Acme Corp", "Ada owns the Acme account", &next_month).await;
    let d = dates(&app, &planned).await;
    assert!(d["occurred"].as_str().unwrap() > chrono::Utc::now().to_rfc3339().as_str(), "still dated next month for display: {d}");
    assert!(d["asserted"].as_str().unwrap() <= chrono::Utc::now().to_rfc3339().as_str(), "told when it was read, not next month: {d}");
    let (_, correction) = write_kind(&app, "Acme Corp", "organisation", "The handover is cancelled: Bob keeps the Acme account", None).await;
    check(&app, &acme, std::slice::from_ref(&correction)).await;
    assert!(!superseded(&app, &correction).await, "today's correction is not replaced by next month's plan");
    assert!(superseded(&app, &planned).await, "the correction replaces the plan");
    let prompt = answers.prompts.lock().unwrap().last().cloned().unwrap();
    assert!(prompt.contains("Ada owns the Acme account (planned for "), "a plan is not worded as done: {prompt}");
    // the organiser adds an attendee: the event is re-synced and re-extracted with the same statement. It
    // is the same telling: no second row, the plan stays replaced and is not told again now
    let before = dates(&app, &planned).await;
    let (_, again) = extract_fact(&app, &answers, "organisations", "Acme Corp", "Ada owns the Acme account", &next_month).await;
    assert_eq!(again, planned, "the same row, not a new live one");
    assert_eq!(dates(&app, &planned).await, before);
    assert!(superseded(&app, &planned).await && !superseded(&app, &correction).await);
}

/// (4) A fact edited in 2026 is not replaced by a 2024 email imported later: an edit is being told again,
/// and it queues its own supersede check.
#[tokio::test]
async fn an_edited_fact_is_not_replaced_by_an_older_email_imported_later() {
    let answers = Answers::default();
    *answers.pairs.lock().unwrap() = both_ways();
    let app = app(answers.clone()).await;
    let (_, cto) = write(&app, "Ada Lovelace", "Ada is the CTO", None).await;
    // as if written in 2023 (told then)
    app.db()
        .await
        .test_raw()
        .query("UPDATE $id SET created_at = d'2023-01-10T00:00:00Z', updated_at = d'2023-01-10T00:00:00Z', asserted_at = d'2023-01-10T00:00:00Z'")
        .bind(("id", cto.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();
    let edited = app.tool("memory_update", json!({"memory_id": cto.to_string(), "text": "Ada is the CTO of Acme"})).await;
    assert!(edited.get("error").is_none(), "{edited}");
    let asserted = dates(&app, &cto).await["asserted"].as_str().unwrap().to_string();
    assert!(asserted.starts_with("20") && asserted.as_str() > "2026-01-01", "an edit is being told again: {asserted}");
    let keys: Vec<String> = app
        .control()
        .test_raw()
        .query("SELECT VALUE idempotency_key FROM job WHERE kind = 'supersede'")
        .await
        .unwrap()
        .take(0)
        .unwrap();
    let cto_key = cto.to_string();
    assert!(keys.iter().any(|k| k.contains(cto_key.as_str()) && k.ends_with(":v2")), "the edit queues its own check: {keys:?}");

    let (ada, vp) = extract_fact(&app, &answers, "people", "Ada Lovelace", "Ada is VP of Engineering", "2024-05-01T09:00:00+00:00").await;
    check(&app, &ada, std::slice::from_ref(&vp)).await;
    assert!(!superseded(&app, &cto).await, "the 2026 edit stands");
    assert!(superseded(&app, &vp).await);
}

/// Two facts told at the same moment: the later write is the correction, never the other way round.
#[tokio::test]
async fn a_correction_written_later_replaces_the_first_write() {
    let answers = Answers::default();
    *answers.pairs.lock().unwrap() = both_ways();
    let app = app(answers).await;
    let (ada, first) = write(&app, "Ada Lovelace", "Ada's salary from 2025-01-01 is 120k", Some("2025-01-01")).await;
    let (_, correction) = write(&app, "Ada Lovelace", "Ada's salary from 2025-01-01 is 125k", Some("2025-01-01")).await;
    check(&app, &ada, std::slice::from_ref(&correction)).await;
    assert!(superseded(&app, &first).await);
    assert!(!superseded(&app, &correction).await);
}

/// A history read newest first ends as one read oldest first: the latest-told fact stands.
#[tokio::test]
async fn a_history_extracted_in_any_order_keeps_the_latest_fact() {
    let answers = Answers::default();
    *answers.pairs.lock().unwrap() = both_ways();
    let app = app(answers.clone()).await;
    let history = [
        ("is a junior engineer", "2023-04-01T09:00:00+00:00"),
        ("is a senior engineer", "2025-06-01T09:00:00+00:00"),
        ("is a staff engineer", "2026-02-01T09:00:00+00:00"),
    ];
    for (name, order) in [("Ada Inorder", [0, 1, 2]), ("Ada Reversed", [2, 1, 0]), ("Ada Shuffled", [1, 2, 0])] {
        let mut ids = [None, None, None];
        for i in order {
            let text = format!("{name} {}", history[i].0);
            let (s, m) = extract_fact(&app, &answers, "people", name, &text, history[i].1).await;
            check(&app, &s, std::slice::from_ref(&m)).await;
            ids[i] = Some(m);
        }
        let mut live = Vec::new();
        for (i, id) in ids.iter().enumerate() {
            if !superseded(&app, id.as_ref().unwrap()).await {
                live.push(history[i].0);
            }
        }
        assert_eq!(live, ["is a staff engineer"], "{name}: only the latest title stays live");
    }
}

// ---- re-syncs: the same claim from the same record is the same telling -----------------------

#[tokio::test]
async fn a_resynced_record_does_not_tell_its_facts_again() {
    let answers = Answers::default();
    let app = app(answers.clone()).await;
    extract_record(&app, &answers, "notes:wiki", "2025-02-01T09:00:00+00:00", "people", "Ada Lovelace", &["Ada leads the platform team"]).await;
    let (_, lead) = fact_by_text(&app, "Ada leads the platform team").await;
    let told = dates(&app, &lead).await;
    // unchanged re-sync: nothing new, the fact keeps its row and when it was told
    extract_record(&app, &answers, "notes:wiki", "2025-02-01T09:00:00+00:00", "people", "Ada Lovelace", &["Ada leads the platform team"]).await;
    assert_eq!(count_text(&app, "Ada leads the platform team").await, 1);
    assert_eq!(dates(&app, &lead).await, told);
    // the record is edited later and a statement is added (and the old one restated with other spacing and
    // a full stop): only the new statement is new, told at the record's new date
    extract_record(&app, &answers, "notes:wiki", "2025-06-01T09:00:00+00:00", "people", "Ada Lovelace", &["ada leads the  platform team.", "Ada hired two engineers"])
        .await;
    assert_eq!(count_text(&app, "Ada leads the platform team").await + count_text(&app, "ada leads the  platform team.").await, 1);
    assert_eq!(dates(&app, &lead).await, told, "the unchanged statement is not told again");
    let (_, hired) = fact_by_text(&app, "Ada hired two engineers").await;
    assert!(dates(&app, &hired).await["asserted"].as_str().unwrap().starts_with("2025-06-01"));
    // the same statement from another record is another telling
    extract_record(&app, &answers, "notes:other", "2025-07-01T09:00:00+00:00", "people", "Ada Lovelace", &["Ada leads the platform team"]).await;
    assert_eq!(count_text(&app, "Ada leads the platform team").await, 2);
}

/// A job that wrote some facts and failed is retried: it writes what was missing and duplicates nothing.
#[tokio::test]
async fn a_retried_extraction_job_does_not_duplicate_facts() {
    let answers = Answers::default();
    let app = app(answers.clone()).await;
    // the first attempt got as far as one fact
    extract_record(&app, &answers, "notes:mail", "2025-03-01T09:00:00+00:00", "people", "Grace Hopper", &["Grace runs the compiler group"]).await;
    // the retry writes both
    extract_record(&app, &answers, "notes:mail", "2025-03-01T09:00:00+00:00", "people", "Grace Hopper", &["Grace runs the compiler group", "Grace prefers email"]).await;
    assert_eq!(count_text(&app, "Grace runs the compiler group").await, 1);
    assert_eq!(count_text(&app, "Grace prefers email").await, 1);
}

// ---- facts written before tenant 0046 are placed by their record, not their extraction -------

#[tokio::test]
async fn an_undated_old_extracted_fact_is_dated_by_its_record_when_compared() {
    let answers = Answers::default();
    *answers.pairs.lock().unwrap() = both_ways();
    let app = app(answers.clone()).await;
    let (ada, _) = write(&app, "Ada Lovelace", "Ada likes tea", None).await;
    // extracted last month from a 2019 email, before the upgrade: no dates; the backfill has not reached it
    let email = plant_record(&app, "old", "2019-05-01T09:00:00Z").await;
    let old: Vec<String> = app
        .db()
        .await
        .test_raw()
        .query(format!(
            "CREATE memory SET owner = {}, vault = (SELECT VALUE vault FROM ONLY {ada}), subject = {ada}, text = 'Ada works at Acme', type = 'world', \
             source = {email}, created_at = time::now() - 30d, updated_at = time::now() - 30d RETURN VALUE <string> id",
            app.user.id.to_string(),
            ada = ada.to_string()
        ))
        .await
        .unwrap()
        .take(0)
        .unwrap();
    let old = eunomia_backend::rid::parse(&old[0]).unwrap();
    let (_, left) = extract_fact(&app, &answers, "people", "Ada Lovelace", "Ada left Acme", "2026-09-15T09:00:00+00:00").await;
    check(&app, &ada, std::slice::from_ref(&left)).await;
    assert!(!superseded(&app, &left).await, "a 2026 message is not replaced by a 2019 email read last month");
    assert!(superseded(&app, &old).await);
    let d = dates(&app, &old).await;
    assert!(d["asserted"].as_str().unwrap().starts_with("2019-05-01"), "dated from its record on the spot, and kept: {d}");
    let ledger: Vec<Value> = app.db().await.test_raw().query("SELECT id FROM fact_dates_redated").await.unwrap().take(0).unwrap();
    assert_eq!(ledger.len(), 1, "recorded like a backfill re-date");
}

// ---- edits: only a changed claim on a current fact is a telling -------------------------------

#[tokio::test]
async fn editing_a_replaced_fact_or_only_its_type_tells_nothing() {
    let answers = Answers::default();
    *answers.pairs.lock().unwrap() = both_ways();
    let app = app(answers).await;
    let (ada, vp) = write(&app, "Ada Lovelace", "Ada is VP of Engineering", None).await;
    let (_, cto) = write(&app, "Ada Lovelace", "Ada is the CTO", None).await;
    check(&app, &ada, std::slice::from_ref(&cto)).await;
    assert!(superseded(&app, &vp).await);
    let before = dates(&app, &vp).await;
    // a typo fix in the replaced fact: it stays replaced, by the same fact, told when it was
    let out = app.tool("memory_update", json!({"memory_id": vp.to_string(), "text": "Ada was VP of Engineering"})).await;
    assert!(out.get("error").is_none(), "{out}");
    assert!(superseded(&app, &vp).await, "still replaced");
    assert_eq!(dates(&app, &vp).await, before, "same superseded_by, same asserted_at");
    // a type-only edit of the current fact: not told again
    let cto_before = dates(&app, &cto).await;
    let out = app.tool("memory_update", json!({"memory_id": cto.to_string(), "type": "experience"})).await;
    assert!(out.get("error").is_none(), "{out}");
    assert_eq!(dates(&app, &cto).await, cto_before);
    let (vp_key, cto_key) = (vp.to_string(), cto.to_string());
    let jobs = supersede_jobs(&app).await;
    assert!(!jobs.iter().any(|k| (k.contains(vp_key.as_str()) || k.contains(cto_key.as_str())) && k.contains(":v")), "no edit queued a check: {jobs:?}");
}

// ---- deleting the replacing fact ------------------------------------------------------------

#[tokio::test]
async fn deleting_the_replacing_fact_restores_only_the_fact_it_replaced() {
    let answers = Answers::default();
    *answers.pairs.lock().unwrap() = both_ways();
    let app = app(answers).await;
    let (ada, a) = write(&app, "Ada Lovelace", "Ada works at Acme", None).await;
    let (_, b) = write(&app, "Ada Lovelace", "Ada works at Globex", None).await;
    check(&app, &ada, std::slice::from_ref(&b)).await;
    let (_, c) = write(&app, "Ada Lovelace", "Ada works at Initech", None).await;
    check(&app, &ada, std::slice::from_ref(&c)).await;
    assert!(superseded(&app, &a).await && superseded(&app, &b).await, "a chain: a by b, b by c");
    assert_eq!(dates(&app, &b).await["by"], json!(c.to_string()));
    // the wrong correction is deleted: the fact it replaced is current again; the one further down stays
    let out = app.tool("memory_delete", json!({"memory_id": c.to_string()})).await;
    assert_eq!(out["deleted"], json!(true), "{out}");
    assert!(!superseded(&app, &b).await, "b is current again");
    assert_eq!(dates(&app, &b).await["by"], json!("NONE"));
    assert!(superseded(&app, &a).await, "a stays replaced by b");
    let b_key = b.to_string();
    assert!(supersede_jobs(&app).await.iter().any(|k| k.contains(b_key.as_str()) && k.contains(":restored:")), "b is checked again");
}

// ---- two facts from one record ----------------------------------------------------------------

#[tokio::test]
async fn two_facts_from_one_record_never_replace_each_other() {
    let answers = Answers::default();
    *answers.pairs.lock().unwrap() = both_ways();
    let app = app(answers.clone()).await;
    extract_record(&app, &answers, "notes:promo", "2025-05-01T09:00:00+00:00", "people", "Ada Lovelace", &["Ada was VP", "Ada is now CTO"]).await;
    let (ada, vp) = fact_by_text(&app, "Ada was VP").await;
    let (_, cto) = fact_by_text(&app, "Ada is now CTO").await;
    check(&app, &ada, std::slice::from_ref(&cto)).await;
    check(&app, &ada, std::slice::from_ref(&vp)).await;
    assert!(!superseded(&app, &vp).await && !superseded(&app, &cto).await, "one record told both at once");
}

// ---- vault copy keeps the dates and the replacements --------------------------------------

#[tokio::test]
async fn a_vault_copy_keeps_dates_status_and_who_replaced_whom() {
    let answers = Answers::default();
    *answers.pairs.lock().unwrap() = both_ways();
    let app = app(answers).await;
    let (ada, old) = write(&app, "Ada Lovelace", "Ada works at Acme", Some("2024-02-01")).await;
    let (_, new) = write(&app, "Ada Lovelace", "Ada works at Globex", Some("2025-03-01")).await;
    check(&app, &ada, std::slice::from_ref(&new)).await;
    let vault: Vec<String> =
        app.db().await.test_raw().query(format!("SELECT VALUE <string> vault FROM ONLY {}", ada.to_string())).await.unwrap().take(0).unwrap();
    let out = app.tool("vault_clone", json!({"vault_id": vault[0], "name": "Ada Copy"})).await;
    assert!(out.get("error").is_none(), "{out}");
    let copy = out["id"].as_str().or(out["vault"]["id"].as_str()).unwrap_or_else(|| panic!("{out}")).to_string();
    let rows: Vec<Value> = app
        .db()
        .await
        .test_raw()
        .query(format!(
            "SELECT text, status, <string> occurred_at AS occurred, <string> asserted_at AS asserted, superseded_by.text AS by, <string> superseded_by.vault AS by_vault FROM memory WHERE vault = {copy} AND type = 'world'"
        ))
        .await
        .unwrap()
        .take(0)
        .unwrap();
    let get = |t: &str| rows.iter().find(|r| r["text"] == t).cloned().unwrap_or_else(|| panic!("{t}: {rows:?}"));
    let (o, n) = (get("Ada works at Acme"), get("Ada works at Globex"));
    assert_eq!(o["status"], json!("superseded"), "{o}");
    assert_eq!(o["by"], json!("Ada works at Globex"), "points at the copy of the fact that replaced it: {o}");
    assert_eq!(o["by_vault"], json!(copy), "the copy's own replacer, not the original's: {o}");
    assert!(o["occurred"].as_str().unwrap().starts_with("2024-02-01") && n["occurred"].as_str().unwrap().starts_with("2025-03-01"));
    assert_eq!(o["asserted"], dates(&app, &old).await["asserted"], "told when the original was");
}
