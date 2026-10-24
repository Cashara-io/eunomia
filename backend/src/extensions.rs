//! Extensions, phase 1 (docs/extensions.md): a registry of a user's own scripts, each with a webhook
//! inbox that buffers deliveries until the script claims them in batches. The script runs wherever
//! the user runs it and talks to Eunomia over HTTP; nothing here executes user code.
//!
//! This file is the service: secrets and signatures, the buffer (deliver, claim, ack, nack) and
//! housekeeping. The HTTP surface is `routers/extensions.rs`.

use std::collections::HashMap;

use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use crate::connectors::crypto;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::store;

/// Largest delivery body kept (JSON text). Bigger is refused with 413 before any database work.
pub const BODY_MAX_BYTES: usize = 256 * 1024;
/// Default for waiting plus leased events per extension; `EXTENSION_BUFFER_MAX` overrides it.
pub const BUFFER_MAX: i64 = 10_000;
/// Times an event is handed out before it is dead-lettered.
pub const MAX_ATTEMPTS: i64 = 5;
/// Most events one claim may take, and the most ids one ack or nack may name.
pub const CLAIM_MAX: i64 = 500;
/// A claim stops adding events once their bodies add up to this many bytes (it always takes one).
pub const CLAIM_BYTES: i64 = 8 << 20;
pub const LEASE_DEFAULT_SECS: i64 = 300;
pub const LEASE_MIN_SECS: i64 = 5;
pub const LEASE_MAX_SECS: i64 = 3600;
/// Retention of finished and dead events.
pub const KEEP_DONE: &str = "7d";
pub const KEEP_DEAD: &str = "30d";

pub fn buffer_max() -> i64 {
    std::env::var("EXTENSION_BUFFER_MAX").ok().and_then(|v| v.trim().parse().ok()).filter(|n: &i64| *n > 0).unwrap_or(BUFFER_MAX)
}

#[derive(Debug, Clone, Deserialize, SurrealValue)]
pub struct Extension {
    pub id: RecordId,
    pub owner: RecordId,
    pub name: String,
    #[serde(default)]
    #[surreal(default)]
    pub description: String,
    #[serde(default)]
    #[surreal(default)]
    pub vault: Option<RecordId>,
    pub enabled: bool,
    pub secret_enc: String,
    #[serde(default)]
    #[surreal(default)]
    pub secret_hint: String,
    pub secret_rotated_at: Datetime,
    pub created_at: Datetime,
    pub updated_at: Datetime,
}

impl Extension {
    /// The id used in the webhook URL: the record key, which starts with the owner's key.
    pub fn hook_key(&self) -> String {
        crate::rid::key_string(self.id.key()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Deserialize, SurrealValue)]
pub struct Event {
    pub id: RecordId,
    pub delivery_id: String,
    pub body: String,
    pub status: String,
    pub attempts: i64,
    #[serde(default)]
    #[surreal(default)]
    pub claimed_until: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    pub last_error: Option<String>,
    pub received_at: Datetime,
}

#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub pending: i64,
    pub claimed: i64,
    pub done: i64,
    pub dead: i64,
}

// ---------------------------------------------------------------- secrets

/// A new secret: 32 random bytes, base64url, with a prefix that says what it is.
pub fn new_secret() -> String {
    format!("exs_{}", crate::models_user::generate_token())
}

fn hint(secret: &str) -> String {
    let n = secret.chars().count();
    secret.chars().skip(n.saturating_sub(4)).collect()
}

/// Compares two byte strings in time that does not depend on where they differ: both are hashed to
/// a fixed length first.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let (a, b) = (Sha256::digest(a), Sha256::digest(b));
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Whether a delivery proves it knows `secret`: either `X-Eunomia-Signature: sha256=<hex>`, an
/// HMAC-SHA256 of the raw body keyed with the secret, or `Authorization: Bearer <secret>`. A
/// signature header that is present decides; a bad one is not rescued by a good bearer.
pub fn authentic(secret: &str, headers: &HeaderMap, body: &[u8]) -> bool {
    if let Some(sig) = headers.get("x-eunomia-signature") {
        let Some(hex_sig) = sig.to_str().ok().and_then(|s| s.trim().strip_prefix("sha256=")) else { return false };
        let Ok(bytes) = hex::decode(hex_sig.trim()) else { return false };
        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else { return false };
        mac.update(body);
        return mac.verify_slice(&bytes).is_ok(); // constant time
    }
    crate::auth::bearer_token(headers).is_some_and(|t| ct_eq(t.as_bytes(), secret.as_bytes()))
}

