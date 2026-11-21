//! The operator API: what the billing service of a hosted Eunomia calls to create an organisation for a
//! buyer, size it, pause it when payment fails and resume it when they pay (`routers/operator.rs`,
//! docs/deployment.md "Running a hosted service").
//!
//! * **One caller, one credential.** The routes answer only to `EUNOMIA_OPERATOR_TOKEN` (`gate::operator_gate`)
//!   and exist only while it is set. Nothing here is reachable with a session, an API token or an OAuth token,
//!   and the token opens nothing else.
//! * **The organisation comes from the path**, validated: an id that is not a well-formed organisation, or
//!   names none, is `operator.org_not_found`. No function reads a member's request context.
//! * **Control database only.** Everything here reads and writes the `control` database: organisation
//!   records, membership and invitation counts. Nothing opens an organisation's own database
//!   (`Pool::for_org` is called once, to learn whether provisioning finished, and reads nothing from it), so
//!   a memory, a document or a chat can never come out of this API. A test pins it.
//! * **Every call is on the instance ledger** as actor `operator` / `operator-api`, with its outcome. The
//!   token is in no row, no log line and no error.
//! * **Creating is idempotent.** The organisation's id is derived from the external reference, so a retried
//!   webhook finds the same organisation. The records are made first, the database is provisioned next and
//!   the owner claim last, and a call that finds a half-made organisation finishes the missing steps.
//! * **The owner claim** is an organisation invitation of kind `owner` (`invitations.rs`): a single-use,
//!   expiring token whose hash is stored, claimed through the ordinary invitation page. The account that
//!   claims it signs up as for any invitation (password or single sign-on, two-step verification rules
//!   applying) and becomes the organisation's one owner.

use std::future::Future;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use crate::audit::{self, Event};
use crate::authz::Actor;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::invitations::TTL_DAYS;
use crate::models_user;
use crate::plan::{self, PlanState};
use crate::pool::OrgId;
use crate::rid::RecordIdExt;
use crate::state::AppState;
use crate::store;
use crate::store::plan as q;

/// What the instance ledger calls the operator API.
pub const ACTOR_KIND: &str = "operator";
pub const ACTOR_ID: &str = "operator-api";

fn actor() -> Actor {
    Actor { kind: ACTOR_KIND, id: ACTOR_ID.to_string() }
}

tokio::task_local! {
    /// Set by the gate around an operator call: whether a handler wrote its own ledger row, so the gate writes
    /// one for a call that never reached a handler (a body that does not parse, an unknown path or method).
    pub static AUDITED: std::sync::Arc<std::sync::atomic::AtomicBool>;
}

/// Runs `fut` and writes one instance-ledger row for it, whatever it returns.
async fn audited<T>(state: &AppState, action: &str, target: &str, detail: String, fut: impl Future<Output = AppResult<T>>) -> AppResult<T> {
    let _ = AUDITED.try_with(|a| a.store(true, std::sync::atomic::Ordering::SeqCst));
    let result = fut.await;
    let outcome = match &result {
        Ok(_) => "ok",
        Err(e) => e.code.as_str(),
    };
    audit::record(&state.control, Event { user: None, actor: &actor(), action, target, outcome, detail: &detail }).await;
    result
}

/// `trial`, `team` or `enterprise`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PlanName {
    Trial,
    Team,
    Enterprise,
}

impl PlanName {
    pub fn as_str(self) -> &'static str {
        match self {
            PlanName::Trial => "trial",
            PlanName::Team => "team",
            PlanName::Enterprise => "enterprise",
        }
    }
}

/// Distinguishes "not sent" from "sent as null" (a field that can be cleared).
fn sent<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::deserialize(d)?))
}

const MAX_SEATS: i64 = 1_000_000;
const MAX_REF_LEN: usize = 200;

fn invalid(msg: impl Into<String>) -> AppError {
    AppError::coded(ErrorCode::ValidationInvalid, msg)
}

