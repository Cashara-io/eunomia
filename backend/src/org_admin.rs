//! An organisation runs itself: who may administer what, in one place.
//!
//! Three kinds of authority, kept apart:
//!
//! * **Organisation owner and admin** (`membership.role`): administer the members, the audit log, the
//!   access reviews, the MFA policy and (owner only) the name of **their own organisation**. Nothing here
//!   reaches another organisation: the organisation is always the caller's own (`User::org`, their oldest
//!   membership, never a request value), and a target account is looked up *through the membership of
//!   that organisation* before anything about it is read or changed. An account that is not a member (in
//!   another organisation, or nowhere) is "not found", with the same answer in both cases.
//! * **Instance admin** (`authz::is_admin`: the install's first account, an `EUNOMIA_ADMIN_EMAILS`
//!   address, or an account given the `user.admin` flag): what the routes always allowed. They keep every
//!   one of their old rules (the routes in `routers/admin.rs` still apply them); here they are treated
//!   like an owner of their own organisation, as `invitations::manager` already does.
//! * **Instance operator** (`authz::is_instance_operator`): the instance ledger, the instance-wide MFA
//!   switch, debug routes, erasing an account, granting `user.admin`. None of that moves: those routes
//!   keep their gates, and an organisation's owner or admin passes none of them.
//!
//! Every capability is one function that takes the caller (and the target) and every route calls it:
//! [`may_administer_members`], [`may_act_on_member`], [`may_read_org_audit`], [`may_run_access_reviews`],
//! [`may_set_org_mfa_policy`], [`may_rename_org`]. A route never compares a role itself. The rule for
//! acting on a member is the pure [`decide`], which the people list also uses to say which buttons a
//! viewer gets, so the list and the route cannot disagree.
//!
//! What a manager may do to a member (the caller is neither an instance admin nor the member):
//!
//! | target | member | admin | owner | operator, first account, or `user.admin` |
//! |---|---|---|---|---|
//! | caller owner | yes | yes | no | no |
//! | caller admin | yes | no | no | no |
//!
//! Changing a role needs the owner (only the owner grants or removes `admin`, the rule invitations
//! already have), and never touches the owner. There is no "remove from the organisation" distinct from
//! erasing an account (which stays with the person and the operator): suspension is the way to take
//! someone out, and it ends their sessions, tokens and apps at once.

use serde::{Deserialize, Serialize};
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use crate::authz;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::User;
use crate::pool::ControlDb;
use crate::rid::RecordIdExt;
use crate::state::AppState;
use crate::store::org_admin as q;

/// A member's place in an organisation (`membership.role`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrgRole {
    Owner,
    Admin,
    Member,
}

impl OrgRole {
    pub fn parse(role: &str) -> Self {
        match role {
            "owner" => OrgRole::Owner,
            "admin" => OrgRole::Admin,
            _ => OrgRole::Member,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            OrgRole::Owner => "owner",
            OrgRole::Admin => "admin",
            OrgRole::Member => "member",
        }
    }
}

/// Proof that [`authority`] found the caller may administer their own organisation. Only this module
/// builds one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrgAuthority {
    /// The caller's role in their own organisation.
    pub role: OrgRole,
    /// The caller is an instance admin (`authz::is_admin`): their old rules apply.
    pub instance_admin: bool,
}

impl OrgAuthority {
    /// May grant and remove the `admin` role, rename the organisation: its owner, or an instance admin.
    pub fn is_owner_level(&self) -> bool {
        self.instance_admin || self.role == OrgRole::Owner
    }
}

fn forbidden(what: &str) -> AppError {
    AppError::coded(ErrorCode::AuthForbidden, format!("Only an owner or admin of this organisation can {what}."))
}

/// The caller's standing in their own organisation, or `None` for a plain member (no powers).
pub async fn standing(control: &ControlDb, user: &User) -> AppResult<Option<OrgAuthority>> {
    let role = crate::invitations::org_role(control, user).await?.map(|r| OrgRole::parse(&r)).unwrap_or(OrgRole::Member);
    let instance_admin = authz::is_admin(control, user).await?;
    Ok((instance_admin || role != OrgRole::Member).then_some(OrgAuthority { role, instance_admin }))
}

