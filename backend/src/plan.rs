//! Hosted plans: what an organisation's plan, seats and suspension mean for its members
//! (docs/deployment.md, "Running a hosted service"). The operator API (`operator.rs`) sets them; this
//! module reads them and enforces them.
//!
//! * **Unmanaged is the default.** An organisation with none of the plan fields set (control migration
//!   0015) is unmanaged and unlimited: every organisation on a self-hosted install. Nothing here changes
//!   what it does.
//! * **Seats.** Seats in use are active members plus pending (open, unexpired) invitations; suspended
//!   members do not count. The statements that take a seat check the limit in the transaction that takes
//!   it (`store/plan.rs`), and answer `org.seat_limit`.
//! * **Suspension is enforced at one point**: the gate (`gate.rs`) asks [`PlanCache::get`] for the caller's
//!   organisation on every authenticated request and refuses everything that is not on
//!   [`allowed_while_suspended`], so a new route cannot forget it. The answer is cached for [`TTL`]
//!   (3 seconds) per process, and dropped at once in the process that suspends or resumes, so the
//!   operator API takes effect on every replica within a few seconds. Background work is paused by the job
//!   claim (`store/jobs.rs`), the webhook entry points ask [`user_org_suspended`], and an open change
//!   stream rechecks.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use axum::http::Method;
use serde::{Deserialize, Serialize};
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::User;
use crate::pool::{ControlDb, OrgId};
use crate::state::AppState;
use crate::store::plan as q;

/// How long a process trusts an organisation's plan state before asking the database again.
pub const TTL: Duration = Duration::from_secs(3);

/// The shortest operator token accepted at boot.
pub const MIN_OPERATOR_TOKEN_LEN: usize = 32;

/// `EUNOMIA_OPERATOR_TOKEN`, `EUNOMIA_MAX_ORGS` and `EUNOMIA_BILLING_URL`. All unset is a self-hosted install.
#[derive(Clone, Default)]
pub struct HostedConfig {
    /// The bearer token of the operator API. `None`: the routes do not exist.
    pub operator_token: Option<String>,
    /// The most organisations this server holds. `None`: no limit.
    pub max_orgs: Option<u32>,
    /// Where an organisation's owner goes to add seats or pay (shown in the plan status).
    pub billing_url: Option<String>,
}

impl std::fmt::Debug for HostedConfig {
    // never the token
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostedConfig")
            .field("operator_token", &self.operator_token.as_ref().map(|_| "<set>"))
            .field("max_orgs", &self.max_orgs)
            .field("billing_url", &self.billing_url)
            .finish()
    }
}

impl HostedConfig {
    /// Reads the three variables; a value that cannot work is an error so boot can refuse it.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let var = |k: &str| get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let operator_token = var("EUNOMIA_OPERATOR_TOKEN");
        if let Some(t) = &operator_token
            && t.chars().count() < MIN_OPERATOR_TOKEN_LEN
        {
            return Err(format!("EUNOMIA_OPERATOR_TOKEN is too short: use at least {MIN_OPERATOR_TOKEN_LEN} characters (for example `openssl rand -base64 48`)"));
        }
        let max_orgs = match var("EUNOMIA_MAX_ORGS") {
            None => None,
            Some(v) => Some(v.parse::<u32>().map_err(|_| format!("EUNOMIA_MAX_ORGS must be a whole number, got {v:?}"))?),
        };
        let billing_url = match var("EUNOMIA_BILLING_URL") {
            None => None,
            Some(v) if v.starts_with("https://") || v.starts_with("http://") => Some(v),
            Some(v) => return Err(format!("EUNOMIA_BILLING_URL must be an http(s) address, got {v:?}")),
        };
        Ok(HostedConfig { operator_token, max_orgs, billing_url })
    }
}

/// What a hosted configuration should say at boot. A hosted server (an operator token is set) should not let the
/// public sign up: organisations come from the operator API and from invitations.
pub fn boot_warnings(signup_mode: &str, hosted: &HostedConfig) -> Vec<String> {
    let open = matches!(signup_mode.trim().to_ascii_lowercase().as_str(), "" | "open");
    let mut out = Vec::new();
    if open && hosted.operator_token.is_some() {
        out.push(
            "EUNOMIA_OPERATOR_TOKEN is set (this is a hosted service) while SIGNUP=open: anyone who can reach this server can sign up. \
             Set SIGNUP=invite (and EUNOMIA_SIGNUP_ORG=personal) so organisations come only from the operator API and invitations; see docs/deployment.md, \"Running a hosted service\"."
                .to_string(),
        );
    }
    out
}