fn valid_name(name: &str) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(invalid("`name` must be 1 to 80 characters."));
    }
    // a name with an address in it is read as a personal workspace and hidden from invitation links
    if name.contains('@') || name.chars().any(char::is_control) {
        return Err(invalid("`name` can't contain `@` or control characters."));
    }
    Ok(name.to_string())
}

fn valid_seats(n: i64) -> AppResult<i64> {
    if (1..=MAX_SEATS).contains(&n) {
        Ok(n)
    } else {
        Err(invalid(format!("`seat_limit` must be between 1 and {MAX_SEATS}.")))
    }
}

fn valid_ref(r: &str) -> AppResult<String> {
    let r = r.trim();
    if r.is_empty() || r.chars().count() > MAX_REF_LEN || r.chars().any(char::is_control) {
        return Err(invalid(format!("`external_ref` must be 1 to {MAX_REF_LEN} printable characters.")));
    }
    Ok(r.to_string())
}

/// The organisation a path names. A malformed id and an unknown one are the same answer.
fn parse_org(id: &str) -> AppResult<OrgId> {
    OrgId::parse(id.trim()).ok_or_else(org_not_found)
}

fn org_not_found() -> AppError {
    AppError::coded(ErrorCode::OperatorOrgNotFound, "No such organisation.")
}

/// The organisation id a reference makes: the same reference is always the same organisation.
pub fn org_for_ref(external_ref: &str) -> OrgId {
    OrgId::from_label(&format!("operator-ref:{external_ref}"))
}

// ---------------------------------------------------------------- what the API shows

/// Seats in use, in the words the billing service needs.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Seats {
    /// Members whose account is not suspended.
    pub active_members: i64,
    pub suspended_members: i64,
    /// Open invitations that have not expired (the owner's unclaimed invitation included).
    pub pending_invitations: i64,
    /// `active_members + pending_invitations`: what the seat limit is checked against.
    pub in_use: i64,
}

/// One organisation as the operator sees it. Control-database metadata only.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OrgSummary {
    /// The organisation id (32 hex digits).
    pub id: String,
    pub name: String,
    /// `trial`, `team` or `enterprise`; null for an organisation no plan was set on (every organisation on a
    /// self-hosted install, and the server's own).
    pub plan: Option<String>,
    /// `active` or `suspended`. An organisation with no plan state is `active`.
    pub status: String,
    /// Null: no limit.
    pub seat_limit: Option<i64>,
    pub seats: Seats,
    #[schema(value_type = Option<String>)]
    pub trial_ends_at: Option<Datetime>,
    #[schema(value_type = Option<String>)]
    pub suspended_at: Option<Datetime>,
    pub suspended_reason: Option<String>,
    /// The reference the billing service gave when it created the organisation.
    pub external_ref: Option<String>,
    /// The owner's email: the account that claimed the organisation, or the address the claim was sent to.
    pub owner_email: Option<String>,
    /// The owner has claimed the organisation.
    pub owner_claimed: bool,
    /// When the unclaimed owner claim expires (resend it for a new link); null once claimed.
    #[schema(value_type = Option<String>)]
    pub owner_claim_expires_at: Option<Datetime>,
    #[schema(value_type = String)]
    pub created_at: Datetime,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct OrgRow {
    id: RecordId,
    name: String,
    created_at: Datetime,
    #[serde(default)]
    #[surreal(default)]
    plan: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    seat_limit: Option<i64>,
    #[serde(default)]
    #[surreal(default)]
    status: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    suspended_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    suspended_reason: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    trial_ends_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    external_ref: Option<String>,
}

#[derive(Deserialize, SurrealValue)]
struct CountRow {
    org: RecordId,
    n: i64,
}

#[derive(Deserialize, SurrealValue)]
struct OwnerRow {
    org: RecordId,
    #[serde(default)]
    #[surreal(default)]
    email: Option<String>,
}

#[derive(Deserialize, SurrealValue)]
struct ClaimRow {
    org: RecordId,
    email: String,
    expires_at: Datetime,
}

