//! Recall ranking against a real in-memory SurrealDB, through the registry (the path MCP `recall` and
//! REST `/api/tools/recall` share): `time_range` is a filter, ranked by relevance inside the range,
//! and candidates from several vaults are fused in one ranking, so a strong match in one vault
//! outranks a weak match in another. No embedding provider here: the keyword, memory text, graph and
//! relation arms rank (the semantic arms are covered with a stub provider in `recall_semantic.rs`).

mod common;

use common::TestApp;
use serde_json::{json, Value};

async fn write(app: &TestApp, subject: &str, kind: &str, text: &str, extra: Value) -> Value {
    let mut args = json!({"subject_name": subject, "subject_kind": kind, "text": text});
    args.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
    let out = app.tool("memory_write", args).await;
    assert!(out.get("error").is_none(), "{out}");
    out
}

async fn recall(app: &TestApp, args: Value) -> Vec<Value> {
    let out = app.tool("recall", args).await;
    assert!(out.get("error").is_none(), "{out}");
    out["results"].as_array().cloned().unwrap()
}

fn texts(hits: &[Value]) -> Vec<String> {
    hits.iter().map(|h| h["text"].as_str().unwrap().to_string()).collect()
}

fn date(h: &Value) -> chrono::DateTime<chrono::Utc> {
    let s = h["occurred_at"].as_str().unwrap_or_else(|| panic!("every ranged hit is dated: {h}"));
    chrono::DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&chrono::Utc)
}

/// A synced record in the test user's personal vault, dated `at`.
async fn record(app: &TestApp, key: &str, title: &str, body: &str, at: &str) {
    let db = app.db().await;
    db.test_raw()
        .query(format!(
            "CREATE cache_record:{key} SET owner = $owner, source = 'demo', type = 'note', external_id = '{key}', \
             content_hash = '{key}', title = $title, body_text = $body, occurred_at = <datetime>$at, deleted = false, \
             ingested_at = time::now(), updated_at = time::now()"
        ))
        .bind(("owner", app.user.id.clone()))
        .bind(("title", title.to_string()))
        .bind(("body", body.to_string()))
        .bind(("at", at.to_string()))
        .await
        .unwrap()
        .check()
        .unwrap();
}

const MARCH: [&str; 2] = ["2026-03-01", "2026-03-31T23:59:59Z"];

/// "What did we decide about pricing in March": only March, ranked by relevance to pricing.
#[tokio::test]
async fn a_time_range_is_a_filter_and_ranks_by_relevance_inside_it() {
    let app = TestApp::new().await;
    // in March: the answer, a weaker pricing mention, and the newest fact of the month about something else
    write(&app, "Acme Corp", "organisation", "Pricing decision: the Pro plan price goes to 49 dollars", json!({"occurred_at": "2026-03-10"})).await;
    write(&app, "Acme Corp", "organisation", "Someone mentioned pricing at lunch", json!({"occurred_at": "2026-03-20"})).await;
    write(&app, "Bob", "person", "Bob prefers tea over coffee", json!({"occurred_at": "2026-03-30"})).await;
    record(&app, "march_minutes", "Board minutes", "Pricing decision recorded for the Pro plan", "2026-03-15T09:00:00Z").await;
    // outside March: stronger or equal pricing matches, which must never come back
    write(&app, "Acme Corp", "organisation", "Pricing decision: pricing decision deferred, pricing decision pending", json!({})).await;
    write(&app, "Acme Corp", "organisation", "Pricing decision: the Pro plan price was 39 dollars", json!({"occurred_at": "2025-12-01"})).await;
    record(&app, "april_minutes", "Pricing decision", "Pricing decision reversed for the Pro plan", "2026-04-02T09:00:00Z").await;

    let (since, until) = (cs_parse(MARCH[0]), cs_parse(MARCH[1]));
    let hits = recall(&app, json!({"query": "pricing decision", "time_range": MARCH})).await;
    let found = texts(&hits);
    for h in &hits {
        assert!(date(h) >= since && date(h) <= until, "out-of-range item returned: {h}");
    }
    for out in ["deferred", "39 dollars", "reversed"] {
        assert!(!found.iter().any(|t| t.contains(out)), "{out}: {found:?}");
    }
    // relevance, not recency: the newest fact of the month is not a pricing fact and is not returned
    assert!(!found.iter().any(|t| t.contains("tea")), "{found:?}");
    let pos = |needle: &str| found.iter().position(|t| t.contains(needle)).unwrap_or_else(|| panic!("{needle}: {found:?}"));
    assert!(pos("49 dollars") < pos("at lunch"), "both terms beat one term, though the one-term fact is newer: {found:?}");
    assert!(pos("Board minutes") < pos("at lunch"), "{found:?}");
    assert_eq!(hits[0]["score"], 1.0);

    // An empty query with a range: the newest items in it, newest first, nothing from outside it.
    let hits = recall(&app, json!({"query": "", "time_range": MARCH})).await;
    let found = texts(&hits);
    assert_eq!(found.len(), 4, "{found:?}");
    assert!(found[0].contains("tea") && found[1].contains("at lunch") && found[2].contains("Board minutes") && found[3].contains("49 dollars"), "{found:?}");
    let dates: Vec<_> = hits.iter().map(date).collect();
    assert!(dates.windows(2).all(|w| w[0] >= w[1]), "{dates:?}");

    // the same with a limit and a types filter
    let hits = recall(&app, json!({"query": "", "time_range": MARCH, "limit": 1})).await;
    assert_eq!(texts(&hits), ["Bob prefers tea over coffee"]);
    let hits = recall(&app, json!({"query": "pricing decision", "time_range": MARCH, "types": ["experience"]})).await;
    assert!(hits.iter().all(|h| h["kind"] != "memory"), "{hits:?}");

    // without a range everything still competes, the newest stronger match included
    let hits = recall(&app, json!({"query": "pricing decision"})).await;
    assert!(texts(&hits).iter().any(|t| t.contains("deferred")), "{hits:?}");
}

