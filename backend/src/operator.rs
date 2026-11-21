//! The operator API: what the billing service of a hosted Cupel calls to create an organisation for a
//! buyer, size it, pause it when payment fails and resume it when they pay (`routers/operator.rs`,
//! docs/deployment.md "Running a hosted service").
//!
//! * **One caller, one credential.** The routes answer only to `EUNOMIA_OPERATOR_TOKEN` (or, while it is being
//!   rotated, `EUNOMIA_OPERATOR_TOKEN_NEXT`: `gate::operator_gate`) and exist only while one is set. Nothing here is
//!   reachable with a session, an API token or an OAuth token, and the token opens nothing else.
//! * **The organisation comes from the path**, validated: an id that is not a well-formed organisation, or
//!   names none, is `operator.org_not_found`. No function reads a member's request context.
//! * **Control database only.** Everything here reads and writes the `control` database: organisation
//!   records, membership and invitation counts. Nothing opens an organisation's own database
//!   (`Pool::for_org` is called once, to learn whether provisioning finished, and reads nothing from it), so
//!   a memory, a document or a chat can never come out of this API. A test pins it.
//! * **Every call is on the instance ledger** as actor `operator` / `operator-api`, with its outcome and which of the
//!   two tokens it used (`token=current` or `token=next` at the end of the row's detail). The token is in no row, no
//!   log line and no error.
//! * **Creating is idempotent.** The organisation's id is derived from the external reference, so a retried
//!   webhook finds the same organisation. The records are made first, the database is provisioned next and
//!   the owner claim last, and a call that finds a half-made organisation finishes the missing steps.
//! * **The owner claim** is an organisation invitation of kind `owner` (`invitations.rs`): a single-use,
//!   expiring token whose hash is stored, claimed through the ordinary invitation page. The account that
//!   claims it signs up as for any invitation (password or single sign-on, two-step verification rules
//!   applying) and becomes the organisation's one owner.
//! * **Who can own a new organisation** is decided in one place, [`owner_availability`]: the owner-availability route,
//!   creating and resending a claim all ask it, so the billing service learns before it takes money what creating
//!   would do after.

use std::future::Future;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use crate::audit::{self, Event};
use crate::authz::Actor;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::invitations::TTL_DAYS;
use crate::models_user;
use crate::plan::{self, PlanState, TokenSlot};
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
    /// Set by the gate around an operator call: which of the two operator tokens it presented.
    pub static TOKEN_SLOT: TokenSlot;
}

/// Runs `fut` and writes one instance-ledger row for it, whatever it returns.
async fn audited<T>(state: &AppState, action: &str, target: &str, detail: String, fut: impl Future<Output = AppResult<T>>) -> AppResult<T> {
    let _ = AUDITED.try_with(|a| a.store(true, std::sync::atomic::Ordering::SeqCst));
    let result = fut.await;
    let outcome = match &result {
        Ok(_) => "ok",
        Err(e) => e.code.as_str(),
    };
    // which token made the call, last: `token=current` or `token=next` (never the token)
    let detail = match TOKEN_SLOT.try_with(|t| t.as_str()) {
        Ok(slot) if detail.is_empty() => format!("token={slot}"),
        Ok(slot) => format!("{detail} token={slot}"),
        Err(_) => detail,
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

/// A monthly allowance in US dollars, as micro-dollars. Zero is allowed (no model use on our key at all).
fn valid_allowance(usd: f64) -> AppResult<i64> {
    if !usd.is_finite() || !(0.0..=MAX_ALLOWANCE_USD).contains(&usd) {
        return Err(invalid(format!("`ai_allowance_usd` must be between 0 and {MAX_ALLOWANCE_USD} US dollars.")));
    }
    Ok((usd * 1_000_000.0).round() as i64)
}

const MAX_ALLOWANCE_USD: f64 = 1_000_000.0;

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

/// Model spend of one organisation this month, as the billing service needs it. Never a key.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AiSummary {
    /// The calendar month (UTC) the figures are for, `2026-11`.
    pub month: String,
    /// What the server's key cost this organisation this month, in US dollars, from the price table.
    pub cost_usd: f64,
    /// The monthly allowance on the server's key, in US dollars; null: unlimited.
    pub allowance_usd: Option<f64>,
    /// Share of the allowance used (0 to 100); null when there is no allowance.
    pub used_percent: Option<i64>,
    /// The allowance is used up: model calls on the server's key are refused until it renews or is raised.
    pub allowance_used_up: bool,
    /// An owner or admin has not switched AI off. When false, no model is called for this organisation.
    pub ai_enabled: bool,
    /// The organisation has set a provider key of its own: its model use costs this server nothing.
    pub own_key: bool,
    /// This month's counters by purpose, model and whose key paid; only on a single organisation (`GET`), empty in lists.
    pub usage: Vec<AiUsageLine>,
}

/// One counter: what a purpose and model used this month, on whose key.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AiUsageLine {
    /// `extraction`, `embedding`, `chat`, `reflect`, `consolidation`, `supersede` or `ontology`.
    pub purpose: String,
    pub model: String,
    /// `server` (our key: costed), `org` (the organisation's own key) or `user` (a person's own key): the last two
    /// are counted for visibility and cost nothing.
    pub source: String,
    pub calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// How many of the calls had their tokens counted here instead of reported by the provider.
    pub estimated_calls: i64,
    /// US dollars at the price table of the time of each call; 0 for a key that is not ours.
    pub cost_usd: f64,
}

