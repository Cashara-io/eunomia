//! Two-step verification for password accounts: TOTP (RFC 6238, through the `totp-rs` crate), single-use
//! recovery codes, the short-lived sign-in challenge between the password and the second step, step-up for
//! sensitive actions, and the instance-wide "require MFA" switch. See docs/security/mfa.md.
//!
//! - The TOTP secret is stored AES-GCM encrypted under `ENCRYPTION_KEY` (the credential encryption in
//!   `connectors/crypto.rs`) and is returned exactly once, while enrolling. A secret only counts after a
//!   valid code confirmed it.
//! - A TOTP step is accepted once per account (`mfa_last_step`), so a code cannot be replayed in its window.
//! - Recovery codes are random, stored as SHA-256 only, and spent atomically.
//! - A password that checks out yields a [`Challenge`], not a session: an HttpOnly cookie scoped to
//!   `/api/auth/mfa` that can only finish the second step (or enrol, when MFA is required). It expires after
//!   [`CHALLENGE_TTL_SECS`] and is deleted after [`CHALLENGE_MAX_FAILS`] wrong codes. Every code also takes
//!   one of the account's [`MFA_FAILS`] attempts per 15 minutes. Both are reserved in the database before
//!   the code is checked ([`verify_attempt`]), so parallel requests cannot exceed them.

use axum::{extract::Extension, http::{header, HeaderMap}};
use rand::Rng;
use serde::{Deserialize, Serialize};
use surrealdb::types::{Datetime, RecordId, SurrealValue};
use totp_rs::{Builder, Secret, Totp};

use crate::audit::{self, Event};
use crate::authz::Actor;
use crate::connectors::crypto;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::gate::ForwardedHttps;
use crate::models_user::{self, User};
use crate::pool::ControlDb;
use crate::state::AppState;
use crate::store::control as stmt;

/// The browser's half of a pending sign-in.
pub const MFA_COOKIE: &str = "eunomia_mfa";
/// How long a password sign-in waits for its second step.
pub const CHALLENGE_TTL_SECS: i64 = 300;
/// Wrong codes one challenge takes before it is deleted (the person signs in again).
pub const CHALLENGE_MAX_FAILS: i64 = 5;
/// Codes per account in a 15-minute window, across challenges, Settings and step-up, before every attempt
/// is refused for the rest of the window. A right code starts the window over.
pub const MFA_FAILS: u32 = 10;
/// A sensitive action within this long of passing a second factor needs no new code.
pub const STEP_UP_SECS: i64 = 600;
/// A secret shown during enrolment must be confirmed within this long.
pub const ENROL_TTL_SECS: i64 = 900;
/// Recovery codes made at enrolment and on every regeneration.
pub const RECOVERY_CODES: usize = 10;
const ISSUER: &str = "Eunomia";
/// 160 bits, what RFC 4226 recommends.
const SECRET_BYTES: usize = 20;
/// Crockford-style base32 without i, l, o, u: easy to read back and type.
const RECOVERY_ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";

// ------------------------------------------------------------------ TOTP

fn internal(e: impl std::fmt::Display) -> AppError {
    AppError::internal(e.to_string())
}

/// A new random TOTP secret, base32 (what authenticator apps take by hand).
pub fn new_secret() -> String {
    let mut bytes = [0u8; SECRET_BYTES];
    rand::thread_rng().fill(&mut bytes);
    Secret::from(bytes.to_vec()).to_base32()
}

fn totp(secret_b32: &str, account: &str) -> AppResult<Totp> {
    let secret = Secret::try_from_base32(secret_b32).map_err(|e| internal(format!("{e:?}")))?;
    // the label is `Issuer:account`, so a colon in the account would split it
    let account: String = account.replace(':', "_");
    Builder::new().with_secret(secret).with_issuer(Some(ISSUER)).with_account_name(account).build().map_err(|e| internal(format!("{e:?}")))
}

fn now_secs() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp()).unwrap_or(0)
}

/// Spaces and dashes people type between digit groups.
fn squeeze(code: &str) -> String {
    code.chars().filter(|c| !c.is_whitespace() && *c != '-').collect()
}

/// True for something shaped like an authenticator code (six digits).
pub fn is_totp_shaped(code: &str) -> bool {
    let c = squeeze(code);
    c.len() == 6 && c.bytes().all(|b| b.is_ascii_digit())
}

