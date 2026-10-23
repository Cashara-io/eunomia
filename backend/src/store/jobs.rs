//! Statements for the job queue (docs/architecture/jobs.md). See the module docs in store/mod.rs.

use super::{ControlStmt, Stmt};

/// The queue and the leader lease live in the control database.
pub const CONTROL_ALL: &[&ControlStmt] = &[
    &ENQUEUE,
    &CLAIM,
    &HEARTBEAT,
    &COMPLETE,
    &FAIL,
    &RELEASE_WORKER,
    &REAP_POISONED,
    &PRUNE_DONE,
    &LEADER_ACQUIRE,
    &LEADER_RELEASE,
    &JOB_STATUS,
    &SYNC_ACTIVE,
    &SYNC_RUNNING_OTHER,
];

/// Reconciler probes run in the org's database.
pub const ALL: &[&Stmt] = &[
    &EMBED_BACKLOG,
    &MEMORY_EMBED_BACKLOG,
    &STALE_OBSERVATIONS,
];

/// Idempotent: the record id is derived from the key, and IGNORE turns a second
/// insert (id or unique-index collision) into an empty result.
pub const ENQUEUE: ControlStmt = ControlStmt::new(
    "jobs.enqueue",
    "INSERT IGNORE INTO job { id: $id, org: $org, kind: $kind, owner: $owner, payload: $payload, idempotency_key: $key, \
     run_at: $run_at, max_attempts: $max_attempts, traceparent: $traceparent } RETURN VALUE id",
);

/// One transaction: pick candidates (expired leases first, then ready), skip owners at their
/// running cap, and take them with a conditional UPDATE. Two workers racing for a row conflict
/// on commit (retried by `tx::with_retry`) and the loser re-reads, so a row is claimed once.
/// The cap is soft: one claim batch can overshoot it.
pub const CLAIM: ControlStmt = ControlStmt::new(
    "jobs.claim",
    "BEGIN TRANSACTION; \
     LET $full = (SELECT VALUE owner FROM (SELECT owner, count() AS n FROM job \
         WHERE status = 'running' AND locked_until >= time::now() GROUP BY owner) WHERE n >= $cap); \
     LET $expired = (SELECT VALUE id FROM job WHERE status = 'running' AND locked_until < time::now() \
         AND attempts < max_attempts AND owner NOT IN $full LIMIT $n); \
     LET $ready = (SELECT VALUE id FROM job WHERE status = 'ready' AND run_at <= time::now() \
         AND owner NOT IN $full LIMIT $n); \
     UPDATE array::slice(array::concat($expired, $ready), 0, $n) \
         SET status = 'running', locked_by = $worker, locked_until = time::now() + <duration>$lease, \
             attempts += 1, updated_at = time::now() \
         WHERE (status = 'ready' AND run_at <= time::now()) OR (status = 'running' AND locked_until < time::now()) \
         RETURN AFTER; \
     COMMIT TRANSACTION;",
);

/// Returns a row only while this worker still holds the lease.
pub const HEARTBEAT: ControlStmt = ControlStmt::new(
    "jobs.heartbeat",
    "UPDATE $id SET locked_until = time::now() + <duration>$lease, updated_at = time::now() \
     WHERE status = 'running' AND locked_by = $worker RETURN VALUE id",
);

pub const COMPLETE: ControlStmt = ControlStmt::new(
    "jobs.complete",
    "UPDATE $id SET status = 'done', locked_by = NONE, locked_until = NONE, last_error_code = NONE, updated_at = time::now() \
     WHERE status = 'running' AND locked_by = $worker RETURN VALUE id",
);

/// Back to `ready` after `$delay`, or `dead` when `$dead` or the attempts are used up.
pub const FAIL: ControlStmt = ControlStmt::new(
    "jobs.fail",
    "UPDATE $id SET status = IF $dead OR attempts >= max_attempts THEN 'dead' ELSE 'ready' END, \
         run_at = time::now() + <duration>$delay, last_error_code = $code, \
         locked_by = NONE, locked_until = NONE, updated_at = time::now() \
     WHERE status = 'running' AND locked_by = $worker RETURN status",
);

/// Graceful shutdown: hand back what this worker still holds without burning an attempt.
pub const RELEASE_WORKER: ControlStmt = ControlStmt::new(
    "jobs.release_worker",
    "UPDATE job SET status = 'ready', run_at = time::now(), attempts = math::max([attempts - 1, 0]), \
         locked_by = NONE, locked_until = NONE, updated_at = time::now() \
     WHERE status = 'running' AND locked_by = $worker RETURN VALUE id",
);

