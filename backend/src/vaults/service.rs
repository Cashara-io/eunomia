//! Vaults: a knowledge scope (person/organisation/location/... + memory rows)
//! that more than one user can belong to. Access is membership, not a single
//! `owner` field -- a personal vault, a team's org vault, and "share my
//! stuff with Bob" are all the same `vault` + `vault_member` primitive (see
//! `db.rs`'s schema statements). Two roles only: "owner" (can invite/remove
//! members, rename, delete) and "member" (read/write content). The vault's
//! creator is its first owner; anyone who invites must already be an owner,
//! so admin rights only ever come from creating a vault or being promoted by
//! an existing owner -- no separate "who can invite" check needed.
//!
//! Callers resolve a path/user-supplied vault id into a `RecordId` before
//! calling in (see `routers/vaults.rs`); everything here deals in `RecordId`.

use surrealdb::types::SurrealValue;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use surrealdb::types::{Datetime, RecordId};
use crate::rid::RecordIdExt;

use crate::authz::{self, Action};
use crate::pool::{ControlDb, OrgDb};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::store;
use crate::tx::{with_retry, with_retry_dup};

/// Entity tables a vault's data can live in: the built-in kinds' tables and the custom-kind table, the
/// same set as `memory.subject`'s record union.
use crate::entities::service::ENTITY_TABLES as ENTITY_KINDS;