/// The signature a sender computes (used by the tests and the docs' example).
pub fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key length");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

// ---------------------------------------------------------------- ids

/// A new extension id. The key is `{owner key}_{16 random lowercase letters and digits}`, so the
/// public webhook URL (`/hooks/ext/{key}`) names the owner, which is how the unauthenticated route
/// finds the org database to look in (the same reason a source webhook carries the owner id).
fn new_ext_id(owner: &RecordId) -> AppResult<RecordId> {
    use rand::Rng;
    let owner_key = crate::rid::key_string(owner.key()).ok_or_else(|| AppError::internal("user id has no string key"))?;
    let rand: String = (0..16).map(|_| "abcdefghijklmnopqrstuvwxyz0123456789".as_bytes()[rand::thread_rng().gen_range(0..36)] as char).collect();
    Ok(RecordId::new("extension", format!("{owner_key}_{rand}")))
}

/// The owner and extension a webhook URL key names, or `None` for anything that is not one of ours.
pub fn parse_hook_key(key: &str) -> Option<(RecordId, RecordId)> {
    if key.len() > 96 || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let (owner_key, rand) = key.rsplit_once('_')?;
    if owner_key.is_empty() || rand.len() != 16 {
        return None;
    }
    Some((RecordId::new("user", owner_key.to_string()), RecordId::new("extension", key.to_string())))
}

/// An id from a client: `extension:key` (or just the key). It must belong to `owner`'s key space.
pub fn parse_id(s: &str, owner: &RecordId) -> Option<RecordId> {
    let rid: RecordId = crate::rid::parse(s).ok().filter(|r: &RecordId| r.table() == "extension")?;
    let key = crate::rid::key_string(rid.key())?;
    let (o, _) = parse_hook_key(&key)?;
    (o == *owner).then_some(rid)
}

fn event_id(ext: &RecordId, delivery_id: &str) -> RecordId {
    RecordId::new("extension_event", crate::tx::stable_key('e', &format!("{}\n{delivery_id}", ext.to_string())))
}

// ---------------------------------------------------------------- registry

fn check_text(name: &str, description: &str) -> AppResult<()> {
    if name.is_empty() || name.chars().count() > 80 {
        return Err(AppError::bad_request("Give the extension a name of 1 to 80 characters."));
    }
    if description.chars().count() > 500 {
        return Err(AppError::bad_request("The description is limited to 500 characters."));
    }
    Ok(())
}

/// Creates an extension. Returns the row and the secret, which exists in the clear only here.
pub async fn create(
    db: &OrgDb,
    encryption_key: &str,
    owner: &RecordId,
    name: &str,
    description: &str,
    vault: Option<RecordId>,
) -> AppResult<(Extension, String)> {
    let (name, description) = (name.trim(), description.trim());
    check_text(name, description)?;
    let secret = new_secret();
    let mut res = store::extensions::CREATE
        .on(db)
        .bind(("id", new_ext_id(owner)?))
        .bind(("owner", owner.clone()))
        .bind(("name", name.to_string()))
        .bind(("description", description.to_string()))
        .bind(("vault", vault))
        .bind(("secret_enc", crypto::encrypt(encryption_key, &secret)))
        .bind(("secret_hint", hint(&secret)))
        .await?;
    let row: Option<Extension> = res.take(0)?;
    Ok((row.ok_or_else(|| AppError::internal("extension was not created"))?, secret))
}

fn not_found() -> AppError {
    AppError::coded(ErrorCode::ExtensionNotFound, "Extension not found.")
}

/// An extension of this owner's. Anyone else's is `extension.not_found`, like a missing one.
pub async fn get(db: &OrgDb, owner: &RecordId, id: &RecordId) -> AppResult<Extension> {
    let mut res = store::extensions::GET.on(db).bind(("id", id.clone())).bind(("owner", owner.clone())).await?;
    res.take::<Vec<Extension>>(0)?.into_iter().next().ok_or_else(not_found)
}

pub async fn list(db: &OrgDb, owner: &RecordId) -> AppResult<Vec<Extension>> {
    let mut res = store::extensions::LIST.on(db).bind(("owner", owner.clone())).await?;
    Ok(res.take(0)?)
}

