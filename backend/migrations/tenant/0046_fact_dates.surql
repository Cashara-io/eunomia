-- The one-off backfill that dates facts extracted from synced records before extraction passed the
-- record's date on (jobs::fact_dates). One row (`fact_dates_backfill:v1`): `cursor` says where the walk
-- over `memory` is, so a crashed run resumes there; `cutoff` bounds it to facts written before it first
-- ran; `done_at` makes it run once per org. Facts already carrying a date, and facts with no source
-- record, are never touched. `fact_dates_redated` (below) keeps one row per fact it re-dated.
DEFINE TABLE OVERWRITE fact_dates_backfill SCHEMAFULL;
-- the last memory already checked; NONE: start of the table
DEFINE FIELD OVERWRITE cursor ON fact_dates_backfill TYPE option<record<memory>>;
-- only facts written before this are re-dated (set when the walk starts)
DEFINE FIELD OVERWRITE cutoff ON fact_dates_backfill TYPE option<datetime>;
-- facts re-dated (a count only)
DEFINE FIELD OVERWRITE changed ON fact_dates_backfill TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE updated_at ON fact_dates_backfill TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE done_at ON fact_dates_backfill TYPE option<datetime>;
-- re-dated facts whose replacement may be wrong: marked superseded while a live fact of the same subject
-- is now dated before them (an upper bound; which fact replaced which is not stored). Counted, not changed.
DEFINE FIELD OVERWRITE suspect_superseded ON fact_dates_backfill TYPE int DEFAULT 0;

-- One row per fact the backfill re-dated, keyed by the memory's own key: what it was before and what
-- the backfill set, so the change can be undone (jobs::fact_dates::undo) and no later run dates a row
-- twice. Ids and dates only, never text.
DEFINE TABLE OVERWRITE fact_dates_redated SCHEMAFULL;
DEFINE FIELD OVERWRITE memory ON fact_dates_redated TYPE record<memory>;
-- the fact's occurred_at before the backfill (NONE: undated, the only case the backfill touches)
DEFINE FIELD OVERWRITE previous ON fact_dates_redated TYPE option<datetime>;
-- the date the backfill set, from its source record
DEFINE FIELD OVERWRITE occurred_at ON fact_dates_redated TYPE datetime;
DEFINE FIELD OVERWRITE source ON fact_dates_redated TYPE option<record<cache_record>>;
DEFINE FIELD OVERWRITE version ON fact_dates_redated TYPE int;
DEFINE FIELD OVERWRITE redated_at ON fact_dates_redated TYPE datetime DEFAULT time::now();
-- set by an undo: the previous date is back (unless the date changed since), and the row stays so no
-- later run dates the fact again
DEFINE FIELD OVERWRITE undone_at ON fact_dates_redated TYPE option<datetime>;
