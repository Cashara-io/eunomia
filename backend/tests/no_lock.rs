//! The striped in-process write lock (`tx::lock`) makes writers in one process queue up, so the
//! concurrent tests in `concurrent_memory.rs` and `jobs.rs` cannot fail if the transaction and retry
//! path breaks. These runs switch the lock off (`tx::LOCKS_OFF`, `test-support` builds only) so the
//! writers meet at the database the way writers in separate processes do, and report what happens.
//!
//! What the in-memory engine guarantees is asserted; what it does not (it misses a fraction of
//! tight write-write races: both commit and the later one wins) is measured and printed, never
//! asserted. Before `store::surface_root_cause` existed, a lost commit race came back as a 500 here
//! ("not executed due to a failed transaction") and 80 of 400 writes failed. See docs/testing.md ("Concurrency ceiling") and docs/architecture/jobs.md.
//! The switch is process-global, so every test here takes `SERIAL` and this file is its own binary.

mod common;

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

use common::TestApp;
use eunomia_backend::jobs::{self, NewJob};
use eunomia_backend::tools::registry;
use serde_json::json;
use surrealdb::types::RecordId;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct LocksOff;
impl LocksOff {
    fn set(off: bool) -> Self {
        eunomia_backend::tx::LOCKS_OFF.store(off, Ordering::SeqCst);
        LocksOff
    }
}
impl Drop for LocksOff {
    fn drop(&mut self) {
        eunomia_backend::tx::LOCKS_OFF.store(false, Ordering::SeqCst);
    }
}

struct Outcome {
    attempted: [usize; 2],
    ok: [usize; 2],
    failed: Vec<String>,
    worlds: i64,
    observations: i64,
    persons: i64,
    version: i64,
}

/// Two tasks write facts and observations about one entity; returns what was attempted, what succeeded
/// and what the database holds afterwards.
async fn hammer(per_task: usize) -> Outcome {
    let app = TestApp::new().await;
    let mut tasks = Vec::new();
    for t in 0..2 {
        let (state, user) = (app.state.clone(), app.user.clone());
        tasks.push(tokio::spawn(async move {
            let (mut ok, mut failed) = ([0usize; 2], Vec::new());
            for i in 0..per_task {
                for (n, (kind, text)) in [("world", format!("fact {t}-{i}")), ("observation", format!("belief {t}-{i}"))].into_iter().enumerate() {
                    let args = json!({"subject_name":"Alice","subject_kind":"person","text":text,"type":kind});
                    match common::sys(registry::call(&state, &user, "memory_write", args)).await {
                        Ok(v) if v.get("error").is_none() => ok[n] += 1,
                        other => failed.push(format!("{other:?}")),
                    }
                }
            }
            (ok, failed)
        }));
    }
    let (mut ok, mut failed) = ([0usize; 2], Vec::new());
    for t in tasks {
        let (o, f) = t.await.unwrap();
        ok[0] += o[0];
        ok[1] += o[1];
        failed.extend(f);
    }
    let db = app.db().await;
    let count = |q: &'static str| {
        let db = db.clone();
        async move {
            let mut res = db.test_raw().query(q).await.unwrap();
            let n: Option<i64> = res.take("n").unwrap();
            n.unwrap_or(0)
        }
    };
    Outcome {
        attempted: [2 * per_task; 2],
        ok,
        failed,
        persons: count("SELECT count() AS n FROM person GROUP ALL").await,
        worlds: count("SELECT count() AS n FROM memory WHERE type = 'world' GROUP ALL").await,
        observations: count("SELECT count() AS n FROM memory WHERE type = 'observation' GROUP ALL").await,
        version: count("SELECT version AS n FROM memory WHERE type = 'observation'").await,
    }
}

/// Lock on: the in-process queue makes the result exact (the reference run).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn memory_writes_with_the_lock_on_are_exact() {
    let _s = SERIAL.lock().await;
    let _l = LocksOff::set(false);
    let o = hammer(100).await;
    assert!(o.failed.is_empty(), "{:?}", o.failed.first());
    assert_eq!((o.persons, o.worlds, o.observations, o.version), (1, 200, 1, 200));
}

/// Lock off: two tasks write the same entity straight into the transaction and retry path.
/// Asserted (what the engine guarantees): a write either succeeds or fails loudly (no silent
/// drop), every successful fact is stored, a failure is only a retryable conflict (409
/// `db.conflict`) and never any other error. Measured and printed: how many 409s, whether the
/// version counter lost updates.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn memory_writes_with_the_lock_off_lose_nothing_that_succeeded() {
    let _s = SERIAL.lock().await;
    let _l = LocksOff::set(true);
    let o = hammer(100).await;
    let conflicts = o.failed.iter().filter(|f| f.contains("db.conflict")).count();
    eprintln!(
        "NO_LOCK memory: attempted={:?} ok={:?} failed={} (conflict 409s={conflicts}) persons={} world_rows={} observation_rows={} version={} (successful observation writes={})",
        o.attempted, o.ok, o.failed.len(), o.persons, o.worlds, o.observations, o.version, o.ok[1]
    );
    assert_eq!(o.failed.len(), conflicts, "only conflicts may fail, got: {:?}", o.failed.iter().find(|f| !f.contains("db.conflict")));
    assert_eq!(o.persons, 1, "one Alice: the deterministic id makes racing creators collide instead of duplicate");
    assert!(o.observations <= 1, "one observation row at most");
    assert_eq!(o.worlds, o.ok[0] as i64, "every fact reported ok is stored once (inserts have unique ids, no lost-update race)");
}

