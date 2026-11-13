//! Access reviews (`routers/access_reviews.rs`, tenant migration 0044). A completed review is
//! frozen by the schema's events: no statement here can change it afterwards.

use super::Stmt;

pub const ALL: &[&Stmt] = &[&REVIEW_CREATE, &REVIEW_LIST, &REVIEW_GET, &REVIEW_DECISIONS, &REVIEW_DECIDE, &REVIEW_COMPLETE, &VAULT_ACCESS];

/// Opens a review. The unique `open_slot` index refuses a second open review.
pub const REVIEW_CREATE: Stmt = Stmt::new(
    "access_reviews.create",
    "CREATE access_review SET org = $org, status = 'open', open_slot = 'open:' + $org, started_by = $by, started_by_email = $email, \
     started_at = time::now(), snapshot = $snapshot, snapshot_hash = $snapshot_hash RETURN id",
);

/// The newest reviews first.
pub const REVIEW_LIST: Stmt = Stmt::new(
    "access_reviews.list",
    "SELECT id, status, started_by_email, started_at, completed_by_email, completed_at, result_hash, summary, \
     array::len(snapshot.people) AS people FROM access_review WHERE org = $org ORDER BY started_at DESC LIMIT $limit",
);

pub const REVIEW_GET: Stmt = Stmt::new(
    "access_reviews.get",
    "SELECT id, status, started_by, started_by_email, started_at, completed_by, completed_by_email, completed_at, \
     snapshot, snapshot_hash, result_hash, summary, rev FROM access_review WHERE id = $id AND org = $org",
);

pub const REVIEW_DECISIONS: Stmt = Stmt::new(
    "access_reviews.decisions",
    "SELECT subject, subject_email, decision, note, reviewer, reviewer_email, decided_at FROM access_review_decision \
     WHERE review = $review ORDER BY subject_email, subject",
);

/// One decision per person per review (`$id` is derived from both); deciding again replaces it
/// while the review is open. Bumps the review's `rev` in the same transaction, so it conflicts with a
/// completion that read the review before it.
pub const REVIEW_DECIDE: Stmt = Stmt::new(
    "access_reviews.decide",
    "BEGIN TRANSACTION; \
     LET $r = UPDATE $review SET rev = (rev ?? 0) + 1 WHERE status = 'open' AND org = $org RETURN rev; \
     IF array::len($r) = 0 { THROW 'access_review.completed: a completed access review cannot be changed' }; \
     UPSERT $id SET review = $review, subject = $subject, subject_email = $subject_email, decision = $decision, note = $note, \
     reviewer = $reviewer, reviewer_email = $reviewer_email, decided_at = time::now() RETURN NONE; \
     COMMIT TRANSACTION;",
);

/// Completes an open review whose `rev` is still the one the caller read with its decisions
/// (`$rev`). Returns nothing otherwise: a decision landed in between and the hash would be stale.
pub const REVIEW_COMPLETE: Stmt = Stmt::new(
    "access_reviews.complete",
    "UPDATE $id SET status = 'completed', open_slot = NONE, completed_by = $by, completed_by_email = $email, \
     completed_at = $at, result_hash = $result_hash, summary = $summary WHERE status = 'open' AND org = $org AND (rev ?? 0) = $rev RETURN id",
);

/// Every space membership of `$ids` in a space one of `$ids` owns, with the space's name and kind.
/// `$ids` are this org's people, so only this org's spaces count, even with several orgs in one
/// database (tests/isolation.rs, shared-database mode).
pub const VAULT_ACCESS: Stmt = Stmt::new(
    "access_reviews.vault_access",
    "SELECT user, role, status, vault AS vault_id, vault.name AS vault_name, vault.kind AS vault_kind FROM vault_member \
     WHERE user IN $ids AND vault IN (SELECT VALUE vault FROM vault_member WHERE role = 'owner' AND user IN $ids) \
     ORDER BY vault_name, vault_id",
);
