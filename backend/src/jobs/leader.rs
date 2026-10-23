//! The scheduler leader. One process at a time holds the `job_leader:scheduler` lease and, each
//! tick, re-derives periodic work from state and enqueues it with idempotency keys. Losing or
//! doubling the leader is harmless: keys dedupe, and a missed tick only delays work.

use surrealdb::types::SurrealValue;
use crate::rid::RecordIdExt;
use tokio::sync::watch;

use super::{kind, NewJob, WorkerConfig};
use crate::error::AppResult;
use crate::sources::scheduler;
use crate::pool::OrgId;
use crate::state::{AppState, OrgState};
use crate::store;

/// Embedding and stale-observation probes run every this many ticks (they scan more than they return).
const SLOW_EVERY: u64 = 10;
/// Finished jobs are kept this long (they are the idempotency record for their window).
const KEEP_DONE: &str = "7d";
/// Enqueue at most this many consolidations per owner per tick.
const STALE_BATCH: i64 = 100;

pub async fn run(state: AppState, cfg: WorkerConfig, mut shutdown: watch::Receiver<bool>) {
    let ttl = cfg.tick * 3;
    let mut ticks = 0u64;
    let mut leading = false;
    while !*shutdown.borrow() {
        match super::acquire_leader(&state.control, &cfg.id, ttl).await {
            Ok(true) => {
                if !leading {
                    tracing::info!(worker = %cfg.id, "became scheduler leader");
                }
                leading = true;
                tick(&state, ticks).await;
                ticks += 1;
            }
            Ok(false) => leading = false,
            Err(e) => tracing::warn!(error = %e.message, "scheduler lease check failed"),
        }
        tokio::select! {
            _ = tokio::time::sleep(cfg.tick) => {}
            _ = shutdown.changed() => {}
        }
    }
    if leading {
        let _ = super::release_leader(&state.control, &cfg.id).await; // else the lease just expires
    }
}

/// One scheduler pass. `n` counts the passes this leader has made.
pub async fn tick(state: &AppState, n: u64) {
    reconcile_tenant_migrations(state).await;
    for org in ready_orgs(state).await {
        let Ok(org_state) = state.org(&org).await else { continue }; // not ready or behind: the migration job handles it
        reconcile_syncs(&org_state).await;
        reconcile_observations(&org_state).await;
        if n.is_multiple_of(SLOW_EVERY) {
            reconcile_embeddings(&org_state).await;
            if let Err(e) = crate::vaults::service::discard_stale_copies(&org_state.db).await {
                tracing::warn!(error = %e.message, "deleting stale vault copies failed");
            }
        }
    }
    if n.is_multiple_of(SLOW_EVERY)
        && let Err(e) = housekeeping(&state.control).await
    {
        tracing::warn!(error = %e.message, "job housekeeping failed");
    }
}

#[derive(serde::Deserialize, SurrealValue)]
struct TenantRow {
    org: surrealdb::types::RecordId,
    schema_version: i64,
    status: String,
}

async fn tenants(state: &AppState) -> Vec<TenantRow> {
    match store::tenant::LIST.on(&state.control).await.and_then(|mut r| r.take(0)) {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(error = %e, "listing orgs failed");
            Vec::new()
        }
    }
}

fn org_id(r: &surrealdb::types::RecordId) -> Option<OrgId> {
    crate::rid::key_string(&r.key).and_then(|k| OrgId::parse(&k))
}

async fn ready_orgs(state: &AppState) -> Vec<OrgId> {
    tenants(state).await.iter().filter(|t| t.status == "ready").filter_map(|t| org_id(&t.org)).collect()
}

/// One `migrate_tenant` job per org whose database is behind the schema this code writes. The key
/// carries the target version, so a retry or a second leader does not queue it twice.
pub async fn reconcile_tenant_migrations(state: &AppState) {
    let latest = crate::migrate::LATEST_TENANT as i64;
    for t in tenants(state).await.iter().filter(|t| t.status == "ready" && t.schema_version < latest) {
        let Some(org) = org_id(&t.org) else { continue };
        let Ok(mut res) = store::control::ORG_FIRST_OWNER.on(&state.control).bind(("org", org.record())).await else { continue };
        #[derive(serde::Deserialize, SurrealValue)]
        struct Owner {
            user: surrealdb::types::RecordId,
        }
        let Some(owner) = res.take::<Vec<Owner>>(0).ok().and_then(|o| o.into_iter().next()) else { continue };
        let key = format!("{}:{}:{latest}", kind::MIGRATE_TENANT, org.key());
        super::enqueue_lossy(&state.control, NewJob::new(kind::MIGRATE_TENANT, owner.user, key).in_org(org)).await;
    }
}

