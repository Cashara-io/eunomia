//! Data-subject rights: a full export of what Eunomia holds about one person, and a verified
//! erasure of that person. Design, the inventory of where a person's data lives and what each step
//! does with it: docs/privacy/subject-rights.md.
//!
//! Who may do what (one definition of "server operator": [`crate::authz::is_server_operator`], an
//! address in `EUNOMIA_ADMIN_EMAILS` or the install's first account):
//!
//! - **The person**: export all their own data; erase their own account (every workspace, then the
//!   account), closing a workspace whose only member they are after an explicit confirmation, refused
//!   while they own a workspace with other members until ownership is handed over.
//! - **A server operator**: erase any account; cancel an erasure that has not started deleting; place
//!   and lift legal holds (never their own); hand a workspace over.
//! - **A workspace owner**: hand their own workspace over.
//! - **An in-app admin**: start the export of the workspace's data about a member; place a legal hold.
//!
//! The first account and operator accounts are never erased by anyone but themselves (and the first
//! account not even by itself: it would make the next one an admin). Erasure by an organisation's own
//! admin, scoped to that organisation, is not in this release (docs/privacy/subject-rights.md).

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

/// How recent a sign-in must be to confirm an erasure for an account with no password.
pub const REAUTH_WINDOW_SECS: i64 = 600;

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

/// True when `user` has an erasure on record (requested, running, held or done; not cancelled). A sync
/// for such an account stands down, so a resync cannot bring its records back as theirs.
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
    AppError::coded(ErrorCode::SubjectLegalHold, "This account is under a legal hold, so it can't be erased. A server operator can lift the hold.")
}

#[derive(Debug, Deserialize, SurrealValue)]
struct ConfirmRow {
    #[serde(default)]
    #[surreal(default)]
    email: String,
    /// The stored, normalised address: operator status is read from it, never from the raw `email`.
    #[serde(default)]
    #[surreal(default)]
    email_lc: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    password_hash: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    legal_hold_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    suspended_at: Option<Datetime>,
}

async fn confirm_row(control: &ControlDb, user: &RecordId) -> AppResult<ConfirmRow> {
    let mut res = q::CONFIRM_ROW.on(control).bind(("u", user.clone())).await?;
    res.take::<Option<ConfirmRow>>(0)?.ok_or_else(|| AppError::coded(ErrorCode::AuthNotFound, "That account doesn't exist."))
}

/// How the person asking proved it is them (self-service) or that they mean it (an admin or operator).
/// For an account with no password (single sign-on only) the proof is a sign-in within
/// [`REAUTH_WINDOW_SECS`]: `signed_in_at` is when the browser session making the request was created.
pub enum Confirmation<'a> {
    /// The account's own password, typed again.
    SelfService { password: Option<&'a str>, signed_in_at: Option<chrono::DateTime<chrono::Utc>> },
    /// The acting operator's own password, typed again.
    Admin { admin: &'a RecordId, password: Option<&'a str>, signed_in_at: Option<chrono::DateTime<chrono::Utc>> },
}

fn reauthenticate() -> AppError {
    AppError::coded(
        ErrorCode::SubjectReauthenticate,
        "This account signs in with single sign-on, so it has no password to type. Sign in again, then confirm within 10 minutes. Nothing was deleted.",
    )
}

fn recent(signed_in_at: Option<chrono::DateTime<chrono::Utc>>) -> bool {
    signed_in_at.is_some_and(|t| chrono::Utc::now().signed_duration_since(t).num_seconds() <= REAUTH_WINDOW_SECS)
}