/// The step `code` is valid for at `time` (one step of skew either way), if any. Constant-time compare
/// (`totp_rs::Token`'s `PartialEq`).
pub fn check_code(secret_b32: &str, code: &str, time: u64) -> AppResult<Option<u64>> {
    let code = squeeze(code);
    if !is_totp_shaped(&code) {
        return Ok(None);
    }
    Ok(totp(secret_b32, "x")?.check(&code, time))
}

/// The code an authenticator shows at `time` (tests and the enrolment walkthrough in docs).
pub fn code_at(secret_b32: &str, time: u64) -> AppResult<String> {
    Ok(totp(secret_b32, "x")?.generate(time).to_string())
}

/// What the enrolment screen shows, once.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Enrolment {
    /// The secret in base32, for typing into an authenticator app by hand. Shown only now.
    pub secret: String,
    /// `otpauth://totp/...` URI the QR code encodes.
    pub otpauth_uri: String,
    /// The QR code as a standalone SVG document (no scripts), for an `<img>` tag.
    pub qr_svg: String,
}

fn enrolment(secret_b32: &str, email: &str) -> AppResult<Enrolment> {
    let uri = totp(secret_b32, email)?.to_url().map_err(|e| internal(format!("{e:?}")))?;
    let qr = qrcode::QrCode::new(uri.as_bytes()).map_err(internal)?;
    let svg = qr.render::<qrcode::render::svg::Color>().min_dimensions(200, 200).quiet_zone(true).build();
    Ok(Enrolment { secret: secret_b32.to_string(), otpauth_uri: uri, qr_svg: svg })
}

// ------------------------------------------------------------------ recovery codes

/// `RECOVERY_CODES` fresh codes, `xxxxx-xxxxx` (50 random bits each).
pub fn new_recovery_codes() -> Vec<String> {
    let mut rng = rand::thread_rng();
    (0..RECOVERY_CODES)
        .map(|_| {
            let s: String = (0..10).map(|_| RECOVERY_ALPHABET[rng.gen_range(0..RECOVERY_ALPHABET.len())] as char).collect();
            format!("{}-{}", &s[..5], &s[5..])
        })
        .collect()
}

/// The canonical form of a typed recovery code: lowercase, no separators, look-alikes folded.
pub fn normalize_recovery(code: &str) -> String {
    squeeze(code)
        .to_ascii_lowercase()
        .chars()
        .map(|c| match c {
            'o' => '0',
            'i' | 'l' => '1',
            c => c,
        })
        .collect()
}

pub fn hash_recovery(code: &str) -> String {
    models_user::hash_token(&normalize_recovery(code))
}

// ------------------------------------------------------------------ account state

#[derive(Debug, Deserialize, SurrealValue)]
struct StateRow {
    email: String,
    #[serde(default)]
    #[surreal(default)]
    has_password: bool,
    #[serde(default)]
    #[surreal(default)]
    mfa_secret_enc: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    mfa_enabled_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    mfa_pending_enc: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    mfa_pending_at: Option<Datetime>,
}

async fn state_row(control: &ControlDb, user: &RecordId) -> AppResult<StateRow> {
    let row: Option<StateRow> = stmt::MFA_USER_STATE.on(control).bind(("id", user.clone())).await?.take(0)?;
    row.ok_or_else(|| AppError::coded(ErrorCode::AuthNotFound, "Account not found."))
}