pub(crate) fn usd(micros: i64) -> f64 {
    (micros as f64) / 1_000_000.0
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
    /// This month's model spend, the allowance, and whether the organisation has its own key or has switched AI off.
    pub ai: AiSummary,
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
struct SpendRow {
    org: RecordId,
    #[serde(default)]
    #[surreal(default)]
    n: Option<i64>,
}

#[derive(Deserialize, SurrealValue)]
struct AiConfigRow {
    org: RecordId,
    #[serde(default)]
    #[surreal(default)]
    enabled: Option<bool>,
    #[serde(default)]
    #[surreal(default)]
    allowance_micros: Option<i64>,
    #[serde(default)]
    #[surreal(default)]
    own_key: Option<bool>,
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
    // this process's own unwritten counters first, so the figures below include them
    let hub = state.pool.ai();
    let _ = hub.flush().await;
    let month = hub.month();
    let spend: Vec<SpendRow> = store::ai::SPEND_MONTH_FOR_ORGS.on(&state.control).bind(("orgs", ids.clone())).bind(("month", month.clone())).await?.take(0)?;
    let configs: Vec<AiConfigRow> = store::ai::CONFIGS_BY_ORG.on(&state.control).bind(("orgs", ids.clone())).await?.take(0)?;
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
            let cost = spend.iter().find(|s| s.org.to_string() == key).and_then(|s| s.n).unwrap_or(0);
            let cfg = configs.iter().find(|c| c.org.to_string() == key);
            let allowance = cfg.and_then(|c| c.allowance_micros);
            let view = crate::ai::orgcfg::usage_view(cost, allowance);
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
                ai: AiSummary {
                    month: month.clone(),
                    cost_usd: usd(cost),
                    allowance_usd: allowance.map(usd),
                    used_percent: view.percent,
                    allowance_used_up: view.state.as_deref() == Some("used_up"),
                    ai_enabled: cfg.and_then(|c| c.enabled).unwrap_or(true),
                    own_key: cfg.and_then(|c| c.own_key).unwrap_or(false),
                    usage: Vec::new(),
                },
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
    let mut summary = summarise(state, vec![row]).await?.remove(0);
    #[derive(Deserialize, SurrealValue)]
    struct Line {
        purpose: String,
        model: String,
        source: String,
        calls: i64,
        input_tokens: i64,
        output_tokens: i64,
        estimated_calls: i64,
        cost_micros: i64,
    }
    let lines: Vec<Line> = store::ai::USAGE_MONTH.on(&state.control).bind(("org", org.record())).bind(("month", summary.ai.month.clone())).await?.take(0)?;
    summary.ai.usage = lines
        .into_iter()
        .map(|l| AiUsageLine {
            purpose: l.purpose,
            model: l.model,
            source: l.source,
            calls: l.calls,
            input_tokens: l.input_tokens,
            output_tokens: l.output_tokens,
            estimated_calls: l.estimated_calls,
            cost_usd: usd(l.cost_micros),
        })
        .collect();
    Ok(summary)
}

// ---------------------------------------------------------------- routes

/// One page of organisations, oldest first.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OrgList {
    pub organisations: Vec<OrgSummary>,
    /// Every organisation that matches the filters (every organisation on this server when there are none).
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

/// What the list can be narrowed to. Both are exact matches; with both, an organisation must match both.
#[derive(Debug, Default)]
pub struct ListFilter {
    /// The reference the billing service gave when it created the organisation.
    pub external_ref: Option<String>,
    /// The owner's email (the account that claimed the organisation, or the address its open claim was sent to),
    /// normalised the way sign-up normalises it.
    pub owner_email: Option<String>,
}

pub async fn list(state: &AppState, limit: Option<i64>, offset: Option<i64>, filter: ListFilter) -> AppResult<OrgList> {
    let detail = format!("external_ref={} owner_email={}", filter.external_ref.is_some(), filter.owner_email.is_some());
    audited(state, "operator.org_list", "", detail, async {
        let limit = limit.unwrap_or(100).clamp(1, 500);
        let offset = offset.unwrap_or(0).max(0);
        // a filter that is not a usable value is refused, not read as "no filter" (which would list everything)
        let external_ref = filter.external_ref.as_deref().map(valid_ref).transpose()?;
        let owner_email = filter.owner_email.as_deref().map(models_user::normalize_new_email).transpose()?;
        let orgs: Vec<RecordId> = match &owner_email {
            Some(email) => owned_by(state, email).await?,
            None => Vec::new(),
        };
        let by_owner = owner_email.is_some();
        let rows: Vec<OrgRow> = filtered(q::LIST.on(&state.control), &external_ref, by_owner, &orgs).bind(("limit", limit)).bind(("start", offset)).await?.take(0)?;
        let total = {
            #[derive(Deserialize, SurrealValue)]
            struct C {
                count: i64,
            }
            let counted: Vec<C> = filtered(q::LIST_COUNT.on(&state.control), &external_ref, by_owner, &orgs).await?.take(0)?;
            counted.first().map_or(0, |c| c.count)
        };
        Ok(OrgList { organisations: summarise(state, rows).await?, total, limit, offset })
    })
    .await
}

/// The filter parameters of `plan.list` and `plan.list_count`.
fn filtered<'a>(q: store::Q<'a>, external_ref: &Option<String>, by_owner: bool, orgs: &[RecordId]) -> store::Q<'a> {
    q.bind(("ref", external_ref.clone())).bind(("by_owner", by_owner)).bind(("orgs", orgs.to_vec()))
}

