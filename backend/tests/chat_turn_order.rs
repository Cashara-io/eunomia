//! Database-level fencing, without any process-local mutex or model provider.
mod common;

use common::TestApp;
use eunomia_backend::rid::RecordIdExt;
use std::future::IntoFuture;
use eunomia_backend::{chat::service, pool::OrgDb, store::app as q};
use serde_json::{json, Value};
use surrealdb::types::RecordId;

async fn claim(db: &OrgDb, owner: &RecordId, thread: &RecordId, token: &str) -> Vec<RecordId> {
    eunomia_backend::tx::with_retry(|| async {
        q::CHAT_TURN_CLAIM.on(db).bind(("owner", owner.clone())).bind(("thread_id", thread.clone()))
            .bind(("turn_key", token.to_string())).bind(("lease", "30s")).await?.take(0)
    }).await.unwrap()
}

async fn append(db: &OrgDb, owner: &RecordId, thread: &RecordId, token: &str, messages: Value) -> surrealdb::Result<()> {
    q::CHAT_TURN_APPEND.on(db).bind(("owner", owner.clone())).bind(("thread_id", thread.clone()))
        .bind(("turn_key", token.to_string())).bind(("lease", "30s")).bind(("messages", messages))
        .await?.check()?;
    Ok(())
}

#[tokio::test]
async fn expired_claim_fences_old_writes_renewals_and_releases() {
    let app = TestApp::new().await;
    let db = app.db().await;
    let thread = service::create_thread(&db, &app.user.id, None).await.unwrap();
    let id = eunomia_backend::rid::parse(&thread.id).unwrap();
    assert_eq!(claim(&db, &app.user.id, &id, "a").await.len(), 1);
    assert!(claim(&db, &app.user.id, &id, "b").await.is_empty());
    // A process dies or stops renewing. Database time, rather than the application's clock, wins.
    db.test_raw().query("UPDATE $id SET turn_until = time::now() - 1s")
        .bind(("id", id.clone())).await.unwrap().check().unwrap();
    assert_eq!(claim(&db, &app.user.id, &id, "b").await.len(), 1);
    let renewed: Vec<RecordId> = q::CHAT_TURN_RENEW.on(&db).bind(("owner", app.user.id.clone()))
        .bind(("thread_id", id.clone())).bind(("turn_key", "a")).bind(("lease", "30s"))
        .await.unwrap().take(0).unwrap();
    assert!(renewed.is_empty());
    assert!(append(&db, &app.user.id, &id, "a", json!([{"role":"assistant","content":"late answer"}])).await.is_err());
    q::CHAT_TURN_RELEASE.on(&db).bind(("owner", app.user.id.clone())).bind(("thread_id", id.clone()))
        .bind(("turn_key", "a")).await.unwrap().check().unwrap();
    assert!(claim(&db, &app.user.id, &id, "c").await.is_empty(), "a stale release cannot release B");
    append(&db, &app.user.id, &id, "b", json!([{"role":"user","content":"new turn"}])).await.unwrap();
    let history = service::history(&db, &app.user.id, &id).await.unwrap().unwrap();
    assert_eq!(history.len(), 1); assert_eq!(history[0].content, "new turn");
}

#[tokio::test]
async fn persisted_clock_orders_batches_even_when_wall_clock_is_behind() {
    let app = TestApp::new().await;
    let db = app.db().await;
    let thread = service::create_thread(&db, &app.user.id, None).await.unwrap();
    let id = eunomia_backend::rid::parse(&thread.id).unwrap();
    // Pre-upgrade history can be ahead of the database's clock. The first claim seeds its clock.
    db.test_raw().query("CREATE chat_message SET owner = $owner, thread_id = $id, role = 'user', content = 'old', created_at = d'2999-01-01T00:00:00Z'")
        .bind(("owner", app.user.id.clone())).bind(("id", id.clone())).await.unwrap().check().unwrap();
    claim(&db, &app.user.id, &id, "a").await;
    append(&db, &app.user.id, &id, "a", json!([
        {"role":"assistant","content":"", "tool_calls":[{"id":"call","type":"function","function":{"name":"ontology_get","arguments":"{}"}}]},
        {"role":"tool","content":"{}","tool_call_id":"call"}
    ])).await.unwrap();
    append(&db, &app.user.id, &id, "a", json!([{"role":"assistant","content":"answer"}])).await.unwrap();
    let history = service::history(&db, &app.user.id, &id).await.unwrap().unwrap();
    assert_eq!(history.iter().map(|m| m.role.as_str()).collect::<Vec<_>>(), ["user", "assistant", "tool", "assistant"]);
    assert!(history.windows(2).all(|pair| pair[0].created_at < pair[1].created_at), "{history:?}");
}

#[tokio::test]
async fn racing_claims_have_one_database_winner() {
    let app = TestApp::new().await;
    let db = app.db().await;
    let thread = service::create_thread(&db, &app.user.id, None).await.unwrap();
    let id = eunomia_backend::rid::parse(&thread.id).unwrap();
    let (a, b) = tokio::join!(claim(&db, &app.user.id, &id, "a"), claim(&db, &app.user.id, &id, "b"));
    assert_eq!(a.len() + b.len(), 1);
    let other = service::create_thread(&db, &app.user.id, None).await.unwrap();
    assert_eq!(claim(&db, &app.user.id, &eunomia_backend::rid::parse(&other.id).unwrap(), "c").await.len(), 1,
        "another thread must remain independent");
}

async fn wait_for_file(path: &std::path::Path) {
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        while !path.exists() { tokio::time::sleep(std::time::Duration::from_millis(20)).await; }
    }).await.expect("process barrier timed out");
}

