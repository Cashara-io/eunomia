//! Append-only security ledger (`audit_event`). One row per auth event and per
//! mutating tool call: who (user, token or OAuth client), what, on which target,
//! the outcome code and the request's trace id. Nothing in the app updates or
//! deletes these rows except sealing them into their ledger's hash chain and the
//! retention cut ([`prune`]), which leaves a checkpoint behind (see [`chain`] and
//! docs/audit-chain.md).
//!
//! Delivery is durable within the process, and never silently dropped: the write runs in a task of
//! its own, so a caller that is cancelled mid-write (a client that disconnects) does not lose it; a
//! failed write is retried, then queued and replayed by the scheduler (`chain::replay`); a row written
//! but not yet sealed (a crash between the two) is sealed by the same replay. A failure is logged,
//! never surfaced to the caller.

use surrealdb::types::RecordId;

use crate::authz::Actor;
use crate::rid::RecordIdExt;
use crate::pool::{ControlDb, OrgId};

pub mod chain;
pub(crate) mod scope_refusals;

pub struct Event<'a> {
    pub user: Option<&'a RecordId>,
    pub actor: &'a Actor,
    pub action: &'a str,
    pub target: &'a str,
    /// `ok` or the error code that ended the action.
    pub outcome: &'a str,
    pub detail: &'a str,
}

pub fn anonymous() -> Actor {
    Actor { kind: "anonymous", id: String::new() }
}

pub async fn record(db: &ControlDb, e: Event<'_>) {
    let detail: String = e.detail.chars().take(500).collect();
    record_structured(db, Event { detail: &detail, ..e }).await;
}

/// Structured callers bound each field before JSON serialisation. Cutting the
/// resulting JSON string would corrupt it and could erase suppression counts.
pub(crate) async fn record_structured(db: &ControlDb, e: Event<'_>) {
    deliver(db, chain::Pending::new(e, None)).await;
}

/// [`record`] for an event about one org that has no user behind it (a job, the self-host move):
/// it goes to that org's ledger instead of the instance ledger.
pub async fn record_in_org(db: &ControlDb, org: &OrgId, e: Event<'_>) {
    let detail: String = e.detail.chars().take(500).collect();
    deliver(db, chain::Pending::new(Event { detail: &detail, ..e }, Some(chain::org_ledger(org)))).await;
}

/// Writes and seals in a task of its own and waits for it. Dropping the caller's future (a
/// cancelled request) leaves the task running, so the event still lands (issue #219).
async fn deliver(db: &ControlDb, pending: chain::Pending) {
    let db = db.clone();
    let _ = tokio::spawn(chain::append(db, pending)).await;
}

/// Records an event for the current request's credential (the gate's caller), or
/// a plain user actor outside a request.
pub async fn record_as_caller(db: &ControlDb, user: &RecordId, action: &str, target: &str, outcome: &str) {
    record_as_caller_detail(db, user, action, target, outcome, "").await;
}

/// [`record_as_caller`] with a short free-text `detail` (counts, say).
pub async fn record_as_caller_detail(db: &ControlDb, user: &RecordId, action: &str, target: &str, outcome: &str, detail: &str) {
    let actor = crate::authz::current().map(|c| c.actor).unwrap_or_else(|| Actor { kind: "user", id: user.to_string() });
    record(db, Event { user: Some(user), actor: &actor, action, target, outcome, detail }).await;
}

fn days_from_env(key: &str, default: u32) -> u32 {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

/// Deletes audit rows older than `AUDIT_RETENTION_DAYS` (default 365, `0` keeps them forever) and
/// sessions that expired more than `SESSION_RETENTION_DAYS` (default 7) ago.
pub async fn prune_default(db: &ControlDb) -> crate::error::AppResult<()> {
    prune(db, days_from_env("AUDIT_RETENTION_DAYS", 365), days_from_env("SESSION_RETENTION_DAYS", 7)).await
}

/// Chained ledgers are cut with a retention checkpoint, so what is left still verifies; rows from
/// before the chain started are deleted by age as before.
pub async fn prune(db: &ControlDb, audit_days: u32, session_days: u32) -> crate::error::AppResult<()> {
    if audit_days > 0 {
        chain::prune(db, chrono::Utc::now() - chrono::TimeDelta::days(audit_days as i64)).await?;
        crate::store::control::AUDIT_PRUNE.on(db).bind(("age", format!("{audit_days}d"))).await?.check()?;
    }
    crate::store::control::SESSION_PRUNE.on(db).bind(("age", format!("{session_days}d"))).await?.check()?;
    Ok(())
}
