//! Tool-call usage counters (`tool_usage`, control database).
//!
//! Every tool call, read-only or mutating, successful or not, adds one to a counter row keyed by
//! (hour, user, credential, channel, session, tool). The row id is derived from that key, so one
//! atomic `UPSERT ... calls += 1` per call is all it takes and the table holds one row per distinct
//! hourly key, not one per call (the `audit_event` ledger stays for security events and mutating
//! calls). Rows hold counts and timings only: no arguments, no content, so the control database
//! stays free of tenant text.
//!
//! Recording is best effort and off the request path: [`record`] spawns a task and returns, a
//! failure is logged, and nothing here can fail or slow the tool call. Old hours are pruned by the
//! housekeeping job ([`prune_default`]).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::{DurationRound, TimeDelta, Utc};
use surrealdb::types::{Datetime, RecordId};

use crate::authz::Caller;
use crate::pool::ControlDb;
use crate::rid::RecordIdExt;
use crate::store;

/// How a tool call reached the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Channel {
    /// `POST /api/tools/{name}` and anything else that calls the registry directly.
    #[default]
    Rest,
    /// `tools/call` on `/mcp`.
    Mcp,
    /// The in-app chat agent.
    Chat,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Rest => "rest",
            Channel::Mcp => "mcp",
            Channel::Chat => "chat",
        }
    }
}

/// What the entry point knows about the call that the registry does not.
#[derive(Debug, Clone, Default)]
pub struct Context {
    pub channel: Channel,
    /// The MCP session (`Mcp-Session-Id` request header) or chat thread; empty when there is none.
    pub session: String,
}

tokio::task_local! {
    static CONTEXT: Context;
}

/// The channel and session of the call in progress (the defaults outside one).
pub fn current_context() -> Context {
    CONTEXT.try_with(Context::clone).unwrap_or_default()
}

/// Runs `fut` with the channel and session its tool calls are counted under.
pub async fn with_context<F: std::future::Future>(channel: Channel, session: impl Into<String>, fut: F) -> F::Output {
    CONTEXT.scope(Context { channel, session: session.into() }, fut).await
}

/// The MCP session id a client sent, if it is usable as a label: visible ASCII, at most 64 chars.
pub fn session_from_header(value: Option<&str>) -> String {
    match value.map(str::trim) {
        Some(v) if !v.is_empty() && v.len() <= 64 && v.chars().all(|c| c.is_ascii_graphic()) => v.to_string(),
        _ => String::new(),
    }
}

/// One finished tool call.
#[derive(Debug, Clone)]
pub struct Sample {
    pub org: String,
    pub user: RecordId,
    pub tool: String,
    pub errored: bool,
    pub ms: u64,
}

/// The counted key of a call: everything but the numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub org: String,
    pub bucket: chrono::DateTime<Utc>,
    pub user: RecordId,
    pub actor_kind: &'static str,
    pub actor_id: String,
    pub channel: Channel,
    pub session: String,
    pub tool: String,
}

/// The start of the hour `at` falls in.
pub fn hour_bucket(at: chrono::DateTime<Utc>) -> chrono::DateTime<Utc> {
    at.duration_trunc(TimeDelta::hours(1)).unwrap_or(at)
}

impl Key {
    /// Deterministic record id: the same key always lands on the same row.
    pub fn row_id(&self) -> RecordId {
        let parts = format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            self.org,
            self.bucket.timestamp(),
            self.user.to_string(),
            self.actor_kind,
            self.actor_id,
            self.channel.as_str(),
            self.session,
            self.tool
        );
        RecordId::new("tool_usage", crate::tx::stable_key('u', &parts))
    }
}