/// Checks the confirmation before anything is touched. A wrong password is
/// `subject.confirmation_failed`; an account with no password and no recent sign-in is
/// `subject.reauthenticate`.
pub async fn confirm(control: &ControlDb, user: &RecordId, how: Confirmation<'_>) -> AppResult<()> {
    let failed = |m: &str| AppError::coded(ErrorCode::SubjectConfirmationFailed, m.to_string());
    let (who, password, signed_in_at, wrong) = match how {
        Confirmation::SelfService { password, signed_in_at } => (user, password, signed_in_at, "That password isn't right. Nothing was deleted."),
        Confirmation::Admin { admin, password, signed_in_at } => (admin, password, signed_in_at, "That isn't your password. Type your own password to confirm. Nothing was deleted."),
    };
    confirm_row(control, user).await?;
    let row = confirm_row(control, who).await?;
    match row.password_hash.as_deref() {
        Some(hash) => {
            if !crate::models_user::verify_password(password.unwrap_or(""), Some(hash)).await {
                return Err(failed(wrong));
            }
        }
        None => {
            if !recent(signed_in_at) {
                return Err(reauthenticate());
            }
        }
    }
    Ok(())
}

/// When the browser session `sid` of `user` signed in.
pub async fn session_signed_in_at(control: &ControlDb, user: &RecordId, sid: Option<String>) -> AppResult<Option<chrono::DateTime<chrono::Utc>>> {
    let Some(sid) = sid else { return Ok(None) };
    let at: Vec<Datetime> = q::SESSION_CREATED.on(control).bind(("sid", sid)).bind(("u", user.clone())).await?.take(0)?;
    Ok(at.into_iter().next().and_then(|d| crate::sources::base::datetime_to_chrono(&d)))
}

/// What happens in one workspace.
#[derive(Debug, Clone)]
pub struct OrgPlan {
    pub org: OrgId,
    /// Inherits org-level rows; none when the workspace closes, or when nobody who may inherit is left.
    pub heir: Option<RecordId>,
    /// The person is its only member: it closes with them.
    pub close: bool,
    /// The person owns it and nobody who may inherit owns it: the heir is made the owner first (a
    /// restore re-apply only).
    pub transfer: bool,
    /// Everyone left in the workspace is being erased too: the last of them closes it.
    pub joint: Vec<RecordId>,
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
        "This account owns a workspace that has other members, and nobody else owns it. Hand the workspace over to one of them first (the owner, or a server operator, in Settings > People); then the account can be deleted. Someone under a legal hold or being erased cannot take it.",
    )
}

#[derive(Deserialize, SurrealValue)]
struct Member {
    user: RecordId,
    #[serde(default)]
    #[surreal(default)]
    role: String,
}

/// The plan per workspace `user` belongs to. `pending` are people with an erasure open and `held` people
/// under a legal hold: both stay readers whose content is kept, but neither inherits anything.
/// `allow_close` is the confirmation that a workspace whose only member is `user` closes. `reapply` (a
/// restore re-apply) hands ownership over instead of refusing.
pub async fn plan(control: &ControlDb, user: &RecordId, pending: &[RecordId], held: &[RecordId], allow_close: bool, reapply: bool) -> AppResult<Vec<OrgPlan>> {
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
            out.push(OrgPlan { org: id, heir: None, close: true, transfer: false, joint: Vec::new() });
            continue;
        }
        let owns = mine.is_some_and(|m| m.role == "owner");
        // who may inherit: nobody with an erasure open and nobody under a legal hold
        let eligible: Vec<&&Member> = others.iter().filter(|m| !pending.contains(&m.user) && !held.contains(&m.user)).collect();
        if eligible.is_empty() && others.iter().all(|m| pending.contains(&m.user) && !held.contains(&m.user)) {
            // everyone left is being erased too: this one goes first (the workspace has no owner meanwhile)
            // and the last of them closes it
            out.push(OrgPlan { org: id, heir: None, close: false, transfer: false, joint: others.iter().map(|m| m.user.clone()).collect() });
            continue;
        }
        let other_owner = eligible.iter().find(|m| m.role == "owner");
        let (heir, transfer) = match (other_owner, eligible.first()) {
            (Some(o), _) => (Some(o.user.clone()), false),
            (None, first) if !owns => (first.map(|m| m.user.clone()), false),
            (None, Some(first)) if reapply => (Some(first.user.clone()), true),
            _ => return Err(transfer_first()),
        };
        out.push(OrgPlan { org: id, heir, close: false, transfer, joint: Vec::new() });
    }
    Ok(out)
}

