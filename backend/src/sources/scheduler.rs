//! Sync scheduling. [`sync_source`] runs one source for one user: the `sync` job handler calls it,
//! and so does the on-demand `/sources/{key}/sync` endpoint (which enqueues a `sync` job and waits for it).
//! [`due_syncs`] says which `(owner, source)` pairs should sync now; the scheduler leader
//! (`jobs::leader`) turns those into `sync:<owner>:<source>:<window>` jobs. There are no per-user
//! timer loops. A source honours its interval (Settings, default 15 minutes, heypocket 24 hours) and,
//! after failures, the longer backoff.
//!
//! One sync of a given `(owner, source)` at a time: an in-process guard covers runs in this process, and
//! the job queue covers other replicas (a running `sync` job for the same pair makes this one stand down).
//!
//! "Source" here is a sync key (`registry::sync_key`): one connection, so a user's two Gmail
//! connections (`gmail` and `gmail~<instance>`) keep separate cursors, health and jobs.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use chrono::Utc;
use surrealdb::types::SurrealValue;
use serde::Deserialize;
use serde_json::{json, Value};
use surrealdb::types::RecordId;
use crate::rid::RecordIdExt;

use crate::pool::OrgDb;
use crate::state::OrgState;
use crate::store;
use crate::error::AppResult;
use crate::sources::base::{datetime_to_chrono, owner_key_str};
use crate::jobs::NewJob;
use crate::sources::registry;

const BACKOFF: [u64; 5] = [900, 1800, 3600, 7200, 21600];

/// Seconds to wait before retrying after `failures` consecutive failures.
pub fn backoff_seconds(failures: i64) -> u64 {
    let idx = failures.max(0) as usize;
    BACKOFF[idx.min(BACKOFF.len() - 1)]
}

fn sync_status_id(owner: &RecordId, key: &str) -> RecordId {
    RecordId::from_table_key("sync_status", format!("{}:{}", owner_key_str(owner), key))
}

async fn all_user_ids(state: &OrgState) -> AppResult<Vec<RecordId>> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        id: RecordId,
    }
    let mut res = store::control::ORG_MEMBERS.on(&state.control).bind(("org", state.db.org().record())).await?;
    let rows: Vec<Row> = res.take(0)?;
    Ok(rows.into_iter().map(|r| r.id).collect())
}

#[derive(Debug, Clone, Deserialize, SurrealValue)]
struct SyncStatusRow {
    #[serde(default)]
    #[surreal(default)]
    cursor: String,
    #[serde(default)]
    #[surreal(default)]
    last_run: Option<surrealdb::types::Datetime>,
    #[serde(default)]
    #[surreal(default)]
    consecutive_failures: i64,
}

async fn get_sync_status(db: &OrgDb, owner: &RecordId, key: &str) -> AppResult<SyncStatusRow> {
    let rid = sync_status_id(owner, key);
    if let Some(row) = store::get::<SyncStatusRow>(db, &rid).await? {
        return Ok(row);
    }
    let mut res = store::app::SOURCES_SYNC_STATUS_UPSERT
        .on(db)
        .bind(("id", rid))
        .bind(("owner", owner.clone()))
        .await?;
    let rows: Vec<SyncStatusRow> = res.take(0)?;
    Ok(rows.into_iter().next().unwrap_or(SyncStatusRow { cursor: String::new(), last_run: None, consecutive_failures: 0 }))
}

type Claims = LazyLock<Mutex<HashSet<String>>>;

/// `(owner, source)` syncs running in this process. Exact within one process (manual and scheduled
/// runs); the job-queue check below covers other replicas.
static RUNNING: Claims = LazyLock::new(Default::default);
/// `(owner, source)` manual syncs being queued right now, so two clicks make one job.
static QUEUEING: Claims = LazyLock::new(Default::default);