pub async fn update(db: &OrgDb, owner: &RecordId, current: &Extension, name: &str, description: &str, vault: Option<RecordId>, enabled: bool) -> AppResult<Extension> {
    let (name, description) = (name.trim(), description.trim());
    check_text(name, description)?;
    let mut res = store::extensions::UPDATE
        .on(db)
        .bind(("id", current.id.clone()))
        .bind(("owner", owner.clone()))
        .bind(("name", name.to_string()))
        .bind(("description", description.to_string()))
        .bind(("vault", vault))
        .bind(("enabled", enabled))
        .await?;
    res.take::<Vec<Extension>>(0)?.into_iter().next().ok_or_else(not_found)
}

/// A new secret; the old one stops working at once. Returns the row and the new secret.
pub async fn rotate(db: &OrgDb, encryption_key: &str, owner: &RecordId, id: &RecordId) -> AppResult<(Extension, String)> {
    get(db, owner, id).await?;
    let secret = new_secret();
    let mut res = store::extensions::ROTATE
        .on(db)
        .bind(("id", id.clone()))
        .bind(("owner", owner.clone()))
        .bind(("secret_enc", crypto::encrypt(encryption_key, &secret)))
        .bind(("secret_hint", hint(&secret)))
        .await?;
    let row = res.take::<Vec<Extension>>(0)?.into_iter().next().ok_or_else(not_found)?;
    Ok((row, secret))
}

/// Deletes the extension and every buffered event.
pub async fn delete(db: &OrgDb, owner: &RecordId, id: &RecordId) -> AppResult<()> {
    store::extensions::DELETE_EVENTS.on(db).bind(("id", id.clone())).bind(("owner", owner.clone())).await?;
    #[derive(Deserialize, SurrealValue)]
    struct Gone {
        #[allow(dead_code)]
        id: RecordId,
    }
    let mut res = store::extensions::DELETE.on(db).bind(("id", id.clone())).bind(("owner", owner.clone())).await?;
    if res.take::<Vec<Gone>>(0)?.is_empty() {
        return Err(not_found());
    }
    Ok(())
}

#[derive(Deserialize, SurrealValue)]
struct CountRow {
    extension: RecordId,
    status: String,
    #[serde(default)]
    #[surreal(default)]
    lapsed: bool,
    n: i64,
}

/// Event counts per extension (by id string) for one owner. A leased event whose lease ran out is
/// counted as pending, which is what the next claim will treat it as.
pub async fn stats(db: &OrgDb, owner: &RecordId) -> AppResult<HashMap<String, Stats>> {
    let mut res = store::extensions::COUNTS.on(db).bind(("owner", owner.clone())).await?;
    let mut out: HashMap<String, Stats> = HashMap::new();
    for row in res.take::<Vec<CountRow>>(0)? {
        let s = out.entry(row.extension.to_string()).or_default();
        match (row.status.as_str(), row.lapsed) {
            ("pending", _) | ("claimed", true) => s.pending += row.n,
            ("claimed", false) => s.claimed += row.n,
            ("done", _) => s.done += row.n,
            ("dead", _) => s.dead += row.n,
            _ => {}
        }
    }
    Ok(out)
}

pub async fn last_delivery(db: &OrgDb, owner: &RecordId, id: &RecordId) -> AppResult<Option<Datetime>> {
    let mut res = store::extensions::LAST_DELIVERY.on(db).bind(("ext", id.clone())).bind(("owner", owner.clone())).await?;
    Ok(res.take::<Vec<Datetime>>(0)?.into_iter().next())
}

// ---------------------------------------------------------------- the buffer

/// The secret an extension currently accepts (decrypted for the length of one verification).
pub fn secret_of(encryption_key: &str, ext: &Extension) -> AppResult<String> {
    crypto::decrypt(encryption_key, &ext.secret_enc)
}

pub enum Delivery {
    Accepted(RecordId),
    /// The same delivery id arrived before; nothing was stored.
    Duplicate(RecordId),
}