/// The people with an erasure open and the people under a legal hold, both without `user`.
pub async fn pending_and_held(control: &ControlDb, user: &RecordId) -> AppResult<(Vec<RecordId>, Vec<RecordId>)> {
    let pending: Vec<RecordId> = q::OPEN_SUBJECTS.on(control).await?.take::<Vec<RecordId>>(0)?.into_iter().filter(|u| u != user).collect();
    let held: Vec<RecordId> = q::HELD_USERS.on(control).await?.take::<Vec<RecordId>>(0)?.into_iter().filter(|u| u != user).collect();
    Ok((pending, held))
}

/// Who asked for an erasure. It always covers the whole account.
#[derive(Debug, Clone)]
pub enum Requester {
    /// The person.
    Myself,
    /// A server operator.
    Operator(RecordId),
    /// The restore re-apply of a finished erasure.
    Reapply,
}

impl Requester {
    fn by(&self) -> &'static str {
        match self {
            Requester::Myself => "self",
            Requester::Operator(_) => "operator",
            Requester::Reapply => "reapply",
        }
    }
}

/// The request-time checks: a hold, the account that set up the server and operator accounts (never
/// erased by anyone but themselves; the first account not even by itself), and the plan.
pub async fn check_erasable(control: &ControlDb, user: &RecordId, who: &Requester, allow_close: bool) -> AppResult<Vec<OrgPlan>> {
    let row = confirm_row(control, user).await?;
    if row.legal_hold_at.is_some() {
        return Err(hold_error());
    }
    protected_account(control, user, row.email_lc.as_deref(), who).await?;
    let (pending, held) = pending_and_held(control, user).await?;
    let plans = plan(control, user, &pending, &held, allow_close, false).await?;
    if plans.is_empty() {
        return Err(AppError::coded(ErrorCode::TenantNotFound, "This account does not belong to an organisation."));
    }
    Ok(plans)
}

/// The first account is never erased in the app, and an operator's account only by its owner. Checked on
/// every request path, including one that joins an erasure already open.
async fn protected_account(control: &ControlDb, user: &RecordId, email_lc: Option<&str>, who: &Requester) -> AppResult<()> {
    if crate::capsules::first_user(control).await?.as_ref() == Some(user) {
        // the oldest account is always an admin (`authz::is_admin`): erasing it would quietly make the next one an admin
        return Err(AppError::coded(
            ErrorCode::SubjectLastOwner,
            "This is the account that set up the server, so it can't be erased in the app. The server's operator can remove it (docs/privacy/subject-rights.md).",
        ));
    }
    if !matches!(who, Requester::Myself) && crate::authz::stored_email_is_operator(email_lc) {
        return Err(AppError::coded(ErrorCode::AuthForbidden, "This is a server operator's account: only its owner can erase it."));
    }
    Ok(())
}

/// An erasure as the routes report it.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, utoipa::ToSchema)]
pub struct ErasureTicket {
    pub id: String,
    /// `pending`, `running`, `held`, `done`, `failed` or `cancelled`.
    pub status: String,
    #[schema(value_type = String)]
    pub requested_at: Datetime,
}

/// The owner of every erasure job: an account that never exists, so the job outlives the person and
/// no erasure deletes another's job.
fn job_owner() -> RecordId {
    RecordId::new("user", "erasure-job")
}

