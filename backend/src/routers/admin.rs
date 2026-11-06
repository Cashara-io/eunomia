//! Workspace administration (instance admin only): the security ledger (`audit_event`) as a
//! readable activity log, and the people directory with the sign-up policy.

use std::collections::HashMap;

use axum::{extract::{Query, State}, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;
use surrealdb::types::{Datetime, RecordId};
use crate::rid::RecordIdExt;

use crate::authz;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::User;
use crate::state::AppState;
use crate::store;

pub fn router() -> Router<AppState> {
    Router::new().route("/admin/audit", get(get_workspace_audit)).route("/admin/people", get(get_people))
}

async fn require_admin(state: &AppState, user: &User) -> AppResult<()> {
    if authz::is_admin(&state.control, user).await? {
        Ok(())
    } else {
        Err(AppError::coded(ErrorCode::AuthForbidden, "Only a workspace admin can see this."))
    }
}

// ---------------------------------------------------------------- activity log

/// Which kind of event a filter chip asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, utoipa::ToSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum AuditCategory {
    #[default]
    All,
    Spaces,
    Signins,
    Tokens,
    Apps,
}

const SIGNIN_ACTIONS: &[&str] = &["auth.login", "auth.register", "auth.oidc_login", "auth.oidc_link", "auth.failed"];