/// A held claim on one `(owner, source)` pair in a process-wide set; released on drop.
struct Claim(&'static Claims, String);

impl Claim {
    fn take(set: &'static Claims, owner: &RecordId, key: &str) -> Option<Self> {
        let id = format!("{}:{key}", owner_key_str(owner));
        set.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone()).then(|| Claim(set, id))
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.1);
    }
}

/// The job is queued but no worker picked it up within the wait (e.g. an API-only node).
fn queued(key: &str) -> Value {
    json!({"source": key, "status": "queued"})
}

fn already_running(key: &str) -> Value {
    json!({"source": key, "status": "already_running"})
}

/// Whether a sync job for this pair other than `me` holds a live lease (another replica is running it).
async fn running_elsewhere(state: &OrgState, owner: &RecordId, key: &str, me: Option<&RecordId>) -> AppResult<bool> {
    let me = me.cloned().unwrap_or_else(|| RecordId::from_table_key("job", "none"));
    let mut res = store::jobs::SYNC_RUNNING_OTHER
        .on(&state.control)
        .bind(("owner", owner.clone()))
        .bind(("source", key.to_string()))
        .bind(("me", me))
        .await?;
    Ok(!res.take::<Vec<RecordId>>(0)?.is_empty())
}

/// Whether a `sync` job for this pair is waiting or running: what "already running" means to the
/// Connectors page.
pub async fn sync_active(state: &OrgState, owner: &RecordId, key: &str) -> AppResult<bool> {
    let mut res = store::jobs::SYNC_ACTIVE
        .on(&state.control)
        .bind(("owner", owner.clone()))
        .bind(("source", key.to_string()))
        .await?;
    Ok(!res.take::<Vec<RecordId>>(0)?.is_empty())
}

/// Runs one source sync for `owner` and records its health on the `sync_status` row. Never returns an
/// `Err`: any failure (missing or bad credentials, a provider error, records that failed to save, the
/// checkpoint write itself) is stored as `last_error` -- shown on the Connectors and dashboard pages --
/// and returned as `{"source", "error"}`. The cursor only advances when every record was stored, so a
/// failed record is fetched again next time (the replay is idempotent). If another run for the same
/// source is in progress, returns `{"status": "already_running"}` without calling the provider.
/// `job` is the `sync` job this run belongs to, if any.
pub async fn sync_source(state: &OrgState, owner: &RecordId, key: &str, job: Option<&RecordId>) -> Value {
    let Some(_running) = Claim::take(&RUNNING, owner, key) else {
        return already_running(key);
    };
    match running_elsewhere(state, owner, key, job).await {
        Ok(false) => {}
        Ok(true) => return already_running(key),
        Err(e) => return json!({"source": key, "error": e.message}),
    }
    sync_locked(state, owner, key).await
}

async fn sync_locked(state: &OrgState, owner: &RecordId, key: &str) -> Value {
    let db = &state.db;
    let st = match get_sync_status(db, owner, key).await {
        Ok(st) => st,
        Err(e) => return json!({"source": key, "error": e.message}),
    };
    let now = surrealdb::types::Datetime::from(Utc::now());
    let cursor = if st.cursor.is_empty() { None } else { Some(st.cursor.clone()) };

    let error = match registry::run_sync(state, owner, key, cursor).await {
        Ok((report, _)) if report.failed > 0 => format!(
            "{} of {} records failed to save; will retry: {}",
            report.failed,
            report.failed + report.written + report.skipped,
            report.errors.first().map(String::as_str).unwrap_or("")
        ),
        Ok((report, next_cursor)) => {
            let report_value = report.as_dict();
            let saved = store::app::SOURCES_SYNC_OK
                .on(db)
                .bind(("id", sync_status_id(owner, key)))
                .bind(("now", now))
                .bind(("cursor", next_cursor.unwrap_or(st.cursor.clone())))
                .bind(("report", report_value.clone()))
                .await;
            match saved {
                Ok(_) => return report_value,
                Err(e) => format!("records saved, but the sync checkpoint could not be: {e}"),
            }
        }
        Err(e) => e.message,
    };

    tracing::warn!(source = %key, owner = %owner.to_string(), error = %error, "sync failed");
    if let Err(e) = store::app::SOURCES_SYNC_FAILED
        .on(db)
        .bind(("id", sync_status_id(owner, key)))
        .bind(("now", now))
        .bind(("failures", st.consecutive_failures + 1))
        .bind(("error", error.clone()))
        .await
    {
        tracing::warn!(source = %key, error = %e, "could not record sync failure");
    }
    json!({"source": key, "error": error})
}

