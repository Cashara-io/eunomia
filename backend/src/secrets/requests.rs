//! Secret requests (docs/architecture/specs/secret-requests.md): an agent asks for a secret with
//! `secret_request`, the person behind the agent's credential pastes it into Eunomia's own pop-up, and the
//! agent only ever gets the handle. This module holds the request lifecycle (create, status, list, fill,
//! decline) and the management of held secrets (list, replace, delete), with who may do each; the REST
//! routes (`routers/secret_requests.rs`) and the tools (`tools/registry/secrets.rs`) are thin layers on it.
//!
//! The pasted value only passes through [`fill`] and [`replace_held`]: a `Zeroizing<String>` from the
//! request body, encrypted at once, never logged, never a tool argument.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use surrealdb::types::{Datetime, RecordId, SurrealValue};
use zeroize::Zeroizing;

use super::held::{self, HeldMeta};
use super::reference;
use crate::authz::{self, Caller};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::state::OrgState;
use crate::store::secret_requests as q;

/// At most this many pending requests per person, so an agent cannot flood the pop-up.
pub const MAX_PENDING: i64 = 10;
pub const DEFAULT_TTL_MINUTES: i64 = 60;
pub const MAX_TTL_MINUTES: i64 = 10080;

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct RequestRow {
    pub id: RecordId,
    pub vault: RecordId,
    pub name: String,
    pub label: String,
    pub description: String,
    pub target_hosts: Vec<String>,
    pub placements: Vec<String>,
    #[serde(default)]
    #[surreal(default)]
    pub connector: Option<String>,
    pub requested_by: RecordId,
    pub actor_kind: String,
    pub actor_id: String,
    pub client_name: String,
    pub channel: String,
    pub session: String,
    pub status: String,
    pub replace: bool,
    #[serde(default)]
    #[surreal(default)]
    pub secret_ttl_days: Option<i64>,
    pub expires_at: Datetime,
    pub created_at: Datetime,
    #[serde(default)]
    #[surreal(default)]
    pub decided_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    pub decided_by: Option<RecordId>,
    #[serde(default)]
    #[surreal(default)]
    pub secret: Option<RecordId>,
}

impl RequestRow {
    pub fn handle(&self) -> String {
        held::handle_for(&self.vault, &self.name)
    }

    pub fn expired(&self) -> bool {
        chrono::DateTime::<chrono::Utc>::from(self.expires_at) <= chrono::Utc::now()
    }

    /// `expired` for a pending request past its time, even before the next read marks it.
    pub fn effective_status(&self) -> &str {
        if self.status == "pending" && self.expired() { "expired" } else { &self.status }
    }
}

// ---------------------------------------------------------------- audit

/// One `secret.*` row in control `audit_event`, for the current credential, scoped to the org by `target`.
pub async fn audit(state: &OrgState, user: &RecordId, action: &str, outcome: &str, detail: &str) {
    let actor = authz::current().map(|c| c.actor).unwrap_or(authz::Actor { kind: "user", id: user.to_string() });
    let org = state.db.org().key();
    crate::audit::record(&state.control, crate::audit::Event { user: Some(user), actor: &actor, action, target: &org, outcome, detail }).await;
}

// ---------------------------------------------------------------- who may

/// `owner`, `member` or `None` (not an active member).
pub async fn vault_role(db: &OrgDb, user: &RecordId, vault: &RecordId) -> AppResult<Option<String>> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        #[serde(default)]
        #[surreal(default)]
        role: String,
    }
    let rows: Vec<Row> = crate::store::vaults::MEMBERSHIP_ACTIVE.on(db).bind(("vault", vault.clone())).bind(("user", user.clone())).await?.check()?.take(0)?;
    Ok(rows.into_iter().next().map(|r| if r.role == "owner" { "owner".to_string() } else { "member".to_string() }))
}

/// Whether `user` manages what is in `vault`: an org owner or instance admin (`org_manager`, which the
/// route works out), or an owner of the vault.
pub async fn manages_vault(db: &OrgDb, user: &RecordId, org_manager: bool, vault: &RecordId) -> AppResult<bool> {
    Ok(org_manager || vault_role(db, user, vault).await?.as_deref() == Some("owner"))
}

