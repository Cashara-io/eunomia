//! `secret_transform` (docs/architecture/specs/secret-requests.md, "Transforms"): server-side operations
//! on held secrets that produce a NEW held secret and hand back only its handle. The operations are an
//! allowlist of one-way or expanding functions; nothing here decodes, compares, measures or slices a
//! value, so a transform cannot become an oracle for a value the agent may not see. A derived secret
//! never allows more than its inputs: hosts, placements and tools are intersected, actors narrowed, and
//! the earliest expiry wins.

use std::collections::BTreeMap;

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use surrealdb::types::{Datetime, RecordId};
use zeroize::Zeroizing;

use super::held::{self, HeldMeta};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::rid::RecordIdExt;

pub const OPS: &[&str] = &["base64", "base64url", "url_encode", "template", "basic_auth", "hmac_sha256", "sha256", "jwt_hs256", "jwt_rs256", "jwt_es256"];
/// At most this many inputs to one transform.
pub const MAX_INPUTS: usize = 8;
const MAX_TEMPLATE: usize = 4096;
const MAX_CLAIMS: usize = 8192;

/// The values a transform works on, by the input names the agent gave.
pub type Inputs = BTreeMap<String, Zeroizing<String>>;

fn need<'a>(inputs: &'a Inputs, name: &str, op: &str) -> Result<&'a Zeroizing<String>, String> {
    inputs.get(name).ok_or_else(|| format!("{op} needs an input named {name:?}"))
}

fn only(inputs: &Inputs, allowed: &[&str], op: &str) -> Result<(), String> {
    match inputs.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(format!("{op} takes the inputs {}; {k:?} is not one of them", allowed.join(", "))),
        None => Ok(()),
    }
}

fn param_str<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str)
}

/// RFC 3986 percent-encoding of everything but the unreserved characters.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn encode_digest(bytes: &[u8], params: &Value) -> Result<String, String> {
    Ok(match param_str(params, "encoding").unwrap_or("hex") {
        "hex" => hex::encode(bytes),
        "base64" => STANDARD.encode(bytes),
        "base64url" => URL_SAFE_NO_PAD.encode(bytes),
        _ => return Err("encoding is hex, base64 or base64url".into()),
    })
}

/// A secret used as key bytes: as text (default), or decoded from hex or base64 (`key_encoding`), as
/// webhook signing secrets often are. A decode failure says only that it failed.
fn key_bytes(key: &str, params: &Value) -> Result<Zeroizing<Vec<u8>>, String> {
    let bytes = match param_str(params, "key_encoding").unwrap_or("utf8") {
        "utf8" => key.as_bytes().to_vec(),
        "hex" => hex::decode(key).map_err(|_| "the key input is not hex (key_encoding)".to_string())?,
        "base64" => STANDARD.decode(key).or_else(|_| URL_SAFE_NO_PAD.decode(key.trim_end_matches('='))).map_err(|_| "the key input is not base64 (key_encoding)".to_string())?,
        _ => return Err("key_encoding is utf8, hex or base64".into()),
    };
    Ok(Zeroizing::new(bytes))
}

/// A literal message as given (`message_encoding` utf8, the default, or hex or base64 for binary).
fn literal_bytes(text: &str, params: &Value) -> Result<Vec<u8>, String> {
    match param_str(params, "message_encoding").unwrap_or("utf8") {
        "utf8" => Ok(text.as_bytes().to_vec()),
        "hex" => hex::decode(text).map_err(|_| "message is not hex".to_string()),
        "base64" => STANDARD.decode(text).map_err(|_| "message is not base64".to_string()),
        _ => Err("message_encoding is utf8, hex or base64".into()),
    }
}