/// Whether the account has two-step verification on.
pub async fn enabled(control: &ControlDb, user: &RecordId) -> AppResult<bool> {
    Ok(state_row(control, user).await?.mfa_secret_enc.is_some())
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MfaStatus {
    /// Two-step verification is on for this account.
    pub enabled: bool,
    #[schema(value_type = Option<String>)]
    pub enabled_at: Option<Datetime>,
    /// Unused recovery codes left.
    pub recovery_codes_left: i64,
    /// An admin requires two-step verification for every password account on this server.
    pub required: bool,
    /// False for an account that signs in only through single sign-on: the provider handles its second factor.
    pub available: bool,
    /// True when the stored authenticator can no longer be read (`ENCRYPTION_KEY` changed): sign in with a
    /// recovery code, turn it off and set it up again.
    pub unreadable: bool,
}

pub async fn status(control: &ControlDb, user: &RecordId) -> AppResult<MfaStatus> {
    let row = state_row(control, user).await?;
    Ok(MfaStatus {
        enabled: row.mfa_secret_enc.is_some(),
        enabled_at: row.mfa_enabled_at,
        recovery_codes_left: recovery_left(control, user).await?,
        required: require_mfa(control).await?,
        available: row.has_password,
        unreadable: false,
    })
}

async fn recovery_left(control: &ControlDb, user: &RecordId) -> AppResult<i64> {
    #[derive(Deserialize, SurrealValue)]
    struct Count {
        count: i64,
    }
    let rows: Vec<Count> = stmt::MFA_RECOVERY_LEFT.on(control).bind(("user", user.clone())).await?.take(0)?;
    Ok(rows.first().map_or(0, |c| c.count))
}

// ------------------------------------------------------------------ instance policy

#[derive(Debug, Default, Serialize, Deserialize, SurrealValue, utoipa::ToSchema)]
pub struct Security {
    /// Every password account must pass a second factor before it gets a session.
    #[serde(default)]
    #[surreal(default)]
    pub require_mfa: bool,
    #[serde(default)]
    #[surreal(default)]
    #[schema(value_type = Option<String>)]
    pub updated_at: Option<Datetime>,
    /// The admin who last changed it.
    #[serde(default)]
    #[surreal(default)]
    pub updated_by: Option<String>,
}

pub async fn security(control: &ControlDb) -> AppResult<Security> {
    let row: Option<Security> = stmt::INSTANCE_SECURITY_GET.on(control).await?.take(0)?;
    Ok(row.unwrap_or_default())
}

pub async fn require_mfa(control: &ControlDb) -> AppResult<bool> {
    Ok(security(control).await?.require_mfa)
}

pub async fn set_require_mfa(control: &ControlDb, on: bool, by: &RecordId) -> AppResult<()> {
    stmt::INSTANCE_SECURITY_SET.on(control).bind(("require_mfa", on)).bind(("by", by.clone())).await?.check()?;
    Ok(())
}

/// A session that did not pass a second factor, while MFA is required, is refused. Exempt: single sign-on
/// sessions (`via = oidc`) of accounts without their own authenticator (the provider owns their second
/// factor), and single sign-on accounts' sessions from before sessions recorded how they signed in. An
/// account with its own authenticator passes it on every route, single sign-on included
/// (`routers/oidc.rs`), and its unverified sessions end when it enrols ([`confirm_enrolment`]).
pub async fn session_allowed(control: &ControlDb, user: &RecordId, mfa_at: Option<&Datetime>, via: Option<&str>) -> AppResult<()> {
    if mfa_at.is_some() || !require_mfa(control).await? {
        return Ok(());
    }
    let row = state_row(control, user).await?;
    if (via == Some("oidc") && row.mfa_secret_enc.is_none()) || (!row.has_password && via.is_none()) {
        return Ok(());
    }
    Err(AppError::coded(ErrorCode::AuthMfaRequired, "This server now requires two-step verification. Sign in again to set it up."))
}

// ------------------------------------------------------------------ verifying a code

/// Which second factor a code was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Factor {
    Totp,
    Recovery,
}

impl Factor {
    pub fn words(self) -> &'static str {
        match self {
            Factor::Totp => "authenticator code",
            Factor::Recovery => "recovery code",
        }
    }
}

/// The stored secret no longer decrypts (`ENCRYPTION_KEY` changed without moving it). Not the person's
/// mistake and not a 500: they sign in with a recovery code and set the authenticator up again.
fn unreadable() -> AppError {
    AppError::coded(
        ErrorCode::AuthMfaUnreadable,
        "Your authenticator can't be checked because this server's encryption key changed. Sign in with a recovery code, then set up two-step verification again in Settings.",
    )
}

/// Decrypts a stored TOTP secret; a value the current key cannot read is [`unreadable`].
fn open_secret(state: &AppState, enc: &str) -> AppResult<String> {
    crypto::decrypt(&state.settings.encryption_key, enc).map_err(|e| {
        tracing::warn!(error = %e.message, "a stored TOTP secret does not decrypt with the current ENCRYPTION_KEY");
        unreadable()
    })
}

/// True when the account's confirmed secret cannot be decrypted with the current key.
pub async fn secret_unreadable(state: &AppState, user: &RecordId) -> AppResult<bool> {
    Ok(state_row(&state.control, user).await?.mfa_secret_enc.is_some_and(|enc| open_secret(state, &enc).is_err()))
}