// ---------------------------------------------------------------- the fill URL

/// `<frontend origin>/secrets/requests/<key>`: no token, nothing pre-authenticated; the page needs the
/// person's browser session. The flag is set for plain http on a host that is not this machine.
pub fn fill_url(settings: &crate::config::Settings, id: &RecordId) -> (String, bool) {
    fill_url_at(&crate::oauth::frontend_url(settings), id)
}

fn fill_url_at(front: &str, id: &RecordId) -> (String, bool) {
    let key = crate::rid::key_string(id.key()).unwrap_or_default();
    let insecure = url::Url::parse(front).is_ok_and(|u| u.scheme() == "http" && !matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")));
    (format!("{front}/secrets/requests/{key}"), insecure)
}

/// The name of the credential that asked, for the pop-up's "who asked".
async fn client_name(state: &OrgState, user: &RecordId, who: &Caller) -> String {
    match who.actor.kind {
        "token" => {
            let Ok(id) = crate::rid::parse(&who.actor.id) else { return String::new() };
            let names: Vec<String> = match q::TOKEN_NAME.on(&state.control).bind(("id", id)).bind(("owner", user.clone())).await {
                Ok(mut r) => r.take(0).unwrap_or_default(),
                Err(_) => Vec::new(),
            };
            names.into_iter().next().unwrap_or_default()
        }
        "oauth" => {
            let names: Vec<String> = match q::CLIENT_NAME.on(&state.control).bind(("client", who.actor.id.clone())).await {
                Ok(mut r) => r.take(0).unwrap_or_default(),
                Err(_) => Vec::new(),
            };
            names.into_iter().next().unwrap_or_default()
        }
        "user" => "Eunomia web app".into(),
        _ => String::new(),
    }
}

fn redacted(text: &str, max: usize) -> String {
    let t: String = text.trim().chars().take(max).collect();
    crate::redact::redact(&t, &mut Vec::new()).chars().take(max).collect()
}

fn caller(user: &RecordId) -> Caller {
    authz::current().unwrap_or_else(|| Caller::session(user))
}

// ---------------------------------------------------------------- the tools

#[derive(Deserialize, Default)]
struct Target {
    #[serde(default)]
    hosts: Vec<String>,
    #[serde(default)]
    placements: Option<Vec<String>>,
    #[serde(default)]
    connector: Option<String>,
}

#[derive(Deserialize)]
struct RequestArgs {
    name: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    vault_id: Option<String>,
    #[serde(default)]
    target: Option<Target>,
    #[serde(default)]
    expires_in_minutes: Option<i64>,
    #[serde(default)]
    secret_expires_in_days: Option<i64>,
    #[serde(default)]
    replace: bool,
}

const NEXT: &str = "Ask the user to open fill_url in their browser (signed in to Eunomia) and paste the secret there, never in this chat. \
Then call secret_request_status with request_id. Once it is filled, use handle in http_request or secret_transform; you never see the value.";

fn pending_value(state: &OrgState, r: &RequestRow) -> Value {
    let (url, insecure) = fill_url(&state.settings, &r.id);
    let mut v = json!({
        "request_id": r.id.to_string(),
        "status": "pending",
        "fill_url": url,
        "handle": held::handle_value(&r.handle()),
        "vault_id": r.vault.to_string(),
        "expires_at": r.expires_at,
        "next": NEXT,
    });
    if insecure {
        v["insecure_url"] = json!(true);
    }
    v
}

/// `secret_request`: a pending request (or the caller's existing one, or `filled` when a usable secret
/// already has that name). Never a value.
pub async fn create(state: &OrgState, user: &RecordId, args: Value) -> AppResult<Value> {
    let a: RequestArgs = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return Ok(json!({"error": format!("invalid arguments: {e}")})),
    };
    let invalid = |m: &str| Ok(AppError::coded(ErrorCode::ValidationInvalid, m.to_string()).to_tool_value());
    if !reference::valid_name(&a.name) {
        return invalid("name is a stable key such as stripe_api_key: lowercase letters, digits, _, . and -, at most 64 characters");
    }
    let ttl = a.expires_in_minutes.unwrap_or(DEFAULT_TTL_MINUTES);
    if !(1..=MAX_TTL_MINUTES).contains(&ttl) {
        return invalid("expires_in_minutes is 1 to 10080 (a week)");
    }
    if a.secret_expires_in_days.is_some_and(|d| !(1..=3650).contains(&d)) {
        return invalid("secret_expires_in_days is 1 to 3650");
    }
    let target = a.target.unwrap_or_default();
    let hosts = match held::check_hosts(&target.hosts) {
        Ok(h) => h,
        Err(e) => return Ok(e.to_tool_value()),
    };
    let placements = match target.placements.as_deref().map(held::check_placements).unwrap_or_else(|| Ok(vec!["header".to_string()])) {
        Ok(p) if p.is_empty() => vec!["header".to_string()],
        Ok(p) => p,
        Err(e) => return Ok(e.to_tool_value()),
    };
    let connector = target.connector.map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
    if connector.as_ref().is_some_and(|c| c.len() > 64 || !c.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))) {
        return invalid("target.connector is a connector kind such as square, at most 64 characters");
    }

    // the vault: as named (an id by now), else a restricted token's own, else personal
    let vault = match &a.vault_id {
        Some(v) => match crate::rid::parse(v).ok().filter(|r: &RecordId| r.table() == "vault") {
            Some(r) => r,
            None => return Ok(AppError::coded(ErrorCode::VaultNotFound, format!("no vault {v:?}")).to_tool_value()),
        },
        None => crate::vaults::service::default_vault_id(&state.db, user).await?,
    };
    if let Err(e) = authz::authorize(&state.db, user, authz::Action::WriteMemories, &vault).await {
        return Ok(e.to_tool_value());
    }
    let who = caller(user);
    let vaults = super::my_vaults(&state.db, user).await?;

    // a secret with that name already: hand back its handle when this caller may use it
    if let Some(existing) = held::by_name(&state.db, &vaults, &vault, &a.name).await? {
        let usable = held::refusal(&state.db, user, &who, "http_request", &existing).await.is_none();
        if !a.replace {
            if usable {
                return Ok(json!({"status": "filled", "handle": held::handle_value(&existing.handle()), "vault_id": vault.to_string(),
                    "next": "A secret with this name is already held and you may use it. Pass replace: true to ask the user for a new value."}));
            }
            return Ok(AppError::coded(ErrorCode::DbDuplicate, format!("{} already exists and this credential may not use it: choose another name", existing.handle())).to_tool_value());
        }
        if existing.created_by != *user && vault_role(&state.db, user, &vault).await?.as_deref() != Some("owner") {
            return Ok(AppError::coded(ErrorCode::AuthForbidden, "Only whoever filled that secret or an owner of the vault may replace it.").to_tool_value());
        }
    }

    // a retry gets the same pending request
    let same: Vec<RequestRow> = q::REQ_PENDING_SAME
        .on(&state.db)
        .bind(("vault", vault.clone()))
        .bind(("name", a.name.clone()))
        .bind(("actor_id", who.actor.id.clone()))
        .await?
        .check()?
        .take(0)?;
    if let Some(r) = same.into_iter().next() {
        return Ok(pending_value(state, &r));
    }

    #[derive(Deserialize, SurrealValue)]
    struct Count {
        n: i64,
    }
    let pending: Vec<Count> = q::REQ_PENDING_COUNT.on(&state.db).bind(("requester", user.clone())).bind(("vaults", vaults.clone())).await?.check()?.take(0)?;
    if pending.first().is_some_and(|c| c.n >= MAX_PENDING) {
        return Ok(AppError::coded(ErrorCode::RateLimited, format!("{MAX_PENDING} secret requests are already waiting for this person; ask them to answer those first")).to_tool_value());
    }

    let ctx = crate::usage::current_context();
    let label = redacted(a.label.as_deref().unwrap_or(&a.name), 120);
    let rows: Vec<RequestRow> = q::REQ_CREATE
        .on(&state.db)
        .bind(("vault", vault.clone()))
        .bind(("name", a.name.clone()))
        .bind(("label", if label.is_empty() { a.name.clone() } else { label }))
        .bind(("description", redacted(a.description.as_deref().unwrap_or_default(), 1000)))
        .bind(("hosts", hosts.clone()))
        .bind(("placements", placements))
        .bind(("connector", connector))
        .bind(("requester", user.clone()))
        .bind(("actor_kind", who.actor.kind.to_string()))
        .bind(("actor_id", who.actor.id.clone()))
        .bind(("client_name", client_name(state, user, &who).await))
        .bind(("channel", ctx.channel.as_str().to_string()))
        .bind(("mcp_session", ctx.session.clone()))
        .bind(("replace", a.replace))
        .bind(("ttl_days", a.secret_expires_in_days))
        .bind(("expires_at", Datetime::from(chrono::Utc::now() + chrono::TimeDelta::minutes(ttl))))
        .await?
        .check()?
        .take(0)?;
    let r = rows.into_iter().next().ok_or_else(|| AppError::internal("secret_request create returned no row"))?;
    let (_, insecure) = fill_url(&state.settings, &r.id);
    if insecure {
        tracing::warn!("secret request fill URL is plain http on a non-local host: set the frontend origin to https");
    }
    let detail = json!({"request": r.id.to_string(), "vault": vault.to_string(), "name": r.name, "hosts": hosts, "channel": ctx.channel.as_str(), "session": ctx.session}).to_string();
    audit(state, user, "secret.request", "ok", &detail).await;
    Ok(pending_value(state, &r))
}

