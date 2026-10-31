//! The credential broker (docs/architecture/specs/broker.md). Agents name a secret held in the org's own
//! password manager (`{"$secret": "op://Ops/Square/access_token"}`); [`broker::prepare`] resolves it through
//! the `secrets-helper` sidecar for one tool execution, and the value never enters what the call leaves
//! behind. Eunomia stores references ([`CredentialRef`]) and, per provider kind, the bootstrap token as
//! `enc:v1:` ciphertext ([`ProviderRow`]); never a secret value.

pub mod broker;
pub mod client;
pub mod reference;

use serde::{Deserialize, Serialize};
use surrealdb::types::{Datetime, RecordId, SurrealValue};
use zeroize::Zeroizing;

use crate::error::{AppError, AppResult, ErrorCode};
use crate::pool::OrgDb;
use crate::store::secrets as q;

/// The provider kinds, as stored and as the sidecar names them.
pub const KINDS: &[&str] = &["onepassword_connect", "onepassword_service_account", "bitwarden_sm"];

/// The longest bootstrap token accepted (a 1Password service account token is about 1 KB).
pub const MAX_TOKEN: usize = 4096;

pub fn valid_kind(kind: &str) -> AppResult<&'static str> {
    KINDS.iter().copied().find(|k| *k == kind).ok_or_else(|| AppError::bad_request(format!("the provider kind is one of {}", KINDS.join(", "))))
}

/// Which provider kinds can resolve a reference with this scheme.
pub fn kind_fits(kind: &str, scheme: &str) -> bool {
    matches!((kind, scheme), ("onepassword_connect" | "onepassword_service_account", "op") | ("bitwarden_sm", "bws"))
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct CredentialRef {
    pub id: RecordId,
    pub reference: String,
    pub provider: String,
    pub vault: RecordId,
    pub label: String,
    pub allowed_tools: Vec<String>,
    pub allowed_actors: Vec<String>,
    pub created_by: RecordId,
    pub created_at: Datetime,
    pub updated_at: Datetime,
}

/// A provider's settings without its token.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct ProviderRow {
    pub kind: String,
    #[serde(default)]
    #[surreal(default)]
    pub host: Option<String>,
    pub created_by: RecordId,
    pub created_at: Datetime,
    pub updated_by: RecordId,
    pub updated_at: Datetime,
}

pub fn provider_id(kind: &str) -> RecordId {
    RecordId::new("secret_provider", kind.to_string())
}

pub async fn providers(db: &OrgDb) -> AppResult<Vec<ProviderRow>> {
    Ok(q::PROVIDER_LIST.on(db).await?.check()?.take(0)?)
}

/// The decrypted settings the sidecar needs, or `None` when the kind is not configured.
pub async fn provider_config(db: &OrgDb, key: &str, kind: &str) -> AppResult<Option<client::ProviderConfig>> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        kind: String,
        #[serde(default)]
        #[surreal(default)]
        host: Option<String>,
        token_enc: String,
    }
    let rows: Vec<Row> = q::PROVIDER_TOKEN.on(db).bind(("id", provider_id(kind))).await?.check()?.take(0)?;
    let Some(row) = rows.into_iter().next() else { return Ok(None) };
    let token = Zeroizing::new(crate::connectors::crypto::decrypt(key, &row.token_enc)?);
    Ok(Some(client::ProviderConfig { kind: row.kind, token, host: row.host }))
}

/// A token as an admin typed it: trimmed, non-empty, short, no control characters.
pub fn check_token(token: &str) -> AppResult<&str> {
    let t = token.trim();
    if t.is_empty() || t.len() > MAX_TOKEN || t.chars().any(char::is_control) {
        return Err(AppError::bad_request(format!("the token is 1 to {MAX_TOKEN} characters with no line breaks")));
    }
    Ok(t)
}

/// An http(s) URL with a host, or `None` for an empty string.
pub fn check_host(host: Option<&str>) -> AppResult<Option<String>> {
    let Some(h) = host.map(str::trim).filter(|h| !h.is_empty()) else { return Ok(None) };
    let url = reqwest::Url::parse(h).map_err(|_| AppError::bad_request("the host is a URL such as https://connect.example.com"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::bad_request("the host is an http(s) URL with no user or password in it"));
    }
    Ok(Some(h.trim_end_matches('/').to_string()))
}

/// Saves a provider. `token`: `Some` replaces it (encrypted here, never stored in the clear); `None` keeps
/// the stored one, and fails when there is none.
pub async fn put_provider(db: &OrgDb, key: &str, user: &RecordId, kind: &str, host: Option<String>, token: Option<&str>) -> AppResult<()> {
    let kind = valid_kind(kind)?;
    if kind == "onepassword_connect" && host.is_none() {
        return Err(AppError::bad_request("a 1Password Connect provider needs the Connect server's URL"));
    }
    if kind == "onepassword_service_account" && host.is_some() {
        return Err(AppError::bad_request("a 1Password service account has no host"));
    }
    match token {
        Some(t) => {
            let token = Zeroizing::new(check_token(t)?.to_string());
            let enc = crate::connectors::crypto::encrypt(key, &token);
            q::PROVIDER_PUT
                .on(db)
                .bind(("id", provider_id(kind)))
                .bind(("kind", kind.to_string()))
                .bind(("host", host))
                .bind(("token_enc", enc))
                .bind(("user", user.clone()))
                .await?
                .check()?;
        }
        None => {
            let ids: Vec<RecordId> =
                q::PROVIDER_PUT_KEEP_TOKEN.on(db).bind(("id", provider_id(kind))).bind(("host", host)).bind(("user", user.clone())).await?.check()?.take(0)?;
            if ids.is_empty() {
                return Err(AppError::bad_request("enter the token: none is stored for this provider yet"));
            }
        }
    }
    Ok(())
}