/// May list and manage the members of the caller's own organisation: its owner or admin, or an instance admin.
pub async fn may_administer_members(control: &ControlDb, user: &User) -> AppResult<OrgAuthority> {
    standing(control, user).await?.ok_or_else(|| forbidden("manage its people"))
}

/// May read, verify and export the caller's own organisation's audit log.
pub async fn may_read_org_audit(control: &ControlDb, user: &User) -> AppResult<OrgAuthority> {
    standing(control, user).await?.ok_or_else(|| forbidden("read its audit log"))
}

/// May start, decide and complete access reviews of the caller's own organisation.
pub async fn may_run_access_reviews(control: &ControlDb, user: &User) -> AppResult<OrgAuthority> {
    standing(control, user).await?.ok_or_else(|| forbidden("run its access reviews"))
}

/// May read and set the caller's own organisation's "require two-step verification" policy.
pub async fn may_set_org_mfa_policy(control: &ControlDb, user: &User) -> AppResult<OrgAuthority> {
    standing(control, user).await?.ok_or_else(|| forbidden("set its security policy"))
}

/// May rename the caller's own organisation: its owner (an instance admin is the owner of the install's one
/// organisation by construction; an `admin` role or the `user.admin` flag alone is not enough).
pub async fn may_rename_org(control: &ControlDb, user: &User) -> AppResult<OrgAuthority> {
    match standing(control, user).await? {
        Some(a) if a.role == OrgRole::Owner => Ok(a),
        _ => Err(AppError::coded(ErrorCode::AuthForbidden, "Only the organisation's owner can rename it.")),
    }
}

/// What an administrator does to a member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberAct {
    Suspend,
    Restore,
    /// End browser sessions.
    EndSessions,
    /// List or revoke API tokens.
    Tokens,
    /// List or revoke OAuth grants (connected apps).
    Apps,
    /// Change the role (`member` or `admin`).
    SetRole,
}

/// The account an administrator acts on, as the organisation's membership shows it.
#[derive(Debug, Clone)]
pub struct Target {
    pub id: RecordId,
    pub email: String,
    pub role: OrgRole,
    /// An operator of the instance: the first account, an `EUNOMIA_ADMIN_EMAILS` address, or an account
    /// carrying the `user.admin` flag. An organisation's owner or admin never acts on one.
    pub protected: bool,
    pub suspended: bool,
}

/// Why [`decide`] refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Yourself,
    Protected,
    Owner,
    NeedsOwner,
}

impl Refusal {
    pub fn into_error(self) -> AppError {
        match self {
            Refusal::Yourself => AppError::bad_request("You can't do that to your own account here. Use your own Settings."),
            Refusal::Protected => AppError::coded(ErrorCode::OrgProtectedMember, "This account is an operator of the server, so an organisation's admins can't change it."),
            Refusal::Owner => AppError::coded(ErrorCode::OrgProtectedMember, "This account is the organisation's owner, so it can't be changed here."),
            Refusal::NeedsOwner => AppError::coded(ErrorCode::AuthForbidden, "Only the organisation's owner can manage an admin or change anyone's role."),
        }
    }
}

/// The one rule for acting on a member, pure so the people list can say what each row allows.
///
/// An instance admin keeps the rules of the routes they always had (the route applies them); the only
/// thing added for them is that nobody's owner role changes through a role change.
pub fn decide(auth: &OrgAuthority, caller: &RecordId, target: &Target, act: MemberAct) -> Result<(), Refusal> {
    if auth.instance_admin {
        return match act {
            MemberAct::SetRole if &target.id == caller => Err(Refusal::Yourself),
            MemberAct::SetRole if target.role == OrgRole::Owner => Err(Refusal::Owner),
            _ => Ok(()),
        };
    }
    if &target.id == caller {
        return Err(Refusal::Yourself);
    }
    if target.protected {
        return Err(Refusal::Protected);
    }
    match target.role {
        OrgRole::Owner => return Err(Refusal::Owner),
        OrgRole::Admin if auth.role != OrgRole::Owner => return Err(Refusal::NeedsOwner),
        _ => {}
    }
    if act == MemberAct::SetRole && auth.role != OrgRole::Owner {
        return Err(Refusal::NeedsOwner);
    }
    Ok(())
}