/// `{{name}}` slots filled from `inputs`; every slot names an input and every input is used.
fn template(t: &str, inputs: &Inputs) -> Result<Zeroizing<String>, String> {
    if t.is_empty() || t.chars().count() > MAX_TEMPLATE {
        return Err(format!("template is 1 to {MAX_TEMPLATE} characters"));
    }
    let mut out = Zeroizing::new(String::with_capacity(t.len() + 64));
    let mut used: Vec<&str> = Vec::new();
    let mut rest = t;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find("}}").ok_or("a {{ in the template is not closed by }}")?;
        let name = after[..end].trim();
        let v = inputs.get(name).ok_or_else(|| format!("the template names {name:?}, which is not an input"))?;
        out.push_str(v);
        used.push(inputs.get_key_value(name).map(|(k, _)| k.as_str()).unwrap_or_default());
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    if let Some(unused) = inputs.keys().find(|k| !used.contains(&k.as_str())) {
        return Err(format!("the input {unused:?} is not used in the template"));
    }
    Ok(out)
}

fn jwt(op: &str, key: &str, params: &Value, now: i64) -> Result<Zeroizing<String>, String> {
    use jsonwebtoken::{Algorithm, EncodingKey};
    let (alg, name) = match op {
        "jwt_hs256" => (Algorithm::HS256, "HS256"),
        "jwt_rs256" => (Algorithm::RS256, "RS256"),
        _ => (Algorithm::ES256, "ES256"),
    };
    let mut claims = match params.get("claims") {
        Some(Value::Object(m)) => m.clone(),
        None => serde_json::Map::new(),
        Some(_) => return Err("claims is a JSON object".into()),
    };
    if let Some(secs) = params.get("expires_in_seconds") {
        let secs = secs.as_i64().filter(|s| (1..=31_536_000).contains(s)).ok_or("expires_in_seconds is 1 to 31536000")?;
        claims.insert("iat".into(), json!(now));
        claims.insert("exp".into(), json!(now + secs));
    }
    let claims_json = Value::Object(claims).to_string();
    if claims_json.len() > MAX_CLAIMS {
        return Err(format!("claims are at most {MAX_CLAIMS} bytes"));
    }
    // the header is ours, in a fixed order: alg, typ, then kid when given
    let mut header = serde_json::Map::new();
    header.insert("alg".into(), json!(name));
    header.insert("typ".into(), json!(param_str(params.get("header").unwrap_or(&Value::Null), "typ").unwrap_or("JWT")));
    if let Some(h) = params.get("header") {
        let obj = h.as_object().ok_or("header is a JSON object with kid and typ only")?;
        if let Some(k) = obj.keys().find(|k| !matches!(k.as_str(), "kid" | "typ")) {
            return Err(format!("header takes kid and typ only, not {k:?}"));
        }
        if let Some(kid) = obj.get("kid") {
            header.insert("kid".into(), json!(kid.as_str().ok_or("kid is a string")?));
        }
    }
    let input = format!("{}.{}", URL_SAFE_NO_PAD.encode(Value::Object(header).to_string()), URL_SAFE_NO_PAD.encode(&claims_json));
    let ekey = match alg {
        Algorithm::HS256 => EncodingKey::from_secret(&key_bytes(key, params)?),
        Algorithm::RS256 => EncodingKey::from_rsa_pem(key.as_bytes()).map_err(|_| "the key input is not an RSA private key in PEM form".to_string())?,
        _ => EncodingKey::from_ec_pem(key.as_bytes()).map_err(|_| "the key input is not a P-256 private key in PKCS#8 PEM form".to_string())?,
    };
    let sig = jsonwebtoken::crypto::sign(input.as_bytes(), &ekey, alg).map_err(|_| format!("{op}: the key could not sign"))?;
    Ok(Zeroizing::new(format!("{input}.{sig}")))
}