fn request_id(raw: &str) -> Option<RecordId> {
    let raw = raw.trim();
    let full = if raw.contains(':') { raw.to_string() } else { format!("secret_request:{raw}") };
    crate::rid::parse(&full).ok().filter(|r: &RecordId| r.table() == "secret_request")
}

async fn expire(db: &OrgDb, vaults: &[RecordId]) -> AppResult<()> {
    q::REQ_EXPIRE.on(db).bind(("vaults", vaults.to_vec())).await?.check()?;
    Ok(())
}

async fn get_request(db: &OrgDb, vaults: &[RecordId], id: &RecordId) -> AppResult<Option<RequestRow>> {
    Ok(q::REQ_GET.on(db).bind(("id", id.clone())).bind(("vaults", vaults.to_vec())).await?.check()?.take::<Vec<RequestRow>>(0)?.into_iter().next())
}

/// `secret_request_status`: the request's state and, once filled, its handle. Only the credential's own
/// person sees their requests here (another member's request reads as not found).
pub async fn status(state: &OrgState, user: &RecordId, args: Value) -> AppResult<Value> {
    let Some(id) = args.get("request_id").and_then(Value::as_str).and_then(request_id) else {
        return Ok(AppError::coded(ErrorCode::ValidationInvalid, "request_id is the id secret_request returned").to_tool_value());
    };
    let vaults = super::my_vaults(&state.db, user).await?;
    let restricted = authz::restricted_vault()?;
    let found = get_request(&state.db, &vaults, &id).await?.filter(|r| r.requested_by == *user && restricted.as_ref().is_none_or(|v| *v == r.vault));
    let Some(r) = found else {
        return Ok(AppError::not_found("No such secret request of yours.").to_tool_value());
    };
    let status = r.effective_status().to_string();
    let mut v = json!({"request_id": r.id.to_string(), "status": status, "vault_id": r.vault.to_string(), "expires_at": r.expires_at, "decided_at": r.decided_at});
    match status.as_str() {
        "filled" => {
            v["handle"] = held::handle_value(&r.handle());
            v["next"] = json!("Use handle in http_request or secret_transform. The value is never shown to you.");
        }
        "pending" => {
            let (url, _) = fill_url(&state.settings, &r.id);
            v["fill_url"] = json!(url);
            v["next"] = json!("Still waiting for the user. Remind them to open fill_url; do not ask for the secret in chat.");
        }
        _ => v["next"] = json!("The user did not fill it. Ask them before making a new request."),
    }
    Ok(v)
}