pub async fn delete_provider(db: &OrgDb, kind: &str) -> AppResult<bool> {
    let kind = valid_kind(kind)?;
    #[derive(Deserialize, SurrealValue)]
    struct Gone {
        #[allow(dead_code)]
        kind: String,
    }
    let gone: Vec<Gone> = q::PROVIDER_DELETE.on(db).bind(("id", provider_id(kind))).await?.check()?.take(0)?;
    Ok(!gone.is_empty())
}

pub async fn refs(db: &OrgDb) -> AppResult<Vec<CredentialRef>> {
    Ok(q::REF_LIST.on(db).await?.check()?.take(0)?)
}

pub async fn refs_named(db: &OrgDb, reference: &str) -> AppResult<Vec<CredentialRef>> {
    Ok(q::REF_BY_REFERENCE.on(db).bind(("reference", reference.to_string())).await?.check()?.take(0)?)
}

pub async fn get_ref(db: &OrgDb, id: &RecordId) -> AppResult<Option<CredentialRef>> {
    Ok(q::REF_GET.on(db).bind(("id", id.clone())).await?.check()?.take::<Vec<CredentialRef>>(0)?.into_iter().next())
}

/// The editable part of a reference.
pub struct RefFields {
    pub vault: RecordId,
    pub label: String,
    pub allowed_tools: Vec<String>,
    pub allowed_actors: Vec<String>,
}

fn clean_list(list: Vec<String>, what: &str, ok: impl Fn(&str) -> bool) -> AppResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for item in list.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        if item.len() > 200 || !ok(&item) {
            return Err(AppError::bad_request(format!("{item:?} is not a valid {what}")));
        }
        if !out.contains(&item) {
            out.push(item);
        }
    }
    if out.len() > 100 {
        return Err(AppError::bad_request(format!("at most 100 {what}s")));
    }
    Ok(out)
}

pub fn check_fields(mut f: RefFields) -> AppResult<RefFields> {
    f.label = f.label.trim().chars().take(120).collect();
    f.allowed_tools = clean_list(f.allowed_tools, "tool name", |t| t == "*" || t.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))?;
    f.allowed_actors = clean_list(f.allowed_actors, "actor id", |a| {
        a.split_once(':').is_some_and(|(t, k)| matches!(t, "user" | "api_token" | "oauth_client" | "oauth") && !k.is_empty())
    })?;
    Ok(f)
}

/// Registers a reference. The text must parse and fit the provider kind; it is stored normalised.
pub async fn create_ref(db: &OrgDb, user: &RecordId, raw: &str, provider: &str, fields: RefFields) -> AppResult<CredentialRef> {
    let provider = valid_kind(provider)?;
    let parsed = reference::parse(raw.trim()).map_err(|m| AppError::bad_request(m.0))?;
    if !kind_fits(provider, parsed.scheme()) {
        return Err(AppError::bad_request(format!("a {}:// reference does not belong to {provider}", parsed.scheme())));
    }
    let f = check_fields(fields)?;
    let label = if f.label.is_empty() { parsed.text() } else { f.label };
    let rows: Vec<CredentialRef> = q::REF_CREATE
        .on(db)
        .bind(("reference", parsed.text()))
        .bind(("provider", provider.to_string()))
        .bind(("vault", f.vault))
        .bind(("label", label))
        .bind(("allowed_tools", f.allowed_tools))
        .bind(("allowed_actors", f.allowed_actors))
        .bind(("user", user.clone()))
        .await
        .map_err(|e| duplicate(e.into()))?
        .check()
        .map_err(|e| duplicate(e.into()))?
        .take(0)?;
    rows.into_iter().next().ok_or_else(|| AppError::internal("credential_ref create returned no row"))
}

fn duplicate(e: AppError) -> AppError {
    if e.code == ErrorCode::DbDuplicate { AppError::coded(ErrorCode::DbDuplicate, "That reference is already registered for this vault.") } else { e }
}

pub async fn update_ref(db: &OrgDb, id: &RecordId, fields: RefFields) -> AppResult<Option<CredentialRef>> {
    let f = check_fields(fields)?;
    let current = get_ref(db, id).await?;
    let Some(current) = current else { return Ok(None) };
    let label = if f.label.is_empty() { current.reference.clone() } else { f.label };
    let rows: Vec<CredentialRef> = q::REF_UPDATE
        .on(db)
        .bind(("id", id.clone()))
        .bind(("vault", f.vault))
        .bind(("label", label))
        .bind(("allowed_tools", f.allowed_tools))
        .bind(("allowed_actors", f.allowed_actors))
        .await
        .map_err(|e| duplicate(e.into()))?
        .check()
        .map_err(|e| duplicate(e.into()))?
        .take(0)?;
    Ok(rows.into_iter().next())
}

pub async fn delete_ref(db: &OrgDb, id: &RecordId) -> AppResult<bool> {
    let gone: Vec<CredentialRef> = q::REF_DELETE.on(db).bind(("id", id.clone())).await?.check()?.take(0)?;
    Ok(!gone.is_empty())
}
