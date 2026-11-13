//! Workspace administration (instance admin only): the security ledger (`audit_event`) as a
//! readable activity log, and the people directory with the sign-up policy, where an admin can make
//! another account an admin, take that back, and suspend or restore an account.

use std::collections::HashMap;

use axum::{extract::{Path, Query, State}, routing::{delete, get, put}, Json, Router};
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
    Router::new()
        .route("/admin/audit", get(get_workspace_audit))
        .route("/admin/people", get(get_people))
        .route("/admin/people/{user_id}/admin", put(set_admin))
        .route("/admin/people/{user_id}/suspended", put(set_suspended))
        .route("/admin/people/{user_id}/tokens", get(get_person_tokens))
        .route("/admin/people/{user_id}/tokens/{token_id}", delete(revoke_person_token))
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
    /// Everything except sign-ins: spaces, tokens, apps, admin and the rest.
    Changes,
    Spaces,
    Signins,
    Tokens,
    Apps,
    /// Admin rights granted or taken back, accounts suspended or restored, other people's API tokens revoked,
    /// organisation invitations sent, resent, withdrawn, accepted or refused.
    Admin,
    /// Spaces shared with another Eunomia: share codes, joins, links stopped. These copy data off the server.
    Sharing,
    /// Server settings: HTTPS, updates, the AI provider.
    Server,
}

const SIGNIN_ACTIONS: &[&str] = &["auth.login", "auth.register", "auth.oidc_login", "auth.oidc_link", "auth.oidc_set_aside", "auth.failed"];
const ADMIN_ACTIONS: &[&str] = &[
    "admin.grant",
    "admin.revoke",
    "user.suspend",
    "user.restore",
    "admin.token_revoke",
    "org.invite",
    "org.invite_resend",
    "org.invite_revoke",
    "org.invite_accept",
    "org.invite_expired",
];

