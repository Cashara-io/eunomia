//! Memory correctness through the registry and the HTTP routes against the real schema:
//! tombstoned records stay gone, observations follow their evidence, recall inputs are bounded,
//! search/list filters and paging are exact, merges are atomic and entity identity is
//! case-insensitive. Fixtures the public API cannot produce (several sources, a tombstone, a link,
//! an injected failure) are written straight to the org database.

mod common;

use common::TestApp;
use eunomia_backend::rid::RecordIdExt;
use eunomia_backend::tools::registry;
use serde_json::{json, Value};

/// The error code a failing tool call ends with: an `Err`, or the decorated `{"error", "code"}` body the
/// registry turns an error into.
async fn refused(app: &TestApp, name: &str, args: Value) -> String {
    let out = eunomia_backend::authz::as_system(registry::call(&app.state, &app.user, name, args.clone())).await;
    match out {
        Err(e) => e.code.as_str().to_string(),
        Ok(v) if v.get("error").is_some() => v["code"].as_str().unwrap_or_default().to_string(),
        Ok(v) => panic!("{name} {args} should fail but returned {v}"),
    }
}

fn ids(v: &Value, key: &str) -> Vec<String> {
    v[key].as_array().unwrap_or_else(|| panic!("{key} in {v}")).iter().map(|r| r["id"].as_str().unwrap().to_string()).collect()
}

fn observations(entity: &Value) -> Vec<Value> {
    entity["memory"].as_array().unwrap().iter().filter(|m| m["type"] == "observation").cloned().collect()
}

fn owner_key(app: &TestApp) -> String {
    eunomia_backend::rid::key_string(app.user.id.key()).unwrap()
}

/// A synced record for the test user, written straight to the cache.
#[allow(clippy::too_many_arguments)]
async fn plant(app: &TestApp, id: &str, source: &str, ty: &str, title: &str, body: &str, occurred: Option<&str>, payload: Value) {
    let db = app.db().await;
    db.test_raw()
        .query(
            "CREATE $k SET owner = $o, source = $s, type = $t, external_id = $x, title = $title, body_text = $body, \
             occurred_at = $at, url = '', payload = $p, content_hash = 'fx', ingested_at = time::now(), \
             updated_at = time::now(), deleted = false",
        )
        .bind(("k", surrealdb::types::RecordId::new("cache_record", format!("{}:{id}", owner_key(app)))))
        .bind(("o", app.user.id.clone()))
        .bind(("s", source.to_string()))
        .bind(("t", ty.to_string()))
        .bind(("x", id.to_string()))
        .bind(("title", title.to_string()))
        .bind(("body", body.to_string()))
        .bind(("at", occurred.map(|d| surrealdb::types::Datetime::from(d.parse::<chrono::DateTime<chrono::Utc>>().unwrap()))))
        .bind(("p", payload))
        .await
        .unwrap()
        .check()
        .unwrap();
}

async fn raw(app: &TestApp, sql: &str) {
    app.db().await.test_raw().query(sql).await.unwrap().check().unwrap();
}

