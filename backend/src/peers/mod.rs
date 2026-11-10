//! Peer vaults (docs/peer-vaults.md, issues #108 and #134): one vault shared between two separate
//! Eunomia instances, each keeping a full copy, kept in step over an encrypted, authenticated exchange
//! of the vault's change log (`changes.rs`).
//!
//! * The owner makes a one-time **share code** ([`create_share`]): the instance's addresses, a share
//!   id and a 256-bit secret. Only the secret's ciphertext is kept, wiped when the code is used.
//! * The other instance **joins** with it ([`join`]): it finds the first address that answers as the
//!   right instance, then both prove they know the secret (HMAC) and derive the link key from it and
//!   two fresh nonces. The joiner gets a new vault, the copy, and syncs at once.
//! * From then on the guest (the joiner) calls the host every ~30 s with what changed on its side and
//!   gets back what changed on the host's (`sync.rs`). Guests always call hosts, so only the host has
//!   to be reachable; a guest can host the copy onward to others, which makes a tree.
//!
//! `crypto.rs` holds the primitives, `addrs.rs` the address list, `net.rs` the HTTP calls.

pub mod addrs;
pub mod crypto;
pub mod net;
pub mod sync;

use serde::{Deserialize, Serialize};
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use crate::authz::{self, Action};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::state::OrgState;
use crate::store;
use crate::store::peers as q;

/// What every share code starts with.
pub const CODE_PREFIX: &str = "eunomia1:";
/// A code works this long unless the owner picks otherwise.
pub const DEFAULT_EXPIRY_HOURS: i64 = 24 * 7;
pub const MAX_EXPIRY_HOURS: i64 = 24 * 30;
pub const ROLE_EDIT: &str = "edit";
pub const ROLE_READ: &str = "read";
pub const HOST: &str = "host";
pub const GUEST: &str = "guest";

/// Status words a link has (the UI turns them into sentences).
pub mod status {
    pub const UP_TO_DATE: &str = "up_to_date";
    pub const SYNCING: &str = "syncing";
    pub const UNREACHABLE: &str = "unreachable";
    pub const INTEGRITY_PROBLEM: &str = "integrity_problem";
    pub const REVOKED: &str = "revoked";
}

/// The decoded body of a share code.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodeBody {
    pub v: u32,
    /// The host's org key: the peer routes are per org.
    pub org: String,
    pub share: String,
    /// The 32-byte secret, base64url.
    pub secret: String,
    /// The host's instance id, so a guest knows it reached the right instance.
    pub inst: String,
    pub vault: String,
    pub role: String,
    pub addrs: Vec<String>,
}

pub fn encode_code(body: &CodeBody) -> String {
    format!("{CODE_PREFIX}{}", crypto::b64(serde_json::to_string(body).expect("code serializes").as_bytes()))
}

pub fn decode_code(code: &str) -> AppResult<CodeBody> {
    let bad = || AppError::bad_request("That doesn't look like a share code. Copy the whole code, starting with eunomia1:");
    // tolerate the line breaks and spaces a chat app adds
    let compact: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    let rest = compact.strip_prefix(CODE_PREFIX).ok_or_else(bad)?;
    let raw = crypto::unb64(rest).ok_or_else(bad)?;
    let body: CodeBody = serde_json::from_slice(&raw).map_err(|_| bad())?;
    if body.v != 1 {
        return Err(AppError::bad_request("This share code was made by a newer Eunomia. Update this Eunomia and try again."));
    }
    if crypto::unb64(&body.secret).is_none_or(|s| s.len() != 32) || body.share.is_empty() || body.inst.is_empty() {
        return Err(bad());
    }
    Ok(body)
}

/// This org database's instance id (made on first use).
pub async fn instance_id(db: &OrgDb) -> AppResult<String> {
    let mut res = q::INSTANCE.on(db).bind(("new", crypto::random_id())).await?;
    let id: Option<String> = res.take(0)?;
    id.ok_or_else(|| AppError::internal("instance id not stored"))
}

/// The first 8 hex digits of the instance id, grouped: what a person compares by eye.
pub fn fingerprint(instance: &str) -> String {
    let s: String = instance.chars().take(8).collect();
    if s.len() == 8 { format!("{}-{}", &s[..4], &s[4..]) } else { s }
}