/// Claim every job with `claimers` concurrent loops (no handler: claim only), return how many times
/// each job was handed out.
async fn claim_all(jobs_n: usize, claimers: usize) -> (HashMap<String, u32>, u32) {
    let state = common::bare_state().await;
    for i in 0..jobs_n {
        jobs::enqueue(&state.control, NewJob::new("t", RecordId::new("user", format!("u{}", i % 5)), format!("k{i}"))).await.unwrap();
    }
    let handed: std::sync::Arc<std::sync::Mutex<HashMap<String, u32>>> = Default::default();
    let conflicts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let mut tasks = Vec::new();
    for w in 0..claimers {
        let (state, handed, conflicts) = (state.clone(), handed.clone(), conflicts.clone());
        tasks.push(tokio::spawn(async move {
            let mut empty = 0;
            while empty < 3 {
                match jobs::claim(&state.control, &format!("w{w}"), 5, Duration::from_secs(60), 1000).await {
                    Ok(batch) if batch.is_empty() => empty += 1,
                    Ok(batch) => {
                        empty = 0;
                        let mut h = handed.lock().unwrap();
                        for j in batch {
                            *h.entry(format!("{:?}", j.id)).or_default() += 1;
                        }
                    }
                    // the worker loop treats this as "nothing now" and polls again; so does this
                    Err(e) if e.code == eunomia_backend::error::ErrorCode::DbConflict => {
                        conflicts.fetch_add(1, Ordering::Relaxed);
                        empty = 0;
                    }
                    Err(e) => panic!("claim failed with something other than a conflict: {e:?}"),
                }
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    let h = handed.lock().unwrap().clone();
    (h, conflicts.load(Ordering::Relaxed))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn job_claims_with_the_lock_on_hand_out_each_job_once() {
    let _s = SERIAL.lock().await;
    let _l = LocksOff::set(false);
    let (h, conflicts) = claim_all(200, 4).await;
    assert_eq!((h.len(), conflicts), (200, 0));
    assert!(h.values().all(|n| *n == 1));
}

/// Lock off: asserted, no job is lost and `claim` only ever fails with a retryable conflict (after its own retries).
/// Measured: how many jobs were handed to two workers (the engine missing the write-write race).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn job_claims_with_the_lock_off_lose_no_job() {
    let _s = SERIAL.lock().await;
    let _l = LocksOff::set(true);
    let (h, conflicts) = claim_all(200, 4).await;
    let doubles = h.values().filter(|n| **n > 1).count();
    eprintln!("NO_LOCK claim: jobs=200 claimers=4 handed_out={} handed_out_twice={doubles} claim_calls_that_exhausted_retries_409={conflicts}", h.len());
    assert_eq!(h.len(), 200, "every job is claimed at least once");
}

/// Two replicas booting an empty install and taking their first signup at once (lock off stands in
/// for two processes): both must land in one org, not make a "Default" each.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_first_signups_converge_on_one_org() {
    let _s = SERIAL.lock().await;
    let _l = LocksOff::set(true);
    let state = common::bare_state().await;
    let (a, b) = tokio::join!(common::register(&state, "first-a@example.com"), common::register(&state, "first-b@example.com"));
    assert_eq!(a.org, b.org, "one install, one org");
    let mut res = state.control.test_raw().query("SELECT count() FROM org GROUP ALL").await.unwrap();
    let n: Option<i64> = res.take("count").unwrap();
    assert_eq!(n, Some(1));
    // both signups saw an empty org and both provisioned it (idempotent): still only one may own it
    let mut res = state.control.test_raw().query("SELECT count() FROM membership WHERE role = 'owner' GROUP ALL").await.unwrap();
    let owners: Option<i64> = res.take("count").unwrap();
    assert_eq!(owners, Some(1), "exactly one owner");
}

/// Lock off: four writers append to one vault's change log at once. Asserted: the seqs are exactly
/// 1..=n, no repeat and no gap, whatever the engine does with the racing counter updates. Measured
/// and printed: how many of the appends made it (one that exhausts its retries is logged, not written).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn change_log_appends_with_the_lock_off_have_no_gap_or_repeat() {
    use eunomia_backend::changes;
    let _s = SERIAL.lock().await;
    let _l = LocksOff::set(true);
    let app = TestApp::new().await;
    let db = app.db().await;
    let vault = common::sys(eunomia_backend::vaults::service::default_vault_id(&db, &app.user.id)).await.unwrap();
    let mut tasks = Vec::new();
    for t in 0..4 {
        let (db, vault) = (db.clone(), vault.clone());
        tasks.push(tokio::spawn(async move {
            for i in 0..50 {
                changes::record(&db, &vault, changes::CREATED, changes::MEMORY, &RecordId::new("memory", format!("m{t}x{i}"))).await;
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    let seqs: Vec<i64> = changes::after(&db, &vault, 0, 1000).await.unwrap().changes.iter().map(|c| c.seq).collect();
    eprintln!("NO_LOCK changes: attempted=200 written={}", seqs.len());
    assert_eq!(seqs, (1..=seqs.len() as i64).collect::<Vec<_>>(), "consecutive from 1");
    assert!(seqs.len() > 150, "most appends land: {}", seqs.len());
}
