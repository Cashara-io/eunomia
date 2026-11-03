//! Secrets Eunomia holds itself (docs/architecture/specs/secret-requests.md): a value a person pasted into
//! a secret request's pop-up, or one a transform derived from others. Stored only as `enc:v1:` ciphertext
//! (`connectors/crypto.rs`) in `vault_secret`, addressed by a handle `eu://<vault key>/<name>`, and never
//! returned by anything: [`for_use`] decrypts one for the broker's resolution or a transform, and nothing
//! else reads `value_enc`.

use serde::{Deserialize, Serialize};
use surrealdb::types::{Datetime, RecordId, SurrealValue};
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use zeroize::Zeroizing;

use crate::authz::Caller;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::store::secret_requests as q;

/// The owner-approved minimum. Empty or whitespace-only values remain invalid.
pub const MIN_LEN: usize = 1;
/// The longest value held (a PEM private key fits).
pub const MAX_LEN: usize = 16384;
/// The parts of an outbound request a handle may go in.
pub const PLACEMENTS: &[&str] = &["header", "query", "body"];
/// What a held secret may be used by unless the person narrows it.
pub const DEFAULT_TOOLS: &[&str] = &["http_request", "secret_transform"];
pub const MAX_HOSTS: usize = 20;

/// A held secret without its value.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct HeldMeta {
    pub id: RecordId,
    pub vault: RecordId,
    pub name: String,
    pub label: String,
    pub allowed_hosts: Vec<String>,
    pub placements: Vec<String>,
    pub allowed_tools: Vec<String>,
    pub allowed_actors: Vec<String>,
    pub derived_from: Vec<RecordId>,
    #[serde(default)]
    #[surreal(default)]
    pub op: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    pub request: Option<RecordId>,
    pub created_by: RecordId,
    pub created_at: Datetime,
    pub updated_at: Datetime,
    #[serde(default)]
    #[surreal(default)]
    pub expires_at: Option<Datetime>,
}

impl HeldMeta {
    pub fn handle(&self) -> String {
        handle_for(&self.vault, &self.name)
    }

    pub fn expired(&self) -> bool {
        self.expires_at.as_ref().is_some_and(|t| chrono::DateTime::<chrono::Utc>::from(*t) <= chrono::Utc::now())
    }
}

/// `eu://<vault key>/<name>`.
pub fn handle_for(vault: &RecordId, name: &str) -> String {
    super::reference::handle(&vault_key(vault), name)
}

pub fn vault_key(vault: &RecordId) -> String {
    crate::rid::key_string(vault.key()).unwrap_or_default()
}

/// The handle object agents put in tool arguments.
pub fn handle_value(handle: &str) -> serde_json::Value {
    serde_json::json!({ super::reference::MARKER: handle })
}

/// [`HeldMeta`] plus the ciphertext (only [`for_use`] reads it).
#[derive(Deserialize, SurrealValue)]
struct ForUse {
    id: RecordId,
    vault: RecordId,
    name: String,
    label: String,
    allowed_hosts: Vec<String>,
    placements: Vec<String>,
    allowed_tools: Vec<String>,
    allowed_actors: Vec<String>,
    derived_from: Vec<RecordId>,
    #[serde(default)]
    #[surreal(default)]
    op: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    request: Option<RecordId>,
    created_by: RecordId,
    created_at: Datetime,
    updated_at: Datetime,
    #[serde(default)]
    #[surreal(default)]
    expires_at: Option<Datetime>,
    value_enc: String,
}

impl ForUse {
    fn split(self) -> (HeldMeta, String) {
        let meta = HeldMeta {
            id: self.id,
            vault: self.vault,
            name: self.name,
            label: self.label,
            allowed_hosts: self.allowed_hosts,
            placements: self.placements,
            allowed_tools: self.allowed_tools,
            allowed_actors: self.allowed_actors,
            derived_from: self.derived_from,
            op: self.op,
            request: self.request,
            created_by: self.created_by,
            created_at: self.created_at,
            updated_at: self.updated_at,
            expires_at: self.expires_at,
        };
        (meta, self.value_enc)
    }
}

