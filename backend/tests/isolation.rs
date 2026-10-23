//! The tenant isolation proof (foundation-plan 3.3, "Proving no leaks"; docs/architecture/tenancy.md).
//!
//! Two orgs, each with a unique canary string planted in every org table and every control table that
//! has per-org rows. Then org A calls EVERY registry tool (directly and over `/mcp`) and EVERY HTTP
//! route using org B's ids, vault ids and names, and the suite asserts that no B canary shows up in a
//! response body, an error, a captured log line or a row A caused to be written; and that B's data is
//! unchanged afterwards. The same attack, in both directions, runs in three modes:
//!
//!  * normal: the database wall and the app filters both stand;
//!  * `NoAppFilters`: every owner/vault filter is rewritten to match everything, so only the
//!    database wall (one database per org, one database user per org) stands;
//!  * `SharedDb`: all orgs share one database, so only the app filters stand.
//!
//! A mutation check proves the harness can fail: remove one filter from one statement and the
//! suite goes red. The credential wall and the runtime "query without org context" counter have
//! their own checks.
//!
//! The layer switches are process-global, so every test in this file takes `SERIAL`.

mod common;

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use axum::body::Body;
use axum::http::{header, Request};
use axum::Router;
use eunomia_backend::connectors::crypto;
use eunomia_backend::error::AppError;
use eunomia_backend::isolation;
use eunomia_backend::jobs::{self, NewJob};
use eunomia_backend::models_user::{self, User};
use eunomia_backend::openapi;
use eunomia_backend::pool::{OrgDb, NO_ORG_CONTEXT};
use eunomia_backend::rid::RecordIdExt;
use eunomia_backend::state::AppState;
use eunomia_backend::store::{CONTROL_TABLES, TENANT_TABLES};
use eunomia_backend::tools::registry;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use tracing_subscriber::fmt::MakeWriter;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// `ISOLATION_TEST_NO_APP_FILTERS=1 cargo test --test isolation` runs the whole suite with the app
/// filters off. The tests that need the filters on (shared database, mutation check, both layers
/// removed) have nothing to say then and return early.
fn filters_forced_off() -> bool {
    std::env::var("ISOLATION_TEST_NO_APP_FILTERS").is_ok_and(|v| v == "1")
}

// ---- log capture ----------------------------------------------------------------------------

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

/// Every event and span of this crate at trace level, everything else at warn, into [`LOG`].
fn capture_logs() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let sub = tracing_subscriber::fmt()
            .with_env_filter("warn,eunomia_backend=trace")
            .with_writer(Sink)
            .with_ansi(false)
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
            .finish();
        let _ = tracing::subscriber::set_global_default(sub);
    });
}

// ---- the two orgs ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Normal,
    NoAppFilters,
    SharedDb,
}

/// Names are plain words an attacker may use as input; the canary is the secret that must never
/// come back out.
struct Org {
    name: &'static str,
    user: User,
    token: String,
    canary: String,
    ids: Ids,
}

#[derive(Default, Clone)]
struct Ids {
    vault: String,
    /// person, organisation, location, repository, file, symbol, then a custom kind (`entity` table)
    entities: Vec<String>,
    memory: String,
    thread: String,
    cache_literal: String,
    /// a record routed to `vault`, by the id its vault's members use
    routed: String,
    document: String,
    token_id: String,
    session_id: String,
    grant_id: String,
    trace: String,
}

impl Org {
    /// What a response must not contain: the canary and the org's key (it is in the database name,
    /// the org and tenant record ids).
    fn secrets(&self) -> Vec<String> {
        vec![self.canary.clone(), self.user.org.key()]
    }

    fn word(&self, w: &str) -> String {
        format!("{} {}", self.name, w)
    }
}

struct World {
    state: AppState,
    router: Router,
    a: Org,
    b: Org,
}

fn canary(who: &str) -> String {
    format!("CANARY-{}-{}", who.to_uppercase(), &uuid::Uuid::new_v4().simple().to_string()[..8])
}

async fn call(state: &AppState, user: &User, tool: &str, args: Value) -> Value {
    common::sys(registry::call(state, user, tool, args)).await.unwrap_or_else(|e| panic!("{tool}: {}", e.message))
}

