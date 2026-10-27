//! Background jobs: a `job` table in SurrealDB, claimed with leases by worker loops.
//! Design and operations: docs/architecture/jobs.md.
//!
//! This file is the queue itself (enqueue, claim, complete, fail, leader lease);
//! `worker` runs handlers, `leader` runs the periodic reconcilers, `handlers`
//! holds the built-in job kinds.

use surrealdb::types::SurrealValue;
use std::time::Duration;

use rand::Rng;
use serde::Deserialize;
use serde_json::{json, Value};
use surrealdb::types::{Datetime, RecordId};
use crate::rid::RecordIdExt;

use crate::pool::{ControlDb, OrgId};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::sources::base::owner_key_str;
use crate::store::jobs as q;
use crate::tx::{stable_key, with_retry, with_retry_dup};

pub mod handlers;
pub mod leader;
pub mod redact_backfill;
pub mod worker;

/// Job kinds the built-in handlers serve.
pub mod kind {
    pub const SYNC: &str = "sync";
    pub const EMBED: &str = "embed";
    pub const EXTRACT: &str = "extract";
    pub const CONSOLIDATE: &str = "consolidate";
    pub const PRUNE_CAPSULES: &str = "prune_capsules";
    /// Audit rows past their retention and long-expired sessions.
    pub const PRUNE_AUTH: &str = "prune_auth";
    /// Bring one org's database to the latest tenant schema (the per-org fan-out of a migration).
    pub const MIGRATE_TENANT: &str = "migrate_tenant";
    /// Extract, chunk and index one revision of an uploaded document (`documents::index_job`).
    pub const INDEX_DOCUMENT: &str = "index_document";
    /// Mark the earlier facts a newly written one makes no longer true (`entities::supersede`).
    pub const SUPERSEDE: &str = "supersede";
    /// Redact secrets stored before writes were redacted, once per org per detector version (`redact_backfill`).
    pub const REDACT_BACKFILL: &str = "redact_backfill";
}

/// Which loops this process runs (`EUNOMIA_ROLE`). `all` keeps the one-container install working.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Api,
    Worker,
    All,
}

impl Role {
    pub fn parse(s: &str) -> Option<Role> {
        match s.trim().to_ascii_lowercase().as_str() {
            "api" => Some(Role::Api),
            "worker" => Some(Role::Worker),
            "all" | "" => Some(Role::All),
            _ => None,
        }
    }

    /// Unset means `all`; a typo is a startup error, not a silent fallback.
    pub fn from_env() -> Role {
        let raw = std::env::var("EUNOMIA_ROLE").unwrap_or_default();
        Role::parse(&raw).unwrap_or_else(|| panic!("EUNOMIA_ROLE must be api, worker or all (got {raw:?})"))
    }

    pub fn serves_http(self) -> bool {
        self != Role::Worker
    }

    pub fn runs_jobs(self) -> bool {
        self != Role::Api
    }
}

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Unique per process; written to `job.locked_by` and `job_leader.holder`.
    pub id: String,
    pub concurrency: usize,
    pub poll: Duration,
    pub lease: Duration,
    /// Soft cap on running jobs per owner, across all workers.
    pub owner_cap: usize,
    /// How long shutdown waits for running jobs before aborting them and releasing their leases.
    pub grace: Duration,
    /// First retry delay; doubles per attempt up to `RETRY_MAX`.
    pub retry_base: Duration,
    /// Scheduler leader loop period; the leader lease lasts three of these.
    pub tick: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        WorkerConfig {
            id: format!("w-{}-{}", std::process::id(), &uuid::Uuid::new_v4().simple().to_string()[..6]),
            concurrency: 4,
            poll: Duration::from_secs(1),
            lease: Duration::from_secs(30),
            owner_cap: 4,
            grace: Duration::from_secs(8),
            retry_base: Duration::from_secs(5),
            tick: Duration::from_secs(30),
        }
    }
}

