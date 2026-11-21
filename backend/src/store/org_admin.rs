//! Statements for an organisation's own administration (`org_admin.rs`, control migration 0014). The
//! control database is shared by every organisation, so each statement names `$org`, the caller's own
//! organisation (never a request value): an id from another organisation matches nothing.

use super::ControlStmt;

pub const CONTROL_ALL: &[&ControlStmt] = &[
    &MEMBER,
    &ROLES,
    &SET_ROLE,
    &ORG_GET,
    &SET_NAME,
    &SET_MFA_POLICY,
    &MFA_POLICY_FOR_USER,
    &WITHOUT_MFA,
    &REVOKE_ACCESS,
];

/// One member of `$org`, read through the membership, so a user who is not in `$org` is no row at all.
pub const MEMBER: ControlStmt = ControlStmt::new(
    "org_admin.member",
    "SELECT user AS id, role, user.email AS email, user.email_lc AS email_lc, user.admin AS admin, user.suspended_at AS suspended_at \
     FROM membership WHERE org = $org AND user = $user LIMIT 1",
);

/// The role of every member of `$org`.
pub const ROLES: ControlStmt = ControlStmt::new("org_admin.roles", "SELECT user, role FROM membership WHERE org = $org");

/// Changes a member's role between `member` and `admin`. The guard names the role it was read with and
/// never matches the owner, so a role changed meanwhile (or an owner) is left alone: no row comes back.
pub const SET_ROLE: ControlStmt = ControlStmt::new(
    "org_admin.set_role",
    "UPDATE membership SET role = $role WHERE org = $org AND user = $user AND role = $from AND role != 'owner' RETURN user",
);

/// The organisation's name and its own MFA policy.
pub const ORG_GET: ControlStmt = ControlStmt::new(
    "org_admin.org_get",
    "SELECT name, require_mfa, require_mfa_at, require_mfa_by.email AS require_mfa_by FROM ONLY $org",
);

pub const SET_NAME: ControlStmt = ControlStmt::new("org_admin.set_name", "UPDATE $org SET name = $name RETURN NONE");

pub const SET_MFA_POLICY: ControlStmt = ControlStmt::new(
    "org_admin.set_mfa_policy",
    "UPDATE $org SET require_mfa = $require_mfa, require_mfa_at = time::now(), require_mfa_by = $by RETURN NONE",
);

/// How many of the user's organisations require two-step verification (`mfa::required_for`).
pub const MFA_POLICY_FOR_USER: ControlStmt = ControlStmt::new(
    "org_admin.mfa_policy_for_user",
    "SELECT count() AS n FROM membership WHERE user = $user AND org.require_mfa = true GROUP ALL",
);

/// Active password accounts of `$org` without two-step verification: what the policy asks to enrol.
pub const WITHOUT_MFA: ControlStmt = ControlStmt::new(
    "org_admin.without_mfa",
    "SELECT count() AS n FROM user WHERE id IN (SELECT VALUE user FROM membership WHERE org = $org) \
     AND password_hash != NONE AND mfa_secret_enc = NONE AND suspended_at = NONE GROUP ALL",
);

/// Ends everything `$user` holds that signs in as them: browser sessions, API tokens, OAuth grants with
/// their access tokens and pending authorization codes. Returns the counts, then the ids of the tokens
/// and grants so the credential cache can forget them.
pub const REVOKE_ACCESS: ControlStmt = ControlStmt::at(
    "org_admin.revoke_access",
    "BEGIN TRANSACTION; \
     LET $grants = (SELECT VALUE id FROM oauth_grant WHERE owner = $user); \
     LET $tokens = (SELECT VALUE id FROM api_token WHERE owner = $user); \
     LET $sessions = (UPDATE session SET revoked = true WHERE owner = $user AND revoked = false RETURN VALUE id); \
     DELETE oauth_token WHERE family IN $grants; \
     DELETE oauth_grant WHERE owner = $user; \
     DELETE oauth_code WHERE owner = $user; \
     DELETE api_token WHERE owner = $user; \
     RETURN { sessions: array::len($sessions), tokens: $tokens, grants: $grants }; \
     COMMIT TRANSACTION;",
    8, // BEGIN 0, LET 1, LET 2, LET 3, DELETE 4, 5, 6, 7, RETURN 8
);