async fn raw(router: &Router, method: &str, path: &str, body: Option<Value>, bearer: Option<&str>, ua: Option<&str>) -> (u16, String) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(t) = bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(ua) = ua {
        req = req.header(header::USER_AGENT, ua);
    }
    let req = match body {
        Some(b) => req.header(header::CONTENT_TYPE, "application/json").body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let resp = router.clone().oneshot(req).await.expect("router");
    let status = resp.status().as_u16();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Every org table gets a B row carrying the canary: through the real tools and routes where there
/// is one, straight into the database where there is not.
async fn plant(state: &AppState, router: &Router, name: &'static str, user: User) -> Org {
    let canary = canary(name);
    let token = models_user::create_api_token(&state.control, &user.id, "plant").await.unwrap().token;
    let db = state.pool.for_org(&user.org).await.unwrap();
    let org = || user.org;
    let mut ids = Ids::default();

    // vault, and the six entity kinds with the canary in their summary and aliases
    let v = call(state, &user, "vault_create", json!({"name": format!("{canary} vault"), "kind": "org"})).await;
    ids.vault = v["id"].as_str().unwrap_or_else(|| panic!("{v}")).to_string();
    let vault = ids.vault.clone();
    let words = [("person", "Bea"), ("organisation", "Holdings"), ("location", "Harbour"), ("db", "Ledger")];
    for (kind, w) in words {
        call(state, &user, "memory_write", json!({
            "subject_name": format!("{name} {w}"), "subject_kind": kind, "vault_id": vault,
            "text": format!("{name} {w} renewal terms {canary}"),
        }))
        .await;
    }
    call(state, &user, "memory_write", json!({
        "subject_name": format!("{name} Holdings"), "subject_kind": "organisation", "vault_id": vault, "type": "observation",
        "text": format!("{name} Holdings is stable {canary}"),
    }))
    .await;
    let repo = call(state, &user, "code_entity_upsert", json!({"kind": "repository", "name": format!("{name}-repo"), "vault_id": vault})).await;
    let repo_id = repo["id"].as_str().unwrap_or_else(|| panic!("{repo}")).to_string();
    let file = call(state, &user, "code_entity_upsert", json!({"kind": "file", "name": format!("{name}/main.rs"), "parent_id": repo_id, "vault_id": vault})).await;
    let file_id = file["id"].as_str().unwrap().to_string();
    call(state, &user, "code_entity_upsert", json!({"kind": "symbol", "name": format!("{name}_fn"), "parent_id": file_id, "vault_id": vault})).await;
    call(state, &user, "code_relate", json!({"from_id": file_id, "to_id": repo_id, "label": format!("in {canary}")})).await;

    for kind in ["person", "organisation", "location", "repository", "file", "symbol", "entity"] {
        let rows: Vec<surrealdb::types::RecordId> = db
            .test_raw()
            .query(format!("SELECT VALUE id FROM {kind} WHERE vault = $v"))
            .bind(("v", eunomia_backend::rid::parse(&vault).unwrap()))
            .await
            .unwrap()
            .take(0)
            .unwrap();
        let id = rows.into_iter().next().unwrap_or_else(|| panic!("no {kind} planted"));
        ids.entities.push(id.to_string());
        call(state, &user, "entity_update", json!({"entity_id": id.to_string(), "summary": format!("{canary} summary"), "aliases": [format!("{canary} alias")]})).await;
    }
    let mem: Vec<surrealdb::types::RecordId> = db.test_raw().query("SELECT VALUE id FROM memory WHERE vault = $v").bind(("v", eunomia_backend::rid::parse(&vault).unwrap())).await.unwrap().take(0).unwrap();
    ids.memory = mem[0].to_string();

    // cache_record (+ linked_to), chat, settings, connector, sync_status, embed_cache, pocket_recording: no tool plants them
    let env = |n: &str| -> eunomia_backend::cache::search::Envelope {
        serde_json::from_value(json!({
            "id": format!("demo:note:{name}-{n}"), "source": "demo", "type": "note", "external_id": format!("{name}-{n}"),
            "title": format!("{name} quarterly note {n}"), "body_text": format!("{name} renewal terms {canary}"),
            "payload": {"secret": canary},
            "links": if n == "2" { json!([{"target": format!("demo:note:{name}-1"), "rel": format!("see {canary}")}]) } else { json!([]) },
        }))
        .unwrap()
    };
    eunomia_backend::cache::search::upsert(&db, &user.id, &env("1")).await.unwrap();
    eunomia_backend::cache::search::upsert(&db, &user.id, &env("2")).await.unwrap();
    ids.cache_literal = format!("demo:note:{name}-1");
    // a record a connector routed to the org vault: its members read it, by the owner-keyed id
    let mut routed = env("routed");
    let vault_rid = eunomia_backend::rid::parse(&vault).unwrap();
    routed.placement = Some(eunomia_backend::cache::search::Placement {
        vault: Some(vault_rid.clone()),
        route_keys: vec![format!("title:{canary}")],
        writable: std::sync::Arc::new(vec![vault_rid]),
    });
    eunomia_backend::cache::search::upsert(&db, &user.id, &routed).await.unwrap();
    ids.routed = format!("{}:demo:note:{name}-routed", eunomia_backend::rid::key_string(&user.id.key).unwrap());

    // a document: the canary in its name and text, stored in the org's bucket and indexed (the index job
    // run by hand, there is no worker here), so its chunks carry the canary too
    let doc = call(state, &user, "document_upload", json!({"filename": format!("{canary}.md"), "text": format!("{name} renewal terms {canary}")})).await;
    ids.document = doc["id"].as_str().unwrap_or_else(|| panic!("{doc}")).to_string();
    let job = jobs::Job {
        id: surrealdb::types::RecordId::new("job", format!("plant-{name}")),
        org: Some(user.org.key()),
        kind: jobs::kind::INDEX_DOCUMENT.into(),
        owner: user.id.clone(),
        payload: json!({"document": ids.document, "revision": 1}),
        idempotency_key: format!("plant-{name}"),
        attempts: 1,
        max_attempts: 5,
        status: "running".into(),
        locked_until: None,
        traceparent: None,
    };
    common::sys(eunomia_backend::documents::index_job(state.clone(), job)).await.unwrap();

    let (_, body) = raw(router, "POST", "/api/chat/threads", Some(json!({"title": format!("{canary} thread")})), Some(&token), None).await;
    ids.thread = serde_json::from_str::<Value>(&body).unwrap()["id"].as_str().unwrap_or_else(|| panic!("{body}")).to_string();
    let enc = crypto::encrypt(&state.settings.encryption_key, &json!({"token": canary}).to_string());
    let sql = "
        CREATE chat_message SET owner = $u, thread_id = $thread, role = 'user', content = $c;
        UPSERT $settings SET owner = $u, observations_mission = $c, memory_skill = $c;
        CREATE connector SET owner = $u, kind = 'github', enabled = true, config = {note: $c}, credentials_encrypted = $enc;
        UPSERT $sync SET owner = $u, cursor = $c, last_error = $c;
        CREATE embed_cache SET text_hmac = $c, vector = [0.5, 0.25];
        CREATE pocket_recording SET owner = $u, recording_id = 'rec', title = $c, transcript = $c, raw = {note: $c};
        DELETE vault_member WHERE vault = $v;
        CREATE $vm SET vault = $v, user = $u, role = 'owner';";
    let uid = user.id.clone();
    db.test_raw()
        .query(sql)
        .bind(("u", uid.clone()))
        .bind(("thread", eunomia_backend::rid::parse(&ids.thread).unwrap()))
        .bind(("c", canary.clone()))
        .bind(("enc", enc))
        .bind(("settings", surrealdb::types::RecordId::new("app_settings", uid.key().clone())))
        .bind(("sync", surrealdb::types::RecordId::new("sync_status", format!("{}:github", eunomia_backend::rid::key_string(uid.key()).unwrap()))))
        .bind(("v", eunomia_backend::rid::parse(&vault).unwrap()))
        .bind(("vm", surrealdb::types::RecordId::new("vault_member", canary.clone())))
        .await
        .unwrap()
        .check()
        .unwrap();
    // an audit_log row (tenant database) comes from every mutating tool call above; a few more with the canary in the args.
    // The control audit_event never gets tool arguments, so its canary row is planted directly below.
    call(state, &user, "memory_write", json!({"subject_name": format!("{name} Bea"), "subject_kind": "person", "text": format!("audit {canary}")})).await;

    // control rows: a token and a session named for the canary, an OAuth grant with a token and a code,
    // a job, a capsule (a failing tool call), and the org / membership / user rows
    let (_, body) = raw(router, "POST", "/api/auth/tokens", Some(json!({"name": format!("{canary} token")})), Some(&token), None).await;
    ids.token_id = serde_json::from_str::<Value>(&body).unwrap()["id"].as_str().unwrap_or_else(|| panic!("{body}")).to_string();
    let login = raw(router, "POST", "/api/auth/login", Some(json!({"email": user.email, "password": common::PASSWORD})), None, Some(&format!("{canary} browser"))).await;
    assert_eq!(login.0, 200, "{}", login.1);
    ids.session_id = {
        let mut res = state.control.test_raw().query("SELECT VALUE id FROM session WHERE owner = $u ORDER BY created_at DESC LIMIT 1").bind(("u", uid.clone())).await.unwrap();
        res.take::<Vec<surrealdb::types::RecordId>>(0).unwrap()[0].to_string()
    };
    let trace = format!("{:032x}", uuid::Uuid::new_v4().as_u128());
    ids.trace = trace.clone();
    eunomia_backend::telemetry::with_trace_id(
        trace,
        common::sys(registry::call(state, &user, "entity_update", json!({"entity_id": "person:nobody", "summary": format!("{canary} capsule")}))),
    )
    .await
    .unwrap();
    jobs::enqueue(&state.control, NewJob::new("sync", uid.clone(), format!("plant-{canary}")).in_org(org()).payload(json!({"source": canary}))).await.unwrap();
    let control_sql = "
        UPDATE $org SET name = $c;
        UPDATE $u SET api_token_hash = $c;
        DELETE membership WHERE user = $u;
        CREATE $mem SET user = $u, org = $org, role = 'owner';
        CREATE identity SET user = $u, issuer = 'https://idp.example', subject = $c, email = $c;
        CREATE audit_event SET user = $u, actor_kind = 'user', actor_id = 'plant', action = 'plant', target = $c, outcome = 'ok';
        LET $g = (CREATE oauth_grant SET owner = $u, client_id = 'client-x', client_name = $c, scope = ['memory:read'], resource = 'http://localhost:8001/mcp' RETURN VALUE id)[0];
        CREATE oauth_token SET kind = 'access', token_hash = $c, family = $g, expires_at = time::now() + 15m;
        CREATE oauth_code SET code_hash = $c, owner = $u, client_id = 'client-x', redirect_uri = 'http://localhost/cb', code_challenge = $c, scope = ['memory:read'], resource = 'http://x', expires_at = time::now() + 1m;
        RETURN $g;";
    let mut res = state
        .control
        .test_raw()
        .query(control_sql)
        .bind(("org", user.org.record()))
        .bind(("u", uid.clone()))
        .bind(("c", canary.clone()))
        .bind(("mem", surrealdb::types::RecordId::new("membership", canary.clone())))
        .await
        .unwrap();
    let grant: Option<surrealdb::types::RecordId> = res.take(res.num_statements() - 1).unwrap();
    ids.grant_id = grant.unwrap().to_string();

    Org { name, user, token, canary, ids }
}

async fn build(mode: Mode) -> World {
    capture_logs();
    let state = common::bare_state().await;
    let a_user = common::register_personal(&state, "alpha@example.com").await;
    if mode == Mode::SharedDb {
        // every org now lives in A's database: only the app-level filters keep them apart
        let mut res = state.control.test_raw().query("SELECT db, db_pass_enc FROM ONLY $t").bind(("t", a_user.org.tenant_record())).await.unwrap();
        let row: Value = res.take::<Option<Value>>(0).unwrap().unwrap();
        let pass = crypto::decrypt(&state.settings.encryption_key, row["db_pass_enc"].as_str().unwrap()).unwrap();
        state.pool.set_shared_database(row["db"].as_str().unwrap(), "app", &pass);
    }
    let b_user = common::register_personal(&state, "beta@example.com").await;
    assert_ne!(a_user.org, b_user.org);
    let router = eunomia_backend::app(state.clone());
    let a = plant(&state, &router, "Alpha", a_user).await;
    let b = plant(&state, &router, "Beta", b_user).await;
    World { state, router, a, b }
}

// ---- what is stored -------------------------------------------------------------------------

/// How many rows in each table carry one of `org`'s secrets. Control rows are found by scanning the
/// control database; org rows by scanning the org's own database (or the shared one).
async fn secret_rows(w: &World, org: &Org) -> BTreeMap<String, usize> {
    let db = w.state.pool.for_org(&org.user.org).await.unwrap();
    let mut out = BTreeMap::new();
    let secrets = org.secrets();
    for (table, raw_db) in TENANT_TABLES.iter().map(|t| (*t, db.test_raw())).chain(CONTROL_TABLES.iter().map(|t| (*t, w.state.control.test_raw()))) {
        let rows: Vec<Value> = raw_db.query(format!("SELECT * FROM {table}")).await.unwrap().take(0).unwrap();
        let n = rows.iter().filter(|r| secrets.iter().any(|s| r.to_string().contains(s.as_str()))).count();
        out.insert(table.to_string(), n);
    }
    out
}

/// Tables that never carry a secret, and why.
const NO_SECRET_ROWS: &[&str] = &[
    "oauth_client",  // an instance-wide registry of MCP clients, not per org
    "job_leader",    // one global lease row
];

// ---- the attack -----------------------------------------------------------------------------

/// A plausible-looking value for a tool argument, using the victim's ids and plain names.
fn attack_value(tool: &str, prop: &str, schema: &Value, victim: &Org) -> Value {
    let id = |i: usize| json!(victim.ids.entities[i]);
    match (tool, prop) {
        (_, p) if p.contains("vault") && p.ends_with("ids") => json!([victim.ids.vault]),
        (_, p) if p.contains("vault") => json!(victim.ids.vault),
        ("get", "id") | ("links", "id") => json!(victim.ids.routed),
        (t, "id" | "document_id") if t.starts_with("document_") => json!(victim.ids.document),
        (_, "source_record_id") => json!(victim.ids.cache_literal),
        ("entities_get", "id") => id(0),
        (_, "parent_id") => id(3),
        (_, "from_id") | (_, "winner_id") | (_, "subject_id") | (_, "entity_id") => id(0),
        (_, "to_id") | (_, "loser_id") => id(1),
        (_, "memory_id") => json!(victim.ids.memory),
        (_, "email") => json!("beta@example.com"),
        (_, "query") => json!(victim.word("renewal terms")),
        (_, "name") | (_, "subject_name") => json!(victim.word("Holdings")),
        (_, "topic") => json!("overview"),
        _ => match schema["type"].as_str() {
            Some("integer") => json!(5),
            Some("array") => json!([]),
            Some("object") => json!({}),
            _ => match prop {
                "kind" | "subject_kind" => json!(if tool == "code_entity_upsert" { "repository" } else if tool.starts_with("vault") { "org" } else { "person" }),
                "role" => json!("member"),
                "type" => json!("world"),
                "mode" => json!("keyword"),
                _ => json!("x"),
            },
        },
    }
}

fn tool_args(tool: &str, schema: &Value, victim: &Org) -> Value {
    let mut args = serde_json::Map::new();
    for (prop, s) in schema["properties"].as_object().into_iter().flatten() {
        args.insert(prop.clone(), attack_value(tool, prop, s, victim));
    }
    Value::Object(args)
}

fn path_value(param: &str, victim: &Org) -> String {
    match param {
        "vault_id" => victim.ids.vault.clone(),
        "entity_id" => victim.ids.entities[0].clone(),
        "memory_id" => victim.ids.memory.clone(),
        "thread_id" => victim.ids.thread.clone(),
        "token_id" => victim.ids.token_id.clone(),
        "session_id" => victim.ids.session_id.clone(),
        "grant_id" => victim.ids.grant_id.clone(),
        "trace_id" => victim.ids.trace.clone(),
        "owner_id" => victim.user.id.to_string(),
        "email" => "beta@example.com".into(),
        "key" => "demo".into(),
        "kind" => "github".into(),
        "name" => "recall".into(),
        "recording_id" => victim.ids.cache_literal.clone(),
        "document_id" => victim.ids.document.clone(),
        _ => "x".into(),
    }
}

/// Query strings that name the victim's data. Unknown parameters are ignored by the routes.
fn query_string(victim: &Org) -> String {
    format!(
        "?vault_id={v}&vault_ids={v}&query={q}&q={q}&kind=person&kinds=person&limit=1000&offset=0&days=30",
        v = victim.ids.vault.replace(':', "%3A"),
        q = victim.word("renewal terms").replace(' ', "%20")
    )
}

/// Where a body may have come from, and what leaked.
struct Hit {
    call: String,
    secret: String,
}

fn scan(call: &str, body: &str, victim: &Org, hits: &mut Vec<Hit>) {
    for s in victim.secrets() {
        if body.contains(&s) {
            hits.push(Hit { call: call.to_string(), secret: s });
        }
    }
}

fn error_text(e: &AppError) -> String {
    format!("{} {} {:?}", e.code.as_str(), e.message, e.source)
}

/// `attacker` calls everything with `victim`'s ids. Returns what leaked; also asserts coverage.
async fn attack(w: &World, attacker: &Org, victim: &Org, victim_must_stay_unchanged: bool) -> Vec<Hit> {
    let mut hits = Vec::new();
    LOG.lock().unwrap().clear();
    let victim_before = secret_rows(w, victim).await;

    // 1. every registry tool, directly and over MCP
    let tools = registry::all_tools();
    let mut called = Vec::new();
    for (name, spec) in tools {
        let args = tool_args(name, &spec.schema, victim);
        let out = match common::sys(registry::call(&w.state, &attacker.user, name, args.clone())).await {
            Ok(v) => v.to_string(),
            Err(e) => error_text(&e),
        };
        scan(&format!("tool {name}"), &out, victim, &mut hits);
        let rpc = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}});
        let (_, body) = raw(&w.router, "POST", "/mcp", Some(rpc), Some(&attacker.token), None).await;
        scan(&format!("mcp {name}"), &body, victim, &mut hits);
        called.push(name.to_string());
    }
    assert!(called.len() >= 29, "only {} tools attacked", called.len());
    let tool_count = called.len();

    // 2. every route in the spec
    let spec: Value = serde_json::from_str(&openapi::spec_json()).unwrap();
    let mut routes = 0;
    for (path, item) in spec["paths"].as_object().unwrap() {
        for (method, op) in item.as_object().unwrap() {
            let mut url = path.clone();
            for p in op["parameters"].as_array().into_iter().flatten().filter(|p| p["in"] == "path") {
                let name = p["name"].as_str().unwrap();
                url = url.replace(&format!("{{{name}}}"), &path_value(name, victim));
            }
            let has_query = op["parameters"].as_array().into_iter().flatten().any(|p| p["in"] == "query");
            if has_query {
                url.push_str(&query_string(victim));
            }
            let body = op.get("requestBody").map(|_| {
                json!({
                    "name": victim.word("Holdings"), "title": victim.word("Holdings"), "email": "beta@example.com", "role": "member",
                    "kind": "person", "vault_id": victim.ids.vault, "vault_id_a": victim.ids.vault, "vault_id_b": victim.ids.vault,
                    "text": "x", "message": victim.word("renewal terms"), "winner_id": victim.ids.entities[0], "loser_id": victim.ids.entities[1],
                    "to_id": victim.ids.entities[1], "label": "x", "approve": true, "client_id": "client-x", "redirect_uri": "http://localhost/cb",
                    "code_challenge": "x", "code_challenge_method": "S256", "response_type": "code",
                })
            });
            // the same route with only paging and kind: it runs in the attacker's default scope, where a
            // missing filter would show the victim's rows instead of being stopped by the vault check
            if has_query && method == "get" {
                let plain = format!("{}?limit=1000&offset=0&kind=person&kinds=person&days=30", url.split('?').next().unwrap());
                let (_, text) = raw(&w.router, "GET", &plain, None, Some(&attacker.token), None).await;
                scan(&format!("GET {path} (default scope)"), &text, victim, &mut hits);
            }
            let (status, text) = raw(&w.router, &method.to_uppercase(), &url, body, Some(&attacker.token), None).await;
            // with every filter rewritten a few routes may fail closed (500); the app is broken on purpose there
            assert!(status != 500 || no_app_filters(), "{method} {path} answered 500: {text}");
            scan(&format!("{} {path}", method.to_uppercase()), &text, victim, &mut hits);
            routes += 1;
        }
    }
    assert!(routes >= 60, "only {routes} routes attacked");
    // routes outside /api: MCP discovery and the OAuth endpoints
    for url in ["/healthz", "/.well-known/oauth-authorization-server", "/.well-known/oauth-protected-resource"] {
        let (_, text) = raw(&w.router, "GET", url, None, Some(&attacker.token), None).await;
        scan(&format!("GET {url}"), &text, victim, &mut hits);
    }

    // 3. logs and spans, and what the attack caused to be stored in the attacker's name
    let log = LOG.lock().unwrap().clone();
    assert!(log.contains("store.query") && log.contains("op=") || log.contains("tool call"), "the log capture is not capturing ({} bytes)", log.len());
    eprintln!("{} attacking {}: {tool_count} tools (direct and over /mcp), {routes} routes, {} bytes of logs, {} org tables scanned", attacker.name, victim.name, log.len(), victim_before.len());
    scan("captured logs", &log, victim, &mut hits);
    let mut res = w
        .state
        .control
        .test_raw()
        .query("SELECT * FROM audit_event WHERE user = $u; SELECT * FROM failure_capsule WHERE user = $s;")
        .bind(("u", attacker.user.id.clone()))
        .bind(("s", attacker.user.id.to_string()))
        .await
        .unwrap();
    let stored: Vec<Value> = res.take::<Vec<Value>>(0).unwrap().into_iter().chain(res.take::<Vec<Value>>(1).unwrap()).collect();
    scan("audit and capsule rows written for the attacker", &serde_json::to_string(&stored).unwrap(), victim, &mut hits);

    // 4. the victim's data is exactly as it was
    let victim_after = secret_rows(w, victim).await;
    if victim_must_stay_unchanged {
        assert_eq!(victim_before, victim_after, "{} attacking {}: the victim's rows changed", attacker.name, victim.name);
    }
    hits
}