/// The organisations `email` (normalised) owns or holds an open owner claim for.
async fn owned_by(state: &AppState, email: &str) -> AppResult<Vec<RecordId>> {
    let mut res = q::OWNED_BY_EMAIL.on(&state.control).bind(("email", email.to_string())).await?;
    let mut orgs: Vec<RecordId> = res.take(0)?;
    orgs.extend(res.take::<Vec<RecordId>>(1)?);
    Ok(orgs)
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
    /// Model spend this month across every organisation, so the billing service can alert on it.
    pub ai: AiTotals,
}

/// Model spend on the server's key this month across organisations.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AiTotals {
    /// The calendar month (UTC), `2026-11`.
    pub month: String,
    /// Metered cost of the server's key across every organisation, in US dollars.
    pub cost_usd: f64,
    /// Organisations that have an allowance.
    pub organisations_with_allowance: i64,
    /// Of those, how many have used it up.
    pub organisations_used_up: i64,
}

pub async fn capacity(state: &AppState) -> AppResult<Capacity> {
    audited(state, "operator.capacity", "", String::new(), async {
        let organisations = org_count(state).await?;
        let max = state.settings.hosted.max_orgs;
        let hub = state.pool.ai();
        let _ = hub.flush().await;
        let month = hub.month();
        let spend: Vec<SpendRow> = store::ai::SPEND_MONTH_BY_ORG.on(&state.control).bind(("month", month.clone())).await?.take(0)?;
        #[derive(Deserialize, SurrealValue)]
        struct Allowance {
            org: RecordId,
            allowance_micros: i64,
        }
        let allowances: Vec<Allowance> = store::ai::ALLOWANCES.on(&state.control).await?.take(0)?;
        let used = |org: &RecordId| spend.iter().find(|s| &s.org == org).and_then(|s| s.n).unwrap_or(0);
        let ai = AiTotals {
            month,
            cost_usd: usd(spend.iter().filter_map(|s| s.n).sum()),
            organisations_with_allowance: allowances.len() as i64,
            organisations_used_up: allowances.iter().filter(|a| used(&a.org) >= a.allowance_micros).count() as i64,
        };
        Ok(Capacity { organisations, max_organisations: max, at_capacity: max.is_some_and(|m| organisations >= i64::from(m)), ai })
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
    /// The monthly allowance of model spend on the server's key, in US dollars (calendar month, UTC). Omit or
    /// `null`: unlimited. When it is used up, model calls on the server's key stop until next month unless the
    /// organisation adds a key of its own.
    #[serde(default)]
    pub ai_allowance_usd: Option<f64>,
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

/// Why an address cannot be given a new organisation. One value per reason, the same for the availability route (as
/// `reason`) and for the error creating or resending a claim answers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OwnerBlock {
    /// The address is listed in `EUNOMIA_ADMIN_EMAILS` (an operator of this server), or only differs from a listed one by
    /// a look-alike character. Sign-up refuses it the same way.
    Reserved,
    /// The address already has an account, in whichever organisation. An account belongs to exactly one organisation.
    HasAccount,
    /// Another organisation holds an owner claim for the address that nobody has accepted or withdrawn (expired or not:
    /// a resend revives it). The first claim to be accepted makes the account, so a second could never be accepted.
    PendingClaim,
}

impl OwnerBlock {
    pub fn as_str(self) -> &'static str {
        match self {
            OwnerBlock::Reserved => "reserved",
            OwnerBlock::HasAccount => "has_account",
            OwnerBlock::PendingClaim => "pending_claim",
        }
    }

    pub fn into_error(self) -> AppError {
        match self {
            OwnerBlock::Reserved => AppError::coded(
                ErrorCode::OperatorOwnerReserved,
                "That address is listed in EUNOMIA_ADMIN_EMAILS (an operator of this server) and can't be made an organisation's owner this way.",
            ),
            OwnerBlock::HasAccount => AppError::coded(
                ErrorCode::OperatorOwnerInOtherOrg,
                "That email already has an account, and an account belongs to exactly one organisation. Use a different email for the owner.",
            ),
            OwnerBlock::PendingClaim => AppError::coded(
                ErrorCode::OperatorOwnerClaimPending,
                "Another organisation already has an unclaimed owner claim for that email. Only one of them could ever be accepted: use a different email for the owner, or finish that claim first.",
            ),
        }
    }
}

/// The first half of [`owner_availability`]: what `raw` is as an address. `Ok(Err(block))`: reserved, judged on the
/// address as typed, so a look-alike of a listed address (a character that lowercases into another letter) is
/// reserved too, exactly as sign-up judges it (`routers/auth.rs`). `Ok(Ok(email))`: the form a new address is stored and
/// compared in (`models_user::normalize_new_email`). An address that is not one, or has a look-alike character that
/// does not make it a listed one, is a `400` as for any sign-up.
pub fn owner_address(raw: &str) -> AppResult<Result<String, OwnerBlock>> {
    if crate::authz::operator_email_listed(raw) {
        return Ok(Err(OwnerBlock::Reserved));
    }
    Ok(Ok(models_user::normalize_new_email(raw)?))
}

/// The second half: what the control database says about a normalised address. An account anywhere, or an open owner
/// claim in an organisation other than `except` (the one the claim is being made or resent for).
pub async fn owner_standing(state: &AppState, email: &str, except: Option<&OrgId>) -> AppResult<Option<OwnerBlock>> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        #[allow(dead_code)]
        #[serde(default)]
        #[surreal(default)]
        id: Option<RecordId>,
    }
    let rows: Vec<Row> = store::control::AUTH_USER_ID_BY_EMAIL.on(&state.control).bind(("email", email.to_string())).await?.take(0)?;
    if !rows.is_empty() {
        return Ok(Some(OwnerBlock::HasAccount));
    }
    #[derive(Deserialize, SurrealValue)]
    struct Claim {
        #[allow(dead_code)]
        org: RecordId,
    }
    let claims: Vec<Claim> = q::OPEN_OWNER_CLAIM_FOR.on(&state.control).bind(("email", email.to_string())).bind(("org", except.map(OrgId::record))).await?.take(0)?;
    Ok(claims.first().map(|_| OwnerBlock::PendingClaim))
}

