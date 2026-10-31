//! Secret redaction for text an agent asks Eunomia to keep (memories, skills). Each secret of a known
//! shape is replaced by `[redacted: <kind>]` and reported back by kind and count, never by value, so the
//! write still succeeds and the agent learns what was dropped. Known shapes only, no entropy guessing:
//! ordinary prose, record ids, git SHAs and UUIDs pass through untouched.

use std::sync::{Arc, LazyLock};

use zeroize::Zeroizing;

use regex::Regex;
use serde::Serialize;
use serde_json::Value;

/// Bump when a detector is added or widened: each org's stored text is then redacted again once
/// (`jobs::redact_backfill`), so rows written before the new shape was known lose it too.
/// 2: the password managers' own tokens (1Password service accounts, Bitwarden machine accounts).
pub const DETECTOR_VERSION: i64 = 2;

/// What one write had redacted: `count` secrets of `kind` (e.g. "github token").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Redaction {
    pub kind: String,
    pub count: usize,
}

static PEM_BEGIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----").expect("pem begin"));
static PEM_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-----END (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----").expect("pem end"));
/// A block's body when it has no END line (a truncated paste): its header lines and base64. One blank line
/// may open the body (PGP armor), a later one ends it. Anchored, so it only reads as far as it matches.
static PEM_BODY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^(?:[ \t]*\r?\n[A-Za-z][A-Za-z0-9-]*: [^\n]*)*(?:(?:[ \t]*\r?\n){1,2}[ \t]*|[ \t]+)[A-Za-z0-9+/=]+",
        r"(?:(?:[ \t]*\r?\n[ \t]*|[ \t]+)[A-Za-z0-9+/=]+)*",
    ))
    .expect("pem body")
});
/// Farthest an END line may sit from its BEGIN. A PEM key is a few KB (RSA 4096 is ~3.3 KB) and an armored
/// PGP secret key with subkeys is usually under 20 KB; past this the block is redacted as unterminated.
const PEM_MAX_BLOCK: usize = 64 * 1024;

/// Private key blocks, BEGIN to END, or BEGIN over its body when no END follows within [`PEM_MAX_BLOCK`].
/// Linear: one pass each for BEGIN and END markers, and the blocks it redacts never overlap. (A single
/// regex with a lazy `.*?` to the END line rescans the rest of the text from every BEGIN.)
fn redact_private_keys(text: &str) -> (String, usize) {
    let ends: Vec<_> = PEM_END.find_iter(text).collect();
    let (mut out, mut last, mut next_end, mut count) = (String::with_capacity(text.len()), 0, 0, 0);
    for begin in PEM_BEGIN.find_iter(text) {
        if begin.start() < last {
            continue; // inside a block already redacted
        }
        while ends.get(next_end).is_some_and(|e| e.start() < begin.end()) {
            next_end += 1;
        }
        let stop = match ends.get(next_end) {
            Some(e) if e.end() - begin.start() <= PEM_MAX_BLOCK => e.end(),
            _ => begin.end() + PEM_BODY.find(&text[begin.end()..]).map_or(0, |m| m.end()),
        };
        out.push_str(&text[last..begin.start()]);
        out.push_str("[redacted: private key]");
        (last, count) = (stop, count + 1);
    }
    out.push_str(&text[last..]);
    (out, count)
}