#[derive(Debug, Deserialize, SurrealValue)]
struct VaultRow {
    id: RecordId,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct VaultFullRow {
    id: RecordId,
    #[serde(default)]
    #[surreal(default)]
    name: String,
    #[serde(default = "default_kind")]
    #[surreal(default = "default_kind")]
    kind: String,
    created_at: Option<Datetime>,
}

fn default_kind() -> String {
    "personal".to_string()
}

impl From<VaultFullRow> for VaultOut {
    fn from(r: VaultFullRow) -> Self {
        VaultOut { id: r.id.to_string(), name: r.name, kind: r.kind, created_at: r.created_at }
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct VaultOut {
    pub id: String,
    pub name: String,
    pub kind: String,
    #[schema(value_type = Option<String>)]
    pub created_at: Option<Datetime>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VaultWithRole {
    pub id: String,
    pub name: String,
    pub kind: String,
    #[schema(value_type = Option<String>)]
    pub created_at: Option<Datetime>,
    pub role: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InviteOut {
    pub vault_id: String,
    pub user_email: String,
    pub role: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MemberOut {
    pub email: String,
    pub role: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InvitationOut {
    pub vault_id: String,
    pub vault_name: String,
    pub vault_kind: String,
    pub role: String,
    #[schema(value_type = Option<String>)]
    pub created_at: Option<Datetime>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CloneOut {
    pub id: String,
    pub name: String,
    pub kind: String,
    #[schema(value_type = Option<String>)]
    pub created_at: Option<Datetime>,
    pub entities_copied: usize,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct MembershipRow {
    id: RecordId,
    #[serde(default)]
    #[surreal(default)]
    role: String,
    #[serde(default)]
    #[surreal(default)]
    status: String,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct EmailLookupRow {
    id: RecordId,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct CountRow {
    count: i64,
}

/// Called once, at registration. Every user has exactly one `kind="personal"`
/// vault -- it's never shared by anyone else joining it as an owner-less
/// member, only ever by the user inviting others into it.
pub async fn create_personal_vault(db: &OrgDb, user_id: &RecordId) -> AppResult<RecordId> {
    // Deterministic id: a second personal vault for the same user collides on
    // the key, so "one personal vault per user" holds even under a race.
    let vault_id = RecordId::from_table_key("vault", crate::tx::stable_key('p', &user_id.to_string()));
    with_retry_dup(|| async {
        store::vaults::CREATE_PERSONAL.on(db)
        .bind(("vault", vault_id.clone()))
        .bind(("user", user_id.clone()))
        .await?
        .check()
    })
    .await?;
    Ok(vault_id)
}

const MAX_VAULT_NAME: usize = 80;

/// A vault name must be non-empty once trimmed, at most 80 characters, and unique
/// (case-insensitively) among the vaults the caller belongs to: that is the set a
/// by-name lookup or a dropdown searches, so per membership set (not per owner)
/// is what keeps them unambiguous. `except` is the vault being renamed.
/// ponytail: check-then-write, two simultaneous creates of one name can both pass.
async fn check_name(db: &OrgDb, user_id: &RecordId, name: &str, except: Option<&RecordId>) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::bad_request("vault name can't be empty"));
    }
    if name.chars().count() > MAX_VAULT_NAME {
        return Err(AppError::bad_request(format!("vault name can't be longer than {MAX_VAULT_NAME} characters")));
    }
    if taken(db, user_id, name, except).await? {
        return Err(AppError::coded(ErrorCode::VaultNameTaken, format!("you already have a vault named {name:?}: pick another name")));
    }
    Ok(name.to_string())
}

async fn taken(db: &OrgDb, user_id: &RecordId, name: &str, except: Option<&RecordId>) -> AppResult<bool> {
    let want = name.to_lowercase();
    let mine = list_my_vaults(db, user_id).await?;
    Ok(mine.iter().any(|v| v.name.to_lowercase() == want && except.is_none_or(|e| e.to_string() != v.id)))
}

/// "base", or "base 2", "base 3", ... for a clone or merge named by default.
async fn free_name(db: &OrgDb, user_id: &RecordId, base: String) -> AppResult<String> {
    let mut name = base.clone();
    for n in 2.. {
        if !taken(db, user_id, &name, None).await? {
            break;
        }
        name = format!("{base} {n}");
    }
    Ok(name)
}

/// Create a new vault (org, or an extra personal-style one -- a user can have
/// several, per the product ask); creator becomes its owner.
pub async fn create_vault(db: &OrgDb, user_id: &RecordId, name: &str, kind: &str) -> AppResult<VaultOut> {
    authz::require_unrestricted()?;
    let name = &check_name(db, user_id, name, None).await?;
    let vault = with_retry(|| async {
        let mut res = store::vaults::CREATE_VAULT
            .on(db)
            .bind(("name", name.to_string()))
            .bind(("kind", kind.to_string()))
            .bind(("user", user_id.clone()))
            .await?
            .check()?;
        let rows: Vec<VaultFullRow> = res.take(store::vaults::CREATE_VAULT.slot)?;
        Ok(rows.into_iter().next())
    })
    .await?
    .ok_or_else(|| AppError::internal("vault insert returned no row"))?;
    Ok(vault.into())
}

/// Active membership only -- a pending invite isn't membership yet.
async fn membership(db: &OrgDb, vault_id: &RecordId, user_id: &RecordId) -> AppResult<Option<MembershipRow>> {
    let mut res = store::vaults::MEMBERSHIP_ACTIVE.on(db)
        .bind(("vault", vault_id.clone()))
        .bind(("user", user_id.clone()))
        .await?;
    let rows: Vec<MembershipRow> = res.take(0)?;
    Ok(rows.into_iter().next())
}

/// Any `vault_member` row regardless of status -- used only where a pending
/// invite also needs to count (duplicate-invite checks, accept/decline).
async fn membership_any_status(db: &OrgDb, vault_id: &RecordId, user_id: &RecordId) -> AppResult<Option<MembershipRow>> {
    let mut res = store::vaults::MEMBERSHIP_ANY.on(db)
        .bind(("vault", vault_id.clone()))
        .bind(("user", user_id.clone()))
        .await?;
    let rows: Vec<MembershipRow> = res.take(0)?;
    Ok(rows.into_iter().next())
}

/// Every vault `user_id` belongs to (any role) -- the read/write scope passed
/// to entity/recall lookups.
pub async fn accessible_vault_ids(db: &OrgDb, user_id: &RecordId) -> AppResult<Vec<RecordId>> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        vault: RecordId,
    }
    let mut res = store::vaults::ACCESSIBLE_IDS.on(db)
        .bind(("user", user_id.clone()))
        .await?;
    let rows: Vec<Row> = res.take(0)?;
    let only = authz::restricted_vault()?;
    Ok(rows.into_iter().map(|r| r.vault).filter(|v| only.as_ref().is_none_or(|o| o == v)).collect())
}

/// `user_id`'s personal vault -- the implicit scope for any tool call that
/// doesn't pass `vault_id`, so existing single-user callers need no changes.
pub async fn default_vault_id(db: &OrgDb, user_id: &RecordId) -> AppResult<RecordId> {
    // a vault-restricted token's default scope is its vault, not the personal one
    if let Some(only) = authz::restricted_vault()? {
        authz::ensure_member(db, user_id, &only).await?;
        return Ok(only);
    }
    personal_vault_id(db, user_id).await
}

/// The user's own personal vault, whatever the credential is restricted to. The
/// only vault that also draws on the user's synced source records.
pub async fn personal_vault_id(db: &OrgDb, user_id: &RecordId) -> AppResult<RecordId> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        vault: RecordId,
    }
    let mut res = store::vaults::DEFAULT_ID.on(db)
        .bind(("user", user_id.clone()))
        .await?;
    let rows: Vec<Row> = res.take(0)?;
    rows.into_iter().next().map(|r| r.vault).ok_or_else(|| {
        AppError::internal(format!("user {} has no personal vault -- registration should have created one", user_id.to_string()))
    })
}

/// The vaults a read covers, from one membership query: `vault_ids` (each must be one of the
/// user's, and inside a vault-restricted token's vault; duplicates dropped), or without them the user's personal vault first
/// plus every org vault they're an active member of, oldest membership first -- not other people's
/// personal vaults they've joined, which stay opt-in. A vault-restricted credential reads only its
/// vault. The same checks `authz::authorize` makes for `ReadMemories`, without a query per vault.
pub struct ReadScope {
    pub vaults: Vec<RecordId>,
    /// The user's own personal vault (whatever the credential is restricted to).
    pub personal: RecordId,
    /// Vault id -> name, for the vaults the credential may see.
    pub names: HashMap<String, String>,
}

pub async fn read_scope(db: &OrgDb, user_id: &RecordId, vault_ids: &[RecordId]) -> AppResult<ReadScope> {
    authz::require_scope(Action::ReadMemories.scope())?;
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        vault: RecordId,
        #[serde(default)]
        #[surreal(default)]
        kind: Option<String>,
        #[serde(default)]
        #[surreal(default)]
        name: Option<String>,
    }
    let mut res = store::vaults::MY_MEMBERSHIPS.on(db).bind(("user", user_id.clone())).await?;
    let rows: Vec<Row> = res.take(0)?;
    let personal = rows.iter().find(|r| r.kind.as_deref() == Some("personal")).map(|r| r.vault.clone()).ok_or_else(|| {
        AppError::internal(format!("user {} has no personal vault -- registration should have created one", user_id.to_string()))
    })?;
    let only = authz::restricted_vault()?;
    let member = |v: &RecordId| {
        if rows.iter().any(|r| &r.vault == v) {
            Ok(())
        } else {
            Err(AppError::coded(ErrorCode::VaultForbidden, format!("not a member of vault {}", v.to_string())))
        }
    };
    let vaults = match (vault_ids, &only) {
        ([_, ..], _) => {
            let mut out: Vec<RecordId> = Vec::with_capacity(vault_ids.len());
            for v in vault_ids {
                authz::check_vault(v)?;
                member(v)?;
                if !out.contains(v) {
                    out.push(v.clone());
                }
            }
            out
        }
        ([], Some(o)) => {
            member(o)?;
            vec![o.clone()]
        }
        ([], None) => std::iter::once(personal.clone())
            .chain(rows.iter().filter(|r| r.kind.as_deref() == Some("org")).map(|r| r.vault.clone()))
            .collect(),
    };
    let names = rows
        .into_iter()
        .filter(|r| only.as_ref().is_none_or(|o| o == &r.vault))
        .map(|r| (r.vault.to_string(), r.name.unwrap_or_default()))
        .collect();
    Ok(ReadScope { vaults, personal, names })
}

pub async fn list_my_vaults(db: &OrgDb, user_id: &RecordId) -> AppResult<Vec<VaultWithRole>> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        vault: VaultFullRow,
        role: String,
    }
    let mut res = store::vaults::LIST_MINE.on(db)
        .bind(("user", user_id.clone()))
        .await?;
    let rows: Vec<Row> = res.take(0)?;
    let only = authz::restricted_vault()?;
    Ok(rows
        .into_iter()
        .filter(|r| only.as_ref().is_none_or(|o| o == &r.vault.id))
        .map(|r| {
            let v: VaultOut = r.vault.into();
            VaultWithRole { id: v.id, name: v.name, kind: v.kind, created_at: v.created_at, role: r.role }
        })
        .collect())
}

pub async fn rename_vault(db: &OrgDb, user_id: &RecordId, vault_id: &RecordId, name: &str) -> AppResult<VaultOut> {
    authz::authorize(db, user_id, Action::Rename, vault_id).await?;
    let name = &check_name(db, user_id, name, Some(vault_id)).await?;
    let mut res = store::vaults::RENAME.on(db)
        .bind(("id", vault_id.clone()))
        .bind(("name", name.to_string()))
        .await?;
    let rows: Vec<VaultFullRow> = res.take(0)?;
    let vault = rows.into_iter().next().ok_or_else(|| AppError::internal("vault update returned no row"))?;
    Ok(vault.into())
}

/// Owner-only. Deletes memberships; leaves any person/organisation/.../memory
/// rows already in the vault in place (orphaned-but-inaccessible, same
/// accepted tradeoff as the rest of this codebase's delete paths -- add a
/// cascade if dangling vault data ever becomes a real problem).
pub async fn delete_vault(db: &OrgDb, user_id: &RecordId, vault_id: &RecordId) -> AppResult<()> {
    authz::authorize(db, user_id, Action::Delete, vault_id).await?;
    with_retry(|| async {
        store::vaults::DELETE_VAULT.on(db)
            .bind(("vault", vault_id.clone()))
            .await?
            .check()
    })
    .await?;
    Ok(())
}

/// Add `email` to `vault_id`. Caller must already be an owner (so admin
/// rights only ever trace back to "created it" or "an existing owner
/// promoted me" -- never self-granted).
/// The user with `email`, if they belong to this org: an invitee outside it is "no such user", so an
/// email cannot be probed across orgs.
async fn user_in_org(db: &OrgDb, control: &ControlDb, email: &str) -> AppResult<Option<RecordId>> {
    let mut res = store::control::EMAIL_IN_ORG
        .on(control)
        .bind(("email", crate::models_user::normalize_email(email)))
        .bind(("org", db.org().record()))
        .await?;
    let rows: Vec<EmailLookupRow> = res.take(0)?;
    Ok(rows.into_iter().next().map(|r| r.id))
}

pub async fn invite_member(
    db: &OrgDb,
    control: &ControlDb,
    user_id: &RecordId,
    vault_id: &RecordId,
    email: &str,
    role: &str,
) -> AppResult<InviteOut> {
    authz::authorize(db, user_id, Action::Invite, vault_id).await?;

    let invitee = user_in_org(db, control, email)
        .await?
        .ok_or_else(|| AppError::bad_request(format!("no user with email '{email}'")))?;

    if membership_any_status(db, vault_id, &invitee).await?.is_some() {
        return Err(AppError::bad_request(format!("{email} is already a member or has a pending invite")));
    }

    let mut res = store::vaults::INVITE.on(db)
        .bind(("vault", vault_id.clone()))
        .bind(("user", invitee))
        .bind(("role", role.to_string()))
        .await?
        .check()
        .map_err(|e| {
            // lost the race against a concurrent invite of the same user
            if e.to_string().contains("already contains") {
                AppError::bad_request(format!("{email} is already a member or has a pending invite"))
            } else {
                e.into()
            }
        })?;
    let rows: Vec<MembershipRow> = res.take(0)?;
    let member = rows.into_iter().next().ok_or_else(|| AppError::internal("vault_member insert returned no row"))?;

    Ok(InviteOut { vault_id: vault_id.to_string(), user_email: email.to_string(), role: member.role })
}

pub async fn list_members(db: &OrgDb, control: &ControlDb, user_id: &RecordId, vault_id: &RecordId) -> AppResult<Vec<MemberOut>> {
    authz::authorize(db, user_id, Action::ListMembers, vault_id).await?;
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        user: RecordId,
        role: String,
    }
    #[derive(Deserialize, SurrealValue)]
    struct Email {
        id: RecordId,
        email: String,
    }
    let mut res = store::vaults::LIST_MEMBERS.on(db)
        .bind(("vault", vault_id.clone()))
        .await?;
    let rows: Vec<Row> = res.take(0)?;
    let mut res = store::entities::EMAILS_FOR.on(control).bind(("ids", rows.iter().map(|r| r.user.clone()).collect::<Vec<_>>())).await?;
    let emails: Vec<Email> = res.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(|r| emails.iter().find(|e| e.id == r.user).map(|e| MemberOut { email: e.email.clone(), role: r.role }))
        .collect())
}

