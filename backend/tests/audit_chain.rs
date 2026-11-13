//! The hash-chained security log (compliance PR03, docs/audit-chain.md) against the real database:
//! durable delivery (a cancelled caller, a failed write, a crash between write and seal, all
//! replayed), one unbroken chain under concurrent writers (with and without the in-process lock),
//! tamper detection (edit, delete, reorder), retention that leaves a checkpoint, a metadata-only
//! export that verifies offline, and access reviews.
//!
//! The fault switches (`chain::FAIL_WRITES`, `chain::SKIP_INLINE_SEAL`, `tx::LOCKS_OFF`) are
//! process-global, so every test here takes [`serial`].

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use eunomia_backend::audit::{self, chain, Event};
use eunomia_backend::authz::Actor;
use eunomia_backend::pool::ControlDb;
use serde_json::{json, Value};

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn serial() -> tokio::sync::MutexGuard<'static, ()> {
    let g = SERIAL.lock().await;
    chain::FAIL_WRITES.store(0, std::sync::atomic::Ordering::SeqCst);
    chain::SKIP_INLINE_SEAL.store(false, std::sync::atomic::Ordering::SeqCst);
    eunomia_backend::tx::LOCKS_OFF.store(false, std::sync::atomic::Ordering::SeqCst);
    g
}

async fn emit(db: &ControlDb, user: &surrealdb::types::RecordId, action: &str, detail: &str) {
    let actor = Actor { kind: "user", id: user.to_string_lossy() };
    audit::record(db, Event { user: Some(user), actor: &actor, action, target: "", outcome: "ok", detail }).await;
}

trait Lossy {
    fn to_string_lossy(&self) -> String;
}
impl Lossy for surrealdb::types::RecordId {
    fn to_string_lossy(&self) -> String {
        chain::rid(self)
    }
}

async fn sql(db: &ControlDb, q: &str, vars: Value) -> Vec<Value> {
    let mut res = db.test_raw().query(q).bind(vars).await.expect("query");
    res.take::<Vec<Value>>(0).unwrap_or_default()
}

async fn rows(app: &common::TestApp) -> Vec<Value> {
    sql(app.control(), "SELECT seq, pending, action FROM audit_event WHERE ledger = $l ORDER BY seq", json!({ "l": ledger(app) })).await
}

fn ledger(app: &common::TestApp) -> String {
    chain::org_ledger(&app.user.org)
}

async fn verify(app: &common::TestApp) -> chain::AuditVerification {
    chain::verify(app.control(), &ledger(app)).await.expect("verify")
}