/// Summaries for `rows`, with their seat counts and owners read in one grouped query each.
async fn summarise(state: &AppState, rows: Vec<OrgRow>) -> AppResult<Vec<OrgSummary>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<RecordId> = rows.iter().map(|r| r.id.clone()).collect();
    let counts = |stmt: &'static store::ControlStmt| {
        let ids = ids.clone();
        async move {
            let rows: Vec<CountRow> = stmt.on(&state.control).bind(("orgs", ids)).await?.take(0)?;
            AppResult::Ok(rows.into_iter().map(|r| (r.org.to_string(), r.n)).collect::<std::collections::HashMap<_, _>>())
        }
    };
    let active = counts(&q::ACTIVE_BY_ORG).await?;
    let suspended = counts(&q::SUSPENDED_BY_ORG).await?;
    let pending = counts(&q::PENDING_BY_ORG).await?;
    let owners: Vec<OwnerRow> = q::OWNERS.on(&state.control).bind(("orgs", ids.clone())).await?.take(0)?;
    let claims: Vec<ClaimRow> = q::OWNER_CLAIMS.on(&state.control).bind(("orgs", ids.clone())).await?.take(0)?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let key = r.id.to_string();
            let n = |m: &std::collections::HashMap<String, i64>| m.get(&key).copied().unwrap_or(0);
            let owner = owners.iter().find(|o| o.org.to_string() == key);
            let claim = claims.iter().find(|c| c.org.to_string() == key);
            let (active, pending) = (n(&active), n(&pending));
            OrgSummary {
                id: crate::rid::key_string(&r.id.key).unwrap_or_default(),
                name: r.name,
                plan: r.plan,
                status: if r.status.as_deref() == Some("suspended") { "suspended" } else { "active" }.into(),
                seat_limit: r.seat_limit,
                seats: Seats { active_members: active, suspended_members: n(&suspended), pending_invitations: pending, in_use: active + pending },
                trial_ends_at: r.trial_ends_at,
                suspended_at: r.suspended_at,
                suspended_reason: r.suspended_reason,
                external_ref: r.external_ref,
                owner_email: owner.and_then(|o| o.email.clone()).or_else(|| claim.map(|c| c.email.clone())),
                owner_claimed: owner.is_some(),
                owner_claim_expires_at: if owner.is_some() { None } else { claim.map(|c| c.expires_at) },
                created_at: r.created_at,
            }
        })
        .collect())
}

async fn row_of(state: &AppState, org: &OrgId) -> AppResult<Option<OrgRow>> {
    let mut res = q::GET.on(&state.control).bind(("org", org.record())).await?;
    Ok(res.take::<Vec<OrgRow>>(0)?.into_iter().next())
}

async fn summary_of(state: &AppState, org: &OrgId) -> AppResult<OrgSummary> {
    let row = row_of(state, org).await?.ok_or_else(org_not_found)?;
    Ok(summarise(state, vec![row]).await?.remove(0))
}

// ---------------------------------------------------------------- routes

/// One page of organisations, oldest first.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OrgList {
    pub organisations: Vec<OrgSummary>,
    /// Every organisation on this server.
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

pub async fn list(state: &AppState, limit: Option<i64>, offset: Option<i64>) -> AppResult<OrgList> {
    audited(state, "operator.org_list", "", String::new(), async {
        let limit = limit.unwrap_or(100).clamp(1, 500);
        let offset = offset.unwrap_or(0).max(0);
        let rows: Vec<OrgRow> = q::LIST.on(&state.control).bind(("limit", limit)).bind(("start", offset)).await?.take(0)?;
        let total = org_count(state).await?;
        Ok(OrgList { organisations: summarise(state, rows).await?, total, limit, offset })
    })
    .await
}

pub async fn get(state: &AppState, id: &str) -> AppResult<OrgSummary> {
    let target = format!("org:{}", id.trim());
    audited(state, "operator.org_get", &target, String::new(), async { summary_of(state, &parse_org(id)?).await }).await
}