static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// Counts one finished tool call under the current credential and context. Returns at once; the
/// write runs in a spawned task. Server-initiated work (no credential, or the system caller) is not
/// counted: it is not anyone's use of the API.
pub fn record(db: &ControlDb, sample: Sample) {
    let Some(caller) = crate::authz::current() else { return };
    let ctx = CONTEXT.try_with(Context::clone).unwrap_or_default();
    let Some(key) = key_for(&caller, &ctx, &sample, Utc::now()) else { return };
    let db = db.clone();
    IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
    let write = async move {
        write(&db, &key, sample.errored, sample.ms).await;
        IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    };
    match tokio::runtime::Handle::try_current() {
        Ok(rt) => {
            rt.spawn(write);
        }
        Err(_) => {
            IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

fn key_for(caller: &Caller, ctx: &Context, sample: &Sample, now: chrono::DateTime<Utc>) -> Option<Key> {
    let actor_kind = match caller.actor.kind {
        "user" => "user",
        "token" => "token",
        "oauth" => "oauth",
        _ => return None,
    };
    Some(Key {
        org: sample.org.clone(),
        bucket: hour_bucket(now),
        user: sample.user.clone(),
        actor_kind,
        actor_id: caller.actor.id.clone(),
        channel: ctx.channel,
        session: ctx.session.clone(),
        tool: sample.tool.clone(),
    })
}

async fn write(db: &ControlDb, key: &Key, errored: bool, ms: u64) {
    let id = key.row_id();
    // Concurrent calls on one key conflict at commit; the retry re-runs the single statement.
    let res = crate::tx::with_retry(|| async {
        store::control::TOOL_USAGE_RECORD
            .on(db)
            .bind(("id", id.clone()))
            .bind(("org", key.org.clone()))
            .bind(("bucket", Datetime::from(key.bucket)))
            .bind(("user", key.user.clone()))
            .bind(("actor_kind", key.actor_kind.to_string()))
            .bind(("actor_id", key.actor_id.clone()))
            .bind(("channel", key.channel.as_str().to_string()))
            .bind(("sess", key.session.clone()))
            .bind(("tool", key.tool.clone()))
            .bind(("errors", i64::from(errored)))
            .bind(("ms", ms as i64))
            .await?
            .check()
    })
    .await;
    if let Err(err) = res {
        tracing::warn!(tool = %key.tool, error = %err, "tool_usage write failed");
    }
}

/// Waits until every write started so far has finished. For tests and an orderly shutdown; the
/// request path never waits on usage.
pub async fn settle() {
    while IN_FLIGHT.load(Ordering::SeqCst) > 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Deletes hourly rows older than `TOOL_USAGE_RETENTION_DAYS` (default 90, `0` keeps them forever).
pub async fn prune_default(db: &ControlDb) -> crate::error::AppResult<()> {
    let days = std::env::var("TOOL_USAGE_RETENTION_DAYS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(90);
    prune(db, days).await
}

pub async fn prune(db: &ControlDb, days: u32) -> crate::error::AppResult<()> {
    if days > 0 {
        store::control::TOOL_USAGE_PRUNE.on(db).bind(("age", format!("{days}d"))).await?.check()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::Actor;

    fn caller(kind: &'static str, id: &str) -> Caller {
        Caller { actor: Actor { kind, id: id.into() }, scopes: vec![], vault: None, expires_at: None }
    }

    fn sample() -> Sample {
        Sample { org: "o".into(), user: RecordId::new("user", "u1"), tool: "recall".into(), errored: false, ms: 3 }
    }

    #[test]
    fn same_hour_same_key_same_row_and_other_dimensions_differ() {
        let t = chrono::DateTime::parse_from_rfc3339("2026-05-01T10:20:30Z").unwrap().with_timezone(&Utc);
        let later = t + TimeDelta::minutes(20);
        let tok = caller("token", "api_token:a");
        let ctx = Context::default();
        let k = key_for(&tok, &ctx, &sample(), t).unwrap();
        assert_eq!(k.row_id(), key_for(&tok, &ctx, &sample(), later).unwrap().row_id());
        assert_ne!(k.row_id(), key_for(&tok, &ctx, &sample(), t + TimeDelta::hours(1)).unwrap().row_id());
        assert_ne!(k.row_id(), key_for(&caller("token", "api_token:b"), &ctx, &sample(), t).unwrap().row_id());
        assert_ne!(k.row_id(), key_for(&tok, &Context { channel: Channel::Mcp, session: String::new() }, &sample(), t).unwrap().row_id());
        assert_ne!(k.row_id(), key_for(&tok, &Context { session: "s1".into(), ..Context::default() }, &sample(), t).unwrap().row_id());
        let mut other_tool = sample();
        other_tool.tool = "get".into();
        assert_ne!(k.row_id(), key_for(&tok, &ctx, &other_tool, t).unwrap().row_id());
    }

    #[test]
    fn system_and_anonymous_callers_are_not_counted() {
        let now = Utc::now();
        assert!(key_for(&caller("system", "system"), &Context::default(), &sample(), now).is_none());
        assert!(key_for(&caller("anonymous", ""), &Context::default(), &sample(), now).is_none());
        assert!(key_for(&caller("user", "user:u1"), &Context::default(), &sample(), now).is_some());
    }

    #[test]
    fn session_header_is_sanitised() {
        assert_eq!(session_from_header(Some(" 1717.abc ")), "1717.abc");
        assert_eq!(session_from_header(None), "");
        assert_eq!(session_from_header(Some("")), "");
        assert_eq!(session_from_header(Some("has space")), "");
        assert_eq!(session_from_header(Some(&"x".repeat(65))), "");
    }
}
