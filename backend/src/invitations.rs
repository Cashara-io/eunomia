//! Organisation invitations: how an owner or admin of one org brings a colleague into that org on a
//! host that runs many orgs (docs/architecture/tenancy.md, "Organisation invitations").
//!
//! - An invitation names an email (normalized like `user.email_lc`) and a role (`member` or `admin`).
//!   Its token is 32 random bytes; only the sha256 is stored (`org_invitation.token_hash`), and the token
//!   is never logged or written to the audit ledger. It expires [`TTL_DAYS`] after it was (re)sent and
//!   is single use.
//! - The product sends no email: the admin gets the link and sends it themselves.
//! - A valid invitation is itself the permission to create an account, whatever `SIGNUP` says, and the
//!   account lands in the inviting org (not an org of its own under `EUNOMIA_SIGNUP_ORG=personal`).
//! - An invitation is only ever used through its token: password signup sends it, and single sign-on
//!   carries its hash in the signed flow cookie from the invitation page's form post. Nothing looks an
//!   invitation up by email, so a second org's invitation can never capture someone another org invited.
//! - An address in `EUNOMIA_ADMIN_EMAILS` is never invitable ([`create`]) and never accepted
//!   ([`check_usable`]); both refusals say nothing about why.
//! - Only the org's owner (or an instance admin) manages `admin` invitations; an org admin, `member` ones.
//! - An account belongs to one org for now (a credential resolves to the oldest membership, with no org
//!   switcher; see `models_user::org_of`). So an account already in another org cannot accept: it gets
//!   `invitation.other_org`.
//! - Acceptance is one transaction ([`store::invitations::CLAIM`]): two accepts of one token make one
//!   membership.
//! - Every admin read and write names the admin's own org, so an id from another org is "not found".

use serde::{Deserialize, Serialize};
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use crate::audit::{self, Event};
use crate::authz::{self, Actor};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::{self, User};
use crate::pool::{ControlDb, OrgId};
use crate::ratelimit;
use crate::rid::RecordIdExt;
use crate::state::AppState;
use crate::store;
use crate::store::invitations as q;

/// Days an invitation link stays valid after it was made or resent.
pub const TTL_DAYS: i64 = 7;

/// The role an invitation gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, utoipa::ToSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum OrgInviteRole {
    #[default]
    Member,
    /// Can invite people to the org and see its seats (not an instance admin).
    Admin,
}

impl OrgInviteRole {
    pub fn as_str(self) -> &'static str {
        match self {
            OrgInviteRole::Member => "member",
            OrgInviteRole::Admin => "admin",
        }
    }
}

#[derive(Debug, Deserialize, SurrealValue)]
struct Row {
    id: RecordId,
    email: String,
    role: String,
    created_at: Datetime,
    sent_at: Datetime,
    expires_at: Datetime,
    #[serde(default)]
    #[surreal(default)]
    expired: bool,
    #[serde(default)]
    #[surreal(default)]
    invited_by: Option<String>,
}

/// An open invitation, as an org admin sees it. Never carries the token.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OrgInvitation {
    pub id: String,
    pub email: String,
    /// `member` or `admin`.
    pub role: String,
    #[schema(value_type = String)]
    pub created_at: Datetime,
    /// When the current link was made (a resend makes a new one).
    #[schema(value_type = String)]
    pub sent_at: Datetime,
    #[schema(value_type = String)]
    pub expires_at: Datetime,
    /// The link has expired; resend it to make a new one.
    pub expired: bool,
    /// The email of the admin who sent it.
    pub invited_by: String,
}

impl From<Row> for OrgInvitation {
    fn from(r: Row) -> Self {
        OrgInvitation {
            id: r.id.to_string(),
            email: r.email,
            role: r.role,
            created_at: r.created_at,
            sent_at: r.sent_at,
            expires_at: r.expires_at,
            expired: r.expired,
            invited_by: r.invited_by.unwrap_or_default(),
        }
    }
}