/// How this instance introduces itself (`EUNOMIA_INSTANCE_NAME`, else empty: the other side then
/// names it after the address it reached).
pub fn instance_name() -> String {
    std::env::var("EUNOMIA_INSTANCE_NAME").unwrap_or_default().trim().chars().take(80).collect()
}

// --- rows --------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, SurrealValue)]
pub struct LinkRow {
    pub id: RecordId,
    pub vault: RecordId,
    pub direction: String,
    pub role: String,
    pub key_enc: String,
    pub user: RecordId,
    pub peer_instance: String,
    #[serde(default)]
    #[surreal(default)]
    pub peer_name: String,
    #[serde(default)]
    #[surreal(default)]
    pub peer_org: String,
    #[serde(default)]
    #[surreal(default)]
    pub peer_addrs: Vec<String>,
    #[serde(default)]
    #[surreal(default)]
    pub addr: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    pub out_cursor: i64,
    #[serde(default)]
    #[surreal(default)]
    pub in_cursor: i64,
    #[serde(default)]
    #[surreal(default)]
    pub status: String,
    #[serde(default)]
    #[surreal(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    pub last_attempt_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    pub last_sync_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    pub integrity_failures: i64,
    #[serde(default)]
    #[surreal(default)]
    pub local_digest: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    pub remote_digest: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    pub digests_match: Option<bool>,
    #[serde(default)]
    #[surreal(default)]
    pub last_report: Option<serde_json::Value>,
}