/// Pending invitations for `user_id` across every vault -- what the vaults
/// page's invitations panel renders to accept/decline.
pub async fn list_my_invitations(db: &OrgDb, user_id: &RecordId) -> AppResult<Vec<InvitationOut>> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        vault: VaultFullRow,
        role: String,
        created_at: Option<Datetime>,
    }
    let mut res = store::vaults::LIST_INVITATIONS.on(db)
        .bind(("user", user_id.clone()))
        .await?;
    let rows: Vec<Row> = res.take(0)?;
    let only = authz::restricted_vault()?;
    Ok(rows
        .into_iter()
        .filter(|r| only.as_ref().is_none_or(|o| o == &r.vault.id))
        .map(|r| InvitationOut {
            vault_id: r.vault.id.to_string(),
            vault_name: r.vault.name,
            vault_kind: r.vault.kind,
            role: r.role,
            created_at: r.created_at,
        })
        .collect())
}

/// Accept a pending invitation into `vault_id`. Only the invitee can accept
/// their own invite.
pub async fn accept_invitation(db: &OrgDb, user_id: &RecordId, vault_id: &RecordId) -> AppResult<VaultWithRole> {
    authz::check_vault(vault_id)?;
    let m = membership_any_status(db, vault_id, user_id)
        .await?
        .filter(|m| m.status == "pending")
        .ok_or_else(|| AppError::bad_request("no pending invitation for this vault"))?;

    // Guarded flip plus the vault read in one transaction: a double accept, or
    // an accept racing a decline/withdraw, activates at most once.
    let mut res = with_retry(|| async {
        store::vaults::ACCEPT_INVITATION.on(db)
        .bind(("id", m.id.clone()))
        .bind(("vault", vault_id.clone()))
        .await?
        .check()
    })
    .await?;
    let rows: Vec<VaultFullRow> = res.take(store::vaults::ACCEPT_INVITATION.slot)?;
    let vault = rows.into_iter().next().ok_or_else(|| AppError::bad_request("no pending invitation for this vault"))?;
    let v: VaultOut = vault.into();
    Ok(VaultWithRole { id: v.id, name: v.name, kind: v.kind, created_at: v.created_at, role: m.role })
}

