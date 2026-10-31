//! The credential broker (docs/architecture/specs/broker.md) end to end against a MOCK secrets-helper
//! sidecar (an in-process axum server speaking the sidecar's v1 API): the provider token is stored only as
//! enc:v1: ciphertext and never returned; a resolved canary value reaches the tool and nothing else (no
//! audit_log, audit_event, tool_usage, capsule, change log, memory, skill or log line); every refusal is
//! `secret.unavailable` and audited with its outcome; org A's references never resolve for org B; a
//! vault-restricted token cannot use a reference bound to another vault.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
use common::TestApp;
use eunomia_backend::error::AppResult;
use eunomia_backend::rid::RecordIdExt;
use eunomia_backend::store::{CONTROL_TABLES, TENANT_TABLES};
use eunomia_backend::tools::registry;
use serde_json::{json, Value};
use tracing_subscriber::fmt::MakeWriter;

const CANARY: &str = "canary-secret-7f3a9c41-value-do-not-store";
const PROVIDER_TOKEN: &str = "connect-token-canary-0b5e2d88-bootstrap";
const REF: &str = "op://Ops/Square/access_token";
const KEY: &str = "internal-key-for-the-mock";

// ---- log capture (as tests/isolation.rs) ------------------------------------------------------

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
        let sub = tracing_subscriber::fmt().with_env_filter("warn,eunomia_backend=trace").with_writer(Sink).with_ansi(false).finish();
        let _ = tracing::subscriber::set_global_default(sub);
    });
}

// ---- the mock sidecar -------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Mock {
    /// reference -> value; anything else answers not_found
    values: Arc<Mutex<HashMap<String, String>>>,
    /// every request body the mock saw
    seen: Arc<Mutex<Vec<Value>>>,
    /// answer every request with this status instead
    fail_with: Arc<Mutex<Option<u16>>>,
}

async fn resolve(State(m): State<Mock>, headers: HeaderMap, Json(body): Json<Value>) -> (axum::http::StatusCode, Json<Value>) {
    m.seen.lock().unwrap().push(body.clone());
    if let Some(code) = *m.fail_with.lock().unwrap() {
        return (axum::http::StatusCode::from_u16(code).unwrap(), Json(json!({"error": "internal"})));
    }
    if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some(&format!("Bearer {KEY}")) {
        return (axum::http::StatusCode::UNAUTHORIZED, Json(json!({"error": "unauthorized"})));
    }
    let token_ok = body["provider"]["token"] == PROVIDER_TOKEN;
    let values = m.values.lock().unwrap().clone();
    let results: Vec<Value> = body["refs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let r = r.as_str().unwrap();
            match values.get(r) {
                _ if !token_ok => json!({"ref": r, "ok": false, "error": "denied", "message": "the Connect server refused the token"}),
                Some(v) => json!({"ref": r, "ok": true, "value": v}),
                None => json!({"ref": r, "ok": false, "error": "not_found", "message": "no such field"}),
            }
        })
        .collect();
    (axum::http::StatusCode::OK, Json(json!({"results": results})))
}

async fn check(State(m): State<Mock>, Json(body): Json<Value>) -> Json<Value> {
    m.seen.lock().unwrap().push(body.clone());
    if body["provider"]["token"] == PROVIDER_TOKEN { Json(json!({"ok": true, "detail": "1 vault visible"})) } else { Json(json!({"ok": false, "error": "denied", "message": "refused"})) }
}

async fn mock_sidecar() -> (String, Mock) {
    let mock = Mock::default();
    mock.values.lock().unwrap().insert(REF.to_string(), CANARY.to_string());
    let app = Router::new().route("/v1/resolve", post(resolve)).route("/v1/check", post(check)).with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, mock)
}

// ---- test tools (registered at runtime; not in all_tools(), so no other suite sees them) ------

