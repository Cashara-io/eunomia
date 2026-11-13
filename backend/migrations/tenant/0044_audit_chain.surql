-- Access reviews (compliance PR03, docs/audit-chain.md). An admin snapshots who has access to
-- what, records a decision per person and completes the review. A completed review cannot be
-- changed: the events below refuse any update or delete of it and of its decisions.
DEFINE TABLE IF NOT EXISTS access_review SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS status ON access_review TYPE string DEFAULT "open" ASSERT $value IN ["open", "completed"];
-- "open" while open, NONE once completed: the unique index allows one open review at a time.
DEFINE FIELD IF NOT EXISTS open_slot ON access_review TYPE option<string>;
DEFINE FIELD IF NOT EXISTS started_by ON access_review TYPE record<user>;
DEFINE FIELD IF NOT EXISTS started_by_email ON access_review TYPE string;
DEFINE FIELD IF NOT EXISTS started_at ON access_review TYPE datetime DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS completed_by ON access_review TYPE option<record<user>>;
DEFINE FIELD IF NOT EXISTS completed_by_email ON access_review TYPE option<string>;
DEFINE FIELD IF NOT EXISTS completed_at ON access_review TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS snapshot ON access_review TYPE object FLEXIBLE;
DEFINE FIELD IF NOT EXISTS snapshot_hash ON access_review TYPE string;
DEFINE FIELD IF NOT EXISTS result_hash ON access_review TYPE option<string>;
DEFINE FIELD IF NOT EXISTS summary ON access_review TYPE option<object> FLEXIBLE;
DEFINE INDEX IF NOT EXISTS access_review_open_unique ON access_review FIELDS open_slot UNIQUE;
DEFINE INDEX IF NOT EXISTS access_review_started_idx ON access_review FIELDS started_at;
DEFINE EVENT IF NOT EXISTS access_review_frozen ON access_review
    WHEN $before.status = "completed" THEN { THROW "access_review.completed: a completed access review cannot be changed" };

DEFINE TABLE IF NOT EXISTS access_review_decision SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS review ON access_review_decision TYPE record<access_review>;
DEFINE FIELD IF NOT EXISTS subject ON access_review_decision TYPE record<user>;
DEFINE FIELD IF NOT EXISTS subject_email ON access_review_decision TYPE string;
DEFINE FIELD IF NOT EXISTS decision ON access_review_decision TYPE string ASSERT $value IN ["keep", "remove", "suspend"];
DEFINE FIELD IF NOT EXISTS note ON access_review_decision TYPE string DEFAULT "";
DEFINE FIELD IF NOT EXISTS reviewer ON access_review_decision TYPE record<user>;
DEFINE FIELD IF NOT EXISTS reviewer_email ON access_review_decision TYPE string;
DEFINE FIELD IF NOT EXISTS decided_at ON access_review_decision TYPE datetime DEFAULT time::now();
DEFINE INDEX IF NOT EXISTS access_review_decision_unique ON access_review_decision FIELDS review, subject UNIQUE;
DEFINE EVENT IF NOT EXISTS access_review_decision_frozen ON access_review_decision
    WHEN ($after.review ?? $before.review).status = "completed" THEN { THROW "access_review.completed: a completed access review cannot be changed" };
