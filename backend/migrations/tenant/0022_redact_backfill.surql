-- The one-off backfill that redacts secrets stored before writes were redacted (jobs::redact_backfill).
-- One row per detector version (`redact_backfill:v<redact::DETECTOR_VERSION>`): `stage` and `cursor`
-- say where the walk is, so a crashed run resumes there; `done_at` makes it run once per org per version.
DEFINE TABLE OVERWRITE redact_backfill SCHEMAFULL;
-- the table being walked: memory, then skill, then audit_log, then done
DEFINE FIELD OVERWRITE stage ON redact_backfill TYPE string ASSERT $value IN ["memory", "skill", "audit_log", "done"];
-- the last row of `stage` already checked; NONE: start of the table
DEFINE FIELD OVERWRITE cursor ON redact_backfill TYPE option<record<memory | skill | audit_log>>;
-- rows rewritten per table (counts only, never text)
DEFINE FIELD OVERWRITE changed_memory ON redact_backfill TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE changed_skill ON redact_backfill TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE changed_audit_log ON redact_backfill TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE updated_at ON redact_backfill TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE done_at ON redact_backfill TYPE option<datetime>;
