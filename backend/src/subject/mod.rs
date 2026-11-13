//! Data-subject rights: a full export of what Eunomia holds about one person, and a verified
//! erasure of that person. Design, the inventory of where a person's data lives and what each step
//! does with it: docs/privacy/subject-rights.md.
//!
//! - [`request_export`](export::request_export) queues a `subject_export` job that builds a zip
//!   archive over every workspace the person belongs to and keeps it for seven days ([`export`]).
//! - [`request_erasure`] checks the account may be erased (no legal hold, a workspace with other
//!   members keeps an owner, the account that set the server up stays) and plans each workspace: a
//!   workspace whose only member the person is closes with them, after an explicit confirmation. The
//!   `erasure` row and its job are created in one transaction, the account is suspended at once, and
//!   the `subject_erase` job does the rest ([`erase`]).
//! - The row's `subject_ref` (a keyed hash of the user id) is the tombstone: a sync for an erased
//!   account stands down ([`is_erased`]), and after a backup restore [`reapply_erasures`] erases again
//!   every restored account, and every restored row whose owner, by hash, was erased.

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

/// The tombstone reference of an account under the current key: HMAC-SHA256 of its record id under
/// `ENCRYPTION_KEY`, hex. Opaque without the key; the same id always gives the same reference.
pub fn subject_ref(settings: &Settings, user: &RecordId) -> String {
    reference(&settings.encryption_key, user)
}

/// The references to match an account against: under the current key and under every key in
/// `ENCRYPTION_KEY_PREVIOUS` (comma separated). A reference is one-way, so after a key change the old
/// tombstones can only be matched under the old key: keep it there for as long as backups from before
/// the change exist (docs/privacy/subject-rights.md).
pub fn subject_refs(settings: &Settings, user: &RecordId) -> Vec<String> {
    refs_under(&settings.encryption_key, &std::env::var("ENCRYPTION_KEY_PREVIOUS").unwrap_or_default(), user)
}

fn refs_under(current: &str, previous: &str, user: &RecordId) -> Vec<String> {
    std::iter::once(current).chain(previous.split(',').map(str::trim).filter(|k| !k.is_empty())).map(|k| reference(k, user)).collect()
}