/// Applies `op`. Errors name the rule that failed, never a value. `now` is the Unix time for JWT claims.
pub fn apply(op: &str, inputs: &Inputs, params: &Value, now: i64) -> Result<Zeroizing<String>, String> {
    if !params.is_object() && !params.is_null() {
        return Err("params is a JSON object".into());
    }
    match op {
        "base64" | "base64url" | "url_encode" | "sha256" => {
            only(inputs, &["value"], op)?;
            let v = need(inputs, "value", op)?;
            Ok(Zeroizing::new(match op {
                "base64" => STANDARD.encode(v.as_bytes()),
                "base64url" => URL_SAFE_NO_PAD.encode(v.as_bytes()),
                "url_encode" => url_encode(v),
                _ => encode_digest(&Sha256::digest(v.as_bytes()), params)?,
            }))
        }
        "template" => template(param_str(params, "template").ok_or("template needs params.template")?, inputs),
        "basic_auth" => {
            only(inputs, &["user", "password"], op)?;
            let password = need(inputs, "password", op)?;
            let user: Zeroizing<String> = match (inputs.get("user"), param_str(params, "user")) {
                (Some(u), None) => u.clone(),
                (None, Some(u)) if !u.is_empty() => Zeroizing::new(u.to_string()),
                (Some(_), Some(_)) => return Err("basic_auth takes the user as an input or as params.user, not both".into()),
                _ => return Err("basic_auth needs a user: an input named \"user\" or params.user".into()),
            };
            if user.contains(':') {
                return Err("a Basic auth user name has no colon (RFC 7617)".into());
            }
            let pair = Zeroizing::new(format!("{}:{}", user.as_str(), password.as_str()));
            Ok(Zeroizing::new(format!("Basic {}", STANDARD.encode(pair.as_bytes()))))
        }
        "hmac_sha256" => {
            only(inputs, &["key", "message"], op)?;
            let key = key_bytes(need(inputs, "key", op)?, params)?;
            let message: Vec<u8> = match (inputs.get("message"), param_str(params, "message")) {
                (Some(m), None) => m.as_bytes().to_vec(),
                (None, Some(m)) => literal_bytes(m, params)?,
                (Some(_), Some(_)) => return Err("hmac_sha256 takes the message as an input or as params.message, not both".into()),
                (None, None) => return Err("hmac_sha256 needs a message: an input named \"message\" or params.message".into()),
            };
            let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&key).map_err(|_| "the key could not be used".to_string())?;
            mac.update(&message);
            Ok(Zeroizing::new(encode_digest(&mac.finalize().into_bytes(), params)?))
        }
        "jwt_hs256" | "jwt_rs256" | "jwt_es256" => {
            only(inputs, &["key"], op)?;
            jwt(op, need(inputs, "key", op)?, params, now)
        }
        _ => Err(format!("op is one of {}", OPS.join(", "))),
    }
}

// ---------------------------------------------------------------- inheritance

/// What a derived secret may do: never more than any of its inputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub vault: RecordId,
    pub hosts: Vec<String>,
    pub placements: Vec<String>,
    pub tools: Vec<String>,
    pub actors: Vec<String>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn intersect(lists: &[&Vec<String>]) -> Vec<String> {
    let Some(first) = lists.first() else { return Vec::new() };
    first.iter().filter(|x| lists.iter().all(|l| l.contains(x))).cloned().collect()
}

/// Tools are intersected, with `*` standing for any tool.
fn intersect_tools(lists: &[&Vec<String>]) -> Vec<String> {
    let specific: Vec<&Vec<String>> = lists.iter().copied().filter(|l| !l.iter().any(|t| t == "*")).collect();
    if specific.is_empty() {
        return vec!["*".into()];
    }
    intersect(&specific)
}