fn no_app_filters() -> bool {
    NO_FILTERS.load(std::sync::atomic::Ordering::SeqCst) || filters_forced_off()
}

static NO_FILTERS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn report(hits: &[Hit]) -> String {
    hits.iter().map(|h| format!("{} leaked {}", h.call, h.secret)).collect::<Vec<_>>().join("\n")
}

/// The detector works: each org sees its own canary through the same tools it is later denied.
async fn control_group(w: &World) {
    for org in [&w.a, &w.b] {
        let search = call(&w.state, &org.user, "entities_search", json!({"query": org.word("Holdings"), "vault_id": org.ids.vault})).await.to_string();
        assert!(search.contains(&org.canary), "the owner must see its own canary: {search}");
        let routed = call(&w.state, &org.user, "get", json!({"id": org.ids.routed})).await.to_string();
        assert!(routed.contains(&org.canary), "the vault's member must read its routed record: {routed}");
        let (_, threads) = raw(&w.router, "GET", "/api/chat/threads", None, Some(&org.token), None).await;
        assert!(threads.contains(&org.canary), "{threads}");
        let (_, audit) = raw(&w.router, "GET", "/api/audit", None, Some(&org.token), None).await;
        assert!(audit.contains(&org.canary), "{audit}");
        let (_, tokens) = raw(&w.router, "GET", "/api/auth/tokens", None, Some(&org.token), None).await;
        assert!(tokens.contains(&org.canary), "{tokens}");
    }
}

