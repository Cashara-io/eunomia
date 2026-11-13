-- Data-subject erasure (compliance PR08; docs/privacy/subject-rights.md). Expand-only.
-- `legal_hold_at`: set while an admin holds the account (Settings > People). Erasure refuses, and a running
-- erasure parks as `held`, until it is lifted.
DEFINE FIELD IF NOT EXISTS legal_hold_at ON user TYPE option<datetime>;
-- Who placed the hold (only the workspace owner or a server operator may lift it).
DEFINE FIELD IF NOT EXISTS legal_hold_by ON user TYPE option<record<user>>;

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
-- `scope` is `account` (the person or a server operator asked: every workspace and the account) or
-- `workspace` (an admin asked: only `org`; the account is kept unless nothing else is left of it).
-- `open_slot` is the reference while the erasure is open (one open erasure per person) and the row id
-- once finished. The table is never pruned.
DEFINE TABLE IF NOT EXISTS erasure SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS org ON erasure TYPE string;
DEFINE FIELD IF NOT EXISTS subject_ref ON erasure TYPE string;
DEFINE FIELD IF NOT EXISTS subject ON erasure TYPE option<record<user>>;
DEFINE FIELD IF NOT EXISTS requested_by ON erasure TYPE string ASSERT $value IN ["self", "admin", "operator", "reapply"];
DEFINE FIELD IF NOT EXISTS scope ON erasure TYPE string DEFAULT "account" ASSERT $value IN ["account", "workspace"];
DEFINE FIELD IF NOT EXISTS requester ON erasure TYPE option<record<user>>;
-- The workspace whose sign-up or invitation made the account (its oldest membership): a workspace erasure
-- deletes the account only when it was made there and no other workspace is left.
DEFINE FIELD IF NOT EXISTS first_org ON erasure TYPE option<string>;
DEFINE FIELD IF NOT EXISTS status ON erasure TYPE string ASSERT $value IN ["pending", "running", "held", "done", "failed", "cancelled"];
DEFINE FIELD IF NOT EXISTS open_slot ON erasure TYPE string;
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

-- One running erasure per workspace: a running erasure holds a lock row for every workspace it touches
-- (the row id is the workspace key), taken in one transaction and released when it stops.
DEFINE TABLE IF NOT EXISTS erasure_lock SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS erasure ON erasure_lock TYPE record<erasure>;
DEFINE FIELD IF NOT EXISTS at ON erasure_lock TYPE datetime DEFAULT time::now();
DEFINE INDEX IF NOT EXISTS erasure_lock_erasure ON erasure_lock FIELDS erasure;