pub fn inherit(inputs: &[&HeldMeta], requested_expiry: Option<chrono::DateTime<chrono::Utc>>) -> Result<Policy, String> {
    let first = inputs.first().ok_or("a transform needs at least one input")?;
    if inputs.iter().any(|m| m.vault != first.vault) {
        return Err("every input of a transform is in the same vault".into());
    }
    let hosts = intersect(&inputs.iter().map(|m| &m.allowed_hosts).collect::<Vec<_>>());
    if hosts.is_empty() {
        return Err("the inputs share no approved host, so the result could not be sent anywhere".into());
    }
    let placements = intersect(&inputs.iter().map(|m| &m.placements).collect::<Vec<_>>());
    if placements.is_empty() {
        return Err("the inputs share no approved placement (header, query or body)".into());
    }
    let tools = intersect_tools(&inputs.iter().map(|m| &m.allowed_tools).collect::<Vec<_>>());
    if tools.is_empty() {
        return Err("the inputs share no allowed tool".into());
    }
    let narrowed: Vec<&Vec<String>> = inputs.iter().map(|m| &m.allowed_actors).filter(|a| !a.is_empty()).collect();
    let actors = intersect(&narrowed);
    if !narrowed.is_empty() && actors.is_empty() {
        return Err("the inputs are allowed for different credentials, so no one could use the result".into());
    }
    let expiry = inputs
        .iter()
        .filter_map(|m| m.expires_at.map(chrono::DateTime::<chrono::Utc>::from))
        .chain(requested_expiry)
        .min();
    Ok(Policy { vault: first.vault.clone(), hosts, placements, tools, actors, expires_at: expiry })
}

// ---------------------------------------------------------------- the tool

/// `secret_transform`'s arguments.
#[derive(serde::Deserialize)]
pub struct Args {
    pub op: String,
    /// input name -> `eu://` handle (a plain string, or a `{"$secret": ...}` object's text)
    pub inputs: BTreeMap<String, Value>,
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub expires_in_days: Option<i64>,
    #[serde(default)]
    pub replace: bool,
}

fn input_handle(v: &Value) -> Option<&str> {
    v.as_str().or_else(|| v.get(super::reference::MARKER).and_then(Value::as_str))
}

