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
    let mark = format!("canary-{}", who.id.key_str());

    // a shared space the workspace owner runs; `who` writes in it (this stays, reassigned)
    let team = call(state, &app.user, "vault_create", json!({"name": format!("Team {mark}"), "kind": "org"})).await["id"].as_str().unwrap().to_string();
    call(state, &app.user, "vault_invite", json!({"vault_id": team, "email": who.email})).await;
    call(state, who, "vault_accept_invitation", json!({"vault_id": team})).await;
    call(state, who, "memory_write", json!({"subject_name": "Quarterly plan", "subject_kind": "organisation", "vault_id": team, "text": "Plan agreed by the team"})).await;
    // the owner mentions them by email in the shared space (scrubbed, not deleted)
    call(state, &app.user, "memory_write", json!({"subject_name": "Contacts", "subject_kind": "organisation", "vault_id": team, "text": format!("Reach {} for invoices", who.email)})).await;
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
    let path: Vec<String> = db.test_raw().query("SELECT VALUE object_path FROM type::record($d)").bind(("d", doc_id.clone())).await.unwrap().take(0).unwrap();
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
        CREATE ontology_kind SET vault = $club, kind = $c, status = 'approved', decided_by = $u, activity_actor = <string> $u, activity_actor_name = $email;";
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

    // control: an identity, an audit event naming their email, usage, an OAuth grant with a token and a code, a capsule, a job
    jobs::enqueue(&state.control, NewJob::new("no_such_kind", who.id.clone(), format!("seed-{mark}")).in_org(who.org)).await.unwrap();
    let control_sql = "
        CREATE identity SET user = $u, issuer = 'https://idp.example', subject = $c, email = $email;
        CREATE audit_event SET user = $u, actor_kind = 'user', actor_id = <string> $u, action = 'vault.invite', target = $c, outcome = 'ok', detail = string::concat('email=', $email);
        CREATE audit_event SET user = $admin, actor_kind = 'user', actor_id = <string> $admin, action = 'admin.grant', target = <string> $u, outcome = 'ok', detail = string::concat('email=', $email);
        CREATE tool_usage SET org = $okey, bucket = time::now(), user = $u, actor_kind = 'user', actor_id = <string> $u, channel = 'mcp', tool = 'recall', calls = 1;
        LET $g = (CREATE oauth_grant SET owner = $u, client_id = 'client-x', client_name = $c, scope = ['memory:read'], resource = 'http://localhost:8001/mcp' RETURN VALUE id)[0];
        CREATE oauth_token SET kind = 'access', token_hash = $c, family = $g, expires_at = time::now() + 15m;
        CREATE oauth_code SET code_hash = $c, owner = $u, client_id = 'client-x', redirect_uri = 'http://localhost/cb', code_challenge = $c, scope = ['memory:read'], resource = 'http://x', expires_at = time::now() + 1m;
        CREATE failure_capsule SET kind = 'tool', name = 'recall', source = 'test', status = 500, code = 'internal', trace_id = $c, version = 'x', args = '', user = <string> $u;";
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
    // an export of theirs, waiting
    let (s, b) = req(&app.router, "POST", "/api/export/subject", None, Some(&token), None).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));

    Seeded { team, club, files: vec![path[0].clone(), audio] }
}

/// Every row of every table, as text, that mentions `needle`.
async fn rows_naming(state: &AppState, org: &eunomia_backend::pool::OrgDb, needles: &[String]) -> Vec<String> {
    let mut hits = Vec::new();
    for t in TENANT_TABLES.iter().chain(["subject_export"].iter()) {
        let rows: Vec<Value> = org.test_raw().query(format!("SELECT * OMIT embedding, archive FROM {t}")).await.unwrap().take(0).unwrap();
        for r in rows {
            let text = r.to_string().to_lowercase();
            if needles.iter().any(|n| text.contains(&n.to_lowercase())) {
                hits.push(format!("{t}: {}", text.chars().take(300).collect::<String>()));
            }
        }
    }
    for t in CONTROL_TABLES {
        let rows: Vec<Value> = state.control.test_raw().query(format!("SELECT * FROM {t}")).await.unwrap().take(0).unwrap();
        for r in rows {
            let text = r.to_string().to_lowercase();
            if needles.iter().any(|n| text.contains(&n.to_lowercase())) {
                hits.push(format!("control {t}: {}", text.chars().take(300).collect::<String>()));
            }
        }
    }
    hits
}