/// How long a manual sync request waits for its job before answering that the sync is still running.
const MANUAL_WAIT: std::time::Duration = std::time::Duration::from_secs(25);

#[derive(Debug, Deserialize, SurrealValue)]
struct StatusReport {
    #[serde(default)]
    #[surreal(default)]
    last_run: Option<surrealdb::types::Datetime>,
    #[serde(default)]
    #[surreal(default)]
    last_error: String,
    #[serde(default)]
    #[surreal(default)]
    last_report: Value,
}

/// "Sync now": queue a `sync` job and wait for the result, so a manual run gets the same lease, claim
/// cap and retry rules as a scheduled one. If a sync job for the pair is already waiting or running (or
/// answers `{"status": "already_running"}` instead of starting another. If no worker finishes it within the wait, answers `{"status": "queued"}`.
pub async fn sync_now(state: &OrgState, owner: &RecordId, key: &str) -> AppResult<Value> {
    let Some(queueing) = Claim::take(&QUEUEING, owner, key) else { return Ok(already_running(key)) };
    if sync_active(state, owner, key).await? {
        return Ok(already_running(key));
    }
    let started = Utc::now();
    let job_key = format!("{}:{}:{key}:manual:{}", crate::jobs::kind::SYNC, owner_key_str(owner), uuid::Uuid::new_v4().simple());
    let job_id = RecordId::from_table_key("job", crate::tx::stable_key('j', &job_key));
    let job = NewJob::new(crate::jobs::kind::SYNC, owner.clone(), job_key).in_org(state.db.org()).payload(json!({ "source": key }));
    crate::jobs::enqueue(&state.control, job).await?;
    drop(queueing);

    let deadline = tokio::time::Instant::now() + MANUAL_WAIT;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let mut res = store::jobs::JOB_STATUS.on(&state.control).bind(("id", job_id.clone())).await?;
        if matches!(res.take::<Option<String>>(0)?.as_deref(), Some("done" | "dead")) {
            let status: Option<StatusReport> = store::get(&state.db, &sync_status_id(owner, key)).await?;
            let ran = status.as_ref().and_then(|s| s.last_run.as_ref()).and_then(datetime_to_chrono).is_some_and(|t| t >= started);
            return Ok(match status {
                Some(s) if ran && !s.last_error.is_empty() => json!({"source": key, "error": s.last_error}),
                Some(s) if ran => s.last_report,
                _ => already_running(key), // the job stood down for another run
            });
        }
    }
    Ok(queued(key))
}

/// One source that should sync now. `period` (the source's interval) sizes the idempotency window.
pub struct DueSync {
    pub owner: RecordId,
    /// The connection's sync key.
    pub source: String,
    pub period: u64,
}

/// Whether a source last run at `last_run` should run again: its interval has passed, and after
/// failures the longer backoff has too. A source that never ran is due.
pub fn is_due(last_run: Option<chrono::DateTime<Utc>>, failures: i64, interval: u64, now: chrono::DateTime<Utc>) -> bool {
    let Some(last_run) = last_run else { return true };
    let wait = if failures > 0 { interval.max(backoff_seconds(failures - 1)) } else { interval };
    (now - last_run).num_seconds() >= wait as i64
}

