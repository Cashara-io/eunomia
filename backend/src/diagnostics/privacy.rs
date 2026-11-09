//! Diagnostics contain operational metadata only. Arbitrary prose cannot be made safe by regexes.
use std::sync::LazyLock;
use regex::Regex;
use serde_json::Value;

static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| vec![
    Regex::new(r"(?i)[a-z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-z0-9.-]+\.[a-z]{2,}").unwrap(),
    Regex::new(r#"(?i)(?:bearer|basic)\s+[a-z0-9+/_.=:-]+"#).unwrap(),
    Regex::new(r#"(?i)(?:api[_-]?key|access[_-]?token|refresh[_-]?token|token|password|passwd|secret|authorization)[\s"':=]+[^\s,;"'}]+"#).unwrap(),
    Regex::new(r"\b[a-zA-Z0-9_+/=-]{40,}\b").unwrap(),
]);

pub fn scrub(text: &str) -> String {
    let mut out = crate::redact::redact(text, &mut Vec::new());
    for re in PATTERNS.iter() {
        out = re.replace_all(&out, "[redacted]").into_owned();
    }
    out
}

/// JSON is scrubbed before serialization so replacement never corrupts its structure.
pub fn scrub_value(value: &mut Value) {
    match value {
        Value::String(s) => *s = scrub(s),
        Value::Array(a) => a.iter_mut().for_each(scrub_value),
        Value::Object(o) => {
            let fields = std::mem::take(o);
            for (key, mut value) in fields {
                scrub_value(&mut value);
                o.insert(scrub(&key), value);
            }
        }
        _ => {}
    }
}

pub fn error_summary(text: &str) -> Value {
    let text = scrub(text);
    if text.is_empty() { return Value::Null; }
    // Provider messages may quote documents or emails. Export only recognised error categories.
    let lower = text.to_lowercase();
    let category = if lower.contains("429") || lower.contains("rate limit") { "rate_limited" }
        else if lower.contains("401") || lower.contains("403") { "authentication" }
        else if lower.contains("timeout") || lower.contains("timed out") { "timeout" }
        else if lower.contains("partial") || lower.contains("failed to save") { "partial" }
        else { "error_free_text_omitted" };
    Value::String(category.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removes_emails_credentials_jwts_and_private_keys() {
        for secret in ["person@example.com", "password=canary-password", "api_key: canary-key", "Bearer canary-token",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJjYW5hcnkifQ.signaturecanary", "sk-proj-abcdefghijklmnopqrstuvwxyz1234567890",
            "-----BEGIN PRIVATE KEY-----\nY2FuYXJ5c2VjcmV0\n-----END PRIVATE KEY-----"] {
            let out = scrub(secret);
            assert!(!out.contains(secret), "secret survived");
            assert!(!out.contains("canary-password") && !out.contains("Y2FuYXJ5c2VjcmV0"));
        }
    }
}