/// Whether `raw` can be the owner of a new organisation: `Ok(Ok(normalised address))`, or why not. The one rule behind
/// the owner-availability route, creating an organisation, and resending a claim.
pub async fn owner_availability(state: &AppState, raw: &str, except: Option<&OrgId>) -> AppResult<Result<String, OwnerBlock>> {
    let email = match owner_address(raw)? {
        Ok(e) => e,
        Err(block) => return Ok(Err(block)),
    };
    Ok(match owner_standing(state, &email, except).await? {
        Some(block) => Err(block),
        None => Ok(email),
    })
}

/// The answer of the owner-availability route.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OwnerAvailability {
    /// Creating an organisation for this owner email would be accepted now.
    pub available: bool,
    /// Why not: `has_account`, `reserved` or `pending_claim`. Absent when `available`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<OwnerBlock>,
}

/// Asks, before the buyer's card is charged, whether `email` can own a new organisation. The ledger row has neither
/// the address nor the answer: the addresses asked about are mostly people who never buy, and the instance ledger is
/// no place to keep a list of who has an account.
pub async fn owner_availability_for(state: &AppState, email: &str) -> AppResult<OwnerAvailability> {
    audited(state, "operator.owner_availability", "", String::new(), async {
        Ok(match owner_availability(state, email, None).await? {
            Ok(_) => OwnerAvailability { available: true, reason: None },
            Err(block) => OwnerAvailability { available: false, reason: Some(block) },
        })
    })
    .await
}

