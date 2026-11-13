-- Data-subject erasure (compliance PR08; docs/privacy/subject-rights.md). Expand-only.
-- `legal_hold_at`: set while an admin holds the account (Settings > People). Erasure refuses until it is lifted.
DEFINE FIELD IF NOT EXISTS legal_hold_at ON user TYPE option<datetime>;

-- One row per erasure: the request, its progress and, once done, the completion record that outlives the
-- person. `subject_ref` is a keyed hash (HMAC-SHA256 under ENCRYPTION_KEY) of the user's record id: it names
-- nobody, but lets a restored copy of the account be recognised and erased again, and lets a sync job for an
-- erased account stand down. `subject` holds the user's id only while the erasure is pending or running; the
-- job clears it when it writes `counts` and `post_check`. The table is never pruned.
DEFINE TABLE IF NOT EXISTS erasure SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS org ON erasure TYPE string;
DEFINE FIELD IF NOT EXISTS subject_ref ON erasure TYPE string;
DEFINE FIELD IF NOT EXISTS subject ON erasure TYPE option<record<user>>;
DEFINE FIELD IF NOT EXISTS requested_by ON erasure TYPE string ASSERT $value IN ["self", "admin", "reapply"];
DEFINE FIELD IF NOT EXISTS status ON erasure TYPE string ASSERT $value IN ["pending", "running", "done", "failed"];
DEFINE FIELD IF NOT EXISTS counts ON erasure TYPE object FLEXIBLE DEFAULT {};
DEFINE FIELD IF NOT EXISTS post_check ON erasure TYPE object FLEXIBLE DEFAULT {};
DEFINE FIELD IF NOT EXISTS error ON erasure TYPE option<string>;
DEFINE FIELD IF NOT EXISTS requested_at ON erasure TYPE datetime DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS completed_at ON erasure TYPE option<datetime>;
DEFINE INDEX IF NOT EXISTS erasure_subject_ref ON erasure FIELDS subject_ref;
DEFINE INDEX IF NOT EXISTS erasure_org ON erasure FIELDS org, requested_at;