// ---------------------------------------------------------------- rules the person approves

/// A host pattern as a person approves it: `api.example.com`, `api.example.com:8443` or `*.example.com`.
/// Lowercased; a name with a dot (no IP literals, no single-label names such as `localhost`), each label
/// 1 to 63 of `a-z0-9-` not starting or ending with `-`, a numeric port 1 to 65535.
pub fn check_host(raw: &str) -> Result<String, String> {
    let h = raw.trim().to_ascii_lowercase();
    let h = h.strip_prefix("https://").unwrap_or(&h).trim_end_matches('/').to_string();
    let bad = || format!("{raw:?} is not a host name such as api.example.com, api.example.com:8443 or *.example.com");
    let (name, port) = match h.rsplit_once(':') {
        Some((n, p)) => match p.parse::<u16>() {
            Ok(p) if p > 0 => (n.to_string(), Some(p)),
            _ => return Err(bad()),
        },
        None => (h.clone(), None),
    };
    let bare = name.strip_prefix("*.").unwrap_or(&name);
    let labels: Vec<&str> = bare.split('.').collect();
    let label_ok = |l: &&str| (1..=63).contains(&l.len()) && !l.starts_with('-') && !l.ends_with('-') && l.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-');
    if bare.len() > 253 || labels.len() < 2 || !labels.iter().all(label_ok) {
        return Err(bad());
    }
    // the top-level label is never all digits: that would be an IPv4 literal
    if labels.last().is_some_and(|l| l.bytes().all(|c| c.is_ascii_digit())) {
        return Err(format!("{raw:?} is an IP address: approve a host name instead"));
    }
    if matches!(*labels.last().unwrap_or(&""), "localhost" | "local" | "internal") {
        return Err(format!("{raw:?} names this machine or its network, which outbound calls never reach"));
    }
    Ok(match port {
        Some(p) => format!("{name}:{p}"),
        None => name,
    })
}

pub fn check_hosts(list: &[String]) -> AppResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for h in list.iter().filter(|h| !h.trim().is_empty()) {
        let h = check_host(h).map_err(AppError::bad_request)?;
        if !out.contains(&h) {
            out.push(h);
        }
    }
    if out.len() > MAX_HOSTS {
        return Err(AppError::bad_request(format!("at most {MAX_HOSTS} hosts")));
    }
    Ok(out)
}

pub fn check_placements(list: &[String]) -> AppResult<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for p in list.iter().map(|p| p.trim().to_ascii_lowercase()).filter(|p| !p.is_empty()) {
        if !PLACEMENTS.contains(&p.as_str()) {
            return Err(AppError::bad_request(format!("a placement is one of {}", PLACEMENTS.join(", "))));
        }
        if !out.contains(&p) {
            out.push(p);
        }
    }
    Ok(out)
}

/// Whether `host:port` (from a URL; port 443 when the URL has none) matches an approved pattern.
pub fn host_allowed(pattern: &str, host: &str, port: u16) -> bool {
    let (name, want_port) = match pattern.rsplit_once(':') {
        Some((n, p)) => (n, p.parse::<u16>().unwrap_or(0)),
        None => (pattern, 443),
    };
    if want_port != port {
        return false;
    }
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    match name.strip_prefix("*.") {
        Some(base) => host.len() > base.len() + 1 && host.ends_with(base) && host.as_bytes()[host.len() - base.len() - 1] == b'.',
        None => host == name,
    }
}