/// A new or resent invitation, with its link. The link is shown once: only its hash is kept.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OrgInvitationLink {
    pub invitation: OrgInvitation,
    /// `<frontend>/invite#<token>`. The token rides in the fragment, which browsers never send to a server.
    pub link: String,
    /// The token alone, for clients that build their own link.
    pub token: String,
}

/// OrgSeats for one org (the hosted business bills on these). Counts only.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OrgSeats {
    /// Members whose account is not suspended.
    pub active_members: i64,
    pub suspended_members: i64,
    /// Open invitations that have not expired.
    pub pending_invitations: i64,
}

/// An invitation found by its token or email, before it is accepted.
#[derive(Debug, Clone, Deserialize, SurrealValue)]
pub struct Pending {
    pub id: RecordId,
    pub org: RecordId,
    #[serde(default)]
    #[surreal(default)]
    pub org_name: Option<String>,
    pub email: String,
    pub role: String,
    pub invited_by: RecordId,
    /// The hash of the link this acceptance started from: the claim only succeeds while it is current.
    pub token_hash: String,
    #[serde(default)]
    #[surreal(default)]
    accepted_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    revoked_at: Option<Datetime>,
    pub expires_at: Datetime,
    #[serde(default)]
    #[surreal(default)]
    pub expired: bool,
}

impl Pending {
    pub fn org_id(&self) -> Option<OrgId> {
        crate::rid::key_string(self.org.key()).and_then(|k| OrgId::parse(&k))
    }

    fn open(&self) -> bool {
        self.accepted_at.is_none() && self.revoked_at.is_none()
    }
}

fn not_found() -> AppError {
    AppError::coded(ErrorCode::InvitationNotFound, "This invitation link isn't valid any more. It may have been used or withdrawn. Ask for a new one.")
}

fn admin_not_found() -> AppError {
    AppError::coded(ErrorCode::InvitationNotFound, "That invitation isn't open any more.")
}

/// The organisation's name as its admins chose it, or `None` for a personal workspace, which is named after
/// its founder's email (provisioning does that): that address is not shown to whoever holds a link.
fn shown_org_name(p: &Pending) -> Option<String> {
    p.org_name.as_deref().map(str::trim).filter(|n| !n.is_empty() && !n.contains('@')).map(str::to_string)
}

fn expired_error(p: &Pending) -> AppError {
    // the organisation, not the inviter's email: nobody verified that address
    let org = shown_org_name(p).map(|n| format!("the organisation called \u{201c}{n}\u{201d}")).unwrap_or_else(|| "the workspace".into());
    AppError::coded(ErrorCode::InvitationExpired, format!("This invitation has expired. Ask an admin of {org} to send a new one."))
}

/// The email check every accept makes: the accepting account's email, normalized, is the invited one.
pub fn email_matches(p: &Pending, email: &str) -> AppResult<()> {
    // a look-alike that only lowercases into the invited address is not the invited address
    if models_user::case_folds_cleanly(email.trim()) && models_user::normalize_email(email) == p.email {
        Ok(())
    } else {
        Err(AppError::coded(ErrorCode::InvitationEmailMismatch, format!("This invitation is for {}. Use that email address to accept it.", p.email)))
    }
}

/// The caller's role in their own org: `owner`, `admin` or `member` (none if they have no membership).
pub async fn org_role(control: &ControlDb, user: &User) -> AppResult<Option<String>> {
    let mut res = q::ROLE_IN_ORG.on(control).bind(("user", user.id.clone())).bind(("org", user.org.record())).await?;
    Ok(res.take::<Vec<String>>(0)?.into_iter().next())
}

/// How much of the org's invitations a caller may manage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Manager {
    /// The org's owner, or an instance admin acting in their own org: every invitation, any role.
    Owner,
    /// An org admin: invitations for members only.
    Admin,
}

impl Manager {
    /// The invitation roles this manager may create, resend or withdraw.
    fn roles(self) -> Vec<String> {
        match self {
            Manager::Owner => vec!["member".into(), "admin".into()],
            Manager::Admin => vec!["member".into()],
        }
    }

    fn require_role(self, role: &str) -> AppResult<()> {
        if self.roles().iter().any(|r| r == role) {
            Ok(())
        } else {
            Err(AppError::coded(ErrorCode::AuthForbidden, "Only the organisation's owner can invite admins or manage admin invitations."))
        }
    }
}