async fn org_count(state: &AppState) -> AppResult<i64> {
    #[derive(Deserialize, SurrealValue)]
    struct C {
        count: i64,
    }
    let rows: Vec<C> = store::tenant::ORG_COUNT.on(&state.control).await?.take(0)?;
    Ok(rows.first().map_or(0, |c| c.count))
}

/// How many organisations this server holds and how many it may.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Capacity {
    pub organisations: i64,
    /// `EUNOMIA_MAX_ORGS`; null: no limit.
    pub max_organisations: Option<u32>,
    /// Creating another organisation is refused (`operator.at_capacity`).
    pub at_capacity: bool,
}

pub async fn capacity(state: &AppState) -> AppResult<Capacity> {
    audited(state, "operator.capacity", "", String::new(), async {
        let organisations = org_count(state).await?;
        let max = state.settings.hosted.max_orgs;
        Ok(Capacity { organisations, max_organisations: max, at_capacity: max.is_some_and(|m| organisations >= i64::from(m)) })
    })
    .await
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateOrg {
    /// The organisation's name (1 to 80 characters, no `@`); shown to people it invites.
    pub name: String,
    /// The owner's email. They claim the organisation with the link this call returns.
    pub owner_email: String,
    pub plan: PlanName,
    /// How many seats the organisation has (at least 1): active members plus pending invitations.
    pub seat_limit: i64,
    /// When the trial ends, for display. This server never suspends on its own.
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub trial_ends_at: Option<DateTime<Utc>>,
    /// The billing service's own id for this purchase. The same reference is the same organisation: a retried
    /// call returns it and never makes a second one.
    pub external_ref: String,
}

/// The result of creating an organisation.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CreateOrgResult {
    pub organisation_id: String,
    /// `true` when this call made the organisation; `false` when the reference already had one (a retry).
    pub created: bool,
    /// The link the owner opens to claim the organisation and become its owner. Returned once, when this call
    /// made it (a retry after the first reply was lost has none: resend the claim for a new one). Null when
    /// the organisation was already claimed or its claim was already made.
    pub claim_link: Option<String>,
    #[schema(value_type = Option<String>)]
    pub claim_expires_at: Option<Datetime>,
    /// This server sends email (`SMTP_HOST`).
    pub email_enabled: bool,
    /// The link was also emailed to the owner and the mail server accepted it.
    pub emailed: bool,
    pub organisation: OrgSummary,
}

/// A mail address already has an account, whichever organisation it is in.
async fn account_exists(state: &AppState, email: &str) -> AppResult<bool> {
    #[derive(Deserialize, SurrealValue)]
    struct IdRow {
        #[allow(dead_code)]
        id: RecordId,
    }
    let rows: Vec<IdRow> = store::control::AUTH_USER_ID_BY_EMAIL.on(&state.control).bind(("email", email.to_string())).await?.take(0)?;
    Ok(!rows.is_empty())
}

fn owner_not_allowed(email: &str) -> AppResult<()> {
    if crate::authz::operator_email_listed(email) {
        return Err(AppError::coded(
            ErrorCode::OperatorOwnerReserved,
            "That address is listed in EUNOMIA_ADMIN_EMAILS (an operator of this server) and can't be made an organisation's owner this way.",
        ));
    }
    Ok(())
}

fn in_other_org() -> AppError {
    AppError::coded(
        ErrorCode::OperatorOwnerInOtherOrg,
        "That email already has an account, and an account belongs to exactly one organisation. Use a different email for the owner.",
    )
}

/// Makes (or finds) the organisation `external_ref` names, provisions its database and makes the owner claim.
pub async fn create(state: &AppState, req: CreateOrg) -> AppResult<CreateOrgResult> {
    let target = format!("ref:{}", req.external_ref.chars().take(MAX_REF_LEN).collect::<String>());
    let detail = format!("plan={} seats={} owner={}", req.plan.as_str(), req.seat_limit, req.owner_email.trim().to_lowercase());
    audited(state, "operator.org_create", &target, detail, create_inner(state, req)).await
}