/// A value as pasted: trimmed, [`MIN_LEN`] to [`MAX_LEN`] characters, no NUL. The message never echoes it.
pub fn check_value(raw: &str) -> AppResult<Zeroizing<String>> {
    let v = raw.trim();
    let n = v.chars().count();
    if !(MIN_LEN..=MAX_LEN).contains(&n) || v.contains('\0') {
        return Err(AppError::bad_request(format!("the secret is {MIN_LEN} to {MAX_LEN} characters")));
    }
    Ok(Zeroizing::new(v.to_string()))
}

// ---------------------------------------------------------------- the scrub set

/// The encoded forms of a value that an upstream or a tool may echo instead of the value itself: base64
/// (padded and not), base64url (padded and not), percent- and form-encoded, hex (both cases) and
/// JSON-escaped. Include short encoded forms too, so a one-character value stays protected.
pub fn encoded_forms(value: &str) -> Vec<Zeroizing<String>> {
    let form: String = url::form_urlencoded::byte_serialize(value.as_bytes()).collect();
    let json = serde_json::to_string(value).unwrap_or_default();
    let mut out: Vec<Zeroizing<String>> = Vec::new();
    // base64 of a longer text that holds the value (an echo of "Bearer <value> ...") contains the value's
    // bytes encoded at one of three alignments: the groups wholly inside the value, whatever is around it
    let bytes = value.as_bytes();
    let mut cores: Vec<String> = Vec::new();
    for skip in 0..3.min(bytes.len()) {
        let rest = &bytes[skip..];
        let n = rest.len() / 3 * 3;
        if n >= 6 {
            cores.push(STANDARD_NO_PAD.encode(&rest[..n]));
            cores.push(URL_SAFE_NO_PAD.encode(&rest[..n]));
        }
    }
    for f in cores.into_iter().chain([
        STANDARD.encode(value),
        STANDARD_NO_PAD.encode(value),
        URL_SAFE.encode(value),
        URL_SAFE_NO_PAD.encode(value),
        form.replace('+', "%20"),
        form,
        hex::encode(value),
        hex::encode_upper(value),
        json.get(1..json.len().saturating_sub(1)).unwrap_or_default().to_string(),
    ]) {
        if f != value && !f.is_empty() && !out.iter().any(|o| o.as_str() == f) {
            out.push(Zeroizing::new(f));
        }
    }
    out
}

/// `values` plus every encoded form of each.
pub fn with_encoded_forms(values: Vec<Zeroizing<String>>) -> Vec<Zeroizing<String>> {
    let extra: Vec<Zeroizing<String>> = values.iter().flat_map(|v| encoded_forms(v)).collect();
    let mut all = values;
    for e in extra {
        if !all.iter().any(|v| v.as_str() == e.as_str()) {
            all.push(e);
        }
    }
    all
}

// ---------------------------------------------------------------- reads

pub async fn list(db: &OrgDb, vaults: &[RecordId]) -> AppResult<Vec<HeldMeta>> {
    Ok(q::HELD_LIST.on(db).bind(("vaults", vaults.to_vec())).await?.check()?.take(0)?)
}

pub async fn get(db: &OrgDb, vaults: &[RecordId], id: &RecordId) -> AppResult<Option<HeldMeta>> {
    Ok(q::HELD_GET.on(db).bind(("id", id.clone())).bind(("vaults", vaults.to_vec())).await?.check()?.take::<Vec<HeldMeta>>(0)?.into_iter().next())
}

pub async fn by_name(db: &OrgDb, vaults: &[RecordId], vault: &RecordId, name: &str) -> AppResult<Option<HeldMeta>> {
    Ok(q::HELD_BY_NAME
        .on(db)
        .bind(("vault", vault.clone()))
        .bind(("name", name.to_string()))
        .bind(("vaults", vaults.to_vec()))
        .await?
        .check()?
        .take::<Vec<HeldMeta>>(0)?
        .into_iter()
        .next())
}

