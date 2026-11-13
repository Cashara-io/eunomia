//! Authorization in one place. Two independent questions, both answered here:
//!
//! 1. What may this *vault role* do? ([`permits`], an exhaustive matrix over
//!    owner and member, applied by [`authorize`].)
//! 2. What may this *credential* do? A personal access token carries scopes and
//!    an optional vault restriction. The gate (`gate.rs`) puts the request's
//!    [`Caller`] in a task-local, and [`authorize`] and the vault lookups read
//!    it, so a restricted token cannot reach another vault by any path that goes
//!    through here.
//!
//! [`VaultScope`] can only be built by [`authorize`]: holding one is the proof
//! that the check ran.

use surrealdb::types::SurrealValue;
use std::future::Future;

use serde::Deserialize;
use surrealdb::types::RecordId;
use crate::rid::RecordIdExt;

use crate::models_user::User;
use crate::pool::{ControlDb, OrgDb};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::scopes;
use crate::store;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Owner,
    Member,
}

/// Everything a caller can ask to do to a vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    ReadMemories,
    WriteMemories,
    ListMembers,
    ManageMembers,
    Invite,
    Rename,
    Delete,
    Clone,
    Merge,
    Leave,
    SetRole,
    /// Approve or reject the vault's ontology proposals (`ontology::decide_kind`).
    DecideOntology,
}

impl Action {
    pub const ALL: [Action; 12] = [
        Action::ReadMemories,
        Action::WriteMemories,
        Action::ListMembers,
        Action::ManageMembers,
        Action::Invite,
        Action::Rename,
        Action::Delete,
        Action::Clone,
        Action::Merge,
        Action::Leave,
        Action::SetRole,
        Action::DecideOntology,
    ];

    /// The token scope this action needs.
    pub fn scope(self) -> &'static str {
        match self {
            Action::ReadMemories | Action::ListMembers => scopes::MEMORY_READ,
            Action::WriteMemories => scopes::MEMORY_WRITE,
            Action::ManageMembers
            | Action::Invite
            | Action::Rename
            | Action::Delete
            | Action::Clone
            | Action::Merge
            | Action::Leave
            | Action::SetRole
            | Action::DecideOntology => scopes::VAULTS_ADMIN,
        }
    }
}

/// The role matrix. Owners administer; members read and write content and may
/// clone, merge from, list and leave a vault they belong to.
pub fn permits(role: Role, action: Action) -> bool {
    match action {
        Action::ReadMemories
        | Action::WriteMemories
        | Action::ListMembers
        | Action::Clone
        | Action::Merge
        | Action::Leave => true,
        Action::ManageMembers | Action::Invite | Action::Rename | Action::Delete | Action::SetRole | Action::DecideOntology => role == Role::Owner,
    }
}

/// Proof that `authorize` allowed an action on a vault. No public constructor.
#[derive(Debug, Clone)]
pub struct VaultScope {
    vault: RecordId,
    role: Role,
}

impl VaultScope {
    pub fn vault(&self) -> &RecordId {
        &self.vault
    }

    pub fn role(&self) -> Role {
        self.role
    }
}

/// Who is acting, for the audit ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    /// `user` (browser session), `token` (personal access token), `oauth` (client) or `anonymous`.
    pub kind: &'static str,
    pub id: String,
}

