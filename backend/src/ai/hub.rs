//! What this process knows about every organisation's model use: its settings, its spend this month, and the
//! counters waiting to be written. One [`Hub`] per [`crate::pool::Pool`] (so one per process, and one per app
//! state in a test), reached from an [`crate::pool::OrgDb`] as `db.ai()`.
//!
//! # Recording without a database round trip per call
//!
//! A finished model call adds to an in-memory counter keyed by (organisation, day, purpose, model, whose key).
//! A background task writes the counters every [`FLUSH_INTERVAL`] (and at once when an organisation is close to
//! its allowance, and on an orderly shutdown), one upsert per counter, not per call. **What a crash can lose is
//! the calls that finished since the last write: at most [`FLUSH_INTERVAL`] of this process's model use**, plus
//! whatever accumulated while the control database refused writes (it is kept in memory and retried, and is
//! lost only if the process dies meanwhile). A call that did not finish when the process died is not counted at
//! all. The lost amount is spend, not a risk to the cap: it makes the organisation look slightly cheaper than it
//! was for the rest of the month.
//!
//! # The allowance, and how far it can be overshot
//!
//! An organisation with an allowance admits a call on the server's key only while
//! `spend read from the database + spend not yet written + the reservations of calls in flight + this call's
//! reservation <= allowance`. A call's reservation is its worst case: the input it is sending, estimated
//! conservatively ([`super::estimate_tokens`]), plus the most output it may produce ([`super::MAX_OUTPUT_TOKENS`],
//! sent to the provider as `max_completion_tokens` on api.openai.com), at the model's price. When the call ends the
//! reservation is replaced by what it really cost. So **within one process the allowance is never overshot unless
//! a provider bills more than the reservation: the estimate of a call's input being too low is the only way, and
//! the overshoot is then that call's excess** (tested with a provider that reports more than the estimate). Other
//! replicas' unwritten spend is invisible here: **across `R` processes the allowance can be overshot by what the
//! other `R - 1` spend in one [`FLUSH_INTERVAL`] plus one [`SPEND_TTL`]**, which close to the cap shrinks, because
//! within [`NEAR_CAP_PERCENT`] of the allowance every admission first reads the database and every settled call
//! asks for an immediate write.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use super::prices::{self, Price, Priced, Table};
use super::{KeySource, Purpose};
use crate::error::{AppError, AppResult};
use crate::pool::{ControlDb, OrgId};
use crate::store::ai as q;

/// How often counters are written to the control database.
pub const FLUSH_INTERVAL: Duration = Duration::from_secs(5);
/// How long a process trusts an organisation's settings before reading them again (the same as the plan state).
pub const CONFIG_TTL: Duration = Duration::from_secs(3);
/// How long a process trusts the month's spend it read from the database.
pub const SPEND_TTL: Duration = Duration::from_secs(2);
/// Within this share of the allowance of being used up, every admission reads the database first.
pub const NEAR_CAP_PERCENT: i64 = 5;
/// How long the job claim leaves an organisation's model jobs alone after a worker paused one (see `ai::hold`).
pub const HOLD: Duration = Duration::from_secs(60);
/// The most distinct model names counted for one organisation on one day. A model past it is counted in a row named
/// `other` (everything but the name is kept, the cost included). Model names are free text a person chooses, so
/// without a bound a script could make a row per call.
pub const MAX_MODELS_PER_DAY: i64 = 20;
/// The name the overflow is counted under.
pub const OTHER_MODEL: &str = "other";
/// How many months before the current one usage rows are kept by default (`AI_USAGE_RETENTION_MONTHS`).
pub const DEFAULT_RETENTION_MONTHS: u32 = 24;
/// The fewest months before the current one that are ever pruned into: the previous month is never pruned (the
/// billing service reads it after the month turns), nor is the current one.
pub const MIN_RETENTION_MONTHS: u32 = 1;
/// Organisations kept in each cache; past it the oldest entries go.
const CAP: usize = 10_000;
/// Counters kept while the database refuses writes; past it the oldest are dropped and counted in the log.
const PENDING_CAP: usize = 50_000;

