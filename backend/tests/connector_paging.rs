//! The connector detail page pages a source's records with `list` (no search term) and `search` (with
//! one), 50 at a time by offset. Every record must come back exactly once across the pages, and another
//! user's or another org's records of the same source never show up.

mod common;

use std::collections::HashSet;

use eunomia_backend::cache::search::{upsert, Envelope};
use eunomia_backend::models_user::User;
use eunomia_backend::tools::registry;
use serde_json::{json, Value};
use surrealdb::types::Datetime;

const SOURCE: &str = "heypocket";
const MINE: usize = 120;

async fn plant(app: &common::TestApp, user: &User, who: &str, n: usize) {
    let db = common::org_db(&app.state, user).await;
    let tie = Datetime::from(chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap().with_timezone(&chrono::Utc));
    for i in 0..n {
        // Two words on some titles only, so the term-hit fusion reorders between pool sizes.
        let title = if i % 2 == 0 { format!("{who} budget review {i}") } else { format!("{who} budget {i}") };
        let mut env: Envelope = serde_json::from_value(json!({
            "id": format!("{SOURCE}:recording:{who}-{i}"), "source": SOURCE, "type": "recording",
            "external_id": format!("{who}-{i}"), "title": title, "body_text": "weekly notes",
        }))
        .unwrap();
        // Most share one timestamp and a few have none: the sort alone cannot order them.
        if i % 10 != 0 {
            env.occurred_at = Some(tie);
        }
        common::sys(upsert(&db, &user.id, &env)).await.unwrap();
    }
    // Another source of the same user stays out of this source's pages.
    let other: Envelope = serde_json::from_value(json!({
        "id": format!("github:issue:{who}"), "source": "github", "type": "issue", "external_id": who, "title": format!("{who} budget review"),
    }))
    .unwrap();
    common::sys(upsert(&db, &user.id, &other)).await.unwrap();
}

async fn call(app: &common::TestApp, user: &User, name: &str, args: Value) -> Value {
    Box::pin(common::sys(registry::call(&app.state, user, name, args))).await.unwrap()
}

/// Every title on every page, following `has_more` from offset 0.
async fn all_pages(app: &common::TestApp, user: &User, name: &str, mut args: Value) -> Vec<String> {
    let mut titles = Vec::new();
    let mut offset = 0;
    loop {
        args["offset"] = json!(offset);
        let page = call(app, user, name, args.clone()).await;
        let hits = page["results"].as_array().unwrap_or_else(|| panic!("{page}"));
        titles.extend(hits.iter().map(|h| h["title"].as_str().unwrap().to_string()));
        if name == "list" {
            assert_eq!(page["total"], json!(MINE), "list total");
        }
        if !page["has_more"].as_bool().unwrap() {
            return titles;
        }
        assert_eq!(hits.len(), 50, "a full page before has_more ends");
        offset += 50;
    }
}

/// The full-text indexes fill `CONCURRENTLY`, so a fresh write is searchable a moment later: wait until
/// `term` finds `n` of `user`'s records.
async fn wait_indexed(app: &common::TestApp, user: &User, term: &str, n: usize) {
    let args = json!({ "query": term, "sources": [SOURCE], "mode": "keyword", "limit": 100 });
    for _ in 0..600 {
        let first = call(app, user, "search", args.clone()).await["results"].as_array().unwrap().len();
        let rest = if first == 100 {
            let mut more = args.clone();
            more["offset"] = json!(100);
            call(app, user, "search", more).await["results"].as_array().unwrap().len()
        } else {
            0
        };
        if first + rest >= n {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("{term:?} never found {n} records");
}

fn assert_each_once(titles: &[String], what: &str) {
    let unique: HashSet<&String> = titles.iter().collect();
    assert_eq!(unique.len(), titles.len(), "{what}: a record repeated across pages");
    assert_eq!(titles.len(), MINE, "{what}: every record once");
    assert!(titles.iter().all(|t| t.starts_with("alice ") && !t.ends_with("review")), "{what}: another user's, org's or source's record: {titles:?}");
}

#[tokio::test]
async fn source_records_page_through_every_record_once() {
    let app = common::TestApp::new().await;
    let alice = app.user.clone();
    // bob shares alice's org (his records sit in his personal vault); carol is in an org of her own.
    let bob = common::register(&app.state, "bob@example.com").await;
    let carol = common::register_personal(&app.state, "carol@example.com").await;
    plant(&app, &alice, "alice", MINE).await;
    plant(&app, &bob, "bob", 30).await;
    plant(&app, &carol, "carol", 30).await;
    wait_indexed(&app, &alice, "budget", MINE).await;
    wait_indexed(&app, &alice, "review", MINE / 2).await;
    wait_indexed(&app, &carol, "budget", 30).await;

    let list = all_pages(&app, &alice, "list", json!({ "filters": { "source": SOURCE }, "sort": "-occurred_at", "limit": 50 })).await;
    assert_each_once(&list, "list");

    let found = all_pages(&app, &alice, "search", json!({ "query": "budget review", "sources": [SOURCE], "mode": "keyword", "limit": 50 })).await;
    assert_each_once(&found, "search");

    // The other two see only their own.
    let bobs = call(&app, &bob, "list", json!({ "filters": { "source": SOURCE }, "limit": 200 })).await;
    assert_eq!(bobs["total"], json!(30));
    let carols = call(&app, &carol, "search", json!({ "query": "budget", "sources": [SOURCE], "mode": "keyword", "limit": 100 })).await;
    let carols = carols["results"].as_array().unwrap();
    assert_eq!(carols.len(), 30);
    assert!(carols.iter().all(|h| h["title"].as_str().unwrap().starts_with("carol ")));
}