/// Queues the erasure of `user` after [`check_erasable`] (the caller has already [`confirm`]ed).
pub async fn request_erasure(state: &AppState, user: &RecordId, who: Requester, allow_close: bool) -> AppResult<ErasureTicket> {
    // a repeat request joins the open erasure; it can add a confirmation that a workspace closes
    if open_ticket(&state.control, &subject_refs(&state.settings, user)).await?.is_some() {
        if !matches!(who, Requester::Reapply) {
            let row = confirm_row(&state.control, user).await?;
            protected_account(&state.control, user, row.email_lc.as_deref(), &who).await?;
        }
        let plans = if allow_close {
            let (pending, held) = pending_and_held(&state.control, user).await?;
            plan(&state.control, user, &pending, &held, true, false).await.unwrap_or_default()
        } else {
            Vec::new()
        };
        return enqueue_erasure(state, user, who, &plans).await;
    }
    let plans = match check_erasable(&state.control, user, &who, allow_close).await {
        Ok(p) => p,
        // a request for the same person that raced this one opened it first: join it
        Err(e) => match open_ticket(&state.control, &subject_refs(&state.settings, user)).await? {
            Some(_) => Vec::new(),
            None => return Err(e),
        },
    };
    enqueue_erasure(state, user, who, &plans).await
}

/// Creates (or joins) the erasure of `user` and makes sure it has a live job; suspends the account and
/// signs it out everywhere at once, and records the request in the workspace's activity log.
pub(crate) async fn enqueue_erasure(state: &AppState, user: &RecordId, who: Requester, plans: &[OrgPlan]) -> AppResult<ErasureTicket> {
    let reference = subject_ref(&state.settings, user);
    let refs = subject_refs(&state.settings, user);
    let closes: Vec<String> = plans.iter().filter(|p| p.close).map(|p| p.org.key()).collect();
    let ticket = match open_ticket(&state.control, &refs).await? {
        Some(t) => {
            // a repeat request adds its confirmation that a workspace closes
            if !closes.is_empty() {
                q::ERASURE_ADD_CLOSES.on(&state.control).bind(("id", t.id.clone())).bind(("closes", closes.clone())).await?.check()?;
            }
            ensure_job(&state.control, &t.id).await?;
            t
        }
        None => {
            let was_suspended = confirm_row(&state.control, user).await.map(|r| r.suspended_at.is_some()).unwrap_or(false);
            match create(state, user, &who, plans, &reference, !was_suspended).await {
                Ok(t) => t,
                // two requests at once for one person: the unique open slot keeps one, the other joins it
                Err(e) if e.code == ErrorCode::DbDuplicate || e.code == ErrorCode::DbConflict => {
                    let t = open_ticket(&state.control, &refs).await?.ok_or(e)?;
                    ensure_job(&state.control, &t.id).await?;
                    t
                }
                Err(e) => return Err(e),
            }
        }
    };
    // nothing new is written while it runs (each step is idempotent: a request that failed here is finished by the next)
    crate::store::control::USER_SET_SUSPENDED.on(&state.control).bind(("id", user.clone())).bind(("suspended", true)).await?.check()?;
    crate::store::control::AUTH_SESSION_REVOKE_OWNER.on(&state.control).bind(("owner", user.clone())).await?.check()?;
    state.credentials.forget_user(user);
    // the workspace's activity log keeps who asked; the person appears only as the opaque reference
    let scope = match &who {
        Requester::Operator(a) => Some(a.clone()),
        _ => plans.iter().find_map(|p| p.heir.clone()),
    }
    .unwrap_or_else(|| RecordId::new("user", "erased"));
    let actor = crate::authz::current().map(|c| c.actor).unwrap_or_else(|| crate::authz::Caller::system().actor);
    let detail = format!("by={} ref={}", who.by(), &reference[..12]);
    crate::audit::record(&state.control, crate::audit::Event { user: Some(&scope), actor: &actor, action: "subject.erase_request", target: &ticket.id, outcome: "ok", detail: &detail })
        .await;
    if let Requester::Operator(_) = who {
        // the person's own activity shows it too (until the erasure removes them)
        crate::audit::record(&state.control, crate::audit::Event { user: Some(user), actor: &actor, action: "subject.erase_request", target: &ticket.id, outcome: "ok", detail: &detail })
            .await;
    }
    Ok(ticket)
}

async fn open_ticket(control: &ControlDb, refs: &[String]) -> AppResult<Option<ErasureTicket>> {
    let rows: Vec<ErasureTicket> = q::ERASURE_OPEN.on(control).bind(("refs", refs.to_vec())).await?.take(0)?;
    Ok(rows.into_iter().next())
}

