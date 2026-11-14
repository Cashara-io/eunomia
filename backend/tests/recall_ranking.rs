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

async fn raw(app: &TestApp, sql: &str) {
    app.db().await.test_raw().query(sql).await.unwrap().check().unwrap();
}

/// A query that matches nothing inside the range gets nothing back: not the range's newest items
/// scored 1.0 as if they were the answer. Only an empty query asks for "what happened then".
#[tokio::test]
async fn a_query_matching_nothing_in_the_range_returns_nothing() {
    let app = TestApp::new().await;
    write(&app, "Kitchen", "location", "Bought a new kettle for the office", json!({"occurred_at": "2026-03-28"})).await;
    record(&app, "march_note", "Standup", "Standup moved to ten", "2026-03-12T09:00:00Z").await;
    // matches, but only outside the range
    write(&app, "Acme Corp", "organisation", "Acme signed the renewal", json!({"occurred_at": "2026-05-02"})).await;

    // words that match nothing at all (no embeddings either: every arm is empty)
    assert!(recall(&app, json!({"query": "zebra xylophone", "time_range": MARCH})).await.is_empty());
    // words that match only outside the range
    let hits = recall(&app, json!({"query": "Did Acme sign the renewal?", "time_range": MARCH})).await;
    assert!(hits.is_empty(), "{hits:?}");
    // the same question without the range finds it, and an empty query lists the range
    assert!(!recall(&app, json!({"query": "Did Acme sign the renewal?"})).await.is_empty());
    assert_eq!(recall(&app, json!({"query": "  ", "time_range": MARCH})).await.len(), 2);
}

