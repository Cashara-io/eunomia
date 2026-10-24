//! Shared harness: a fresh in-memory SurrealDB per test, the real schema, the
//! real axum router, and helpers to call registry tools and HTTP routes.
//! Nothing here touches the network.
#![allow(dead_code)]

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use eunomia_backend::config::Settings;
use eunomia_backend::models_user::{self, User};
use eunomia_backend::pool::{ControlDb, OrgDb};
use eunomia_backend::state::AppState;
use eunomia_backend::tools::registry;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

/// Runs service code as the system: direct calls into services have no request credential, and an
/// authorization check with none in scope fails closed.
pub async fn sys<F: std::future::Future>(f: F) -> F::Output {
    eunomia_backend::authz::as_system(f).await
}

pub const PASSWORD: &str = "correct-horse-battery";

pub struct TestApp {
    pub state: AppState,
    pub user: User,
    pub token: String,
    pub router: Router,
}

/// In-memory SurrealDB by default. `TEST_SURREAL_URL=ws://127.0.0.1:8231/rpc` (with
/// `TEST_SURREAL_USER` / `TEST_SURREAL_PASS`) runs the same suite against a real server, e.g. one
/// started with the hardened flags from docker-compose.yml; each call then gets a fresh database.
pub fn test_settings() -> Settings {
    let url = std::env::var("TEST_SURREAL_URL").unwrap_or_else(|_| "mem://".into());
    // a real server is shared between tests: every call gets its own namespace, so its own control database
    let ns = if url == "mem://" { "test".to_string() } else { format!("t{}", uuid::Uuid::new_v4().simple()) };
    Settings {
        jwt_secret: "test-jwt-secret".into(),
        surreal_url: url,
        surreal_user: std::env::var("TEST_SURREAL_USER").unwrap_or_else(|_| "root".into()),
        surreal_pass: std::env::var("TEST_SURREAL_PASS").unwrap_or_else(|_| "root".into()),
        surreal_ns: ns,
        surreal_db: "legacy".into(),
        openai_api_key: None,
        openai_base_url: "https://api.openai.com/v1".into(), // no server key: no model is configured, so nothing is called
        openai_chat_model: String::new(),
        encryption_key: "test-encryption-key".into(),
        embeddings_backend: "openai".into(),
        cors_allowed_origins: "http://localhost:3000".into(),
        log_level: "WARN".into(),
        update_status_dir: "/nonexistent".into(),
        bind_addr: "127.0.0.1:0".into(),
        public_url: "http://localhost:8001".into(),
        // SurrealDB's built-in file bucket, in memory
        documents_backend: "memory".into(),
        documents_max_bytes: 256 * 1024,
        auth: Default::default(),
    }
}

/// Engine options like production. `TEST_HARDENED=1` (mem:// only) runs the embedded engine with
/// everything denied except the `--allow-funcs` list in docker-compose.yml, so the whole suite
/// proves the list is complete.
pub fn engine_config() -> surrealdb::opt::Config {
    use surrealdb::opt::capabilities::{Capabilities, ExperimentalFeature};
    // file buckets (documents) are experimental: the server runs with --allow-experimental=files
    let mut config = surrealdb::opt::Config::new().capabilities(Capabilities::new().with_experimental_feature_allowed(ExperimentalFeature::Files));
    if std::env::var("TEST_HARDENED").is_ok() {
        let compose = include_str!("../../../docker-compose.yml");
        let list = compose.split("--allow-funcs=").nth(1).expect("--allow-funcs in docker-compose.yml").split_whitespace().next().unwrap();
        let mut caps = Capabilities::none().with_experimental_feature_allowed(ExperimentalFeature::Files);
        for f in list.trim_end_matches(['"', '\'']).split(',') {
            caps = caps.with_function_allowed(f).expect("function target");
        }
        config = config.capabilities(caps);
    }
    config
}

/// Fresh databases (control, no orgs yet) + app state, no users yet.
pub async fn bare_state() -> AppState {
    AppState::build(&test_settings(), engine_config()).await.expect("state")
}

/// Register a user the way signup does (and, for the first, create the install's org).
pub async fn register(state: &AppState, email: &str) -> User {
    models_user::register_user(state, email, PASSWORD).await.expect("register")
}

/// The user's org database.
pub async fn org_db(state: &AppState, user: &User) -> OrgDb {
    state.pool.for_org(&user.org).await.expect("org db")
}