fn reference(key: &str, user: &RecordId) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC takes a key of any length");
    mac.update(b"eunomia-erasure-subject:");
    mac.update(user.to_string().as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// True when `user` has an erasure on record (requested, running, held or done). A sync for such an
/// account stands down, so a resync cannot bring its records back as theirs.
pub async fn is_erased(control: &ControlDb, settings: &Settings, user: &RecordId) -> bool {
    match q::ERASURE_HAS_REF.on(control).bind(("refs", subject_refs(settings, user))).await.and_then(|mut r| r.take::<Vec<RecordId>>(0)) {
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
    /// The admin's own password, typed again. An admin with no password (single sign-on only) types
    /// the person's email instead (the weaker fallback, see docs/privacy/subject-rights.md).
    Admin { admin: &'a RecordId, password: Option<&'a str>, email: Option<&'a str> },
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
        Confirmation::Admin { admin, password, email } => {
            let me = confirm_row(control, admin).await?;
            match me.password_hash.as_deref() {
                Some(hash) => {
                    if !crate::models_user::verify_password(password.unwrap_or(""), Some(hash)).await {
                        return Err(failed("That isn't your password. Type your own password to confirm. Nothing was deleted."));
                    }
                }
                None => {
                    if !email_matches(email) {
                        return Err(failed("Type the person's email address exactly to confirm. Nothing was deleted."));
                    }
                }
            }
        }
    }
    Ok(())
}

/// What happens in one workspace.
#[derive(Debug, Clone)]
pub struct OrgPlan {
    pub org: OrgId,
    /// Inherits org-level rows; none when the workspace closes.
    pub heir: Option<RecordId>,
    /// The person is its only member: it closes with them.
    pub close: bool,
    /// The person owns it with other members and no other owner (a restore re-apply only): the heir
    /// is made the owner first.
    pub transfer: bool,
}

fn closes_error() -> AppError {
    AppError::coded(
        ErrorCode::SubjectWorkspaceCloses,
        "You're the only person in this workspace, so deleting your account also deletes the workspace and everything in it: every space, document and connected source. Confirm that to go ahead.",
    )
}

fn transfer_first() -> AppError {
    AppError::coded(
        ErrorCode::SubjectLastOwner,
        "This account owns a workspace that has other members, and nobody else owns it. Make one of them the workspace owner first (Settings > People, Make workspace owner), then delete the account.",
    )
}

/// The plan per workspace `user` belongs to. `pending` are people with an erasure open: they inherit
/// nothing. `allow_close` is the confirmation that a workspace whose only member is `user` closes.
/// `reapply` (a restore re-apply) hands ownership over instead of refusing.
pub async fn plan(control: &ControlDb, user: &RecordId, pending: &[RecordId], allow_close: bool, reapply: bool) -> AppResult<Vec<OrgPlan>> {
    #[derive(Deserialize, SurrealValue)]
    struct Member {
        user: RecordId,
        #[serde(default)]
        #[surreal(default)]
        role: String,
    }
    let orgs: Vec<RecordId> = q::USER_ORGS.on(control).bind(("u", user.clone())).await?.take(0)?;
    let mut out = Vec::new();
    for org in orgs {
        let Some(id) = crate::rid::key_string(&org.key).and_then(|k| OrgId::parse(&k)) else { continue };
        let members: Vec<Member> = q::ORG_MEMBERSHIPS.on(control).bind(("org", org.clone())).await?.take(0)?;
        let mine = members.iter().find(|m| &m.user == user);
        let others: Vec<&Member> = members.iter().filter(|m| &m.user != user).collect();
        if others.is_empty() {
            if !allow_close {
                return Err(closes_error());
            }
            out.push(OrgPlan { org: id, heir: None, close: true, transfer: false });
            continue;
        }
        let staying: Vec<&&Member> = others.iter().filter(|m| !pending.contains(&m.user)).collect();
        let Some(first) = staying.first() else {
            return Err(AppError::coded(
                ErrorCode::SubjectLastOwner,
                "Everyone else in this workspace is being erased too. Wait for those erasures to finish, then try again.",
            ));
        };
        let other_owner = staying.iter().find(|m| m.role == "owner");
        let owns = mine.is_some_and(|m| m.role == "owner");
        let (heir, transfer) = match other_owner {
            Some(o) => (o.user.clone(), false),
            None if !owns => (first.user.clone(), false),
            None if reapply => (first.user.clone(), true),
            None => return Err(transfer_first()),
        };
        out.push(OrgPlan { org: id, heir: Some(heir), close: false, transfer });
    }
    Ok(out)
}

/// The request-time checks: a hold, the account that set up the server, and the plan.
pub async fn check_erasable(control: &ControlDb, user: &RecordId, allow_close: bool) -> AppResult<Vec<OrgPlan>> {
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
    let pending: Vec<RecordId> = q::OPEN_SUBJECTS.on(control).await?.take::<Vec<RecordId>>(0)?.into_iter().filter(|u| u != user).collect();
    let plans = plan(control, user, &pending, allow_close, false).await?;
    if plans.is_empty() {
        return Err(AppError::coded(ErrorCode::TenantNotFound, "This account does not belong to an organisation."));
    }
    Ok(plans)
}

/// An erasure as the routes report it.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, utoipa::ToSchema)]
pub struct ErasureTicket {
    pub id: String,
    /// `pending`, `running`, `held`, `done` or `failed`.
    pub status: String,
    #[schema(value_type = String)]
    pub requested_at: Datetime,
}

/// Who asked: the person themselves, an admin (named), or the restore re-apply.
#[derive(Debug, Clone)]
pub enum Requester {
    Myself,
    Admin(RecordId),
    Reapply,
}

impl Requester {
    fn by(&self) -> &'static str {
        match self {
            Requester::Myself => "self",
            Requester::Admin(_) => "admin",
            Requester::Reapply => "reapply",
        }
    }
}

/// The owner of every erasure job: an account that never exists, so the job outlives the person and
/// no erasure deletes another's job.
fn job_owner() -> RecordId {
    RecordId::new("user", "erasure-job")
}

/// Queues the erasure of `user` after [`check_erasable`] (the caller has already [`confirm`]ed).
/// `allow_close` is the confirmation that a workspace whose only member they are closes with them.
pub async fn request_erasure(state: &AppState, user: &RecordId, who: Requester, allow_close: bool) -> AppResult<ErasureTicket> {
    let plans = check_erasable(&state.control, user, allow_close).await?;
    enqueue_erasure(state, user, who, &plans).await
}

/// Creates (or joins) the erasure of `user` and makes sure it has a live job; then suspends the
/// account, signs it out everywhere and records the request in the workspace's activity log.
pub(crate) async fn enqueue_erasure(state: &AppState, user: &RecordId, who: Requester, plans: &[OrgPlan]) -> AppResult<ErasureTicket> {
    let reference = subject_ref(&state.settings, user);
    let ticket = match open_ticket(&state.control, &reference).await? {
        Some(t) => {
            ensure_job(&state.control, &t.id).await?;
            t
        }
        None => match create(state, user, &who, plans, &reference).await {
            Ok(t) => t,
            // two requests at once for one person: the unique open slot keeps one, the other joins it
            Err(e) if e.code == ErrorCode::DbDuplicate || e.code == ErrorCode::DbConflict => {
                let t = open_ticket(&state.control, &reference).await?.ok_or(e)?;
                ensure_job(&state.control, &t.id).await?;
                t
            }
            Err(e) => return Err(e),
        },
    };
    // nothing new is written while it runs: the account is suspended, its sessions end, cached
    // credentials go (each step is idempotent, so a request that failed here is finished by the next)
    crate::store::control::USER_SET_SUSPENDED.on(&state.control).bind(("id", user.clone())).bind(("suspended", true)).await?.check()?;
    crate::store::control::AUTH_SESSION_REVOKE_OWNER.on(&state.control).bind(("owner", user.clone())).await?.check()?;
    state.credentials.forget_user(user);
    // the workspace's activity log keeps who asked; the person appears only as the opaque reference
    let scope = match &who {
        Requester::Admin(a) => Some(a.clone()),
        _ => plans.iter().find_map(|p| p.heir.clone()),
    }
    .unwrap_or_else(|| RecordId::new("user", "erased"));
    let actor = crate::authz::current().map(|c| c.actor).unwrap_or_else(|| crate::authz::Caller::system().actor);
    let detail = format!("by={} ref={}", who.by(), &reference[..12]);
    crate::audit::record(&state.control, crate::audit::Event { user: Some(&scope), actor: &actor, action: "subject.erase_request", target: &ticket.id, outcome: "ok", detail: &detail })
        .await;
    Ok(ticket)
}

async fn open_ticket(control: &ControlDb, reference: &str) -> AppResult<Option<ErasureTicket>> {
    let rows: Vec<ErasureTicket> = q::ERASURE_OPEN.on(control).bind(("ref", reference.to_string())).await?.take(0)?;
    Ok(rows.into_iter().next())
}

async fn create(state: &AppState, user: &RecordId, who: &Requester, plans: &[OrgPlan], reference: &str) -> AppResult<ErasureTicket> {
    let home = plans.first().map(|p| p.org).ok_or_else(|| AppError::internal("no org to erase in"))?;
    let eid = RecordId::new("erasure", uuid::Uuid::new_v4().simple().to_string());
    let key = format!("{}:{}", jobs::kind::SUBJECT_ERASE, eid.to_string());
    let jid = RecordId::new("job", crate::tx::stable_key('j', &key));
    let payload = json!({
        "erasure": eid.to_string(),
        "orgs": plans.iter().map(|p| json!({ "org": p.org.key(), "heir": p.heir.as_ref().map(|h| h.to_string()), "close": p.close })).collect::<Vec<_>>(),
    });
    let closes: Vec<String> = plans.iter().filter(|p| p.close).map(|p| p.org.key()).collect();
    let requester = match who {
        Requester::Admin(a) => Some(a.clone()),
        _ => None,
    };
    let mut res = q::ERASURE_CREATE
        .on(&state.control)
        .bind(("eid", eid))
        .bind(("org", home.key()))
        .bind(("ref", reference.to_string()))
        .bind(("u", user.clone()))
        .bind(("by", who.by()))
        .bind(("requester", requester))
        .bind(("closes", closes))
        .bind(("jid", jid))
        .bind(("owner", job_owner()))
        .bind(("payload", payload))
        .bind(("key", key))
        .await?;
    let ticket: Option<ErasureTicket> = res.take(q::ERASURE_CREATE.slot)?;
    ticket.ok_or_else(|| AppError::internal("erasure row was not created"))
}

/// Gives an open erasure a job if it has no live one (its job died, or the request stopped half way).
pub async fn ensure_job(control: &ControlDb, erasure: &str) -> AppResult<()> {
    let live: Vec<RecordId> = q::ERASURE_LIVE_JOB.on(control).bind(("eid", erasure.to_string())).await?.take(0)?;
    if live.is_empty() {
        let key = format!("{}:{erasure}:{}", jobs::kind::SUBJECT_ERASE, uuid::Uuid::new_v4().simple());
        let mut job = NewJob::new(jobs::kind::SUBJECT_ERASE, job_owner(), key).payload(json!({ "erasure": erasure }));
        job.max_attempts = 8;
        jobs::enqueue(control, job).await?;
    }
    Ok(())
}

/// Places or lifts a legal hold on `user`. Lifting it resumes an erasure the hold parked. Returns the
/// hold time and whether an erasure is open (a hold placed now parks it before anything is deleted,
/// or before the account goes if the job is already past that point).
pub async fn set_legal_hold(control: &ControlDb, user: &RecordId, hold: bool) -> AppResult<(Option<Datetime>, bool)> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        #[serde(default)]
        #[surreal(default)]
        legal_hold_at: Option<Datetime>,
    }
    let rows: Vec<Row> = q::SET_LEGAL_HOLD.on(control).bind(("u", user.clone())).bind(("hold", hold)).await?.take(0)?;
    if !hold {
        let resumed: Vec<String> = q::ERASURES_HELD_OF.on(control).bind(("u", user.clone())).await?.take(0)?;
        for e in resumed {
            ensure_job(control, &e).await?;
        }
    }
    let open: Vec<RecordId> = q::ERASURE_OPEN_OF.on(control).bind(("u", user.clone())).await?.take(0)?;
    Ok((rows.into_iter().next().and_then(|r| r.legal_hold_at), !open.is_empty()))
}

