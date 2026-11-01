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
    let app = Router::new()
        .route("/v1/resolve", post(resolve))
        .route("/v1/check", post(check))
        .route("/healthz", axum::routing::get(|| async { Json(json!({"ok": true, "api": 1, "version": "9.9.9-mock"})) }))
        .with_state(mock.clone());
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
    world_with(|_| {}).await
}

/// A world with changed settings (the helper switched off, unreachable, a real update-status directory).
async fn world_with(change: impl FnOnce(&mut eunomia_backend::config::Settings)) -> World {
    capture_logs();
    register_tools();
    let (url, mock) = mock_sidecar().await;
    let mut settings = common::test_settings();
    settings.secrets_helper_url = url;
    settings.secrets_helper_key = KEY.into();
    change(&mut settings);
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

    // edit and remove through the API (the vault check repeats on every statement)
    let id = r["id"].as_str().unwrap();
    let (status, edited) = w.app.http_session("PUT", &format!("/api/secrets/refs/{id}"), Some(json!({"vault_id": w.personal, "allowed_tools": ["test_echo_secret", "test_fail_secret", "other_tool"], "label": "Square"}))).await;
    assert_eq!(status, 200, "{edited}");
    assert_eq!(edited["allowed_tools"], json!(["test_echo_secret", "test_fail_secret", "other_tool"]));
    assert_eq!(edited["label"], "Square");
    let (status, _) = w.app.http_session("POST", "/api/secrets/refs", Some(json!({"reference": "op://Ops/Temp/x", "provider": "onepassword_connect", "vault_id": w.personal, "allowed_tools": ["*"]}))).await;
    assert_eq!(status, 200);
    let temp = raw(&w, "org", "SELECT VALUE <string> id FROM credential_ref WHERE reference = 'op://Ops/Temp/x'").await;
    let temp = temp[0].as_str().unwrap();
    assert_eq!(w.app.http_session("DELETE", &format!("/api/secrets/refs/{temp}"), None).await.0, 200);
    assert_eq!(w.app.http_session("DELETE", &format!("/api/secrets/refs/{temp}"), None).await.0, 404, "gone");

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

    // org B's owner sees an empty broker; a member of org A who is not an org owner sees only the
    // references of the vaults they own (none here: org A's are in the owner's vaults) and no provider
    let b_token = eunomia_backend::models_user::create_api_token(&w.app.state.control, &b.id, "b").await.unwrap().token;
    let ((status, refs), _) = common::http(&w.app.router, "GET", "/api/secrets/refs", None, Some(&b_token), None).await;
    assert_eq!((status.as_u16(), refs), (200, json!([])));
    let member = common::register(&w.app.state, "member@example.com").await;
    assert_eq!(member.org, w.app.user.org, "a second signup joins the install's org");
    let m_token = eunomia_backend::models_user::create_api_token(&w.app.state.control, &member.id, "m").await.unwrap().token;
    let ((status, body), _) = common::http(&w.app.router, "GET", "/api/secrets/refs", None, Some(&m_token), None).await;
    assert_eq!((status.as_u16(), body), (200, json!([])));
    let ((status, body), _) =
        common::http(&w.app.router, "PUT", "/api/secrets/providers/onepassword_connect", Some(json!({"host": "https://x.example.com"})), Some(&m_token), None).await;
    assert_eq!(status, 403, "{body}");
}

