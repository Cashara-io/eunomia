//! The per-vault change log (docs/projects.md, migration 0022): every write that changes what a vault
//! holds appends one numbered row (`seq`, `op`, `kind`, `id`, `actor`, `at`), with no content, so a
//! client catches up from a cursor and fetches the items it cares about. Read over REST
//! (`GET /api/vaults/{vault}/changes`), the `changes_since` tool, or live over a WebSocket
//! (`routers/changes.rs`), which this module feeds through an in-process broadcast.
//!
//! The write paths call [`record`] after their own write commits: `entities/service.rs` (memories and
//! entities), `tools/registry/skills.rs` and `documents/`.
// ponytail: the row is a second transaction after the write, so a crash between the two loses one row
// (never a seq). Moving it into each write's own transaction closes that if it ever matters.

use std::sync::OnceLock;

use serde::Serialize;
use surrealdb::types::{Datetime, RecordId, SurrealValue};
use tokio::sync::broadcast;

use crate::authz;
use crate::error::AppResult;
use crate::pool::{OrgDb, OrgId};
use crate::rid::RecordIdExt;
use crate::store;
use crate::tx::with_retry_dup;

pub const CREATED: &str = "created";
pub const UPDATED: &str = "updated";
pub const DELETED: &str = "deleted";
pub const MERGED: &str = "merged";

pub const MEMORY: &str = "memory";
pub const ENTITY: &str = "entity";
pub const SKILL: &str = "skill";
pub const DOCUMENT: &str = "document";

pub const DEFAULT_LIMIT: i64 = 100;
pub const MAX_LIMIT: i64 = 1000;

/// One row of a vault's change log.
#[derive(Debug, Clone, Serialize, SurrealValue, utoipa::ToSchema)]
pub struct Change {
    /// Strictly increasing per vault, with no gaps.
    pub seq: i64,
    /// `created`, `updated`, `deleted` or `merged` (an entity merged into another, which gets `updated`).
    pub op: String,
    /// `memory`, `entity`, `skill` or `document`.
    pub kind: String,
    /// The item's id; fetch it to see what changed (it may be gone).
    pub id: String,
    /// Who wrote it: a user, token or OAuth client id, or `system`.
    pub actor: String,
    #[schema(value_type = String)]
    pub at: Datetime,
}

/// A page of changes and the cursor for the next one.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ChangePage {
    pub changes: Vec<Change>,
    /// Pass as `after` next time: the last seq returned, or the `after` given when there was none.
    pub next: i64,
}

/// A committed change, for live followers.
#[derive(Debug, Clone)]
pub struct Event {
    pub org: OrgId,
    pub vault: RecordId,
    pub change: Change,
}

// ponytail: in-process only. With several backend replicas a follower sees only the writes of the
// replica it is connected to (until it reconnects and reads the backlog); a shared bus (SurrealDB
// LIVE SELECT, Redis pub/sub) would fix that.
fn bus() -> &'static broadcast::Sender<Event> {
    static BUS: OnceLock<broadcast::Sender<Event>> = OnceLock::new();
    BUS.get_or_init(|| broadcast::channel(1024).0)
}

/// Every change committed in this process from now on. A lagging receiver skips ahead; followers
/// re-read the gap from the database.
pub fn subscribe() -> broadcast::Receiver<Event> {
    bus().subscribe()
}

/// Appends a change to `vault`'s log, as the current caller. Never fails the write it follows (that
/// already committed): a failure is logged.
pub async fn record(db: &OrgDb, vault: &RecordId, op: &'static str, kind: &'static str, item: &RecordId) {
    let actor = authz::current().map_or_else(|| "system".to_string(), |c| c.actor.id);
    // its own task, awaited: every write path ends here, and polled inline it takes them past a 2 MiB
    // test stack (debug builds); the row is still written before the write returns
    let (db2, vault2, item2) = (db.clone(), vault.clone(), item.clone());
    let result = match tokio::spawn(async move { append(&db2, &vault2, op, kind, &item2, actor).await }).await {
        Ok(r) => r.map_err(|e| e.message),
        Err(e) => Err(e.to_string()),
    };
    if let Err(e) = result {
        tracing::error!(vault = %vault.to_string(), item = %item.to_string(), error = %e, "change log: row not written");
    }
}

/// Like `tx::lock`, but stripes of its own: callers append while holding a `tx::lock` stripe (a
/// memory write holds its subject's), and sharing those stripes would deadlock on a hash collision.
async fn vault_lock(vault: &RecordId) -> Option<tokio::sync::MutexGuard<'static, ()>> {
    use std::hash::{Hash, Hasher};
    static LOCKS: [tokio::sync::Mutex<()>; 64] = [const { tokio::sync::Mutex::const_new(()) }; 64];
    #[cfg(feature = "test-support")]
    if crate::tx::LOCKS_OFF.load(std::sync::atomic::Ordering::SeqCst) {
        return None;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    vault.to_string().hash(&mut h);
    Some(LOCKS[(h.finish() as usize) % LOCKS.len()].lock().await)
}

async fn append(db: &OrgDb, vault: &RecordId, op: &'static str, kind: &'static str, item: &RecordId, actor: String) -> AppResult<()> {
    let counter = RecordId::from_table_key("change_seq", vault.key().clone());
    // one writer per vault in this process; the transaction still orders writers in other processes
    let _guard = vault_lock(vault).await;
    let change = with_retry_dup(|| async {
        let mut res = store::changes::APPEND
            .on(db)
            .bind(("counter", counter.clone()))
            .bind(("vault", vault.clone()))
            .bind(("op", op))
            .bind(("kind", kind))
            .bind(("item", item.to_string()))
            .bind(("actor", actor.clone()))
            .await?;
        let rows: Vec<Change> = res.take(store::changes::APPEND.slot)?;
        rows.into_iter().next().ok_or_else(|| surrealdb::Error::query("change append returned no row".into(), None))
    })
    .await?;
    // no receivers is fine
    let _ = bus().send(Event { org: db.org(), vault: vault.clone(), change });
    Ok(())
}

/// `vault`'s changes after `after`, oldest first. The caller has authorized the read.
pub async fn after(db: &OrgDb, vault: &RecordId, after: i64, limit: i64) -> AppResult<ChangePage> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let changes: Vec<Change> =
        store::changes::AFTER.on(db).bind(("vault", vault.clone())).bind(("after", after)).bind(("limit", limit)).await?.take(0)?;
    let next = changes.last().map_or(after, |c| c.seq);
    Ok(ChangePage { changes, next })
}
