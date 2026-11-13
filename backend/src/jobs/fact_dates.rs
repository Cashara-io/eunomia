//! The one-off fact-date backfill. Facts extracted from synced records now carry the record's date
//! twice: when the fact happened (`occurred_at`) and when we were told it (`asserted_at`, the record's
//! date but never later than the fact's write; docs/concepts.md, "A fact's two dates"). Facts extracted
//! before that shipped have neither, so supersession placed them at the time their job ran. This walks
//! an org's memories by record-id range, a batch at a time, and dates each such fact by its record.
//! Progress lives in `fact_dates_backfill:v1`: a crashed run resumes at its stage and cursor, and a
//! finished one is never repeated.
//!
//! A fact is re-dated only when all of these hold:
//! - it is a raw fact (`world` or `experience`; an observation is a summary, not an event);
//! - it has neither date (a date its writer gave, or one an edit set, is never overwritten);
//! - nobody has edited it since it was written (`version` 1, `updated_at` at its write);
//! - it still has a source record, and that record has a usable date (same rule as extraction);
//! - it was written before the walk first started (`cutoff`), so only facts from before the fix.
//!
//! The dates it sets are never later than the fact's own write, except a future `occurred_at` from a
//! record dated by a fixed moment (a calendar event's start): that is when it happens, as extraction
//! sets it. When the record's date is after the fact's write, the record has been edited since (Notion,
//! GitHub, Linear, Todoist and Ed date a record by its last edit): both dates then come from the
//! record's creation date where the connector keeps one, else the fact's write. Otherwise both are the
//! record's date. See [`dates_for`].
//!
//! Facts with no source record (written by a person or an agent, copied from a peer, a relation's
//! reason) are never touched. A `memory_write` that cited a source record but gave no date is re-dated
//! like an extracted fact: a fact citing a record is about that record.
//!
//! Reversible and auditable: each re-date is a compare-and-set (still undated, unedited, same source)
//! written in one transaction with a `fact_dates_redated` ledger row (keyed by the memory) holding the
//! previous dates and the ones set. A fact with a ledger row is never re-dated by any later run, so a
//! date a person changed afterwards stands. [`undo`] restores the previous dates where the backfill's
//! are still in place. Each re-date also gets an `updated` row in its vault's change log, for clients
//! following the vault on this instance; peers do not receive dates (their content has no
//! `occurred_at`).
//!
//! Supersession is not re-run and no replacement is undone. Once every fact is re-dated, a second pass
//! over the ledger flags the replacements that may have gone the wrong way under the old job-run dates,
//! in both directions, and leaves the rows as they are:
//! - `replaced`: the re-dated fact is superseded while a live raw fact of its subject is now told before
//!   it (that fact may be what replaced it);
//! - `replacer`: the re-dated fact is live while a superseded raw fact of its subject is now told after
//!   it (this fact may be what replaced that one).
//!
//! Each flag means "a pair on this subject is now in the wrong order", not "this replacement was wrong":
//! which fact replaced which was not recorded before this release, so the pair may be unrelated facts.
//! A wrong replacement between a re-dated fact and a fact that is still live is always flagged; one
//! whose replacing fact was itself replaced later is not (nothing live is left to compare with).

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

type Utc = chrono::DateTime<chrono::Utc>;

/// Rows read per batch. Each re-date is its own statement: no long transaction, no table lock.
const BATCH: i64 = 200;

/// The marker's key. A later rule for dating facts that should run again takes a new version.
pub const VERSION: u32 = 1;

/// Connectors that date a record by its last edit, so its stored date can be after a fact drawn from it.
const EDIT_DATED: &[&str] = &["notion", "github", "linear", "todoist", "ed"];

/// Where one org's backfill stands (the `fact_dates_backfill` row).
#[derive(Debug, Clone, Deserialize, SurrealValue)]
pub struct Progress {
    /// `redate` (walking `memory`), `suspects` (walking the ledger), then `done`.
    pub stage: String,
    pub cursor: Option<RecordId>,
    pub cutoff: Option<Datetime>,
    pub changed: i64,
    pub suspect_replaced: i64,
    pub suspect_replacer: i64,
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
        .bind(("stage", p.stage.clone()))
        .bind(("cursor", p.cursor.clone()))
        .bind(("cutoff", p.cutoff))
        .bind(("changed", p.changed))
        .bind(("suspect_replaced", p.suspect_replaced))
        .bind(("suspect_replacer", p.suspect_replacer))
        .bind(("done_at", p.done_at))
        .await?
        .check()?;
    Ok(())
}