async fn erasure_status(state: &AppState, id: &str) -> Value {
    let rows = control_rows(state, "SELECT * FROM type::record($id)", ("id", json!(id))).await;
    rows.into_iter().next().unwrap_or(Value::Null)
}

#[tokio::test]
async fn erasure_removes_every_category_and_proves_it() {
    let (app, _stop) = app_with_worker().await;
    let alice = common::register(&app.state, "Alice@Example.com").await;
    let bob = common::register(&app.state, "bob@example.com").await;
    let seeded = seed(&app, &alice, &bob).await;
    let db = common::org_db(&app.state, &alice).await;
    let key = alice.id.key_str();
    let needles = vec![key.clone(), "alice@example.com".to_string()];
    assert!(!rows_naming(&app.state, &db, &needles).await.is_empty(), "the seed names the person");
    let tester_token_ok = req(&app.router, "GET", "/api/auth/me", None, Some(&app.token), None).await.0;
    assert_eq!(tester_token_ok, 200);

    // a wrong password changes nothing
    let cookie = login(&app.router, &alice.email).await;
    let (s, b) = req(&app.router, "POST", "/api/account/erasure", Some(json!({"password": "not-it"})), None, Some(&cookie)).await;
    assert_eq!(s, 403, "{}", String::from_utf8_lossy(&b));
    assert_eq!(json_of(&b)["code"], "subject.confirmation_failed");
    // a token cannot ask for it, only a browser session
    let tok = token_for(&app.state, &alice).await;
    let (s, _) = req(&app.router, "POST", "/api/account/erasure", Some(json!({"password": common::PASSWORD})), Some(&tok), None).await;
    assert_eq!(s, 403);

    let (s, b) = req(&app.router, "POST", "/api/account/erasure", Some(json!({"password": common::PASSWORD})), None, Some(&cookie)).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    let ticket = json_of(&b);
    let id = ticket["id"].as_str().unwrap().to_string();
    // signed out at once
    assert_eq!(req(&app.router, "GET", "/api/auth/me", None, None, Some(&cookie)).await.0, 401);

    wait_for("the erasure to finish", || async { erasure_status(&app.state, &id).await["status"] == "done" }).await;
    let record = erasure_status(&app.state, &id).await;
    assert!(record.get("subject").is_none_or(Value::is_null), "the completion record drops the user id: {record}");
    let check = record["post_check"].as_object().unwrap();
    assert!(check.len() > 40, "the post-check covers every table: {record}");
    assert!(check.values().all(|v| v == 0), "{record}");
    assert!(record["counts"]["memories"].as_i64().unwrap() >= 2, "{record}");
    assert!(record["counts"]["bucket_files"].as_i64().unwrap() >= 2, "{record}");
    assert_eq!(record["counts"]["ownership_handed_over"], 1, "{record}");

    // nothing in any table names them, by id or email, and their files are gone
    let left = rows_naming(&app.state, &db, &needles).await;
    assert!(left.is_empty(), "rows still naming the person:\n{}", left.join("\n"));
    for f in &seeded.files {
        assert!(!object_exists(&db, f).await, "{f} is still in the bucket");
    }

    // what they wrote in the shared space stays, owned by someone who remains; the email in it is gone
    let kept: Vec<Value> = db
        .test_raw()
        .query("SELECT text, <string> owner AS owner FROM memory WHERE vault = type::record($v)")
        .bind(("v", seeded.team.clone()))
        .await
        .unwrap()
        .take(0)
        .unwrap();
    assert!(kept.iter().any(|m| m["text"] == "Plan agreed by the team" && m["owner"] == app.user.id.to_string()), "{kept:?}");
    assert!(kept.iter().any(|m| m["text"].as_str().unwrap().contains("[erased person]")), "{kept:?}");
    // the space they ran with bob is bob's now
    let roles: Vec<String> = db.test_raw().query("SELECT VALUE role FROM vault_member WHERE vault = type::record($v) AND user = $b").bind(("v", seeded.club.clone())).bind(("b", bob.id.clone())).await.unwrap().take(0).unwrap();
    assert_eq!(roles, vec!["owner"]);

    // the completion record is in the audit trail, in this workspace's activity log
    let ev = control_rows(&app.state, "SELECT action, detail FROM audit_event WHERE action = 'subject.erase' AND target = $t", ("t", json!(id))).await;
    assert_eq!(ev.len(), 1, "{ev:?}");
    let cookie = app.session_cookie().await;
    let (s, b) = req(&app.router, "GET", "/api/admin/erasures", None, None, Some(&cookie)).await;
    assert_eq!(s, 200);
    assert!(json_of(&b).as_array().unwrap().iter().any(|e| e["id"] == id.as_str() && e["status"] == "done"));

    // everyone else still works: the owner's token and session, bob's account
    assert_eq!(req(&app.router, "GET", "/api/auth/me", None, Some(&app.token), None).await.0, 200);
    login(&app.router, &bob.email).await;

    // resync suppression: a sync for the erased account stands down
    let org = app.state.org(&alice.org).await.unwrap();
    let report = eunomia_backend::sources::scheduler::sync_source(&org, &alice.id, "github", None).await;
    assert_eq!(report["status"], "erased", "{report}");

    // restore re-apply: a backup taken before the erasure brings the account back; the re-apply erases it again
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
    let (s, b) = req(&app.router, "POST", "/api/admin/erasures/reapply", None, None, Some(&cookie)).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    assert_eq!(json_of(&b)["queued"], 1);
    wait_for("the re-applied erasure", || async {
        control_rows(&app.state, "SELECT VALUE id FROM erasure WHERE requested_by = 'reapply' AND status = 'done'", ("x", json!(null))).await.len() == 1
    })
    .await;
    let left = rows_naming(&app.state, &db, &needles).await;
    assert!(left.is_empty(), "rows still naming the restored person:\n{}", left.join("\n"));
}

