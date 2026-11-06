//! Database round trips per request on the hot MCP and REST paths, counted at the one place every
//! statement goes out (`store::SENT`). Its own test binary with one test, so nothing else in the
//! process sends statements while it counts. Prints the table; the asserts are ceilings, so a change
//! that adds a round trip to a hot path has to raise one on purpose.
//!   cargo test --release --test round_trips -- --nocapture

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use common::{http, TestApp};
use eunomia_backend::store::SENT;
use serde_json::{json, Value};

/// Statements one request sends, including what it spawns (given a moment to land).
async fn count(app: &TestApp, body: Value) -> u64 {
    let before = SENT.load(Ordering::SeqCst);
    let ((status, reply), _) = http(&app.router, "POST", "/mcp", Some(body), Some(&app.token), None).await;
    assert_eq!(status, 200, "{reply}");
    assert!(reply.get("error").is_none() && reply["result"]["isError"] != json!(true), "{reply}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    SENT.load(Ordering::SeqCst) - before
}

fn call(name: &str, args: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": args } })
}

#[tokio::test]
async fn round_trips_per_request() {
    let app = TestApp::new().await;
    let names = ["Ada Okafor", "Bruno Smith", "Chen Garcia", "Dara Ito"];
    for i in 0..40 {
        let args = json!({ "subject_name": names[i % 4], "subject_kind": "person", "text": format!("{} owns the project deadline {i}", names[i % 4]) });
        app.tool("memory_write", args).await;
    }

    let cases: Vec<(&str, Value)> = vec![
        ("tools/list", json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })),
        ("recall", call("recall", json!({ "query": "Ada project deadline", "limit": 10 }))),
        ("entities_search", call("entities_search", json!({ "query": "ada", "limit": 20 }))),
        ("entities_search fact", call("entities_search", json!({ "query": "deadline", "limit": 20 }))),
        ("search", call("search", json!({ "query": "deadline" }))),
        ("memory_write known", call("memory_write", json!({ "subject_name": "Ada Okafor", "subject_kind": "person", "text": "Ada likes tea" }))),
    ];
    let mut rows = Vec::new();
    for (label, body) in &cases {
        // the first call warms whatever the process caches (org handle, credential); the second is the steady state
        let cold = count(&app, body.clone()).await;
        let warm = count(&app, body.clone()).await;
        rows.push((*label, cold, warm));
    }
    let new_subject = |n: u32| call("memory_write", json!({ "subject_name": format!("Emil Novak {n}"), "subject_kind": "person", "text": "Emil joined" }));
    count(&app, new_subject(1)).await;
    let fresh = count(&app, new_subject(2)).await;
    rows.push(("memory_write new", fresh, fresh));

    println!("\n{:<22} {:>5} {:>5}", "request", "cold", "warm");
    for (label, cold, warm) in &rows {
        println!("{label:<22} {cold:>5} {warm:>5}");
    }
    // Before the hot-path work (v2.0.2): 4, 14, 14, 14, 6, 12, 14 (the four of every request were
    // authentication). A verified token now costs nothing until its cache entry ages out.
    // Every tool call also sends one `tool_usage` upsert (spawned after the call, off the request path), the +1 on each.
    let ceilings = [
        ("tools/list", 0),
        ("recall", 7),
        ("entities_search", 3),
        ("entities_search fact", 3),
        ("search", 4), // + the vault membership lookup since search covers several vaults (#93)
        // + 1 off the request path: the post-write embed checks whether the writer has a model (#65)
        // + 1 per change log row: the memory, and for a new subject its entity too (migration 0023)
        ("memory_write known", 9),
        ("memory_write new", 12),
    ];
    for ((label, _, warm), (name, most)) in rows.iter().zip(ceilings) {
        assert_eq!(*label, name);
        assert!(*warm <= most, "{label}: {warm} round trips, at most {most} expected");
    }
}
