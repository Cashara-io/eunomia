//! Statements for organisation plans, seats and the operator API (control migration 0015). See `plan.rs`
//! and `operator.rs`.
//!
//! Every statement that takes a seat (an invitation made or resent, a membership added by accepting one,
//! a suspended member restored) reads the seat limit and the seats in use and writes in one transaction,
//! and writes `org.seat_guard` first. Two such transactions for one limited organisation therefore meet on
//! one row: the first to commit wins and the other retries (`tx::with_retry`) and sees the seat taken.
//! An organisation with no seat limit skips all of it, so an unmanaged organisation pays nothing.

use super::ControlStmt;

pub const CONTROL_ALL: &[&ControlStmt] = &[
    &GET,
    &LIST,
    &CREATE_ORG,
    &UPDATE,
    &SUSPEND,
    &RESUME,
    &RESTORE_MEMBER,
    &CREATE_OWNER_CLAIM,
    &RESEND_OWNER_CLAIM,
    &ACTIVE_BY_ORG,
    &SUSPENDED_BY_ORG,
    &PENDING_BY_ORG,
    &OWNERS,
    &OWNER_CLAIMS,
    &SEATS_USED,
];

/// The seats in use by `$org`: active members plus pending (open, unexpired) invitations. Suspended members
/// do not count. A SurrealQL expression, shared so every statement counts the same way.
macro_rules! used {
    () => {
        "((SELECT count() AS n FROM membership WHERE org = $org AND user.suspended_at IS NONE GROUP ALL)[0].n ?? 0) + \
         ((SELECT count() AS n FROM org_invitation WHERE org = $org AND accepted_at IS NONE AND revoked_at IS NONE AND expires_at > time::now() GROUP ALL)[0].n ?? 0)"
    };
}
pub(crate) use used;

/// The plan fields of one organisation (an organisation with none set is unmanaged).
pub const GET: ControlStmt = ControlStmt::new(
    "plan.get",
    "SELECT id, name, created_at, plan, seat_limit, status, suspended_at, suspended_reason, trial_ends_at, external_ref FROM org WHERE id = $org",
);

/// One page of organisations, oldest first.
pub const LIST: ControlStmt = ControlStmt::new(
    "plan.list",
    "SELECT id, name, created_at, plan, seat_limit, status, suspended_at, suspended_reason, trial_ends_at, external_ref FROM org ORDER BY created_at, id LIMIT $limit START $start",
);

/// Makes the organisation record, once: a second call with the same id (a retried webhook, two replicas) leaves
/// the first record as it is. The capacity check shares the transaction with the insert.
pub const CREATE_ORG: ControlStmt = ControlStmt::new(
    "plan.create_org",
    "BEGIN TRANSACTION; \
     LET $n = (SELECT count() AS n FROM org GROUP ALL)[0].n ?? 0; \
     LET $exists = array::len((SELECT id FROM org WHERE id = $org)) > 0; \
     IF !$exists AND $max >= 0 AND $n >= $max { THROW 'operator.at_capacity' }; \
     INSERT IGNORE INTO org { id: $org, name: $name, plan: $plan, seat_limit: $seat_limit, status: 'active', trial_ends_at: $trial_ends_at, external_ref: $external_ref }; \
     COMMIT TRANSACTION;",
);

/// Changes the plan fields that were sent. Lowering the seat limit below the seats in use is refused in the
/// same transaction that writes it, so an invitation racing it either lands first (and the lowering is refused)
/// or finds the new limit.
pub const UPDATE: ControlStmt = ControlStmt::new(
    "plan.update",
    concat!(
        "BEGIN TRANSACTION; \
         UPDATE $org SET seat_guard = (seat_guard ?? 0) + 1 RETURN NONE; \
         LET $used = ",
        used!(),
        "; \
         IF $set_limit AND $seat_limit != NONE AND $seat_limit < $used { THROW 'operator.seats_in_use' }; \
         UPDATE $org SET name = IF $set_name THEN $name ELSE name END, plan = IF $set_plan THEN $plan ELSE plan END, \
             seat_limit = IF $set_limit THEN $seat_limit ELSE seat_limit END, \
             trial_ends_at = IF $set_trial THEN $trial_ends_at ELSE trial_ends_at END RETURN NONE; \
         COMMIT TRANSACTION;"
    ),
);

