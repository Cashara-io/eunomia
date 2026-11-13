-- Facts carry two dates (docs/concepts.md, "A fact's two dates"). `occurred_at` (when it happened,
-- 0019) is unchanged. `asserted_at` is when we were last told it: an extracted fact's record date,
-- never later than its write; a directly written fact's write time; an edited fact's edit time. Only
-- `asserted_at` decides which fact is current (entities::supersede). NONE on facts written before
-- this migration: they fall back to their last write.
DEFINE FIELD OVERWRITE asserted_at ON memory TYPE option<datetime>;
-- the fact that replaced this one, set when it is marked superseded (NONE before this migration)
DEFINE FIELD OVERWRITE superseded_by ON memory TYPE option<record<memory>>;

-- The one-off backfill that dates facts extracted from synced records before extraction passed the
-- record's date on (jobs::fact_dates). One row (`fact_dates_backfill:v1`): `stage` and `cursor` say
-- where the walk is (over `memory`, then over the ledger to flag suspect replacements), so a crashed
-- run resumes there; `cutoff` bounds it to facts written before it first ran; `done_at` makes it run
-- once per org. `fact_dates_redated` (below) keeps one row per fact it re-dated.
DEFINE TABLE OVERWRITE fact_dates_backfill SCHEMAFULL;
DEFINE FIELD OVERWRITE stage ON fact_dates_backfill TYPE string DEFAULT "redate" ASSERT $value IN ["redate", "suspects", "done"];
-- the last row of `stage` already handled; NONE: start of its table
DEFINE FIELD OVERWRITE cursor ON fact_dates_backfill TYPE option<record<memory | fact_dates_redated>>;
-- only facts written before this are re-dated (set when the walk starts)
DEFINE FIELD OVERWRITE cutoff ON fact_dates_backfill TYPE option<datetime>;
-- facts re-dated (a count only)
DEFINE FIELD OVERWRITE changed ON fact_dates_backfill TYPE int DEFAULT 0;
-- re-dated facts flagged as possibly wrongly replaced, and as possibly having wrongly replaced another
DEFINE FIELD OVERWRITE suspect_replaced ON fact_dates_backfill TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE suspect_replacer ON fact_dates_backfill TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE updated_at ON fact_dates_backfill TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE done_at ON fact_dates_backfill TYPE option<datetime>;

-- One row per fact the backfill re-dated, keyed by the memory's own key: what it was before and what
-- the backfill set, so the change can be undone (jobs::fact_dates::undo) and no later run dates a row
-- twice. Ids, dates and a flag only, never text.
DEFINE TABLE OVERWRITE fact_dates_redated SCHEMAFULL;
DEFINE FIELD OVERWRITE memory ON fact_dates_redated TYPE record<memory>;
-- the fact's dates before the backfill (NONE: undated, the only case the backfill touches)
DEFINE FIELD OVERWRITE previous ON fact_dates_redated TYPE option<datetime>;
DEFINE FIELD OVERWRITE previous_asserted ON fact_dates_redated TYPE option<datetime>;
-- the dates the backfill set, from its source record
DEFINE FIELD OVERWRITE occurred_at ON fact_dates_redated TYPE datetime;
DEFINE FIELD OVERWRITE asserted_at ON fact_dates_redated TYPE datetime;
DEFINE FIELD OVERWRITE source ON fact_dates_redated TYPE option<record<cache_record>>;
DEFINE FIELD OVERWRITE version ON fact_dates_redated TYPE int;
DEFINE FIELD OVERWRITE redated_at ON fact_dates_redated TYPE datetime DEFAULT time::now();
-- a replacement that may have gone the wrong way once the dates moved (jobs::fact_dates, "suspects"):
-- "replaced" (this fact is superseded while a live fact on its subject was told before it) or
-- "replacer" (this fact is live while a superseded fact on its subject was told after it)
DEFINE FIELD OVERWRITE suspect ON fact_dates_redated TYPE option<string> ASSERT $value = NONE OR $value IN ["replaced", "replacer"];
-- set by an undo: the previous dates are back (unless they changed since), and the row stays so no
-- later run dates the fact again
DEFINE FIELD OVERWRITE undone_at ON fact_dates_redated TYPE option<datetime>;
