//! The hash chain over `audit_event` (docs/audit-chain.md).
//!
//! **Ledgers.** Each org has its own chain (`org:<32 hex>`); events with no org (a failed sign-in
//! for an unknown email) go to the `instance` ledger. Tenants never share a chain.
//!
//! **Two steps.** A writer creates the row `pending` (cheap, and two writers never touch the same
//! record), then seals: it reads the ledger's head, gives the oldest pending rows the next sequence
//! numbers and hashes, and commits them together with the new head in one transaction that first
//! checks the head has not moved ([`store::control::AUDIT_SEAL`]). A writer that lost the race
//! re-reads and tries again, so concurrent writers, in one process or several, make one unbroken
//! chain. Anything left pending (a crash between the steps, a seal that failed) is sealed by
//! [`replay`], which the scheduler runs every tick; a write that failed is queued in memory and
//! replayed by the same pass.
//!
//! **Hash.** `hash = sha256_hex(canonical)` where `canonical` is the compact JSON array
//! `["eunomia.audit.v1", ledger, seq, prev_hash, id, created_at, user, actor_kind, actor_id, action,
//! target, outcome, trace_id, detail_digest]` ([`canonical`]). `created_at` is RFC 3339 UTC
//! with exactly six fractional digits and `Z`; `id` and `user` are `table:key`, `user` is `null`
//! when absent; strings use JSON's minimal escaping (serde_json, the same as Python's
//! `json.dumps(..., ensure_ascii=False, separators=(",", ":"))`). The detail is committed by
//! `detail_digest`, HMAC-SHA256 of the detail under a key derived from `ENCRYPTION_KEY` ([`digest`]),
//! stored on the row when it is written: an export can leave the free text out and still verify, and
//! a digest in an export cannot be reversed by guessing the detail (an email address, say). The first event's `prev_hash` is
//! the genesis hash `sha256_hex(["eunomia.audit.v1","genesis",ledger,genesis_at])`, recorded as
//! checkpoint 0.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use crate::error::{AppError, AppResult};
use crate::pool::{ControlDb, OrgId};
use crate::rid::RecordIdExt;
use crate::store;

use super::Event;

pub const VERSION: &str = "eunomia.audit.v1";
/// The ledger of events that belong to no org.
pub const INSTANCE: &str = "instance";
/// Rows sealed per transaction.
const SEAL_BATCH: i64 = 200;
/// Batches one [`seal`] call commits at most before it returns (the background sealer and the
/// replay come back for the rest).
const MAX_SEAL_BATCHES: u32 = 25;
/// Rows read per page while verifying or exporting.
const PAGE: i64 = 1000;
/// An export holds at most this many events; `next_seq` says where the next one starts.
pub const EXPORT_MAX: i64 = 50_000;
/// An export carries a checkpoint every this many events (and one at its last event).
pub const EXPORT_CHECKPOINT_EVERY: i64 = 1000;
/// Events kept in memory for replay while the database refuses writes. Past this the oldest is
/// dropped, counted in [`DROPPED`] and logged at error level.
const OUTBOX_CAP: usize = 10_000;

/// The ledger of one org's events.
pub fn org_ledger(org: &OrgId) -> String {
    format!("org:{}", org.key())
}

pub fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(bytes.as_ref()))
}

static DETAIL_KEY: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();

/// Sets this process's detail key from `ENCRYPTION_KEY` (at boot, `AppState::build`). Derived like
/// the control database password: nothing about it is stored. The first call wins.
pub fn set_detail_key(encryption_key: &str) {
    DETAIL_KEY.get_or_init(|| derive_detail_key(encryption_key));
}

fn derive_detail_key(encryption_key: &str) -> (String, String) {
    let key = crate::pool::derived_password(encryption_key, "eunomia/audit-detail");
    let id = crate::pool::derived_password(&key, "eunomia/audit-detail-id")[..16].to_string();
    (key, id)
}

fn detail_key() -> &'static (String, String) {
    DETAIL_KEY.get_or_init(|| derive_detail_key(""))
}

/// The fingerprint of the current detail key, stored with each digest so a later key change shows
/// as "not checkable" rather than as tampering.
pub fn detail_key_id() -> &'static str {
    &detail_key().1
}

/// `digest_key` of a detail committed by its plain SHA-256 (see [`public_detail`]).
pub const PLAIN: &str = "sha256";

/// Actions whose detail holds no personal data, only hashes and counts, so it is committed by its
/// plain SHA-256: anyone holding the detail can check it against the chain offline. An access review's
/// completion is one: its detail carries the review's `result_hash`, which an auditor checks against
/// the exported review (`verify-audit-export.py --review`). Every other detail is committed by the keyed
/// [`digest`], because it can hold an email address or a name.
pub fn public_detail(action: &str) -> bool {
    matches!(action, "access_review.start" | "access_review.complete")
}

/// What a sealed event's detail becomes when the person it named is erased
/// (docs/privacy/subject-rights.md, "Audit trail"). The hash covers the detail's digest, not the detail,
/// so the chain still verifies; the detail itself can no longer be checked and counts as unchecked.
pub const ERASED_DETAIL: &str = "[erased with the person it named]";

/// HMAC-SHA256 of an event's detail under the detail key, hex.
pub fn digest(detail: &str) -> String {
    use hmac::{Hmac, Mac};
    let mut mac = Hmac::<Sha256>::new_from_slice(detail_key().0.as_bytes()).expect("hmac takes any key length");
    mac.update(detail.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// The canonical time: RFC 3339, UTC, six fractional digits, `Z`.
pub fn ts(t: &DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Micros, true)
}

fn now_micros() -> DateTime<Utc> {
    let now = Utc::now();
    DateTime::from_timestamp_micros(now.timestamp_micros()).unwrap_or(now)
}

/// `table:key`, the key unescaped (record keys here are plain strings).
pub fn rid(r: &RecordId) -> String {
    format!("{}:{}", r.table(), crate::rid::key_string(r.key()).unwrap_or_else(|| r.to_string()))
}

/// The fields one event's hash covers, in their canonical form.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct AuditEventFields {
    pub ledger: String,
    pub seq: i64,
    pub prev_hash: String,
    pub id: String,
    pub created_at: String,
    pub user: Option<String>,
    pub actor_kind: String,
    pub actor_id: String,
    pub action: String,
    pub target: String,
    pub outcome: String,
    pub trace_id: String,
    pub detail_digest: String,
}

/// The exact bytes an event's hash is taken over.
pub fn canonical(c: &AuditEventFields) -> String {
    serde_json::Value::Array(vec![
        VERSION.into(),
        c.ledger.clone().into(),
        c.seq.into(),
        c.prev_hash.clone().into(),
        c.id.clone().into(),
        c.created_at.clone().into(),
        c.user.clone().map(Into::into).unwrap_or(serde_json::Value::Null),
        c.actor_kind.clone().into(),
        c.actor_id.clone().into(),
        c.action.clone().into(),
        c.target.clone().into(),
        c.outcome.clone().into(),
        c.trace_id.clone().into(),
        c.detail_digest.clone().into(),
    ])
    .to_string()
}

pub fn event_hash(c: &AuditEventFields) -> String {
    sha256_hex(canonical(c))
}

pub fn genesis_hash(ledger: &str, at: &str) -> String {
    sha256_hex(serde_json::json!([VERSION, "genesis", ledger, at]).to_string())
}

// ---------------------------------------------------------------- writing