/// Checks `code` (an authenticator code or a recovery code) for the account and spends it. `None`: wrong,
/// replayed, or the account has no MFA.
async fn spend(state: &AppState, user: &RecordId, code: &str) -> AppResult<Option<Factor>> {
    let row = state_row(&state.control, user).await?;
    let Some(enc) = row.mfa_secret_enc else { return Ok(None) };
    if is_totp_shaped(code) {
        let secret = open_secret(state, &enc)?;
        let Some(step) = check_code(&secret, code, now_secs())? else { return Ok(None) };
        let accepted: Vec<serde_json::Value> = stmt::MFA_STEP_ACCEPT
            .on(&state.control)
            .bind(("id", user.clone()))
            .bind(("step", i64::try_from(step).unwrap_or(i64::MAX)))
            .await?
            .take(0)?;
        // empty: this step (or a later one) was already used, so this is a replay
        return Ok((!accepted.is_empty()).then_some(Factor::Totp));
    }
    let hash = hash_recovery(code);
    let spent: Vec<serde_json::Value> =
        stmt::MFA_RECOVERY_SPEND.on(&state.control).bind(("user", user.clone())).bind(("hash", hash)).await?.take(0)?;
    Ok((!spent.is_empty()).then_some(Factor::Recovery))
}

fn invalid() -> AppError {
    AppError::coded(ErrorCode::AuthMfaInvalid, "That code didn't work. Check your authenticator app (or the recovery code) and try again.")
}

fn locked_out(wait_secs: i64) -> AppError {
    let minutes = (wait_secs.max(1) + 59) / 60;
    AppError::coded(ErrorCode::RateLimited, format!("Too many codes tried. Try again in {minutes} minute{}.", if minutes == 1 { "" } else { "s" }))
}

/// Takes one of the account's [`MFA_FAILS`] attempts for the current 15-minute window, before any code is
/// checked: a conditional UPDATE in the control database, so parallel requests and other replicas share
/// one budget. Refused (`rate.limited`) once the window's attempts are used.
async fn reserve_attempt(state: &AppState, user: &RecordId) -> AppResult<()> {
    let taken: Vec<serde_json::Value> =
        stmt::MFA_ATTEMPT_RESERVE.on(&state.control).bind(("id", user.clone())).bind(("max", i64::from(MFA_FAILS))).await?.take(0)?;
    if !taken.is_empty() {
        return Ok(());
    }
    let since: Option<Datetime> = stmt::MFA_ATTEMPT_WAIT.on(&state.control).bind(("id", user.clone())).await?.take(0)?;
    let ends = since.as_ref().and_then(crate::sources::base::datetime_to_chrono).map(|t| t + chrono::Duration::minutes(15));
    Err(locked_out(ends.map_or(60, |e| (e - chrono::Utc::now()).num_seconds())))
}

/// Takes one of the challenge's [`CHALLENGE_MAX_FAILS`] attempts before its code is checked. `None` when
/// it is used up, expired or already finished; otherwise how many it has used, this one included.
async fn reserve_challenge(control: &ControlDb, c: &Challenge) -> AppResult<Option<i64>> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        failures: i64,
    }
    let rows: Vec<Row> = stmt::MFA_CHALLENGE_RESERVE.on(control).bind(("id", c.id.clone())).bind(("max", CHALLENGE_MAX_FAILS)).await?.take(0)?;
    Ok(rows.first().map(|r| r.failures))
}

fn challenge_over() -> AppError {
    AppError::coded(ErrorCode::AuthMfaChallengeExpired, "Too many wrong codes. Enter your email and password again.")
}

