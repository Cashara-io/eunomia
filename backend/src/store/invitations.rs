//! Statements for organisation invitations (`org_invitation`, control migration 0012). Every read and
//! write an org admin makes names `$org`, the admin's own org: an id from another org matches nothing.
//! See `invitations.rs`.

use super::ControlStmt;

pub const CONTROL_ALL: &[&ControlStmt] = &[
    &CREATE,
    &LIST,
    &RESEND,
    &REVOKE,
    &BY_HASH,
    &OPEN_FOR_EMAIL,
    &CLAIM,
    &REOPEN,
    &SEATS,
    &ROLE_IN_ORG,
];

/// A new open invitation. The unique `slot` (`<org>/<email>`) refuses a second open one for the same
/// email in the same org, even when two admins race.
pub const CREATE: ControlStmt = ControlStmt::new(
    "invitations.create",
    "CREATE org_invitation SET org = $org, email = $email, role = $role, token_hash = $hash, invited_by = $by, \
     expires_at = time::now() + <duration>$ttl, slot = string::concat(<string>$org, '/', $email) \
     RETURN id, email, role, created_at, sent_at, expires_at, false AS expired, invited_by.email AS invited_by",
);

/// The org's open invitations (expired ones too, so they can be resent), newest first.
pub const LIST: ControlStmt = ControlStmt::new(
    "invitations.list",
    "SELECT id, email, role, created_at, sent_at, expires_at, expires_at <= time::now() AS expired, invited_by.email AS invited_by \
     FROM org_invitation WHERE org = $org AND accepted_at IS NONE AND revoked_at IS NONE ORDER BY created_at DESC",
);

/// A new token and a new expiry for one of the org's open invitations. The old token stops working.
pub const RESEND: ControlStmt = ControlStmt::new(
    "invitations.resend",
    "UPDATE org_invitation SET token_hash = $hash, sent_at = time::now(), expires_at = time::now() + <duration>$ttl \
     WHERE id = $id AND org = $org AND accepted_at IS NONE AND revoked_at IS NONE \
     RETURN id, email, role, created_at, sent_at, expires_at, false AS expired, invited_by.email AS invited_by",
);

/// Withdraws one of the org's open invitations and frees its slot for a new one.
pub const REVOKE: ControlStmt = ControlStmt::new(
    "invitations.revoke",
    "UPDATE org_invitation SET revoked_at = time::now(), slot = string::concat('closed:', <string>id) \
     WHERE id = $id AND org = $org AND accepted_at IS NONE AND revoked_at IS NONE RETURN id, email, role",
);

/// The invitation a token belongs to, in any state (the caller tells unknown, closed and expired apart).
pub const BY_HASH: ControlStmt = ControlStmt::new(
    "invitations.by_hash",
    "SELECT id, org, org.name AS org_name, email, role, invited_by, invited_by.email AS invited_by_email, accepted_at, revoked_at, \
     expires_at, expires_at <= time::now() AS expired FROM org_invitation WHERE token_hash = $hash LIMIT 1",
);

/// The newest open, unexpired invitation for an email (single sign-on honours it for a new account).
pub const OPEN_FOR_EMAIL: ControlStmt = ControlStmt::new(
    "invitations.open_for_email",
    "SELECT id, org, org.name AS org_name, email, role, invited_by, invited_by.email AS invited_by_email, accepted_at, revoked_at, \
     expires_at, false AS expired FROM org_invitation \
     WHERE email = $email AND accepted_at IS NONE AND revoked_at IS NONE AND expires_at > time::now() ORDER BY sent_at DESC LIMIT 1",
);

/// Accepts an invitation for `$user`: marks it accepted (only if it is still open, unexpired and for
/// `$email`) and, when `$join`, adds the membership with the invited role, in one transaction. Two
/// accepts of one token cannot both pass the first step, and `membership_unique` (user, org) backs it
/// up; a lost race is a commit conflict that the caller retries into `invitation.unavailable`.
pub const CLAIM: ControlStmt = ControlStmt::at(
    "invitations.claim",
    "BEGIN TRANSACTION; \
     LET $inv = (UPDATE org_invitation SET accepted_at = time::now(), accepted_by = $user, slot = string::concat('closed:', <string>id) \
        WHERE id = $id AND email = $email AND accepted_at IS NONE AND revoked_at IS NONE AND expires_at > time::now() RETURN AFTER); \
     IF array::len($inv) = 0 { THROW 'invitation.unavailable' }; \
     LET $org = $inv[0].org; \
     IF $join { CREATE membership SET user = $user, org = $org, role = $inv[0].role, slot = string::concat(<string>$org, '/', <string>$user); }; \
     RETURN $org; \
     COMMIT TRANSACTION;",
    5, // BEGIN 0, LET 1, IF 2, LET 3, IF 4, RETURN 5
);

/// Undoes [`CLAIM`] for a signup that failed after it (the account is being deleted).
pub const REOPEN: ControlStmt = ControlStmt::new(
    "invitations.reopen",
    "UPDATE org_invitation SET accepted_at = NONE, accepted_by = NONE, slot = string::concat(<string>org, '/', email) \
     WHERE id = $id AND accepted_by = $user",
);

/// Seats for one org: active members, suspended members, and open unexpired invitations.
pub const SEATS: ControlStmt = ControlStmt::new(
    "invitations.seats",
    "SELECT count() AS n FROM membership WHERE org = $org AND user.suspended_at IS NONE GROUP ALL; \
     SELECT count() AS n FROM membership WHERE org = $org AND user.suspended_at IS NOT NONE GROUP ALL; \
     SELECT count() AS n FROM org_invitation WHERE org = $org AND accepted_at IS NONE AND revoked_at IS NONE AND expires_at > time::now() GROUP ALL",
);

/// The caller's role in an org (`owner`, `admin` or `member`), if they belong to it.
pub const ROLE_IN_ORG: ControlStmt =
    ControlStmt::new("invitations.role_in_org", "SELECT VALUE role FROM membership WHERE user = $user AND org = $org LIMIT 1");