fn cs_parse(s: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap().and_hms_opt(0, 0, 0).unwrap().and_utc())
}

/// A relation is dated by when it was recorded: a range that leaves today out leaves it out too.
#[tokio::test]
async fn a_relation_outside_the_range_is_not_returned() {
    let app = TestApp::new().await;
    let ada = write(&app, "Ada Lovelace", "person", "Ada prefers async updates", json!({"occurred_at": "2026-03-05"})).await;
    let acme = write(&app, "Acme Corp", "organisation", "Acme renewed their contract", json!({"occurred_at": "2026-03-06"})).await;
    let (ada, acme) = (ada["entity"]["id"].as_str().unwrap(), acme["entity"]["id"].as_str().unwrap());
    app.db().await.test_raw().query(format!("RELATE {ada}->relates_to->{acme} SET label = 'works_at'")).await.unwrap().check().unwrap();

    let q = "How is Ada Lovelace connected to Acme Corp?";
    let hits = recall(&app, json!({"query": q, "time_range": ["2000-01-01", "2999-01-01"]})).await;
    assert!(hits.iter().any(|h| h["kind"] == "relation"), "{hits:?}");
    let hits = recall(&app, json!({"query": q, "time_range": MARCH})).await;
    assert!(!hits.iter().any(|h| h["kind"] == "relation"), "recorded today, not in March: {hits:?}");
    assert!(texts(&hits).contains(&"Ada prefers async updates".to_string()), "{hits:?}");
}

/// A small org vault's weak best hit no longer scores the same as the personal vault's exact answer.
#[tokio::test]
async fn a_strong_match_in_one_vault_outranks_a_weak_match_in_another() {
    let app = TestApp::new().await;
    let team = app.tool("vault_create", json!({"name": "Team"})).await;
    assert!(team.get("error").is_none(), "{team}");
    // personal: three facts matching every word of the question
    for text in [
        "The quarterly revenue forecast is 2.1 million",
        "Quarterly revenue forecast assumes five percent churn",
        "The quarterly board pack needs the revenue forecast by Friday",
    ] {
        write(&app, "Finance", "organisation", text, json!({})).await;
    }
    // Team: one fact matching a single word
    write(&app, "Office", "location", "Revenue came up once at the offsite", json!({"vault_id": "Team"})).await;

    let q = "quarterly revenue forecast";
    let hits = recall(&app, json!({"query": q, "limit": 3})).await;
    assert_eq!(hits.len(), 3, "{hits:?}");
    assert!(hits.iter().all(|h| h["vault_name"] == "Personal"), "the weak Team hit must not take a top-3 place: {hits:?}");

    let hits = recall(&app, json!({"query": q})).await;
    let weak = hits.iter().position(|h| h["vault_name"] == "Team").unwrap_or_else(|| panic!("{hits:?}"));
    assert_eq!(weak, 3, "{hits:?}");
    assert_eq!(hits[0]["score"], 1.0, "scores stay relative to the best hit of the whole recall");
    assert!(hits[weak]["score"].as_f64().unwrap() < 1.0, "{hits:?}");

    // a single-vault call still ranks inside that vault, its best hit at 1
    let hits = recall(&app, json!({"query": q, "vault_id": "Team"})).await;
    assert_eq!(texts(&hits), ["Revenue came up once at the offsite"]);
    assert_eq!(hits[0]["score"], 1.0);
    let hits = recall(&app, json!({"query": q, "vault_ids": ["Team", "Personal"], "limit": 3})).await;
    assert!(hits.iter().all(|h| h["vault_name"] == "Personal"), "{hits:?}");

    // a user of the org who is not a member of Team gets nothing from it, by default or by asking
    let other = common::register(&app.state, "outsider@example.com").await;
    let call = |args: Value| {
        let (state, other) = (app.state.clone(), other.clone());
        async move { eunomia_backend::authz::as_system(eunomia_backend::tools::registry::call(&state, &other, "recall", args)).await }
    };
    let out = call(json!({"query": q})).await.unwrap();
    assert!(!out.to_string().contains("offsite") && !out.to_string().contains("quarterly"), "{out}");
    let team_id = app.tool("vault_list", json!({})).await["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "Team")
        .and_then(|v| v["id"].as_str())
        .unwrap()
        .to_string();
    match call(json!({"query": q, "vault_id": team_id})).await {
        Err(e) => assert_eq!(e.status.as_u16(), 403, "{e:?}"),
        Ok(out) => panic!("a non-member read Team: {out}"),
    }
}