async fn manager(control: &ControlDb, user: &User) -> AppResult<Option<Manager>> {
    if authz::is_admin(control, user).await? {
        return Ok(Some(Manager::Owner));
    }
    Ok(match org_role(control, user).await?.as_deref() {
        Some("owner") => Some(Manager::Owner),
        Some("admin") => Some(Manager::Admin),
        _ => None,
    })
}

/// May `user` manage their org's invitations? An owner or admin of the org, or an instance admin.
pub async fn can_manage(control: &ControlDb, user: &User) -> AppResult<bool> {
    Ok(manager(control, user).await?.is_some())
}

async fn require_manager(state: &AppState, user: &User) -> AppResult<Manager> {
    manager(&state.control, user)
        .await?
        .ok_or_else(|| AppError::coded(ErrorCode::AuthForbidden, "Only an owner or admin of this organisation can manage its invitations."))
}

/// The answer for an address that may not be invited. One text whatever the reason, so it says nothing
/// about the address; the reason goes to the server log.
fn uninvitable() -> AppError {
    AppError::coded(ErrorCode::ValidationInvalid, "This address can't be invited. Check it, or ask whoever runs this Eunomia server.")
}

/// The role a caller may change on an open invitation of their org, read before the change.
async fn require_role_of(state: &AppState, by: &User, m: Manager, id: &RecordId) -> AppResult<()> {
    let mut res = q::ROLE_OF.on(&state.control).bind(("id", id.clone())).bind(("org", by.org.record())).await?;
    let role = res.take::<Vec<String>>(0)?.into_iter().next().ok_or_else(admin_not_found)?;
    m.require_role(&role)
}

fn rate(state: &AppState, key: String, per_min: u32) -> AppResult<()> {
    state.invite_limiter.check(&key, per_min).map_err(|wait| {
        let mut e = AppError::coded(ErrorCode::RateLimited, "Too many invitations at once. Wait a minute, then try again.");
        e.retry_after_secs = Some(wait);
        e
    })
}

/// Creating and resending share one bucket per org and one per admin.
fn creation_rate(state: &AppState, by: &User) -> AppResult<()> {
    rate(state, format!("org-invite:{}", by.org.key()), ratelimit::INVITES_PER_MIN)?;
    rate(state, format!("org-invite-by:{}", by.id.to_string()), ratelimit::INVITES_PER_MIN)
}

fn link(state: &AppState, token: &str) -> String {
    format!("{}/invite#{token}", crate::oauth::frontend_url(&state.settings))
}

async fn record(control: &ControlDb, scope: &RecordId, actor: &Actor, action: &str, target: &str, outcome: &str, detail: &str) {
    audit::record(control, Event { user: Some(scope), actor, action, target, outcome, detail }).await;
}

fn caller_actor(user: &RecordId) -> Actor {
    authz::current().map(|c| c.actor).unwrap_or_else(|| Actor { kind: "user", id: user.to_string() })
}