/// Every org table and every control table with per-org rows holds a secret of each org.
async fn assert_planted_everywhere(w: &World) {
    for org in [&w.a, &w.b] {
        let rows = secret_rows(w, org).await;
        let empty: Vec<&String> = rows.iter().filter(|(t, n)| **n == 0 && !NO_SECRET_ROWS.contains(&t.as_str())).map(|(t, _)| t).collect();
        assert!(empty.is_empty(), "{}: no planted row in {empty:?}", org.name);
    }
}

async fn run_mode(mode: Mode) {
    isolation::set_no_app_filters(mode == Mode::NoAppFilters);
    NO_FILTERS.store(mode == Mode::NoAppFilters, std::sync::atomic::Ordering::SeqCst);
    let w = build(mode).await;
    assert_planted_everywhere(&w).await;
    control_group(&w).await;
    let mut hits = attack(&w, &w.a, &w.b, true).await;
    hits.extend(attack(&w, &w.b, &w.a, true).await);
    isolation::set_no_app_filters(false);
    isolation::clear_mutations();
    if mode == Mode::NoAppFilters || filters_forced_off() {
        assert!(isolation::rewritten() > 100, "the filter switch rewrote only {} statements", isolation::rewritten());
    }
    assert!(hits.is_empty(), "{mode:?}: isolation leaks:\n{}", report(&hits));
    assert_eq!(NO_ORG_CONTEXT.load(std::sync::atomic::Ordering::Relaxed), 0, "{mode:?}: a query ran without an org context");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn isolation_holds_with_both_layers() {
    let _g = SERIAL.lock().await;
    run_mode(Mode::Normal).await;
}

/// App filters off: the database wall alone keeps the orgs apart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn isolation_holds_with_app_filters_off() {
    let _g = SERIAL.lock().await;
    run_mode(Mode::NoAppFilters).await;
}