/// Appends one delivery to the buffer. `body` is the raw JSON text. A delivery id seen before is a
/// no-op (senders retry); a full buffer is `extension.buffer_full`. The bound is checked, then the
/// row inserted, without a lock between them: deliveries racing at the edge can overshoot it by a few.
pub async fn deliver(db: &OrgDb, ext: &Extension, delivery_id: &str, body: &str) -> AppResult<Delivery> {
    let mut res = store::extensions::OPEN_COUNT.on(db).bind(("ext", ext.id.clone())).bind(("owner", ext.owner.clone())).await?;
    #[derive(Deserialize, SurrealValue)]
    struct N {
        n: i64,
    }
    let open = res.take::<Vec<N>>(0)?.first().map_or(0, |r| r.n);
    if open >= buffer_max() {
        return Err(AppError::coded(
            ErrorCode::ExtensionBufferFull,
            format!("This extension's inbox holds {open} unprocessed events, its maximum. Retry after the consumer has acked some."),
        ));
    }
    let id = event_id(&ext.id, delivery_id);
    let mut res = store::extensions::INSERT_EVENT
        .on(db)
        .bind(("id", id.clone()))
        .bind(("ext", ext.id.clone()))
        .bind(("owner", ext.owner.clone()))
        .bind(("delivery", delivery_id.to_string()))
        .bind(("body", body.to_string()))
        .bind(("size", body.len() as i64))
        .await?;
    Ok(if res.take::<Vec<RecordId>>(0)?.is_empty() { Delivery::Duplicate(id) } else { Delivery::Accepted(id) })
}

#[derive(Deserialize, SurrealValue)]
struct Peeked {
    id: RecordId,
    #[serde(default)]
    #[surreal(default)]
    size: i64,
}

/// Leases up to `max` of the oldest claimable events to the caller for `lease_secs`. An event is
/// claimable when pending, or leased with the lease run out (its consumer crashed or was slow). Each
/// claim counts as an attempt; an event handed out [`MAX_ATTEMPTS`] times without an ack is dead.
/// Two consumers never hold the same event: the lease is taken by a conditional UPDATE in a
/// transaction, a lost race is a commit conflict that `tx::with_retry` repeats from the read, and
/// an in-process lock per extension covers the in-memory engine used by the tests.
pub async fn claim(db: &OrgDb, ext: &Extension, max: i64, lease_secs: i64) -> AppResult<Vec<Event>> {
    let max = max.clamp(1, CLAIM_MAX);
    let lease = lease_secs.clamp(LEASE_MIN_SECS, LEASE_MAX_SECS);
    let _fence = crate::tx::lock(&format!("extension.claim:{}", ext.id.to_string())).await;
    let mut rows = crate::tx::with_retry(|| async {
        let mut peek = store::extensions::PEEK
            .on(db)
            .bind(("ext", ext.id.clone()))
            .bind(("owner", ext.owner.clone()))
            .bind(("n", max))
            .await?;
        let mut budget = CLAIM_BYTES;
        let mut ids = Vec::new();
        for p in peek.take::<Vec<Peeked>>(0)? {
            if !ids.is_empty() && p.size > budget {
                break;
            }
            budget -= p.size;
            ids.push(p.id);
        }
        let mut res = store::extensions::CLAIM
            .on(db)
            .bind(("ext", ext.id.clone()))
            .bind(("owner", ext.owner.clone()))
            .bind(("ids", ids))
            .bind(("max_attempts", MAX_ATTEMPTS))
            .bind(("lease", format!("{lease}s")))
            .await?;
        res.take::<Vec<Event>>(store::extensions::CLAIM.slot)
    })
    .await?;
    rows.sort_by(|a, b| a.received_at.cmp(&b.received_at).then_with(|| a.id.to_string().cmp(&b.id.to_string())));
    Ok(rows)
}

/// Marks leased events done. Only ids that belong to this extension and are currently leased count;
/// the number acked is returned.
pub async fn ack(db: &OrgDb, ext: &Extension, ids: Vec<RecordId>) -> AppResult<usize> {
    let _fence = crate::tx::lock(&format!("extension.claim:{}", ext.id.to_string())).await;
    let done = crate::tx::with_retry(|| async {
        let mut res = store::extensions::ACK
            .on(db)
            .bind(("ext", ext.id.clone()))
            .bind(("owner", ext.owner.clone()))
            .bind(("ids", ids.clone()))
            .await?;
        res.take::<Vec<RecordId>>(0)
    })
    .await?;
    Ok(done.len())
}

#[derive(Deserialize, SurrealValue)]
struct Nacked {
    status: String,
}

