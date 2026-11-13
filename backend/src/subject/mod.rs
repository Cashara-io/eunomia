//! Data-subject rights: a full export of what Eunomia holds about one person, and a verified
//! erasure of that person. Design, the inventory of where a person's data lives and what each step
//! does with it: docs/privacy/subject-rights.md.
//!
//! - [`request_export`] queues a `subject_export` job that builds a zip archive (a manifest plus one
//!   file per category and the person's document files) and keeps it for seven days ([`export`]).
//! - [`request_erasure`] checks the account may be erased (no legal hold, not the workspace's last
//!   owner, not the account that set the server up), suspends it at once so nothing new is written,
//!   records an `erasure` row and queues a `subject_erase` job ([`erase`]). The job removes or
//!   anonymises every inventory category, runs a post-check that must find nothing, and writes the
//!   completion record through the audit path. The row keeps only a keyed hash of the user id.
//! - That hash is the tombstone: a sync for an erased account stands down ([`is_erased`]), and after
//!   a backup restore [`reapply_erasures`] (run when a scheduler leader starts, or by an admin)
//!   erases again every restored account whose hash is on the list.

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use crate::config::Settings;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::jobs::{self, NewJob};
use crate::pool::{ControlDb, OrgId};
use crate::rid::RecordIdExt;
use crate::state::AppState;
use crate::store::subject as q;

pub mod erase;
pub mod export;

/// The tombstone reference of an account: HMAC-SHA256 of its record id under `ENCRYPTION_KEY`, hex.
/// Opaque without the key; the same id always gives the same reference, so a restored copy matches.
pub fn subject_ref(settings: &Settings, user: &RecordId) -> String {
    reference(&settings.encryption_key, user)
}