/// Suspends (now, keeping the first time on a repeat) and records why.
pub const SUSPEND: ControlStmt = ControlStmt::new(
    "plan.suspend",
    "UPDATE $org SET status = 'suspended', suspended_at = (suspended_at ?? time::now()), suspended_reason = $reason RETURN NONE",
);

pub const RESUME: ControlStmt =
    ControlStmt::new("plan.resume", "UPDATE $org SET status = 'active', suspended_at = NONE, suspended_reason = NONE RETURN NONE");

/// Restores a suspended member: a seat is taken again, so the limit is checked in the same transaction.
pub const RESTORE_MEMBER: ControlStmt = ControlStmt::new(
    "plan.restore_member",
    concat!(
        "BEGIN TRANSACTION; \
         LET $limit = (SELECT VALUE seat_limit FROM org WHERE id = $org)[0]; \
         IF $limit != NONE { \
             UPDATE $org SET seat_guard = (seat_guard ?? 0) + 1 RETURN NONE; \
             LET $used = ",
        used!(),
        "; \
             IF $used >= $limit { THROW 'org.seat_limit' }; \
         }; \
         UPDATE user SET suspended_at = NONE WHERE id = $id RETURN id; \
         COMMIT TRANSACTION;"
    ),
);

/// The owner claim of a new organisation: an invitation of kind `owner` with no inviting account. The first
/// seat belongs to it (a seat limit is at least 1), so there is no seat check.
pub const CREATE_OWNER_CLAIM: ControlStmt = ControlStmt::new(
    "plan.create_owner_claim",
    "CREATE org_invitation SET org = $org, email = $email, role = 'owner', token_hash = $hash, \
     expires_at = time::now() + <duration>$ttl, slot = string::concat(<string>$org, '/', $email) \
     RETURN id, email, role, created_at, sent_at, expires_at, false AS expired, invited_by.email AS invited_by",
);

/// A new token and a new expiry for the organisation's unclaimed owner claim. The old token stops working.
pub const RESEND_OWNER_CLAIM: ControlStmt = ControlStmt::new(
    "plan.resend_owner_claim",
    "UPDATE org_invitation SET token_hash = $hash, sent_at = time::now(), expires_at = time::now() + <duration>$ttl \
     WHERE org = $org AND role = 'owner' AND accepted_at IS NONE AND revoked_at IS NONE \
     RETURN id, email, role, created_at, sent_at, expires_at, false AS expired, invited_by.email AS invited_by",
);

/// Seat counts for a page of organisations (`$orgs`), one grouped read each.
pub const ACTIVE_BY_ORG: ControlStmt =
    ControlStmt::new("plan.active_by_org", "SELECT org, count() AS n FROM membership WHERE org IN $orgs AND user.suspended_at IS NONE GROUP BY org");

pub const SUSPENDED_BY_ORG: ControlStmt =
    ControlStmt::new("plan.suspended_by_org", "SELECT org, count() AS n FROM membership WHERE org IN $orgs AND user.suspended_at IS NOT NONE GROUP BY org");

pub const PENDING_BY_ORG: ControlStmt = ControlStmt::new(
    "plan.pending_by_org",
    "SELECT org, count() AS n FROM org_invitation WHERE org IN $orgs AND accepted_at IS NONE AND revoked_at IS NONE AND expires_at > time::now() GROUP BY org",
);

/// The owners of a page of organisations.
pub const OWNERS: ControlStmt = ControlStmt::new("plan.owners", "SELECT org, user.email AS email FROM membership WHERE org IN $orgs AND role = 'owner'");

/// The unclaimed owner claims of a page of organisations.
pub const OWNER_CLAIMS: ControlStmt = ControlStmt::new(
    "plan.owner_claims",
    "SELECT org, email, expires_at, expires_at <= time::now() AS expired FROM org_invitation WHERE org IN $orgs AND role = 'owner' AND accepted_at IS NONE AND revoked_at IS NONE",
);

/// Seats in use by `$org` (the same count the seat checks make), for the plan status.
pub const SEATS_USED: ControlStmt = ControlStmt::new("plan.seats_used", concat!("RETURN ", used!(), ";"));