/// Makes (or finds) the organisation `external_ref` names, provisions its database and makes the owner claim.
pub async fn create(state: &AppState, req: CreateOrg) -> AppResult<CreateOrgResult> {
    let target = format!("ref:{}", req.external_ref.chars().take(MAX_REF_LEN).collect::<String>());
    let detail = format!("plan={} seats={} owner={} ai_allowance_usd={}", req.plan.as_str(), req.seat_limit, req.owner_email.trim().to_lowercase(), req.ai_allowance_usd.map_or("-".to_string(), |a| a.to_string()));
    audited(state, "operator.org_create", &target, detail, create_inner(state, req)).await
}

async fn create_inner(state: &AppState, req: CreateOrg) -> AppResult<CreateOrgResult> {
    let name = valid_name(&req.name)?;
    let seat_limit = valid_seats(req.seat_limit)?;
    let external_ref = valid_ref(&req.external_ref)?;
    // the address first, as the availability route reads it (reserved, look-alikes, form); an account or an open claim
    // elsewhere is asked below, for a new organisation only
    let owner = owner_address(&req.owner_email)?.map_err(OwnerBlock::into_error)?;
    let allowance = req.ai_allowance_usd.map(valid_allowance).transpose()?;
    let org = org_for_ref(&external_ref);
    // deciding and inserting the record: one create per reference at a time in this process (not while the database
    // is provisioned or a mail is sent); across processes the derived id and the insert-if-absent keep it to one organisation
    let created = {
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
            if let Some(block) = owner_standing(state, &owner, None).await? {
                return Err(block.into_error());
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
    created
    };

    // the allowance: with the record on a first call, and on a retry only where the first call never got that far
    if let Some(micros) = allowance
        && (created || state.pool.ai().org_config(&org).await.map(|c| c.allowance_micros.is_none()).unwrap_or(true))
    {
        crate::ai::orgcfg::set_allowance(state, &org, Some(micros)).await?;
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
        match make_claim(state, &org, &summary.name, &owner, ClaimMode::Create).await {
            Ok((link, expires_at, sent)) => (claim_link, claim_expires_at, emailed) = (Some(link), Some(expires_at), sent),
            // a twin of this call (a retry that overlapped it) made the claim first: that call holds the link
            Err(e) if e.code == ErrorCode::DbDuplicate => {}
            Err(e) => return Err(e),
        }
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
            // one claim for an address at a time in this process, asked again under the lock: two organisations made
            // together for one address (different references) cannot both get a claim. Across processes the early check
            // in `create_inner` is the guard, and a race there is possible (see docs/deployment.md).
            let _one = crate::tx::lock(&format!("operator.owner:{owner}")).await;
            if let Some(block) = owner_standing(state, owner, Some(org)).await? {
                return Err(block.into_error());
            }
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
    /// A new monthly allowance of model spend on the server's key in US dollars, or `null` for unlimited. Raising
    /// it lets model work that was waiting for lack of allowance continue within seconds.
    #[serde(default, deserialize_with = "sent")]
    #[schema(value_type = Option<f64>)]
    pub ai_allowance_usd: Option<Option<f64>>,
}

pub async fn update(state: &AppState, id: &str, req: UpdateOrg) -> AppResult<OrgSummary> {
    let target = format!("org:{}", id.trim());
    let detail = format!(
        "name={} plan={} seat_limit={} trial_ends_at={} ai_allowance_usd={}",
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
        },
        match req.ai_allowance_usd {
            None => "-".to_string(),
            Some(None) => "none".to_string(),
            Some(Some(a)) => a.to_string(),
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
    let allowance = req.ai_allowance_usd.map(|a| a.map(valid_allowance).transpose()).transpose()?;
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
    if let Some(a) = allowance {
        crate::ai::orgcfg::set_allowance(state, &org, a).await?;
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
        // the address may have got an account or another organisation's claim since, or been listed as an operator's:
        // the claim could never be used. The same rule as for a new organisation, apart from this organisation's own claim.
        if let Err(block) = owner_availability(state, &owner, Some(&org)).await? {
            return Err(block.into_error());
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

// ---------------------------------------------------------------- the price table

/// One model's price and where it comes from.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PriceRow {
    pub input_per_million_usd: f64,
    pub output_per_million_usd: f64,
    /// `default` (built into the server) or `operator` (set with `PUT /api/operator/v1/ai/prices`).
    pub source: String,
}

/// The price table in force.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PriceTableOut {
    /// By model name.
    pub prices: std::collections::BTreeMap<String, PriceRow>,
    /// What a model that is in neither the operator's table nor the defaults is counted at. Deliberately high, so an
    /// unknown model can never be free; price it to bring it down.
    pub fallback: crate::ai::prices::Price,
}

pub async fn get_prices(state: &AppState) -> AppResult<PriceTableOut> {
    audited(state, "operator.ai_prices_get", "", String::new(), async { prices_out(state).await }).await
}

async fn prices_out(state: &AppState) -> AppResult<PriceTableOut> {
    let table = state.pool.ai().price_table().await;
    let overrides = table.overrides().clone();
    let prices = table
        .effective()
        .into_iter()
        .map(|(model, p)| {
            let source = if overrides.contains_key(&model) { "operator" } else { "default" };
            (model, PriceRow { input_per_million_usd: p.input_per_million_usd, output_per_million_usd: p.output_per_million_usd, source: source.to_string() })
        })
        .collect();
    Ok(PriceTableOut { prices, fallback: crate::ai::prices::FALLBACK })
}

/// Prices to set. A model with `null` goes back to its default (or the fallback).
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PricesUpdate {
    /// Model name to its price, or `null` to remove the operator's price for it.
    #[schema(value_type = Object)]
    pub prices: std::collections::BTreeMap<String, Option<crate::ai::prices::Price>>,
}

const MAX_PRICE_ROWS: usize = 200;

pub async fn set_prices(state: &AppState, req: PricesUpdate) -> AppResult<PriceTableOut> {
    let detail = format!("models={}", req.prices.keys().take(20).cloned().collect::<Vec<_>>().join(","));
    audited(state, "operator.ai_prices_set", "", detail, async {
        if req.prices.is_empty() || req.prices.len() > MAX_PRICE_ROWS {
            return Err(invalid(format!("send between 1 and {MAX_PRICE_ROWS} models.")));
        }
        for (model, price) in &req.prices {
            let m = model.trim();
            if m.is_empty() || m.chars().count() > 100 || m.chars().any(char::is_control) || m != model {
                return Err(invalid("a model name is 1 to 100 printable characters, with no spaces at the ends."));
            }
            if let Some(p) = price
                && !p.valid()
            {
                return Err(invalid(format!("the price of `{m}` must be between 0 and 10000 US dollars per million tokens, for input and for output.")));
            }
        }
        for (model, price) in &req.prices {
            let id = RecordId::new("ai_price", crate::tx::stable_key('p', model));
            match price {
                Some(p) => {
                    crate::tx::with_retry(|| async {
                        store::ai::PRICE_SET
                            .on(&state.control)
                            .bind(("id", id.clone()))
                            .bind(("model", model.clone()))
                            .bind(("input", p.input_per_million_usd))
                            .bind(("output", p.output_per_million_usd))
                            .await?
                            .check()
                    })
                    .await?;
                }
                None => {
                    store::ai::PRICE_DELETE.on(&state.control).bind(("model", model.clone())).await?.check()?;
                }
            }
        }
        state.pool.ai().forget_prices();
        prices_out(state).await
    })
    .await
}