fn reference(key: &str, user: &RecordId) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC takes a key of any length");
    mac.update(b"eunomia-erasure-subject:");
    mac.update(user.to_string().as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// True when `user` has an erasure on record (requested, running or done). A sync for such an
/// account stands down, so a resync cannot bring its records back as theirs.
pub async fn is_erased(control: &ControlDb, settings: &Settings, user: &RecordId) -> bool {
    match q::ERASURE_HAS_REF.on(control).bind(("ref", subject_ref(settings, user))).await.and_then(|mut r| r.take::<Vec<RecordId>>(0)) {
        Ok(rows) => !rows.is_empty(),
        Err(e) => {
            // the table is there from control migration 0011 on; any other failure: do not block syncs on it
            tracing::warn!(error = %e, "erasure tombstone check failed");
            false
        }
    }
}

fn hold_error() -> AppError {
    AppError::coded(ErrorCode::SubjectLegalHold, "This account is under a legal hold, so it can't be erased. An admin can lift the hold in Settings > People.")
}

#[derive(Debug, Deserialize, SurrealValue)]
struct ConfirmRow {
    #[serde(default)]
    #[surreal(default)]
    email: String,
    #[serde(default)]
    #[surreal(default)]
    password_hash: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    legal_hold_at: Option<Datetime>,
}

async fn confirm_row(control: &ControlDb, user: &RecordId) -> AppResult<ConfirmRow> {
    let mut res = q::CONFIRM_ROW.on(control).bind(("u", user.clone())).await?;
    res.take::<Option<ConfirmRow>>(0)?.ok_or_else(|| AppError::coded(ErrorCode::AuthNotFound, "That account doesn't exist."))
}

/// How the person asking proved it is them (self-service) or that they mean it (an admin).
pub enum Confirmation<'a> {
    /// The account's own password, typed again. An account with no password (single sign-on only)
    /// types its email instead.
    SelfService { password: Option<&'a str>, email: Option<&'a str> },
    /// An admin typed the person's email.
    Admin { email: &'a str },
}

/// Checks the confirmation before anything is touched. A wrong one is `subject.confirmation_failed`.
pub async fn confirm(control: &ControlDb, user: &RecordId, how: Confirmation<'_>) -> AppResult<()> {
    let row = confirm_row(control, user).await?;
    let failed = |m: &str| AppError::coded(ErrorCode::SubjectConfirmationFailed, m.to_string());
    let email_matches = |typed: Option<&str>| typed.is_some_and(|t| crate::models_user::normalize_email(t) == crate::models_user::normalize_email(&row.email));
    match how {
        Confirmation::SelfService { password, email } => match row.password_hash.as_deref() {
            Some(hash) => {
                if !crate::models_user::verify_password(password.unwrap_or(""), Some(hash)).await {
                    return Err(failed("That password isn't right. Nothing was deleted."));
                }
            }
            None => {
                if !email_matches(email) {
                    return Err(failed("Type your email address exactly as it appears on your account to confirm. Nothing was deleted."));
                }
            }
        },
        Confirmation::Admin { email } => {
            if !email_matches(Some(email)) {
                return Err(failed("Type the person's email address exactly to confirm. Nothing was deleted."));
            }
        }
    }
    Ok(())
}

/// Every org `user` belongs to, and for each the account that inherits org-level rows the person
/// created (an owner who stays, else the longest-standing member). Refuses with `subject.legal_hold`,
/// `subject.last_owner`, or for the account that set the server up.
pub async fn check_erasable(control: &ControlDb, user: &RecordId) -> AppResult<Vec<(OrgId, RecordId)>> {
    let row = confirm_row(control, user).await?;
    if row.legal_hold_at.is_some() {
        return Err(hold_error());
    }
    if crate::capsules::first_user(control).await?.as_ref() == Some(user) {
        // the oldest account is always an admin (`authz::is_admin`): erasing it would quietly make the next one an admin
        return Err(AppError::coded(
            ErrorCode::SubjectLastOwner,
            "This is the account that set up the server, so it can't be erased in the app. The server's operator can remove it (docs/privacy/subject-rights.md).",
        ));
    }
    let orgs: Vec<RecordId> = q::USER_ORGS.on(control).bind(("u", user.clone())).await?.take(0)?;
    let mut out = Vec::new();
    for org in orgs {
        let Some(id) = crate::rid::key_string(&org.key).and_then(|k| OrgId::parse(&k)) else { continue };
        #[derive(Deserialize, SurrealValue)]
        struct RoleRow {
            #[serde(default)]
            #[surreal(default)]
            role: String,
        }
        let role: Vec<RoleRow> = crate::store::control::MEMBERSHIP_ROLE.on(control).bind(("user", user.clone())).bind(("org", org.clone())).await?.take(0)?;
        let owners: Vec<RecordId> = q::OTHER_ORG_OWNERS.on(control).bind(("u", user.clone())).bind(("org", org.clone())).await?.take(0)?;
        if role.iter().any(|r| r.role == "owner") && owners.is_empty() {
            return Err(AppError::coded(
                ErrorCode::SubjectLastOwner,
                "This account is the workspace's last owner. Make someone else an owner first, so the workspace isn't left without one.",
            ));
        }
        let heir = match owners.into_iter().next() {
            Some(h) => h,
            None => {
                let members: Vec<RecordId> = q::OTHER_ORG_MEMBERS.on(control).bind(("u", user.clone())).bind(("org", org.clone())).await?.take(0)?;
                members.into_iter().next().ok_or_else(|| {
                    AppError::coded(ErrorCode::SubjectLastOwner, "This account is the only one in its workspace, so erasing it would leave the workspace empty.")
                })?
            }
        };
        out.push((id, heir));
    }
    if out.is_empty() {
        return Err(AppError::coded(ErrorCode::TenantNotFound, "This account does not belong to an organisation."));
    }
    Ok(out)
}

/// An erasure as the routes report it.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, utoipa::ToSchema)]
pub struct ErasureTicket {
    pub id: String,
    /// `pending`, `running`, `done` or `failed`.
    pub status: String,
    #[schema(value_type = String)]
    pub requested_at: Datetime,
}

/// Queues the erasure of `user` (`by`: `self`, `admin` or `reapply`), after [`check_erasable`] (the
/// caller has already [`confirm`]ed). The account is suspended and signed out everywhere at once, so
/// nothing new is written while the job runs. A second request while one is open returns that one.
pub async fn request_erasure(state: &AppState, user: &RecordId, by: &'static str) -> AppResult<ErasureTicket> {
    let orgs = check_erasable(&state.control, user).await?;
    enqueue_erasure(state, user, by, &orgs).await
}

async fn enqueue_erasure(state: &AppState, user: &RecordId, by: &'static str, orgs: &[(OrgId, RecordId)]) -> AppResult<ErasureTicket> {
    let reference = subject_ref(&state.settings, user);
    let open: Vec<ErasureTicket> = q::ERASURE_OPEN.on(&state.control).bind(("ref", reference.clone())).await?.take(0)?;
    if let Some(t) = open.into_iter().next() {
        return Ok(t);
    }
    let (home, heir) = orgs.first().cloned().ok_or_else(|| AppError::internal("no org to erase in"))?;
    let ticket: Option<ErasureTicket> = q::ERASURE_CREATE
        .on(&state.control)
        .bind(("org", home.key()))
        .bind(("ref", reference))
        .bind(("u", user.clone()))
        .bind(("by", by))
        .await?
        .take(0)?;
    let ticket = ticket.ok_or_else(|| AppError::internal("erasure row was not created"))?;

    // nothing new is written while it runs: the account is suspended, its sessions end, cached credentials go
    crate::store::control::USER_SET_SUSPENDED.on(&state.control).bind(("id", user.clone())).bind(("suspended", true)).await?.check()?;
    crate::store::control::AUTH_SESSION_REVOKE_OWNER.on(&state.control).bind(("owner", user.clone())).await?.check()?;
    state.credentials.forget_user(user);

    let org_heirs: Vec<serde_json::Value> = orgs.iter().map(|(o, h)| json!({ "org": o.key(), "heir": h.to_string() })).collect();
    // owned by the heir, not the person: the job must outlive their account and name nobody once done
    let job = NewJob::new(jobs::kind::SUBJECT_ERASE, heir, format!("{}:{}", jobs::kind::SUBJECT_ERASE, ticket.id))
        .payload(json!({ "erasure": ticket.id, "orgs": org_heirs }));
    jobs::enqueue(&state.control, job).await?;
    crate::audit::record_as_caller_detail(&state.control, user, "subject.erase_request", &ticket.id, "ok", &format!("by={by}")).await;
    Ok(ticket)
}

/// Places or lifts a legal hold on `user`. Audited both ways by the caller.
pub async fn set_legal_hold(control: &ControlDb, user: &RecordId, hold: bool) -> AppResult<Option<Datetime>> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        #[serde(default)]
        #[surreal(default)]
        legal_hold_at: Option<Datetime>,
    }
    let rows: Vec<Row> = q::SET_LEGAL_HOLD.on(control).bind(("u", user.clone())).bind(("hold", hold)).await?.take(0)?;
    Ok(rows.into_iter().next().and_then(|r| r.legal_hold_at))
}