/// CI uses the existing disposable RocksDB server. Local runs skip this rather than use any server.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_turns_across_processes() {
    if std::env::var("TEST_SURREAL_URL").is_err() {
        eprintln!("chat_turns_across_processes: real-server check deferred to CI");
        return;
    }
    let app = TestApp::new().await;
    let db = app.db().await;
    let thread = service::create_thread(&db, &app.user.id, None).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut children = Vec::new();
    for n in 0..3 {
        children.push(tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["chat_turns_across_processes_child", "--exact", "--ignored", "--nocapture"])
            .env("CHAT_TURN_NS", &app.state.settings.surreal_ns).env("CHAT_TURN_ORG", app.user.org.key())
            .env("CHAT_TURN_OWNER", app.user.id.to_string()).env("CHAT_TURN_THREAD", &thread.id)
            .env("CHAT_TURN_DIR", dir.path()).env("CHAT_TURN_CHILD", n.to_string())
            .kill_on_drop(true).spawn().unwrap());
    }
    for n in 0..3 { wait_for_file(&dir.path().join(format!("{n}.ready"))).await; }
    std::fs::write(dir.path().join("go"), "").unwrap();
    for mut child in children {
        assert!(tokio::time::timeout(std::time::Duration::from_secs(60), child.wait()).await.unwrap().unwrap().success());
    }
    let id = eunomia_backend::rid::parse(&thread.id).unwrap();
    let rows: Vec<service::MessageOut> = service::history(&db, &app.user.id, &id).await.unwrap().unwrap();
    assert_eq!(rows.len(), 60);
    assert!(rows.windows(2).all(|pair| pair[0].created_at < pair[1].created_at));
    for pair in rows.as_chunks::<2>().0 {
        assert_eq!((&*pair[0].role, &*pair[1].role), ("user", "assistant"));
        assert_eq!(pair[0].content, pair[1].content, "two processes interleaved turns");
    }
}

#[tokio::test]
#[ignore = "spawned only by chat_turns_across_processes on CI's disposable server"]
async fn chat_turns_across_processes_child() {
    let Ok(ns) = std::env::var("CHAT_TURN_NS") else { return };
    let settings = eunomia_backend::config::Settings { surreal_ns: ns, ..common::test_settings() };
    let state = eunomia_backend::state::AppState::attach(&settings, common::engine_config()).await.unwrap();
    let org = eunomia_backend::pool::OrgId::parse(&std::env::var("CHAT_TURN_ORG").unwrap()).unwrap();
    let db = state.org(&org).await.unwrap().db;
    let owner = eunomia_backend::rid::parse(&std::env::var("CHAT_TURN_OWNER").unwrap()).unwrap();
    let thread = eunomia_backend::rid::parse(&std::env::var("CHAT_TURN_THREAD").unwrap()).unwrap();
    let dir = std::path::PathBuf::from(std::env::var("CHAT_TURN_DIR").unwrap());
    let n = std::env::var("CHAT_TURN_CHILD").unwrap();
    std::fs::write(dir.join(format!("{n}.ready")), "").unwrap();
    wait_for_file(&dir.join("go")).await;
    for turn in 0..10 {
        let token = format!("worker {n} turn {turn}");
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while claim(&db, &owner, &thread, &token).await.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        append(&db, &owner, &thread, &token, json!([{"role":"user", "content":token}])).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        append(&db, &owner, &thread, &token, json!([{"role":"assistant", "content":token}])).await.unwrap();
        q::CHAT_TURN_RELEASE.on(&db).bind(("owner", owner.clone())).bind(("thread_id", thread.clone()))
            .bind(("turn_key", token)).await.unwrap().check().unwrap();
    }
}

#[tokio::test]
async fn delayed_submitted_transaction_cannot_write_after_release_and_next_claim() {
    let app = TestApp::new().await;
    let db = app.db().await;
    let thread = service::create_thread(&db, &app.user.id, None).await.unwrap();
    let id = eunomia_backend::rid::parse(&thread.id).unwrap();
    claim(&db, &app.user.id, &id, "a").await;
    let sql = q::CHAT_TURN_APPEND.sql.replacen("BEGIN TRANSACTION;", "BEGIN TRANSACTION; SLEEP 500ms;", 1);
    // SLEEP is test fault injection and requires the harness root session, never the app user.
    let root = app.state.provisioner.as_ref().unwrap().session_on(&app.user.org.db_name()).await.unwrap();
    let mut old = std::pin::pin!(root.query(sql).bind(("owner", app.user.id.clone()))
        .bind(("thread_id", id.clone())).bind(("turn_key", "a")).bind(("lease", "30s"))
        .bind(("messages", json!([{"role":"assistant","content":"late answer"}]))).into_future());
    // Poll the SDK future to submit it, then leave it in flight while the next turn takes over.
    let submitted = futures::poll!(&mut old);
    assert!(submitted.is_pending(), "delayed query ended before submission: {submitted:?}");
    q::CHAT_TURN_RELEASE.on(&db).bind(("owner", app.user.id.clone())).bind(("thread_id", id.clone()))
        .bind(("turn_key", "a")).await.unwrap().check().unwrap();
    assert_eq!(claim(&db, &app.user.id, &id, "b").await.len(), 1);
    append(&db, &app.user.id, &id, "b", json!([{"role":"user","content":"new turn"}])).await.unwrap();
    assert!(old.await.and_then(|res| res.check()).is_err(), "a submitted stale transaction must conflict or lose its token");
    let history = service::history(&db, &app.user.id, &id).await.unwrap().unwrap();
    assert_eq!(history.len(), 1); assert_eq!(history[0].content, "new turn");
}