/// An organisation's AI settings as stored (`org_ai`). No row reads as [`OrgAi::default`]: AI on, no own
/// provider, no allowance.
#[derive(Debug, Clone, Deserialize, SurrealValue)]
pub struct OrgAi {
    #[serde(default = "yes")]
    #[surreal(default)]
    pub enabled: bool,
    #[serde(default)]
    #[surreal(default)]
    pub base_url: String,
    #[serde(default)]
    #[surreal(default)]
    pub api_key_enc: String,
    #[serde(default)]
    #[surreal(default)]
    pub chat_model: String,
    #[serde(default)]
    #[surreal(default)]
    pub embedding_model: String,
    #[serde(default)]
    #[surreal(default)]
    pub allowance_micros: Option<i64>,
    #[serde(default)]
    #[surreal(default)]
    pub hold_until: Option<Datetime>,
}

fn yes() -> bool {
    true
}

impl Default for OrgAi {
    fn default() -> Self {
        OrgAi { enabled: true, base_url: String::new(), api_key_enc: String::new(), chat_model: String::new(), embedding_model: String::new(), allowance_micros: None, hold_until: None }
    }
}

impl OrgAi {
    /// The organisation has set a provider of its own (a key, or an endpoint other than the server's).
    pub fn has_own_provider(&self) -> bool {
        !self.api_key_enc.is_empty() || !self.base_url.trim().is_empty()
    }

    pub fn has_key(&self) -> bool {
        !self.api_key_enc.is_empty()
    }
}

/// What a counter is keyed by.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UsageKey {
    pub org: OrgId,
    pub day: String,
    pub month: String,
    pub purpose: Purpose,
    pub model: String,
    pub source: KeySource,
}

impl UsageKey {
    fn row_id(&self) -> RecordId {
        self.row_id_for(&self.model)
    }

    /// The row this counter has under `model` (its own, or [`OTHER_MODEL`] when the day's names are used up).
    fn row_id_for(&self, model: &str) -> RecordId {
        let parts = format!("{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}", self.org.key(), self.day, self.purpose.as_str(), model, self.source.as_str());
        RecordId::new("ai_usage", crate::tx::stable_key('a', &parts))
    }
}

/// What the counter has gathered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    pub calls: i64,
    pub input: i64,
    pub output: i64,
    pub estimated: i64,
    pub cost: i64,
}

impl Counters {
    fn add(&mut self, o: &Counters) {
        self.calls += o.calls;
        self.input += o.input;
        self.output += o.output;
        self.estimated += o.estimated;
        self.cost += o.cost;
    }
}

/// One organisation's spend this month on the server's key, as this process sees it.
#[derive(Debug)]
struct Book {
    month: String,
    /// The database's total as of `refreshed`.
    db_total: Option<i64>,
    refreshed: Instant,
    /// Settled here, not yet written.
    unflushed: i64,
    /// Being written now.
    flushing: i64,
    /// Worst cases of the calls in flight.
    reserved: i64,
}

impl Book {
    fn new(month: &str) -> Self {
        Book { month: month.to_string(), db_total: None, refreshed: Instant::now(), unflushed: 0, flushing: 0, reserved: 0 }
    }

    fn committed(&self) -> i64 {
        self.db_total.unwrap_or(0) + self.unflushed + self.flushing
    }

    fn used(&self) -> i64 {
        self.committed() + self.reserved
    }
}

#[derive(Default)]
struct State {
    pending: HashMap<UsageKey, Counters>,
    books: HashMap<OrgId, Book>,
}

/// The allowance cannot cover the call.
#[derive(Debug)]
pub enum Reserve {
    Used,
    Unavailable(AppError),
}

/// Part of the allowance held for a call in flight. Dropping it without settling releases it.
pub struct Reservation {
    hub: Arc<Hub>,
    org: OrgId,
    amount: i64,
    allowance: i64,
    live: bool,
}

impl Reservation {
    pub fn amount(&self) -> i64 {
        self.amount
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.live {
            self.live = false;
            self.hub.release(self.org, self.amount);
        }
    }
}

pub struct Hub {
    control: ControlDb,
    configs: Mutex<HashMap<OrgId, (Instant, Arc<OrgAi>)>>,
    state: Mutex<State>,
    prices: Mutex<Option<(Instant, Arc<Table>)>>,
    warned: Mutex<HashSet<String>>,
    /// One flush at a time, so what is `flushing` is one batch.
    flush_gate: tokio::sync::Mutex<()>,
    wake: tokio::sync::Notify,
    started: AtomicBool,
    interval_ms: AtomicU64,
    hold_ms: AtomicU64,
    #[cfg(feature = "test-support")]
    clock: Mutex<Option<DateTime<Utc>>>,
}