/// After a backup restore: every live account whose reference is on the erasure list (it was erased
/// after the backup was taken) is erased again, with the same rules. Holds and the last-owner check
/// are not consulted (the person was already erased once); an account that cannot be erased because
/// it is now alone in its workspace is logged and skipped. Returns how many erasures were queued.
pub async fn reapply_erasures(state: &AppState) -> AppResult<usize> {
    let refs: Vec<String> = q::ERASURE_REFS.on(&state.control).await?.take(0)?;
    if refs.is_empty() {
        return Ok(0);
    }
    let refs: std::collections::HashSet<String> = refs.into_iter().collect();
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        id: RecordId,
    }
    let users: Vec<Row> = q::USERS_WITH_ORG.on(&state.control).await?.take(0)?;
    let mut queued = 0;
    for u in users {
        if !refs.contains(&subject_ref(&state.settings, &u.id)) {
            continue;
        }
        let orgs = match heirs_ignoring_holds(&state.control, &u.id).await {
            Ok(o) if !o.is_empty() => o,
            Ok(_) | Err(_) => {
                tracing::warn!("a restored account was erased before, but cannot be erased again automatically (it is alone in its workspace); erase it from Settings > People");
                continue;
            }
        };
        enqueue_erasure(state, &u.id, "reapply", &orgs).await?;
        queued += 1;
    }
    if queued > 0 {
        tracing::info!(queued, "re-applied erasures recorded after the restored backup");
    }
    Ok(queued)
}

