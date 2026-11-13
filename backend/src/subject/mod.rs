//! Data-subject rights: a full export of what Eunomia holds about one person, and a verified
//! erasure of that person. Design, the inventory of where a person's data lives and what each step
//! does with it: docs/privacy/subject-rights.md.
//!
//! Who may do what is the core of this module:
//!
//! - **The person, or a server operator** (`authz::is_operator`, an email in `EUNOMIA_ADMIN_EMAILS`;
//!   not the in-app admin flag) may erase the whole account: every workspace it belongs to, then the
//!   account. A workspace whose only member is the person closes with them, after an explicit
//!   confirmation.
//! - **A workspace admin** may erase a person from *their own* workspace only: the membership, the
//!   person's content and files there and what the system holds about them there. The account and every
//!   other workspace are untouched, unless no workspace is left and the account was made by this
//!   workspace's sign-up or invitation (its oldest membership); otherwise the completion record says the
//!   account was kept.
//!
//! The `erasure` row and its job are created in one transaction; the job ([`erase`]) works from a
//! working record, takes a lock on every workspace it touches, and proves the result with a post-check.
//! The row's `subject_ref` (a keyed hash of the user id) is the tombstone for resync suppression and
//! the restore re-apply ([`reapply_erasures`]).

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

/// True when `user` has an erasure on record (requested, running, held or done) that covers workspace
/// `org`. A sync for such an account in that workspace stands down, so a resync cannot bring its
/// records back as theirs.
pub async fn is_erased(control: &ControlDb, settings: &Settings, user: &RecordId, org: &OrgId) -> bool {
    match q::ERASURE_HAS_REF.on(control).bind(("refs", subject_refs(settings, user))).bind(("org", org.key())).await.and_then(|mut r| r.take::<Vec<RecordId>>(0)) {
        Ok(rows) => !rows.is_empty(),
        Err(e) => {
            // the table is there from control migration 0011 on; any other failure: do not block syncs on it
            tracing::warn!(error = %e, "erasure tombstone check failed");
            false
        }
    }
}

fn hold_error() -> AppError {
    AppError::coded(ErrorCode::SubjectLegalHold, "This account is under a legal hold, so it can't be erased. The workspace owner or a server operator can lift the hold.")
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

/// How the person asking proved it is them (self-service) or that they mean it (an admin or operator).
/// For an account with no password (single sign-on only) the proof is a sign-in within
/// [`REAUTH_WINDOW_SECS`]: `signed_in_at` is when the browser session making the request was created.
pub enum Confirmation<'a> {
    /// The account's own password, typed again.
    SelfService { password: Option<&'a str>, signed_in_at: Option<chrono::DateTime<chrono::Utc>> },
    /// The acting admin's or operator's own password, typed again.
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
    /// Inherits org-level rows; none when the workspace closes.
    pub heir: Option<RecordId>,
    /// The person is its only member: it closes with them.
    pub close: bool,
    /// The person owns it and nobody who stays does: the heir is made the owner first (a restore
    /// re-apply, or everyone left in it is being erased too).
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
        "This account owns a workspace that has other members, and nobody else owns it. The owner (or a server operator) makes one of them the workspace owner first (Settings > People, Make workspace owner); then the account can be deleted.",
    )
}

#[derive(Deserialize, SurrealValue)]
struct Member {
    user: RecordId,
    #[serde(default)]
    #[surreal(default)]
    role: String,
}

/// The plan per workspace `user` belongs to (only `only` when given: a workspace admin's erasure).
/// `pending` are people with an erasure open: they inherit nothing. `allow_close` is the confirmation
/// that a workspace whose only member is `user` closes. `reapply` (a restore re-apply) hands ownership
/// over instead of refusing.
pub async fn plan(control: &ControlDb, user: &RecordId, pending: &[RecordId], allow_close: bool, reapply: bool, only: Option<&OrgId>) -> AppResult<Vec<OrgPlan>> {
    let orgs: Vec<RecordId> = q::USER_ORGS.on(control).bind(("u", user.clone())).await?.take(0)?;
    let mut out = Vec::new();
    for org in orgs {
        let Some(id) = crate::rid::key_string(&org.key).and_then(|k| OrgId::parse(&k)) else { continue };
        if only.is_some_and(|o| o != &id) {
            continue;
        }
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
        let staying: Vec<&&Member> = others.iter().filter(|m| !pending.contains(&m.user)).collect();
        if staying.is_empty() {
            // everyone left is being erased too: this one goes first, owned by one of them meanwhile, and
            // the last of them closes the workspace
            let next = others.iter().find(|m| m.role == "owner").unwrap_or(&others[0]);
            out.push(OrgPlan { org: id, heir: Some(next.user.clone()), close: false, transfer: owns, joint: others.iter().map(|m| m.user.clone()).collect() });
            continue;
        }
        let other_owner = staying.iter().find(|m| m.role == "owner");
        let (heir, transfer) = match other_owner {
            Some(o) => (o.user.clone(), false),
            None if !owns => (staying[0].user.clone(), false),
            None if reapply => (staying[0].user.clone(), true),
            None => return Err(transfer_first()),
        };
        out.push(OrgPlan { org: id, heir: Some(heir), close: false, transfer, joint: Vec::new() });
    }
    Ok(out)
}

