//! Vault integrity check: finds what a crash, an interrupted bulk write or a bug can leave behind in
//! one vault, and repairs what is safe to repair.
//!
//! - memories whose subject entity is gone (invisible in the app, but still recalled by text);
//! - relations with one end gone;
//! - memories filed in this vault whose subject lives in another (reported only: which side is
//!   right is a person's call);
//! - a change-log counter behind the highest seq written (the next write would fail to append).
//!
//! Any member may check; repairing deletes rows, so it needs the owner (`Action::Delete`). Exposed as
//! `GET`/`POST /api/vaults/{vault}/integrity` and the `vault_check` tool.

use serde::Serialize;
use surrealdb::types::RecordId;

use crate::authz::{self, Action};
use crate::changes;
use crate::error::AppResult;
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::store;

/// Most rows of each kind one check reports (and one repair removes); run it again for more.
pub const LIMIT: i64 = 1000;

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct IntegrityReport {
    pub vault: String,
    /// True when nothing below needs attention.
    pub ok: bool,
    /// Memories whose subject entity no longer exists.
    pub dangling_memories: Vec<String>,
    /// Memories filed here whose subject is in another vault (not repaired automatically).
    pub misfiled_memories: Vec<String>,
    /// Relations with one end missing.
    pub dangling_relations: Vec<String>,
    /// Highest seq in the change log, and the counter the next write continues from.
    pub change_log_top: i64,
    pub change_log_counter: i64,
    /// True when the counter is behind the log (the next write's append would collide).
    pub change_log_behind: bool,
    /// Set on a repair: what it removed or fixed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repaired: Option<Repaired>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Repaired {
    pub memories_removed: usize,
    pub relations_removed: usize,
    pub counter_fixed: bool,
}

async fn scan(db: &OrgDb, vault: &RecordId) -> AppResult<IntegrityReport> {
    let mut res = store::integrity::SCAN
        .on(db)
        .bind(("vault", vault.clone()))
        .bind(("counter", changes::counter(vault)))
        .bind(("limit", LIMIT))
        .await?;
    let dangling: Vec<RecordId> = res.take(0)?;
    let misfiled: Vec<RecordId> = res.take(1)?;
    let relations: Vec<RecordId> = res.take(2)?;
    let top: Vec<i64> = res.take(3)?;
    let counter: Vec<Option<i64>> = res.take(4)?;
    let top = top.first().copied().unwrap_or(0);
    let counter = counter.first().copied().flatten().unwrap_or(0);
    let behind = counter < top;
    let s = |ids: Vec<RecordId>| ids.iter().map(|r| r.to_string()).collect::<Vec<_>>();
    let (dangling, misfiled, relations) = (s(dangling), s(misfiled), s(relations));
    Ok(IntegrityReport {
        vault: vault.to_string(),
        ok: dangling.is_empty() && misfiled.is_empty() && relations.is_empty() && !behind,
        dangling_memories: dangling,
        misfiled_memories: misfiled,
        dangling_relations: relations,
        change_log_top: top,
        change_log_counter: counter,
        change_log_behind: behind,
        repaired: None,
    })
}

/// Checks `vault` as `user` (any member).
pub async fn check(db: &OrgDb, user: &RecordId, vault: &RecordId) -> AppResult<IntegrityReport> {
    authz::authorize(db, user, Action::ReadMemories, vault).await?;
    scan(db, vault).await
}

/// Checks `vault`, removes dangling memories and relations, lifts a counter that fell behind, and
/// returns the report taken after the repair with `repaired` set. Owner only.
pub async fn repair(db: &OrgDb, user: &RecordId, vault: &RecordId) -> AppResult<IntegrityReport> {
    authz::authorize(db, user, Action::Delete, vault).await?;
    let before = scan(db, vault).await?;
    let parse = |ids: &[String]| ids.iter().filter_map(|i| crate::rid::parse(i).ok()).collect::<Vec<RecordId>>();
    let memories = parse(&before.dangling_memories);
    let relations = parse(&before.dangling_relations);
    if !memories.is_empty() || !relations.is_empty() || before.change_log_behind {
        crate::tx::with_retry(|| async {
            store::integrity::REPAIR
                .on(db)
                .bind(("vault", vault.clone()))
                .bind(("memories", memories.clone()))
                .bind(("relations", relations.clone()))
                .bind(("counter", changes::counter(vault)))
                .bind(("top", before.change_log_top))
                .await?
                .check()
        })
        .await?;
        if !memories.is_empty() {
            changes::record_many(db, vault, memories.iter().map(|m| (changes::DELETED, changes::MEMORY, m.clone())).collect()).await;
        }
    }
    let mut after = scan(db, vault).await?;
    after.repaired = Some(Repaired {
        memories_removed: before.dangling_memories.len().saturating_sub(after.dangling_memories.len()),
        relations_removed: before.dangling_relations.len().saturating_sub(after.dangling_relations.len()),
        counter_fixed: before.change_log_behind && !after.change_log_behind,
    });
    Ok(after)
}
