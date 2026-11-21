//! Getting back into an account: a reset link by email, and changing a password while signed in.
//! The messages sent by email (reset link, organisation invitation) are built here too.
//!
//! - **Forgot** ([`request_reset`]). Always answers the same whether or not the account exists. An
//!   account gets a link only when it has a password (a single sign-on account has none to reset) and is
//!   not suspended. The same database statement runs for an unknown address, the mail is sent from a
//!   task of its own, and the audit row is written either way, so neither the answer nor its timing
//!   says whether the account exists. Limits: per client address (a 429, which says nothing about any
//!   account) and per target address (past it no link is sent and the answer is unchanged); and a link
//!   is not issued within a minute of the last one (`user.reset_sent_at`), on any replica.
//! - **Reset** ([`reset_password`]). The link's token is 32 random bytes; only its sha256 is stored, on the
//!   account (`user.reset_token_hash`), so there is one open link per account and asking again replaces it.
//!   It expires after [`RESET_TTL_MINUTES`] and is spent by the transaction that sets the password. The
//!   token rides in the link's fragment, so no server or proxy log ever sees it, and it is in no log,
//!   audit row or error message.
//! - **What a reset ends.** Every browser session of the account, and its pending sign-in steps. It does
//!   not start a session (the person signs in with the new password), does not touch two-step
//!   verification (the next sign-in still asks for the second factor; enrolment is neither removed nor
//!   bypassed), and leaves API tokens and app connections alone. That is what the closest cases do:
//!   linking a single sign-on identity to an existing account (`routers/oidc.rs`) and suspending an
//!   account (`routers/admin.rs`) end sessions and nothing else, and tokens are revoked one at a time, by
//!   their owner or an admin, each with an audit row. Revoking every token on every reset would also
//!   break every connected agent each time someone forgets a password.
//! - **Change** ([`change_password`]). A browser session proves the current password (counted like the
//!   password asked again to set up two-step verification: 10 wrong in 15 minutes and it is refused, on
//!   every replica), and an account with two-step verification passes the same step-up as for a new API
//!   token. Every other session ends; this one stays.

use serde::Deserialize;
use surrealdb::types::{RecordId, SurrealValue};

use crate::audit::{self, Event};
use crate::authz;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::mail::Mail;
use crate::models_user;
use crate::state::AppState;
use crate::store::recovery as q;

/// Minutes a reset link works for.
pub const RESET_TTL_MINUTES: i64 = 30;
/// Reset links one client address may ask for in 15 minutes (counted per process, like login failures).
pub const FORGOT_PER_ADDRESS: u32 = 10;
/// Reset links one email address may be asked for in 15 minutes. Past it nothing is sent and the answer
/// does not change.
pub const FORGOT_PER_EMAIL: u32 = 3;
/// Reset links that did not check out (unknown, used or expired) per client address in 15 minutes.
pub const RESET_TOKEN_FAILS: u32 = 10;

/// The longest token the reset route looks up (a real one is 43 characters).
const MAX_TOKEN_LEN: usize = 128;

/// `<origin>/reset-password#<token>`. The origin is `PUBLIC_URL`, which is the address people open in a
/// browser (docs/deployment.md); the token is in the fragment.
fn reset_link(state: &AppState, token: &str) -> String {
    format!("{}/reset-password#{token}", state.settings.public_url)
}

fn rate_limited(wait: u64, text: &str) -> AppError {
    let mut e = AppError::coded(ErrorCode::RateLimited, text);
    e.retry_after_secs = Some(wait);
    e
}

fn invalid() -> AppError {
    AppError::coded(ErrorCode::AuthResetInvalid, "This password reset link isn't valid any more. It may have been used, or a newer one was sent. Ask for a new one.")
}

fn expired() -> AppError {
    AppError::coded(ErrorCode::AuthResetExpired, format!("This password reset link has expired (they last {RESET_TTL_MINUTES} minutes). Ask for a new one."))
}