// ---------------------------------------------------------------- the pop-up's routes

/// A request as the pop-up and the Connectors page show it. Never a value.
#[derive(Serialize, utoipa::ToSchema)]
pub struct RequestOut {
    /// `secret_request:...`
    pub id: String,
    pub vault_id: String,
    pub name: String,
    pub handle: String,
    /// Written by the agent.
    pub label: String,
    /// Written by the agent.
    pub description: String,
    pub target_hosts: Vec<String>,
    pub placements: Vec<String>,
    pub connector: Option<String>,
    pub requested_by: String,
    /// `token`, `oauth` or `user`.
    pub actor_kind: String,
    pub actor_id: String,
    pub client_name: String,
    /// `rest`, `mcp` or `chat`.
    pub channel: String,
    pub session: String,
    /// `pending`, `filled`, `declined` or `expired`.
    pub status: String,
    pub replace: bool,
    pub secret_ttl_days: Option<i64>,
    #[schema(value_type = String)]
    pub expires_at: Datetime,
    #[schema(value_type = String)]
    pub created_at: Datetime,
    #[schema(value_type = Option<String>)]
    pub decided_at: Option<Datetime>,
    pub fill_url: String,
    /// The signed-in person may paste the value (they asked, it is pending).
    pub can_fill: bool,
    pub can_decline: bool,
}

