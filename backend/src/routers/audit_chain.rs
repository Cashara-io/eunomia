//! The security log's integrity (admin only): verify the hash chain, and export a time range of it
//! as a file anyone can verify offline (`scripts/audit/verify-audit-export.py`). See
//! docs/audit-chain.md.

use axum::{
    extract::{Query, State},
    http::header,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::audit::chain::{self, AuditExport, AuditVerification};
use crate::error::{AppError, AppResult};
use crate::models_user::User;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/admin/audit/verify", get(verify)).route("/admin/audit/export", get(export))
}

#[derive(Serialize, utoipa::ToSchema)]
struct AuditVerifyReport {
    /// The workspace's own ledger first, then, for a server operator (or any admin while the
    /// install has one workspace), the `instance` ledger: events with no workspace, such as sign-ins
    /// with an unknown email.
    ledgers: Vec<AuditVerification>,
}

#[utoipa::path(
    operation_id = "verifyAuditChain",
    get,
    path = "/api/admin/audit/verify",
    tag = "admin",
    summary = "Check that the security log was not altered (admin only)",
    description = "Seals anything still pending, then walks the workspace's hash chain and the instance chain from their starting checkpoints to their newest event and reports the first break: an edited, deleted or reordered event.",
    responses((status = 200, body = AuditVerifyReport), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn verify(State(state): State<AppState>, user: User) -> AppResult<Json<AuditVerifyReport>> {
    super::admin::require_admin(&state, &user).await?;
    let mut ledgers = vec![chain::verify(&state.control, &chain::org_ledger(&user.org)).await?];
    if crate::authz::sees_instance(&state.control, &user).await? {
        ledgers.push(chain::verify(&state.control, chain::INSTANCE).await?);
    }
    Ok(Json(AuditVerifyReport { ledgers }))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ExportQuery {
    /// Start of the range (inclusive): RFC 3339 or `YYYY-MM-DD` (UTC midnight). Omit for the oldest kept event.
    from: Option<String>,
    /// End of the range (exclusive): RFC 3339 or `YYYY-MM-DD`. Omit for the newest event.
    to: Option<String>,
    /// `org` (default): this workspace's events. `instance`: events with no workspace (server
    /// operators only on a host with several workspaces).
    ledger: Option<String>,
}

fn parse_time(field: &str, v: Option<&str>) -> AppResult<Option<DateTime<Utc>>> {
    let Some(v) = v.map(str::trim).filter(|v| !v.is_empty()) else { return Ok(None) };
    if let Ok(t) = DateTime::parse_from_rfc3339(v) {
        return Ok(Some(t.with_timezone(&Utc)));
    }
    if let Ok(d) = NaiveDate::parse_from_str(v, "%Y-%m-%d") {
        return Ok(d.and_hms_opt(0, 0, 0).map(|t| t.and_utc()));
    }
    Err(AppError::bad_request(format!("`{field}` must be a date (YYYY-MM-DD) or an RFC 3339 time.")))
}

#[utoipa::path(
    operation_id = "exportAuditChain",
    get,
    path = "/api/admin/audit/export",
    tag = "admin",
    summary = "Download the security log for a time range, with checkpoints to verify it offline (admin only)",
    description = "Metadata only: who, what, when, on which record and the outcome. Free-text detail is replaced by a keyed digest (HMAC-SHA256 under a server key), so the file holds no memory text, chat content, prompts or secrets and still verifies. At most 50,000 events per file: `truncated` and `next_seq` say where to continue.",
    params(ExportQuery),
    responses((status = 200, body = AuditExport), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn export(State(state): State<AppState>, user: User, Query(q): Query<ExportQuery>) -> AppResult<impl IntoResponse> {
    // the export and the event that records it finish together, whatever the client does meanwhile
    crate::audit::detached(export_now(state, user, q)).await
}

async fn export_now(state: AppState, user: User, q: ExportQuery) -> AppResult<([(header::HeaderName, String); 1], Json<AuditExport>)> {
    super::admin::require_admin(&state, &user).await?;
    let ledger = match q.ledger.as_deref().unwrap_or("org") {
        "org" | "" => chain::org_ledger(&user.org),
        "instance" if crate::authz::sees_instance(&state.control, &user).await? => chain::INSTANCE.to_string(),
        "instance" => {
            return Err(AppError::coded(
                crate::error::ErrorCode::AuthForbidden,
                "Only the server's operators can download sign-in attempts that belong to no workspace.",
            ));
        }
        _ => return Err(AppError::bad_request("`ledger` must be `org` or `instance`.")),
    };
    let from = parse_time("from", q.from.as_deref())?;
    let to = parse_time("to", q.to.as_deref())?;
    let out = chain::export(&state.control, &ledger, from, to).await?;
    let detail = format!(
        "ledger={ledger} events={} first={} last={} from={} to={}",
        out.events.len(),
        out.events.first().map(|e| e.fields.seq).unwrap_or_default(),
        out.events.last().map(|e| e.fields.seq).unwrap_or_default(),
        if out.from.is_empty() { "-" } else { &out.from },
        if out.to.is_empty() { "-" } else { &out.to },
    );
    crate::audit::record_as_caller_detail(&state.control, &user.id, "audit.export", &ledger, "ok", &detail).await;
    let name = format!("eunomia-audit-{}-{}.json", ledger.replace(':', "-"), Utc::now().format("%Y%m%d-%H%M%S"));
    Ok(([(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\""))], Json(out)))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(verify, export))]
pub struct Doc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_accepts_dates_and_times() {
        assert_eq!(parse_time("from", Some("2026-11-01")).unwrap().unwrap().to_rfc3339(), "2026-11-01T00:00:00+00:00");
        assert!(parse_time("from", Some("2026-11-01T10:00:00+10:00")).unwrap().is_some());
        assert!(parse_time("from", Some(" ")).unwrap().is_none());
        assert!(parse_time("from", Some("last week")).is_err());
    }
}
