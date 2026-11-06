//! `http_request` (docs/architecture/specs/secret-requests.md, "The consumer"): the one built-in tool that
//! takes resolved secrets. It sends one HTTPS request whose headers, query or body hold `eu://` handles,
//! to a host the person approved for every handle in the call, through an SSRF guard, and returns the
//! status, content type and body with every resolved value (and its encoded forms) scrubbed.
//!
//! The broker has already resolved the handles when the tool runs; it hands over, in a task-local
//! ([`with_grants`]), where each value sits in the arguments and which hosts and placements it allows.
//!
//! The guard: `https` only, a host name (never an IP literal), no credentials in the URL; every address
//! the name resolves to is checked when the connection is made (a guarded resolver, so DNS rebinding to
//! a forbidden address fails): loopback, private, CGNAT, link-local and cloud metadata, unique local,
//! multicast, documentation, unspecified, and this stack's database. No redirects.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use surrealdb::types::RecordId;

use crate::error::{AppError, AppResult, ErrorCode};

pub const METHODS: &[&str] = &["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"];
const MAX_HEADERS: usize = 32;
const MAX_BODY: usize = 64 * 1024;
const MAX_READ: usize = 256 * 1024;
const MAX_RETURN: usize = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const TIMEOUT: Duration = Duration::from_secs(15);
/// Headers a caller may not set: they would let a request smuggle or reroute itself.
const FORBIDDEN_HEADERS: &[&str] = &["host", "content-length", "transfer-encoding", "connection", "upgrade", "te", "trailer", "cookie", "keep-alive", "expect"];

/// One resolved reference of the call: where it is in the arguments (a JSON pointer) and what the person
/// approved for it. op:// and bws:// references carry no hosts (`None`).
#[derive(Debug, Clone)]
pub struct Grant {
    pub pointer: String,
    pub handle: String,
    pub hosts: Option<Vec<String>>,
    pub placements: Option<Vec<String>>,
}

tokio::task_local! {
    static GRANTS: Arc<Vec<Grant>>;
}

/// Runs a tool call with the broker's grants in scope.
pub async fn with_grants<F: std::future::Future>(grants: Arc<Vec<Grant>>, fut: F) -> F::Output {
    GRANTS.scope(grants, fut).await
}

fn grants() -> Arc<Vec<Grant>> {
    GRANTS.try_with(Arc::clone).unwrap_or_default()
}

// ---------------------------------------------------------------- the guard

/// Test builds: host names that resolve to a fixed local address and may be plain http (the mock upstream
/// in `tests/secret_requests.rs`). Empty, and absent, in a release build.
#[cfg(feature = "test-support")]
pub static TEST_TARGETS: std::sync::Mutex<Vec<(String, IpAddr)>> = std::sync::Mutex::new(Vec::new());
/// Test builds: names that resolve to these addresses through the guard (the rebinding test).
#[cfg(feature = "test-support")]
pub static TEST_DNS: std::sync::Mutex<Vec<(String, Vec<IpAddr>)>> = std::sync::Mutex::new(Vec::new());

#[cfg(feature = "test-support")]
fn test_target(host: &str) -> Option<IpAddr> {
    TEST_TARGETS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(h, _)| h == host).map(|(_, ip)| *ip)
}

#[cfg(not(feature = "test-support"))]
fn test_target(_host: &str) -> Option<IpAddr> {
    None
}

async fn lookup(host: &str) -> std::io::Result<Vec<IpAddr>> {
    #[cfg(feature = "test-support")]
    if let Some((_, ips)) = TEST_DNS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(h, _)| h == host) {
        return Ok(ips.clone());
    }
    let found = tokio::time::timeout(Duration::from_secs(5), tokio::net::lookup_host((host, 443)))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "lookup timed out"))??;
    Ok(found.map(|a| a.ip()).collect())
}