/// Why `meta` may not be used by `who` (acting for `user`) in `tool`, or `None` when it may. The broker's
/// rules for a registered reference, plus expiry.
pub async fn refusal(db: &OrgDb, user: &RecordId, who: &Caller, tool: &str, meta: &HeldMeta) -> Option<String> {
    if crate::authz::ensure_member(db, user, &meta.vault).await.is_err() {
        return Some("you are not a member of the vault the secret is in".into());
    }
    if who.vault.as_ref().is_some_and(|v| *v != meta.vault) {
        return Some("this credential is restricted to another vault".into());
    }
    if !meta.allowed_tools.iter().any(|t| t == "*" || t == tool) {
        return Some(format!("the secret is not allowed for the tool {tool}"));
    }
    if !meta.allowed_actors.is_empty() && !meta.allowed_actors.iter().any(|a| *a == who.actor.id || *a == user.to_string()) {
        return Some("the secret is not allowed for this credential".into());
    }
    if meta.expired() {
        return Some("the secret has expired".into());
    }
    None
}

/// The broker's outcome for a refused or failed use, and why.
pub type Refused = (&'static str, String);

/// Checks and decrypts the held secret `eu://<vault_key>/<name>` for one use. Errors are the broker's
/// outcomes (`denied`, `not_found`, `provider_error`) with a reason that never holds the value.
pub async fn for_use(db: &OrgDb, enc_key: &str, user: &RecordId, who: &Caller, tool: &str, vault_key: &str, name: &str) -> Result<(HeldMeta, Zeroizing<String>), Refused> {
    use super::broker::outcome;
    let vaults = super::my_vaults(db, user).await.map_err(|_| (outcome::PROVIDER_ERROR, "the vaults could not be read".to_string()))?;
    let vault = RecordId::new("vault", vault_key.to_string());
    let rows: Vec<ForUse> = match q::HELD_FOR_USE.on(db).bind(("vault", vault)).bind(("name", name.to_string())).bind(("vaults", vaults)).await {
        Ok(mut r) => r.take(0).map_err(|_| (outcome::PROVIDER_ERROR, "the secret could not be read".to_string()))?,
        Err(_) => return Err((outcome::PROVIDER_ERROR, "the secret could not be read".to_string())),
    };
    let Some((meta, value_enc)) = rows.into_iter().next().map(ForUse::split) else {
        // a vault the caller is not in reads as absent: the same answer, so it says nothing about other vaults
        return Err((outcome::DENIED, "no secret by that handle in a vault you belong to (a requested one is held only once the user fills it)".to_string()));
    };
    if let Some(why) = refusal(db, user, who, tool, &meta).await {
        return Err((outcome::DENIED, why));
    }
    let value = crate::connectors::crypto::decrypt(enc_key, &value_enc)
        .map(Zeroizing::new)
        .map_err(|_| (outcome::PROVIDER_ERROR, "the stored secret does not decrypt (ENCRYPTION_KEY changed?)".to_string()))?;
    Ok((meta, value))
}

// ---------------------------------------------------------------- writes

/// A new held secret's fields; the value is encrypted here.
pub struct NewHeld<'a> {
    pub vault: RecordId,
    pub name: String,
    pub label: String,
    pub value: &'a Zeroizing<String>,
    pub hosts: Vec<String>,
    pub placements: Vec<String>,
    pub tools: Vec<String>,
    pub actors: Vec<String>,
    pub derived_from: Vec<RecordId>,
    pub op: Option<String>,
    pub request: Option<RecordId>,
    pub creator: RecordId,
    pub expires_at: Option<Datetime>,
}