async fn create(state: &AppState, user: &RecordId, who: &Requester, plans: &[OrgPlan], reference: &str, suspended_here: bool) -> AppResult<ErasureTicket> {
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
        Requester::Operator(a) => Some(a.clone()),
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
        .bind(("suspended_here", suspended_here))
        .bind(("closes", closes))
        .bind(("jid", jid))
        .bind(("owner", job_owner()))
        .bind(("payload", payload))
        .bind(("key", key))
        .await?;
    let ticket: Option<ErasureTicket> = res.take(q::ERASURE_CREATE.slot)?;
    ticket.ok_or_else(|| AppError::internal("erasure row was not created"))
}

/// Gives an open erasure a job if it has no live one (its job died, or the request stopped half way),
/// releasing locks a dead job left behind.
pub async fn ensure_job(control: &ControlDb, erasure: &str) -> AppResult<()> {
    let live: Vec<RecordId> = q::ERASURE_LIVE_JOB.on(control).bind(("eid", erasure.to_string())).await?.take(0)?;
    if live.is_empty() {
        if let Ok(id) = crate::rid::parse(erasure) {
            q::LOCKS_RELEASE.on(control).bind(("id", id)).await?.check()?;
        }
        let key = format!("{}:{erasure}:{}", jobs::kind::SUBJECT_ERASE, uuid::Uuid::new_v4().simple());
        let mut job = NewJob::new(jobs::kind::SUBJECT_ERASE, job_owner(), key).payload(json!({ "erasure": erasure }));
        job.max_attempts = 8;
        jobs::enqueue(control, job).await?;
    }
    Ok(())
}

/// The workspace's owner.
async fn owner_of(control: &ControlDb, org: &OrgId) -> AppResult<Option<RecordId>> {
    let members: Vec<Member> = q::ORG_MEMBERSHIPS.on(control).bind(("org", org.record())).await?.take(0)?;
    Ok(members.into_iter().find(|m| m.role == "owner").map(|m| m.user))
}

/// May `actor` act as the owner of `org` (hand it over)? Its owner or a server operator.
pub async fn owner_or_operator(control: &ControlDb, actor: &crate::models_user::User, org: &OrgId) -> AppResult<bool> {
    Ok(crate::authz::is_server_operator(control, actor).await? || owner_of(control, org).await?.as_ref() == Some(&actor.id))
}

/// Places or lifts a legal hold on `user`. An admin or an operator places one (the route checks that);
/// only a server operator lifts one, and never a hold on themselves. Lifting it resumes an erasure the
/// hold parked. Returns the hold time and whether an erasure is open.
pub async fn set_legal_hold(control: &ControlDb, actor: &crate::models_user::User, user: &RecordId, hold: bool) -> AppResult<(Option<Datetime>, bool)> {
    if !hold && (!crate::authz::is_server_operator(control, actor).await? || &actor.id == user) {
        return Err(AppError::coded(ErrorCode::AuthForbidden, "Only a server operator can lift a legal hold, and never one on their own account."));
    }
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        #[serde(default)]
        #[surreal(default)]
        legal_hold_at: Option<Datetime>,
    }
    let rows: Vec<Row> = q::SET_LEGAL_HOLD.on(control).bind(("u", user.clone())).bind(("hold", hold)).bind(("by", actor.id.clone())).await?.take(0)?;
    if !hold {
        let resumed: Vec<String> = q::ERASURES_HELD_OF.on(control).bind(("u", user.clone())).await?.take(0)?;
        for e in resumed {
            ensure_job(control, &e).await?;
        }
    }
    let open: Vec<RecordId> = q::ERASURE_OPEN_OF.on(control).bind(("u", user.clone())).await?.take(0)?;
    Ok((rows.into_iter().next().and_then(|r| r.legal_hold_at), !open.is_empty()))
}