fn out(state: &OrgState, r: RequestRow, user: &RecordId, manages: bool) -> RequestOut {
    let status = r.effective_status().to_string();
    let pending = status == "pending";
    let mine = r.requested_by == *user;
    let (fill_url, _) = fill_url(&state.settings, &r.id);
    RequestOut {
        id: r.id.to_string(),
        vault_id: r.vault.to_string(),
        handle: r.handle(),
        name: r.name,
        label: r.label,
        description: r.description,
        target_hosts: r.target_hosts,
        placements: r.placements,
        connector: r.connector,
        requested_by: r.requested_by.to_string(),
        actor_kind: r.actor_kind,
        actor_id: r.actor_id,
        client_name: r.client_name,
        channel: r.channel,
        session: r.session,
        status,
        replace: r.replace,
        secret_ttl_days: r.secret_ttl_days,
        expires_at: r.expires_at,
        created_at: r.created_at,
        decided_at: r.decided_at,
        fill_url,
        can_fill: pending && mine,
        can_decline: pending && (mine || manages),
    }
}

/// Whether `user` may see `r`: they asked, or they manage its vault.
async fn visible(db: &OrgDb, user: &RecordId, org_manager: bool, r: &RequestRow) -> AppResult<Option<bool>> {
    if r.requested_by == *user {
        return Ok(Some(manages_vault(db, user, org_manager, &r.vault).await?));
    }
    Ok(manages_vault(db, user, org_manager, &r.vault).await?.then_some(true))
}

pub async fn list(state: &OrgState, user: &RecordId, org_manager: bool, status: Option<String>) -> AppResult<Vec<RequestOut>> {
    let vaults = super::my_vaults(&state.db, user).await?;
    expire(&state.db, &vaults).await?;
    let rows: Vec<RequestRow> = q::REQ_LIST.on(&state.db).bind(("vaults", vaults)).bind(("status", status)).await?.check()?.take(0)?;
    let mut outv = Vec::new();
    for r in rows {
        if let Some(manages) = visible(&state.db, user, org_manager, &r).await? {
            outv.push(out(state, r, user, manages));
        }
    }
    Ok(outv)
}

pub async fn get(state: &OrgState, user: &RecordId, org_manager: bool, raw_id: &str) -> AppResult<RequestOut> {
    let id = request_id(raw_id).ok_or_else(|| AppError::not_found("Secret request not found."))?;
    let vaults = super::my_vaults(&state.db, user).await?;
    expire(&state.db, &vaults).await?;
    let r = get_request(&state.db, &vaults, &id).await?.ok_or_else(|| AppError::not_found("Secret request not found."))?;
    let manages = visible(&state.db, user, org_manager, &r).await?.ok_or_else(|| AppError::not_found("Secret request not found."))?;
    Ok(out(state, r, user, manages))
}