/// `["2026-03-01", "2026-03-31"]` is the whole of March: both ends inclusive, a bare date its whole
/// day in UTC; a full timestamp is exact.
#[tokio::test]
async fn a_date_only_range_covers_whole_days_and_timestamps_are_exact() {
    let app = TestApp::new().await;
    for (key, at) in [
        ("feb_last", "2026-02-28T23:59:59.999Z"),
        ("mar_first", "2026-03-01T00:00:00Z"),
        ("mar_last_evening", "2026-03-31T18:00:00Z"),
        ("mar_last_instant", "2026-03-31T23:59:59.999Z"),
        ("apr_first", "2026-04-01T00:00:00Z"),
    ] {
        record(&app, key, &format!("Gadget log {key}"), "gadget calibration", at).await;
    }
    write(&app, "Lab", "location", "Gadget calibration finished late", json!({"occurred_at": "2026-03-31T22:30:00Z"})).await;

    let ids = |hits: &[Value]| hits.iter().map(|h| h["id"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    let hits = recall(&app, json!({"query": "gadget calibration", "time_range": ["2026-03-01", "2026-03-31"]})).await;
    let got = ids(&hits);
    for want in ["mar_first", "mar_last_evening", "mar_last_instant"] {
        assert!(got.contains(&want.to_string()), "{want}: {got:?}");
    }
    assert!(texts(&hits).iter().any(|t| t.contains("finished late")), "{hits:?}");
    for not in ["feb_last", "apr_first"] {
        assert!(!got.contains(&not.to_string()), "{not}: {got:?}");
    }
    // the same with an empty query (the newest-in-range list)
    let got = ids(&recall(&app, json!({"query": "", "time_range": ["2026-03-01", "2026-03-31"]})).await);
    assert!(got.contains(&"mar_last_instant".to_string()) && !got.contains(&"apr_first".to_string()), "{got:?}");
    // a timestamp end is exact and inclusive
    let got = ids(&recall(&app, json!({"query": "gadget calibration", "time_range": ["2026-03-01", "2026-03-31T18:00:00Z"]})).await);
    assert!(got.contains(&"mar_last_evening".to_string()) && !got.contains(&"mar_last_instant".to_string()), "{got:?}");
    // one whole day
    let got = ids(&recall(&app, json!({"query": "gadget calibration", "time_range": ["2026-03-31", "2026-03-31"]})).await);
    assert_eq!(got.len(), 3, "{got:?}");
}

/// Observations summarise current belief: under a range they are left out unless `types` asks for
/// them, and then dated by their last update (not by any `occurred_at`).
#[tokio::test]
async fn observations_join_a_range_only_when_asked_for_and_are_dated_by_their_last_update() {
    let app = TestApp::new().await;
    write(&app, "Ada Lovelace", "person", "Ada moved the budget review to Friday", json!({"occurred_at": "2026-03-12"})).await;
    // Ada's observation: last updated in March, nothing else dates it
    let ada = write(&app, "Ada Lovelace", "person", "Ada owns the budget review", json!({"type": "observation"})).await;
    raw(&app, &format!("UPDATE {} SET updated_at = d'2026-03-20T10:00:00Z'", ada["memory"]["id"].as_str().unwrap())).await;
    // Bob's: an occurred_at in March, but last updated in June
    let bob = write(&app, "Bob Builder", "person", "Bob chairs the budget review", json!({"type": "observation"})).await;
    raw(&app, &format!("UPDATE {} SET occurred_at = d'2026-03-15T00:00:00Z', updated_at = d'2026-06-01T10:00:00Z'", bob["memory"]["id"].as_str().unwrap())).await;

    let q = "budget review";
    let has = |hits: &[Value], text: &str| texts(hits).iter().any(|t| t == text);
    let (ada, bob) = ("Ada owns the budget review", "Bob chairs the budget review");

    // not asked for: left out of a range, though in it
    let hits = recall(&app, json!({"query": q, "time_range": MARCH})).await;
    assert!(!has(&hits, ada), "an observation is not an event: {hits:?}");
    assert!(texts(&hits).iter().any(|t| t.contains("Friday")), "{hits:?}");
    assert!(!has(&recall(&app, json!({"query": "", "time_range": MARCH})).await, ada), "nor in the newest-in-range list");

    // asked for: in, dated by its last update
    let hits = recall(&app, json!({"query": q, "time_range": MARCH, "types": ["observation", "world"]})).await;
    let o = hits.iter().find(|h| h["text"] == ada).unwrap_or_else(|| panic!("asked for: {hits:?}"));
    assert!(o["occurred_at"].as_str().unwrap().starts_with("2026-03-20"), "{o}");
    assert!(has(&recall(&app, json!({"query": "", "time_range": MARCH, "types": ["observation"]})).await, ada));
    // by its update, not its occurred_at: Bob's is a June observation
    assert!(!has(&hits, bob), "dated by its last update, not occurred_at: {hits:?}");
    let june = recall(&app, json!({"query": q, "time_range": ["2026-06-01", "2026-06-30"], "types": ["observation"]})).await;
    let o = june.iter().find(|h| h["text"] == bob).unwrap_or_else(|| panic!("{june:?}"));
    assert!(o["occurred_at"].as_str().unwrap().starts_with("2026-06-01"), "{o}");

    // no range: observations as before
    assert!(has(&recall(&app, json!({"query": q})).await, ada));
}

/// A relation is dated by when it held if it carries such a date (a field no relation has yet; the
/// test defines it), else by when it was written.
#[tokio::test]
async fn a_relation_is_dated_by_its_own_date_when_it_has_one() {
    let app = TestApp::new().await;
    let ada = write(&app, "Ada Lovelace", "person", "Ada prefers async updates", json!({"occurred_at": "2026-03-05"})).await;
    let acme = write(&app, "Acme Corp", "organisation", "Acme renewed their contract", json!({"occurred_at": "2026-03-06"})).await;
    let (ada, acme) = (ada["entity"]["id"].as_str().unwrap(), acme["entity"]["id"].as_str().unwrap());
    raw(&app, "DEFINE FIELD IF NOT EXISTS occurred_at ON relates_to TYPE option<datetime>").await;
    raw(&app, &format!("RELATE {ada}->relates_to->{acme} SET label = 'works_at', occurred_at = d'2026-03-09T00:00:00Z'")).await;

    let q = "How is Ada Lovelace connected to Acme Corp?";
    let hits = recall(&app, json!({"query": q, "time_range": MARCH})).await;
    let rel = hits.iter().find(|h| h["kind"] == "relation").unwrap_or_else(|| panic!("{hits:?}"));
    assert!(rel["occurred_at"].as_str().unwrap().starts_with("2026-03-09"), "{rel}");
    let hits = recall(&app, json!({"query": q, "time_range": ["2026-11-01", "2999-01-01"]})).await;
    assert!(!hits.iter().any(|h| h["kind"] == "relation"), "not by when it was written: {hits:?}");
}

/// Cost of the memory semantic arm under a narrow range. Not a CI test: `BENCH_N=5000 cargo test
/// --test recall_ranking exact_scan_cost -- --ignored --nocapture`. Times the exact scan recall runs
/// under a range against the windowed HNSW search it replaced, on N memories with vectors, few of
/// them in range. Observed (debug build, machine loaded by other builds, N=5000, 50 in range): exact
/// scan 8.8 to 13.8 s; windowed KNN K=40 28.6 s, K=400 39 to 47 s; plain KNN K=400 10 to 55 s.
#[tokio::test]
#[ignore]
async fn exact_scan_cost() {
    let n: usize = std::env::var("BENCH_N").ok().and_then(|v| v.parse().ok()).unwrap_or(5000);
    let in_range: usize = std::env::var("BENCH_IN_RANGE").ok().and_then(|v| v.parse().ok()).unwrap_or(50);
    let app = TestApp::new().await;
    let first = write(&app, "Bench", "organisation", "seed", json!({})).await;
    let id = first["memory"]["id"].as_str().unwrap().to_string();
    let db = app.db().await;
    let mut seed: u64 = 7;
    let mut vector = || -> Vec<f32> {
        (0..1536)
            .map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((seed >> 33) as f32 / u32::MAX as f32) - 0.25
            })
            .collect()
    };
    let started = std::time::Instant::now();
    for chunk in (0..n).collect::<Vec<_>>().chunks(250) {
        let rows: Vec<Value> = chunk
            .iter()
            .map(|i| {
                let at = if *i < in_range { "2026-03-15T00:00:00Z" } else { "2024-06-01T00:00:00Z" };
                json!({"text": format!("bench fact {i}"), "embedding": vector(), "at": at})
            })
            .collect();
        db.test_raw()
            .query(format!(
                "LET $m = (SELECT * FROM ONLY {id}); FOR $r IN $rows {{ CREATE memory SET owner = $m.owner, vault = $m.vault, \
                 subject = $m.subject, type = 'world', text = $r.text, embedding = $r.embedding, embedding_spec = 'bench', \
                 occurred_at = <datetime>$r.at }};"
            ))
            .bind(("rows", rows))
            .await
            .unwrap()
            .check()
            .unwrap();
    }
    println!("seeded {n} memories ({in_range} in range) in {:?}", started.elapsed());
    let vault: Value = db.test_raw().query(format!("SELECT VALUE vault FROM ONLY {id}")).await.unwrap().take::<Option<Value>>(0).unwrap().unwrap();
    let _ = vault;
    let window = " AND type != \"observation\" AND (occurred_at ?? updated_at ?? created_at) >= $since AND (occurred_at ?? updated_at ?? created_at) <= $until";
    let exact = format!(
        "LET $v = (SELECT VALUE vault FROM ONLY {id}); SELECT id, vector::similarity::cosine(embedding, $vec) AS sim FROM memory \
         WHERE vault = $v AND embedding_spec = 'bench'{window} ORDER BY sim DESC LIMIT 40"
    );
    let knn_windowed = format!(
        "LET $v = (SELECT VALUE vault FROM ONLY {id}); SELECT id FROM memory \
         WHERE vault = $v AND embedding_spec = 'bench'{window} AND embedding <|40,80|> $vec"
    );
    let q = vector();
    for (name, sql, stmt) in [("exact scan (shipped)", &exact, 1), ("windowed knn (rejected)", &knn_windowed, 1), ("exact scan (shipped)", &exact, 1)] {
        let t = std::time::Instant::now();
        let mut res = db
            .test_raw()
            .query(sql.as_str())
            .bind(("vec", q.clone()))
            .bind(("since", surrealdb::types::Datetime::from(chrono::DateTime::parse_from_rfc3339("2026-03-01T00:00:00Z").unwrap().with_timezone(&chrono::Utc))))
            .bind(("until", surrealdb::types::Datetime::from(chrono::DateTime::parse_from_rfc3339("2026-03-31T23:59:59Z").unwrap().with_timezone(&chrono::Utc))))
            .await
            .unwrap();
        let rows: Vec<Value> = res.take(stmt).unwrap();
        println!("{name}: {} rows in {:?}", rows.len(), t.elapsed());
    }
}
