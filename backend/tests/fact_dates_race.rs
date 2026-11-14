//! A supersede check must not hide an edit made while the model was deciding (audit T1, review round 2
//! item 4). The check reads the facts, waits on the model, then marks; the mark lands only if neither
//! fact changed since it was read. The striped in-process lock is switched off (`tx::LOCKS_OFF`,
//! docs/testing.md "Concurrency ceiling") so the edit and the mark meet at the database the way two
//! processes would; the switch is process-global, so this file is its own binary.

mod common;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use common::TestApp;
use eunomia_backend::entities::supersede;
use eunomia_backend::rid::RecordIdExt;
use serde_json::{json, Value};
use surrealdb::types::RecordId;
use tokio::sync::Notify;

struct LocksOff;
impl Drop for LocksOff {
    fn drop(&mut self) {
        eunomia_backend::tx::LOCKS_OFF.store(false, Ordering::SeqCst);
    }
}

/// The model gate: the supersede call signals `entered`, then waits for `release`.
#[derive(Clone)]
struct Gate {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

/// Embeddings (one constant vector per input) and a chat model that, for the supersede prompt, waits at
/// the gate and then answers "fact 1 is replaced by fact 2".
async fn stub(gate: Gate) -> String {
    use axum::{extract::State, routing::post, Json, Router};
    async fn embed(Json(body): Json<Value>) -> Json<Value> {
        let n = body["input"].as_array().map_or(1, Vec::len);
        let v = vec![0.01_f32; eunomia_backend::embeddings::service::dim()];
        Json(json!({ "data": (0..n).map(|i| json!({"index": i, "embedding": v})).collect::<Vec<_>>() }))
    }
    async fn chat(State(g): State<Gate>, Json(body): Json<Value>) -> Json<Value> {
        let prompt = body["messages"][0]["content"].as_str().unwrap_or_default();
        let content = if prompt.contains("\"superseded\"") {
            g.entered.notify_one();
            g.release.notified().await;
            json!({"superseded": [{"fact": 1, "by": 2}]})
        } else {
            json!({})
        };
        Json(json!({"choices": [{"message": {"content": content.to_string()}}]}))
    }
    let router = Router::new().route("/v1/embeddings", post(embed)).route("/v1/chat/completions", post(chat)).with_state(gate);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}/v1")
}

async fn write(app: &TestApp, text: &str) -> (RecordId, RecordId) {
    let out = app.tool("memory_write", json!({"subject_name": "Ada Lovelace", "subject_kind": "person", "text": text})).await;
    assert!(out.get("error").is_none(), "{out}");
    let p = |v: &Value| eunomia_backend::rid::parse(v.as_str().unwrap()).unwrap();
    (p(&out["entity"]["id"]), p(&out["memory"]["id"]))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_edit_made_while_the_model_decides_is_not_hidden_by_that_decision() {
    let gate = Gate { entered: Arc::new(Notify::new()), release: Arc::new(Notify::new()) };
    let mut settings = common::test_settings();
    settings.openai_api_key = Some("sk-test".into());
    settings.openai_base_url = stub(gate.clone()).await;
    settings.openai_chat_model = "test-model".into();
    let app = Arc::new(TestApp::with_settings(settings).await);
    eunomia_backend::tx::LOCKS_OFF.store(true, Ordering::SeqCst);
    let _restore = LocksOff;

    let (ada, old) = write(&app, "Ada works at Acme").await;
    let (_, new) = write(&app, "Ada works at Globex").await;
    // the check for the new fact reads both, then waits on the model
    let check = {
        let (app, ada, new) = (app.clone(), ada.clone(), new.clone());
        tokio::spawn(async move {
            let db = app.db().await;
            common::sys(supersede::check(&db, &app.state.settings, &app.user.id, &ada, &[new])).await
        })
    };
    gate.entered.notified().await;
    // meanwhile a person corrects the old fact
    let out = app.tool("memory_update", json!({"memory_id": old.to_string(), "text": "Ada works at Acme again since November"})).await;
    assert!(out.get("error").is_none(), "{out}");
    gate.release.notify_one();
    let marked = check.await.unwrap();
    assert!(marked.is_empty(), "the decision was about the fact before the edit: {marked:?}");
    let status: Vec<Value> =
        app.db().await.test_raw().query("SELECT VALUE status FROM $id").bind(("id", old.clone())).await.unwrap().take(0).unwrap();
    assert!(status.first().is_none_or(|s| s != "superseded"), "the edit stays current: {status:?}");
}