/// What kind of erasure, and who asked.
#[derive(Debug, Clone)]
pub enum Requester {
    /// The person: the whole account.
    Myself,
    /// A server operator: the whole account.
    Operator(RecordId),
    /// A workspace admin: only that workspace.
    Admin(RecordId, OrgId),
    /// The restore re-apply of a finished erasure, with its scope.
    Reapply { workspace: Option<OrgId> },
}

impl Requester {
    fn by(&self) -> &'static str {
        match self {
            Requester::Myself => "self",
            Requester::Operator(_) => "operator",
            Requester::Admin(..) => "admin",
            Requester::Reapply { .. } => "reapply",
        }
    }

    /// The one workspace a workspace-scoped erasure covers.
    fn workspace(&self) -> Option<&OrgId> {
        match self {
            Requester::Admin(_, org) => Some(org),
            Requester::Reapply { workspace } => workspace.as_ref(),
            _ => None,
        }
    }

    fn scope(&self) -> &'static str {
        if self.workspace().is_some() { "workspace" } else { "account" }
    }
}

/// The request-time checks: a hold, the account that set up the server (for a whole-account erasure),
/// and the plan.
pub async fn check_erasable(control: &ControlDb, user: &RecordId, who: &Requester, allow_close: bool) -> AppResult<Vec<OrgPlan>> {
    let row = confirm_row(control, user).await?;
    if row.legal_hold_at.is_some() {
        return Err(hold_error());
    }
    if who.workspace().is_none() && crate::capsules::first_user(control).await?.as_ref() == Some(user) {
        // the oldest account is always an admin (`authz::is_admin`): erasing it would quietly make the next one an admin
        return Err(AppError::coded(
            ErrorCode::SubjectLastOwner,
            "This is the account that set up the server, so it can't be erased in the app. The server's operator can remove it (docs/privacy/subject-rights.md).",
        ));
    }
    let pending: Vec<RecordId> = q::OPEN_SUBJECTS.on(control).await?.take::<Vec<RecordId>>(0)?.into_iter().filter(|u| u != user).collect();
    // only the person or an operator closes a workspace, and only one whose only member is the person
    let allow_close = allow_close && matches!(who, Requester::Myself | Requester::Operator(_));
    let plans = plan(control, user, &pending, allow_close, false, who.workspace()).await?;
    if plans.is_empty() {
        return Err(AppError::coded(ErrorCode::TenantNotFound, "This account does not belong to that workspace."));
    }
    Ok(plans)
}