/// One second-factor attempt, in this order: hold the account's lock (one attempt at a time per account in
/// this process), reserve an account attempt, reserve a challenge attempt (sign-in only), then check and
/// spend the code. A right code resets the account's window; a wrong one stays counted. A challenge whose
/// last attempt was wrong is deleted. Audited: `auth.mfa_failed`, `auth.mfa_recovery_used`,
/// `auth.mfa_secret_unreadable`. `place` says where the code was typed, for the log.
pub async fn verify_attempt(state: &AppState, user: &RecordId, code: &str, place: &str, challenge: Option<&Challenge>) -> AppResult<Factor> {
    let _one_at_a_time = crate::tx::lock(&format!("mfa:{}", crate::rid::RecordIdExt::to_string(user))).await;
    reserve_attempt(state, user).await?;
    let mut used = None;
    if let Some(c) = challenge {
        match reserve_challenge(&state.control, c).await? {
            Some(n) => used = Some(n),
            None => {
                stmt::MFA_ATTEMPT_REFUND.on(&state.control).bind(("id", user.clone())).await?.check()?;
                return Err(AppError::coded(ErrorCode::AuthMfaChallengeExpired, "Your sign-in has expired. Enter your email and password again."));
            }
        }
    }
    let actor = Actor { kind: "user", id: crate::rid::RecordIdExt::to_string(user) };
    match spend(state, user, code).await {
        Ok(Some(factor)) => {
            stmt::MFA_ATTEMPT_RESET.on(&state.control).bind(("id", user.clone())).await?.check()?;
            // spent while the account's lock is held: a racing answer on the same challenge finds it gone
            // before it can spend a code of its own
            if let Some(c) = challenge {
                stmt::MFA_CHALLENGE_TAKE.on(&state.control).bind(("id", c.id.clone())).await?.check()?;
            }
            if factor == Factor::Recovery {
                let left = recovery_left(&state.control, user).await.unwrap_or(0);
                let detail = format!("where={place} left={left}");
                audit::record(&state.control, Event { user: Some(user), actor: &actor, action: "auth.mfa_recovery_used", target: "", outcome: "ok", detail: &detail }).await;
            }
            Ok(factor)
        }
        Ok(None) => {
            let detail = format!("where={place}");
            let outcome = ErrorCode::AuthMfaInvalid.as_str();
            audit::record(&state.control, Event { user: Some(user), actor: &actor, action: "auth.mfa_failed", target: "", outcome, detail: &detail }).await;
            if let (Some(c), Some(n)) = (challenge, used)
                && n >= CHALLENGE_MAX_FAILS
            {
                stmt::MFA_CHALLENGE_TAKE.on(&state.control).bind(("id", c.id.clone())).await?.check()?;
                return Err(challenge_over());
            }
            Err(invalid())
        }
        Err(e) if e.code == ErrorCode::AuthMfaUnreadable => {
            // no code was checked: the attempts are given back
            stmt::MFA_ATTEMPT_REFUND.on(&state.control).bind(("id", user.clone())).await?.check()?;
            if let Some(c) = challenge {
                stmt::MFA_CHALLENGE_REFUND.on(&state.control).bind(("id", c.id.clone())).await?.check()?;
            }
            let detail = format!("where={place}");
            audit::record(&state.control, Event { user: Some(user), actor: &actor, action: "auth.mfa_secret_unreadable", target: "", outcome: e.code.as_str(), detail: &detail }).await;
            Err(e)
        }
        Err(e) => Err(e),
    }
}

/// [`verify_attempt`] outside a sign-in challenge (Settings, step-up).
pub async fn verify(state: &AppState, user: &RecordId, code: &str, place: &str) -> AppResult<Factor> {
    verify_attempt(state, user, code, place, None).await
}

// ------------------------------------------------------------------ enrolment

/// Starts enrolment: a new secret waits for a confirming code. Refused while MFA is already on (turn it
/// off first) and for an account without a password (its provider owns the second factor).
pub async fn begin_enrolment(state: &AppState, user: &RecordId) -> AppResult<Enrolment> {
    let row = state_row(&state.control, user).await?;
    if !row.has_password {
        return Err(AppError::bad_request("This account signs in through single sign-on. Your identity provider handles two-step verification."));
    }
    if row.mfa_secret_enc.is_some() {
        return Err(AppError::coded(ErrorCode::DbConflict, "Two-step verification is already on. Turn it off first to set up a new authenticator."));
    }
    let secret = new_secret();
    let enc = crypto::encrypt(&state.settings.encryption_key, &secret);
    let set: Vec<serde_json::Value> = stmt::MFA_PENDING_SET.on(&state.control).bind(("id", user.clone())).bind(("enc", enc)).await?.take(0)?;
    if set.is_empty() {
        return Err(AppError::coded(ErrorCode::DbConflict, "Two-step verification is already on."));
    }
    enrolment(&secret, &row.email)
}