/// What the pop-up sends. `value` is zeroised when this is dropped; Debug never prints it.
#[derive(Deserialize, utoipa::ToSchema)]
pub struct FillBody {
    /// The secret. Write-only: encrypted at once, never returned, logged or shown again.
    #[schema(value_type = String)]
    pub value: Zeroizing<String>,
    /// The hosts it may be sent to (defaults to what the agent asked for).
    #[serde(default)]
    pub allowed_hosts: Option<Vec<String>>,
    /// `header`, `query`, `body` (defaults to what the agent asked for).
    #[serde(default)]
    pub placements: Option<Vec<String>>,
    /// Let any credential of a vault member use it, not only the one that asked.
    #[serde(default)]
    pub any_agent: bool,
}

impl std::fmt::Debug for FillBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FillBody").field("value", &"[secret]").field("allowed_hosts", &self.allowed_hosts).finish()
    }
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct FillOut {
    pub status: String,
    /// `eu://<vault>/<name>`
    pub handle: String,
    pub secret_id: String,
}

/// The pop-up's Save: browser session, the person who asked, a pending request.
pub async fn fill(state: &OrgState, user: &RecordId, org_manager: bool, raw_id: &str, body: FillBody) -> AppResult<FillOut> {
    authz::require_session()?;
    let id = request_id(raw_id).ok_or_else(|| AppError::not_found("Secret request not found."))?;
    let vaults = super::my_vaults(&state.db, user).await?;
    expire(&state.db, &vaults).await?;
    let r = get_request(&state.db, &vaults, &id).await?.ok_or_else(|| AppError::not_found("Secret request not found."))?;
    if r.requested_by != *user {
        // the MCP rule: whoever completes the request is whoever it was for (and no co-member fills another's slot)
        return Err(AppError::coded(ErrorCode::AuthForbidden, "Only the person whose agent asked can fill this request."));
    }
    if r.effective_status() != "pending" {
        return Err(AppError::coded(ErrorCode::SecretRequestClosed, format!("This request is {} and can no longer be filled.", r.effective_status())));
    }
    let value = held::check_value(&body.value)?;
    let hosts = held::check_hosts(body.allowed_hosts.as_deref().unwrap_or(&r.target_hosts))?;
    let placements = held::check_placements(body.placements.as_deref().unwrap_or(&r.placements))?;
    let placements = if placements.is_empty() { vec!["header".to_string()] } else { placements };
    let actors = if body.any_agent || r.actor_id.is_empty() { Vec::new() } else { vec![r.actor_id.clone()] };

    let existing = held::by_name(&state.db, &vaults, &r.vault, &r.name).await?;
    if let Some(e) = &existing {
        if !r.replace {
            return Err(AppError::coded(ErrorCode::DbDuplicate, format!("{} already exists; the agent did not ask to replace it.", e.handle())));
        }
        if e.created_by != *user && !manages_vault(&state.db, user, org_manager, &r.vault).await? {
            return Err(AppError::coded(ErrorCode::AuthForbidden, "Only whoever filled that secret or an owner of the vault may replace it."));
        }
    }

    let claimed: Vec<RequestRow> = q::REQ_CLAIM.on(&state.db).bind(("id", id.clone())).bind(("decider", user.clone())).bind(("vaults", vaults.clone())).await?.check()?.take(0)?;
    if claimed.is_empty() {
        return Err(AppError::coded(ErrorCode::SecretRequestClosed, "This request was answered or expired just now."));
    }
    let mut cascade = 0;
    if let Some(e) = &existing {
        cascade = held::delete_with_descendants(&state.db, &vaults, std::slice::from_ref(&e.id)).await?;
    }
    let made = held::create(
        &state.db,
        &state.settings.encryption_key,
        &vaults,
        held::NewHeld {
            vault: r.vault.clone(),
            name: r.name.clone(),
            label: r.label.clone(),
            value: &value,
            hosts: hosts.clone(),
            placements: placements.clone(),
            tools: held::DEFAULT_TOOLS.iter().map(|t| t.to_string()).collect(),
            actors,
            derived_from: Vec::new(),
            op: None,
            request: Some(id.clone()),
            creator: user.clone(),
            expires_at: r.secret_ttl_days.map(|d| Datetime::from(chrono::Utc::now() + chrono::TimeDelta::days(d))),
        },
    )
    .await;
    drop(value);
    let made = match made {
        Ok(m) => m,
        Err(e) => {
            let _ = q::REQ_UNCLAIM.on(&state.db).bind(("id", id.clone())).bind(("vaults", vaults.clone())).await;
            return Err(e);
        }
    };
    q::REQ_LINK.on(&state.db).bind(("id", id.clone())).bind(("secret", made.id.clone())).bind(("vaults", vaults)).await?.check()?;
    let detail = json!({"request": id.to_string(), "secret": made.id.to_string(), "handle": made.handle(), "hosts": hosts, "placements": placements, "any_agent": body.any_agent, "replaced": existing.is_some(), "cascade": cascade}).to_string();
    audit(state, user, "secret.fill", "ok", &detail).await;
    Ok(FillOut { status: "filled".into(), handle: made.handle(), secret_id: made.id.to_string() })
}

