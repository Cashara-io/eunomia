//! Secret references (docs/architecture/specs/broker.md, "References"): `op://<vault>/<item>/<field>`,
//! `op://<vault>/<item>/<section>/<field>` (1Password's own syntax, with an optional `?attribute=<name>`)
//! and `bws://<secret-id>` (ours: a Bitwarden Secrets Manager secret by its UUID). In tool arguments a
//! reference is an object with exactly one key, `{"$secret": "<reference>"}`. The sidecar parses the same
//! grammar (`secrets-helper/src/refs.ts`); keep the two in step.

use serde_json::Value;

/// The key that marks a reference object in tool arguments and connector settings.
pub const MARKER: &str = "$secret";
/// At most this many references in one call.
pub const MAX_REFS: usize = 16;
const MAX_PART: usize = 256;
const MAX_LEN: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reference {
    Op { vault: String, item: String, section: Option<String>, field: String, attribute: Option<String> },
    Bws { id: String },
}

impl Reference {
    /// The provider family: `op` (either 1Password kind) or `bws`.
    pub fn scheme(&self) -> &'static str {
        match self {
            Reference::Op { .. } => "op",
            Reference::Bws { .. } => "bws",
        }
    }

    /// The normalised text: the scheme lowercased, a Bitwarden id lowercased, the rest as written.
    pub fn text(&self) -> String {
        match self {
            Reference::Op { vault, item, section, field, attribute } => {
                let mut s = format!("op://{vault}/{item}/");
                if let Some(sec) = section {
                    s.push_str(sec);
                    s.push('/');
                }
                s.push_str(field);
                if let Some(a) = attribute {
                    s.push_str("?attribute=");
                    s.push_str(a);
                }
                s
            }
            Reference::Bws { id } => format!("bws://{id}"),
        }
    }
}

/// Why a reference does not parse. The message names the rule, never echoes the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Malformed(pub &'static str);

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { *c == b'-' } else { c.is_ascii_hexdigit() })
}

pub fn parse(raw: &str) -> Result<Reference, Malformed> {
    if raw.is_empty() || raw.len() > MAX_LEN {
        return Err(Malformed("a reference is a non-empty string of at most 1024 characters"));
    }
    if raw.chars().any(char::is_control) {
        return Err(Malformed("a reference holds no control characters"));
    }
    let Some((scheme, rest)) = raw.split_once("://") else {
        return Err(Malformed("a reference starts with op:// or bws://"));
    };
    match scheme.to_ascii_lowercase().as_str() {
        "bws" if is_uuid(rest) => Ok(Reference::Bws { id: rest.to_ascii_lowercase() }),
        "bws" => Err(Malformed("bws:// is followed by a secret id (a UUID) and nothing else")),
        "op" => parse_op(rest),
        _ => Err(Malformed("a reference starts with op:// or bws://")),
    }
}

fn parse_op(rest: &str) -> Result<Reference, Malformed> {
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rest, None),
    };
    let attribute = match query {
        None => None,
        Some(q) => match q.split_once('=') {
            Some(("attribute", v)) if !v.is_empty() && !v.contains(['&', '=']) => Some(v.to_ascii_lowercase()),
            _ => return Err(Malformed("the only query an op:// reference takes is ?attribute=<name>")),
        },
    };
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() != 3 && parts.len() != 4 {
        return Err(Malformed("op:// takes vault/item/field or vault/item/section/field"));
    }
    if parts.iter().any(|p| p.trim().is_empty()) {
        return Err(Malformed("every part of an op:// reference is non-empty"));
    }
    if parts.iter().any(|p| p.len() > MAX_PART) {
        return Err(Malformed("each part of an op:// reference is at most 256 characters"));
    }
    let s = |i: usize| parts[i].to_string();
    Ok(match parts.len() {
        3 => Reference::Op { vault: s(0), item: s(1), section: None, field: s(2), attribute },
        _ => Reference::Op { vault: s(0), item: s(1), section: Some(s(2)), field: s(3), attribute },
    })
}

/// One `{"$secret": ...}` found in a value: where it is (a JSON pointer) and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub pointer: String,
    pub reference: Result<Reference, Malformed>,
    /// The raw text, for the audit row when it does not parse (references are not secrets).
    pub raw: String,
}

/// Whether `v` is a reference object; `Some(Err)` for a `$secret` object that is malformed.
fn as_ref_object(v: &Value) -> Option<Result<String, Malformed>> {
    let obj = v.as_object()?;
    let inner = obj.get(MARKER)?;
    if obj.len() != 1 {
        return Some(Err(Malformed("a {\"$secret\": ...} object has no other keys")));
    }
    Some(inner.as_str().map(str::to_string).ok_or(Malformed("$secret is a string")))
}

/// Every reference object in `v`, depth first, in document order.
pub fn find(v: &Value) -> Vec<Found> {
    let mut out = Vec::new();
    walk(v, String::new(), &mut out);
    out
}