/// Confirms enrolment with a code from the new secret: MFA turns on and fresh recovery codes are returned
/// (shown once). The attempt is reserved like any other code ([`verify_attempt`]), and against the
/// sign-in challenge when enrolling during sign-in. `current_sid` (Settings) is marked verified; every
/// other browser session of the account that never passed a second factor is signed out.
pub async fn confirm_enrolment(state: &AppState, user: &RecordId, code: &str, challenge: Option<&Challenge>, current_sid: Option<&str>) -> AppResult<Vec<String>> {
    let _one_at_a_time = crate::tx::lock(&format!("mfa:{}", crate::rid::RecordIdExt::to_string(user))).await;
    let row = state_row(&state.control, user).await?;
    if row.mfa_secret_enc.is_some() {
        return Err(AppError::coded(ErrorCode::DbConflict, "Two-step verification is already on."));
    }
    let fresh = row.mfa_pending_at.as_ref().and_then(crate::sources::base::datetime_to_chrono).is_some_and(|at| chrono::Utc::now() - at < chrono::Duration::seconds(ENROL_TTL_SECS));
    let (Some(enc), true) = (row.mfa_pending_enc, fresh) else {
        return Err(AppError::bad_request("That setup has expired. Start again to get a new QR code."));
    };
    reserve_attempt(state, user).await?;
    let mut used = None;
    if let Some(c) = challenge {
        match reserve_challenge(&state.control, c).await? {
            Some(n) => used = Some(n),
            None => {
                stmt::MFA_ATTEMPT_REFUND.on(&state.control).bind(("id", user.clone())).await?.check()?;
                return Err(AppError::coded(ErrorCode::AuthMfaChallengeExpired, "Your sign-in has expired. Enter your email and password again."));
            }
        }
    }
    let secret = match open_secret(state, &enc) {
        Ok(s) => s,
        Err(e) => {
            stmt::MFA_ATTEMPT_REFUND.on(&state.control).bind(("id", user.clone())).await?.check()?;
            if let Some(c) = challenge {
                stmt::MFA_CHALLENGE_REFUND.on(&state.control).bind(("id", c.id.clone())).await?.check()?;
            }
            return Err(e);
        }
    };
    let Some(step) = check_code(&secret, code, now_secs())? else {
        let actor = Actor { kind: "user", id: crate::rid::RecordIdExt::to_string(user) };
        audit::record(&state.control, Event { user: Some(user), actor: &actor, action: "auth.mfa_failed", target: "", outcome: ErrorCode::AuthMfaInvalid.as_str(), detail: "where=setup" }).await;
        if let (Some(c), Some(n)) = (challenge, used)
            && n >= CHALLENGE_MAX_FAILS
        {
            stmt::MFA_CHALLENGE_TAKE.on(&state.control).bind(("id", c.id.clone())).await?.check()?;
            return Err(challenge_over());
        }
        return Err(invalid());
    };
    let done: Vec<serde_json::Value> = stmt::MFA_CONFIRM
        .on(&state.control)
        .bind(("id", user.clone()))
        .bind(("enc", enc))
        .bind(("step", i64::try_from(step).unwrap_or(i64::MAX)))
        .await?
        .take(0)?;
    if done.is_empty() {
        return Err(AppError::coded(ErrorCode::DbConflict, "Setup changed in another window. Start again."));
    }
    stmt::MFA_ATTEMPT_RESET.on(&state.control).bind(("id", user.clone())).await?.check()?;
    if let Some(c) = challenge {
        stmt::MFA_CHALLENGE_TAKE.on(&state.control).bind(("id", c.id.clone())).await?.check()?;
    }
    if let Some(sid) = current_sid {
        stmt::MFA_SESSION_MARK.on(&state.control).bind(("sid", sid.to_string())).bind(("owner", user.clone())).await?.check()?;
    }
    stmt::MFA_REVOKE_UNVERIFIED_SESSIONS.on(&state.control).bind(("owner", user.clone())).await?.check()?;
    state.credentials.forget_user(user);
    let codes = replace_recovery_codes(&state.control, user).await?;
    audit::record_as_caller(&state.control, user, "auth.mfa_enrol", "", "ok").await;
    Ok(codes)
}

async fn replace_recovery_codes(control: &ControlDb, user: &RecordId) -> AppResult<Vec<String>> {
    let codes = new_recovery_codes();
    let hashes: Vec<String> = codes.iter().map(|c| hash_recovery(c)).collect();
    stmt::MFA_RECOVERY_REPLACE.on(control).bind(("user", user.clone())).bind(("hashes", hashes)).await?.check()?;
    Ok(codes)
}

/// New recovery codes; every old one stops working. The caller has already checked a fresh code.
pub async fn regenerate_recovery_codes(control: &ControlDb, user: &RecordId) -> AppResult<Vec<String>> {
    let codes = replace_recovery_codes(control, user).await?;
    audit::record_as_caller(control, user, "auth.mfa_recovery_regenerate", "", "ok").await;
    Ok(codes)
}

