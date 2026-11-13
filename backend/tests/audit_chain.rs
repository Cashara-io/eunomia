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

/// Waits (bounded) for the background sealer to leave nothing pending in the test org's ledger,
/// without sealing anything itself.
async fn settled(app: &common::TestApp) -> bool {
    for _ in 0..500 {
        if sql(app.control(), "SELECT id FROM audit_event WHERE pending = true AND ledger = $l", json!({ "l": ledger(app) })).await.is_empty() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
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
    assert!(settled(&app).await, "the background sealer seals every event without being asked");
    let mine = rows(&app).await;
    assert!(mine.iter().all(|r| r["pending"].is_null()), "{mine:?}");
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
    // the caller's future is polled once (the write is now in flight in its own task), then dropped
    let mut caller = Box::pin(emit(app.control(), &app.user.id, "test.cancelled", ""));
    assert!(futures::poll!(&mut caller).is_pending(), "the write had not finished when the caller went away");
    drop(caller);
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

#[tokio::test]
async fn concurrent_writers_make_one_unbroken_chain() {
    let _g = serial().await;
    let app = common::TestApp::new().await;
    let start = rows(&app).await.len();
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
    // no replay and no explicit seal: the background sealers alone must have chained everything
    assert!(settled(&app).await, "sealed in the background");
    let all = rows(&app).await;
    assert_eq!(all.len(), start + 200, "every event landed once");
    let seqs: Vec<i64> = all.iter().map(|r| r["seq"].as_i64().expect("sealed")).collect();
    assert_eq!(seqs, (1..=seqs.len() as i64).collect::<Vec<_>>(), "one gapless sequence, no duplicates");
    let v = verify(&app).await;
    assert!(v.ok, "{v:?}");
    assert_eq!(v.checked as usize, start + 200);
}

#[tokio::test]
async fn sealers_racing_without_the_in_process_lock_meet_at_the_head_check() {
    // Eight sealers on one ledger with `tx::lock` off meet only at AUDIT_SEAL's head check and the
    // commit conflict, the way sealers in separate replicas do. Every one must finish without an
    // error (a lost race is retried, never surfaced), and together they seal each row exactly once.
    let _g = serial().await;
    let app = common::TestApp::new().await;
    chain::seal(app.control(), &ledger(&app)).await.unwrap();
    let start = rows(&app).await.len();
    chain::SKIP_INLINE_SEAL.store(true, std::sync::atomic::Ordering::SeqCst);
    for i in 0..200 {
        emit(app.control(), &app.user.id, "test.race", &format!("n={i}")).await;
    }
    chain::SKIP_INLINE_SEAL.store(false, std::sync::atomic::Ordering::SeqCst);
    eunomia_backend::tx::LOCKS_OFF.store(true, std::sync::atomic::Ordering::SeqCst);
    let sealers: Vec<_> = (0..8)
        .map(|_| {
            let control = app.control().clone();
            let l = ledger(&app);
            tokio::spawn(async move { chain::seal(&control, &l).await })
        })
        .collect();
    let mut sealed = 0;
    for s in sealers {
        sealed += s.await.unwrap().expect("a sealer that lost the race retries; none fails");
    }
    eunomia_backend::tx::LOCKS_OFF.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(sealed, 200, "each row sealed once, by one sealer");
    let all = rows(&app).await;
    assert_eq!(all.len(), start + 200);
    let seqs: Vec<i64> = all.iter().map(|r| r["seq"].as_i64().expect("sealed")).collect();
    assert_eq!(seqs, (1..=seqs.len() as i64).collect::<Vec<_>>(), "one gapless sequence, no duplicates");
    assert!(verify(&app).await.ok);
}

#[tokio::test]
async fn verify_stays_intact_while_writers_append() {
    let _g = serial().await;
    let app = common::TestApp::new().await;
    for i in 0..30 {
        emit(app.control(), &app.user.id, "test.before", &format!("n={i}")).await;
    }
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writers: Vec<_> = (0..3)
        .map(|t| {
            let (control, user, stop) = (app.control().clone(), app.user.id.clone(), stop.clone());
            tokio::spawn(async move {
                let mut i = 0;
                while !stop.load(std::sync::atomic::Ordering::SeqCst) && i < 300 {
                    emit(&control, &user, "test.busy", &format!("t={t} i={i}")).await;
                    i += 1;
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                i
            })
        })
        .collect();
    let mut checks = 0;
    for _ in 0..8 {
        let v = verify(&app).await;
        assert!(v.ok, "a busy log is not a tampered one: {v:?}");
        // a periodic checkpoint written mid-walk must not read as missing events either
        chain::checkpoint_due(app.control(), chrono::TimeDelta::zero()).await.unwrap();
        checks += 1;
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut written = 0;
    for w in writers {
        written += w.await.unwrap();
    }
    assert!(written > 0 && checks == 8, "writers appended during the checks ({written})");
    assert!(verify(&app).await.ok);
}

#[tokio::test]
async fn a_missing_head_or_genesis_is_not_intact() {
    let _g = serial().await;
    let app = common::TestApp::new().await;
    ten_events(&app).await;
    sql(app.control(), "DELETE audit_head WHERE ledger = $l", json!({ "l": ledger(&app) })).await;
    let v = verify(&app).await;
    assert!(!v.ok, "events but no head: {v:?}");
    assert_eq!(v.broken.unwrap().kind, "head");

    let app = common::TestApp::new().await;
    ten_events(&app).await;
    sql(app.control(), "DELETE audit_event WHERE ledger = $l; DELETE audit_head WHERE ledger = $l", json!({ "l": ledger(&app) })).await;
    assert!(!verify(&app).await.ok, "events and head gone, checkpoints left");

    let app = common::TestApp::new().await;
    ten_events(&app).await;
    sql(app.control(), "DELETE audit_checkpoint WHERE ledger = $l AND kind = 'genesis'", json!({ "l": ledger(&app) })).await;
    assert_eq!(verify(&app).await.broken.unwrap().kind, "genesis");
}

#[tokio::test]
async fn every_replica_drains_its_own_queue_on_its_own_timer() {
    // two processes' queues, no scheduler leader: each drain timer writes only its own queue
    let _g = serial().await;
    let app = common::TestApp::new().await;
    let a: &'static chain::Outbox = Box::leak(Box::new(chain::Outbox::new()));
    let b: &'static chain::Outbox = Box::leak(Box::new(chain::Outbox::new()));
    for _ in 0..3 {
        a.queue_for_test(app.control(), &app.user.id, "test.replica_a");
        b.queue_for_test(app.control(), &app.user.id, "test.replica_b");
    }
    assert_eq!((a.len(), b.len()), (3, 3));
    let ha = chain::spawn_drain(a, Duration::from_millis(20));
    assert!(landed(app.control(), "test.replica_a", 3).await, "replica A drained its queue");
    assert_eq!(b.len(), 3, "A's timer never touches B's queue");
    let hb = chain::spawn_drain(b, Duration::from_millis(20));
    assert!(landed(app.control(), "test.replica_b", 3).await, "replica B drained its own");
    ha.abort();
    hb.abort();
    assert_eq!((a.len(), b.len()), (0, 0));
    assert!(settled(&app).await);
    assert!(verify(&app).await.ok);
}

#[tokio::test]
async fn a_dropped_client_cannot_separate_an_action_from_its_event() {
    let _g = serial().await;
    let app = common::TestApp::new().await;
    let (control, user) = (app.control().clone(), app.user.id.clone());
    // the "action" is still running when the client goes away
    let request = audit::detached(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        emit(&control, &user, "test.action_done", "").await;
        emit(&control, &user, "test.action_audited", "").await;
    });
    assert!(tokio::time::timeout(Duration::from_millis(20), request).await.is_err(), "the client left mid-action");
    assert!(landed(app.control(), "test.action_done", 1).await, "the action finished");
    assert!(landed(app.control(), "test.action_audited", 1).await, "and so did its audit event");
}

#[tokio::test]
async fn the_instance_ledger_is_for_server_operators_on_a_host_with_several_orgs() {
    let _g = serial().await;
    let app = common::TestApp::new().await;
    // one org: a granted admin of it sees the sign-in attempts with no workspace, as before
    let bob = common::register(&app.state, "bob@example.com").await;
    sql(app.control(), &format!("UPDATE {} SET admin = true", chain::rid(&bob.id)), json!({})).await;
    let bob_token = |u: &eunomia_backend::models_user::User| {
        let control = app.control().clone();
        let id = u.id.clone();
        async move {
            eunomia_backend::models_user::create_api_token_with(&control, &id, "t", &eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect::<Vec<_>>(), None, None).await.unwrap().token
        }
    };
    let t_bob = bob_token(&bob).await;
    const TRIED: &str = "victim-of-another-org@example.com";
    let ((status, _), _) = common::http(&app.router, "POST", "/api/auth/login", Some(json!({ "email": TRIED, "password": "x" })), None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let ((status, _), _) = common::http(&app.router, "GET", "/api/admin/audit/export?ledger=instance", None, Some(&t_bob), None).await;
    assert_eq!(status, StatusCode::OK, "single org: its admins keep the view they had");

    // a second org on the same host: its admin (granted in their own org) is not an operator
    let carol = common::register_personal(&app.state, "carol@example.com").await;
    assert_ne!(carol.org, app.user.org);
    sql(app.control(), &format!("UPDATE {} SET admin = true", chain::rid(&carol.id)), json!({})).await;
    let t_carol = bob_token(&carol).await;
    for (who, token) in [("carol", &t_carol), ("bob", &t_bob)] {
        let ((status, body), _) = common::http(&app.router, "GET", "/api/admin/audit/export?ledger=instance", None, Some(token), None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{who}: {body}");
        let ((status, report), _) = common::http(&app.router, "GET", "/api/admin/audit/verify", None, Some(token), None).await;
        assert_eq!(status, StatusCode::OK);
        let ledgers: Vec<&str> = report["ledgers"].as_array().unwrap().iter().map(|l| l["ledger"].as_str().unwrap()).collect();
        assert!(!ledgers.contains(&chain::INSTANCE), "{who}: {ledgers:?}");
        let ((_, page), _) = common::http(&app.router, "GET", "/api/admin/audit?category=signins&limit=200", None, Some(token), None).await;
        assert!(!page.to_string().contains(TRIED), "{who} cannot read other tenants' sign-in attempts in the activity log");
    }
    // the install's first account (and EUNOMIA_ADMIN_EMAILS) still can
    let (status, doc) = app.http("GET", "/api/admin/audit/export?ledger=instance", None, true).await;
    assert_eq!(status, StatusCode::OK, "{doc}");
    let text = doc.to_string();
    assert!(!text.contains(TRIED), "no email in the export");
    assert!(!text.contains(&chain::sha256_hex(TRIED)), "and no plain hash of it a dictionary could reverse");
    assert!(text.contains(&chain::digest(TRIED)), "only the keyed digest");
    let (_, page) = app.http("GET", "/api/admin/audit?category=signins&limit=200", None, true).await;
    assert!(page.to_string().contains(TRIED), "the operator's activity log still shows the attempt");
    // and carol's own export carries nothing of the other org
    let ((status, mine), _) = common::http(&app.router, "GET", "/api/admin/audit/export", None, Some(&t_carol), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(mine["events"].as_array().unwrap().iter().all(|e| e["ledger"] == chain::org_ledger(&carol.org)));
}

#[tokio::test]
async fn retention_never_cuts_newer_events_for_one_late_old_timestamp() {
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
    // a row written with an old time but sealed last (a replica with a slow clock, a late replay)
    sql(
        app.control(),
        "CREATE audit_event SET user = $u, actor_kind = 'user', actor_id = 'late', action = 'test.late', outcome = 'ok', created_at = time::now() - 30d, ledger = $l, pending = true",
        json!({ "u": app.user.id, "l": ledger(&app) }),
    )
    .await;
    chain::seal(app.control(), &ledger(&app)).await.unwrap();
    chain::prune(app.control(), cutoff).await.unwrap();
    let left: Vec<String> = rows(&app).await.iter().map(|r| r["action"].as_str().unwrap().to_string()).collect();
    assert_eq!(left.iter().filter(|a| *a == "test.new").count(), 5, "every event inside the window is kept: {left:?}");
    assert!(verify(&app).await.ok);
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

    // the newest event deleted and the head wound back to the one before: caught by the periodic
    // checkpoint taken before (a checkpoint past the head)
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
    let detail = format!("email={CANARY}@example.com");
    assert!(text.contains(&chain::digest(&detail)), "the detail is committed by its keyed digest");
    assert!(!text.contains(&chain::sha256_hex(&detail)), "not by a plain hash a dictionary could reverse");
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

// ---------------------------------------------------------------- review round 2

/// Runs the offline verifier on `doc` (and `review`), as an auditor would. `None` when python3 is
/// not installed.
fn offline(doc: &Value, extra: &[&str], review: Option<&Value>) -> Option<(Option<i32>, String)> {
    if std::process::Command::new("python3").arg("--version").output().is_err() {
        eprintln!("python3 not found: offline verifier not run");
        return None;
    }
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../scripts/audit/verify-audit-export.py");
    let dir = std::env::temp_dir().join(format!("audit-offline-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("export.json");
    std::fs::write(&path, doc.to_string()).unwrap();
    let mut cmd = std::process::Command::new("python3");
    cmd.arg("-I").arg(script).arg(&path).args(extra);
    if let Some(r) = review {
        let rpath = dir.join("review.json");
        std::fs::write(&rpath, r.to_string()).unwrap();
        cmd.arg("--review").arg(rpath);
    }
    let out = cmd.output().unwrap();
    std::fs::remove_dir_all(&dir).ok();
    Some((out.status.code(), String::from_utf8_lossy(&out.stdout).to_string()))
}

async fn org_root(app: &common::TestApp, org: &eunomia_backend::pool::OrgId) -> surrealdb::Surreal<surrealdb::engine::any::Any> {
    let mut res = app.control().test_raw().query("SELECT VALUE db FROM tenant WHERE org = $o").bind(("o", org.record())).await.unwrap();
    let names: Vec<String> = res.take(0).unwrap();
    app.state.provisioner.as_ref().unwrap().session_on(&names[0]).await.unwrap()
}

#[tokio::test]
async fn an_export_with_events_but_no_head_reports_the_break_and_fails_offline() {
    let _g = serial().await;
    let app = common::TestApp::new().await;
    ten_events(&app).await;
    sql(app.control(), "DELETE audit_head WHERE ledger = $l", json!({ "l": ledger(&app) })).await;
    let doc = chain::export(app.control(), &ledger(&app), None, None).await.unwrap();
    assert!(!doc.ok && doc.events.is_empty(), "nothing exported, and not a valid-looking empty file");
    assert_eq!(doc.broken.as_ref().unwrap().kind, "head");
    let doc = serde_json::to_value(&doc).unwrap();
    if let Some((code, out)) = offline(&doc, &[], None) {
        assert_eq!(code, Some(1), "{out}");
        assert!(out.contains("reported this log as broken"), "{out}");
    }
    // a ledger that really is empty: honest, but only accepted when the auditor says so
    let empty = serde_json::to_value(chain::export(app.control(), "org:00000000000000000000000000000000", None, None).await.unwrap()).unwrap();
    assert_eq!(empty["ok"], true);
    if let Some((code, out)) = offline(&empty, &[], None) {
        assert_eq!(code, Some(1), "{out}");
        assert_eq!(offline(&empty, &["--expect-empty"], None).unwrap().0, Some(0));
    }
}

#[tokio::test]
async fn an_access_review_rewritten_after_completion_no_longer_matches_the_chain() {
    let _g = serial().await;
    let app = common::TestApp::new().await;
    let bob = common::register(&app.state, "bob@example.com").await;
    let me = login(&app, &app.user.email).await;
    let (_, review) = as_session(&app, &me, "POST", "/api/admin/access-reviews", None).await;
    let id = review["id"].as_str().unwrap().to_string();
    let (status, _) = as_session(&app, &me, "PUT", &format!("/api/admin/access-reviews/{id}/decisions/{}", chain::rid(&bob.id)), Some(json!({ "decision": "keep" }))).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = as_session(&app, &me, "POST", &format!("/api/admin/access-reviews/{id}/complete"), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, honest) = app.http("GET", &format!("/api/admin/access-reviews/{id}/export"), None, true).await;
    assert_eq!((honest["result_hash_matches"].clone(), honest["chain_matches"].clone()), (json!(true), json!(true)), "{honest}");
    let (_, log) = app.http("GET", "/api/admin/audit/export", None, true).await;
    if let Some((code, out)) = offline(&log, &[], Some(&honest)) {
        assert_eq!(code, Some(0), "{out}");
        assert!(out.contains("is the one the log recorded"), "{out}");
    }

    // someone with database access lifts the freeze, changes a decision and recomputes the hash
    let root = org_root(&app, &app.user.org).await;
    let rid = eunomia_backend::rid::parse(&id).unwrap();
    root.query("REMOVE EVENT access_review_frozen ON access_review; REMOVE EVENT access_review_decision_frozen ON access_review_decision; \
                UPDATE access_review_decision SET decision = 'remove' WHERE review = $id")
        .bind(("id", rid.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();
    let (_, changed) = app.http("GET", &format!("/api/admin/access-reviews/{id}/export"), None, true).await;
    assert_eq!(changed["result_hash_matches"], false);
    let forged = chain::sha256_hex(eunomia_backend::routers::access_reviews::canonical_json(&changed["result"]));
    root.query("UPDATE $id SET result_hash = $h").bind(("id", rid)).bind(("h", forged)).await.unwrap().check().unwrap();
    let (_, rewritten) = app.http("GET", &format!("/api/admin/access-reviews/{id}/export"), None, true).await;
    assert_eq!(rewritten["result_hash_matches"], true, "the forger recomputed the review's own hash");
    assert_eq!(rewritten["chain_matches"], false, "but the chain still holds the original: {rewritten}");
    let (_, log) = app.http("GET", "/api/admin/audit/export", None, true).await;
    if let Some((code, out)) = offline(&log, &[], Some(&rewritten)) {
        assert_eq!(code, Some(1), "{out}");
        assert!(out.contains("records another result"), "{out}");
    }
}

#[tokio::test]
async fn an_org_past_a_missing_migration_is_not_served_until_repaired() {
    let _g = serial().await;
    let app = common::TestApp::new().await;
    let p = app.state.provisioner.as_ref().unwrap();
    assert!(p.find_migration_gaps(app.control()).await.unwrap().is_empty());
    // as if this org were migrated by a build that lacked tenant 0030
    let root = org_root(&app, &app.user.org).await;
    root.query("DELETE _migration WHERE version = 30").await.unwrap().check().unwrap();
    app.state.pool.evict(&app.user.org);
    let gaps = p.find_migration_gaps(app.control()).await.unwrap();
    assert_eq!(gaps, vec![(app.user.org, vec![30])]);
    eunomia_backend::state::guard_skipped_migrations(p, app.control(), &app.state.pool, false).await.unwrap();
    let refused = app.state.pool.for_org(&app.user.org).await.err().expect("not served");
    assert_eq!(refused.code.as_str(), "tenant.schema_behind");
    assert!(refused.message.contains("0030"), "{}", refused.message);
    // the operator's repair applies it and serves the org again
    eunomia_backend::state::guard_skipped_migrations(p, app.control(), &app.state.pool, true).await.unwrap();
    assert!(app.state.pool.for_org(&app.user.org).await.is_ok());
    assert!(p.find_migration_gaps(app.control()).await.unwrap().is_empty());
}

#[tokio::test]
async fn one_failing_ledger_does_not_stop_the_replay_or_retention_of_the_others() {
    let _g = serial().await;
    let app = common::TestApp::new().await;
    let other = common::register_personal(&app.state, "other@example.com").await;
    chain::seal(app.control(), &ledger(&app)).await.unwrap();
    chain::SKIP_INLINE_SEAL.store(true, std::sync::atomic::Ordering::SeqCst);
    for i in 0..3 {
        emit(app.control(), &app.user.id, "test.stuck", &format!("n={i}")).await;
        emit(app.control(), &other.id, "test.fine", &format!("n={i}")).await;
    }
    chain::SKIP_INLINE_SEAL.store(false, std::sync::atomic::Ordering::SeqCst);
    *chain::FAIL_SEAL_LEDGER.lock().unwrap() = Some(ledger(&app));
    let failures = chain::SEAL_FAILURES.load(std::sync::atomic::Ordering::SeqCst);
    let pass = chain::replay(app.control()).await;
    *chain::FAIL_SEAL_LEDGER.lock().unwrap() = None;
    assert!(pass.is_ok(), "the replay carries on past a failing ledger");
    assert!(chain::SEAL_FAILURES.load(std::sync::atomic::Ordering::SeqCst) > failures, "and counts the failure");
    let pending = |l: String| {
        let control = app.control().clone();
        async move { sql(&control, "SELECT id FROM audit_event WHERE pending = true AND ledger = $l", json!({ "l": l })).await.len() }
    };
    assert_eq!(pending(chain::org_ledger(&other.org)).await, 0, "the other ledger was sealed");
    assert_eq!(pending(ledger(&app)).await, 3, "the failing one waits");
    // retention runs too, and the failed ledger is left alone for a while, then sealed
    chain::prune(app.control(), chrono::Utc::now() - chrono::TimeDelta::days(365)).await.unwrap();
    chain::replay(app.control()).await.unwrap();
    assert_eq!(pending(ledger(&app)).await, 3, "backed off");
    tokio::time::sleep(Duration::from_millis(2100)).await;
    chain::replay(app.control()).await.unwrap();
    assert_eq!(pending(ledger(&app)).await, 0, "and sealed once its back-off passed");
    assert!(verify(&app).await.ok);
}

#[tokio::test]
async fn an_access_review_lists_only_this_orgs_spaces_in_a_shared_database() {
    // The isolation suite's shared-database mode: every org in one database, so only the app's own
    // filters keep them apart. A membership row pointing an alpha user at a beta space must not put
    // beta's space into alpha's review.
    let _g = serial().await;
    let state = common::bare_state().await;
    let alpha = common::register_personal(&state, "alpha@example.com").await;
    let mut res = state.control.test_raw().query("SELECT db, db_pass_enc FROM ONLY $t").bind(("t", alpha.org.tenant_record())).await.unwrap();
    let row: Value = res.take::<Option<Value>>(0).unwrap().unwrap();
    let pass = eunomia_backend::connectors::crypto::decrypt(&state.settings.encryption_key, row["db_pass_enc"].as_str().unwrap()).unwrap();
    state.pool.set_shared_database(row["db"].as_str().unwrap(), "app", &pass);
    let beta = common::register_personal(&state, "beta@example.com").await;
    const CANARY: &str = "BETA_SPACE_CANARY_41c9";
    let made = common::sys(eunomia_backend::tools::registry::call(&state, &beta, "vault_create", json!({ "name": CANARY, "kind": "org" }))).await.unwrap();
    let beta_vault = eunomia_backend::rid::parse(made["id"].as_str().unwrap()).unwrap();
    let shared = state.pool.for_org(&alpha.org).await.unwrap();
    shared.test_raw().query("CREATE vault_member SET vault = $v, user = $u, role = 'member'").bind(("v", beta_vault)).bind(("u", alpha.id.clone())).await.unwrap().check().unwrap();
    let router = eunomia_backend::app(state.clone());
    let ((status, _), cookie) =
        common::http(&router, "POST", "/api/auth/login", Some(json!({ "email": "alpha@example.com", "password": common::PASSWORD })), None, None).await;
    assert_eq!(status, StatusCode::OK);
    let cookie = cookie.unwrap().split(';').next().unwrap().to_string();
    let ((status, review), _) = common::http(&router, "POST", "/api/admin/access-reviews", None, None, Some(&cookie)).await;
    assert_eq!(status, StatusCode::OK, "{review}");
    assert!(!review.to_string().contains(CANARY), "another org's space in this org's review: {review}");
    assert!(review["snapshot"]["people"][0]["spaces"].as_array().is_some_and(|s| !s.is_empty()), "alpha's own spaces are listed");
}

#[tokio::test]
async fn sealing_never_waits_on_the_locks_request_paths_share() {
    // Every stripe of the shared `tx::lock` (sign-up, job claims) is held elsewhere: a seal must
    // still finish, because the sealer has locks of its own.
    let _g = serial().await;
    let app = common::TestApp::new().await;
    chain::SKIP_INLINE_SEAL.store(true, std::sync::atomic::Ordering::SeqCst);
    emit(app.control(), &app.user.id, "test.locked_out", "").await;
    chain::SKIP_INLINE_SEAL.store(false, std::sync::atomic::Ordering::SeqCst);
    let mut held = Vec::new();
    for i in 0..2000 {
        if let Ok(Some(g)) = tokio::time::timeout(Duration::from_millis(2), eunomia_backend::tx::lock(&format!("stripe-{i}"))).await {
            held.push(g);
        }
    }
    assert!(held.len() >= 64, "every stripe is taken ({})", held.len());
    let sealed = tokio::time::timeout(Duration::from_secs(10), chain::seal(app.control(), &ledger(&app))).await;
    drop(held);
    assert!(matches!(sealed, Ok(Ok(n)) if n >= 1), "the seal ran while every shared stripe was held: {sealed:?}");
}
