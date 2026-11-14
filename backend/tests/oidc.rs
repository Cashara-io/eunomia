//! Sign-in through an OpenID Connect provider, end to end: the real router and an in-memory SurrealDB against
//! an in-test mock provider (axum on 127.0.0.1:0) that serves discovery, a JWKS, `/authorize` and `/token`
//! and signs ID tokens with an ES256 test key. Nothing leaves the machine.

mod common;
#[path = "common/oidc_keys.rs"]
mod keys;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::response::{IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use common::{PASSWORD, engine_config, http, test_settings};
use eunomia_backend::config::Settings;
use eunomia_backend::models_user;
use eunomia_backend::oidc::{AuthConfig, AuthMode, OidcConfig};
use eunomia_backend::rid::RecordIdExt;
use eunomia_backend::state::AppState;
use http_body_util::BodyExt;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

const CLIENT_ID: &str = "eunomia-test-client";
const CLIENT_SECRET: &str = "s3cret-for-tests";

// -- the mock provider -------------------------------------------------------------------------

/// What the next ID token looks like. Defaults to a valid token for alice.
#[derive(Clone)]
struct Behavior {
    sub: String,
    email: Option<String>,
    email_verified: Value,
    iss: Option<String>,
    aud: Value,
    azp: Option<String>,
    /// seconds from now
    exp_in: i64,
    nonce: Option<String>,
    /// which key signs: "a" (served), "b" (not served: a forgery) or "c" (served after a rotation)
    key: &'static str,
    kid: Option<String>,
    /// "ES256", or "HS256" / "none" for the algorithm-confusion attempts
    alg: &'static str,
}

impl Default for Behavior {
    fn default() -> Self {
        Behavior {
            sub: "alice-sub".into(),
            email: Some("alice@example.test".into()),
            email_verified: json!(true),
            iss: None,
            aud: json!(CLIENT_ID),
            azp: None,
            exp_in: 300,
            nonce: None,
            key: "a",
            kid: Some("key-a".into()),
            alg: "ES256",
        }
    }
}

struct Pending {
    nonce: String,
    challenge: String,
    redirect_uri: String,
}

#[derive(Default)]
struct Inner {
    codes: HashMap<String, Pending>,
    behavior: Option<Behavior>,
    /// the JWKS served: (kid, key letter)
    served: Vec<(String, &'static str)>,
    jwks_hits: usize,
    token_hits: usize,
    /// the userinfo answer, when the provider keeps the email out of the ID token
    userinfo: Option<Value>,
    issuer: String,
}

#[derive(Clone)]
struct Idp {
    inner: Arc<Mutex<Inner>>,
}

fn jwk(kid: &str, key: &str) -> Value {
    let (x, y) = match key {
        "a" => (keys::KEY_A_X, keys::KEY_A_Y),
        "c" => (keys::KEY_C_X, keys::KEY_C_Y),
        _ => (keys::KEY_B_X, keys::KEY_B_Y),
    };
    json!({"kty": "EC", "crv": "P-256", "alg": "ES256", "use": "sig", "kid": kid, "x": x, "y": y})
}

impl Idp {
    async fn start() -> (Idp, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let idp = Idp { inner: Arc::new(Mutex::new(Inner { issuer: issuer.clone(), served: vec![("key-a".into(), "a")], ..Default::default() })) };
        let app = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/jwks", get(jwks))
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .route("/userinfo", get(userinfo))
            .with_state(idp.clone());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (idp, issuer)
    }

    fn behave(&self, b: Behavior) {
        self.inner.lock().unwrap().behavior = Some(b);
    }

    fn serve_keys(&self, served: Vec<(&str, &'static str)>) {
        self.inner.lock().unwrap().served = served.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    }

    fn jwks_hits(&self) -> usize {
        self.inner.lock().unwrap().jwks_hits
    }

    fn token_hits(&self) -> usize {
        self.inner.lock().unwrap().token_hits
    }
}

async fn discovery(State(idp): State<Idp>) -> Json<Value> {
    let i = idp.inner.lock().unwrap().issuer.clone();
    Json(json!({
        "issuer": i, "authorization_endpoint": format!("{i}/authorize"), "token_endpoint": format!("{i}/token"),
        "jwks_uri": format!("{i}/jwks"), "userinfo_endpoint": format!("{i}/userinfo"),
        "token_endpoint_auth_methods_supported": ["client_secret_basic"], "response_types_supported": ["code"],
    }))
}

async fn jwks(State(idp): State<Idp>) -> Json<Value> {
    let mut g = idp.inner.lock().unwrap();
    g.jwks_hits += 1;
    Json(json!({"keys": g.served.iter().map(|(k, v)| jwk(k, v)).collect::<Vec<_>>()}))
}

async fn authorize(State(idp): State<Idp>, Query(q): Query<HashMap<String, String>>) -> impl IntoResponse {
    assert_eq!(q["client_id"], CLIENT_ID);
    assert_eq!(q["response_type"], "code");
    assert_eq!(q["code_challenge_method"], "S256");
    assert!(q["scope"].split(' ').any(|s| s == "openid"));
    let code = format!("code-{}", q["state"].chars().take(8).collect::<String>());
    idp.inner.lock().unwrap().codes.insert(
        code.clone(),
        Pending { nonce: q["nonce"].clone(), challenge: q["code_challenge"].clone(), redirect_uri: q["redirect_uri"].clone() },
    );
    let back = format!("{}?{}", q["redirect_uri"], serde_urlencoded::to_string([("code", code.as_str()), ("state", q["state"].as_str())]).unwrap());
    Redirect::to(&back)
}

fn bad_request(error: &str) -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({"error": error})))
}