/// Turns MFA off. The caller has already checked a fresh code and the policy.
pub async fn disable(control: &ControlDb, user: &RecordId) -> AppResult<()> {
    stmt::MFA_DISABLE.on(control).bind(("id", user.clone())).await?.check()?;
    audit::record_as_caller(control, user, "auth.mfa_disable", "", "ok").await;
    Ok(())
}

// ------------------------------------------------------------------ the sign-in challenge

/// What a correct password leads to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterPassword {
    /// No second step: start a session.
    Session,
    /// The account has MFA: ask for a code.
    Verify,
    /// MFA is required and the account has none: enrol before any session.
    Enrol,
}

impl AfterPassword {
    pub fn purpose(self) -> &'static str {
        match self {
            AfterPassword::Verify => "verify",
            AfterPassword::Enrol => "enrol",
            AfterPassword::Session => "",
        }
    }
}

pub async fn after_password(control: &ControlDb, user: &RecordId) -> AppResult<AfterPassword> {
    if state_row(control, user).await?.mfa_secret_enc.is_some() {
        Ok(AfterPassword::Verify)
    } else if require_mfa(control).await? {
        Ok(AfterPassword::Enrol)
    } else {
        Ok(AfterPassword::Session)
    }
}

/// A pending second step, found by the browser's cookie.
#[derive(Debug, Deserialize, SurrealValue)]
pub struct Challenge {
    pub id: RecordId,
    pub user: RecordId,
    pub email: String,
    pub purpose: String,
    #[serde(default)]
    #[surreal(default)]
    pub via: String,
    #[serde(default)]
    #[surreal(default)]
    pub user_agent: String,
    #[serde(default)]
    #[surreal(default)]
    pub failures: i64,
}

/// Creates the challenge; returns the token for the cookie (only its hash is stored).
pub async fn create_challenge(control: &ControlDb, user: &RecordId, next: AfterPassword, via: &str, user_agent: Option<&str>) -> AppResult<String> {
    let token = models_user::generate_token();
    let ua: String = user_agent.unwrap_or("").chars().take(300).collect();
    stmt::MFA_CHALLENGE_CREATE
        .on(control)
        .bind(("user", user.clone()))
        .bind(("hash", models_user::hash_token(&token)))
        .bind(("purpose", next.purpose().to_string()))
        .bind(("via", via.to_string()))
        .bind(("user_agent", ua))
        .bind(("ttl", format!("{CHALLENGE_TTL_SECS}s")))
        .await?
        .check()?;
    Ok(token)
}

fn expired() -> AppError {
    AppError::coded(ErrorCode::AuthMfaChallengeExpired, "Your sign-in has expired. Enter your email and password again.")
}

/// The live challenge this browser holds, for `purpose`.
pub async fn find_challenge(control: &ControlDb, headers: &HeaderMap, purpose: &str) -> AppResult<Challenge> {
    let token = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|c| c.split(';').find_map(|p| p.trim().strip_prefix(&format!("{MFA_COOKIE}=")).map(str::to_string)))
        .filter(|t| !t.is_empty())
        .ok_or_else(expired)?;
    let rows: Vec<Challenge> = stmt::MFA_CHALLENGE_FIND.on(control).bind(("hash", models_user::hash_token(&token))).await?.take(0)?;
    rows.into_iter().find(|c| c.purpose == purpose).ok_or_else(expired)
}

pub fn challenge_cookie(token: &str, public_url: &str, https: Option<Extension<ForwardedHttps>>) -> String {
    format!(
        "{MFA_COOKIE}={token}; HttpOnly; SameSite=Strict; Path=/api/auth/mfa; Max-Age={CHALLENGE_TTL_SECS}{}",
        crate::routers::auth::cookie_flags(public_url, https)
    )
}

pub fn clear_challenge_cookie(public_url: &str, https: Option<Extension<ForwardedHttps>>) -> String {
    format!("{MFA_COOKIE}=; HttpOnly; SameSite=Strict; Path=/api/auth/mfa; Max-Age=0{}", crate::routers::auth::cookie_flags(public_url, https))
}

// ------------------------------------------------------------------ step-up