/// All orgs in one database: the app filters alone keep them apart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn isolation_holds_in_a_shared_database() {
    let _g = SERIAL.lock().await;
    if filters_forced_off() {
        return;
    }
    run_mode(Mode::SharedDb).await;
}

/// Both layers removed at once leaks, so the suite measures the layers and not the data: shared database
/// and no app filters is exactly what a cross-tenant bug looks like.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removing_both_layers_leaks() {
    let _g = SERIAL.lock().await;
    if filters_forced_off() {
        return;
    }
    isolation::set_no_app_filters(true);
    NO_FILTERS.store(true, std::sync::atomic::Ordering::SeqCst);
    let w = build(Mode::SharedDb).await;
    let (status, text) = raw(&w.router, "GET", "/api/chat/threads", None, Some(&w.a.token), None).await;
    isolation::set_no_app_filters(false);
    NO_FILTERS.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(status, 200);
    assert!(text.contains(&w.b.canary), "with the database wall and the filters gone, A sees B's thread: {text}");
}

/// What a mutation case calls as the attacker, with the victim's ids at hand.
enum Call {
    Route(String),
    Tool(&'static str, Value),
}

/// The harness can fail: take the owner or vault filter out of one statement (or the small group that
/// guards one read) and the attacker, calling the matching route or tool as an ordinary user, gets the
/// victim's data. Shared-database mode, so only the app filters stand between the orgs. Eleven
/// statements across vaults, entities, cache (its owner and routed-vault filters), chat, connectors, audit
/// and sync status.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_check_removing_one_filter_turns_the_suite_red() {
    let _g = SERIAL.lock().await;
    if filters_forced_off() {
        return;
    }
    let w = build(Mode::SharedDb).await;
    assert!(attack(&w, &w.a, &w.b, true).await.is_empty(), "baseline must be clean before mutating");