/// One event, owned, on its way to the database.
#[derive(Debug, Clone)]
pub(crate) struct Pending {
    id: String,
    ledger: Option<String>,
    user: Option<RecordId>,
    actor_kind: String,
    actor_id: String,
    action: String,
    target: String,
    outcome: String,
    trace_id: String,
    detail: String,
    detail_digest: String,
    digest_key: String,
    created_at: DateTime<Utc>,
}

impl Pending {
    pub(crate) fn action(&self) -> &str {
        &self.action
    }

    pub(crate) fn new(e: Event<'_>, ledger: Option<String>) -> Self {
        Pending {
            id: uuid::Uuid::new_v4().simple().to_string(),
            ledger,
            user: e.user.cloned(),
            actor_kind: e.actor.kind.to_string(),
            actor_id: e.actor.id.clone(),
            action: e.action.to_string(),
            target: e.target.to_string(),
            outcome: e.outcome.to_string(),
            trace_id: crate::telemetry::current_trace_id(),
            detail: e.detail.to_string(),
            detail_digest: if public_detail(e.action) { sha256_hex(e.detail) } else { digest(e.detail) },
            digest_key: if public_detail(e.action) { PLAIN.to_string() } else { detail_key_id().to_string() },
            created_at: now_micros(),
        }
    }
}

/// Events dropped because a replay queue was full (logged at error level each time).
pub static DROPPED: AtomicU64 = AtomicU64::new(0);

/// A process's queue of events whose write failed. Every process (each replica, leader or not)
/// has its own and drains it on its own timer ([`spawn_drain`]) and at graceful shutdown
/// ([`shutdown_flush`]); nothing else reads it. An event held only here is lost if the process
/// dies before the database accepts it (docs/audit-chain.md, "What can go wrong").
pub struct Outbox {
    q: Mutex<VecDeque<(ControlDb, Pending)>>,
}

impl Default for Outbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Outbox {
    pub const fn new() -> Self {
        Outbox { q: Mutex::new(VecDeque::new()) }
    }

    /// Events waiting in this queue.
    pub fn len(&self) -> usize {
        self.q.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn push(&self, db: ControlDb, p: Pending) {
        let mut q = self.q.lock().unwrap_or_else(|e| e.into_inner());
        if q.len() >= OUTBOX_CAP
            && let Some((_, lost)) = q.pop_front()
        {
            DROPPED.fetch_add(1, Ordering::Relaxed);
            tracing::error!(action = %lost.action, id = %lost.id, "audit replay queue full: oldest event dropped");
        }
        q.push_back((db, p));
    }

    /// Writes up to `max` queued events, oldest first, each to its own database, and schedules their
    /// ledgers' seals. Those that still fail go back to the front, in order. Returns how many it wrote.
    pub async fn flush(&self, max: usize) -> usize {
        let batch: Vec<(ControlDb, Pending)> = {
            let mut q = self.q.lock().unwrap_or_else(|e| e.into_inner());
            let n = max.min(q.len());
            q.drain(..n).collect()
        };
        let mut written = 0;
        let mut failed = Vec::new();
        for (db, mut p) in batch {
            // after one failure the database is likely still away: keep the rest for the next pass
            if !failed.is_empty() {
                failed.push((db, p));
                continue;
            }
            match write(&db, &mut p).await {
                Ok(()) => {
                    written += 1;
                    mark_dirty(&db, p.ledger.clone().unwrap_or_else(|| INSTANCE.into()));
                }
                Err(_) => failed.push((db, p)),
            }
        }
        let mut q = self.q.lock().unwrap_or_else(|e| e.into_inner());
        for item in failed.into_iter().rev() {
            q.push_front(item);
        }
        written
    }

    /// Test-only (feature `test-support`): queues one event as if its write had failed.
    #[cfg(feature = "test-support")]
    pub fn queue_for_test(&self, db: &ControlDb, user: &RecordId, action: &str) {
        let actor = crate::authz::Actor { kind: "user", id: rid(user) };
        let p = Pending::new(Event { user: Some(user), actor: &actor, action, target: "", outcome: "ok", detail: "" }, None);
        self.push(db.clone(), p);
    }
}

/// This process's queue.
static OUTBOX: Outbox = Outbox::new();

pub fn outbox() -> &'static Outbox {
    &OUTBOX
}

/// Events waiting in this process's replay queue.
pub fn queued() -> usize {
    OUTBOX.len()
}

/// Queued events one drain pass writes at most (the rest wait for the next pass).
const DRAIN_BATCH: usize = 500;

/// The background pass, every `every` until the process ends: writes what `outbox` holds, then
/// seals every ledger this process wrote to since the last pass ([`seal_dirty`]), in batches. The
/// server starts one for its own queue in `main` ([`start_drain`]); tests start their own.
pub fn spawn_drain(outbox: &'static Outbox, every: Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if !outbox.is_empty() {
                let n = outbox.flush(DRAIN_BATCH).await;
                if n > 0 {
                    tracing::info!(written = n, left = outbox.len(), "audit replay queue drained");
                }
            }
            seal_dirty().await;
        }
    })
}

/// How often the background pass runs in the server.
pub const DRAIN_EVERY: Duration = Duration::from_secs(5);

/// Starts this process's background pass once (the server calls it at start-up).
pub fn start_drain() {
    static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    STARTED.get_or_init(|| {
        spawn_drain(&OUTBOX, DRAIN_EVERY);
    });
}