/// Asks for a reset link for `email`. `ip` is the client address. Errors only for something about the
/// request itself (email off, a malformed address, too many requests from this address); never for an
/// account that is unknown, has no password, is suspended or was asked for too often.
pub async fn request_reset(state: &AppState, ip: &str, raw_email: &str) -> AppResult<()> {
    state.mailer.require()?;
    let email = models_user::normalize_email(raw_email);
    models_user::validate_email(&email)?;
    state
        .fail_throttle
        .reserve(&format!("forgot-ip:{ip}"), FORGOT_PER_ADDRESS)
        .map_err(|wait| rate_limited(wait, "Too many password reset requests from this address. Wait a while, then try again."))?;
    let allowed = state.fail_throttle.reserve(&format!("forgot-email:{email}"), FORGOT_PER_EMAIL).is_ok();

    #[derive(Deserialize, SurrealValue)]
    struct Issued {
        id: RecordId,
        email: String,
    }
    let token = models_user::generate_token();
    let issued: Option<Issued> = if allowed {
        let mut res = q::FORGOT_ISSUE.on(&state.control).bind(("email", email.clone())).bind(("hash", models_user::hash_token(&token))).await?.check()?;
        res.take::<Vec<Issued>>(0)?.into_iter().next()
    } else {
        None
    };
    let why = match (&issued, allowed) {
        (Some(_), _) => "sent",
        (None, false) => "too_often",
        (None, true) => "no_link",
    };
    // the same row, the same work, for every request; the account is named only when there is one
    let event = Event { user: issued.as_ref().map(|i| &i.id), actor: &audit::anonymous(), action: "auth.password_forgot", target: "", outcome: "ok", detail: &format!("email={email} link={why}") };
    audit::record(&state.control, event).await;
    if let Some(i) = issued {
        let link = reset_link(state, &token);
        state.mailer.send_in_background("password_reset", reset_mail(&i.email, &link));
    }
    Ok(())
}

fn reset_mail(to: &str, link: &str) -> Mail {
    Mail {
        to: to.to_string(),
        subject: "Reset your Eunomia password".into(),
        body: format!(
            "Someone asked to reset the password for your Eunomia account.\n\n\
             To choose a new password, open this link within {RESET_TTL_MINUTES} minutes:\n\n{link}\n\n\
             The link works once. Using it signs you out everywhere, and your two-step verification (if you use it) still applies when you sign in again.\n\n\
             If you didn't ask for this, you can ignore this email: your password hasn't changed and nobody can use the link without it.\n"
        ),
    }
}

/// The email an organisation invitation goes out in. `link` is the same link the admin can copy.
pub fn invitation_mail(to: &str, invited_by: &str, link: &str, days: i64) -> Mail {
    Mail {
        to: to.to_string(),
        subject: "You've been invited to Eunomia".into(),
        body: format!(
            "{invited_by} invited you to join their workspace on Eunomia, a shared memory for people and their AI tools.\n\n\
             To accept, open this link and sign in or create your account with this email address ({to}):\n\n{link}\n\n\
             The link works once and expires in {days} days.\n\n\
             If you weren't expecting this, you can ignore this email.\n"
        ),
    }
}

/// The email a new workspace's owner claim goes out in (made through the operator API). `link` is the link
/// the billing service also receives.
pub fn owner_claim_mail(to: &str, org_name: &str, link: &str, days: i64) -> Mail {
    Mail {
        to: to.to_string(),
        subject: "Your Eunomia workspace is ready".into(),
        body: format!(
            "Your Eunomia workspace \u{201c}{org_name}\u{201d} is ready, and you're its owner.\n\n\
             To set it up, open this link and sign in or create your account with this email address ({to}):\n\n{link}\n\n\
             The link works once and expires in {days} days. If it expires, ask whoever set up the workspace for a new one.\n\n\
             If you weren't expecting this, you can ignore this email.\n"
        ),
    }
}

/// Sets a new password with a reset link's `token`. `ip` is the client address. Ends every session of the
/// account; starts none.
pub async fn reset_password(state: &AppState, ip: &str, token: &str, new_password: &str) -> AppResult<()> {
    let fail_key = format!("reset-fail:{ip}");
    state.fail_throttle.check(&fail_key, RESET_TOKEN_FAILS).map_err(|wait| rate_limited(wait, "Too many reset links that didn't work. Wait a while, then try again."))?;
    // a weak password is refused before the link is looked at, so the link is not spent on it
    models_user::validate_password(new_password)?;
    let token = token.trim();
    if token.is_empty() || token.len() > MAX_TOKEN_LEN {
        state.fail_throttle.fail(&fail_key);
        return Err(invalid());
    }
    let hash = models_user::hash_token(token);

    #[derive(Deserialize, SurrealValue)]
    struct Peek {
        id: RecordId,
        #[serde(default)]
        #[surreal(default)]
        expired: bool,
    }
    let peek: Option<Peek> = q::RESET_PEEK.on(&state.control).bind(("hash", hash.clone())).await?.take::<Vec<Peek>>(0)?.into_iter().next();
    let Some(peek) = peek else {
        state.fail_throttle.fail(&fail_key);
        return Err(invalid());
    };
    if peek.expired {
        state.fail_throttle.fail(&fail_key);
        record(state, &peek.id, "auth.password_reset", crate::error::ErrorCode::AuthResetExpired.as_str(), "").await;
        return Err(expired());
    }
    let password_hash = models_user::hash_password(new_password).await?;
    let applied = crate::tx::with_retry(|| async {
        q::RESET_APPLY.on(&state.control).bind(("hash", hash.clone())).bind(("password_hash", password_hash.clone())).await?.check()
    })
    .await;
    let mut res = match applied {
        Ok(r) => r,
        Err(e) if e.to_string().contains("recovery.unavailable") => {
            state.fail_throttle.fail(&fail_key);
            return Err(invalid());
        }
        Err(e) => return Err(e.into()),
    };
    #[derive(Deserialize, SurrealValue)]
    struct Done {
        id: RecordId,
    }
    let done: Done = res.take::<Vec<Done>>(q::RESET_APPLY.slot)?.into_iter().next().ok_or_else(|| AppError::internal("reset returned no account"))?;
    state.credentials.forget_user(&done.id);
    state.fail_throttle.clear(&fail_key);
    record(state, &done.id, "auth.password_reset", "ok", "sessions ended; two-step verification, API tokens and app connections unchanged").await;
    Ok(())
}