/// The credential behind the current request.
#[derive(Debug, Clone)]
pub struct Caller {
    pub actor: Actor,
    pub scopes: Vec<String>,
    /// A vault-restricted token may only touch this vault.
    pub vault: Option<RecordId>,
    /// When the credential stops working (a PAT with an expiry); `None` for sessions and tokens that never expire.
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Caller {
    /// A browser session: every scope, every vault the user belongs to.
    pub fn session(user_id: &RecordId) -> Self {
        Caller { actor: Actor { kind: "user", id: user_id.to_string() }, scopes: scopes::ALL.iter().map(|s| s.to_string()).collect(), vault: None, expires_at: None }
    }

    /// Server-initiated work with no request behind it (jobs, boot, replay):
    /// every scope, every vault. Request code never gets this by default.
    pub fn system() -> Self {
        Caller { actor: Actor { kind: "system", id: "system".into() }, scopes: scopes::ALL.iter().map(|s| s.to_string()).collect(), vault: None, expires_at: None }
    }

    /// `memory:write` implies `memory:read` (see `scopes::allows`).
    pub fn allows(&self, scope: &str) -> bool {
        scopes::allows(&self.scopes, scope)
    }
}

tokio::task_local! {
    static CALLER: Caller;
}

/// Runs `fut` with `caller` as the current credential (the gate does this per request).
pub async fn with_caller<F: Future>(caller: Caller, fut: F) -> F::Output {
    CALLER.scope(caller, fut).await
}

/// The current task's credential, if any. Only for reading who is acting
/// (audit); checks use [`caller`], which fails closed.
pub fn current() -> Option<Caller> {
    CALLER.try_with(Caller::clone).ok()
}

/// The credential for an authorization check. No credential in scope means a
/// task was spawned without [`with_caller`] (or non-request code forgot
/// `Caller::system()`): refuse loudly instead of treating it as unrestricted.
pub fn caller() -> AppResult<Caller> {
    current().ok_or_else(|| {
        tracing::error!("authorization check ran with no caller in scope: a spawned task lost the request credential");
        AppError::internal("no credential in scope for an authorization check")
    })
}

/// Runs `fut` as the system (jobs, boot, replay, tests of services).
pub async fn as_system<F: Future>(fut: F) -> F::Output {
    with_caller(Caller::system(), fut).await
}

/// 403 `auth.scope` unless the current credential carries `scope`.
pub fn require_scope(scope: &str) -> AppResult<()> {
    if caller()?.allows(scope) {
        Ok(())
    } else {
        Err(AppError::coded(ErrorCode::AuthScope, format!("this token does not have the {scope} scope")))
    }
}

/// 403 `auth.scope` unless the request came from a browser session (not a token
/// or an OAuth client): for routes that mint or manage credentials.
pub fn require_session() -> AppResult<()> {
    match caller()?.actor.kind {
        "user" | "system" => Ok(()),
        _ => Err(AppError::coded(ErrorCode::AuthScope, "this route needs a browser session, not a token")),
    }
}

/// The vault the current token is restricted to, if any.
pub fn restricted_vault() -> AppResult<Option<RecordId>> {
    Ok(caller()?.vault)
}

/// 403 `auth.scope` if the current token is restricted to a vault other than `vault`.
pub fn check_vault(vault: &RecordId) -> AppResult<()> {
    match restricted_vault()? {
        Some(only) if &only != vault => {
            Err(AppError::coded(ErrorCode::AuthScope, format!("this token is restricted to vault {}", only.to_string())))
        }
        _ => Ok(()),
    }
}

/// 403 `auth.scope` for a vault-restricted token. For operations that create
/// data outside the restricted vault (new, cloned or merged vaults) or are not
/// about a vault at all.
pub fn require_unrestricted() -> AppResult<()> {
    match restricted_vault()? {
        Some(only) => Err(AppError::coded(ErrorCode::AuthScope, format!("this token is restricted to vault {}", only.to_string()))),
        None => Ok(()),
    }
}

/// True for an email listed in `EUNOMIA_ADMIN_EMAILS` (comma separated).
pub fn operator_email_listed(email: &str) -> bool {
    std::env::var("EUNOMIA_ADMIN_EMAILS")
        .unwrap_or_default()
        .split(',')
        .any(|e| !e.trim().is_empty() && e.trim().eq_ignore_ascii_case(email.trim()))
}

/// An email listed in `EUNOMIA_ADMIN_EMAILS`: an operator of the whole instance. Emails are not
/// verified, so signup refuses a listed address once the install has users (`routers/auth.rs`):
/// an operator must register the account first, then add it to the list.
pub fn is_operator(user: &User) -> bool {
    operator_email_listed(&user.email)
}

/// Admin = the instance's first user, an operator, or an account an admin made one (Settings > People).
pub async fn is_admin(db: &ControlDb, user: &User) -> AppResult<bool> {
    Ok(is_operator(user) || granted_admin(db, &user.id).await? || crate::capsules::first_user(db).await?.as_ref() == Some(&user.id))
}

/// An operator of the whole server: the instance's first account or an email in `EUNOMIA_ADMIN_EMAILS`.
/// Admin rights granted in the app (Settings > People) are not enough: they are given within one
/// organisation, while an instance setting reaches every organisation on the host.
pub async fn is_instance_operator(db: &ControlDb, user: &User) -> AppResult<bool> {
    Ok(is_operator(user) || crate::capsules::first_user(db).await?.as_ref() == Some(&user.id))
}

/// True when an admin granted `user` admin rights in the app (the `admin` flag on the account).
pub async fn granted_admin(db: &ControlDb, user: &RecordId) -> AppResult<bool> {
    #[derive(Deserialize, SurrealValue)]
    struct Flag {
        #[serde(default)]
        #[surreal(default)]
        admin: Option<bool>,
    }
    let mut res = store::control::USER_ADMIN_FLAG.on(db).bind(("id", user.clone())).await?;
    Ok(res.take::<Vec<Flag>>(0)?.into_iter().next().and_then(|f| f.admin).unwrap_or(false))
}

#[derive(Deserialize, SurrealValue)]
struct RoleRow {
    #[serde(default)]
    #[surreal(default)]
    role: String,
}

pub(crate) async fn role_of(db: &OrgDb, user: &RecordId, vault: &RecordId) -> AppResult<Option<Role>> {
    let mut res = store::vaults::MEMBERSHIP_ACTIVE
        .on(db)
        .bind(("vault", vault.clone()))
        .bind(("user", user.clone()))
        .await?;
    let rows: Vec<RoleRow> = res.take(0)?;
    Ok(rows.into_iter().next().map(|r| if r.role == "owner" { Role::Owner } else { Role::Member }))
}

/// 403 unless `user` is an active member of `vault`. Not an authorization of
/// any action: only for resolving a restricted token's own vault.
pub async fn ensure_member(db: &OrgDb, user: &RecordId, vault: &RecordId) -> AppResult<()> {
    match role_of(db, user, vault).await? {
        Some(_) => Ok(()),
        None => Err(AppError::coded(ErrorCode::VaultForbidden, format!("not a member of vault {}", vault.to_string()))),
    }
}

/// The one gate for acting on a vault: the credential's scope and vault
/// restriction, then the user's active role against the matrix.
pub async fn authorize(db: &OrgDb, user: &RecordId, action: Action, vault: &RecordId) -> AppResult<VaultScope> {
    require_scope(action.scope())?;
    check_vault(vault)?;
    match role_of(db, user, vault).await? {
        Some(role) if permits(role, action) => Ok(VaultScope { vault: vault.clone(), role }),
        _ if permits(Role::Member, action) => {
            Err(AppError::coded(ErrorCode::VaultForbidden, format!("not a member of vault {}", vault.to_string())))
        }
        _ => Err(AppError::coded(ErrorCode::VaultForbidden, format!("must be an owner of vault {}", vault.to_string()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_matrix() {
        use Action::*;
        // (action, owner, member)
        let table = [
            (ReadMemories, true, true),
            (WriteMemories, true, true),
            (ListMembers, true, true),
            (ManageMembers, true, false),
            (Invite, true, false),
            (Rename, true, false),
            (Delete, true, false),
            (Clone, true, true),
            (Merge, true, true),
            (Leave, true, true),
            (SetRole, true, false),
            (DecideOntology, true, false),
        ];
        assert_eq!(table.len(), Action::ALL.len(), "every action is in the table");
        for (action, owner, member) in table {
            assert_eq!(permits(Role::Owner, action), owner, "owner {action:?}");
            assert_eq!(permits(Role::Member, action), member, "member {action:?}");
        }
    }

    #[test]
    fn action_scopes() {
        assert_eq!(Action::ReadMemories.scope(), scopes::MEMORY_READ);
        assert_eq!(Action::WriteMemories.scope(), scopes::MEMORY_WRITE);
        for a in [Action::Invite, Action::Rename, Action::Delete, Action::Clone, Action::Merge, Action::ManageMembers, Action::SetRole] {
            assert_eq!(a.scope(), scopes::VAULTS_ADMIN, "{a:?}");
        }
    }

    #[tokio::test]
    async fn restriction_and_scope_checks_follow_the_current_caller() {
        let v1: RecordId = crate::rid::parse("vault:one").unwrap();
        let v2: RecordId = crate::rid::parse("vault:two").unwrap();
        // no caller: fails closed
        assert_eq!(check_vault(&v1).unwrap_err().code, ErrorCode::Internal);
        assert_eq!(require_scope(scopes::MEMORY_READ).unwrap_err().code, ErrorCode::Internal);
        assert_eq!(require_unrestricted().unwrap_err().code, ErrorCode::Internal);
        as_system(async {
            assert!(check_vault(&v1).is_ok() && require_scope(scopes::VAULTS_ADMIN).is_ok() && require_unrestricted().is_ok() && require_session().is_ok());
        })
        .await;

        let caller = Caller {
            actor: Actor { kind: "token", id: "api_token:x".into() },
            scopes: vec![scopes::MEMORY_READ.to_string()],
            vault: Some(v1.clone()),
            expires_at: None,
        };
        with_caller(caller, async {
            assert!(check_vault(&v1).is_ok());
            assert_eq!(check_vault(&v2).unwrap_err().code, ErrorCode::AuthScope);
            assert!(require_scope(scopes::MEMORY_READ).is_ok());
            assert_eq!(require_scope(scopes::MEMORY_WRITE).unwrap_err().code, ErrorCode::AuthScope);
            assert_eq!(require_unrestricted().unwrap_err().code, ErrorCode::AuthScope);
            assert_eq!(require_session().unwrap_err().code, ErrorCode::AuthScope);
        })
        .await;
    }
}