/// Makes `to` the owner of `org` in place of its current owner (Settings > People): the way to let a
/// sole owner of a workspace with other members be erased.
pub async fn transfer_workspace(control: &ControlDb, org: &OrgId, to: &RecordId) -> AppResult<()> {
    #[derive(Deserialize, SurrealValue)]
    struct Member {
        user: RecordId,
        #[serde(default)]
        #[surreal(default)]
        role: String,
    }
    let members: Vec<Member> = q::ORG_MEMBERSHIPS.on(control).bind(("org", org.record())).await?.take(0)?;
    if !members.iter().any(|m| &m.user == to) {
        return Err(AppError::coded(ErrorCode::AuthNotFound, "That account isn't in this workspace."));
    }
    let Some(owner) = members.iter().find(|m| m.role == "owner") else {
        return Err(AppError::internal("the workspace has no owner"));
    };
    if &owner.user == to {
        return Ok(());
    }
    crate::tx::with_retry(|| async {
        q::ORG_OWNER_TRANSFER.on(control).bind(("org", org.record())).bind(("from", owner.user.clone())).bind(("to", to.clone())).await?.check()
    })
    .await?;
    Ok(())
}

/// After a backup restore: erases again (a) every live account whose reference is on the erasure list
/// and (b) in every workspace, rows whose owner has no account but whose reference is on the list (a
/// restore of only an org database). The re-apply hands workspace ownership over instead of leaving a
/// workspace without an owner, closes a workspace again when the person is its only member, and does
/// not consult holds (the person was already erased once). One failure is logged and the rest go on.
pub async fn reapply_erasures(state: &AppState) -> AppResult<usize> {
    #[derive(Deserialize, SurrealValue)]
    struct Done {
        subject_ref: String,
    }
    let done: Vec<Done> = q::ERASURE_REFS.on(&state.control).await?.take(0)?;
    if done.is_empty() {
        return Ok(0);
    }
    let refs: std::collections::HashSet<String> = done.into_iter().map(|d| d.subject_ref).collect();
    let erased = |u: &RecordId| subject_refs(&state.settings, u).iter().any(|r| refs.contains(r));
    let mut queued = 0;

    let users: Vec<RecordId> = q::USERS_WITH_ORG.on(&state.control).await?.take(0)?;
    for u in users.iter().filter(|u| erased(u)) {
        let pending: Vec<RecordId> = q::OPEN_SUBJECTS.on(&state.control).await?.take::<Vec<RecordId>>(0)?.into_iter().filter(|p| p != u).collect();
        match plan(&state.control, u, &pending, true, true).await {
            Ok(p) if !p.is_empty() => match enqueue_erasure(state, u, Requester::Reapply, &p).await {
                Ok(_) => queued += 1,
                Err(e) => tracing::warn!(error = %e.message, "could not queue a re-applied erasure; it is retried at the next start"),
            },
            Ok(_) => tracing::warn!("a restored account that was erased belongs to no workspace; erase it from Settings > People"),
            Err(e) => tracing::warn!(error = %e.message, "a restored account that was erased cannot be erased again automatically; erase it from Settings > People"),
        }
    }

    // a restore of only an org database: rows whose owner the control database no longer has
    let orgs: Vec<RecordId> = q::READY_ORGS.on(&state.control).await?.take(0)?;
    for org in orgs {
        let Some(id) = crate::rid::key_string(&org.key).and_then(|k| OrgId::parse(&k)) else { continue };
        let Ok(org_state) = state.org(&id).await else { continue };
        let owners: Vec<RecordId> = q::ROW_OWNERS.on(&org_state.db).await?.take(0)?;
        let candidates: Vec<RecordId> = owners.into_iter().filter(|o| erased(o)).collect();
        if candidates.is_empty() {
            continue;
        }
        let existing: Vec<RecordId> = q::USERS_EXISTING.on(&state.control).bind(("ids", candidates.clone())).await?.take(0)?;
        #[derive(Deserialize, SurrealValue)]
        struct Member {
            user: RecordId,
            #[serde(default)]
            #[surreal(default)]
            role: String,
        }
        let members: Vec<Member> = q::ORG_MEMBERSHIPS.on(&state.control).bind(("org", org.clone())).await?.take(0)?;
        let heir = members.iter().find(|m| m.role == "owner").or_else(|| members.first()).map(|m| m.user.clone());
        for gone in candidates.into_iter().filter(|c| !existing.contains(c)) {
            let p = vec![OrgPlan { org: id, heir: heir.clone(), close: false, transfer: false }];
            match enqueue_erasure(state, &gone, Requester::Reapply, &p).await {
                Ok(_) => queued += 1,
                Err(e) => tracing::warn!(error = %e.message, "could not queue the erasure of restored rows; it is retried at the next start"),
            }
        }
    }
    if queued > 0 {
        tracing::info!(queued, "re-applied erasures recorded after the restored backup");
    }
    Ok(queued)
}