#[derive(Deserialize, SurrealValue)]
struct MemberRow {
    id: RecordId,
    role: String,
    #[serde(default)]
    #[surreal(default)]
    email: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    email_lc: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    admin: Option<bool>,
    #[serde(default)]
    #[surreal(default)]
    suspended_at: Option<Datetime>,
}

fn not_a_member() -> AppError {
    AppError::coded(ErrorCode::AuthNotFound, "That account isn't in this workspace.")
}

/// `id` as a member of the caller's own organisation, read through its membership. An id that is not a
/// member (another organisation's, or nobody's, or not an id at all) is the same "not found".
pub async fn member(control: &ControlDb, user: &User, id: &str) -> AppResult<Target> {
    let rid: RecordId = crate::rid::parse(id).ok().filter(|r: &RecordId| r.table() == "user").ok_or_else(not_a_member)?;
    let mut res = q::MEMBER.on(control).bind(("org", org_record(user))).bind(("user", rid.clone())).await?;
    let row = res.take::<Vec<MemberRow>>(0)?.into_iter().next().ok_or_else(not_a_member)?;
    let first = crate::capsules::first_user(control).await?;
    Ok(Target {
        protected: first.as_ref() == Some(&row.id) || authz::stored_email_is_operator(row.email_lc.as_deref()) || row.admin == Some(true),
        role: OrgRole::parse(&row.role),
        email: row.email.unwrap_or_default(),
        suspended: row.suspended_at.is_some(),
        id: row.id,
    })
}

fn org_record(user: &User) -> RecordId {
    user.org.record()
}

/// May the caller do `act` to the member `id` of their own organisation? The target is found through the
/// caller's own organisation's membership first, so an account elsewhere is "not found", whatever the caller's role.
pub async fn may_act_on_member(control: &ControlDb, user: &User, id: &str, act: MemberAct) -> AppResult<(OrgAuthority, Target)> {
    let auth = may_administer_members(control, user).await?;
    let target = member(control, user, id).await?;
    decide(&auth, &user.id, &target, act).map_err(Refusal::into_error)?;
    Ok((auth, target))
}

// ---------------------------------------------------------------- operations

/// Every member's role in the caller's organisation, by user id.
pub async fn roles(control: &ControlDb, user: &User) -> AppResult<std::collections::HashMap<String, OrgRole>> {
    #[derive(Deserialize, SurrealValue)]
    struct R {
        user: RecordId,
        role: String,
    }
    let rows: Vec<R> = q::ROLES.on(control).bind(("org", org_record(user))).await?.take(0)?;
    Ok(rows.into_iter().map(|r| (r.user.to_string(), OrgRole::parse(&r.role))).collect())
}

/// Sets a member's role to `member` or `admin`. The target's current role is part of the update's guard, so
/// a role changed meanwhile, or an owner, is left as it is.
pub async fn set_role(state: &AppState, user: &User, id: &str, role: OrgRole) -> AppResult<()> {
    if role == OrgRole::Owner {
        return Err(AppError::bad_request("The role must be `member` or `admin`: an owner is made by handing the workspace over, not by a role change."));
    }
    let (_, target) = may_act_on_member(&state.control, user, id, MemberAct::SetRole).await?;
    if target.role == role {
        return Ok(());
    }
    let changed: Vec<serde_json::Value> = q::SET_ROLE
        .on(&state.control)
        .bind(("org", org_record(user)))
        .bind(("user", target.id.clone()))
        .bind(("role", role.as_str().to_string()))
        .bind(("from", target.role.as_str().to_string()))
        .await?
        .take(0)?;
    if changed.is_empty() {
        return Err(AppError::coded(ErrorCode::DbConflict, "That person's role changed meanwhile. Reload and try again."));
    }
    // a role is read from the membership on every request, but the people list a client holds is stale
    crate::audit::record_as_caller_detail(&state.control, &user.id, "org.role", &target.id.to_string(), "ok", &format!("email={} role={} from={}", target.email, role.as_str(), target.role.as_str())).await;
    Ok(())
}

