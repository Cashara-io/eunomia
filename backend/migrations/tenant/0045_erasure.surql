-- Data-subject export archives (compliance PR08; docs/privacy/subject-rights.md). Expand-only.
-- One row per export job: the zip archive (manifest.json plus one file per category and the subject's
-- document files) is built by the `subject_export` job and kept for seven days.
DEFINE TABLE IF NOT EXISTS subject_export SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS subject ON subject_export TYPE record<user>;
DEFINE FIELD IF NOT EXISTS requested_by ON subject_export TYPE record<user>;
-- `self`: the person's own export, every workspace (one row per workspace, the same id: each workspace's
-- part stays in its own database; the home row also holds the account-level part and is the one listed).
-- `organisation`: an admin's export of this workspace's data about the person, one row, here.
DEFINE FIELD IF NOT EXISTS scope ON subject_export TYPE string DEFAULT "self" ASSERT $value IN ["self", "organisation"];
DEFINE FIELD IF NOT EXISTS part ON subject_export TYPE bool DEFAULT false;
DEFINE FIELD IF NOT EXISTS status ON subject_export TYPE string ASSERT $value IN ["pending", "ready", "failed"];
DEFINE FIELD IF NOT EXISTS manifest ON subject_export TYPE object FLEXIBLE DEFAULT {};
DEFINE FIELD IF NOT EXISTS archive ON subject_export TYPE option<bytes>;
DEFINE FIELD IF NOT EXISTS size_bytes ON subject_export TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS error ON subject_export TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at ON subject_export TYPE datetime DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS expires_at ON subject_export TYPE datetime DEFAULT time::now() + 7d;
DEFINE INDEX IF NOT EXISTS subject_export_subject ON subject_export FIELDS subject;
DEFINE INDEX IF NOT EXISTS subject_export_requested_by ON subject_export FIELDS requested_by;
