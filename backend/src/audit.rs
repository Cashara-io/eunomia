//! Append-only security ledger (`audit_event`). One row per auth event and per
//! mutating tool call: who (user, token or OAuth client), what, on which target,
//! the outcome code and the request's trace id. Nothing in the app updates or
//! deletes these rows except [`prune`], which applies the retention window.
//! Writes are best effort: a failure is logged, never surfaced to the caller.

use surrealdb::types::RecordId;
use crate::rid::RecordIdExt;

use crate::authz::Actor;
use crate::pool::ControlDb;
use crate::store;

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
    let res = store::control::AUDIT_EVENT_CREATE
        .on(db)
        .bind(("user", e.user.cloned()))
        .bind(("actor_kind", e.actor.kind.to_string()))
        .bind(("actor_id", e.actor.id.clone()))
        .bind(("action", e.action.to_string()))
        .bind(("target", e.target.to_string()))
        .bind(("outcome", e.outcome.to_string()))
        .bind(("trace_id", crate::telemetry::current_trace_id()))
        .bind(("detail", detail))
        .await;
    let failure = match res {
        Ok(r) => r.check().err(),
        Err(err) => Some(err),
    };
    if let Some(err) = failure {
        tracing::warn!(action = e.action, error = %err, "audit_event write failed");
    }
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

pub async fn prune(db: &ControlDb, audit_days: u32, session_days: u32) -> crate::error::AppResult<()> {
    if audit_days > 0 {
        store::control::AUDIT_PRUNE.on(db).bind(("age", format!("{audit_days}d"))).await?.check()?;
    }
    store::control::SESSION_PRUNE.on(db).bind(("age", format!("{session_days}d"))).await?.check()?;
    Ok(())
}