/// Runs the backfill from where it stopped, at most `max_batches` batches (`None`: to the end).
///
/// Counts are saved after each batch, so a crash between a batch's writes and its save undercounts: the
/// resumed run finds those facts already dated (or flagged). The dates are right either way.
pub async fn run(state: &OrgState, max_batches: Option<usize>) -> AppResult<Progress> {
    let mut p = match load(state).await? {
        Some(p) => p,
        None => {
            // the cutoff is fixed before the first batch and saved with it: a resumed run keeps it
            let p = Progress {
                stage: "redate".into(),
                cursor: None,
                cutoff: Some(Datetime::from(chrono::Utc::now())),
                changed: 0,
                suspect_replaced: 0,
                suspect_replacer: 0,
                done_at: None,
            };
            save(state, &p).await?;
            p
        }
    };
    let cutoff = p.cutoff.as_ref().and_then(datetime_to_chrono).unwrap_or_else(chrono::Utc::now);
    let mut batches = 0;
    while p.done_at.is_none() && max_batches.is_none_or(|m| batches < m) {
        let last = if p.stage == "redate" {
            let (last, changed) = redate_batch(state, &p.cursor, cutoff).await?;
            p.changed += changed;
            last
        } else {
            let (last, replaced, replacer) = suspect_batch(state, &p.cursor).await?;
            p.suspect_replaced += replaced;
            p.suspect_replacer += replacer;
            last
        };
        p.cursor = last;
        if p.cursor.is_none() {
            if p.stage == "redate" {
                p.stage = "suspects".into();
            } else {
                p.stage = "done".into();
                p.done_at = Some(Datetime::from(chrono::Utc::now()));
                // before the marker says done: a crash between the two repeats the event, never loses it
                report(state, &p).await;
            }
        }
        save(state, &p).await?;
        batches += 1;
    }
    Ok(p)
}

/// One log line and one `audit_event` per org when it finishes: counts only.
async fn report(state: &OrgState, p: &Progress) {
    let org = state.db.org().key();
    tracing::info!(
        org = %org,
        version = VERSION,
        memory = p.changed,
        suspect_replaced = p.suspect_replaced,
        suspect_replacer = p.suspect_replacer,
        "fact date backfill finished"
    );
    let detail = format!(
        "version={VERSION} memory={} suspect_replaced={} suspect_replacer={}",
        p.changed, p.suspect_replaced, p.suspect_replacer
    );
    let actor = Actor { kind: "system", id: "system".into() };
    audit::record(&state.control, Event { user: None, actor: &actor, action: "facts.backfill_dates", target: &org, outcome: "ok", detail: &detail }).await;
}

#[derive(Debug, Deserialize, SurrealValue)]
struct Row {
    id: RecordId,
    vault: RecordId,
    #[serde(rename = "type")]
    #[surreal(rename = "type")]
    kind: String,
    source: Option<RecordId>,
    occurred_at: Option<Datetime>,
    asserted_at: Option<Datetime>,
    created_at: Option<Datetime>,
    updated_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    version: i64,
    source_kind: Option<String>,
    source_at: Option<Datetime>,
    source_created: Option<String>,
}

/// `(occurred_at, asserted_at)` for `row`, if it is one the backfill re-dates (see the module docs).
fn dates_for(row: &Row, cutoff: Utc) -> Option<(Datetime, Datetime)> {
    if row.occurred_at.is_some() || row.asserted_at.is_some() || row.source.is_none() || !matches!(row.kind.as_str(), "world" | "experience") {
        return None;
    }
    let written = row.created_at.as_ref().and_then(datetime_to_chrono)?;
    if written >= cutoff || row.version > 1 {
        return None;
    }
    // edited since its write (an edit moves `updated_at`; a second of slack for the write itself)
    if row.updated_at.as_ref().and_then(datetime_to_chrono).is_some_and(|u| u > written + chrono::Duration::seconds(1)) {
        return None;
    }
    let usable = |d: Utc| crate::entities::extract::fact_date(Some(&d.to_rfc3339())).and_then(|d| datetime_to_chrono(&d));
    let at = usable(datetime_to_chrono(row.source_at.as_ref()?)?)?;
    let (happened, told) = if at <= written {
        (at, at)
    } else {
        // the record is dated after the fact was written: edited since, or an event still to come
        let created = row
            .source_created
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&chrono::Utc))
            .and_then(usable)
            .filter(|c| *c <= written);
        let told = created.unwrap_or(written);
        if EDIT_DATED.contains(&row.source_kind.as_deref().unwrap_or_default()) { (told, told) } else { (at, written) }
    };
    Some((Datetime::from(happened), Datetime::from(told)))
}

