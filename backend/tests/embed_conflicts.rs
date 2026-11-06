//! Two writers set a vector on the same memory at once: after every `memory_write`, its quick embed
//! (`embed_soon`) and the fact's supersede check both save the new fact's vector. A commit conflict
//! between them must be retried, not returned. Returned, it failed the supersede check, which then
//! marked nothing (the e2e test "a fact a later one contradicts is superseded" timed out on it).

mod common;

use common::TestApp;
use eunomia_backend::embeddings::service as embeddings;
use serde_json::{json, Value};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_vector_writes_on_one_memory_never_return_a_conflict() {
    let mut settings = common::test_settings();
    settings.embeddings_backend = "stub".into();
    let app = TestApp::with_settings(settings).await;
    let org = app.state.org(&app.user.org).await.unwrap();
    for i in 0..4 {
        app.tool("memory_write", json!({"subject_name": "Riley Chen", "subject_kind": "person", "text": format!("Riley fact {i}")})).await;
    }
    let mut res = org.db.test_raw().query("SELECT <string> id AS id, text FROM memory").await.unwrap();
    let rows: Vec<Value> = res.take(0).unwrap();
    let spec = embeddings::spec_for(&org.db, &org.settings, &app.user.id).await.unwrap().tag();

    // Two writers per memory, as in production (embed_soon and the supersede check), over many
    // rounds. More writers than that can outlast the shared retry budget (tx::MAX_ATTEMPTS) on a
    // slow runner, which tests the budget, not the race this file guards.
    for round in 0..60 {
        let mut writers = tokio::task::JoinSet::new();
        for w in 0..2 {
            for row in &rows {
                let (db, spec) = (org.db.clone(), spec.clone());
                let id = eunomia_backend::rid::parse(row["id"].as_str().unwrap()).unwrap();
                let text = row["text"].as_str().unwrap().to_string();
                let vector = vec![(round * 2 + w) as f32 + 1.0; embeddings::dim()];
                writers.spawn(async move { embeddings::set_memory_embedding(&db, &id, vector, &spec, &text).await });
            }
        }
        while let Some(done) = writers.join_next().await {
            if let Err(e) = done.unwrap() {
                panic!("round {round}: a racing vector write returned {:?}: {}", e.code, e.message);
            }
        }
    }
}
