//! Data-subject rights end to end against a real in-memory SurrealDB and its file bucket
//! (docs/privacy/subject-rights.md):
//!
//! - erasure: a person with data in every inventory category is erased through the real route and
//!   job; afterwards no row in any control or org table names them (by id or email), their files are
//!   gone from the bucket, what they wrote in a shared space stays under an owner who remains, the
//!   completion record keeps only counts, an opaque reference and a clean post-check, and the
//!   control fails closed: with one category's deletion removed the post-check reports it;
//! - refusals: wrong password, legal hold (audited both ways), the workspace's last owner;
//! - suppression: a sync for an erased account stands down, and a restored copy of the account is
//!   erased again by the re-apply;
//! - export: the archive holds every category with counts in its manifest, what the person wrote
//!   in spaces they can read and nothing from a space they left.

mod common;

use std::io::Read;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{header, Request};
use axum::Router;
use common::TestApp;
use eunomia_backend::config::Settings;
use eunomia_backend::jobs::{self, handlers, worker, NewJob, WorkerConfig};
use eunomia_backend::models_user::{self, User};
use eunomia_backend::rid::RecordIdExt;
use eunomia_backend::state::AppState;
use eunomia_backend::store::{CONTROL_TABLES, TENANT_TABLES};
use eunomia_backend::tools::registry;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use surrealdb::types::RecordId;
use tower::ServiceExt;

trait KeyStr {
    fn key_str(&self) -> String;
}
impl KeyStr for RecordId {
    fn key_str(&self) -> String {
        eunomia_backend::rid::key_string(self.key()).unwrap_or_default()
    }
}

async fn call(state: &AppState, user: &User, tool: &str, args: Value) -> Value {
    let v = common::sys(registry::call(state, user, tool, args.clone())).await.unwrap_or_else(|e| panic!("{tool}: {}", e.message));
    assert!(v.get("error").is_none(), "{tool} {args}: {v}");
    v
}