/// A sensitive action (a new API token, changing two-step verification, the admin switch) by a browser
/// session of an account with MFA on needs a second factor from the last [`STEP_UP_SECS`]: the sign-in's
/// own, or `code` now (which then counts for the session). API tokens and app connections are not asked:
/// they cannot answer, and what they may do is bounded by their scopes and expiry.
pub async fn step_up(state: &AppState, user: &User, headers: &HeaderMap, code: Option<&str>) -> AppResult<()> {
    let session = crate::authz::current().is_some_and(|c| c.actor.kind == "user");
    if !session || !enabled(&state.control, &user.id).await? {
        return Ok(());
    }
    let sid = crate::auth::session_sid(&state.settings.jwt_secret, headers);
    if let Some(code) = code.map(str::trim).filter(|c| !c.is_empty()) {
        verify(state, &user.id, code, "step-up").await?;
        if let Some(sid) = sid {
            stmt::MFA_SESSION_MARK.on(&state.control).bind(("sid", sid)).bind(("owner", user.id.clone())).await?.check()?;
        }
        return Ok(());
    }
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        #[serde(default)]
        #[surreal(default)]
        mfa_at: Option<Datetime>,
    }
    if let Some(sid) = sid {
        let rows: Vec<Row> = stmt::MFA_SESSION_BY_SID.on(&state.control).bind(("sid", sid)).bind(("owner", user.id.clone())).await?.take(0)?;
        let fresh = rows
            .first()
            .and_then(|r| r.mfa_at.as_ref())
            .and_then(crate::sources::base::datetime_to_chrono)
            .is_some_and(|at| chrono::Utc::now() - at < chrono::Duration::seconds(STEP_UP_SECS));
        if fresh {
            return Ok(());
        }
    }
    Err(AppError::coded(ErrorCode::AuthStepUpRequired, "Enter a code from your authenticator app to confirm this change."))
}

/// Drops expired challenges (the audit pruner calls this).
pub async fn prune(control: &ControlDb) -> AppResult<()> {
    stmt::MFA_CHALLENGE_PRUNE.on(control).await?.check()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6238_sha1_vectors() {
        // RFC 6238 appendix B, SHA-1, secret "12345678901234567890", last six digits
        let secret = Secret::from(b"12345678901234567890".to_vec()).to_base32();
        for (t, code) in [(59u64, "287082"), (1_111_111_109, "081804"), (1_234_567_890, "005924"), (2_000_000_000, "279037")] {
            assert_eq!(code_at(&secret, t).unwrap(), code, "t={t}");
            assert_eq!(check_code(&secret, code, t).unwrap(), Some(t / 30));
        }
    }

    #[test]
    fn a_code_is_good_for_one_step_either_side_and_no_more() {
        let s = new_secret();
        let t = 1_700_000_000;
        let code = code_at(&s, t).unwrap();
        assert!(check_code(&s, &code, t + 30).unwrap().is_some());
        assert!(check_code(&s, &code, t - 30).unwrap().is_some());
        assert!(check_code(&s, &code, t + 90).unwrap().is_none());
        assert!(check_code(&s, "12 34 56", t).unwrap().is_none() || code == "123456");
        assert!(check_code(&s, "abcdef", t).unwrap().is_none());
    }

    #[test]
    fn secrets_are_160_bits_and_differ() {
        let (a, b) = (new_secret(), new_secret());
        assert_ne!(a, b);
        assert_eq!(Secret::try_from_base32(&a).unwrap().as_ref().len(), SECRET_BYTES);
    }

    #[test]
    fn the_enrolment_uri_names_the_issuer_and_account() {
        let e = enrolment(&new_secret(), "a:b@example.com").unwrap();
        assert!(e.otpauth_uri.starts_with("otpauth://totp/Eunomia:"), "{}", e.otpauth_uri);
        assert!(e.otpauth_uri.contains(&format!("secret={}", e.secret)));
        assert!(e.qr_svg.starts_with("<?xml") && e.qr_svg.contains("<svg") && !e.qr_svg.contains("<script"));
    }

    #[test]
    fn recovery_codes_are_unique_readable_and_normalize() {
        let codes = new_recovery_codes();
        assert_eq!(codes.len(), RECOVERY_CODES);
        let mut sorted = codes.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), RECOVERY_CODES);
        for c in &codes {
            assert_eq!(c.len(), 11);
            assert!(!is_totp_shaped(c), "a recovery code is never mistaken for a TOTP code: {c}");
            assert_eq!(hash_recovery(&c.to_uppercase().replace('-', " ")), hash_recovery(c));
        }
        assert_eq!(normalize_recovery("O1l-iO"), "01110");
    }
}