/// Decline (delete) a pending invitation into `vault_id`.
pub async fn decline_invitation(db: &OrgDb, user_id: &RecordId, vault_id: &RecordId) -> AppResult<()> {
    authz::check_vault(vault_id)?;
    let m = membership_any_status(db, vault_id, user_id)
        .await?
        .filter(|m| m.status == "pending")
        .ok_or_else(|| AppError::bad_request("no pending invitation for this vault"))?;

    store::vaults::DELETE_RECORD.on(db).bind(("id", m.id)).await?;
    Ok(())
}

/// Owner-only. Refuses to remove the last owner, so a vault can't be left
/// admin-less.
pub async fn remove_member(db: &OrgDb, control: &ControlDb, user_id: &RecordId, vault_id: &RecordId, email: &str) -> AppResult<()> {
    authz::authorize(db, user_id, Action::ManageMembers, vault_id).await?;

    let target_id = user_in_org(db, control, email).await?.ok_or_else(|| AppError::bad_request(format!("no user with email '{email}'")))?;

    // any status: this also withdraws a pending invitation
    let target_membership = membership_any_status(db, vault_id, &target_id)
        .await?
        .ok_or_else(|| AppError::bad_request(format!("{email} is not a member or invitee")))?;

    // Owner count check and delete in one transaction, so two concurrent
    // removals cannot each see a second owner and strand the vault.
    with_retry(|| async {
        store::vaults::REMOVE_MEMBER.on(db)
        .bind(("vault", vault_id.clone()))
        .bind(("is_owner", target_membership.role == "owner"))
        .bind(("id", target_membership.id.clone()))
        .await?
        .check()
    })
    .await
    .map_err(|e| {
        if e.to_string().contains("last_owner") { AppError::bad_request("cannot remove the last owner") } else { e.into() }
    })?;
    Ok(())
}