/// (kind, pattern, replacement), after the private keys. Most specific first: `sk-ant-` before `sk-`. A
/// replacement other than the plain marker keeps the capture groups around the secret.
static PATTERNS: LazyLock<Vec<(&'static str, Regex, String)>> = LazyLock::new(|| {
    let p = |kind: &'static str, re: &str| (kind, Regex::new(re).expect("redaction pattern"), format!("[redacted: {kind}]"));
    vec![
        // only the password, which runs to the last `@` before a host: `redis://:p@ss@db:6379/0` keeps
        // the scheme, the (possibly empty) user and the host
        (
            "url password",
            Regex::new(r"(\b[A-Za-z][A-Za-z0-9+.-]*://[^\s:/@]*:)\S*@([A-Za-z0-9.\[\]-]+(?::[0-9]+)?(?:[^A-Za-z0-9.\[\]:@_-]|$))").expect("redaction pattern"),
            "${1}[redacted: url password]@${2}".into(),
        ),
        // 1Password service account token: `ops_` and a long base64url string (1Password documents the
        // prefix for scanners; the tokens run to about a kilobyte, so 80 characters rules out names like ops_team)
        p("1password service account token", r"\bops_[A-Za-z0-9_-]{80,}={0,2}"),
        // Bitwarden Secrets Manager machine account access token: 0.<access token id>.<client secret>:<16-byte key, base64>
        p(
            "bitwarden access token",
            r"\b0\.[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\.[A-Za-z0-9]{20,}:[A-Za-z0-9+/]{21,22}={0,2}",
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

/// Compiles every pattern now, at boot ([`crate::state::AppState::build`]). Compiling takes ~150 KiB of
/// stack in a debug build; left lazy, the first write pays it at the bottom of a deep tool call, which
/// overflowed a 2 MiB test thread. A search on compiled patterns needs under 20 KiB.
pub fn init() {
    for re in [&PEM_BEGIN, &PEM_END, &PEM_BODY] {
        LazyLock::force(re);
    }
    LazyLock::force(&PATTERNS);
}

/// `text` with every known secret replaced by its marker. Each kind found is added to `found` (counts
/// summed when the kind is already there, so one report can cover several fields).
pub fn redact(text: &str, found: &mut Vec<Redaction>) -> String {
    let mut note = |kind: &str, count: usize| match found.iter_mut().find(|r| r.kind == kind) {
        Some(r) => r.count += count,
        None => found.push(Redaction { kind: kind.to_string(), count }),
    };
    // first the exact values the credential broker resolved for the call in progress, if any
    let (text, exact) = EXACT.try_with(|v| scrub_exact(text, v)).unwrap_or_else(|_| (text.to_string(), 0));
    if exact > 0 {
        note(EXACT_KIND, exact);
    }
    let text = text.as_str();
    let (mut out, keys) = redact_private_keys(text);
    if keys > 0 {
        note("private key", keys);
    }
    for (kind, re, replacement) in PATTERNS.iter() {
        let count = re.find_iter(&out).count();
        if count == 0 {
            continue;
        }
        out = re.replace_all(&out, replacement.as_str()).into_owned();
        note(kind, count);
    }
    out
}

/// [`redact`] on every string inside `v`, at any depth, before it is serialized anywhere.
/// The kind (and so the marker, `[redacted: secret]`) for a value the credential broker resolved.
pub const EXACT_KIND: &str = "secret";
const EXACT_MARKER: &str = "[redacted: secret]";

tokio::task_local! {
    /// The values the broker resolved for the tool call in progress (`secrets::broker`).
    static EXACT: Arc<Vec<Zeroizing<String>>>;
}

/// Runs a tool call with the broker's resolved `values` in scope: every [`redact`] inside it (the memory,
/// skill and audit write paths) replaces them first, exactly, whatever their shape.
pub async fn with_exact<F: std::future::Future>(values: Arc<Vec<Zeroizing<String>>>, fut: F) -> F::Output {
    EXACT.scope(values, fut).await
}

/// `text` with each exact value replaced by `[redacted: secret]`, longest first (so a value that
/// contains another is not left half replaced), and how many were replaced.
pub fn scrub_exact(text: &str, values: &[Zeroizing<String>]) -> (String, usize) {
    let mut sorted: Vec<&str> = values.iter().map(|v| v.as_str()).filter(|v| !v.is_empty()).collect();
    sorted.sort_by_key(|v| std::cmp::Reverse(v.len()));
    let mut out = text.to_string();
    let mut n = 0;
    for v in sorted {
        let c = out.matches(v).count();
        if c > 0 {
            n += c;
            out = out.replace(v, EXACT_MARKER);
        }
    }
    (out, n)
}

/// [`scrub_exact`] on every string inside `v` (keys too: a value used as an object key goes as well).
pub fn scrub_exact_value(v: &mut Value, values: &[Zeroizing<String>]) -> usize {
    match v {
        Value::String(s) => {
            let (out, n) = scrub_exact(s, values);
            *s = out;
            n
        }
        Value::Array(a) => a.iter_mut().map(|x| scrub_exact_value(x, values)).sum(),
        Value::Object(m) => {
            let mut n = 0;
            let entries: Vec<(String, Value)> = std::mem::take(m).into_iter().collect();
            for (k, mut x) in entries {
                n += scrub_exact_value(&mut x, values);
                let (k, kn) = scrub_exact(&k, values);
                n += kn;
                m.insert(k, x);
            }
            n
        }
        _ => 0,
    }
}

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
    fn password_manager_tokens() {
        let ops = format!("ops_{}", "eyJzaWduSW5BZGRyZXNzIjoibXkuMXBhc3N3b3JkLmNvbSJ9".repeat(3));
        let bws = "0.6b2f0a8e-1c3d-4e5f-8a9b-0c1d2e3f4a5b.e2eClientSecretNotReal012345:ABEiM0RVZneImaq7zN3u/w==";
        for (kind, secret) in [("1password service account token", ops.as_str()), ("bitwarden access token", bws)] {
            let (out, found) = run(&format!("token: {secret} end"));
            assert_eq!(out, format!("token: [redacted: {kind}] end"), "{secret}");
            assert_eq!(found, vec![Redaction { kind: kind.into(), count: 1 }]);
        }
        // look-alikes stay
        for fine in ["ops_team is on call", "ops_dashboard_v2", "version 0.6b2f0a8e-1c3d-4e5f-8a9b-0c1d2e3f4a5b is a uuid", "1.0.3:abc"] {
            assert_eq!(run(fine).0, fine);
        }
    }

    #[tokio::test]
    async fn exact_values_in_scope_are_scrubbed_first() {
        let values = Arc::new(vec![Zeroizing::new("plain-looking-value".to_string()), Zeroizing::new("plain".to_string())]);
        let (out, found) = with_exact(values.clone(), async { run("a plain-looking-value and plain words") }).await;
        assert_eq!(out, "a [redacted: secret] and [redacted: secret] words");
        assert_eq!(found, vec![Redaction { kind: "secret".into(), count: 2 }]);
        assert_eq!(run("plain-looking-value").0, "plain-looking-value", "outside the scope nothing is exact-scrubbed");
        let mut v = serde_json::json!({"a": ["x plain-looking-value"], "plain-looking-value": 1});
        assert_eq!(scrub_exact_value(&mut v, &values), 2);
        assert_eq!(v, serde_json::json!({"a": ["x [redacted: secret]"], "[redacted: secret]": 1}));
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
            ("a\r\n-----BEGIN EC PRIVATE KEY-----\r\nMHcCAQEE\r\nIBx+/=\r\n-----END EC PRIVATE KEY-----\r\nb", "a\r\n[redacted: private key]\r\nb"),
            ("-----BEGIN PRIVATE KEY-----\r\nMHcCAQEE\r\nIBx+/=\r\n\r\nsee above", "[redacted: private key]\r\n\r\nsee above"),
            ("-----BEGIN PRIVATE KEY-----", "[redacted: private key]"),
        ] {
            let (out, found) = run(text);
            assert_eq!(out, want, "{text}");
            assert_eq!(found, vec![Redaction { kind: "private key".into(), count: 1 }], "{text}");
        }
    }

    #[test]
    fn an_end_line_past_the_cap_leaves_the_block_unterminated() {
        let filler = "words ".repeat(PEM_MAX_BLOCK / 6 + 1);
        let text = format!("-----BEGIN PRIVATE KEY-----\nMIIE\n\n{filler}\n-----END PRIVATE KEY-----");
        let (out, found) = run(&text);
        assert_eq!(out, format!("[redacted: private key]\n\n{filler}\n-----END PRIVATE KEY-----"));
        assert_eq!(found, vec![Redaction { kind: "private key".into(), count: 1 }]);
    }

    #[test]
    fn a_mib_of_begin_markers_redacts_in_linear_time() {
        let m = "-----BEGIN PRIVATE KEY-----";
        let block = format!("{m}\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASC\n");
        for (text, n) in [(m.repeat((1 << 20) / m.len()), (1 << 20) / m.len()), (block.repeat((1 << 20) / block.len()), (1 << 20) / block.len())] {
            let started = std::time::Instant::now();
            let (out, found) = run(&text);
            // the quadratic regex took ~55 s here in release; linear is a few ms (tens in debug)
            assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
            assert_eq!(found, vec![Redaction { kind: "private key".into(), count: n }]);
            assert!(!out.contains("BEGIN") && !out.contains("MIIE"));
        }
    }

    #[test]
    fn after_init_a_redaction_fits_a_small_stack() {
        // compiling the patterns takes ~150 KiB of stack in debug; init() pays that once at boot, so a
        // write deep in a tool call only searches (under 20 KiB). Any pattern init() misses fails here.
        init();
        let text = format!("ghp_{} postgres://u:p@h -----BEGIN PRIVATE KEY-----\nMIIE", "a".repeat(36));
        let out = std::thread::Builder::new().stack_size(128 * 1024).spawn(move || run(&text).0).unwrap().join().unwrap();
        assert!(!out.contains("ghp_") && !out.contains("MIIE"), "{out}");
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
