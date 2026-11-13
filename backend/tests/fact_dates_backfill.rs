//! The one-off fact-date backfill (`jobs::fact_dates`): facts extracted before extraction passed the
//! record's date on (seeded here with raw SQL in their old shape) get their record's date as when they
//! happened and when we were told them, never later than their own write; facts a person wrote or
//! edited, facts their writer dated, observations and facts of undated records are untouched; the walk
//! resumes by record-id range across batches; undo restores both dates except where a person changed one
//! since.

mod common;

use common::TestApp;
use eunomia_backend::jobs::fact_dates;
use eunomia_backend::rid::RecordIdExt;
use serde_json::{json, Value};

async fn sql(app: &TestApp, q: &str) -> Vec<Value> {
    let db = app.db().await;
    let mut res = db.test_raw().query(q).await.unwrap_or_else(|e| panic!("{q}: {e}")).check().unwrap_or_else(|e| panic!("{q}: {e}"));
    res.take::<Vec<Value>>(0).unwrap()
}

fn owner(app: &TestApp) -> String {
    app.user.id.to_string()
}

/// A synced record of `source` (the cache key the extract path uses), with or without a date.
async fn plant_record(app: &TestApp, id: &str, source: &str, occurred: Option<&str>, payload: &str) -> String {
    let key = eunomia_backend::rid::key_string(app.user.id.key()).unwrap();
    let rid = format!("cache_record:`{key}:{id}`");
    let at = occurred.map(|d| format!("d'{d}'")).unwrap_or_else(|| "NONE".into());
    sql(
        app,
        &format!(
            "CREATE {rid} SET owner = {}, source = '{source}', type = '{source}.item', external_id = '{id}', title = 't', body_text = 'b', \
             occurred_at = {at}, url = '', payload = {payload}, content_hash = 'fx', ingested_at = time::now(), updated_at = time::now(), deleted = false",
            owner(app)
        ),
    )
    .await;
    rid
}

/// A memory in the shape extraction (or a dateless `memory_write` citing a record) wrote before this fix.
async fn plant_fact(app: &TestApp, subject: &str, vault: &str, text: &str, source: Option<&str>, extra: &str) -> String {
    let src = source.unwrap_or("NONE");
    let rows = sql(
        app,
        &format!("CREATE memory SET owner = {}, vault = {vault}, subject = {subject}, text = '{text}', type = 'world', source = {src}{extra} RETURN VALUE <string> id", owner(app)),
    )
    .await;
    rows[0].as_str().unwrap().to_string()
}

/// `(occurred_at, asserted_at)` as strings.
async fn dates_of(app: &TestApp, id: &str) -> (Option<String>, Option<String>) {
    let v = sql(app, &format!("SELECT occurred_at AS o, asserted_at AS a FROM {id}")).await;
    let s = |k: &str| v[0].get(k).and_then(|x| x.as_str()).map(String::from);
    (s("o"), s("a"))
}

async fn date_of(app: &TestApp, id: &str) -> Option<String> {
    dates_of(app, id).await.0
}

async fn ledger(app: &TestApp) -> Vec<String> {
    let mut rows: Vec<String> = sql(app, "SELECT VALUE <string> memory FROM fact_dates_redated").await.into_iter().map(|v| v.as_str().unwrap().to_string()).collect();
    rows.sort();
    rows
}

async fn entity(app: &TestApp, name: &str) -> (String, String) {
    let w = app.tool("memory_write", json!({"subject_name": name, "subject_kind": "person", "text": format!("{name} likes tea")})).await;
    (w["entity"]["id"].as_str().unwrap().to_string(), w["entity"]["vault"].as_str().unwrap().to_string())
}

/// An upgraded install: no marker yet (a new org is marked done when it is provisioned).
async fn unmark(app: &TestApp) {
    sql(app, &format!("DELETE fact_dates_backfill:v{}", fact_dates::VERSION)).await;
}