/// Why an address may never be the target of an outbound call, or `None` when it may.
pub fn ip_refusal(ip: IpAddr) -> Option<&'static str> {
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    };
    let metadata = match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80 || v6.segments()[..6] == [0xfd00, 0x0ec2, 0, 0, 0, 0],
    };
    let cgnat = matches!(ip, IpAddr::V4(v4) if v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1]));
    let benchmark = matches!(ip, IpAddr::V4(v4) if v4.octets()[0] == 198 && (18..20).contains(&v4.octets()[1]));
    let reserved = matches!(ip, IpAddr::V4(v4) if v4.octets()[0] >= 240 || v4.octets()[0] == 0);
    let unique_local = matches!(ip, IpAddr::V6(v6) if (v6.segments()[0] & 0xfe00) == 0xfc00);
    if metadata {
        Some("a link-local or cloud metadata address")
    } else if ip.is_loopback() {
        Some("a loopback address")
    } else if crate::oauth::cimd::ip_blocked(ip) || cgnat || benchmark || reserved || unique_local {
        Some("a private, reserved or internal address")
    } else {
        None
    }
}

/// The compose service `surrealdb`'s address, resolved once (`None` outside compose).
async fn surrealdb_ip() -> Option<IpAddr> {
    static IP: tokio::sync::OnceCell<Option<IpAddr>> = tokio::sync::OnceCell::const_new();
    *IP.get_or_init(|| async { tokio::net::lookup_host(("surrealdb", 8000)).await.ok().and_then(|mut v| v.next()).map(|a| a.ip()) }).await
}

struct GuardedResolver;

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            if let Some(ip) = test_target(&host) {
                return Ok(Box::new(std::iter::once(SocketAddr::new(ip, 0))) as reqwest::dns::Addrs);
            }
            let ips = lookup(&host).await?;
            if ips.is_empty() {
                return Err("the host did not resolve".into());
            }
            let db = surrealdb_ip().await;
            for ip in &ips {
                if let Some(why) = ip_refusal(*ip) {
                    return Err(format!("blocked: {why}").into());
                }
                if db.is_some_and(|d| d == *ip) {
                    return Err("blocked: this stack's database".into());
                }
            }
            Ok(Box::new(ips.into_iter().map(|ip| SocketAddr::new(ip, 0))) as reqwest::dns::Addrs)
        })
    }
}

/// The URL's host and port when it may be called, or why not (`secret.outbound_blocked`).
pub fn check_url(raw: &str) -> Result<(url::Url, String, u16), String> {
    let url = url::Url::parse(raw.trim()).map_err(|_| "the url is not a valid URL".to_string())?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("the url holds a user or password; put credentials in a header handle".into());
    }
    let host = match url.host() {
        Some(url::Host::Domain(d)) => d.trim_end_matches('.').to_ascii_lowercase(),
        Some(_) => return Err("the url names an IP address; outbound calls go only to approved host names".into()),
        None => return Err("the url has no host".into()),
    };
    let test = test_target(&host).is_some();
    match url.scheme() {
        "https" => {}
        "http" if test => {}
        _ => return Err("the url is not https".into()),
    }
    if url.fragment().is_some() {
        return Err("the url has a fragment".into());
    }
    let port = url.port_or_known_default().unwrap_or(443);
    Ok((url, host, port))
}

/// Which approved placement a JSON pointer into `http_request`'s arguments is, if any.
fn placement_of(pointer: &str) -> Option<&'static str> {
    let first = pointer.trim_start_matches('/').split('/').next().unwrap_or_default();
    match first {
        "headers" => Some("header"),
        "query" => Some("query"),
        "body" | "json" => Some("body"),
        _ => None,
    }
}

/// Every grant must allow this host and its own placement; `Err` is the reason (`secret.host_denied`).
pub fn check_grants(grants: &[Grant], host: &str, port: u16) -> Result<(), String> {
    if grants.is_empty() {
        return Err("http_request sends at least one secret handle ({\"$secret\": \"eu://...\"}); it is not a general fetch tool".into());
    }
    for g in grants {
        let Some(hosts) = &g.hosts else {
            return Err(format!("{} has no approved hosts (op:// and bws:// references cannot be sent by http_request yet)", g.handle));
        };
        if !hosts.iter().any(|p| super::held::host_allowed(p, host, port)) {
            let shown = if port == 443 { host.to_string() } else { format!("{host}:{port}") };
            return Err(format!("{} may not be sent to {shown}; it is approved for {}", g.handle, if hosts.is_empty() { "no host".to_string() } else { hosts.join(", ") }));
        }
        let Some(place) = placement_of(&g.pointer) else {
            return Err(format!("{} is at {}; a handle goes only in headers, query or body", g.handle, g.pointer));
        };
        if !g.placements.as_ref().is_some_and(|p| p.iter().any(|x| x == place)) {
            return Err(format!("{} may not go in the {place}; it is approved for {}", g.handle, g.placements.as_ref().map(|p| p.join(", ")).unwrap_or_default()));
        }
    }
    Ok(())
}