impl WorkerConfig {
    pub fn from_env() -> Self {
        fn num(key: &str) -> Option<u64> {
            std::env::var(key).ok()?.trim().parse().ok().filter(|n| *n > 0)
        }
        let d = WorkerConfig::default();
        WorkerConfig {
            concurrency: num("EUNOMIA_WORKER_CONCURRENCY").map_or(d.concurrency, |n| n as usize),
            poll: num("EUNOMIA_JOB_POLL_MS").map_or(d.poll, Duration::from_millis),
            lease: num("EUNOMIA_JOB_LEASE_SECS").map_or(d.lease, Duration::from_secs),
            owner_cap: num("EUNOMIA_JOB_OWNER_CAP").map_or(d.owner_cap, |n| n as usize),
            grace: num("EUNOMIA_JOB_GRACE_SECS").map_or(d.grace, Duration::from_secs),
            tick: num("EUNOMIA_SCHEDULER_TICK_SECS").map_or(d.tick, Duration::from_secs),
            ..d
        }
    }
}

/// A claimed (or listed) job row.
#[derive(Debug, Clone, Deserialize, SurrealValue)]
pub struct Job {
    pub id: RecordId,
    /// The org key whose database the handler works in; `None` for instance-level jobs.
    #[serde(default)]
    #[surreal(default)]
    pub org: Option<String>,
    pub kind: String,
    pub owner: RecordId,
    #[serde(default)]
    #[surreal(default)]
    pub payload: Value,
    pub idempotency_key: String,
    pub attempts: i64,
    pub max_attempts: i64,
    pub status: String,
    pub locked_until: Option<Datetime>,
    pub traceparent: Option<String>,
}

impl Job {
    /// The org this job works in.
    pub fn org_id(&self) -> Result<OrgId, JobError> {
        self.org
            .as_deref()
            .and_then(OrgId::parse)
            .ok_or_else(|| JobError::permanent(ErrorCode::ValidationInvalid, "job has no org"))
    }
}

/// What a handler returns on failure. `retry: false` dead-letters immediately.
#[derive(Debug, Clone)]
pub struct JobError {
    pub code: ErrorCode,
    pub message: String,
    pub retry: bool,
}

impl JobError {
    pub fn retryable(code: ErrorCode, message: impl Into<String>) -> Self {
        JobError { code, message: message.into(), retry: true }
    }

    pub fn permanent(code: ErrorCode, message: impl Into<String>) -> Self {
        JobError { code, message: message.into(), retry: false }
    }
}

impl From<AppError> for JobError {
    /// A 4xx means the input is wrong and a retry cannot help, except a write conflict.
    fn from(e: AppError) -> Self {
        let retry = !e.status.is_client_error() || e.code == ErrorCode::DbConflict;
        JobError { code: e.code, message: e.source.unwrap_or(e.message), retry }
    }
}

pub struct NewJob {
    pub kind: &'static str,
    pub org: Option<OrgId>,
    pub owner: RecordId,
    pub payload: Value,
    /// Same key, same job: enqueueing it twice creates one row.
    pub key: String,
    pub run_at: Option<chrono::DateTime<chrono::Utc>>,
    pub max_attempts: i64,
}

impl NewJob {
    pub fn new(kind: &'static str, owner: RecordId, key: impl Into<String>) -> Self {
        NewJob { kind, org: None, owner, payload: json!({}), key: key.into(), run_at: None, max_attempts: 5 }
    }

    /// Run the job against this org's database.
    pub fn in_org(mut self, org: OrgId) -> Self {
        self.org = Some(org);
        self
    }

    pub fn payload(mut self, payload: Value) -> Self {
        self.payload = payload;
        self
    }
}

/// Seconds since the epoch divided by `period_secs`: the window part of a periodic idempotency key.
pub fn window(period_secs: u64) -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64 / period_secs.max(1)
}