fn walk(v: &Value, at: String, out: &mut Vec<Found>) {
    if let Some(r) = as_ref_object(v) {
        let (raw, reference) = match r {
            Ok(raw) => (raw.clone(), parse(&raw)),
            Err(e) => (String::new(), Err(e)),
        };
        out.push(Found { pointer: at, reference, raw });
        return;
    }
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                walk(child, format!("{at}/{}", k.replace('~', "~0").replace('/', "~1")), out);
            }
        }
        Value::Array(items) => {
            for (i, child) in items.iter().enumerate() {
                walk(child, format!("{at}/{i}"), out);
            }
        }
        _ => {}
    }
}

/// Whether `v` holds any `$secret` object (the cheap check before a call does any broker work).
pub fn contains(v: &Value) -> bool {
    match v {
        Value::Object(map) => map.contains_key(MARKER) || map.values().any(contains),
        Value::Array(items) => items.iter().any(contains),
        _ => false,
    }
}

/// `v` with the reference object at each pointer replaced by its value.
pub fn substitute(v: &Value, values: &[(String, &str)]) -> Value {
    let mut out = v.clone();
    for (pointer, value) in values {
        if let Some(slot) = out.pointer_mut(pointer) {
            *slot = Value::String((*value).to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn op(v: &str, i: &str, s: Option<&str>, f: &str) -> Reference {
        Reference::Op { vault: v.into(), item: i.into(), section: s.map(Into::into), field: f.into(), attribute: None }
    }

    #[test]
    fn op_references() {
        assert_eq!(parse("op://Ops/Square/access_token"), Ok(op("Ops", "Square", None, "access_token")));
        assert_eq!(parse("op://Ops/Square/api/secret"), Ok(op("Ops", "Square", Some("api"), "secret")));
        assert_eq!(parse("OP://My Vault/My Item/my field").map(|r| r.text()), Ok("op://My Vault/My Item/my field".into()));
        let otp = parse("op://Ops/GitHub/one-time password?attribute=OTP").unwrap();
        assert!(matches!(&otp, Reference::Op { attribute: Some(a), .. } if a == "otp"));
        assert_eq!(otp.text(), "op://Ops/GitHub/one-time password?attribute=otp");
    }

    #[test]
    fn bws_references() {
        let r = parse("bws://3F1C2B4A-5D6E-4F70-8A91-B2C3D4E5F607").unwrap();
        assert_eq!(r, Reference::Bws { id: "3f1c2b4a-5d6e-4f70-8a91-b2c3d4e5f607".into() });
        assert_eq!(r.text(), "bws://3f1c2b4a-5d6e-4f70-8a91-b2c3d4e5f607");
        assert_eq!(r.scheme(), "bws");
    }

    #[test]
    fn malformed_references() {
        let long = format!("op://{}/i/f", "v".repeat(257));
        let huge = "x".repeat(1025);
        for bad in [
            "",
            "op://",
            "op://Ops",
            "op://Ops/Square",
            "op://Ops/Square/a/b/c",
            "op:///Square/field",
            "op://Ops//field",
            "op://Ops/Square/ ",
            "op://Ops/Square/field?attribute=",
            "op://Ops/Square/field?foo=bar",
            "op://Ops/Square/field?attribute=otp&x=1",
            "op://Ops/Sq\nuare/field",
            "op://Ops/Square/f\u{7f}",
            long.as_str(),
            "bws://not-a-uuid",
            "bws://3f1c2b4a-5d6e-4f70-8a91-b2c3d4e5f607/extra",
            "bws://3f1c2b4a5d6e4f708a91b2c3d4e5f607",
            "https://example.com/a/b",
            "Ops/Square/field",
            "op:/Ops/Square/field",
            huge.as_str(),
        ] {
            assert!(parse(bad).is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn finds_reference_objects_at_any_depth_and_substitutes_them() {
        let args = json!({
            "a": {"$secret": "op://Ops/Square/access_token"},
            "list": [1, {"headers": {"x/y": {"$secret": "bws://3f1c2b4a-5d6e-4f70-8a91-b2c3d4e5f607"}}}],
            "plain": "op://not/a/reference/object",
        });
        assert!(contains(&args));
        let found = find(&args);
        let pointers: Vec<&str> = found.iter().map(|f| f.pointer.as_str()).collect();
        assert_eq!(pointers, ["/a", "/list/1/headers/x~1y"]);
        assert!(found.iter().all(|f| f.reference.is_ok()));
        let out = substitute(&args, &[("/a".into(), "v1"), ("/list/1/headers/x~1y".into(), "v2")]);
        assert_eq!(out["a"], "v1");
        assert_eq!(out["list"][1]["headers"]["x/y"], "v2");
        assert_eq!(args["a"]["$secret"], "op://Ops/Square/access_token", "the original keeps the reference");
        assert!(!contains(&json!({"text": "hello", "n": [1, 2]})));
    }

    #[test]
    fn malformed_reference_objects() {
        for bad in [json!({"$secret": "op://Ops/Square/f", "extra": 1}), json!({"$secret": 5}), json!({"$secret": "op://bad"}), json!({"$secret": null})] {
            let found = find(&json!({"x": bad}));
            assert_eq!(found.len(), 1, "{bad}");
            assert!(found[0].reference.is_err(), "{bad}");
        }
    }
}