/// Dead-letter crash-looping jobs and prune old done ones.
async fn housekeeping(db: &crate::pool::ControlDb) -> AppResult<()> {
    store::jobs::REAP_POISONED.on(db).await?.check()?;
    store::jobs::PRUNE_DONE.on(db).bind(("age", KEEP_DONE)).await?.check()?;
    if let Some(owner) = crate::capsules::first_user(db).await? {
        let key = super::periodic_key(kind::PRUNE_CAPSULES, &owner, 3600);
        super::enqueue_lossy(db, NewJob::new(kind::PRUNE_CAPSULES, owner.clone(), key)).await;
        let key = super::periodic_key(kind::PRUNE_AUTH, &owner, 86400);
        super::enqueue_lossy(db, NewJob::new(kind::PRUNE_AUTH, owner, key)).await;
    }
    Ok(())
}

/// Connector sync for every source in the org whose interval and backoff have elapsed.
pub async fn reconcile_syncs(state: &OrgState) {
    let due = match scheduler::due_syncs(state).await {
        Ok(d) => d,
        Err(e) => return tracing::warn!(error = %e.message, "listing due syncs failed"),
    };
    for d in due {
        let key = format!("{}:{}:{}:{}", kind::SYNC, crate::sources::base::owner_key_str(&d.owner), d.source, super::window(d.period));
        let job = NewJob::new(kind::SYNC, d.owner, key).in_org(state.db.org()).payload(serde_json::json!({ "source": d.source }));
        super::enqueue_lossy(&state.control, job).await;
    }
}

/// Cache records still missing an embedding, for owners that can embed.
pub async fn reconcile_embeddings(state: &OrgState) {
    let Ok(owners) = owners(state).await else { return };
    for owner in owners {
        if !crate::embeddings::service::available(&state.db, &state.settings, &owner).await {
            continue;
        }
        if has_embed_backlog(state, &owner).await.unwrap_or(false) {
            let key = super::periodic_key(kind::EMBED, &owner, 300);
            super::enqueue_lossy(&state.control, NewJob::new(kind::EMBED, owner, key).in_org(state.db.org())).await;
        }
    }
}

async fn has_embed_backlog(state: &OrgState, owner: &surrealdb::types::RecordId) -> AppResult<bool> {
    let db = &state.db;
    let mut res = store::jobs::EMBED_BACKLOG.on(db).bind(("owner", owner.clone())).bind(("limit", 1)).await?;
    if !res.take::<Vec<surrealdb::types::RecordId>>("id")?.is_empty() {
        return Ok(true);
    }
    let spec = crate::embeddings::service::spec_for(db, &state.settings, owner).await?.tag();
    let mut res = store::jobs::MEMORY_EMBED_BACKLOG.on(db).bind(("owner", owner.clone())).bind(("spec", spec)).bind(("limit", 1)).await?;
    Ok(!res.take::<Vec<surrealdb::types::RecordId>>("id")?.is_empty())
}

#[derive(serde::Deserialize, SurrealValue)]
struct StaleRow {
    owner: surrealdb::types::RecordId,
    subject: surrealdb::types::RecordId,
}

async fn stale_observations(db: &crate::pool::OrgDb) -> AppResult<Vec<StaleRow>> {
    let mut res = store::jobs::STALE_OBSERVATIONS.on(db).bind(("limit", STALE_BATCH)).await?;
    Ok(res.take(0)?)
}

/// Entities whose observation was marked stale by a fact write.
pub async fn reconcile_observations(state: &OrgState) {
    let rows = match stale_observations(&state.db).await {
        Ok(r) => r,
        Err(e) => return tracing::warn!(error = %e.message, "listing stale observations failed"),
    };
    let mut can_chat: std::collections::HashMap<String, bool> = Default::default();
    for row in rows {
        let ok = match can_chat.get(&row.owner.to_string()) {
            Some(ok) => *ok,
            None => {
                let ok = crate::embeddings::service::chat_available(&state.db, &state.settings, &row.owner).await;
                can_chat.insert(row.owner.to_string(), ok);
                ok
            }
        };
        if ok {
            super::enqueue_lossy(&state.control, consolidate_job(state.db.org(), &row.owner, &row.subject)).await;
        }
    }
}

/// `consolidate:<subject>:<10 minute window>`: one consolidation per entity per window.
pub fn consolidate_job(org: OrgId, owner: &surrealdb::types::RecordId, subject: &surrealdb::types::RecordId) -> NewJob {
    let key = format!("{}:{}:{}", kind::CONSOLIDATE, subject.to_string(), super::window(600));
    NewJob::new(kind::CONSOLIDATE, owner.clone(), key).in_org(org).payload(serde_json::json!({ "subject": subject.to_string() }))
}

/// The org's users: the owners reconcilers check for work.
async fn owners(state: &OrgState) -> Result<Vec<surrealdb::types::RecordId>, ()> {
    #[derive(serde::Deserialize, SurrealValue)]
    struct Row {
        id: surrealdb::types::RecordId,
    }
    async fn list(state: &OrgState) -> AppResult<Vec<Row>> {
        let mut res = store::control::ORG_MEMBERS.on(&state.control).bind(("org", state.db.org().record())).await?;
        Ok(res.take(0)?)
    }
    list(state).await.map(|r| r.into_iter().map(|u| u.id).collect()).map_err(|e| tracing::warn!(error = %e.message, "listing users failed"))
}