/// Key for owner-level periodic work, e.g. `embed:<owner>:<window>`.
pub fn periodic_key(kind: &str, owner: &RecordId, period_secs: u64) -> String {
    format!("{kind}:{}:{}", owner_key_str(owner), window(period_secs))
}

/// Create the job unless one with this key exists. `Ok(true)` means a new row.
pub async fn enqueue(db: &ControlDb, job: NewJob) -> AppResult<bool> {
    let _fence = crate::tx::lock(&job.key).await;
    let id = RecordId::from_table_key("job", stable_key('j', &job.key));
    let run_at = Datetime::from(job.run_at.unwrap_or_else(chrono::Utc::now));
    let traceparent = crate::telemetry::current_traceparent();
    let rows = with_retry_dup(|| async {
        let mut res = q::ENQUEUE
            .on(db)
            .bind(("id", id.clone()))
            .bind(("org", job.org.map(|o| o.key())))
            .bind(("kind", job.kind.to_string()))
            .bind(("owner", job.owner.clone()))
            .bind(("payload", job.payload.clone()))
            .bind(("key", job.key.clone()))
            .bind(("run_at", run_at))
            .bind(("max_attempts", job.max_attempts))
            .bind(("traceparent", traceparent.clone()))
            .await?
            .check()?;
        res.take::<Vec<RecordId>>(0)
    })
    .await?;
    Ok(!rows.is_empty())
}

/// Enqueue where a lost job only costs latency (a reconciler re-derives it): log and move on.
pub async fn enqueue_lossy(db: &ControlDb, job: NewJob) {
    let (kind, key) = (job.kind, job.key.clone());
    if let Err(e) = enqueue(db, job).await {
        tracing::warn!(kind, key, error = %e.message, "enqueue failed; the reconciler will retry");
    }
}

/// Take up to `n` due jobs for `worker`. An empty result means nothing is due or other workers won the race.
pub async fn claim(db: &ControlDb, worker: &str, n: usize, lease: Duration, owner_cap: usize) -> AppResult<Vec<Job>> {
    // ponytail: in-process fence. Across processes the storage engine's commit-time conflict check
    // (RocksDB, SurrealKV) decides the race; the in-memory engine (SurrealMX 0.27) misses it about
    // one tight race in ten, so tests that run several workers in one process rely on this lock.
    let _fence = crate::tx::lock("job.claim").await;
    let rows = with_retry(|| async {
        let mut res = q::CLAIM
            .on(db)
            .bind(("worker", worker.to_string()))
            .bind(("n", n as i64))
            .bind(("lease", format!("{}ms", lease.as_millis())))
            .bind(("cap", owner_cap as i64))
            .await?
            .check()?;
        res.take::<Vec<Job>>(CLAIM_RESULT)
    })
    .await?;
    Ok(rows)
}

/// Index of the claiming UPDATE in the CLAIM response: 3.x gives BEGIN and each LET a slot too.
const CLAIM_RESULT: usize = 4;

/// Extend the lease. `false` means it was lost (expired and reclaimed, or the job was released).
pub async fn heartbeat(db: &ControlDb, job: &Job, worker: &str, lease: Duration) -> AppResult<bool> {
    let mut res = q::HEARTBEAT
        .on(db)
        .bind(("id", job.id.clone()))
        .bind(("worker", worker.to_string()))
        .bind(("lease", format!("{}ms", lease.as_millis())))
        .await?
        .check()?;
    Ok(!res.take::<Vec<RecordId>>(0)?.is_empty())
}

/// Mark done. `false` means this worker no longer held the lease.
pub async fn complete(db: &ControlDb, job: &Job, worker: &str) -> AppResult<bool> {
    let mut res = with_retry(|| async {
        q::COMPLETE.on(db).bind(("id", job.id.clone())).bind(("worker", worker.to_string())).await?.check()
    })
    .await?;
    Ok(!res.take::<Vec<RecordId>>(0)?.is_empty())
}