/// Echoes `text` back and writes it to memory through the real memory_write handler, so both the
/// result scrub and the write-path scrub are exercised.
fn register_tools() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        registry::register_test_tool(
            "test_echo_secret",
            Arc::new(|state, owner, args| {
                Box::pin(async move {
                    let text = args["text"].as_str().unwrap_or_default().to_string();
                    let write = registry::all_tools()["memory_write"].handler.clone();
                    let wrote = write(state, owner, json!({"subject_name": "Square", "subject_kind": "organisation", "text": format!("token is {text}")})).await?;
                    Ok(json!({"echo": text, "nested": [{"again": format!("x{text}y")}], "stored": wrote["memory"]["text"]}))
                })
            }),
        );
        registry::register_test_tool(
            "test_fail_secret",
            Arc::new(|_state, _owner, args| {
                Box::pin(async move {
                    let text = args["text"].as_str().unwrap_or_default().to_string();
                    let r: AppResult<Value> = Err(eunomia_backend::error::AppError::internal(format!("upstream said {text}")).with_source(format!("raw upstream body with {text}")));
                    r
                })
            }),
        );
    });
}

struct World {
    app: TestApp,
    mock: Mock,
    personal: String,
}

async fn world() -> World {
    capture_logs();
    register_tools();
    let (url, mock) = mock_sidecar().await;
    let mut settings = common::test_settings();
    settings.secrets_helper_url = url;
    settings.secrets_helper_key = KEY.into();
    let app = TestApp::with_settings(settings).await;
    let personal = eunomia_backend::vaults::service::personal_vault_id(&app.db().await, &app.user.id).await.unwrap().to_string();
    World { app, mock, personal }
}

async fn configure(w: &World) {
    let (status, body) = w.app.http_session("PUT", "/api/secrets/providers/onepassword_connect", Some(json!({"host": "https://connect.example.com", "token": PROVIDER_TOKEN}))).await;
    assert_eq!(status, 200, "{body}");
}

async fn register_ref(w: &World, reference: &str, tools: &[&str], actors: &[&str], vault: &str) -> Value {
    let (status, body) = w
        .app
        .http_session("POST", "/api/secrets/refs", Some(json!({"reference": reference, "provider": "onepassword_connect", "vault_id": vault, "allowed_tools": tools, "allowed_actors": actors})))
        .await;
    assert_eq!(status, 200, "{body}");
    body
}

async fn raw(w: &World, db: &str, q: &str) -> Vec<Value> {
    if db == "control" {
        return w.app.control().test_raw().query(q).await.unwrap().take::<Vec<Value>>(0).unwrap();
    }
    let org = w.app.db().await;
    org.test_raw().query(q).await.unwrap().take::<Vec<Value>>(0).unwrap()
}

async fn resolutions(w: &World) -> Vec<Value> {
    raw(w, "control", "SELECT outcome, detail FROM audit_event WHERE action = 'secret.resolve' ORDER BY created_at, id").await
}

async fn call(w: &World, tool: &str, args: Value) -> Value {
    Box::pin(w.app.tool(tool, args)).await
}