/// Invite `email` to the admin's org with `role`.
pub async fn create(state: &AppState, by: &User, email: &str, role: OrgInviteRole) -> AppResult<OrgInvitationLink> {
    let m = require_manager(state, by).await?;
    m.require_role(role.as_str())?;
    let email = models_user::normalize_new_email(email)?;
    creation_rate(state, by)?;
    // an instance operator's address is never invitable: whoever holds the link would become an operator
    if authz::operator_email_listed(&email) {
        tracing::info!("organisation invitation refused: the address is in EUNOMIA_ADMIN_EMAILS");
        return Err(uninvitable());
    }
    // an existing member of this org: the admin already sees them in People. An account in another org
    // is not looked at, so the answer is the same as for an email with no account.
    let mut res = store::control::EMAIL_IN_ORG.on(&state.control).bind(("email", email.clone())).bind(("org", by.org.record())).await?;
    #[derive(Deserialize, SurrealValue)]
    struct IdRow {
        #[allow(dead_code)]
        id: RecordId,
    }
    if !res.take::<Vec<IdRow>>(0)?.is_empty() {
        return Err(AppError::coded(ErrorCode::InvitationExists, "That person is already in this organisation."));
    }
    let token = models_user::generate_token();
    let created: AppResult<surrealdb::IndexedResults> = async { Ok(q::CREATE
        .on(&state.control)
        .bind(("org", by.org.record()))
        .bind(("email", email.clone()))
        .bind(("role", role.as_str().to_string()))
        .bind(("hash", models_user::hash_token(&token)))
        .bind(("by", by.id.clone()))
        .bind(("ttl", format!("{TTL_DAYS}d")))
        .await?
        .check()?) }
    .await;
    let mut res = match created {
        Ok(r) => r,
        // the unique open slot: someone already invited this email here
        Err(e) if e.code == ErrorCode::DbDuplicate => {
            return Err(AppError::coded(ErrorCode::InvitationExists, "There's already an open invitation for that email. Resend it instead."));
        }
        Err(e) => return Err(e),
    };
    let row: Row = res.take::<Vec<Row>>(0)?.into_iter().next().ok_or_else(|| AppError::internal("invitation insert returned no row"))?;
    let actor = caller_actor(&by.id);
    record(&state.control, &by.id, &actor, "org.invite", &row.id.to_string(), "ok", &format!("email={email} role={}", role.as_str())).await;
    Ok(OrgInvitationLink { invitation: row.into(), link: link(state, &token), token })
}

/// The admin's org's open invitations, newest first.
pub async fn list(state: &AppState, by: &User) -> AppResult<Vec<OrgInvitation>> {
    let _ = require_manager(state, by).await?;
    let mut res = q::LIST.on(&state.control).bind(("org", by.org.record())).await?;
    Ok(res.take::<Vec<Row>>(0)?.into_iter().map(OrgInvitation::from).collect())
}

fn invitation_id(id: &str) -> AppResult<RecordId> {
    crate::rid::parse(id).ok().filter(|r: &RecordId| r.table() == "org_invitation").ok_or_else(admin_not_found)
}

/// A new link (and a new expiry) for one of the admin's org's open invitations; the old link stops working.
pub async fn resend(state: &AppState, by: &User, id: &str) -> AppResult<OrgInvitationLink> {
    let m = require_manager(state, by).await?;
    let id = invitation_id(id)?;
    require_role_of(state, by, m, &id).await?;
    creation_rate(state, by)?;
    let token = models_user::generate_token();
    let mut res = q::RESEND
        .on(&state.control)
        .bind(("id", id.clone()))
        .bind(("org", by.org.record()))
        .bind(("roles", m.roles()))
        .bind(("hash", models_user::hash_token(&token)))
        .bind(("ttl", format!("{TTL_DAYS}d")))
        .await?
        .check()?;
    let row: Row = res.take::<Vec<Row>>(0)?.into_iter().next().ok_or_else(admin_not_found)?;
    let actor = caller_actor(&by.id);
    record(&state.control, &by.id, &actor, "org.invite_resend", &row.id.to_string(), "ok", &format!("email={} role={}", row.email, row.role)).await;
    Ok(OrgInvitationLink { invitation: row.into(), link: link(state, &token), token })
}

/// Withdraws one of the admin's org's open invitations: its link stops working.
pub async fn revoke(state: &AppState, by: &User, id: &str) -> AppResult<()> {
    let m = require_manager(state, by).await?;
    let id = invitation_id(id)?;
    require_role_of(state, by, m, &id).await?;
    #[derive(Deserialize, SurrealValue)]
    struct Gone {
        id: RecordId,
        email: String,
        role: String,
    }
    let mut res = q::REVOKE.on(&state.control).bind(("id", id)).bind(("org", by.org.record())).bind(("roles", m.roles())).await?.check()?;
    let gone = res.take::<Vec<Gone>>(0)?.into_iter().next().ok_or_else(admin_not_found)?;
    let actor = caller_actor(&by.id);
    record(&state.control, &by.id, &actor, "org.invite_revoke", &gone.id.to_string(), "ok", &format!("email={} role={}", gone.email, gone.role)).await;
    Ok(())
}