impl Hub {
    pub fn new(control: ControlDb) -> Self {
        Hub {
            control,
            configs: Mutex::new(HashMap::new()),
            state: Mutex::new(State::default()),
            prices: Mutex::new(None),
            warned: Mutex::new(HashSet::new()),
            flush_gate: tokio::sync::Mutex::new(()),
            wake: tokio::sync::Notify::new(),
            started: AtomicBool::new(false),
            interval_ms: AtomicU64::new(FLUSH_INTERVAL.as_millis() as u64),
            hold_ms: AtomicU64::new(HOLD.as_millis() as u64),
            #[cfg(feature = "test-support")]
            clock: Mutex::new(None),
        }
    }

    pub fn control(&self) -> &ControlDb {
        &self.control
    }

    // ------------------------------------------------------------------ time

    /// Now, UTC. Tests can move it (`set_clock`) to cross a month boundary.
    pub fn now(&self) -> DateTime<Utc> {
        #[cfg(feature = "test-support")]
        if let Some(t) = *self.clock.lock().unwrap_or_else(|e| e.into_inner()) {
            return t;
        }
        Utc::now()
    }

    /// The calendar month (UTC) the allowance is counted in: `2026-11`.
    pub fn month(&self) -> String {
        month_of(self.now())
    }

    pub fn day(&self) -> String {
        self.now().format("%Y-%m-%d").to_string()
    }

    #[cfg(feature = "test-support")]
    pub fn set_clock(&self, at: Option<DateTime<Utc>>) {
        *self.clock.lock().unwrap_or_else(|e| e.into_inner()) = at;
    }

    /// Tests: how often the background task writes counters.
    #[cfg(feature = "test-support")]
    pub fn set_flush_interval(&self, every: Duration) {
        self.interval_ms.store(every.as_millis().max(1) as u64, Ordering::Relaxed);
        self.wake.notify_one();
    }

    /// Tests: how long the claim leaves an organisation's model jobs alone after a pause.
    #[cfg(feature = "test-support")]
    pub fn set_hold(&self, hold: Duration) {
        self.hold_ms.store(hold.as_millis().max(1) as u64, Ordering::Relaxed);
    }

    pub fn hold_for(&self) -> Duration {
        Duration::from_millis(self.hold_ms.load(Ordering::Relaxed))
    }

    // ------------------------------------------------------------------ settings

    /// The organisation's AI settings, from the cache or the database.
    pub async fn org_config(&self, org: &OrgId) -> AppResult<Arc<OrgAi>> {
        if let Some((at, c)) = self.configs.lock().unwrap_or_else(|e| e.into_inner()).get(org)
            && at.elapsed() < CONFIG_TTL
        {
            return Ok(c.clone());
        }
        let mut res = q::CONFIG_GET.on(&self.control).bind(("org", org.record())).await?;
        let fresh = Arc::new(res.take::<Vec<OrgAi>>(0)?.into_iter().next().unwrap_or_default());
        let mut map = self.configs.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() >= CAP {
            map.retain(|_, (at, _)| at.elapsed() < CONFIG_TTL);
            if map.len() >= CAP {
                map.clear();
            }
        }
        map.insert(*org, (Instant::now(), fresh.clone()));
        Ok(fresh)
    }

    /// Drops what this process knows of `org`'s settings (a change made here applies at once; another replica
    /// catches up within [`CONFIG_TTL`]).
    pub fn forget_config(&self, org: &OrgId) {
        self.configs.lock().unwrap_or_else(|e| e.into_inner()).remove(org);
    }

    // ------------------------------------------------------------------ prices