pub(crate) fn client() -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .dns_resolver(Arc::new(GuardedResolver))
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TIMEOUT)
        .no_proxy()
        .build()
        .map_err(|_| AppError::internal("the outbound client could not be built"))
}

// ---------------------------------------------------------------- the tool

#[derive(serde::Deserialize)]
struct Args {
    #[serde(default = "get")]
    method: String,
    url: String,
    #[serde(default)]
    headers: serde_json::Map<String, Value>,
    #[serde(default)]
    query: serde_json::Map<String, Value>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    json: Option<Value>,
}

fn get() -> String {
    "GET".into()
}

fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// The current call's exact values plus their encoded forms, and #111's detectors, over `text`.
fn scrub(text: &str) -> String {
    let values = crate::redact::current_exact();
    if values.iter().any(|v| v.chars().count() < 8) {
        return "[response withheld for a short secret]".into();
    }
    let all = super::held::with_encoded_forms(values.iter().cloned().collect());
    let (out, _) = crate::redact::scrub_exact(text, &all);
    crate::redact::redact(&out, &mut Vec::new())
}

/// `http_request`'s handler. `args` holds the resolved values; the grants say where they are.
pub async fn run(state: &crate::state::OrgState, user: &RecordId, args: Value) -> AppResult<Value> {
    let a: Args = match serde_json::from_value(args) {
        Ok(a) => a,
        Err(e) => return Ok(json!({"error": format!("invalid arguments: {e}")})),
    };
    let fail = |code: ErrorCode, m: String| Ok(AppError::coded(code, m).to_tool_value());
    let method = a.method.trim().to_ascii_uppercase();
    if !METHODS.contains(&method.as_str()) {
        return fail(ErrorCode::ValidationInvalid, format!("method is one of {}", METHODS.join(", ")));
    }
    let grants = grants();
    let handles: Vec<&str> = grants.iter().map(|g| g.handle.as_str()).collect();
    // a refused call is audited too, so an attempt to send a secret elsewhere shows in the log (host only)
    let (mut url, host, port) = match check_url(&a.url) {
        Ok(x) => x,
        Err(m) => {
            let host = url::Url::parse(&a.url).ok().and_then(|u| u.host_str().map(str::to_ascii_lowercase)).unwrap_or_default();
            audit_http(state, user, &host, &method, 0, "blocked", &handles).await;
            return fail(ErrorCode::SecretOutboundBlocked, m);
        }
    };
    if let Err(m) = check_grants(&grants, &host, port) {
        audit_http(state, user, &host, &method, 0, "denied", &handles).await;
        return fail(ErrorCode::SecretHostDenied, m);
    }
    if a.headers.len() > MAX_HEADERS {
        return fail(ErrorCode::ValidationInvalid, format!("at most {MAX_HEADERS} headers"));
    }
    let mut headers = reqwest::header::HeaderMap::new();
    for (k, v) in &a.headers {
        let lower = k.to_ascii_lowercase();
        if FORBIDDEN_HEADERS.contains(&lower.as_str()) || lower.starts_with("proxy-") {
            return fail(ErrorCode::ValidationInvalid, format!("the header {k} cannot be set"));
        }
        let (Ok(name), Some(Ok(value))) = (reqwest::header::HeaderName::from_bytes(k.as_bytes()), scalar(v).map(|s| reqwest::header::HeaderValue::from_str(&s))) else {
            return fail(ErrorCode::ValidationInvalid, format!("the header {k} is not a valid header name and text value"));
        };
        headers.insert(name, value);
    }
    if !a.query.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (k, v) in &a.query {
            let Some(s) = scalar(v) else { return fail(ErrorCode::ValidationInvalid, format!("the query value {k} is not text")) };
            pairs.append_pair(k, &s);
        }
    }
    let body: Option<(Vec<u8>, &'static str)> = match (&a.body, &a.json) {
        (Some(_), Some(_)) => return fail(ErrorCode::ValidationInvalid, "body and json cannot both be given".into()),
        (Some(b), None) => Some((b.as_bytes().to_vec(), "text/plain; charset=utf-8")),
        (None, Some(j)) => Some((j.to_string().into_bytes(), "application/json")),
        (None, None) => None,
    };
    if body.as_ref().is_some_and(|(b, _)| b.len() > MAX_BODY) {
        return fail(ErrorCode::ValidationInvalid, format!("the body is at most {MAX_BODY} bytes"));
    }
    let mut req = client()?.request(reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET), url).headers(headers);
    if let Some((b, ct)) = body {
        if !a.headers.keys().any(|k| k.eq_ignore_ascii_case("content-type")) {
            req = req.header(reqwest::header::CONTENT_TYPE, ct);
        }
        req = req.body(b);
    }

    let started = std::time::Instant::now();
    let sent = req.send().await;
    let mut res = match sent {
        Ok(r) => r,
        Err(e) => {
            // reqwest's text can hold the URL and so a query value: report the kind only
            let (code, kind) = if e.is_timeout() {
                (ErrorCode::SecretOutboundFailed, "timeout")
            } else if blocked_by_guard(&e) {
                (ErrorCode::SecretOutboundBlocked, "blocked")
            } else {
                (ErrorCode::SecretOutboundFailed, "connect")
            };
            audit_http(state, user, &host, &method, 0, kind, &handles).await;
            let msg = match kind {
                "blocked" => format!("{host} resolves to an address outbound calls never reach (loopback, private, link-local or metadata)"),
                "timeout" => format!("{host} did not answer in {} seconds", TIMEOUT.as_secs()),
                _ => format!("could not connect to {host} (connection or TLS failed)"),
            };
            return fail(code, msg);
        }
    };
    let status = res.status();
    let content_type = res.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    let mut buf: Vec<u8> = Vec::new();
    let mut truncated = false;
    loop {
        match res.chunk().await {
            Ok(Some(c)) => {
                if buf.len() + c.len() > MAX_READ {
                    buf.extend_from_slice(&c[..MAX_READ - buf.len()]);
                    truncated = true;
                    break;
                }
                buf.extend_from_slice(&c);
            }
            Ok(None) => break,
            Err(_) => {
                truncated = true;
                break;
            }
        }
    }
    let text = match String::from_utf8(buf) {
        Ok(mut t) => {
            if t.len() > MAX_RETURN {
                let mut cut = MAX_RETURN;
                while !t.is_char_boundary(cut) {
                    cut -= 1;
                }
                t.truncate(cut);
                truncated = true;
            }
            scrub(&t)
        }
        Err(e) => format!("<{} bytes of binary content>", e.as_bytes().len()),
    };
    audit_http(state, user, &host, &method, status.as_u16(), "ok", &handles).await;
    tracing::info!(host = %host, status = status.as_u16(), ms = started.elapsed().as_millis() as u64, "http_request");
    Ok(json!({
        "status": status.as_u16(),
        "ok": status.is_success(),
        "redirect": status.is_redirection(),
        "content_type": scrub(&content_type),
        "body": text,
        "truncated": truncated,
    }))
}

