-- Data-subject erasure (compliance PR08; docs/privacy/subject-rights.md). Expand-only.
-- `legal_hold_at`: set while an admin holds the account (Settings > People). Erasure refuses, and a running
-- erasure parks as `held`, until it is lifted.
DEFINE FIELD IF NOT EXISTS legal_hold_at ON user TYPE option<datetime>;

-- A workspace whose only member was erased is closed: its database is emptied, nobody can be served from
-- it or join it, and the empty database stays for an operator to drop.
DEFINE FIELD OVERWRITE status ON tenant TYPE string DEFAULT "provisioning" ASSERT $value IN ["provisioning", "moving", "ready", "closed"];
DEFINE FIELD IF NOT EXISTS closed_at ON org TYPE option<datetime>;

-- One row per erasure: the request, its working record while it runs, and once done the completion record
-- that outlives the person. `subject_ref` is a keyed hash (HMAC-SHA256 under ENCRYPTION_KEY) of the user's
-- record id: it names nobody, but lets a restored copy be recognised and erased again, and a sync for an
-- erased account stand down. `subject` and `work` (the identifiers the post-check needs: the email in
-- lowercase, the person's token ids, their key, their file paths, the plan per workspace) are held only
-- while the erasure is open; the job clears them when it writes `counts` and `post_check`.
-- `open_slot` is the reference while the erasure is open (one open erasure per person) and the row id
-- once done; `running_slot` is the home workspace while it runs (one running erasure per workspace) and
-- the row id otherwise. The table is never pruned.
DEFINE TABLE IF NOT EXISTS erasure SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS org ON erasure TYPE string;
DEFINE FIELD IF NOT EXISTS subject_ref ON erasure TYPE string;
DEFINE FIELD IF NOT EXISTS subject ON erasure TYPE option<record<user>>;
DEFINE FIELD IF NOT EXISTS requested_by ON erasure TYPE string ASSERT $value IN ["self", "admin", "reapply"];
DEFINE FIELD IF NOT EXISTS requester ON erasure TYPE option<record<user>>;
DEFINE FIELD IF NOT EXISTS status ON erasure TYPE string ASSERT $value IN ["pending", "running", "held", "done", "failed"];
DEFINE FIELD IF NOT EXISTS open_slot ON erasure TYPE string;
DEFINE FIELD IF NOT EXISTS running_slot ON erasure TYPE string;
DEFINE FIELD IF NOT EXISTS work ON erasure TYPE object FLEXIBLE DEFAULT {};
DEFINE FIELD IF NOT EXISTS closes ON erasure TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS counts ON erasure TYPE object FLEXIBLE DEFAULT {};
DEFINE FIELD IF NOT EXISTS post_check ON erasure TYPE object FLEXIBLE DEFAULT {};
DEFINE FIELD IF NOT EXISTS review ON erasure TYPE object FLEXIBLE DEFAULT {};
DEFINE FIELD IF NOT EXISTS error ON erasure TYPE option<string>;
DEFINE FIELD IF NOT EXISTS requested_at ON erasure TYPE datetime DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS held_at ON erasure TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS completed_at ON erasure TYPE option<datetime>;
DEFINE INDEX IF NOT EXISTS erasure_subject_ref ON erasure FIELDS subject_ref;
DEFINE INDEX IF NOT EXISTS erasure_org ON erasure FIELDS org, requested_at;
DEFINE INDEX IF NOT EXISTS erasure_open_slot ON erasure FIELDS open_slot UNIQUE;
DEFINE INDEX IF NOT EXISTS erasure_running_slot ON erasure FIELDS running_slot UNIQUE;
