//! Secret redaction for text an agent asks Eunomia to keep (memories, skills). Each secret of a known
//! shape is replaced by `[redacted: <kind>]` and reported back by kind and count, never by value, so the
//! write still succeeds and the agent learns what was dropped. Known shapes only, no entropy guessing:
//! ordinary prose, record ids, git SHAs and UUIDs pass through untouched.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use serde_json::Value;

/// What one write had redacted: `count` secrets of `kind` (e.g. "github token").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Redaction {
    pub kind: String,
    pub count: usize,
}

/// (kind, pattern, replacement). Most specific first: `sk-ant-` before `sk-`. A replacement other than
/// the plain marker keeps the capture groups around the secret.
static PATTERNS: LazyLock<Vec<(&'static str, Regex, String)>> = LazyLock::new(|| {
    let p = |kind: &'static str, re: &str| (kind, Regex::new(re).expect("redaction pattern"), format!("[redacted: {kind}]"));
    vec![
        // to the END line or, without one (a truncated paste), over its header lines and base64 body: one
        // blank line may open the body (PGP armor), a later one ends it
        p(
            "private key",
            concat!(
                r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----(?:",
                r"(?s:.*?)-----END (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----",
                r"|(?:[ \t]*\r?\n[A-Za-z][A-Za-z0-9-]*: [^\n]*)*(?:(?:[ \t]*\r?\n){1,2}[ \t]*|[ \t]+)[A-Za-z0-9+/=]+",
                r"(?:(?:[ \t]*\r?\n[ \t]*|[ \t]+)[A-Za-z0-9+/=]+)*",
                r"|)",
            ),
        ),
        // only the password, which runs to the last `@` before a host: `redis://:p@ss@db:6379/0` keeps
        // the scheme, the (possibly empty) user and the host
        (
            "url password",
            Regex::new(r"(\b[A-Za-z][A-Za-z0-9+.-]*://[^\s:/@]*:)\S*@([A-Za-z0-9.\[\]-]+(?::[0-9]+)?(?:[^A-Za-z0-9.\[\]:@_-]|$))").expect("redaction pattern"),
            "${1}[redacted: url password]@${2}".into(),
        ),
        p("jwt", r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}"),
        p("anthropic key", r"\bsk-ant-[A-Za-z0-9_-]{20,}"),
        p("openai key", r"\bsk-(?:proj|svcacct|admin)-[A-Za-z0-9_-]{20,}|\bsk-[A-Za-z0-9]{32,}"),
        p("stripe key", r"\b(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{16,}"),
        p("github token", r"\bgh[pousr]_[A-Za-z0-9]{36,}|\bgithub_pat_[A-Za-z0-9_]{22,}"),
        p("aws access key", r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b"),
        p("slack token", r"\bxox[abprs]-[A-Za-z0-9-]{10,}"),
        p("google api key", r"\bAIza[A-Za-z0-9_-]{35}"),
    ]
});

/// `text` with every known secret replaced by its marker. Each kind found is added to `found` (counts
/// summed when the kind is already there, so one report can cover several fields).
pub fn redact(text: &str, found: &mut Vec<Redaction>) -> String {
    let mut out = text.to_string();
    for (kind, re, replacement) in PATTERNS.iter() {
        let count = re.find_iter(&out).count();
        if count == 0 {
            continue;
        }
        out = re.replace_all(&out, replacement.as_str()).into_owned();
        match found.iter_mut().find(|r| r.kind == *kind) {
            Some(r) => r.count += count,
            None => found.push(Redaction { kind: kind.to_string(), count }),
        }
    }
    out
}

/// [`redact`] on every string inside `v`, at any depth, before it is serialized anywhere.
pub fn redact_value(v: &mut Value) {
    match v {
        Value::String(s) => *s = redact(s, &mut Vec::new()),
        Value::Array(a) => a.iter_mut().for_each(redact_value),
        Value::Object(m) => m.values_mut().for_each(redact_value),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> (String, Vec<Redaction>) {
        let mut found = Vec::new();
        (redact(text, &mut found), found)
    }

    #[test]
    fn catches_each_kind() {
        let alnum = "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8s9T0";
        let cases = [
            ("private key", "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA\nabc\n-----END RSA PRIVATE KEY-----".to_string()),
            ("private key", "-----BEGIN PRIVATE KEY-----\nMIIE\n-----END PRIVATE KEY-----".to_string()),
            ("jwt", "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U".to_string()),
            ("anthropic key", format!("sk-ant-api03-{alnum}")),
            ("openai key", format!("sk-proj-{alnum}_-x")),
            ("openai key", format!("sk-{alnum}")),
            ("stripe key", format!("sk_live_{alnum}")),
            ("stripe key", format!("rk_live_{alnum}")),
            ("stripe key", format!("sk_test_{alnum}")),
            ("github token", format!("ghp_{}", &alnum[..36])),
            ("github token", format!("gho_{}", &alnum[..36])),
            ("github token", format!("ghu_{}", &alnum[..36])),
            ("github token", format!("ghs_{}", &alnum[..36])),
            ("github token", format!("ghr_{}", &alnum[..36])),
            ("github token", format!("github_pat_11ABCDEFG0_{alnum}")),
            ("aws access key", "AKIAIOSFODNN7EXAMPLE".to_string()),
            ("aws access key", "ASIAIOSFODNN7EXAMPLE".to_string()),
            ("slack token", "xoxb-123456789012-1234567890123-AbCdEfGhIjKl".to_string()),
            ("slack token", "xoxp-123456789012-abcdef".to_string()),
            ("google api key", format!("AIza{}", &alnum[..35])),
        ];
        for (kind, secret) in cases {
            let (out, found) = run(&format!("the key is {secret} for prod"));
            assert_eq!(out, format!("the key is [redacted: {kind}] for prod"), "{secret}");
            assert_eq!(found, vec![Redaction { kind: kind.into(), count: 1 }], "{secret}");
        }
    }

    #[test]
    fn url_keeps_scheme_user_and_host() {
        let m = "[redacted: url password]";
        for (text, want) in [
            ("connect with postgres://app:hunter2@db.internal:5432/main please", format!("connect with postgres://app:{m}@db.internal:5432/main please")),
            ("redis://:secretpw@cache:6379/0", format!("redis://:{m}@cache:6379/0")),
            ("postgres://u:p@ss@host/db", format!("postgres://u:{m}@host/db")),
            ("postgres://u:pa/ss@host", format!("postgres://u:{m}@host")),
            ("a https://u:x@y.com, b", format!("a https://u:{m}@y.com, b")),
        ] {
            let (out, found) = run(text);
            assert_eq!(out, want, "{text}");
            assert_eq!(found, vec![Redaction { kind: "url password".into(), count: 1 }], "{text}");
        }
    }

    #[test]
    fn private_keys_without_an_end_line_and_pgp_blocks() {
        for (text, want) in [
            ("key:\n-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA\nqW9xZ+/abc=\n\nsee above.", "key:\n[redacted: private key]\n\nsee above."),
            ("-----BEGIN OPENSSH PRIVATE KEY----- b3BlbnNzaC1rZXk AAAABG5vbmU", "[redacted: private key]"),
            (
                "-----BEGIN PGP PRIVATE KEY BLOCK-----\nVersion: GnuPG v2\n\nlQOYBF0abc\n=tWzN\n-----END PGP PRIVATE KEY BLOCK-----\ndone",
                "[redacted: private key]\ndone",
            ),
            ("-----BEGIN PGP PRIVATE KEY BLOCK-----\n\nlQOYBF0abc\nxyz+/", "[redacted: private key]"),
        ] {
            let (out, found) = run(text);
            assert_eq!(out, want, "{text}");
            assert_eq!(found, vec![Redaction { kind: "private key".into(), count: 1 }], "{text}");
        }
    }

    #[test]
    fn redact_value_reaches_nested_strings() {
        let gh = format!("ghp_{}", "x".repeat(36));
        let mut v = serde_json::json!({"a": [{"b": format!("\n{gh}")}], "n": 1});
        redact_value(&mut v);
        assert_eq!(v, serde_json::json!({"a": [{"b": "\n[redacted: github token]"}], "n": 1}));
    }

    #[test]
    fn counts_and_merges_across_calls() {
        let gh = format!("ghp_{}", "x".repeat(36));
        let mut found = Vec::new();
        redact(&format!("{gh} and {gh}"), &mut found);
        redact(&format!("{gh} plus AKIAIOSFODNN7EXAMPLE"), &mut found);
        assert_eq!(found, vec![Redaction { kind: "github token".into(), count: 3 }, Redaction { kind: "aws access key".into(), count: 1 }]);
    }

    #[test]
    fn look_alikes_pass_through() {
        for text in [
            "Avi prefers dark mode and ships on Fridays.",
            "see memory:abc123 and person:01HZX3K9Q2 for context",
            "fixed in 9a0820b2a4b9732eb0cdecda3d700b63dccf290a (short 9a0820b)",
            "request id 550e8400-e29b-41d4-a716-446655440000",
            "she uses sk-learn and scikit-learn for the task-sk model",
            "the ghp_ prefix marks a GitHub token; AKIA starts AWS keys",
            "https://example.com:8080/path?q=1 and mailto:avi@example.com",
            "http://host:8080 and ssh://git@github.com:22/org/repo.git",
            "https://github.com/Cashara-io/eunomia/pull/110",
            "user@host.example ssh git@github.com:org/repo.git",
            "eyJ alone is not a token, nor is a.b.c",
            "-----BEGIN PUBLIC KEY-----\nMFkw\n-----END PUBLIC KEY-----",
            "AKIA lowercase akiaiosfodnn7example and AKIASHORT",
            "xoxb- with nothing after",
            "",
        ] {
            let (out, found) = run(text);
            assert_eq!(out, text);
            assert!(found.is_empty(), "{text}: {found:?}");
        }
    }
}