impl LinkRow {
    /// The link id both sides share.
    pub fn key(&self) -> String {
        crate::rid::key_string(self.id.key()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Deserialize, SurrealValue)]
struct ShareRow {
    id: RecordId,
    vault: RecordId,
    role: String,
    #[serde(default)]
    #[surreal(default)]
    secret_enc: Option<String>,
    created_by: RecordId,
    expires_at: Datetime,
    #[serde(default)]
    #[surreal(default)]
    used_at: Option<Datetime>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct VaultName {
    #[serde(default)]
    #[surreal(default)]
    name: String,
}

pub fn link_rid(key: &str) -> RecordId {
    RecordId::from_table_key("peer_link", key.to_string())
}

fn share_rid(key: &str) -> RecordId {
    RecordId::from_table_key("peer_share", key.to_string())
}

/// A link id as given by a caller: 32 hex digits.
pub fn valid_key(key: &str) -> bool {
    key.len() == 32 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

pub async fn load_link(db: &OrgDb, key: &str) -> AppResult<Option<LinkRow>> {
    if !valid_key(key) {
        return Ok(None);
    }
    Ok(store::get(db, &link_rid(key)).await?)
}

/// The link key, decrypted.
pub fn link_secret(settings: &crate::config::Settings, row: &LinkRow) -> AppResult<[u8; 32]> {
    let hexed = crate::connectors::crypto::decrypt_exact(&settings.encryption_key, &row.key_enc)?;
    let raw = hex::decode(hexed.trim()).map_err(|_| AppError::internal("link key is not hex"))?;
    raw.try_into().map_err(|_| AppError::internal("link key has the wrong length"))
}

pub(crate) async fn vault_name(db: &OrgDb, vault: &RecordId) -> AppResult<Option<String>> {
    let row: Option<VaultName> = store::get(db, vault).await?;
    Ok(row.map(|v| v.name))
}

fn ts(d: &Option<Datetime>) -> Option<String> {
    d.map(|d| d.into_inner().to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

// --- share codes ------------------------------------------------------------------------------

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ShareCodeOut {
    pub id: String,
    /// Send this to the person running the other Eunomia. It works once.
    pub code: String,
    pub role: String,
    pub expires_at: String,
    /// The addresses in the code, best first.
    pub addresses: Vec<String>,
    /// This instance's fingerprint, to compare by eye.
    pub fingerprint: String,
}

/// A one-time code for `vault` (owner only). `addrs` are this instance's addresses
/// ([`addrs::advertised`] for a real server).
pub async fn create_share(state: &OrgState, user: &RecordId, vault: &RecordId, role: &str, hours: Option<i64>, addrs: Vec<String>) -> AppResult<ShareCodeOut> {
    authz::authorize(&state.db, user, Action::Invite, vault).await?;
    if role != ROLE_EDIT && role != ROLE_READ {
        return Err(AppError::bad_request("role must be \"edit\" or \"read\""));
    }
    if addrs.is_empty() {
        return Err(AppError::bad_request(
            "Eunomia couldn't work out how another computer would reach this one. Open Eunomia using this computer's network address (for example http://192.168.1.20:3000) instead of localhost, then try again. Running Eunomia on a server? See docs/peer-vaults.md.",
        ));
    }
    let hours = hours.unwrap_or(DEFAULT_EXPIRY_HOURS).clamp(1, MAX_EXPIRY_HOURS);
    let name = vault_name(&state.db, vault).await?.ok_or_else(|| AppError::coded(ErrorCode::VaultNotFound, "Vault not found."))?;
    let secret = crypto::random_bytes::<32>();
    let expires = chrono::Utc::now() + chrono::Duration::hours(hours);
    let key = crypto::random_id();
    let enc = crate::connectors::crypto::encrypt(&state.settings.encryption_key, &hex::encode(secret));
    let id = share_rid(&key);
    crate::tx::with_retry(|| async {
        q::CREATE_SHARE
            .on(&state.db)
            .bind(("id", id.clone()))
            .bind(("vault", vault.clone()))
            .bind(("role", role.to_string()))
            .bind(("secret_enc", enc.clone()))
            .bind(("user", user.clone()))
            .bind(("expires_at", Datetime::from(expires)))
            .await?
            .check()
    })
    .await?;
    let inst = instance_id(&state.db).await?;
    let body = CodeBody { v: 1, org: state.db.org().key(), share: key.clone(), secret: crypto::b64(&secret), inst: inst.clone(), vault: name, role: role.to_string(), addrs: addrs.clone() };
    Ok(ShareCodeOut { id: key, code: encode_code(&body), role: role.to_string(), expires_at: expires.to_rfc3339_opts(chrono::SecondsFormat::Secs, true), addresses: addrs, fingerprint: fingerprint(&inst) })
}

/// Withdraws an unused code (owner only).
pub async fn delete_share(state: &OrgState, user: &RecordId, vault: &RecordId, share: &str) -> AppResult<bool> {
    authz::authorize(&state.db, user, Action::Invite, vault).await?;
    if !valid_key(share) {
        return Ok(false);
    }
    let mut res = q::DELETE_SHARE.on(&state.db).bind(("id", share_rid(share))).bind(("vault", vault.clone())).await?;
    let gone: Vec<RecordId> = res.take("id")?;
    Ok(!gone.is_empty())
}

// --- the host side of a join -------------------------------------------------------------------

/// `POST /api/peer/v1/{org}/join`, unauthenticated: the MAC over the request is the credential.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinRequest {
    pub share: String,
    pub guest_instance: String,
    #[serde(default)]
    pub guest_org: String,
    #[serde(default)]
    pub guest_name: String,
    #[serde(default)]
    pub guest_addrs: Vec<String>,
    pub nonce_g: String,
    pub mac: String,
}

impl JoinRequest {
    fn parts(&self) -> Vec<String> {
        vec!["join".into(), self.share.clone(), self.guest_instance.clone(), self.guest_org.clone(), self.guest_name.clone(), self.guest_addrs.join(","), self.nonce_g.clone()]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinResponse {
    pub link: String,
    pub nonce_h: String,
    pub vault_name: String,
    pub role: String,
    pub host_instance: String,
    #[serde(default)]
    pub host_name: String,
    pub mac: String,
}

impl JoinResponse {
    fn parts(&self, req: &JoinRequest) -> Vec<String> {
        vec!["joined".into(), req.share.clone(), self.link.clone(), req.nonce_g.clone(), self.nonce_h.clone(), self.vault_name.clone(), self.role.clone(), self.host_instance.clone(), self.host_name.clone()]
    }
}

fn refs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

/// One message for every refusal: a caller without the secret learns nothing about the share.
fn not_valid() -> AppError {
    AppError::coded(ErrorCode::AuthForbidden, "This share code isn't valid any more. Ask for a new one.")
}

/// Host-only reasons. These never enter the peer protocol or the public error.
#[derive(Clone, Copy)]
pub(crate) enum JoinRefusal {
    Expired,
    Used,
    Malformed,
}

pub(crate) async fn refuse_join(state: &OrgState, user: Option<&RecordId>, reason: JoinRefusal) -> AppError {
    // Unknown codes still belong in this host workspace's log. Do not trust the claimed guest
    // identity, and do not put anonymous rows in every workspace's audit feed.
    #[derive(serde::Deserialize, surrealdb::types::SurrealValue)]
    struct Owner { user: RecordId }
    let owner = if user.is_none() {
        match store::control::ORG_FIRST_OWNER.on(&state.control).bind(("org", state.db.org().record())).await {
            Ok(mut res) => res.take::<Vec<Owner>>(0).ok().and_then(|rows| rows.into_iter().next()).map(|row| row.user),
            Err(_) => None,
        }
    } else { None };
    if let Some(user) = user.or(owner.as_ref()) {
        let detail = match reason {
            JoinRefusal::Expired => "why=expired",
            JoinRefusal::Used => "why=used",
            JoinRefusal::Malformed => "why=malformed",
        };
        crate::audit::record(&state.control, crate::audit::Event {
            user: Some(user), actor: &crate::audit::anonymous(), action: "peer.join", target: "",
            outcome: ErrorCode::AuthForbidden.as_str(), detail,
        }).await;
    }
    not_valid()
}

pub async fn accept_join(state: &OrgState, req: JoinRequest) -> AppResult<JoinResponse> {
    if !valid_key(&req.share) || req.nonce_g.len() > 64 || req.guest_name.len() > 200 || req.guest_instance.len() > 64 || req.guest_org.len() > 64 {
        return Err(refuse_join(state, None, JoinRefusal::Malformed).await);
    }
    let share: Option<ShareRow> = store::get(&state.db, &share_rid(&req.share)).await?;
    let Some(share) = share else { return Err(refuse_join(state, None, JoinRefusal::Malformed).await) };
    if share.used_at.is_some() {
        return Err(refuse_join(state, Some(&share.created_by), JoinRefusal::Used).await);
    }
    if share.expires_at.into_inner() <= chrono::Utc::now() {
        return Err(refuse_join(state, Some(&share.created_by), JoinRefusal::Expired).await);
    }
    let Some(enc) = share.secret_enc.as_deref() else { return Err(refuse_join(state, Some(&share.created_by), JoinRefusal::Malformed).await) };
    let secret = hex::decode(crate::connectors::crypto::decrypt_exact(&state.settings.encryption_key, enc)?.trim()).map_err(|_| AppError::internal("share secret is not hex"))?;
    if !crypto::verify_mac(&secret, &refs(&req.parts()), &req.mac) {
        return Err(refuse_join(state, Some(&share.created_by), JoinRefusal::Malformed).await);
    }
    let link = crypto::random_id();
    // only one join wins a code
    let mut res = q::CLAIM_SHARE.on(&state.db).bind(("id", share.id.clone())).bind(("link", link.clone())).await?;
    let claimed: Vec<RecordId> = res.take("id")?;
    if claimed.is_empty() {
        // Another request may have claimed the code, or it expired during verification.
        let current: Option<ShareRow> = store::get(&state.db, &share.id).await?;
        let reason = if current.as_ref().is_some_and(|row| row.used_at.is_some()) { JoinRefusal::Used }
            else if current.as_ref().is_some_and(|row| row.expires_at.into_inner() <= chrono::Utc::now()) { JoinRefusal::Expired }
            else { JoinRefusal::Malformed };
        return Err(refuse_join(state, Some(&share.created_by), reason).await);
    }
    let nonce_h = crypto::random_id();
    let key = crypto::link_key(&secret, &link, &req.nonce_g, &nonce_h);
    let vault_name = vault_name(&state.db, &share.vault).await?.ok_or_else(not_valid)?;
    let host_instance = instance_id(&state.db).await?;
    q::CREATE_LINK
        .on(&state.db)
        .bind(("id", link_rid(&link)))
        .bind(("vault", share.vault.clone()))
        .bind(("direction", HOST))
        .bind(("role", share.role.clone()))
        .bind(("key_enc", crate::connectors::crypto::encrypt(&state.settings.encryption_key, &hex::encode(key))))
        .bind(("user", share.created_by.clone()))
        .bind(("peer_instance", req.guest_instance.clone()))
        .bind(("peer_name", req.guest_name.clone()))
        .bind(("peer_org", req.guest_org.clone()))
        .bind(("peer_addrs", addrs::sanitize(&req.guest_addrs)))
        .bind(("addr", None::<String>))
        .await?
        .check()?;
    let guest = if req.guest_name.trim().is_empty() { "another Eunomia" } else { req.guest_name.trim() };
    audit_from_peer(state, &share.created_by, &req.guest_instance, "peer.joined", &vault_name, &format!("role={} from={guest}", share.role)).await;
    let mut out = JoinResponse { link, nonce_h, vault_name, role: share.role, host_instance, host_name: instance_name(), mac: String::new() };
    out.mac = crypto::mac(&secret, &refs(&out.parts(&req)));
    Ok(out)
}

/// A ledger row for something another Eunomia did here (redeemed a code, left a link): it counts
/// against `user`, who made the code, so it shows in that workspace's log; the actor is the peer.
pub(crate) async fn audit_from_peer(state: &OrgState, user: &RecordId, peer_instance: &str, action: &str, space: &str, detail: &str) {
    let actor = authz::Actor { kind: "peer", id: peer_instance.to_string() };
    crate::audit::record(&state.control, crate::audit::Event { user: Some(user), actor: &actor, action, target: space, outcome: "ok", detail }).await;
}

// --- the guest side of a join ------------------------------------------------------------------

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct JoinOut {
    /// The new vault holding this Eunomia's copy.
    pub vault: crate::vaults::service::VaultOut,
    pub link: PeerOut,
}

/// The first of `addrs` that answers `hello` as `instance`.
pub async fn probe(addrs: &[String], org: &str, instance: &str) -> Option<String> {
    for addr in addrs {
        let url = format!("{addr}/api/peer/v1/{org}/hello");
        if let Ok(reply) = net::get(&url, net::PROBE_TIMEOUT).await
            && reply.status == 200
            && reply.json().is_some_and(|v| v["instance_id"].as_str() == Some(instance))
        {
            return Some(addr.clone());
        }
    }
    None
}

/// "base", or "base 2", "base 3", ...: a vault name `user` does not have yet.
async fn free_vault_name(db: &OrgDb, user: &RecordId, base: &str) -> AppResult<String> {
    let mine: Vec<String> = crate::vaults::service::list_my_vaults(db, user).await?.into_iter().map(|v| v.name.to_lowercase()).collect();
    let base: String = base.trim().chars().take(70).collect();
    let base = if base.is_empty() { "Shared space".to_string() } else { base };
    let mut name = base.clone();
    let mut n = 2;
    while mine.contains(&name.to_lowercase()) {
        name = format!("{base} {n}");
        n += 1;
    }
    Ok(name)
}

/// Joins the vault a share code names: a new vault here becomes a copy of it, synced from then on.
/// `my_addrs` are this instance's addresses (sent along, for a later handover); `name` is how the
/// host will see this instance.
pub async fn join(state: &OrgState, user: &RecordId, code: &str, my_addrs: Vec<String>, name: &str) -> AppResult<JoinOut> {
    authz::require_unrestricted()?;
    let body = decode_code(code)?;
    let me = instance_id(&state.db).await?;
    if body.inst == me {
        return Err(AppError::bad_request("This share code was made on this same Eunomia. Use it on the other Eunomia instead."));
    }
    let candidates = addrs::sanitize(&body.addrs);
    let Some(addr) = probe(&candidates, &body.org, &body.inst).await else {
        return Err(AppError::new(
            axum::http::StatusCode::BAD_GATEWAY,
            "Couldn't reach the other Eunomia at any of the addresses in the code. Check that it is running and on the same network, then try again.",
        ));
    };
    let secret = crypto::unb64(&body.secret).ok_or_else(|| AppError::bad_request("bad share code"))?;
    let mut req = JoinRequest {
        share: body.share.clone(),
        guest_instance: me,
        guest_org: state.db.org().key(),
        guest_name: name.chars().take(200).collect(),
        guest_addrs: my_addrs,
        nonce_g: crypto::random_id(),
        mac: String::new(),
    };
    req.mac = crypto::mac(&secret, &refs(&req.parts()));
    let url = format!("{addr}/api/peer/v1/{}/join", body.org);
    let reply = net::post(&url, serde_json::to_string(&req).expect("join serializes"), net::CALL_TIMEOUT)
        .await
        .map_err(|e| AppError::new(axum::http::StatusCode::BAD_GATEWAY, "The other Eunomia stopped answering while joining. Try again.").with_source(e))?;
    if reply.status == 403 {
        return Err(AppError::bad_request("The other Eunomia says this code isn't valid any more (it works once and expires). Ask for a new one."));
    }
    if reply.status != 200 {
        return Err(AppError::new(axum::http::StatusCode::BAD_GATEWAY, format!("The other Eunomia refused to join (HTTP {}). Try again.", reply.status)));
    }
    let resp: JoinResponse = serde_json::from_slice(&reply.body).map_err(|_| AppError::new(axum::http::StatusCode::BAD_GATEWAY, "The other Eunomia sent an answer this one can't read."))?;
    // the host proves it knows the secret too: an address answering in its place cannot
    if resp.host_instance != body.inst || !valid_key(&resp.link) || !crypto::verify_mac(&secret, &refs(&resp.parts(&req)), &resp.mac) {
        return Err(AppError::new(axum::http::StatusCode::BAD_GATEWAY, "The answer didn't prove it came from the Eunomia that made the code, so nothing was joined."));
    }
    if resp.role != ROLE_EDIT && resp.role != ROLE_READ {
        return Err(AppError::new(axum::http::StatusCode::BAD_GATEWAY, "The other Eunomia sent an unknown role."));
    }
    let key = crypto::link_key(&secret, &resp.link, &req.nonce_g, &resp.nonce_h);
    let vault_label = if resp.vault_name.trim().is_empty() { body.vault.clone() } else { resp.vault_name.clone() };
    let vname = free_vault_name(&state.db, user, &vault_label).await?;
    let vault = crate::vaults::service::create_vault(&state.db, &state.control, user, &vname, "org").await?;
    let vault_id = crate::rid::parse(&vault.id).map_err(|_| AppError::internal("vault id did not round-trip"))?;
    let peer_name = if resp.host_name.trim().is_empty() {
        url::Url::parse(&addr).ok().and_then(|u| u.host_str().map(|h| format!("Eunomia at {h}"))).unwrap_or_else(|| "The other Eunomia".into())
    } else {
        resp.host_name.clone()
    };
    let mut peer_addrs = vec![addr.clone()];
    peer_addrs.extend(candidates.into_iter().filter(|a| a != &addr));
    q::CREATE_LINK
        .on(&state.db)
        .bind(("id", link_rid(&resp.link)))
        .bind(("vault", vault_id.clone()))
        .bind(("direction", GUEST))
        .bind(("role", resp.role.clone()))
        .bind(("key_enc", crate::connectors::crypto::encrypt(&state.settings.encryption_key, &hex::encode(key))))
        .bind(("user", user.clone()))
        .bind(("peer_instance", body.inst.clone()))
        .bind(("peer_name", peer_name))
        .bind(("peer_org", body.org.clone()))
        .bind(("peer_addrs", peer_addrs))
        .bind(("addr", Some(addr)))
        .await?
        .check()?;
    // the first copy; a failure here only means the scheduler finishes it
    if let Err(e) = sync::sync_link(state, &resp.link).await {
        tracing::warn!(link = %resp.link, error = %e.message, "first peer sync failed; it retries");
    }
    let row = load_link(&state.db, &resp.link).await?.ok_or_else(|| AppError::internal("link vanished"))?;
    Ok(JoinOut { vault, link: peer_out(&row) })
}

// --- listing and stopping ----------------------------------------------------------------------

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PeerOut {
    /// The link id.
    pub id: String,
    /// `host`: this Eunomia holds the original and shares it out; `guest`: this is a copy of another's.
    pub direction: String,
    /// What the guest may do: `edit` or `read`.
    pub role: String,
    pub peer_name: String,
    /// The other Eunomia's fingerprint.
    pub peer_fingerprint: String,
    /// `up_to_date`, `syncing`, `unreachable`, `integrity_problem` or `revoked`.
    pub status: String,
    pub last_sync_at: Option<String>,
    /// The address that answered last (a guest's view of its host).
    pub address: Option<String>,
    /// Whether both copies held the same items at the last check (`None`: not checked yet).
    pub digests_match: Option<bool>,
    pub integrity_failures: i64,
    pub last_error: Option<String>,
    /// Counts from the last exchange.
    #[schema(value_type = Option<Object>)]
    pub last_report: Option<serde_json::Value>,
}

/// A host has not heard from a guest for this long: its status reads unreachable.
const HOST_QUIET_SECS: i64 = 180;

fn peer_out(row: &LinkRow) -> PeerOut {
    let key = row.key();
    let status = if row.direction == HOST && row.status != status::REVOKED && row.status != status::INTEGRITY_PROBLEM {
        let quiet = row.last_sync_at.is_none_or(|t| (chrono::Utc::now() - t.into_inner()).num_seconds() > HOST_QUIET_SECS);
        if quiet { status::UNREACHABLE } else { status::UP_TO_DATE }.to_string()
    } else if sync::in_progress(&key) {
        status::SYNCING.to_string()
    } else {
        row.status.clone()
    };
    PeerOut {
        id: key,
        direction: row.direction.clone(),
        role: row.role.clone(),
        peer_name: row.peer_name.clone(),
        peer_fingerprint: fingerprint(&row.peer_instance),
        status,
        last_sync_at: ts(&row.last_sync_at),
        address: row.addr.clone(),
        digests_match: row.digests_match,
        integrity_failures: row.integrity_failures,
        last_error: row.last_error.clone(),
        last_report: row.last_report.clone(),
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PendingCodeOut {
    pub id: String,
    pub role: String,
    pub created_at: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PeersOut {
    pub vault: String,
    /// This instance's fingerprint.
    pub fingerprint: String,
    /// Other Eunomias this vault is shared with (or copied from).
    pub links: Vec<PeerOut>,
    /// Codes made for this vault and not used yet (owners only; empty for members).
    pub codes: Vec<PendingCodeOut>,
    /// The addresses a new code would carry, best first.
    pub addresses: Vec<String>,
}

pub async fn links_of_vault(db: &OrgDb, vault: &RecordId) -> AppResult<Vec<LinkRow>> {
    let mut res = q::LINKS_OF_VAULT.on(db).bind(("vault", vault.clone())).await?;
    Ok(res.take(0)?)
}

/// The vault's peers and pending codes (any member reads them; codes only for owners).
pub async fn list(state: &OrgState, user: &RecordId, vault: &RecordId, addrs: Vec<String>) -> AppResult<PeersOut> {
    let scope = authz::authorize(&state.db, user, Action::ReadMemories, vault).await?;
    let links = links_of_vault(&state.db, vault).await?;
    // a host forgets a guest it revoked once the guest was told; until then it is not shown
    let links = links.iter().filter(|l| !(l.direction == HOST && l.status == status::REVOKED)).map(peer_out).collect();
    let codes = if scope.role() == authz::Role::Owner {
        #[derive(Deserialize, SurrealValue)]
        struct Row {
            id: RecordId,
            role: String,
            #[serde(default)]
            #[surreal(default)]
            created_at: Option<Datetime>,
            #[serde(default)]
            #[surreal(default)]
            expires_at: Option<Datetime>,
        }
        let mut res = q::SHARES_OF_VAULT.on(&state.db).bind(("vault", vault.clone())).await?;
        let rows: Vec<Row> = res.take(0)?;
        rows.into_iter()
            .map(|r| PendingCodeOut { id: crate::rid::key_string(r.id.key()).unwrap_or_default(), role: r.role, created_at: ts(&r.created_at), expires_at: ts(&r.expires_at) })
            .collect()
    } else {
        Vec::new()
    };
    Ok(PeersOut { vault: vault.to_string(), fingerprint: fingerprint(&instance_id(&state.db).await?), links, codes, addresses: addrs })
}

/// A link `user` may see (a member of its vault) and, with `owner`, change.
async fn link_for(state: &OrgState, user: &RecordId, key: &str, action: Action) -> AppResult<LinkRow> {
    let row = load_link(&state.db, key).await?.ok_or_else(|| AppError::not_found("No such shared space link."))?;
    authz::authorize(&state.db, user, action, &row.vault).await?;
    Ok(row)
}

/// Stops sharing over one link (owner only). A guest tells its host (best effort) and keeps its copy
/// as an ordinary vault; a host revokes the guest, which learns it on its next exchange and keeps its
/// copy too.
pub async fn stop(state: &OrgState, user: &RecordId, key: &str) -> AppResult<()> {
    let row = link_for(state, user, key, Action::Invite).await?;
    if row.direction == GUEST {
        if row.status != status::REVOKED {
            let _ = sync::call(state, &row, serde_json::json!({ "op": "leave" })).await;
        }
        forget(&state.db, &row).await
    } else {
        Ok(q::SET_STATUS.on(&state.db).bind(("id", row.id.clone())).bind(("status", status::REVOKED)).bind(("last_error", None::<String>)).await?.check().map(|_| ())?)
    }
}

/// Deletes a link and its item map.
pub async fn forget(db: &OrgDb, row: &LinkRow) -> AppResult<()> {
    q::DELETE_LINK.on(db).bind(("key", row.key())).bind(("id", row.id.clone())).await?.check()?;
    Ok(())
}

/// "Sync now": runs a guest's exchange at once (a host just reports: its guest calls in).
pub async fn sync_now(state: &OrgState, user: &RecordId, key: &str) -> AppResult<PeerOut> {
    let row = link_for(state, user, key, Action::ReadMemories).await?;
    if row.direction == GUEST {
        sync::sync_link(state, key).await?;
    }
    let row = load_link(&state.db, key).await?.ok_or_else(|| AppError::not_found("No such shared space link."))?;
    Ok(peer_out(&row))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VerifyOut {
    pub link: String,
    pub local_digest: String,
    pub local_items: usize,
    /// The other side's digest: asked just now (guest), or as of its last exchange (host).
    pub remote_digest: Option<String>,
    /// True when both copies hold exactly the same items.
    pub matches: Option<bool>,
}

/// Recomputes this side's digest, and the other side's when this is the guest.
pub async fn verify(state: &OrgState, user: &RecordId, key: &str) -> AppResult<VerifyOut> {
    let row = link_for(state, user, key, Action::ReadMemories).await?;
    let (local, items) = sync::digest(&state.db, &row.vault).await?;
    let remote = if row.direction == GUEST {
        let (v, _) = sync::call(state, &row, serde_json::json!({ "op": "verify" })).await.map_err(|e| e.into_app())?;
        v["digest"].as_str().map(str::to_string)
    } else {
        row.remote_digest.clone()
    };
    let matches = remote.as_ref().map(|r| r == &local);
    q::SAVE_PROGRESS
        .on(&state.db)
        .bind(("id", row.id.clone()))
        .bind(("out_cursor", row.out_cursor))
        .bind(("in_cursor", row.in_cursor))
        .bind(("addr", row.addr.clone()))
        .bind(("status", if row.status.is_empty() { status::SYNCING.to_string() } else { row.status.clone() }))
        .bind(("last_error", row.last_error.clone()))
        .bind(("ok", false))
        .bind(("integrity", 0))
        .bind(("local_digest", Some(local.clone())))
        .bind(("remote_digest", remote.clone()))
        .bind(("digests_match", matches))
        .bind(("report", row.last_report.clone()))
        .await?
        .check()?;
    Ok(VerifyOut { link: row.key(), local_digest: local, local_items: items, remote_digest: remote, matches })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_codes_round_trip() {
        let body = CodeBody {
            v: 1,
            org: "abc".into(),
            share: crypto::random_id(),
            secret: crypto::b64(&crypto::random_bytes::<32>()),
            inst: crypto::random_id(),
            vault: "Project Atlas".into(),
            role: "edit".into(),
            addrs: vec!["http://192.168.1.4:3000".into()],
        };
        let code = encode_code(&body);
        assert!(code.starts_with(CODE_PREFIX));
        assert_eq!(decode_code(&code).unwrap(), body);
        // a chat app's line breaks are fine
        let wrapped: String = code.chars().enumerate().flat_map(|(i, c)| if i % 20 == 19 { vec![c, '\n'] } else { vec![c] }).collect();
        assert_eq!(decode_code(&wrapped).unwrap(), body);
        assert!(decode_code("eunomia1:nope").is_err());
        assert!(decode_code("hello").is_err());
    }

    #[test]
    fn fingerprints_are_short() {
        assert_eq!(fingerprint("0123456789abcdef"), "0123-4567");
    }
}