#[derive(Debug, Deserialize, SurrealValue)]
struct EntityRow {
    id: RecordId,
    /// set on a custom-kind (`entity` table) row only
    #[serde(default)]
    #[surreal(default)]
    kind: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    name: String,
    #[serde(default)]
    #[surreal(default)]
    aliases: Vec<String>,
    #[serde(default)]
    #[surreal(default)]
    summary: String,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct MemoryRow {
    #[serde(default)]
    #[surreal(default)]
    text: String,
    #[serde(rename = "type", default = "default_memory_type")]
    #[surreal(rename = "type", default = "default_memory_type")]
    mem_type: String,
    #[serde(default)]
    #[surreal(default)]
    source: Option<RecordId>,
}

fn default_memory_type() -> String {
    "world".to_string()
}

#[derive(Debug, Deserialize, SurrealValue)]
struct RelationRow {
    #[serde(rename = "out")]
    #[surreal(rename = "out")]
    other: RecordId,
    #[serde(default)]
    #[surreal(default)]
    label: String,
}

/// Looks an entity up in `vault` by case-insensitive name (merge matching).
/// The `table` entity in `vault` named `name` (any case), of the same custom `kind` for the custom-kind table.
async fn find_by_name(db: &OrgDb, table: &str, kind: Option<&str>, vault: &RecordId, name: &str) -> AppResult<Option<EntityRow>> {
    let k = if kind.is_some() { " AND kind = $kind" } else { "" };
    // dynamic: table name varies over the entity tables
    let mut res = store::dynamic(db, "vaults.find_by_name", format!("SELECT * FROM {table} WHERE vault = $vault{k} AND string::lowercase(name) = $name LIMIT 1"))
        .bind(("kind", kind.unwrap_or_default().to_string()))
        .bind(("vault", vault.clone()))
        .bind(("name", name.to_lowercase()))
        .await?;
    let rows: Vec<EntityRow> = res.take(0)?;
    Ok(rows.into_iter().next())
}

/// Merge rule for one incoming memory against what `subject` already has:
/// an identical fact is skipped, and a second observation is appended to the
/// existing one (one observation per subject). Returns true if handled.
async fn fold_into_existing_memory(db: &OrgDb, subject: &RecordId, mem: &MemoryRow) -> AppResult<bool> {
    #[derive(Deserialize, SurrealValue)]
    struct Existing {
        id: RecordId,
        #[serde(default)]
        #[surreal(default)]
        text: String,
    }
    if mem.mem_type == "observation" {
        let mut res = store::vaults::OBSERVATION_OF.on(db)
            .bind(("s", subject.clone()))
            .await?;
        let existing: Vec<Existing> = res.take(0)?;
        let Some(existing) = existing.into_iter().next() else { return Ok(false) };
        if existing.text != mem.text {
            store::vaults::APPEND_OBSERVATION.on(db)
                .bind(("id", existing.id))
                .bind(("text", format!("{}\n\n{}", existing.text, mem.text)))
                .await?;
        }
        return Ok(true);
    }
    let mut res = store::vaults::SAME_MEMORY.on(db)
        .bind(("s", subject.clone()))
        .bind(("type", mem.mem_type.clone()))
        .bind(("text", mem.text.clone()))
        .await?;
    let same: Vec<VaultRow> = res.take(0)?;
    Ok(!same.is_empty())
}

/// Test-only: when set to n > 0, the n-th memory `copy_into` is about to write fails instead (then it
/// is 0 again), to prove a failed clone or merge leaves nothing behind.
#[cfg(feature = "test-support")]
pub static FAIL_COPY_AT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A copy in progress older than this is one whose process died; [`discard_stale_copies`] removes it.
const STALE_COPY_AGE: &str = "1h";

/// The new vault a clone or merge copies into: no one can see it until [`publish_or_discard`].
async fn stage_copy(db: &OrgDb, name: &str, kind: &str) -> AppResult<VaultFullRow> {
    let rows: Vec<VaultFullRow> = with_retry(|| async {
        store::vaults::STAGE_COPY.on(db).bind(("name", name.to_string())).bind(("kind", kind.to_string())).await?.take(0)
    })
    .await?;
    rows.into_iter().next().ok_or_else(|| AppError::internal("vault insert returned no row"))
}

/// The end of a clone or merge into the staged `vault`: when the copy succeeded, it becomes a vault
/// `user_id` owns, in one transaction; when that or the copy failed, the partial vault is deleted, so
/// a failure leaves no target behind (if even the delete fails, the scheduler retries it).
async fn publish_or_discard<T>(db: &OrgDb, user_id: &RecordId, vault: &RecordId, copied: AppResult<T>) -> AppResult<T> {
    let out = match copied {
        Ok(v) => with_retry(|| async {
            store::vaults::PUBLISH_COPY.on(db).bind(("vault", vault.clone())).bind(("user", user_id.clone())).await?.check()
        })
        .await
        .map(|_| v)
        .map_err(AppError::from),
        Err(e) => Err(e),
    };
    if out.is_err()
        && let Err(e) = discard_copy(db, vault).await
    {
        tracing::warn!(vault = %vault.to_string(), error = %e.message, "deleting a failed vault copy failed; the scheduler retries");
    }
    out
}

/// Deletes a copy in progress and everything it wrote; a vault that is not one is left alone.
async fn discard_copy(db: &OrgDb, vault: &RecordId) -> AppResult<()> {
    with_retry(|| async { store::vaults::DISCARD_COPY_ROWS.on(db).bind(("vault", vault.clone())).await?.check() }).await?;
    with_retry(|| async { store::vaults::DISCARD_COPY_VAULT.on(db).bind(("vault", vault.clone())).await?.check() }).await?;
    Ok(())
}

/// Deletes the clone and merge targets a crash left half-copied. Run by the scheduler (its first pass
/// after a start, then periodically). Returns how many it removed.
pub async fn discard_stale_copies(db: &OrgDb) -> AppResult<usize> {
    let mut res = store::vaults::STALE_COPIES.on(db).bind(("age", STALE_COPY_AGE)).await?;
    let stale: Vec<RecordId> = res.take(0)?;
    for vault in &stale {
        discard_copy(db, vault).await?;
    }
    Ok(stale.len())
}

/// Copies every entity, memory and relation of `src` into `dest` (both
/// already accessible to `user_id`); returns how many entities of `src` landed
/// in `dest`. With `merge_duplicates`, entities that share a kind and
/// (case-insensitive) name with one already in `dest` are folded into it
/// instead of duplicated.
async fn copy_into(db: &OrgDb, user_id: &RecordId, src: &RecordId, dest: &RecordId, merge_duplicates: bool) -> AppResult<usize> {
    #[allow(clippy::mutable_key_type)] // RecordId hashes by value; the interior mutability is never touched
    let mut id_map: HashMap<RecordId, RecordId> = HashMap::new();
    for &entity_kind in ENTITY_KINDS {
        // dynamic: table name varies over the entity tables
        let res = store::dynamic(db, "vaults.entities_in_vault", format!("SELECT * FROM {entity_kind} WHERE vault = $vault"))
            .bind(("vault", src.clone()))
            .await;
        let rows: Vec<EntityRow> = match res {
            Err(e) if crate::entities::service::no_custom_table(&e) => Vec::new(),
            res => res?.take(0)?,
        };
        for row in rows {
            if merge_duplicates
                && let Some(existing) = find_by_name(db, entity_kind, row.kind.as_deref(), dest, &row.name).await? {
                    let mut aliases = existing.aliases.clone();
                    for alias in row.aliases.iter().cloned() {
                        if !aliases.iter().any(|a| a.eq_ignore_ascii_case(&alias)) {
                            aliases.push(alias);
                        }
                    }
                    let summary = if existing.summary.is_empty() { row.summary.clone() } else { existing.summary.clone() };
                    store::vaults::MERGE_ENTITY_INTO.on(db)
                        .bind(("id", existing.id.clone()))
                        .bind(("aliases", aliases))
                        .bind(("summary", summary))
                        .await?;
                    id_map.insert(row.id, existing.id);
                    continue;
                }
            // dynamic: table name varies over the entity tables; a custom-kind row keeps its kind
            let k = if row.kind.is_some() { ", kind = $kind" } else { "" };
            let mut created = store::dynamic(
                db,
                "vaults.copy_entity",
                format!(
                    "CREATE {entity_kind} SET owner = $owner, vault = $vault{k}, name = $name, \
                     aliases = $aliases, summary = $summary RETURN AFTER"
                ),
            )
                .bind(("kind", row.kind.unwrap_or_default()))
                .bind(("owner", user_id.clone()))
                .bind(("vault", dest.clone()))
                .bind(("name", row.name))
                .bind(("aliases", row.aliases))
                .bind(("summary", row.summary))
                .await?;
            let created_rows: Vec<VaultRow> = created.take(0)?;
            let created_row =
                created_rows.into_iter().next().ok_or_else(|| AppError::internal("entity insert returned no row"))?;
            id_map.insert(row.id, created_row.id);
        }
    }

    for (old_id, new_id) in id_map.clone() {
        let mut res = store::vaults::MEMORIES_OF.on(db)
            .bind(("id", old_id))
            .bind(("vault", src.clone()))
            .await?;
        let memories: Vec<MemoryRow> = res.take(0)?;
        for mem in memories {
            if merge_duplicates && fold_into_existing_memory(db, &new_id, &mem).await? {
                continue;
            }
            #[cfg(feature = "test-support")]
            if FAIL_COPY_AT.load(std::sync::atomic::Ordering::SeqCst) > 0 && FAIL_COPY_AT.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 {
                return Err(AppError::internal("injected vault copy failure"));
            }
            store::vaults::COPY_MEMORY.on(db)
            .bind(("owner", user_id.clone()))
            .bind(("vault", dest.clone()))
            .bind(("subject", new_id.clone()))
            .bind(("text", mem.text))
            .bind(("type", mem.mem_type))
            .bind(("source", mem.source))
            .await?;
        }
    }

    #[allow(clippy::mutable_key_type)] // RecordId hashes by value; the interior mutability is never touched
    let mut seen_edges: HashSet<(RecordId, RecordId, String)> = HashSet::new();
    for (old_id, new_id) in id_map.clone() {
        let mut res = store::vaults::RELATIONS_FROM.on(db).bind(("id", old_id.clone())).await?;
        let edges: Vec<RelationRow> = res.take(0)?;
        for edge in edges {
            let Some(other_new) = id_map.get(&edge.other) else { continue };
            let key = (old_id.clone(), edge.other.clone(), edge.label.clone());
            if seen_edges.contains(&key) {
                continue;
            }
            seen_edges.insert(key);
            if merge_duplicates {
                let mut res = store::vaults::SAME_RELATION.on(db)
                    .bind(("in", new_id.clone()))
                    .bind(("out", other_new.clone()))
                    .bind(("label", edge.label.clone()))
                    .await?;
                let found: Vec<VaultRow> = res.take(0)?;
                if !found.is_empty() {
                    continue;
                }
            }
            store::vaults::COPY_RELATION.on(db)
                .bind(("in", new_id.clone()))
                .bind(("out", other_new.clone()))
                .bind(("label", edge.label))
                .bind(("owner", user_id.clone()))
                .await?;
        }
    }

    Ok(id_map.len())
}

/// Deep-copy `vault_id` (read access required, not necessarily owner) into a
/// brand-new vault the caller owns -- every entity, memory, and relation is
/// duplicated, not referenced, so editing the clone never touches the
/// source. The typical use: clone your personal vault into a fresh org vault
/// to start sharing a snapshot of it, without exposing the original.
///
/// `source_memories` (an observation's consolidation lineage, pointing at
/// other `memory` ids) is dropped on the copies rather than remapped -- those
/// ids belong to the source vault and would dangle; a cloned observation
/// reads fine, it just starts its lineage over. Likewise a relation whose
/// other endpoint isn't in the cloned vault (e.g. it points cross-vault at
/// something `user_id` can't read) is skipped rather than left dangling.
pub async fn clone_vault(
    db: &OrgDb,
    user_id: &RecordId,
    vault_id: &RecordId,
    name: Option<&str>,
    kind: &str,
) -> AppResult<CloneOut> {
    authz::authorize(db, user_id, Action::Clone, vault_id).await?;
    authz::require_unrestricted()?;

    let source: Option<VaultFullRow> = store::get(db, vault_id).await?;
    let source = source.ok_or_else(|| AppError::coded(crate::error::ErrorCode::VaultNotFound, format!("vault not found: {}", vault_id.to_string())))?;

    let clone_name = match name {
        Some(n) => n.to_string(),
        None => free_name(db, user_id, format!("{} (copy)", source.name)).await?,
    };
    let clone_name = check_name(db, user_id, &clone_name, None).await?;
    let staged = stage_copy(db, &clone_name, kind).await?;
    let copied = copy_into(db, user_id, vault_id, &staged.id, false).await;
    let copied = publish_or_discard(db, user_id, &staged.id, copied).await?;
    let clone: VaultOut = staged.into();

    Ok(CloneOut {
        id: clone.id,
        name: clone.name,
        kind: clone.kind,
        created_at: clone.created_at,
        entities_copied: copied,
    })
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MergeOut {
    pub id: String,
    pub name: String,
    pub kind: String,
    #[schema(value_type = Option<String>)]
    pub created_at: Option<Datetime>,
    /// Entities in the merged vault (duplicates across the two sources count once).
    pub entities: usize,
    pub merged_from: [String; 2],
}

/// Merge two vaults into a NEW one: both are copied (never moved -- the
/// sources stay exactly as they were) into a fresh vault the caller owns, the
/// second folding into the first wherever an entity of the same kind and name
/// exists in both (aliases unioned, identical facts kept once, relations
/// deduplicated, a second observation appended to the first).
pub async fn merge_vaults(
    db: &OrgDb,
    user_id: &RecordId,
    a: &RecordId,
    b: &RecordId,
    name: Option<&str>,
    kind: &str,
) -> AppResult<MergeOut> {
    if a == b {
        return Err(AppError::bad_request("pick two different vaults to merge"));
    }
    authz::authorize(db, user_id, Action::Merge, a).await?;
    authz::authorize(db, user_id, Action::Merge, b).await?;
    authz::require_unrestricted()?;
    let (va, vb): (Option<VaultFullRow>, Option<VaultFullRow>) = (store::get(db, a).await?, store::get(db, b).await?);
    let va = va.ok_or_else(|| AppError::coded(crate::error::ErrorCode::VaultNotFound, format!("vault not found: {}", a.to_string())))?;
    let vb = vb.ok_or_else(|| AppError::coded(crate::error::ErrorCode::VaultNotFound, format!("vault not found: {}", b.to_string())))?;

    let merged_name = match name {
        Some(n) => n.to_string(),
        None => free_name(db, user_id, format!("{} + {}", va.name, vb.name)).await?,
    };
    let merged_name = check_name(db, user_id, &merged_name, None).await?;
    let staged = stage_copy(db, &merged_name, kind).await?;
    let dest = staged.id.clone();

    let copied = async {
        copy_into(db, user_id, a, &dest, true).await?;
        copy_into(db, user_id, b, &dest, true).await?;
        let mut entities = 0;
        for entity_kind in ENTITY_KINDS {
            // dynamic: table name varies over the entity tables
            let res = store::dynamic(db, "vaults.count_entities", format!("SELECT count() FROM {entity_kind} WHERE vault = $vault GROUP ALL"))
                .bind(("vault", dest.clone()))
                .await;
            let rows: Vec<CountRow> = match res {
                Err(e) if crate::entities::service::no_custom_table(&e) => Vec::new(),
                res => res?.take(0)?,
            };
            entities += rows.first().map(|r| r.count).unwrap_or(0) as usize;
        }
        AppResult::Ok(entities)
    }
    .await;
    let entities = publish_or_discard(db, user_id, &dest, copied).await?;
    let merged: VaultOut = staged.into();
    Ok(MergeOut {
        id: merged.id,
        name: merged.name,
        kind: merged.kind,
        created_at: merged.created_at,
        entities,
        merged_from: [a.to_string(), b.to_string()],
    })
}

/// Any member can leave their own membership, except the last owner of a
/// vault that still has other members (would strand them admin-less --
/// delete the vault instead if that's the intent).
pub async fn leave_vault(db: &OrgDb, user_id: &RecordId, vault_id: &RecordId) -> AppResult<()> {
    match authz::authorize(db, user_id, Action::Leave, vault_id).await {
        Ok(_) => {}
        // not a member: nothing to leave
        Err(e) if e.code == crate::error::ErrorCode::VaultForbidden => return Ok(()),
        Err(e) => return Err(e),
    }
    let Some(m) = membership(db, vault_id, user_id).await? else { return Ok(()) };

    with_retry(|| async {
        store::vaults::LEAVE.on(db)
        .bind(("vault", vault_id.clone()))
        .bind(("user", user_id.clone()))
        .bind(("is_owner", m.role == "owner"))
        .bind(("id", m.id.clone()))
        .await?
        .check()
    })
    .await
    .map_err(|e| {
        if e.to_string().contains("last_owner") {
            AppError::bad_request("you are the last owner -- promote someone else first")
        } else {
            e.into()
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod vault_tests {
    use super::*;

    #[test]
    fn vault_out_from_full_row_carries_fields() {
        let row = VaultFullRow {
            id: crate::rid::parse("vault:abc").unwrap(),
            name: "Personal".to_string(),
            kind: "personal".to_string(),
            created_at: None,
        };
        let out: VaultOut = row.into();
        assert_eq!(out.id, "vault:abc");
        assert_eq!(out.name, "Personal");
        assert_eq!(out.kind, "personal");
    }

    #[test]
    fn default_kind_is_personal() {
        assert_eq!(default_kind(), "personal");
    }

    #[test]
    fn default_memory_type_is_world() {
        assert_eq!(default_memory_type(), "world");
    }

    #[test]
    fn entity_kinds_match_memory_subject_union() {
        assert_eq!(ENTITY_KINDS, ["person", "organisation", "location", "repository", "file", "symbol", "entity"]);
    }
}