/// Every open erasure gets a live job (a request that stopped half way, a job that died).
pub async fn heal_open(state: &AppState) -> AppResult<()> {
    let open: Vec<String> = q::ERASURES_OPEN_ALL.on(&state.control).await?.take(0)?;
    for e in open {
        ensure_job(&state.control, &e).await?;
    }
    Ok(())
}

/// The leader runs this once when it takes the lease (so once per boot of a single-replica install):
/// the heal and the restore re-apply, logged and never fatal.
pub async fn reapply_on_start(state: &AppState) {
    if let Err(e) = heal_open(state).await {
        tracing::warn!(error = %e.message, "giving open erasures a job failed; it runs again on the next start");
    }
    if let Err(e) = reapply_erasures(state).await {
        tracing::warn!(error = %e.message, "re-applying recorded erasures failed; it runs again on the next start, or from Settings > People");
    }
}

/// The record a completed erasure leaves, for the People page (no personal data: an opaque reference,
/// per-category counts, the post-check result and the review counts).
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
    /// Mentions of the person's email left in content other people wrote, for review (not counted as
    /// residue: that content is theirs).
    #[serde(default)]
    #[surreal(default)]
    #[schema(value_type = Object)]
    pub review: serde_json::Value,
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

    #[test]
    fn a_tombstone_written_under_the_previous_key_still_matches() {
        let u: RecordId = crate::rid::parse("user:abc").unwrap();
        let old = reference("old key", &u);
        assert!(!refs_under("new key", "", &u).contains(&old), "after a key change the old tombstone no longer matches");
        let refs = refs_under("new key", "older key, old key", &u);
        assert!(refs.contains(&old) && refs[0] == reference("new key", &u), "ENCRYPTION_KEY_PREVIOUS keeps it matching");
    }
}