/// The pop-up's Decline: the person who asked, an owner of the vault, an org owner or admin.
pub async fn decline(state: &OrgState, user: &RecordId, org_manager: bool, raw_id: &str) -> AppResult<String> {
    authz::require_session()?;
    let id = request_id(raw_id).ok_or_else(|| AppError::not_found("Secret request not found."))?;
    let vaults = super::my_vaults(&state.db, user).await?;
    expire(&state.db, &vaults).await?;
    let r = get_request(&state.db, &vaults, &id).await?.ok_or_else(|| AppError::not_found("Secret request not found."))?;
    if r.requested_by != *user && !manages_vault(&state.db, user, org_manager, &r.vault).await? {
        return Err(AppError::not_found("Secret request not found."));
    }
    let rows: Vec<RequestRow> = q::REQ_DECLINE.on(&state.db).bind(("id", id.clone())).bind(("decider", user.clone())).bind(("vaults", vaults)).await?.check()?.take(0)?;
    if rows.is_empty() {
        return Err(AppError::coded(ErrorCode::SecretRequestClosed, format!("This request is {} already.", r.effective_status())));
    }
    audit(state, user, "secret.decline", "ok", &json!({"request": id.to_string(), "vault": r.vault.to_string()}).to_string()).await;
    Ok("declined".into())
}

// ---------------------------------------------------------------- held secrets

/// A held secret as listed. Never its value or ciphertext.
#[derive(Serialize, utoipa::ToSchema)]
pub struct HeldOut {
    /// `vault_secret:...`
    pub id: String,
    pub handle: String,
    pub vault_id: String,
    pub name: String,
    pub label: String,
    pub allowed_hosts: Vec<String>,
    pub placements: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub allowed_actors: Vec<String>,
    /// The held secrets it was derived from (empty for a pasted one).
    pub derived_from: Vec<String>,
    pub op: Option<String>,
    pub created_by: String,
    #[schema(value_type = String)]
    pub created_at: Datetime,
    #[schema(value_type = String)]
    pub updated_at: Datetime,
    #[schema(value_type = Option<String>)]
    pub expires_at: Option<Datetime>,
    pub can_manage: bool,
}

fn held_out(m: HeldMeta, can_manage: bool) -> HeldOut {
    HeldOut {
        id: m.id.to_string(),
        handle: m.handle(),
        vault_id: m.vault.to_string(),
        name: m.name,
        label: m.label,
        allowed_hosts: m.allowed_hosts,
        placements: m.placements,
        allowed_tools: m.allowed_tools,
        allowed_actors: m.allowed_actors,
        derived_from: m.derived_from.iter().map(|r| r.to_string()).collect(),
        op: m.op,
        created_by: m.created_by.to_string(),
        created_at: m.created_at,
        updated_at: m.updated_at,
        expires_at: m.expires_at,
        can_manage,
    }
}

