//! The one-off fact-date backfill. Facts extracted from synced records now carry the record's date
//! (`occurred_at`, see `entities::extract::fact_date`), but facts extracted before that shipped were
//! dated by when their extraction job ran. This walks an org's memories in id order, a batch at a time,
//! and gives each such fact its record's date. Progress lives in `fact_dates_backfill:v1`: a crashed
//! run resumes at its cursor and a finished one is never repeated.
//!
//! A fact is re-dated only when all of these hold:
//! - it is a raw fact (`world` or `experience`; an observation is a summary, not an event);
//! - it has no `occurred_at` (a date its writer gave, or one set since, is never overwritten);
//! - it still has a source record, and that record has a usable date (same rule as extraction);
//! - it was written before the walk first started (`cutoff`), so only facts from before the fix.
//!
//! Facts with no source record (written by a person or an agent, copied from a peer, a relation's
//! reason) are never touched, nor is any fact whose writer gave an `occurred_at`. A `memory_write` that
//! cited a source record but gave no date is re-dated like an extracted fact: a fact citing a record is
//! about that record.
//!
//! Reversible and auditable: each re-date is a compare-and-set (still undated, still the same source)
//! written in one transaction with a `fact_dates_redated` ledger row (keyed by the memory) holding the
//! previous date and the one set. A fact with a ledger row is never re-dated by any later run, so a date
//! a person changed afterwards stands. [`undo`] restores the previous dates where the backfill's date is
//! still in place. Each re-date also gets an `updated` row in its vault's change log, so a client
//! following the vault fetches the new date.
//!
//! Supersession is not re-run for re-dated facts. The run counts (`suspect_superseded`) the re-dated
//! facts that are marked superseded while a live raw fact of the same subject is now dated before them:
//! the replacements that may have gone the wrong way. It is an upper bound (which fact replaced which is
//! not stored), and those rows are left as they are.

use serde::Deserialize;
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use crate::audit::{self, Event};
use crate::authz::Actor;
use crate::changes;
use crate::error::AppResult;
use crate::rid::RecordIdExt;
use crate::sources::base::datetime_to_chrono;
use crate::state::OrgState;
use crate::store::fact_dates as q;
use crate::tx::with_retry;

/// Rows read per batch. Each re-date is its own statement: no long transaction, no table lock.
const BATCH: i64 = 200;

/// The marker's key. A later rule for dating facts that should run again takes a new version.
pub const VERSION: u32 = 1;

/// Where one org's backfill stands (the `fact_dates_backfill` row).
#[derive(Debug, Clone, Deserialize, SurrealValue)]
pub struct Progress {
    pub cursor: Option<RecordId>,
    pub cutoff: Option<Datetime>,
    pub changed: i64,
    #[serde(default)]
    #[surreal(default)]
    pub suspect_superseded: i64,
    pub done_at: Option<Datetime>,
}

pub fn marker_id() -> RecordId {
    RecordId::from_table_key("fact_dates_backfill", format!("v{VERSION}"))
}

async fn load(state: &OrgState) -> AppResult<Option<Progress>> {
    Ok(q::MARKER.on(&state.db).bind(("id", marker_id())).await?.check()?.take(0)?)
}

/// Whether this org's facts have been dated. Errors on a database still at the schema before the marker table.
pub async fn done(state: &OrgState) -> AppResult<bool> {
    Ok(load(state).await?.is_some_and(|p| p.done_at.is_some()))
}

async fn save(state: &OrgState, p: &Progress) -> AppResult<()> {
    q::SAVE_MARKER
        .on(&state.db)
        .bind(("id", marker_id()))
        .bind(("cursor", p.cursor.clone()))
        .bind(("cutoff", p.cutoff))
        .bind(("changed", p.changed))
        .bind(("suspect", p.suspect_superseded))
        .bind(("done_at", p.done_at))
        .await?
        .check()?;
    Ok(())
}

