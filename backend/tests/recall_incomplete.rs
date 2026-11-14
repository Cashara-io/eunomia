//! A recall that could not finish says so (`incomplete`), through MCP/registry, REST and `reflect`,
//! for an empty and a non-empty result; a complete recall carries no notice. Parts of recall are made
//! to hang with `cache::recall::test_hooks` (test-support builds), which is process-global, so this
//! file holds one test and runs its cases in order.

mod common;

use common::TestApp;
use eunomia_backend::cache::recall::test_hooks;
use eunomia_backend::embeddings::service as embeddings;
use serde_json::{json, Value};
use std::time::Duration;

/// An OpenAI-compatible `/v1/embeddings` stub: a word-hash vector per text, so the semantic arms run.
async fn stub_embeddings() -> String {
    use axum::{routing::post, Json, Router};
    async fn embed(Json(body): Json<Value>) -> Json<Value> {
        let input: Vec<String> = match &body["input"] {
            Value::String(s) => vec![s.clone()],
            v => serde_json::from_value(v.clone()).unwrap_or_default(),
        };
        let data: Vec<Value> = input
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let mut v = vec![0.0_f32; embeddings::dim()];
                for w in t.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 2) {
                    v[w.bytes().fold(0usize, |h, b| (h * 31 + b as usize) % 1000)] += 1.0;
                }
                let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
                json!({"index": i, "embedding": v.iter().map(|x| x / n).collect::<Vec<_>>()})
            })
            .collect();
        Json(json!({ "data": data }))
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, Router::new().route("/v1/embeddings", post(embed))).await.unwrap() });
    format!("http://{addr}/v1")
}

async fn app_with_embeddings() -> TestApp {
    let mut settings = common::test_settings();
    settings.openai_api_key = Some("sk-test".into());
    let app = TestApp::with_settings(settings).await;
    let base = stub_embeddings().await;
    let owner = eunomia_backend::rid::RecordIdExt::to_string(&app.user.id);
    let key = eunomia_backend::rid::key_string(&app.user.id.key).unwrap();
    app.db().await.test_raw().query(format!("UPSERT app_settings:`{key}` SET owner = {owner}, openai_base_url = '{base}'")).await.unwrap().check().unwrap();
    app
}

async fn recall(app: &TestApp, args: Value) -> Value {
    let out = app.tool("recall", args).await;
    assert!(out.get("error").is_none(), "{out}");
    out
}

fn notices(out: &Value) -> Vec<String> {
    out.get("incomplete").map(|v| serde_json::from_value(v.clone()).unwrap()).unwrap_or_default()
}

#[tokio::test]
async fn a_recall_that_could_not_finish_says_so() {
    let app = app_with_embeddings().await;
    let w = app.tool("memory_write", json!({"subject_name": "Acme Corp", "subject_kind": "organisation", "text": "Acme renewed the hosting contract"})).await;
    assert!(w.get("error").is_none(), "{w}");
    let db = app.db().await;
    eunomia_backend::authz::as_system(embeddings::embed_memory_backlog(&db, &app.state.settings, &app.user.id, 100)).await.unwrap();

    // complete: no notice, and the field is absent (same shape as before for old clients)
    test_hooks::set(&[], None);
    let out = recall(&app, json!({"query": "hosting contract"})).await;
    assert!(!out["results"].as_array().unwrap().is_empty(), "{out}");
    assert!(out.get("incomplete").is_none(), "{out}");
    let out = recall(&app, json!({"query": "nothing like this anywhere"})).await;
    assert!(out.get("incomplete").is_none(), "an empty but complete search is just empty: {out}");

    // an arm past its deadline, with a non-empty result (the text search still finds it)
    test_hooks::set(&["memory_semantic"], Some(Duration::from_millis(300)));
    let out = recall(&app, json!({"query": "hosting contract"})).await;
    assert!(!out["results"].as_array().unwrap().is_empty(), "{out}");
    assert_eq!(notices(&out), ["semantic search over memories timed out"], "{out}");

    // ... and with an empty result: the only arm that could find it timed out
    test_hooks::set(&["memory_text", "memory_semantic", "graph"], Some(Duration::from_millis(300)));
    let out = recall(&app, json!({"query": "hosting contract", "time_range": ["2000-01-01", "2999-01-01"]})).await;
    assert!(out["results"].as_array().unwrap().is_empty(), "{out}");
    let n = notices(&out);
    for want in ["text search over memories timed out", "semantic search over memories timed out", "entity graph search timed out"] {
        assert!(n.contains(&want.to_string()), "{want}: {n:?}");
    }

    // the same through REST
    let (code, body) = app.http("POST", "/api/tools/recall", Some(json!({"query": "hosting contract"})), true).await;
    assert!(code.is_success(), "{code} {body}");
    let rest = if body.get("results").is_some() { body.clone() } else { body["result"].clone() };
    assert!(!notices(&rest).is_empty(), "REST carries the notice: {body}");

    // reflect: not "no information"
    let out = app.tool("reflect", json!({"query": "hosting contract"})).await;
    assert!(out["answer"].as_str().unwrap_or_default().contains("could not finish"), "{out}");
    assert!(!notices(&out).is_empty(), "{out}");

    // the query's embedding timing out
    test_hooks::set(&["embedding"], Some(Duration::from_millis(300)));
    let out = recall(&app, json!({"query": "hosting contract"})).await;
    assert_eq!(notices(&out), ["semantic search skipped: embedding the query timed out"], "{out}");

    // no embedding provider is a supported setup (words and links), so its search is complete: no
    // notice, whether something is found or not, and reflect's plain answer
    test_hooks::set(&[], None);
    let plain = TestApp::new().await;
    plain.tool("memory_write", json!({"subject_name": "Acme Corp", "subject_kind": "organisation", "text": "Acme renewed the hosting contract"})).await;
    let out = recall(&plain, json!({"query": "hosting contract"})).await;
    assert!(!out["results"].as_array().unwrap().is_empty());
    assert!(out.get("incomplete").is_none(), "no provider is not incomplete: {out}");
    let out = recall(&plain, json!({"query": "nothing like this anywhere"})).await;
    assert!(out["results"].as_array().unwrap().is_empty() && out.get("incomplete").is_none(), "{out}");
    let out = plain.tool("reflect", json!({"query": "nothing like this anywhere"})).await;
    assert_eq!(out["answer"], "No relevant memories found.", "{out}");
    assert!(out.get("incomplete").is_none(), "{out}");
    let out = recall(&plain, json!({"query": "", "time_range": ["2000-01-01", "2999-01-01"]})).await;
    assert!(out.get("incomplete").is_none(), "{out}");
}
