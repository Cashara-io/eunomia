//! Job queue behaviour against a real in-memory SurrealDB: concurrent claims, crash recovery,
//! retries, leader election and the reconcilers. Plus spike S3 (`job_claim_throughput`, ignored).

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{bare_state, test_settings};
use eunomia_backend::error::ErrorCode;
use eunomia_backend::jobs::worker::{self, Registry};
use eunomia_backend::jobs::{self, handlers, kind, leader, Job, JobError, NewJob, WorkerConfig};
use eunomia_backend::models_user::User;
use eunomia_backend::state::AppState;
use serde::Deserialize;
use serde_json::{json, Value};
use surrealdb::types::SurrealValue;
use eunomia_backend::rid::RecordIdExt;
use surrealdb::types::RecordId;
use tokio::sync::watch;
use tokio::task::JoinHandle;

fn owner(n: u32) -> RecordId {
    RecordId::from_table_key("user", format!("u{n}"))
}

fn cfg(id: &str) -> WorkerConfig {
    WorkerConfig {
        id: id.into(),
        concurrency: 4,
        poll: Duration::from_millis(20),
        lease: Duration::from_secs(30),
        owner_cap: 1000,
        grace: Duration::from_millis(200),
        retry_base: Duration::from_millis(10),
        tick: Duration::from_secs(30),
    }
}

fn spawn_worker(state: &AppState, reg: Registry, cfg: WorkerConfig) -> (JoinHandle<()>, watch::Sender<bool>) {
    let (tx, rx) = watch::channel(false);
    (tokio::spawn(worker::run(state.clone(), reg, cfg, rx)), tx)
}

