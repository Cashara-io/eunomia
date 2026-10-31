//! The per-vault change log (docs/projects.md, migration 0023): every write that changes what a vault
//! holds appends one numbered row (`seq`, `op`, `kind`, `id`, `actor`, `at`), with no content, so a
//! client catches up from a cursor and fetches the items it cares about. Read over REST
//! (`GET /api/vaults/{vault}/changes`), the `changes_since` tool, or live over a WebSocket
//! (`routers/changes.rs`), which this module feeds through an in-process broadcast.
//!
//! Retention: the scheduler leader trims each org's log at most hourly ([`prune_due`]) to the last
//! `CHANGE_LOG_RETENTION_DAYS` (default 90) and at most `CHANGE_LOG_MAX_ROWS` rows per vault (default
//! 100,000). A cursor below what is kept, or past the newest change (a restored database), is refused
//! with `changes.resync` (HTTP 410): the client re-reads what it needs and starts again from `after=0`.
//!
//! The write paths call [`record`] or [`record_many`] after their own write commits:
//! `entities/service.rs` (memories, entities, relations), `entities/consolidate.rs`,
//! `tools/registry/skills.rs`, `documents/`, `sources/registry.rs` (a connection's data going) and
//! `vaults/service.rs` (what a clone or merge copies into its new vault).
// ponytail: the rows are a second transaction after the write, retried a few times; a crash or a
// database outage in between loses rows (never a seq), so the log is at most once. Moving the append
// into each write's own transaction closes that if it ever matters.

use std::sync::OnceLock;

use serde::Serialize;
use surrealdb::types::{Datetime, RecordId, SurrealValue};
use tokio::sync::broadcast;

use crate::authz;
use crate::error::{AppError, AppResult, ErrorCode};
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
pub const RELATION: &str = "relation";
pub const SKILL: &str = "skill";
pub const DOCUMENT: &str = "document";

pub const DEFAULT_LIMIT: i64 = 100;
pub const MAX_LIMIT: i64 = 1000;

/// Rows per append transaction.
const BATCH: usize = 500;
/// Tries per batch (each try also retries a commit conflict, `tx::with_retry_dup`).
const ATTEMPTS: u32 = 3;

/// One row of a vault's change log.
#[derive(Debug, Clone, Serialize, SurrealValue, utoipa::ToSchema)]
pub struct Change {
    /// Strictly increasing per vault, with no gaps.
    pub seq: i64,
    /// `created`, `updated`, `deleted` or `merged` (an entity merged into another, which gets `updated`).
    pub op: String,
    /// `memory`, `entity`, `relation`, `skill` or `document`.
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

// ponytail: in-process only. With several backend replicas a follower hears only the writes of the
// replica it is connected to, and reads the rest from the database on its periodic catch-up
// (`routers/changes.rs`); a shared bus (SurrealDB LIVE SELECT, Redis pub/sub) would make them live.
fn bus() -> &'static broadcast::Sender<Event> {
    static BUS: OnceLock<broadcast::Sender<Event>> = OnceLock::new();
    BUS.get_or_init(|| broadcast::channel(1024).0)
}

/// Every change committed in this process from now on. A lagging receiver skips ahead; followers
/// re-read the gap from the database.
pub fn subscribe() -> broadcast::Receiver<Event> {
    bus().subscribe()
}

/// Appends a change to `vault`'s log, as the current caller. See [`record_many`].
pub async fn record(db: &OrgDb, vault: &RecordId, op: &'static str, kind: &'static str, item: &RecordId) {
    record_many(db, vault, vec![(op, kind, item.clone())]).await;
}

/// Appends changes to `vault`'s log in order, as the current caller, [`BATCH`] rows per transaction
/// (the bulk writes: a vault copy, a connection's data going). Never fails the write it follows (that
/// already committed): a batch still failing after [`ATTEMPTS`] tries is logged and its rows are lost.
pub async fn record_many(db: &OrgDb, vault: &RecordId, items: Vec<(&'static str, &'static str, RecordId)>) {
    if items.is_empty() {
        return;
    }
    let actor = authz::current().map_or_else(|| "system".to_string(), |c| c.actor.id);
    // its own task, awaited: every write path ends here, and polled inline it takes them past a 2 MiB
    // test stack (debug builds); the rows are still written before the write returns
    let (db2, vault2) = (db.clone(), vault.clone());
    let task = tokio::spawn(async move {
        for chunk in items.chunks(BATCH) {
            let mut wait = std::time::Duration::from_millis(100);
            for attempt in 1..=ATTEMPTS {
                match append(&db2, &vault2, chunk, &actor).await {
                    Ok(()) => break,
                    Err(e) if attempt == ATTEMPTS => {
                        tracing::error!(vault = %vault2.to_string(), rows = chunk.len(), error = %e.message, "change log: rows not written");
                    }
                    Err(_) => {
                        tokio::time::sleep(wait).await;
                        wait *= 4;
                    }
                }
            }
        }
    });
    if let Err(e) = task.await {
        tracing::error!(vault = %vault.to_string(), error = %e, "change log: append task failed");
    }
}

