//! Only new generic scope-refusal rows have a budget. Each credential gets a
//! burst of 30 and one replacement every two seconds, independent of its targets.
//! A single timer flushes pending summaries even when a user makes no more calls.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use chrono::{DateTime, Utc};
use surrealdb::types::RecordId;
use tokio::sync::Notify;
use tokio::time::Instant;

use crate::{audit, authz::Actor, pool::ControlDb};
use crate::rid::RecordIdExt;

const CAPACITY: usize = 4096;
const REFILL: Duration = Duration::from_secs(2);
const BURST_CREDIT: Duration = Duration::from_secs(60);

type Key = (String, &'static str, String);

pub struct ScopeRefusals(Arc<Inner>);

struct Inner {
    db: ControlDb,
    buckets: Mutex<HashMap<Key, Bucket>>,
    wake: Arc<Notify>,
}

impl Drop for Inner {
    fn drop(&mut self) { self.wake.notify_one(); }
}

struct Bucket {
    credit: Duration,
    updated: Instant,
    pending: Option<Summary>,
}

struct Summary {
    user: RecordId,
    actor: Actor,
    count: u64,
    tools: BTreeSet<&'static str>,
    window_start: DateTime<Utc>,
    due: Instant,
}

impl Summary {
    async fn record(self, db: &ControlDb) {
        let detail = serde_json::json!({
            "count": self.count,
            "window_start": self.window_start,
            "window_end": Utc::now(),
            "tools": self.tools,
        }).to_string();
        audit::record_structured(db, audit::Event {
            user: Some(&self.user), actor: &self.actor,
            action: "auth.scope_refused_suppressed", target: crate::vaults::service::HIDDEN_SPACE,
            outcome: "auth.scope", detail: &detail,
        }).await;
    }
}

impl ScopeRefusals {
    pub fn new(db: ControlDb) -> Self {
        let inner = Arc::new(Inner { db, buckets: Mutex::new(HashMap::new()), wake: Arc::new(Notify::new()) });
        tokio::spawn(flush_when_due(Arc::downgrade(&inner)));
        Self(inner)
    }

    pub async fn allow(&self, user: &RecordId, actor: &Actor, tool: &str) -> bool {
        // Registry names only, never arguments or an unbounded set of caller input.
        let tool = crate::tools::registry::all_tools().get_key_value(tool).map(|(&name, _)| name).unwrap_or("other");
        let (allowed, summaries) = {
            let mut buckets = self.0.buckets.lock().unwrap_or_else(|e| e.into_inner());
            take(&mut buckets, user, actor, tool, CAPACITY)
        };
        self.0.wake.notify_one();
        for summary in summaries { summary.record(&self.0.db).await; }
        allowed
    }
}

fn take(buckets: &mut HashMap<Key, Bucket>, user: &RecordId, actor: &Actor, tool: &'static str, capacity: usize) -> (bool, Vec<Summary>) {
    let now = Instant::now();
    let key = (user.to_string(), actor.kind, actor.id.clone());
    let mut summaries = Vec::new();
    if !buckets.contains_key(&key) && buckets.len() >= capacity {
        let oldest = buckets.iter().min_by_key(|(_, b)| b.updated).map(|(k, _)| k.clone());
        if let Some(oldest) = oldest
            && let Some(bucket) = buckets.remove(&oldest)
            && let Some(summary) = bucket.pending
        {
            // Eviction must not silently discard a quiet user's pending count.
            summaries.push(summary);
        }
    }
    let bucket = buckets.entry(key).or_insert(Bucket { credit: BURST_CREDIT, updated: now, pending: None });
    bucket.credit = bucket.credit.saturating_add(now.duration_since(bucket.updated)).min(BURST_CREDIT);
    bucket.updated = now;
    if bucket.credit >= REFILL {
        bucket.credit -= REFILL;
        if let Some(summary) = bucket.pending.take() { summaries.push(summary); }
        (true, summaries)
    } else {
        let pending = bucket.pending.get_or_insert_with(|| Summary {
            user: user.clone(), actor: actor.clone(), count: 0, tools: BTreeSet::new(),
            window_start: Utc::now(), due: now + (REFILL - bucket.credit),
        });
        pending.count = pending.count.saturating_add(1);
        pending.tools.insert(tool);
        (false, summaries)
    }
}

async fn flush_when_due(weak: Weak<Inner>) {
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let (summaries, next) = {
            let now = Instant::now();
            let mut buckets = inner.buckets.lock().unwrap_or_else(|e| e.into_inner());
            let mut summaries = Vec::new();
            for bucket in buckets.values_mut() {
                if bucket.pending.as_ref().is_some_and(|s| s.due <= now)
                    && let Some(summary) = bucket.pending.take()
                {
                    summaries.push(summary);
                }
            }
            let next = buckets.values().filter_map(|b| b.pending.as_ref().map(|s| s.due)).min();
            (summaries, next)
        };
        for summary in summaries { summary.record(&inner.db).await; }
        // The timer retains neither the app nor its database while waiting.
        let wake = inner.wake.clone();
        drop(inner);
        match next {
            Some(due) => tokio::select! {
                _ = tokio::time::sleep_until(due) => {},
                _ = wake.notified() => {},
            },
            None => wake.notified().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn bounded_cache_returns_pending_counts_on_eviction() {
        let user = crate::rid::parse("user:owner").unwrap();
        let mut buckets = HashMap::new();
        let first = Actor { kind: "token", id: "api_token:first".into() };
        for _ in 0..30 { assert!(take(&mut buckets, &user, &first, "vault_peers", 2).0); }
        for _ in 0..7 { assert!(!take(&mut buckets, &user, &first, "vault_peers", 2).0); }
        tokio::time::advance(Duration::from_millis(1)).await;
        for i in 0..100 {
            let actor = Actor { kind: "token", id: format!("api_token:{i}") };
            let (allowed, summaries) = take(&mut buckets, &user, &actor, "ontology_get", 2);
            assert!(allowed);
            assert!(buckets.len() <= 2);
            if i == 1 {
                assert_eq!(summaries.len(), 1);
                assert_eq!(summaries[0].count, 7);
                assert_eq!(summaries[0].user, user);
                assert_eq!(summaries[0].actor, first);
                assert_eq!(summaries[0].tools, ["vault_peers"].into_iter().collect());
            } else {
                assert!(summaries.is_empty());
            }
            tokio::time::advance(Duration::from_millis(1)).await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn next_allowed_event_takes_one_summary_and_refills_at_thirty_per_minute() {
        let user = crate::rid::parse("user:owner").unwrap();
        let actor = Actor { kind: "token", id: "api_token:first".into() };
        let mut buckets = HashMap::new();
        for _ in 0..30 { assert!(take(&mut buckets, &user, &actor, "vault_peers", 2).0); }
        assert!(!take(&mut buckets, &user, &actor, "vault_peers", 2).0);
        tokio::time::advance(REFILL).await;
        let (allowed, summaries) = take(&mut buckets, &user, &actor, "vault_peers", 2);
        assert!(allowed);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].count, 1);
        let (allowed, summaries) = take(&mut buckets, &user, &actor, "vault_peers", 2);
        assert!(!allowed);
        assert!(summaries.is_empty());
        tokio::time::advance(Duration::from_secs(60)).await;
        for _ in 0..30 { assert!(take(&mut buckets, &user, &actor, "vault_peers", 2).0); }
        assert!(!take(&mut buckets, &user, &actor, "vault_peers", 2).0);
    }
}