/// Runs a transform for `user`: checks and decrypts every input (each audited as a use), derives,
/// stores the result as a new held secret, audits `secret.transform` with the lineage. Returns the tool
/// value: the handle and the policy, never a value.
pub async fn run(state: &crate::state::OrgState, user: &RecordId, args: Value) -> AppResult<Value> {
    let a: Args = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return Ok(json!({"error": format!("invalid arguments: {e}")})),
    };
    let invalid = |m: String| Ok(AppError::coded(ErrorCode::ValidationInvalid, m).to_tool_value());
    if !OPS.contains(&a.op.as_str()) {
        return invalid(format!("op is one of {}", OPS.join(", ")));
    }
    if a.inputs.is_empty() || a.inputs.len() > MAX_INPUTS {
        return invalid(format!("inputs holds 1 to {MAX_INPUTS} eu:// handles by name"));
    }
    if let Some(n) = &a.name
        && !super::reference::valid_name(n)
    {
        return invalid("name is lowercase letters, digits, _, . and -, at most 64 characters".into());
    }
    let requested_expiry = match a.expires_in_days {
        None => None,
        Some(d) if (1..=3650).contains(&d) => Some(chrono::Utc::now() + chrono::TimeDelta::days(d)),
        Some(_) => return invalid("expires_in_days is 1 to 3650".into()),
    };

    let who = crate::authz::current().unwrap_or_else(|| crate::authz::Caller::session(user));
    let mut metas: Vec<HeldMeta> = Vec::new();
    let mut values: Inputs = BTreeMap::new();
    let mut refused: Option<String> = None;
    for (input, v) in &a.inputs {
        let Some(text) = input_handle(v) else {
            refused.get_or_insert(format!("the input {input:?} is not an eu:// handle"));
            continue;
        };
        let parsed = match super::reference::parse(text) {
            Ok(super::reference::Reference::Eu { vault, name }) => (vault, name),
            Ok(_) => {
                refused.get_or_insert(format!("the input {input:?} is not an eu:// handle: transforms work only on secrets Eunomia holds"));
                continue;
            }
            Err(m) => {
                refused.get_or_insert(format!("the input {input:?} is malformed: {}", m.0));
                continue;
            }
        };
        let outcome = held::for_use(&state.db, &state.settings.encryption_key, user, &who, "secret_transform", &parsed.0, &parsed.1).await;
        let (out, reason, ref_id, vault) = match &outcome {
            Ok((m, _)) => (super::broker::outcome::OK, String::new(), m.id.to_string(), m.vault.to_string()),
            Err((o, r)) => (*o, r.clone(), String::new(), String::new()),
        };
        audit_resolve(state, user, &who, text, &ref_id, &vault, out, &reason).await;
        match outcome {
            Ok((m, value)) => {
                metas.push(m);
                values.insert(input.clone(), value);
            }
            Err((_, r)) => {
                refused.get_or_insert(format!("{text}: {r}"));
            }
        }
    }
    if let Some(why) = refused {
        return Ok(AppError::coded(ErrorCode::SecretUnavailable, format!("A transform input could not be used: {why}")).to_tool_value());
    }

    let refs: Vec<&HeldMeta> = metas.iter().collect();
    let policy = match inherit(&refs, requested_expiry) {
        Ok(p) => p,
        Err(m) => return invalid(m),
    };
    let derived = match apply(&a.op, &values, &a.params, chrono::Utc::now().timestamp()) {
        Ok(v) => v,
        Err(m) => return invalid(m),
    };
    drop(values);
    let n = derived.chars().count();
    if !(held::MIN_LEN..=held::MAX_LEN).contains(&n) {
        return invalid(format!("the result would be {n} characters; a held secret is {} to {}", held::MIN_LEN, held::MAX_LEN));
    }

    let first_name = metas.first().map(|m| m.name.clone()).unwrap_or_default();
    let name = a.name.clone().unwrap_or_else(|| {
        let tag = uuid::Uuid::new_v4().simple().to_string();
        let base: String = format!("{}_{first_name}", a.op).chars().take(56).collect();
        format!("{base}_{}", &tag[..6])
    });
    let vaults = super::my_vaults(&state.db, user).await?;
    if let Some(existing) = held::by_name(&state.db, &vaults, &policy.vault, &name).await? {
        if !a.replace || existing.derived_from.is_empty() {
            let why = if existing.derived_from.is_empty() { "a secret a person pasted is never overwritten by a transform" } else { "pass replace: true to overwrite it" };
            return Ok(AppError::coded(ErrorCode::DbDuplicate, format!("{} already exists: {why}", existing.handle())).to_tool_value());
        }
        held::delete_with_descendants(&state.db, &vaults, std::slice::from_ref(&existing.id)).await?;
    }
    let lineage: Vec<RecordId> = metas.iter().map(|m| m.id.clone()).collect();
    let made = held::create(
        &state.db,
        &state.settings.encryption_key,
        &vaults,
        held::NewHeld {
            vault: policy.vault.clone(),
            name: name.clone(),
            label: format!("{} of {}", a.op, metas.iter().map(|m| m.name.as_str()).collect::<Vec<_>>().join(", ")).chars().take(120).collect(),
            value: &derived,
            hosts: policy.hosts.clone(),
            placements: policy.placements.clone(),
            tools: policy.tools.clone(),
            actors: policy.actors.clone(),
            derived_from: lineage.clone(),
            op: Some(a.op.clone()),
            request: None,
            creator: user.clone(),
            expires_at: policy.expires_at.map(Datetime::from),
        },
    )
    .await?;
    drop(derived);

    // the whole lineage (inputs and their own ancestors), for the audit row
    let mut ancestry: Vec<String> = lineage.iter().map(|r| r.to_string()).collect();
    for m in &metas {
        ancestry.extend(m.derived_from.iter().map(|r| r.to_string()));
    }
    ancestry.dedup();
    let detail = json!({"op": a.op, "handle": made.handle(), "secret": made.id.to_string(), "lineage": ancestry, "vault": made.vault.to_string()}).to_string();
    super::requests::audit(state, user, "secret.transform", "ok", &detail).await;
    Ok(json!({
        "handle": held::handle_value(&made.handle()),
        "name": made.name,
        "op": a.op,
        "lineage": metas.iter().map(|m| m.handle()).collect::<Vec<_>>(),
        "allowed_hosts": made.allowed_hosts,
        "placements": made.placements,
        "allowed_tools": made.allowed_tools,
        "expires_at": made.expires_at,
    }))
}

