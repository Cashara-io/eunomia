//! The cryptography of peer vaults (docs/peer-vaults.md, "Protocol"), on crates the backend already
//! has (sha2, hmac, aes-gcm, rand, base64, hex):
//!
//! * [`mac`]: HMAC-SHA256 over length-prefixed parts, so two field lists never serialize alike.
//! * [`link_key`]: the per-link key both sides derive from the share secret and both nonces.
//! * [`seal`] / [`open`]: AES-256-GCM with associated data binding a message to its method, path,
//!   link, time and nonce, so a captured message cannot be replayed elsewhere.
//! * [`Replay`]: rejects a message outside the clock-skew window or whose nonce was seen.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// Messages older or newer than this (seconds) are refused.
pub const MAX_SKEW_SECS: i64 = 300;
/// Nonces remembered per link (well above what one link sends in the skew window).
const REPLAY_CAP: usize = 4096;

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    rand::thread_rng().fill_bytes(&mut b);
    b
}

/// 32 random hex digits (128 bits): instance ids, link ids, nonces.
pub fn random_id() -> String {
    hex::encode(random_bytes::<16>())
}

pub fn b64(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

pub fn unb64(s: &str) -> Option<Vec<u8>> {
    B64.decode(s.trim()).ok()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn mac_state(key: &[u8], parts: &[&str]) -> HmacSha256 {
    let mut m = <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC takes any key length");
    for p in parts {
        m.update(format!("{}:", p.len()).as_bytes());
        m.update(p.as_bytes());
    }
    m
}

/// HMAC-SHA256 of `parts` under `key`, hex.
pub fn mac(key: &[u8], parts: &[&str]) -> String {
    hex::encode(mac_state(key, parts).finalize().into_bytes())
}

/// Constant-time check of a [`mac`].
pub fn verify_mac(key: &[u8], parts: &[&str], tag_hex: &str) -> bool {
    let Ok(tag) = hex::decode(tag_hex.trim()) else { return false };
    mac_state(key, parts).verify_slice(&tag).is_ok()
}

/// `K = HMAC-SHA256(secret, "eunomia-peer-link-v1" || link_id || nonce_g || nonce_h)`.
pub fn link_key(secret: &[u8], link_id: &str, nonce_g: &str, nonce_h: &str) -> [u8; 32] {
    let mut m = <HmacSha256 as Mac>::new_from_slice(secret).expect("HMAC takes any key length");
    for p in ["eunomia-peer-link-v1", link_id, nonce_g, nonce_h] {
        m.update(format!("{}:", p.len()).as_bytes());
        m.update(p.as_bytes());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&m.finalize().into_bytes());
    out
}

/// The associated data of a request: `method|path|link|timestamp|nonce`.
pub fn request_aad(method: &str, path: &str, link: &str, ts: i64, nonce: &str) -> String {
    format!("{method}|{path}|{link}|{ts}|{nonce}")
}

/// The associated data of a response, bound to the request it answers.
pub fn response_aad(link: &str, request_nonce: &str, ts: i64, nonce: &str) -> String {
    format!("response|{link}|{request_nonce}|{ts}|{nonce}")
}

/// AES-256-GCM: base64url of a fresh 12-byte nonce followed by the ciphertext and tag.
pub fn seal(key: &[u8; 32], aad: &str, plaintext: &[u8]) -> String {
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    let iv = random_bytes::<12>();
    let ct = cipher.encrypt(Nonce::from_slice(&iv), Payload { msg: plaintext, aad: aad.as_bytes() }).expect("AES-GCM encrypts any message");
    let mut out = iv.to_vec();
    out.extend_from_slice(&ct);
    b64(&out)
}

/// The plaintext of a [`seal`], or `None` when the data, key or associated data do not match.
pub fn open(key: &[u8; 32], aad: &str, sealed: &str) -> Option<Vec<u8>> {
    let raw = unb64(sealed)?;
    if raw.len() < 12 + 16 {
        return None;
    }
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    cipher.decrypt(Nonce::from_slice(&raw[..12]), Payload { msg: &raw[12..], aad: aad.as_bytes() }).ok()
}

/// Why a message was refused before its content was looked at.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    Skew,
    Replayed,
}

/// Nonces seen per link, within the skew window. In-process: a replica that did not see a nonce would
/// accept its replay, but only within the window, and every write a replay causes is idempotent.
#[derive(Default)]
pub struct Replay {
    seen: Mutex<HashMap<String, SeenNonces>>,
}

/// One link's nonces: in arrival order with their time (to expire them), and as a set (to look up).
type SeenNonces = (VecDeque<(i64, String)>, std::collections::HashSet<String>);

impl Replay {
    /// Accepts (and remembers) `nonce` from `link` sent at `ts`, as of `now` (both unix seconds).
    pub fn check(&self, link: &str, nonce: &str, ts: i64, now: i64) -> Result<(), Refused> {
        if (now - ts).abs() > MAX_SKEW_SECS {
            return Err(Refused::Skew);
        }
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        let (order, set) = seen.entry(link.to_string()).or_default();
        while let Some((at, _)) = order.front() {
            if now - *at > 2 * MAX_SKEW_SECS || order.len() > REPLAY_CAP {
                let (_, old) = order.pop_front().expect("front exists");
                set.remove(&old);
            } else {
                break;
            }
        }
        if !set.insert(nonce.to_string()) {
            return Err(Refused::Replayed);
        }
        order.push_back((now, nonce.to_string()));
        Ok(())
    }
}

/// The process-wide replay guard for incoming peer messages (both directions: requests on a host,
/// responses on a guest).
pub fn replay() -> &'static Replay {
    static R: std::sync::OnceLock<Replay> = std::sync::OnceLock::new();
    R.get_or_init(Replay::default)
}