/// What ending a member's access removed.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AccessRevoked {
    /// Browser sessions that were open.
    pub sessions: i64,
    /// API tokens revoked.
    pub tokens: i64,
    /// Connected apps (OAuth grants) revoked.
    pub apps: i64,
}

/// Ends every session, API token and connected app of a member at once (a lost laptop, a leaver). The
/// account itself is untouched: it can sign in again unless it is suspended.
pub async fn revoke_all(state: &AppState, user: &User, id: &str) -> AppResult<AccessRevoked> {
    let (auth, target) = may_act_on_member(&state.control, user, id, MemberAct::EndSessions).await?;
    // each kind has its own act in `decide`, and they are one rule today: check them all so a future
    // split cannot leave this route behind
    decide(&auth, &user.id, &target, MemberAct::Tokens).map_err(Refusal::into_error)?;
    decide(&auth, &user.id, &target, MemberAct::Apps).map_err(Refusal::into_error)?;
    #[derive(Deserialize, SurrealValue)]
    struct Out {
        sessions: i64,
        tokens: Vec<RecordId>,
        grants: Vec<RecordId>,
    }
    let out: Option<Out> = q::REVOKE_ACCESS.on(&state.control).bind(("user", target.id.clone())).await?.check()?.take(q::REVOKE_ACCESS.slot)?;
    let out = out.ok_or_else(|| AppError::internal("revoking access returned nothing"))?;
    // the credential cache holds a verified credential for a few seconds: drop this account's now
    state.credentials.forget_user(&target.id);
    for t in out.tokens.iter().chain(&out.grants) {
        state.credentials.forget(&t.to_string());
    }
    let detail = format!("email={} sessions={} tokens={} apps={}", target.email, out.sessions, out.tokens.len(), out.grants.len());
    crate::audit::record_as_caller_detail(&state.control, &user.id, "admin.access_revoke", &target.id.to_string(), "ok", &detail).await;
    Ok(AccessRevoked { sessions: out.sessions, tokens: out.tokens.len() as i64, apps: out.grants.len() as i64 })
}

/// Ends every browser session of a member. Their API tokens and connected apps are left alone.
pub async fn end_sessions(state: &AppState, user: &User, id: &str) -> AppResult<i64> {
    let (_, target) = may_act_on_member(&state.control, user, id, MemberAct::EndSessions).await?;
    let ended: Vec<serde_json::Value> = crate::store::control::AUTH_SESSION_REVOKE_OWNER.on(&state.control).bind(("owner", target.id.clone())).await?.check()?.take(0)?;
    state.credentials.forget_user(&target.id);
    crate::audit::record_as_caller_detail(&state.control, &user.id, "admin.sessions_end", &target.id.to_string(), "ok", &format!("email={} sessions={}", target.email, ended.len())).await;
    Ok(ended.len() as i64)
}

/// An app a member connected through OAuth (an MCP client such as Claude Desktop).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PersonApp {
    pub id: String,
    pub name: String,
    pub client_id: String,
    pub scope: Vec<String>,
    #[schema(value_type = Option<String>)]
    pub created_at: Option<Datetime>,
    #[schema(value_type = Option<String>)]
    pub last_used_at: Option<Datetime>,
}

async fn apps_of(control: &ControlDb, owner: &RecordId) -> AppResult<Vec<PersonApp>> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        id: RecordId,
        client_id: String,
        client_name: String,
        #[serde(default)]
        #[surreal(default)]
        scope: Vec<String>,
        #[serde(default)]
        #[surreal(default)]
        created_at: Option<Datetime>,
        #[serde(default)]
        #[surreal(default)]
        last_used_at: Option<Datetime>,
    }
    let rows: Vec<Row> = crate::store::control::OAUTH_GRANT_LIST.on(control).bind(("owner", owner.clone())).await?.take(0)?;
    Ok(rows
        .into_iter()
        .map(|r| PersonApp { id: r.id.to_string(), name: r.client_name, client_id: r.client_id, scope: r.scope, created_at: r.created_at, last_used_at: r.last_used_at })
        .collect())
}

