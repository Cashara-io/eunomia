//! OAuth 2.1 for MCP clients, end to end against the real router and an
//! in-memory SurrealDB. Clients register dynamically here; CIMD fetching is
//! covered by unit tests (SSRF guard, metadata parsing) and a cache-hit test.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use common::{TestApp, http};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

const REDIRECT: &str = "http://127.0.0.1:7777/callback";
const MCP: &str = "http://localhost:8001/mcp";
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";

fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

struct Resp {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Value,
}

async fn send(app: &TestApp, req: Request<Body>) -> Resp {
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    Resp { status, headers, body: serde_json::from_slice(&bytes).unwrap_or(Value::Null) }
}

async fn form(app: &TestApp, path: &str, fields: &[(&str, &str)]) -> Resp {
    let body = serde_urlencoded::to_string(fields).unwrap();
    let req = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    send(app, req).await
}

async fn get(app: &TestApp, path: &str, bearer: Option<&str>) -> Resp {
    let mut req = Request::builder().uri(path);
    if let Some(b) = bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {b}"));
    }
    send(app, req.body(Body::empty()).unwrap()).await
}

async fn mcp(app: &TestApp, token: &str, body: Value) -> Resp {
    let req = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    send(app, req).await
}

async fn register(app: &TestApp) -> String {
    let (status, body) = http(
        &app.router,
        "POST",
        "/oauth/register",
        Some(json!({"client_name": "Test Agent", "redirect_uris": [REDIRECT], "token_endpoint_auth_method": "none"})),
        None,
        None,
    )
    .await
    .0;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["token_endpoint_auth_method"], "none");
    body["client_id"].as_str().unwrap().to_string()
}

fn authz(client_id: &str, scope: Option<&str>, verifier: &str) -> Value {
    let mut v = json!({
        "response_type": "code", "client_id": client_id, "redirect_uri": REDIRECT,
        "code_challenge": challenge(verifier), "code_challenge_method": "S256",
        "state": "st-1", "resource": MCP,
    });
    if let Some(s) = scope {
        v["scope"] = json!(s);
    }
    v
}