/// Changes the signed-in account's password. `sid` is the session making the change: it stays signed in.
pub async fn change_password(state: &AppState, user: &models_user::User, headers: &axum::http::HeaderMap, current: &str, new_password: &str, mfa_code: Option<&str>) -> AppResult<()> {
    models_user::validate_password(new_password)?;
    let status = crate::mfa::status(&state.control, &user.id).await?;
    if !status.available {
        return Err(AppError::bad_request("This account signs in through single sign-on, so it has no password to change."));
    }
    // a stolen session alone must not change the password: an account with two-step verification passes it again
    crate::mfa::step_up(state, user, headers, mfa_code).await?;
    // the current password, counted before it is checked (database-backed, every replica)
    crate::mfa::reserve_reauth(state, &user.id).await?;
    let Some(old_hash) = models_user::password_hash_of(&state.control, &user.id).await? else {
        return Err(AppError::bad_request("This account signs in through single sign-on, so it has no password to change."));
    };
    if current.is_empty() || !models_user::verify_password(current, Some(&old_hash)).await {
        record_as_caller(state, user, "auth.password_change", ErrorCode::AuthForbidden.as_str(), "where=settings").await;
        // 403, not 401: the session is fine, only the password typed again was wrong
        return Err(AppError::coded(ErrorCode::AuthForbidden, "That password isn't right."));
    }
    crate::mfa::reauth_passed(state, &user.id).await?;
    if current == new_password {
        return Err(AppError::bad_request("Choose a password you aren't using now."));
    }
    let sid = crate::auth::session_sid(&state.settings.jwt_secret, headers).unwrap_or_default();
    let password_hash = models_user::hash_password(new_password).await?;
    let applied = crate::tx::with_retry(|| async {
        q::CHANGE_APPLY
            .on(&state.control)
            .bind(("id", user.id.clone()))
            .bind(("old_hash", old_hash.clone()))
            .bind(("password_hash", password_hash.clone()))
            .bind(("sid", sid.clone()))
            .await?
            .check()
    })
    .await;
    match applied {
        Ok(_) => {}
        // the password changed under this request (a reset, or another change): nothing was changed
        Err(e) if e.to_string().contains("recovery.unavailable") => {
            return Err(AppError::coded(ErrorCode::DbConflict, "Your password changed while this request ran. Try again."));
        }
        Err(e) => return Err(e.into()),
    }
    state.credentials.forget_user(&user.id);
    record_as_caller(state, user, "auth.password_change", "ok", "other sessions ended").await;
    Ok(())
}

/// A row for `user` with nobody signed in (a reset link's holder).
async fn record(state: &AppState, user: &RecordId, action: &str, outcome: &str, detail: &str) {
    audit::record(&state.control, Event { user: Some(user), actor: &audit::anonymous(), action, target: "", outcome, detail }).await;
}

async fn record_as_caller(state: &AppState, user: &models_user::User, action: &str, outcome: &str, detail: &str) {
    let actor = authz::current().map(|c| c.actor).unwrap_or(authz::Actor { kind: "user", id: crate::rid::RecordIdExt::to_string(&user.id) });
    audit::record(&state.control, Event { user: Some(&user.id), actor: &actor, action, target: "", outcome, detail }).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_messages_carry_the_link_and_say_what_to_expect() {
        let m = reset_mail("pat@example.com", "https://eunomia.example/reset-password#abc");
        assert_eq!(m.to, "pat@example.com");
        assert!(m.body.contains("https://eunomia.example/reset-password#abc"));
        assert!(m.body.contains("30 minutes"));
        let i = invitation_mail("pat@example.com", "kate@acme.test", "https://eunomia.example/invite#xyz", 7);
        assert!(i.body.contains("kate@acme.test") && i.body.contains("https://eunomia.example/invite#xyz") && i.body.contains("7 days"));
    }
}