    type Case = (&'static str, &'static [(&'static str, &'static str)], fn(&Org) -> Call);
    // (what it covers, [(statement, filter text)], the call that reads it)
    let cases: [Case; 11] = [
        ("chat threads", &[("app.chat_thread_list", "WHERE owner = $owner")], |_| Call::Route("/api/chat/threads".into())),
        ("chat messages", &[("app.chat_messages_for_owner", "WHERE owner = $owner")], |_| Call::Route("/api/export".into())),
        ("audit", &[("app.audit_list", "WHERE owner = $owner")], |_| Call::Route("/api/audit".into())),
        ("vault list", &[("vaults.list_mine", "WHERE user = $user")], |_| Call::Tool("vault_list", json!({}))),
        ("vault membership", &[("vaults.membership_active", "WHERE vault = $vault")], |v| Call::Tool("entities_get", json!({"id": v.ids.entities[0]}))),
        ("connectors", &[("app.connectors_of_kind", "WHERE owner = $owner")], |_| Call::Route("/api/connectors/github".into())),
        ("sync status", &[("app.sources_sync_status_list", "WHERE owner = $owner")], |_| Call::Route("/api/sources/status".into())),
        ("entity list", &[("entities.list_by_vault", "WHERE vault = $vault")], |_| Call::Route("/api/entities".into())),
        ("entity graph", &[("entities.select_by_vault", "WHERE vault = $vault")], |_| Call::Tool("entities_graph", json!({}))),
        ("cache list", &[("cache.list_records", "owner = $owner")], |_| Call::Tool("list", json!({"type": "note"}))),
        ("routed records", &[("cache.list_records", "vault IN $vaults")], |_| Call::Tool("list", json!({"type": "note"}))),
    ];
    let probe = |caller: &Org, victim: &Org, call: Call| {
        let w = &w;
        let (token, user, secrets) = (caller.token.clone(), caller.user.clone(), victim.secrets());
        async move {
            let body = match call {
                Call::Route(path) => raw(&w.router, "GET", &path, None, Some(&token), None).await.1,
                Call::Tool(name, args) => common::sys(registry::call(&w.state, &user, name, args)).await.map(|v| v.to_string()).unwrap_or_default(),
            };
            secrets.iter().any(|s| body.contains(s.as_str()))
        }
    };
    let mk = |f: fn(&Org) -> Call, victim: &Org| f(victim);