async fn token(State(idp): State<Idp>, headers: HeaderMap, Form(f): Form<HashMap<String, String>>) -> (StatusCode, Json<Value>) {
    // the client authenticates with HTTP Basic, as the discovery document says
    let want = format!("Basic {}", STANDARD.encode(format!("{CLIENT_ID}:{CLIENT_SECRET}")));
    if headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) != Some(want.as_str()) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "invalid_client"})));
    }
    let mut g = idp.inner.lock().unwrap();
    g.token_hits += 1;
    // codes are single use
    let Some(p) = f.get("code").and_then(|c| g.codes.remove(c)) else { return bad_request("invalid_grant") };
    if f.get("redirect_uri") != Some(&p.redirect_uri) || f.get("grant_type").map(String::as_str) != Some("authorization_code") {
        return bad_request("invalid_grant");
    }
    let verifier = f.get("code_verifier").cloned().unwrap_or_default();
    if URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) != p.challenge {
        return bad_request("invalid_grant");
    }
    let b = g.behavior.clone().unwrap_or_default();
    let now = chrono::Utc::now().timestamp();
    let mut claims = json!({
        "iss": b.iss.clone().unwrap_or_else(|| g.issuer.clone()), "sub": b.sub, "aud": b.aud, "exp": now + b.exp_in, "iat": now,
        "nonce": b.nonce.clone().unwrap_or(p.nonce),
    });
    if let Some(e) = &b.email {
        claims["email"] = json!(e);
        claims["email_verified"] = b.email_verified.clone();
    }
    if let Some(a) = &b.azp {
        claims["azp"] = json!(a);
    }
    let id_token = sign(&b, &claims);
    (StatusCode::OK, Json(json!({"access_token": "at-1", "token_type": "Bearer", "id_token": id_token})))
}

async fn userinfo(State(idp): State<Idp>, headers: HeaderMap) -> (StatusCode, Json<Value>) {
    if headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) != Some("Bearer at-1") {
        return (StatusCode::UNAUTHORIZED, Json(json!({})));
    }
    (StatusCode::OK, Json(idp.inner.lock().unwrap().userinfo.clone().unwrap_or(json!({}))))
}

fn sign(b: &Behavior, claims: &Value) -> String {
    let pem = match b.key {
        "a" => keys::KEY_A_PEM,
        "c" => keys::KEY_C_PEM,
        _ => keys::KEY_B_PEM,
    };
    match b.alg {
        "none" => {
            let h = URL_SAFE_NO_PAD.encode(json!({"alg": "none", "typ": "JWT"}).to_string());
            format!("{h}.{}.", URL_SAFE_NO_PAD.encode(claims.to_string()))
        }
        "HS256" => {
            let mut h = Header::new(Algorithm::HS256);
            h.kid = b.kid.clone();
            // an attacker's HMAC "key" built from public material
            jsonwebtoken::encode(&h, claims, &EncodingKey::from_secret(keys::KEY_A_X.as_bytes())).unwrap()
        }
        _ => {
            let mut h = Header::new(Algorithm::ES256);
            h.kid = b.kid.clone();
            jsonwebtoken::encode(&h, claims, &EncodingKey::from_ec_pem(pem.as_bytes()).unwrap()).unwrap()
        }
    }
}

// -- the app under test ------------------------------------------------------------------------

struct App {
    state: AppState,
    router: Router,
    idp: Idp,
}

fn settings(mode: AuthMode, issuer: &str) -> Settings {
    Settings {
        auth: AuthConfig {
            mode,
            oidc: Some(OidcConfig {
                issuer: issuer.to_string(),
                client_id: CLIENT_ID.into(),
                client_secret: Some(CLIENT_SECRET.into()),
                scopes: "openid email profile".into(),
                provider_name: "Mock IdP".into(),
            }),
        },
        ..test_settings()
    }
}

/// An empty install (no users) whose sign-in goes through the mock provider.
async fn app(mode: AuthMode) -> App {
    let (idp, issuer) = Idp::start().await;
    let state = AppState::build(&settings(mode, &issuer), engine_config()).await.expect("state");
    let router = eunomia_backend::app(state.clone());
    App { state, router, idp }
}

struct Landed {
    status: StatusCode,
    location: String,
    /// `name=value` of the session cookie when one was set
    session: Option<String>,
    /// `name=value` of the cookie a sign-in leaves when it must ask before linking to a password account
    link: Option<String>,
}

impl Landed {
    fn error(&self) -> Option<String> {
        let q = self.location.split_once('?')?.1;
        serde_urlencoded::from_str::<HashMap<String, String>>(q).ok()?.remove("oidc_error")
    }

    fn path(&self) -> &str {
        &self.location
    }
}

async fn raw(router: &Router, req: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let resp = router.clone().oneshot(req).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    (status, headers, resp.into_body().collect().await.unwrap().to_bytes().to_vec())
}

fn cookie_pairs(headers: &HeaderMap) -> Vec<String> {
    headers.get_all(header::SET_COOKIE).iter().filter_map(|v| v.to_str().ok()).map(|v| v.split(';').next().unwrap().to_string()).collect()
}

/// Follows the provider's redirect without a redirect policy (the mock is a real HTTP server).
async fn at_provider(authorize_url: &str) -> String {
    let c = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let r = c.get(authorize_url).send().await.unwrap();
    assert!(r.status().is_redirection(), "{}", r.status());
    r.headers()[header::LOCATION].to_str().unwrap().to_string()
}

#[derive(Default)]
struct Tamper {
    state: Option<&'static str>,
    drop_cookie: bool,
    error: Option<&'static str>,
}