/// Active members, suspended members and pending invitations of the admin's org.
pub async fn seats(state: &AppState, by: &User) -> AppResult<OrgSeats> {
    let _ = require_manager(state, by).await?;
    #[derive(Deserialize, SurrealValue)]
    struct N {
        n: i64,
    }
    let mut res = q::SEATS.on(&state.control).bind(("org", by.org.record())).await?;
    let mut n = |i: usize| -> AppResult<i64> { Ok(res.take::<Vec<N>>(i)?.first().map_or(0, |r| r.n)) };
    Ok(OrgSeats { active_members: n(0)?, suspended_members: n(1)?, pending_invitations: n(2)? })
}

/// Which throttle bucket a token attempt counts against: the client address for a signed-out visitor,
/// the account for a signed-in one.
pub enum Who<'a> {
    Address(&'a str),
    Account(&'a RecordId),
}

impl Who<'_> {
    fn key(&self) -> String {
        match self {
            Who::Address(ip) => format!("ip:{ip}"),
            Who::Account(u) => format!("user:{}", u.to_string()),
        }
    }
}

/// What the invitation page shows before anyone accepts.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InvitationLookup {
    /// The organisation's name as its admins chose it (not verified by anyone); empty for a personal
    /// workspace, whose name is its founder's email and is not shown.
    pub org_name: String,
    /// The invitation is to someone's personal workspace (its name is not shown).
    pub personal: bool,
    /// The invited email: the account that accepts must have this one.
    pub email: String,
    /// `member` or `admin`.
    pub role: String,
    #[schema(value_type = String)]
    pub expires_at: Datetime,
    /// `open`, or `expired` (ask for a new link).
    pub status: String,
}

/// The invitation `token` belongs to, in any state; counts a miss against `who`.
async fn by_token(state: &AppState, token: &str, who: &Who<'_>) -> AppResult<Option<Pending>> {
    let key = who.key();
    rate(state, format!("invite-accept:{key}"), ratelimit::INVITE_ACCEPTS_PER_MIN)?;
    let fail_key = format!("invite-fail:{key}");
    if let Err(wait) = state.fail_throttle.check(&fail_key, ratelimit::INVITE_TOKEN_FAILS) {
        let mut e = AppError::coded(ErrorCode::RateLimited, "Too many invitation links that didn't work. Wait a few minutes, then try again.");
        e.retry_after_secs = Some(wait);
        return Err(e);
    }
    let token = token.trim();
    let found = if token.is_empty() || token.len() > 128 {
        None
    } else {
        let mut res = q::BY_HASH.on(&state.control).bind(("hash", models_user::hash_token(token))).await?;
        res.take::<Vec<Pending>>(0)?.into_iter().next()
    };
    if !found.as_ref().is_some_and(|p| p.open() && !p.expired) {
        state.fail_throttle.fail(&fail_key);
    }
    Ok(found)
}

/// The invitation page's view of a token: unknown, used and withdrawn links are all "not found".
pub async fn lookup(state: &AppState, token: &str, who: Who<'_>) -> AppResult<InvitationLookup> {
    let p = by_token(state, token, &who).await?.filter(Pending::open).filter(|p| !authz::operator_email_listed(&p.email)).ok_or_else(not_found)?;
    let shown = shown_org_name(&p);
    Ok(InvitationLookup {
        personal: shown.is_none(),
        org_name: shown.unwrap_or_default(),
        email: p.email.clone(),
        role: p.role.clone(),
        expires_at: p.expires_at,
        status: if p.expired { "expired" } else { "open" }.into(),
    })
}

/// The open, unexpired invitation `token` belongs to. An expired one is refused with
/// `invitation.expired` and the refusal goes to the inviting org's audit log.
pub async fn usable(state: &AppState, token: &str, who: Who<'_>) -> AppResult<Pending> {
    let p = by_token(state, token, &who).await?.filter(Pending::open).ok_or_else(not_found)?;
    check_usable(&state.control, p).await
}

/// The sha256 a token is stored under.
pub fn token_hash(token: &str) -> String {
    models_user::hash_token(token.trim())
}