// ---- tests ------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_provider_token_is_ciphertext_and_never_returned() {
    let w = world().await;
    configure(&w).await;
    let stored = raw(&w, "org", "SELECT VALUE token_enc FROM secret_provider").await;
    assert_eq!(stored.len(), 1);
    let enc = stored[0].as_str().unwrap();
    assert!(enc.starts_with("enc:v1:") && !enc.contains(PROVIDER_TOKEN), "{enc}");

    let (status, list) = w.app.http_session("GET", "/api/secrets/providers", None).await;
    assert_eq!(status, 200);
    let text = list.to_string();
    assert!(!text.contains(PROVIDER_TOKEN) && !text.contains("enc:v1:") && !text.contains("token_enc"), "{text}");
    assert_eq!(list[0]["configured"], true);
    assert_eq!(list[0]["host"], "https://connect.example.com");

    // a host change keeps the token; a token cannot be set from a PAT; a plaintext token is refused by the schema
    let (status, _) = w.app.http_session("PUT", "/api/secrets/providers/onepassword_connect", Some(json!({"host": "https://connect2.example.com"}))).await;
    assert_eq!(status, 200);
    assert_eq!(raw(&w, "org", "SELECT VALUE token_enc FROM secret_provider").await[0].as_str().unwrap(), enc);
    let (status, body) = w.app.http("PUT", "/api/secrets/providers/onepassword_connect", Some(json!({"token": "x-from-a-pat"})), true).await;
    assert_eq!(status, 403, "{body}");
    let refused = w.app.db().await.test_raw().query("UPSERT secret_provider:bitwarden_sm SET kind = 'bitwarden_sm', token_enc = 'plain', created_by = user:x, updated_by = user:x").await.unwrap().check();
    assert!(refused.is_err(), "a plaintext token must not be storable");
    // no token for a new kind is an error, not an empty token
    let (status, _) = w.app.http_session("PUT", "/api/secrets/providers/bitwarden_sm", Some(json!({}))).await;
    assert_eq!(status, 400);

    // test connection goes to the sidecar with the decrypted token
    let (status, body) = w.app.http_session("POST", "/api/secrets/providers/onepassword_connect/test", None).await;
    assert_eq!((status.as_u16(), body["ok"].clone()), (200, json!(true)), "{body}");
    assert!(w.mock.seen.lock().unwrap().iter().any(|b| b["provider"]["token"] == PROVIDER_TOKEN));

    let (status, _) = w.app.http_session("DELETE", "/api/secrets/providers/onepassword_connect", None).await;
    assert_eq!(status, 200);
    assert!(raw(&w, "org", "SELECT * FROM secret_provider").await.is_empty());
    assert!(!LOG.lock().unwrap().contains(PROVIDER_TOKEN), "the token was logged");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resolved_value_reaches_the_tool_and_nothing_else() {
    let w = world().await;
    configure(&w).await;
    let r = register_ref(&w, REF, &["test_echo_secret", "test_fail_secret"], &[], &w.personal).await;

    let out = call(&w, "test_echo_secret", json!({"text": {"$secret": REF}})).await;
    assert!(out.get("error").is_none(), "{out}");
    let text = out.to_string();
    assert!(!text.contains(CANARY), "the result kept the value: {text}");
    assert_eq!(out["echo"], "[redacted: secret]");
    assert_eq!(out["nested"][0]["again"], "x[redacted: secret]y");
    assert_eq!(out["stored"], "token is [redacted: secret]", "the memory write in the same call was scrubbed");
    // the tool really got the value: the mock was asked, with the token, for exactly this reference
    let seen = w.mock.seen.lock().unwrap().clone();
    assert!(seen.iter().any(|b| b["refs"] == json!([REF]) && b["provider"]["token"] == PROVIDER_TOKEN), "{seen:?}");

    // a failing tool whose error carries the value: scrubbed before the agent and the capsule see it
    let failed = Box::pin(eunomia_backend::authz::as_system(registry::call(&w.app.state, &w.app.user, "test_fail_secret", json!({"text": {"$secret": REF}})))).await;
    let e = failed.expect_err("the tool fails");
    assert!(!e.message.contains(CANARY) && !e.source.as_deref().unwrap_or_default().contains(CANARY), "{e:?}");
    assert!(e.message.contains("[redacted: secret]"), "{e:?}");

    // nowhere at rest, in either database, or in the logs
    for table in TENANT_TABLES {
        let rows = raw(&w, "org", &format!("SELECT * FROM {table}")).await;
        assert!(!serde_json::to_string(&rows).unwrap().contains(CANARY), "{table} holds the value");
    }
    for table in CONTROL_TABLES {
        let rows = raw(&w, "control", &format!("SELECT * FROM {table}")).await;
        assert!(!serde_json::to_string(&rows).unwrap().contains(CANARY), "control {table} holds the value");
    }
    // the audit summary keeps the reference
    let summaries = raw(&w, "org", "SELECT VALUE args_summary FROM audit_log WHERE tool_name = 'test_echo_secret'").await;
    assert!(summaries.iter().any(|s| s.as_str().unwrap().contains(REF)), "{summaries:?}");
    assert!(!LOG.lock().unwrap().contains(CANARY), "the value was logged");

    // one ok resolution per call, with who, the ref, the tool and the channel; no value
    let log = resolutions(&w).await;
    assert_eq!(log.len(), 2, "{log:?}");
    let d: Value = serde_json::from_str(log[0]["detail"].as_str().unwrap()).unwrap();
    assert_eq!((log[0]["outcome"].as_str(), d["ref"].as_str(), d["tool"].as_str(), d["ref_id"].as_str()), (Some("ok"), Some(REF), Some("test_echo_secret"), r["id"].as_str()));

    // the resolution log and counts, admin only
    let (status, page) = w.app.http_session("GET", "/api/secrets/log?limit=1", None).await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["entries"].as_array().unwrap().len(), 1);
    assert!(page["next_before"].is_string());
    let (_, counts) = w.app.http_session("GET", "/api/secrets/counts", None).await;
    assert_eq!(counts[0]["reference"], REF);
    assert_eq!(counts[0]["ok"], 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_refusal_is_secret_unavailable_audited_and_runs_nothing() {
    let w = world().await;
    let unavailable = |v: &Value| v["code"] == "secret.unavailable" && !v.to_string().contains(CANARY);

    // no provider configured yet
    register_ref(&w, REF, &["test_echo_secret"], &[], &w.personal).await;
    let out = call(&w, "test_echo_secret", json!({"text": {"$secret": REF}})).await;
    assert!(unavailable(&out), "{out}");
    configure(&w).await;

    for (args, tool, want) in [
        (json!({"text": {"$secret": "op://Ops/Square/unregistered"}}), "test_echo_secret", "denied"),
        (json!({"text": {"$secret": "op://bad"}}), "test_echo_secret", "denied"),
        (json!({"text": {"$secret": REF, "extra": 1}}), "test_echo_secret", "denied"),
        (json!({"text": {"$secret": REF}}), "test_fail_secret", "denied"), // not in allowed_tools
    ] {
        let out = call(&w, tool, args.clone()).await;
        assert!(unavailable(&out), "{args}: {out}");
        let last = resolutions(&w).await.pop().unwrap();
        assert_eq!(last["outcome"], want, "{args}");
    }

    // a built-in tool never resolves one (it would store the value), and the memory is not written
    let before = raw(&w, "org", "SELECT count() FROM memory GROUP ALL").await;
    let out = call(&w, "memory_write", json!({"subject_name": "Square", "subject_kind": "organisation", "text": {"$secret": REF}})).await;
    assert!(unavailable(&out), "{out}");
    assert!(out["error"].as_str().unwrap().contains("does not take secrets"), "{out}");
    assert_eq!(raw(&w, "org", "SELECT count() FROM memory GROUP ALL").await, before);

    // the provider says not found
    register_ref(&w, "op://Ops/Square/gone", &["test_echo_secret"], &[], &w.personal).await;
    let out = call(&w, "test_echo_secret", json!({"text": {"$secret": "op://Ops/Square/gone"}})).await;
    assert!(unavailable(&out), "{out}");
    assert_eq!(resolutions(&w).await.pop().unwrap()["outcome"], "not_found");

    // two references, one fine and one not: neither is used
    let out = call(&w, "test_echo_secret", json!({"text": {"$secret": REF}, "other": {"$secret": "op://Ops/Square/gone"}})).await;
    assert!(unavailable(&out), "{out}");
    let last2: Vec<Value> = resolutions(&w).await.into_iter().rev().take(2).collect();
    assert!(last2.iter().all(|r| r["outcome"] != "ok"), "{last2:?}");

    // the sidecar down, or refusing the key: provider_error, never a fallback
    *w.mock.fail_with.lock().unwrap() = Some(500);
    let out = call(&w, "test_echo_secret", json!({"text": {"$secret": REF}})).await;
    assert!(unavailable(&out), "{out}");
    assert_eq!(resolutions(&w).await.pop().unwrap()["outcome"], "provider_error");
    *w.mock.fail_with.lock().unwrap() = Some(401);
    let out = call(&w, "test_echo_secret", json!({"text": {"$secret": REF}})).await;
    assert!(out["error"].as_str().unwrap().contains("internal key"), "{out}");
    *w.mock.fail_with.lock().unwrap() = None;

    // the provider refuses the token
    let (status, _) = w.app.http_session("PUT", "/api/secrets/providers/onepassword_connect", Some(json!({"host": "https://connect.example.com", "token": "a-wrong-token"}))).await;
    assert_eq!(status, 200);
    let out = call(&w, "test_echo_secret", json!({"text": {"$secret": REF}})).await;
    assert!(unavailable(&out), "{out}");
    assert_eq!(resolutions(&w).await.pop().unwrap()["outcome"], "denied");
    assert!(!LOG.lock().unwrap().contains("a-wrong-token"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn allowed_actors_and_vault_restricted_tokens() {
    let w = world().await;
    configure(&w).await;
    let team = call(&w, "vault_create", json!({"name": "Team"})).await["id"].as_str().unwrap_or_else(|| panic!("no vault id")).to_string();
    // bound to the personal vault, any member; and one bound to Team for one named token only
    register_ref(&w, REF, &["test_echo_secret"], &[], &w.personal).await;
    let other = models::token(&w, None).await;
    register_ref(&w, "op://Ops/Team/key", &["test_echo_secret"], &[&other.1], &team).await;
    w.mock.values.lock().unwrap().insert("op://Ops/Team/key".into(), "team-value-4c1d".into());

    // over REST with a PAT: the token named in allowed_actors may, the test app's own token may not
    let ok = models::invoke(&w, &other.0, json!({"text": {"$secret": "op://Ops/Team/key"}})).await;
    assert_eq!(ok["echo"], "[redacted: secret]", "{ok}");
    let refused = models::invoke(&w, &w.app.token, json!({"text": {"$secret": "op://Ops/Team/key"}})).await;
    assert_eq!(refused["code"], "secret.unavailable", "{refused}");

    // a token restricted to Team cannot use the reference bound to the personal vault
    let restricted = models::token(&w, Some(&team)).await;
    let out = models::invoke(&w, &restricted.0, json!({"vault_id": team, "text": {"$secret": REF}})).await;
    assert_eq!(out["code"], "secret.unavailable", "{out}");
    // it cannot even see that reference: a restricted token reads only its own vault
    assert!(out["error"].as_str().unwrap().contains("(denied: "), "{out}");
    let channel = resolutions(&w).await.pop().unwrap();
    let d: Value = serde_json::from_str(channel["detail"].as_str().unwrap()).unwrap();
    assert_eq!(d["channel"], "rest");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn org_a_references_never_resolve_for_org_b_and_settings_are_owner_only() {
    let w = world().await;
    configure(&w).await;
    register_ref(&w, REF, &["*"], &[], &w.personal).await;
    let b = common::register_personal(&w.app.state, "orgb@example.com").await;
    assert_ne!(b.org, w.app.user.org);
    let out = Box::pin(eunomia_backend::authz::as_system(registry::call(&w.app.state, &b, "test_echo_secret", json!({"text": {"$secret": REF}})))).await.unwrap();
    assert_eq!(out["code"], "secret.unavailable", "{out}");
    assert!(out["error"].as_str().unwrap().contains("not registered"), "{out}");
    assert!(!w.mock.seen.lock().unwrap().iter().any(|b| b["refs"].is_array() && b["provider"]["token"] != PROVIDER_TOKEN), "org B never reached the sidecar with a token of its own");

    // org B's owner sees an empty broker; a member of org A who is not an owner is refused
    let b_token = eunomia_backend::models_user::create_api_token(&w.app.state.control, &b.id, "b").await.unwrap().token;
    let ((status, refs), _) = common::http(&w.app.router, "GET", "/api/secrets/refs", None, Some(&b_token), None).await;
    assert_eq!((status.as_u16(), refs), (200, json!([])));
    let member = common::register(&w.app.state, "member@example.com").await;
    assert_eq!(member.org, w.app.user.org, "a second signup joins the install's org");
    let m_token = eunomia_backend::models_user::create_api_token(&w.app.state.control, &member.id, "m").await.unwrap().token;
    let ((status, body), _) = common::http(&w.app.router, "GET", "/api/secrets/refs", None, Some(&m_token), None).await;
    assert_eq!(status, 403, "{body}");
}

mod models {
    use super::*;

    /// A PAT for the test user, optionally restricted to a vault: (token, actor id).
    pub async fn token(w: &World, vault: Option<&str>) -> (String, String) {
        let scopes: Vec<String> = eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect();
        let v = vault.map(|v| eunomia_backend::rid::parse(v).unwrap());
        let t = eunomia_backend::models_user::create_api_token_with(&w.app.state.control, &w.app.user.id, "t", &scopes, v.as_ref(), None).await.unwrap();
        (t.token, t.id.to_string())
    }

    /// The echo tool over REST as `token`.
    pub async fn invoke(w: &World, token: &str, args: Value) -> Value {
        let ((_, body), _) = common::http(&w.app.router, "POST", "/api/tools/test_echo_secret", Some(args), Some(token), None).await;
        body
    }
}