impl App {
    /// The whole browser round trip: start, the provider, the callback.
    async fn sign_in_with(&self, next: &str, tamper: Tamper) -> Landed {
        let uri = format!("/api/auth/oidc/start?{}", serde_urlencoded::to_string([("next", next)]).unwrap());
        let (status, headers, _) = raw(&self.router, Request::get(uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        let flow = cookie_pairs(&headers).into_iter().find(|c| c.starts_with("eunomia_oidc=")).expect("flow cookie");
        let set = headers.get_all(header::SET_COOKIE).iter().next().unwrap().to_str().unwrap().to_string();
        assert!(set.contains("HttpOnly") && set.contains("SameSite=Lax") && set.contains("Path=/api/auth/oidc"), "{set}");
        let callback = at_provider(headers[header::LOCATION].to_str().unwrap()).await;
        let mut path = callback.strip_prefix("http://localhost:8001").expect("redirect_uri is PUBLIC_URL based").to_string();
        if let Some(s) = tamper.state {
            let (p, q) = path.split_once('?').unwrap();
            let mut q: HashMap<String, String> = serde_urlencoded::from_str(q).unwrap();
            q.insert("state".into(), s.into());
            path = format!("{p}?{}", serde_urlencoded::to_string(&q).unwrap());
        }
        if let Some(e) = tamper.error {
            path = format!("/api/auth/oidc/callback?{}", serde_urlencoded::to_string([("error", e)]).unwrap());
        }
        let mut req = Request::get(path);
        if !tamper.drop_cookie {
            req = req.header(header::COOKIE, flow);
        }
        let (status, headers, _) = raw(&self.router, req.body(Body::empty()).unwrap()).await;
        let location = headers.get(header::LOCATION).map(|v| v.to_str().unwrap().to_string()).unwrap_or_default();
        let session = cookie_pairs(&headers).into_iter().find(|c| c.starts_with("eunomia_session=") && c.len() > "eunomia_session=".len());
        let link = cookie_pairs(&headers).into_iter().find(|c| c.starts_with("eunomia_oidc_link=") && c.len() > "eunomia_oidc_link=".len());
        Landed { status, location, session, link }
    }

    async fn sign_in(&self) -> Landed {
        self.sign_in_with("/", Tamper::default()).await
    }

    async fn get_as(&self, cookie: &str, path: &str) -> (StatusCode, Value) {
        http(&self.router, "GET", path, None, None, Some(cookie)).await.0
    }

    async fn user_count(&self) -> usize {
        let mut res = self.state.control.test_raw().query("SELECT count() FROM user GROUP ALL").await.unwrap();
        let rows: Vec<Value> = res.take(0).unwrap();
        rows.first().map_or(0, |r| r["count"].as_u64().unwrap_or(0) as usize)
    }
}

fn expect_signed_in(l: &Landed) -> String {
    assert_eq!(l.status, StatusCode::SEE_OTHER, "{}", l.location);
    assert_eq!(l.error(), None, "{}", l.location);
    l.session.clone().expect("a session cookie")
}

fn expect_refused(l: &Landed, code: &str) {
    assert_eq!(l.status, StatusCode::SEE_OTHER);
    assert_eq!(l.error().as_deref(), Some(code), "{}", l.location);
    assert!(l.location.starts_with("/login?"), "{}", l.location);
    assert!(l.session.is_none(), "no session on a refused sign-in");
}

// -- tests -------------------------------------------------------------------------------------

#[tokio::test]
async fn first_sign_in_creates_the_admin_with_a_personal_vault_and_a_session() {
    let app = app(AuthMode::Oidc).await;
    let landed = app.sign_in_with("/consent?client_id=x", Tamper::default()).await;
    let cookie = expect_signed_in(&landed);
    // a new account goes through onboarding, then on to where it was headed
    assert_eq!(landed.path(), "/onboarding?next=%2Fconsent%3Fclient_id%3Dx");
    assert_eq!(app.user_count().await, 1);

    let (status, me) = app.get_as(&cookie, "/api/auth/me").await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["email"], "alice@example.test");
    assert_eq!(me["has_password"], false);
    assert_eq!(me["oidc_provider"], "Mock IdP");

    let (_, vaults) = app.get_as(&cookie, "/api/vaults").await;
    let list = vaults.as_array().or(vaults["results"].as_array()).expect("vault list");
    assert_eq!(list.len(), 1, "{vaults}");
    assert_eq!(list[0]["kind"], "personal", "{vaults}");

    // the first user of an install is its admin
    let user = models_user::user_by_email(&app.state.control, "alice@example.test").await.unwrap().unwrap();
    assert!(eunomia_backend::authz::is_admin(&app.state.control, &user).await.unwrap());

    // the ledger has the signup and the login
    let mut res = app.state.control.test_raw().query("SELECT action, outcome FROM audit_event WHERE action IN ['auth.register', 'auth.oidc_login'] ORDER BY created_at").await.unwrap();
    let rows: Vec<Value> = res.take(0).unwrap();
    assert_eq!(rows.iter().map(|r| r["action"].as_str().unwrap()).collect::<Vec<_>>(), ["auth.register", "auth.oidc_login"]);
}

#[tokio::test]
async fn the_second_sign_in_maps_to_the_same_account() {
    let app = app(AuthMode::Both).await;
    let first = expect_signed_in(&app.sign_in().await);
    let (_, me1) = app.get_as(&first, "/api/auth/me").await;

    let second = app.sign_in_with("/vaults", Tamper::default()).await;
    let cookie = expect_signed_in(&second);
    assert_eq!(second.path(), "/vaults", "an existing account goes straight to next");
    let (_, me2) = app.get_as(&cookie, "/api/auth/me").await;
    assert_eq!(me1["id"], me2["id"]);
    assert_eq!(app.user_count().await, 1);

    // a different person is a different account, and not the admin
    app.idp.behave(Behavior { sub: "bob-sub".into(), email: Some("Bob@Example.test".into()), ..Default::default() });
    let bob = expect_signed_in(&app.sign_in().await);
    let (_, me3) = app.get_as(&bob, "/api/auth/me").await;
    assert_ne!(me3["id"], me1["id"]);
    assert_eq!(me3["email"], "bob@example.test");
    assert_eq!(app.user_count().await, 2);
    let bob_user = models_user::user_by_email(&app.state.control, "bob@example.test").await.unwrap().unwrap();
    assert!(!eunomia_backend::authz::is_admin(&app.state.control, &bob_user).await.unwrap());
}

#[tokio::test]
async fn an_oidc_account_cannot_log_in_with_a_password() {
    let app = app(AuthMode::Both).await;
    expect_signed_in(&app.sign_in().await);
    for guess in [PASSWORD, "", "x", "no password"] {
        let (status, body) = http(&app.router, "POST", "/api/auth/login", Some(json!({"email": "alice@example.test", "password": guess})), None, None).await.0;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    }
}

#[tokio::test]
async fn a_token_that_does_not_validate_is_refused() {
    let app = app(AuthMode::Both).await;
    let cases: Vec<(&str, Behavior, &str)> = vec![
        ("wrong audience", Behavior { aud: json!("someone-else"), ..Default::default() }, "token"),
        ("audience list without us", Behavior { aud: json!(["a", "b"]), ..Default::default() }, "token"),
        ("several audiences, no azp", Behavior { aud: json!([CLIENT_ID, "other"]), ..Default::default() }, "token"),
        ("azp is someone else", Behavior { azp: Some("other".into()), ..Default::default() }, "token"),
        ("wrong issuer", Behavior { iss: Some("https://evil.example".into()), ..Default::default() }, "token"),
        ("expired", Behavior { exp_in: -3600, ..Default::default() }, "token"),
        ("signed by another key", Behavior { key: "b", ..Default::default() }, "token"),
        ("unknown kid", Behavior { kid: Some("nope".into()), ..Default::default() }, "token"),
        ("nonce mismatch", Behavior { nonce: Some("replayed-nonce".into()), ..Default::default() }, "token"),
        ("alg none", Behavior { alg: "none", ..Default::default() }, "token"),
        ("alg HS256", Behavior { alg: "HS256", ..Default::default() }, "token"),
    ];
    for (name, b, code) in cases {
        app.idp.behave(b);
        let landed = app.sign_in().await;
        assert_eq!(landed.error().as_deref(), Some(code), "{name}: {}", landed.location);
        assert!(landed.session.is_none(), "{name}");
    }
    assert_eq!(app.user_count().await, 0, "nothing was created by any refused token");
    // and the failures are in the ledger, without the provider's token
    let mut res = app.state.control.test_raw().query("SELECT detail FROM audit_event WHERE action = 'auth.oidc_login' AND outcome != 'ok'").await.unwrap();
    let rows: Vec<Value> = res.take(0).unwrap();
    assert_eq!(rows.len(), 11);
    // a good token still works afterwards
    app.idp.behave(Behavior::default());
    expect_signed_in(&app.sign_in().await);
}

#[tokio::test]
async fn state_cookie_and_provider_errors_are_refused() {
    let app = app(AuthMode::Both).await;
    expect_refused(&app.sign_in_with("/", Tamper { state: Some("forged"), ..Default::default() }).await, "state");
    // a callback from another browser has no flow cookie (login CSRF)
    expect_refused(&app.sign_in_with("/", Tamper { drop_cookie: true, ..Default::default() }).await, "state");
    expect_refused(&app.sign_in_with("/", Tamper { error: Some("access_denied"), ..Default::default() }).await, "denied");
    // a callback with a code the provider never issued
    let (_, headers, _) = raw(&app.router, Request::get("/api/auth/oidc/start").body(Body::empty()).unwrap()).await;
    let flow = cookie_pairs(&headers).remove(0);
    let url = reqwest::Url::parse(headers[header::LOCATION].to_str().unwrap()).unwrap();
    let state = url.query_pairs().find(|(k, _)| k == "state").unwrap().1.to_string();
    let uri = format!("/api/auth/oidc/callback?{}", serde_urlencoded::to_string([("code", "made-up"), ("state", state.as_str())]).unwrap());
    let (status, headers, _) = raw(&app.router, Request::get(uri).header(header::COOKIE, flow).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(headers[header::LOCATION].to_str().unwrap().contains("oidc_error=provider"));
    // a response naming another issuer (RFC 9207)
    let uri = format!("/api/auth/oidc/callback?{}", serde_urlencoded::to_string([("code", "x"), ("state", state.as_str()), ("iss", "https://evil.example")]).unwrap());
    let (_, headers, _) = raw(&app.router, Request::get(uri).header(header::COOKIE, cookie_pairs(&raw(&app.router, Request::get("/api/auth/oidc/start").body(Body::empty()).unwrap()).await.1).remove(0)).body(Body::empty()).unwrap()).await;
    assert!(headers[header::LOCATION].to_str().unwrap().contains("oidc_error="));
    assert_eq!(app.user_count().await, 0);
}

#[tokio::test]
async fn a_code_cannot_be_replayed_and_a_stolen_flow_cookie_is_single_use_at_the_provider() {
    let app = app(AuthMode::Both).await;
    // run the flow by hand so the same callback can be sent twice
    let (_, headers, _) = raw(&app.router, Request::get("/api/auth/oidc/start").body(Body::empty()).unwrap()).await;
    let flow = cookie_pairs(&headers).remove(0);
    let callback = at_provider(headers[header::LOCATION].to_str().unwrap()).await;
    let path = callback.strip_prefix("http://localhost:8001").unwrap().to_string();
    let go = || raw(&app.router, Request::get(path.clone()).header(header::COOKIE, flow.clone()).body(Body::empty()).unwrap());
    let (_, h1, _) = go().await;
    assert!(!h1[header::LOCATION].to_str().unwrap().contains("oidc_error"));
    let (_, h2, _) = go().await;
    assert!(h2[header::LOCATION].to_str().unwrap().contains("oidc_error=provider"), "the provider refuses a reused code");
    assert_eq!(app.user_count().await, 1);
}

#[tokio::test]
async fn an_unverified_email_does_not_take_over_a_password_account() {
    let app = app(AuthMode::Both).await;
    let victim = common::register(&app.state, "alice@example.test").await;
    app.idp.behave(Behavior { email_verified: json!(false), ..Default::default() });
    expect_refused(&app.sign_in().await, "email_unverified");
    // "email_verified" as a string "false" is no better; a missing claim neither
    app.idp.behave(Behavior { email_verified: json!("false"), ..Default::default() });
    expect_refused(&app.sign_in().await, "email_unverified");
    app.idp.behave(Behavior { email_verified: Value::Null, ..Default::default() });
    expect_refused(&app.sign_in().await, "email_unverified");
    let issuer = app.idp.inner.lock().unwrap().issuer.clone();
    assert!(models_user::user_by_identity(&app.state.control, &issuer, "alice-sub").await.unwrap().is_none());
    assert_eq!(app.user_count().await, 1);

    // an unverified email with no existing account may still make a new one (the provider vouches for the person, not the address)
    app.idp.behave(Behavior { sub: "carol-sub".into(), email: Some("carol@example.test".into()), email_verified: json!(false), ..Default::default() });
    expect_signed_in(&app.sign_in().await);

    // verified: it does not link silently to an account with a password; it asks first
    app.idp.behave(Behavior { email_verified: json!(true), ..Default::default() });
    let landed = app.sign_in().await;
    assert_eq!((landed.location.as_str(), landed.session.is_some()), ("/login/link", false));
    assert!(landed.link.is_some());
    assert!(models_user::user_by_identity(&app.state.control, &issuer, "alice-sub").await.unwrap().is_none());
    let _ = victim;
}

/// POST /api/auth/oidc/link with the cookie the provider sign-in left.
async fn link_as(app: &App, link_cookie: &str, body: Value) -> ((StatusCode, Value), Option<String>) {
    http(&app.router, "POST", "/api/auth/oidc/link", Some(body), None, Some(link_cookie)).await
}

/// A browser session for alice, by password.
async fn set_session(app: &App) -> String {
    let (_, set) = http(&app.router, "POST", "/api/auth/login", Some(json!({"email": "alice@example.test", "password": PASSWORD})), None, None).await;
    set.unwrap().split(';').next().unwrap().to_string()
}

async fn password_login(app: &App, email: &str, password: &str) -> StatusCode {
    http(&app.router, "POST", "/api/auth/login", Some(json!({"email": email, "password": password})), None, None).await.0 .0
}

async fn count_rows(app: &App, sql: &str, user: &eunomia_backend::models_user::User) -> i64 {
    let rows: Vec<Value> = app.state.control.test_raw().query(sql).bind(("u", user.id.clone())).await.unwrap().take(0).unwrap();
    rows.first().map_or(0, |r| r["n"].as_i64().unwrap_or(0))
}

/// A planted OAuth access token (an app connection) for `user`; the raw token.
async fn oauth_access_token(app: &App, user: &eunomia_backend::models_user::User) -> String {
    let raw = format!("{}{}", eunomia_backend::oauth::ACCESS_PREFIX, uuid::Uuid::new_v4().simple());
    app.state.control.test_raw()
        .query("LET $g = (CREATE oauth_grant SET owner = $u, client_id = 'client-x', client_name = 'Squatter App', scope = ['memory:read', 'memory:write'], resource = 'http://localhost:8001/mcp' RETURN VALUE id)[0]; \
                CREATE oauth_token SET kind = 'access', token_hash = $h, family = $g, scope = ['memory:read', 'memory:write'], expires_at = time::now() + 15m;")
        .bind(("u", user.id.clone()))
        .bind(("h", models_user::hash_token(&raw)))
        .await.unwrap().check().unwrap();
    raw
}

async fn mcp_ping(app: &App, token: &str) -> StatusCode {
    http(&app.router, "POST", "/mcp", Some(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "vault_list", "arguments": {}}})), Some(token), None).await.0 .0
}