/// The vault's counter row (migration 0023).
pub fn counter(vault: &RecordId) -> RecordId {
    RecordId::from_table_key("change_seq", vault.key().clone())
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

async fn append(db: &OrgDb, vault: &RecordId, items: &[(&'static str, &'static str, RecordId)], actor: &str) -> AppResult<()> {
    let rows: Vec<serde_json::Value> =
        items.iter().enumerate().map(|(i, (op, kind, item))| serde_json::json!({"i": i + 1, "op": op, "kind": kind, "item": item.to_string()})).collect();
    // one writer per vault in this process; the transaction still orders writers in other processes
    let _guard = vault_lock(vault).await;
    let changes = with_retry_dup(|| async {
        let mut res = store::changes::APPEND
            .on(db)
            .bind(("counter", counter(vault)))
            .bind(("vault", vault.clone()))
            .bind(("items", rows.clone()))
            .bind(("actor", actor.to_string()))
            .await?;
        res.take::<Vec<Change>>(store::changes::APPEND.slot)
    })
    .await?;
    for change in changes {
        // no receivers is fine
        let _ = bus().send(Event { org: db.org(), vault: vault.clone(), change });
    }
    Ok(())
}

/// Logs the memories a bulk delete reports (`deleted`: `[id, vault]` pairs, as text) in their vaults,
/// and takes the list out of the report, which callers show.
pub async fn record_deleted_memories(db: &OrgDb, report: &mut serde_json::Value) {
    let Some(pairs) = report.as_object_mut().and_then(|o| o.remove("deleted")) else { return };
    let mut by_vault: std::collections::BTreeMap<String, Vec<(&'static str, &'static str, RecordId)>> = Default::default();
    for pair in pairs.as_array().into_iter().flatten() {
        if let (Some(id), Some(vault)) = (pair[0].as_str(), pair[1].as_str())
            && let Ok(id) = crate::rid::parse(id)
        {
            by_vault.entry(vault.to_string()).or_default().push((DELETED, MEMORY, id));
        }
    }
    for (vault, items) in by_vault {
        if let Ok(vault) = crate::rid::parse(&vault) {
            record_many(db, &vault, items).await;
        }
    }
}

/// `vault`'s changes after `after`, oldest first. The caller has authorized the read.
///
/// `after = 0` reads from the oldest row still kept. Any other cursor must lie in the log: a cursor
/// whose next rows retention removed, or one past the newest change (the database was restored from a
/// backup), is `changes.resync`, never a silently short page.
pub async fn after(db: &OrgDb, vault: &RecordId, after: i64, limit: i64) -> AppResult<ChangePage> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let changes: Vec<Change> =
        store::changes::AFTER.on(db).bind(("vault", vault.clone())).bind(("after", after)).bind(("limit", limit)).await?.take(0)?;
    // the common case, the next row right after the cursor, costs nothing more
    if after != 0 && changes.first().is_none_or(|c| c.seq != after + 1) {
        check_cursor(db, vault, after, changes.first().map(|c| c.seq)).await?;
    }
    let next = changes.last().map_or(after, |c| c.seq);
    Ok(ChangePage { changes, next })
}

/// The vault's last seq handed out (0 before its first change) and its oldest kept row's seq.
async fn bounds(db: &OrgDb, vault: &RecordId) -> AppResult<(i64, Option<i64>)> {
    let mut res = store::changes::BOUNDS.on(db).bind(("counter", counter(vault))).bind(("vault", vault.clone())).await?;
    let head: Vec<i64> = res.take(0)?;
    let oldest: Vec<i64> = res.take(1)?;
    Ok((head.first().copied().unwrap_or(0), oldest.first().copied()))
}