/// Every enabled source, for every user, whose interval and backoff window have elapsed.
pub async fn due_syncs(state: &OrgState) -> AppResult<Vec<DueSync>> {
    let db = &state.db;
    let mut out = Vec::new();
    let now = Utc::now();
    for owner in all_user_ids(state).await? {
        let intervals = sync_intervals_for(db, &owner).await.unwrap_or_default();
        for (src, conn) in registry::enabled(db, &owner).await? {
            let key = registry::sync_key(src.as_ref(), &conn);
            let row: Option<SyncStatusRow> = store::get(db, &sync_status_id(&owner, &key)).await?;
            let last_run = row.as_ref().and_then(|r| r.last_run.as_ref()).and_then(datetime_to_chrono);
            let failures = row.map_or(0, |r| r.consecutive_failures);
            let period = interval_for(&intervals, src.key());
            if is_due(last_run, failures, period, now) {
                out.push(DueSync { owner: owner.clone(), source: key, period });
            }
        }
    }
    Ok(out)
}

async fn sync_intervals_for(db: &OrgDb, owner: &RecordId) -> AppResult<Value> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        #[serde(default)]
        #[surreal(default)]
        sync_intervals: Value,
    }
    let mut res = store::app::SOURCES_SYNC_INTERVALS.on(db).bind(("owner", owner.clone())).await?;
    let rows: Vec<Row> = res.take(0)?;
    Ok(rows.into_iter().next().map(|r| r.sync_intervals).unwrap_or(json!({})))
}

/// `sync_intervals.get(key, default)`, where the default is heypocket's
/// 24h plan-mandated minimum and everything else's fallback is 15 minutes.
fn interval_for(sync_intervals: &Value, key: &str) -> u64 {
    if let Some(v) = sync_intervals.get(key).and_then(|v| v.as_u64()) {
        return v.max(1);
    }
    if key == "heypocket" {
        86400
    } else {
        900
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_due_follows_interval_and_backoff() {
        let now = Utc::now();
        let ago = |s: i64| Some(now - chrono::Duration::seconds(s));
        assert!(is_due(None, 0, 900, now));
        assert!(!is_due(ago(100), 0, 900, now));
        assert!(is_due(ago(901), 0, 900, now));
        // after two failures wait at least 30 minutes, even with a short interval
        assert!(!is_due(ago(1000), 2, 300, now));
        assert!(is_due(ago(1801), 2, 300, now));
        // one failure waits the first backoff step (15 minutes)
        assert!(!is_due(ago(500), 1, 300, now));
        assert!(is_due(ago(901), 1, 300, now));
    }

    #[test]
    fn backoff_seconds_follows_the_fixed_schedule_and_clamps() {
        assert_eq!(backoff_seconds(0), 900);
        assert_eq!(backoff_seconds(1), 1800);
        assert_eq!(backoff_seconds(4), 21600);
        assert_eq!(backoff_seconds(100), 21600); // clamps to the last tier
    }

    #[test]
    fn sync_status_id_embeds_owner_key_and_source() {
        let owner: RecordId = crate::rid::parse("user:abc123").unwrap();
        let id = sync_status_id(&owner, "up_bank");
        assert_eq!(id.table(), "sync_status");
        let key_str = crate::rid::key_string(id.key()).expect("string key");
        assert_eq!(key_str, "abc123:up_bank");
    }

    #[test]
    fn interval_for_defers_to_configured_value_when_present() {
        let intervals = json!({"up_bank": 120, "heypocket": 3600});
        assert_eq!(interval_for(&intervals, "up_bank"), 120);
        assert_eq!(interval_for(&intervals, "heypocket"), 3600);
    }

    #[test]
    fn interval_for_defaults_heypocket_to_24h_and_others_to_15m() {
        let intervals = json!({});
        assert_eq!(interval_for(&intervals, "heypocket"), 86400);
        assert_eq!(interval_for(&intervals, "up_bank"), 900);
    }
}