/// Approve on the consent API as the signed-in user; returns the code.
async fn approve(app: &TestApp, params: Value) -> String {
    let mut body = params;
    body["approve"] = json!(true);
    let (status, res) = app.http_session("POST", "/api/oauth/consent", Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let url = reqwest::Url::parse(res["redirect_to"].as_str().unwrap()).unwrap();
    let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(q["state"], "st-1");
    assert_eq!(q["iss"], "http://localhost:8001");
    q["code"].clone()
}

async fn exchange(app: &TestApp, client_id: &str, code: &str, verifier: &str) -> Resp {
    form(app, "/oauth/token", &[
        ("grant_type", "authorization_code"), ("code", code), ("redirect_uri", REDIRECT),
        ("client_id", client_id), ("code_verifier", verifier), ("resource", MCP),
    ])
    .await
}

async fn connect(app: &TestApp, scope: Option<&str>) -> (String, Value) {
    let client_id = register(app).await;
    let code = approve(app, authz(&client_id, scope, VERIFIER)).await;
    let t = exchange(app, &client_id, &code, VERIFIER).await;
    assert_eq!(t.status, StatusCode::OK, "{}", t.body);
    (client_id, t.body)
}

fn rpc(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
}

#[tokio::test]
async fn metadata_documents_have_the_required_shape() {
    let app = TestApp::new().await;
    for path in ["/.well-known/oauth-protected-resource", "/.well-known/oauth-protected-resource/mcp"] {
        let r = get(&app, path, None).await;
        assert_eq!(r.status, StatusCode::OK);
        assert_eq!(r.body["resource"], MCP);
        assert_eq!(r.body["authorization_servers"], json!(["http://localhost:8001"]));
        assert!(r.body["scopes_supported"].as_array().unwrap().contains(&json!("memory:read")));
    }
    let r = get(&app, "/.well-known/oauth-authorization-server", None).await;
    let b = &r.body;
    assert_eq!(b["issuer"], "http://localhost:8001");
    assert_eq!(b["authorization_endpoint"], "http://localhost:8001/oauth/authorize");
    assert_eq!(b["token_endpoint"], "http://localhost:8001/oauth/token");
    assert_eq!(b["registration_endpoint"], "http://localhost:8001/oauth/register");
    assert_eq!(b["revocation_endpoint"], "http://localhost:8001/oauth/revoke");
    assert_eq!(b["code_challenge_methods_supported"], json!(["S256"]));
    assert_eq!(b["client_id_metadata_document_supported"], true);
    assert_eq!(b["authorization_response_iss_parameter_supported"], true);
    assert_eq!(b["token_endpoint_auth_methods_supported"], json!(["none"]));
    assert!(b["grant_types_supported"].as_array().unwrap().contains(&json!("refresh_token")));
}

#[tokio::test]
async fn mcp_401_points_clients_at_the_resource_metadata() {
    let app = TestApp::new().await;
    let anon = send(&app, Request::builder().method("POST").uri("/mcp").body(Body::from("{}")).unwrap()).await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);
    let h = anon.headers[header::WWW_AUTHENTICATE].to_str().unwrap();
    assert!(h.starts_with("Bearer resource_metadata=\"http://localhost:8001/.well-known/oauth-protected-resource/mcp\""), "{h}");
    assert!(h.contains("scope=\"memory:read memory:write\""));
    assert!(!h.contains("invalid_token"));

    let bad = mcp(&app, "eoa_not-a-token", rpc("tools/list", json!({}))).await;
    assert_eq!(bad.status, StatusCode::UNAUTHORIZED);
    assert!(bad.headers[header::WWW_AUTHENTICATE].to_str().unwrap().contains("error=\"invalid_token\""));
}

#[tokio::test]
async fn code_flow_with_pkce_issues_a_working_scoped_token() {
    let app = TestApp::new().await;
    let client_id = register(&app).await;

    // the consent page is told who is asking, from where, and for what
    let q = serde_urlencoded::to_string(
        authz(&client_id, Some("memory:read vaults:admin"), VERIFIER).as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect::<Vec<_>>(),
    )
    .unwrap();
    let (status, info) = app.http_session("GET", &format!("/api/oauth/consent?{q}"), None).await;
    assert_eq!(status, StatusCode::OK, "{info}");
    assert_eq!(info["client"]["name"], "Test Agent");
    assert_eq!(info["redirect_host"], "127.0.0.1:7777");
    assert_eq!(info["loopback"], true);
    assert_eq!(info["scopes"].as_array().unwrap().len(), 2);

    // a wrong verifier fails (and burns the code)
    let code = approve(&app, authz(&client_id, None, VERIFIER)).await;
    let bad = exchange(&app, &client_id, &code, "x".repeat(43).as_str()).await;
    assert_eq!((bad.status, bad.body["error"].as_str()), (StatusCode::BAD_REQUEST, Some("invalid_grant")));
    let again = exchange(&app, &client_id, &code, VERIFIER).await;
    assert_eq!(again.body["error"], "invalid_grant");

    // the right one works, the code is single use, and default scope is read+write
    let code = approve(&app, authz(&client_id, None, VERIFIER)).await;
    let ok = exchange(&app, &client_id, &code, VERIFIER).await;
    assert_eq!(ok.status, StatusCode::OK);
    assert_eq!(ok.body["token_type"], "Bearer");
    assert_eq!(ok.body["expires_in"], 900);
    assert_eq!(ok.body["scope"], "memory:read memory:write");
    assert!(ok.headers[header::CACHE_CONTROL].to_str().unwrap().contains("no-store"));

    let access = ok.body["access_token"].as_str().unwrap();
    let list = mcp(&app, access, rpc("tools/list", json!({}))).await;
    assert_eq!(list.status, StatusCode::OK);
    assert!(list.body["result"]["tools"].as_array().unwrap().len() > 5);
    let call = mcp(&app, access, rpc("tools/call", json!({"name": "vault_list", "arguments": {}}))).await;
    assert_eq!(call.body["result"]["isError"], false);
    assert_eq!(exchange(&app, &client_id, &code, VERIFIER).await.body["error"], "invalid_grant");
}