impl TestApp {
    /// Fresh DB plus one registered user (with their personal vault, exactly
    /// as signup creates it) and an API token for Bearer auth.
    pub async fn new() -> Self {
        Self::with_settings(test_settings()).await
    }

    /// Like [`TestApp::new`] with other settings (for example a real `update_status_dir`).
    pub async fn with_settings(settings: Settings) -> Self {
        let state = AppState::build(&settings, engine_config()).await.expect("state");
        let user = register(&state, "tester@example.com").await;
        let token = models_user::create_api_token(&state.control, &user.id, "test").await.expect("token").token;
        let router = eunomia_backend::app(state.clone());
        TestApp { state, user, token, router }
    }

    /// The test user's org database.
    pub async fn db(&self) -> OrgDb {
        org_db(&self.state, &self.user).await
    }

    pub fn control(&self) -> &ControlDb {
        &self.state.control
    }

    /// Call a registry tool as the test user (the same choke point REST and MCP use).
    pub async fn tool(&self, name: &str, args: Value) -> Value {
        eunomia_backend::authz::as_system(registry::call(&self.state, &self.user, name, args)).await.expect("tool call")
    }

    /// Log in as the test user the way the browser does; the `name=value` cookie pair.
    pub async fn session_cookie(&self) -> String {
        let body = serde_json::json!({ "email": self.user.email, "password": PASSWORD });
        let (_, set_cookie) = http(&self.router, "POST", "/api/auth/login", Some(body), None, None).await;
        set_cookie.expect("login sets a cookie").split(';').next().unwrap().to_string()
    }

    /// One HTTP request as the logged-in browser session (not a token).
    pub async fn http_session(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let cookie = self.session_cookie().await;
        http(&self.router, method, path, body, None, Some(&cookie)).await.0
    }

    /// One HTTP request against the real router. `auth` adds the Bearer token.
    pub async fn http(&self, method: &str, path: &str, body: Option<Value>, auth: bool) -> (StatusCode, Value) {
        http(&self.router, method, path, body, auth.then_some(self.token.as_str()), None).await.0
    }
}

/// Returns ((status, json body), set-cookie header if any).
pub async fn http(
    router: &Router,
    method: &str,
    path: &str,
    body: Option<Value>,
    bearer: Option<&str>,
    cookie: Option<&str>,
) -> ((StatusCode, Value), Option<String>) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(t) = bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, c);
    }
    let req = match body {
        Some(b) => req.header(header::CONTENT_TYPE, "application/json").body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let resp = router.clone().oneshot(req).await.expect("router");
    let status = resp.status();
    let set_cookie = resp.headers().get(header::SET_COOKIE).and_then(|v| v.to_str().ok()).map(str::to_string);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    ((status, json), set_cookie)
}

/// Turns run-specific values (record ids, timestamps, hashes) into stable
/// placeholders, numbered by first appearance so equal values stay equal in
/// the snapshot.
#[derive(Default)]
pub struct Normalizer {
    ids: std::collections::HashMap<String, String>,
}

impl Normalizer {
    pub fn apply(&mut self, v: &Value) -> Value {
        use regex::Regex;
        let ts = Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})?").unwrap();
        let hash = Regex::new(r"\b[0-9a-f]{64}\b").unwrap();
        // surreal random ids are 20 lowercase alphanumerics, e.g. entity:abc...
        let rid = Regex::new(r"\b[0-9a-z]{20}\b").unwrap();
        let mut s = v.to_string();
        s = ts.replace_all(&s, "<ts>").into_owned();
        s = hash.replace_all(&s, "<hash>").into_owned();
        // trace ids are random per request
        s = Regex::new(r"\b[0-9a-f]{32}\b").unwrap().replace_all(&s, "<trace>").into_owned();
        let found: Vec<String> = rid.find_iter(&s).map(|m| m.as_str().to_string()).collect();
        for id in found {
            let n = self.ids.len() + 1;
            let ph = self.ids.entry(id.clone()).or_insert_with(|| format!("<id{n}>")).clone();
            s = s.replace(&id, &ph);
        }
        serde_json::from_str(&s).unwrap()
    }
}

/// Register a user in an org of their own (signup with `EUNOMIA_SIGNUP_ORG=personal`).
pub async fn register_personal(state: &AppState, email: &str) -> User {
    models_user::register_user_with(state, email, PASSWORD, true).await.expect("register")
}