async fn create_inner(state: &AppState, req: CreateOrg) -> AppResult<CreateOrgResult> {
    let name = valid_name(&req.name)?;
    let seat_limit = valid_seats(req.seat_limit)?;
    let external_ref = valid_ref(&req.external_ref)?;
    let owner = models_user::normalize_new_email(&req.owner_email)?;
    owner_not_allowed(&owner)?;
    let org = org_for_ref(&external_ref);
    // one create per reference at a time in this process; across processes the derived id and the
    // insert-if-absent keep it to one organisation
    let _one = crate::tx::lock(&format!("operator.org:{}", org.key())).await;

    let existing = row_of(state, &org).await?;
    let created = existing.is_none();
    match &existing {
        Some(_) => {
            // a retry: the same reference for another owner is a mistake, not a retry
            let known = summary_of(state, &org).await?;
            if known.owner_email.as_deref().is_some_and(|e| models_user::normalize_email(e) != owner) {
                return Err(AppError::coded(ErrorCode::OperatorReferenceReused, "That external_ref already belongs to an organisation with a different owner."));
            }
        }
        None => {
            if account_exists(state, &owner).await? {
                return Err(in_other_org());
            }
            let max = state.settings.hosted.max_orgs.map_or(-1, i64::from);
            let made = crate::tx::with_retry_dup(|| async {
                q::CREATE_ORG
                    .on(&state.control)
                    .bind(("org", org.record()))
                    .bind(("name", name.clone()))
                    .bind(("plan", req.plan.as_str().to_string()))
                    .bind(("seat_limit", seat_limit))
                    .bind(("trial_ends_at", req.trial_ends_at.map(Datetime::from)))
                    .bind(("external_ref", external_ref.clone()))
                    .bind(("max", max))
                    .await?
                    .check()
            })
            .await;
            if let Err(e) = made {
                let e: AppError = e.into();
                return Err(if e.code == ErrorCode::OperatorAtCapacity { capacity_error(state).await } else { e });
            }
        }
    }

    // the organisation's database; a ready one is left alone (provisioning it again would reset its name)
    if let Err(e) = state.pool.for_org(&org).await {
        if e.code != ErrorCode::TenantNotFound {
            return Err(e);
        }
        let Some(p) = &state.provisioner else {
            return Err(AppError::coded(ErrorCode::TenantProvisioningDisabled, "This server cannot create organisations."));
        };
        let current = row_of(state, &org).await?.map(|r| r.name).unwrap_or_else(|| name.clone());
        p.provision_org(&state.control, org, &current).await?;
    }

    // the owner claim, unless the organisation already has an owner or an open claim
    let summary = summary_of(state, &org).await?;
    let (mut claim_link, mut claim_expires_at, mut emailed) = (None, None, false);
    if !summary.owner_claimed && summary.owner_claim_expires_at.is_none() {
        let (link, expires_at, sent) = make_claim(state, &org, &summary.name, &owner, ClaimMode::Create).await?;
        (claim_link, claim_expires_at, emailed) = (Some(link), Some(expires_at), sent);
    }
    Ok(CreateOrgResult {
        organisation_id: org.key(),
        created,
        claim_link,
        claim_expires_at,
        email_enabled: state.mailer.enabled(),
        emailed,
        organisation: summary_of(state, &org).await?,
    })
}

async fn capacity_error(state: &AppState) -> AppError {
    let n = org_count(state).await.unwrap_or_default();
    let max = state.settings.hosted.max_orgs.map_or_else(|| "its limit".to_string(), |m| m.to_string());
    AppError::coded(ErrorCode::OperatorAtCapacity, format!("This server holds {n} organisations and is limited to {max} (EUNOMIA_MAX_ORGS)."))
}

enum ClaimMode {
    /// The first claim of a new organisation.
    Create,
    /// A new token for the claim that is open; the old link stops working.
    Resend,
}

#[derive(Deserialize, SurrealValue)]
struct ClaimMade {
    expires_at: Datetime,
}