#[tokio::test]
async fn facts_from_dated_records_are_dated_once_reversibly_and_nothing_else_is_touched() {
    let app = TestApp::new().await;
    let w = app.tool("memory_write", json!({"subject_name": "Ada Lovelace", "subject_kind": "person", "text": "Ada likes tea"})).await;
    let (subject, vault) = (w["entity"]["id"].as_str().unwrap().to_string(), w["entity"]["vault"].as_str().unwrap().to_string());
    let person_wrote = w["memory"]["id"].as_str().unwrap().to_string();
    let dated = plant_record(&app, "m1", "gmail", Some("2023-04-01T09:30:00Z"), "{}").await;
    let undated = plant_record(&app, "m2", "gmail", None, "{}").await;
    // Notion pages edited after the facts were drawn from them: a fact is dated by its own write, not by the
    // page's later edit nor by its creation date (which can be years too early)
    let edited_page = plant_record(&app, "p1", "notion", Some("2026-11-10T00:00:00Z"), "{ created_time: '2024-01-01T00:00:00.000Z' }").await;
    let edited_page_bare = plant_record(&app, "p2", "notion", Some("2026-11-10T00:00:00Z"), "{}").await;

    let extracted = plant_fact(&app, &subject, &vault, "Ada agreed to lead the pilot", Some(&dated), "").await;
    // a dateless memory_write citing the record: dated by it too
    let cited = plant_fact(&app, &subject, &vault, "Ada asked for the pilot budget", Some(&dated), "").await;
    let writer_dated = plant_fact(&app, &subject, &vault, "Ada met Acme", Some(&dated), ", occurred_at = d'2020-02-02T00:00:00Z'").await;
    let no_date = plant_fact(&app, &subject, &vault, "Ada uses Gmail", Some(&undated), "").await;
    let observation = plant_fact(&app, &subject, &vault, "Ada leads the pilot and likes tea", Some(&dated), ", type = 'observation'").await;
    let edited = plant_fact(&app, &subject, &vault, "Ada is the staff engineer", Some(&dated), ", version = 2, updated_at = time::now() + 1m").await;
    let at_2025 = ", created_at = d'2025-02-01T00:00:00Z', updated_at = d'2025-02-01T00:00:00Z'";
    let from_page = plant_fact(&app, &subject, &vault, "Ada wrote the pilot plan", Some(&edited_page), at_2025).await;
    let from_bare = plant_fact(&app, &subject, &vault, "Ada reviewed the plan", Some(&edited_page_bare), at_2025).await;

    unmark(&app).await;

    let org = app.state.org(&app.user.org).await.unwrap();
    assert!(!fact_dates::done(&org).await.unwrap());
    let p = common::sys(fact_dates::run(&org, Some(1))).await.unwrap();
    assert!(p.done_at.is_none() && p.cursor.is_some(), "stopped after one batch, cursor saved: {p:?}");
    let (o, a) = dates_of(&app, &extracted).await;
    assert!(o.as_deref().is_some_and(|d| d.starts_with("2023-04-01T09:30:00")) && a == o, "both dates from the record: {o:?} {a:?}");

    // written after the walk started (an agent citing the record, no date): left alone, like any new write
    let later = plant_fact(&app, &subject, &vault, "Ada sent the contract", Some(&dated), "").await;
    let p = common::sys(fact_dates::run(&org, None)).await.unwrap();
    assert!(p.done_at.is_some(), "{p:?}");
    assert_eq!(p.changed, 4, "{p:?}");
    assert!(fact_dates::done(&org).await.unwrap());
    for id in [&cited] {
        assert!(date_of(&app, id).await.is_some_and(|d| d.starts_with("2023-04-01")), "{id}");
    }
    // never later than the fact's write, and never the page's creation date: the write itself
    let both = |d: &str| (Some(d.to_string()), Some(d.to_string()));
    assert_eq!(dates_of(&app, &from_page).await, both("2025-02-01T00:00:00Z"));
    assert_eq!(dates_of(&app, &from_bare).await, both("2025-02-01T00:00:00Z"));
    assert!(date_of(&app, &writer_dated).await.is_some_and(|d| d.starts_with("2020-02-02")), "a writer's date stays");
    for (id, why) in [
        (&person_wrote, "a fact a person wrote"),
        (&no_date, "the record has no date"),
        (&observation, "an observation"),
        (&edited, "edited by a person since"),
        (&later, "written after the cutoff"),
    ] {
        assert_eq!(dates_of(&app, id).await.0, None, "{why}");
        assert_eq!(sql(&app, &format!("SELECT VALUE id FROM fact_dates_redated WHERE memory = {id}")).await, Vec::<Value>::new(), "{why}");
    }
    // one ledger row per re-dated fact, with the previous dates (none) and the ones set
    let mut want = vec![extracted.clone(), cited.clone(), from_page.clone(), from_bare.clone()];
    want.sort();
    assert_eq!(ledger(&app).await, want);
    let row = sql(&app, &format!("SELECT previous, previous_asserted, <string> occurred_at AS at, version FROM fact_dates_redated WHERE memory = {extracted}")).await;
    assert!(row[0].get("previous").is_none_or(Value::is_null) && row[0].get("previous_asserted").is_none_or(Value::is_null), "{row:?}");
    assert!(row[0]["at"].as_str().unwrap().starts_with("2023-04-01") && row[0]["version"] == 1, "{row:?}");
    // the re-date is in the vault's change log as an update by system
    let key = extracted.trim_start_matches("memory:").trim_matches(['`', '⟨', '⟩']);
    let actors = sql(&app, &format!("SELECT VALUE actor FROM change WHERE op = 'updated' AND kind = 'memory' AND string::contains(item, '{key}')")).await;
    assert_eq!(actors, vec![json!("system")]);

    // a finished backfill runs no more
    sql(&app, &format!("UPDATE {later} SET created_at = d'2020-01-01T00:00:00Z', updated_at = d'2020-01-01T00:00:00Z'")).await;
    let p = common::sys(fact_dates::run(&org, None)).await.unwrap();
    assert_eq!(p.changed, 4);
    assert_eq!(date_of(&app, &later).await, None);

    // a person re-dates one fact by hand; undo restores the rest and leaves that one
    sql(&app, &format!("UPDATE {cited} SET occurred_at = d'2023-05-05T00:00:00Z'")).await;
    assert_eq!(common::sys(fact_dates::undo(&org)).await.unwrap(), 3);
    assert_eq!(dates_of(&app, &extracted).await, (None, None), "both dates restored");
    assert_eq!(dates_of(&app, &from_page).await, (None, None));
    assert!(date_of(&app, &cited).await.is_some_and(|d| d.starts_with("2023-05-05")), "a later change by a person stands");
    assert_eq!(ledger(&app).await, want, "ledger rows stay, marked undone");
    assert_eq!(sql(&app, "SELECT VALUE count() FROM fact_dates_redated WHERE undone_at != NONE GROUP ALL").await, vec![json!(4)]);
    assert_eq!(common::sys(fact_dates::undo(&org)).await.unwrap(), 0, "undo is idempotent");

    // a later run (a new marker version, here: the marker deleted) never dates a fact in the ledger again;
    // only the one fact it never dated (now older than the new cutoff) is a candidate
    unmark(&app).await;
    let p = common::sys(fact_dates::run(&org, None)).await.unwrap();
    assert_eq!(p.changed, 1, "{p:?}");
    assert!(date_of(&app, &later).await.is_some());
    assert_eq!(date_of(&app, &extracted).await, None);
}

