//! Model spend on a hosted Cupel (`ai/`, docs/deployment.md "Model spend and the AI switch"): one decision point
//! for every model call, metering by organisation, a monthly allowance, an organisation's own provider key and the
//! AI switch (capability C31).
//!
//! The provider is a local stub that records which key every request carried (its `Authorization` header) and
//! answers with usage the test chooses, so what these tests prove is read from what the provider actually saw.
//! Nothing here calls a real provider.
//!
//! Like `hosted.rs` the whole binary is a shared host (`EUNOMIA_SIGNUP_ORG=personal`, `SIGNUP=closed`): every
//! organisation comes from the operator API.

mod common;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use eunomia_backend::ai::{self, Purpose, Refusal};
use eunomia_backend::config::Settings;
use eunomia_backend::jobs::worker::{self, Registry};
use eunomia_backend::jobs::{self, NewJob, WorkerConfig};
use eunomia_backend::mail::MailBackend;
use eunomia_backend::models_user::{self, User};
use eunomia_backend::plan::HostedConfig;
use serde_json::{json, Value};

const TOKEN: &str = "operator-token-for-tests-0123456789abcdef";
const SERVER_KEY: &str = "sk-server-key";
const ORG_KEY: &str = "sk-organisation-key";
const USER_KEY: &str = "sk-personal-key";

fn host_env() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: set once, before any test reads them, to the same values for every test in this binary
        unsafe {
            std::env::set_var("EUNOMIA_SIGNUP_ORG", "personal");
            std::env::set_var("SIGNUP", "closed");
            std::env::set_var("EUNOMIA_ADMIN_EMAILS", "op@corp.test");
            std::env::set_var("RATE_LIMIT_AUTH_PER_MIN", "1000");
        }
    });
}

// ------------------------------------------------------------------ the stub provider

#[derive(Debug, Clone)]
struct Hit {
    path: String,
    auth: String,
    model: String,
    stream: bool,
    max_completion_tokens: Option<u64>,
    has_stream_options: bool,
}

#[derive(Clone)]
struct Stub {
    base: String,
    hits: Arc<Mutex<Vec<Hit>>>,
    /// Report `usage` in answers (else none, so the server has to count).
    report_usage: Arc<AtomicBool>,
    /// What chat answers report.
    usage_in: Arc<AtomicU64>,
    usage_out: Arc<AtomicU64>,
    /// Hold every answer this long.
    delay_ms: Arc<AtomicU64>,
}

impl Stub {
    fn hits(&self) -> Vec<Hit> {
        self.hits.lock().unwrap().clone()
    }

    fn count(&self, path: &str) -> usize {
        self.hits().iter().filter(|h| h.path == path).count()
    }

    fn total(&self) -> usize {
        self.hits.lock().unwrap().len()
    }

    /// The keys the provider saw on `path`, in order.
    fn keys(&self, path: &str) -> Vec<String> {
        self.hits().into_iter().filter(|h| h.path == path).map(|h| h.auth).collect()
    }

    fn clear(&self) {
        self.hits.lock().unwrap().clear();
    }
}