fn record_rid(app: &TestApp, id: &str) -> String {
    format!("cache_record:`{}:{id}`", owner_key(app))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tombstoned_record_is_gone_from_get_links_and_recall() {
    let app = TestApp::new().await;
    plant(&app, "fx:t:gone", "bank", "bank.tx", "Lunch payment", "paid for lunch", None, json!({})).await;
    plant(&app, "fx:t:neighbour", "bank", "bank.tx", "Neighbour", "next door", None, json!({})).await;
    plant(&app, "fx:t:other", "bank", "bank.tx", "Other", "unrelated", None, json!({})).await;
    raw(&app, &format!("RELATE {}->linked_to->{} SET rel = 'fixture', origin = 'sync'", record_rid(&app, "fx:t:gone"), record_rid(&app, "fx:t:neighbour"))).await;
    app.tool(
        "memory_write",
        json!({"subject_name": "Tomas Tombstone", "subject_kind": "person", "text": "Tomas paid for lunch", "source_record_id": "fx:t:gone"}),
    )
    .await;

    let links = app.tool("links", json!({"id": "fx:t:neighbour"})).await;
    assert_eq!(links["links"][0]["target_id"], "fx:t:gone");
    let before = app.tool("recall", json!({"query": "Tomas Tombstone"})).await;
    assert!(ids(&before, "results").contains(&"fx:t:gone".to_string()), "the graph arm follows the fact to its source record: {before}");

    raw(&app, &format!("UPDATE {} SET deleted = true", record_rid(&app, "fx:t:gone"))).await;

    assert_eq!(app.tool("get", json!({"id": "fx:t:gone"})).await["error"], "not found");
    assert_eq!(app.tool("links", json!({"id": "fx:t:neighbour"})).await["links"], json!([]), "the link to a tombstone is hidden");
    assert_eq!(app.tool("links", json!({"id": "fx:t:gone"})).await["links"], json!([]), "a tombstone has no links");
    let after = app.tool("recall", json!({"query": "Tomas Tombstone"})).await;
    assert!(!ids(&after, "results").contains(&"fx:t:gone".to_string()), "{after}");
    assert!(!ids(&after, "results").contains(&"fx:t:neighbour".to_string()), "the graph arm does not hop through a tombstone: {after}");
    let texts: Vec<&str> = after["results"].as_array().unwrap().iter().map(|r| r["text"].as_str().unwrap()).collect();
    assert!(texts.contains(&"Tomas paid for lunch"), "the derived fact outlives its source record: {texts:?}");
    assert!(!ids(&app.tool("list", json!({"type": "bank.tx"})).await, "results").contains(&"fx:t:gone".to_string()));
    assert_eq!(app.tool("get", json!({"id": "fx:t:other"})).await["id"], "fx:t:other");
    assert_eq!(app.tool("get", json!({"id": "fx:t:neighbour"})).await["id"], "fx:t:neighbour");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn observations_follow_their_evidence() {
    let app = TestApp::new().await;
    let w = app.tool("memory_write", json!({"subject_name": "Alice Mover", "subject_kind": "person", "text": "Alice lives in Paris"})).await;
    let (alice, fact) = (w["entity"]["id"].as_str().unwrap().to_string(), w["memory"]["id"].as_str().unwrap().to_string());
    app.tool("memory_write", json!({"subject_name": "Alice Mover", "subject_kind": "person", "type": "observation", "text": "Alice lives in Paris"})).await;
    let obs = observations(&app.tool("entities_get", json!({"id": alice})).await);
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0]["status"], "fresh");
    let recalled = app.tool("recall", json!({"query": "Alice Mover Paris"})).await;
    assert!(ids(&recalled, "results").contains(&obs[0]["id"].as_str().unwrap().to_string()), "a fresh observation is recalled");

    // an edit makes it stale and recall leaves it out
    app.tool("memory_update", json!({"memory_id": fact, "text": "Alice lives in Berlin"})).await;
    let stale = observations(&app.tool("entities_get", json!({"id": alice})).await);
    assert_eq!(stale[0]["status"], "stale", "entities_get still shows it, marked stale");
    let recalled = app.tool("recall", json!({"query": "Alice Mover Paris"})).await;
    assert!(!ids(&recalled, "results").contains(&obs[0]["id"].as_str().unwrap().to_string()));
    let texts: Vec<&str> = recalled["results"].as_array().unwrap().iter().map(|r| r["text"].as_str().unwrap()).collect();
    assert!(texts.contains(&"Alice lives in Berlin"));

    // with no model key consolidation changes nothing: the observation stays stale
    app.tool("consolidate_observations", json!({"subject_id": alice})).await;
    assert_eq!(observations(&app.tool("entities_get", json!({"id": alice})).await)[0]["status"], "stale");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleting_facts_prunes_lineage_and_removes_a_sole_source_observation() {
    let app = TestApp::new().await;
    // the observation's lineage is what consolidation would write; set it directly
    let lineage = |ids: &[&str]| format!("[{}]", ids.join(", "));

    let only = app.tool("memory_write", json!({"subject_name": "Bea Single", "subject_kind": "person", "text": "Bea plays cello"})).await;
    app.tool("memory_write", json!({"subject_name": "Bea Single", "subject_kind": "person", "type": "observation", "text": "Bea is a cellist"})).await;
    raw(&app, &format!("UPDATE memory SET source_memories = {} WHERE type = 'observation' AND subject = {}", lineage(&[only["memory"]["id"].as_str().unwrap()]), only["entity"]["id"].as_str().unwrap())).await;
    app.tool("memory_delete", json!({"memory_id": only["memory"]["id"]})).await;
    assert!(observations(&app.tool("entities_get", json!({"id": only["entity"]["id"]})).await).is_empty(), "built only from the deleted fact: gone");

    let f1 = app.tool("memory_write", json!({"subject_name": "Cy Pair", "subject_kind": "person", "text": "Cy drinks tea"})).await;
    let f2 = app.tool("memory_write", json!({"subject_name": "Cy Pair", "subject_kind": "person", "text": "Cy runs marathons"})).await;
    let cy = f1["entity"]["id"].as_str().unwrap().to_string();
    let (m1, m2) = (f1["memory"]["id"].as_str().unwrap(), f2["memory"]["id"].as_str().unwrap());
    app.tool("memory_write", json!({"subject_name": "Cy Pair", "subject_kind": "person", "type": "observation", "text": "Cy drinks tea and runs"})).await;
    raw(&app, &format!("UPDATE memory SET source_memories = {}, status = 'fresh' WHERE type = 'observation' AND subject = {cy}", lineage(&[m1, m2]))).await;

    app.tool("memory_delete", json!({"memory_id": m1})).await;
    let pruned = observations(&app.tool("entities_get", json!({"id": cy})).await);
    assert_eq!(pruned.len(), 1);
    assert_eq!(pruned[0]["status"], "stale");
    assert_eq!(pruned[0]["source_memories"], json!([m2]), "lineage lists only surviving facts");
    let recalled = app.tool("recall", json!({"query": "Cy Pair tea"})).await;
    assert!(!ids(&recalled, "results").contains(&pruned[0]["id"].as_str().unwrap().to_string()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recall_rejects_unbounded_or_malformed_input() {
    let app = TestApp::new().await;
    for args in [
        json!({"query": "x", "limit": 0}),
        json!({"query": "x", "limit": 101}),
        json!({"query": "x", "limit": 1_000_000_000_000_000_i64}),
        json!({"query": "x", "max_tokens": 0}),
        json!({"query": "x", "max_tokens": 10_000_000}),
        json!({"query": "x".repeat(2001)}),
        json!({"query": "x", "time_range": ["2024-02-01", "2024-01-01"]}),
        json!({"query": "x", "time_range": ["last week", "2024-01-01"]}),
        json!({"query": "x", "time_range": ["2024-01-01"]}),
    ] {
        assert_eq!(refused(&app, "recall", args.clone()).await, "validation.invalid", "{args}");
    }
    let (status, body) = app.http("POST", "/api/tools/recall", Some(json!({"query": "x", "limit": 5000})), true).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["code"], "validation.invalid", "{body}");
    let ok = app.tool("recall", json!({"query": "x", "time_range": ["2020-01-01", "2030-01-01"], "limit": 100, "max_tokens": 100000})).await;
    assert!(ok["results"].is_array());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn graph_recall_matches_names_and_aliases_through_the_index() {
    let app = TestApp::new().await;
    let db = app.db().await;
    let vault = app.tool("vault_list", json!({})).await["results"][0]["id"].as_str().unwrap().to_string();
    db.test_raw()
        .query(format!("FOR $i IN 0..500 {{ CREATE person SET owner = {}, vault = {vault}, name = 'Filler Person ' + <string>$i; }};", app.user.id.to_string()))
        .await
        .unwrap()
        .check()
        .unwrap();
    app.tool("memory_write", json!({"subject_name": "Zygmunt Needle", "subject_kind": "person", "text": "Zygmunt repairs clocks"})).await;
    let started = std::time::Instant::now();
    let r = app.tool("recall", json!({"query": "what does zygmunt NEEDLE do?"})).await;
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    let texts: Vec<&str> = r["results"].as_array().unwrap().iter().map(|x| x["text"].as_str().unwrap()).collect();
    assert!(texts.contains(&"Zygmunt repairs clocks"), "{texts:?}");

    // an alias, also matched case-insensitively and with a possessive
    let e = app.tool("entities_search", json!({"query": "Zygmunt"})).await["results"][0]["id"].as_str().unwrap().to_string();
    app.tool("entity_update", json!({"entity_id": e, "aliases": ["The Clockman"]})).await;
    let r = app.tool("recall", json!({"query": "tell me about the clockman's work"})).await;
    let texts: Vec<&str> = r["results"].as_array().unwrap().iter().map(|x| x["text"].as_str().unwrap()).collect();
    assert!(texts.contains(&"Zygmunt repairs clocks"), "alias match: {texts:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn search_pushes_filters_into_candidates_and_pages_exactly() {
    let app = TestApp::new().await;
    for i in 0..60 {
        plant(&app, &format!("fx:crowd:{i}"), "crowd", "fx.note", &format!("zebrafjord crowd {i}"), "zebrafjord zebrafjord", None, json!({})).await;
    }
    plant(&app, "fx:rare:1", "rare", "fx.note", "an unrelated title", "a lone zebrafjord mention", None, json!({})).await;

    // the rare source's single, body-only match beats 60 better-ranked crowd hits
    let rare = app.tool("search", json!({"query": "zebrafjord", "sources": ["rare"], "mode": "keyword", "limit": 1})).await;
    assert_eq!(ids(&rare, "results"), vec!["fx:rare:1"]);
    assert_eq!(rare["has_more"], false);

    let mut seen = Vec::new();
    let mut more = Vec::new();
    for offset in [0, 25, 50] {
        let p = app.tool("search", json!({"query": "zebrafjord", "sources": ["crowd"], "mode": "keyword", "limit": 25, "offset": offset})).await;
        seen.extend(ids(&p, "results"));
        more.push(p["has_more"].as_bool().unwrap());
    }
    assert_eq!(seen.len(), 60);
    assert_eq!(seen.iter().collect::<std::collections::HashSet<_>>().len(), 60, "no repeats across pages");
    assert_eq!(more, vec![true, true, false]);
    assert_eq!(refused(&app, "search", json!({"query": "zebrafjord", "limit": 100, "offset": 950})).await, "validation.invalid");

    // body text has its own BM25 index: a body-only match is found, ranked by its BM25 score
    let body_only = app.tool("search", json!({"query": "lone", "mode": "keyword"})).await;
    assert_eq!(ids(&body_only, "results"), vec!["fx:rare:1"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dates_compare_as_datetimes_and_payload_filters_target_the_named_field() {
    let app = TestApp::new().await;
    plant(&app, "fx:d:1", "dated", "fx.dated", "d1", "", Some("2020-01-01T00:00:00Z"), json!({"category": "Groceries", "amount": {"cents": 500}})).await;
    plant(&app, "fx:d:2", "dated", "fx.dated", "d2", "", Some("2020-01-05T12:00:00Z"), json!({"category": "Travel", "amount": {"cents": 9000}})).await;
    plant(&app, "fx:d:3", "dated", "fx.dated", "d3", "", Some("2020-01-10T00:00:00Z"), json!({"category": "Groceries", "amount": {"cents": 2500}})).await;
    let titles = |filters: Value| {
        let app = &app;
        async move {
            let out = app.tool("list", json!({"type": "fx.dated", "filters": filters, "sort": "occurred_at"})).await;
            out["results"].as_array().unwrap().iter().map(|r| r["title"].as_str().unwrap().to_string()).collect::<Vec<_>>()
        }
    };
    assert_eq!(titles(json!({"occurred_at__gte": "2020-01-05"})).await, ["d2", "d3"]);
    assert_eq!(titles(json!({"occurred_at__gt": "2020-01-05"})).await, ["d2", "d3"], "12:00 on the 5th is after midnight");
    assert_eq!(titles(json!({"occurred_at__lt": "2020-01-05T12:00:00Z"})).await, ["d1"]);
    assert_eq!(titles(json!({"occurred_at__lte": "2020-01-05T12:00:00Z"})).await, ["d1", "d2"]);
    assert_eq!(titles(json!({"occurred_at__gte": "2020-01-02", "occurred_at__lte": "2020-01-09"})).await, ["d2"]);
    assert_eq!(titles(json!({"occurred_at__ne": "2020-01-01"})).await, ["d2", "d3"]);
    assert_eq!(titles(json!({"title__ne": "d2"})).await, ["d1", "d3"]);
    assert_eq!(titles(json!({"payload__category": "Groceries"})).await, ["d1", "d3"]);
    assert_eq!(titles(json!({"payload__amount__cents__gt": 1000})).await, ["d2", "d3"]);
    assert_eq!(titles(json!({"payload__amount__cents__gte": 2500})).await, ["d2", "d3"]);
    assert_eq!(titles(json!({"payload__amount__cents__lt": 2500})).await, ["d1"]);
    assert_eq!(titles(json!({"payload__amount__cents__lte": 2500})).await, ["d1", "d3"]);
    assert_eq!(titles(json!({"payload__category__ne": "Groceries"})).await, ["d2"]);
    for filters in [json!({"title__contains": "d"}), json!({"bogus": 1}), json!({"occurred_at__gte": "yesterday"}), json!({"payload": {"category": "x"}})] {
        assert_eq!(refused(&app, "list", json!({"type": "fx.dated", "filters": filters.clone()})).await, "validation.invalid", "{filters}");
    }

    let ranged = app.tool("search", json!({"query": "d2", "sources": ["dated"], "since": "2020-01-05", "until": "2020-01-06", "mode": "keyword"})).await;
    assert_eq!(ranged["results"][0]["title"], "d2");
    assert_eq!(refused(&app, "search", json!({"query": "d2", "since": "2020-02-01", "until": "2020-01-01"})).await, "validation.invalid");
    assert_eq!(refused(&app, "search", json!({"query": "d2", "since": "not a date"})).await, "validation.invalid");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_equivalent_names_land_on_one_entity() {
    let app = TestApp::new().await;
    let names = ["Concurrent Carl", "concurrent carl", "CONCURRENT CARL", " Concurrent Carl "];
    let mut tasks = Vec::new();
    for i in 0..8 {
        let state = app.state.clone();
        let user = app.user.clone();
        let name = names[i % names.len()];
        tasks.push(tokio::spawn(async move {
            let args = json!({"subject_name": name, "subject_kind": "person", "text": format!("Carl fact {i}")});
            common::sys(registry::call(&state, &user, "memory_write", args)).await.expect("write")
        }));
    }
    let mut entities = std::collections::HashSet::new();
    for t in tasks {
        entities.insert(t.await.unwrap()["entity"]["id"].as_str().unwrap().to_string());
    }
    assert_eq!(entities.len(), 1, "{entities:?}");
    let carl = entities.into_iter().next().unwrap();
    let detail = app.tool("entities_get", json!({"id": carl})).await;
    assert_eq!(detail["memory"].as_array().unwrap().len(), 8);
    assert_eq!(detail["name"].as_str().unwrap().to_lowercase(), "concurrent carl", "one spelling, trimmed");
    assert!(detail["aliases"].as_array().unwrap().is_empty());
    assert_eq!(refused(&app, "memory_write", json!({"subject_name": "   ", "subject_kind": "person", "text": "x"})).await, "validation.invalid");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failing_merge_rolls_back_and_a_merge_of_two_observed_entities_leaves_one_stale_observation() {
    let app = TestApp::new().await;
    let mk = |name: &'static str, fact: &'static str| {
        let app = &app;
        async move { app.tool("memory_write", json!({"subject_name": name, "subject_kind": "person", "text": fact})).await["entity"]["id"].as_str().unwrap().to_string() }
    };
    let winner = mk("Mergy Winner", "Winner fact").await;
    let loser = mk("Mergy Loser", "Loser fact").await;
    let friend = mk("Mergy Friend", "Friend fact").await;
    app.tool("memory_write", json!({"subject_name": "Mergy Winner", "subject_kind": "person", "type": "observation", "text": "Winner belief"})).await;
    app.tool("memory_write", json!({"subject_name": "Mergy Loser", "subject_kind": "person", "type": "observation", "text": "Loser belief"})).await;
    for (from, to, label) in [(&winner, &friend, "knows"), (&loser, &friend, "knows"), (&loser, &friend, "boom"), (&friend, &loser, "mentors")] {
        app.tool("code_relate", json!({"from_id": from, "to_id": to, "label": label})).await;
    }

    // inject a failure into copying the loser's unique edge
    raw(&app, "DEFINE EVENT inject ON relates_to WHEN $event = 'CREATE' AND $after.label = 'boom' THEN { THROW 'injected edge-copy failure' }").await;
    let code = refused(&app, "entity_merge", json!({"winner_id": winner, "loser_id": loser})).await;
    assert_ne!(code, "validation.invalid", "a database failure, not a bad request");
    raw(&app, "REMOVE EVENT inject ON relates_to").await;

    let loser_after = app.tool("entities_get", json!({"id": loser})).await;
    assert_eq!(loser_after["name"], "Mergy Loser");
    let mut labels: Vec<&str> = loser_after["relations"].as_array().unwrap().iter().map(|r| r["label"].as_str().unwrap()).collect();
    labels.sort();
    assert_eq!(labels, ["boom", "knows", "mentors"]);
    let mut loser_texts: Vec<&str> = loser_after["memory"].as_array().unwrap().iter().map(|m| m["text"].as_str().unwrap()).collect();
    loser_texts.sort();
    assert_eq!(loser_texts, ["Loser belief", "Loser fact"]);
    let winner_after = app.tool("entities_get", json!({"id": winner})).await;
    assert_eq!(winner_after["memory"].as_array().unwrap().len(), 2);
    assert_eq!(winner_after["relations"].as_array().unwrap().len(), 1);

    // the real merge
    app.tool("entity_merge", json!({"winner_id": winner, "loser_id": loser})).await;
    assert_eq!(refused(&app, "entities_get", json!({"id": loser})).await, "entity.not_found");
    let merged = app.tool("entities_get", json!({"id": winner})).await;
    assert!(merged["aliases"].as_array().unwrap().iter().any(|a| a == "Mergy Loser"));
    let obs = observations(&merged);
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0]["status"], "stale");
    assert!(obs[0]["text"].as_str().unwrap().contains("Winner belief") && obs[0]["text"].as_str().unwrap().contains("Loser belief"));
    let mut facts: Vec<&str> = merged["memory"].as_array().unwrap().iter().filter(|m| m["type"] == "world").map(|m| m["text"].as_str().unwrap()).collect();
    facts.sort();
    assert_eq!(facts, ["Loser fact", "Winner fact"]);
    let mut rels: Vec<String> = merged["relations"].as_array().unwrap().iter().map(|r| format!("{}:{}", r["direction"].as_str().unwrap(), r["label"].as_str().unwrap())).collect();
    rels.sort();
    assert_eq!(rels, ["in:mentors", "out:boom", "out:knows"]);
    let db = app.db().await;
    for q in [format!("SELECT count() AS n FROM relates_to WHERE in = {loser} OR out = {loser} GROUP ALL"), format!("SELECT count() AS n FROM memory WHERE subject = {loser} GROUP ALL")] {
        let mut res = db.test_raw().query(q).await.unwrap();
        let n: Option<i64> = res.take("n").unwrap();
        assert_eq!(n.unwrap_or(0), 0, "dangling rows");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn entity_search_orders_name_hits_by_vault_kind_and_name_then_fact_mentions_and_pages_them() {
    let app = TestApp::new().await;
    app.tool("vault_create", json!({"name": "Acme"})).await;
    let write = |name: &str, kind: &str, text: &str, vault: Option<&str>| {
        let mut args = json!({"subject_name": name, "subject_kind": kind, "text": text});
        if let Some(v) = vault {
            args["vault_id"] = json!(v);
        }
        args
    };
    for args in [
        write("Bob Quill", "person", "Bob writes", None),
        write("Dan", "person", "Dan edits", None),
        write("Quill Press", "organisation", "a printer", None),
        write("Carl", "person", "met Carl at the quill conference", None),
        write("Erin", "person", "Erin reads", None),
        write("Ann Quill", "person", "Ann prints", Some("Acme")),
    ] {
        app.tool("memory_write", args).await;
    }
    let dan = app.tool("entities_search", json!({"query": "Dan"})).await["results"][0]["id"].as_str().unwrap().to_string();
    app.tool("entity_update", json!({"entity_id": dan, "aliases": ["QUILLMASTER"]})).await;

    let names = |v: &Value| v["results"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    // personal vault first (people by name, then organisations), then the org vault, then fact mentions
    let all = app.tool("entities_search", json!({"query": "QUILL"})).await;
    assert_eq!(names(&all), ["Bob Quill", "Dan", "Quill Press", "Ann Quill", "Carl"], "{all}");
    assert_eq!((all["total"].as_u64(), all["has_more"].as_bool()), (Some(5), Some(false)));

    let first = app.tool("entities_search", json!({"query": "quill", "limit": 2})).await;
    assert_eq!(names(&first), ["Bob Quill", "Dan"]);
    assert_eq!((first["total"].as_u64(), first["has_more"].as_bool()), (Some(5), Some(true)));
    let last = app.tool("entities_search", json!({"query": "quill", "limit": 2, "offset": 3})).await;
    assert_eq!(names(&last), ["Ann Quill", "Carl"]);
    assert_eq!(last["has_more"], json!(false));
    let past = app.tool("entities_search", json!({"query": "quill", "offset": 9})).await;
    assert_eq!((names(&past).len(), past["total"].as_u64()), (0, Some(5)));

    let orgs = app.tool("entities_search", json!({"query": "quill", "kind": "organisation"})).await;
    assert_eq!(names(&orgs), ["Quill Press"]);
    let acme = app.tool("entities_search", json!({"query": "quill", "vault_id": "Acme"})).await;
    assert_eq!(names(&acme), ["Ann Quill"]);
    // an empty query lists everything in the same order
    let every = app.tool("entities_search", json!({"query": ""})).await;
    assert_eq!(names(&every), ["Bob Quill", "Carl", "Dan", "Erin", "Quill Press", "Ann Quill"]);
}
