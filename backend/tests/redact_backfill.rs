//! The one-off secret backfill (`jobs::redact_backfill`): plaintext secrets stored before writes were
//! redacted (seeded here with raw SQL, past the redacting write path) are replaced by markers in memories,
//! skills and audit summaries; other text is untouched; a second run changes nothing; a run stopped after
//! one batch resumes at its cursor.

mod common;

use std::future::Future;
use std::pin::Pin;

use common::TestApp;
use eunomia_backend::embeddings::service as embeddings;
use eunomia_backend::jobs::{kind, leader, redact_backfill};
use eunomia_backend::state::OrgState;
use serde_json::{json, Value};

type Boxed<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Boxed, like golden_tools.rs `snap`, so the long test frame holds a pointer per call, not a whole future.
fn sql<'a>(app: &'a TestApp, q: &'a str, binds: Value) -> Boxed<'a, Vec<Value>> {
    Box::pin(async move {
        let db = app.db().await;
        // a write's own follow-up (its embedding) can still be landing: retry a write conflict
        for _ in 0..20 {
            match db.test_raw().query(q).bind(binds.clone()).await.and_then(|r| r.check()) {
                Ok(mut res) => return res.take::<Vec<Value>>(0).unwrap(),
                Err(e) if e.to_string().contains("conflict") => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
                Err(e) => panic!("{q}: {e}"),
            }
        }
        panic!("{q}: kept conflicting")
    })
}

fn tool<'a>(app: &'a TestApp, name: &'a str, args: Value) -> Boxed<'a, Value> {
    Box::pin(app.tool(name, args))
}

fn run(org: &OrgState, max_batches: Option<usize>) -> Boxed<'_, redact_backfill::Progress> {
    Box::pin(async move { redact_backfill::run(org, max_batches).await.expect("backfill") })
}

fn tick(app: &TestApp) -> Boxed<'_, ()> {
    Box::pin(leader::tick(&app.state, 0)) // tick 0 runs the slow reconcilers
}

const PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASC\n-----END PRIVATE KEY-----";

fn github() -> String {
    format!("ghp_{}", "a1B2c3D4e5".repeat(4))
}

fn stripe() -> String {
    format!("sk_live_{}", "Zz9Yy8Xx7Ww6".repeat(2))
}