/// Who placed `user`'s hold, if any.
pub async fn hold_placed_by(control: &ControlDb, user: &RecordId) -> AppResult<Option<RecordId>> {
    let by: Option<Option<RecordId>> = q::HOLD_BY.on(control).bind(("u", user.clone())).await?.take(0)?;
    Ok(by.flatten())
}

/// True while an erasure of `user` is open: the account stays suspended (no restore of access, no
/// sign-in) until it finishes or an operator cancels it before it starts deleting.
pub async fn account_erasure_open(control: &ControlDb, user: &RecordId) -> AppResult<bool> {
    let open: Vec<RecordId> = q::ERASURE_OPEN_OF.on(control).bind(("u", user.clone())).await?.take(0)?;
    Ok(!open.is_empty())
}

/// Makes `to` the owner of `org` in place of its current owner. Only the current owner or a server
/// operator; `to` must be an active member of the workspace, not suspended, not under a hold, with no
/// erasure open. Returns the previous owner. The workspace is never left without an owner: the change
/// is one transaction that promotes exactly one membership or changes nothing.
pub async fn transfer_workspace(control: &ControlDb, actor: &crate::models_user::User, org: &OrgId, to: &RecordId) -> AppResult<RecordId> {
    if !owner_or_operator(control, actor, org).await? {
        return Err(AppError::coded(ErrorCode::AuthForbidden, "Only the workspace owner or a server operator can hand the workspace over."));
    }
    let members: Vec<Member> = q::ORG_MEMBERSHIPS.on(control).bind(("org", org.record())).await?.take(0)?;
    if !members.iter().any(|m| &m.user == to) {
        return Err(AppError::coded(ErrorCode::AuthNotFound, "That account isn't in this workspace."));
    }
    let owner = members.iter().find(|m| m.role == "owner").map(|m| m.user.clone()).ok_or_else(|| AppError::internal("the workspace has no owner"))?;
    if &owner == to {
        return Ok(owner);
    }
    #[derive(Deserialize, SurrealValue)]
    struct State {
        #[serde(default)]
        #[surreal(default)]
        suspended_at: Option<Datetime>,
        #[serde(default)]
        #[surreal(default)]
        legal_hold_at: Option<Datetime>,
    }
    let st: Option<State> = q::USER_STATE.on(control).bind(("u", to.clone())).await?.take(0)?;
    let st = st.ok_or_else(|| AppError::coded(ErrorCode::AuthNotFound, "That account doesn't exist."))?;
    let open: Vec<RecordId> = q::ERASURE_OPEN_OF.on(control).bind(("u", to.clone())).await?.take(0)?;
    if st.suspended_at.is_some() || st.legal_hold_at.is_some() || !open.is_empty() {
        return Err(AppError::bad_request("That account is suspended, under a legal hold or being erased, so it can't own the workspace."));
    }
    crate::tx::with_retry(|| async {
        q::ORG_OWNER_TRANSFER.on(control).bind(("org", org.record())).bind(("from", owner.clone())).bind(("to", to.clone())).await?.check()
    })
    .await?;
    Ok(owner)
}

/// An operator stops an erasure that has not started deleting: the working record and the person's id
/// are cleared and the account is left exactly as it was (unsuspended if the request suspended it).
/// Once deletion has begun an erasure completes or waits (`held`, `failed` and retried); it is never
/// abandoned half done.
pub async fn cancel_erasure(control: &ControlDb, erasure: &str) -> AppResult<()> {
    let id = crate::rid::parse(erasure).map_err(|_| AppError::coded(ErrorCode::ResourceNotFound, "No such erasure."))?;
    if id.table() != "erasure" {
        return Err(AppError::coded(ErrorCode::ResourceNotFound, "No such erasure."));
    }
    let exists: Vec<RecordId> = q::ERASURE_EXISTS.on(control).bind(("id", id.clone())).await?.take(0)?;
    if exists.is_empty() {
        return Err(AppError::coded(ErrorCode::ResourceNotFound, "No such erasure."));
    }
    #[derive(Deserialize, SurrealValue)]
    struct Was {
        #[serde(default)]
        #[surreal(default)]
        subject: Option<RecordId>,
        #[serde(default)]
        #[surreal(default)]
        suspended_here: bool,
    }
    let mut res = q::ERASURE_CANCEL.on(control).bind(("id", id)).await?;
    let was: Option<Was> = res.take(q::ERASURE_CANCEL.slot)?;
    let Some(was) = was else {
        return Err(AppError::coded(
            ErrorCode::DbConflict,
            "That erasure has started deleting (or has finished), so it can't be cancelled: it completes, or waits while it is held.",
        ));
    };
    if let (true, Some(u)) = (was.suspended_here, was.subject) {
        crate::store::control::USER_SET_SUSPENDED.on(control).bind(("id", u)).bind(("suspended", false)).await?.check()?;
    }
    Ok(())
}