/// Gives leased events back: pending again, or dead once they have been handed out [`MAX_ATTEMPTS`]
/// times. Returns `(requeued, dead)`.
pub async fn nack(db: &OrgDb, ext: &Extension, ids: Vec<RecordId>, error: Option<String>) -> AppResult<(usize, usize)> {
    let error = error.map(|e| e.chars().take(500).collect::<String>());
    let _fence = crate::tx::lock(&format!("extension.claim:{}", ext.id.to_string())).await;
    let rows = crate::tx::with_retry(|| async {
        let mut res = store::extensions::NACK
            .on(db)
            .bind(("ext", ext.id.clone()))
            .bind(("owner", ext.owner.clone()))
            .bind(("ids", ids.clone()))
            .bind(("max_attempts", MAX_ATTEMPTS))
            .bind(("error", error.clone()))
            .await?;
        res.take::<Vec<Nacked>>(0)
    })
    .await?;
    let dead = rows.iter().filter(|r| r.status == "dead").count();
    Ok((rows.len() - dead, dead))
}

/// A look at the buffer without leasing anything, newest first (for debugging and the dead letters).
pub async fn events(db: &OrgDb, ext: &Extension, status: Option<&str>, limit: i64) -> AppResult<Vec<Event>> {
    let mut res = store::extensions::LIST_EVENTS
        .on(db)
        .bind(("ext", ext.id.clone()))
        .bind(("owner", ext.owner.clone()))
        .bind(("status", status.map(String::from)))
        .bind(("limit", limit.clamp(1, 100)))
        .await?;
    Ok(res.take(0)?)
}

/// Drops finished events after [`KEEP_DONE`] and dead ones after [`KEEP_DEAD`], org-wide. Run by the
/// scheduler leader (`jobs/leader.rs`); idempotent.
pub async fn prune(db: &OrgDb) -> AppResult<()> {
    store::extensions::PRUNE.on(db).bind(("done_age", KEEP_DONE)).bind(("dead_age", KEEP_DEAD)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&'static str, String)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn bearer_and_signature_both_prove_the_secret() {
        let secret = "exs_secret";
        let body = br#"{"a":1}"#;
        assert!(authentic(secret, &headers(&[("authorization", format!("Bearer {secret}"))]), body));
        assert!(!authentic(secret, &headers(&[("authorization", "Bearer nope".into())]), body));
        assert!(authentic(secret, &headers(&[("x-eunomia-signature", sign(secret, body))]), body));
        // the signature covers the body
        assert!(!authentic(secret, &headers(&[("x-eunomia-signature", sign(secret, body))]), br#"{"a":2}"#));
        // a bad signature is not rescued by a good bearer
        let both = headers(&[("x-eunomia-signature", "sha256=00".into()), ("authorization", format!("Bearer {secret}"))]);
        assert!(!authentic(secret, &both, body));
        assert!(!authentic(secret, &headers(&[("x-eunomia-signature", "sha256=zz".into())]), body));
        assert!(!authentic(secret, &headers(&[("x-eunomia-signature", sign(secret, body).replace("sha256=", ""))]), body));
        assert!(!authentic(secret, &HeaderMap::new(), body));
    }

    #[test]
    fn hook_keys_round_trip_and_reject_junk() {
        let owner = RecordId::new("user", "abc123");
        let id = new_ext_id(&owner).unwrap();
        let key = crate::rid::key_string(id.key()).unwrap();
        assert_eq!(parse_hook_key(&key), Some((owner.clone(), id.clone())));
        assert_eq!(parse_id(&id.to_string(), &owner), Some(id));
        assert_eq!(parse_id(&format!("extension:{key}"), &RecordId::new("user", "other")), None);
        for bad in ["", "abc", "abc_def", "a b_0123456789abcdef", "../x_0123456789abcdef"] {
            assert_eq!(parse_hook_key(bad), None, "{bad}");
        }
    }

    #[test]
    fn secrets_are_long_and_distinct() {
        let (a, b) = (new_secret(), new_secret());
        assert!(a.starts_with("exs_") && a.len() > 40);
        assert_ne!(a, b);
        assert_eq!(hint("exs_abcdWXYZ"), "WXYZ");
    }

    #[test]
    fn event_ids_are_stable_per_delivery() {
        let ext = RecordId::new("extension", "u_0123456789abcdef");
        assert_eq!(event_id(&ext, "d1"), event_id(&ext, "d1"));
        assert_ne!(event_id(&ext, "d1"), event_id(&ext, "d2"));
    }
}