/// `value` with object keys sorted at every level, serialized without whitespace: the bytes an item
/// hash covers, the same on every instance.
pub fn canonical_json(value: &serde_json::Value) -> String {
    fn sort(v: &serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(o) => {
                let mut keys: Vec<&String> = o.keys().collect();
                keys.sort();
                let mut out = serde_json::Map::new();
                for k in keys {
                    out.insert(k.clone(), sort(&o[k]));
                }
                serde_json::Value::Object(out)
            }
            serde_json::Value::Array(a) => serde_json::Value::Array(a.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    sort(value).to_string()
}

/// sha256 of the canonical JSON of `value`, hex.
pub fn content_hash(value: &serde_json::Value) -> String {
    sha256_hex(canonical_json(value).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_roundtrip_and_tamper() {
        let key = random_bytes::<32>();
        let tag = mac(&key, &["join", "share", "inst"]);
        assert!(verify_mac(&key, &["join", "share", "inst"], &tag));
        assert!(!verify_mac(&key, &["join", "share", "inst2"], &tag));
        assert!(!verify_mac(&random_bytes::<32>(), &["join", "share", "inst"], &tag));
        // length prefixes: moving a character between parts changes the MAC
        assert_ne!(mac(&key, &["ab", "c"]), mac(&key, &["a", "bc"]));
        assert!(!verify_mac(&key, &["join"], "not hex"));
    }

    #[test]
    fn link_key_depends_on_every_input() {
        let s = random_bytes::<32>();
        let k = link_key(&s, "l", "g", "h");
        assert_eq!(k, link_key(&s, "l", "g", "h"));
        assert_ne!(k, link_key(&s, "l", "g", "x"));
        assert_ne!(k, link_key(&s, "l2", "g", "h"));
        assert_ne!(k, link_key(&random_bytes::<32>(), "l", "g", "h"));
    }

    #[test]
    fn aead_roundtrip_rejects_wrong_aad_key_or_bytes() {
        let key = random_bytes::<32>();
        let aad = request_aad("POST", "/api/peer/v1/o/links/l", "l", 100, "n");
        let sealed = seal(&key, &aad, b"hello");
        assert_eq!(open(&key, &aad, &sealed).as_deref(), Some(&b"hello"[..]));
        assert!(open(&key, &request_aad("POST", "/other", "l", 100, "n"), &sealed).is_none());
        assert!(open(&random_bytes::<32>(), &aad, &sealed).is_none());
        let mut raw = unb64(&sealed).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 1;
        assert!(open(&key, &aad, &b64(&raw)).is_none());
        assert!(open(&key, &aad, "short").is_none());
    }

    #[test]
    fn replay_and_skew_are_refused() {
        let r = Replay::default();
        assert_eq!(r.check("l", "n1", 1000, 1000), Ok(()));
        assert_eq!(r.check("l", "n1", 1000, 1001), Err(Refused::Replayed));
        // another link may use the same nonce
        assert_eq!(r.check("l2", "n1", 1000, 1001), Ok(()));
        assert_eq!(r.check("l", "n2", 1000 - MAX_SKEW_SECS - 1, 1000), Err(Refused::Skew));
        assert_eq!(r.check("l", "n3", 1000 + MAX_SKEW_SECS + 1, 1000), Err(Refused::Skew));
        assert_eq!(r.check("l", "n4", 1000 + MAX_SKEW_SECS, 1000), Ok(()));
    }

    #[test]
    fn canonical_json_sorts_keys() {
        let a = serde_json::json!({"b": 1, "a": {"d": [1, {"z": 0, "y": 1}], "c": "x"}});
        assert_eq!(canonical_json(&a), r#"{"a":{"c":"x","d":[1,{"y":1,"z":0}]},"b":1}"#);
        assert_eq!(content_hash(&a), content_hash(&serde_json::json!({"a": {"c": "x", "d": [1, {"y": 1, "z": 0}]}, "b": 1})));
    }
}