impl AuditCategory {
    /// `(prefix, exact actions)` for the ledger query; both empty means every event.
    fn filter(self) -> (&'static str, &'static [&'static str]) {
        match self {
            AuditCategory::All => ("", &[]),
            AuditCategory::Spaces => ("vault.", &[]),
            AuditCategory::Signins => ("", SIGNIN_ACTIONS),
            AuditCategory::Tokens => ("auth.token_", &[]),
            AuditCategory::Apps => ("oauth.", &[]),
        }
    }

    fn of(action: &str) -> Self {
        if action.starts_with("vault.") {
            AuditCategory::Spaces
        } else if action.starts_with("auth.token_") {
            AuditCategory::Tokens
        } else if action.starts_with("oauth.") {
            AuditCategory::Apps
        } else if SIGNIN_ACTIONS.contains(&action) {
            AuditCategory::Signins
        } else {
            AuditCategory::All
        }
    }
}

fn default_limit() -> i64 {
    50
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct WorkspaceAuditQuery {
    /// Rows per page (1 to 200, default 50).
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
    /// Only this kind of event (default `all`).
    #[serde(default)]
    #[param(inline)]
    category: AuditCategory,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct LedgerRow {
    id: RecordId,
    #[serde(default)]
    #[surreal(default)]
    user: Option<RecordId>,
    #[serde(default)]
    #[surreal(default)]
    email: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    actor_kind: String,
    action: String,
    #[serde(default)]
    #[surreal(default)]
    target: String,
    #[serde(default)]
    #[surreal(default)]
    outcome: String,
    #[serde(default)]
    #[surreal(default)]
    detail: String,
    created_at: Option<Datetime>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct NamedRow {
    #[serde(default)]
    #[surreal(default)]
    id: Option<RecordId>,
    #[serde(default)]
    #[surreal(default)]
    client_id: String,
    #[serde(default)]
    #[surreal(default)]
    name: String,
}

#[derive(Serialize, utoipa::ToSchema)]
struct WorkspaceAuditEntry {
    id: String,
    #[schema(value_type = Option<String>)]
    created_at: Option<Datetime>,
    /// The person who acted; for a failed sign-in, the email that was tried. Empty for the system.
    actor_email: String,
    /// How they acted: empty for the browser, `API token`, `app` (an OAuth client) or `system`.
    via: String,
    /// The raw action code, such as `vault.invite`.
    action: String,
    category: AuditCategory,
    /// What happened, in plain words ("Invited user@acme.test to acme-engineering").
    summary: String,
    /// `ok`, or the code that ended the action.
    outcome: String,
    ok: bool,
    /// The outcome in words: "Done", "Not allowed", "Declined", "Refused" or "Failed".
    result: String,
}

#[derive(Serialize, utoipa::ToSchema)]
struct WorkspaceAuditPage {
    results: Vec<WorkspaceAuditEntry>,
    has_more: bool,
}

/// `key=value` pairs of a vault event's detail (see `vaults::service::audit_vault`).
fn kv<'a>(detail: &'a str, key: &str) -> Option<&'a str> {
    detail.split_whitespace().find_map(|p| p.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

/// One ledger row as a sentence an admin can read without knowing the action codes.
fn describe(action: &str, target: &str, detail: &str, ok: bool, names: &HashMap<String, String>) -> String {
    let email = kv(detail, "email").unwrap_or("someone");
    let space = if target.is_empty() { "a space" } else { target };
    let name = |fallback: &str| names.get(target).map(|n| format!("\u{201c}{n}\u{201d}")).unwrap_or_else(|| fallback.to_string());
    let tried = if ok { "" } else { "Tried to: " };
    let s = match action {
        "vault.invite" => match kv(detail, "role") {
            Some("owner") => format!("Invited {email} to {space} as an owner"),
            _ => format!("Invited {email} to {space}"),
        },
        "vault.revoke" => format!("Withdrew the invitation for {email} to {space}"),
        "vault.member_remove" => format!("Removed {email} from {space}"),
        "vault.role" => match kv(detail, "role") {
            Some("owner") => format!("Made {email} an owner of {space}"),
            _ => format!("Made {email} a member of {space}"),
        },
        "vault.transfer" => format!("Handed {space} over to {email}"),
        "vault.delete" => format!("Deleted the space {space}"),
        "auth.login" if ok => return "Signed in".into(),
        "auth.login" => return "Sign-in failed (wrong email or password)".into(),
        "auth.oidc_login" if ok => return "Signed in with single sign-on".into(),
        "auth.oidc_login" => return "Single sign-on sign-in failed".into(),
        "auth.oidc_link" => "Linked a single sign-on account".into(),
        "auth.register" => "Created an account".into(),
        "auth.failed" => return "Used a sign-in or token that is no longer valid".into(),
        "auth.token_create" => format!("Created the API token {}", name("(since revoked)")),
        "auth.token_revoke" => "Revoked an API token".into(),
        "auth.token_rename" => format!("Renamed an API token to {}", name("a new name")),
        "oauth.consent" if ok => format!("Allowed {} to connect", name("an app")),
        "oauth.consent" => return format!("Declined to connect {}", name("an app")),
        "oauth.grant_revoke" => "Disconnected an app".into(),
        "authz.denied" => return "A token was refused (it lacks the permission)".into(),
        "extension.create" => "Added an extension".into(),
        "extension.update" => "Changed an extension".into(),
        "extension.delete" => "Removed an extension".into(),
        "extension.rotate_secret" => "Replaced an extension's secret".into(),
        "connector.delete" => format!("Disconnected {}", if target.is_empty() { "a source" } else { target }),
        "connector.delete_data" => format!("Deleted the data from {}", if target.is_empty() { "a source" } else { target }),
        "connector.move_data" => format!("Moved the data from {}", if target.is_empty() { "a source" } else { target }),
        "source.reextract" => "Re-read a source".into(),
        "redact.backfill" => "Removed secrets from stored history".into(),
        "legacy.move" => "Moved data over from the previous version".into(),
        other => other.to_string(),
    };
    format!("{tried}{s}")
}

fn result_words(outcome: &str) -> &'static str {
    match outcome {
        "ok" => "Done",
        "access_denied" => "Declined",
        o if o.ends_with(".forbidden") || o == "auth.scope" => "Not allowed",
        "auth.unauthorized" | "auth.token_expired" | "auth.session_expired" => "Refused",
        _ => "Failed",
    }
}

fn via_words(actor_kind: &str) -> &'static str {
    match actor_kind {
        "token" => "API token",
        "oauth" => "app",
        "system" => "system",
        _ => "",
    }
}

#[utoipa::path(
    operation_id = "listWorkspaceAudit",
    get,
    path = "/api/admin/audit",
    tag = "admin",
    summary = "Workspace activity log: sign-ins, tokens, app connections and space changes (admin only)",
    description = "Reads the security ledger for the admin's workspace, newest first. Per-tool-call rows are left out (see `/api/audit/tool-usage`).",
    params(WorkspaceAuditQuery),
    responses((status = 200, body = WorkspaceAuditPage), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_workspace_audit(State(state): State<AppState>, user: User, Query(q): Query<WorkspaceAuditQuery>) -> AppResult<Json<WorkspaceAuditPage>> {
    require_admin(&state, &user).await?;
    let limit = q.limit.clamp(1, 200);
    let (prefix, actions) = q.category.filter();
    let mut rows: Vec<LedgerRow> = store::control::ADMIN_AUDIT_PAGE
        .on(&state.control)
        .bind(("org", user.org.record()))
        .bind(("prefix", prefix.to_string()))
        .bind(("actions", actions.iter().map(|a| a.to_string()).collect::<Vec<_>>()))
        // one extra row says whether there is another page
        .bind(("limit", limit + 1))
        .bind(("offset", q.offset.max(0)))
        .await?
        .take(0)?;
    let has_more = rows.len() as i64 > limit;
    rows.truncate(limit as usize);

    // names for the sentences: API tokens by id, OAuth clients by client id
    let token_ids: Vec<RecordId> = rows
        .iter()
        .filter(|r| r.action.starts_with("auth.token_"))
        .filter_map(|r| crate::rid::parse(&r.target).ok())
        .filter(|id| id.table() == "api_token")
        .collect();
    let clients: Vec<String> = rows.iter().filter(|r| r.action == "oauth.consent").map(|r| r.target.clone()).collect();
    let mut names: HashMap<String, String> = HashMap::new();
    if !token_ids.is_empty() {
        let found: Vec<NamedRow> = store::control::ADMIN_TOKEN_NAMES.on(&state.control).bind(("ids", token_ids)).await?.take(0)?;
        names.extend(found.into_iter().filter_map(|r| r.id.map(|id| (id.to_string(), r.name))));
    }
    if !clients.is_empty() {
        let found: Vec<NamedRow> = store::control::TOOL_USAGE_CLIENTS.on(&state.control).bind(("ids", clients)).await?.take(0)?;
        names.extend(found.into_iter().map(|r| (r.client_id, r.name)));
    }

    let results = rows
        .into_iter()
        .map(|r| {
            let ok = r.outcome == "ok";
            let actor_email = match r.email.filter(|e| !e.is_empty()) {
                Some(e) => e,
                // a failed password sign-in keeps the email that was tried in its detail
                None if r.user.is_none() && r.action == "auth.login" => r.detail.clone(),
                None => String::new(),
            };
            WorkspaceAuditEntry {
                id: r.id.to_string(),
                created_at: r.created_at,
                actor_email,
                via: via_words(&r.actor_kind).into(),
                summary: describe(&r.action, &r.target, &r.detail, ok, &names),
                category: AuditCategory::of(&r.action),
                result: result_words(&r.outcome).into(),
                ok,
                outcome: r.outcome,
                action: r.action,
            }
        })
        .collect();
    Ok(Json(WorkspaceAuditPage { results, has_more }))
}

// ---------------------------------------------------------------- people

#[derive(Debug, Deserialize, SurrealValue)]
struct PersonRow {
    id: RecordId,
    #[serde(default)]
    #[surreal(default)]
    email: String,
    created_at: Option<Datetime>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct SeenRow {
    owner: RecordId,
    #[serde(default)]
    #[surreal(default)]
    at: Option<Datetime>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct SpacesRow {
    user: RecordId,
    #[serde(default)]
    #[surreal(default)]
    spaces: i64,
}

#[derive(Serialize, utoipa::ToSchema)]
struct Person {
    id: String,
    email: String,
    #[schema(value_type = Option<String>)]
    created_at: Option<Datetime>,
    /// Latest browser session activity or API token use, RFC 3339; none if never seen.
    last_active: Option<String>,
    /// Shared spaces this person is an active member of (their personal space not counted).
    shared_spaces: i64,
    /// `creator` (the first account), `operator` (listed in the server's admin emails), or empty.
    admin: String,
}

#[derive(Serialize, utoipa::ToSchema)]
struct SignupPolicy {
    /// `open`, `invite` or `closed`.
    mode: String,
    /// The policy in words.
    summary: String,
    /// The emails that may sign up when `mode` is `invite`.
    allowlist: Vec<String>,
    /// Emails the server lists as admins (`EUNOMIA_ADMIN_EMAILS`).
    operators: Vec<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct PeopleReport {
    people: Vec<Person>,
    signup: SignupPolicy,
}

fn emails_of(list: &str) -> Vec<String> {
    list.split(',').map(crate::models_user::normalize_email).filter(|e| !e.is_empty()).collect()
}

/// The `SIGNUP` value as `routers::auth::signup_allowed` reads it: empty or `open` is open, unknown closes.
fn signup_policy(mode: &str, allowlist: &str, operators: &str) -> SignupPolicy {
    let (mode, summary) = match mode.trim().to_ascii_lowercase().as_str() {
        "" | "open" => ("open", "Anyone can sign up"),
        "invite" => ("invite", "Invite only: emails on the allowlist"),
        _ => ("closed", "Closed: nobody new can sign up"),
    };
    SignupPolicy {
        mode: mode.into(),
        summary: summary.into(),
        allowlist: if mode == "invite" { emails_of(allowlist) } else { Vec::new() },
        operators: emails_of(operators),
    }
}

#[utoipa::path(
    operation_id = "listPeople",
    get,
    path = "/api/admin/people",
    tag = "admin",
    summary = "Every account in the workspace and the sign-up policy (admin only)",
    responses((status = 200, body = PeopleReport), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_people(State(state): State<AppState>, user: User) -> AppResult<Json<PeopleReport>> {
    require_admin(&state, &user).await?;
    let org = state.org(&user.org).await?;
    let rows: Vec<PersonRow> = store::control::ADMIN_PEOPLE.on(&state.control).bind(("org", user.org.record())).await?.take(0)?;
    let ids: Vec<RecordId> = rows.iter().map(|r| r.id.clone()).collect();

    let mut seen_res = store::control::ADMIN_LAST_SEEN.on(&state.control).bind(("ids", ids)).await?;
    let sessions: Vec<SeenRow> = seen_res.take(0)?;
    let tokens: Vec<SeenRow> = seen_res.take(1)?;
    let mut last: HashMap<String, chrono::DateTime<chrono::Utc>> = HashMap::new();
    for r in sessions.iter().chain(&tokens) {
        if let Some(t) = r.at.as_ref().and_then(crate::sources::base::datetime_to_chrono) {
            let e = last.entry(r.owner.to_string()).or_insert(t);
            if t > *e {
                *e = t;
            }
        }
    }

    let spaces: HashMap<String, i64> = store::vaults::SHARED_COUNTS
        .on(&org.db)
        .await?
        .take::<Vec<SpacesRow>>(0)?
        .into_iter()
        .map(|r| (r.user.to_string(), r.spaces))
        .collect();

    let creator = crate::capsules::first_user(&state.control).await?;
    let people = rows
        .into_iter()
        .map(|r| {
            let key = r.id.to_string();
            let admin = if creator.as_ref() == Some(&r.id) {
                "creator"
            } else if authz::operator_email_listed(&r.email) {
                "operator"
            } else {
                ""
            };
            Person {
                last_active: last.get(&key).map(|t| t.to_rfc3339()),
                shared_spaces: spaces.get(&key).copied().unwrap_or(0),
                admin: admin.into(),
                id: key,
                email: r.email,
                created_at: r.created_at,
            }
        })
        .collect();

    let env = |k: &str| std::env::var(k).unwrap_or_default();
    Ok(Json(PeopleReport { people, signup: signup_policy(&env("SIGNUP"), &env("SIGNUP_ALLOWLIST"), &env("EUNOMIA_ADMIN_EMAILS")) }))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(get_workspace_audit, get_people))]
pub struct Doc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_events_read_as_sentences() {
        let n = HashMap::new();
        assert_eq!(describe("vault.invite", "acme-engineering", "email=user@acme.test role=member", true, &n), "Invited user@acme.test to acme-engineering");
        assert_eq!(describe("vault.invite", "acme-engineering", "email=user@acme.test role=owner", true, &n), "Invited user@acme.test to acme-engineering as an owner");
        assert_eq!(describe("vault.revoke", "x", "email=a@b.c", true, &n), "Withdrew the invitation for a@b.c to x");
        assert_eq!(describe("vault.role", "x", "email=a@b.c role=owner", true, &n), "Made a@b.c an owner of x");
        assert_eq!(describe("vault.delete", "x", "", true, &n), "Deleted the space x");
        assert_eq!(describe("vault.member_remove", "x", "email=a@b.c", false, &n), "Tried to: Removed a@b.c from x");
    }

    #[test]
    fn names_fill_token_and_app_sentences() {
        let n = HashMap::from([("api_token:t1".to_string(), "laptop".to_string()), ("cli-1".to_string(), "Claude".to_string())]);
        assert_eq!(describe("auth.token_create", "api_token:t1", "", true, &n), "Created the API token \u{201c}laptop\u{201d}");
        assert_eq!(describe("oauth.consent", "cli-1", "", true, &n), "Allowed \u{201c}Claude\u{201d} to connect");
        assert_eq!(describe("oauth.consent", "cli-1", "", false, &n), "Declined to connect \u{201c}Claude\u{201d}");
    }

    #[test]
    fn categories_round_trip() {
        assert_eq!(AuditCategory::of("vault.transfer"), AuditCategory::Spaces);
        assert_eq!(AuditCategory::of("auth.token_revoke"), AuditCategory::Tokens);
        assert_eq!(AuditCategory::of("auth.login"), AuditCategory::Signins);
        assert_eq!(AuditCategory::of("oauth.grant_revoke"), AuditCategory::Apps);
        assert_eq!(result_words("vault.forbidden"), "Not allowed");
    }

    #[test]
    fn signup_policy_in_words() {
        assert_eq!(signup_policy("", "", "").summary, "Anyone can sign up");
        let p = signup_policy(" Invite ", "A@x.test, b@x.test", "root@x.test");
        assert_eq!((p.mode.as_str(), p.allowlist.len(), p.operators[0].as_str()), ("invite", 2, "root@x.test"));
        assert_eq!(signup_policy("nope", "a@x.test", "").mode, "closed");
        assert!(signup_policy("closed", "a@x.test", "").allowlist.is_empty());
    }
}
