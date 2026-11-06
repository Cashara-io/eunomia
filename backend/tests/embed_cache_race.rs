//! Two callers embed the same new text at once: after every `memory_write`, its quick embed
//! (`embed_soon`) and the fact's supersede check both embed the new fact, and both put the same
//! `embed_cache` row. That put's commit conflict must not fail the embedding. It failed the supersede
//! check, which then marked nothing (e2e "a fact a later one contradicts is superseded", CI run
//! 42168772196: "supersede check failed ...: The write conflicted with another").

mod common;

use common::TestApp;
use eunomia_backend::embeddings::service as embeddings;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_embeds_of_one_text_never_fail_on_the_cache() {
    let mut settings = common::test_settings();
    settings.embeddings_backend = "stub".into();
    let app = TestApp::with_settings(settings).await;
    let org = app.state.org(&app.user.org).await.unwrap();

    for round in 0..40 {
        // a text no earlier round cached, so every caller misses the cache and puts the row
        let texts = vec![format!("Riley was granted access to build server {round}")];
        let mut callers = tokio::task::JoinSet::new();
        for _ in 0..2 {
            let (db, settings, owner, texts) = (org.db.clone(), org.settings.clone(), app.user.id.clone(), texts.clone());
            callers.spawn(async move { embeddings::embed(&db, &settings, &texts, Some(&owner)).await });
        }
        while let Some(done) = callers.join_next().await {
            match done.unwrap() {
                Ok(v) => assert_eq!(v.len(), 1),
                Err(e) => panic!("round {round}: a racing embed returned {:?}: {}", e.code, e.message),
            }
        }
    }
}