const JWT: &str = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backfill_redacts_stored_secrets_once_and_resumes() {
    let mut settings = common::test_settings();
    settings.embeddings_backend = "stub".into(); // vectors made offline, so the re-embed path runs
    let app = TestApp::with_settings(settings).await;
    let org = app.state.org(&app.user.org).await.unwrap();

    // what the leader queues before the backfill has run
    tick(&app).await;
    let mut res = app.control().test_raw().query("SELECT count() AS n FROM job WHERE kind = $k GROUP ALL").bind(("k", kind::REDACT_BACKFILL)).await.unwrap();
    assert_eq!(res.take::<Option<i64>>("n").unwrap(), Some(1), "one backfill job per org");

    // facts and an observation, then their text swapped for plaintext the way rows from before redaction hold it
    for (text, ty) in [("fact one", "world"), ("fact two", "world"), ("fact three", "experience"), ("Alice is a person.", "observation"), ("Alice likes tea", "world")] {
        tool(&app, "memory_write", json!({"subject_name": "Alice", "subject_kind": "person", "text": text, "type": ty})).await;
    }
    let secrets = [
        ("fact one", format!("deploy token\n{}\tfor CI", github())),
        ("fact two", "db is postgres://app:hunter2@db.internal:5432/main".to_string()),
        ("fact three", format!("pasted key\n{PEM}\nend of note")),
        ("Alice is a person.", "Alice uses AKIAIOSFODNN7EXAMPLE at work.".to_string()),
    ];
    // let each write's own background embedding land first, so it cannot race the swap below
    let waiting = || sql(&app, "SELECT VALUE id FROM memory WHERE embedding = NONE", json!({}));
    for _ in 0..100 {
        if waiting().await.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    for (old, new) in &secrets {
        sql(&app, "UPDATE memory SET text = $new WHERE text = $old", json!({"old": old, "new": new})).await;
    }
    // embedded as stored, secrets included, as an old install's rows were
    Box::pin(embeddings::embed_memory_backlog(&org.db, &org.settings, &app.user.id, 100)).await.unwrap();
    let embedded = |text: &'static str| sql(&app, "SELECT VALUE embedding != NONE FROM memory WHERE text = $t", json!({"t": text}));
    assert_eq!(embedded("Alice likes tea").await, vec![json!(true)]);
    let cached = || sql(&app, "SELECT VALUE id FROM embed_cache", json!({}));

    tool(&app, "skill_save", json!({"name": "ship-it", "description": "how to ship", "body": "run the deploy"})).await;
    sql(&app, "UPDATE skill SET description = $d, body = $b", json!({"d": format!("ship with {}", stripe()), "b": format!("steps:\n{}\nthen push", github())})).await;

    // an audit summary written before redaction: serialized JSON, the token right after a `\n` escape
    let summary = json!({"subject_name": "Alice", "text": format!("note\n{JWT}")}).to_string();
    sql(&app, "UPDATE audit_log SET args_summary = $s WHERE tool_name = 'skill_save'", json!({"s": summary})).await;

    // "crash" after one batch: the memories are done, the rest still waits at the saved cursor
    let cached_before = cached().await.len();
    let p = run(&org, Some(1)).await;
    // the memoized vectors of the 4 old memory texts went with them
    assert_eq!(cached().await.len(), cached_before - 4);
    assert_eq!((p.stage.as_str(), p.changed_memory, p.done_at.is_none()), ("memory", 4, true));
    assert!(p.cursor.is_some());
    let skill = sql(&app, "SELECT description, body, embedding_spec FROM skill", json!({})).await;
    assert!(skill[0]["body"].as_str().unwrap().contains(&github()), "not reached yet");

    // the resumed run finishes the walk
    let p = run(&org, None).await;
    assert_eq!((p.stage.as_str(), p.changed_memory, p.changed_skill, p.changed_audit_log), ("done", 4, 1, 1));
    assert!(p.done_at.is_some());

    let texts = sql(&app, "SELECT VALUE text FROM memory ORDER BY text", json!({})).await;
    let all = serde_json::to_string(&texts).unwrap();
    for leak in [github().as_str(), "hunter2", "MIIEvQIBADANBgkqhkiG9w0BAQEFAASC", "AKIAIOSFODNN7EXAMPLE"] {
        assert!(!all.contains(leak), "{leak} still stored: {all}");
    }
    for want in [
        "deploy token\n[redacted: github token]\tfor CI",
        "db is postgres://app:[redacted: url password]@db.internal:5432/main",
        "pasted key\n[redacted: private key]\nend of note",
        "Alice uses [redacted: aws access key] at work.",
        "Alice likes tea",
    ] {
        assert!(texts.contains(&json!(want)), "missing {want:?} in {all}");
    }
    // a rewritten memory drops its vector for the embed job; an untouched one keeps it
    assert_eq!(embedded("Alice likes tea").await, vec![json!(true)]);
    let cleared = sql(&app, "SELECT VALUE embedding = NONE FROM memory WHERE text CONTAINS '[redacted: '", json!({})).await;
    assert_eq!(cleared, vec![json!(true); 4]);

    let skill = sql(&app, "SELECT description, body, embedding_spec FROM skill", json!({})).await;
    assert_eq!(skill[0]["description"], "ship with [redacted: stripe key]");
    assert_eq!(skill[0]["body"], "steps:\n[redacted: github token]\nthen push");
    assert!(!skill[0]["embedding_spec"].is_null(), "the new description was embedded again");

    let audit = sql(&app, "SELECT VALUE args_summary FROM audit_log", json!({})).await;
    let audit = serde_json::to_string(&audit).unwrap();
    assert!(!audit.contains(JWT) && audit.contains("[redacted: jwt]"), "{audit}");
    assert!(audit.contains("Alice"), "the rest of the summary stays: {audit}");

    // one admin-visible audit event with counts, no text
    let mut res = app.control().test_raw().query("SELECT VALUE detail FROM audit_event WHERE action = 'redact.backfill'").await.unwrap();
    let details: Vec<String> = res.take(0).unwrap();
    assert_eq!(details, vec!["version=1 memory=4 skill=1 audit_log=1".to_string()]);

    // a second run (a redelivered job) changes nothing, and the leader stops queueing it
    let before = sql(&app, "SELECT id, text, version, updated_at FROM memory ORDER BY id", json!({})).await;
    let again = run(&org, None).await;
    assert_eq!((again.changed_memory, again.changed_skill, again.changed_audit_log, again.done_at), (4, 1, 1, p.done_at));
    assert_eq!(sql(&app, "SELECT id, text, version, updated_at FROM memory ORDER BY id", json!({})).await, before);
    app.control().test_raw().query("DELETE job WHERE kind = $k").bind(("k", kind::REDACT_BACKFILL)).await.unwrap().check().unwrap();
    tick(&app).await;
    let mut res = app.control().test_raw().query("SELECT count() AS n FROM job WHERE kind = $k GROUP ALL").bind(("k", kind::REDACT_BACKFILL)).await.unwrap();
    assert_eq!(res.take::<Option<i64>>("n").unwrap().unwrap_or(0), 0, "a finished org is not queued again");
    let mut res = app.control().test_raw().query("SELECT count() AS n FROM audit_event WHERE action = 'redact.backfill' GROUP ALL").await.unwrap();
    assert_eq!(res.take::<Option<i64>>("n").unwrap(), Some(1));
}