/// Waits (bounded) for `n` events with this action to land.
async fn landed(db: &ControlDb, action: &str, n: usize) -> bool {
    for _ in 0..200 {
        if sql(db, "SELECT id FROM audit_event WHERE action = $a", json!({ "a": action })).await.len() >= n {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

#[tokio::test]
async fn every_event_is_chained_in_its_own_orgs_ledger() {
    let _g = serial().await;
    let app = common::TestApp::new().await;
    let other = common::register_personal(&app.state, "other@example.com").await;
    assert_ne!(other.org, app.user.org);
    for i in 0..5 {
        emit(app.control(), &app.user.id, "test.a", &format!("n={i}")).await;
        emit(app.control(), &other.id, "test.b", &format!("n={i}")).await;
    }
    // a real route writes too, through the same path
    let (status, _) = app.http("POST", "/api/auth/tokens", Some(json!({ "name": "ci" })), true).await;
    assert!(status.is_success(), "{status}");
    let mine = rows(&app).await;
    assert!(mine.iter().all(|r| r["pending"].is_null()), "everything sealed inline: {mine:?}");
    let seqs: Vec<i64> = mine.iter().map(|r| r["seq"].as_i64().unwrap()).collect();
    assert_eq!(seqs, (1..=seqs.len() as i64).collect::<Vec<_>>(), "one gapless sequence");
    assert!(mine.iter().all(|r| r["action"] != "test.b"), "another org's events never enter this chain");
    assert!(mine.iter().any(|r| r["action"] == "auth.token_create"));
    let theirs = chain::verify(app.control(), &chain::org_ledger(&other.org)).await.unwrap();
    assert!(theirs.ok, "{theirs:?}");
    assert!(theirs.checked >= 5, "{theirs:?}");
    let v = verify(&app).await;
    assert!(v.ok, "{v:?}");
    assert_eq!(v.checked as usize, seqs.len());
    assert_eq!(v.anchor.as_ref().unwrap().kind, "genesis");
    // an event with no org (a sign-in with an unknown email) goes to the instance ledger
    let ((status, _), _) =
        common::http(&app.router, "POST", "/api/auth/login", Some(json!({ "email": "nobody@example.com", "password": "x" })), None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let inst = chain::verify(app.control(), chain::INSTANCE).await.unwrap();
    assert!(inst.ok && inst.head_seq.unwrap_or(0) >= 1, "{inst:?}");
}

#[tokio::test]
async fn a_cancelled_caller_does_not_lose_its_event() {
    let _g = serial().await;
    let app = common::TestApp::new().await;
    // the caller's future is dropped after its first poll, while the write is in flight
    let control = app.control().clone();
    let user = app.user.id.clone();
    let cut = tokio::time::timeout(Duration::ZERO, async move { emit(&control, &user, "test.cancelled", "").await }).await;
    assert!(cut.is_err(), "the caller was cancelled before the write finished");
    assert!(landed(app.control(), "test.cancelled", 1).await, "the event landed anyway");
    let v = verify(&app).await;
    assert!(v.ok && v.pending == 0, "{v:?}");
}

#[tokio::test]
async fn a_failed_write_is_queued_and_replayed() {
    let _g = serial().await;
    let app = common::TestApp::new().await;
    let before = chain::queued();
    chain::FAIL_WRITES.store(1, std::sync::atomic::Ordering::SeqCst);
    emit(app.control(), &app.user.id, "test.refused_write", "").await;
    assert_eq!(chain::queued(), before + 1, "the event waits in the replay queue");
    assert!(sql(app.control(), "SELECT id FROM audit_event WHERE action = 'test.refused_write'", json!({})).await.is_empty());
    // the scheduler's replay pass delivers it and seals it
    let r = chain::replay(app.control()).await.unwrap();
    assert!(r.written >= 1, "{r:?}");
    assert_eq!(chain::queued(), before);
    let landed = sql(app.control(), "SELECT seq, pending FROM audit_event WHERE action = 'test.refused_write'", json!({})).await;
    assert_eq!(landed.len(), 1, "written once");
    assert!(landed[0]["seq"].is_number() && landed[0]["pending"].is_null(), "{landed:?}");
    assert!(verify(&app).await.ok);
    // replaying again writes nothing twice
    chain::replay(app.control()).await.unwrap();
    assert_eq!(sql(app.control(), "SELECT id FROM audit_event WHERE action = 'test.refused_write'", json!({})).await.len(), 1);
}

#[tokio::test]
async fn events_written_but_never_sealed_are_sealed_by_replay() {
    let _g = serial().await;
    let app = common::TestApp::new().await;
    let sealed_before = rows(&app).await.len();
    // as if the process died after each write and before its seal
    chain::SKIP_INLINE_SEAL.store(true, std::sync::atomic::Ordering::SeqCst);
    for i in 0..7 {
        emit(app.control(), &app.user.id, "test.interrupted", &format!("n={i}")).await;
    }
    chain::SKIP_INLINE_SEAL.store(false, std::sync::atomic::Ordering::SeqCst);
    let pending = sql(app.control(), "SELECT id FROM audit_event WHERE pending = true AND ledger = $l", json!({ "l": ledger(&app) })).await;
    assert_eq!(pending.len(), 7, "durably written, waiting for replay");
    let r = chain::replay(app.control()).await.unwrap();
    assert_eq!(r.sealed, 7);
    let all = rows(&app).await;
    assert_eq!(all.len(), sealed_before + 7);
    assert!(all.iter().all(|r| r["pending"].is_null() && r["seq"].is_number()));
    let v = verify(&app).await;
    assert!(v.ok, "{v:?}");
    assert_eq!(v.checked as usize, sealed_before + 7);
}

async fn concurrent(locks_off: bool) {
    let app = common::TestApp::new().await;
    let start = rows(&app).await.len();
    eunomia_backend::tx::LOCKS_OFF.store(locks_off, std::sync::atomic::Ordering::SeqCst);
    let mut tasks = Vec::new();
    for t in 0..8 {
        let control = app.control().clone();
        let user = app.user.id.clone();
        tasks.push(tokio::spawn(async move {
            for i in 0..25 {
                emit(&control, &user, "test.concurrent", &format!("t={t} i={i}")).await;
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    eunomia_backend::tx::LOCKS_OFF.store(false, std::sync::atomic::Ordering::SeqCst);
    chain::replay(app.control()).await.unwrap();
    let all = rows(&app).await;
    assert_eq!(all.len(), start + 200, "every event landed once");
    let seqs: Vec<i64> = all.iter().map(|r| r["seq"].as_i64().expect("sealed")).collect();
    assert_eq!(seqs, (1..=seqs.len() as i64).collect::<Vec<_>>(), "one gapless sequence, no duplicates");
    let v = verify(&app).await;
    assert!(v.ok, "{v:?}");
    assert_eq!(v.checked as usize, start + 200);
}

#[tokio::test]
async fn concurrent_writers_make_one_unbroken_chain() {
    let _g = serial().await;
    concurrent(false).await;
}

#[tokio::test]
async fn concurrent_writers_without_the_in_process_lock_meet_at_the_head_check() {
    // With `tx::lock` off, sealers only meet at AUDIT_SEAL's head check and the commit conflict,
    // the way sealers in separate processes do.
    let _g = serial().await;
    concurrent(true).await;
}

async fn ten_events(app: &common::TestApp) -> i64 {
    for i in 0..10 {
        emit(app.control(), &app.user.id, "test.tamper", &format!("n={i}")).await;
    }
    verify(app).await.head_seq.unwrap()
}

#[tokio::test]
async fn editing_deleting_or_reordering_an_event_is_detected() {
    let _g = serial().await;
    // an edit: the content no longer matches the hash
    let app = common::TestApp::new().await;
    let head = ten_events(&app).await;
    sql(app.control(), "UPDATE audit_event SET outcome = 'auth.forbidden' WHERE ledger = $l AND seq = 4", json!({ "l": ledger(&app) })).await;
    let v = verify(&app).await;
    let b = v.broken.expect("broken");
    assert!(!v.ok);
    assert_eq!((b.kind.as_str(), b.seq), ("altered", Some(4)), "{}", b.message);

    // an edit of the detail only (the export carries just its hash)
    let app = common::TestApp::new().await;
    ten_events(&app).await;
    sql(app.control(), "UPDATE audit_event SET detail = 'n=99' WHERE ledger = $l AND seq = 6", json!({ "l": ledger(&app) })).await;
    assert_eq!(verify(&app).await.broken.unwrap().seq, Some(6));

    // a deletion in the middle
    let app = common::TestApp::new().await;
    ten_events(&app).await;
    sql(app.control(), "DELETE audit_event WHERE ledger = $l AND seq = 5", json!({ "l": ledger(&app) })).await;
    let b = verify(&app).await.broken.unwrap();
    assert_eq!((b.kind.as_str(), b.seq), ("missing", Some(5)), "{}", b.message);

    // the newest event deleted
    let app = common::TestApp::new().await;
    let head2 = ten_events(&app).await;
    sql(app.control(), "DELETE audit_event WHERE ledger = $l AND seq = $s", json!({ "l": ledger(&app), "s": head2 })).await;
    let b = verify(&app).await.broken.unwrap();
    assert_eq!((b.kind.as_str(), b.seq), ("missing", Some(head2)), "{}", b.message);

    // the oldest events deleted without a retention checkpoint
    let app = common::TestApp::new().await;
    ten_events(&app).await;
    sql(app.control(), "DELETE audit_event WHERE ledger = $l AND seq <= 2", json!({ "l": ledger(&app) })).await;
    let b = verify(&app).await.broken.unwrap();
    assert_eq!(b.kind, "missing", "{}", b.message);

    // two events swapped (their sequence numbers exchanged)
    let app = common::TestApp::new().await;
    ten_events(&app).await;
    sql(
        app.control(),
        "UPDATE audit_event SET seq = -1 WHERE ledger = $l AND seq = 3; UPDATE audit_event SET seq = 3 WHERE ledger = $l AND seq = 7; UPDATE audit_event SET seq = 7 WHERE ledger = $l AND seq = -1",
        json!({ "l": ledger(&app) }),
    )
    .await;
    assert_eq!(verify(&app).await.broken.unwrap().seq, Some(3));

    // the whole tail rewritten with fresh hashes still disagrees with a checkpoint taken earlier
    let app = common::TestApp::new().await;
    ten_events(&app).await;
    chain::checkpoint_due(app.control(), chrono::TimeDelta::zero()).await.unwrap();
    let cp = verify(&app).await.head_seq.unwrap();
    sql(app.control(), "DELETE audit_event WHERE ledger = $l AND seq = $s", json!({ "l": ledger(&app), "s": cp })).await;
    sql(app.control(), "UPDATE audit_head SET seq = $s - 1, hash = (SELECT VALUE hash FROM audit_event WHERE ledger = $l AND seq = $s - 1)[0] WHERE ledger = $l", json!({ "l": ledger(&app), "s": cp })).await;
    let v = verify(&app).await;
    assert!(!v.ok, "a deleted newest event plus a rewound head is caught by the periodic checkpoint: {v:?}");
    let _ = head;
}

#[tokio::test]
async fn retention_leaves_a_checkpoint_the_chain_verifies_from() {
    let _g = serial().await;
    let app = common::TestApp::new().await;
    for i in 0..5 {
        emit(app.control(), &app.user.id, "test.old", &format!("n={i}")).await;
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    let cutoff = chrono::Utc::now();
    tokio::time::sleep(Duration::from_millis(20)).await;
    for i in 0..5 {
        emit(app.control(), &app.user.id, "test.new", &format!("n={i}")).await;
    }
    let head = verify(&app).await.head_seq.unwrap();
    let old = rows(&app).await.iter().filter(|r| r["action"] != "test.new").count() as i64;
    chain::prune(app.control(), cutoff).await.unwrap();
    let left = rows(&app).await;
    assert!(left.iter().all(|r| r["action"] == "test.new"), "{left:?}");
    let v = verify(&app).await;
    assert!(v.ok, "{v:?}");
    let anchor = v.anchor.unwrap();
    assert_eq!((anchor.kind.as_str(), anchor.seq, anchor.pruned), ("retention", old, Some(old)));
    assert_eq!((v.first_seq, v.head_seq), (Some(old + 1), Some(head)));
    // a second pass deletes nothing more and keeps verifying
    chain::prune(app.control(), cutoff).await.unwrap();
    assert!(verify(&app).await.ok);
    // the checkpoint is what makes the cut verifiable
    sql(app.control(), "DELETE audit_checkpoint WHERE ledger = $l AND kind = 'retention'", json!({ "l": ledger(&app) })).await;
    let b = verify(&app).await.broken.unwrap();
    assert_eq!(b.kind, "missing", "{}", b.message);
}

#[tokio::test]
async fn retention_by_days_keeps_recent_events_and_old_unchained_rows_go_by_age() {
    let _g = serial().await;
    let app = common::TestApp::new().await;
    // a row written before the chain started: no ledger, no hash
    sql(app.control(), "CREATE audit_event SET actor_kind = 'anonymous', action = 'auth.login', outcome = 'auth.unauthorized', created_at = time::now() - 400d", json!({})).await;
    sql(app.control(), "CREATE audit_event SET actor_kind = 'anonymous', action = 'auth.login', outcome = 'auth.unauthorized', created_at = time::now() - 1d", json!({})).await;
    emit(app.control(), &app.user.id, "test.recent", "").await;
    assert_eq!(chain::verify(app.control(), chain::INSTANCE).await.unwrap().before_chain, 2);
    audit::prune(app.control(), 365, 7).await.unwrap();
    assert_eq!(chain::verify(app.control(), chain::INSTANCE).await.unwrap().before_chain, 1, "the old unchained row went by age");
    let v = verify(&app).await;
    assert!(v.ok && v.anchor.unwrap().kind == "genesis", "recent chained events are kept");
}

#[tokio::test]
async fn the_export_holds_metadata_only_and_verifies_offline() {
    let _g = serial().await;
    let app = common::TestApp::new().await;
    const CANARY: &str = "AUDIT_EXPORT_CANARY_7f3a";
    // tenant content through a real tool: the memory text never reaches the control ledger
    app.tool("memory_write", json!({ "text": format!("{CANARY} the launch code is 1234"), "entity_name": "Acme" })).await;
    // free text in an event's detail is exported only as its hash
    emit(app.control(), &app.user.id, "test.detail", &format!("email={CANARY}@example.com")).await;
    for i in 0..3 {
        emit(app.control(), &app.user.id, "test.more", &format!("n={i}")).await;
    }
    let (status, doc) = app.http("GET", "/api/admin/audit/export", None, true).await;
    assert_eq!(status, StatusCode::OK, "{doc}");
    let text = doc.to_string();
    assert!(!text.contains(CANARY), "no tenant content or detail text in the export");
    assert!(!text.contains("launch code"));
    assert!(text.contains(&chain::sha256_hex(format!("email={CANARY}@example.com"))), "the detail is committed by its hash");
    assert_eq!(doc["ok"], true, "{doc}");
    assert_eq!(doc["anchor"]["kind"], "genesis");
    let events = doc["events"].as_array().unwrap();
    assert!(events.iter().any(|e| e["action"] == "tool.memory_write"));
    assert!(doc["checkpoints"].as_array().unwrap().iter().any(|c| c["kind"] == "export" && c["seq"] == events.last().unwrap()["seq"]));
    // the export itself is in the log
    let v = verify(&app).await;
    assert!(v.ok);

    // offline: the standard-library verifier agrees, and catches an edit
    if std::process::Command::new("python3").arg("--version").output().is_err() {
        eprintln!("python3 not found: offline verifier not run");
        return;
    }
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../scripts/audit/verify-audit-export.py");
    let dir = std::env::temp_dir().join(format!("audit-export-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let run = |doc: &Value, extra: &[&str]| {
        let path = dir.join("export.json");
        std::fs::write(&path, doc.to_string()).unwrap();
        let out = std::process::Command::new("python3").arg("-I").arg(script).arg(&path).args(extra).output().unwrap();
        (out.status.code(), String::from_utf8_lossy(&out.stdout).to_string())
    };
    let (code, out) = run(&doc, &[]);
    assert_eq!(code, Some(0), "{out}");
    let last = events.last().unwrap();
    let kept = format!("{}:{}", last["seq"], last["hash"].as_str().unwrap());
    assert_eq!(run(&doc, &["--checkpoint", &kept]).0, Some(0));
    let mut edited = doc.clone();
    edited["events"][1]["outcome"] = json!("auth.forbidden");
    let (code, out) = run(&edited, &[]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains(&format!("Event {} was changed", events[1]["seq"])), "{out}");
    let mut dropped = doc.clone();
    let gone = dropped["events"].as_array_mut().unwrap().remove(2);
    let (code, out) = run(&dropped, &[]);
    assert_eq!(code, Some(1));
    assert!(out.contains(&format!("Event {} is missing", gone["seq"])), "{out}");
    let (code, _) = run(&doc, &["--checkpoint", &format!("{}:{}", last["seq"], "0".repeat(64))]);
    assert_eq!(code, Some(1), "a checkpoint kept elsewhere that disagrees fails the file");
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn an_export_of_a_time_range_is_one_contiguous_run() {
    let _g = serial().await;
    let app = common::TestApp::new().await;
    for i in 0..3 {
        emit(app.control(), &app.user.id, "test.before", &format!("n={i}")).await;
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    let from = chrono::Utc::now();
    for i in 0..4 {
        emit(app.control(), &app.user.id, "test.inside", &format!("n={i}")).await;
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    let to = chrono::Utc::now();
    emit(app.control(), &app.user.id, "test.after", "").await;
    let doc = chain::export(app.control(), &ledger(&app), Some(from), Some(to)).await.unwrap();
    let actions: Vec<&str> = doc.events.iter().map(|e| e.fields.action.as_str()).collect();
    assert_eq!(actions, ["test.inside"; 4]);
    assert!(doc.ok);
    let anchor = doc.anchor.unwrap();
    assert_eq!(anchor.seq + 1, doc.events[0].fields.seq);
    assert_eq!(anchor.hash, doc.events[0].fields.prev_hash);
    // a bad range is refused, and only admins export
    let (status, _) = app.http("GET", "/api/admin/audit/export?from=2026-12-01&to=2026-11-01", None, true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let bob = common::register(&app.state, "bob@example.com").await;
    let token = eunomia_backend::models_user::create_api_token_with(app.control(), &bob.id, "t", &eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect::<Vec<_>>(), None, None).await.unwrap().token;
    for path in ["/api/admin/audit/export", "/api/admin/audit/verify", "/api/admin/access-reviews"] {
        let ((status, _), _) = common::http(&app.router, "GET", path, None, Some(&token), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
}

// ---------------------------------------------------------------- access reviews

async fn as_session(app: &common::TestApp, cookie: &str, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    common::http(&app.router, method, path, body, None, Some(cookie)).await.0
}

async fn login(app: &common::TestApp, email: &str) -> String {
    let ((status, _), cookie) =
        common::http(&app.router, "POST", "/api/auth/login", Some(json!({ "email": email, "password": common::PASSWORD })), None, None).await;
    assert_eq!(status, StatusCode::OK);
    cookie.unwrap().split(';').next().unwrap().to_string()
}

#[tokio::test]
async fn an_access_review_is_recorded_completed_frozen_and_exported() {
    let _g = serial().await;
    let app = common::TestApp::new().await;
    let bob = common::register(&app.state, "bob@example.com").await;
    let carol = common::register(&app.state, "carol@example.com").await;
    let issued = eunomia_backend::models_user::create_api_token_with(app.control(), &bob.id, "bob laptop", &["memory:read".to_string()], None, None).await.unwrap();
    let me = login(&app, &app.user.email).await;

    // a token cannot start one; a browser session can
    let (status, _) = app.http("POST", "/api/admin/access-reviews", None, true).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, review) = as_session(&app, &me, "POST", "/api/admin/access-reviews", None).await;
    assert_eq!(status, StatusCode::OK, "{review}");
    let id = review["id"].as_str().unwrap().to_string();
    let people = review["snapshot"]["people"].as_array().unwrap();
    assert_eq!(people.len(), 3);
    let bob_row = people.iter().find(|p| p["email"] == "bob@example.com").unwrap();
    assert_eq!(bob_row["role"], "member");
    assert_eq!(bob_row["tokens"][0]["name"], "bob laptop");
    assert!(!review.to_string().contains(&issued.token), "never a token secret");
    assert!(!review.to_string().contains("token_hash") && !review.to_string().contains("password"));
    let creator = people.iter().find(|p| p["email"] == app.user.email.as_str()).unwrap();
    assert_eq!((creator["role"].as_str(), creator["admin_source"].as_str()), (Some("admin"), Some("creator")));
    assert!(creator["spaces"].as_array().unwrap().iter().any(|s| s["role"] == "owner"));

    // one open review at a time
    let (status, body) = as_session(&app, &me, "POST", "/api/admin/access-reviews", None).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::CONFLICT, Some("access_review.open_exists")));

    // no deciding about yourself
    let mine = format!("/api/admin/access-reviews/{id}/decisions/{}", chain::rid(&app.user.id));
    let (status, body) = as_session(&app, &me, "PUT", &mine, Some(json!({ "decision": "keep" }))).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("access_review.self_review")));

    let bob_path = format!("/api/admin/access-reviews/{id}/decisions/{}", chain::rid(&bob.id));
    let (status, body) = as_session(&app, &me, "PUT", &bob_path, Some(json!({ "decision": "keep", "note": "needs read access" }))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = as_session(&app, &me, "PUT", &bob_path, Some(json!({ "decision": "nope" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // carol is still undecided
    let complete = format!("/api/admin/access-reviews/{id}/complete");
    let (status, body) = as_session(&app, &me, "POST", &complete, None).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::CONFLICT, Some("access_review.incomplete")), "{body}");
    let carol_path = format!("/api/admin/access-reviews/{id}/decisions/{}", chain::rid(&carol.id));
    let (status, _) = as_session(&app, &me, "PUT", &carol_path, Some(json!({ "decision": "suspend" }))).await;
    assert_eq!(status, StatusCode::OK);

    let (status, done) = as_session(&app, &me, "POST", &complete, None).await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert_eq!(done["status"], "completed");
    assert_eq!(done["summary"], json!({ "keep": 1, "remove": 0, "suspend": 1, "not_reviewed": 1 }), "the reviewer's own access is recorded as not reviewed");

    // completed means frozen: the API refuses, and so does the database itself
    let (status, body) = as_session(&app, &me, "PUT", &bob_path, Some(json!({ "decision": "remove" }))).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::CONFLICT, Some("access_review.completed")));
    let org = app.db().await;
    let raw = org.test_raw();
    let rid = eunomia_backend::rid::parse(&id).unwrap();
    let edit = raw.query("UPDATE $id SET summary = { keep: 3 }").bind(("id", rid.clone())).await.unwrap().check();
    assert!(edit.is_err() && edit.unwrap_err().to_string().contains("access_review.completed"), "the schema refuses an edit");
    let del = raw.query("DELETE $id").bind(("id", rid.clone())).await.unwrap().check();
    assert!(del.is_err(), "and a delete");
    let del = raw.query("DELETE access_review_decision WHERE review = $id").bind(("id", rid.clone())).await.unwrap().check();
    assert!(del.is_err(), "and deleting its decisions");

    // a new review can start once the last one is completed
    let (status, _) = as_session(&app, &me, "POST", "/api/admin/access-reviews", None).await;
    assert_eq!(status, StatusCode::OK);

    // the export ties the review to the hash-chained log
    let (status, file) = app.http("GET", &format!("/api/admin/access-reviews/{id}/export"), None, true).await;
    assert_eq!(status, StatusCode::OK, "{file}");
    assert_eq!(file["result_hash_matches"], true);
    assert_eq!(file["snapshot_hash_matches"], true);
    assert_eq!(file["result"]["decisions"].as_array().unwrap().len(), 2);
    assert_eq!(file["result"]["not_reviewed"], json!([chain::rid(&app.user.id)]));
    assert!(file["audit_event"]["seq"].is_number(), "{file}");
    let found = sql(
        app.control(),
        "SELECT detail, hash FROM audit_event WHERE action = 'access_review.complete' AND ledger = $l",
        json!({ "l": ledger(&app) }),
    )
    .await;
    assert!(found[0]["detail"].as_str().unwrap().contains(file["result_hash"].as_str().unwrap()));
    assert_eq!(found[0]["hash"], file["audit_event"]["hash"]);
    let v = verify(&app).await;
    assert!(v.ok, "{v:?}");
    // the start, the decisions and the completion are in the activity log
    let (_, page) = app.http("GET", "/api/admin/audit?category=admin&limit=50", None, true).await;
    let summaries: Vec<&str> = page["results"].as_array().unwrap().iter().map(|r| r["summary"].as_str().unwrap()).collect();
    for want in ["Completed an access review", "Decided in an access review to suspend carol@example.com", "Started an access review of 3 people"] {
        assert!(summaries.contains(&want), "missing {want:?} in {summaries:?}");
    }

    // another admin can decide about the first one in the next review
    let (status, _) = as_session(&app, &me, "PUT", &format!("/api/admin/people/{}/admin", chain::rid(&bob.id)), Some(json!({ "admin": true }))).await;
    assert_eq!(status, StatusCode::OK);
    let bob_cookie = login(&app, "bob@example.com").await;
    let (_, list) = as_session(&app, &bob_cookie, "GET", "/api/admin/access-reviews", None).await;
    let open = list.as_array().unwrap().iter().find(|r| r["status"] == "open").unwrap()["id"].as_str().unwrap().to_string();
    let (status, _) = as_session(&app, &bob_cookie, "PUT", &format!("/api/admin/access-reviews/{open}/decisions/{}", chain::rid(&app.user.id)), Some(json!({ "decision": "keep" }))).await;
    assert_eq!(status, StatusCode::OK);
}
