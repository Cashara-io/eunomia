//! The one-off fact-date backfill (`jobs::fact_dates`): facts extracted before extraction passed the
//! record's date on (seeded here with raw SQL in their old shape) get their record's date, each with a
//! ledger row; facts a person wrote, facts their writer dated, observations and facts of undated records
//! are untouched; a run stopped after one batch resumes at its cursor; a finished one changes nothing
//! more; facts written after it started are left alone; replacements that may have gone the wrong way
//! are counted, not changed; undo restores the previous dates except where a person changed one since.

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

/// A synced record (the cache key the extract path uses), with or without a date.
async fn plant_record(app: &TestApp, id: &str, occurred: Option<&str>) -> String {
    let key = eunomia_backend::rid::key_string(app.user.id.key()).unwrap();
    let rid = format!("cache_record:`{key}:{id}`");
    let at = occurred.map(|d| format!("d'{d}'")).unwrap_or_else(|| "NONE".into());
    sql(
        app,
        &format!(
            "CREATE {rid} SET owner = {}, source = 'gmail', type = 'gmail.message', external_id = '{id}', title = 't', body_text = 'b', \
             occurred_at = {at}, url = '', payload = {{}}, content_hash = 'fx', ingested_at = time::now(), updated_at = time::now(), deleted = false",
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

async fn date_of(app: &TestApp, id: &str) -> Option<String> {
    sql(app, &format!("SELECT VALUE <string> occurred_at FROM {id} WHERE occurred_at != NONE")).await.first().and_then(|v| v.as_str()).map(String::from)
}

async fn ledger(app: &TestApp) -> Vec<String> {
    let mut rows: Vec<String> = sql(app, "SELECT VALUE <string> memory FROM fact_dates_redated").await.into_iter().map(|v| v.as_str().unwrap().to_string()).collect();
    rows.sort();
    rows
}

#[tokio::test]
async fn facts_from_dated_records_are_redated_once_reversibly_and_nothing_else_is_touched() {
    let app = TestApp::new().await;
    let w = app.tool("memory_write", json!({"subject_name": "Ada Lovelace", "subject_kind": "person", "text": "Ada likes tea"})).await;
    let (subject, vault) = (w["entity"]["id"].as_str().unwrap().to_string(), w["entity"]["vault"].as_str().unwrap().to_string());
    let person_wrote = w["memory"]["id"].as_str().unwrap().to_string();
    let dated = plant_record(&app, "m1", Some("2023-04-01T09:30:00Z")).await;
    let undated = plant_record(&app, "m2", None).await;

    let extracted = plant_fact(&app, &subject, &vault, "Ada agreed to lead the pilot", Some(&dated), "").await;
    // a dateless memory_write citing the record: dated by it too
    let cited = plant_fact(&app, &subject, &vault, "Ada asked for the pilot budget", Some(&dated), "").await;
    let writer_dated = plant_fact(&app, &subject, &vault, "Ada met Acme", Some(&dated), ", occurred_at = d'2020-02-02T00:00:00Z'").await;
    let no_date = plant_fact(&app, &subject, &vault, "Ada uses Gmail", Some(&undated), "").await;
    let observation = plant_fact(&app, &subject, &vault, "Ada leads the pilot and likes tea", Some(&dated), ", type = 'observation'").await;
    // replaced earlier, when it looked newer than the fact that replaced it; now dated 2023, after a live
    // 2021 fact: a replacement that may have gone the wrong way (counted, left as it is)
    plant_fact(&app, &subject, &vault, "Ada is a junior analyst", None, ", occurred_at = d'2021-01-01T00:00:00Z'").await;
    let replaced = plant_fact(&app, &subject, &vault, "Ada is a senior analyst", Some(&dated), ", status = 'superseded'").await;
    // upgraded install: no marker yet (a new org is marked done when it is provisioned)
    sql(&app, &format!("DELETE fact_dates_backfill:v{}", fact_dates::VERSION)).await;

    let org = app.state.org(&app.user.org).await.unwrap();
    assert!(!fact_dates::done(&org).await.unwrap());
    let p = common::sys(fact_dates::run(&org, Some(1))).await.unwrap();
    assert!(p.done_at.is_none() && p.cursor.is_some(), "stopped after one batch, cursor saved: {p:?}");
    assert!(date_of(&app, &extracted).await.is_some_and(|d| d.starts_with("2023-04-01T09:30:00")));

    // written after the walk started (an agent citing the record, no date): left alone, like any new write
    let later = plant_fact(&app, &subject, &vault, "Ada sent the contract", Some(&dated), "").await;
    let p = common::sys(fact_dates::run(&org, None)).await.unwrap();
    assert!(p.done_at.is_some(), "{p:?}");
    assert_eq!((p.changed, p.suspect_superseded), (3, 1), "{p:?}");
    assert!(fact_dates::done(&org).await.unwrap());
    for id in [&cited, &replaced] {
        assert!(date_of(&app, id).await.is_some_and(|d| d.starts_with("2023-04-01")), "{id}");
    }
    assert_eq!(sql(&app, &format!("SELECT VALUE status FROM {replaced}")).await, vec![json!("superseded")], "counted, not changed");
    assert!(date_of(&app, &writer_dated).await.is_some_and(|d| d.starts_with("2020-02-02")), "a writer's date stays");
    for (id, why) in [(&person_wrote, "a fact a person wrote"), (&no_date, "the record has no date"), (&observation, "an observation"), (&later, "written after the cutoff")] {
        assert_eq!(date_of(&app, id).await, None, "{why}");
    }
    // one ledger row per re-dated fact, with the previous date (none) and the one set
    let mut want = vec![extracted.clone(), cited.clone(), replaced.clone()];
    want.sort();
    assert_eq!(ledger(&app).await, want);
    let row = sql(&app, &format!("SELECT previous, <string> occurred_at AS at, version FROM fact_dates_redated WHERE memory = {extracted}")).await;
    assert!(row[0].get("previous").is_none_or(Value::is_null) && row[0]["at"].as_str().unwrap().starts_with("2023-04-01") && row[0]["version"] == 1, "{row:?}");
    // the re-date is in the vault's change log as an update by system
    assert_eq!(sql(&app, &format!("SELECT VALUE actor FROM change WHERE op = 'updated' AND kind = 'memory' AND string::contains(item, '{}')", extracted.trim_start_matches("memory:").trim_matches(['`', '⟨', '⟩']))).await, vec![json!("system")]);

    // a finished backfill runs no more
    sql(&app, &format!("UPDATE {later} SET created_at = d'2020-01-01T00:00:00Z'")).await;
    let p = common::sys(fact_dates::run(&org, None)).await.unwrap();
    assert_eq!(p.changed, 3);
    assert_eq!(date_of(&app, &later).await, None);

    // a person re-dates one fact by hand; undo restores the rest and leaves that one
    sql(&app, &format!("UPDATE {cited} SET occurred_at = d'2023-05-05T00:00:00Z'")).await;
    assert_eq!(common::sys(fact_dates::undo(&org)).await.unwrap(), 2);
    assert_eq!(date_of(&app, &extracted).await, None);
    assert_eq!(date_of(&app, &replaced).await, None);
    assert!(date_of(&app, &cited).await.is_some_and(|d| d.starts_with("2023-05-05")), "a later change by a person stands");
    assert_eq!(ledger(&app).await, want, "ledger rows stay, marked undone");
    assert_eq!(sql(&app, "SELECT VALUE count() FROM fact_dates_redated WHERE undone_at != NONE GROUP ALL").await, vec![json!(3)]);
    assert_eq!(common::sys(fact_dates::undo(&org)).await.unwrap(), 0, "undo is idempotent");

    // a later run (a new marker version, here: the marker deleted) never dates a fact in the ledger again;
    // only the one fact it never dated (now older than the new cutoff) is a candidate
    sql(&app, &format!("DELETE fact_dates_backfill:v{}", fact_dates::VERSION)).await;
    let p = common::sys(fact_dates::run(&org, None)).await.unwrap();
    assert_eq!(p.changed, 1, "{p:?}");
    assert!(date_of(&app, &later).await.is_some());
    assert_eq!(date_of(&app, &extracted).await, None);
    assert_eq!(date_of(&app, &replaced).await, None);
}