/// The ledger row of `memory`: the same key in `fact_dates_redated`.
fn ledger_id(memory: &RecordId) -> RecordId {
    RecordId::from_table_key("fact_dates_redated", memory.key().clone())
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

/// (last id read, facts re-dated); no last id once the table is exhausted.
async fn redate_batch(state: &OrgState, cursor: &Option<RecordId>, cutoff: Utc) -> AppResult<(Option<RecordId>, i64)> {
    let stmt = if cursor.is_some() { &q::MEMORY_AFTER } else { &q::MEMORY_FIRST };
    let key = cursor.as_ref().map(|c| c.key().clone());
    let rows: Vec<Row> = stmt.on(&state.db).bind(("key", key)).bind(("limit", BATCH)).await?.check()?.take(0)?;
    let mut changed = 0;
    let mut logged = Logged::default();
    for row in &rows {
        let Some((at, told)) = dates_for(row, cutoff) else { continue };
        let set = || async {
            q::REDATE
                .on(&state.db)
                .bind(("id", row.id.clone()))
                .bind(("ledger", ledger_id(&row.id)))
                .bind(("at", at))
                .bind(("told", told))
                .bind(("source", row.source.clone()))
                .bind(("version", row.version.max(1)))
                .await?
                .check()
        };
        if !with_retry(set).await?.take::<Vec<RecordId>>(q::REDATE.slot)?.is_empty() {
            changed += 1;
            logged.add(&row.vault, &row.id);
        }
    }
    logged.record(state).await;
    Ok((rows.last().map(|r| r.id.clone()), changed))
}

/// (last ledger row read, `replaced` flags, `replacer` flags) for one batch of the ledger.
async fn suspect_batch(state: &OrgState, cursor: &Option<RecordId>) -> AppResult<(Option<RecordId>, i64, i64)> {
    #[derive(Deserialize, SurrealValue)]
    struct Entry {
        id: RecordId,
        memory: RecordId,
        subject: Option<RecordId>,
        status: Option<String>,
        asserted_at: Datetime,
    }
    let stmt = if cursor.is_some() { &q::LEDGER_AFTER } else { &q::LEDGER_FIRST };
    let key = cursor.as_ref().map(|c| c.key().clone());
    let rows: Vec<Entry> = stmt.on(&state.db).bind(("key", key)).bind(("limit", BATCH)).await?.check()?.take(0)?;
    let (mut replaced, mut replacer) = (0, 0);
    for r in &rows {
        // the fact is gone (deleted since): nothing to flag
        let Some(subject) = &r.subject else { continue };
        let superseded = r.status.as_deref() == Some("superseded");
        let probe = if superseded { &q::REPLACED_SUSPECT } else { &q::REPLACER_SUSPECT };
        let hit: Vec<RecordId> =
            probe.on(&state.db).bind(("subject", subject.clone())).bind(("id", r.memory.clone())).bind(("told", r.asserted_at)).await?.check()?.take(0)?;
        if hit.is_empty() {
            continue;
        }
        let flag = if superseded { "replaced" } else { "replacer" };
        let set = || async { q::SET_SUSPECT.on(&state.db).bind(("id", r.id.clone())).bind(("suspect", flag)).await?.check() };
        with_retry(set).await?;
        if superseded { replaced += 1 } else { replacer += 1 }
    }
    Ok((rows.last().map(|r| r.id.clone()), replaced, replacer))
}

/// Undoes the backfill's re-dates: each fact gets its previous dates back where the backfill's are
/// still in place (a date changed since stands). Ledger rows stay, marked undone, so no later run dates
/// those facts again; the marker stays done. Returns the facts restored. Not wired to a route: an
/// operator runs it (or its statements) by hand if the re-dating has to be reverted.
pub async fn undo(state: &OrgState) -> AppResult<i64> {
    #[derive(Deserialize, SurrealValue)]
    struct Entry {
        id: RecordId,
        memory: RecordId,
        previous: Option<Datetime>,
        previous_asserted: Option<Datetime>,
        occurred_at: Datetime,
        asserted_at: Datetime,
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
                    .bind(("previous_asserted", r.previous_asserted))
                    .bind(("at", r.occurred_at))
                    .bind(("told", r.asserted_at))
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
        Some(Datetime::from(s.parse::<Utc>().unwrap()))
    }

    fn row() -> Row {
        Row {
            id: crate::rid::parse("memory:a").unwrap(),
            vault: crate::rid::parse("vault:v").unwrap(),
            kind: "world".into(),
            source: Some(crate::rid::parse("cache_record:r").unwrap()),
            occurred_at: None,
            asserted_at: None,
            created_at: dt("2026-11-01T00:00:00Z"),
            updated_at: dt("2026-11-01T00:00:00Z"),
            version: 1,
            source_kind: Some("gmail".into()),
            source_at: dt("2023-04-01T09:30:00Z"),
            source_created: None,
        }
    }

    fn got(r: &Row) -> Option<(String, String)> {
        let cutoff: Utc = "2026-11-13T00:00:00Z".parse().unwrap();
        let s = |d: &Datetime| datetime_to_chrono(d).unwrap().to_rfc3339();
        dates_for(r, cutoff).map(|(a, t)| (s(&a), s(&t)))
    }

    #[test]
    fn a_record_dated_before_the_fact_dates_it_twice() {
        let d = "2023-04-01T09:30:00+00:00".to_string();
        assert_eq!(got(&row()), Some((d.clone(), d)));
    }

    #[test]
    fn a_record_edited_after_the_fact_never_dates_it_later_than_its_write() {
        let written = "2026-11-01T00:00:00+00:00".to_string();
        let mut r = row();
        r.source_kind = Some("notion".into());
        r.source_at = dt("2026-11-10T00:00:00Z");
        assert_eq!(got(&r), Some((written.clone(), written.clone())), "no creation date: clamped to the write");
        r.source_created = Some("2024-01-01T00:00:00.000Z".into());
        let created = "2024-01-01T00:00:00+00:00".to_string();
        assert_eq!(got(&r), Some((created.clone(), created)), "the record's creation date is preferred");
        // a calendar event still to come keeps its start as when it happens, but is told at the write
        let mut r = row();
        r.source_kind = Some("google_calendar".into());
        r.source_at = dt("2027-01-15T00:00:00Z");
        assert_eq!(got(&r), Some(("2027-01-15T00:00:00+00:00".into(), written)));
    }

    #[test]
    fn only_undated_unedited_sourced_raw_facts_from_before_the_cutoff_are_redated() {
        type Change = Box<dyn Fn(&mut Row)>;
        let cases: [(&str, Change); 9] = [
            ("dated by its writer or by hand", Box::new(|r| r.occurred_at = dt("2020-01-01T00:00:00Z"))),
            ("told by an edit", Box::new(|r| r.asserted_at = dt("2026-11-02T00:00:00Z"))),
            ("edited (version)", Box::new(|r| r.version = 2)),
            ("edited (updated_at)", Box::new(|r| r.updated_at = dt("2026-11-05T00:00:00Z"))),
            ("no source record: a person wrote it", Box::new(|r| r.source = None)),
            ("an observation", Box::new(|r| r.kind = "observation".into())),
            ("written after the fix", Box::new(|r| r.created_at = dt("2026-11-14T00:00:00Z"))),
            ("the record has no date", Box::new(|r| r.source_at = None)),
            ("the record's date is the epoch sentinel", Box::new(|r| r.source_at = dt("1970-01-01T00:00:00Z"))),
        ];
        for (why, change) in cases {
            let mut r = row();
            change(&mut r);
            assert!(got(&r).is_none(), "{why}");
        }
    }
}
