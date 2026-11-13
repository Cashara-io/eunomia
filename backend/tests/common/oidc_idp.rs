//! The in-test OpenID Connect provider and the app harness around it, shared by `tests/oidc.rs` and
//! `tests/oidc_invitations.rs` (axum on 127.0.0.1:0 serving discovery, a JWKS, `/authorize`, `/token` and
//! `/userinfo`, signing ID tokens with an ES256 test key). Each binary declares `mod common;` and
//! `#[path = "common/oidc_keys.rs"] mod keys;` before including this file.
#![allow(dead_code, unused_imports)]

pub use std::collections::HashMap;
pub use std::sync::{Arc, Mutex};

pub use axum::body::Body;
pub use axum::extract::{Query, State};
pub use axum::http::{HeaderMap, Request, StatusCode, header};
pub use axum::response::{IntoResponse, Redirect};
pub use axum::routing::{get, post};
pub use axum::{Form, Json, Router};
pub use base64::Engine;
pub use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
pub use crate::common::{PASSWORD, engine_config, http, test_settings};
pub use eunomia_backend::config::Settings;
pub use eunomia_backend::models_user;
pub use eunomia_backend::oidc::{AuthConfig, AuthMode, OidcConfig};
pub use eunomia_backend::rid::RecordIdExt;
pub use eunomia_backend::state::AppState;
pub use http_body_util::BodyExt;
pub use jsonwebtoken::{Algorithm, EncodingKey, Header};
pub use serde_json::{Value, json};
pub use sha2::{Digest, Sha256};
pub use tower::ServiceExt;

pub const CLIENT_ID: &str = "eunomia-test-client";
/// The browser origin the test settings allow (`CORS_ALLOWED_ORIGINS`).
pub const FRONTEND_ORIGIN: &str = "http://localhost:3000";
pub const CLIENT_SECRET: &str = "s3cret-for-tests";

// -- the mock provider -------------------------------------------------------------------------