/// The apps a member of the caller's own organisation connected.
pub async fn list_apps(state: &AppState, user: &User, id: &str) -> AppResult<Vec<PersonApp>> {
    let (_, target) = may_act_on_member(&state.control, user, id, MemberAct::Apps).await?;
    apps_of(&state.control, &target.id).await
}

/// Disconnects one app of a member: the grant and every token it issued stop working at once. Returns what is left.
pub async fn revoke_app(state: &AppState, user: &User, id: &str, grant_id: &str) -> AppResult<Vec<PersonApp>> {
    let (_, target) = may_act_on_member(&state.control, user, id, MemberAct::Apps).await?;
    let gone = || AppError::coded(ErrorCode::ResourceNotFound, "No such connected app.");
    let grant: RecordId = crate::rid::parse(grant_id).ok().filter(|r: &RecordId| r.table() == "oauth_grant").ok_or_else(gone)?;
    let apps = apps_of(&state.control, &target.id).await?;
    let name = apps.iter().find(|a| a.id == grant.to_string()).map(|a| a.name.clone()).ok_or_else(gone)?;
    #[derive(Deserialize, SurrealValue)]
    struct Gone {
        #[allow(dead_code)]
        id: RecordId,
    }
    // the delete names the owner, so a grant of another account is no row, whatever its id
    let removed: Vec<Gone> = crate::store::control::OAUTH_GRANT_DELETE.on(&state.control).bind(("id", grant.clone())).bind(("owner", target.id.clone())).await?.take(0)?;
    if removed.is_empty() {
        return Err(gone());
    }
    crate::store::control::OAUTH_TOKENS_DELETE_FAMILY.on(&state.control).bind(("family", grant.clone())).await?.check()?;
    state.credentials.forget(&grant.to_string());
    crate::audit::record_as_caller_detail(&state.control, &user.id, "admin.app_revoke", &grant.to_string(), "ok", &format!("email={} app={name}", target.email)).await;
    apps_of(&state.control, &target.id).await
}

// ---------------------------------------------------------------- the organisation itself

/// The organisation's name and security policy, and what the caller may do with them.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OrgOut {
    /// The organisation's display name. A workspace made at sign-up is named after its founder's email until renamed.
    pub name: String,
    /// The caller's role in it: `owner`, `admin` or `member`.
    pub your_role: String,
    /// The caller may rename it (its owner).
    pub can_rename: bool,
    /// This organisation requires two-step verification of every member who signs in with a password.
    pub require_mfa: bool,
    /// The server's operator requires it for everyone on the server too: an organisation cannot turn that off.
    pub server_requires_mfa: bool,
    #[schema(value_type = Option<String>)]
    pub updated_at: Option<Datetime>,
    /// Email of the member who last changed the organisation's policy.
    pub updated_by: Option<String>,
    /// Active password members of this organisation without two-step verification yet: while it is required
    /// they must set it up at their next sign-in.
    pub without_mfa: i64,
}

#[derive(Deserialize, SurrealValue)]
struct OrgRow {
    #[serde(default)]
    #[surreal(default)]
    name: String,
    #[serde(default)]
    #[surreal(default)]
    require_mfa: Option<bool>,
    #[serde(default)]
    #[surreal(default)]
    require_mfa_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    require_mfa_by: Option<String>,
}