/// An erasure as the routes report it.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, utoipa::ToSchema)]
pub struct ErasureTicket {
    pub id: String,
    /// `pending`, `running`, `held`, `done`, `failed` or `cancelled`.
    pub status: String,
    /// `account` (every workspace and the account) or `workspace` (one workspace).
    #[serde(default)]
    #[surreal(default)]
    pub scope: String,
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
    // a repeat request joins the open erasure (a workspace erasure already took the person out of the
    // workspace, so it would not plan again); it can add a confirmation that a workspace closes
    if open_ticket(&state.control, &subject_refs(&state.settings, user)).await?.is_some() {
        let plans = match &who {
            Requester::Myself | Requester::Operator(_) if allow_close => {
                let pending: Vec<RecordId> = q::OPEN_SUBJECTS.on(&state.control).await?.take::<Vec<RecordId>>(0)?.into_iter().filter(|u| u != user).collect();
                plan(&state.control, user, &pending, true, false, None).await.unwrap_or_default()
            }
            _ => Vec::new(),
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

/// Creates (or joins) the erasure of `user` and makes sure it has a live job. A whole-account erasure
/// suspends the account and signs it out everywhere at once; a workspace erasure takes the person out
/// of that workspace at once. The request is recorded in the workspace's activity log.
pub(crate) async fn enqueue_erasure(state: &AppState, user: &RecordId, who: Requester, plans: &[OrgPlan]) -> AppResult<ErasureTicket> {
    let reference = subject_ref(&state.settings, user);
    let refs = subject_refs(&state.settings, user);
    let closes: Vec<String> = plans.iter().filter(|p| p.close).map(|p| p.org.key()).collect();
    let ticket = match open_ticket(&state.control, &refs).await? {
        Some(t) => {
            // a repeat request adds its confirmation that a workspace closes, and can widen a workspace erasure to the account
            if !closes.is_empty() {
                q::ERASURE_ADD_CLOSES.on(&state.control).bind(("id", t.id.clone())).bind(("closes", closes.clone())).await?.check()?;
            }
            if who.scope() == "account" && t.scope == "workspace" {
                return Err(AppError::coded(
                    ErrorCode::DbConflict,
                    "An admin's erasure of this account from one workspace is still running. Try again when it has finished.",
                ));
            }
            ensure_job(&state.control, &t.id).await?;
            t
        }
        None => match create(state, user, &who, plans, &reference).await {
            Ok(t) => t,
            // two requests at once for one person: the unique open slot keeps one, the other joins it
            Err(e) if e.code == ErrorCode::DbDuplicate || e.code == ErrorCode::DbConflict => {
                let t = open_ticket(&state.control, &refs).await?.ok_or(e)?;
                ensure_job(&state.control, &t.id).await?;
                t
            }
            Err(e) => return Err(e),
        },
    };
    match who.workspace() {
        None => {
            // nothing new is written while it runs (each step is idempotent: a request that failed here is finished by the next)
            crate::store::control::USER_SET_SUSPENDED.on(&state.control).bind(("id", user.clone())).bind(("suspended", true)).await?.check()?;
            crate::store::control::AUTH_SESSION_REVOKE_OWNER.on(&state.control).bind(("owner", user.clone())).await?.check()?;
        }
        Some(org) => {
            // out of this workspace at once; the account and other workspaces are untouched
            crate::store::control::MEMBERSHIP_DELETE_ORG.on(&state.control).bind(("user", user.clone())).bind(("org", org.record())).await?.check()?;
        }
    }
    state.credentials.forget_user(user);
    // the workspace's activity log keeps who asked; the person appears only as the opaque reference
    let scope = match &who {
        Requester::Admin(a, _) | Requester::Operator(a) => Some(a.clone()),
        _ => plans.iter().find_map(|p| p.heir.clone()),
    }
    .unwrap_or_else(|| RecordId::new("user", "erased"));
    let actor = crate::authz::current().map(|c| c.actor).unwrap_or_else(|| crate::authz::Caller::system().actor);
    let detail = format!("by={} scope={} ref={}", who.by(), who.scope(), &reference[..12]);
    crate::audit::record(&state.control, crate::audit::Event { user: Some(&scope), actor: &actor, action: "subject.erase_request", target: &ticket.id, outcome: "ok", detail: &detail })
        .await;
    if let Requester::Admin(..) | Requester::Operator(_) = who {
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

async fn create(state: &AppState, user: &RecordId, who: &Requester, plans: &[OrgPlan], reference: &str) -> AppResult<ErasureTicket> {
    let home = plans.first().map(|p| p.org).ok_or_else(|| AppError::internal("no org to erase in"))?;
    // the workspace whose sign-up or invitation made the account: its oldest membership
    let first: Vec<RecordId> = q::USER_ORGS.on(&state.control).bind(("u", user.clone())).await?.take(0)?;
    let first_org = first.first().and_then(|o| crate::rid::key_string(&o.key));
    let eid = RecordId::new("erasure", uuid::Uuid::new_v4().simple().to_string());
    let key = format!("{}:{}", jobs::kind::SUBJECT_ERASE, eid.to_string());
    let jid = RecordId::new("job", crate::tx::stable_key('j', &key));
    let payload = json!({
        "erasure": eid.to_string(),
        "first_org": first_org,
        "orgs": plans.iter().map(|p| json!({ "org": p.org.key(), "heir": p.heir.as_ref().map(|h| h.to_string()), "close": p.close })).collect::<Vec<_>>(),
    });
    let closes: Vec<String> = plans.iter().filter(|p| p.close).map(|p| p.org.key()).collect();
    let requester = match who {
        Requester::Admin(a, _) | Requester::Operator(a) => Some(a.clone()),
        _ => None,
    };
    let mut res = q::ERASURE_CREATE
        .on(&state.control)
        .bind(("eid", eid))
        .bind(("org", home.key()))
        .bind(("ref", reference.to_string()))
        .bind(("u", user.clone()))
        .bind(("by", who.by()))
        .bind(("scope", who.scope()))
        .bind(("requester", requester))
        .bind(("first_org", first_org.clone()))
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

/// May `actor` act as the owner of `org` (lift a hold, hand ownership over)? The owner or a server operator.
pub async fn owner_or_operator(control: &ControlDb, actor: &crate::models_user::User, org: &OrgId) -> AppResult<bool> {
    Ok(crate::authz::is_operator(actor) || owner_of(control, org).await?.as_ref() == Some(&actor.id))
}

/// Places or lifts a legal hold on `user`. Any admin places one; only the workspace's owner or a
/// server operator lifts it. Lifting it resumes an erasure the hold parked. Returns the hold time and
/// whether an erasure is open.
pub async fn set_legal_hold(control: &ControlDb, actor: &crate::models_user::User, user: &RecordId, hold: bool) -> AppResult<(Option<Datetime>, bool)> {
    if !hold && !owner_or_operator(control, actor, &actor.org).await? {
        return Err(AppError::coded(ErrorCode::AuthForbidden, "Only the workspace owner or a server operator can lift a legal hold."));
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

/// True while a whole-account erasure of `user` is open: the account stays suspended (no restore of
/// access, no sign-in) until it finishes or an operator cancels it.
pub async fn account_erasure_open(control: &ControlDb, user: &RecordId) -> AppResult<bool> {
    let open: Vec<RecordId> = q::ACCOUNT_ERASURE_OPEN_OF.on(control).bind(("u", user.clone())).await?.take(0)?;
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

/// An operator stops an erasure that is not running: the working record and the person's id are
/// cleared. The account stays suspended; an admin can restore it afterwards.
pub async fn cancel_erasure(control: &ControlDb, org: &OrgId, erasure: &str) -> AppResult<()> {
    let id = crate::rid::parse(erasure).map_err(|_| AppError::coded(ErrorCode::ResourceNotFound, "No such erasure."))?;
    let mine: Vec<RecordId> = q::ERASURE_IN_ORG.on(control).bind(("id", id.clone())).bind(("org", org.key())).await?.take(0)?;
    if mine.is_empty() {
        return Err(AppError::coded(ErrorCode::ResourceNotFound, "No such erasure."));
    }
    let mut res = q::ERASURE_CANCEL.on(control).bind(("id", id)).await?;
    let done: Vec<String> = res.take(q::ERASURE_CANCEL.slot)?;
    if done.is_empty() {
        return Err(AppError::coded(ErrorCode::DbConflict, "That erasure is running or already finished, so it can't be cancelled."));
    }
    Ok(())
}

/// After a backup restore: erases again (a) every live account whose reference is on the erasure list
/// (with the scope it had: a workspace erasure again only where its workspace has the person back), and
/// (b) in every workspace, rows whose owner has no account but whose reference is on the list (a restore
/// of only an org database). The re-apply hands workspace ownership over instead of leaving a workspace
/// without an owner, closes a workspace again when the person is its only member, and does not consult
/// holds (the person was already erased once). One failure is logged and the rest go on.
pub async fn reapply_erasures(state: &AppState) -> AppResult<usize> {
    #[derive(Deserialize, SurrealValue)]
    struct Done {
        subject_ref: String,
        #[serde(default)]
        #[surreal(default)]
        scope: String,
        #[serde(default)]
        #[surreal(default)]
        org: String,
    }
    let done: Vec<Done> = q::ERASURE_REFS.on(&state.control).await?.take(0)?;
    if done.is_empty() {
        return Ok(0);
    }
    // reference -> the workspaces a workspace-scoped erasure covered (None: the whole account)
    let mut scopes: std::collections::HashMap<String, Option<Vec<String>>> = std::collections::HashMap::new();
    for d in done {
        let e = scopes.entry(d.subject_ref).or_insert_with(|| Some(Vec::new()));
        match (d.scope.as_str(), e.as_mut()) {
            ("workspace", Some(list)) => list.push(d.org),
            _ => *e = None,
        }
    }
    let find = |u: &RecordId| subject_refs(&state.settings, u).into_iter().find_map(|r| scopes.get(&r).cloned());
    let mut queued = 0;

    let users: Vec<RecordId> = q::USERS_WITH_ORG.on(&state.control).await?.take(0)?;
    for u in &users {
        let Some(scope) = find(u) else { continue };
        let pending: Vec<RecordId> = q::OPEN_SUBJECTS.on(&state.control).await?.take::<Vec<RecordId>>(0)?.into_iter().filter(|p| p != u).collect();
        let targets: Vec<Option<OrgId>> = match &scope {
            None => vec![None],
            Some(orgs) => orgs.iter().filter_map(|o| OrgId::parse(o)).map(Some).collect(),
        };
        for workspace in targets {
            let p = match plan(&state.control, u, &pending, true, true, workspace.as_ref()).await {
                Ok(p) if !p.is_empty() => p,
                Ok(_) => continue, // not back in that workspace
                Err(e) => {
                    tracing::warn!(error = %e.message, "a restored account that was erased cannot be erased again automatically; erase it from Settings > People");
                    continue;
                }
            };
            match enqueue_erasure(state, u, Requester::Reapply { workspace }, &p).await {
                Ok(_) => queued += 1,
                Err(e) => tracing::warn!(error = %e.message, "could not queue a re-applied erasure; it is retried at the next start"),
            }
            break; // one open erasure per person: the next workspace is re-applied at the next start
        }
    }

    // a restore of only an org database: rows whose owner the control database no longer has
    let orgs: Vec<RecordId> = q::READY_ORGS.on(&state.control).await?.take(0)?;
    for org in orgs {
        let Some(id) = crate::rid::key_string(&org.key).and_then(|k| OrgId::parse(&k)) else { continue };
        let Ok(org_state) = state.org(&id).await else { continue };
        let owners: Vec<RecordId> = q::ROW_OWNERS.on(&org_state.db).await?.take(0)?;
        let candidates: Vec<RecordId> = owners
            .into_iter()
            .filter(|o| match find(o) {
                None => false,
                Some(None) => true,
                Some(Some(orgs)) => orgs.contains(&id.key()),
            })
            .collect();
        if candidates.is_empty() {
            continue;
        }
        let existing: Vec<RecordId> = q::USERS_EXISTING.on(&state.control).bind(("ids", candidates.clone())).await?.take(0)?;
        let members: Vec<Member> = q::ORG_MEMBERSHIPS.on(&state.control).bind(("org", org.clone())).await?.take(0)?;
        let heir = members.iter().find(|m| m.role == "owner").or_else(|| members.first()).map(|m| m.user.clone());
        for gone in candidates.into_iter().filter(|c| !existing.contains(c)) {
            let p = vec![OrgPlan { org: id, heir: heir.clone(), close: false, transfer: false, joint: Vec::new() }];
            match enqueue_erasure(state, &gone, Requester::Reapply { workspace: Some(id) }, &p).await {
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
    /// `self`, `admin`, `operator` or `reapply` (a restore brought the person back and they were erased again).
    pub requested_by: String,
    /// `account` or `workspace`.
    #[serde(default)]
    #[surreal(default)]
    pub scope: String,
    pub status: String,
    /// Rows removed or anonymised per category.
    #[schema(value_type = Object)]
    pub counts: serde_json::Value,
    /// Rows still naming the person per table after the erasure: every value is 0 when it passed.
    #[schema(value_type = Object)]
    pub post_check: serde_json::Value,
    /// Mentions of the person's email left in content other people wrote, for review, and for a
    /// workspace erasure whether the account was kept.
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
    fn an_admin_erasure_is_one_workspace_and_never_the_account_scope() {
        let org = OrgId::new();
        let admin = Requester::Admin(crate::rid::parse("user:a").unwrap(), org);
        assert_eq!((admin.scope(), admin.workspace()), ("workspace", Some(&org)));
        assert_eq!(Requester::Myself.scope(), "account");
        assert_eq!(Requester::Operator(crate::rid::parse("user:o").unwrap()).scope(), "account");
    }
}