    /// The price table in force (the operator's rows over the defaults), cached for [`prices::CACHE_TTL`].
    pub async fn price_table(&self) -> Arc<Table> {
        if let Some((at, t)) = self.prices.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
            && at.elapsed() < prices::CACHE_TTL
        {
            return t.clone();
        }
        let table = match self.read_prices().await {
            Ok(t) => Arc::new(t),
            Err(e) => {
                tracing::warn!(error = %e.message, "model price table unreadable; using the last one or the defaults");
                // keep the last good table
                if let Some((_, t)) = self.prices.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                    t.clone()
                } else {
                    Arc::new(Table::default())
                }
            }
        };
        *self.prices.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), table.clone()));
        table
    }

    async fn read_prices(&self) -> AppResult<Table> {
        #[derive(Deserialize, SurrealValue)]
        struct Row {
            model: String,
            input_per_m: f64,
            output_per_m: f64,
        }
        let rows: Vec<Row> = q::PRICES_ALL.on(&self.control).await?.take(0)?;
        let overrides: BTreeMap<String, Price> = rows.into_iter().map(|r| (r.model, Price::new(r.input_per_m, r.output_per_m))).collect();
        Ok(Table::with_overrides(overrides))
    }

    pub fn forget_prices(&self) {
        *self.prices.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// `model`'s price. A model nobody priced gets the high fallback and one warning per process.
    pub async fn price(&self, model: &str) -> Priced {
        let priced = self.price_table().await.price(model);
        if !priced.known && self.warned.lock().unwrap_or_else(|e| e.into_inner()).insert(model.to_string()) {
            tracing::warn!(model, "no price for this model: counting it at the fallback price ({} / {} US$ per million tokens); set one with the operator API", priced.price.input_per_million_usd, priced.price.output_per_million_usd);
        }
        priced
    }

    // ------------------------------------------------------------------ the allowance

    async fn read_spend(&self, org: &OrgId, month: &str) -> AppResult<i64> {
        let mut res = q::SPEND_MONTH.on(&self.control).bind(("org", org.record())).bind(("month", month.to_string())).await?;
        #[derive(Deserialize, SurrealValue)]
        struct N {
            n: Option<i64>,
        }
        Ok(res.take::<Vec<N>>(0)?.into_iter().next().and_then(|r| r.n).unwrap_or(0))
    }

    /// Refreshes `org`'s book from the database when it is missing, old, or the allowance is close.
    async fn refresh(&self, org: &OrgId, allowance: i64, month: &str, force: bool) -> AppResult<()> {
        let stale = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let book = st.books.entry(*org).or_insert_with(|| Book::new(month));
            if book.month != month {
                // a new month: what was settled belongs to the last one (its rows carry their own month)
                let reserved = book.reserved;
                *book = Book::new(month);
                book.reserved = reserved;
            }
            let near = allowance - book.used() < allowance / 100 * NEAR_CAP_PERCENT;
            force || book.db_total.is_none() || book.refreshed.elapsed() >= SPEND_TTL || near
        };
        if !stale {
            return Ok(());
        }
        let n = self.read_spend(org, month).await?;
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(book) = st.books.get_mut(org)
            && book.month == month
        {
            book.db_total = Some(n);
            book.refreshed = Instant::now();
        }
        Ok(())
    }

    /// Room left on the server's key this month, as far as this process can tell. Reads the database when its
    /// view is old; not a reservation.
    pub async fn remaining(&self, org: &OrgId, allowance: i64) -> AppResult<i64> {
        let month = self.month();
        self.refresh(org, allowance, &month, false).await?;
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Ok(allowance - st.books.get(org).map_or(0, Book::used))
    }

    /// Holds `cost` micro-dollars of `org`'s allowance for a call about to be made.
    pub async fn reserve(self: &Arc<Self>, org: OrgId, allowance: i64, cost: i64) -> Result<Reservation, Reserve> {
        let month = self.month();
        self.refresh(&org, allowance, &month, false).await.map_err(Reserve::Unavailable)?;
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let book = st.books.entry(org).or_insert_with(|| Book::new(&month));
        // a call can always be admitted when nothing else is spent or in flight: a worst case above the whole
        // allowance must not make a small allowance unusable
        let amount = cost.clamp(0, allowance.max(0));
        if book.used() >= allowance || book.used() + amount > allowance {
            return Err(Reserve::Used);
        }
        book.reserved += amount;
        Ok(Reservation { hub: self.clone(), org, amount, allowance, live: true })
    }

    fn release(&self, org: OrgId, amount: i64) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(b) = st.books.get_mut(&org) {
            b.reserved = (b.reserved - amount).max(0);
        }
    }

    // ------------------------------------------------------------------ recording

    /// Counts a finished call. Returns at once: the write happens in the background.
    pub fn record(self: &Arc<Self>, org: OrgId, purpose: Purpose, model: &str, source: KeySource, used: Counters, reservation: Option<Reservation>) {
        let key = UsageKey { org, day: self.day(), month: self.month(), purpose, model: model.to_string(), source };
        let near = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(r) = &reservation
                && let Some(b) = st.books.get_mut(&org)
            {
                b.reserved = (b.reserved - r.amount).max(0);
            }
            let mut near = false;
            if source == KeySource::Server {
                let month = key.month.clone();
                let allowance = reservation.as_ref().map(|r| r.allowance);
                let b = st.books.entry(org).or_insert_with(|| Book::new(&month));
                if b.month == month {
                    b.unflushed += used.cost;
                    near = allowance.is_some_and(|a| a - b.used() < a / 100 * NEAR_CAP_PERCENT);
                }
            }
            if st.pending.len() >= PENDING_CAP && !st.pending.contains_key(&key) {
                tracing::error!(cap = PENDING_CAP, "model usage counters are piling up unwritten; dropping the oldest");
                if let Some(k) = st.pending.keys().next().cloned() {
                    st.pending.remove(&k);
                }
            }
            st.pending.entry(key).or_default().add(&used);
            near
        };
        // the reservation was already given back above
        if let Some(mut r) = reservation {
            r.live = false;
        }
        self.ensure_flusher();
        if near {
            self.wake.notify_one();
        }
    }

    fn ensure_flusher(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            self.started.store(false, Ordering::SeqCst);
            return;
        };
        let weak: Weak<Hub> = Arc::downgrade(self);
        rt.spawn(async move {
            loop {
                let Some(hub) = weak.upgrade() else { return };
                let every = Duration::from_millis(hub.interval_ms.load(Ordering::Relaxed));
                tokio::select! {
                    _ = tokio::time::sleep(every) => {}
                    _ = hub.wake.notified() => {}
                }
                if let Err(e) = hub.flush().await {
                    tracing::warn!(error = %e.message, "model usage not written; it will be retried");
                }
            }
        });
    }

    /// Writes every counter gathered so far. A counter that cannot be written is put back and retried; nothing is
    /// dropped while the process lives. Returns how many counters were written.
    pub async fn flush(&self) -> AppResult<usize> {
        let _one = self.flush_gate.lock().await;
        let batch: Vec<(UsageKey, Counters)> = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let taken: Vec<(UsageKey, Counters)> = std::mem::take(&mut st.pending).into_iter().collect();
            for (k, c) in &taken {
                if k.source == KeySource::Server
                    && let Some(b) = st.books.get_mut(&k.org)
                    && b.month == k.month
                {
                    b.unflushed -= c.cost;
                    b.flushing += c.cost;
                }
            }
            taken
        };
        if batch.is_empty() {
            return Ok(0);
        }
        // counters of different organisations and days are written together; those of one organisation and day one
        // after another, so the day's bound on model names (`MAX_MODELS_PER_DAY`) sees each write that came before
        let mut groups: HashMap<(OrgId, String), Vec<usize>> = HashMap::new();
        for (i, (k, _)) in batch.iter().enumerate() {
            groups.entry((k.org, k.day.clone())).or_default().push(i);
        }
        let batch_ref = &batch;
        let per_group = groups.into_values().map(|indexes| async move {
            let mut done = Vec::with_capacity(indexes.len());
            for i in indexes {
                done.push((i, self.write(&batch_ref[i].0, &batch_ref[i].1).await));
            }
            done
        });
        let mut slots: Vec<Option<AppResult<()>>> = batch.iter().map(|_| None).collect();
        for (i, r) in futures::future::join_all(per_group).await.into_iter().flatten() {
            slots[i] = Some(r);
        }
        let results: Vec<AppResult<()>> = slots.into_iter().map(|r| r.unwrap_or(Ok(()))).collect();
        let mut written = 0;
        let mut first_err = None;
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        for ((k, c), r) in batch.into_iter().zip(results) {
            let server = k.source == KeySource::Server;
            let book_month_matches = st.books.get(&k.org).is_some_and(|b| b.month == k.month);
            match r {
                Ok(()) => {
                    written += 1;
                    if server && book_month_matches && let Some(b) = st.books.get_mut(&k.org) {
                        b.flushing -= c.cost;
                        if let Some(t) = b.db_total.as_mut() {
                            *t += c.cost;
                        }
                    }
                }
                Err(e) => {
                    if server && book_month_matches && let Some(b) = st.books.get_mut(&k.org) {
                        b.flushing -= c.cost;
                        b.unflushed += c.cost;
                    }
                    st.pending.entry(k).or_default().add(&c);
                    first_err.get_or_insert(e);
                }
            }
        }
        match first_err {
            None => Ok(written),
            Some(e) => Err(e),
        }
    }

    async fn write(&self, key: &UsageKey, c: &Counters) -> AppResult<()> {
        let id = key.row_id();
        let other = key.row_id_for(OTHER_MODEL);
        crate::tx::with_retry(|| async {
            q::USAGE_ADD
                .on(&self.control)
                .bind(("id", id.clone()))
                .bind(("other", other.clone()))
                .bind(("max", MAX_MODELS_PER_DAY))
                .bind(("org", key.org.record()))
                .bind(("day", key.day.clone()))
                .bind(("month", key.month.clone()))
                .bind(("purpose", key.purpose.as_str().to_string()))
                .bind(("model", key.model.clone()))
                .bind(("source", key.source.as_str().to_string()))
                .bind(("calls", c.calls))
                .bind(("input", c.input))
                .bind(("output", c.output))
                .bind(("estimated", c.estimated))
                .bind(("cost", c.cost))
                .await?
                .check()
        })
        .await?;
        Ok(())
    }

    /// What this process has counted and not yet written (tests and the shutdown log).
    pub fn unwritten(&self) -> usize {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).pending.len()
    }

    /// `org`'s spend on the server's key in `month`: the database plus what this process has not written yet.
    pub async fn month_spend(&self, org: &OrgId, month: &str) -> AppResult<i64> {
        let db = self.read_spend(org, month).await?;
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let local = st.books.get(org).filter(|b| b.month == month).map_or(0, |b| b.unflushed + b.flushing);
        // `flushing` may already be in the database read above; counting it twice only ever errs high, and
        // only while a write is in flight
        Ok(db + local)
    }

    // ------------------------------------------------------------------ pausing background work

    /// A worker paused a job of `org` for lack of allowance: the claim leaves its model jobs alone for a while.
    pub async fn mark_hold(&self, org: &OrgId) {
        let hold = format!("{}ms", self.hold_for().as_millis());
        let id = RecordId::new("org_ai", org.key());
        if let Err(e) = q::HOLD_SET.on(&self.control).bind(("id", id)).bind(("org", org.record())).bind(("hold", hold)).await.and_then(|r| r.check()) {
            tracing::warn!(error = %e, "could not mark the organisation's model jobs as held");
        }
        self.forget_config(org);
    }

    /// The reason to hold has gone (the allowance was raised, a key was added, AI was switched on).
    pub async fn clear_hold(&self, org: &OrgId) {
        if let Err(e) = q::HOLD_CLEAR.on(&self.control).bind(("org", org.record())).await.and_then(|r| r.check()) {
            tracing::warn!(error = %e, "could not clear the organisation's model job hold");
        }
        self.forget_config(org);
    }
}