/// The invitation stored under `hash`, in any state (no throttle: the caller holds a hash it was given,
/// such as the signed single sign-on flow cookie, not a guess).
pub async fn by_hash(control: &ControlDb, hash: &str) -> AppResult<Option<Pending>> {
    let mut res = q::BY_HASH.on(control).bind(("hash", hash.to_string())).await?;
    Ok(res.take::<Vec<Pending>>(0)?.into_iter().next())
}

/// `p` if it can still be accepted: open, not for an instance operator's address (refused as not found)
/// and not expired (refused with `invitation.expired`; the refusal goes to the inviting org's log).
pub async fn check_usable(control: &ControlDb, p: Pending) -> AppResult<Pending> {
    if !p.open() {
        return Err(not_found());
    }
    if authz::operator_email_listed(&p.email) {
        tracing::info!("organisation invitation refused at acceptance: the address is in EUNOMIA_ADMIN_EMAILS");
        record_not_accepted(control, &p).await;
        return Err(not_found());
    }
    if p.expired {
        // scoped to the inviter so the inviting org's log shows it; the actor is whoever held the link
        record(control, &p.invited_by, &audit::anonymous(), "org.invite_expired", &p.id.to_string(), ErrorCode::InvitationExpired.as_str(), &format!("email={}", p.email)).await;
        return Err(expired_error(&p));
    }
    Ok(p)
}

/// Accepts `p` for `user` (whose email the caller has checked): marks it used and, when `join`, adds the
/// membership, atomically. A second accept, or one that lost a race, is `invitation.not_found`.
pub async fn claim(control: &ControlDb, user: &RecordId, p: &Pending, join: bool) -> AppResult<()> {
    let res = crate::tx::with_retry(|| async {
        q::CLAIM
            .on(control)
            .bind(("id", p.id.clone()))
            .bind(("user", user.clone()))
            .bind(("email", p.email.clone()))
            .bind(("hash", p.token_hash.clone()))
            .bind(("join", join))
            .await?
            .check()
    })
    .await;
    match res {
        Ok(_) => Ok(()),
        Err(e) if e.to_string().contains("invitation.unavailable") => Err(not_found()),
        Err(e) => Err(e.into()),
    }
}

/// Undoes [`claim`] for a signup that failed after it.
pub(crate) async fn reopen(control: &ControlDb, user: &RecordId, p: &Pending) {
    let _ = q::REOPEN.on(control).bind(("id", p.id.clone())).bind(("user", user.clone())).await;
}

/// The `org.invite_accept` row, in the joining org's log, with the role the account holds now.
pub async fn record_accepted(control: &ControlDb, user: &RecordId, p: &Pending, role: &str, how: &str) {
    let actor = caller_actor(user);
    record(control, user, &actor, "org.invite_accept", &p.id.to_string(), "ok", &format!("email={} role={role} via={how}", p.email)).await;
}

/// What accepting while signed in did.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InvitationAccepted {
    pub org_name: String,
    /// The role the account holds in the organisation now: `owner`, `admin` or `member`.
    pub role: String,
    /// The account was already in the organisation; the invitation is now used up (an admin invitation
    /// made a member an admin).
    pub already_member: bool,
}