pub async fn org_out(state: &AppState, user: &User, auth: &OrgAuthority) -> AppResult<OrgOut> {
    #[derive(Deserialize, SurrealValue)]
    struct N {
        n: i64,
    }
    let row: Option<OrgRow> = q::ORG_GET.on(&state.control).bind(("org", org_record(user))).await?.take(0)?;
    let row = row.ok_or_else(|| AppError::internal("the caller's organisation has no record"))?;
    let without: Vec<N> = q::WITHOUT_MFA.on(&state.control).bind(("org", org_record(user))).await?.take(0)?;
    Ok(OrgOut {
        name: row.name,
        your_role: auth.role.as_str().into(),
        can_rename: auth.role == OrgRole::Owner,
        require_mfa: row.require_mfa.unwrap_or(false),
        server_requires_mfa: crate::mfa::require_mfa(&state.control).await?,
        updated_at: row.require_mfa_at,
        updated_by: row.require_mfa_by,
        without_mfa: without.first().map_or(0, |n| n.n),
    })
}

/// The name a person typed, trimmed, or why it is refused. At most 80 characters, no control characters, and
/// no `@`: a name with one reads as a personal workspace's email and is hidden from an invitation page
/// (`invitations::shown_org_name`), so an owner who chose it would find it silently not shown.
pub fn clean_name(name: &str) -> AppResult<String> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err(AppError::bad_request("The organisation needs a name."));
    }
    if name.chars().count() > 80 {
        return Err(AppError::bad_request("The name can be at most 80 characters."));
    }
    if name.chars().any(char::is_control) {
        return Err(AppError::bad_request("The name can't contain control characters."));
    }
    if name.contains('@') {
        return Err(AppError::bad_request("The name can't contain @ (it would read as an email address)."));
    }
    Ok(name)
}

/// Renames the caller's own organisation. Owner only.
pub async fn rename(state: &AppState, user: &User, name: &str) -> AppResult<OrgOut> {
    let auth = may_rename_org(&state.control, user).await?;
    let name = clean_name(name)?;
    let before = org_out(state, user, &auth).await?;
    if before.name != name {
        q::SET_NAME.on(&state.control).bind(("org", org_record(user))).bind(("name", name.clone())).await?.check()?;
        crate::audit::record_as_caller_detail(&state.control, &user.id, "org.rename", &user.org.record().to_string(), "ok", &format!("name={name}")).await;
    }
    org_out(state, user, &auth).await
}