impl Hub {
    // ------------------------------------------------------------------ retention

    /// Deletes the usage rows of months older than `months` before the current one (`months` is raised to
    /// [`MIN_RETENTION_MONTHS`]; `0` keeps everything). The current month and the one before it are never touched:
    /// the allowance reads the current month, and the billing service reads the last one after the month turns.
    /// Returns the month rows before which were deleted, or `None` when nothing was asked to go.
    pub async fn prune_usage(&self, months: u32) -> AppResult<Option<String>> {
        if months == 0 {
            return Ok(None);
        }
        let before = retention_cutoff(&self.month(), months.max(MIN_RETENTION_MONTHS));
        q::USAGE_PRUNE.on(&self.control).bind(("before", before.clone())).await?.check()?;
        Ok(Some(before))
    }

    /// [`Self::prune_usage`] for `AI_USAGE_RETENTION_MONTHS` (default [`DEFAULT_RETENTION_MONTHS`]).
    pub async fn prune_usage_default(&self) -> AppResult<Option<String>> {
        let months = std::env::var("AI_USAGE_RETENTION_MONTHS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(DEFAULT_RETENTION_MONTHS);
        self.prune_usage(months).await
    }
}

/// The month `months` before `current` (`2026-11`, 24 -> `2024-11`): rows of an earlier month are past retention.
pub fn retention_cutoff(current: &str, months: u32) -> String {
    let (y, m) = current.split_once('-').and_then(|(y, m)| Some((y.parse::<i64>().ok()?, m.parse::<i64>().ok()?))).unwrap_or((1970, 1));
    let index = y * 12 + (m - 1) - i64::from(months);
    format!("{:04}-{:02}", index.div_euclid(12), index.rem_euclid(12) + 1)
}

/// The calendar month a moment falls in, UTC: `2026-11`.
pub fn month_of(at: DateTime<Utc>) -> String {
    at.format("%Y-%m").to_string()
}
