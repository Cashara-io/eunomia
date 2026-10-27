//! Secret redaction for text an agent asks Eunomia to keep (memories, skills). Each secret of a known
//! shape is replaced by `[redacted: <kind>]` and reported back by kind and count, never by value, so the
//! write still succeeds and the agent learns what was dropped. Known shapes only, no entropy guessing:
//! ordinary prose, record ids, git SHAs and UUIDs pass through untouched.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

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
        p("private key", r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY-----(?s:.*?)-----END (?:[A-Z0-9]+ )*PRIVATE KEY-----"),
        // only the password: `postgres://app:hunter2@db` keeps the user and host
        ("url password", Regex::new(r"(\b[A-Za-z][A-Za-z0-9+.-]*://[^\s:/@]+:)[^\s@/]+@").expect("redaction pattern"), "${1}[redacted: url password]@".into()),
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
    fn url_keeps_user_and_host() {
        let (out, found) = run("connect with postgres://app:hunter2@db.internal:5432/main please");
        assert_eq!(out, "connect with postgres://app:[redacted: url password]@db.internal:5432/main please");
        assert_eq!(found, vec![Redaction { kind: "url password".into(), count: 1 }]);
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