/// After a backup restore: erases again (a) every live account whose reference is on the erasure list,
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
        let (pending, held) = pending_and_held(&state.control, u).await?;
        match plan(&state.control, u, &pending, &held, true, true).await {
            Ok(p) if !p.is_empty() => match enqueue_erasure(state, u, Requester::Reapply, &p).await {
                Ok(_) => queued += 1,
                Err(e) => tracing::warn!(error = %e.message, "could not queue a re-applied erasure; it is retried at the next start"),
            },
            Ok(_) => tracing::warn!("a restored account that was erased belongs to no workspace; an operator erases it"),
            Err(e) => tracing::warn!(error = %e.message, "a restored account that was erased cannot be erased again automatically; an operator erases it"),
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
        let members: Vec<Member> = q::ORG_MEMBERSHIPS.on(&state.control).bind(("org", org.clone())).await?.take(0)?;
        let heir = members.iter().find(|m| m.role == "owner").or_else(|| members.first()).map(|m| m.user.clone());
        for gone in candidates.into_iter().filter(|c| !existing.contains(c)) {
            let p = vec![OrgPlan { org: id, heir: heir.clone(), close: false, transfer: false, joint: Vec::new() }];
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
    /// `self`, `operator` or `reapply` (a restore brought the person back and they were erased again).
    pub requested_by: String,
    pub status: String,
    /// When it took its first irreversible step; none while it can still be cancelled.
    #[serde(default)]
    #[surreal(default)]
    #[schema(value_type = Option<String>)]
    pub deleting_at: Option<Datetime>,
    /// Rows removed or anonymised per category.
    #[schema(value_type = Object)]
    pub counts: serde_json::Value,
    /// Rows still naming the person per table after the erasure: every value is 0 when it passed.
    #[schema(value_type = Object)]
    pub post_check: serde_json::Value,
    /// Mentions of the person's email left in content other people wrote, for review.
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

/// A row of the person's own activity about their data.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, utoipa::ToSchema)]
pub struct ActivityRow {
    pub action: String,
    pub outcome: String,
    #[serde(default)]
    #[surreal(default)]
    pub detail: String,
    #[schema(value_type = Option<String>)]
    pub created_at: Option<Datetime>,
}

pub async fn member_activity(control: &ControlDb, user: &RecordId) -> AppResult<Vec<ActivityRow>> {
    Ok(q::MEMBER_ACTIVITY.on(control).bind(("u", user.clone())).await?.take(0)?)
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

    #[test]
    fn only_a_recent_sign_in_confirms_an_account_with_no_password() {
        let now = chrono::Utc::now();
        assert!(recent(Some(now - chrono::Duration::seconds(30))));
        assert!(!recent(Some(now - chrono::Duration::seconds(REAUTH_WINDOW_SECS + 5))));
        assert!(!recent(None));
    }

    #[test]
    fn every_erasure_covers_the_whole_account() {
        assert_eq!(Requester::Myself.by(), "self");
        assert_eq!(Requester::Operator(crate::rid::parse("user:o").unwrap()).by(), "operator");
        assert_eq!(Requester::Reapply.by(), "reapply");
    }
}