/// What the next ID token looks like. Defaults to a valid token for alice.
#[derive(Clone)]
pub struct Behavior {
    pub sub: String,
    pub email: Option<String>,
    pub email_verified: Value,
    pub iss: Option<String>,
    pub aud: Value,
    pub azp: Option<String>,
    /// seconds from now
    pub exp_in: i64,
    pub nonce: Option<String>,
    /// which key signs: "a" (served), "b" (not served: a forgery) or "c" (served after a rotation)
    pub key: &'static str,
    pub kid: Option<String>,
    /// "ES256", or "HS256" / "none" for the algorithm-confusion attempts
    pub alg: &'static str,
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

pub struct Pending {
    pub nonce: String,
    pub challenge: String,
    pub redirect_uri: String,
}

#[derive(Default)]
pub struct Inner {
    pub codes: HashMap<String, Pending>,
    pub behavior: Option<Behavior>,
    /// the JWKS served: (kid, key letter)
    pub served: Vec<(String, &'static str)>,
    pub jwks_hits: usize,
    pub token_hits: usize,
    /// the userinfo answer, when the provider keeps the email out of the ID token
    pub userinfo: Option<Value>,
    pub issuer: String,
}

#[derive(Clone)]
pub struct Idp {
    pub inner: Arc<Mutex<Inner>>,
}

pub fn jwk(kid: &str, key: &str) -> Value {
    let (x, y) = match key {
        "a" => (crate::keys::KEY_A_X, crate::keys::KEY_A_Y),
        "c" => (crate::keys::KEY_C_X, crate::keys::KEY_C_Y),
        _ => (crate::keys::KEY_B_X, crate::keys::KEY_B_Y),
    };
    json!({"kty": "EC", "crv": "P-256", "alg": "ES256", "use": "sig", "kid": kid, "x": x, "y": y})
}

impl Idp {
    pub async fn start() -> (Idp, String) {
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

    pub fn behave(&self, b: Behavior) {
        self.inner.lock().unwrap().behavior = Some(b);
    }

    pub fn serve_keys(&self, served: Vec<(&str, &'static str)>) {
        self.inner.lock().unwrap().served = served.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    }

    pub fn jwks_hits(&self) -> usize {
        self.inner.lock().unwrap().jwks_hits
    }

    pub fn token_hits(&self) -> usize {
        self.inner.lock().unwrap().token_hits
    }
}

pub async fn discovery(State(idp): State<Idp>) -> Json<Value> {
    let i = idp.inner.lock().unwrap().issuer.clone();
    Json(json!({
        "issuer": i, "authorization_endpoint": format!("{i}/authorize"), "token_endpoint": format!("{i}/token"),
        "jwks_uri": format!("{i}/jwks"), "userinfo_endpoint": format!("{i}/userinfo"),
        "token_endpoint_auth_methods_supported": ["client_secret_basic"], "response_types_supported": ["code"],
    }))
}

pub async fn jwks(State(idp): State<Idp>) -> Json<Value> {
    let mut g = idp.inner.lock().unwrap();
    g.jwks_hits += 1;
    Json(json!({"keys": g.served.iter().map(|(k, v)| jwk(k, v)).collect::<Vec<_>>()}))
}

pub async fn authorize(State(idp): State<Idp>, Query(q): Query<HashMap<String, String>>) -> impl IntoResponse {
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

pub fn bad_request(error: &str) -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({"error": error})))
}

pub async fn token(State(idp): State<Idp>, headers: HeaderMap, Form(f): Form<HashMap<String, String>>) -> (StatusCode, Json<Value>) {
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

pub async fn userinfo(State(idp): State<Idp>, headers: HeaderMap) -> (StatusCode, Json<Value>) {
    if headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) != Some("Bearer at-1") {
        return (StatusCode::UNAUTHORIZED, Json(json!({})));
    }
    (StatusCode::OK, Json(idp.inner.lock().unwrap().userinfo.clone().unwrap_or(json!({}))))
}

pub fn sign(b: &Behavior, claims: &Value) -> String {
    let pem = match b.key {
        "a" => crate::keys::KEY_A_PEM,
        "c" => crate::keys::KEY_C_PEM,
        _ => crate::keys::KEY_B_PEM,
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
            jsonwebtoken::encode(&h, claims, &EncodingKey::from_secret(crate::keys::KEY_A_X.as_bytes())).unwrap()
        }
        _ => {
            let mut h = Header::new(Algorithm::ES256);
            h.kid = b.kid.clone();
            jsonwebtoken::encode(&h, claims, &EncodingKey::from_ec_pem(pem.as_bytes()).unwrap()).unwrap()
        }
    }
}

// -- the app under test ------------------------------------------------------------------------

pub struct App {
    pub state: AppState,
    pub router: Router,
    pub idp: Idp,
}

pub fn settings(mode: AuthMode, issuer: &str) -> Settings {
    Settings {
        auth: AuthConfig {
            mode,
            oidc: Some(OidcConfig {
                issuer: issuer.to_string(),
                client_id: CLIENT_ID.into(),
                client_secret: Some(CLIENT_SECRET.into()),
                scopes: "openid email profile".into(),
                provider_name: "Mock IdP".into(),
                email_verified_when_absent: false,
            }),
        },
        ..test_settings()
    }
}

/// An empty install (no users) whose sign-in goes through the mock provider.
pub async fn app(mode: AuthMode) -> App {
    app_with(mode, false).await
}

/// [`app`] with `OIDC_EMAIL_VERIFIED_WHEN_ABSENT` set as given.
pub async fn app_with(mode: AuthMode, email_verified_when_absent: bool) -> App {
    let (idp, issuer) = Idp::start().await;
    let mut settings = settings(mode, &issuer);
    if let Some(o) = settings.auth.oidc.as_mut() {
        o.email_verified_when_absent = email_verified_when_absent;
    }
    let state = AppState::build(&settings, engine_config()).await.expect("state");
    let router = eunomia_backend::app(state.clone());
    App { state, router, idp }
}

pub struct Landed {
    pub status: StatusCode,
    pub location: String,
    /// `name=value` of the session cookie when one was set
    pub session: Option<String>,
    /// `name=value` of the cookie a sign-in leaves when it must ask before linking to a password account
    pub link: Option<String>,
}

impl Landed {
    pub fn error(&self) -> Option<String> {
        let q = self.location.split_once('?')?.1;
        serde_urlencoded::from_str::<HashMap<String, String>>(q).ok()?.remove("oidc_error")
    }

