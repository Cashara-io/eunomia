//! Two writers hammer the same entity through the registry. Every write must
//! succeed and the counters must add up exactly.

mod common;

use common::TestApp;
use eunomia_backend::tools::registry;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_memory_writes_keep_exact_counts() {
    const PER_TASK: usize = 200;
    let app = TestApp::new().await;
    app.tool("memory_write", json!({"subject_name":"Alice","subject_kind":"person","text":"Initial belief","type":"observation"})).await;

    let mut tasks = Vec::new();
    for t in 0..2 {
        let state = app.state.clone();
        let user = app.user.clone();
        tasks.push(tokio::spawn(async move {
            let mut errors = Vec::new();
            for i in 0..PER_TASK {
                for (kind, text) in [("world", format!("fact {t}-{i}")), ("observation", format!("belief {t}-{i}"))] {
                    let args = json!({"subject_name":"Alice","subject_kind":"person","text":text,"type":kind});
                    let out = common::sys(registry::call(&state, &user, "memory_write", args)).await;
                    match out {
                        Ok(v) if v.get("error").is_none() => {}
                        other => errors.push(format!("{other:?}")),
                    }
                }
            }
            errors
        }));
    }
    let mut errors = Vec::new();
    for t in tasks {
        errors.extend(t.await.unwrap());
    }
    assert!(errors.is_empty(), "{} writes failed, first: {:?}", errors.len(), errors.first());

    let db = app.db().await;
    let count = |q: &'static str| {
        let db = db.clone();
        async move {
            let mut res = db.test_raw().query(q).await.unwrap();
            let n: Option<i64> = res.take("n").unwrap();
            n.unwrap_or(0)
        }
    };
    let total = (2 * PER_TASK) as i64;
    assert_eq!(count("SELECT count() AS n FROM person GROUP ALL").await, 1, "one Alice, no duplicate entities");
    assert_eq!(count("SELECT count() AS n FROM memory WHERE type = 'world' GROUP ALL").await, total, "world memories");
    assert_eq!(count("SELECT count() AS n FROM memory WHERE type = 'observation' GROUP ALL").await, 1, "one observation");
    assert_eq!(
        count("SELECT version AS n FROM memory WHERE type = 'observation'").await,
        1 + 2 * total,
        "observation version includes the initial belief, every fact invalidation and every belief write"
    );
}