/// A request as a bearer token or a session cookie; returns status, headers' content type and body bytes.
async fn req(router: &Router, method: &str, path: &str, body: Option<Value>, bearer: Option<&str>, cookie: Option<&str>) -> (u16, Vec<u8>) {
    let mut r = Request::builder().method(method).uri(path);
    if let Some(t) = bearer {
        r = r.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(c) = cookie {
        r = r.header(header::COOKIE, c);
    }
    let r = match body {
        Some(b) => r.header(header::CONTENT_TYPE, "application/json").body(Body::from(b.to_string())),
        None => r.body(Body::empty()),
    }
    .unwrap();
    let resp = router.clone().oneshot(r).await.expect("router");
    let status = resp.status().as_u16();
    (status, resp.into_body().collect().await.unwrap().to_bytes().to_vec())
}

fn json_of(b: &[u8]) -> Value {
    serde_json::from_slice(b).unwrap_or(Value::Null)
}

async fn login(router: &Router, email: &str) -> String {
    let ((status, body), cookie) = common::http(router, "POST", "/api/auth/login", Some(json!({"email": email, "password": common::PASSWORD})), None, None).await;
    assert_eq!(status, 200, "{body}");
    cookie.expect("a session cookie").split(';').next().unwrap().to_string()
}

async fn token_for(state: &AppState, user: &User) -> String {
    models_user::create_api_token_with(&state.control, &user.id, "erasable", &eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect::<Vec<_>>(), None, None)
        .await
        .unwrap()
        .token
}

async fn app_with_worker() -> (TestApp, tokio::sync::watch::Sender<bool>) {
    let app = TestApp::with_settings(Settings { embeddings_backend: "stub".into(), ..common::test_settings() }).await;
    let (tx, rx) = tokio::sync::watch::channel(false);
    let cfg = WorkerConfig { id: "subject".into(), poll: Duration::from_millis(20), retry_base: Duration::from_millis(10), owner_cap: 1000, ..WorkerConfig::default() };
    tokio::spawn(worker::run(app.state.clone(), handlers::registry(), cfg, rx));
    (app, tx)
}

async fn wait_for<F, Fut>(what: &str, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let start = Instant::now();
    while !f().await {
        assert!(start.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn control_rows(state: &AppState, sql: &str, bind: (&'static str, Value)) -> Vec<Value> {
    state.control.test_raw().query(sql).bind(bind).await.unwrap().take(0).unwrap()
}

async fn object_exists(db: &eunomia_backend::pool::OrgDb, path: &str) -> bool {
    let mut res = db.test_raw().query("file::exists(type::file('documents', $p))").bind(("p", path.to_string())).await.unwrap();
    res.take::<Option<bool>>(0).unwrap().unwrap_or(false)
}

struct Seeded {
    team: String,
    club: String,
    files: Vec<String>,
}

/// Puts `who` in every inventory category (docs/privacy/subject-rights.md): through the real tools and
/// routes where there is one, straight into the database where there is not.
async fn seed(app: &TestApp, who: &User, bob: &User) -> Seeded {
    let state = &app.state;
    let db = common::org_db(state, who).await;
    let token = token_for(state, who).await;
    // random, not the user key: the scan afterwards looks for the key, so the seed text must not carry it
    let mark = format!("canary-{}", uuid::Uuid::new_v4().simple());

    // a shared space the workspace owner runs; `who` writes in it (this stays, reassigned)
    let team = call(state, &app.user, "vault_create", json!({"name": format!("Team {mark}"), "kind": "org"})).await["id"].as_str().unwrap().to_string();
    call(state, &app.user, "vault_invite", json!({"vault_id": team, "email": who.email})).await;
    call(state, who, "vault_accept_invitation", json!({"vault_id": team})).await;
    call(state, who, "memory_write", json!({"subject_name": "Quarterly plan", "subject_kind": "organisation", "vault_id": team, "text": "Plan agreed by the team"})).await;
    // the owner mentions them by email, in another case, in the shared space: a fact and an entity name (scrubbed, not deleted)
    let shouted = who.email.to_uppercase().replacen("EXAMPLE", "Example", 1);
    call(state, &app.user, "memory_write", json!({"subject_name": "Contacts", "subject_kind": "organisation", "vault_id": team, "text": format!("Reach {shouted} for invoices")})).await;
    call(state, &app.user, "memory_write", json!({"subject_name": format!("Billing desk {shouted}"), "subject_kind": "person", "vault_id": team, "text": "Handles refunds"})).await;
    // a relation the person wrote between two of the owner's entities in the shared space (it stays, reassigned)
    call(state, &app.user, "memory_write", json!({"subject_name": "Acme", "subject_kind": "organisation", "vault_id": team, "text": "A customer"})).await;
    call(state, &app.user, "memory_write", json!({"subject_name": "Globex", "subject_kind": "organisation", "vault_id": team, "text": "A supplier"})).await;
    let team_rid = eunomia_backend::rid::parse(&team).unwrap();
    db.test_raw()
        .query("LET $a = (SELECT VALUE id FROM organisation WHERE vault = $v AND name = 'Acme')[0]; LET $g = (SELECT VALUE id FROM organisation WHERE vault = $v AND name = 'Globex')[0]; RELATE $a->relates_to->$g SET owner = $u, label = 'buys from';")
        .bind(("v", team_rid.clone()))
        .bind(("u", who.id.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();
    // a space they own alone with bob in it (ownership is handed to bob)
    let club = call(state, who, "vault_create", json!({"name": format!("Club {mark}"), "kind": "org"})).await["id"].as_str().unwrap().to_string();
    call(state, who, "vault_invite", json!({"vault_id": club, "email": bob.email})).await;
    call(state, bob, "vault_accept_invitation", json!({"vault_id": club})).await;
    call(state, who, "memory_write", json!({"subject_name": "Club rules", "subject_kind": "organisation", "vault_id": club, "text": "Meet on Fridays"})).await;

    // their personal space: entities, facts, a relation, a skill, a document (indexed by the worker)
    call(state, who, "memory_write", json!({"subject_name": "Dr Lee", "subject_kind": "person", "text": format!("My GP, {mark}")})).await;
    call(state, who, "memory_write", json!({"subject_name": "Clinic", "subject_kind": "organisation", "text": "Open late on Thursdays"})).await;
    call(state, who, "code_entity_upsert", json!({"kind": "repository", "name": format!("{mark}-repo")})).await;
    call(state, who, "skill_save", json!({"name": "private-runbook", "description": "steps", "body": format!("steps {mark}")})).await;
    let doc = call(state, who, "document_upload", json!({"filename": "notes.md", "text": format!("private notes {mark}")})).await;
    let doc_id = doc["id"].as_str().unwrap().to_string();
    let path: Vec<String> = db.test_raw().query("SELECT VALUE object_path FROM $d").bind(("d", rec(&doc_id))).await.unwrap().take(0).unwrap();
    // a document in the shared space (stays, reassigned with its file)
    call(state, who, "document_upload", json!({"filename": "team.md", "text": "team handbook", "vault_id": team})).await;

    // chat, an extension with a delivery, a session, more tokens
    let (s, b) = req(&app.router, "POST", "/api/chat/threads", Some(json!({"title": format!("{mark} thread")})), Some(&token), None).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    let thread = json_of(&b)["id"].as_str().unwrap().to_string();
    let (s, b) = req(&app.router, "POST", "/api/extensions", Some(json!({"name": format!("{mark} extension")})), Some(&token), None).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    let ext = json_of(&b);
    let key = ext["extension"]["webhook_url"].as_str().unwrap().rsplit('/').next().unwrap().to_string();
    let (s, _) = req(&app.router, "POST", &format!("/hooks/ext/{key}"), Some(json!({"note": mark})), Some(ext["secret"].as_str().unwrap()), None).await;
    assert_eq!(s, 202);
    login(&app.router, &who.email).await;

    // a synced record of theirs
    let env: eunomia_backend::cache::search::Envelope = serde_json::from_value(json!({
        "id": format!("demo:note:{mark}"), "source": "demo", "type": "note", "external_id": mark,
        "title": "a synced note", "body_text": format!("synced {mark}"), "payload": {"from": who.email}, "links": [],
    }))
    .unwrap();
    eunomia_backend::cache::search::upsert(&db, &who.id, &env).await.unwrap();

    // a recording with its audio in the bucket, and every table no tool plants
    let audio = format!("/recordings/{}/rec.mp3", who.id.key_str());
    let personal = eunomia_backend::vaults::service::personal_vault_id(&db, &who.id).await.unwrap();
    let enc = eunomia_backend::connectors::crypto::encrypt(&state.settings.encryption_key, "{\"token\":\"x\"}");
    let sql = "
        file::put(type::file('documents', $audio), <bytes>'audio');
        CREATE pocket_recording SET owner = $u, recording_id = 'rec', title = $c, transcript = $c, audio_path = $audio;
        CREATE chat_message SET owner = $u, thread_id = $thread, role = 'user', content = $c;
        UPSERT $settings SET owner = $u, observations_mission = $c;
        LET $conn = (CREATE ONLY connector SET owner = $u, kind = 'github', enabled = true, config = {note: $c}, credentials_encrypted = $enc).id;
        CREATE composio_file SET owner = $u, connector = $conn, external_id = $c, document_id = $c;
        UPSERT $sync SET owner = $u, cursor = $c;
        CREATE embed_cache SET text_hmac = $c, vector = [0.5, 0.25];
        CREATE peer_share SET vault = $pv, role = 'read', secret_enc = $c, created_by = $u, expires_at = time::now() + 1d;
        CREATE peer_link SET vault = $team, direction = 'host', role = 'read', key_enc = $c, user = $u, peer_instance = $c, peer_name = $c;
        CREATE change SET vault = $team, seq = 1000000, op = 'created', kind = 'memory', item = $c, actor = <string> $u;
        CREATE ontology_event SET vault = $team, seq = 1000000, actor = <string> $u, actor_type = 'user', actor_name = $email, action = 'rejected', definition_type = 'kind', kind = 'db', name = 'db', target = 'db', revision = 1;
        UPDATE $team SET ontology_actor = <string> $u, ontology_actor_name = $email;
        CREATE credential_ref SET reference = 'op://Ops/Item/field', provider = 'onepassword_connect', vault = $team, label = $c, created_by = $u;
        CREATE $sp SET kind = 'onepassword_connect', host = $c, token_enc = 'enc:v1:x', created_by = $u, updated_by = $u;
        CREATE secret_request SET vault = $team, name = 'asked', label = $c, description = $c, target_hosts = ['api.example.com'], requested_by = $u, actor_id = <string> $u, client_name = $c, expires_at = time::now() + 1d;
        CREATE vault_secret SET vault = $pv, name = 'held', label = $c, value_enc = $enc, allowed_hosts = ['api.example.com'], allowed_actors = [$c], created_by = $u;
        CREATE ontology_kind SET vault = $club, kind = 'held_kind', origin = 'human', status = 'approved', decided_by = $u, activity_actor = <string> $u, activity_actor_name = $email;";
    db.test_raw()
        .query(sql)
        .bind(("u", who.id.clone()))
        .bind(("c", mark.clone()))
        .bind(("email", who.email.clone()))
        .bind(("audio", audio.clone()))
        .bind(("enc", enc))
        .bind(("thread", eunomia_backend::rid::parse(&thread).unwrap()))
        .bind(("settings", RecordId::new("app_settings", who.id.key().clone())))
        .bind(("sync", RecordId::new("sync_status", format!("{}:github", who.id.key_str()))))
        .bind(("sp", RecordId::new("secret_provider", mark.clone())))
        .bind(("pv", personal))
        .bind(("team", eunomia_backend::rid::parse(&team).unwrap()))
        .bind(("club", eunomia_backend::rid::parse(&club).unwrap()))
        .await
        .unwrap()
        .check()
        .unwrap();

    // control: an identity, an audit event naming their email, usage, an OAuth grant with a token and a code, a capsule, a job,
    // two-step recovery codes and challenges (one of another account's, carrying their address for a pending link), the
    // instance setting they changed last
    jobs::enqueue(&state.control, NewJob::new("no_such_kind", who.id.clone(), format!("seed-{mark}")).in_org(who.org)).await.unwrap();
    let control_sql = "
        CREATE identity SET user = $u, issuer = 'https://idp.example', subject = $c, email = $email;
        CREATE audit_event SET user = $u, actor_kind = 'user', actor_id = <string> $u, action = 'vault.invite', target = $c, outcome = 'ok', detail = string::concat('email=', $email);
        CREATE audit_event SET user = $admin, actor_kind = 'user', actor_id = <string> $admin, action = 'admin.grant', target = <string> $u, outcome = 'ok', detail = string::concat('email=', $email);
        CREATE tool_usage SET org = $okey, bucket = time::now(), user = $u, actor_kind = 'user', actor_id = <string> $u, channel = 'mcp', tool = 'recall', calls = 1;
        LET $g = (CREATE oauth_grant SET owner = $u, client_id = 'client-x', client_name = $c, scope = ['memory:read'], resource = 'http://localhost:8001/mcp' RETURN VALUE id)[0];
        CREATE oauth_token SET kind = 'access', token_hash = $c, family = $g, expires_at = time::now() + 15m;
        CREATE oauth_code SET code_hash = $c, owner = $u, client_id = 'client-x', redirect_uri = 'http://localhost/cb', code_challenge = $c, scope = ['memory:read'], resource = 'http://x', expires_at = time::now() + 1m;
        CREATE failure_capsule SET kind = 'tool', name = 'recall', source = 'test', status = 500, code = 'internal', trace_id = $c, version = 'x', args = '{q: secret}', user = <string> $u;
        CREATE failure_capsule SET kind = 'tool', name = 'vault_invite', source = 'test', status = 500, code = 'internal', trace_id = 'x', version = 'x', args = string::concat('{email: ', string::uppercase($email), '}'), user = <string> $admin;
        CREATE mfa_recovery_code SET user = $u, code_hash = $c;
        CREATE mfa_challenge SET user = $u, token_hash = $c, purpose = 'verify', expires_at = time::now() + 5m;
        CREATE mfa_challenge SET user = $admin, token_hash = string::concat($c, '-link'), purpose = 'verify', link_issuer = 'https://idp.example', link_subject = $c, link_email = string::uppercase($email), expires_at = time::now() + 5m;
        UPSERT instance_setting:security SET updated_by = $u;";
    state
        .control
        .test_raw()
        .query(control_sql)
        .bind(("u", who.id.clone()))
        .bind(("admin", app.user.id.clone()))
        .bind(("c", mark.clone()))
        .bind(("email", who.email.clone()))
        .bind(("okey", who.org.to_string()))
        .await
        .unwrap()
        .check()
        .unwrap();
    // a colleague's own chat mentioning them: their content, counted for review, not deleted
    let owner_db = common::org_db(state, &app.user).await;
    owner_db
        .test_raw()
        .query("LET $t = (CREATE ONLY chat_thread SET owner = $o, title = 'x').id; CREATE chat_message SET owner = $o, thread_id = $t, role = 'user', content = string::concat('ask ', $e);")
        .bind(("o", app.user.id.clone()))
        .bind(("e", shouted.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();
    // an export of theirs, waiting (started from a browser session: tokens cannot)
    let cookie = login(&app.router, &who.email).await;
    assert_eq!(req(&app.router, "POST", "/api/export/subject", None, Some(&token), None).await.0, 403, "an API token cannot start an export");
    let (s, b) = req(&app.router, "POST", "/api/export/subject", None, None, Some(&cookie)).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));

    Seeded { team, club, files: vec![path[0].clone(), audio] }
}

/// Every row of every table, as text, that mentions one of `needles` (any case). `others_content`
/// tables may still hold the email in other people's own content (kept, counted for review): there
/// only the first needle (the key) counts.
async fn rows_naming(state: &AppState, org: &eunomia_backend::pool::OrgDb, needles: &[String], others_content: &[&str]) -> Vec<String> {
    let mut hits = Vec::new();
    for t in TENANT_TABLES.iter() {
        let rows: Vec<Value> = org.test_raw().query(format!("SELECT * OMIT embedding, archive FROM {t}")).await.unwrap().take(0).unwrap();
        let check: &[String] = if others_content.contains(t) { &needles[..1] } else { needles };
        for r in rows {
            let text = r.to_string().to_lowercase();
            if check.iter().any(|n| text.contains(&n.to_lowercase())) {
                hits.push(format!("{t}: {}", text.chars().take(300).collect::<String>()));
            }
        }
    }
    for t in CONTROL_TABLES {
        let rows: Vec<Value> = state.control.test_raw().query(format!("SELECT * FROM {t}")).await.unwrap().take(0).unwrap();
        for r in rows {
            let text = r.to_string().to_lowercase();
            // an event sealed into the audit chain keeps the ids its hash covers (of an account that is gone):
            // there an email counts anywhere, an id only in the detail
            let hit = if *t == "audit_event" && r["seq"].is_number() {
                let detail = r["detail"].as_str().unwrap_or_default().to_lowercase();
                needles.iter().any(|n| if n.contains('@') { text.contains(&n.to_lowercase()) } else { detail.contains(&n.to_lowercase()) })
            } else {
                needles.iter().any(|n| text.contains(&n.to_lowercase()))
            };
            if hit {
                hits.push(format!("control {t}: {}", text.chars().take(300).collect::<String>()));
            }
        }
    }
    hits
}

/// A record id from its `table:key` text (the hardened engine allows no `type::record`).
fn rec(s: &str) -> RecordId {
    eunomia_backend::rid::parse(s).unwrap_or_else(|_| panic!("not a record id: {s}"))
}

async fn erasure_status(state: &AppState, id: &str) -> Value {
    let rows: Vec<Value> = state.control.test_raw().query("SELECT * FROM $id").bind(("id", rec(id))).await.unwrap().take(0).unwrap();
    rows.into_iter().next().unwrap_or(Value::Null)
}

/// Every erasure test takes this: a few use `isolation::mutate`, which rewrites a statement for the
/// whole process, so they must not overlap with one another.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn erase_route(app: &TestApp, who: &User, close: bool) -> String {
    let cookie = login(&app.router, &who.email).await;
    let (s, b) = req(&app.router, "POST", "/api/account/erasure", Some(json!({"password": common::PASSWORD, "close_workspace": close})), None, Some(&cookie)).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    json_of(&b)["id"].as_str().unwrap().to_string()
}

async fn wait_status(app: &TestApp, id: &str, want: &str) -> Value {
    let start = Instant::now();
    loop {
        let r = erasure_status(&app.state, id).await;
        if r["status"] == want {
            return r;
        }
        assert!(start.elapsed() < Duration::from_secs(90), "erasure not {want}: {r}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn account_exists(app: &TestApp, u: &User) -> bool {
    !app.state.control.test_raw().query("SELECT VALUE id FROM user WHERE id = $u").bind(("u", u.id.clone())).await.unwrap().take::<Vec<RecordId>>(0).unwrap().is_empty()
}

#[tokio::test]
async fn erasure_removes_every_category_and_proves_it() {
    let _serial = SERIAL.lock().await;
    let (app, _stop) = app_with_worker().await;
    let alice = common::register(&app.state, "Alice@Example.com").await;
    let bob = common::register(&app.state, "bob@example.com").await;
    let seeded = seed(&app, &alice, &bob).await;
    let db = common::org_db(&app.state, &alice).await;
    let key = alice.id.key_str();
    let needles = vec![key.clone(), "alice@example.com".to_string()];
    assert!(!rows_naming(&app.state, &db, &needles, &[]).await.is_empty(), "the seed names the person");

    // a wrong password changes nothing; a token cannot ask at all
    let cookie = login(&app.router, &alice.email).await;
    let (s, b) = req(&app.router, "POST", "/api/account/erasure", Some(json!({"password": "not-it"})), None, Some(&cookie)).await;
    assert_eq!((s, json_of(&b)["code"].clone()), (403, json!("subject.confirmation_failed")));
    let tok = token_for(&app.state, &alice).await;
    assert_eq!(req(&app.router, "POST", "/api/account/erasure", Some(json!({"password": common::PASSWORD})), Some(&tok), None).await.0, 403);

    let id = erase_route(&app, &alice, false).await;
    assert_eq!(req(&app.router, "GET", "/api/auth/me", None, None, Some(&cookie)).await.0, 401, "signed out at once");
    let record = wait_status(&app, &id, "done").await;
    assert!(record.get("subject").is_none_or(Value::is_null), "the completion record drops the user id: {record}");
    assert_eq!(record["work"], json!({}), "and the working record with the email: {record}");
    let check = record["post_check"].as_object().unwrap();
    assert!(check.len() > 40, "the post-check covers every table: {record}");
    assert!(check.values().all(|v| v == 0), "{record}");
    assert!(record["counts"]["bucket_files"].as_i64().unwrap() >= 2, "{record}");
    assert_eq!(record["counts"]["ownership_handed_over"], 1, "{record}");
    assert!(record["counts"]["email_scrubbed"].as_i64().unwrap() >= 2, "the mixed-case mentions: {record}");
    assert_eq!(record["review"]["mentions_left_in_others_content"]["chat_messages"], 1, "a colleague's own chat is counted for review: {record}");

    // nothing names them, by key or email in any case (a colleague's own chat keeps its words); their files are gone
    let left = rows_naming(&app.state, &db, &needles, &["chat_message"]).await;
    assert!(left.is_empty(), "rows still naming the person:\n{}", left.join("\n"));
    for f in &seeded.files {
        assert!(!object_exists(&db, f).await, "{f} is still in the bucket");
    }
    assert!(!account_exists(&app, &alice).await);

    // the shared space keeps what they wrote, owned by someone who stays: facts, the relation, the document
    let team = seeded.team.clone();
    let kept: Vec<Value> = db.test_raw().query("SELECT text, <string> owner AS owner FROM memory WHERE vault = $v").bind(("v", rec(&team))).await.unwrap().take(0).unwrap();
    assert!(kept.iter().any(|m| m["text"] == "Plan agreed by the team" && m["owner"] == app.user.id.to_string()), "{kept:?}");
    assert!(kept.iter().any(|m| m["text"].as_str().unwrap().contains("[erased person]")), "{kept:?}");
    let rel: Vec<Value> = db.test_raw().query("SELECT <string> owner AS owner FROM relates_to WHERE label = 'buys from'").await.unwrap().take(0).unwrap();
    assert_eq!(rel, vec![json!({"owner": app.user.id.to_string()})], "the relation they wrote in the shared space is reassigned, not deleted");
    let names: Vec<String> = db.test_raw().query("SELECT VALUE name FROM person WHERE vault = $v").bind(("v", rec(&team))).await.unwrap().take(0).unwrap();
    assert!(names.iter().any(|n| n == "Billing desk [erased person]"), "{names:?}");
    let docs: Vec<Value> = db.test_raw().query("SELECT <string> owner AS owner, object_path FROM document WHERE vault = $v").bind(("v", rec(&team))).await.unwrap().take(0).unwrap();
    assert_eq!(docs.len(), 1, "{docs:?}");
    assert_eq!(docs[0]["owner"], app.user.id.to_string());
    assert!(object_exists(&db, docs[0]["object_path"].as_str().unwrap()).await);
    let roles: Vec<String> = db.test_raw().query("SELECT VALUE role FROM vault_member WHERE vault = $v AND user = $b").bind(("v", rec(&seeded.club))).bind(("b", bob.id.clone())).await.unwrap().take(0).unwrap();
    assert_eq!(roles, vec!["owner"]);

    // the workspace's activity log keeps who asked (the person only as the opaque reference) and the completion
    let cookie = app.session_cookie().await;
    let (_, b) = req(&app.router, "GET", "/api/admin/audit?category=admin", None, None, Some(&cookie)).await;
    let log = json_of(&b).to_string();
    assert!(log.contains("Someone asked for their own account and data to be deleted"), "{log}");
    assert!(log.contains("Deleted an account and its data"), "{log}");
    assert!(!log.to_lowercase().contains("alice@example.com"), "{log}");

    // everyone else still works
    assert_eq!(req(&app.router, "GET", "/api/auth/me", None, Some(&app.token), None).await.0, 200);
    login(&app.router, &bob.email).await;

    // resync suppression
    let org = app.state.org(&alice.org).await.unwrap();
    let report = eunomia_backend::sources::scheduler::sync_source(&org, &alice.id, "github", None).await;
    assert_eq!(report["status"], "erased", "{report}");

    // a restore of control and the org brings the account back: the leader's start-up step erases it again
    app.state
        .control
        .test_raw()
        .query("CREATE $u SET email = $e, email_lc = $e, password_hash = 'x'; CREATE membership SET user = $u, org = $o, role = 'member', slot = string::concat(<string> $o, '/', <string> $u);")
        .bind(("u", alice.id.clone()))
        .bind(("e", "alice@example.com"))
        .bind(("o", alice.org.record()))
        .await
        .unwrap()
        .check()
        .unwrap();
    db.test_raw().query("CREATE chat_thread SET owner = $u, title = 'restored'").bind(("u", alice.id.clone())).await.unwrap().check().unwrap();
    eunomia_backend::subject::reapply_on_start(&app.state).await;
    wait_for("the re-applied erasure", || async {
        control_rows(&app.state, "SELECT VALUE id FROM erasure WHERE requested_by = 'reapply' AND status = 'done'", ("x", json!(null))).await.len() == 1
    })
    .await;
    assert!(!account_exists(&app, &alice).await);

    // a restore of only the org database brings rows back under an id control no longer has: matched by reference
    db.test_raw()
        .query("CREATE chat_thread SET owner = $u, title = 'restored org only'; CREATE app_settings SET owner = $u;")
        .bind(("u", alice.id.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();
    eunomia_backend::subject::reapply_on_start(&app.state).await;
    wait_for("the org-only re-apply", || async {
        control_rows(&app.state, "SELECT VALUE id FROM erasure WHERE requested_by = 'reapply' AND status = 'done'", ("x", json!(null))).await.len() == 2
    })
    .await;
    let left = rows_naming(&app.state, &db, &needles[..1], &[]).await;
    assert!(left.is_empty(), "rows still naming the restored person:\n{}", left.join("\n"));
}

#[tokio::test]
async fn a_failing_post_check_keeps_the_account_and_every_retry_checks_again() {
    let _serial = SERIAL.lock().await;
    let (app, _stop) = app_with_worker().await;
    let carol = common::register(&app.state, "carol@example.com").await;
    let team = call(&app.state, &app.user, "vault_create", json!({"name": "Ops", "kind": "org"})).await["id"].as_str().unwrap().to_string();
    call(&app.state, &app.user, "memory_write", json!({"subject_name": "Rota", "subject_kind": "organisation", "vault_id": team, "text": "Call CAROL@Example.COM on Mondays"})).await;

    // the control removed: the case-insensitive scrub of facts does nothing
    eunomia_backend::isolation::mutate(
        "subject.scrub_email",
        "LET $m = (UPDATE memory SET text = string::replace(text, <regex> $email_re, '[erased person]'), embedding = NONE",
        "LET $m = (UPDATE memory SET text = text, embedding = NONE",
    );
    let id = erase_route(&app, &carol, false).await;
    let failed = wait_status(&app, &id, "failed").await;
    assert_eq!(failed["post_check"]["memory"], 1, "the first run finds the mixed-case mention: {failed}");
    assert!(failed["error"].as_str().unwrap().contains("memory"), "{failed}");
    // retries keep finding it with the captured email, and the account is never deleted while they do
    wait_for("a retry", || async {
        let jobs: Vec<i64> = app.state.control.test_raw().query("SELECT VALUE attempts FROM job WHERE kind = 'subject_erase'").await.unwrap().take(0).unwrap();
        jobs.iter().any(|a| *a >= 3)
    })
    .await;
    let again = erasure_status(&app.state, &id).await;
    assert_ne!(again["status"], "done", "done is never written while residue exists: {again}");
    assert!(again["work"]["email_lc"] == "carol@example.com", "the working record keeps the email for the retries: {again}");
    assert!(account_exists(&app, &carol).await, "the account is kept until every org post-check is clean");
    eunomia_backend::isolation::clear_mutations();

    // with the scrub back, the next retry passes
    eunomia_backend::subject::heal_open(&app.state).await.unwrap();
    let done = wait_status(&app, &id, "done").await;
    assert!(done["post_check"].as_object().unwrap().values().all(|v| v == 0), "{done}");
    let texts: Vec<String> = common::org_db(&app.state, &carol).await.test_raw().query("SELECT VALUE text FROM memory").await.unwrap().take(0).unwrap();
    assert!(texts.iter().all(|t| !t.to_lowercase().contains("carol@example.com")), "{texts:?}");
}

#[tokio::test]
async fn a_hold_placed_after_the_request_parks_the_erasure_until_it_is_lifted() {
    let _serial = SERIAL.lock().await;
    for removed in [false, true] {
        let app = TestApp::new().await;
        let dan = common::register(&app.state, "dan@example.com").await;
        call(&app.state, &dan, "memory_write", json!({"subject_name": "Me", "subject_kind": "person", "text": "kept while held"})).await;
        let id = erase_route(&app, &dan, false).await;
        let admin = app.session_cookie().await;
        let (s, b) = req(&app.router, "PUT", &format!("/api/admin/people/{}/legal-hold", dan.id.to_string()), Some(json!({"hold": true})), None, Some(&admin)).await;
        assert_eq!(s, 200);
        assert_eq!(json_of(&b)["erasure_open"], true, "the admin is told an erasure is open");
        if removed {
            // the control removed: the job no longer sees the hold
            eunomia_backend::isolation::mutate("subject.hold_of", "SELECT VALUE legal_hold_at FROM ONLY $u", "SELECT VALUE NONE FROM ONLY $u");
        }
        let (tx, rx) = tokio::sync::watch::channel(false);
        let cfg = WorkerConfig { id: "hold".into(), poll: Duration::from_millis(20), retry_base: Duration::from_millis(10), owner_cap: 1000, ..WorkerConfig::default() };
        tokio::spawn(worker::run(app.state.clone(), handlers::registry(), cfg, rx));
        if removed {
            let r = wait_status(&app, &id, "done").await;
            eunomia_backend::isolation::clear_mutations();
            assert_eq!(r["status"], "done", "without the check the held account is erased: the test below detects that");
            let _ = tx.send(true);
            continue;
        }
        let r = wait_status(&app, &id, "held").await;
        assert!(r["held_at"].is_string(), "{r}");
        assert!(account_exists(&app, &dan).await, "a held erasure deletes nothing");
        let texts: Vec<String> = common::org_db(&app.state, &dan).await.test_raw().query("SELECT VALUE text FROM memory").await.unwrap().take(0).unwrap();
        assert!(texts.contains(&"kept while held".to_string()));
        let (s, _) = req(&app.router, "PUT", &format!("/api/admin/people/{}/legal-hold", dan.id.to_string()), Some(json!({"hold": false})), None, Some(&admin)).await;
        assert_eq!(s, 200);
        wait_status(&app, &id, "done").await;
        assert!(!account_exists(&app, &dan).await);
        let _ = tx.send(true);
    }
}

#[tokio::test]
async fn two_erasures_in_one_workspace_keep_it_owned_and_never_hand_rows_to_each_other() {
    let _serial = SERIAL.lock().await;
    let (app, _stop) = app_with_worker().await;
    let dave = common::register(&app.state, "dave@example.com").await;
    let erin = common::register(&app.state, "erin@example.com").await;
    // a space only dave and erin share (dave owns it), and a team space the owner runs
    let pair = call(&app.state, &dave, "vault_create", json!({"name": "Pair", "kind": "org"})).await["id"].as_str().unwrap().to_string();
    call(&app.state, &dave, "vault_invite", json!({"vault_id": pair, "email": erin.email})).await;
    call(&app.state, &erin, "vault_accept_invitation", json!({"vault_id": pair})).await;
    call(&app.state, &dave, "memory_write", json!({"subject_name": "Plan", "subject_kind": "organisation", "vault_id": pair, "text": "dave's"})).await;
    call(&app.state, &erin, "memory_write", json!({"subject_name": "Plan", "subject_kind": "organisation", "vault_id": pair, "text": "erin's"})).await;
    let admin = app.session_cookie().await;
    let ask = |u: &User| {
        let path = format!("/api/admin/people/{}/erasure", u.id.to_string());
        let (router, admin) = (app.router.clone(), admin.clone());
        async move { req(&router, "POST", &path, Some(json!({"password": common::PASSWORD})), None, Some(&admin)).await }
    };
    // two at once, and the same person twice at once: one erasure per person
    let ((s1, b1), (s2, b2), (s3, b3)) = tokio::join!(ask(&dave), ask(&erin), ask(&dave));
    assert_eq!((s1, s2, s3), (200, 200, 200), "{} {} {}", String::from_utf8_lossy(&b1), String::from_utf8_lossy(&b2), String::from_utf8_lossy(&b3));
    assert_eq!(json_of(&b1)["id"], json!(json_of(&b3)["id"]), "two requests for one person make one erasure");
    let (d, e) = (json_of(&b1)["id"].as_str().unwrap().to_string(), json_of(&b2)["id"].as_str().unwrap().to_string());
    wait_status(&app, &d, "done").await;
    wait_status(&app, &e, "done").await;
    let db = common::org_db(&app.state, &app.user).await;
    let left = rows_naming(&app.state, &db, &[dave.id.key_str(), erin.id.key_str()], &[]).await;
    assert!(left.is_empty(), "no row is left owned by either, or handed from one to the other:\n{}", left.join("\n"));
    let pair_rows: Vec<Value> = db.test_raw().query("SELECT * FROM memory WHERE vault = $v").bind(("v", rec(&pair))).await.unwrap().take(0).unwrap();
    assert!(pair_rows.is_empty(), "the space only they read is gone with them: {pair_rows:?}");
    let owner: Vec<RecordId> = app.state.control.test_raw().query("SELECT VALUE user FROM membership WHERE org = $o AND role = 'owner'").bind(("o", app.user.org.record())).await.unwrap().take(0).unwrap();
    assert_eq!(owner, vec![app.user.id.clone()], "the workspace keeps its owner");
}

#[tokio::test]
async fn a_request_left_without_a_job_is_finished_by_the_next_request_or_the_next_start() {
    let _serial = SERIAL.lock().await;
    let (app, _stop) = app_with_worker().await;
    let frank = common::register(&app.state, "frank@example.com").await;
    let gina = common::register(&app.state, "gina@example.com").await;
    // a request that stopped half way: the row is there, no job (what a crash between the two used to leave)
    for (u, id) in [(&frank, "stuckf"), (&gina, "stuckg")] {
        let r = eunomia_backend::subject::subject_ref(&app.state.settings, &u.id);
        app.state
            .control
            .test_raw()
            .query("CREATE $k SET org = $o, subject_ref = $r, subject = $u, requested_by = 'self', status = 'pending', open_slot = $r")
            .bind(("k", RecordId::new("erasure", id)))
            .bind(("o", u.org.key()))
            .bind(("r", r))
            .bind(("u", u.id.clone()))
            .await
            .unwrap()
            .check()
            .unwrap();
    }
    // asking again joins the stuck request and gives it a job
    let again = erase_route(&app, &frank, false).await;
    assert_eq!(again, "erasure:stuckf");
    wait_status(&app, "erasure:stuckf", "done").await;
    // the start-up heal gives the other one its job
    eunomia_backend::subject::reapply_on_start(&app.state).await;
    wait_status(&app, "erasure:stuckg", "done").await;
    assert!(!account_exists(&app, &gina).await);
}

#[tokio::test]
async fn refusals_admin_reauthentication_and_closing_a_workspace() {
    let _serial = SERIAL.lock().await;
    let (app, _stop) = app_with_worker().await;
    let hal = common::register(&app.state, "hal@example.com").await;
    let admin = app.session_cookie().await;
    let path = format!("/api/admin/people/{}/erasure", hal.id.to_string());

    // an admin confirms with their own password: the person's email (visible on People) is not enough
    let (s, b) = req(&app.router, "POST", &path, Some(json!({"confirm_email": hal.email})), None, Some(&admin)).await;
    assert_eq!((s, json_of(&b)["code"].clone()), (403, json!("subject.confirmation_failed")));
    let (s, b) = req(&app.router, "POST", &path, Some(json!({"password": "wrong"})), None, Some(&admin)).await;
    assert_eq!((s, json_of(&b)["code"].clone()), (403, json!("subject.confirmation_failed")));
    assert_eq!(req(&app.router, "GET", "/api/auth/me", None, None, Some(&login(&app.router, &hal.email).await)).await.0, 200, "nothing changed");
    // the account that set up the server stays
    let (s, b) = req(&app.router, "POST", "/api/account/erasure", Some(json!({"password": common::PASSWORD})), None, Some(&admin)).await;
    assert_eq!((s, json_of(&b)["code"].clone()), (409, json!("subject.last_owner")));
    assert_eq!(req(&app.router, "POST", &format!("/api/admin/people/{}/erasure", app.user.id.to_string()), Some(json!({"password": common::PASSWORD})), None, Some(&admin)).await.0, 400);

    // alone in a workspace: refused until the person confirms the workspace closes with them, then it is emptied and closed
    let solo = common::register_personal(&app.state, "solo@example.com").await;
    call(&app.state, &solo, "memory_write", json!({"subject_name": "Diary", "subject_kind": "organisation", "text": "private"})).await;
    let doc = call(&app.state, &solo, "document_upload", json!({"filename": "a.md", "text": "a file"})).await;
    let solo_db = common::org_db(&app.state, &solo).await;
    let path_of: Vec<String> = solo_db.test_raw().query("SELECT VALUE object_path FROM $d").bind(("d", rec(doc["id"].as_str().unwrap()))).await.unwrap().take(0).unwrap();
    let cookie = login(&app.router, &solo.email).await;
    let (s, b) = req(&app.router, "POST", "/api/account/erasure", Some(json!({"password": common::PASSWORD})), None, Some(&cookie)).await;
    assert_eq!((s, json_of(&b)["code"].clone()), (409, json!("subject.workspace_closes")));
    assert!(json_of(&b)["detail"].as_str().unwrap().contains("also deletes the workspace"));
    let id = erase_route(&app, &solo, true).await;
    let r = wait_status(&app, &id, "done").await;
    assert_eq!(r["counts"]["workspaces_closed"], 1, "{r}");
    assert_eq!(r["post_check"]["closed_workspace_rows"], 0, "{r}");
    assert!(!object_exists(&solo_db, &path_of[0]).await, "its files are gone");
    assert_eq!(eunomia_backend::subject::erase::count_all_rows(&solo_db).await.unwrap(), 0, "every table of the workspace is empty");
    let status: Vec<String> = app.state.control.test_raw().query("SELECT VALUE status FROM $k").bind(("k", solo.org.tenant_record())).await.unwrap().take(0).unwrap();
    assert_eq!(status, vec!["closed"]);
    assert_eq!(app.state.org(&solo.org).await.err().map(|e| e.message), Some("This workspace was closed.".into()), "nothing is served from it again");
    // the next sign-up does not land in it
    let next = common::register(&app.state, "next@example.com").await;
    assert_ne!(next.org, solo.org);

    // owning a workspace that has other members: refused until ownership is handed over
    let boss = common::register_personal(&app.state, "boss@example.com").await;
    let ivy = common::register(&app.state, "ivy@example.com").await;
    app.state
        .control
        .test_raw()
        .query("CREATE membership SET user = $u, org = $o, role = 'member', slot = string::concat(<string> $o, '/', <string> $u)")
        .bind(("u", ivy.id.clone()))
        .bind(("o", boss.org.record()))
        .await
        .unwrap()
        .check()
        .unwrap();
    let cookie = login(&app.router, &boss.email).await;
    let (s, b) = req(&app.router, "POST", "/api/account/erasure", Some(json!({"password": common::PASSWORD, "close_workspace": true})), None, Some(&cookie)).await;
    assert_eq!((s, json_of(&b)["code"].clone()), (409, json!("subject.last_owner")));
    assert!(json_of(&b)["detail"].as_str().unwrap().contains("Hand the workspace over to one of them first"));
    eunomia_backend::subject::transfer_workspace(&app.state.control, &boss, &boss.org, &ivy.id).await.unwrap();
    let id = erase_route(&app, &boss, false).await;
    wait_status(&app, &id, "done").await;
    let owner: Vec<RecordId> = app.state.control.test_raw().query("SELECT VALUE user FROM membership WHERE org = $o AND role = 'owner'").bind(("o", boss.org.record())).await.unwrap().take(0).unwrap();
    assert_eq!(owner, vec![ivy.id.clone()]);

    // a legal hold refuses at request time
    let jo = common::register(&app.state, "jo@example.com").await;
    let (s, _) = req(&app.router, "PUT", &format!("/api/admin/people/{}/legal-hold", jo.id.to_string()), Some(json!({"hold": true})), None, Some(&admin)).await;
    assert_eq!(s, 200);
    let cookie = login(&app.router, &jo.email).await;
    let (s, b) = req(&app.router, "POST", "/api/account/erasure", Some(json!({"password": common::PASSWORD})), None, Some(&cookie)).await;
    assert_eq!((s, json_of(&b)["code"].clone()), (409, json!("subject.legal_hold")));
    let actions: Vec<String> = app.state.control.test_raw().query("SELECT VALUE action FROM audit_event WHERE action = 'subject.legal_hold'").await.unwrap().take(0).unwrap();
    assert_eq!(actions.len(), 1);
}

#[tokio::test]
async fn with_one_control_removed_the_detectors_go_red() {
    let _serial = SERIAL.lock().await;
    // relations in shared spaces: without them in the reassignment, the person's relation is deleted
    let (app, _stop) = app_with_worker().await;
    let kim = common::register(&app.state, "kim@example.com").await;
    let team = call(&app.state, &app.user, "vault_create", json!({"name": "Graph", "kind": "org"})).await["id"].as_str().unwrap().to_string();
    call(&app.state, &app.user, "vault_invite", json!({"vault_id": team, "email": kim.email})).await;
    call(&app.state, &kim, "vault_accept_invitation", json!({"vault_id": team})).await;
    call(&app.state, &app.user, "memory_write", json!({"subject_name": "Acme", "subject_kind": "organisation", "vault_id": team, "text": "x"})).await;
    call(&app.state, &app.user, "memory_write", json!({"subject_name": "Globex", "subject_kind": "organisation", "vault_id": team, "text": "y"})).await;
    let db = common::org_db(&app.state, &kim).await;
    db.test_raw()
        .query("LET $a = (SELECT VALUE id FROM organisation WHERE vault = $v AND name = 'Acme')[0]; LET $g = (SELECT VALUE id FROM organisation WHERE vault = $v AND name = 'Globex')[0]; RELATE $a->relates_to->$g SET owner = $u, label = 'only kim';")
        .bind(("v", rec(&team)))
        .bind(("u", kim.id.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();
    eunomia_backend::isolation::mutate("subject.vaults_with_rows", "(SELECT VALUE <string> in.vault FROM relates_to WHERE owner = $u AND in.vault IS NOT NONE)", "[]");
    eunomia_backend::isolation::mutate("subject.vaults_with_rows", "(SELECT VALUE <string> out.vault FROM relates_to WHERE owner = $u AND out.vault IS NOT NONE)", "[]");
    let id = erase_route(&app, &kim, false).await;
    wait_status(&app, &id, "done").await;
    eunomia_backend::isolation::clear_mutations();
    let rel: Vec<Value> = db.test_raw().query("SELECT * FROM relates_to WHERE label = 'only kim'").await.unwrap().take(0).unwrap();
    assert!(rel.is_empty(), "without the control the relation is deleted, which erasure_removes_every_category_and_proves_it detects");

    // the org-only restore sweep: without it, restored rows of an erased owner stay
    db.test_raw().query("CREATE chat_thread SET owner = $u, title = 'back'").bind(("u", kim.id.clone())).await.unwrap().check().unwrap();
    eunomia_backend::isolation::mutate("subject.row_owners", "(SELECT VALUE owner FROM chat_thread GROUP BY owner)", "[]");
    eunomia_backend::subject::reapply_on_start(&app.state).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    eunomia_backend::isolation::clear_mutations();
    let back: Vec<Value> = db.test_raw().query("SELECT * FROM chat_thread WHERE title = 'back'").await.unwrap().take(0).unwrap();
    assert_eq!(back.len(), 1, "without the sweep the restored row stays, which the restore step of erasure_removes_every_category_and_proves_it detects");
}

#[tokio::test]
async fn export_is_the_persons_covers_every_workspace_and_leaves_out_spaces_they_left() {
    let _serial = SERIAL.lock().await;
    let (app, _stop) = app_with_worker().await;
    let erin = common::register(&app.state, "erin@example.com").await;
    let token = token_for(&app.state, &erin).await;
    let cookie = login(&app.router, &erin.email).await;
    let team = call(&app.state, &app.user, "vault_create", json!({"name": "Team", "kind": "org"})).await["id"].as_str().unwrap().to_string();
    call(&app.state, &app.user, "vault_invite", json!({"vault_id": team, "email": erin.email})).await;
    call(&app.state, &erin, "vault_accept_invitation", json!({"vault_id": team})).await;
    call(&app.state, &erin, "memory_write", json!({"subject_name": "Roadmap", "subject_kind": "organisation", "vault_id": team, "text": "erin wrote this"})).await;
    call(&app.state, &app.user, "memory_write", json!({"subject_name": "Roadmap", "subject_kind": "organisation", "vault_id": team, "text": "owner wrote this"})).await;
    let gone = call(&app.state, &app.user, "vault_create", json!({"name": "Old", "kind": "org"})).await["id"].as_str().unwrap().to_string();
    call(&app.state, &app.user, "vault_invite", json!({"vault_id": gone, "email": erin.email})).await;
    call(&app.state, &erin, "vault_accept_invitation", json!({"vault_id": gone})).await;
    call(&app.state, &erin, "memory_write", json!({"subject_name": "Secret project", "subject_kind": "organisation", "vault_id": gone, "text": "from a space erin left"})).await;
    call(&app.state, &erin, "vault_leave", json!({"vault_id": gone})).await;
    call(&app.state, &erin, "memory_write", json!({"subject_name": "Me", "subject_kind": "person", "text": "personal fact"})).await;
    call(&app.state, &erin, "document_upload", json!({"filename": "cv.md", "text": "my cv"})).await;
    let db = common::org_db(&app.state, &erin).await;
    let (s, b) = req(&app.router, "POST", "/api/chat/threads", Some(json!({"title": "t"})), Some(&token), None).await;
    assert_eq!(s, 200);
    let thread = json_of(&b)["id"].as_str().unwrap().to_string();
    db.test_raw()
        .query("CREATE chat_message SET owner = $u, thread_id = $t, role = 'tool', content = string::concat('{\"vault\":\"', $g, '\",\"text\":\"Secret project\"}'); CREATE chat_message SET owner = $u, thread_id = $t, role = 'user', content = 'hello';")
        .bind(("u", erin.id.clone()))
        .bind(("t", rec(&thread)))
        .bind(("g", gone.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();
    app.state.control.test_raw().query("CREATE audit_event SET user = $u, actor_kind = 'user', action = 'vault.invite', target = 'x', outcome = 'ok', detail = 'email=someone.else@example.com'").bind(("u", erin.id.clone())).await.unwrap().check().unwrap();
    // erin is in a second workspace too, with a fact there
    let other = common::register_personal(&app.state, "other@example.com").await;
    let other_vault = call(&app.state, &other, "vault_create", json!({"name": "Theirs", "kind": "org"})).await["id"].as_str().unwrap().to_string();
    app.state
        .control
        .test_raw()
        .query("CREATE membership SET user = $u, org = $o, role = 'member', slot = string::concat(<string> $o, '/', <string> $u)")
        .bind(("u", erin.id.clone()))
        .bind(("o", other.org.record()))
        .await
        .unwrap()
        .check()
        .unwrap();
    // erin acting in that workspace (a credential lands in the home workspace, so the tools are called with it set)
    let erin_there = User { id: erin.id.clone(), email: erin.email.clone(), org: other.org };
    call(&app.state, &other, "vault_invite", json!({"vault_id": other_vault, "email": erin.email})).await;
    call(&app.state, &erin_there, "vault_accept_invitation", json!({"vault_id": other_vault})).await;
    call(&app.state, &erin_there, "memory_write", json!({"subject_name": "Elsewhere", "subject_kind": "organisation", "vault_id": other_vault, "text": "written in the second workspace"})).await;

    // a token cannot start or download; a browser session can
    assert_eq!(req(&app.router, "POST", "/api/export/subject", None, Some(&token), None).await.0, 403);
    let (s, b) = req(&app.router, "POST", "/api/export/subject", None, None, Some(&cookie)).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    let id = json_of(&b)["id"].as_str().unwrap().to_string();
    wait_for("the export", || async {
        let (_, b) = req(&app.router, "GET", "/api/export/subject", None, Some(&token), None).await;
        json_of(&b).as_array().unwrap().iter().any(|e| e["id"] == id.as_str() && e["status"] == "ready")
    })
    .await;
    assert_eq!(req(&app.router, "GET", &format!("/api/export/subject/{id}"), None, Some(&token), None).await.0, 403, "not with a token");
    let stranger = login(&app.router, &common::register(&app.state, "frank@example.com").await.email).await;
    assert_eq!(req(&app.router, "GET", &format!("/api/export/subject/{id}"), None, None, Some(&stranger)).await.0, 404);
    let (s, zip) = req(&app.router, "GET", &format!("/api/export/subject/{id}"), None, None, Some(&cookie)).await;
    assert_eq!(s, 200);
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip)).unwrap();
    let mut read = |name: &str| -> String {
        let mut s = String::new();
        archive.by_name(name).unwrap_or_else(|_| panic!("{name} in the archive")).read_to_string(&mut s).unwrap();
        s
    };
    let home = format!("workspaces/{}", erin.org.key());
    let second = format!("workspaces/{}", other.org.key());
    let manifest: Value = serde_json::from_str(&read("manifest.json")).unwrap();
    assert_eq!(manifest["workspaces"].as_array().unwrap().len(), 2, "every workspace the person belongs to: {manifest}");
    let names: Vec<&str> = manifest["categories"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    for want in ["account", "spaces", "entities", "memories", "relations", "skills", "documents", "synced_records", "chat", "tool_log", "sources", "recordings", "api_tokens", "sessions", "connected_apps", "audit_events", "usage"] {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }
    let memories = read(&format!("{home}/memories.json"));
    assert!(memories.contains("personal fact") && memories.contains("erin wrote this"), "{memories}");
    assert!(!memories.contains("owner wrote this"), "other members' facts are theirs");
    assert!(!memories.contains("from a space erin left"), "a space erin left is left out");
    assert!(read(&format!("{second}/memories.json")).contains("written in the second workspace"));
    let chat = read(&format!("{home}/chat.json"));
    assert!(!chat.contains("Secret project") && chat.contains("Left out of this export"), "{chat}");
    assert!(!read("audit_events.json").contains("someone.else@example.com"));
    let tokens = read("api_tokens.json");
    assert!(tokens.contains("erasable") && !tokens.contains("token_hash"), "{tokens}");
    assert_eq!(manifest["document_files"]["included"], 1, "{manifest}");

    // each workspace's part of the person's own export is stored in that workspace's own database
    let other_db = common::org_db(&app.state, &other).await;
    let parts: Vec<Value> = other_db.test_raw().query("SELECT VALUE <string> id FROM subject_export WHERE part = true").await.unwrap().take(0).unwrap();
    assert_eq!(parts, vec![json!(id)], "the second workspace's part is in the second workspace");
    let home_raw: Vec<surrealdb::types::Bytes> = db.test_raw().query("SELECT VALUE archive FROM $i").bind(("i", rec(&id))).await.unwrap().take(0).unwrap();
    assert!(!String::from_utf8_lossy(&home_raw[0].to_vec()).contains("written in the second workspace"), "the home workspace holds no other workspace's data");

    // an admin's export reads tokens, apps and audit events account-wide: while erin is in two
    // workspaces it is refused rather than show this workspace's admin the other's activity
    let admin = app.session_cookie().await;
    let (s, b) = req(&app.router, "POST", &format!("/api/admin/people/{}/export", erin.id.to_string()), None, None, Some(&admin)).await;
    assert_eq!((s, json_of(&b)["code"].clone()), (403, json!("auth.forbidden")), "an account in two workspaces: {}", String::from_utf8_lossy(&b));
    app.state.control.test_raw().query("DELETE membership WHERE user = $u AND org = $o").bind(("u", erin.id.clone())).bind(("o", other.org.record())).await.unwrap().check().unwrap();

    // an admin's export: only this workspace's data about the person, for the admin
    let (s, b) = req(&app.router, "POST", &format!("/api/admin/people/{}/export", erin.id.to_string()), None, None, Some(&admin)).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    let by_admin = json_of(&b)["id"].as_str().unwrap().to_string();
    assert_eq!(json_of(&b)["scope"], "organisation");
    wait_for("the admin's export", || async {
        let (_, b) = req(&app.router, "GET", "/api/export/subject", None, None, Some(&admin)).await;
        json_of(&b).as_array().unwrap().iter().any(|e| e["id"] == by_admin.as_str() && e["status"] == "ready")
    })
    .await;
    let (s, zip) = req(&app.router, "GET", &format!("/api/export/subject/{by_admin}"), None, None, Some(&admin)).await;
    assert_eq!(s, 200);
    let text = String::from_utf8_lossy(&zip).to_string();
    assert!(text.contains("erin wrote this"), "what they wrote in the workspace's shared spaces");
    // ("Secret project" is something they wrote in this workspace's shared space "Old": the workspace's, so included)
    for private in ["personal fact", "hello", "written in the second workspace", "my cv"] {
        assert!(!text.contains(private), "{private} is not the workspace's to export");
    }
    assert_eq!(req(&app.router, "GET", &format!("/api/export/subject/{by_admin}"), None, None, Some(&cookie)).await.0, 404, "the workspace's export is the admin's");
    // the person sees in their own activity that an admin exported their data
    let (s, b) = req(&app.router, "GET", "/api/account/activity", None, None, Some(&cookie)).await;
    assert_eq!(s, 200);
    assert!(json_of(&b).as_array().unwrap().iter().any(|a| a["action"] == "subject.export_request_admin"), "{}", json_of(&b));
    assert!(json_of(&b).as_array().unwrap().iter().any(|a| a["action"] == "subject.export_download_admin"), "and that the admin downloaded it: {}", json_of(&b));
    // suspend, try to download the person's own export, restore: nothing personal comes out, ever
    let (s, _) = req(&app.router, "PUT", &format!("/api/admin/people/{}/suspended", erin.id.to_string()), Some(json!({"suspended": true})), None, Some(&admin)).await;
    assert_eq!(s, 200);
    assert_eq!(req(&app.router, "GET", &format!("/api/export/subject/{id}"), None, None, Some(&admin)).await.0, 404);
    let (s, _) = req(&app.router, "PUT", &format!("/api/admin/people/{}/suspended", erin.id.to_string()), Some(json!({"suspended": false})), None, Some(&admin)).await;
    assert_eq!(s, 200);
}

/// Admin of workspace A and a person who is in A and in B (their own workspace).
async fn two_workspaces(app: &TestApp, name: &str) -> (User, User, User) {
    let person = common::register(&app.state, &format!("{name}@example.com")).await;
    let b_owner = common::register_personal(&app.state, &format!("{name}-b@example.com")).await;
    let in_b = User { id: person.id.clone(), email: person.email.clone(), org: b_owner.org };
    app.state
        .control
        .test_raw()
        .query("CREATE membership SET user = $u, org = $o, role = 'member', slot = string::concat(<string> $o, '/', <string> $u)")
        .bind(("u", person.id.clone()))
        .bind(("o", b_owner.org.record()))
        .await
        .unwrap()
        .check()
        .unwrap();
    let b_vault = call(&app.state, &b_owner, "vault_create", json!({"name": "B team", "kind": "org"})).await["id"].as_str().unwrap().to_string();
    call(&app.state, &b_owner, "vault_invite", json!({"vault_id": b_vault, "email": person.email})).await;
    call(&app.state, &in_b, "vault_accept_invitation", json!({"vault_id": b_vault})).await;
    call(&app.state, &in_b, "memory_write", json!({"subject_name": "B plan", "subject_kind": "organisation", "vault_id": b_vault, "text": "written in B"})).await;
    call(&app.state, &person, "memory_write", json!({"subject_name": "A note", "subject_kind": "organisation", "text": "written in A"})).await;
    (person, b_owner, in_b)
}

/// A member of the tester's workspace made an admin in the app: an admin, but not a server operator
/// (the tester is the server's first account, so the tester is the operator).
async fn granted_admin(app: &TestApp, name: &str) -> (User, String) {
    unsafe { std::env::remove_var("EUNOMIA_ADMIN_EMAILS") };
    let u = common::register(&app.state, &format!("{name}@example.com")).await;
    let owner = app.session_cookie().await;
    let (s, _) = req(&app.router, "PUT", &format!("/api/admin/people/{}/admin", u.id.to_string()), Some(json!({"admin": true})), None, Some(&owner)).await;
    assert_eq!(s, 200);
    let cookie = login(&app.router, &u.email).await;
    (u, cookie)
}

async fn texts(db: &eunomia_backend::pool::OrgDb) -> Vec<String> {
    db.test_raw().query("SELECT VALUE text FROM memory").await.unwrap().take(0).unwrap()
}

#[tokio::test]
async fn only_an_operator_erases_someone_and_always_the_whole_account() {
    let _serial = SERIAL.lock().await;
    let (app, _stop) = app_with_worker().await;
    let (m, b_owner, _) = two_workspaces(&app, "mira").await;
    let (_ray, ray) = granted_admin(&app, "ray").await;
    let path = format!("/api/admin/people/{}/erasure", m.id.to_string());
    // an in-app admin who is not an operator erases nobody, in any form
    for body in [json!({"password": common::PASSWORD}), json!({"password": common::PASSWORD, "scope": "workspace"}), json!({"password": common::PASSWORD, "close_workspace": true})] {
        let (s, b) = req(&app.router, "POST", &path, Some(body), None, Some(&ray)).await;
        assert_eq!((s, json_of(&b)["code"].clone()), (403, json!("auth.forbidden")));
    }
    assert!(account_exists(&app, &m).await);
    assert!(texts(&common::org_db(&app.state, &app.user).await).await.contains(&"written in A".to_string()), "nothing was deleted");

    // the operator (the server's first account here) erases the whole account, in every workspace
    let operator = app.session_cookie().await;
    let (s, b) = req(&app.router, "POST", &path, Some(json!({"password": common::PASSWORD})), None, Some(&operator)).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    wait_status(&app, json_of(&b)["id"].as_str().unwrap(), "done").await;
    assert!(!account_exists(&app, &m).await);
    assert!(!texts(&common::org_db(&app.state, &app.user).await).await.contains(&"written in A".to_string()));
    // what they wrote in B's shared space stays, owned by B's owner (the shared-space rule)
    let owners: Vec<String> = common::org_db(&app.state, &b_owner).await.test_raw().query("SELECT VALUE <string> owner FROM memory WHERE text = 'written in B'").await.unwrap().take(0).unwrap();
    assert_eq!(owners, vec![b_owner.id.to_string()]);

    // an operator's account (EUNOMIA_ADMIN_EMAILS) is erased by nobody but its owner, not even another operator
    let olga = common::register(&app.state, "olga@example.com").await;
    unsafe { std::env::set_var("EUNOMIA_ADMIN_EMAILS", &olga.email) };
    let (s, b) = req(&app.router, "POST", &format!("/api/admin/people/{}/erasure", olga.id.to_string()), Some(json!({"password": common::PASSWORD})), None, Some(&operator)).await;
    unsafe { std::env::remove_var("EUNOMIA_ADMIN_EMAILS") };
    assert_eq!((s, json_of(&b)["code"].clone()), (403, json!("auth.forbidden")));
    assert!(account_exists(&app, &olga).await);
}

#[tokio::test]
async fn an_admin_of_one_workspace_cannot_reach_another_through_any_erasure_route() {
    let _serial = SERIAL.lock().await;
    let app = TestApp::new().await;
    let other = common::register_personal(&app.state, "other-owner@example.com").await;
    call(&app.state, &other, "memory_write", json!({"subject_name": "Theirs", "subject_kind": "organisation", "text": "B only"})).await;
    // an admin of A who is not a server operator
    let (_ray, admin) = granted_admin(&app, "ray").await;
    let base = format!("/api/admin/people/{}", other.id.to_string());
    for (method, path, body, want) in [
        ("POST", format!("{base}/erasure"), json!({"password": common::PASSWORD}), 403),
        ("PUT", format!("{base}/legal-hold"), json!({"hold": true}), 404),
        ("POST", format!("{base}/export"), json!({}), 404),
        ("PUT", format!("{base}/workspace-owner"), json!({}), 404),
    ] {
        let (s, b) = req(&app.router, method, &path, Some(body), None, Some(&admin)).await;
        assert_eq!(s, want, "{method} {path}: {}", String::from_utf8_lossy(&b));
    }
    // nor cancel or re-apply an erasure: those are the operator's
    let r = eunomia_backend::subject::subject_ref(&app.state.settings, &other.id);
    app.state.control.test_raw()
        .query("CREATE erasure:inb SET org = $o, subject_ref = $r, subject = $u, requested_by = 'self', status = 'held', open_slot = $r")
        .bind(("o", other.org.key())).bind(("r", r)).bind(("u", other.id.clone())).await.unwrap().check().unwrap();
    let (s, _) = req(&app.router, "POST", "/api/admin/erasures/erasure:inb/cancel", None, None, Some(&admin)).await;
    assert_eq!(s, 403);
    assert_eq!(req(&app.router, "POST", "/api/admin/erasures/reapply", None, None, Some(&admin)).await.0, 403);
    assert_eq!(erasure_status(&app.state, "erasure:inb").await["status"], "held");
    assert!(account_exists(&app, &other).await);
    assert!(texts(&common::org_db(&app.state, &other).await).await.contains(&"B only".to_string()));
    let held: Vec<Value> = app.state.control.test_raw().query("SELECT VALUE legal_hold_at FROM $u").bind(("u", other.id.clone())).await.unwrap().take(0).unwrap();
    assert!(held.iter().all(Value::is_null), "{held:?}");
}

#[tokio::test]
async fn a_held_erasure_keeps_the_account_suspended_and_resumes_from_live_state() {
    let _serial = SERIAL.lock().await;
    let app = TestApp::new().await;
    let (pia, b_owner, _) = two_workspaces(&app, "pia").await;
    let id = erase_route(&app, &pia, false).await;
    let admin = app.session_cookie().await;
    let path = format!("/api/admin/people/{}", pia.id.to_string());
    assert_eq!(req(&app.router, "PUT", &format!("{path}/legal-hold"), Some(json!({"hold": true})), None, Some(&admin)).await.0, 200);
    let (tx, rx) = tokio::sync::watch::channel(false);
    let cfg = WorkerConfig { id: "resume".into(), poll: Duration::from_millis(20), retry_base: Duration::from_millis(10), owner_cap: 1000, ..WorkerConfig::default() };
    tokio::spawn(worker::run(app.state.clone(), handlers::registry(), cfg, rx));
    wait_status(&app, &id, "held").await;
    // access cannot be restored while the erasure is open
    let (s, b) = req(&app.router, "PUT", &format!("{path}/suspended"), Some(json!({"suspended": false})), None, Some(&admin)).await;
    assert_eq!((s, json_of(&b)["code"].clone()), (403, json!("auth.forbidden")));
    // meanwhile the account gains a token, a file and a workspace (an operator's import, say)
    let tok = models_user::create_api_token_with(&app.state.control, &pia.id, "late", &["memory:read".to_string()], None, None).await.unwrap();
    let late = tok.id.to_string();
    app.state.control.test_raw().query("CREATE audit_event SET user = $a, actor_kind = 'token', actor_id = $t, action = 'vault.create', target = 'x', outcome = 'ok'")
        .bind(("a", app.user.id.clone())).bind(("t", late.clone())).await.unwrap().check().unwrap();
    let a_db = common::org_db(&app.state, &app.user).await;
    a_db.test_raw().query("file::put(type::file('documents', '/late/file.txt'), <bytes>'late'); CREATE pocket_recording SET owner = $u, recording_id = 'late', title = 'late', audio_path = '/late/file.txt';")
        .bind(("u", pia.id.clone())).await.unwrap().check().unwrap();
    let c_owner = common::register_personal(&app.state, "pia-c@example.com").await;
    app.state.control.test_raw().query("CREATE membership SET user = $u, org = $o, role = 'member', slot = string::concat(<string> $o, '/', <string> $u)")
        .bind(("u", pia.id.clone())).bind(("o", c_owner.org.record())).await.unwrap().check().unwrap();
    common::org_db(&app.state, &c_owner).await.test_raw().query("CREATE chat_thread SET owner = $u, title = 'joined late'").bind(("u", pia.id.clone())).await.unwrap().check().unwrap();
    // as if it had parked after capturing: a saved working record that knows none of that
    app.state.control.test_raw()
        .query("UPDATE $e SET work = { captured: true, email_lc: 'pia@example.com', actors: [<string> $u], key: $k, \
                orgs: [{ org: $a, heir: <string> $h, close: false, transfer: false, solo: [], kept: [], member_of: [], files: [] }] }")
        .bind(("e", rec(&id))).bind(("u", pia.id.clone())).bind(("k", pia.id.key_str())).bind(("a", app.user.org.key())).bind(("h", app.user.id.clone()))
        .await.unwrap().check().unwrap();
    // the operator lifts the hold: the job rebuilds its plan from live state before doing anything
    assert_eq!(req(&app.router, "PUT", &format!("{path}/legal-hold"), Some(json!({"hold": false})), None, Some(&admin)).await.0, 200);
    let r = wait_status(&app, &id, "done").await;
    assert!(r["post_check"].as_object().unwrap().values().all(|v| v == 0), "{r}");
    assert!(!object_exists(&a_db, "/late/file.txt").await, "the file added while held is gone");
    let actors: Vec<String> = app.state.control.test_raw().query("SELECT VALUE actor_id FROM audit_event WHERE action = 'vault.create' AND target = 'x'").await.unwrap().take(0).unwrap();
    assert_eq!(actors, vec!["erased"], "the token made while held is no longer named");
    let left: Vec<Value> = common::org_db(&app.state, &c_owner).await.test_raw().query("SELECT * FROM chat_thread WHERE owner = $u").bind(("u", pia.id.clone())).await.unwrap().take(0).unwrap();
    assert!(left.is_empty(), "the workspace joined while held is covered: {left:?}");
    let owners: Vec<String> = common::org_db(&app.state, &b_owner).await.test_raw().query("SELECT VALUE <string> owner FROM memory WHERE text = 'written in B'").await.unwrap().take(0).unwrap();
    assert_eq!(owners, vec![b_owner.id.to_string()], "what they wrote in B stays, owned by B's owner");
    let _ = tx.send(true);
}

#[tokio::test]
async fn ownership_hand_over_holds_and_space_names_and_the_change_log() {
    let _serial = SERIAL.lock().await;
    let (app, _stop) = app_with_worker().await;
    let admin = app.session_cookie().await;
    let quinn = common::register(&app.state, "quinn@example.com").await;
    let ray = common::register(&app.state, "ray@example.com").await;
    // a second in-app admin who is not the owner can neither hand the workspace over nor lift a hold
    unsafe { std::env::remove_var("EUNOMIA_ADMIN_EMAILS") };
    let (s, _) = req(&app.router, "PUT", &format!("/api/admin/people/{}/admin", ray.id.to_string()), Some(json!({"admin": true})), None, Some(&admin)).await;
    assert_eq!(s, 200);
    let ray_cookie = login(&app.router, &ray.email).await;
    let (s, _) = req(&app.router, "PUT", &format!("/api/admin/people/{}/workspace-owner", ray.id.to_string()), None, None, Some(&ray_cookie)).await;
    assert_eq!(s, 403, "an admin cannot take ownership");
    let (s, _) = req(&app.router, "PUT", &format!("/api/admin/people/{}/legal-hold", quinn.id.to_string()), Some(json!({"hold": true})), None, Some(&ray_cookie)).await;
    assert_eq!(s, 200, "any admin places a hold");
    let by: Vec<Option<RecordId>> = app.state.control.test_raw().query("SELECT VALUE legal_hold_by FROM $u").bind(("u", quinn.id.clone())).await.unwrap().take(0).unwrap();
    assert_eq!(by, vec![Some(ray.id.clone())], "who placed it is recorded");
    let (s, b) = req(&app.router, "PUT", &format!("/api/admin/people/{}/legal-hold", quinn.id.to_string()), Some(json!({"hold": false})), None, Some(&ray_cookie)).await;
    assert_eq!((s, json_of(&b)["code"].clone()), (403, json!("auth.forbidden")), "only an operator lifts it");
    // the owner cannot hand over to someone held, suspended or being erased
    let (s, _) = req(&app.router, "PUT", &format!("/api/admin/people/{}/workspace-owner", quinn.id.to_string()), None, None, Some(&admin)).await;
    assert_eq!(s, 400, "held");
    let (s, _) = req(&app.router, "PUT", &format!("/api/admin/people/{}/legal-hold", quinn.id.to_string()), Some(json!({"hold": false})), None, Some(&admin)).await;
    assert_eq!(s, 200, "the operator lifts it");
    let lift: Vec<String> = app.state.control.test_raw().query("SELECT VALUE detail FROM audit_event WHERE action = 'subject.legal_hold_lift'").await.unwrap().take(0).unwrap();
    assert!(lift[0].contains("placed_by=ray@example.com") && lift[0].contains("by=tester@example.com"), "{lift:?}");
    let (s, b) = req(&app.router, "PUT", &format!("/api/admin/people/{}/workspace-owner", quinn.id.to_string()), None, None, Some(&admin)).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    let owners: Vec<RecordId> = app.state.control.test_raw().query("SELECT VALUE user FROM membership WHERE org = $o AND role = 'owner'").bind(("o", app.user.org.record())).await.unwrap().take(0).unwrap();
    assert_eq!(owners, vec![quinn.id.clone()], "exactly one owner, the new one");
    let t: Vec<String> = app.state.control.test_raw().query("SELECT VALUE detail FROM audit_event WHERE action = 'org.owner_transfer'").await.unwrap().take(0).unwrap();
    assert!(t.len() == 1 && t[0].contains("from=tester@example.com"), "the owner handed it over themselves: one row naming the previous owner: {t:?}");
    eunomia_backend::subject::transfer_workspace(&app.state.control, &quinn, &app.user.org, &app.user.id).await.unwrap();

    // a shared space named with the person's email: scrubbed (it stays), so the erasure finishes; an
    // edit the scrub makes is in the change log as `updated`
    let sam = common::register(&app.state, "sam@example.com").await;
    let named = call(&app.state, &app.user, "vault_create", json!({"name": "Handover sam@example.com", "kind": "org"})).await["id"].as_str().unwrap().to_string();
    call(&app.state, &app.user, "vault_invite", json!({"vault_id": named, "email": sam.email})).await;
    call(&app.state, &sam, "vault_accept_invitation", json!({"vault_id": named})).await;
    call(&app.state, &app.user, "memory_write", json!({"subject_name": "Desk", "subject_kind": "organisation", "vault_id": named, "text": "ask SAM@example.com"})).await;
    let elsewhere = call(&app.state, &app.user, "vault_create", json!({"name": "Notes on sam@example.com", "kind": "org"})).await["id"].as_str().unwrap().to_string();
    let id = erase_route(&app, &sam, false).await;
    let r = wait_status(&app, &id, "done").await;
    let db = common::org_db(&app.state, &app.user).await;
    let names: Vec<String> = db.test_raw().query("SELECT VALUE name FROM $v").bind(("v", rec(&named))).await.unwrap().take(0).unwrap();
    assert_eq!(names, vec!["Handover [erased person]"]);
    assert_eq!(r["review"]["mentions_left_in_others_content"]["space_names"], 1, "a space the person was not in is someone else's, counted: {r}");
    let _ = elsewhere;
    let ops: Vec<String> = db.test_raw().query("SELECT VALUE op FROM change WHERE vault = $v AND kind = 'memory'").bind(("v", rec(&named))).await.unwrap().take(0).unwrap();
    assert!(ops.contains(&"updated".to_string()), "the scrubbed fact is announced to sync clients: {ops:?}");

    // the subject of a hold never lifts it, even an operator: ray holds the operator's own account
    let (s, _) = req(&app.router, "PUT", &format!("/api/admin/people/{}/legal-hold", app.user.id.to_string()), Some(json!({"hold": true})), None, Some(&ray_cookie)).await;
    assert_eq!(s, 200);
    let (s, b) = req(&app.router, "PUT", &format!("/api/admin/people/{}/legal-hold", app.user.id.to_string()), Some(json!({"hold": false})), None, Some(&admin)).await;
    assert_eq!((s, json_of(&b)["code"].clone()), (403, json!("auth.forbidden")), "an operator cannot lift a hold on themselves");
    let held: Vec<Value> = app.state.control.test_raw().query("SELECT VALUE legal_hold_at FROM $u").bind(("u", app.user.id.clone())).await.unwrap().take(0).unwrap();
    assert!(held[0].is_string(), "{held:?}");
}

#[tokio::test]
async fn members_erasing_at_once_finish_and_the_lock_covers_every_workspace_touched() {
    let _serial = SERIAL.lock().await;
    let app = TestApp::new().await;
    // (a) the only two members of a workspace ask at once: the last of them closes it
    let x = common::register_personal(&app.state, "xia@example.com").await;
    let y = common::register(&app.state, "yan@example.com").await;
    app.state.control.test_raw().query("CREATE membership SET user = $u, org = $o, role = 'member', slot = string::concat(<string> $o, '/', <string> $u)")
        .bind(("u", y.id.clone())).bind(("o", x.org.record())).await.unwrap().check().unwrap();
    // y (a member) asks, then x (the owner): x's request is allowed because everyone left is being erased
    let ey = common::sys(eunomia_backend::subject::request_erasure(&app.state, &y.id, eunomia_backend::subject::Requester::Myself, false)).await.unwrap();
    let ex = common::sys(eunomia_backend::subject::request_erasure(&app.state, &x.id, eunomia_backend::subject::Requester::Myself, false)).await.unwrap();
    // the lock: an erasure running elsewhere holds x's workspace, so neither may start there
    app.state.control.test_raw().query("CREATE $k SET erasure = erasure:elsewhere").bind(("k", RecordId::new("erasure_lock", x.org.key()))).await.unwrap().check().unwrap();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let cfg = WorkerConfig { id: "joint".into(), poll: Duration::from_millis(20), retry_base: Duration::from_millis(10), owner_cap: 1000, ..WorkerConfig::default() };
    tokio::spawn(worker::run(app.state.clone(), handlers::registry(), cfg, rx));
    tokio::time::sleep(Duration::from_secs(4)).await;
    let x_now = erasure_status(&app.state, &ex.id).await;
    assert_eq!(x_now["status"], "pending", "a workspace another erasure holds is waited for: {x_now}");
    assert!(account_exists(&app, &x).await);
    app.state.control.test_raw().query("DELETE $k").bind(("k", RecordId::new("erasure_lock", x.org.key()))).await.unwrap().check().unwrap();
    wait_status(&app, &ex.id, "done").await;
    wait_status(&app, &ey.id, "done").await;
    let status: Vec<String> = app.state.control.test_raw().query("SELECT VALUE status FROM $k").bind(("k", x.org.tenant_record())).await.unwrap().take(0).unwrap();
    assert_eq!(status, vec!["closed"], "the last of them closed the workspace");

    // (b) a confirmation that the workspace closes can be added by a repeat request: z is a member of
    // w's workspace; w goes before z's job runs, so z is now alone in it
    let w = common::register_personal(&app.state, "wen@example.com").await;
    let z = common::register(&app.state, "zed@example.com").await;
    app.state.control.test_raw().query("CREATE membership SET user = $u, org = $o, role = 'member', slot = string::concat(<string> $o, '/', <string> $u)")
        .bind(("u", z.id.clone())).bind(("o", w.org.record())).await.unwrap().check().unwrap();
    let _ = tx.send(true);
    let ez = common::sys(eunomia_backend::subject::request_erasure(&app.state, &z.id, eunomia_backend::subject::Requester::Myself, false)).await.unwrap();
    // the other member (its owner) leaves before the job runs: now it would close, which z did not confirm
    app.state.control.test_raw().query("DELETE membership WHERE user = $u AND org = $o").bind(("u", w.id.clone())).bind(("o", w.org.record())).await.unwrap().check().unwrap();
    let again = common::sys(eunomia_backend::subject::request_erasure(&app.state, &z.id, eunomia_backend::subject::Requester::Operator(app.user.id.clone()), true)).await.unwrap();
    assert_eq!(again.id, ez.id);
    let closes: Vec<Vec<String>> = app.state.control.test_raw().query("SELECT VALUE closes FROM $e").bind(("e", rec(&ez.id))).await.unwrap().take(0).unwrap();
    assert_eq!(closes, vec![vec![w.org.key()]], "the repeat request added the confirmation");
    let (tx, rx) = tokio::sync::watch::channel(false);
    let cfg = WorkerConfig { id: "repeat".into(), poll: Duration::from_millis(20), retry_base: Duration::from_millis(10), owner_cap: 1000, ..WorkerConfig::default() };
    tokio::spawn(worker::run(app.state.clone(), handlers::registry(), cfg, rx));
    wait_status(&app, &ez.id, "done").await;
    let status: Vec<String> = app.state.control.test_raw().query("SELECT VALUE status FROM $k").bind(("k", w.org.tenant_record())).await.unwrap().take(0).unwrap();
    assert_eq!(status, vec!["closed"]);
    let _ = tx.send(true);
}

#[tokio::test]
async fn single_sign_on_needs_a_recent_sign_in_and_an_operator_can_cancel() {
    let _serial = SERIAL.lock().await;
    let app = TestApp::new().await;
    let uma = common::register(&app.state, "uma@example.com").await;
    app.state.control.test_raw().query("UPDATE $u SET password_hash = NONE").bind(("u", uma.id.clone())).await.unwrap().check().unwrap();
    let token = eunomia_backend::auth::start_session(&app.state.control, &app.state.settings.jwt_secret, &uma, None, eunomia_backend::auth::Via::Oidc, false).await.unwrap();
    let cookie = format!("{}={token}", eunomia_backend::auth::SESSION_COOKIE);
    // a session signed in an hour ago is not proof
    app.state.control.test_raw().query("UPDATE session SET created_at = time::now() - 1h WHERE owner = $u").bind(("u", uma.id.clone())).await.unwrap().check().unwrap();
    let (s, b) = req(&app.router, "POST", "/api/account/erasure", Some(json!({})), None, Some(&cookie)).await;
    assert_eq!((s, json_of(&b)["code"].clone()), (403, json!("subject.reauthenticate")));
    // a fresh sign-in is
    let token = eunomia_backend::auth::start_session(&app.state.control, &app.state.settings.jwt_secret, &uma, None, eunomia_backend::auth::Via::Oidc, false).await.unwrap();
    let cookie = format!("{}={token}", eunomia_backend::auth::SESSION_COOKIE);
    let (s, b) = req(&app.router, "POST", "/api/account/erasure", Some(json!({})), None, Some(&cookie)).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    let id = json_of(&b)["id"].as_str().unwrap().to_string();
    // nobody but an operator cancels
    let (_ray, ray) = granted_admin(&app, "ray").await;
    assert_eq!(req(&app.router, "POST", &format!("/api/admin/erasures/{id}/cancel"), None, None, Some(&ray)).await.0, 403);
    // the operator cancels before the first deletion: the working record is cleared and the account is
    // exactly as it was before the request (no longer suspended)
    app.state.control.test_raw().query("UPDATE $e SET work = { email_lc: 'uma@example.com', captured: true }").bind(("e", rec(&id))).await.unwrap().check().unwrap();
    let operator = app.session_cookie().await;
    let (s, _) = req(&app.router, "POST", &format!("/api/admin/erasures/{id}/cancel"), None, None, Some(&operator)).await;
    assert_eq!(s, 200);
    let r = erasure_status(&app.state, &id).await;
    assert_eq!((r["status"].clone(), r["work"].clone()), (json!("cancelled"), json!({})), "{r}");
    assert!(r.get("subject").is_none_or(Value::is_null));
    let suspended: Vec<Value> = app.state.control.test_raw().query("SELECT VALUE suspended_at FROM $u").bind(("u", uma.id.clone())).await.unwrap().take(0).unwrap();
    assert!(suspended[0].is_null(), "the account is no longer suspended: {suspended:?}");

    // once the erasure has started deleting it cannot be cancelled: it completes, or waits
    let token = eunomia_backend::auth::start_session(&app.state.control, &app.state.settings.jwt_secret, &uma, None, eunomia_backend::auth::Via::Oidc, false).await.unwrap();
    let cookie = format!("{}={token}", eunomia_backend::auth::SESSION_COOKIE);
    let (s, b) = req(&app.router, "POST", "/api/account/erasure", Some(json!({})), None, Some(&cookie)).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    let second = json_of(&b)["id"].as_str().unwrap().to_string();
    assert_ne!(second, id, "a new erasure");
    app.state.control.test_raw().query("UPDATE $e SET status = 'failed', deleting_at = time::now()").bind(("e", rec(&second))).await.unwrap().check().unwrap();
    let (s, b) = req(&app.router, "POST", &format!("/api/admin/erasures/{second}/cancel"), None, None, Some(&operator)).await;
    assert_eq!((s, json_of(&b)["code"].clone()), (409, json!("db.conflict")), "{}", String::from_utf8_lossy(&b));
    let r = erasure_status(&app.state, &second).await;
    assert_eq!(r["status"], "failed", "{r}");
    assert!(r["subject"].is_string(), "the working record stays for the retry: {r}");

}

#[tokio::test]
async fn someone_held_or_being_erased_still_reads_a_shared_space_and_keeps_their_content() {
    let _serial = SERIAL.lock().await;
    let app = TestApp::new().await;
    let operator = app.session_cookie().await;
    let db = common::org_db(&app.state, &app.user).await;
    // ben shares a space only with ana, who is under a legal hold; dan shares one only with cleo, whose
    // own erasure is open and held
    let pair = |owner: User, other: User, name: &'static str| {
        let state = app.state.clone();
        async move {
            let v = call(&state, &owner, "vault_create", json!({"name": name, "kind": "org"})).await["id"].as_str().unwrap().to_string();
            call(&state, &owner, "vault_invite", json!({"vault_id": v, "email": other.email})).await;
            call(&state, &other, "vault_accept_invitation", json!({"vault_id": v})).await;
            call(&state, &owner, "memory_write", json!({"subject_name": "Shared", "subject_kind": "organisation", "vault_id": v, "text": format!("{} wrote", owner.email)})).await;
            call(&state, &other, "memory_write", json!({"subject_name": "Shared", "subject_kind": "organisation", "vault_id": v, "text": format!("{} wrote", other.email)})).await;
            v
        }
    };
    let (ana, ben) = (common::register(&app.state, "ana@example.com").await, common::register(&app.state, "ben@example.com").await);
    let (cleo, dan) = (common::register(&app.state, "cleo@example.com").await, common::register(&app.state, "dan@example.com").await);
    let ab = pair(ben.clone(), ana.clone(), "Ana and Ben").await;
    let cd = pair(dan.clone(), cleo.clone(), "Cleo and Dan").await;
    let (s, _) = req(&app.router, "PUT", &format!("/api/admin/people/{}/legal-hold", ana.id.to_string()), Some(json!({"hold": true})), None, Some(&operator)).await;
    assert_eq!(s, 200);
    let ec = erase_route(&app, &cleo, false).await;
    let (s, _) = req(&app.router, "PUT", &format!("/api/admin/people/{}/legal-hold", cleo.id.to_string()), Some(json!({"hold": true})), None, Some(&operator)).await;
    assert_eq!(s, 200);
    let eb = erase_route(&app, &ben, false).await;
    let ed = erase_route(&app, &dan, false).await;
    let (tx, rx) = tokio::sync::watch::channel(false);
    let cfg = WorkerConfig { id: "f1".into(), poll: Duration::from_millis(20), retry_base: Duration::from_millis(10), owner_cap: 1000, ..WorkerConfig::default() };
    tokio::spawn(worker::run(app.state.clone(), handlers::registry(), cfg, rx));
    wait_status(&app, &eb, "done").await;
    wait_status(&app, &ed, "done").await;
    wait_status(&app, &ec, "held").await;
    for (v, stays, gone) in [(&ab, &ana, &ben), (&cd, &cleo, &dan)] {
        let space: Vec<Value> = db.test_raw().query("SELECT * FROM $v").bind(("v", rec(v))).await.unwrap().take(0).unwrap();
        assert_eq!(space.len(), 1, "the space {v} stays: {} still reads it", stays.email);
        let rows: Vec<Value> = db.test_raw().query("SELECT text, <string> owner AS owner FROM memory WHERE vault = $v").bind(("v", rec(v))).await.unwrap().take(0).unwrap();
        assert_eq!(rows, vec![json!({"text": format!("{} wrote", stays.email), "owner": stays.id.to_string()})], "{} keeps what they wrote, and inherits nothing", stays.email);
        let members: Vec<String> = db.test_raw().query("SELECT VALUE <string> user FROM vault_member WHERE vault = $v").bind(("v", rec(v))).await.unwrap().take(0).unwrap();
        assert_eq!(members, vec![stays.id.to_string()], "{} is still a member and {} is gone", stays.email, gone.email);
    }
    assert!(account_exists(&app, &ana).await && account_exists(&app, &cleo).await);
    let _ = tx.send(true);
}

/// Runs the offline verifier (`scripts/audit/verify-audit-export.py`) on an export, as an auditor would.
/// `None` when python3 is not installed.
/// The fact-date backfill's ledger (tenant 0046) names a person's facts and, through the record id, the
/// person: its rows go with the facts, and the post-check proves none is left.
#[tokio::test]
async fn erasure_leaves_no_fact_date_ledger_rows() {
    let _serial = SERIAL.lock().await;
    let (app, _stop) = app_with_worker().await;
    let alice = common::register(&app.state, "dana@example.com").await;
    let db = common::org_db(&app.state, &alice).await;
    let w = call(&app.state, &alice, "memory_write", json!({"subject_name": "Dana Scully", "subject_kind": "person", "text": "Dana likes tea"})).await;
    let (subject, vault) = (w["entity"]["id"].as_str().unwrap().to_string(), w["entity"]["vault"].as_str().unwrap().to_string());
    // a fact extracted from one of her synced records before facts carried dates
    let record = format!("cache_record:`{}:gmail:m1`", alice.id.key_str());
    let raw = |q: String| {
        let db = db.clone();
        async move { db.test_raw().query(q).await.unwrap().check().unwrap() }
    };
    raw(format!(
        "CREATE {record} SET owner = {u}, source = 'gmail', type = 'gmail.message', external_id = 'm1', title = 't', body_text = 'b', \
         occurred_at = d'2023-04-01T09:30:00Z', url = '', payload = {{}}, content_hash = 'fx', ingested_at = time::now(), updated_at = time::now(), deleted = false; \
         CREATE memory SET owner = {u}, vault = {vault}, subject = {subject}, text = 'Dana agreed to lead the pilot', type = 'world', source = {record}; \
         DELETE fact_dates_backfill",
        u = alice.id.to_string()
    ))
    .await;
    let org = app.state.org(&alice.org).await.unwrap();
    common::sys(eunomia_backend::jobs::fact_dates::run(&org, None)).await.unwrap();
    let ledger = || async { db.test_raw().query("SELECT VALUE id FROM fact_dates_redated").await.unwrap().take::<Vec<RecordId>>(0).unwrap().len() };
    assert_eq!(ledger().await, 1, "the backfill dated her fact");

    let id = erase_route(&app, &alice, false).await;
    let done = wait_status(&app, &id, "done").await;
    assert_eq!(done["post_check"]["fact_dates_redated"], json!(0), "the post-check covers the ledger: {done}");
    assert_eq!(ledger().await, 0, "no ledger row outlives her facts");
}

/// A fact the person had replaced in a space that stays (someone else's fact, `superseded_by` theirs) is
/// current again once their fact is erased, rather than hidden behind a fact that is gone.
#[tokio::test]
async fn erasing_a_replacing_fact_makes_the_fact_it_replaced_current_again() {
    let _serial = SERIAL.lock().await;
    let app = TestApp::new().await;
    let db = app.db().await;
    let u = app.user.id.to_string();
    let w = call(&app.state, &app.user, "memory_write", json!({"subject_name": "Ada Lovelace", "subject_kind": "person", "text": "Ada likes tea"})).await;
    let (subject, vault) = (w["entity"]["id"].as_str().unwrap().to_string(), w["entity"]["vault"].as_str().unwrap().to_string());
    db.test_raw()
        .query(format!(
            "CREATE memory:theirs SET owner = {u}, vault = {vault}, subject = {subject}, text = 'Ada works at Globex', type = 'world'; \
             CREATE memory:others SET owner = user:someone_else, vault = {vault}, subject = {subject}, text = 'Ada works at Acme', type = 'world', \
                status = 'superseded', superseded_by = memory:theirs"
        ))
        .await
        .unwrap()
        .check()
        .unwrap();
    // as the erasure job runs it: retried on a commit conflict (the write's own background work, its embedding)
    let mut res = eunomia_backend::tx::with_retry(|| async {
        eunomia_backend::store::subject::ERASE_OWNED
            .on(&db)
            .bind(("u", app.user.id.clone()))
            .bind(("heir", app.user.id.clone()))
            .bind(("actors", vec![u.clone()]))
            .bind(("email_lc", app.user.email.to_lowercase()))
            .await?
            .check()
    })
    .await
    .unwrap();
    let _: Option<Value> = res.take(eunomia_backend::store::subject::ERASE_OWNED.slot).unwrap();
    let rows: Vec<Value> = db.test_raw().query("SELECT status, superseded_by FROM memory:others").await.unwrap().take(0).unwrap();
    assert!(rows[0].get("status").is_none_or(Value::is_null) && rows[0].get("superseded_by").is_none_or(Value::is_null), "{rows:?}");
}

fn offline_verify(doc: &Value) -> Option<(Option<i32>, String)> {
    if std::process::Command::new("python3").arg("--version").output().is_err() {
        eprintln!("python3 not found: offline verifier not run");
        return None;
    }
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../scripts/audit/verify-audit-export.py");
    let dir = std::env::temp_dir().join(format!("erasure-audit-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("export.json");
    std::fs::write(&path, doc.to_string()).unwrap();
    let out = std::process::Command::new("python3").arg("-I").arg(script).arg(&path).output().unwrap();
    std::fs::remove_dir_all(&dir).ok();
    Some((out.status.code(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))))
}

#[tokio::test]
async fn erasing_someone_keeps_the_audit_chain_verifiable_and_takes_their_details_out() {
    use eunomia_backend::audit::chain;
    let _serial = SERIAL.lock().await;
    let (app, _stop) = app_with_worker().await;
    let vic = common::register(&app.state, "Vic@Example.com").await;
    let ledger = chain::org_ledger(&vic.org);
    // chained events of theirs and about them: their own export request, and an operator's hold and lift
    // (whose details name their email)
    let cookie = login(&app.router, &vic.email).await;
    assert_eq!(req(&app.router, "POST", "/api/export/subject", None, None, Some(&cookie)).await.0, 200);
    let operator = app.session_cookie().await;
    for hold in [true, false] {
        let (s, _) = req(&app.router, "PUT", &format!("/api/admin/people/{}/legal-hold", vic.id.to_string()), Some(json!({"hold": hold})), None, Some(&operator)).await;
        assert_eq!(s, 200);
    }
    common::sys(chain::replay(&app.state.control)).await.unwrap();
    let before = chain::verify(&app.state.control, &ledger).await.unwrap();
    assert!(before.ok, "{before:?}");
    let about: Vec<Value> = app.state.control.test_raw().query("SELECT * FROM audit_event WHERE ledger = $l AND seq IS NOT NONE AND (user = $u OR string::contains(string::lowercase(detail), 'vic@example.com'))")
        .bind(("l", ledger.clone())).bind(("u", vic.id.clone())).await.unwrap().take(0).unwrap();
    assert!(about.len() >= 3, "sealed events about vic before the erasure: {about:?}");

    let id = erase_route(&app, &vic, false).await;
    let r = wait_status(&app, &id, "done").await;
    assert!(r["post_check"].as_object().unwrap().values().all(|v| v == 0), "{r}");
    common::sys(chain::replay(&app.state.control)).await.unwrap();

    // the chain still verifies, on the server and offline; the erased details count as unchecked
    let after = chain::verify(&app.state.control, &ledger).await.unwrap();
    assert!(after.ok, "the chain verifies after the erasure: {after:?}");
    assert!(r["counts"]["audit_event_details_erased"].as_i64().unwrap() >= 3, "{r}");
    assert!(after.detail_unchecked >= 3, "{after:?}");
    let doc = serde_json::to_value(chain::export(&app.state.control, &ledger, None, None).await.unwrap()).unwrap();
    assert_eq!(doc["ok"], true, "{doc}");
    if let Some((code, out)) = offline_verify(&doc) {
        assert_eq!(code, Some(0), "the offline verifier accepts the ledger after the erasure: {out}");
    }
    // and nothing in the ledger shows their email; their sealed events keep only the ids the hash covers
    let rows: Vec<Value> = app.state.control.test_raw().query("SELECT * FROM audit_event").await.unwrap().take(0).unwrap();
    for row in &rows {
        assert!(!row.to_string().to_lowercase().contains("vic@example.com"), "the email is gone from the ledger: {row}");
    }
    for row in rows.iter().filter(|r| r["user"] == vic.id.to_string() && r["seq"].is_number()) {
        assert_eq!(row["detail"], chain::ERASED_DETAIL, "{row}");
    }
    assert!(!account_exists(&app, &vic).await);
}

#[tokio::test]
async fn with_two_step_verification_erasing_needs_a_fresh_second_factor() {
    let _serial = SERIAL.lock().await;
    let (app, _stop) = app_with_worker().await;
    let enrol = |cookie: String| {
        let router = app.router.clone();
        async move {
            let (s, e) = req(&router, "POST", "/api/auth/mfa/enrol", Some(json!({"password": common::PASSWORD})), None, Some(&cookie)).await;
            assert_eq!(s, 200, "{}", String::from_utf8_lossy(&e));
            let secret = json_of(&e)["secret"].as_str().unwrap().to_string();
            let code = eunomia_backend::mfa::code_at(&secret, u64::try_from(chrono::Utc::now().timestamp()).unwrap()).unwrap();
            let (s, done) = req(&router, "POST", "/api/auth/mfa/enrol/confirm", Some(json!({"code": code})), None, Some(&cookie)).await;
            assert_eq!(s, 200, "{}", String::from_utf8_lossy(&done));
            json_of(&done)["recovery_codes"].as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect::<Vec<_>>()
        }
    };
    let stale = |u: &User| {
        let (control, id) = (app.state.control.clone(), u.id.clone());
        async move { control.test_raw().query("UPDATE session SET mfa_at = time::now() - 1h WHERE owner = $u").bind(("u", id)).await.unwrap().check().unwrap() }
    };

    // the person: password, then a second factor from the last 10 minutes or a code now
    let wes = common::register(&app.state, "wes@example.com").await;
    let cookie = login(&app.router, &wes.email).await;
    let codes = enrol(cookie.clone()).await;
    stale(&wes).await;
    app.state.credentials.clear();
    let (s, b) = req(&app.router, "POST", "/api/account/erasure", Some(json!({"password": common::PASSWORD})), None, Some(&cookie)).await;
    assert_eq!((s, json_of(&b)["code"].clone()), (403, json!("auth.step_up_required")), "{}", String::from_utf8_lossy(&b));
    let (s, b) = req(&app.router, "POST", "/api/account/erasure", Some(json!({"password": common::PASSWORD, "mfa_code": "000000"})), None, Some(&cookie)).await;
    assert_eq!((s, json_of(&b)["code"].clone()), (401, json!("auth.mfa_invalid")));
    let queued: Vec<RecordId> = app.state.control.test_raw().query("SELECT VALUE id FROM erasure WHERE subject = $u").bind(("u", wes.id.clone())).await.unwrap().take(0).unwrap();
    assert!(queued.is_empty(), "nothing was queued");
    assert_eq!(req(&app.router, "GET", "/api/auth/me", None, None, Some(&cookie)).await.0, 200, "nor suspended");
    let (s, b) = req(&app.router, "POST", "/api/account/erasure", Some(json!({"password": common::PASSWORD, "mfa_code": codes[0]})), None, Some(&cookie)).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    wait_status(&app, json_of(&b)["id"].as_str().unwrap(), "done").await;

    // the operator: their own password, then their own second factor
    let xan = common::register(&app.state, "xan@example.com").await;
    let operator = app.session_cookie().await;
    let codes = enrol(operator.clone()).await;
    stale(&app.user).await;
    app.state.credentials.clear();
    let path = format!("/api/admin/people/{}/erasure", xan.id.to_string());
    let (s, b) = req(&app.router, "POST", &path, Some(json!({"password": common::PASSWORD})), None, Some(&operator)).await;
    assert_eq!((s, json_of(&b)["code"].clone()), (403, json!("auth.step_up_required")), "{}", String::from_utf8_lossy(&b));
    assert!(account_exists(&app, &xan).await);
    let (s, b) = req(&app.router, "POST", &path, Some(json!({"password": common::PASSWORD, "mfa_code": codes[0]})), None, Some(&operator)).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    wait_status(&app, json_of(&b)["id"].as_str().unwrap(), "done").await;
}

#[tokio::test]
async fn access_reviews_are_exported_to_the_person_and_kept_when_they_are_erased() {
    let _serial = SERIAL.lock().await;
    let (app, _stop) = app_with_worker().await;
    let yan = common::register(&app.state, "yan@example.com").await;
    let operator = app.session_cookie().await;
    let (s, review) = req(&app.router, "POST", "/api/admin/access-reviews", None, None, Some(&operator)).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&review));
    let rid = json_of(&review)["id"].as_str().unwrap().to_string();
    let decide = format!("/api/admin/access-reviews/{rid}/decisions/{}", eunomia_backend::audit::chain::rid(&yan.id));
    assert_eq!(req(&app.router, "PUT", &decide, Some(json!({"decision": "keep", "note": "reads the team space"})), None, Some(&operator)).await.0, 200);
    assert_eq!(req(&app.router, "POST", &format!("/api/admin/access-reviews/{rid}/complete"), None, None, Some(&operator)).await.0, 200);

    // the person's own export holds their row of the snapshot and the decision about them, not the reviewer
    let cookie = login(&app.router, &yan.email).await;
    let (s, b) = req(&app.router, "POST", "/api/export/subject", None, None, Some(&cookie)).await;
    assert_eq!(s, 200);
    let eid = json_of(&b)["id"].as_str().unwrap().to_string();
    wait_for("yan's export", || async {
        let (_, b) = req(&app.router, "GET", "/api/export/subject", None, None, Some(&cookie)).await;
        json_of(&b).as_array().unwrap().iter().any(|e| e["id"] == eid.as_str() && e["status"] == "ready")
    })
    .await;
    let (s, zip) = req(&app.router, "GET", &format!("/api/export/subject/{eid}"), None, None, Some(&cookie)).await;
    assert_eq!(s, 200);
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip)).unwrap();
    let mut text = String::new();
    archive.by_name(&format!("workspaces/{}/access_reviews.json", yan.org.key())).unwrap().read_to_string(&mut text).unwrap();
    let reviews: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(reviews[0]["your_access"]["email"], "yan@example.com", "{reviews}");
    assert_eq!(reviews[0]["decisions_about_you"][0]["decision"], "keep", "{reviews}");
    assert!(!text.contains(&app.user.email), "the reviewer and other people's rows are not in it: {text}");
    let mut account = String::new();
    archive.by_name("account.json").unwrap().read_to_string(&mut account).unwrap();
    assert!(account.contains("two_step_enabled_at") && account.contains("recovery_codes_unused"), "{account}");

    // a completed review is frozen: the erasure finishes and the review stays as recorded
    let db = common::org_db(&app.state, &yan).await;
    let before: Vec<Value> = db.test_raw().query("SELECT * FROM $r").bind(("r", rec(&rid))).await.unwrap().take(0).unwrap();
    let id = erase_route(&app, &yan, false).await;
    let r = wait_status(&app, &id, "done").await;
    assert!(r["post_check"].as_object().unwrap().values().all(|v| v == 0), "{r}");
    let after: Vec<Value> = db.test_raw().query("SELECT * FROM $r").bind(("r", rec(&rid))).await.unwrap().take(0).unwrap();
    assert_eq!(before, after, "the completed review is kept unchanged");
    let (s, export) = req(&app.router, "GET", &format!("/api/admin/access-reviews/{rid}/export"), None, None, Some(&operator)).await;
    assert_eq!(s, 200);
    assert_eq!((json_of(&export)["result_hash_matches"].clone(), json_of(&export)["chain_matches"].clone()), (json!(true), json!(true)), "it still matches the chain");

    // closing a workspace with a completed review: everything else goes, the frozen review stays
    let solo = common::register_personal(&app.state, "solo-review@example.com").await;
    let solo_db = common::org_db(&app.state, &solo).await;
    solo_db.test_raw().query("CREATE access_review:old SET org = $o, status = 'completed', started_by = $u, started_by_email = $e, snapshot = {}, snapshot_hash = 'x'")
        .bind(("o", solo.org.key())).bind(("u", solo.id.clone())).bind(("e", solo.email.clone())).await.unwrap().check().unwrap();
    let id = erase_route(&app, &solo, true).await;
    let r = wait_status(&app, &id, "done").await;
    assert_eq!(r["post_check"]["closed_workspace_rows"], 0, "{r}");
    let kept: Vec<Value> = solo_db.test_raw().query("SELECT VALUE id FROM access_review").await.unwrap().take(0).unwrap();
    assert_eq!(kept.len(), 1, "the frozen review stays in the closed database for the operator who drops it");
}

#[tokio::test]
async fn a_set_aside_account_is_erased_by_its_kept_address_unless_another_account_has_it() {
    let _serial = SERIAL.lock().await;
    let (app, _stop) = app_with_worker().await;
    let operator = app.session_cookie().await;
    let db = common::org_db(&app.state, &app.user).await;
    let set_aside = |u: &User| {
        let (control, id) = (app.state.control.clone(), u.id.clone());
        async move { eunomia_backend::store::control::AUTH_SET_ASIDE.on(&control).bind(("user", id)).await.unwrap().check().unwrap() }
    };
    let erase = |u: &User| {
        let (router, cookie, path) = (app.router.clone(), operator.clone(), format!("/api/admin/people/{}/erasure", u.id.to_string()));
        async move {
            let (s, b) = req(&router, "POST", &path, Some(json!({"password": common::PASSWORD})), None, Some(&cookie)).await;
            assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
            json_of(&b)["id"].as_str().unwrap().to_string()
        }
    };
    // nobody else has the address: it is erased like any account's
    let zed = common::register(&app.state, "zed@example.com").await;
    call(&app.state, &app.user, "memory_write", json!({"subject_name": "Desk", "subject_kind": "organisation", "text": "ask ZED@example.com"})).await;
    set_aside(&zed).await;
    let id = erase(&zed).await;
    let r = wait_status(&app, &id, "done").await;
    assert!(r["post_check"].as_object().unwrap().values().all(|v| v == 0), "{r}");
    assert!(!account_exists(&app, &zed).await);
    assert!(texts(&db).await.iter().any(|t| t == "ask [erased person]"), "the kept address is scrubbed: {:?}", texts(&db).await);

    // a new account signs in with the address now: the set-aside one goes, the address stays the new one's
    let amy = common::register(&app.state, "amy@example.com").await;
    call(&app.state, &app.user, "memory_write", json!({"subject_name": "Desk", "subject_kind": "organisation", "text": "ask amy@example.com"})).await;
    set_aside(&amy).await;
    let amy_now = common::register(&app.state, "amy@example.com").await;
    assert_ne!(amy_now.id, amy.id);
    let id = erase(&amy).await;
    let r = wait_status(&app, &id, "done").await;
    assert!(r["post_check"].as_object().unwrap().values().all(|v| v == 0), "{r}");
    assert!(!account_exists(&app, &amy).await && account_exists(&app, &amy_now).await);
    assert!(texts(&db).await.iter().any(|t| t == "ask amy@example.com"), "the address belongs to the new account: not scrubbed");
}

/// An organisation invitation (`POST /api/org/invitations`) from the account behind `cookie`; returns the token.
async fn org_invite(app: &TestApp, cookie: &str, email: &str, role: &str) -> String {
    let (s, b) = req(&app.router, "POST", "/api/org/invitations", Some(json!({"email": email, "role": role})), None, Some(cookie)).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    json_of(&b)["token"].as_str().unwrap().to_string()
}

/// Organisation invitations name the person by email (open or used) and by `accepted_by` / `invited_by`.
/// The export carries the ones addressed to or accepted by them (never a token hash or the inviter); the
/// erasure deletes those, keeps the ones they sent for the organisation under `user:erased`, and the
/// post-check counts the table (with the delete removed, it goes red).
#[tokio::test]
async fn organisation_invitations_naming_the_person_are_exported_and_erased() {
    let _serial = SERIAL.lock().await;
    let (app, _stop) = app_with_worker().await;
    let admin = app.session_cookie().await;
    // dana joins through an invitation, as an admin, and invites erin herself
    let token = org_invite(&app, &admin, "Dana@Example.com", "admin").await;
    let (s, b) = req(&app.router, "POST", "/api/auth/register", Some(json!({"email": "dana@example.com", "password": common::PASSWORD, "invitation": token})), None, None).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    let dana = models_user::user_by_email(&app.state.control, "dana@example.com").await.unwrap().unwrap();
    let cookie = login(&app.router, &dana.email).await;
    org_invite(&app, &cookie, "erin@example.com", "member").await;
    // and another workspace has an open invitation addressed to her
    let olga = common::register_personal(&app.state, "olga@example.com").await;
    let olga_cookie = login(&app.router, &olga.email).await;
    org_invite(&app, &olga_cookie, "dana@example.com", "member").await;

    // export: both invitations naming her, without token hashes or inviters
    let (s, b) = req(&app.router, "POST", "/api/export/subject", None, None, Some(&cookie)).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    let id = json_of(&b)["id"].as_str().unwrap().to_string();
    wait_for("the export", || async {
        let (_, b) = req(&app.router, "GET", "/api/export/subject", None, None, Some(&cookie)).await;
        json_of(&b).as_array().unwrap().iter().any(|e| e["id"] == id.as_str() && e["status"] == "ready")
    })
    .await;
    let (s, zip) = req(&app.router, "GET", &format!("/api/export/subject/{id}"), None, None, Some(&cookie)).await;
    assert_eq!(s, 200);
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip)).unwrap();
    let mut account = String::new();
    archive.by_name("account.json").unwrap().read_to_string(&mut account).unwrap();
    let account: Value = serde_json::from_str(&account).unwrap();
    let invs = account["organisation_invitations"].as_array().unwrap_or_else(|| panic!("{account}"));
    assert_eq!(invs.len(), 2, "{invs:?}");
    let joined: Vec<&Value> = invs.iter().filter(|i| i["accepted_at"].is_string()).collect();
    assert_eq!((joined.len(), joined[0]["role"].as_str(), joined[0]["org"].as_str()), (1, Some("admin"), Some(app.user.org.record().to_string().as_str())), "{invs:?}");
    assert!(invs.iter().any(|i| i["org"] == olga.org.record().to_string() && i["accepted_at"].is_null()), "{invs:?}");
    let text = account.to_string();
    assert!(!text.contains("token_hash") && !text.contains("invited_by") && !text.contains("erin@example.com"), "{text}");

    // erasure, first with the invitation delete removed: the post-check reports the table and nothing is done
    eunomia_backend::isolation::mutate(
        "subject.erase_control",
        "LET $oi = (DELETE org_invitation WHERE accepted_by = $u OR string::lowercase(email) = $email_lc RETURN BEFORE);",
        "LET $oi = [];",
    );
    let id = erase_route(&app, &dana, false).await;
    let failed = wait_status(&app, &id, "failed").await;
    eunomia_backend::isolation::clear_mutations();
    assert_eq!(failed["post_check"]["org_invitation"], 2, "the one she joined through and the open one addressed to her: {failed}");
    eunomia_backend::subject::heal_open(&app.state).await.unwrap();
    let done = wait_status(&app, &id, "done").await;
    assert_eq!(done["post_check"]["org_invitation"], 0, "{done}");
    assert_eq!(done["counts"]["organisation_invitations"], 2, "{done}");

    // none names her any more; the one she sent stays for the organisation, under no one
    let rows = control_rows(&app.state, "SELECT email, <string> invited_by AS invited_by, accepted_by FROM org_invitation", ("x", json!(null))).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!((rows[0]["email"].as_str(), rows[0]["invited_by"].as_str()), (Some("erin@example.com"), Some("user:erased")), "{rows:?}");
    let db = common::org_db(&app.state, &app.user).await;
    let hits = rows_naming(&app.state, &db, &[dana.id.key_str(), "dana@example.com".to_string()], &[]).await;
    assert!(hits.iter().all(|h| !h.starts_with("control org_invitation")), "{hits:?}");
}