    pub fn path(&self) -> &str {
        &self.location
    }
}

pub async fn raw(router: &Router, req: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let resp = router.clone().oneshot(req).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    (status, headers, resp.into_body().collect().await.unwrap().to_bytes().to_vec())
}

pub fn cookie_pairs(headers: &HeaderMap) -> Vec<String> {
    headers.get_all(header::SET_COOKIE).iter().filter_map(|v| v.to_str().ok()).map(|v| v.split(';').next().unwrap().to_string()).collect()
}

/// Follows the provider's redirect without a redirect policy (the mock is a real HTTP server).
pub async fn at_provider(authorize_url: &str) -> String {
    let c = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let r = c.get(authorize_url).send().await.unwrap();
    assert!(r.status().is_redirection(), "{}", r.status());
    r.headers()[header::LOCATION].to_str().unwrap().to_string()
}

#[derive(Default)]
pub struct Tamper {
    pub state: Option<&'static str>,
    pub drop_cookie: bool,
    pub error: Option<&'static str>,
}

impl App {
    /// The whole browser round trip: start, the provider, the callback.
    pub async fn sign_in_with(&self, next: &str, tamper: Tamper) -> Landed {
        let uri = format!("/api/auth/oidc/start?{}", serde_urlencoded::to_string([("next", next)]).unwrap());
        self.complete(Request::get(uri).body(Body::empty()).unwrap(), tamper).await
    }

    /// The invitation page's "Continue with" button: a same-site form post carrying the link's token.
    pub async fn sign_in_invited(&self, token: &str) -> Landed {
        self.complete(self.invited_start(token, Some(FRONTEND_ORIGIN)), Tamper::default()).await
    }

    /// The start request the invitation page posts, from `origin` (none: no Origin header).
    pub fn invited_start(&self, token: &str, origin: Option<&str>) -> Request<Body> {
        let body = serde_urlencoded::to_string([("next", "/invite"), ("invitation", token)]).unwrap();
        let mut req = Request::post("/api/auth/oidc/start").header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some(o) = origin {
            req = req.header(header::ORIGIN, o);
        }
        req.body(Body::from(body)).unwrap()
    }

    /// Start (as given), the provider, the callback.
    pub async fn complete(&self, start: Request<Body>, tamper: Tamper) -> Landed {
        let (status, headers, _) = raw(&self.router, start).await;
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

    pub async fn sign_in(&self) -> Landed {
        self.sign_in_with("/", Tamper::default()).await
    }

    pub async fn get_as(&self, cookie: &str, path: &str) -> (StatusCode, Value) {
        http(&self.router, "GET", path, None, None, Some(cookie)).await.0
    }

    pub async fn user_count(&self) -> usize {
        let mut res = self.state.control.test_raw().query("SELECT count() FROM user GROUP ALL").await.unwrap();
        let rows: Vec<Value> = res.take(0).unwrap();
        rows.first().map_or(0, |r| r["count"].as_u64().unwrap_or(0) as usize)
    }
}

pub fn expect_signed_in(l: &Landed) -> String {
    assert_eq!(l.status, StatusCode::SEE_OTHER, "{}", l.location);
    assert_eq!(l.error(), None, "{}", l.location);
    l.session.clone().expect("a session cookie")
}

pub fn expect_refused(l: &Landed, code: &str) {
    assert_eq!(l.status, StatusCode::SEE_OTHER);
    assert_eq!(l.error().as_deref(), Some(code), "{}", l.location);
    assert!(l.location.starts_with("/login?"), "{}", l.location);
    assert!(l.session.is_none(), "no session on a refused sign-in");
}