#[tokio::test]
async fn the_post_check_fails_when_one_category_is_not_deleted() {
    let app = TestApp::new().await;
    let carol = common::register(&app.state, "carol@example.com").await;
    let db = common::org_db(&app.state, &carol).await;
    db.test_raw()
        .query("LET $t = (CREATE ONLY chat_thread SET owner = $u, title = 'x').id; CREATE chat_message SET owner = $u, thread_id = $t, role = 'user', content = 'hello';")
        .bind(("u", carol.id.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();
    let erasure = RecordId::new("erasure", "mutated");
    let orgs = eunomia_backend::subject::check_erasable(&app.state.control, &carol.id).await.unwrap();

    // the control removed: chat messages are no longer deleted
    eunomia_backend::isolation::mutate("subject.erase_owned", "DELETE chat_message WHERE owner = $u", "DELETE chat_message WHERE false");
    let res = common::sys(eunomia_backend::subject::erase::erase_subject(&app.state, &carol.id, &orgs, &erasure)).await;
    eunomia_backend::isolation::clear_mutations();
    let (_, check) = res.unwrap();
    assert_eq!(eunomia_backend::subject::erase::residue(&check), vec!["chat_message".to_string()], "{check}");
}

#[tokio::test]
async fn erasure_is_refused_under_a_legal_hold_for_the_last_owner_and_for_a_wrong_confirmation() {
    let app = TestApp::new().await;
    let dan = common::register(&app.state, "dan@example.com").await;
    let admin_cookie = app.session_cookie().await;
    let path = format!("/api/admin/people/{}", dan.id.to_string());

    let (s, b) = req(&app.router, "PUT", &format!("{path}/legal-hold"), Some(json!({"hold": true})), None, Some(&admin_cookie)).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    assert!(json_of(&b)["legal_hold_at"].is_string());
    let dan_cookie = login(&app.router, &dan.email).await;
    let (s, b) = req(&app.router, "POST", "/api/account/erasure", Some(json!({"password": common::PASSWORD})), None, Some(&dan_cookie)).await;
    assert_eq!((s, json_of(&b)["code"].clone()), (409, json!("subject.legal_hold")));
    let (s, b) = req(&app.router, "POST", &format!("{path}/erasure"), Some(json!({"confirm_email": dan.email})), None, Some(&admin_cookie)).await;
    assert_eq!((s, json_of(&b)["code"].clone()), (409, json!("subject.legal_hold")));
    // People shows the hold; the activity log has it both ways
    let (_, b) = req(&app.router, "GET", "/api/admin/people", None, None, Some(&admin_cookie)).await;
    assert!(json_of(&b)["people"].as_array().unwrap().iter().any(|p| p["email"] == dan.email.as_str() && p["legal_hold_at"].is_string()));
    let (s, _) = req(&app.router, "PUT", &format!("{path}/legal-hold"), Some(json!({"hold": false})), None, Some(&admin_cookie)).await;
    assert_eq!(s, 200);
    let actions: Vec<String> = app.state.control.test_raw().query("SELECT VALUE action FROM audit_event WHERE string::starts_with(action, 'subject.legal_hold') ORDER BY created_at").await.unwrap().take(0).unwrap();
    assert_eq!(actions, vec!["subject.legal_hold", "subject.legal_hold_lift"]);

    // an admin confirms with the person's email; a wrong one is refused and nothing changes
    let (s, b) = req(&app.router, "POST", &format!("{path}/erasure"), Some(json!({"confirm_email": "someone@example.com"})), None, Some(&admin_cookie)).await;
    assert_eq!((s, json_of(&b)["code"].clone()), (403, json!("subject.confirmation_failed")));
    assert_eq!(req(&app.router, "GET", "/api/auth/me", None, None, Some(&dan_cookie)).await.0, 200, "still signed in");
    let (s, b) = req(&app.router, "POST", &format!("{path}/erasure"), Some(json!({"confirm_email": "DAN@example.com"})), None, Some(&admin_cookie)).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    assert_eq!(json_of(&b)["status"], "pending");

    // the account that set up the server (here also the workspace's last owner) cannot erase itself
    let (s, b) = req(&app.router, "POST", "/api/account/erasure", Some(json!({"password": common::PASSWORD})), None, Some(&admin_cookie)).await;
    assert_eq!((s, json_of(&b)["code"].clone()), (409, json!("subject.last_owner")));
    // an admin cannot use the People route on themselves
    let (s, _) = req(&app.router, "POST", &format!("/api/admin/people/{}/erasure", app.user.id.to_string()), Some(json!({"confirm_email": app.user.email})), None, Some(&admin_cookie)).await;
    assert_eq!(s, 400);
}

#[tokio::test]
async fn the_last_owner_of_a_workspace_cannot_be_erased() {
    // two accounts, each with a workspace of their own: the second is its workspace's only owner
    let app = TestApp::new().await;
    let solo = common::register_personal(&app.state, "solo@example.com").await;
    let err = eunomia_backend::subject::check_erasable(&app.state.control, &solo.id).await.unwrap_err();
    assert_eq!(err.code.as_str(), "subject.last_owner");
}

#[tokio::test]
async fn export_holds_every_category_and_leaves_out_spaces_you_left() {
    let (app, _stop) = app_with_worker().await;
    let erin = common::register(&app.state, "erin@example.com").await;
    let token = token_for(&app.state, &erin).await;
    // a shared space erin writes in, and the owner writes in; another space erin left
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
        .query("CREATE chat_message SET owner = $u, thread_id = type::record($t), role = 'tool', content = string::concat('{\"vault\":\"', $g, '\",\"text\":\"Secret project\"}'); CREATE chat_message SET owner = $u, thread_id = type::record($t), role = 'user', content = 'hello';")
        .bind(("u", erin.id.clone()))
        .bind(("t", thread))
        .bind(("g", gone.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();
    app.state.control.test_raw().query("CREATE audit_event SET user = $u, actor_kind = 'user', action = 'vault.invite', target = 'x', outcome = 'ok', detail = 'email=someone.else@example.com'").bind(("u", erin.id.clone())).await.unwrap().check().unwrap();

    let (s, b) = req(&app.router, "POST", "/api/export/subject", None, Some(&token), None).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    let id = json_of(&b)["id"].as_str().unwrap().to_string();
    // not ready yet, or ready: never another person's
    wait_for("the export", || async {
        let (_, b) = req(&app.router, "GET", "/api/export/subject", None, Some(&token), None).await;
        json_of(&b).as_array().unwrap().iter().any(|e| e["id"] == id.as_str() && e["status"] == "ready")
    })
    .await;
    let other = token_for(&app.state, &common::register(&app.state, "frank@example.com").await).await;
    assert_eq!(req(&app.router, "GET", &format!("/api/export/subject/{id}"), None, Some(&other), None).await.0, 404, "only the person (or the admin who asked) downloads it");
    let (s, zip) = req(&app.router, "GET", &format!("/api/export/subject/{id}"), None, Some(&token), None).await;
    assert_eq!(s, 200);
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip)).unwrap();
    let mut read = |name: &str| -> String {
        let mut s = String::new();
        archive.by_name(name).unwrap_or_else(|_| panic!("{name} in the archive")).read_to_string(&mut s).unwrap();
        s
    };
    let manifest: Value = serde_json::from_str(&read("manifest.json")).unwrap();
    let names: Vec<&str> = manifest["categories"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    for want in ["account", "spaces", "entities", "memories", "relations", "skills", "documents", "synced_records", "chat", "tool_log", "sources", "recordings", "api_tokens", "sessions", "connected_apps", "audit_events", "usage"] {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }
    let memories = read("memories.json");
    assert!(memories.contains("personal fact") && memories.contains("erin wrote this"), "{memories}");
    assert!(!memories.contains("owner wrote this"), "other members' facts are theirs");
    assert!(!memories.contains("from a space erin left"), "a space erin left is left out");
    let chat = read("chat.json");
    assert!(!chat.contains("Secret project") && chat.contains("Left out of this export"), "{chat}");
    assert_eq!(manifest["redactions"]["chat_messages_replaced"], 1);
    let audit = read("audit_events.json");
    assert!(!audit.contains("someone.else@example.com"), "{audit}");
    let tokens = read("api_tokens.json");
    assert!(tokens.contains("erasable") && !tokens.contains("token_hash"), "{tokens}");
    assert_eq!(manifest["document_files"]["included"], 1, "{manifest}");
    let doc_files: Vec<String> = (0..archive.len()).map(|i| archive.by_index(i).unwrap().name().to_string()).filter(|n| n.starts_with("documents/")).collect();
    assert_eq!(doc_files.len(), 1, "{doc_files:?}");

    // an admin can export for someone in the workspace; the archive is listed for the admin
    let cookie = app.session_cookie().await;
    let (s, b) = req(&app.router, "POST", &format!("/api/admin/people/{}/export", erin.id.to_string()), None, None, Some(&cookie)).await;
    assert_eq!(s, 200, "{}", String::from_utf8_lossy(&b));
    assert_eq!(json_of(&b)["subject"], erin.id.to_string());
}