/// A job whose every lease expired is a crash loop: dead-letter it instead of reclaiming forever.
pub const REAP_POISONED: ControlStmt = ControlStmt::new(
    "jobs.reap_poisoned",
    "UPDATE job SET status = 'dead', last_error_code = 'job.lease_expired', locked_by = NONE, locked_until = NONE, \
         updated_at = time::now() \
     WHERE status = 'running' AND locked_until < time::now() AND attempts >= max_attempts RETURN VALUE id",
);

pub const PRUNE_DONE: ControlStmt = ControlStmt::new(
    "jobs.prune_done",
    "DELETE job WHERE status = 'done' AND updated_at < time::now() - <duration>$age RETURN NONE",
);

/// Take or renew the scheduler lease; a row back means this candidate is the leader.
pub const LEADER_ACQUIRE: ControlStmt = ControlStmt::new(
    "jobs.leader_acquire",
    "UPDATE job_leader:scheduler SET holder = $worker, until = time::now() + <duration>$ttl \
     WHERE holder = $worker OR until < time::now() RETURN VALUE holder",
);

pub const LEADER_RELEASE: ControlStmt = ControlStmt::new(
    "jobs.leader_release",
    "UPDATE job_leader:scheduler SET holder = NONE, until = d\"1970-01-01T00:00:00Z\" WHERE holder = $worker RETURN VALUE holder",
);

/// One job's status (a manual sync waits on it).
pub const JOB_STATUS: ControlStmt = ControlStmt::new("jobs.status", "SELECT VALUE status FROM ONLY $id");

/// A `sync` job for this owner and source that is waiting or running: the Connectors page's
/// "already running".
pub const SYNC_ACTIVE: ControlStmt = ControlStmt::new(
    "jobs.sync_active",
    "SELECT VALUE id FROM job WHERE kind = 'sync' AND owner = $owner AND payload.source = $source \
     AND (status = 'ready' OR (status = 'running' AND locked_until >= time::now())) LIMIT 1",
);

/// A `sync` job other than `$me` holding a live lease for this owner and source: another replica is
/// syncing it right now.
pub const SYNC_RUNNING_OTHER: ControlStmt = ControlStmt::new(
    "jobs.sync_running_other",
    "SELECT VALUE id FROM job WHERE kind = 'sync' AND owner = $owner AND payload.source = $source \
     AND status = 'running' AND locked_until >= time::now() AND id != $me LIMIT 1",
);

// --- reconcilers: cheap "is there work" probes ---

pub const EMBED_BACKLOG: Stmt = Stmt::new(
    "jobs.embed_backlog",
    "SELECT id, title, body_text FROM cache_record WHERE owner = $owner AND deleted = false AND embedding IS NONE \
     AND string::len(string::trim(string::concat(title, body_text))) > 0 LIMIT $limit",
);

/// An owner's memories with no vector from the current model (`$spec`, see `embeddings::EmbedSpec`):
/// never embedded, edited since (the `memory_text_changed` event cleared it), or made by another model.
pub const MEMORY_EMBED_BACKLOG: Stmt = Stmt::new(
    "jobs.memory_embed_backlog",
    "SELECT id, text FROM memory WHERE owner = $owner AND (embedding_spec IS NONE OR embedding_spec != $spec) \
     AND string::len(string::trim(text)) > 0 LIMIT $limit",
);

pub const STALE_OBSERVATIONS: Stmt = Stmt::new(
    "jobs.stale_observations",
    "SELECT owner, subject FROM memory WHERE type = 'observation' AND status = 'stale' LIMIT $limit",
);

/// Early wake-up when a job is created. LIVE queries are node-local and have had stability fixes,
/// so this is only a latency optimisation: polling is the guarantee.
pub async fn live_wake(db: crate::pool::ControlDb, wake: std::sync::Arc<tokio::sync::Notify>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    use futures::StreamExt;
    use surrealdb::types::Action;
    while !*shutdown.borrow() {
        match db.raw(super::RawKey(())).select::<Vec<surrealdb::types::Value>>("job").live().await {
            Ok(mut stream) => loop {
                tokio::select! {
                    n = stream.next() => match n {
                        Some(Ok(n)) => if n.action == Action::Create { wake.notify_one() },
                        _ => break,
                    },
                    _ = shutdown.changed() => return,
                }
            },
            Err(e) => tracing::debug!(error = %e, "job live query unavailable; polling only"),
        }
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
            _ = shutdown.changed() => {}
        }
    }
}
