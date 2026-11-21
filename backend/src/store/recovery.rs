//! Statements for password recovery (`user.reset_*`, control migration 0013) and for changing a
//! password. See `recovery.rs`. Nothing here uses a function outside the hardened server's
//! `--allow-funcs` list (`time::now()` and a duration literal only).

use super::ControlStmt;

pub const CONTROL_ALL: &[&ControlStmt] = &[&FORGOT_ISSUE, &RESET_PEEK, &RESET_APPLY, &CHANGE_APPLY];

/// Opens a reset link for the account with this email, if it may have one: it has a password (a
/// single sign-on account has none to reset), it is not suspended, and no link was sent in the last
/// minute. Asking again replaces the open link. The same statement runs for an unknown address (it just
/// matches nothing), so the work done does not say whether the account exists.
pub const FORGOT_ISSUE: ControlStmt = ControlStmt::new(
    "recovery.forgot_issue",
    "UPDATE user SET reset_token_hash = $hash, reset_expires_at = time::now() + 30m, reset_sent_at = time::now() \
     WHERE email_lc = $email AND password_hash IS NOT NONE AND suspended_at IS NONE \
     AND (reset_sent_at IS NONE OR reset_sent_at < time::now() - 1m) RETURN id, email",
);

/// What a reset link is, before the new password is hashed: its account, and whether it has expired.
pub const RESET_PEEK: ControlStmt = ControlStmt::new(
    "recovery.reset_peek",
    "SELECT id, reset_expires_at <= time::now() AS expired FROM user WHERE reset_token_hash = $hash LIMIT 1",
);

/// Spends a reset link and sets the new password, in one transaction: the link is single use (a second
/// use finds no link), unexpired, and still for an account with a password that is not suspended. Every
/// browser session of the account ends and its pending sign-in steps are dropped. Two-step verification
/// is not touched: the next sign-in still asks for the second factor. API tokens and app connections are
/// left as they are (what single sign-on linking and suspension do too). A lost race is a commit
/// conflict the caller retries into `recovery.unavailable`.
pub const RESET_APPLY: ControlStmt = ControlStmt::at(
    "recovery.reset_apply",
    "BEGIN TRANSACTION; \
     LET $u = (UPDATE user SET password_hash = $password_hash, reset_token_hash = NONE, reset_expires_at = NONE, reset_sent_at = NONE \
        WHERE reset_token_hash = $hash AND reset_expires_at > time::now() AND password_hash IS NOT NONE AND suspended_at IS NONE \
        RETURN id, email); \
     IF array::len($u) = 0 { THROW 'recovery.unavailable' }; \
     UPDATE session SET revoked = true WHERE owner = $u[0].id AND revoked = false RETURN NONE; \
     DELETE mfa_challenge WHERE user = $u[0].id RETURN NONE; \
     RETURN $u[0]; \
     COMMIT TRANSACTION;",
    // BEGIN 0, LET 1, IF 2, UPDATE session 3, DELETE 4, RETURN 5
    5,
);

/// A signed-in account changes its password: only while the hash is still the one the current password
/// was checked against, so two changes cannot both win. Every other browser session ends (`$sid` is
/// the one making the change), an open reset link dies with the old password, and pending sign-in
/// steps are dropped. Returns the account id, or nothing when the password changed meanwhile.
pub const CHANGE_APPLY: ControlStmt = ControlStmt::at(
    "recovery.change_apply",
    "BEGIN TRANSACTION; \
     LET $u = (UPDATE user SET password_hash = $password_hash, reset_token_hash = NONE, reset_expires_at = NONE, reset_sent_at = NONE \
        WHERE id = $id AND password_hash = $old_hash RETURN id); \
     IF array::len($u) = 0 { THROW 'recovery.unavailable' }; \
     UPDATE session SET revoked = true WHERE owner = $id AND sid != $sid AND revoked = false RETURN NONE; \
     DELETE mfa_challenge WHERE user = $id RETURN NONE; \
     RETURN $u[0].id; \
     COMMIT TRANSACTION;",
    5,
);