/// Graceful shutdown: tries to write every queued event before the process exits, for up to `wait`.
/// What is still queued after that is lost, and logged at error level.
pub async fn shutdown_flush(wait: Duration) {
    let deadline = tokio::time::Instant::now() + wait;
    while !OUTBOX.is_empty() && tokio::time::Instant::now() < deadline {
        if tokio::time::timeout_at(deadline, OUTBOX.flush(DRAIN_BATCH)).await.map(|n| n == 0).unwrap_or(true) {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    let left = OUTBOX.len();
    if left > 0 {
        tracing::error!(left, "audit events still queued at shutdown were lost");
    }
    // and seal what this process wrote since the last pass, so nothing it wrote waits for the leader
    if tokio::time::timeout_at(deadline, seal_dirty()).await.is_err() {
        tracing::warn!("audit seal at shutdown did not finish; the scheduler's replay seals the rest");
    }
}

/// Test-only fault injection (feature `test-support`): the next this-many writes fail as if the
/// database refused them. Process-global, so the tests that use it serialise on a lock.
#[cfg(feature = "test-support")]
pub static FAIL_WRITES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// Test-only (feature `test-support`): every seal of this ledger fails, as a ledger with a row that
/// cannot be sealed would.
#[cfg(feature = "test-support")]
pub static FAIL_SEAL_LEDGER: Mutex<Option<String>> = Mutex::new(None);
/// Test-only: when set, a written event is not handed to the background pass (as if the process died
/// right after the write), so only [`replay`] or an explicit [`seal`] can seal it.
#[cfg(feature = "test-support")]
pub static SKIP_INLINE_SEAL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Writes one event and marks its ledger for the next background pass. The caller waits only for
/// the write (one statement, at most two quick retries; one more to find the user's org when the
/// caller did not name it): never for a seal, a lock, another writer or the replay queue, and
/// nothing is sent behind the request either. A write that cannot be made goes to this process's
/// queue. The row is durable once written: if the process dies before the pass, the scheduler
/// leader's [`replay`] seals it.
pub(crate) async fn append(db: ControlDb, mut p: Pending) {
    match write(&db, &mut p).await {
        Ok(()) => {
            #[cfg(feature = "test-support")]
            if SKIP_INLINE_SEAL.load(Ordering::SeqCst) {
                return;
            }
            mark_dirty(&db, p.ledger.clone().unwrap_or_else(|| INSTANCE.into()));
        }
        Err(e) => {
            tracing::warn!(action = %p.action, error = %e, "audit_event write failed; queued for replay");
            OUTBOX.push(db, p);
        }
    }
}

/// Ledgers this process wrote to since the last background pass, per database (keyed by its
/// identity, with a handle to it). The pass seals each once, in batches, however many events arrived.
type SealKey = (usize, String);
type Dirty = std::collections::HashMap<usize, (ControlDb, std::collections::BTreeSet<String>)>;
static DIRTY: Mutex<Option<Dirty>> = Mutex::new(None);

fn mark_dirty(db: &ControlDb, ledger: String) {
    let mut m = DIRTY.lock().unwrap_or_else(|e| e.into_inner());
    m.get_or_insert_with(Default::default).entry(db.identity()).or_insert_with(|| (db.clone(), Default::default())).1.insert(ledger);
}

/// Seals every ledger marked since the last call. A ledger whose seal fails is marked again for the
/// next pass (and the leader's replay seals it too). Returns how many rows it sealed.
pub async fn seal_dirty() -> u64 {
    let batch: Dirty = DIRTY.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default();
    let mut sealed = 0;
    for (_, (db, ledgers)) in batch {
        for ledger in ledgers {
            // one ledger that hangs (a database that went away) must not hold the pass for the others
            match tokio::time::timeout(SEAL_PASS_LIMIT, seal(&db, &ledger)).await {
                Ok(Ok(n)) => sealed += n,
                Ok(Err(e)) => {
                    tracing::warn!(ledger = %ledger, error = %e.message, "audit seal deferred to the next pass");
                    mark_dirty(&db, ledger);
                }
                Err(_) => {
                    tracing::warn!(ledger = %ledger, "audit seal timed out; deferred to the next pass");
                    mark_dirty(&db, ledger);
                }
            }
        }
    }
    sealed
}

/// How long the background pass gives one ledger's seal.
const SEAL_PASS_LIMIT: Duration = Duration::from_secs(30);

/// Test-only (feature `test-support`): forgets the ledgers marked for the next pass (a finished test's
/// database is gone with its runtime).
#[cfg(feature = "test-support")]
pub fn forget_marked() {
    DIRTY.lock().unwrap_or_else(|e| e.into_inner()).take();
}

#[derive(Debug, Deserialize, SurrealValue)]
struct OrgRow {
    org: RecordId,
}

async fn resolve_ledger(db: &ControlDb, user: Option<&RecordId>) -> surrealdb::Result<String> {
    let Some(user) = user else { return Ok(INSTANCE.into()) };
    let rows: Vec<OrgRow> = store::control::ORG_OF_USER.on(db).bind(("user", user.clone())).await?.take(0)?;
    Ok(match rows.first().and_then(|r| crate::rid::key_string(r.org.key())) {
        Some(key) => format!("org:{key}"),
        None => INSTANCE.into(),
    })
}

/// Creates the pending row. Idempotent: the id is the event's own, so a replay of an event that did
/// land is a duplicate and counts as written.
async fn write(db: &ControlDb, p: &mut Pending) -> surrealdb::Result<()> {
    #[cfg(feature = "test-support")]
    if FAIL_WRITES.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok() {
        return Err(surrealdb::Error::internal("injected audit write failure".into()));
    }
    if p.ledger.is_none() {
        p.ledger = Some(resolve_ledger(db, p.user.as_ref()).await?);
    }
    let mut attempt = 0u64;
    loop {
        let res = store::control::AUDIT_EVENT_CREATE
            .on(db)
            .bind(("id", RecordId::new("audit_event", p.id.as_str())))
            .bind(("user", p.user.clone()))
            .bind(("actor_kind", p.actor_kind.clone()))
            .bind(("actor_id", p.actor_id.clone()))
            .bind(("action", p.action.clone()))
            .bind(("target", p.target.clone()))
            .bind(("outcome", p.outcome.clone()))
            .bind(("trace_id", p.trace_id.clone()))
            .bind(("detail", p.detail.clone()))
            .bind(("detail_digest", p.detail_digest.clone()))
            .bind(("digest_key", p.digest_key.clone()))
            .bind(("created_at", Datetime::from(p.created_at)))
            .bind(("ledger", p.ledger.clone()))
            .await;
        match res {
            Ok(_) => return Ok(()),
            Err(e) if crate::tx::is_duplicate(&e) => return Ok(()),
            Err(e) if attempt < 2 => {
                tracing::debug!(error = %e, attempt, "audit_event write failed, retrying");
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(25 * attempt)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// What one [`replay`] pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Replayed {
    /// Queued events written.
    pub written: usize,
    /// Pending rows sealed.
    pub sealed: u64,
}

/// The durable replay: writes a batch of this process's queued events, then seals every pending
/// row of every ledger (whichever process wrote it). The scheduler leader runs it each tick; every
/// process also drains its own queue on its own timer ([`spawn_drain`]). Safe to run anywhere, any
/// number of times at once.
pub async fn replay(db: &ControlDb) -> AppResult<Replayed> {
    let written = OUTBOX.flush(DRAIN_BATCH).await;
    #[derive(Deserialize, SurrealValue)]
    struct L {
        #[serde(default)]
        #[surreal(default)]
        ledger: Option<String>,
    }
    let ledgers: Vec<L> = store::control::AUDIT_PENDING_LEDGERS.on(db).await?.take(0)?;
    let mut sealed = 0;
    for l in ledgers.into_iter().filter_map(|l| l.ledger) {
        // one ledger that fails is logged, counted and backed off; the rest carry on
        let key = (db.identity(), l.clone());
        if backing_off(&key) {
            continue;
        }
        match seal(db, &l).await {
            Ok(n) => {
                sealed += n;
                backoff_clear(&key);
            }
            Err(e) => {
                SEAL_FAILURES.fetch_add(1, Ordering::Relaxed);
                let wait = backoff_note(&key);
                tracing::warn!(ledger = %l, error = %e.message, retry_in_secs = wait.as_secs(), "audit replay could not seal this ledger; carrying on with the others");
            }
        }
    }
    Ok(Replayed { written, sealed })
}

/// Ledgers whose seal failed in a replay pass (each failure counted once).
pub static SEAL_FAILURES: AtomicU64 = AtomicU64::new(0);

static BACKOFF: Mutex<Option<std::collections::HashMap<SealKey, (std::time::Instant, u32)>>> = Mutex::new(None);

fn backing_off(key: &SealKey) -> bool {
    let m = BACKOFF.lock().unwrap_or_else(|e| e.into_inner());
    m.as_ref().and_then(|m| m.get(key)).is_some_and(|(until, _)| std::time::Instant::now() < *until)
}

fn backoff_clear(key: &SealKey) {
    if let Some(m) = BACKOFF.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        m.remove(key);
    }
}

/// Records one more failure for `key` and returns how long the replay leaves it alone: 2, 4, 8 ...
/// seconds, at most five minutes.
fn backoff_note(key: &SealKey) -> Duration {
    let mut m = BACKOFF.lock().unwrap_or_else(|e| e.into_inner());
    let entry = m.get_or_insert_with(Default::default).entry(key.clone()).or_insert((std::time::Instant::now(), 0));
    entry.1 = entry.1.saturating_add(1);
    let wait = Duration::from_secs((1u64 << entry.1.min(9)).min(300));
    entry.0 = std::time::Instant::now() + wait;
    wait
}

// ---------------------------------------------------------------- sealing

/// One stored event, as the chain reads it.
#[derive(Debug, Clone, Deserialize, SurrealValue)]
pub(crate) struct Row {
    pub id: RecordId,
    #[serde(default)]
    #[surreal(default)]
    pub ledger: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    pub seq: Option<i64>,
    #[serde(default)]
    #[surreal(default)]
    pub prev_hash: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    pub hash: Option<String>,
    pub created_at: Datetime,
    #[serde(default)]
    #[surreal(default)]
    pub user: Option<RecordId>,
    #[serde(default)]
    #[surreal(default)]
    pub actor_kind: String,
    #[serde(default)]
    #[surreal(default)]
    pub actor_id: String,
    #[serde(default)]
    #[surreal(default)]
    pub action: String,
    #[serde(default)]
    #[surreal(default)]
    pub target: String,
    #[serde(default)]
    #[surreal(default)]
    pub outcome: String,
    #[serde(default)]
    #[surreal(default)]
    pub trace_id: String,
    #[serde(default)]
    #[surreal(default)]
    pub detail: String,
    #[serde(default)]
    #[surreal(default)]
    pub detail_digest: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    pub digest_key: Option<String>,
}

impl Row {
    /// The digest the hash covers: the one stored when the row was written.
    fn stored_digest(&self) -> String {
        self.detail_digest.clone().unwrap_or_else(|| digest(&self.detail))
    }

    /// `Some(false)` when the detail no longer matches its digest, `None` when the digest was made
    /// under another detail key (the encryption key changed since), so it cannot be checked here.
    fn detail_matches(&self) -> Option<bool> {
        if self.detail == ERASED_DETAIL && self.detail_digest.is_some() {
            return None;
        }
        match self.digest_key.as_deref() {
            Some(PLAIN) => Some(sha256_hex(&self.detail) == self.stored_digest()),
            Some(k) if k == detail_key_id() => Some(digest(&self.detail) == self.stored_digest()),
            None if self.detail_digest.is_none() => Some(true),
            _ => None,
        }
    }

    /// The canonical fields of this row placed at `seq` after `prev_hash` in `ledger`.
    fn canonical(&self, ledger: &str, seq: i64, prev_hash: &str) -> AuditEventFields {
        AuditEventFields {
            ledger: ledger.to_string(),
            seq,
            prev_hash: prev_hash.to_string(),
            id: rid(&self.id),
            created_at: crate::sources::base::datetime_to_chrono(&self.created_at).map(|t| ts(&t)).unwrap_or_default(),
            user: self.user.as_ref().map(rid),
            actor_kind: self.actor_kind.clone(),
            actor_id: self.actor_id.clone(),
            action: self.action.clone(),
            target: self.target.clone(),
            outcome: self.outcome.clone(),
            trace_id: self.trace_id.clone(),
            detail_digest: self.stored_digest(),
        }
    }

    /// The canonical fields as stored (its own ledger, seq and prev_hash).
    fn stored(&self) -> AuditEventFields {
        self.canonical(self.ledger.as_deref().unwrap_or_default(), self.seq.unwrap_or_default(), self.prev_hash.as_deref().unwrap_or_default())
    }
}

#[derive(Debug, Clone, Deserialize, SurrealValue)]
pub(crate) struct Head {
    pub ledger: String,
    pub seq: i64,
    pub hash: String,
    #[serde(default)]
    #[surreal(default)]
    pub genesis_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    pub updated_at: Option<Datetime>,
}

#[derive(Debug, Clone, Serialize, SurrealValue)]
struct SealRow {
    id: RecordId,
    seq: i64,
    prev_hash: String,
    hash: String,
}

fn head_id(ledger: &str) -> RecordId {
    RecordId::new("audit_head", ledger)
}

async fn read_head(db: &ControlDb, ledger: &str) -> AppResult<Option<Head>> {
    let rows: Vec<Head> = store::control::AUDIT_HEAD.on(db).bind(("head", head_id(ledger))).await?.take(0)?;
    Ok(rows.into_iter().next())
}

async fn heads(db: &ControlDb) -> AppResult<Vec<Head>> {
    Ok(store::control::AUDIT_HEADS.on(db).await?.take(0)?)
}

/// The audit sealer's own locks, one per database and ledger, apart from the striped `tx::lock`
/// that request paths (sign-up, job claims) share.
static SEAL_LOCKS: Mutex<Option<std::collections::HashMap<SealKey, std::sync::Arc<tokio::sync::Mutex<()>>>>> = Mutex::new(None);

async fn seal_lock(db: &ControlDb, ledger: &str) -> Option<tokio::sync::OwnedMutexGuard<()>> {
    #[cfg(feature = "test-support")]
    if crate::tx::LOCKS_OFF.load(Ordering::SeqCst) {
        return None;
    }
    let lock = {
        let mut m = SEAL_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
        m.get_or_insert_with(Default::default).entry((db.identity(), ledger.to_string())).or_default().clone()
    };
    Some(lock.lock_owned().await)
}

fn lost_race(e: &surrealdb::Error) -> bool {
    crate::tx::is_conflict(e) || crate::tx::is_duplicate(e) || e.to_string().contains("audit.chain_moved")
}

/// Seals every pending row of `ledger`, oldest first. Returns how many it sealed.
pub async fn seal(db: &ControlDb, ledger: &str) -> AppResult<u64> {
    #[cfg(feature = "test-support")]
    if FAIL_SEAL_LEDGER.lock().unwrap_or_else(|e| e.into_inner()).as_deref() == Some(ledger) {
        return Err(AppError::internal("injected seal failure"));
    }
    let mut sealed = 0u64;
    let mut lost = 0u32;
    let mut batches = 0u32;
    loop {
        // one sealer per ledger in this process, held for one batch only (never across a retry
        // pause), on locks of its own: request paths never wait behind a seal. Other processes meet
        // at the head check.
        let _guard = seal_lock(db, ledger).await;
        let head = read_head(db, ledger).await?;
        let rows: Vec<Row> =
            store::control::AUDIT_PENDING.on(db).bind(("ledger", ledger.to_string())).bind(("limit", SEAL_BATCH)).await?.take(0)?;
        if rows.is_empty() {
            return Ok(sealed);
        }
        // a short batch means this call has caught up: rows written from now on are the next
        // call's (the background sealer goes round again for them), so a busy ledger cannot keep
        // one call, or a `verify` waiting on it, sealing forever
        let caught_up = (rows.len() as i64) < SEAL_BATCH;
        let (expect, mut seq, mut prev, genesis) = match &head {
            Some(h) => (h.seq, h.seq, h.hash.clone(), None),
            None => {
                let at = now_micros();
                let g = genesis_hash(ledger, &ts(&at));
                (-1, 0, g.clone(), Some((g, at)))
            }
        };
        let seals: Vec<SealRow> = rows
            .iter()
            .map(|r| {
                seq += 1;
                let hash = event_hash(&r.canonical(ledger, seq, &prev));
                SealRow { id: r.id.clone(), seq, prev_hash: std::mem::replace(&mut prev, hash.clone()), hash }
            })
            .collect();
        let n = seals.len() as u64;
        let res = store::control::AUDIT_SEAL
            .on(db)
            .bind(("head", head_id(ledger)))
            .bind(("expect", expect))
            .bind(("ledger", ledger.to_string()))
            .bind(("rows", seals))
            .bind(("seq", seq))
            .bind(("hash", prev))
            .bind(("genesis_hash", genesis.as_ref().map(|g| g.0.clone())))
            .bind(("genesis_at", genesis.map(|g| Datetime::from(g.1))))
            .await;
        match res {
            Ok(_) if caught_up => return Ok(sealed + n),
            Ok(_) => {
                sealed += n;
                batches += 1;
                if batches >= MAX_SEAL_BATCHES {
                    return Ok(sealed);
                }
            }
            Err(e) if lost_race(&e) && lost < 50 => {
                drop(_guard);
                lost += 1;
                let ms = rand::random::<u64>() % (5 * lost as u64).min(100) + 1;
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

// ---------------------------------------------------------------- checkpoints and retention

#[derive(Debug, Clone, Deserialize, SurrealValue)]
pub(crate) struct CheckpointRow {
    pub ledger: String,
    pub seq: i64,
    pub hash: String,
    pub kind: String,
    pub at: Datetime,
    #[serde(default)]
    #[surreal(default)]
    pub pruned: Option<i64>,
}

/// A point a verifier can start from or compare against.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct AuditCheckpoint {
    pub seq: i64,
    pub hash: String,
    /// RFC 3339.
    pub at: String,
    /// `genesis` (the chain's start, seq 0), `retention` (the last event the retention window
    /// deleted), `periodic` (a copy of the head) or `export` (written into an export file).
    pub kind: String,
    /// For `retention`: how many events that cut deleted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pruned: Option<i64>,
}

impl From<&CheckpointRow> for AuditCheckpoint {
    fn from(c: &CheckpointRow) -> Self {
        AuditCheckpoint {
            seq: c.seq,
            hash: c.hash.clone(),
            at: crate::sources::base::datetime_to_chrono(&c.at).map(|t| ts(&t)).unwrap_or_default(),
            kind: c.kind.clone(),
            pruned: c.pruned,
        }
    }
}

async fn checkpoints(db: &ControlDb, ledger: &str) -> AppResult<Vec<CheckpointRow>> {
    Ok(store::control::AUDIT_CHECKPOINTS.on(db).bind(("ledger", ledger.to_string())).await?.take(0)?)
}

#[derive(Debug, Deserialize, SurrealValue)]
struct SeqHash {
    seq: i64,
    #[serde(default)]
    #[surreal(default)]
    hash: Option<String>,
}

/// Retention for chained ledgers: the events sealed before the first one written at or after
/// `cutoff` are deleted, and the last deleted event's sequence and hash are kept as a `retention`
/// checkpoint in the same transaction. The cut is found from the newest events down, so one row with
/// an old timestamp sealed late (a replayed event, a replica with a slow clock) can never pull newer
/// events inside the window into the cut.
pub async fn prune(db: &ControlDb, cutoff: DateTime<Utc>) -> AppResult<()> {
    // seal what can be sealed first; a ledger that cannot is skipped below, not the whole run
    if let Err(e) = replay(db).await {
        tracing::warn!(error = %e.message, "audit replay before retention failed; cutting what is sealed");
    }
    for head in heads(db).await? {
        let ledger = head.ledger.clone();
        if let Err(e) = cut_ledger(db, &head, cutoff).await {
            SEAL_FAILURES.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(ledger = %ledger, error = %e.message, "audit retention could not cut this ledger; carrying on with the others");
        }
    }
    Ok(())
}

async fn cut_ledger(db: &ControlDb, head: &Head, cutoff: DateTime<Utc>) -> AppResult<()> {
    let ledger = head.ledger.clone();
    let since: Vec<SeqHash> =
        store::control::AUDIT_FIRST_SEQ_SINCE.on(db).bind(("ledger", ledger.clone())).bind(("from", Datetime::from(cutoff))).await?.take(0)?;
    let cut = match since.first() {
        Some(r) => r.seq - 1,
        // nothing inside the window: everything sealed so far is older
        None => head.seq,
    };
    if cut < 1 {
        return Ok(());
    }
    let rows = page(db, &ledger, cut - 1, cut, 1).await?;
    let Some(hash) = rows.first().and_then(|r| r.hash.clone()) else { return Ok(()) }; // already cut
    store::control::AUDIT_RETENTION_CUT.on(db).bind(("ledger", ledger.clone())).bind(("seq", cut)).bind(("hash", hash)).await?.check()?;
    tracing::info!(ledger = %ledger, through_seq = cut, "audit retention cut, checkpoint kept");
    Ok(())
}

/// Writes a `periodic` checkpoint (a copy of the head) for every ledger whose head moved since its
/// newest checkpoint, at most once per `every`. It first checks the events since that checkpoint,
/// so it never endorses a head that does not follow from the last one; a ledger that fails gets no
/// checkpoint and an error in the log. Run by the scheduler; these checkpoints are what an operator
/// copies off the server to anchor the chain (docs/audit-chain.md).
pub async fn checkpoint_due(db: &ControlDb, every: TimeDelta) -> AppResult<usize> {
    let mut written = 0;
    for head in heads(db).await? {
        let latest: Vec<CheckpointRow> =
            store::control::AUDIT_LATEST_CHECKPOINT.on(db).bind(("ledger", head.ledger.clone())).await?.take(0)?;
        let Some(from) = latest.first() else { continue }; // no genesis: verify reports it
        let due = from.seq < head.seq && crate::sources::base::datetime_to_chrono(&from.at).is_none_or(|at| Utc::now() - at >= every);
        if !due {
            continue;
        }
        let mut walk = Walk::new(from.seq, from.hash.clone());
        let mut after = from.seq;
        while after < head.seq && walk.broken.is_none() {
            let rows = page(db, &head.ledger, after, head.seq, PAGE).await?;
            let Some(last) = rows.last() else { break };
            after = last.seq.unwrap_or(head.seq);
            for r in &rows {
                if !walk.step(&r.stored(), r.hash.as_deref().unwrap_or_default()) {
                    break;
                }
            }
        }
        if walk.broken.is_some() || walk.last_seq() != head.seq || walk.prev_hash != head.hash {
            tracing::error!(ledger = %head.ledger, from = from.seq, head = head.seq, "audit chain does not follow from its last checkpoint: no new checkpoint written");
            continue;
        }
        store::control::AUDIT_CHECKPOINT_CREATE
            .on(db)
            .bind(("ledger", head.ledger.clone()))
            .bind(("seq", head.seq))
            .bind(("hash", head.hash.clone()))
            .bind(("kind", "periodic"))
            .await?
            .check()?;
        written += 1;
    }
    Ok(written)
}

// ---------------------------------------------------------------- verification

/// Where and how a chain stops verifying.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct AuditChainBreak {
    /// The sequence number where the chain stops verifying, when there is one.
    pub seq: Option<i64>,
    /// `altered` (an event's content no longer matches its hash), `link` (an event does not
    /// follow the one before it), `missing` (events are gone), `duplicate`, `head` (the newest
    /// event is not the recorded head), `checkpoint` (a checkpoint disagrees with its event) or
    /// `genesis`.
    pub kind: String,
    /// The break in plain words.
    pub message: String,
}

fn brk(seq: Option<i64>, kind: &str, message: String) -> AuditChainBreak {
    AuditChainBreak { seq, kind: kind.into(), message }
}

/// The result of walking one ledger.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AuditVerification {
    pub ledger: String,
    /// True when every stored event verified from the anchor to the head.
    pub ok: bool,
    /// Events whose hash and link were checked.
    pub checked: u64,
    pub first_seq: Option<i64>,
    pub last_seq: Option<i64>,
    pub head_seq: Option<i64>,
    pub head_hash: Option<String>,
    /// The checkpoint the walk started from (the genesis, or the last retention cut).
    pub anchor: Option<AuditCheckpoint>,
    /// The first break, if any.
    #[serde(rename = "break")]
    pub broken: Option<AuditChainBreak>,
    /// Events written but not sealed yet (sealed by the next replay).
    pub pending: i64,
    /// For the `instance` ledger: events written before the chain started, which have no hash.
    pub before_chain: i64,
    /// Events whose detail could not be checked against its digest because the encryption key
    /// changed since they were written (their hash and link were still checked).
    pub detail_unchecked: u64,
    /// When the check ran, RFC 3339.
    pub checked_at: String,
}

/// Walks consecutive rows from an anchor: each must have the next sequence number, link to the
/// previous hash and hash to its stored value. Returns the first break and the last good (seq, hash).
pub(crate) struct Walk {
    pub expect_seq: i64,
    pub prev_hash: String,
    pub checked: u64,
    pub broken: Option<AuditChainBreak>,
}

impl Walk {
    pub(crate) fn new(anchor_seq: i64, anchor_hash: String) -> Self {
        Walk { expect_seq: anchor_seq + 1, prev_hash: anchor_hash, checked: 0, broken: None }
    }

    /// Checks the next event; false once broken.
    pub(crate) fn step(&mut self, c: &AuditEventFields, stored_hash: &str) -> bool {
        if self.broken.is_some() {
            return false;
        }
        if c.seq < self.expect_seq {
            self.broken = Some(brk(Some(c.seq), "duplicate", format!("Event {} appears more than once.", c.seq)));
        } else if c.seq > self.expect_seq {
            let msg = if c.seq == self.expect_seq + 1 {
                format!("Event {} is missing.", self.expect_seq)
            } else {
                format!("Events {} to {} are missing.", self.expect_seq, c.seq - 1)
            };
            self.broken = Some(brk(Some(self.expect_seq), "missing", msg));
        } else if c.prev_hash != self.prev_hash {
            self.broken = Some(brk(Some(c.seq), "link", format!("Event {} does not follow the event before it.", c.seq)));
        } else if event_hash(c) != stored_hash {
            self.broken = Some(brk(Some(c.seq), "altered", format!("Event {} was changed after it was written.", c.seq)));
        } else {
            self.expect_seq += 1;
            self.prev_hash = stored_hash.to_string();
            self.checked += 1;
        }
        self.broken.is_none()
    }

    pub(crate) fn last_seq(&self) -> i64 {
        self.expect_seq - 1
    }
}

async fn count(db: &ControlDb, stmt: &store::ControlStmt, ledger: &str) -> AppResult<i64> {
    #[derive(Deserialize, SurrealValue)]
    struct N {
        #[serde(default)]
        #[surreal(default)]
        n: i64,
    }
    let rows: Vec<N> = stmt.on(db).bind(("ledger", ledger.to_string())).await?.take(0)?;
    Ok(rows.first().map(|r| r.n).unwrap_or(0))
}

async fn page(db: &ControlDb, ledger: &str, after: i64, until: i64, limit: i64) -> AppResult<Vec<Row>> {
    Ok(store::control::AUDIT_CHAIN_PAGE
        .on(db)
        .bind(("ledger", ledger.to_string()))
        .bind(("after", after))
        .bind(("until", until))
        .bind(("limit", limit))
        .await?
        .take(0)?)
}

/// The checkpoint a walk starting at `first_seq` hangs from: one stored at `first_seq - 1`
/// (the genesis or a retention cut preferred).
fn anchor_for(cps: &[CheckpointRow], seq: i64) -> Option<&CheckpointRow> {
    let at: Vec<&CheckpointRow> = cps.iter().filter(|c| c.seq == seq).collect();
    at.iter().find(|c| c.kind == "genesis" || c.kind == "retention").or_else(|| at.first()).copied()
}

async fn verify_reads(db: &ControlDb, ledger: &str) -> AppResult<(Vec<CheckpointRow>, Option<i64>, Option<Head>)> {
    let cps = checkpoints(db, ledger).await?;
    let first: Vec<SeqHash> = store::control::AUDIT_FIRST_SEQ.on(db).bind(("ledger", ledger.to_string())).await?.take(0)?;
    let head = read_head(db, ledger).await?;
    Ok((cps, first.first().map(|r| r.seq), head))
}

/// Seals what is pending, then walks the ledger from its anchor to its head and reports the
/// first break.
pub async fn verify(db: &ControlDb, ledger: &str) -> AppResult<AuditVerification> {
    if let Err(e) = seal(db, ledger).await {
        tracing::warn!(ledger = %ledger, error = %e.message, "audit seal before verify failed");
    }
    let mut v = AuditVerification {
        ledger: ledger.to_string(),
        ok: true,
        checked: 0,
        first_seq: None,
        last_seq: None,
        head_seq: None,
        head_hash: None,
        anchor: None,
        broken: None,
        pending: count(db, &store::control::AUDIT_PENDING_COUNT, ledger).await?,
        before_chain: if ledger == INSTANCE { count(db, &store::control::AUDIT_LEGACY_COUNT, ledger).await? } else { 0 },
        detail_unchecked: 0,
        checked_at: ts(&Utc::now()),
    };
    let fail = |mut v: AuditVerification, b: AuditChainBreak| {
        v.ok = false;
        v.broken = Some(b);
        Ok(v)
    };
    // Checkpoints first, then the head: a checkpoint is a copy of an earlier head, so every one read
    // here is at or below the head read next, however busy the ledger is. If the oldest stored event
    // then has no checkpoint under it, a retention cut may have committed between the two reads:
    // read once more before calling it a break.
    let (mut cps, mut first, mut head) = verify_reads(db, ledger).await?;
    if let Some(f) = first
        && anchor_for(&cps, f - 1).is_none()
    {
        (cps, first, head) = verify_reads(db, ledger).await?;
    }
    let Some(head) = head else {
        if first.is_some() || !cps.is_empty() {
            return fail(v, brk(None, "head", "This log has events or checkpoints but no head: the record of its newest event was deleted.".into()));
        }
        return Ok(v);
    };
    v.head_seq = Some(head.seq);
    v.head_hash = Some(head.hash.clone());

    for c in cps.iter().filter(|c| c.kind == "genesis") {
        let at = crate::sources::base::datetime_to_chrono(&c.at).map(|t| ts(&t)).unwrap_or_default();
        if c.seq != 0 || genesis_hash(ledger, &at) != c.hash {
            return fail(v, brk(Some(0), "genesis", "The chain's starting point was changed.".into()));
        }
    }
    if !cps.iter().any(|c| c.kind == "genesis") {
        return fail(v, brk(Some(0), "genesis", "The chain's starting point is missing.".into()));
    }
    // a checkpoint past the head means the newest events were deleted and the head wound back
    if let Some(c) = cps.iter().find(|c| c.seq > head.seq) {
        return fail(v, brk(Some(head.seq + 1), "checkpoint", format!("A checkpoint records event {}, but the log ends at event {}: the newest events are missing.", c.seq, head.seq)));
    }
    let Some(first) = first else {
        // nothing stored: every event was cut by retention, or the newest were deleted
        return match anchor_for(&cps, head.seq) {
            Some(a) if a.hash == head.hash => {
                v.anchor = Some(a.into());
                Ok(v)
            }
            _ if head.seq == 0 => Ok(v),
            _ => fail(v, brk(Some(head.seq), "missing", format!("Events up to {} are missing and no retention checkpoint accounts for them.", head.seq))),
        };
    };
    v.first_seq = Some(first);
    let Some(anchor) = anchor_for(&cps, first - 1) else {
        let msg = if first == 2 { "Event 1 is missing and no retention checkpoint accounts for it.".to_string() } else { format!("Events 1 to {} are missing and no retention checkpoint accounts for them.", first - 1) };
        return fail(v, brk(Some(first - 1), "missing", msg));
    };
    v.anchor = Some(anchor.into());
    let by_seq: BTreeMap<i64, Vec<&CheckpointRow>> = cps.iter().fold(BTreeMap::new(), |mut m, c| {
        m.entry(c.seq).or_default().push(c);
        m
    });

    // the walk stops at the head read above; events sealed since are newer, not errors
    let mut walk = Walk::new(anchor.seq, anchor.hash.clone());
    let mut after = anchor.seq;
    'pages: loop {
        let rows = page(db, ledger, after, head.seq, PAGE).await?;
        let Some(last) = rows.last() else { break };
        after = last.seq.unwrap_or(after);
        for r in &rows {
            if !walk.step(&r.stored(), r.hash.as_deref().unwrap_or_default()) {
                break 'pages;
            }
            match r.detail_matches() {
                Some(false) => {
                    walk.broken = Some(brk(Some(walk.last_seq()), "altered", format!("The detail of event {} was changed after it was written.", walk.last_seq())));
                    break 'pages;
                }
                None => v.detail_unchecked += 1,
                Some(true) => {}
            }
            if let Some(cs) = by_seq.get(&walk.last_seq())
                && cs.iter().any(|c| c.hash != walk.prev_hash)
            {
                walk.broken = Some(brk(Some(walk.last_seq()), "checkpoint", format!("A checkpoint disagrees with event {}.", walk.last_seq())));
                break 'pages;
            }
        }
        if (rows.len() as i64) < PAGE {
            break;
        }
    }
    v.checked = walk.checked;
    v.last_seq = Some(walk.last_seq());
    if let Some(b) = walk.broken {
        return fail(v, b);
    }
    if walk.last_seq() < head.seq {
        let msg = if walk.last_seq() + 1 == head.seq {
            format!("The newest event, {}, is missing.", head.seq)
        } else {
            format!("The newest events, {} to {}, are missing.", walk.last_seq() + 1, head.seq)
        };
        return fail(v, brk(Some(walk.last_seq() + 1), "missing", msg));
    }
    if walk.prev_hash != head.hash {
        return fail(v, brk(Some(head.seq), "head", "The newest event does not match the recorded head.".into()));
    }
    // Events above the head read at the start: fine if the head has moved past them since and the
    // first of them follows the last event checked; a break if the head did not move (or moved back).
    let beyond = page(db, ledger, head.seq, i64::MAX, 1).await?;
    if let Some(r) = beyond.first() {
        let now = read_head(db, ledger).await?;
        let seq = r.seq.unwrap_or_default();
        let moved_on = now.as_ref().is_some_and(|h| h.seq >= seq);
        let follows = seq == head.seq + 1 && r.prev_hash.as_deref() == Some(head.hash.as_str());
        if !(moved_on && follows) {
            return fail(v, brk(Some(seq), "head", format!("Event {seq} is newer than the recorded head.")));
        }
    }
    Ok(v)
}

// ---------------------------------------------------------------- export

/// One event in an export: its canonical fields (the detail only as its hash) and stored hash.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AuditExportEvent {
    #[serde(flatten)]
    pub fields: AuditEventFields,
    pub hash: String,
}

/// An offline-verifiable slice of one ledger. Metadata only: no memory text, chat content,
/// prompts, secrets or free-text detail (the detail is committed by its hash).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AuditExport {
    /// `eunomia-audit-export`.
    pub format: String,
    /// The canonical form the hashes use (`eunomia.audit.v1`).
    pub version: String,
    pub generated_at: String,
    pub ledger: String,
    /// The requested range, RFC 3339; empty when open.
    pub from: String,
    pub to: String,
    /// The point the first event hangs from: its sequence minus one and its `prev_hash`.
    pub anchor: Option<AuditCheckpoint>,
    pub events: Vec<AuditExportEvent>,
    /// Stored checkpoints inside the range, plus one every 1000 events and one at the last event.
    pub checkpoints: Vec<AuditCheckpoint>,
    /// The ledger's head when the export was made.
    pub head: Option<AuditCheckpoint>,
    /// The file's own range: its first and last event, the ledger's head when it was made, and how
    /// many events were still unsealed (written, not yet in the chain) and so are not in it. The
    /// offline verifier checks it against the events and the head.
    pub range: AuditExportRange,
    /// True when the range held more than one export carries; `next_seq` is where to resume.
    pub truncated: bool,
    pub next_seq: Option<i64>,
    /// The server's own check of the events in this file.
    pub ok: bool,
    #[serde(rename = "break")]
    pub broken: Option<AuditChainBreak>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AuditExportRange {
    pub first_seq: Option<i64>,
    pub last_seq: Option<i64>,
    pub head_seq: Option<i64>,
    pub head_hash: Option<String>,
    pub unsealed: i64,
}

/// Exports the sealed events of `ledger` written in `[from, to)` (open ends allowed), as one
/// contiguous run of sequence numbers.
pub async fn export(db: &ControlDb, ledger: &str, from: Option<DateTime<Utc>>, to: Option<DateTime<Utc>>) -> AppResult<AuditExport> {
    if let (Some(f), Some(t)) = (from, to)
        && f >= t
    {
        return Err(AppError::bad_request("The start of the range must be before its end."));
    }
    if let Err(e) = seal(db, ledger).await {
        tracing::warn!(ledger = %ledger, error = %e.message, "audit seal before export failed");
    }
    let head = read_head(db, ledger).await?;
    let cps = checkpoints(db, ledger).await?;
    let mut out = AuditExport {
        format: "eunomia-audit-export".into(),
        version: VERSION.into(),
        generated_at: ts(&Utc::now()),
        ledger: ledger.to_string(),
        from: from.map(|t| ts(&t)).unwrap_or_default(),
        to: to.map(|t| ts(&t)).unwrap_or_default(),
        anchor: None,
        events: Vec::new(),
        checkpoints: Vec::new(),
        head: head.as_ref().map(|h| AuditCheckpoint {
            seq: h.seq,
            hash: h.hash.clone(),
            at: h.updated_at.as_ref().and_then(crate::sources::base::datetime_to_chrono).map(|t| ts(&t)).unwrap_or_default(),
            kind: "head".into(),
            pruned: None,
        }),
        range: AuditExportRange::default(),
        truncated: false,
        next_seq: None,
        ok: true,
        broken: None,
    };
    out.range.unsealed = count(db, &store::control::AUDIT_PENDING_COUNT, ledger).await?;
    if let Some(h) = &head {
        out.range.head_seq = Some(h.seq);
        out.range.head_hash = Some(h.hash.clone());
    }
    let Some(head) = head else {
        // no head: an empty file is only honest when the ledger is really empty
        let any: Vec<SeqHash> = store::control::AUDIT_FIRST_SEQ.on(db).bind(("ledger", ledger.to_string())).await?.take(0)?;
        if !any.is_empty() || !cps.is_empty() {
            out.ok = false;
            out.broken = Some(brk(None, "head", "This log has events or checkpoints but no head: the record of its newest event was deleted. Nothing was exported.".into()));
        }
        return Ok(out);
    };
    let first: Vec<SeqHash> = match from {
        Some(f) => store::control::AUDIT_FIRST_SEQ_SINCE.on(db).bind(("ledger", ledger.to_string())).bind(("from", Datetime::from(f))).await?.take(0)?,
        None => store::control::AUDIT_FIRST_SEQ.on(db).bind(("ledger", ledger.to_string())).await?.take(0)?,
    };
    let last: i64 = match to {
        Some(t) => {
            let rows: Vec<SeqHash> = store::control::AUDIT_LAST_SEQ_BEFORE.on(db).bind(("ledger", ledger.to_string())).bind(("to", Datetime::from(t))).await?.take(0)?;
            match rows.first() {
                Some(r) => r.seq,
                None => return Ok(out),
            }
        }
        None => head.seq,
    };
    let Some(first) = first.first().map(|r| r.seq) else { return Ok(out) };
    if first > last {
        return Ok(out);
    }
    let until = last.min(first - 1 + EXPORT_MAX);
    if until < last {
        out.truncated = true;
        out.next_seq = Some(until + 1);
    }
    let mut after = first - 1;
    let mut rows: Vec<Row> = Vec::new();
    while after < until {
        let p = page(db, ledger, after, until, PAGE).await?;
        let Some(l) = p.last() else { break };
        after = l.seq.unwrap_or(until);
        let short = (p.len() as i64) < PAGE;
        rows.extend(p);
        if short {
            break;
        }
    }
    let Some(first_row) = rows.first() else { return Ok(out) };
    let anchor_hash = first_row.prev_hash.clone().unwrap_or_default();
    let anchor_kind = anchor_for(&cps, first - 1).map(|c| c.kind.clone()).unwrap_or_else(|| "export".into());
    let anchor_at = anchor_for(&cps, first - 1).map(|c| AuditCheckpoint::from(c).at).unwrap_or_default();
    out.anchor = Some(AuditCheckpoint { seq: first - 1, hash: anchor_hash.clone(), at: anchor_at, kind: anchor_kind, pruned: None });
    let mut walk = Walk::new(first - 1, anchor_hash);
    let mut n = 0i64;
    for r in &rows {
        let c = r.stored();
        let hash = r.hash.clone().unwrap_or_default();
        walk.step(&c, &hash);
        n += 1;
        for cp in cps.iter().filter(|cp| cp.seq == c.seq) {
            out.checkpoints.push(cp.into());
        }
        if n % EXPORT_CHECKPOINT_EVERY == 0 || n as usize == rows.len() {
            out.checkpoints.push(AuditCheckpoint { seq: c.seq, hash: hash.clone(), at: c.created_at.clone(), kind: "export".into(), pruned: None });
        }
        out.events.push(AuditExportEvent { fields: c, hash });
    }
    if let Some(cp) = cps.iter().find(|cp| cp.seq == first - 1 && cp.kind != "periodic") {
        out.checkpoints.insert(0, cp.into());
    }
    out.range.first_seq = out.events.first().map(|e| e.fields.seq);
    out.range.last_seq = out.events.last().map(|e| e.fields.seq);
    out.ok = walk.broken.is_none();
    out.broken = walk.broken;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(seq: i64, prev: &str) -> AuditEventFields {
        AuditEventFields {
            ledger: "org:abc".into(),
            seq,
            prev_hash: prev.into(),
            id: format!("audit_event:e{seq}"),
            created_at: "2026-11-13T01:02:03.000004Z".into(),
            user: Some("user:u1".into()),
            actor_kind: "user".into(),
            actor_id: "user:u1".into(),
            action: "vault.create".into(),
            target: "vault:v1".into(),
            outcome: "ok".into(),
            trace_id: "t".into(),
            detail_digest: sha256_hex(""),
        }
    }

    fn chain(n: i64) -> Vec<(AuditEventFields, String)> {
        let mut prev = genesis_hash("org:abc", "2026-11-13T00:00:00.000000Z");
        (1..=n)
            .map(|s| {
                let c = ev(s, &prev);
                let h = event_hash(&c);
                prev = h.clone();
                (c, h)
            })
            .collect()
    }

    #[test]
    fn canonical_form_is_pinned() {
        // Changing this string breaks every chain already written: bump VERSION instead.
        let c = ev(7, "p");
        assert_eq!(
            canonical(&c),
            format!(
                r#"["eunomia.audit.v1","org:abc",7,"p","audit_event:e7","2026-11-13T01:02:03.000004Z","user:u1","user","user:u1","vault.create","vault:v1","ok","t","{}"]"#,
                sha256_hex("")
            )
        );
        let mut none = c.clone();
        none.user = None;
        none.target = "q\"\n\u{1}é".into();
        assert!(!canonical(&none).contains(r#","user:u1","user","user:u1""#));
        assert!(canonical(&none).contains(r#"null,"user""#));
        assert!(canonical(&none).contains(r#""q\"\n\u0001é""#), "{}", canonical(&none));
    }

    #[test]
    fn an_erased_detail_is_unchecked_and_any_other_change_is_altered() {
        let row = |detail: &str| Row {
            id: RecordId::new("audit_event", "e"),
            ledger: None,
            seq: Some(1),
            prev_hash: None,
            hash: None,
            created_at: Datetime::from(Utc::now()),
            user: None,
            actor_kind: String::new(),
            actor_id: String::new(),
            action: String::new(),
            target: String::new(),
            outcome: String::new(),
            trace_id: String::new(),
            detail: detail.into(),
            detail_digest: Some(digest("email=alice@example.com")),
            digest_key: Some(detail_key_id().to_string()),
        };
        assert_eq!(row("email=alice@example.com").detail_matches(), Some(true));
        assert_eq!(row(ERASED_DETAIL).detail_matches(), None, "erased: cannot be checked, not tampering");
        assert_eq!(row("email=[erased person]").detail_matches(), Some(false), "rewritten in place is still a change");
        let erased = row(ERASED_DETAIL);
        assert_eq!(erased.stored().detail_digest, row("email=alice@example.com").stored().detail_digest, "the hashed digest is the one written");
    }

    #[test]
    fn time_has_six_fraction_digits() {
        let t = DateTime::parse_from_rfc3339("2026-01-02T03:04:05.1Z").unwrap().with_timezone(&Utc);
        assert_eq!(ts(&t), "2026-01-02T03:04:05.100000Z");
    }

    #[test]
    fn an_intact_chain_walks_to_the_end() {
        let c = chain(5);
        let mut w = Walk::new(0, c[0].0.prev_hash.clone());
        for (e, h) in &c {
            assert!(w.step(e, h));
        }
        assert_eq!((w.checked, w.last_seq()), (5, 5));
    }

    #[test]
    fn each_kind_of_tampering_is_named() {
        let c = chain(5);
        let start = c[0].0.prev_hash.clone();
        let run = |events: Vec<(AuditEventFields, String)>| {
            let mut w = Walk::new(0, start.clone());
            for (e, h) in &events {
                if !w.step(e, h) {
                    break;
                }
            }
            w.broken.map(|b| (b.kind, b.seq))
        };
        let mut edited = c.clone();
        edited[2].0.outcome = "denied".into();
        assert_eq!(run(edited), Some(("altered".into(), Some(3))));
        let mut gap = c.clone();
        gap.remove(1);
        assert_eq!(run(gap), Some(("missing".into(), Some(2))));
        let mut dup = c.clone();
        dup.insert(2, c[1].clone());
        assert_eq!(run(dup), Some(("duplicate".into(), Some(2))));
        let mut relinked = c.clone();
        relinked[3].0.prev_hash = "x".into();
        relinked[3].1 = event_hash(&relinked[3].0);
        assert_eq!(run(relinked), Some(("link".into(), Some(4))));
        let mut detail = c;
        detail[4].0.detail_digest = sha256_hex("changed");
        assert_eq!(run(detail), Some(("altered".into(), Some(5))));
    }
}