pub async fn create(db: &OrgDb, enc_key: &str, vaults: &[RecordId], n: NewHeld<'_>) -> AppResult<HeldMeta> {
    let enc = crate::connectors::crypto::encrypt(enc_key, n.value);
    let ids: Vec<RecordId> = q::HELD_CREATE
        .on(db)
        .bind(("vault", n.vault))
        .bind(("name", n.name))
        .bind(("label", n.label))
        .bind(("value_enc", enc))
        .bind(("hosts", n.hosts))
        .bind(("placements", n.placements))
        .bind(("tools", n.tools))
        .bind(("actors", n.actors))
        .bind(("derived_from", n.derived_from))
        .bind(("op", n.op))
        .bind(("request", n.request))
        .bind(("creator", n.creator))
        .bind(("expires_at", n.expires_at))
        .await
        .map_err(|e| duplicate(e.into()))?
        .check()
        .map_err(|e| duplicate(e.into()))?
        .take(0)?;
    let id = ids.into_iter().next().ok_or_else(|| AppError::internal("vault_secret create returned no id"))?;
    get(db, vaults, &id).await?.ok_or_else(|| AppError::internal("the new secret is not readable"))
}

fn duplicate(e: AppError) -> AppError {
    if e.code == ErrorCode::DbDuplicate { AppError::coded(ErrorCode::DbDuplicate, "A secret with that name already exists in this vault.") } else { e }
}

/// Every held secret derived, directly or not, from `ids` (not `ids` themselves).
pub async fn descendants(db: &OrgDb, vaults: &[RecordId], ids: &[RecordId]) -> AppResult<Vec<RecordId>> {
    let mut out: Vec<RecordId> = Vec::new();
    let mut frontier: Vec<RecordId> = ids.to_vec();
    // a lineage is a DAG (a secret is derived only from older ones), so this ends; the bound is a backstop
    for _ in 0..32 {
        if frontier.is_empty() {
            break;
        }
        let kids: Vec<RecordId> = q::HELD_CHILDREN.on(db).bind(("ids", frontier)).bind(("vaults", vaults.to_vec())).await?.check()?.take(0)?;
        frontier = kids.into_iter().filter(|k| !out.contains(k) && !ids.contains(k)).collect();
        out.extend(frontier.iter().cloned());
    }
    Ok(out)
}

/// Deletes `ids` and everything derived from them; returns how many derived secrets went with them.
pub async fn delete_with_descendants(db: &OrgDb, vaults: &[RecordId], ids: &[RecordId]) -> AppResult<usize> {
    let derived = descendants(db, vaults, ids).await?;
    let mut all = ids.to_vec();
    all.extend(derived.iter().cloned());
    q::HELD_DELETE.on(db).bind(("ids", all)).bind(("vaults", vaults.to_vec())).await?.check()?;
    Ok(derived.len())
}

