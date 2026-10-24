//! Audit REST surface: the owner's own mutating-tool-call history (the `audit_log` table) and
//! tool-call usage counts (the control `tool_usage` table, see `crate::usage`).

use std::collections::{BTreeMap, HashMap};

use axum::{extract::{Query, State}, routing::get, Json, Router};
use chrono::{DurationRound, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;
use surrealdb::types::{Datetime, RecordId};
use crate::rid::RecordIdExt;

use crate::authz;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::store;
use crate::models_user::User;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/audit", get(get_audit)).route("/audit/tool-usage", get(get_tool_usage))
}

fn default_limit() -> i64 {
    50
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct AuditQuery {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct AuditRow {
    id: RecordId,
    tool_name: String,
    #[serde(default)]
    #[surreal(default)]
    args_summary: String,
    outcome: String,
    created_at: Option<Datetime>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct CountRow {
    count: i64,
}

#[derive(Serialize, utoipa::ToSchema)]
struct AuditEntry {
    id: String,
    tool_name: String,
    args_summary: String,
    outcome: String,
    #[schema(value_type = Option<String>)]
    created_at: Option<Datetime>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct AuditPage {
    results: Vec<AuditEntry>,
    total: i64,
    has_more: bool,
}

/// Pure pagination check, factored out for testing: are there more rows
/// beyond what this page already returned?
fn has_more(offset: i64, returned: usize, total: i64) -> bool {
    offset + (returned as i64) < total
}

#[utoipa::path(
    operation_id = "listAudit",
    get,
    path = "/api/audit",
    tag = "audit",
    summary = "Page of the caller's tool-call audit log",
    params(AuditQuery),
    responses((status = 200, body = AuditPage), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_audit(
    State(state): State<AppState>,
    user: User,
    Query(q): Query<AuditQuery>,
) -> AppResult<Json<AuditPage>> {
    let state = state.org(&user.org).await?;
    let mut res = store::app::AUDIT_LIST
        .on(&state.db)
        .bind(("owner", user.id.clone()))
        .bind(("limit", q.limit))
        .bind(("offset", q.offset))
        .await?;
    let rows: Vec<AuditRow> = res.take(0)?;

    let mut count_res = store::app::AUDIT_COUNT
        .on(&state.db)
        .bind(("owner", user.id.clone()))
        .await?;
    let counts: Vec<CountRow> = count_res.take(0)?;
    let total = counts.first().map(|c| c.count).unwrap_or(0);

    let results: Vec<AuditEntry> = rows
        .into_iter()
        .map(|r| AuditEntry {
            id: r.id.to_string(),
            tool_name: r.tool_name,
            args_summary: r.args_summary,
            outcome: r.outcome,
            created_at: r.created_at,
        })
        .collect();

    let has_more = has_more(q.offset, results.len(), total);
    Ok(Json(AuditPage { results, total, has_more }))
}

// ---------------------------------------------------------------- tool-call usage

const DEFAULT_HOURS: i64 = 168;
const MAX_HOURS: i64 = 24 * 90;
/// Rows kept per table of the report (the busiest first).
const TOP: usize = 100;

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct UsageQuery {
    /// Look back this many hours (1 to 2160, default 168 = 7 days).
    hours: Option<i64>,
    /// Admin only: restrict the report to one person (a `user:...` id).
    user: Option<String>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct UsageAgg {
    #[serde(default)]
    #[surreal(default)]
    user: Option<RecordId>,
    #[serde(default)]
    #[surreal(default)]
    actor_kind: String,
    #[serde(default)]
    #[surreal(default)]
    actor_id: String,
    #[serde(default)]
    #[surreal(default)]
    channel: String,
    #[serde(default)]
    #[surreal(default)]
    session: String,
    #[serde(default)]
    #[surreal(default)]
    tool: String,
    #[serde(default)]
    #[surreal(default)]
    bucket: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    calls: i64,
    #[serde(default)]
    #[surreal(default)]
    errors: i64,
    #[serde(default)]
    #[surreal(default)]
    total_ms: i64,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct NameRow {
    #[serde(default)]
    #[surreal(default)]
    id: Option<RecordId>,
    #[serde(default)]
    #[surreal(default)]
    client_id: String,
    #[serde(default)]
    #[surreal(default)]
    email: String,
    #[serde(default)]
    #[surreal(default)]
    name: String,
}

/// Calls, failed calls and mean duration of a group of tool calls.
#[derive(Serialize, utoipa::ToSchema)]
struct UsageCounts {
    calls: i64,
    /// Calls that ended in an error (a tool error value, a refused scope, or a failure).
    errors: i64,
    /// Mean time a call took, in milliseconds.
    avg_ms: f64,
}

impl UsageCounts {
    fn of(a: &UsageAgg) -> Self {
        UsageCounts { calls: a.calls, errors: a.errors, avg_ms: if a.calls > 0 { a.total_ms as f64 / a.calls as f64 } else { 0.0 } }
    }
}

#[derive(Serialize, utoipa::ToSchema)]
struct UsageByPerson {
    user_id: String,
    email: String,
    #[serde(flatten)]
    counts: UsageCounts,
}

#[derive(Serialize, utoipa::ToSchema)]
struct UsageByCredential {
    user_id: String,
    email: String,
    /// `session` (the signed-in browser), `token` (personal API token) or `oauth` (an OAuth client).
    kind: String,
    /// Token name, OAuth client name, or "Browser session".
    label: String,
    /// The token's or OAuth client's id; empty for the browser session.
    credential_id: String,
    #[serde(flatten)]
    counts: UsageCounts,
}

#[derive(Serialize, utoipa::ToSchema)]
struct UsageByChannel {
    /// `mcp`, `rest` or `chat`.
    channel: String,
    #[serde(flatten)]
    counts: UsageCounts,
}

#[derive(Serialize, utoipa::ToSchema)]
struct UsageBySession {
    user_id: String,
    email: String,
    channel: String,
    /// The `Mcp-Session-Id` the client sent, or the chat thread id.
    session: String,
    #[serde(flatten)]
    counts: UsageCounts,
}

#[derive(Serialize, utoipa::ToSchema)]
struct UsageByTool {
    tool: String,
    read_only: bool,
    #[serde(flatten)]
    counts: UsageCounts,
}

#[derive(Serialize, utoipa::ToSchema)]
struct UsagePoint {
    /// Start of the hour or day (UTC), RFC 3339.
    bucket: String,
    calls: i64,
    errors: i64,
}

#[derive(Serialize, utoipa::ToSchema)]
struct ToolUsageReport {
    #[schema(value_type = String)]
    since: chrono::DateTime<Utc>,
    hours: i64,
    /// `org`: everyone in the org (instance admin). `self`: the caller's own usage.
    scope: String,
    totals: UsageCounts,
    by_person: Vec<UsageByPerson>,
    by_credential: Vec<UsageByCredential>,
    by_channel: Vec<UsageByChannel>,
    by_session: Vec<UsageBySession>,
    by_tool: Vec<UsageByTool>,
    /// `hour` for ranges up to 48 hours, `day` beyond.
    series_step: String,
    series: Vec<UsagePoint>,
}

fn top<T>(mut rows: Vec<(i64, T)>) -> Vec<T> {
    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    rows.into_iter().take(TOP).map(|(_, r)| r).collect()
}

/// Per-hour counts folded to `step`, every step from `since` to `now` present.
fn series(rows: &[UsageAgg], since: chrono::DateTime<Utc>, now: chrono::DateTime<Utc>, step: TimeDelta) -> Vec<UsagePoint> {
    let floor = |t: chrono::DateTime<Utc>| t.duration_trunc(step).unwrap_or(t);
    let mut map: BTreeMap<chrono::DateTime<Utc>, (i64, i64)> = BTreeMap::new();
    let mut t = floor(since);
    while t <= now {
        map.insert(t, (0, 0));
        t += step;
    }
    for r in rows {
        if let Some(b) = r.bucket.as_ref().and_then(crate::sources::base::datetime_to_chrono) {
            let e = map.entry(floor(b)).or_default();
            e.0 += r.calls;
            e.1 += r.errors;
        }
    }
    map.into_iter().map(|(b, (calls, errors))| UsagePoint { bucket: b.to_rfc3339(), calls, errors }).collect()
}

#[utoipa::path(
    operation_id = "getToolUsage",
    get,
    path = "/api/audit/tool-usage",
    tag = "audit",
    summary = "Tool-call counts per person, credential, channel, MCP session and tool",
    description = "Counts every tool call (read-only and mutating; REST, MCP and chat) over a look-back window. A regular user sees their own usage; an instance admin sees everyone in the org and may narrow to one person with `user`.",
    params(UsageQuery),
    responses((status = 200, body = ToolUsageReport), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_tool_usage(State(state): State<AppState>, user: User, Query(q): Query<UsageQuery>) -> AppResult<Json<ToolUsageReport>> {
    let admin = authz::is_admin(&state.control, &user).await?;
    let only: Option<RecordId> = match q.user.as_deref().map(crate::rid::parse) {
        None => None,
        Some(Ok(id)) if id.table() == "user" => Some(id),
        Some(_) => return Err(AppError::bad_request("`user` must be a user id")),
    };
    if !admin && only.as_ref().is_some_and(|u| *u != user.id) {
        return Err(AppError::coded(ErrorCode::AuthForbidden, "only an admin can see another person's tool usage"));
    }
    let scope_user = if admin { only } else { Some(user.id.clone()) };

    let hours = q.hours.unwrap_or(DEFAULT_HOURS).clamp(1, MAX_HOURS);
    let now = Utc::now();
    let since = crate::usage::hour_bucket(now - TimeDelta::hours(hours));

    let mut res = store::control::TOOL_USAGE_REPORT
        .on(&state.control)
        .bind(("org", user.org.to_string()))
        .bind(("since", Datetime::from(since)))
        .bind(("user", scope_user.clone()))
        .await?;
    let by_user: Vec<UsageAgg> = res.take(0)?;
    let by_cred: Vec<UsageAgg> = res.take(1)?;
    let by_chan: Vec<UsageAgg> = res.take(2)?;
    let by_sess: Vec<UsageAgg> = res.take(3)?;
    let by_tool: Vec<UsageAgg> = res.take(4)?;
    let hourly: Vec<UsageAgg> = res.take(5)?;

    // names: people, their API tokens, OAuth clients
    let mut ids: Vec<RecordId> = by_user.iter().chain(&by_cred).chain(&by_sess).filter_map(|a| a.user.clone()).collect();
    ids.sort_by_key(|i| i.to_string());
    ids.dedup();
    let emails: HashMap<String, String> = store::control::TOOL_USAGE_USERS
        .on(&state.control)
        .bind(("ids", ids.clone()))
        .await?
        .take::<Vec<NameRow>>(0)?
        .into_iter()
        .filter_map(|r| r.id.map(|id| (id.to_string(), r.email)))
        .collect();
    let tokens: HashMap<String, String> = store::control::TOOL_USAGE_TOKENS
        .on(&state.control)
        .bind(("ids", ids))
        .await?
        .take::<Vec<NameRow>>(0)?
        .into_iter()
        .filter_map(|r| r.id.map(|id| (id.to_string(), r.name)))
        .collect();
    let clients: Vec<String> = by_cred.iter().filter(|a| a.actor_kind == "oauth").map(|a| a.actor_id.clone()).collect();
    let client_names: HashMap<String, String> = store::control::TOOL_USAGE_CLIENTS
        .on(&state.control)
        .bind(("ids", clients))
        .await?
        .take::<Vec<NameRow>>(0)?
        .into_iter()
        .map(|r| (r.client_id, r.name))
        .collect();
    let who = |a: &UsageAgg| {
        let id = a.user.as_ref().map(|u| u.to_string()).unwrap_or_default();
        let email = emails.get(&id).cloned().unwrap_or_default();
        (id, email)
    };

    let totals = UsageCounts {
        calls: by_chan.iter().map(|a| a.calls).sum(),
        errors: by_chan.iter().map(|a| a.errors).sum(),
        avg_ms: {
            let (calls, ms): (i64, i64) = by_chan.iter().fold((0, 0), |t, a| (t.0 + a.calls, t.1 + a.total_ms));
            if calls > 0 { ms as f64 / calls as f64 } else { 0.0 }
        },
    };

    let step_hours = if hours <= 48 { 1 } else { 24 };
    Ok(Json(ToolUsageReport {
        since,
        hours,
        scope: if scope_user.is_none() { "org" } else { "self" }.into(),
        totals,
        by_person: top(
            by_user
                .iter()
                .map(|a| {
                    let (user_id, email) = who(a);
                    (a.calls, UsageByPerson { user_id, email, counts: UsageCounts::of(a) })
                })
                .collect(),
        ),
        by_credential: top(
            by_cred
                .iter()
                .map(|a| {
                    let (user_id, email) = who(a);
                    let (kind, label, credential_id) = match a.actor_kind.as_str() {
                        "token" => ("token", tokens.get(&a.actor_id).cloned().unwrap_or_else(|| "Deleted token".into()), a.actor_id.clone()),
                        "oauth" => ("oauth", client_names.get(&a.actor_id).cloned().unwrap_or_else(|| a.actor_id.clone()), a.actor_id.clone()),
                        _ => ("session", "Browser session".to_string(), String::new()),
                    };
                    (a.calls, UsageByCredential { user_id, email, kind: kind.into(), label, credential_id, counts: UsageCounts::of(a) })
                })
                .collect(),
        ),
        by_channel: top(by_chan.iter().map(|a| (a.calls, UsageByChannel { channel: a.channel.clone(), counts: UsageCounts::of(a) })).collect()),
        by_session: top(
            by_sess
                .iter()
                .map(|a| {
                    let (user_id, email) = who(a);
                    (a.calls, UsageBySession { user_id, email, channel: a.channel.clone(), session: a.session.clone(), counts: UsageCounts::of(a) })
                })
                .collect(),
        ),
        by_tool: top(
            by_tool
                .iter()
                .map(|a| (a.calls, UsageByTool { read_only: crate::tools::registry::is_read_only(&a.tool), tool: a.tool.clone(), counts: UsageCounts::of(a) }))
                .collect(),
        ),
        series_step: if step_hours == 1 { "hour" } else { "day" }.into(),
        series: series(&hourly, since, now, TimeDelta::hours(step_hours)),
    }))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    get_audit,
    get_tool_usage,
))]
pub struct Doc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limit_is_fifty() {
        assert_eq!(default_limit(), 50);
    }

    #[test]
    fn has_more_true_when_page_does_not_cover_total() {
        assert!(has_more(0, 50, 120));
    }

    #[test]
    fn has_more_false_when_page_reaches_total() {
        assert!(!has_more(50, 50, 100));
        assert!(!has_more(0, 10, 10));
    }

    #[test]
    fn has_more_false_when_page_overshoots_total() {
        assert!(!has_more(90, 50, 100));
    }
}