/// Turns the caller's own organisation's "require two-step verification" on or off. Like the instance
/// switch: it asks for a second factor of the caller if they have one, and refuses to switch on while the
/// caller's own password account has none (they would be locked out at the next sign-in).
pub async fn set_mfa_policy(state: &AppState, user: &User, headers: &axum::http::HeaderMap, on: bool, code: Option<&str>) -> AppResult<OrgOut> {
    let auth = may_set_org_mfa_policy(&state.control, user).await?;
    let current = org_out(state, user, &auth).await?;
    if current.require_mfa == on {
        return Ok(current);
    }
    crate::mfa::step_up(state, user, headers, code).await?;
    if on {
        let me = crate::mfa::status(&state.control, &user.id).await?;
        if me.available && !me.enabled {
            return Err(AppError::bad_request("Turn on two-step verification for your own account first (Settings > Two-step verification), so you are not locked out."));
        }
    }
    q::SET_MFA_POLICY.on(&state.control).bind(("org", org_record(user))).bind(("require_mfa", on)).bind(("by", user.id.clone())).await?.check()?;
    // sessions verified in the last few seconds are re-checked against the new rule on their next request
    // (on this replica at once; other replicas within the credential cache's 20 seconds), as for the instance switch
    state.credentials.clear();
    crate::audit::record_as_caller_detail(&state.control, &user.id, "org.require_mfa", &user.org.record().to_string(), "ok", if on { "on" } else { "off" }).await;
    org_out(state, user, &auth).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> RecordId {
        crate::rid::parse(s).unwrap()
    }

    fn target(role: OrgRole, protected: bool) -> Target {
        Target { id: id("user:t"), email: "t@x.test".into(), role, protected, suspended: false }
    }

    fn who(role: OrgRole, instance_admin: bool) -> OrgAuthority {
        OrgAuthority { role, instance_admin }
    }

    #[test]
    fn what_a_manager_may_do_to_whom() {
        let me = id("user:me");
        let acts = [MemberAct::Suspend, MemberAct::Restore, MemberAct::EndSessions, MemberAct::Tokens, MemberAct::Apps];
        for act in acts {
            // owner: members and admins, never the owner or a protected account
            assert_eq!(decide(&who(OrgRole::Owner, false), &me, &target(OrgRole::Member, false), act), Ok(()));
            assert_eq!(decide(&who(OrgRole::Owner, false), &me, &target(OrgRole::Admin, false), act), Ok(()));
            assert_eq!(decide(&who(OrgRole::Owner, false), &me, &target(OrgRole::Owner, false), act), Err(Refusal::Owner));
            assert_eq!(decide(&who(OrgRole::Owner, false), &me, &target(OrgRole::Member, true), act), Err(Refusal::Protected));
            // admin: members only
            assert_eq!(decide(&who(OrgRole::Admin, false), &me, &target(OrgRole::Member, false), act), Ok(()));
            assert_eq!(decide(&who(OrgRole::Admin, false), &me, &target(OrgRole::Admin, false), act), Err(Refusal::NeedsOwner));
            assert_eq!(decide(&who(OrgRole::Admin, false), &me, &target(OrgRole::Owner, false), act), Err(Refusal::Owner));
            assert_eq!(decide(&who(OrgRole::Admin, false), &me, &target(OrgRole::Member, true), act), Err(Refusal::Protected));
            // never yourself, whatever you are
            let mine = Target { id: me.clone(), ..target(OrgRole::Member, false) };
            assert_eq!(decide(&who(OrgRole::Owner, false), &me, &mine, act), Err(Refusal::Yourself));
            assert_eq!(decide(&who(OrgRole::Admin, false), &me, &mine, act), Err(Refusal::Yourself));
        }
    }

    #[test]
    fn only_the_owner_changes_roles_and_never_the_owners() {
        let me = id("user:me");
        let set = MemberAct::SetRole;
        assert_eq!(decide(&who(OrgRole::Owner, false), &me, &target(OrgRole::Member, false), set), Ok(()));
        assert_eq!(decide(&who(OrgRole::Owner, false), &me, &target(OrgRole::Admin, false), set), Ok(()));
        assert_eq!(decide(&who(OrgRole::Owner, false), &me, &target(OrgRole::Owner, false), set), Err(Refusal::Owner));
        assert_eq!(decide(&who(OrgRole::Admin, false), &me, &target(OrgRole::Member, false), set), Err(Refusal::NeedsOwner));
        assert_eq!(decide(&who(OrgRole::Admin, false), &me, &target(OrgRole::Admin, false), set), Err(Refusal::NeedsOwner));
        // an instance admin keeps their routes' rules, and never changes an owner's role or their own
        assert_eq!(decide(&who(OrgRole::Member, true), &me, &target(OrgRole::Member, false), set), Ok(()));
        assert_eq!(decide(&who(OrgRole::Member, true), &me, &target(OrgRole::Owner, false), set), Err(Refusal::Owner));
        assert_eq!(decide(&who(OrgRole::Member, true), &me, &Target { id: me.clone(), ..target(OrgRole::Member, false) }, set), Err(Refusal::Yourself));
        // ...and for the other acts nothing is added to what they could always do
        assert_eq!(decide(&who(OrgRole::Member, true), &me, &target(OrgRole::Owner, true), MemberAct::Suspend), Ok(()));
    }

    #[test]
    fn names_are_cleaned_and_bounded() {
        assert_eq!(clean_name("  Acme   Pty  Ltd ").unwrap(), "Acme Pty Ltd");
        assert!(clean_name("   ").is_err());
        assert!(clean_name(&"x".repeat(81)).is_err());
        assert!(clean_name(&"x".repeat(80)).is_ok());
        assert!(clean_name("ops@acme.test").is_err());
        assert!(clean_name("Acme\u{7}").is_err());
    }

    #[test]
    fn roles_round_trip() {
        for r in [OrgRole::Owner, OrgRole::Admin, OrgRole::Member] {
            assert_eq!(OrgRole::parse(r.as_str()), r);
        }
        assert_eq!(OrgRole::parse("nonsense"), OrgRole::Member);
    }
}