    // clean first: no case leaks without a mutation, in either direction
    for (what, _, call) in &cases {
        assert!(!probe(&w.a, &w.b, mk(*call, &w.b)).await && !probe(&w.b, &w.a, mk(*call, &w.a)).await, "{what} leaks before any mutation");
    }
    for (what, stmts, call) in &cases {
        for (stmt, from) in *stmts {
            let to = from.replace(&from[from.find(|c: char| c.is_lowercase()).unwrap()..], "true");
            isolation::mutate(stmt, from, &to);
        }
        // a mutated filter shows in whichever direction the rows sort the victim's after the caller's
        let leaked = probe(&w.a, &w.b, mk(*call, &w.b)).await || probe(&w.b, &w.a, mk(*call, &w.a)).await;
        isolation::clear_mutations();
        assert!(leaked, "removing the filter from {stmts:?} ({what}) must leak the other org's data");
    }
}

/// Org A's database user cannot reach org B's database, nor the control database, by any route.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn credential_wall_org_a_cannot_read_org_b() {
    let _g = SERIAL.lock().await;
    capture_logs();
    let state = common::bare_state().await;
    let a = common::register_personal(&state, "alpha@example.com").await;
    let b = common::register_personal(&state, "beta@example.com").await;
    call(&state, &b, "memory_write", json!({"subject_name": "Hidden", "subject_kind": "person", "text": "b-only fact"})).await;
    let (a_db, b_db) = (a.org.db_name(), b.org.db_name());

    async fn pass_of(state: &AppState, org: &eunomia_backend::pool::OrgId) -> String {
        let mut res = state.control.test_raw().query("SELECT VALUE db_pass_enc FROM ONLY $t").bind(("t", org.tenant_record())).await.unwrap();
        crypto::decrypt(&state.settings.encryption_key, &res.take::<Option<String>>(0).unwrap().unwrap()).unwrap()
    }
    let (pass_a, pass_b) = (pass_of(&state, &a.org).await, pass_of(&state, &b.org).await);
    assert_ne!(pass_a, pass_b, "every org has its own generated password");

    // 1. A's credentials do not sign in to B's database, and B's do not sign in to A's
    assert!(state.pool.test_session(a.org, &b_db, "app", &pass_a).await.is_err(), "A's password opened B's database");
    assert!(state.pool.test_session(b.org, &a_db, "app", &pass_b).await.is_err(), "B's password opened A's database");
    // the right credentials do work (the wall is not just a broken login)
    assert!(state.pool.test_session(a.org, &a_db, "app", &pass_a).await.is_ok());

    // 2. A's live handle cannot be pointed at B's database, by the client or inside a query. The exact
    // refusal is asserted, not "some error": a wrong-password or network failure must not pass for the wall.
    let a_handle: OrgDb = common::org_db(&state, &a).await;
    let refusal = |db: &str| format!("You don't have permission to change to the {db} database");
    let session = a_handle.test_raw().clone();
    let err = session.use_db(&b_db).await.expect_err("A's session switched to B's database");
    assert_eq!(err.to_string(), refusal(&b_db));
    let sneaky = a_handle.test_raw().clone();
    let mut inline = sneaky.query(format!("USE DB `{b_db}`; SELECT * FROM memory;")).await.expect("the query is sent");
    let errors: Vec<(usize, String)> = inline.take_errors().into_iter().map(|(i, e)| (i, e.to_string())).collect();
    assert_eq!(errors, [(0, refusal(&b_db))], "USE DB inside a query must be refused");
    // the SELECT after it still runs, in A's own database: none of B's rows
    let after: Vec<Value> = inline.take(1).expect("the statement after the refused USE ran in A's database");
    assert!(!serde_json::to_string(&after).unwrap().contains("b-only fact"), "USE DB inside a query reached org B");

    // 3. and not the control database either (accounts, credentials, every org's password)
    let err = a_handle.test_raw().clone().use_db(eunomia_backend::pool::CONTROL_DB).await.expect_err("an org handle switched to control");
    assert_eq!(err.to_string(), refusal(eunomia_backend::pool::CONTROL_DB));

    // 4. an org user cannot manage users or read the root-level info
    let mut manage = a_handle.test_raw().clone().query("DEFINE USER intruder ON DATABASE PASSWORD 'x' ROLES OWNER").await.expect("the query is sent");
    let errors: Vec<(usize, String)> = manage.take_errors().into_iter().map(|(i, e)| (i, e.to_string())).collect();
    assert_eq!(errors, [(0, "IAM error: Not enough permissions to perform this action".to_string())]);

    // 5. the control handle is not an org handle either
    let err = state.control.test_raw().clone().use_db(&b_db).await.expect_err("the control handle switched to an org database");
    assert_eq!(err.to_string(), refusal(&b_db));

    // the refusals did not break A's own handle, and it sees its own data (none of B's)
    let mut own = a_handle.test_raw().query("SELECT * FROM memory").await.unwrap();
    let rows: Vec<Value> = own.take(0).unwrap();
    assert!(!serde_json::to_string(&rows).unwrap().contains("b-only fact"));
}