/// Runs the backfill from where it stopped, at most `max_batches` batches (`None`: to the end).
///
/// The count is saved after each batch, so a crash between a batch's re-dates and its save undercounts:
/// the resumed run finds those facts already dated. The dates are right either way.
pub async fn run(state: &OrgState, max_batches: Option<usize>) -> AppResult<Progress> {
    let mut p = match load(state).await? {
        Some(p) => p,
        None => {
            // the cutoff is fixed before the first batch and saved with it: a resumed run keeps it
            let p = Progress {
                cursor: None,
                cutoff: Some(Datetime::from(chrono::Utc::now())),
                changed: 0,
                suspect_superseded: 0,
                done_at: None,
            };
            save(state, &p).await?;
            p
        }
    };
    let cutoff = p.cutoff.as_ref().and_then(datetime_to_chrono).unwrap_or_else(chrono::Utc::now);
    let mut batches = 0;
    while p.done_at.is_none() && max_batches.is_none_or(|m| batches < m) {
        let (last, changed, suspect) = batch(state, &p.cursor, cutoff).await?;
        p.changed += changed;
        p.suspect_superseded += suspect;
        p.cursor = last;
        if p.cursor.is_none() {
            p.done_at = Some(Datetime::from(chrono::Utc::now()));
            // before the marker says done: a crash between the two repeats the event, never loses it
            report(state, &p).await;
        }
        save(state, &p).await?;
        batches += 1;
    }
    Ok(p)
}

/// One log line and one `audit_event` per org when it finishes: a count only.
async fn report(state: &OrgState, p: &Progress) {
    let org = state.db.org().key();
    tracing::info!(org = %org, version = VERSION, memory = p.changed, suspect_superseded = p.suspect_superseded, "fact date backfill finished");
    let detail = format!("version={VERSION} memory={} suspect_superseded={}", p.changed, p.suspect_superseded);
    let actor = Actor { kind: "system", id: "system".into() };
    audit::record(&state.control, Event { user: None, actor: &actor, action: "facts.backfill_dates", target: &org, outcome: "ok", detail: &detail }).await;
}

#[derive(Debug, Deserialize, SurrealValue)]
struct Row {
    id: RecordId,
    vault: RecordId,
    subject: RecordId,
    #[serde(default)]
    #[surreal(default)]
    status: Option<String>,
    #[serde(rename = "type")]
    #[surreal(rename = "type")]
    kind: String,
    source: Option<RecordId>,
    occurred_at: Option<Datetime>,
    created_at: Option<Datetime>,
    source_at: Option<Datetime>,
}

/// The date `row` should get, if it is one the backfill re-dates (see the module docs).
fn new_date(row: &Row, cutoff: chrono::DateTime<chrono::Utc>) -> Option<Datetime> {
    if row.occurred_at.is_some() || row.source.is_none() || !matches!(row.kind.as_str(), "world" | "experience") {
        return None;
    }
    let written = row.created_at.as_ref().and_then(datetime_to_chrono)?;
    if written >= cutoff {
        return None;
    }
    let at = datetime_to_chrono(row.source_at.as_ref()?)?;
    crate::entities::extract::fact_date(Some(&at.to_rfc3339()))
}

/// One batch's re-dated memories, by vault, as `updated` rows for the change log (migration 0023).
#[derive(Default)]
struct Logged(Vec<(RecordId, Vec<ChangeRow>)>);

/// (op, kind, item), as [`changes::record_many`] takes them.
type ChangeRow = (&'static str, &'static str, RecordId);

impl Logged {
    fn add(&mut self, vault: &RecordId, memory: &RecordId) {
        let entry = (changes::UPDATED, changes::MEMORY, memory.clone());
        match self.0.iter_mut().find(|(v, _)| v == vault) {
            Some((_, items)) => items.push(entry),
            None => self.0.push((vault.clone(), vec![entry])),
        }
    }

    /// As the `system` actor (a job has no caller). Never fails the batch: the dates already committed.
    async fn record(self, state: &OrgState) {
        for (vault, items) in self.0 {
            changes::record_many(&state.db, &vault, items).await;
        }
    }
}

/// The ledger row of `memory`: the same key in `fact_dates_redated`.
fn ledger_id(memory: &RecordId) -> RecordId {
    RecordId::from_table_key("fact_dates_redated", memory.key().clone())
}