impl AuditCategory {
    /// `(prefix, exact actions, excluded actions)` for the ledger query; all empty means every event.
    fn filter(self) -> (&'static str, &'static [&'static str], &'static [&'static str]) {
        match self {
            AuditCategory::All => ("", &[], &[]),
            AuditCategory::Changes => ("", &[], SIGNIN_ACTIONS),
            AuditCategory::Spaces => ("vault.", &[], &[]),
            AuditCategory::Signins => ("", SIGNIN_ACTIONS, &[]),
            AuditCategory::Tokens => ("auth.token_", &[], &[]),
            AuditCategory::Apps => ("oauth.", &[], &[]),
            AuditCategory::Admin => ("", ADMIN_ACTIONS, &[]),
            AuditCategory::Sharing => ("peer.", &[], &[]),
            AuditCategory::Server => ("server.", &[], &[]),
        }
    }

    fn of(action: &str) -> Self {
        if ADMIN_ACTIONS.contains(&action) {
            AuditCategory::Admin
        } else if action.starts_with("vault.") {
            AuditCategory::Spaces
        } else if action.starts_with("auth.token_") {
            AuditCategory::Tokens
        } else if action.starts_with("oauth.") {
            AuditCategory::Apps
        } else if action.starts_with("peer.") {
            AuditCategory::Sharing
        } else if action.starts_with("server.") {
            AuditCategory::Server
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
    /// Why a failed action failed, in words ("Already a member or invited, or no account has that email").
    /// Empty when it worked, or when `summary` already says why.
    reason: String,
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

/// The value of the last `from=` in `detail`, read to the end of the line: the names it carries
/// (spaces, another Eunomia) may hold spaces.
fn from_rest(detail: &str) -> Option<&str> {
    rest_of(detail, "from")
}

/// The value of `key=` in `detail`, read to the end of the line (it must come last).
fn rest_of<'a>(detail: &'a str, key: &str) -> Option<&'a str> {
    let tag = format!("{key}=");
    let i = if detail.starts_with(&tag) { 0 } else { detail.find(&format!(" {tag}"))? + 1 };
    Some(detail[i + tag.len()..].trim()).filter(|v| !v.is_empty())
}

/// The plain verb for the past-tense verb a sentence starts with, so a failed attempt reads
/// "Couldn't invite ..." rather than "Invited ...".
fn base_verb(past: &str) -> Option<&'static str> {
    Some(match past {
        "Invited" => "invite",
        "Withdrew" => "withdraw",
        "Sent" => "send",
        "Removed" => "remove",
        "Made" => "make",
        "Handed" => "hand",
        "Deleted" => "delete",
        "Created" => "create",
        "Renamed" => "rename",
        "Joined" => "join",
        "Declined" => "decline",
        "Copied" => "copy",
        "Merged" => "merge",
        "Stopped" => "stop",
        "Turned" => "turn",
        "Started" => "start",
        "Replaced" => "replace",
        "Changed" => "change",
        "Took" => "take",
        "Suspended" => "suspend",
        "Restored" => "restore",
        "Linked" => "link",
        "Revoked" => "revoke",
        "Allowed" => "allow",
        "Added" => "add",
        "Disconnected" => "disconnect",
        "Moved" => "move",
        "Re-read" => "re-read",
        _ => return None,
    })
}

/// A sentence for something that was attempted and did not happen.
fn couldnt(done: &str) -> String {
    let (first, rest) = done.split_once(' ').unwrap_or((done, ""));
    match base_verb(first) {
        Some(v) if rest.is_empty() => format!("Couldn't {v}"),
        Some(v) => format!("Couldn't {v} {rest}"),
        None => format!("{done} (didn't go through)"),
    }
}

/// One ledger row as a sentence an admin can read without knowing the action codes.
/// How a space that no longer exists reads in the log (rows name spaces by id; see `vaults::service`).
const DELETED_SPACE: &str = "a space that has since been deleted";

fn describe(action: &str, target: &str, detail: &str, ok: bool, names: &HashMap<String, String>) -> String {
    let email = kv(detail, "email").unwrap_or("someone");
    let space = if target.is_empty() { "a space" } else { target };
    let name = |fallback: &str| names.get(target).map(|n| format!("\u{201c}{n}\u{201d}")).unwrap_or_else(|| fallback.to_string());
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
        // the row keeps the name a deleted space had (it can't be looked up any more)
        "vault.delete" => format!("Deleted the space {}", from_rest(detail).unwrap_or(space)),
        "vault.create" if space == DELETED_SPACE => "Created a space that has since been deleted".to_string(),
        "vault.create" => format!("Created the space {space}"),
        "vault.rename" => format!("Renamed {} to {space}", from_rest(detail).unwrap_or("a space")),
        "vault.join" => match kv(detail, "role") {
            Some("owner") => format!("Joined {space} as an owner"),
            _ => format!("Joined {space}"),
        },
        "vault.decline" => format!("Declined the invitation to {space}"),
        "vault.clone" => format!("Copied {} into a new space, {space}", from_rest(detail).unwrap_or("a space")),
        "vault.merge" => format!("Merged {} into a new space, {space}", from_rest(detail).unwrap_or("two spaces")),
        "peer.share_code" => match kv(detail, "role") {
            Some("read") => format!("Made a code that lets another Eunomia copy {space} (read only)"),
            _ => format!("Made a code that lets another Eunomia copy and edit {space}"),
        },
        "peer.share_code_withdraw" => format!("Withdrew an unused share code for {space}"),
        "peer.joined" => match kv(detail, "role") {
            Some("read") => format!("{} joined {space} and got a copy (read only)", peer(detail, true)),
            _ => format!("{} joined {space} and got a copy it can edit", peer(detail, true)),
        },
        "peer.join" => format!("Joined {space}, shared from {}", peer(detail, false)),
        "peer.stop" => format!("Stopped sharing {space} with {}", peer(detail, false)),
        "peer.left" => format!("{} stopped syncing {space} (it keeps its copy)", peer(detail, true)),
        "server.https" => match kv(detail, "domain") {
            Some(d) => format!("Turned on HTTPS for {d}"),
            None => "Turned off HTTPS".into(),
        },
        "server.diagnostics" => "Downloaded support diagnostics".into(),
        "server.update" => "Started a server update".into(),
        "server.ai_provider" => match (kv(detail, "host"), detail.split_whitespace().any(|p| p == "key")) {
            (Some(h), true) => format!("Replaced the key for the AI provider at {h}"),
            (Some(h), false) => format!("Changed the AI provider to {h}"),
            (None, true) => "Replaced the AI provider's key".into(),
            (None, false) => "Changed the AI provider".into(),
        },
        "org.invite" => match kv(detail, "role") {
            Some("admin") => format!("Invited {email} to the organisation as an admin"),
            _ => format!("Invited {email} to the organisation"),
        },
        "org.invite_resend" => format!("Sent {email} a new invitation link"),
        "org.invite_revoke" => format!("Withdrew the organisation invitation for {email}"),
        "org.invite_accept" if ok => match kv(detail, "role") {
            Some("admin") => "Joined the organisation as an admin".into(),
            _ => "Joined the organisation".into(),
        },
        "org.invite_accept" => return format!("The invitation for {email} could not be accepted"),
        "org.invite_expired" => return format!("Someone used the expired invitation link for {email}"),
        "admin.grant" => format!("Made {email} an admin"),
        "admin.revoke" => format!("Took admin rights away from {email}"),
        "user.suspend" => format!("Suspended {email}"),
        "user.restore" => format!("Restored access for {email}"),
        "admin.token_revoke" => match rest_of(detail, "token") {
            Some(t) => format!("Revoked {email}\u{2019}s API token \u{201c}{t}\u{201d}"),
            None => format!("Revoked an API token of {email}"),
        },
        "auth.login" if ok => return "Signed in".into(),
        "auth.login" => return "Sign-in failed (wrong email or password)".into(),
        "auth.oidc_login" if ok => return "Signed in with single sign-on".into(),
        "auth.oidc_login" => return "Single sign-on sign-in failed".into(),
        "auth.oidc_link" if ok => "Linked a single sign-on account".into(),
        "auth.oidc_link" => return "Linking single sign-on failed (wrong password)".into(),
        "auth.oidc_set_aside" => format!("Set aside {}: a single sign-on for its address started a new account instead", kv(detail, "email").unwrap_or("an account")),
        "auth.register" => "Created an account".into(),
        "auth.failed" => return "Used a sign-in or token that is no longer valid".into(),
        "auth.token_create" => format!("Created the API token {}", name("(since revoked)")),
        "auth.token_revoke" => "Revoked an API token".into(),
        "auth.token_rename" => format!("Renamed an API token to {}", name("a new name")),
        "oauth.consent" if ok => format!("Allowed {} to connect", name("an app")),
        "oauth.consent" => return format!("Declined to connect {}", name("an app")),
        "oauth.grant_revoke" => "Disconnected an app".into(),
        "auth.scope_refused_suppressed" => {
            let detail = serde_json::from_str::<serde_json::Value>(detail).unwrap_or_default();
            let count = detail["count"].as_u64().unwrap_or(0);
            let start = detail["window_start"].as_str().unwrap_or("unknown");
            let end = detail["window_end"].as_str().unwrap_or("unknown");
            let tools = detail["tools"].as_array().map(|tools| tools.iter().filter_map(serde_json::Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            return format!("Refused {count} more tool calls from {start} to {end}: {tools}");
        },
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
    if ok { s } else { couldnt(&s) }
}

/// Actions whose failed sentence already says why ([`describe`] returns early for them).
const SELF_EXPLAINED: &[&str] = &["auth.login", "auth.oidc_login", "auth.oidc_link", "auth.failed", "oauth.consent", "authz.denied", "org.invite_accept", "org.invite_expired"];

/// Why an action failed, from the error code it ended with, in words; empty when it worked.
/// The exact cause a failed attempt recorded in its detail (`why=...`), when it recorded one.
fn why_words(detail: &str) -> Option<&'static str> {
    match detail.split_whitespace().find_map(|kv| kv.strip_prefix("why="))? {
        "expired" => Some("The share code had expired"),
        "used" => Some("The share code had already been used"),
        "malformed" => Some("The share code was malformed or did not match"),
        "unknown" => Some("The share code was not found"),
        "mac" => Some("The share code authentication did not match"),
        "no_account" => Some("No account has that email yet"),
        "member" => Some("Already a member"),
        "pending" => Some("Already invited, not answered yet"),
        // rows recorded before the two were told apart
        "already" => Some("Already a member, or already invited"),
        _ => None,
    }
}

fn reason_words(action: &str, outcome: &str) -> String {
    if outcome == "ok" || outcome.is_empty() || SELF_EXPLAINED.contains(&action) {
        return String::new();
    }
    match (action, outcome) {
        // `vaults::service::invite_member_inner` refuses both with a plain bad request
        ("vault.invite", "validation.invalid") => "Already a member or invited, or no account has that email",
        ("vault.join" | "vault.decline", "validation.invalid") => "There was no invitation waiting",
        ("vault.transfer", "validation.invalid") => "They already own it, or aren't a member",
        (_, "auth.not_found") => "No such account",
        (_, "vault.not_found") => "The space doesn't exist any more",
        (_, "vault.name_taken") => "That name is already taken",
        (_, "auth.scope") => "The token doesn't have that permission",
        (_, o) if o.ends_with(".forbidden") => "Not allowed",
        (_, "rate.limited") => "Too many attempts (rate limited)",
        (_, "auth.suspended") => "The account is suspended",
        (_, "auth.token_expired") => "The token had expired",
        (_, "auth.session_expired") => "The session had expired",
        (_, "auth.unauthorized") => "Not signed in",
        (_, "access_denied") => "Declined",
        (_, "validation.invalid") => "The request wasn't valid",
        (_, "db.conflict" | "db.duplicate") => "Clashed with another change",
        (_, o) if o.ends_with(".not_found") => "It doesn't exist any more",
        (_, "internal") => "Something went wrong on the server",
        _ => "Something went wrong",
    }
    .into()
}

/// The other Eunomia a sharing event names, quoted; `start` puts it at the head of a sentence
/// (capitalized, and closed with a comma when it carries a name).
fn peer(detail: &str, start: bool) -> String {
    let lead = if start { "Another Eunomia" } else { "another Eunomia" };
    match from_rest(detail) {
        Some(n) if !n.eq_ignore_ascii_case("another eunomia") => format!("{lead}, \u{201c}{n}\u{201d}{}", if start { "," } else { "" }),
        _ => lead.to_string(),
    }
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
        "peer" => "another Eunomia",
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
    let (prefix, actions, exclude) = q.category.filter();
    let mut rows: Vec<LedgerRow> = store::control::ADMIN_AUDIT_PAGE
        .on(&state.control)
        .bind(("org", user.org.record()))
        .bind(("prefix", prefix.to_string()))
        .bind(("actions", actions.iter().map(|a| a.to_string()).collect::<Vec<_>>()))
        .bind(("exclude", exclude.iter().map(|a| a.to_string()).collect::<Vec<_>>()))
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
    // spaces are stored by id: show their current name, or "a deleted space" once they are gone
    let mut space_names: HashMap<String, String> = HashMap::new();
    let vault_ids: std::collections::BTreeSet<String> =
        rows.iter().filter(|r| r.target.starts_with("vault:")).map(|r| r.target.clone()).collect();
    if !vault_ids.is_empty() {
        let org = state.org(&user.org).await?;
        for id in vault_ids {
            let name = match crate::rid::parse(&id) {
                Ok(rid) => crate::peers::vault_name(&org.db, &rid).await.ok().flatten(),
                Err(_) => None,
            };
            space_names.insert(id, name.unwrap_or_else(|| DELETED_SPACE.to_string()));
        }
    }

    let results = rows
        .into_iter()
        .map(|r| {
            let ok = r.outcome == "ok";
            let actor_email = match r.email.filter(|e| !e.is_empty()) {
                // A refused peer request is anonymous. `user` only scopes its ledger row.
                Some(_) if r.actor_kind == "anonymous" => String::new(),
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
                summary: describe(&r.action, space_names.get(&r.target).map(String::as_str).unwrap_or(&r.target), &r.detail, ok, &names),
                category: AuditCategory::of(&r.action),
                result: result_words(&r.outcome).into(),
                reason: why_words(&r.detail).map(str::to_string).unwrap_or_else(|| reason_words(&r.action, &r.outcome)),
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
    /// The stored, normalized address (`duplicate:` for a legacy duplicate): what operator status is read from.
    #[serde(default)]
    #[surreal(default)]
    email_lc: Option<String>,
    created_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    admin: Option<bool>,
    #[serde(default)]
    #[surreal(default)]
    suspended_at: Option<Datetime>,
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
    /// Why this person is an admin: `creator` (the first account), `operator` (listed in the server's
    /// admin emails), `granted` (made an admin in the app), or empty for a member.
    admin: String,
    /// `admin` or `member`.
    role: String,
    /// When an admin suspended the account, RFC 3339; none while it is active.
    #[schema(value_type = Option<String>)]
    suspended_at: Option<Datetime>,
    /// The admin looking at the list.
    is_you: bool,
    /// How many API tokens this person holds (see `/api/admin/people/{user_id}/tokens`).
    tokens: i64,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct CountRow {
    owner: RecordId,
    #[serde(default)]
    #[surreal(default)]
    n: i64,
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

/// Why `row` is an admin, as [`Person::admin`] spells it.
fn admin_source(row: &PersonRow, creator: Option<&RecordId>) -> &'static str {
    if creator == Some(&row.id) {
        "creator"
    } else if authz::stored_email_is_operator(row.email_lc.as_deref()) {
        "operator"
    } else if row.admin == Some(true) {
        "granted"
    } else {
        ""
    }
}

/// True when some account other than `target` is an admin and not suspended: the workspace keeps an
/// admin after `target` loses admin rights or is suspended.
fn another_admin_left(rows: &[PersonRow], creator: Option<&RecordId>, target: &RecordId) -> bool {
    rows.iter().any(|r| &r.id != target && r.suspended_at.is_none() && !admin_source(r, creator).is_empty())
}

async fn people_rows(state: &AppState, user: &User) -> AppResult<Vec<PersonRow>> {
    Ok(store::control::ADMIN_PEOPLE.on(&state.control).bind(("org", user.org.record())).await?.take(0)?)
}

async fn people_report(state: &AppState, user: &User) -> AppResult<PeopleReport> {
    let org = state.org(&user.org).await?;
    let rows = people_rows(state, user).await?;
    let ids: Vec<RecordId> = rows.iter().map(|r| r.id.clone()).collect();

    let token_counts: HashMap<String, i64> = store::control::ADMIN_TOKEN_COUNTS
        .on(&state.control)
        .bind(("ids", ids.clone()))
        .await?
        .take::<Vec<CountRow>>(0)?
        .into_iter()
        .map(|r| (r.owner.to_string(), r.n))
        .collect();
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
            let admin = admin_source(&r, creator.as_ref());
            Person {
                last_active: last.get(&key).map(|t| t.to_rfc3339()),
                shared_spaces: spaces.get(&key).copied().unwrap_or(0),
                admin: admin.into(),
                role: if admin.is_empty() { "member" } else { "admin" }.into(),
                is_you: r.id == user.id,
                tokens: token_counts.get(&key).copied().unwrap_or(0),
                id: key,
                email: r.email,
                created_at: r.created_at,
                suspended_at: r.suspended_at,
            }
        })
        .collect();

    let env = |k: &str| std::env::var(k).unwrap_or_default();
    Ok(PeopleReport { people, signup: signup_policy(&env("SIGNUP"), &env("SIGNUP_ALLOWLIST"), &env("EUNOMIA_ADMIN_EMAILS")) })
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
    Ok(Json(people_report(&state, &user).await?))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct AdminChange {
    /// `true` makes the person an admin, `false` takes admin rights away.
    admin: bool,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct SuspendChange {
    /// `true` suspends the account, `false` restores it.
    suspended: bool,
}

/// The admin acting (browser session only) and the account they act on, which must be in their workspace.
async fn target_of(state: &AppState, user: &User, user_id: &str) -> AppResult<(Vec<PersonRow>, Option<RecordId>, RecordId)> {
    authz::require_session()?;
    member_of(state, user, user_id).await
}

/// The account `user_id` names, which must be in the admin's workspace (any credential).
async fn member_of(state: &AppState, user: &User, user_id: &str) -> AppResult<(Vec<PersonRow>, Option<RecordId>, RecordId)> {
    require_admin(state, user).await?;
    let not_found = || AppError::coded(ErrorCode::AuthNotFound, "That account isn't in this workspace.");
    let target: RecordId = crate::rid::parse(user_id).map_err(|_| not_found())?;
    let rows = people_rows(state, user).await?;
    if !rows.iter().any(|r| r.id == target) {
        return Err(not_found());
    }
    let creator = crate::capsules::first_user(&state.control).await?;
    Ok((rows, creator, target))
}

#[utoipa::path(
    operation_id = "setPersonAdmin",
    put,
    path = "/api/admin/people/{user_id}/admin",
    tag = "admin",
    summary = "Make someone an admin, or take admin rights away (admin only, browser session)",
    description = "The first account and the emails the server lists as admins are always admins and cannot be changed here. The last admin cannot be demoted.",
    params(("user_id" = String, Path)),
    request_body = AdminChange,
    responses((status = 200, body = PeopleReport), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn set_admin(State(state): State<AppState>, user: User, Path(user_id): Path<String>, Json(body): Json<AdminChange>) -> AppResult<Json<PeopleReport>> {
    let (rows, creator, target) = target_of(&state, &user, &user_id).await?;
    let row = rows.iter().find(|r| r.id == target).expect("checked in target_of");
    let source = admin_source(row, creator.as_ref());
    let email = row.email.clone();
    if !body.admin {
        match source {
            "creator" => return Err(AppError::bad_request("This is the account that set up the workspace. It is always an admin.")),
            "operator" => return Err(AppError::bad_request("The server's settings list this account as an admin, so it can't be changed here.")),
            "" => return Ok(Json(people_report(&state, &user).await?)),
            _ => {}
        }
        if !another_admin_left(&rows, creator.as_ref(), &target) {
            return Err(AppError::bad_request("The workspace needs at least one admin. Make someone else an admin first."));
        }
    } else if !source.is_empty() {
        return Ok(Json(people_report(&state, &user).await?));
    }
    store::control::USER_SET_ADMIN
        .on(&state.control)
        .bind(("id", target.clone()))
        .bind(("admin", body.admin.then_some(true)))
        .await?
        .check()?;
    let action = if body.admin { "admin.grant" } else { "admin.revoke" };
    crate::audit::record_as_caller_detail(&state.control, &user.id, action, &target.to_string(), "ok", &format!("email={email}")).await;
    Ok(Json(people_report(&state, &user).await?))
}

#[utoipa::path(
    operation_id = "setPersonSuspended",
    put,
    path = "/api/admin/people/{user_id}/suspended",
    tag = "admin",
    summary = "Suspend or restore an account (admin only, browser session)",
    description = "A suspended account cannot sign in. Its sessions end at once, and its API tokens and app connections are refused until it is restored. You cannot suspend yourself or the first account.",
    params(("user_id" = String, Path)),
    request_body = SuspendChange,
    responses((status = 200, body = PeopleReport), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn set_suspended(
    State(state): State<AppState>,
    user: User,
    Path(user_id): Path<String>,
    Json(body): Json<SuspendChange>,
) -> AppResult<Json<PeopleReport>> {
    let (rows, creator, target) = target_of(&state, &user, &user_id).await?;
    let row = rows.iter().find(|r| r.id == target).expect("checked in target_of");
    let email = row.email.clone();
    if body.suspended == row.suspended_at.is_some() {
        return Ok(Json(people_report(&state, &user).await?));
    }
    if body.suspended {
        if target == user.id {
            return Err(AppError::bad_request("You can't suspend your own account."));
        }
        if creator.as_ref() == Some(&target) {
            return Err(AppError::bad_request("This is the account that set up the workspace. It can't be suspended."));
        }
        if !admin_source(row, creator.as_ref()).is_empty() && !another_admin_left(&rows, creator.as_ref(), &target) {
            return Err(AppError::bad_request("The workspace needs at least one admin. Make someone else an admin first."));
        }
    }
    store::control::USER_SET_SUSPENDED
        .on(&state.control)
        .bind(("id", target.clone()))
        .bind(("suspended", body.suspended))
        .await?
        .check()?;
    if body.suspended {
        // every browser session ends now; tokens and app connections are refused while suspended (`load_user`)
        store::control::AUTH_SESSION_REVOKE_OWNER.on(&state.control).bind(("owner", target.clone())).await?.check()?;
        state.credentials.forget_user(&target);
    }
    let action = if body.suspended { "user.suspend" } else { "user.restore" };
    crate::audit::record_as_caller_detail(&state.control, &user.id, action, &target.to_string(), "ok", &format!("email={email}")).await;
    Ok(Json(people_report(&state, &user).await?))
}

// ---------------------------------------------------------------- other people's API tokens

#[derive(Serialize, utoipa::ToSchema)]
struct PersonToken {
    id: String,
    name: String,
    #[schema(value_type = Option<String>)]
    created_at: Option<Datetime>,
    #[schema(value_type = Option<String>)]
    last_used_at: Option<Datetime>,
    /// `null` for a token that never expires.
    #[schema(value_type = Option<String>)]
    expires_at: Option<Datetime>,
}

fn person_tokens(rows: Vec<crate::models_user::ApiTokenSummary>) -> Vec<PersonToken> {
    rows.into_iter()
        .map(|r| PersonToken { id: r.id.to_string(), name: r.name, created_at: Some(r.created_at), last_used_at: r.last_used_at, expires_at: r.expires_at })
        .collect()
}

#[utoipa::path(
    operation_id = "listPersonTokens",
    get,
    path = "/api/admin/people/{user_id}/tokens",
    tag = "admin",
    summary = "Someone's API tokens, newest first (admin only)",
    description = "Names and dates only; a token's secret is never stored or shown.",
    params(("user_id" = String, Path)),
    responses((status = 200, body = Vec<PersonToken>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_person_tokens(State(state): State<AppState>, user: User, Path(user_id): Path<String>) -> AppResult<Json<Vec<PersonToken>>> {
    let (_, _, target) = member_of(&state, &user, &user_id).await?;
    Ok(Json(person_tokens(crate::models_user::list_api_tokens(&state.control, &target).await?)))
}

#[utoipa::path(
    operation_id = "revokePersonToken",
    delete,
    path = "/api/admin/people/{user_id}/tokens/{token_id}",
    tag = "admin",
    summary = "Revoke someone's API token (admin only, browser session)",
    description = "The token stops working at once. Returns the person's remaining tokens.",
    params(("user_id" = String, Path), ("token_id" = String, Path)),
    responses((status = 200, body = Vec<PersonToken>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn revoke_person_token(
    State(state): State<AppState>,
    user: User,
    Path((user_id, token_id)): Path<(String, String)>,
) -> AppResult<Json<Vec<PersonToken>>> {
    let (rows, _, target) = target_of(&state, &user, &user_id).await?;
    let email = rows.iter().find(|r| r.id == target).map(|r| r.email.clone()).unwrap_or_default();
    let not_found = || AppError::coded(ErrorCode::AuthNotFound, "That token doesn't exist any more.");
    let rid: RecordId = crate::rid::parse(&token_id).map_err(|_| not_found())?;
    let tokens = crate::models_user::list_api_tokens(&state.control, &target).await?;
    let name = tokens.iter().find(|t| t.id == rid).map(|t| t.name.clone()).ok_or_else(not_found)?;
    let gone = crate::models_user::revoke_api_token(&state.control, &target, &rid).await?;
    state.credentials.forget(&rid.to_string());
    if !gone {
        return Err(not_found());
    }
    crate::audit::record_as_caller_detail(&state.control, &user.id, "admin.token_revoke", &rid.to_string(), "ok", &format!("email={email} token={name}")).await;
    Ok(Json(person_tokens(crate::models_user::list_api_tokens(&state.control, &target).await?)))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(get_workspace_audit, get_people, set_admin, set_suspended, get_person_tokens, revoke_person_token))]
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
        assert_eq!(describe("vault.member_remove", "x", "email=a@b.c", false, &n), "Couldn't remove a@b.c from x");
        assert_eq!(describe("vault.invite", "Team", "email=a@b.c role=member", false, &n), "Couldn't invite a@b.c to Team");
        assert_eq!(describe("peer.left", "Team", "", false, &n), "Another Eunomia stopped syncing Team (it keeps its copy) (didn't go through)");
        assert_eq!(reason_words("vault.invite", "validation.invalid"), "Already a member or invited, or no account has that email");
        assert_eq!(why_words("email=a@b.c role=member why=no_account"), Some("No account has that email yet"));
        assert_eq!(why_words("email=a@b.c why=already"), Some("Already a member, or already invited"));
        assert_eq!(why_words("email=a@b.c why=member"), Some("Already a member"));
        assert_eq!(why_words("email=a@b.c why=pending"), Some("Already invited, not answered yet"));
        assert_eq!(why_words("email=a@b.c"), None);
        assert_eq!(reason_words("vault.role", "vault.forbidden"), "Not allowed");
        assert_eq!(reason_words("vault.create", "rate.limited"), "Too many attempts (rate limited)");
        assert_eq!(reason_words("vault.create", "ok"), "");
        assert_eq!(reason_words("auth.login", "auth.unauthorized"), "");
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
    fn space_sharing_and_server_events_read_as_sentences() {
        let n = HashMap::new();
        let d = |a: &str, t: &str, detail: &str| describe(a, t, detail, true, &n);
        assert_eq!(d("vault.create", "Team", ""), "Created the space Team");
        assert_eq!(d("vault.rename", "Team B", "from=Team A"), "Renamed Team A to Team B");
        assert_eq!(d("vault.join", "Team", "role=member"), "Joined Team");
        assert_eq!(d("vault.decline", "Team", ""), "Declined the invitation to Team");
        assert_eq!(d("vault.clone", "Team (copy)", "from=Team"), "Copied Team into a new space, Team (copy)");
        assert_eq!(d("vault.merge", "A + B", "from=A and B"), "Merged A and B into a new space, A + B");
        assert_eq!(d("peer.share_code", "Team", "role=read"), "Made a code that lets another Eunomia copy Team (read only)");
        assert_eq!(d("peer.share_code", "Team", "role=edit"), "Made a code that lets another Eunomia copy and edit Team");
        assert_eq!(d("peer.share_code_withdraw", "Team", ""), "Withdrew an unused share code for Team");
        assert_eq!(d("peer.joined", "Team", "role=read from=Lab box"), "Another Eunomia, \u{201c}Lab box\u{201d}, joined Team and got a copy (read only)");
        assert_eq!(d("peer.joined", "Team", "role=edit from=another Eunomia"), "Another Eunomia joined Team and got a copy it can edit");
        assert_eq!(d("peer.join", "Team", "role=edit from=Office"), "Joined Team, shared from another Eunomia, \u{201c}Office\u{201d}");
        assert_eq!(d("peer.stop", "Team", "from=Office"), "Stopped sharing Team with another Eunomia, \u{201c}Office\u{201d}");
        assert_eq!(d("peer.left", "Team", "from=Office"), "Another Eunomia, \u{201c}Office\u{201d}, stopped syncing Team (it keeps its copy)");
        assert_eq!(d("server.https", "", "domain=mem.example.com"), "Turned on HTTPS for mem.example.com");
        assert_eq!(d("server.https", "", "off"), "Turned off HTTPS");
        assert_eq!(d("server.update", "", ""), "Started a server update");
        assert_eq!(d("server.ai_provider", "", "host=api.openai.com"), "Changed the AI provider to api.openai.com");
        assert_eq!(d("server.ai_provider", "", "host=api.openai.com key"), "Replaced the key for the AI provider at api.openai.com");
        for (action, cat) in [("peer.joined", AuditCategory::Sharing), ("peer.share_code", AuditCategory::Sharing), ("server.https", AuditCategory::Server), ("vault.merge", AuditCategory::Spaces)] {
            assert_eq!(AuditCategory::of(action), cat, "{action}");
            // "Changes" leaves out sign-ins only, so it shows every one of these
            assert!(!AuditCategory::Changes.filter().2.contains(&action));
        }
        assert_eq!(via_words("peer"), "another Eunomia");
    }

    #[test]
    fn admin_events_read_as_sentences() {
        let n = HashMap::new();
        assert_eq!(describe("admin.grant", "user:b", "email=bob@x.test", true, &n), "Made bob@x.test an admin");
        assert_eq!(describe("admin.revoke", "user:b", "email=bob@x.test", true, &n), "Took admin rights away from bob@x.test");
        assert_eq!(describe("user.suspend", "user:b", "email=bob@x.test", true, &n), "Suspended bob@x.test");
        assert_eq!(describe("user.restore", "user:b", "email=bob@x.test", true, &n), "Restored access for bob@x.test");
        assert_eq!(describe("admin.token_revoke", "api_token:t", "email=bob@x.test token=work laptop", true, &n), "Revoked bob@x.test\u{2019}s API token \u{201c}work laptop\u{201d}");
        assert_eq!(AuditCategory::of("admin.token_revoke"), AuditCategory::Admin);
        assert_eq!(AuditCategory::of("user.suspend"), AuditCategory::Admin);
        assert_eq!(AuditCategory::Changes.filter().2, SIGNIN_ACTIONS);
    }

    #[test]
    fn invitation_events_read_as_sentences() {
        let n = HashMap::new();
        let d = |a: &str, detail: &str, ok: bool| describe(a, "org_invitation:x", detail, ok, &n);
        assert_eq!(d("org.invite", "email=pat@acme.test role=member", true), "Invited pat@acme.test to the organisation");
        assert_eq!(d("org.invite", "email=pat@acme.test role=admin", true), "Invited pat@acme.test to the organisation as an admin");
        assert_eq!(d("org.invite_resend", "email=pat@acme.test role=member", true), "Sent pat@acme.test a new invitation link");
        assert_eq!(d("org.invite_revoke", "email=pat@acme.test role=member", true), "Withdrew the organisation invitation for pat@acme.test");
        assert_eq!(d("org.invite_accept", "email=pat@acme.test role=member via=signup", true), "Joined the organisation");
        let refused = d("org.invite_accept", "email=pat@acme.test", false);
        assert_eq!(refused, "The invitation for pat@acme.test could not be accepted");
        assert!(!refused.contains("organisation") && reason_words("org.invite_accept", crate::invitations::NOT_ACCEPTED).is_empty(), "the inviter is not told why");
        assert_eq!(d("org.invite_expired", "email=pat@acme.test", false), "Someone used the expired invitation link for pat@acme.test");
        assert_eq!(reason_words("org.invite_expired", "invitation.expired"), "");
        for a in ["org.invite", "org.invite_resend", "org.invite_revoke", "org.invite_accept", "org.invite_expired"] {
            assert_eq!(AuditCategory::of(a), AuditCategory::Admin, "{a}");
        }
    }

    #[test]
    fn the_last_admin_is_kept() {
        let row = |id: &str, admin: bool, suspended: bool| PersonRow {
            id: crate::rid::parse(id).unwrap(),
            email: format!("{}@x.test", id.trim_start_matches("user:")),
            email_lc: None,
            created_at: None,
            admin: admin.then_some(true),
            suspended_at: suspended.then(Datetime::now),
        };
        let a: RecordId = crate::rid::parse("user:a").unwrap();
        let b: RecordId = crate::rid::parse("user:b").unwrap();
        // b is the only admin (the creator is in another workspace): b cannot be demoted or suspended
        let rows = vec![row("user:a", false, false), row("user:b", true, false)];
        assert!(!another_admin_left(&rows, None, &b));
        assert!(another_admin_left(&rows, None, &a));
        // a suspended admin does not count
        let rows = vec![row("user:a", true, true), row("user:b", true, false)];
        assert!(!another_admin_left(&rows, None, &b));
        // the creator always counts
        let rows = vec![row("user:a", false, false), row("user:b", true, false)];
        assert!(another_admin_left(&rows, Some(&a), &b));
        assert_eq!(admin_source(&rows[0], Some(&a)), "creator");
        assert_eq!(admin_source(&rows[1], Some(&a)), "granted");
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