/// A new value for a held secret; everything derived from the old value is deleted. Returns how many.
pub async fn replace(db: &OrgDb, enc_key: &str, vaults: &[RecordId], id: &RecordId, value: &Zeroizing<String>) -> AppResult<usize> {
    let derived = descendants(db, vaults, std::slice::from_ref(id)).await?;
    let enc = crate::connectors::crypto::encrypt(enc_key, value);
    let ids: Vec<RecordId> = q::HELD_REPLACE.on(db).bind(("id", id.clone())).bind(("value_enc", enc)).bind(("vaults", vaults.to_vec())).await?.check()?.take(0)?;
    if ids.is_empty() {
        return Err(AppError::not_found("Secret not found."));
    }
    if !derived.is_empty() {
        q::HELD_DELETE.on(db).bind(("ids", derived.clone())).bind(("vaults", vaults.to_vec())).await?.check()?;
    }
    Ok(derived.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_names_with_optional_port_or_wildcard() {
        assert_eq!(check_host("API.Stripe.com").unwrap(), "api.stripe.com");
        assert_eq!(check_host("https://api.example.com/").unwrap(), "api.example.com");
        assert_eq!(check_host("api.example.com:8443").unwrap(), "api.example.com:8443");
        assert_eq!(check_host("*.example.com").unwrap(), "*.example.com");
        for bad in [
            "localhost",
            "127.0.0.1",
            "10.0.0.1:443",
            "[::1]",
            "::1",
            "metadata",
            "metadata.google.internal",
            "printer.local",
            "foo.localhost",
            "a..b",
            "-a.com",
            "a-.com",
            "exa mple.com",
            "example.com:0",
            "example.com:99999",
            "user@example.com",
            "*.com.",
            "",
        ] {
            assert!(check_host(bad).is_err(), "{bad:?} accepted");
        }
    }

    #[test]
    fn host_matching() {
        assert!(host_allowed("api.example.com", "api.example.com", 443));
        assert!(host_allowed("api.example.com", "API.example.com.", 443));
        assert!(!host_allowed("api.example.com", "api.example.com", 8443));
        assert!(host_allowed("api.example.com:8443", "api.example.com", 8443));
        assert!(!host_allowed("api.example.com", "evil-api.example.com", 443));
        assert!(!host_allowed("api.example.com", "api.example.com.evil.net", 443));
        assert!(host_allowed("*.example.com", "a.example.com", 443));
        assert!(host_allowed("*.example.com", "a.b.example.com", 443));
        assert!(!host_allowed("*.example.com", "example.com", 443));
        assert!(!host_allowed("*.example.com", "badexample.com", 443));
    }

    #[test]
    fn placements_and_values() {
        assert_eq!(check_placements(&["Header".into(), "body".into(), "header".into()]).unwrap(), ["header", "body"]);
        assert!(check_placements(&["url".into()]).is_err());
        assert!(check_value(" \n").is_err());
        assert!(check_value("7").is_ok_and(|v| v.as_str() == "7"));
        assert!(check_value("é").is_ok());
        assert!(check_value("  long-enough-value \n").is_ok_and(|v| v.as_str() == "long-enough-value"));
        assert!(check_value("has\0nul-inside").is_err());
        assert!(check_value(&"x".repeat(MAX_LEN + 1)).is_err());
        assert!(!check_value("abc").unwrap_err().message.contains("abc"), "the message never echoes the value");
    }

    #[test]
    fn encoded_forms_cover_what_an_upstream_echoes() {
        let forms: Vec<String> = encoded_forms("sk test/key+1").iter().map(|z| z.to_string()).collect();
        for want in ["c2sgdGVzdC9rZXkrMQ==", "c2sgdGVzdC9rZXkrMQ", "sk+test%2Fkey%2B1", "sk%20test%2Fkey%2B1", "736b20746573742f6b65792b31"] {
            assert!(forms.iter().any(|f| f == want), "{want} missing from {forms:?}");
        }
        assert!(!forms.iter().any(|f| f == "sk test/key+1"), "the value itself is not repeated");
        // base64 of a text with the value inside, at every alignment, still loses the value's core
        let v = "sk_live_0123456789abcdef";
        let forms = encoded_forms(v);
        for around in ["", "x", "xy", "Bearer ", "pre:"] {
            for after in ["", " ", "  ", "!!!"] {
                let echoed = STANDARD.encode(format!("{around}{v}{after}"));
                let (scrubbed, n) = crate::redact::scrub_exact(&echoed, &forms);
                assert!(n > 0, "{around:?}+{after:?}: {echoed}");
                let decoded_tail = scrubbed.split("[redacted: secret]").map(|p| p.len()).max().unwrap_or(0);
                assert!(decoded_tail <= 12, "at most a few characters around the core survive: {scrubbed}");
            }
        }
        let all = with_encoded_forms(vec![Zeroizing::new("plainvalue".into())]);
        assert_eq!(all[0].as_str(), "plainvalue");
        assert!(all.iter().any(|v| v.as_str() == "cGxhaW52YWx1ZQ=="));
    }

    #[test]
    fn handles() {
        let v = RecordId::new("vault", "abc");
        assert_eq!(handle_for(&v, "k1"), "eu://abc/k1");
        assert_eq!(handle_value("eu://abc/k1"), serde_json::json!({"$secret": "eu://abc/k1"}));
    }
}