/// Whether `given` is the configured operator token, in time that does not depend on how much of it matches
/// (both are hashed first, so the length does not leak either).
pub fn operator_token_matches(configured: &str, given: &str) -> bool {
    use sha2::{Digest, Sha256};
    let (a, b) = (Sha256::digest(configured.as_bytes()), Sha256::digest(given.as_bytes()));
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The plan fields of an organisation as stored.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, SurrealValue)]
pub struct PlanState {
    #[serde(default)]
    #[surreal(default)]
    pub plan: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    pub seat_limit: Option<i64>,
    #[serde(default)]
    #[surreal(default)]
    pub status: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    pub suspended_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    pub suspended_reason: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    pub trial_ends_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    pub external_ref: Option<String>,
}

impl PlanState {
    pub fn suspended(&self) -> bool {
        self.status.as_deref() == Some("suspended")
    }

    /// Some plan field is set: the billing service manages this organisation.
    pub fn managed(&self) -> bool {
        self.plan.is_some() || self.seat_limit.is_some() || self.status.is_some() || self.trial_ends_at.is_some() || self.external_ref.is_some()
    }
}

/// An organisation's plan state read from the control database (unmanaged defaults when it has no record).
pub async fn read(control: &ControlDb, org: &RecordId) -> AppResult<PlanState> {
    let mut res = q::GET.on(control).bind(("org", org.clone())).await?;
    Ok(res.take::<Vec<PlanState>>(0)?.into_iter().next().unwrap_or_default())
}

/// Plan state by organisation, held for [`TTL`]. One per process (`AppState::plans`).
#[derive(Default)]
pub struct PlanCache {
    map: RwLock<HashMap<OrgId, (Instant, Arc<PlanState>)>>,
}

/// Most organisations kept; past it the aged-out ones go.
const CACHE_CAP: usize = 10_000;

impl PlanCache {
    pub async fn get(&self, control: &ControlDb, org: &OrgId) -> AppResult<Arc<PlanState>> {
        if let Some((at, s)) = self.map.read().unwrap_or_else(|e| e.into_inner()).get(org)
            && at.elapsed() < TTL
        {
            return Ok(s.clone());
        }
        let fresh = Arc::new(read(control, &org.record()).await?);
        let mut map = self.map.write().unwrap_or_else(|e| e.into_inner());
        if map.len() >= CACHE_CAP {
            map.retain(|_, (at, _)| at.elapsed() < TTL);
            if map.len() >= CACHE_CAP {
                map.clear();
            }
        }
        map.insert(*org, (Instant::now(), fresh.clone()));
        Ok(fresh)
    }

    /// Drops what this process knows of `org`: the next request asks the database (a suspension or a
    /// resume done through this process takes effect at once; another replica catches up within [`TTL`]).
    pub fn forget(&self, org: &OrgId) {
        self.map.write().unwrap_or_else(|e| e.into_inner()).remove(org);
    }
}

/// The refusal for a suspended organisation, wherever it is raised.
pub fn suspended_error() -> AppError {
    AppError::coded(
        ErrorCode::OrgSuspended,
        "This workspace is suspended, so its content can't be read or changed right now. Its owner can see why and how to restore it. You can still sign in, change your password, and export or erase your own data.",
    )
}

/// What a member of a suspended organisation may still call (everything else is `org.suspended`): reading
/// who they are and the plan status, changing their own password, their own data export and erasure, and
/// the People list (the handler lets only an owner or admin read it). Signing in and out do not reach the
/// check (no session yet / public). Matched on the method and path the gate classified.
pub fn allowed_while_suspended(method: &Method, path: &str) -> bool {
    let read = method == Method::GET || method == Method::HEAD;
    let segs: Vec<&str> = path.trim_matches('/').split('/').collect();
    match segs.as_slice() {
        ["api", "auth", "me"] => read,
        ["api", "org", "plan"] => read,
        ["api", "account", "password"] => method == Method::POST,
        ["api", "account", "erasure"] => method == Method::POST,
        ["api", "account", "activity"] => read,
        ["api", "export", "subject"] => read || method == Method::POST,
        ["api", "export", "subject", _] => read,
        ["api", "admin", "people"] => read,
        _ => false,
    }
}

/// The gate's check for an authenticated request: `Ok` unless the caller's organisation is suspended and
/// the route is not one a suspended organisation may still use.
pub async fn enforce(state: &AppState, org: &OrgId, method: &Method, path: &str) -> AppResult<()> {
    if allowed_while_suspended(method, path) {
        return Ok(());
    }
    if state.plans.get(&state.control, org).await?.suspended() {
        return Err(suspended_error());
    }
    Ok(())
}