/// Someone registered alice's address with their own password, connected an app, minted a token and set up
/// an extension that writes in. Alice's verified sign-in does not inherit any of it: starting fresh sets
/// the account aside (every credential it had stops working) and gives her a new, empty account.
#[tokio::test]
async fn starting_fresh_sets_the_old_account_aside_and_nothing_of_it_works_or_shows() {
    let app = app(AuthMode::Both).await;
    let old = common::register(&app.state, "alice@example.test").await;
    let token = models_user::create_api_token_with(&app.state.control, &old.id, "squatter", &eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect::<Vec<_>>(), None, None).await.unwrap().token;
    let oauth = oauth_access_token(&app, &old).await;
    let squatter_session = {
        let (_, set) = http(&app.router, "POST", "/api/auth/login", Some(json!({"email": "alice@example.test", "password": PASSWORD})), None, None).await;
        set.unwrap().split(';').next().unwrap().to_string()
    };
    // a memory, and an extension whose secret keeps writing in
    let wrote = http(&app.router, "POST", "/api/tools/memory_write", Some(json!({"subject_name": "Planted Person", "subject_kind": "person", "text": "planted by the squatter"})), Some(&token), None).await.0;
    assert_eq!(wrote.0, StatusCode::OK, "{}", wrote.1);
    let ((status, ext), _) = http(&app.router, "POST", "/api/extensions", Some(json!({"name": "squatter feed"})), Some(&token), None).await;
    assert_eq!(status, StatusCode::OK, "{ext}");
    let hook = format!("/hooks/ext/{}", ext["extension"]["webhook_url"].as_str().unwrap().rsplit('/').next().unwrap());
    let secret = ext["secret"].as_str().unwrap().to_string();
    assert_eq!(http(&app.router, "POST", &hook, Some(json!({"note": "in"})), Some(&secret), None).await.0 .0, StatusCode::ACCEPTED);
    assert_eq!(mcp_ping(&app, &oauth).await, StatusCode::OK, "the planted app connection works before");

    let landed = app.sign_in().await;
    assert_eq!(landed.location, "/login/link");
    let link_cookie = landed.link.expect("the pending-link cookie");
    let ((status, done), set) = link_as(&app, &link_cookie, json!({"start_fresh": true})).await;
    assert_eq!((status, done["started_fresh"].as_bool(), done["next"].as_str()), (StatusCode::OK, Some(true), Some("/onboarding")), "{done}");
    let session = set.expect("a session").split(';').next().unwrap().to_string();

    // a new account, not the old one, with nothing of it
    let (_, me) = app.get_as(&session, "/api/auth/me").await;
    assert_ne!(me["id"], json!(old.id.to_string()));
    assert_eq!((me["email"].as_str(), me["has_password"].as_bool(), me["oidc_provider"].as_str()), (Some("alice@example.test"), Some(false), Some("Mock IdP")));
    let (_, vaults) = app.get_as(&session, "/api/vaults").await;
    let list = vaults.as_array().or(vaults["results"].as_array()).unwrap().clone();
    assert_eq!(list.len(), 1, "only the new personal space: {vaults}");
    assert!(!app.get_as(&session, "/api/entities").await.1.to_string().contains("Planted Person"), "the old account's data is not visible");

    // every way the previous holder had in stops working
    assert_eq!(password_login(&app, "alice@example.test", PASSWORD).await, StatusCode::UNAUTHORIZED);
    assert_eq!(http(&app.router, "GET", "/api/vaults", None, Some(&token), None).await.0 .0, StatusCode::FORBIDDEN, "suspended");
    assert_ne!(mcp_ping(&app, &oauth).await, StatusCode::OK, "the app connection");
    assert_eq!(app.get_as(&squatter_session, "/api/auth/me").await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(http(&app.router, "POST", &hook, Some(json!({"note": "still in?"})), Some(&secret), None).await.0 .0, StatusCode::UNAUTHORIZED, "the extension secret");

    // the old account is set aside, data untouched, for an operator
    assert!(models_user::is_suspended(&app.state.control, &old.id).await.unwrap());
    let rows: Vec<Value> = app.state.control.test_raw().query("SELECT email, email_lc FROM $u").bind(("u", old.id.clone())).await.unwrap().take(0).unwrap();
    assert!(rows[0]["email_lc"].as_str().unwrap().starts_with("set-aside:"), "{rows:?}");
    assert!(rows[0]["email"].as_str().unwrap().contains("alice@example.test"), "the operator can see what it was: {rows:?}");
    assert_eq!(count_rows(&app, "SELECT count() AS n FROM api_token WHERE owner = $u GROUP ALL", &old).await, 1, "kept for review, not deleted");
    let audit: Vec<Value> = app.state.control.test_raw().query("SELECT user FROM audit_event WHERE action = 'auth.oidc_set_aside'").await.unwrap().take(0).unwrap();
    assert_eq!(audit, vec![json!({"user": old.id.to_string()})]);

    // the choice is spent; the next sign-in is an ordinary one into the new account
    for body in [json!({"start_fresh": true}), json!({"password": PASSWORD})] {
        let ((status, _), set) = link_as(&app, &link_cookie, body).await;
        // the account it named is set aside (suspended, checked first) and the identity is taken: refused, no session
        assert!(matches!(status, StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED) && set.is_none(), "{status}");
    }
    assert_eq!(app.user_count().await, 2, "no third account");
    let again = app.sign_in().await;
    assert!(again.link.is_none());
    let (_, me2) = app.get_as(&expect_signed_in(&again), "/api/auth/me").await;
    assert_eq!(me2["id"], me["id"]);
}

/// The person who made the account proves its password: linked, nothing removed, and the cookie is spent.
#[tokio::test]
async fn the_owner_proves_the_password_and_keeps_everything() {
    let app = app(AuthMode::Both).await;
    let alice = common::register(&app.state, "alice@example.test").await;
    let token = models_user::create_api_token_with(&app.state.control, &alice.id, "laptop", &eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect::<Vec<_>>(), None, None).await.unwrap().token;
    let link_cookie = app.sign_in().await.link.expect("asks first");
    let ((status, body), set) = link_as(&app, &link_cookie, json!({"password": "not-the-password"})).await;
    assert_eq!((status, body["code"].as_str(), set.is_none()), (StatusCode::UNAUTHORIZED, Some("auth.unauthorized"), true));
    let ((status, done), set) = link_as(&app, &link_cookie, json!({"password": PASSWORD})).await;
    assert_eq!((status, done["started_fresh"].as_bool()), (StatusCode::OK, Some(false)), "{done}");
    assert!(set.is_some());
    assert_eq!(http(&app.router, "GET", "/api/vaults", None, Some(&token), None).await.0 .0, StatusCode::OK, "the token survives");
    assert_eq!(password_login(&app, "alice@example.test", PASSWORD).await, StatusCode::OK, "and the password");
    let issuer = app.idp.inner.lock().unwrap().issuer.clone();
    assert_eq!(models_user::user_by_identity(&app.state.control, &issuer, "alice-sub").await.unwrap().map(|u| u.id), Some(alice.id.clone()));
    // the cookie is spent: replaying it starts nothing, and starting fresh with it sets nothing aside
    let ((status, _), set) = link_as(&app, &link_cookie, json!({"password": PASSWORD})).await;
    assert_eq!((status, set.is_none()), (StatusCode::UNAUTHORIZED, true));
    let session = set_session(&app).await;
    let ((status, _), set) = link_as(&app, &link_cookie, json!({"start_fresh": true})).await;
    assert_eq!((status, set.is_none()), (StatusCode::UNAUTHORIZED, true));
    assert!(!models_user::is_suspended(&app.state.control, &alice.id).await.unwrap(), "the owner's account is untouched");
    assert_eq!(app.get_as(&session, "/api/auth/me").await.0, StatusCode::OK, "and their sessions survive");
    // the next sign-in does not ask again
    let again = app.sign_in().await;
    assert!(again.link.is_none());
    expect_signed_in(&again);
}

/// A suspended account is an operator's decision: the link page can neither link it nor replace it.
#[tokio::test]
async fn a_suspended_account_can_be_neither_linked_nor_replaced() {
    let app = app(AuthMode::Both).await;
    let alice = common::register(&app.state, "alice@example.test").await;
    let link_cookie = app.sign_in().await.link.expect("asks first");
    app.state.control.test_raw().query("UPDATE $u SET suspended_at = time::now()").bind(("u", alice.id.clone())).await.unwrap().check().unwrap();
    for body in [json!({"password": PASSWORD}), json!({"start_fresh": true})] {
        let ((status, err), set) = link_as(&app, &link_cookie, body.clone()).await;
        assert_eq!((status, err["code"].as_str(), set.is_none()), (StatusCode::FORBIDDEN, Some("auth.suspended"), true), "{body}");
    }
    assert_eq!(app.user_count().await, 1, "no new account");
    let issuer = app.idp.inner.lock().unwrap().issuer.clone();
    let rows: Vec<Value> = app.state.control.test_raw().query("SELECT * FROM identity WHERE issuer = $i").bind(("i", issuer)).await.unwrap().take(0).unwrap();
    assert!(rows.is_empty(), "nothing linked");
}

/// Guessing the password here counts against the same throttle as the login form.
#[tokio::test]
async fn a_wrong_password_is_throttled_like_login() {
    let app = app(AuthMode::Both).await;
    common::register(&app.state, "alice@example.test").await;
    let link_cookie = app.sign_in().await.link.expect("asks first");
    for _ in 0..eunomia_backend::ratelimit::LOGIN_FAILS {
        assert_eq!(link_as(&app, &link_cookie, json!({"password": "guess-guess"})).await.0 .0, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(link_as(&app, &link_cookie, json!({"password": PASSWORD})).await.0 .0, StatusCode::TOO_MANY_REQUESTS, "even the right one, for a while");
    assert_eq!(password_login(&app, "alice@example.test", PASSWORD).await, StatusCode::TOO_MANY_REQUESTS, "the login form shares the count");
    // and no cookie, no link
    assert_eq!(http(&app.router, "POST", "/api/auth/oidc/link", Some(json!({"start_fresh": true})), None, None).await.0 .0, StatusCode::UNAUTHORIZED);
}

/// One account, one identity per provider: a second identity with the same address is refused, password
/// or not (an operator relinks it).
#[tokio::test]
async fn an_account_is_never_linked_to_a_second_identity_at_the_provider() {
    let app = app(AuthMode::Both).await;
    expect_signed_in(&app.sign_in().await);
    app.idp.behave(Behavior { sub: "alice-second-sub".into(), ..Default::default() });
    let landed = app.sign_in().await;
    assert!(landed.link.is_none());
    expect_refused(&landed, "identity_taken");
    assert_eq!(app.user_count().await, 1);
    let issuer = app.idp.inner.lock().unwrap().issuer.clone();
    assert!(models_user::user_by_identity(&app.state.control, &issuer, "alice-second-sub").await.unwrap().is_none());
}

#[tokio::test]
async fn provider_only_mode_turns_the_password_routes_off() {
    let app = app(AuthMode::Oidc).await;
    for (path, body) in [("/api/auth/login", json!({"email": "a@b.co", "password": PASSWORD})), ("/api/auth/register", json!({"email": "a@b.co", "password": PASSWORD}))] {
        let (status, problem) = http(&app.router, "POST", path, Some(body), None, None).await.0;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
        assert_eq!(problem["code"], "auth.password_disabled", "{problem}");
    }
    assert_eq!(app.user_count().await, 0);
    let (_, boot) = http(&app.router, "GET", "/api/auth/bootstrap", None, None, None).await.0;
    assert_eq!((boot["auth_mode"].as_str(), boot["oidc_provider"].as_str(), boot["has_users"].as_bool()), (Some("oidc"), Some("Mock IdP"), Some(false)));
    // the provider still works
    expect_signed_in(&app.sign_in().await);
    let (_, boot) = http(&app.router, "GET", "/api/auth/bootstrap", None, None, None).await.0;
    assert_eq!(boot["has_users"], true);
}

#[tokio::test]
async fn both_mode_keeps_passwords_and_password_mode_has_no_oidc_routes() {
    let both = app(AuthMode::Both).await;
    let (status, _) = http(&both.router, "POST", "/api/auth/register", Some(json!({"email": "pw@example.test", "password": PASSWORD})), None, None).await.0;
    assert_eq!(status, StatusCode::OK);
    let (_, boot) = http(&both.router, "GET", "/api/auth/bootstrap", None, None, None).await.0;
    assert_eq!(boot["auth_mode"], "both");

    // password mode (today's behaviour): the OIDC routes answer 404 and the bootstrap shows no provider
    let state = AppState::build(&test_settings(), engine_config()).await.unwrap();
    let router = eunomia_backend::app(state);
    let (status, problem) = http(&router, "GET", "/api/auth/oidc/start", None, None, None).await.0;
    assert_eq!(status, StatusCode::NOT_FOUND, "{problem}");
    let (_, boot) = http(&router, "GET", "/api/auth/bootstrap", None, None, None).await.0;
    assert_eq!((boot["auth_mode"].as_str(), boot["oidc_provider"].is_null()), (Some("password"), true));
}

#[tokio::test]
async fn next_is_kept_through_the_round_trip_and_never_leaves_the_site() {
    let app = app(AuthMode::Both).await;
    expect_signed_in(&app.sign_in().await);
    for bad in ["//evil.example/x", "https://evil.example", "/\\evil.example", "javascript:alert(1)"] {
        let l = app.sign_in_with(bad, Tamper::default()).await;
        expect_signed_in(&l);
        assert_eq!(l.location, "/", "{bad}");
    }
    let l = app.sign_in_with("/consent?client_id=abc&state=a%20b", Tamper::default()).await;
    assert_eq!(l.location, "/consent?client_id=abc&state=a%20b");
}

#[tokio::test]
async fn signing_keys_rotate_without_a_restart() {
    let app = app(AuthMode::Both).await;
    app.state.oidc.set_refetch_cooldown(std::time::Duration::from_secs(3600));
    expect_signed_in(&app.sign_in().await);
    let hits = app.idp.jwks_hits();
    assert_eq!(app.idp.jwks_hits(), hits);

    // the provider rotates: key-c replaces key-a. Inside the cooldown a token with an unknown kid does not refetch...
    app.idp.serve_keys(vec![("key-c", "c")]);
    app.idp.behave(Behavior { key: "c", kid: Some("key-c".into()), ..Default::default() });
    expect_refused(&app.sign_in().await, "token");
    assert_eq!(app.idp.jwks_hits(), hits, "an unknown kid inside the cooldown cannot make the server fetch");
    // ...and once the cooldown allows it, the new key is picked up
    app.state.oidc.set_refetch_cooldown(std::time::Duration::ZERO);
    expect_signed_in(&app.sign_in().await);
    assert_eq!(app.idp.jwks_hits(), hits + 1);
    // key-a is gone from the set now: its tokens are refused
    app.idp.behave(Behavior::default());
    expect_refused(&app.sign_in().await, "token");
}

#[tokio::test]
async fn the_email_can_come_from_userinfo_when_the_id_token_has_none() {
    let app = app(AuthMode::Both).await;
    app.idp.behave(Behavior { email: None, ..Default::default() });
    expect_refused(&app.sign_in().await, "no_email");
    app.idp.inner.lock().unwrap().userinfo = Some(json!({"sub": "someone-else", "email": "mallory@example.test", "email_verified": true}));
    expect_refused(&app.sign_in().await, "no_email");
    app.idp.inner.lock().unwrap().userinfo = Some(json!({"sub": "alice-sub", "email": "alice@example.test", "email_verified": true}));
    let cookie = expect_signed_in(&app.sign_in().await);
    assert_eq!(app.get_as(&cookie, "/api/auth/me").await.1["email"], "alice@example.test");
}

#[tokio::test]
async fn an_mcp_client_signs_in_through_the_provider_and_gets_a_working_token() {
    let app = app(AuthMode::Oidc).await;
    // 1. the client registers and sends the browser to /oauth/authorize
    const REDIRECT: &str = "http://127.0.0.1:7777/callback";
    const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let (status, reg) = http(&app.router, "POST", "/oauth/register", Some(json!({"client_name": "Agent", "redirect_uris": [REDIRECT], "token_endpoint_auth_method": "none"})), None, None).await.0;
    assert_eq!(status, StatusCode::CREATED, "{reg}");
    let client_id = reg["client_id"].as_str().unwrap();
    let authz = [
        ("response_type", "code"), ("client_id", client_id), ("redirect_uri", REDIRECT), ("state", "st-1"),
        ("code_challenge", &URL_SAFE_NO_PAD.encode(Sha256::digest(VERIFIER.as_bytes()))), ("code_challenge_method", "S256"),
        ("resource", "http://localhost:8001/mcp"),
    ];
    let q = serde_urlencoded::to_string(authz).unwrap();
    let (status, headers, _) = raw(&app.router, Request::get(format!("/oauth/authorize?{q}")).body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    // 2. not signed in: the login page, remembering the consent URL
    let login = headers[header::LOCATION].to_str().unwrap().to_string();
    let login_url = reqwest::Url::parse(&login).unwrap();
    assert_eq!(login_url.path(), "/login");
    let next = login_url.query_pairs().find(|(k, _)| k == "next").unwrap().1.to_string();
    assert!(next.starts_with("/consent?"), "{next}");
    // 3. the login page's "Continue with" button goes to the provider and back, carrying next
    let landed = app.sign_in_with(&next, Tamper::default()).await;
    let cookie = expect_signed_in(&landed);
    assert!(landed.path().starts_with("/onboarding?next=%2Fconsent"), "{}", landed.location);
    // 4. consent, as the page does it
    let consent_q = next.strip_prefix("/consent?").unwrap();
    let (status, info) = app.get_as(&cookie, &format!("/api/oauth/consent?{consent_q}")).await;
    assert_eq!(status, StatusCode::OK, "{info}");
    let mut params: HashMap<String, String> = serde_urlencoded::from_str(consent_q).unwrap();
    params.insert("approve".into(), "true".into());
    let body: Value = serde_json::to_value(&params).unwrap();
    let body = {
        let mut b = body;
        b["approve"] = json!(true);
        b
    };
    let (status, decided) = http(&app.router, "POST", "/api/oauth/consent", Some(body), None, Some(&cookie)).await.0;
    assert_eq!(status, StatusCode::OK, "{decided}");
    let cb = reqwest::Url::parse(decided["redirect_to"].as_str().unwrap()).unwrap();
    let code = cb.query_pairs().find(|(k, _)| k == "code").unwrap().1.to_string();
    // 5. the code buys a token that works on /mcp as the OIDC user
    let body = serde_urlencoded::to_string([("grant_type", "authorization_code"), ("code", code.as_str()), ("redirect_uri", REDIRECT), ("client_id", client_id), ("code_verifier", VERIFIER), ("resource", "http://localhost:8001/mcp")]).unwrap();
    let (status, _, bytes) = raw(&app.router, Request::post("/oauth/token").header(header::CONTENT_TYPE, "application/x-www-form-urlencoded").body(Body::from(body)).unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
    let access = serde_json::from_slice::<Value>(&bytes).unwrap()["access_token"].as_str().unwrap().to_string();
    let rpc = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "vault_list", "arguments": {}}});
    let (status, mcp) = http(&app.router, "POST", "/mcp", Some(rpc), Some(&access), None).await.0;
    assert_eq!(status, StatusCode::OK, "{mcp}");
    assert!(mcp.to_string().contains("alice@example.test") || mcp.to_string().contains("personal") || mcp.to_string().contains("Personal"), "{mcp}");
    assert!(app.idp.token_hits() >= 1);
}

#[tokio::test]
async fn a_provider_email_written_with_look_alike_letters_makes_no_account() {
    let app = app(AuthMode::Both).await;
    expect_signed_in(&app.sign_in().await);
    for (sub, email) in [("kelvin-sub", "\u{212A}ate@example.test"), ("angstrom-sub", "\u{212B}sa@example.test")] {
        app.idp.behave(Behavior { sub: sub.into(), email: Some(email.into()), ..Default::default() });
        expect_refused(&app.sign_in().await, "no_email");
    }
    assert_eq!(app.user_count().await, 1);
    assert!(models_user::user_by_email(&app.state.control, "kate@example.test").await.unwrap().is_none());
}

#[tokio::test]
async fn a_provider_email_with_an_ordinary_capital_outside_ascii_makes_an_account() {
    let app = app(AuthMode::Both).await;
    expect_signed_in(&app.sign_in().await);
    app.idp.behave(Behavior { sub: "olsen-sub".into(), email: Some("\u{d8}lsen@Firma.no".into()), ..Default::default() });
    expect_signed_in(&app.sign_in().await);
    assert!(models_user::user_by_email(&app.state.control, "\u{f8}lsen@firma.no").await.unwrap().is_some());
}