// ---- decided by Avi on 2026-11-01: Bitwarden is a preview; the helper is behind a profile; vault owners ----

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bitwarden_is_a_preview_in_the_settings_api() {
    let w = world().await;
    let (status, kinds) = w.app.http_session("GET", "/api/secrets/kinds", None).await;
    assert_eq!(status, 200, "{kinds}");
    let by: HashMap<String, Value> = kinds.as_array().unwrap().iter().map(|k| (k["kind"].as_str().unwrap().to_string(), k.clone())).collect();
    assert_eq!(by.len(), 3, "{kinds}");
    assert_eq!((by["bitwarden_sm"]["preview"].clone(), by["bitwarden_sm"]["stability"].clone()), (json!(true), json!("preview")));
    assert!(by["bitwarden_sm"]["note"].as_str().unwrap().starts_with("Preview"), "{kinds}");
    for k in ["onepassword_connect", "onepassword_service_account"] {
        assert_eq!((by[k]["preview"].clone(), by[k]["stability"].clone()), (json!(false), json!("stable")), "{k}");
        assert!(by[k].get("note").is_none(), "{k}");
    }
    // a configured Bitwarden provider carries the same metadata
    let (status, body) = w.app.http_session("PUT", "/api/secrets/providers/bitwarden_sm", Some(json!({"token": "0.machine-account-token-fixture"}))).await;
    assert_eq!(status, 200, "{body}");
    configure(&w).await;
    let (_, providers) = w.app.http_session("GET", "/api/secrets/providers", None).await;
    let bw = providers.as_array().unwrap().iter().find(|p| p["kind"] == "bitwarden_sm").unwrap();
    assert_eq!((bw["preview"].clone(), bw["stability"].clone(), bw["label"].clone()), (json!(true), json!("preview"), json!("Bitwarden Secrets Manager")));
    let op = providers.as_array().unwrap().iter().find(|p| p["kind"] == "onepassword_connect").unwrap();
    assert_eq!(op["preview"], false);
}

/// The time bound for "answered at once" with the helper off. A backstop only: that nothing reaches the
/// mock sidecar is the real proof that no network call was made.
const OFF_FAST: std::time::Duration = std::time::Duration::from_secs(10);

/// The helper switched off (no `secrets` compose profile): every resolution and test-connection fails at
/// once with secret.unavailable / helper_disabled, nothing reaches the sidecar, settings still load.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_switched_off_fails_fast_and_settings_still_load() {
    let w = world_with(|s| s.secrets_helper_enabled = false).await;
    configure(&w).await;
    register_ref(&w, REF, &["test_echo_secret"], &[], &w.personal).await;

    let started = std::time::Instant::now();
    let out = call(&w, "test_echo_secret", json!({"text": {"$secret": REF}})).await;
    // "at once" is proven by nothing reaching the sidecar (checked below); the time bound is only a
    // backstop, loose enough for a loaded CI runner (a 2 s bound failed there with the full suite running)
    assert!(started.elapsed() < OFF_FAST, "not fast: {:?}", started.elapsed());
    assert_eq!((out["code"].clone(), out["reason"].clone()), (json!("secret.unavailable"), json!("helper_disabled")), "{out}");
    assert!(!out.to_string().contains(CANARY));
    let last = resolutions(&w).await.pop().unwrap();
    assert_eq!(last["outcome"], "provider_error");
    let d: Value = serde_json::from_str(last["detail"].as_str().unwrap()).unwrap();
    assert_eq!(d["code"], "helper_disabled", "{d}");

    // over REST too: the tool error value (as every tool error on REST and MCP) with the reason
    let ((_, body), _) = common::http(&w.app.router, "POST", "/api/tools/test_echo_secret", Some(json!({"text": {"$secret": REF}})), Some(&w.app.token), None).await;
    assert_eq!((body["code"].clone(), body["reason"].clone()), (json!("secret.unavailable"), json!("helper_disabled")), "{body}");

    let started = std::time::Instant::now();
    let (status, body) = w.app.http_session("POST", "/api/secrets/providers/onepassword_connect/test", None).await;
    assert!(started.elapsed() < OFF_FAST, "not fast: {:?}", started.elapsed());
    assert_eq!((status.as_u16(), body["code"].clone(), body["reason"].clone()), (503, json!("secret.unavailable"), json!("helper_disabled")), "{body}");
    assert!(w.mock.seen.lock().unwrap().is_empty(), "nothing reached the sidecar");

    // settings load as usual
    for path in ["/api/secrets/providers", "/api/secrets/refs", "/api/secrets/log", "/api/secrets/counts", "/api/secrets/kinds", "/api/secrets/me"] {
        let (status, body) = w.app.http_session("GET", path, None).await;
        assert_eq!(status, 200, "{path}: {body}");
    }
    let (_, helper) = w.app.http_session("GET", "/api/secrets/helper", None).await;
    assert_eq!((helper["state"].clone(), helper["reason"].clone(), helper["enabled"].clone()), (json!("off"), json!("helper_disabled"), json!(false)), "{helper}");
    assert_eq!(helper["profile"], "secrets");
    // a tool call with no reference is untouched
    let out = call(&w, "vault_list", json!({})).await;
    assert!(out.get("code").is_none(), "{out}");
}