/// Whether the organisation `user` belongs to is suspended: for the entry points that carry no session
/// (source and extension webhooks, peer links), which have already found the account they act for.
/// Fails closed: an account with no organisation, or a database that does not answer, is "suspended".
pub async fn user_org_suspended(control: &ControlDb, plans: &PlanCache, user: &RecordId) -> bool {
    let Ok(org) = crate::models_user::org_of(control, user).await else { return true };
    plans.get(control, &org).await.map(|p| p.suspended()).unwrap_or(true)
}

/// The refusal for a full organisation, with its number when it has one.
pub fn seat_limit_error(limit: Option<i64>) -> AppError {
    let seats = match limit {
        Some(n) => format!("{n} seat{}", if n == 1 { "" } else { "s" }),
        None => "a limited number of seats".to_string(),
    };
    AppError::coded(
        ErrorCode::OrgSeatLimit,
        format!("This workspace has {seats} and every one is in use (members plus open invitations). Ask the workspace owner to add seats, or remove someone or withdraw an invitation first."),
    )
}

/// `err` with the seat-limit refusal given its number; any other error is returned as it is.
pub async fn explain_seat_refusal(control: &ControlDb, org: &RecordId, err: AppError) -> AppError {
    if err.code != ErrorCode::OrgSeatLimit {
        return err;
    }
    seat_limit_error(read(control, org).await.ok().and_then(|p| p.seat_limit))
}

/// What the plan means for the people in an organisation.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PlanStatus {
    /// `trial`, `team` or `enterprise`; null for an unmanaged organisation.
    pub plan: Option<String>,
    /// `active` or `suspended`; null for an unmanaged organisation.
    pub status: Option<String>,
    /// How many seats the organisation has; null when it has no limit.
    pub seat_limit: Option<i64>,
    /// Active members plus pending invitations; null for an unmanaged organisation.
    pub seats_in_use: Option<i64>,
    /// When the trial ends, for display: this server never suspends on its own. Null when not set.
    #[schema(value_type = Option<String>)]
    pub trial_ends_at: Option<Datetime>,
    /// Where the owner goes to add seats or pay (`EUNOMIA_BILLING_URL`); null when not configured or unmanaged.
    pub manage_url: Option<String>,
}

/// Seats in use by `org` (active members plus pending invitations).
pub async fn seats_in_use(control: &ControlDb, org: &RecordId) -> AppResult<i64> {
    let mut res = q::SEATS_USED.on(control).bind(("org", org.clone())).await?;
    Ok(res.take::<Option<i64>>(0)?.unwrap_or(0))
}

/// The plan status of the caller's own organisation. Any member may read it.
pub async fn status_for(state: &AppState, user: &User) -> AppResult<PlanStatus> {
    let p = read(&state.control, &user.org.record()).await?;
    if !p.managed() {
        return Ok(PlanStatus { plan: None, status: None, seat_limit: None, seats_in_use: None, trial_ends_at: None, manage_url: None });
    }
    Ok(PlanStatus {
        status: Some(if p.suspended() { "suspended" } else { "active" }.to_string()),
        seats_in_use: Some(seats_in_use(&state.control, &user.org.record()).await?),
        manage_url: state.settings.hosted.billing_url.clone(),
        plan: p.plan,
        seat_limit: p.seat_limit,
        trial_ends_at: p.trial_ends_at,
    })
}