/// Refuses a cursor the log cannot continue from. Seqs have no gaps (each comes with its row, in one
/// transaction), so a first row above `after + 1`, or no row while the counter is past `after`, means
/// retention removed the rows in between.
async fn check_cursor(db: &OrgDb, vault: &RecordId, after: i64, first: Option<i64>) -> AppResult<()> {
    let (head, oldest) = bounds(db, vault).await?;
    if after < 0 || after > head {
        return Err(resync(format!("cursor {after} is not in this vault's change log (its newest change is {head}): the log was restored or reset")));
    }
    if first.map_or(head > after, |seq| seq > after + 1) {
        let kept = oldest.map_or_else(|| "none are kept".to_string(), |o| format!("the oldest kept is {o}"));
        return Err(resync(format!("changes after {after} are no longer kept ({kept})")));
    }
    Ok(())
}

fn resync(why: String) -> AppError {
    AppError::coded(ErrorCode::ChangesResync, format!("{why}; re-read what you need and start again from after=0"))
}

/// How long the log keeps rows, read from the environment when retention runs (a change applies at
/// the next run, no restart needed): `CHANGE_LOG_RETENTION_DAYS` (default [`DEFAULT_RETENTION_DAYS`],
/// `0`: whatever their age) and `CHANGE_LOG_MAX_ROWS`, the newest rows kept per vault (default
/// [`DEFAULT_MAX_ROWS`], `0`: no cap). An unset or unreadable value takes the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    pub days: u32,
    pub max_rows: i64,
}

pub const DEFAULT_RETENTION_DAYS: u32 = 90;
pub const DEFAULT_MAX_ROWS: i64 = 100_000;
/// Rows deleted per statement, so one vault's first trim never meets the hardened server's 60 s limit.
const PRUNE_BATCH: i64 = 5_000;
/// How often one process trims one org's log at most.
const PRUNE_EVERY: std::time::Duration = std::time::Duration::from_secs(3600);

impl Retention {
    pub fn from_env() -> Self {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Self {
        let days = get("CHANGE_LOG_RETENTION_DAYS").and_then(|v| v.trim().parse().ok()).unwrap_or(DEFAULT_RETENTION_DAYS);
        let max_rows = get("CHANGE_LOG_MAX_ROWS").and_then(|v| v.trim().parse::<i64>().ok()).filter(|n| *n >= 0).unwrap_or(DEFAULT_MAX_ROWS);
        Retention { days, max_rows }
    }
}

/// Trims every vault's log in `db` to `r`: rows older than `r.days`, and all but the newest
/// `r.max_rows`. The counters stay, so seqs never restart. Returns the rows deleted. Idempotent.
pub async fn prune(db: &OrgDb, r: Retention) -> AppResult<i64> {
    if r.days == 0 && r.max_rows == 0 {
        return Ok(0);
    }
    #[derive(serde::Deserialize, SurrealValue)]
    struct Counter {
        id: RecordId,
        n: i64,
    }
    let counters: Vec<Counter> = store::changes::COUNTERS.on(db).await?.take(0)?;
    let mut gone = 0;
    for c in counters {
        let vault = RecordId::from_table_key("vault", c.id.key().clone());
        let mut cut = if r.max_rows > 0 { c.n - r.max_rows } else { 0 };
        if r.days > 0 {
            let by_age: Vec<i64> =
                store::changes::CUT_BY_AGE.on(db).bind(("vault", vault.clone())).bind(("age", format!("{}d", r.days))).await?.take(0)?;
            cut = cut.max(by_age.first().copied().unwrap_or(0));
        }
        let Some(oldest) = bounds(db, &vault).await?.1.filter(|o| *o <= cut) else { continue };
        let mut lo = oldest - 1;
        while lo < cut {
            let hi = (lo + PRUNE_BATCH).min(cut);
            store::changes::PRUNE_RANGE.on(db).bind(("vault", vault.clone())).bind(("lo", lo)).bind(("hi", hi)).await?.check()?;
            lo = hi;
        }
        gone += cut - oldest + 1;
    }
    Ok(gone)
}

/// [`prune`] with [`Retention::from_env`], at most every [`PRUNE_EVERY`] per org in this process: the
/// scheduler leader calls it on its slow ticks (`jobs/leader.rs`). A failure is logged and retried
/// at the next slow tick.
pub async fn prune_due(db: &OrgDb) {
    static LAST: OnceLock<std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>> = OnceLock::new();
    let key = db.org().key();
    {
        let last = LAST.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
        if last.get(&key).is_some_and(|t| t.elapsed() < PRUNE_EVERY) {
            return;
        }
    }
    match prune(db, Retention::from_env()).await {
        Ok(gone) => {
            if gone > 0 {
                tracing::info!(rows = gone, "change log: trimmed to the retention window");
            }
            LAST.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner()).insert(key, std::time::Instant::now());
        }
        Err(e) => tracing::warn!(error = %e.message, "change log: retention failed"),
    }
}