/// One `secret.resolve` row for a transform input, in the broker's shape (the resolution log reads it).
#[allow(clippy::too_many_arguments)]
async fn audit_resolve(state: &crate::state::OrgState, user: &RecordId, who: &crate::authz::Caller, handle: &str, ref_id: &str, vault: &str, outcome: &str, reason: &str) {
    let ctx = crate::usage::current_context();
    let detail = json!({
        "ref": handle.chars().take(200).collect::<String>(),
        "ref_id": ref_id,
        "tool": "secret_transform",
        "channel": ctx.channel.as_str(),
        "session": ctx.session,
        "vault": vault,
        "reason": reason.chars().take(120).collect::<String>(),
    })
    .to_string();
    let org = state.db.org().key();
    crate::audit::record(&state.control, crate::audit::Event { user: Some(user), actor: &who.actor, action: "secret.resolve", target: &org, outcome, detail: &detail }).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ins(pairs: &[(&str, &str)]) -> Inputs {
        pairs.iter().map(|(k, v)| (k.to_string(), Zeroizing::new(v.to_string()))).collect()
    }

    fn run(op: &str, pairs: &[(&str, &str)], params: Value) -> Result<String, String> {
        apply(op, &ins(pairs), &params, 1_700_000_000).map(|z| z.to_string())
    }

    #[test]
    fn base64_rfc4648_vectors() {
        for (plain, b64) in [("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(run("base64", &[("value", plain)], Value::Null).unwrap(), b64);
        }
        assert_eq!(run("base64url", &[("value", "\u{ff}\u{fe}?>")], Value::Null).unwrap(), URL_SAFE_NO_PAD.encode("\u{ff}\u{fe}?>"));
        assert!(!run("base64url", &[("value", "??>>??>>")], Value::Null).unwrap().contains(['+', '/', '=']));
    }

    #[test]
    fn url_encoding_and_sha256() {
        assert_eq!(run("url_encode", &[("value", "a b/c?d=e&f~g")], Value::Null).unwrap(), "a%20b%2Fc%3Fd%3De%26f~g");
        assert_eq!(run("sha256", &[("value", "abc")], Value::Null).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(run("sha256", &[("value", "abc")], json!({"encoding": "base64"})).unwrap(), "ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=");
        assert!(run("sha256", &[("value", "abc")], json!({"encoding": "rot13"})).is_err());
    }

    #[test]
    fn hmac_sha256_rfc4231_vectors() {
        // test case 1: key 0x0b x 20, "Hi There"
        let k1 = "\u{0b}".repeat(20);
        assert_eq!(run("hmac_sha256", &[("key", &k1)], json!({"message": "Hi There"})).unwrap(), "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
        // test case 2: "Jefe"
        assert_eq!(
            run("hmac_sha256", &[("key", "Jefe")], json!({"message": "what do ya want for nothing?"})).unwrap(),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        // test case 4: key 0x01..0x19 as hex, message 0xcd x 50 as hex
        let k4: String = (1u8..=25).map(|b| format!("{b:02x}")).collect();
        let m4 = "cd".repeat(50);
        assert_eq!(
            run("hmac_sha256", &[("key", &k4)], json!({"message": m4, "key_encoding": "hex", "message_encoding": "hex"})).unwrap(),
            "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b"
        );
        // the message may be a handle too
        assert_eq!(run("hmac_sha256", &[("key", "Jefe"), ("message", "what do ya want for nothing?")], json!({})).unwrap().len(), 64);
        assert!(run("hmac_sha256", &[("key", "Jefe")], json!({})).is_err(), "no message");
        assert!(run("hmac_sha256", &[("key", "not-hex!")], json!({"message": "m", "key_encoding": "hex"})).unwrap_err().contains("not hex"));
    }

    #[test]
    fn templates_and_basic_auth() {
        assert_eq!(run("template", &[("key", "sk_live_123")], json!({"template": "Bearer {{key}}"})).unwrap(), "Bearer sk_live_123");
        assert_eq!(run("template", &[("user", "me"), ("pass", "pw")], json!({"template": "{{ user }}:{{pass}}"})).unwrap(), "me:pw");
        assert_eq!(run("template", &[("key", "abc12345")], json!({"template": "API_KEY={{key}}\nAGAIN={{key}}"})).unwrap(), "API_KEY=abc12345\nAGAIN=abc12345");
        assert!(run("template", &[("key", "x")], json!({"template": "{{other}}"})).unwrap_err().contains("not an input"));
        assert!(run("template", &[("key", "x"), ("b", "y")], json!({"template": "{{key}}"})).unwrap_err().contains("not used"));
        assert!(run("template", &[("key", "x")], json!({"template": "{{key"})).is_err());
        // RFC 7617's example: Aladdin / open sesame
        assert_eq!(run("basic_auth", &[("password", "open sesame")], json!({"user": "Aladdin"})).unwrap(), "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
        assert_eq!(run("basic_auth", &[("user", "Aladdin"), ("password", "open sesame")], json!({})).unwrap(), "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==");
        assert!(run("basic_auth", &[("password", "pw")], json!({"user": "a:b"})).is_err());
        assert!(run("basic_auth", &[("password", "pw")], json!({})).is_err());
    }

    #[test]
    fn jwt_hs256_matches_the_well_known_vector() {
        // the jwt.io example: header {"alg":"HS256","typ":"JWT"}, secret "your-256-bit-secret"
        let token = run("jwt_hs256", &[("key", "your-256-bit-secret")], json!({"claims": {"sub": "1234567890", "name": "John Doe", "iat": 1516239022}})).unwrap();
        assert_eq!(
            token,
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"
        );
        let with_exp = run("jwt_hs256", &[("key", "your-256-bit-secret")], json!({"claims": {"sub": "x"}, "expires_in_seconds": 60, "header": {"kid": "k1"}})).unwrap();
        let mut v = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
        v.validate_exp = false;
        let data = jsonwebtoken::decode::<Value>(&with_exp, &jsonwebtoken::DecodingKey::from_secret(b"your-256-bit-secret"), &v).unwrap();
        assert_eq!((data.claims["iat"].as_i64(), data.claims["exp"].as_i64()), (Some(1_700_000_000), Some(1_700_000_060)));
        assert_eq!(data.header.kid.as_deref(), Some("k1"));
        assert!(run("jwt_hs256", &[("key", "k")], json!({"header": {"alg": "none"}})).is_err(), "the header cannot change the algorithm");
    }

    #[test]
    fn jwt_rs256_and_es256_sign_with_a_pem_key() {
        let v = |alg| {
            let mut v = jsonwebtoken::Validation::new(alg);
            v.validate_exp = false;
            v.required_spec_claims.clear();
            v
        };
        let rsa = include_str!("../../tests/fixtures/jwt/rsa_test_key.pem");
        let token = run("jwt_rs256", &[("key", rsa)], json!({"claims": {"sub": "svc"}})).unwrap();
        let pubkey = jsonwebtoken::DecodingKey::from_rsa_pem(include_bytes!("../../tests/fixtures/jwt/rsa_test_pub.pem")).unwrap();
        assert_eq!(jsonwebtoken::decode::<Value>(&token, &pubkey, &v(jsonwebtoken::Algorithm::RS256)).unwrap().claims["sub"], "svc");
        let ec = include_str!("../../tests/fixtures/jwt/ec_test_key.pem");
        let token = run("jwt_es256", &[("key", ec)], json!({"claims": {"sub": "svc"}})).unwrap();
        let pubkey = jsonwebtoken::DecodingKey::from_ec_pem(include_bytes!("../../tests/fixtures/jwt/ec_test_pub.pem")).unwrap();
        assert_eq!(jsonwebtoken::decode::<Value>(&token, &pubkey, &v(jsonwebtoken::Algorithm::ES256)).unwrap().claims["sub"], "svc");
        let e = run("jwt_rs256", &[("key", "definitely-not-a-pem-key")], json!({})).unwrap_err();
        assert!(!e.contains("definitely"), "the error never echoes the key: {e}");
    }

    #[test]
    fn unknown_ops_and_stray_inputs_are_refused() {
        for op in ["base64_decode", "reveal", "compare", "length", "slice", "upper"] {
            assert!(run(op, &[("value", "abcdefgh")], Value::Null).is_err(), "{op}");
        }
        assert!(run("base64", &[("value", "a"), ("extra", "b")], Value::Null).is_err());
        assert!(run("base64", &[("other", "a")], Value::Null).is_err());
    }

    fn meta(vault: &str, hosts: &[&str], placements: &[&str], tools: &[&str], actors: &[&str], expires: Option<i64>) -> HeldMeta {
        let s = |l: &[&str]| l.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let now = chrono::Utc::now();
        HeldMeta {
            id: RecordId::new("vault_secret", uuid::Uuid::new_v4().simple().to_string()),
            vault: RecordId::new("vault", vault),
            name: "n".into(),
            label: String::new(),
            allowed_hosts: s(hosts),
            placements: s(placements),
            allowed_tools: s(tools),
            allowed_actors: s(actors),
            derived_from: vec![],
            op: None,
            request: None,
            created_by: RecordId::new("user", "u"),
            created_at: Datetime::from(now),
            updated_at: Datetime::from(now),
            expires_at: expires.map(|d| Datetime::from(now + chrono::TimeDelta::days(d))),
        }
    }

    #[test]
    fn a_derived_secret_inherits_the_most_restrictive_rules() {
        let a = meta("v", &["api.a.com", "api.b.com"], &["header", "body"], &["http_request", "secret_transform"], &[], Some(10));
        let b = meta("v", &["api.b.com"], &["header"], &["*"], &["api_token:t1", "api_token:t2"], Some(3));
        let p = inherit(&[&a, &b], None).unwrap();
        assert_eq!(p.hosts, ["api.b.com"]);
        assert_eq!(p.placements, ["header"]);
        assert_eq!(p.tools, ["http_request", "secret_transform"]);
        assert_eq!(p.actors, ["api_token:t1", "api_token:t2"]);
        let b_exp = chrono::DateTime::<chrono::Utc>::from(b.expires_at.unwrap());
        assert_eq!(p.expires_at, Some(b_exp));
        // a sooner requested expiry wins; a later one does not extend it
        let soon = chrono::Utc::now() + chrono::TimeDelta::days(1);
        assert_eq!(inherit(&[&a, &b], Some(soon)).unwrap().expires_at, Some(soon));
        assert_eq!(inherit(&[&a, &b], Some(soon + chrono::TimeDelta::days(30))).unwrap().expires_at, Some(b_exp));

        let c = meta("v", &["api.c.com"], &["header"], &["*"], &[], None);
        assert!(inherit(&[&a, &c], None).unwrap_err().contains("no approved host"));
        let d = meta("other", &["api.b.com"], &["header"], &["*"], &[], None);
        assert!(inherit(&[&b, &d], None).unwrap_err().contains("same vault"));
        let e = meta("v", &["api.b.com"], &["header"], &["*"], &["api_token:t9"], None);
        assert!(inherit(&[&b, &e], None).unwrap_err().contains("different credentials"));
        let f = meta("v", &["api.b.com"], &["query"], &["*"], &[], None);
        assert!(inherit(&[&b, &f], None).unwrap_err().contains("placement"));
        let star = meta("v", &["api.b.com"], &["header"], &["*"], &[], None);
        assert_eq!(inherit(&[&star], None).unwrap().tools, ["*"]);
    }
}