async fn wait_for<F: std::future::Future<Output = bool>>(what: &str, secs: u64, mut f: impl FnMut() -> F) {
    let start = Instant::now();
    while !f().await {
        assert!(start.elapsed() < Duration::from_secs(secs), "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[derive(Debug, Deserialize, SurrealValue)]
struct Row {
    status: String,
    attempts: i64,
    last_error_code: Option<String>,
}

async fn rows(state: &AppState, kind: &str) -> Vec<Row> {
    let mut res = state.control.test_raw().query("SELECT status, attempts, last_error_code FROM job WHERE kind = $k").bind(("k", kind.to_string())).await.unwrap();
    res.take(0).unwrap()
}

/// A count in the control database (the job queue).
async fn count(state: &AppState, q: &str) -> i64 {
    let mut res = state.control.test_raw().query(q).await.unwrap();
    let n: Option<i64> = res.take("n").unwrap();
    n.unwrap_or(0)
}

/// A count in the user's org database.
async fn org_count(state: &AppState, user: &User, q: &str) -> i64 {
    let mut res = common::org_db(state, user).await.test_raw().query(q).await.unwrap();
    let n: Option<i64> = res.take("n").unwrap();
    n.unwrap_or(0)
}

#[tokio::test]
async fn enqueue_dedupes_on_idempotency_key() {
    let state = bare_state().await;
    let mk = |key: &str| NewJob::new("t", owner(1), key).payload(json!({"x": 1}));
    assert!(jobs::enqueue(&state.control, mk("same")).await.unwrap());
    assert!(!jobs::enqueue(&state.control, mk("same")).await.unwrap());
    assert!(jobs::enqueue(&state.control, mk("other")).await.unwrap());
    // concurrent enqueues of one key: exactly one wins
    let wins = futures::future::join_all((0..8).map(|_| jobs::enqueue(&state.control, mk("race")))).await;
    assert_eq!(wins.into_iter().filter(|r| *r.as_ref().unwrap()).count(), 1);
    assert_eq!(count(&state, "SELECT count() AS n FROM job GROUP ALL").await, 3);
}

/// A `migrate_tenant` that died (issue #85: 0017 failed on every attempt) is retried when a process
/// next becomes leader, so a release that fixes the cause heals the org; its key alone would keep the
/// dead job in place forever. Other dead jobs stay dead.
#[tokio::test]
async fn a_dead_org_migration_is_retried_by_the_next_leader() {
    let state = bare_state().await;
    state
        .control
        .test_raw()
        .query(
            "CREATE job SET kind = 'migrate_tenant', owner = user:u1, idempotency_key = 'migrate_tenant:o:18', status = 'dead', attempts = 5, last_error_code = 'internal';
             CREATE job SET kind = 'sync', owner = user:u1, idempotency_key = 'sync:x', status = 'dead', attempts = 5;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
    eunomia_backend::jobs::leader::revive_dead_migrations(&state.control).await;
    let m = rows(&state, "migrate_tenant").await;
    assert_eq!((m[0].status.as_str(), m[0].attempts, m[0].last_error_code.as_deref()), ("ready", 0, None));
    assert_eq!(rows(&state, "sync").await[0].status, "dead");
    // the same key still dedupes a fresh enqueue: one row, not two
    assert!(!jobs::enqueue(&state.control, NewJob::new("migrate_tenant", owner(1), "migrate_tenant:o:18")).await.unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_workers_run_each_job_once() {
    let state = bare_state().await;
    for i in 0..300 {
        jobs::enqueue(&state.control, NewJob::new("t", owner(i % 5), format!("k{i}"))).await.unwrap();
    }
    let runs: Arc<Mutex<HashMap<String, u32>>> = Default::default();
    let reg = {
        let runs = runs.clone();
        Registry::new().register("t", move |_s, job: Job| {
            let runs = runs.clone();
            async move {
                *runs.lock().unwrap().entry(job.id.to_string()).or_default() += 1;
                tokio::time::sleep(Duration::from_millis(2)).await;
                Ok(())
            }
        })
    };
    let workers: Vec<_> = (0..4).map(|i| spawn_worker(&state, reg.clone(), cfg(&format!("w{i}")))).collect();
    wait_for("all jobs done", 60, || async { count(&state, "SELECT count() AS n FROM job WHERE status != 'done' GROUP ALL").await == 0 }).await;
    let runs = runs.lock().unwrap();
    assert_eq!(runs.len(), 300);
    assert!(runs.values().all(|n| *n == 1), "a job ran twice: {:?}", runs.iter().filter(|(_, n)| **n > 1).collect::<Vec<_>>());
    for (_, stop) in &workers {
        let _ = stop.send(true);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn crashed_worker_job_is_reclaimed_after_the_lease() {
    let state = bare_state().await;
    jobs::enqueue(&state.control, NewJob::new("t", owner(1), "crash")).await.unwrap();
    let started = Arc::new(AtomicU32::new(0));
    let finished = Arc::new(AtomicU32::new(0));
    let reg = {
        let (started, finished) = (started.clone(), finished.clone());
        Registry::new().register("t", move |_s, _j| {
            let (started, finished) = (started.clone(), finished.clone());
            async move {
                // the first run hangs (and is killed); the second finishes
                if started.fetch_add(1, Ordering::SeqCst) == 0 {
                    std::future::pending::<()>().await;
                }
                finished.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        })
    };
    let lease = Duration::from_millis(600);
    let (a, _keep_a) = spawn_worker(&state, reg.clone(), WorkerConfig { lease, ..cfg("a") });
    wait_for("first attempt to start", 10, || async { started.load(Ordering::SeqCst) == 1 }).await;
    a.abort(); // a crash: no completion, no lease release
    let crashed_at = Instant::now();

    let (_b, _keep_b) = spawn_worker(&state, reg, WorkerConfig { lease, ..cfg("b") });
    wait_for("reclaim", 10, || async { rows(&state, "t").await[0].status == "done" }).await;
    assert!(crashed_at.elapsed() >= Duration::from_millis(300), "reclaimed before the lease could have expired");
    let r = rows(&state, "t").await;
    assert_eq!((r[0].status.as_str(), r[0].attempts), ("done", 2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn heartbeats_keep_a_long_job_from_being_stolen() {
    let state = bare_state().await;
    jobs::enqueue(&state.control, NewJob::new("t", owner(1), "long")).await.unwrap();
    let runs = Arc::new(AtomicU32::new(0));
    let reg = {
        let runs = runs.clone();
        Registry::new().register("t", move |_s, _j| {
            let runs = runs.clone();
            async move {
                runs.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(1500)).await; // 3x the lease
                Ok(())
            }
        })
    };
    let lease = Duration::from_millis(500);
    let ws: Vec<_> = (0..2).map(|i| spawn_worker(&state, reg.clone(), WorkerConfig { lease, ..cfg(&format!("w{i}")) })).collect();
    wait_for("done", 20, || async { count(&state, "SELECT count() AS n FROM job WHERE status = 'done' GROUP ALL").await == 1 }).await;
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    drop(ws);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failures_back_off_then_dead_letter() {
    let state = bare_state().await;
    let mut transient = NewJob::new("flaky", owner(1), "flaky");
    transient.max_attempts = 3;
    jobs::enqueue(&state.control, transient).await.unwrap();
    jobs::enqueue(&state.control, NewJob::new("bad", owner(1), "bad")).await.unwrap();
    jobs::enqueue(&state.control, NewJob::new("nobody", owner(1), "nobody")).await.unwrap();
    jobs::enqueue(&state.control, NewJob::new("boom", owner(1), "boom")).await.unwrap();
    let tries = Arc::new(AtomicU32::new(0));
    let reg = {
        let tries = tries.clone();
        Registry::new()
            .register("flaky", move |_s, _j| {
                let tries = tries.clone();
                async move {
                    tries.fetch_add(1, Ordering::SeqCst);
                    Err(JobError::retryable(ErrorCode::Internal, "try again"))
                }
            })
            .register("bad", |_s, _j| async { Err(JobError::permanent(ErrorCode::ValidationInvalid, "never works")) })
            .register("boom", |_s, _j| async { panic!("handler bug") })
    };
    let _w = spawn_worker(&state, reg, WorkerConfig { retry_base: Duration::from_millis(50), ..cfg("w") });
    wait_for("all dead", 30, || async { count(&state, "SELECT count() AS n FROM job WHERE status = 'dead' GROUP ALL").await == 4 }).await;

    let flaky = &rows(&state, "flaky").await[0];
    assert_eq!((flaky.attempts, flaky.last_error_code.as_deref()), (3, Some("internal")));
    assert_eq!(tries.load(Ordering::SeqCst), 3);
    let bad = &rows(&state, "bad").await[0];
    assert_eq!((bad.attempts, bad.last_error_code.as_deref()), (1, Some("validation.invalid")));
    assert_eq!(rows(&state, "nobody").await[0].last_error_code.as_deref(), Some("job.no_handler"));
    let boom = &rows(&state, "boom").await[0];
    assert_eq!((boom.attempts, boom.last_error_code.as_deref()), (5, Some("job.panicked")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retry_waits_for_its_backoff() {
    let state = bare_state().await;
    jobs::enqueue(&state.control, NewJob::new("t", owner(1), "wait")).await.unwrap();
    let stamps: Arc<Mutex<Vec<Instant>>> = Default::default();
    let reg = {
        let stamps = stamps.clone();
        Registry::new().register("t", move |_s, _j| {
            let stamps = stamps.clone();
            async move {
                stamps.lock().unwrap().push(Instant::now());
                Err(JobError::retryable(ErrorCode::Internal, "no"))
            }
        })
    };
    let _w = spawn_worker(&state, reg, WorkerConfig { retry_base: Duration::from_millis(400), ..cfg("w") });
    wait_for("two attempts", 10, || async { stamps.lock().unwrap().len() >= 2 }).await;
    let s = stamps.lock().unwrap();
    assert!(s[1] - s[0] >= Duration::from_millis(190), "retried after only {:?}", s[1] - s[0]); // 50% jitter floor of 400ms
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owner_cap_limits_running_jobs_per_owner() {
    let state = bare_state().await;
    for i in 0..6 {
        jobs::enqueue(&state.control, NewJob::new("t", owner(1), format!("a{i}"))).await.unwrap();
    }
    jobs::enqueue(&state.control, NewJob::new("t", owner(2), "b0")).await.unwrap();
    let first = jobs::claim(&state.control, "w", 3, Duration::from_secs(30), 2).await.unwrap();
    assert_eq!(first.len(), 3); // soft cap: one batch may overshoot
    let second = jobs::claim(&state.control, "w", 3, Duration::from_secs(30), 2).await.unwrap();
    assert_eq!(second.len(), 1, "owner 1 is at its cap, only owner 2 can claim");
    assert_eq!(second[0].owner, owner(2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_releases_leases_without_burning_an_attempt() {
    let state = bare_state().await;
    jobs::enqueue(&state.control, NewJob::new("t", owner(1), "slow")).await.unwrap();
    let started = Arc::new(AtomicU32::new(0));
    let reg = {
        let started = started.clone();
        Registry::new().register("t", move |_s, _j| {
            let started = started.clone();
            async move {
                started.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok(())
            }
        })
    };
    let (handle, stop) = spawn_worker(&state, reg, cfg("w"));
    wait_for("start", 10, || async { started.load(Ordering::SeqCst) == 1 }).await;
    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), handle).await.expect("worker stops within the grace period").unwrap();
    let r = &rows(&state, "t").await[0];
    assert_eq!((r.status.as_str(), r.attempts), ("ready", 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_leader_at_a_time() {
    let state = bare_state().await;
    let ttl = Duration::from_millis(400);
    for _ in 0..5 {
        // reset the lease between rounds
        jobs::release_leader(&state.control, "a").await.unwrap();
        jobs::release_leader(&state.control, "b").await.unwrap();
        let (a, b) = tokio::join!(jobs::acquire_leader(&state.control, "a", ttl), jobs::acquire_leader(&state.control, "b", ttl));
        assert!(a.unwrap() ^ b.unwrap(), "exactly one candidate leads");
    }
    jobs::release_leader(&state.control, "a").await.unwrap();
    jobs::release_leader(&state.control, "b").await.unwrap();
    assert!(jobs::acquire_leader(&state.control, "a", ttl).await.unwrap());
    assert!(jobs::acquire_leader(&state.control, "a", ttl).await.unwrap(), "the leader renews");
    assert!(!jobs::acquire_leader(&state.control, "b", ttl).await.unwrap());
    tokio::time::sleep(ttl + Duration::from_millis(100)).await;
    assert!(jobs::acquire_leader(&state.control, "b", ttl).await.unwrap(), "takeover after the lease expires");
    assert!(!jobs::acquire_leader(&state.control, "a", ttl).await.unwrap());
    jobs::release_leader(&state.control, "b").await.unwrap();
    assert!(jobs::acquire_leader(&state.control, "a", ttl).await.unwrap(), "a released lease is free at once");
}

/// A state whose embeddings backend is `backend` ("stub": embeddings work offline, no LLM;
/// "openai": an unroutable endpoint, so chat counts as available but calls fail).
async fn state_with(backend: &str) -> AppState {
    let mut settings = test_settings();
    settings.embeddings_backend = backend.into();
    AppState::build(&settings, common::engine_config()).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler_enqueues_due_syncs_once_per_window() {
    let state = bare_state().await;
    let user = common::register(&state, "sched@example.com").await;
    common::org_db(&state, &user).await.test_raw().query("CREATE connector SET owner = $o, kind = 'up_bank', enabled = true").bind(("o", user.id.clone())).await.unwrap().check().unwrap();

    leader::tick(&state, 1).await;
    leader::tick(&state, 2).await; // a second tick (or a second leader) must not double-enqueue
    let r = rows(&state, kind::SYNC).await;
    assert_eq!(r.len(), 1, "one sync job for the one enabled source");

    // The worker runs it through the real handler. up_bank has no credentials, so the sync fails
    // and sync_status records the failure and backoff exactly as before.
    let (_w, _stop) = spawn_worker(&state, handlers::registry(), cfg("w"));
    wait_for("sync job done", 20, || async { rows(&state, kind::SYNC).await[0].status == "done" }).await;
    let mut res = common::org_db(&state, &user).await.test_raw().query("SELECT consecutive_failures AS n FROM sync_status").await.unwrap();
    assert_eq!(res.take::<Option<i64>>("n").unwrap(), Some(1));
    // inside its backoff window the source is no longer due
    leader::tick(&state, 3).await;
    assert_eq!(rows(&state, kind::SYNC).await.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_observation_reconciler_picks_up_work() {
    let state = state_with("openai").await;
    let user = common::register(&state, "obs@example.com").await;
    // point this user's LLM at an unroutable local port: "configured", but every call fails fast
    let settings_id = RecordId::from_table_key("app_settings", user.id.key().clone());
    common::org_db(&state, &user)
        .await
        .test_raw()
        .query("UPSERT $id SET owner = $o, openai_base_url = 'http://127.0.0.1:9/v1'")
        .bind(("id", settings_id))
        .bind(("o", user.id.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();
    let w = |text: &str, ty: &str| {
        let args = json!({"subject_name": "Alice", "subject_kind": "person", "text": text, "type": ty});
        common::sys(eunomia_backend::tools::registry::call(&state, &user, "memory_write", args))
    };
    w("likes tea", "world").await.unwrap();
    w("Alice likes tea.", "observation").await.unwrap();
    w("works remote", "world").await.unwrap(); // marks the observation stale
    assert_eq!(org_count(&state, &user, "SELECT count() AS n FROM memory WHERE type = 'observation' AND status = 'stale' GROUP ALL").await, 1);

    leader::tick(&state, 1).await;
    leader::tick(&state, 2).await;
    assert_eq!(rows(&state, kind::CONSOLIDATE).await.len(), 1, "one consolidation, deduped across ticks");

    // The handler runs (its LLM call cannot connect and is logged), so the job still finishes.
    let (_w, _stop) = spawn_worker(&state, handlers::registry(), cfg("w"));
    wait_for("consolidate job done", 30, || async { rows(&state, kind::CONSOLIDATE).await[0].status == "done" }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn embed_reconciler_fills_missing_embeddings() {
    let state = state_with("stub").await;
    let user = common::register(&state, "emb@example.com").await;
    let env: eunomia_backend::cache::search::Envelope = serde_json::from_value(json!({
        "id": "demo:note:1", "source": "demo", "type": "note", "external_id": "1", "title": "Hello", "body_text": "A body that needs a vector."
    }))
    .unwrap();
    eunomia_backend::cache::search::upsert(&common::org_db(&state, &user).await, &user.id, &env).await.unwrap();
    let missing = "SELECT count() AS n FROM cache_record WHERE embedding IS NONE GROUP ALL";
    assert_eq!(org_count(&state, &user, missing).await, 1);

    leader::tick(&state, 0).await; // tick 0 runs the slow probes
    assert_eq!(rows(&state, kind::EMBED).await.len(), 1);
    let (_w, _stop) = spawn_worker(&state, handlers::registry(), cfg("w"));
    wait_for("embedded", 20, || async { org_count(&state, &user, missing).await == 0 }).await;
    wait_for("embed job done", 20, || async { rows(&state, kind::EMBED).await[0].status == "done" }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ingest_queues_extraction_and_the_handler_noops_without_a_key() {
    let state = state_with("stub").await; // stub: no LLM
    let user = common::register(&state, "ing@example.com").await;
    let raw = vec![json!({"_kind": "txn", "id": "1", "account": "Spending", "description": "Alice met Bob in Paris and they talked about the project at length.", "category": "Social", "amount_cents": -500, "created_at": "2026-01-05T09:00:00Z"})];
    let src = eunomia_backend::sources::registry::get("demo").unwrap();
    let report = eunomia_backend::sources::registry::ingest(&state.org(&user.org).await.unwrap(), &user.id, &raw, src.as_ref()).await.unwrap();
    assert_eq!(report.written, 1);
    assert_eq!(rows(&state, kind::EXTRACT).await.len(), 1);
    let (_w, _stop) = spawn_worker(&state, handlers::registry(), cfg("w"));
    wait_for("extract job done", 20, || async { rows(&state, kind::EXTRACT).await[0].status == "done" }).await;
}

// --- spike S3 ---

/// `n` ready jobs of kind `spike` spread over 8 owners, inserted in bulk.
async fn seed_spike_jobs(state: &AppState, n: usize) {
    for chunk in (0..n).collect::<Vec<_>>().chunks(2_500) {
        let rows: Vec<Value> = chunk.iter().map(|i| json!({"k": format!("s3-{i}"), "o": format!("user:u{}", i % 8)})).collect();
        state
            .control
            .test_raw()
            .query("INSERT INTO job (SELECT <record> o AS owner, 'spike' AS kind, k AS idempotency_key, 50 AS max_attempts FROM $rows)")
            .bind(("rows", rows))
            .await
            .unwrap()
            .check()
            .unwrap();
    }
    assert_eq!(count(state, "SELECT count() AS n FROM job GROUP ALL").await, n as i64);
}

/// (job id, locked_until at claim) per execution, in start order.
type Execs = Arc<Mutex<Vec<(String, chrono::DateTime<chrono::Utc>)>>>;

/// A `spike` handler that records each execution in `execs`.
fn spike_registry(execs: Execs) -> Registry {
    Registry::new().register("spike", move |_s, job: Job| {
        let execs = execs.clone();
        async move {
            let until = eunomia_backend::sources::base::datetime_to_chrono(job.locked_until.as_ref().unwrap()).unwrap();
            execs.lock().unwrap().push((job.id.to_string(), until));
            // most jobs are instant; a few are slow enough for a kill to land mid-job
            if rand::random::<u8>() < 5 {
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
            Ok(())
        }
    })
}

/// `cargo test --release -- --ignored job_claim_throughput --nocapture`
/// 50k jobs, 6 workers of 16 slots each, random worker kills (task aborts, no lease release).
/// Checks: no job lost; a job runs twice only after its first lease expired. Prints claims/sec.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore]
async fn job_claim_throughput() {
    const JOBS: usize = 50_000;
    const WORKERS: usize = 6;
    let lease = Duration::from_secs(3);
    let state = bare_state().await;
    seed_spike_jobs(&state, JOBS).await;
    let execs: Execs = Default::default();
    let reg = spike_registry(execs.clone());
    let mk = |i: usize| WorkerConfig { concurrency: 16, lease, poll: Duration::from_millis(50), ..cfg(&format!("s3-{i}")) };
    let (stop, rx) = watch::channel(false);
    let start = Instant::now();
    let mut workers: Vec<JoinHandle<()>> = (0..WORKERS).map(|i| tokio::spawn(worker::run(state.clone(), reg.clone(), mk(i), rx.clone()))).collect();

    let mut kills = 0;
    let mut next_id = WORKERS;
    loop {
        tokio::time::sleep(Duration::from_millis(400)).await;
        if count(&state, "SELECT count() AS n FROM job WHERE status != 'done' GROUP ALL").await == 0 {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(600), "spike did not finish");
        if kills < 25 && start.elapsed() < Duration::from_secs(20) {
            let victim = rand::random::<usize>() % WORKERS;
            workers[victim].abort();
            kills += 1;
            workers[victim] = tokio::spawn(worker::run(state.clone(), reg.clone(), mk(next_id), rx.clone()));
            next_id += 1;
        }
    }
    let elapsed = start.elapsed();
    let _ = stop.send(true);

    let dead = count(&state, "SELECT count() AS n FROM job WHERE status = 'dead' GROUP ALL").await;
    let execs = execs.lock().unwrap();
    let mut by_job: HashMap<&str, Vec<chrono::DateTime<chrono::Utc>>> = HashMap::new();
    for (id, until) in execs.iter() {
        by_job.entry(id).or_default().push(*until);
    }
    let mut early_dupes = 0;
    let mut reclaims = 0;
    for runs in by_job.values() {
        for pair in runs.windows(2) {
            reclaims += 1;
            // the second claim happened at (its locked_until - lease); legal only after the first lease ended
            let second_claimed = pair[1] - chrono::Duration::from_std(lease).unwrap();
            if second_claimed < pair[0] - chrono::Duration::milliseconds(1) {
                early_dupes += 1;
            }
        }
    }
    println!(
        "S3: jobs={JOBS} workers={WORKERS}x16 kills={kills} elapsed={:.1}s executions={} lost={} dead={dead} \
         reclaims_after_lease_expiry={} duplicates_before_expiry={early_dupes} claims_per_sec={:.0}",
        elapsed.as_secs_f64(),
        execs.len(),
        JOBS - by_job.len(),
        reclaims - early_dupes,
        JOBS as f64 / elapsed.as_secs_f64(),
    );
    assert_eq!(by_job.len(), JOBS, "lost jobs");
    assert_eq!(dead, 0);
    assert_eq!(early_dupes, 0, "a job ran twice before its lease expired");
}

// --- spike S3 across processes ---

const MULTI_PROCS: usize = 4;
const MULTI_JOBS: usize = 6_000;

/// Exactly-once claims on the deployed topology: `MULTI_PROCS` separate OS processes (this test binary
/// re-run as `job_claims_across_processes_child`), each with its own connection and one worker loop of
/// 16 slots, claim from one real server. Nothing in one process can fence another (`tx::lock` is
/// per-process), so only the server's commit-time conflict check keeps claims exclusive. The lease is
/// long and nothing is killed, so any job run twice was claimed twice. Asserts: every job ran exactly
/// once, the server agrees (every row `done` with `attempts = 1`), none dead. Needs a real server:
/// `TEST_SURREAL_URL=ws://127.0.0.1:8231/rpc cargo test --release --test jobs job_claims_across_processes -- --nocapture`
/// against `surrealdb/surrealdb:v3.3.5` on RocksDB (docs/testing.md, "Concurrency ceiling"); skips without one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn job_claims_across_processes() {
    if std::env::var("TEST_SURREAL_URL").is_err() {
        eprintln!("job_claims_across_processes skipped: set TEST_SURREAL_URL to a real server (docs/testing.md)");
        return;
    }
    let state = bare_state().await;
    seed_spike_jobs(&state, MULTI_JOBS).await;
    let dir = std::env::temp_dir().join(format!("eunomia-claims-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut children = Vec::new();
    for i in 0..MULTI_PROCS {
        let child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args(["job_claims_across_processes_child", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
            .env("CLAIM_CHILD_NS", &state.settings.surreal_ns)
            .env("CLAIM_CHILD_DIR", &dir)
            .env("CLAIM_CHILD_N", i.to_string())
            .stdout(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        children.push(child);
    }
    // start them together, once every one is connected
    let ready = || (0..MULTI_PROCS).all(|i| dir.join(format!("{i}.ready")).exists());
    wait_for("every child to connect", 120, || async { ready() }).await;
    let start = Instant::now();
    std::fs::write(dir.join("go"), "").unwrap();
    for mut c in children {
        let status = tokio::time::timeout(Duration::from_secs(600), c.wait()).await.expect("child timed out").unwrap();
        assert!(status.success(), "a child process failed: {status}");
    }
    let elapsed = start.elapsed();

    let mut runs: HashMap<String, u32> = HashMap::new();
    let mut per_proc = Vec::new();
    for i in 0..MULTI_PROCS {
        let text = std::fs::read_to_string(dir.join(format!("{i}.txt"))).unwrap();
        per_proc.push(text.lines().count());
        for id in text.lines() {
            *runs.entry(id.to_string()).or_default() += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    let executions: u32 = runs.values().sum();
    let twice = runs.values().filter(|n| **n > 1).count();
    let not_once = count(&state, "SELECT count() AS n FROM job WHERE status != 'done' OR attempts != 1 GROUP ALL").await;
    let dead = count(&state, "SELECT count() AS n FROM job WHERE status = 'dead' GROUP ALL").await;
    println!(
        "S3 multi-process: server={} procs={MULTI_PROCS}x16 jobs={MULTI_JOBS} per_proc={per_proc:?} elapsed={:.1}s executions={executions} \
         claimed_twice={twice} lost={} rows_not_done_once={not_once} dead={dead} claims_per_sec={:.0}",
        state.settings.surreal_url,
        elapsed.as_secs_f64(),
        MULTI_JOBS - runs.len(),
        MULTI_JOBS as f64 / elapsed.as_secs_f64(),
    );
    assert_eq!(twice, 0, "a job was claimed by two workers");
    assert_eq!(runs.len(), MULTI_JOBS, "lost jobs");
    assert_eq!(executions as usize, MULTI_JOBS);
    assert_eq!((not_once, dead), (0, 0), "the server's view: every job done after exactly one claim");
    assert!(per_proc.iter().filter(|n| **n > 0).count() > 1, "only one process claimed anything, so nothing raced: {per_proc:?}");
}

/// One process of `job_claims_across_processes`; does nothing unless that test started it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn job_claims_across_processes_child() {
    let (Ok(ns), Ok(dir), Ok(n)) = (std::env::var("CLAIM_CHILD_NS"), std::env::var("CLAIM_CHILD_DIR"), std::env::var("CLAIM_CHILD_N")) else { return };
    let dir = std::path::PathBuf::from(dir);
    let settings = eunomia_backend::config::Settings { surreal_ns: ns, ..test_settings() };
    let state = AppState::attach(&settings, common::engine_config()).await.unwrap();
    std::fs::write(dir.join(format!("{n}.ready")), "").unwrap();
    wait_for("go", 120, || async { dir.join("go").exists() }).await;

    let execs: Execs = Default::default();
    let (stop, rx) = watch::channel(false);
    let config = WorkerConfig { concurrency: 16, lease: Duration::from_secs(120), poll: Duration::from_millis(50), ..cfg(&format!("proc-{n}")) };
    let worker = tokio::spawn(worker::run(state.clone(), spike_registry(execs.clone()), config, rx));
    wait_for("the queue to drain", 600, || async { count(&state, "SELECT count() AS n FROM job WHERE status != 'done' GROUP ALL").await == 0 }).await;
    let _ = stop.send(true);
    worker.await.unwrap();
    let ids: String = execs.lock().unwrap().iter().map(|(id, _)| format!("{id}\n")).collect();
    std::fs::write(dir.join(format!("{n}.txt")), ids).unwrap();
}