async fn stub() -> Stub {
    use axum::extract::State;
    use axum::http::{header, HeaderMap};
    use axum::response::{IntoResponse, Response};
    use axum::routing::{get, post};
    use axum::{Json, Router};

    async fn pause(s: &Stub) {
        let ms = s.delay_ms.load(Ordering::Relaxed);
        if ms > 0 {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
    }
    fn auth(h: &HeaderMap) -> String {
        h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or_default().trim_start_matches("Bearer ").to_string()
    }
    async fn embeddings(State(s): State<Stub>, headers: HeaderMap, Json(body): Json<Value>) -> Json<Value> {
        let n = body["input"].as_array().map_or(1, Vec::len);
        s.hits.lock().unwrap().push(Hit { path: "/embeddings".into(), auth: auth(&headers), model: body["model"].as_str().unwrap_or_default().into(), stream: false, max_completion_tokens: None, has_stream_options: false });
        pause(&s).await;
        let v = vec![0.01_f32; eunomia_backend::embeddings::service::dim()];
        let mut out = json!({ "data": (0..n).map(|i| json!({"index": i, "embedding": v})).collect::<Vec<_>>() });
        if s.report_usage.load(Ordering::Relaxed) {
            out["usage"] = json!({ "prompt_tokens": 7 * n, "total_tokens": 7 * n });
        }
        Json(out)
    }
    async fn chat(State(s): State<Stub>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
        let stream = body["stream"].as_bool().unwrap_or(false);
        s.hits.lock().unwrap().push(Hit {
            path: "/chat/completions".into(),
            auth: auth(&headers),
            model: body["model"].as_str().unwrap_or_default().into(),
            stream,
            max_completion_tokens: body["max_completion_tokens"].as_u64(),
            has_stream_options: body.get("stream_options").is_some(),
        });
        pause(&s).await;
        let usage = json!({ "prompt_tokens": s.usage_in.load(Ordering::Relaxed), "completion_tokens": s.usage_out.load(Ordering::Relaxed), "total_tokens": s.usage_in.load(Ordering::Relaxed) + s.usage_out.load(Ordering::Relaxed) });
        let report = s.report_usage.load(Ordering::Relaxed);
        if stream {
            let mut text = format!("data: {}\n\n", json!({"choices":[{"delta":{"content":"Hello there, this is the answer."}}]}));
            if report {
                text.push_str(&format!("data: {}\n\n", json!({"choices": [], "usage": usage})));
            }
            text.push_str("data: [DONE]\n\n");
            return ([(header::CONTENT_TYPE, "text/event-stream")], text).into_response();
        }
        // one answer that every JSON-mode caller can read
        let content = json!({ "belief": "A belief.", "superseded": [], "answer": "An answer.", "cited": [], "phrases": ["lead time"], "people": [], "organisations": [], "locations": [], "other": [], "relations": [] });
        let mut out = json!({ "choices": [{ "message": { "content": content.to_string() } }] });
        if report {
            out["usage"] = usage;
        }
        Json(out).into_response()
    }
    async fn models(State(s): State<Stub>, headers: HeaderMap) -> Json<Value> {
        s.hits.lock().unwrap().push(Hit { path: "/models".into(), auth: auth(&headers), model: String::new(), stream: false, max_completion_tokens: None, has_stream_options: false });
        Json(json!({ "data": [{ "id": "gpt-4o-mini" }, { "id": "text-embedding-3-small" }] }))
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let s = Stub {
        base: format!("http://{addr}/v1"),
        hits: Default::default(),
        report_usage: Arc::new(AtomicBool::new(true)),
        usage_in: Arc::new(AtomicU64::new(1_000)),
        usage_out: Arc::new(AtomicU64::new(500)),
        delay_ms: Default::default(),
    };
    let router = Router::new().route("/v1/embeddings", post(embeddings)).route("/v1/chat/completions", post(chat)).route("/v1/models", get(models)).with_state(s.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    s
}

// ------------------------------------------------------------------ the host

struct Host {
    app: common::TestApp,
    stub: Stub,
}

async fn host() -> Host {
    host_env();
    let stub = stub().await;
    let settings = Settings {
        mail: MailBackend::Capture,
        hosted: HostedConfig { operator_token: Some(TOKEN.into()), max_orgs: None, billing_url: None },
        openai_api_key: Some(SERVER_KEY.into()),
        openai_base_url: stub.base.clone(),
        openai_chat_model: "gpt-4o-mini".into(),
        ..common::test_settings()
    };
    Host { app: common::TestApp::with_settings(settings).await, stub }
}

async fn op(app: &common::TestApp, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    common::http(&app.router, method, path, body, Some(TOKEN), None).await.0
}

async fn call(app: &common::TestApp, cookie: &str, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    common::http(&app.router, method, path, body, None, Some(cookie)).await.0
}

fn code(v: &Value) -> &str {
    v["code"].as_str().unwrap_or_default()
}

#[derive(Clone)]
struct P {
    email: String,
    cookie: String,
}

async fn login(app: &common::TestApp, email: &str, cookie: Option<String>) -> P {
    let cookie = match cookie {
        Some(c) => c,
        None => {
            let ((status, body), c) = common::http(&app.router, "POST", "/api/auth/login", Some(json!({ "email": email, "password": common::PASSWORD })), None, None).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            c.unwrap()
        }
    };
    P { email: email.into(), cookie: cookie.split(';').next().unwrap().to_string() }
}

fn token_of(link: &str) -> String {
    link.split_once("/invite#").unwrap_or_else(|| panic!("not an invitation link: {link}")).1.to_string()
}

/// An organisation made through the operator API (with `allowance` US dollars a month, if any), claimed by its owner.
async fn owned_org(app: &common::TestApp, external_ref: &str, owner: &str, seats: i64, allowance: Option<f64>) -> (String, P) {
    let mut body = json!({ "name": format!("Org {external_ref}"), "owner_email": owner, "plan": "team", "seat_limit": seats, "external_ref": external_ref });
    if let Some(a) = allowance {
        body["ai_allowance_usd"] = json!(a);
    }
    let (status, made) = op(app, "POST", "/api/operator/v1/orgs", Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{made}");
    let ((status, body), cookie) = common::http(
        &app.router,
        "POST",
        "/api/auth/register",
        Some(json!({ "email": owner, "password": common::PASSWORD, "invitation": token_of(made["claim_link"].as_str().unwrap()) })),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    (made["organisation_id"].as_str().unwrap().to_string(), login(app, owner, cookie).await)
}

async fn join(app: &common::TestApp, owner: &P, email: &str) -> P {
    let (status, created) = call(app, &owner.cookie, "POST", "/api/org/invitations", Some(json!({ "email": email, "role": "member" }))).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let ((status, body), cookie) = common::http(
        &app.router,
        "POST",
        "/api/auth/register",
        Some(json!({ "email": email, "password": common::PASSWORD, "invitation": created["token"].as_str().unwrap() })),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    login(app, email, cookie).await
}

async fn user_of(app: &common::TestApp, p: &P) -> User {
    models_user::user_by_email(&app.state.control, &p.email).await.unwrap().unwrap()
}

async fn db_of(app: &common::TestApp, u: &User) -> eunomia_backend::pool::OrgDb {
    app.state.pool.for_org(&u.org).await.unwrap()
}

/// The server's key, as `settings` says (so the stub sees what the decision chose).
fn settings(app: &common::TestApp) -> &Settings {
    &app.state.settings
}

fn org_micros(summary: &Value) -> f64 {
    summary["ai"]["cost_usd"].as_f64().unwrap() * 1_000_000.0
}

async fn summary(app: &common::TestApp, id: &str) -> Value {
    let (status, body) = op(app, "GET", &format!("/api/operator/v1/orgs/{id}"), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// Writes what this process has counted so far, so the database (and the operator API) show it.
async fn settle(app: &common::TestApp) {
    app.state.pool.ai().flush().await.unwrap();
}

async fn usage_rows(app: &common::TestApp, org: &str) -> Vec<Value> {
    settle(app).await;
    let org = eunomia_backend::pool::OrgId::parse(org).unwrap().record();
    app.control()
        .test_raw()
        .query("SELECT purpose, model, source, calls, input_tokens, output_tokens, estimated_calls, cost_micros FROM ai_usage WHERE org = $o ORDER BY purpose, model, source")
        .bind(("o", org))
        .await
        .unwrap()
        .take(0)
        .unwrap()
}

fn row<'a>(rows: &'a [Value], purpose: &str, source: &str) -> &'a Value {
    rows.iter().find(|r| r["purpose"] == purpose && r["source"] == source).unwrap_or_else(|| panic!("no {purpose}/{source} row in {rows:?}"))
}

/// A model call through the decision point, as a call site makes it, returning the provider's JSON or the refusal.
async fn chat_call(app: &common::TestApp, u: &User, purpose: Purpose) -> Result<Value, Refusal> {
    let db = db_of(app, u).await;
    let grant = ai::authorize(&db, settings(app), &u.id, purpose).await.unwrap()?;
    let model = grant.chat_model().await;
    grant.complete(json!({ "model": model, "messages": [{ "role": "user", "content": "hello" }] }), Duration::from_secs(20)).await.map_err(|e| {
        assert_eq!(e.code.as_str(), "ai.allowance_used", "{}", e.message);
        Refusal::AllowanceUsed
    })
}

async fn embed_one(app: &common::TestApp, u: &User, text: &str) -> Result<Vec<Vec<f32>>, eunomia_backend::error::AppError> {
    let db = db_of(app, u).await;
    common::sys(eunomia_backend::embeddings::service::embed(&db, settings(app), &[text.to_string()], &u.id)).await
}

/// Adds server-key spend to an organisation directly (what earlier calls would have counted).
async fn spend(app: &common::TestApp, org: &str, micros: i64) {
    settle(app).await;
    let id = eunomia_backend::pool::OrgId::parse(org).unwrap();
    let month = app.state.pool.ai().month();
    app.control()
        .test_raw()
        .query("UPSERT $id SET org = $o, day = $d, month = $m, purpose = 'chat', model = 'seed', source = 'server', calls = 1, input_tokens = 0, output_tokens = 0, estimated_calls = 0, cost_micros = $c")
        .bind(("id", surrealdb::types::RecordId::new("ai_usage", format!("seed_{org}_{month}"))))
        .bind(("o", id.record()))
        .bind(("d", format!("{month}-01")))
        .bind(("m", month))
        .bind(("c", micros))
        .await
        .unwrap()
        .check()
        .unwrap();
    app.state.pool.ai().forget_config(&id);
}

// ------------------------------------------------------------------ one decision point

#[tokio::test]
async fn a_server_key_call_is_metered_by_organisation_purpose_and_model_at_the_price_table() {
    let h = host().await;
    let (a_id, a_owner) = owned_org(&h.app, "m1_a", "m1a@acme.test", 3, None).await;
    let a = user_of(&h.app, &a_owner).await;
    chat_call(&h.app, &a, Purpose::Extraction).await.unwrap();
    chat_call(&h.app, &a, Purpose::Extraction).await.unwrap();
    embed_one(&h.app, &a, "an embedding text").await.unwrap();
    assert_eq!(h.stub.keys("/chat/completions"), vec![SERVER_KEY, SERVER_KEY]);
    let rows = usage_rows(&h.app, &a_id).await;
    let ex = row(&rows, "extraction", "server");
    // 1,000 input and 500 output tokens of gpt-4o-mini, twice: 2 x (150 + 300) micro-dollars
    assert_eq!((ex["calls"].as_i64(), ex["input_tokens"].as_i64(), ex["output_tokens"].as_i64(), ex["estimated_calls"].as_i64(), ex["cost_micros"].as_i64()), (Some(2), Some(2_000), Some(1_000), Some(0), Some(900)), "{ex}");
    assert_eq!(ex["model"], "gpt-4o-mini");
    let em = row(&rows, "embedding", "server");
    // 7 reported tokens of text-embedding-3-small (0.02 per million) is 0.14 micro-dollars, rounded up
    assert_eq!((em["calls"].as_i64(), em["input_tokens"].as_i64(), em["output_tokens"].as_i64(), em["cost_micros"].as_i64()), (Some(1), Some(7), Some(0), Some(1)), "{em}");
    assert_eq!(em["model"], "text-embedding-3-small");
    let s = summary(&h.app, &a_id).await;
    assert_eq!(org_micros(&s).round() as i64, 901, "{s}");
    assert_eq!(s["ai"]["usage"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn where_the_provider_reports_no_usage_tokens_are_counted_conservatively_and_marked() {
    let h = host().await;
    let (a_id, a_owner) = owned_org(&h.app, "m2_a", "m2a@acme.test", 3, None).await;
    let a = user_of(&h.app, &a_owner).await;
    h.stub.report_usage.store(false, Ordering::Relaxed);
    chat_call(&h.app, &a, Purpose::Consolidation).await.unwrap();
    embed_one(&h.app, &a, "some text that is thirty bytes").await.unwrap();
    let rows = usage_rows(&h.app, &a_id).await;
    let c = row(&rows, "consolidation", "server");
    assert_eq!(c["estimated_calls"], 1, "{c}");
    // input: one token per three bytes of the request; output: one per three bytes of the answer. Never zero.
    assert!(c["input_tokens"].as_i64().unwrap() > 10 && c["output_tokens"].as_i64().unwrap() > 10, "{c}");
    assert!(c["cost_micros"].as_i64().unwrap() > 0, "{c}");
    let e = row(&rows, "embedding", "server");
    assert_eq!(e["estimated_calls"], 1, "{e}");
    // 30 bytes of text plus 8 of framing: 38 / 3, rounded up
    assert_eq!(e["input_tokens"], 13, "{e}");
    assert!(e["cost_micros"].as_i64().unwrap() >= 1);
}

#[tokio::test]
async fn a_streamed_chat_is_counted_from_the_providers_usage_chunk_or_estimated() {
    let h = host().await;
    let (a_id, a_owner) = owned_org(&h.app, "m3_a", "m3a@acme.test", 3, None).await;
    let a = user_of(&h.app, &a_owner).await;
    let db = db_of(&h.app, &a).await;
    for (report, total_calls) in [(true, 1), (false, 2)] {
        h.stub.report_usage.store(report, Ordering::Relaxed);
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        let thread = common::sys(eunomia_backend::chat::service::create_thread(&db, &a.id, None)).await.unwrap();
        let id = eunomia_backend::rid::parse(&thread.id).unwrap();
        common::sys(eunomia_backend::chat::service::send_stream(h.app.state.org(&a.org).await.unwrap(), a.clone(), id, "hello".into(), tx)).await;
        let mut last = Value::Null;
        while let Some(e) = rx.recv().await {
            last = serde_json::to_value(e).unwrap();
        }
        assert_eq!(last["type"], "done", "{last}");
        assert!(h.stub.hits().iter().all(|x| x.stream && x.model == "gpt-4o-mini"), "{:?}", h.stub.hits());
        let rows = usage_rows(&h.app, &a_id).await;
        let c = row(&rows, "chat", "server");
        assert_eq!(c["calls"], total_calls, "{c}");
        if report {
            assert_eq!((c["input_tokens"].as_i64(), c["output_tokens"].as_i64(), c["estimated_calls"].as_i64()), (Some(1_000), Some(500), Some(0)), "{c}");
        } else {
            assert_eq!(c["estimated_calls"], 1, "{c}");
        }
    }
}

#[tokio::test]
async fn key_precedence_is_the_persons_own_then_the_organisations_then_the_servers() {
    let h = host().await;
    let (a_id, a_owner) = owned_org(&h.app, "kp_a", "kpa@acme.test", 4, Some(5.0)).await;
    let (b_id, b_owner) = owned_org(&h.app, "kp_b", "kpb@acme.test", 4, Some(5.0)).await;
    let a2 = join(&h.app, &a_owner, "kpa2@acme.test").await;
    let (a, a2u, b) = (user_of(&h.app, &a_owner).await, user_of(&h.app, &a2).await, user_of(&h.app, &b_owner).await);

    // nothing set: the server's key, counted and costed
    chat_call(&h.app, &a, Purpose::Chat).await.unwrap();
    assert_eq!(h.stub.keys("/chat/completions"), vec![SERVER_KEY]);

    // the organisation sets its own key (owner, browser session)
    let (status, view) = call(&h.app, &a_owner.cookie, "PUT", "/api/org/ai/provider", Some(json!({ "api_key": ORG_KEY }))).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["serving"], "organisation_key");
    h.stub.clear();
    chat_call(&h.app, &a, Purpose::Chat).await.unwrap();
    chat_call(&h.app, &a2u, Purpose::Chat).await.unwrap();
    assert_eq!(h.stub.keys("/chat/completions"), vec![ORG_KEY, ORG_KEY]);

    // a person's own key comes first, ahead of the organisation's
    let (status, _) = call(&h.app, &a2.cookie, "PATCH", "/api/settings", Some(json!({ "openai_api_key": USER_KEY }))).await;
    assert_eq!(status, StatusCode::OK);
    h.stub.clear();
    chat_call(&h.app, &a2u, Purpose::Chat).await.unwrap();
    chat_call(&h.app, &a, Purpose::Chat).await.unwrap();
    assert_eq!(h.stub.keys("/chat/completions"), vec![USER_KEY, ORG_KEY]);

    // another organisation is untouched: still the server's key
    h.stub.clear();
    chat_call(&h.app, &b, Purpose::Chat).await.unwrap();
    assert_eq!(h.stub.keys("/chat/completions"), vec![SERVER_KEY]);

    // counted by whose key, and only the server's key costs anything
    let rows = usage_rows(&h.app, &a_id).await;
    let server = row(&rows, "chat", "server");
    assert_eq!((server["calls"].as_i64(), server["cost_micros"].as_i64()), (Some(1), Some(450)), "{server}");
    let org = row(&rows, "chat", "org");
    assert_eq!((org["calls"].as_i64(), org["input_tokens"].as_i64(), org["cost_micros"].as_i64()), (Some(3), Some(3_000), Some(0)), "{org}");
    let user = row(&rows, "chat", "user");
    assert_eq!((user["calls"].as_i64(), user["cost_micros"].as_i64()), (Some(1), Some(0)), "{user}");
    // and the allowance of A saw only the server's call; B's is its own
    assert_eq!(org_micros(&summary(&h.app, &a_id).await).round() as i64, 450);
    assert_eq!(org_micros(&summary(&h.app, &b_id).await).round() as i64, 450);
}

// ------------------------------------------------------------------ the allowance

#[tokio::test]
async fn an_allowance_that_is_used_up_stops_calls_on_the_servers_key_with_one_clear_error() {
    let h = host().await;
    let (a_id, a_owner) = owned_org(&h.app, "al_a", "ala@acme.test", 3, Some(0.01)).await;
    let (b_id, b_owner) = owned_org(&h.app, "al_b", "alb@acme.test", 3, Some(0.01)).await;
    let (a, b) = (user_of(&h.app, &a_owner).await, user_of(&h.app, &b_owner).await);
    let (a_db, _b_db) = (db_of(&h.app, &a).await, db_of(&h.app, &b).await);
    // a cent a month; A has used it all
    spend(&h.app, &a_id, 10_000).await;

    // the decision point: refused, and nothing reached the provider
    assert_eq!(ai::authorize(&a_db, settings(&h.app), &a.id, Purpose::Chat).await.unwrap().err(), Some(Refusal::AllowanceUsed));
    let e = embed_one(&h.app, &a, "no vector for this").await.unwrap_err();
    assert_eq!((e.code.as_str(), e.status.as_u16()), ("ai.allowance_used", 402), "{}", e.message);
    assert!(e.message.contains("allowance") && e.message.contains("owner") && e.message.contains("key"), "{}", e.message);
    assert_eq!(h.stub.total(), 0);

    // in-app chat answers the same documented error, before anything is stored or streamed
    let (status, thread) = call(&h.app, &a_owner.cookie, "POST", "/api/chat/threads", Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{thread}");
    let (status, body) = call(&h.app, &a_owner.cookie, "POST", &format!("/api/chat/threads/{}", thread["id"].as_str().unwrap()), Some(json!({ "message": "hi" }))).await;
    assert_eq!((status, code(&body)), (StatusCode::PAYMENT_REQUIRED, "ai.allowance_used"), "{body}");
    assert_eq!(h.stub.total(), 0);

    // what works without a model keeps working: saving, reading, keyword search
    let token = {
        let scopes: Vec<String> = eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect();
        models_user::create_api_token_with(&h.app.state.control, &a.id, "t", &scopes, None, None).await.unwrap().token
    };
    let w = common::http(&h.app.router, "POST", "/api/tools/memory_write", Some(json!({ "subject_name": "Ada", "subject_kind": "person", "text": "Ada likes tea" })), Some(&token), None).await.0;
    assert_eq!(w.0, StatusCode::OK, "{:?}", w.1);
    let r = common::http(&h.app.router, "POST", "/api/tools/recall", Some(json!({ "query": "Ada tea" })), Some(&token), None).await.0;
    assert_eq!(r.0, StatusCode::OK, "{:?}", r.1);
    assert!(r.1["results"].as_array().is_some_and(|x| !x.is_empty()), "{:?}", r.1);
    assert_eq!(h.stub.total(), 0, "recall fell back to keywords instead of asking a model");

    // members see a state and a percentage, no amount; the operator sees the cost
    let (_, plan) = (StatusCode::OK, call(&h.app, &a_owner.cookie, "GET", "/api/org/plan", None).await.1);
    assert_eq!((plan["ai"]["usage"]["state"].as_str(), plan["ai"]["usage"]["percent"].as_i64()), (Some("used_up"), Some(100)), "{plan}");
    assert!(!plan.to_string().contains("10000") && !plan.to_string().contains("0.01"), "no amounts for members: {plan}");
    assert_eq!(summary(&h.app, &a_id).await["ai"]["allowance_used_up"], true);

    // another organisation is not affected by A's: its calls still go out
    chat_call(&h.app, &b, Purpose::Chat).await.unwrap();
    assert_eq!(h.stub.count("/chat/completions"), 1);
    assert_eq!(summary(&h.app, &b_id).await["ai"]["allowance_used_up"], false);
}

#[tokio::test]
async fn the_allowance_is_a_state_and_a_percentage_for_members_ok_nearly_used_used_up() {
    let h = host().await;
    let (id, owner) = owned_org(&h.app, "st_a", "sta@acme.test", 3, Some(1.0)).await;
    let state = |p: Value| (p["ai"]["usage"]["state"].as_str().map(str::to_string), p["ai"]["usage"]["percent"].as_i64());
    let plan = || async { call(&h.app, &owner.cookie, "GET", "/api/org/plan", None).await.1 };
    assert_eq!(state(plan().await), (Some("ok".into()), Some(0)));
    spend(&h.app, &id, 500_000).await;
    assert_eq!(state(plan().await), (Some("ok".into()), Some(50)));
    spend(&h.app, &id, 850_000).await;
    assert_eq!(state(plan().await), (Some("nearly_used".into()), Some(85)));
    spend(&h.app, &id, 1_000_000).await;
    assert_eq!(state(plan().await), (Some("used_up".into()), Some(100)));
    // no allowance: nothing to report
    let (_, other) = owned_org(&h.app, "st_b", "stb@acme.test", 3, None).await;
    let p = call(&h.app, &other.cookie, "GET", "/api/org/plan", None).await.1;
    assert_eq!(state(p.clone()), (None, None));
    assert_eq!((p["ai"]["enabled"].as_bool(), p["ai"]["own_key"].as_bool()), (Some(true), Some(false)), "{p}");
}

#[tokio::test]
async fn the_cap_holds_under_concurrency_and_the_overshoot_is_bounded() {
    use eunomia_backend::ai::prices::{cost_micros, Table};
    let h = host().await;
    // $0.01 a month: room for four worst-case reservations of a gpt-4o-mini call (4,096 output tokens, about 2,460
    // micro-dollars each) and not for a fifth
    let (a_id, a_owner) = owned_org(&h.app, "cc_a", "cca@acme.test", 3, Some(0.01)).await;
    let a = user_of(&h.app, &a_owner).await;
    let allowance = 10_000_i64;
    let min_reservation = cost_micros(&Table::default().price("gpt-4o-mini").price, 0, ai::MAX_OUTPUT_TOKENS);
    let most_admitted = (allowance / min_reservation) as usize;
    assert_eq!(most_admitted, 4);

    // all sixteen are in flight together: the provider holds every answer
    h.stub.delay_ms.store(400, Ordering::Relaxed);
    h.stub.usage_in.store(50, Ordering::Relaxed);
    h.stub.usage_out.store(50, Ordering::Relaxed);
    let run = |n: usize| {
        let (app, a) = (&h.app, &a);
        async move { futures::future::join_all((0..n).map(|_| chat_call(app, a, Purpose::Chat))).await }
    };
    let results = run(16).await;
    let admitted = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(admitted, most_admitted, "only what the allowance can cover in the worst case is admitted");
    assert_eq!(h.stub.total(), admitted, "the refused calls never reached the provider");
    // a provider that honours the output cap bills less than the reservation: nothing is overshot
    let rows = usage_rows(&h.app, &a_id).await;
    let spent = row(&rows, "chat", "server")["cost_micros"].as_i64().unwrap();
    assert!(spent <= allowance, "spent {spent} of {allowance}");

    // a provider that ignores the cap and bills far more than was reserved: the overshoot is what the admitted
    // calls billed beyond their reservations, and no more than the admitted calls' own cost
    spend(&h.app, &a_id, 0).await; // keep the book honest after the seed row
    let (b_id, b_owner) = owned_org(&h.app, "cc_b", "ccb@acme.test", 3, Some(0.01)).await;
    let b = user_of(&h.app, &b_owner).await;
    h.stub.usage_out.store(8_000, Ordering::Relaxed); // 4,800 micro-dollars a call, above the 2,460 reserved
    let results = futures::future::join_all((0..16).map(|_| chat_call(&h.app, &b, Purpose::Chat))).await;
    let admitted = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(admitted, most_admitted);
    let rows = usage_rows(&h.app, &b_id).await;
    let spent = row(&rows, "chat", "server")["cost_micros"].as_i64().unwrap();
    let per_call = cost_micros(&Table::default().price("gpt-4o-mini").price, 50, 8_000);
    assert_eq!(spent, per_call * admitted as i64);
    assert!(spent > allowance, "the test provider overshot on purpose");
    assert!(spent - allowance <= most_admitted as i64 * per_call, "overshoot {} is bounded by the admitted calls' own cost", spent - allowance);
    // and once it is over, nothing more is admitted
    assert!(matches!(chat_call(&h.app, &b, Purpose::Chat).await, Err(Refusal::AllowanceUsed)));
}

#[tokio::test]
async fn a_call_on_the_servers_key_asks_the_provider_for_a_capped_answer_and_for_usage_in_a_stream() {
    // the pure rule, so it holds without api.openai.com: only our key, only with an allowance, only on OpenAI
    use eunomia_backend::ai::{caps_output, KeySource};
    assert!(caps_output(KeySource::Server, true, "https://api.openai.com/v1"));
    assert!(!caps_output(KeySource::Server, false, "https://api.openai.com/v1"), "no allowance, no cap: as before");
    assert!(!caps_output(KeySource::Org, true, "https://api.openai.com/v1"), "their key, their limits");
    assert!(!caps_output(KeySource::User, true, "https://api.openai.com/v1"));
    assert!(!caps_output(KeySource::Server, true, "http://llm.internal/v1"), "other servers may reject the field");
    // and what the stub saw on a self-hosted base URL: neither field
    let h = host().await;
    let (_, owner) = owned_org(&h.app, "cap_a", "capa@acme.test", 3, Some(5.0)).await;
    let u = user_of(&h.app, &owner).await;
    chat_call(&h.app, &u, Purpose::Chat).await.unwrap();
    let hit = h.stub.hits().pop().unwrap();
    assert_eq!((hit.max_completion_tokens, hit.has_stream_options), (None, false), "{hit:?}");
}

// ------------------------------------------------------------------ background work

fn worker_cfg(id: &str) -> WorkerConfig {
    WorkerConfig { id: id.into(), concurrency: 2, poll: Duration::from_millis(20), lease: Duration::from_secs(30), owner_cap: 100, grace: Duration::from_millis(200), retry_base: Duration::from_millis(10), tick: Duration::from_secs(30) }
}

async fn eventually(what: &str, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < Duration::from_secs(15), "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn job_row(app: &common::TestApp, key: &str) -> Value {
    let rows: Vec<Value> = app.control().test_raw().query("SELECT kind, status, attempts, last_error_code FROM job WHERE idempotency_key = $k").bind(("k", key.to_string())).await.unwrap().take(0).unwrap();
    rows.into_iter().next().unwrap_or(Value::Null)
}

#[tokio::test]
async fn model_work_pauses_without_losing_it_and_resumes_when_the_month_rolls_over_the_allowance_is_raised_or_a_key_is_added() {
    let h = host().await;
    h.app.state.pool.ai().set_hold(Duration::from_millis(150));
    let (a_id, a_owner) = owned_org(&h.app, "bg_a", "bga@acme.test", 3, Some(0.01)).await;
    let (b_id, b_owner) = owned_org(&h.app, "bg_b", "bgb@acme.test", 3, Some(0.01)).await;
    let (a, b) = (user_of(&h.app, &a_owner).await, user_of(&h.app, &b_owner).await);
    let (ran_a, ran_b) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
    // one handler per owner so the test sees whose work ran
    let (ra, rb) = (ran_a.clone(), ran_b.clone());
    let (a_owner_id, b_owner_id) = (a.id.clone(), b.id.clone());
    let registry = Registry::new().register("embed", move |_state, job: eunomia_backend::jobs::Job| {
        let (ra, rb, ao, bo) = (ra.clone(), rb.clone(), a_owner_id.clone(), b_owner_id.clone());
        async move {
            if job.owner == ao {
                ra.fetch_add(1, Ordering::SeqCst);
            } else if job.owner == bo {
                rb.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
    });
    // the allowance is used up for A, and B has room
    spend(&h.app, &a_id, 10_000).await;
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let w = tokio::spawn(worker::run(h.app.state.clone(), registry, worker_cfg("w-ai"), stopped));
    for (owner, key) in [(&a.id, "a-1"), (&a.id, "a-2"), (&b.id, "b-1")] {
        jobs::enqueue(&h.app.state.control, NewJob::new("embed", owner.clone(), key)).await.unwrap();
    }
    // B's work runs; A's waits, queued, with no attempt used and no failure recorded
    let rb2 = ran_b.clone();
    eventually("B's job to run", move || rb2.load(Ordering::SeqCst) == 1).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(ran_a.load(Ordering::SeqCst), 0, "A's model work is held");
    for key in ["a-1", "a-2"] {
        let j = job_row(&h.app, key).await;
        assert_eq!((j["status"].as_str(), j["attempts"].as_i64()), (Some("ready"), Some(0)), "{j}");
        assert!(j["last_error_code"].is_null(), "{j}");
    }

    // 1. the allowance is raised: A's jobs run
    assert_eq!(op(&h.app, "PATCH", &format!("/api/operator/v1/orgs/{a_id}"), Some(json!({ "ai_allowance_usd": 5.0 }))).await.0, StatusCode::OK);
    let ra2 = ran_a.clone();
    eventually("A's jobs after the allowance was raised", move || ra2.load(Ordering::SeqCst) == 2).await;

    // 2. the month rolls over: the used-up allowance is a fresh one
    assert_eq!(op(&h.app, "PATCH", &format!("/api/operator/v1/orgs/{a_id}"), Some(json!({ "ai_allowance_usd": 0.01 }))).await.0, StatusCode::OK);
    spend(&h.app, &a_id, 10_000).await;
    jobs::enqueue(&h.app.state.control, NewJob::new("embed", a.id.clone(), "a-3")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(ran_a.load(Ordering::SeqCst), 2, "held again");
    let next_month = chrono::Utc::now() + chrono::Months::new(1);
    h.app.state.pool.ai().set_clock(Some(next_month));
    let ra3 = ran_a.clone();
    eventually("A's job after the month rolled over", move || ra3.load(Ordering::SeqCst) == 3).await;
    h.app.state.pool.ai().set_clock(None);

    // 3. the organisation adds a key of its own: its model work needs no allowance
    spend(&h.app, &a_id, 10_000).await;
    jobs::enqueue(&h.app.state.control, NewJob::new("embed", a.id.clone(), "a-4")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(ran_a.load(Ordering::SeqCst), 3, "held a third time");
    let (status, view) = call(&h.app, &a_owner.cookie, "PUT", "/api/org/ai/provider", Some(json!({ "api_key": ORG_KEY }))).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let ra4 = ran_a.clone();
    eventually("A's job after a key was added", move || ra4.load(Ordering::SeqCst) == 4).await;
    // B never noticed any of it
    assert_eq!(ran_b.load(Ordering::SeqCst), 1);
    let _ = b_id;
    stop.send(true).unwrap();
    let _ = w.await;
}

#[tokio::test]
async fn a_job_refused_part_way_is_handed_back_without_using_an_attempt() {
    let h = host().await;
    h.app.state.pool.ai().set_hold(Duration::from_millis(100));
    let (a_id, a_owner) = owned_org(&h.app, "pw_a", "pwa@acme.test", 3, Some(5.0)).await;
    let a = user_of(&h.app, &a_owner).await;
    let runs = Arc::new(AtomicU32::new(0));
    let r = runs.clone();
    // the handler is refused the way a model call is when the allowance runs out under it
    let registry = Registry::new().register("extract", move |_state, _job| {
        let r = r.clone();
        async move {
            if r.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(eunomia_backend::jobs::JobError::from(Refusal::AllowanceUsed.into_error()));
            }
            Ok(())
        }
    });
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let w = tokio::spawn(worker::run(h.app.state.clone(), registry, worker_cfg("w-pw"), stopped));
    jobs::enqueue(&h.app.state.control, NewJob::new("extract", a.id.clone(), "pw-1")).await.unwrap();
    let r2 = runs.clone();
    eventually("the first run", move || r2.load(Ordering::SeqCst) >= 1).await;
    // it comes back and runs again; it was never failed or dead-lettered
    let r3 = runs.clone();
    eventually("the second run", move || r3.load(Ordering::SeqCst) >= 2).await;
    let start = Instant::now();
    loop {
        let j = job_row(&h.app, "pw-1").await;
        if j["status"] == "done" {
            assert!(j["last_error_code"].is_null(), "{j}");
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "{j}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let _ = a_id;
    stop.send(true).unwrap();
    let _ = w.await;
}

#[test]
fn the_job_kinds_the_claim_holds_are_the_kinds_that_call_a_model() {
    let sql = include_str!("../src/store/jobs.rs");
    let literal = format!("[{}]", eunomia_backend::ai::MODEL_JOB_KINDS.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(", "));
    assert!(sql.matches(&literal).count() >= 2, "store/jobs.rs CLAIM must name exactly {literal}");
}

// ------------------------------------------------------------------ the AI switch

/// Every entry point that can reach a model, for the organisation of `p`. Returns what each answered.
async fn every_model_entry_point(h: &Host, p: &P) -> Vec<(&'static str, String)> {
    use eunomia_backend::entities::{consolidate, extract, supersede};
    let u = user_of(&h.app, p).await;
    let db = db_of(&h.app, &u).await;
    let mut out = Vec::new();
    out.push(("embed", embed_one(&h.app, &u, "a text nobody embedded").await.err().map(|e| e.code.as_str().to_string()).unwrap_or_default()));
    let record = extract::ExtractRecord {
        id: "notes:1".into(),
        title: "Note".into(),
        body_text: "Ada Lovelace works at Babbage Ltd in London. This note is long enough to be worth extracting facts from.".into(),
        occurred_at: None,
        vault: None,
        source: "notes".into(),
        payload: Value::Null,
        owner_name: None,
    };
    out.push(("extract", common::sys(extract::extract_entities(&db, settings(&h.app), &u.id, &record)).await.err().map(|e| e.code.as_str().to_string()).unwrap_or_default()));
    let token = {
        let scopes: Vec<String> = eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect();
        models_user::create_api_token_with(&h.app.state.control, &u.id, "t", &scopes, None, None).await.unwrap().token
    };
    let tool = |name: &'static str, args: Value| {
        let token = token.clone();
        async move { common::http(&h.app.router, "POST", &format!("/api/tools/{name}"), Some(args), Some(&token), None).await.0 }
    };
    let w = tool("memory_write", json!({ "subject_name": "Ada", "subject_kind": "person", "text": "Ada likes tea" })).await;
    let ada = w.1["entity"]["id"].as_str().unwrap_or_default().to_string();
    let subject = eunomia_backend::rid::parse(&ada).unwrap();
    out.push(("consolidate", common::sys(consolidate::consolidate_subject(&db, settings(&h.app), &u.id, &subject, None)).await.err().map(|e| e.code.as_str().to_string()).unwrap_or_default()));
    let memory = eunomia_backend::rid::parse(w.1["memory"]["id"].as_str().unwrap()).unwrap();
    out.push(("supersede", format!("{}", common::sys(supersede::check(&db, settings(&h.app), &u.id, &subject, &[memory])).await.len())));
    out.push(("reflect", tool("reflect", json!({ "query": "Ada tea" })).await.1["mode"].as_str().unwrap_or("model").to_string()));
    out.push(("ontology_suggest", tool("ontology_suggest", json!({})).await.1["mode"].as_str().unwrap_or("?").to_string()));
    out.push(("ontology_coverage", tool("ontology_coverage", json!({ "question": "lead time of oat milk" })).await.1["mode"].as_str().unwrap_or("?").to_string()));
    let (status, thread) = call(&h.app, &p.cookie, "POST", "/api/chat/threads", Some(json!({}))).await;
    assert_eq!(status, StatusCode::OK, "{thread}");
    let (status, body) = call(&h.app, &p.cookie, "POST", &format!("/api/chat/threads/{}", thread["id"].as_str().unwrap()), Some(json!({ "message": "hi" }))).await;
    out.push(("chat", format!("{} {}", status.as_u16(), code(&body))));
    let (status, body) = call(&h.app, &p.cookie, "GET", "/api/settings/openai-models", None).await;
    out.push(("models", format!("{} {}", status.as_u16(), code(&body))));
    out
}

#[tokio::test]
async fn with_ai_switched_off_no_call_reaches_any_provider_with_any_key_and_features_degrade() {
    let h = host().await;
    let (a_id, a_owner) = owned_org(&h.app, "off_a", "offa@acme.test", 3, None).await;
    let member = join(&h.app, &a_owner, "offa2@acme.test").await;
    // a member's own key: the strongest case, it is theirs and it is free to us
    let (status, _) = call(&h.app, &member.cookie, "PATCH", "/api/settings", Some(json!({ "openai_api_key": USER_KEY }))).await;
    assert_eq!(status, StatusCode::OK);
    // and the organisation has a key of its own too
    let (status, _) = call(&h.app, &a_owner.cookie, "PUT", "/api/org/ai/provider", Some(json!({ "api_key": ORG_KEY }))).await;
    assert_eq!(status, StatusCode::OK);

    // a control: with AI on, the same entry points do call the provider (with the person's own key for the member)
    let on = every_model_entry_point(&h, &member).await;
    assert!(h.stub.total() > 5, "a control: {on:?}");
    assert!(h.stub.hits().iter().all(|x| x.auth == USER_KEY), "{:?}", h.stub.hits());

    // a plain member cannot switch it
    let (status, body) = call(&h.app, &member.cookie, "PUT", "/api/org/ai/enabled", Some(json!({ "enabled": false }))).await;
    assert_eq!((status, code(&body)), (StatusCode::FORBIDDEN, "auth.forbidden"), "{body}");
    // the owner can, and it is on the ledger
    let (status, view) = call(&h.app, &a_owner.cookie, "PUT", "/api/org/ai/enabled", Some(json!({ "enabled": false }))).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!((view["enabled"].as_bool(), view["serving"].as_str()), (Some(false), Some("off")));
    let ledger: Vec<Value> = h.app.control().test_raw().query("SELECT action, outcome, detail FROM audit_event WHERE action = 'org.ai_switch'").await.unwrap().take(0).unwrap();
    assert!(ledger.iter().any(|r| r["detail"] == "enabled=false"), "{ledger:?}");

    h.stub.clear();
    let off = every_model_entry_point(&h, &member).await;
    assert_eq!(h.stub.total(), 0, "no request reached a provider, with the person's key, the organisation's or the server's: {:?} {off:?}", h.stub.hits());
    let by = |name: &str| off.iter().find(|(n, _)| *n == name).unwrap().1.clone();
    assert_eq!(by("embed"), "ai.disabled", "{off:?}");
    assert_eq!(by("extract"), "ai.disabled");
    assert_eq!(by("chat"), "403 ai.disabled");
    assert_eq!(by("models"), "403 ai.disabled");
    // features with a path without a model take it
    assert_eq!(by("reflect"), "recall_only");
    assert_eq!(by("ontology_suggest"), "profile_only");
    assert_eq!(by("ontology_coverage"), "words");
    assert_eq!(by("supersede"), "0");

    // switched back on: the provider is called again
    let (status, _) = call(&h.app, &a_owner.cookie, "PUT", "/api/org/ai/enabled", Some(json!({ "enabled": true }))).await;
    assert_eq!(status, StatusCode::OK);
    let u = user_of(&h.app, &member).await;
    chat_call(&h.app, &u, Purpose::Chat).await.unwrap();
    assert_eq!(h.stub.total(), 1);
    let _ = a_id;
}

#[tokio::test]
async fn background_model_work_waits_while_ai_is_off_and_runs_when_it_is_switched_on() {
    let h = host().await;
    h.app.state.pool.ai().set_hold(Duration::from_millis(100));
    let (_, owner) = owned_org(&h.app, "boff_a", "boffa@acme.test", 3, None).await;
    let a = user_of(&h.app, &owner).await;
    let (status, _) = call(&h.app, &owner.cookie, "PUT", "/api/org/ai/enabled", Some(json!({ "enabled": false }))).await;
    assert_eq!(status, StatusCode::OK);
    let (stop, stopped) = tokio::sync::watch::channel(false);
    // the real handlers: nothing may reach the provider while AI is off
    let w = tokio::spawn(worker::run(h.app.state.clone(), jobs::handlers::registry(), worker_cfg("w-off"), stopped));
    for (kind, key, payload) in [("embed", "boff-embed", json!({})), ("extract", "boff-extract", json!({ "record": "notes:1" })), ("consolidate", "boff-consolidate", json!({ "subject": "person:x" })), ("supersede", "boff-supersede", json!({}))] {
        jobs::enqueue(&h.app.state.control, NewJob::new(match kind { "embed" => "embed", "extract" => "extract", "consolidate" => "consolidate", _ => "supersede" }, a.id.clone(), key).in_org(a.org).payload(payload)).await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(h.stub.total(), 0);
    for key in ["boff-embed", "boff-extract", "boff-consolidate", "boff-supersede"] {
        let j = job_row(&h.app, key).await;
        assert_eq!((j["status"].as_str(), j["attempts"].as_i64()), (Some("ready"), Some(0)), "{key}: {j}");
    }
    // switched on: the embed job runs for real (a memory with no vector gets one from the provider)
    let token = {
        let scopes: Vec<String> = eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect();
        models_user::create_api_token_with(&h.app.state.control, &a.id, "t", &scopes, None, None).await.unwrap().token
    };
    common::http(&h.app.router, "POST", "/api/tools/memory_write", Some(json!({ "subject_name": "Ada", "subject_kind": "person", "text": "Ada likes tea" })), Some(&token), None).await;
    let (status, _) = call(&h.app, &owner.cookie, "PUT", "/api/org/ai/enabled", Some(json!({ "enabled": true }))).await;
    assert_eq!(status, StatusCode::OK);
    let stub = h.stub.clone();
    eventually("the provider to be called again", move || stub.count("/embeddings") > 0).await;
    stop.send(true).unwrap();
    let _ = w.await;
}

// ------------------------------------------------------------------ the organisation's key

#[tokio::test]
async fn the_organisations_key_is_stored_encrypted_validated_never_returned_and_removable() {
    let h = host().await;
    let (a_id, owner) = owned_org(&h.app, "key_a", "keya@acme.test", 3, None).await;
    let member = join(&h.app, &owner, "keya2@acme.test").await;
    let org = eunomia_backend::pool::OrgId::parse(&a_id).unwrap();

    // a member can read nothing of it and change nothing
    for (m, path, body) in [("GET", "/api/org/ai", None), ("PUT", "/api/org/ai/provider", Some(json!({ "api_key": "x" }))), ("DELETE", "/api/org/ai/provider", None)] {
        let (status, body) = call(&h.app, &member.cookie, m, path, body).await;
        assert_eq!((status, code(&body)), (StatusCode::FORBIDDEN, "auth.forbidden"), "{m} {path}: {body}");
    }
    // a token is not a browser session
    let token = {
        let u = user_of(&h.app, &owner).await;
        let scopes: Vec<String> = eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect();
        models_user::create_api_token_with(&h.app.state.control, &u.id, "t", &scopes, None, None).await.unwrap().token
    };
    let ((status, _), _) = common::http(&h.app.router, "PUT", "/api/org/ai/provider", Some(json!({ "api_key": ORG_KEY })), Some(&token), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // addresses a person could not use are refused, as they are for a person's own setting
    for bad in ["http://169.254.169.254/latest", "http://surrealdb:8000/v1", "ftp://models.example.test/v1", "http://user:pass@models.example.test/v1"] {
        let (status, body) = call(&h.app, &owner.cookie, "PUT", "/api/org/ai/provider", Some(json!({ "base_url": bad, "api_key": ORG_KEY }))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }
    // a key is needed for api.openai.com; a model that its vector index cannot hold is refused
    eunomia_backend::llm_net::TEST_HOSTS.lock().unwrap().push(("api.openai.com".into(), vec!["93.184.216.34".parse().unwrap()]));
    let (status, body) = call(&h.app, &owner.cookie, "PUT", "/api/org/ai/provider", Some(json!({ "base_url": "https://api.openai.com/v1" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "no key: {body}");
    let (status, body) = call(&h.app, &owner.cookie, "PUT", "/api/org/ai/provider", Some(json!({ "base_url": "https://api.openai.com/v1", "api_key": ORG_KEY, "embedding_model": "text-embedding-3-large" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["detail"].as_str().unwrap_or_default().contains("1536"), "{body}");

    // saved: the answer says a key is set and never says which
    let (status, view) = call(&h.app, &owner.cookie, "PUT", "/api/org/ai/provider", Some(json!({ "api_key": ORG_KEY, "chat_model": "gpt-4o-mini", "embedding_model": "text-embedding-3-small" }))).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!((view["provider"]["key_set"].as_bool(), view["provider"]["set"].as_bool()), (Some(true), Some(true)));
    assert!(!view.to_string().contains(ORG_KEY));
    let (_, got) = call(&h.app, &owner.cookie, "GET", "/api/org/ai", None).await;
    assert!(!got.to_string().contains(ORG_KEY) && !got.to_string().contains("api_key_enc"), "{got}");
    // not in the plan status the members read, the operator's views, or the ledger
    let (_, plan) = call(&h.app, &member.cookie, "GET", "/api/org/plan", None).await;
    assert_eq!(plan["ai"]["own_key"], true);
    for body in [plan.to_string(), summary(&h.app, &a_id).await.to_string(), op(&h.app, "GET", "/api/operator/v1/orgs", None).await.1.to_string(), op(&h.app, "GET", "/api/operator/v1/capacity", None).await.1.to_string()] {
        assert!(!body.contains(ORG_KEY), "{body}");
    }
    assert_eq!(summary(&h.app, &a_id).await["ai"]["own_key"], true);
    let ledger: Vec<Value> = h.app.control().test_raw().query("SELECT detail FROM audit_event WHERE action = 'org.ai_provider'").await.unwrap().take(0).unwrap();
    assert!(!ledger.is_empty() && !ledger.iter().any(|r| r.to_string().contains(ORG_KEY)), "{ledger:?}");
    // stored encrypted: the row holds ciphertext, not the key
    let rows: Vec<Value> = h.app.control().test_raw().query("SELECT api_key_enc FROM org_ai WHERE org = $o").bind(("o", org.record())).await.unwrap().take(0).unwrap();
    let stored = rows[0]["api_key_enc"].as_str().unwrap();
    assert!(!stored.is_empty() && !stored.contains(ORG_KEY), "{stored}");

    // it is used, and removing it returns the organisation to the server's key
    let u = user_of(&h.app, &owner).await;
    chat_call(&h.app, &u, Purpose::Chat).await.unwrap();
    assert_eq!(h.stub.keys("/chat/completions"), vec![ORG_KEY]);
    let (status, view) = call(&h.app, &owner.cookie, "DELETE", "/api/org/ai/provider", None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!((view["provider"]["key_set"].as_bool(), view["serving"].as_str()), (Some(false), Some("server_key")));
    chat_call(&h.app, &u, Purpose::Chat).await.unwrap();
    assert_eq!(h.stub.keys("/chat/completions"), vec![ORG_KEY, SERVER_KEY]);
}

#[tokio::test]
async fn changing_the_organisations_embedding_model_makes_vectors_stale_and_the_backlog_re_embeds_them() {
    let h = host().await;
    let (_, owner) = owned_org(&h.app, "emb_a", "emba@acme.test", 3, None).await;
    let u = user_of(&h.app, &owner).await;
    let db = db_of(&h.app, &u).await;
    let spec = |app: &common::TestApp| {
        let (db, id) = (db.clone(), u.id.clone());
        let settings = settings(app).clone();
        async move { common::sys(eunomia_backend::embeddings::service::spec_for(&db, &settings, &id)).await.unwrap().tag() }
    };
    let before = spec(&h.app).await;
    let (status, view) = call(&h.app, &owner.cookie, "PUT", "/api/org/ai/provider", Some(json!({ "api_key": ORG_KEY, "base_url": h.stub.base.clone(), "embedding_model": "nomic-embed-1536" }))).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let after = spec(&h.app).await;
    // the same behaviour as a person's own setting: a new spec, so a query meets only vectors made with it, and
    // the backlog job re-embeds the rest
    assert_ne!(before, after);
    assert!(after.ends_with("|nomic-embed-1536|1536"), "{after}");
}

// ------------------------------------------------------------------ the price table

#[tokio::test]
async fn an_unknown_model_is_counted_at_the_high_fallback_until_the_operator_prices_it() {
    let h = host().await;
    let (a_id, owner) = owned_org(&h.app, "pr_a", "pra@acme.test", 3, None).await;
    let u = user_of(&h.app, &owner).await;
    // what the operator sees to start with
    let (status, table) = op(&h.app, "GET", "/api/operator/v1/ai/prices", None).await;
    assert_eq!(status, StatusCode::OK, "{table}");
    assert_eq!(table["prices"]["gpt-4o-mini"]["input_per_million_usd"], 0.15);
    assert_eq!(table["prices"]["gpt-4o-mini"]["source"], "default");
    assert!(table["fallback"]["input_per_million_usd"].as_f64().unwrap() > 2.5);

    // a person picks a model nobody priced: counted at the fallback, never free
    let (status, _) = call(&h.app, &owner.cookie, "PATCH", "/api/settings", Some(json!({ "chat_model": "gpt-6-luna" }))).await;
    assert_eq!(status, StatusCode::OK);
    chat_call(&h.app, &u, Purpose::Chat).await.unwrap();
    // 1,000 x 30 + 500 x 60 micro-dollars
    let rows = usage_rows(&h.app, &a_id).await;
    let r = rows.iter().find(|r| r["model"] == "gpt-6-luna").unwrap();
    assert_eq!(r["cost_micros"], 60_000, "{r}");

    // priced without a deploy: later calls use it, earlier ones keep what they were counted at
    let (status, table) = op(&h.app, "PUT", "/api/operator/v1/ai/prices", Some(json!({ "prices": { "gpt-6-luna": { "input_per_million_usd": 1.0, "output_per_million_usd": 2.0 } } }))).await;
    assert_eq!(status, StatusCode::OK, "{table}");
    assert_eq!(table["prices"]["gpt-6-luna"]["source"], "operator");
    chat_call(&h.app, &u, Purpose::Chat).await.unwrap();
    let rows = usage_rows(&h.app, &a_id).await;
    let r = rows.iter().find(|r| r["model"] == "gpt-6-luna").unwrap();
    assert_eq!(r["cost_micros"], 60_000 + 2_000, "{r}");
    // nonsense is refused
    for bad in [json!({ "prices": { "m": { "input_per_million_usd": -1.0, "output_per_million_usd": 1.0 } } }), json!({ "prices": {} }), json!({ "prices": { " m": null } })] {
        let (status, body) = op(&h.app, "PUT", "/api/operator/v1/ai/prices", Some(bad)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
    // removing the row puts the fallback back
    let (status, _) = op(&h.app, "PUT", "/api/operator/v1/ai/prices", Some(json!({ "prices": { "gpt-6-luna": null } }))).await;
    assert_eq!(status, StatusCode::OK);
    let (_, table) = op(&h.app, "GET", "/api/operator/v1/ai/prices", None).await;
    assert!(table["prices"].get("gpt-6-luna").is_none());
}

// ------------------------------------------------------------------ recording

#[tokio::test]
async fn usage_is_written_in_batches_and_a_crash_loses_at_most_one_interval() {
    let h = host().await;
    let (a_id, owner) = owned_org(&h.app, "rec_a", "reca@acme.test", 3, None).await;
    let u = user_of(&h.app, &owner).await;
    let org = eunomia_backend::pool::OrgId::parse(&a_id).unwrap().record();
    let written = || async {
        let rows: Vec<Value> = h.app.control().test_raw().query("SELECT calls FROM ai_usage WHERE org = $o").bind(("o", org.clone())).await.unwrap().take(0).unwrap();
        rows.iter().map(|r| r["calls"].as_i64().unwrap()).sum::<i64>()
    };
    // a long interval: twenty calls are counted in memory, and not one of them has cost a database round trip
    chat_call(&h.app, &u, Purpose::Chat).await.unwrap(); // starts the background writer
    h.app.state.pool.ai().set_flush_interval(Duration::from_secs(3600));
    tokio::time::sleep(Duration::from_millis(500)).await; // it wakes once, writes that call, and goes back to sleep
    assert_eq!(written().await, 1);
    for _ in 0..20 {
        chat_call(&h.app, &u, Purpose::Chat).await.unwrap();
    }
    assert_eq!(written().await, 1, "nothing is written until the interval: this is what a crash would lose");
    assert!(h.app.state.pool.ai().unwritten() >= 1);
    // the interval passes: one counter row, twenty calls
    h.app.state.pool.ai().set_flush_interval(Duration::from_millis(100));
    let start = Instant::now();
    while written().await != 21 {
        assert!(start.elapsed() < Duration::from_secs(10), "the background write never came");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let rows: Vec<Value> = h.app.control().test_raw().query("SELECT id FROM ai_usage WHERE org = $o").bind(("o", org.clone())).await.unwrap().take(0).unwrap();
    assert_eq!(rows.len(), 1, "counters, not a row per call");
}

// ------------------------------------------------------------------ the operator's and the members' views, and separation

#[tokio::test]
async fn the_operator_sees_each_organisation_and_the_total_and_can_set_the_allowance_on_create_and_update() {
    let h = host().await;
    let (a_id, a_owner) = owned_org(&h.app, "op_a", "opa@acme.test", 3, Some(2.0)).await;
    let (b_id, b_owner) = owned_org(&h.app, "op_b", "opb@acme.test", 3, None).await;
    let (a, b) = (user_of(&h.app, &a_owner).await, user_of(&h.app, &b_owner).await);
    chat_call(&h.app, &a, Purpose::Chat).await.unwrap();
    chat_call(&h.app, &b, Purpose::Chat).await.unwrap();
    let sa = summary(&h.app, &a_id).await;
    assert_eq!((sa["ai"]["allowance_usd"].as_f64(), sa["ai"]["ai_enabled"].as_bool(), sa["ai"]["own_key"].as_bool()), (Some(2.0), Some(true), Some(false)), "{sa}");
    assert_eq!(sa["ai"]["used_percent"].as_i64(), Some(0));
    assert_eq!(sa["ai"]["cost_usd"].as_f64(), Some(0.00045));
    let sb = summary(&h.app, &b_id).await;
    assert!(sb["ai"]["allowance_usd"].is_null() && sb["ai"]["used_percent"].is_null(), "{sb}");
    // the list carries the same, with no per-purpose lines
    let (_, list) = op(&h.app, "GET", "/api/operator/v1/orgs", None).await;
    let la = list["organisations"].as_array().unwrap().iter().find(|o| o["id"] == a_id.as_str()).unwrap();
    assert_eq!(la["ai"]["cost_usd"], sa["ai"]["cost_usd"]);
    assert!(la["ai"]["usage"].as_array().unwrap().is_empty());
    // the capacity route has the total across organisations and who has used theirs up
    let (_, cap) = op(&h.app, "GET", "/api/operator/v1/capacity", None).await;
    assert!(cap["ai"]["cost_usd"].as_f64().unwrap() >= 0.0009, "{cap}");
    assert_eq!(cap["ai"]["organisations_with_allowance"], 1, "{cap}");
    assert_eq!(cap["ai"]["organisations_used_up"], 0, "{cap}");
    spend(&h.app, &a_id, 2_000_000).await;
    let (_, cap) = op(&h.app, "GET", "/api/operator/v1/capacity", None).await;
    assert_eq!(cap["ai"]["organisations_used_up"], 1, "{cap}");

    // update: set, change, clear; nonsense refused
    let b_path = format!("/api/operator/v1/orgs/{b_id}");
    let patch = |v: Value| op(&h.app, "PATCH", &b_path, Some(v));
    let (status, body) = patch(json!({ "ai_allowance_usd": 35.5 })).await;
    assert_eq!((status, body["ai"]["allowance_usd"].as_f64()), (StatusCode::OK, Some(35.5)), "{body}");
    let (status, body) = patch(json!({ "ai_allowance_usd": null })).await;
    assert_eq!((status, body["ai"]["allowance_usd"].is_null()), (StatusCode::OK, true), "{body}");
    for bad in [json!({ "ai_allowance_usd": -1 }), json!({ "ai_allowance_usd": 2000000.0 }), json!({ "ai_allowance_usd": "lots" })] {
        let (status, body) = patch(bad).await;
        assert!(status.is_client_error(), "{body}");
    }
    // a billing change that sends no allowance leaves it alone
    patch(json!({ "ai_allowance_usd": 7.0 })).await;
    let (_, body) = patch(json!({ "seat_limit": 5 })).await;
    assert_eq!(body["ai"]["allowance_usd"].as_f64(), Some(7.0));
    // a retried create keeps the first allowance
    let (_, again) = op(&h.app, "POST", "/api/operator/v1/orgs", Some(json!({ "name": "Org op_a", "owner_email": "opa@acme.test", "plan": "team", "seat_limit": 3, "external_ref": "op_a", "ai_allowance_usd": 2.0 }))).await;
    assert_eq!(again["created"], false);
}

#[tokio::test]
async fn one_organisations_usage_key_switch_and_allowance_are_never_anothers() {
    let h = host().await;
    let (a_id, a_owner) = owned_org(&h.app, "sep_a", "sepa@acme.test", 3, Some(0.01)).await;
    let (b_id, b_owner) = owned_org(&h.app, "sep_b", "sepb@acme.test", 3, Some(0.01)).await;
    let (a, b) = (user_of(&h.app, &a_owner).await, user_of(&h.app, &b_owner).await);
    // A's owner sets a key, switches AI off, and uses up the allowance
    call(&h.app, &a_owner.cookie, "PUT", "/api/org/ai/provider", Some(json!({ "api_key": ORG_KEY }))).await;
    call(&h.app, &a_owner.cookie, "PUT", "/api/org/ai/enabled", Some(json!({ "enabled": false }))).await;
    spend(&h.app, &a_id, 10_000).await;

    // B sees none of it: its own settings, its own spend, and the server's key
    let (status, view) = call(&h.app, &b_owner.cookie, "GET", "/api/org/ai", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!((view["enabled"].as_bool(), view["provider"]["key_set"].as_bool(), view["usage"]["state"].as_str()), (Some(true), Some(false), Some("ok")), "{view}");
    let (_, plan) = call(&h.app, &b_owner.cookie, "GET", "/api/org/plan", None).await;
    assert_eq!((plan["ai"]["enabled"].as_bool(), plan["ai"]["own_key"].as_bool(), plan["ai"]["usage"]["percent"].as_i64()), (Some(true), Some(false), Some(0)), "{plan}");
    chat_call(&h.app, &b, Purpose::Chat).await.unwrap();
    assert_eq!(h.stub.keys("/chat/completions"), vec![SERVER_KEY]);
    // and A cannot reach B's through anything it sends: there is no organisation in the request to change
    for body in [json!({ "api_key": "sk-x", "org": b_id }), json!({ "api_key": "sk-x", "organisation_id": b_id })] {
        let (status, _) = call(&h.app, &a_owner.cookie, "PUT", "/api/org/ai/provider", Some(body)).await;
        assert!(status.is_client_error(), "{status}");
    }
    let (_, view) = call(&h.app, &b_owner.cookie, "GET", "/api/org/ai", None).await;
    assert_eq!(view["provider"]["key_set"], false);
    // the usage counters are per organisation
    let a_rows = usage_rows(&h.app, &a_id).await;
    let b_rows = usage_rows(&h.app, &b_id).await;
    assert_eq!(row(&b_rows, "chat", "server")["calls"], 1);
    assert!(a_rows.iter().all(|r| r["model"] == "seed"), "A made no call of its own: {a_rows:?}");
    assert_eq!(org_micros(&summary(&h.app, &b_id).await).round() as i64, 450);
    let _ = a;
}

#[tokio::test]
async fn nothing_configured_behaves_as_it_always_did() {
    // a self-hosted install: no operator token, no allowance, no organisation key
    let stub = stub().await;
    let settings = Settings { openai_api_key: Some(SERVER_KEY.into()), openai_base_url: stub.base.clone(), openai_chat_model: "gpt-4o-mini".into(), ..common::test_settings() };
    let app = common::TestApp::with_settings(settings).await;
    let u = app.user.clone();
    let db = app.db().await;
    let grant = ai::authorize(&db, &app.state.settings, &u.id, Purpose::Chat).await.unwrap().expect("allowed");
    assert_eq!(grant.source(), ai::KeySource::Server);
    chat_call(&app, &u, Purpose::Chat).await.unwrap();
    embed_one(&app, &u, "text").await.unwrap();
    assert_eq!(stub.keys("/chat/completions"), vec![SERVER_KEY]);
    // no cap, no stream options asked for, no refusal; the plan status says AI is on with no allowance
    let h = stub.hits();
    assert!(h.iter().all(|x| x.max_completion_tokens.is_none() && !x.has_stream_options));
    let plan = app.http("GET", "/api/org/plan", None, true).await.1;
    assert_eq!((plan["ai"]["enabled"].as_bool(), plan["ai"]["own_key"].as_bool(), plan["ai"]["usage"]["state"].is_null()), (Some(true), Some(false), true), "{plan}");
    // with no provider configured at all the answers are as before: not configured, not "AI is off"
    let unconfigured = common::TestApp::new().await;
    let r = ai::authorize(&unconfigured.db().await, &unconfigured.state.settings, &unconfigured.user.id, Purpose::Chat).await.unwrap();
    assert_eq!(r.err(), Some(Refusal::NotConfigured));
}