/// Inviting an email from another org looks exactly like inviting nobody.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn other_orgs_users_cannot_be_probed_by_email() {
    let _g = SERIAL.lock().await;
    let state = common::bare_state().await;
    let a = common::register_personal(&state, "alpha@example.com").await;
    let _b = common::register_personal(&state, "beta@example.com").await;
    let vault = call(&state, &a, "vault_create", json!({"name": "Team", "kind": "org"})).await["id"].as_str().unwrap().to_string();
    let other_org = call(&state, &a, "vault_invite", json!({"vault_id": vault, "email": "beta@example.com"})).await;
    let nobody = call(&state, &a, "vault_invite", json!({"vault_id": vault, "email": "nobody@example.com"})).await;
    let strip = |v: &Value, email: &str| v["error"].as_str().unwrap().replace(email, "<email>");
    assert_eq!(strip(&other_org, "beta@example.com"), strip(&nobody, "nobody@example.com"), "{other_org} vs {nobody}");
    assert!(other_org["error"].as_str().unwrap().contains("no user with email"));
}

/// The runtime counter is exposed to admins and reads zero.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_no_org_context_metric_is_exposed_and_zero() {
    let _g = SERIAL.lock().await;
    let state = common::bare_state().await;
    let a = common::register_personal(&state, "alpha@example.com").await;
    let b = common::register_personal(&state, "beta@example.com").await;
    let router = eunomia_backend::app(state.clone());
    let tok = |u: &User| {
        let (c, id) = (state.control.clone(), u.id.clone());
        async move { models_user::create_api_token(&c, &id, "t").await.unwrap().token }
    };
    let (ta, tb) = (tok(&a).await, tok(&b).await);
    let (status, body) = raw(&router, "GET", "/api/debug/metrics", None, Some(&ta), None).await;
    assert_eq!(status, 200, "{body}");
    let m: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(m["queries_without_org_context"], 0, "{m}");
    assert!(m["open_org_handles"].as_u64().unwrap() >= 1);
    let (status, _) = raw(&router, "GET", "/api/debug/metrics", None, Some(&tb), None).await;
    assert_eq!(status, 403, "only an admin reads metrics");

    // a statement that reaches for the other database's tables is counted
    let before = NO_ORG_CONTEXT.load(std::sync::atomic::Ordering::Relaxed);
    let org_db = common::org_db(&state, &a).await;
    let _ = eunomia_backend::store::dynamic(&org_db, "test.wrong_database", "SELECT * FROM session").await;
    assert_eq!(NO_ORG_CONTEXT.load(std::sync::atomic::Ordering::Relaxed), before + 1);
    NO_ORG_CONTEXT.store(0, std::sync::atomic::Ordering::Relaxed);
}