/// The helper on but not answering: helper_unreachable, within the connect timeout, never a hang.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_unreachable_fails_fast() {
    let w = world_with(|s| s.secrets_helper_url = "http://127.0.0.1:9".into()).await;
    configure(&w).await;
    register_ref(&w, REF, &["test_echo_secret"], &[], &w.personal).await;
    let started = std::time::Instant::now();
    let out = call(&w, "test_echo_secret", json!({"text": {"$secret": REF}})).await;
    assert!(started.elapsed() < std::time::Duration::from_secs(4), "not fast: {:?}", started.elapsed());
    assert_eq!((out["code"].clone(), out["reason"].clone()), (json!("secret.unavailable"), json!("helper_unreachable")), "{out}");
    let (status, body) = w.app.http_session("POST", "/api/secrets/providers/onepassword_connect/test", None).await;
    assert_eq!((status.as_u16(), body["reason"].clone()), (503, json!("helper_unreachable")), "{body}");
    let (_, helper) = w.app.http_session("GET", "/api/secrets/helper", None).await;
    assert_eq!((helper["state"].clone(), helper["reason"].clone(), helper["enabled"].clone()), (json!("unreachable"), json!("helper_unreachable"), json!(true)), "{helper}");
}

/// Status running against the mock; "Enable password managers" drops a request for the updater, admins
/// only and from a browser session; without the update-status directory it says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_status_and_the_enable_request() {
    let w = world().await;
    let (_, helper) = w.app.http_session("GET", "/api/secrets/helper", None).await;
    assert_eq!((helper["state"].clone(), helper["version"].clone(), helper["can_request"].clone()), (json!("running"), json!("9.9.9-mock"), json!(false)), "{helper}");
    assert!(helper["reason"].is_null(), "{helper}");
    let (status, ack) = w.app.http_session("POST", "/api/secrets/helper", Some(json!({"enabled": true}))).await;
    assert_eq!((status.as_u16(), ack), (200, json!({"configured": false, "requested": false})));

    let dir = std::env::temp_dir().join(format!("eunomia-secrets-helper-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let d = dir.to_string_lossy().to_string();
    let w = world_with(|s| {
        s.secrets_helper_enabled = false;
        s.update_status_dir = d;
    })
    .await;
    let (status, ack) = w.app.http_session("POST", "/api/secrets/helper", Some(json!({"enabled": true}))).await;
    assert_eq!((status.as_u16(), ack), (200, json!({"configured": true, "requested": true})));
    let file = std::fs::read_to_string(dir.join("secrets.json")).unwrap();
    assert!(file.lines().any(|l| l.trim() == "\"enabled\": true"), "one key per line for the updater: {file}");
    let (_, helper) = w.app.http_session("GET", "/api/secrets/helper", None).await;
    assert_eq!((helper["state"].clone(), helper["pending"].clone(), helper["can_request"].clone()), (json!("starting"), json!("enable"), json!(true)), "{helper}");

    // the updater's outcome: an error shows as off with its message; a stale "starting" is not starting
    std::fs::remove_file(dir.join("secrets.json")).unwrap();
    std::fs::write(dir.join("secrets-status.json"), r#"{"state": "error", "message": "could not start the secrets helper: boom", "checked_at": "2026-11-01T00:00:00Z"}"#).unwrap();
    let (_, helper) = w.app.http_session("GET", "/api/secrets/helper", None).await;
    assert_eq!((helper["state"].clone(), helper["message"].clone()), (json!("off"), json!("could not start the secrets helper: boom")), "{helper}");
    std::fs::write(dir.join("secrets-status.json"), r#"{"state": "starting", "message": null, "checked_at": "2020-01-01T00:00:00Z"}"#).unwrap();
    let (_, helper) = w.app.http_session("GET", "/api/secrets/helper", None).await;
    assert_eq!(helper["state"], "off", "{helper}");

    // a token may not ask (browser session only), nor may a member who is not an instance admin
    let (status, body) = w.app.http("POST", "/api/secrets/helper", Some(json!({"enabled": false})), true).await;
    assert_eq!((status.as_u16(), body["code"].clone()), (403, json!("auth.scope")), "{body}");
    common::register(&w.app.state, "member@example.com").await;
    let cookie = login(&w, "member@example.com").await;
    let ((status, body), _) = common::http(&w.app.router, "POST", "/api/secrets/helper", Some(json!({"enabled": false})), None, Some(&cookie)).await;
    assert_eq!((status.as_u16(), body["code"].clone()), (403, json!("auth.forbidden")), "{body}");
    assert!(!dir.join("secrets.json").exists());
    let ((status, _), _) = common::http(&w.app.router, "GET", "/api/secrets/helper", None, None, Some(&cookie)).await;
    assert_eq!(status, 200, "anyone signed in reads the status");
    let _ = std::fs::remove_dir_all(&dir);
}

async fn login(w: &World, email: &str) -> String {
    let ((status, body), cookie) = common::http(&w.app.router, "POST", "/api/auth/login", Some(json!({"email": email, "password": common::PASSWORD})), None, None).await;
    assert_eq!(status, 200, "{body}");
    cookie.unwrap().split(';').next().unwrap().to_string()
}

/// One request as `cookie`'s user.
async fn as_user(w: &World, cookie: &str, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let ((status, body), _) = common::http(&w.app.router, method, path, body, None, Some(cookie)).await;
    (status.as_u16(), body)
}

/// Vault owners manage the references of the vaults they own (decided by Avi on 2026-11-01): their own
/// vault yes; another vault, a provider and the org's token no; a plain member no; a vault-restricted
/// token never (auth.scope, even for its own vault); org B no; the org owner and admin as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vault_owners_manage_their_own_vaults_references() {
    let w = world().await;
    configure(&w).await;
    let st = &w.app.state;
    let team = call(&w, "vault_create", json!({"name": "Team"})).await["id"].as_str().unwrap().to_string();
    let team_ref = register_ref(&w, "op://Ops/Team/key", &["test_echo_secret"], &[], &team).await;
    register_ref(&w, REF, &["test_echo_secret"], &[], &w.personal).await;

    // bob: a plain member of Team, and the owner of his own vault "Bob"
    let bob = common::register(st, "bob@example.com").await;
    let db = w.app.db().await;
    let team_id = eunomia_backend::rid::parse(&team).unwrap();
    common::sys(eunomia_backend::vaults::service::invite_member(&db, &st.control, &w.app.user.id, &team_id, "bob@example.com", "member")).await.unwrap();
    common::sys(eunomia_backend::vaults::service::accept_invitation(&db, &bob.id, &team_id)).await.unwrap();
    let own = Box::pin(common::sys(registry::call(st, &bob, "vault_create", json!({"name": "Bob"})))).await.unwrap()["id"].as_str().unwrap().to_string();
    let bob_c = login(&w, "bob@example.com").await;

    let (_, me) = as_user(&w, &bob_c, "GET", "/api/secrets/me", None).await;
    assert_eq!((me["manage_providers"].clone(), me["manage_refs"].clone(), me["toggle_helper"].clone()), (json!(false), json!(true), json!(false)), "{me}");
    let vaults: Vec<&str> = me["vaults"].as_array().unwrap().iter().map(|v| v["id"].as_str().unwrap()).collect();
    assert!(vaults.contains(&own.as_str()) && !vaults.contains(&team.as_str()), "{me}");

    // own vault: create, edit, remove
    let body = json!({"reference": "op://Ops/Bob/key", "provider": "onepassword_connect", "vault_id": own, "allowed_tools": ["test_echo_secret"]});
    let (status, mine) = as_user(&w, &bob_c, "POST", "/api/secrets/refs", Some(body)).await;
    assert_eq!(status, 200, "{mine}");
    let mine_id = mine["id"].as_str().unwrap().to_string();
    let (status, edited) = as_user(&w, &bob_c, "PUT", &format!("/api/secrets/refs/{mine_id}"), Some(json!({"vault_id": own, "label": "Bob's key", "allowed_tools": ["*"]}))).await;
    assert_eq!((status, edited["label"].clone()), (200, json!("Bob's key")), "{edited}");
    // he sees only his own vault's references
    let (_, list) = as_user(&w, &bob_c, "GET", "/api/secrets/refs", None).await;
    let ids: Vec<&str> = list.as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [mine_id.as_str()], "{list}");

    // Team (he is a plain member): no create, no edit, no remove, and no moving his reference there
    let (status, body) = as_user(&w, &bob_c, "POST", "/api/secrets/refs", Some(json!({"reference": "op://Ops/Team/other", "provider": "onepassword_connect", "vault_id": team, "allowed_tools": ["*"]}))).await;
    assert_eq!((status, body["code"].clone()), (403, json!("vault.forbidden")), "{body}");
    let team_ref_id = team_ref["id"].as_str().unwrap();
    let (status, _) = as_user(&w, &bob_c, "PUT", &format!("/api/secrets/refs/{team_ref_id}"), Some(json!({"vault_id": own, "allowed_tools": ["*"]}))).await;
    assert_eq!(status, 404);
    let (status, _) = as_user(&w, &bob_c, "DELETE", &format!("/api/secrets/refs/{team_ref_id}"), None).await;
    assert_eq!(status, 404);
    let (status, body) = as_user(&w, &bob_c, "PUT", &format!("/api/secrets/refs/{mine_id}"), Some(json!({"vault_id": team, "allowed_tools": ["*"]}))).await;
    assert_eq!((status, body["code"].clone()), (403, json!("vault.forbidden")), "{body}");
    let row = raw(&w, "org", &format!("SELECT allowed_tools, vault FROM {team_ref_id}")).await;
    assert_eq!(row[0]["allowed_tools"], json!(["test_echo_secret"]), "the Team reference is untouched");

    // the provider and its org-wide token: never
    for (method, path, body) in [
        ("PUT", "/api/secrets/providers/onepassword_connect", Some(json!({"host": "https://evil.example.com", "token": "bob-token"}))),
        ("DELETE", "/api/secrets/providers/onepassword_connect", None),
        ("POST", "/api/secrets/providers/onepassword_connect/test", None),
        ("POST", "/api/secrets/helper", Some(json!({"enabled": false}))),
    ] {
        let (status, out) = as_user(&w, &bob_c, method, path, body).await;
        assert_eq!((status, out["code"].clone()), (403, json!("auth.forbidden")), "{method} {path}: {out}");
    }
    // he sees which kinds are configured, not the host
    let (_, providers) = as_user(&w, &bob_c, "GET", "/api/secrets/providers", None).await;
    assert_eq!((providers[0]["kind"].clone(), providers[0]["host"].clone()), (json!("onepassword_connect"), Value::Null), "{providers}");

    // the resolution log and counts: his vault's only; the org owner sees all of it
    w.mock.values.lock().unwrap().insert("op://Ops/Bob/key".into(), "bob-value-77aa".into());
    let out = Box::pin(common::sys(registry::call(st, &bob, "test_echo_secret", json!({"text": {"$secret": "op://Ops/Bob/key"}})))).await.unwrap();
    assert_eq!(out["echo"], "[redacted: secret]", "{out}");
    call(&w, "test_echo_secret", json!({"text": {"$secret": REF}})).await;
    let (_, log) = as_user(&w, &bob_c, "GET", "/api/secrets/log", None).await;
    let entries = log["entries"].as_array().unwrap();
    assert!(!entries.is_empty() && entries.iter().all(|e| e["vault_id"] == own), "{log}");
    let (_, counts) = as_user(&w, &bob_c, "GET", "/api/secrets/counts", None).await;
    let refs: Vec<&str> = counts.as_array().unwrap().iter().map(|c| c["reference"].as_str().unwrap()).collect();
    assert_eq!(refs, ["op://Ops/Bob/key"], "{counts}");
    let (_, all) = w.app.http_session("GET", "/api/secrets/log", None).await;
    let vaults: Vec<&str> = all["entries"].as_array().unwrap().iter().map(|e| e["vault_id"].as_str().unwrap()).collect();
    assert!(vaults.contains(&own.as_str()) && vaults.contains(&w.personal.as_str()), "{all}");
    // a small page for a vault owner still pages through his rows only
    let (_, page) = as_user(&w, &bob_c, "GET", "/api/secrets/log?limit=1", None).await;
    assert_eq!(page["entries"].as_array().unwrap().len(), 1, "{page}");

    // a token restricted to his own vault: auth.scope on every route, reading included
    let scopes: Vec<String> = eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect();
    let own_id = eunomia_backend::rid::parse(&own).unwrap();
    let restricted = eunomia_backend::models_user::create_api_token_with(&st.control, &bob.id, "r", &scopes, Some(&own_id), None).await.unwrap().token;
    for (method, path) in [("GET", "/api/secrets/refs"), ("POST", "/api/secrets/refs"), ("GET", "/api/secrets/log"), ("GET", "/api/secrets/me")] {
        let body = (method == "POST").then(|| json!({"reference": "op://Ops/Bob/other", "provider": "onepassword_connect", "vault_id": own, "allowed_tools": ["*"]}));
        let ((status, out), _) = common::http(&w.app.router, method, path, body, Some(&restricted), None).await;
        assert_eq!((status.as_u16(), out["code"].clone()), (403, json!("auth.scope")), "{method} {path}: {out}");
    }
    // his unrestricted token works like his session
    let unrestricted = eunomia_backend::models_user::create_api_token(&st.control, &bob.id, "u").await.unwrap().token;
    let ((status, list), _) = common::http(&w.app.router, "GET", "/api/secrets/refs", None, Some(&unrestricted), None).await;
    assert_eq!((status.as_u16(), list.as_array().map(Vec::len)), (200, Some(1)), "{list}");

    // carol: a plain member with no vault of her own besides her personal one: Team is not hers
    common::register(st, "carol@example.com").await;
    let carol_c = login(&w, "carol@example.com").await;
    let (status, list) = as_user(&w, &carol_c, "GET", "/api/secrets/refs", None).await;
    assert_eq!((status, list), (200, json!([])));
    let (status, body) = as_user(&w, &carol_c, "POST", "/api/secrets/refs", Some(json!({"reference": "op://Ops/Team/c", "provider": "onepassword_connect", "vault_id": team, "allowed_tools": ["*"]}))).await;
    assert_eq!((status, body["code"].clone()), (403, json!("vault.forbidden")), "{body}");
    let (_, log) = as_user(&w, &carol_c, "GET", "/api/secrets/log", None).await;
    assert_eq!(log["entries"], json!([]), "{log}");

    // org B's owner: none of org A's references or log
    let b = common::register_personal(st, "orgb-owner@example.com").await;
    let b_token = eunomia_backend::models_user::create_api_token(&st.control, &b.id, "b").await.unwrap().token;
    let ((status, _), _) = common::http(&w.app.router, "DELETE", &format!("/api/secrets/refs/{mine_id}"), None, Some(&b_token), None).await;
    assert_eq!(status, 404);
    let ((_, log), _) = common::http(&w.app.router, "GET", "/api/secrets/log", None, Some(&b_token), None).await;
    assert_eq!(log["entries"], json!([]), "{log}");

    // the org owner (also the instance admin) as before: Team's reference, and the provider
    let (status, body) = w.app.http_session("PUT", &format!("/api/secrets/refs/{team_ref_id}"), Some(json!({"vault_id": team, "allowed_tools": ["*"]}))).await;
    assert_eq!(status, 200, "{body}");
    let (_, me) = w.app.http_session("GET", "/api/secrets/me", None).await;
    assert_eq!((me["manage_providers"].clone(), me["toggle_helper"].clone()), (json!(true), json!(true)), "{me}");

    // and bob removes his own
    let (status, _) = as_user(&w, &bob_c, "DELETE", &format!("/api/secrets/refs/{mine_id}"), None).await;
    assert_eq!(status, 200);
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