/// Makes or renews the owner claim of `org` for `owner`, and emails the link when this server sends email.
async fn make_claim(state: &AppState, org: &OrgId, org_name: &str, owner: &str, mode: ClaimMode) -> AppResult<(String, Datetime, bool)> {
    let token = models_user::generate_token();
    let hash = models_user::hash_token(&token);
    let ttl = format!("{TTL_DAYS}d");
    let mut res = match mode {
        ClaimMode::Create => {
            let made = crate::tx::with_retry(|| async {
                q::CREATE_OWNER_CLAIM
                    .on(&state.control)
                    .bind(("org", org.record()))
                    .bind(("email", owner.to_string()))
                    .bind(("hash", hash.clone()))
                    .bind(("ttl", ttl.clone()))
                    .await?
                    .check()
            })
            .await;
            made.map_err(AppError::from)?
        }
        ClaimMode::Resend => q::RESEND_OWNER_CLAIM
            .on(&state.control)
            .bind(("org", org.record()))
            .bind(("hash", hash))
            .bind(("ttl", ttl))
            .await?
            .check()?,
    };
    let made: ClaimMade = res
        .take::<Vec<ClaimMade>>(0)?
        .into_iter()
        .next()
        .ok_or_else(|| AppError::coded(ErrorCode::OperatorOwnerClaimed, "This organisation has no unclaimed owner claim: its owner has already claimed it."))?;
    let link = crate::invitations::link(state, &token);
    let emailed = state.mailer.enabled() && state.mailer.send("owner_claim", crate::recovery::owner_claim_mail(owner, org_name, &link, TTL_DAYS)).await;
    Ok((link, made.expires_at, emailed))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateOrg {
    /// A new name (1 to 80 characters, no `@`).
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub plan: Option<PlanName>,
    /// A new seat limit, or `null` for no limit. Refused (`operator.seats_in_use`) below the seats in use.
    #[serde(default, deserialize_with = "sent")]
    #[schema(value_type = Option<i64>)]
    pub seat_limit: Option<Option<i64>>,
    /// A new trial end, or `null` to clear it.
    #[serde(default, deserialize_with = "sent")]
    #[schema(value_type = Option<String>)]
    pub trial_ends_at: Option<Option<DateTime<Utc>>>,
}

pub async fn update(state: &AppState, id: &str, req: UpdateOrg) -> AppResult<OrgSummary> {
    let target = format!("org:{}", id.trim());
    let detail = format!(
        "name={} plan={} seat_limit={} trial_ends_at={}",
        req.name.is_some(),
        req.plan.map_or("-", PlanName::as_str),
        match req.seat_limit {
            None => "-".to_string(),
            Some(None) => "none".to_string(),
            Some(Some(n)) => n.to_string(),
        },
        match req.trial_ends_at {
            None => "-".to_string(),
            Some(None) => "cleared".to_string(),
            Some(Some(t)) => t.to_rfc3339(),
        }
    );
    audited(state, "operator.org_update", &target, detail, update_inner(state, id, req)).await
}

async fn update_inner(state: &AppState, id: &str, req: UpdateOrg) -> AppResult<OrgSummary> {
    let org = parse_org(id)?;
    let name = req.name.as_deref().map(valid_name).transpose()?;
    let seat_limit = match req.seat_limit {
        Some(Some(n)) => Some(Some(valid_seats(n)?)),
        other => other,
    };
    row_of(state, &org).await?.ok_or_else(org_not_found)?;
    // one seat-taking change at a time in this process; across processes the seat guard decides
    let _one = crate::tx::lock(&format!("seats:{}", org.key())).await;
    let done = crate::tx::with_retry(|| async {
        q::UPDATE
            .on(&state.control)
            .bind(("org", org.record()))
            .bind(("set_name", name.is_some()))
            .bind(("name", name.clone().unwrap_or_default()))
            .bind(("set_plan", req.plan.is_some()))
            .bind(("plan", req.plan.map(|p| p.as_str().to_string())))
            .bind(("set_limit", seat_limit.is_some()))
            .bind(("seat_limit", seat_limit.flatten()))
            .bind(("set_trial", req.trial_ends_at.is_some()))
            .bind(("trial_ends_at", req.trial_ends_at.flatten().map(Datetime::from)))
            .await?
            .check()
    })
    .await;
    state.plans.forget(&org);
    if let Err(e) = done {
        let e: AppError = e.into();
        if e.code == ErrorCode::OperatorSeatsInUse {
            let used = plan::seats_in_use(&state.control, &org.record()).await.unwrap_or_default();
            return Err(AppError::coded(
                ErrorCode::OperatorSeatsInUse,
                format!("The organisation uses {used} seat{} (active members plus pending invitations), more than that. Ask the customer to remove people or withdraw invitations first.", if used == 1 { "" } else { "s" }),
            ));
        }
        return Err(e);
    }
    summary_of(state, &org).await
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SuspendOrg {
    /// Why (for example `payment_failed`); kept on the organisation and on the ledger, never shown to its members.
    pub reason: String,
}

pub async fn suspend(state: &AppState, id: &str, req: SuspendOrg) -> AppResult<OrgSummary> {
    let target = format!("org:{}", id.trim());
    let detail = format!("reason={}", req.reason.chars().take(120).collect::<String>());
    audited(state, "operator.org_suspend", &target, detail, async {
        let org = parse_org(id)?;
        let reason = req.reason.trim();
        if reason.is_empty() || reason.chars().count() > 200 || reason.chars().any(char::is_control) {
            return Err(invalid("`reason` must be 1 to 200 printable characters."));
        }
        row_of(state, &org).await?.ok_or_else(org_not_found)?;
        q::SUSPEND.on(&state.control).bind(("org", org.record())).bind(("reason", reason.to_string())).await?.check()?;
        // this process stops serving the organisation now; the others within `plan::TTL`
        state.plans.forget(&org);
        summary_of(state, &org).await
    })
    .await
}

pub async fn resume(state: &AppState, id: &str) -> AppResult<OrgSummary> {
    let target = format!("org:{}", id.trim());
    audited(state, "operator.org_resume", &target, String::new(), async {
        let org = parse_org(id)?;
        row_of(state, &org).await?.ok_or_else(org_not_found)?;
        q::RESUME.on(&state.control).bind(("org", org.record())).await?.check()?;
        state.plans.forget(&org);
        summary_of(state, &org).await
    })
    .await
}

/// A fresh owner claim link.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ClaimResent {
    pub claim_link: String,
    #[schema(value_type = String)]
    pub claim_expires_at: Datetime,
    pub email_enabled: bool,
    pub emailed: bool,
}

pub async fn resend_claim(state: &AppState, id: &str) -> AppResult<ClaimResent> {
    let target = format!("org:{}", id.trim());
    audited(state, "operator.owner_claim_resend", &target, String::new(), async {
        let org = parse_org(id)?;
        let summary = summary_of(state, &org).await?;
        if summary.owner_claimed {
            return Err(AppError::coded(ErrorCode::OperatorOwnerClaimed, "This organisation's owner has already claimed it."));
        }
        let Some(owner) = summary.owner_email.clone() else {
            return Err(AppError::coded(ErrorCode::OperatorOwnerClaimed, "This organisation has no owner claim to resend."));
        };
        // the address may have got an account elsewhere since: the claim could never be used
        if account_exists(state, &owner).await? {
            return Err(in_other_org());
        }
        let (claim_link, claim_expires_at, emailed) = make_claim(state, &org, &summary.name, &owner, ClaimMode::Resend).await?;
        Ok(ClaimResent { claim_link, claim_expires_at, email_enabled: state.mailer.enabled(), emailed })
    })
    .await
}

/// The plan state the members' endpoint reads, for tests and callers that hold an organisation id.
pub async fn plan_of(state: &AppState, org: &OrgId) -> AppResult<PlanState> {
    plan::read(&state.control, &org.record()).await
}