/// Whether the guarded resolver refused the connection (its error is somewhere in the source chain).
fn blocked_by_guard(e: &reqwest::Error) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(e);
    while let Some(err) = cur {
        if err.to_string().starts_with("blocked:") {
            return true;
        }
        cur = err.source();
    }
    false
}

async fn audit_http(state: &crate::state::OrgState, user: &RecordId, host: &str, method: &str, status: u16, outcome: &str, handles: &[&str]) {
    let ctx = crate::usage::current_context();
    let detail = json!({"host": host, "method": method, "status": status, "handles": handles, "channel": ctx.channel.as_str(), "session": ctx.session}).to_string();
    super::requests::audit(state, user, "secret.http", outcome, &detail).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(pointer: &str, hosts: Option<&[&str]>, placements: &[&str]) -> Grant {
        Grant {
            pointer: pointer.into(),
            handle: "eu://v/k".into(),
            hosts: hosts.map(|h| h.iter().map(|x| x.to_string()).collect()),
            placements: Some(placements.iter().map(|x| x.to_string()).collect()),
        }
    }

    #[test]
    fn the_guard_refuses_internal_addresses() {
        for ip in [
            "127.0.0.1",
            "127.1.2.3",
            "10.0.0.1",
            "172.16.5.4",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "100.100.100.200",
            "0.0.0.0",
            "198.18.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "224.0.0.1",
            "::1",
            "::",
            "fe80::1",
            "fd00:ec2::254",
            "fc00::1",
            "fd12:3456::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
            "::ffff:10.1.2.3",
            "ff02::1",
        ] {
            assert!(ip_refusal(ip.parse().unwrap()).is_some(), "{ip} allowed");
        }
        for ip in ["93.184.216.34", "1.1.1.1", "2606:4700:4700::1111", "::ffff:93.184.216.34"] {
            assert!(ip_refusal(ip.parse().unwrap()).is_none(), "{ip} refused");
        }
    }

    #[test]
    fn urls_are_https_host_names_without_credentials() {
        assert_eq!(check_url("https://api.example.com/v1?x=1").map(|(_, h, p)| (h, p)), Ok(("api.example.com".into(), 443)));
        assert_eq!(check_url("https://API.example.com:8443/").map(|(_, h, p)| (h, p)), Ok(("api.example.com".into(), 8443)));
        for bad in [
            "http://api.example.com/",
            "ftp://api.example.com/",
            "https://127.0.0.1/",
            "https://[::1]/",
            "https://10.0.0.1/",
            "https://0x7f000001/",
            "https://2130706433/",
            "https://user:pw@api.example.com/",
            "https://token@api.example.com/",
            "https://api.example.com/#frag",
            "file:///etc/passwd",
            "not a url",
        ] {
            assert!(check_url(bad).is_err(), "{bad} accepted");
        }
    }

    #[test]
    fn every_handle_must_allow_the_host_and_its_placement() {
        let ok = [g("/headers/Authorization", Some(&["api.example.com"]), &["header"])];
        assert!(check_grants(&ok, "api.example.com", 443).is_ok());
        assert!(check_grants(&ok, "api.example.com", 8443).is_err(), "another port");
        assert!(check_grants(&ok, "evil.example.com", 443).unwrap_err().contains("may not be sent to evil.example.com"));
        assert!(check_grants(&[], "api.example.com", 443).unwrap_err().contains("not a general fetch tool"));
        let two = [g("/headers/A", Some(&["api.example.com"]), &["header"]), g("/headers/B", Some(&["other.example.com"]), &["header"])];
        assert!(check_grants(&two, "api.example.com", 443).is_err(), "the second handle is not approved for this host");
        let op_ref = [g("/headers/A", None, &["header"])];
        assert!(check_grants(&op_ref, "api.example.com", 443).unwrap_err().contains("op:// and bws://"));
        let in_query = [g("/query/key", Some(&["api.example.com"]), &["header"])];
        assert!(check_grants(&in_query, "api.example.com", 443).unwrap_err().contains("may not go in the query"));
        let in_body = [g("/json/auth/token", Some(&["api.example.com"]), &["header", "body"])];
        assert!(check_grants(&in_body, "api.example.com", 443).is_ok());
        let in_url = [g("/url", Some(&["api.example.com"]), &["header", "query", "body"])];
        assert!(check_grants(&in_url, "api.example.com", 443).unwrap_err().contains("headers, query or body"));
        let in_method = [g("/method", Some(&["api.example.com"]), &["header"])];
        assert!(check_grants(&in_method, "api.example.com", 443).is_err());
    }
}