#[tokio::test]
async fn read_only_token_cannot_write() {
    let app = TestApp::new().await;
    let (_, t) = connect(&app, Some("memory:read")).await;
    let access = t["access_token"].as_str().unwrap();

    // the tool list offers only what the scopes allow
    let list = mcp(&app, access, rpc("tools/list", json!({}))).await;
    let names: Vec<&str> = list.body["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"recall") && !names.contains(&"memory_write") && !names.contains(&"vault_delete"), "{names:?}");

    let read = mcp(&app, access, rpc("tools/call", json!({"name": "vault_list", "arguments": {}}))).await;
    assert_eq!(read.status, StatusCode::OK);

    let write = mcp(&app, access, rpc("tools/call", json!({"name": "memory_write", "arguments": {"subject": "x", "text": "y"}}))).await;
    assert_eq!(write.status, StatusCode::FORBIDDEN);
    let h = write.headers[header::WWW_AUTHENTICATE].to_str().unwrap();
    assert!(h.contains("error=\"insufficient_scope\"") && h.contains("scope=\"memory:write\"") && h.contains("resource_metadata="), "{h}");

    // a batch asking for two missing scopes gets them in one challenge
    let batch = json!([
        rpc("tools/call", json!({"name": "memory_write", "arguments": {}})),
        rpc("tools/call", json!({"name": "vault_delete", "arguments": {}})),
    ]);
    let both = mcp(&app, access, batch).await;
    assert!(both.headers[header::WWW_AUTHENTICATE].to_str().unwrap().contains("scope=\"memory:write vaults:admin\""));

    // nothing was written
    let (_, mems) = app.http("POST", "/api/tools/recall", Some(json!({"query": "y"})), true).await;
    assert!(!mems.to_string().contains("\"y\""), "{mems}");
}

#[tokio::test]
async fn tokens_are_audience_bound() {
    let app = TestApp::new().await;
    let client_id = register(&app).await;

    // asking for another resource is refused at authorize and at token time
    let mut p = authz(&client_id, None, VERIFIER);
    p["resource"] = json!("https://other.example/mcp");
    let q = serde_urlencoded::to_string(p.as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect::<Vec<_>>()).unwrap();
    let r = get(&app, &format!("/oauth/authorize?{q}"), None).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let loc = r.headers[header::LOCATION].to_str().unwrap();
    assert!(loc.starts_with(REDIRECT) && loc.contains("error=invalid_target") && loc.contains("state=st-1"), "{loc}");

    let code = approve(&app, authz(&client_id, None, VERIFIER)).await;
    let wrong = form(&app, "/oauth/token", &[
        ("grant_type", "authorization_code"), ("code", &code), ("redirect_uri", REDIRECT), ("client_id", &client_id),
        ("code_verifier", VERIFIER), ("resource", "https://other.example/mcp"),
    ])
    .await;
    assert_eq!(wrong.body["error"], "invalid_target");

    // a token whose recorded audience is not this server's MCP URL is rejected on /mcp
    let (_, t) = connect(&app, None).await;
    let access = t["access_token"].as_str().unwrap();
    assert_eq!(mcp(&app, access, rpc("ping", json!({}))).await.status, StatusCode::OK);
    app.state.control.test_raw().query("UPDATE oauth_grant SET resource = 'https://other.example/mcp'").await.unwrap();
    app.state.credentials.clear(); // a hand edit reaches a cached token only once its entry ages out
    assert_eq!(mcp(&app, access, rpc("ping", json!({}))).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn expired_access_tokens_and_codes_are_rejected() {
    let app = TestApp::new().await;
    let (_, t) = connect(&app, None).await;
    let access = t["access_token"].as_str().unwrap();
    app.state.control.test_raw().query("UPDATE oauth_token SET expires_at = time::now() - 1m WHERE kind = 'access'").await.unwrap();
    app.state.credentials.clear(); // a hand edit reaches a cached token only once its entry ages out
    assert_eq!(mcp(&app, access, rpc("ping", json!({}))).await.status, StatusCode::UNAUTHORIZED);

    let client_id = register(&app).await;
    let code = approve(&app, authz(&client_id, None, VERIFIER)).await;
    app.state.control.test_raw().query("UPDATE oauth_code SET expires_at = time::now() - 1s").await.unwrap();
    assert_eq!(exchange(&app, &client_id, &code, VERIFIER).await.body["error"], "invalid_grant");
}

#[tokio::test]
async fn refresh_rotates_and_reuse_revokes_the_family() {
    let app = TestApp::new().await;
    let (client_id, t) = connect(&app, None).await;
    let refresh1 = t["refresh_token"].as_str().unwrap().to_string();

    let r = form(&app, "/oauth/token", &[("grant_type", "refresh_token"), ("refresh_token", &refresh1), ("client_id", &client_id)]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let (access2, refresh2) = (r.body["access_token"].as_str().unwrap().to_string(), r.body["refresh_token"].as_str().unwrap().to_string());
    assert_ne!(refresh1, refresh2);
    assert_eq!(mcp(&app, &access2, rpc("ping", json!({}))).await.status, StatusCode::OK);

    // another client cannot use it
    let other = register(&app).await;
    let stolen = form(&app, "/oauth/token", &[("grant_type", "refresh_token"), ("refresh_token", &refresh2), ("client_id", &other)]).await;
    assert_eq!(stolen.body["error"], "invalid_grant");

    // replaying the spent token is detected and kills the new tokens too
    let replay = form(&app, "/oauth/token", &[("grant_type", "refresh_token"), ("refresh_token", &refresh1), ("client_id", &client_id)]).await;
    assert_eq!(replay.body["error"], "invalid_grant");
    assert!(replay.body["error_description"].as_str().unwrap().contains("reuse"));
    assert_eq!(mcp(&app, &access2, rpc("ping", json!({}))).await.status, StatusCode::UNAUTHORIZED);
    let dead = form(&app, "/oauth/token", &[("grant_type", "refresh_token"), ("refresh_token", &refresh2), ("client_id", &client_id)]).await;
    assert_eq!(dead.body["error"], "invalid_grant");
    let (_, apps) = app.http_session("GET", "/api/oauth/grants", None).await;
    assert_eq!(apps.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn refresh_with_a_narrower_scope_issues_only_that_scope() {
    let app = TestApp::new().await;
    let (client_id, t) = connect(&app, Some("memory:read memory:write")).await;
    let refresh = t["refresh_token"].as_str().unwrap().to_string();

    let r = form(&app, "/oauth/token", &[("grant_type", "refresh_token"), ("refresh_token", &refresh), ("client_id", &client_id), ("scope", "memory:read")]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["scope"], "memory:read");
    let narrow = r.body["access_token"].as_str().unwrap().to_string();
    let write = mcp(&app, &narrow, rpc("tools/call", json!({"name": "memory_write", "arguments": {"subject": "x", "text": "y"}}))).await;
    assert_eq!(write.status, StatusCode::FORBIDDEN);
    assert_eq!(mcp(&app, &narrow, rpc("tools/call", json!({"name": "vault_list", "arguments": {}}))).await.status, StatusCode::OK);

    // the next refresh keeps the narrowed scope unless asked for less
    let again = form(&app, "/oauth/token", &[("grant_type", "refresh_token"), ("refresh_token", r.body["refresh_token"].as_str().unwrap()), ("client_id", &client_id)]).await;
    assert_eq!(again.body["scope"], "memory:read");
    // and can never widen back past what it carries
    let wider = form(&app, "/oauth/token", &[("grant_type", "refresh_token"), ("refresh_token", again.body["refresh_token"].as_str().unwrap()), ("client_id", &client_id), ("scope", "memory:read memory:write")]).await;
    assert_eq!(wider.body["error"], "invalid_scope");
}

#[tokio::test]
async fn authorize_is_rate_limited_per_address_and_dcr_clients_are_capped_and_expire() {
    let app = TestApp::new().await;
    let router = eunomia_backend::app_with(app.state.clone(), eunomia_backend::ratelimit::RateConfig { auth_per_min: 2, ..Default::default() });
    let mut codes = Vec::new();
    for _ in 0..3 {
        let req = Request::builder().uri("/oauth/authorize?client_id=nope").body(Body::empty()).unwrap();
        codes.push(router.clone().oneshot(req).await.unwrap().status());
    }
    assert_eq!(codes[2], StatusCode::TOO_MANY_REQUESTS, "{codes:?}");
    assert_ne!(codes[0], StatusCode::TOO_MANY_REQUESTS);

    // a client unused for 30 days is pruned at the next registration, an active one is not
    let old = register(&app).await;
    let fresh = register(&app).await;
    app.state.control.test_raw().query("UPDATE oauth_client SET fetched_at = time::now() - 31d WHERE client_id = $c").bind(("c", old.clone())).await.unwrap();
    let _ = register(&app).await;
    let mut res = app.state.control.test_raw().query("SELECT VALUE client_id FROM oauth_client WHERE kind = 'dcr'").await.unwrap();
    let left: Vec<String> = res.take(0).unwrap();
    assert!(!left.contains(&old) && left.contains(&fresh), "{left:?}");
}

#[tokio::test]
async fn revoke_endpoint_and_connected_apps() {
    let app = TestApp::new().await;
    let (client_id, t) = connect(&app, None).await;
    let (access, refresh) = (t["access_token"].as_str().unwrap(), t["refresh_token"].as_str().unwrap());

    // revoking an access token kills only that token
    assert_eq!(form(&app, "/oauth/revoke", &[("token", access), ("client_id", &client_id)]).await.status, StatusCode::OK);
    assert_eq!(mcp(&app, access, rpc("ping", json!({}))).await.status, StatusCode::UNAUTHORIZED);
    let r = form(&app, "/oauth/token", &[("grant_type", "refresh_token"), ("refresh_token", refresh), ("client_id", &client_id)]).await;
    assert_eq!(r.status, StatusCode::OK);
    let (access2, refresh2) = (r.body["access_token"].as_str().unwrap(), r.body["refresh_token"].as_str().unwrap());

    // unknown tokens are not an error
    assert_eq!(form(&app, "/oauth/revoke", &[("token", "nope")]).await.status, StatusCode::OK);
    // another client's revoke is a no-op
    form(&app, "/oauth/revoke", &[("token", access2), ("client_id", "someone-else")]).await;
    assert_eq!(mcp(&app, access2, rpc("ping", json!({}))).await.status, StatusCode::OK);

    // the user disconnects the app from Settings: everything dies
    let (_, apps) = app.http_session("GET", "/api/oauth/grants", None).await;
    assert_eq!(apps[0]["client_name"], "Test Agent");
    assert!(apps[0]["last_used_at"].is_string());
    let id = apps[0]["id"].as_str().unwrap();
    let (status, _) = app.http_session("DELETE", &format!("/api/oauth/grants/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(mcp(&app, access2, rpc("ping", json!({}))).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(form(&app, "/oauth/token", &[("grant_type", "refresh_token"), ("refresh_token", refresh2), ("client_id", &client_id)]).await.body["error"], "invalid_grant");
    let (status, _) = app.http_session("DELETE", &format!("/api/oauth/grants/{id}"), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // refresh-token revocation takes the whole grant
    let (client_id, t) = connect(&app, None).await;
    form(&app, "/oauth/revoke", &[("token", t["refresh_token"].as_str().unwrap()), ("client_id", &client_id)]).await;
    assert_eq!(mcp(&app, t["access_token"].as_str().unwrap(), rpc("ping", json!({}))).await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn redirects_pkce_and_scopes_are_validated() {
    let app = TestApp::new().await;
    let client_id = register(&app).await;
    let url = |p: &Value| {
        let pairs: Vec<(String, String)> = p.as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect();
        format!("/oauth/authorize?{}", serde_urlencoded::to_string(pairs).unwrap())
    };

    // an unregistered redirect URI is never redirected to
    let mut p = authz(&client_id, None, VERIFIER);
    p["redirect_uri"] = json!("https://evil.example/cb");
    let r = get(&app, &url(&p), None).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.headers.get(header::LOCATION).is_none());
    // unknown client likewise
    let mut p = authz("nobody", None, VERIFIER);
    assert_eq!(get(&app, &url(&p), None).await.status, StatusCode::BAD_REQUEST);

    // a loopback redirect may use any port
    p = authz(&client_id, None, VERIFIER);
    p["redirect_uri"] = json!("http://127.0.0.1:60001/callback");
    let r = get(&app, &url(&p), None).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert!(r.headers[header::LOCATION].to_str().unwrap().starts_with("http://localhost:3000/login?next=%2Fconsent"), "signed out goes to login first");
    let r = get(&app, &url(&p), Some(&app.token)).await;
    assert!(r.headers[header::LOCATION].to_str().unwrap().starts_with("http://localhost:3000/consent?"), "signed in goes to consent");

    // PKCE is mandatory and S256 only; scopes must be known; only code flow
    for (field, value, error) in [
        ("code_challenge_method", "plain", "invalid_request"),
        ("code_challenge", "", "invalid_request"),
        ("scope", "memory:read root", "invalid_scope"),
        ("response_type", "token", "unsupported_response_type"),
    ] {
        let mut p = authz(&client_id, None, VERIFIER);
        p[field] = json!(value);
        let r = get(&app, &url(&p), None).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{field}");
        let loc = r.headers[header::LOCATION].to_str().unwrap();
        assert!(loc.starts_with(REDIRECT) && loc.contains(&format!("error={error}")) && loc.contains("iss="), "{field}: {loc}");
    }

    // redirect_uri at the token endpoint must repeat the authorization request's
    let code = approve(&app, authz(&client_id, None, VERIFIER)).await;
    let r = form(&app, "/oauth/token", &[("grant_type", "authorization_code"), ("code", &code), ("redirect_uri", "http://127.0.0.1:1/callback"), ("client_id", &client_id), ("code_verifier", VERIFIER)]).await;
    assert_eq!(r.body["error"], "invalid_grant");

    // deny comes back as access_denied
    let mut body = authz(&client_id, None, VERIFIER);
    body["approve"] = json!(false);
    let (_, res) = app.http_session("POST", "/api/oauth/consent", Some(body)).await;
    assert!(res["redirect_to"].as_str().unwrap().contains("error=access_denied"));
}

#[tokio::test]
async fn registration_rejects_bad_redirects_and_cimd_cache_is_used() {
    let app = TestApp::new().await;
    for uris in [json!(["http://example.com/cb"]), json!(["javascript:alert(1)"]), json!([]), json!("x")] {
        let (status, body) = http(&app.router, "POST", "/oauth/register", Some(json!({"redirect_uris": uris})), None, None).await.0;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_client_metadata");
    }

    // a fresh cached CIMD client resolves with no network; an expired one would be refetched
    let id = "https://app.example.com/oauth/client.json";
    app.state.control.test_raw().query(format!(
        "CREATE oauth_client SET client_id = '{id}', name = 'Cached App', redirect_uris = ['{REDIRECT}'], kind = 'cimd', expires_at = time::now() + 1h"
    )).await.unwrap().check().unwrap();
    let p = authz(id, None, VERIFIER);
    let (status, info) = app.http_session("POST", "/api/oauth/consent", Some({ let mut b = p; b["approve"] = json!(true); b })).await;
    assert_eq!(status, StatusCode::OK, "{info}");
    let (status, info) = {
        let q = serde_urlencoded::to_string([("response_type", "code"), ("client_id", id), ("redirect_uri", REDIRECT), ("code_challenge", &challenge(VERIFIER)), ("code_challenge_method", "S256")]).unwrap();
        app.http_session("GET", &format!("/api/oauth/consent?{q}"), None).await
    };
    assert_eq!((status, info["client"]["name"].as_str()), (StatusCode::OK, Some("Cached App")));
}

#[tokio::test]
async fn personal_api_tokens_still_work_on_mcp_and_oauth_tokens_not_on_rest() {
    let app = TestApp::new().await;
    assert_eq!(mcp(&app, &app.token, rpc("tools/call", json!({"name": "vault_list", "arguments": {}}))).await.status, StatusCode::OK);
    let (_, t) = connect(&app, None).await;
    let r = get(&app, "/api/vaults", Some(t["access_token"].as_str().unwrap())).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn replaying_a_redeemed_code_revokes_the_tokens_issued_from_it() {
    let app = TestApp::new().await;
    let client_id = register(&app).await;
    let code = approve(&app, authz(&client_id, None, VERIFIER)).await;
    let ok = exchange(&app, &client_id, &code, VERIFIER).await;
    let (access, refresh) = (ok.body["access_token"].as_str().unwrap(), ok.body["refresh_token"].as_str().unwrap());
    assert_eq!(mcp(&app, access, rpc("ping", json!({}))).await.status, StatusCode::OK);

    assert_eq!(exchange(&app, &client_id, &code, VERIFIER).await.body["error"], "invalid_grant");

    assert_eq!(mcp(&app, access, rpc("ping", json!({}))).await.status, StatusCode::UNAUTHORIZED);
    let r = form(&app, "/oauth/token", &[("grant_type", "refresh_token"), ("refresh_token", refresh), ("client_id", &client_id)]).await;
    assert_eq!(r.body["error"], "invalid_grant");
    let (_, apps) = app.http_session("GET", "/api/oauth/grants", None).await;
    assert_eq!(apps.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn browser_mcp_clients_get_credential_free_cors_and_api_stays_strict() {
    let app = TestApp::new().await;
    let preflight = |path: &'static str, origin: &'static str, method: &'static str| {
        Request::builder()
            .method("OPTIONS")
            .uri(path)
            .header(header::ORIGIN, origin)
            .header("access-control-request-method", method)
            .header("access-control-request-headers", "authorization,content-type,mcp-protocol-version")
            .body(Body::empty())
            .unwrap()
    };
    for (path, method) in [
        ("/mcp", "POST"),
        ("/oauth/token", "POST"),
        ("/oauth/register", "POST"),
        ("/oauth/revoke", "POST"),
        ("/.well-known/oauth-authorization-server", "GET"),
        ("/.well-known/oauth-protected-resource", "GET"),
        ("/.well-known/oauth-protected-resource/mcp", "GET"),
    ] {
        let resp = app.router.clone().oneshot(preflight(path, "https://inspector.example", method)).await.unwrap();
        assert!(resp.status().is_success(), "{path} {}", resp.status());
        let h = resp.headers();
        assert_eq!(h[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*", "{path}");
        assert!(!h.contains_key(header::ACCESS_CONTROL_ALLOW_CREDENTIALS), "{path}");
    }
    // a simple cross-origin GET of metadata also carries the header
    let req = Request::builder().uri("/.well-known/oauth-authorization-server").header(header::ORIGIN, "https://inspector.example").body(Body::empty()).unwrap();
    assert_eq!(app.router.clone().oneshot(req).await.unwrap().headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");

    // /api keeps one credentialed origin: a stranger gets no CORS grant, the app origin does
    let resp = app.router.clone().oneshot(preflight("/api/auth/me", "https://inspector.example", "GET")).await.unwrap();
    assert!(!resp.headers().contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
    let resp = app.router.clone().oneshot(preflight("/api/auth/me", "http://localhost:3000", "GET")).await.unwrap();
    assert_eq!(resp.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "http://localhost:3000");
    assert_eq!(resp.headers()[header::ACCESS_CONTROL_ALLOW_CREDENTIALS], "true");

    // a bearer-only MCP call from a foreign browser origin works
    let req = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header(header::ORIGIN, "https://inspector.example")
        .header(header::AUTHORIZATION, format!("Bearer {}", app.token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(rpc("ping", json!({})).to_string()))
        .unwrap();
    assert_eq!(app.router.clone().oneshot(req).await.unwrap().status(), StatusCode::OK);
}

#[tokio::test]
async fn refresh_grants_have_their_own_bucket_and_code_exchange_stays_strict() {
    let app = TestApp::new().await;
    let limits = eunomia_backend::ratelimit::RateConfig { auth_per_min: 2, refresh_per_min: 5, ..Default::default() };
    let router = eunomia_backend::app_with(app.state.clone(), limits);
    let post = |fields: Vec<(&'static str, &'static str)>| {
        let router = router.clone();
        async move {
            let req = Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(serde_urlencoded::to_string(&fields).unwrap()))
                .unwrap();
            router.oneshot(req).await.unwrap().status()
        }
    };
    // many refreshes through one address and client: well past the strict 2 per minute
    for i in 0..5 {
        let s = post(vec![("grant_type", "refresh_token"), ("client_id", "c1"), ("refresh_token", "bad")]).await;
        assert_ne!(s, StatusCode::TOO_MANY_REQUESTS, "refresh {i}");
    }
    assert_eq!(post(vec![("grant_type", "refresh_token"), ("client_id", "c1"), ("refresh_token", "bad")]).await, StatusCode::TOO_MANY_REQUESTS);
    // another client behind the same address has its own bucket
    assert_ne!(post(vec![("grant_type", "refresh_token"), ("client_id", "c2"), ("refresh_token", "bad")]).await, StatusCode::TOO_MANY_REQUESTS);
    // code exchange is still on the strict per-address bucket
    for _ in 0..2 {
        assert_ne!(post(vec![("grant_type", "authorization_code"), ("code", "x")]).await, StatusCode::TOO_MANY_REQUESTS);
    }
    assert_eq!(post(vec![("grant_type", "authorization_code"), ("code", "x")]).await, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn never_authorized_dcr_clients_are_pruned_after_24_hours() {
    let app = TestApp::new().await;
    let (stale, fresh) = (register(&app).await, register(&app).await);
    let (granted_id, _) = connect(&app, None).await; // authorized: kept
    let raw = app.state.control.test_raw();
    raw.query("UPDATE oauth_client SET fetched_at = time::now() - 25h WHERE client_id IN [$a, $g]").bind(("a", stale.clone())).bind(("g", granted_id.clone())).await.unwrap();
    raw.query("UPDATE oauth_client SET fetched_at = time::now() - 23h WHERE client_id = $f").bind(("f", fresh.clone())).await.unwrap();
    let _ = register(&app).await; // registration prunes
    let mut res = raw.query("SELECT VALUE client_id FROM oauth_client WHERE kind = 'dcr'").await.unwrap();
    let left: Vec<String> = res.take(0).unwrap();
    assert!(!left.contains(&stale), "unused for 25h and never authorized: {left:?}");
    assert!(left.contains(&fresh) && left.contains(&granted_id), "{left:?}");
}
