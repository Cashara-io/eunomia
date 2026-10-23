//! Round 6 review fixes: outbound HTTP goes through the URL guard; login lockout is per account and address.

mod common;

use axum::{body::Body, http::{header, Request, StatusCode}};
use common::TestApp;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

/// A request as if sent from `peer` (no socket exists under `oneshot`).
async fn from(app: &TestApp, method: &str, path: &str, body: Value, peer: &str, cookie: Option<&str>) -> (StatusCode, Value, Option<String>) {
    let mut req = Request::builder().method(method).uri(path).header(header::CONTENT_TYPE, "application/json");
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, c);
    }
    let mut req = req.body(Body::from(body.to_string())).unwrap();
    req.extensions_mut().insert(axum::extract::ConnectInfo(std::net::SocketAddr::new(peer.parse().unwrap(), 5000)));
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let cookie = resp.headers().get(header::SET_COOKIE).and_then(|v| v.to_str().ok()).map(|s| s.split(';').next().unwrap().to_string());
    let json = serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap_or(Value::Null);
    (status, json, cookie)
}

fn creds(password: &str) -> Value {
    json!({"email": "tester@example.com", "password": password})
}

#[tokio::test]
async fn a_strangers_failures_lock_only_their_own_address() {
    let app = TestApp::new().await;
    for i in 0..10 {
        let (s, ..) = from(&app, "POST", "/api/auth/login", creds("wrong"), "203.0.113.7", None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    let (s, body, _) = from(&app, "POST", "/api/auth/login", creds("wrong"), "203.0.113.7", None).await;
    assert_eq!((s, body["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate.limited")));
    // the real owner, from another address, still signs in
    let (s, _, cookie) = from(&app, "POST", "/api/auth/login", creds(common::PASSWORD), "203.0.113.8", None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(cookie.is_some());
}

#[tokio::test]
async fn guessing_spread_over_many_addresses_hits_the_account_ceiling() {
    let app = TestApp::new().await;
    for i in 0..100u32 {
        let ip = format!("198.51.{}.{}", i / 200, i % 200 + 1);
        let (s, ..) = from(&app, "POST", "/api/auth/login", creds("wrong"), &ip, None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    let (s, body, _) = from(&app, "POST", "/api/auth/login", creds("wrong"), "198.51.100.250", None).await;
    assert_eq!((s, body["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("rate.limited")));
}

#[tokio::test]
async fn model_listing_uses_the_url_guard_and_returns_no_upstream_text() {
    let app = TestApp::new().await;
    let cookie = app.session_cookie().await;
    // accepted when saved (public address), later resolves to cloud metadata
    eunomia_backend::llm_net::TEST_HOSTS.lock().unwrap().push(("later-bad.test".into(), vec!["93.184.216.34".parse().unwrap()]));
    let (s, ..) = from(&app, "PATCH", "/api/settings", json!({"openai_base_url": "http://later-bad.test/v1"}), "203.0.113.9", Some(&cookie)).await;
    assert_eq!(s, StatusCode::OK);
    eunomia_backend::llm_net::TEST_HOSTS.lock().unwrap().retain(|(h, _)| h != "later-bad.test");
    eunomia_backend::llm_net::TEST_HOSTS.lock().unwrap().push(("later-bad.test".into(), vec!["169.254.169.254".parse().unwrap()]));
    let req = Request::builder().uri("/api/settings/openai-models").header(header::COOKIE, &cookie).body(Body::empty()).unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let body: Value = serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    // refused at connect time, and shown as the same generic code as any other failure (no URL, no upstream text)
    assert_eq!(body["error"], "unreachable", "{body}");
}

/// Does `text` construct a reqwest client? `Client::new` and friends count only as a whole word (so
/// `UpBankClient::new` does not), and `use reqwest::` is fine only for the `header` module.
fn builds_http_client(text: &str) -> bool {
    const CALLS: &[&str] = &["Client::builder", "ClientBuilder", "Client::new", "Client::default", "reqwest::get", "reqwest::post", "reqwest::blocking"];
    let whole_word = |needle: &str| {
        text.match_indices(needle).any(|(i, _)| !text[..i].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_'))
    };
    CALLS.iter().any(|n| whole_word(n)) || text.match_indices("use reqwest::").any(|(i, m)| !text[i + m.len()..].starts_with("header"))
}

/// Every outbound HTTP client is built in a file on this list; everything else must go through `llm_net::client`.
#[test]
fn only_allow_listed_files_build_http_clients() {
    const ALLOWED: &[(&str, &str)] = &[
        ("llm_net.rs", "the guarded client itself"),
        ("oauth/cimd.rs", "fetches a public client metadata URL with its own resolve-then-pin check and no redirects"),
        ("oidc.rs", "calls only the operator-configured OpenID Connect provider and the endpoints its discovery document names: https (loopback http for development), 10 s timeout, no redirects, capped bodies"),
    ];
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    walk(&src, &mut files);
    let mut bad = Vec::new();
    for f in files {
        let rel = f.strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
        let text = std::fs::read_to_string(&f).unwrap();
        let builds = builds_http_client(&text);
        if builds && !ALLOWED.iter().any(|(a, _)| *a == rel) {
            bad.push(rel);
        }
    }
    assert!(bad.is_empty(), "route these through llm_net::client or add them to ALLOWED with a reason: {bad:?}");
}