async fn may_manage(db: &OrgDb, user: &RecordId, org_manager: bool, m: &HeldMeta) -> AppResult<bool> {
    Ok(m.created_by == *user || manages_vault(db, user, org_manager, &m.vault).await?)
}

pub async fn list_held(state: &OrgState, user: &RecordId, org_manager: bool) -> AppResult<Vec<HeldOut>> {
    let vaults = super::my_vaults(&state.db, user).await?;
    let mut outv = Vec::new();
    for m in held::list(&state.db, &vaults).await? {
        if may_manage(&state.db, user, org_manager, &m).await? {
            outv.push(held_out(m, true));
        }
    }
    Ok(outv)
}

fn held_id(raw: &str) -> AppResult<RecordId> {
    crate::rid::parse(raw.trim()).ok().filter(|r: &RecordId| r.table() == "vault_secret").ok_or_else(|| AppError::not_found("Secret not found."))
}

async fn managed(state: &OrgState, user: &RecordId, org_manager: bool, raw_id: &str) -> AppResult<(HeldMeta, Vec<RecordId>)> {
    let id = held_id(raw_id)?;
    let vaults = super::my_vaults(&state.db, user).await?;
    let m = held::get(&state.db, &vaults, &id).await?.ok_or_else(|| AppError::not_found("Secret not found."))?;
    if !may_manage(&state.db, user, org_manager, &m).await? {
        return Err(AppError::not_found("Secret not found."));
    }
    Ok((m, vaults))
}

/// A new value (browser session). Secrets derived from the old value are deleted; agents re-derive.
pub async fn replace_held(state: &OrgState, user: &RecordId, org_manager: bool, raw_id: &str, value: &Zeroizing<String>) -> AppResult<usize> {
    authz::require_session()?;
    let (m, vaults) = managed(state, user, org_manager, raw_id).await?;
    if !m.derived_from.is_empty() {
        return Err(AppError::bad_request("A derived secret is made by a transform: replace the secret it came from, or delete it."));
    }
    let value = held::check_value(value)?;
    let cascade = held::replace(&state.db, &state.settings.encryption_key, &vaults, &m.id, &value).await?;
    drop(value);
    audit(state, user, "secret.replace", "ok", &json!({"secret": m.id.to_string(), "handle": m.handle(), "cascade": cascade}).to_string()).await;
    Ok(cascade)
}

/// Deletes a held secret and everything derived from it (browser session).
pub async fn delete_held(state: &OrgState, user: &RecordId, org_manager: bool, raw_id: &str) -> AppResult<usize> {
    authz::require_session()?;
    let (m, vaults) = managed(state, user, org_manager, raw_id).await?;
    let cascade = held::delete_with_descendants(&state.db, &vaults, std::slice::from_ref(&m.id)).await?;
    audit(state, user, "secret.delete", "ok", &json!({"secret": m.id.to_string(), "handle": m.handle(), "cascade": cascade}).to_string()).await;
    Ok(cascade)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_take_the_key_or_the_full_id() {
        assert_eq!(request_id("abc").map(|r| r.to_string()).as_deref(), Some("secret_request:abc"));
        assert_eq!(request_id("secret_request:abc").map(|r| r.to_string()).as_deref(), Some("secret_request:abc"));
        assert!(request_id("vault:abc").is_none());
    }

    #[test]
    fn the_fill_url_carries_no_token_and_flags_plain_http() {
        let id = RecordId::new("secret_request", "k3abc");
        assert_eq!(fill_url_at("https://eunomia.example.com", &id), ("https://eunomia.example.com/secrets/requests/k3abc".into(), false));
        assert!(!fill_url_at("http://localhost:3000", &id).1);
        assert!(fill_url_at("http://10.0.0.5:3000", &id).1);
    }

    #[test]
    fn the_fill_body_never_debug_prints_the_value() {
        let b: FillBody = serde_json::from_value(json!({"value": "pasted-canary-value"})).unwrap();
        assert!(!format!("{b:?}").contains("pasted-canary-value"));
    }
}