/// Accepts `token` for the signed-in `user`. Their email must be the invited one. An account in another
/// org cannot accept (one org per account for now).
pub async fn accept_signed_in(state: &AppState, user: &User, token: &str) -> AppResult<InvitationAccepted> {
    let p = usable(state, token, Who::Account(&user.id)).await?;
    require_email(&state.control, &p, &user.email).await?;
    let org = p.org_id().ok_or_else(not_found)?;
    let already = org == user.org || {
        let mut res = q::ROLE_IN_ORG.on(&state.control).bind(("user", user.id.clone())).bind(("org", p.org.clone())).await?;
        !res.take::<Vec<String>>(0)?.is_empty()
    };
    if !already {
        // the inviting org learns only that it was not accepted; why (an account in another org) is for
        // the server's operators, in the log
        tracing::info!(invitation = %p.id.to_string(), "organisation invitation not accepted: the account belongs to another organisation");
        record_not_accepted(&state.control, &p).await;
        return Err(AppError::coded(
            ErrorCode::InvitationOtherOrg,
            "Your account already belongs to another organisation, and an account can only be in one for now. Ask for an invitation to a different email address.",
        ));
    }
    // already a member: an admin invitation makes them an admin (never touching an owner); then report
    // the role they really hold
    claim(&state.control, &user.id, &p, false).await?;
    let mut res = q::ROLE_IN_ORG.on(&state.control).bind(("user", user.id.clone())).bind(("org", p.org.clone())).await?;
    let role = res.take::<Vec<String>>(0)?.into_iter().next().unwrap_or_else(|| "member".into());
    record_accepted(&state.control, &user.id, &p, &role, "signed_in").await;
    Ok(InvitationAccepted { org_name: p.org_name.clone().unwrap_or_default(), role, already_member: true })
}

/// The outcome the inviting org's ledger records when an invitation could not be accepted.
pub const NOT_ACCEPTED: &str = "invitation.not_accepted";

/// The one row the inviting org's ledger gets for every refusal at acceptance (another email, an
/// account in another org, an unverified provider email, an address that may never be invited), so the
/// row never tells the inviter why. The reason is in the server log. An expired link has its own
/// `org.invite_expired` row: that says something about the link, not about the person.
pub async fn record_not_accepted(control: &ControlDb, p: &Pending) {
    record(control, &p.invited_by, &audit::anonymous(), "org.invite_accept", &p.id.to_string(), NOT_ACCEPTED, &format!("email={}", p.email)).await;
}

/// [`email_matches`], and a refusal goes to the inviter's ledger as "not accepted".
pub async fn require_email(control: &ControlDb, p: &Pending, email: &str) -> AppResult<()> {
    let r = email_matches(p, email);
    if r.is_err() {
        tracing::info!(invitation = %p.id.to_string(), "organisation invitation not accepted: a different email");
        record_not_accepted(control, p).await;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(email: &str) -> Pending {
        Pending {
            id: crate::rid::parse("org_invitation:a").unwrap(),
            org: crate::rid::parse("org:b").unwrap(),
            org_name: Some("Acme".into()),
            email: email.into(),
            role: "member".into(),
            invited_by: crate::rid::parse("user:c").unwrap(),
            token_hash: "h".into(),
            accepted_at: None,
            revoked_at: None,
            expires_at: Datetime::now(),
            expired: false,
        }
    }

    #[test]
    fn the_email_check_uses_the_signup_normalisation() {
        let p = pending("pat@acme.test");
        assert!(email_matches(&p, "  Pat@ACME.test ").is_ok());
        let e = email_matches(&p, "pat@acme.test.evil").unwrap_err();
        assert_eq!(e.code, ErrorCode::InvitationEmailMismatch);
        assert!(email_matches(&p, "pat+x@acme.test").is_err());
    }

    #[test]
    fn roles_and_messages_are_plain() {
        assert_eq!(OrgInviteRole::Admin.as_str(), "admin");
        assert_eq!(serde_json::from_str::<OrgInviteRole>("\"member\"").unwrap(), OrgInviteRole::Member);
        assert!(serde_json::from_str::<OrgInviteRole>("\"owner\"").is_err(), "an invitation never makes an owner");
        let e = expired_error(&pending("pat@acme.test"));
        assert_eq!((e.code, e.status.as_u16()), (ErrorCode::InvitationExpired, 410));
        assert!(e.message.contains("an admin of the organisation called \u{201c}Acme\u{201d}"), "{}", e.message);
        let mut personal = pending("pat@acme.test");
        personal.org_name = Some("founder@acme.test".into());
        assert_eq!(shown_org_name(&personal), None, "a personal workspace's name is its founder's address");
        assert!(!expired_error(&personal).message.contains("founder@acme.test"));
    }

    #[test]
    fn throttle_keys_keep_addresses_and_accounts_apart() {
        let u = crate::rid::parse("user:x").unwrap();
        assert_ne!(Who::Address("user:x").key(), Who::Account(&u).key());
    }
}