/// Restores a suspended member, taking a seat back: refused with `org.seat_limit` when the organisation is full.
pub async fn restore_member(state: &AppState, org: &OrgId, user: &RecordId) -> AppResult<()> {
    // one at a time in this process; across processes the seat guard row decides
    let _one = crate::tx::lock(&format!("seats:{}", org.key())).await;
    let done = crate::tx::with_retry(|| async {
        q::RESTORE_MEMBER.on(&state.control).bind(("org", org.record())).bind(("id", user.clone())).await?.check()
    })
    .await;
    match done {
        Ok(_) => Ok(()),
        Err(e) => Err(explain_seat_refusal(&state.control, &org.record(), e.into()).await),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn the_hosted_settings_are_all_optional() {
        let c = HostedConfig::from_vars(vars(&[])).unwrap();
        assert!(c.operator_token.is_none() && c.max_orgs.is_none() && c.billing_url.is_none());
        // set but blank is unset
        let c = HostedConfig::from_vars(vars(&[("EUNOMIA_OPERATOR_TOKEN", "  "), ("EUNOMIA_MAX_ORGS", ""), ("EUNOMIA_BILLING_URL", " ")])).unwrap();
        assert!(c.operator_token.is_none() && c.max_orgs.is_none() && c.billing_url.is_none());
    }

    #[test]
    fn a_short_operator_token_is_refused_at_boot() {
        let e = HostedConfig::from_vars(vars(&[("EUNOMIA_OPERATOR_TOKEN", "short-token-31-characters-long!!")])).unwrap_err();
        assert!(e.contains("too short"), "{e}");
        assert!(!e.contains("short-token"), "the error never repeats the token: {e}");
        let ok = HostedConfig::from_vars(vars(&[("EUNOMIA_OPERATOR_TOKEN", "an-operator-token-of-32-characters")])).unwrap();
        assert!(ok.operator_token.is_some());
        assert!(!format!("{ok:?}").contains("an-operator-token"), "Debug never prints the token");
    }

    #[test]
    fn the_other_settings_are_checked() {
        assert!(HostedConfig::from_vars(vars(&[("EUNOMIA_MAX_ORGS", "many")])).is_err());
        assert_eq!(HostedConfig::from_vars(vars(&[("EUNOMIA_MAX_ORGS", "25")])).unwrap().max_orgs, Some(25));
        assert!(HostedConfig::from_vars(vars(&[("EUNOMIA_BILLING_URL", "billing.example.com")])).is_err());
        assert!(HostedConfig::from_vars(vars(&[("EUNOMIA_BILLING_URL", "https://billing.example.com/manage")])).is_ok());
    }

    #[test]
    fn a_hosted_server_with_open_signup_is_warned_about() {
        let hosted = HostedConfig { operator_token: Some("an-operator-token-of-32-characters".into()), ..Default::default() };
        for open in ["", "open", " OPEN "] {
            assert_eq!(boot_warnings(open, &hosted).len(), 1, "{open:?}");
        }
        assert!(boot_warnings("open", &hosted)[0].contains("SIGNUP=open"));
        assert!(!boot_warnings("open", &hosted)[0].contains("an-operator-token"), "never the token");
        for closed in ["invite", "closed"] {
            assert!(boot_warnings(closed, &hosted).is_empty(), "{closed}");
        }
        // a self-hosted install (no token) is not warned about
        assert!(boot_warnings("open", &HostedConfig::default()).is_empty());
    }

    #[test]
    fn the_token_comparison_is_exact() {
        let t = "an-operator-token-of-32-characters";
        assert!(operator_token_matches(t, t));
        assert!(!operator_token_matches(t, "an-operator-token-of-32-characterS"));
        assert!(!operator_token_matches(t, &t[..t.len() - 1]));
        assert!(!operator_token_matches(t, ""));
    }

    #[test]
    fn a_suspended_organisation_keeps_only_its_account_routes() {
        let (get, post, put, del) = (Method::GET, Method::POST, Method::PUT, Method::DELETE);
        for (m, p) in [
            (&get, "/api/auth/me"),
            (&get, "/api/org/plan"),
            (&post, "/api/account/password"),
            (&post, "/api/account/erasure"),
            (&get, "/api/export/subject"),
            (&post, "/api/export/subject"),
            (&get, "/api/export/subject/subject_export:x"),
            (&get, "/api/admin/people"),
        ] {
            assert!(allowed_while_suspended(m, p), "{m} {p}");
        }
        for (m, p) in [
            (&post, "/api/tools/recall"),
            (&post, "/mcp"),
            (&get, "/api/entities/graph"),
            (&get, "/api/vaults"),
            (&post, "/api/documents"),
            (&post, "/api/chat/threads"),
            (&get, "/api/org/seats"),
            (&post, "/api/org/invitations"),
            (&put, "/api/admin/people/user:x/suspended"),
            (&del, "/api/admin/people"),
            (&get, "/api/connectors"),
            (&post, "/api/auth/tokens"),
        ] {
            assert!(!allowed_while_suspended(m, p), "{m} {p}");
        }
    }

    #[test]
    fn seat_messages_say_how_many_seats_there_are() {
        let e = seat_limit_error(Some(5));
        assert_eq!((e.code, e.status.as_u16()), (ErrorCode::OrgSeatLimit, 409));
        assert!(e.message.contains("5 seats"), "{}", e.message);
        assert!(seat_limit_error(Some(1)).message.contains("1 seat "), "singular");
    }
}