/// (last id read, facts re-dated, re-dated facts whose replacement may be wrong); no last id once the
/// table is exhausted.
async fn batch(state: &OrgState, cursor: &Option<RecordId>, cutoff: chrono::DateTime<chrono::Utc>) -> AppResult<(Option<RecordId>, i64, i64)> {
    let stmt = if cursor.is_some() { &q::MEMORY_AFTER } else { &q::MEMORY_FIRST };
    let rows: Vec<Row> = stmt.on(&state.db).bind(("cursor", cursor.clone())).bind(("limit", BATCH)).await?.check()?.take(0)?;
    let (mut changed, mut suspect) = (0, 0);
    let mut logged = Logged::default();
    for row in &rows {
        let Some(at) = new_date(row, cutoff) else { continue };
        let set = || async {
            q::REDATE
                .on(&state.db)
                .bind(("id", row.id.clone()))
                .bind(("ledger", ledger_id(&row.id)))
                .bind(("at", at))
                .bind(("source", row.source.clone()))
                .bind(("version", VERSION as i64))
                .await?
                .check()
        };
        if with_retry(set).await?.take::<Vec<RecordId>>(q::REDATE.slot)?.is_empty() {
            continue;
        }
        changed += 1;
        if row.status.as_deref() == Some("superseded") {
            let mut res = q::EARLIER_LIVE.on(&state.db).bind(("subject", row.subject.clone())).bind(("id", row.id.clone())).bind(("at", at)).await?.check()?;
            if res.take::<Vec<i64>>(0)?.first().is_some_and(|n| *n > 0) {
                suspect += 1;
            }
        }
        logged.add(&row.vault, &row.id);
    }
    logged.record(state).await;
    Ok((rows.last().map(|r| r.id.clone()), changed, suspect))
}

/// Undoes the backfill's re-dates: each fact gets its previous date back where the backfill's date is
/// still in place (a date changed since stands). Ledger rows stay, marked undone, so no later run dates
/// those facts again; the marker stays done. Returns the facts restored. Not wired to a route: an
/// operator runs it (or its statements) by hand if the re-dating has to be reverted.
pub async fn undo(state: &OrgState) -> AppResult<i64> {
    #[derive(Deserialize, SurrealValue)]
    struct Entry {
        id: RecordId,
        memory: RecordId,
        previous: Option<Datetime>,
        occurred_at: Datetime,
        #[serde(default)]
        #[surreal(default)]
        vault: Option<RecordId>,
    }
    let mut restored = 0;
    loop {
        let rows: Vec<Entry> = q::LEDGER_PENDING.on(&state.db).bind(("limit", BATCH)).await?.check()?.take(0)?;
        if rows.is_empty() {
            return Ok(restored);
        }
        let mut logged = Logged::default();
        for r in &rows {
            let one = || async {
                q::UNDO
                    .on(&state.db)
                    .bind(("id", r.id.clone()))
                    .bind(("memory", r.memory.clone()))
                    .bind(("previous", r.previous))
                    .bind(("at", r.occurred_at))
                    .await?
                    .check()
            };
            if !with_retry(one).await?.take::<Vec<RecordId>>(q::UNDO.slot)?.is_empty() {
                restored += 1;
                if let Some(vault) = &r.vault {
                    logged.add(vault, &r.memory);
                }
            }
        }
        logged.record(state).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> Option<Datetime> {
        Some(Datetime::from(s.parse::<chrono::DateTime<chrono::Utc>>().unwrap()))
    }

    fn row() -> Row {
        Row {
            id: crate::rid::parse("memory:a").unwrap(),
            vault: crate::rid::parse("vault:v").unwrap(),
            subject: crate::rid::parse("person:p").unwrap(),
            status: None,
            kind: "world".into(),
            source: Some(crate::rid::parse("cache_record:r").unwrap()),
            occurred_at: None,
            created_at: dt("2026-11-01T00:00:00Z"),
            source_at: dt("2023-04-01T09:30:00Z"),
        }
    }

    #[test]
    fn only_undated_sourced_raw_facts_from_before_the_cutoff_are_redated() {
        let cutoff: chrono::DateTime<chrono::Utc> = "2026-11-13T00:00:00Z".parse().unwrap();
        let got = new_date(&row(), cutoff).map(|d| datetime_to_chrono(&d).unwrap().to_rfc3339());
        assert_eq!(got.as_deref(), Some("2023-04-01T09:30:00+00:00"));
        type Change = Box<dyn Fn(&mut Row)>;
        let cases: [(&str, Change); 6] = [
            ("dated by its writer or by hand", Box::new(|r| r.occurred_at = dt("2020-01-01T00:00:00Z"))),
            ("no source record: a person wrote it", Box::new(|r| r.source = None)),
            ("an observation", Box::new(|r| r.kind = "observation".into())),
            ("written after the fix", Box::new(|r| r.created_at = dt("2026-11-14T00:00:00Z"))),
            ("the record has no date", Box::new(|r| r.source_at = None)),
            ("the record's date is the epoch sentinel", Box::new(|r| r.source_at = dt("1970-01-01T00:00:00Z"))),
        ];
        for (why, change) in cases {
            let mut r = row();
            change(&mut r);
            assert!(new_date(&r, cutoff).is_none(), "{why}");
        }
    }
}