/// Record a failed attempt: retry after exponential backoff with jitter, or dead-letter.
/// Returns the new status, or `None` if the lease was already lost.
pub async fn fail(db: &ControlDb, job: &Job, worker: &str, err: &JobError, retry_base: Duration) -> AppResult<Option<String>> {
    let delay = backoff(job.attempts, retry_base);
    let mut res = with_retry(|| async {
        q::FAIL
            .on(db)
            .bind(("id", job.id.clone()))
            .bind(("worker", worker.to_string()))
            .bind(("dead", !err.retry))
            .bind(("delay", format!("{}ms", delay.as_millis())))
            .bind(("code", err.code.as_str().to_string()))
            .await?
            .check()
    })
    .await?;
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        status: String,
    }
    Ok(res.take::<Vec<Row>>(0)?.into_iter().next().map(|r| r.status))
}

pub const RETRY_MAX: Duration = Duration::from_secs(15 * 60);

/// `base * 2^(attempts-1)` capped at `RETRY_MAX`, then scaled by a random 50 to 100% so retries spread out.
pub fn backoff(attempts: i64, base: Duration) -> Duration {
    let exp = (attempts.clamp(1, 30) - 1) as u32;
    let full = base.saturating_mul(1u32 << exp.min(20)).min(RETRY_MAX);
    full.mul_f64(rand::thread_rng().gen_range(0.5..=1.0))
}

/// Give back every lease `worker` holds (graceful shutdown), refunding the attempt.
pub async fn release_worker(db: &ControlDb, worker: &str) -> AppResult<usize> {
    let mut res = q::RELEASE_WORKER.on(db).bind(("worker", worker.to_string())).await?.check()?;
    Ok(res.take::<Vec<RecordId>>(0)?.len())
}

/// Take or renew the scheduler lease for `ttl`. `true` means `worker` is the leader.
pub async fn acquire_leader(db: &ControlDb, worker: &str, ttl: Duration) -> AppResult<bool> {
    let _fence = crate::tx::lock("job.leader").await; // see `claim`
    let mut res = with_retry(|| async {
        q::LEADER_ACQUIRE
            .on(db)
            .bind(("worker", worker.to_string()))
            .bind(("ttl", format!("{}ms", ttl.as_millis())))
            .await?
            .check()
    })
    .await?;
    Ok(!res.take::<Vec<String>>(0)?.is_empty())
}

pub async fn release_leader(db: &ControlDb, worker: &str) -> AppResult<()> {
    q::LEADER_RELEASE.on(db).bind(("worker", worker.to_string())).await?.check()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_parses_and_defaults_to_all() {
        assert_eq!(Role::parse(""), Some(Role::All));
        assert_eq!(Role::parse("API"), Some(Role::Api));
        assert_eq!(Role::parse("worker"), Some(Role::Worker));
        assert_eq!(Role::parse("nope"), None);
        assert!(Role::All.serves_http() && Role::All.runs_jobs());
        assert!(!Role::Worker.serves_http() && !Role::Api.runs_jobs());
    }

    #[test]
    fn backoff_doubles_with_jitter_and_caps() {
        let base = Duration::from_secs(10);
        for _ in 0..50 {
            let d1 = backoff(1, base);
            assert!(d1 >= Duration::from_secs(5) && d1 <= base, "{d1:?}");
            let d3 = backoff(3, base);
            assert!(d3 >= Duration::from_secs(20) && d3 <= Duration::from_secs(40), "{d3:?}");
            assert!(backoff(50, base) <= RETRY_MAX);
        }
    }

    #[test]
    fn client_errors_do_not_retry_but_conflicts_do() {
        assert!(!JobError::from(AppError::bad_request("x")).retry);
        assert!(JobError::from(AppError::coded(ErrorCode::DbConflict, "x")).retry);
        assert!(JobError::from(AppError::internal("x")).retry);
    }
}