/// [`check_erasable`] without the hold and first-account refusals (the re-apply of a past erasure).
async fn heirs_ignoring_holds(control: &ControlDb, user: &RecordId) -> AppResult<Vec<(OrgId, RecordId)>> {
    let orgs: Vec<RecordId> = q::USER_ORGS.on(control).bind(("u", user.clone())).await?.take(0)?;
    let mut out = Vec::new();
    for org in orgs {
        let Some(id) = crate::rid::key_string(&org.key).and_then(|k| OrgId::parse(&k)) else { continue };
        let mut heirs: Vec<RecordId> = q::OTHER_ORG_OWNERS.on(control).bind(("u", user.clone())).bind(("org", org.clone())).await?.take(0)?;
        if heirs.is_empty() {
            heirs = q::OTHER_ORG_MEMBERS.on(control).bind(("u", user.clone())).bind(("org", org.clone())).await?.take(0)?;
        }
        if let Some(h) = heirs.into_iter().next() {
            out.push((id, h));
        }
    }
    Ok(out)
}

/// The leader runs this once when it takes the lease (so once per boot of a single-replica install):
/// the restore re-apply, logged and never fatal.
pub async fn reapply_on_start(state: &AppState) {
    if let Err(e) = reapply_erasures(state).await {
        tracing::warn!(error = %e.message, "re-applying recorded erasures failed; it runs again on the next start, or from Settings > People");
    }
}

/// The record a completed erasure leaves, for the People page (no personal data: an opaque reference,
/// per-category counts and the post-check result).
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, utoipa::ToSchema)]
pub struct ErasureRecord {
    pub id: String,
    /// The keyed hash of the erased account's id (never the id or the email).
    pub subject_ref: String,
    /// `self`, `admin` or `reapply` (a restore brought the account back and it was erased again).
    pub requested_by: String,
    pub status: String,
    /// Rows removed or anonymised per category.
    #[schema(value_type = Object)]
    pub counts: serde_json::Value,
    /// Rows still naming the person per table after the erasure: every value is 0 when it passed.
    #[schema(value_type = Object)]
    pub post_check: serde_json::Value,
    pub error: Option<String>,
    #[schema(value_type = String)]
    pub requested_at: Datetime,
    #[schema(value_type = Option<String>)]
    pub completed_at: Option<Datetime>,
}

pub async fn list_erasures(control: &ControlDb, org: &OrgId) -> AppResult<Vec<ErasureRecord>> {
    Ok(q::ERASURE_LIST.on(control).bind(("org", org.key())).await?.take(0)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_ref_is_stable_keyed_and_opaque() {
        let u: RecordId = crate::rid::parse("user:abc").unwrap();
        let a = reference("key", &u);
        assert_eq!(a, reference("key", &u), "the same account always maps to the same reference");
        assert_eq!(a.len(), 64);
        assert!(!a.contains("abc"));
        assert_ne!(a, reference("key", &crate::rid::parse("user:abd").unwrap()));
        assert_ne!(a, reference("another key", &u), "keyed: another install's key gives another reference");
    }
}