/// More facts than one batch: each batch starts where the last stopped (a record-id range), so every fact
/// is dated exactly once and none is skipped.
#[tokio::test]
async fn the_walk_resumes_by_id_range_across_batches() {
    let app = TestApp::new().await;
    let (subject, vault) = entity(&app, "Ada Lovelace").await;
    let dated = plant_record(&app, "m1", "gmail", Some("2023-04-01T09:30:00Z"), "{}").await;
    sql(
        &app,
        &format!(
            "FOR $i IN 0..450 {{ CREATE memory SET owner = {}, vault = {vault}, subject = {subject}, text = 'fact ' + <string> $i, type = 'world', source = {dated} }}",
            owner(&app)
        ),
    )
    .await;
    unmark(&app).await;
    let org = app.state.org(&app.user.org).await.unwrap();
    let first = common::sys(fact_dates::run(&org, Some(1))).await.unwrap();
    assert!((199..=200).contains(&first.changed), "one batch of 200 rows: {first:?}");
    let second = common::sys(fact_dates::run(&org, Some(1))).await.unwrap();
    assert!((399..=400).contains(&second.changed), "the next 200, not the first again: {second:?}");
    let done = common::sys(fact_dates::run(&org, None)).await.unwrap();
    assert_eq!(done.changed, 450, "{done:?}");
    let n = sql(&app, "SELECT VALUE count() FROM memory WHERE string::starts_with(text, 'fact ') AND occurred_at != NONE GROUP ALL").await;
    assert_eq!(n, vec![json!(450)]);
    // undo walks the ledger by range too: three batches, each row once
    let undone = tokio::time::timeout(std::time::Duration::from_secs(120), common::sys(fact_dates::undo(&org))).await.expect("undo finishes");
    assert_eq!(undone.unwrap(), 450);
}
