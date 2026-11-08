//! Vault segregation inside ONE org, attacked with several users (ported from main's
//! `frontend/tests/vault-isolation.spec.ts`; the cross-org proof is `tests/isolation.rs`):
//!   alice   owns her personal vault and the "Shared" vault
//!   bob     accepted member of Shared and of alice's and carol's personal vaults
//!   pat     only has PENDING invitations to Shared and alice's personal vault
//!   mallory an outsider who only knows ids
//! Every attack must come back empty or refused.

mod common;

use common::http;
use eunomia_backend::config::Settings;
use eunomia_backend::entities::consolidate;
use eunomia_backend::models_user::{self, User};
use eunomia_backend::tools::registry;
use eunomia_backend::vaults::service as vaults;
use eunomia_backend::rid;
use serde_json::{json, Value};

const ALICE_SECRET: &str = "ALICESECRET kiwi";
const SHARED_FACT: &str = "SHAREDFACT mango";
const BOB_SECRET: &str = "BOBSECRET papaya";

struct P {
    user: User,
    token: String,
}

struct World {
    app: common::TestApp,
    alice: P,
    bob: P,
    carol: P,
    pat: P,
    mallory: P,
    shared: String,
    alice_personal: String,
    bob_personal: String,
    carol_personal: String,
    alice_zed: String,
    alice_zed_memory: String,
    bob_zed: String,
    quinn: String,
    quinn_memory: String,
}

impl World {
    async fn call(&self, p: &P, name: &str, args: Value) -> Value {
        match Box::pin(common::sys(registry::call(&self.app.state, &p.user, name, args))).await {
            Ok(v) => v,
            // a refused call is an error either way (a tool value or a 4xx)
            Err(e) => json!({ "error": e.message }),
        }
    }
    /// The tool succeeded.
    async fn ok(&self, p: &P, name: &str, args: Value) -> Value {
        let v = self.call(p, name, args).await;
        assert!(v.get("error").is_none(), "{name} failed: {v}");
        v
    }
    /// The tool was refused.
    async fn denied(&self, p: &P, name: &str, args: Value) {
        let v = self.call(p, name, args).await;
        assert!(v.get("error").is_some(), "{name} should have been refused but returned {v}");
    }
    async fn get(&self, p: &P, path: &str) -> (u16, Value) {
        let ((s, b), _) = http(&self.app.router, "GET", path, None, Some(&p.token), None).await;
        (s.as_u16(), b)
    }
    async fn send(&self, p: &P, method: &str, path: &str, body: Value) -> u16 {
        http(&self.app.router, method, path, Some(body), Some(&p.token), None).await.0 .0.as_u16()
    }
    async fn text(&self, p: &P, path: &str) -> String {
        use axum::http::{header, Request};
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        let req = Request::builder().uri(path).header(header::AUTHORIZATION, format!("Bearer {}", p.token)).body(axum::body::Body::empty()).unwrap();
        let resp = self.app.router.clone().oneshot(req).await.unwrap();
        String::from_utf8_lossy(&resp.into_body().collect().await.unwrap().to_bytes()).into_owned()
    }
}

async fn person(app: &common::TestApp, user: User) -> P {
    let token = models_user::create_api_token(&app.state.control, &user.id, "t").await.unwrap().token;
    P { user, token }
}

async fn personal_of(w: &World, p: &P) -> String {
    let list = w.ok(p, "vault_list", json!({})).await;
    list["results"].as_array().unwrap().iter().find(|v| v["kind"] == "personal" && v["role"] == "owner").unwrap()["id"].as_str().unwrap().to_string()
}

async fn accept(w: &World, p: &P, vault: &str) {
    let db = w.app.db().await;
    common::sys(vaults::accept_invitation(&db, &w.app.state.control, &p.user.id, &rid::parse(vault).unwrap())).await.unwrap();
}

async fn world() -> World {
    let app = common::TestApp::new().await;
    let alice = person(&app, app.user.clone()).await;
    let mut others = Vec::new();
    for n in ["bob", "carol", "pat", "mallory"] {
        let u = common::register(&app.state, &format!("{n}@example.com")).await;
        others.push(person(&app, u).await);
    }
    let mallory = others.pop().unwrap();
    let pat = others.pop().unwrap();
    let carol = others.pop().unwrap();
    let bob = others.pop().unwrap();
    let blank = String::new;
    let mut w = World {
        app, alice, bob, carol, pat, mallory,
        shared: blank(), alice_personal: blank(), bob_personal: blank(), carol_personal: blank(),
        alice_zed: blank(), alice_zed_memory: blank(), bob_zed: blank(), quinn: blank(), quinn_memory: blank(),
    };
    w.alice_personal = personal_of(&w, &w.alice).await;
    w.bob_personal = personal_of(&w, &w.bob).await;
    w.carol_personal = personal_of(&w, &w.carol).await;

    // alice's private fact, and a same-named entity in bob's personal vault
    let zed = w.ok(&w.alice, "memory_write", json!({"subject_name": "Zed Secretperson", "subject_kind": "person", "text": ALICE_SECRET})).await;
    w.alice_zed = zed["entity"]["id"].as_str().unwrap().into();
    w.alice_zed_memory = zed["memory"]["id"].as_str().unwrap().into();
    let bz = w.ok(&w.bob, "memory_write", json!({"subject_name": "Zed Secretperson", "subject_kind": "person", "text": BOB_SECRET})).await;
    w.bob_zed = bz["entity"]["id"].as_str().unwrap().into();

    // the shared vault: bob accepted, pat only invited
    w.shared = w.ok(&w.alice, "vault_create", json!({"name": "Shared"})).await["id"].as_str().unwrap().into();
    let q = w.ok(&w.alice, "memory_write", json!({"subject_name": "Quinn Teammate", "subject_kind": "person", "text": SHARED_FACT, "vault_id": w.shared})).await;
    w.quinn = q["entity"]["id"].as_str().unwrap().into();
    w.quinn_memory = q["memory"]["id"].as_str().unwrap().into();
    w.ok(&w.alice, "vault_invite", json!({"vault_id": w.shared, "email": w.bob.user.email})).await;
    accept(&w, &w.bob, &w.shared).await;
    w.ok(&w.alice, "vault_invite", json!({"vault_id": w.shared, "email": w.pat.user.email})).await;
    w.ok(&w.alice, "vault_invite", json!({"vault_id": w.alice_personal, "email": w.pat.user.email})).await;

    // bob also joins two other people's personal vaults, so "his personal vault" is ambiguous by kind alone
    w.ok(&w.alice, "vault_invite", json!({"vault_id": w.alice_personal, "email": w.bob.user.email})).await;
    accept(&w, &w.bob, &w.alice_personal).await;
    w.ok(&w.carol, "vault_invite", json!({"vault_id": w.carol_personal, "email": w.bob.user.email})).await;
    accept(&w, &w.bob, &w.carol_personal).await;
    w
}

fn s(v: &Value) -> String {
    v.to_string()
}

#[tokio::test]
async fn a_member_of_other_peoples_personal_vaults_still_defaults_to_their_own() {
    let w = world().await;
    for i in 0..3 {
        let m = w.ok(&w.bob, "memory_write", json!({"subject_name": format!("Default Probe {i}"), "subject_kind": "person", "text": "probe"})).await;
        assert_eq!(m["memory"]["vault"], w.bob_personal.as_str());
    }
    let found = w.ok(&w.bob, "recall", json!({"query": "Zed Secretperson kiwi papaya"})).await;
    assert!(!s(&found).contains("ALICESECRET") && s(&found).contains("BOBSECRET"), "{found}");
    let search = s(&w.ok(&w.bob, "entities_search", json!({"query": "Zed"})).await);
    assert!(search.contains(&w.bob_zed) && !search.contains(&w.alice_zed), "{search}");
    // recall without a vault covers the user's org vaults, each hit labelled; a named vault narrows it
    let all = w.ok(&w.alice, "recall", json!({"query": "Quinn Teammate mango"})).await;
    let shared = all["results"].as_array().unwrap().iter().find(|i| i["text"].as_str().unwrap_or("").contains("SHAREDFACT")).cloned();
    let shared = shared.unwrap_or_else(|| panic!("org-vault facts are found without vault_id: {all}"));
    assert_eq!(shared["vault"], w.shared.as_str());
    assert_eq!(shared["vault_name"], "Shared");
    let narrowed = w.ok(&w.alice, "recall", json!({"query": "Quinn Teammate mango", "vault_id": w.alice_personal})).await;
    assert!(!s(&narrowed).contains("SHAREDFACT"), "{narrowed}");
}

#[tokio::test]
async fn shared_vault_reads_never_return_personal_data() {
    let w = world().await;
    let all = json!(["2000-01-01T00:00:00Z", "2100-01-01T00:00:00Z"]);
    for p in [&w.alice, &w.bob] {
        let r = s(&w.ok(p, "recall", json!({"query": "Zed Secretperson kiwi papaya Quinn mango", "vault_id": w.shared, "time_range": all})).await);
        assert!(r.contains("SHAREDFACT") && !r.contains("ALICESECRET") && !r.contains("BOBSECRET"), "{r}");
        assert!(!s(&w.ok(p, "recall", json!({"query": "kiwi papaya", "vault_id": "Shared"})).await).contains("SECRET"));
        assert!(!s(&w.ok(p, "reflect", json!({"query": "What about Zed Secretperson? kiwi", "vault_id": w.shared})).await).contains("SECRET"));
    }
    let search = w.ok(&w.bob, "entities_search", json!({"query": "", "vault_id": w.shared})).await;
    let ids: Vec<&str> = search["results"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [w.quinn.as_str()]);
    let graph = w.ok(&w.bob, "entities_graph", json!({"vault_id": w.shared})).await;
    let nodes: Vec<&str> = graph["nodes"].as_array().unwrap().iter().map(|n| n["id"].as_str().unwrap()).collect();
    assert_eq!(nodes, [w.quinn.as_str()]);
    let (status, cloud) = w.get(&w.bob, &format!("/api/entities/cloud?vault_ids={}", urlenc(&w.shared))).await;
    assert_eq!(status, 200);
    assert!(cloud["points"].as_array().unwrap().iter().all(|p| p["vault"] == w.shared.as_str() && p["kind"] != "record"));
    assert!(!s(&cloud).contains("SECRET"));
}

fn urlenc(v: &str) -> String {
    v.replace(':', "%3A")
}

#[tokio::test]
async fn an_outsider_cannot_touch_another_users_entities_or_vaults() {
    let w = world().await;
    let m = &w.mallory;
    assert!(w.call(m, "entities_get", json!({"id": w.alice_zed})).await.get("error").is_some());
    assert_eq!(w.get(m, &format!("/api/entities/{}", urlenc(&w.alice_zed))).await.0, 404);

    outsider_cannot_edit(&w).await;
    outsider_cannot_use_vaults(&w).await;
}

async fn outsider_cannot_edit(w: &World) {
    let m = &w.mallory;
    w.denied(m, "memory_update", json!({"memory_id": w.alice_zed_memory, "text": "pwned"})).await;
    assert_eq!(w.ok(m, "memory_delete", json!({"memory_id": w.alice_zed_memory})).await["deleted"], false);
    w.denied(m, "entity_update", json!({"entity_id": w.alice_zed, "name": "pwned"})).await;
    assert_eq!(w.ok(m, "entity_delete", json!({"entity_id": w.alice_zed})).await["deleted"], false);
    let mine = w.ok(m, "memory_write", json!({"subject_name": "Mallory Thing", "subject_kind": "person", "text": "x"})).await["entity"]["id"].as_str().unwrap().to_string();
    w.denied(m, "entity_merge", json!({"winner_id": mine, "loser_id": w.alice_zed})).await;
    w.denied(m, "entity_merge", json!({"winner_id": w.alice_zed, "loser_id": mine})).await;
    w.denied(m, "code_relate", json!({"from_id": mine, "to_id": w.alice_zed, "label": "knows"})).await;
    w.denied(m, "entity_relate", json!({"from_id": mine, "label": "knows", "to_id": w.alice_zed, "reason": "pwned"})).await;
    w.denied(m, "entity_relate", json!({"from_id": w.alice_zed, "label": "knows", "to_id": mine, "reason": "pwned"})).await;
    assert_eq!(w.send(m, "PATCH", &format!("/api/entities/memory/{}", urlenc(&w.alice_zed_memory)), json!({"text": "pwned"})).await, 404);
    assert_eq!(w.send(m, "POST", &format!("/api/entities/{}/memory", urlenc(&w.alice_zed)), json!({"text": "pwned"})).await, 404);
    assert_eq!(w.send(m, "POST", &format!("/api/entities/{}/relations", urlenc(&mine)), json!({"to_id": w.alice_zed, "label": "x"})).await, 404);
    assert!(w.send(m, "POST", &format!("/api/entities/{}/merge", urlenc(&mine)), json!({"loser_id": w.alice_zed})).await >= 400);
    assert_eq!(w.send(m, "DELETE", &format!("/api/entities/{}", urlenc(&w.alice_zed)), json!(null)).await, 404);

    let zed = w.ok(&w.alice, "entities_get", json!({"id": w.alice_zed})).await;
    assert_eq!(zed["name"], "Zed Secretperson");
    assert_eq!(zed["memory"].as_array().unwrap().len(), 1);
    assert_eq!(zed["relations"], json!([]));
}

async fn outsider_cannot_use_vaults(w: &World) {
    let m = &w.mallory;
    for vault_id in [&w.shared, &w.alice_personal] {
        w.denied(m, "recall", json!({"query": "kiwi mango", "vault_id": vault_id})).await;
        w.denied(m, "reflect", json!({"query": "kiwi mango", "vault_id": vault_id})).await;
        w.denied(m, "entities_search", json!({"query": "", "vault_id": vault_id})).await;
        w.denied(m, "entities_graph", json!({"vault_id": vault_id})).await;
        w.denied(m, "memory_write", json!({"subject_name": "X", "subject_kind": "person", "text": "x", "vault_id": vault_id})).await;
        w.denied(m, "code_entity_upsert", json!({"kind": "repository", "name": "x", "vault_id": vault_id})).await;
        w.denied(m, "vault_members", json!({"vault_id": vault_id})).await;
        w.denied(m, "vault_clone", json!({"vault_id": vault_id})).await;
        w.denied(m, "vault_merge", json!({"vault_id_a": vault_id, "vault_id_b": w.bob_personal})).await;
        w.denied(m, "vault_rename", json!({"vault_id": vault_id, "name": "pwned"})).await;
        w.denied(m, "vault_invite", json!({"vault_id": vault_id, "email": m.user.email})).await;
        w.denied(m, "vault_remove_member", json!({"vault_id": vault_id, "email": w.alice.user.email})).await;
        w.denied(m, "vault_delete", json!({"vault_id": vault_id})).await;
        for path in ["/api/entities/cloud?vault_ids=", "/api/entities?vault_id=", "/api/entities/graph?vault_id="] {
            assert_eq!(w.get(m, &format!("{path}{}", urlenc(vault_id))).await.0, 403, "{path}");
        }
    }
    let err = w.call(m, "recall", json!({"query": "mango", "vault_id": "Shared"})).await;
    assert!(err["error"].as_str().unwrap().contains("no vault named"), "{err}");
    let members = s(&w.ok(&w.alice, "vault_members", json!({"vault_id": w.shared})).await);
    assert!(!members.contains(&m.user.email));
}

/// A stand-in OpenAI-compatible chat endpoint that records every request body it is sent.
async fn fake_model() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let (log, url) = (seen.clone(), format!("http://{}", l.local_addr().unwrap()));
    tokio::spawn(async move {
        while let Ok((mut c, _)) = l.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = c.read(&mut chunk).await.unwrap_or(0);
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let want = head.to_lowercase().split("content-length:").nth(1).and_then(|r| r.split_whitespace().next()).and_then(|n| n.parse::<usize>().ok()).unwrap_or(0);
                        if body.len() >= want || n == 0 {
                            log.lock().unwrap().push(body.to_string());
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                let body = json!({"choices": [{"message": {"content": json!({"belief": "stolen"}).to_string()}}]}).to_string();
                let _ = c.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes()).await;
            });
        }
    });
    (url, seen)
}

#[tokio::test]
async fn an_outsider_cannot_hijack_consolidation_to_send_facts_to_their_own_model() {
    let w = world().await;
    let (url, seen) = fake_model().await;
    let settings: Settings = common::test_settings();
    // the model endpoint is each user's own setting
    for p in [&w.mallory, &w.pat, &w.alice] {
        assert_eq!(w.send(p, "PATCH", "/api/settings", json!({"openai_base_url": url})).await, 200);
    }
    let db = w.app.db().await;
    let zed = rid::parse(&w.alice_zed).unwrap();
    for who in [&w.mallory, &w.pat] {
        let out = common::sys(consolidate::consolidate_subject(&db, &settings, &who.user.id, &zed, None)).await.unwrap();
        assert!(out.is_none());
    }
    // a memory id is not a subject either
    let mem = rid::parse(&w.alice_zed_memory).unwrap();
    assert!(common::sys(consolidate::consolidate_subject(&db, &settings, &w.alice.user.id, &mem, None)).await.unwrap().is_none());
    assert!(seen.lock().unwrap().is_empty(), "the facts reached the attacker's endpoint");
    let zed_out = w.ok(&w.alice, "entities_get", json!({"id": w.alice_zed})).await;
    assert!(zed_out["memory"].as_array().unwrap().iter().all(|m| m["type"] != "observation"));

    // control: the owner's own consolidation does reach the endpoint, so the silence above means something
    assert!(common::sys(consolidate::consolidate_subject(&db, &settings, &w.alice.user.id, &zed, None)).await.unwrap().is_some());
    assert!(seen.lock().unwrap().iter().any(|b| b.contains("ALICESECRET")));
}

#[tokio::test]
async fn a_plain_member_cannot_administer_the_vault() {
    let w = world().await;
    let b = &w.bob;
    w.denied(b, "vault_rename", json!({"vault_id": w.shared, "name": "pwned"})).await;
    w.denied(b, "vault_invite", json!({"vault_id": w.shared, "email": w.mallory.user.email})).await;
    w.denied(b, "vault_remove_member", json!({"vault_id": w.shared, "email": w.alice.user.email})).await;
    w.denied(b, "vault_delete", json!({"vault_id": w.shared})).await;
    let list = w.ok(&w.alice, "vault_list", json!({})).await;
    assert!(list["results"].as_array().unwrap().iter().any(|v| v["id"] == w.shared.as_str() && v["name"] == "Shared"));
}

#[tokio::test]
async fn a_pending_invitee_cannot_read_or_write_and_exports_only_their_own() {
    let w = world().await;
    let p = &w.pat;
    let mine = s(&w.ok(p, "vault_list", json!({})).await);
    assert!(!mine.contains(&w.shared) && !mine.contains(&w.alice_personal), "{mine}");
    for vault_id in [&w.shared, &w.alice_personal] {
        w.denied(p, "recall", json!({"query": "kiwi mango", "vault_id": vault_id})).await;
        w.denied(p, "entities_search", json!({"query": "", "vault_id": vault_id})).await;
        w.denied(p, "memory_write", json!({"subject_name": "X", "subject_kind": "person", "text": "x", "vault_id": vault_id})).await;
        w.denied(p, "vault_members", json!({"vault_id": vault_id})).await;
        w.denied(p, "vault_clone", json!({"vault_id": vault_id})).await;
        assert_eq!(w.get(p, &format!("/api/entities/cloud?vault_ids={}", urlenc(vault_id))).await.0, 403);
    }
    assert!(w.call(p, "recall", json!({"query": "mango", "vault_id": "Shared"})).await["error"].as_str().unwrap().contains("no vault named"));
    assert!(w.call(p, "entities_get", json!({"id": w.quinn})).await.get("error").is_some());
    w.denied(p, "memory_update", json!({"memory_id": w.quinn_memory, "text": "pwned"})).await;

    let exported = w.text(p, "/api/export").await;
    assert!(!exported.contains("ALICESECRET") && !exported.contains("Zed Secretperson"), "{exported}");
    assert!(!s(&w.ok(p, "recall", json!({"query": "Zed Secretperson kiwi"})).await).contains("ALICESECRET"));
}

#[tokio::test]
async fn export_holds_only_the_callers_own_personal_vault() {
    let w = world().await;
    let exported = w.text(&w.bob, "/api/export").await;
    assert!(exported.contains("BOBSECRET"), "{exported}");
    assert!(!exported.contains("ALICESECRET") && !exported.contains("SHAREDFACT"));
}

#[tokio::test]
async fn writes_never_land_in_the_wrong_vault_and_relations_and_merges_cannot_bridge_vaults() {
    let w = world().await;
    let m = w.ok(&w.bob, "memory_write", json!({"subject_name": "Zed Secretperson", "subject_kind": "person", "text": "shared note", "vault_id": w.shared})).await;
    assert_eq!(m["memory"]["vault"], w.shared.as_str());
    let shared_zed = m["entity"]["id"].as_str().unwrap().to_string();
    assert!(shared_zed != w.alice_zed && shared_zed != w.bob_zed);
    assert_eq!(w.ok(&w.alice, "entities_get", json!({"id": w.alice_zed})).await["memory"].as_array().unwrap().len(), 1);

    // bob belongs to both vaults, which used to be enough
    w.denied(&w.bob, "code_relate", json!({"from_id": shared_zed, "to_id": w.bob_zed, "label": "same_as"})).await;
    // a business link across two vaults is refused even for a member of both, and saves no reason fact
    w.denied(&w.bob, "entity_relate", json!({"from_id": shared_zed, "label": "same_as", "to_id": w.bob_zed, "reason": "leaked"})).await;
    w.denied(&w.bob, "entity_merge", json!({"winner_id": shared_zed, "loser_id": w.bob_zed})).await;
    let repo = w.ok(&w.bob, "code_entity_upsert", json!({"kind": "repository", "name": "private-repo"})).await;
    w.denied(&w.bob, "code_entity_upsert", json!({"kind": "file", "name": "leak.rs", "parent_id": repo["id"], "vault_id": w.shared})).await;
    assert!(w.send(&w.bob, "POST", &format!("/api/entities/{}/relations", urlenc(&shared_zed)), json!({"to_id": w.bob_zed, "label": "x"})).await >= 400);
    assert!(w.send(&w.bob, "POST", &format!("/api/entities/{}/merge", urlenc(&shared_zed)), json!({"loser_id": w.bob_zed})).await >= 400);
    w.denied(&w.alice, "code_relate", json!({"from_id": w.quinn, "to_id": w.alice_zed, "label": "knows"})).await;
    w.denied(&w.alice, "entity_relate", json!({"from_id": w.quinn, "label": "knows", "to_id": w.alice_zed})).await;

    let seen = w.ok(&w.alice, "entities_get", json!({"id": shared_zed})).await;
    assert_eq!(seen["relations"], json!([]));
    assert_eq!(seen["memory"].as_array().unwrap().len(), 1, "a refused link saves no reason fact: {seen}");
    assert!(!s(&seen).contains(&w.bob_zed));
    let bz = w.ok(&w.bob, "entities_get", json!({"id": w.bob_zed})).await;
    assert_eq!(bz["memory"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_cross_vault_edge_left_over_from_before_stays_hidden() {
    let w = world().await;
    let db = w.app.db().await;
    // forge one directly, as an older install could hold
    db.test_raw()
        .query("RELATE $a->relates_to->$b SET label = 'legacy', owner = $o")
        .bind(("a", rid::parse(&w.quinn).unwrap()))
        .bind(("b", rid::parse(&w.bob_zed).unwrap()))
        .bind(("o", w.alice.user.id.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();
    let seen = s(&w.ok(&w.alice, "entities_get", json!({"id": w.quinn})).await);
    assert!(!seen.contains("legacy") && !seen.contains(&w.bob_zed), "{seen}");
}

#[tokio::test]
async fn a_memory_or_membership_id_is_not_an_entity_id() {
    let w = world().await;
    assert!(w.call(&w.bob, "entities_get", json!({"id": w.quinn_memory})).await.get("error").is_some());
    let del = w.call(&w.bob, "entity_delete", json!({"entity_id": w.quinn_memory})).await;
    assert!(del.get("error").is_some() || del["deleted"] == false, "{del}");
    // the memory is still there
    let q = w.ok(&w.alice, "entities_get", json!({"id": w.quinn})).await;
    assert_eq!(q["memory"].as_array().unwrap().len(), 1);
    // nor is a vault_member row
    let db = w.app.db().await;
    let members = common::sys(vaults::list_members(&db, &w.app.state.control, &w.alice.user.id, &rid::parse(&w.shared).unwrap())).await.unwrap();
    assert_eq!(members.len(), 2);
}

#[tokio::test]
async fn synced_records_stay_with_their_owner_never_with_a_vault() {
    let w = world().await;
    for p in [&w.alice, &w.bob] {
        // the route queues a job and waits for a worker; this harness runs none, so sync in place
        let org = w.app.state.org(&p.user.org).await.unwrap();
        let out = eunomia_backend::sources::scheduler::sync_source(&org, &p.user.id, "demo", None).await;
        assert!(out.get("error").is_none() && out["errors"] == json!([]), "{out}");
    }
    let rec = w.ok(&w.bob, "list", json!({"limit": 200})).await["results"][0].clone();
    assert!(rec.is_object());
    // bob cites his own record in a shared-vault memory
    w.ok(&w.bob, "memory_write", json!({"subject_name": "Quinn Teammate", "subject_kind": "person", "text": "met about the record", "source_record_id": rec["id"], "vault_id": w.shared})).await;
    for p in [&w.alice, &w.bob] {
        let r = w.ok(p, "recall", json!({"query": format!("Quinn Teammate {}", rec["title"].as_str().unwrap()), "vault_id": w.shared})).await;
        assert!(r["results"].as_array().unwrap().iter().all(|i| i["kind"] != "cache_record"), "records in a shared-vault recall: {r}");
    }
    assert!(w.call(&w.alice, "list", json!({"filters": {"source OR true OR source": "x"}})).await.get("error").is_some());
    assert_eq!(w.ok(&w.carol, "list", json!({"limit": 200})).await["results"], json!([]));
    assert_eq!(w.ok(&w.carol, "search", json!({"query": rec["title"]})).await["results"], json!([]));
    assert!(w.call(&w.carol, "get", json!({"id": rec["id"]})).await.get("error").is_some());
}

#[tokio::test]
async fn the_audit_log_only_shows_the_callers_own_calls() {
    let w = world().await;
    let (status, audit) = w.get(&w.alice, "/api/audit?limit=500").await;
    assert_eq!(status, 200);
    assert!(!audit["results"].as_array().unwrap().is_empty());
    assert!(!s(&audit).contains("BOBSECRET"));
}

#[tokio::test]
async fn a_removed_member_and_a_member_who_leaves_lose_access_immediately() {
    let w = world().await;
    w.ok(&w.alice, "vault_remove_member", json!({"vault_id": w.shared, "email": w.bob.user.email})).await;
    let b = &w.bob;
    w.denied(b, "recall", json!({"query": "mango", "vault_id": w.shared})).await;
    w.denied(b, "memory_write", json!({"subject_name": "X", "subject_kind": "person", "text": "x", "vault_id": w.shared})).await;
    w.denied(b, "memory_update", json!({"memory_id": w.quinn_memory, "text": "pwned"})).await;
    assert_eq!(w.ok(b, "memory_delete", json!({"memory_id": w.quinn_memory})).await["deleted"], false);
    assert!(w.call(b, "entities_get", json!({"id": w.quinn})).await.get("error").is_some());
    assert!(w.call(b, "recall", json!({"query": "mango", "vault_id": "Shared"})).await["error"].as_str().unwrap().contains("no vault named"));
    assert_eq!(w.get(b, &format!("/api/entities/cloud?vault_ids={}", urlenc(&w.shared))).await.0, 403);

    w.ok(&w.alice, "vault_invite", json!({"vault_id": w.shared, "email": w.carol.user.email})).await;
    accept(&w, &w.carol, &w.shared).await;
    assert!(s(&w.ok(&w.carol, "recall", json!({"query": "Quinn mango", "vault_id": w.shared})).await).contains("SHAREDFACT"));
    w.ok(&w.carol, "vault_leave", json!({"vault_id": w.shared})).await;
    w.denied(&w.carol, "recall", json!({"query": "mango", "vault_id": w.shared})).await;
    assert!(w.call(&w.carol, "entities_get", json!({"id": w.quinn})).await.get("error").is_some());
}

#[tokio::test]
async fn last_owner_checks_count_only_active_owners() {
    let w = world().await;
    // a pending co-owner is not an owner yet: the sole real owner can neither be removed nor leave
    let db = w.app.db().await;
    let v = rid::parse(&w.shared).unwrap();
    common::sys(vaults::invite_member(&db, &w.app.state.control, &w.alice.user.id, &v, &w.mallory.user.email, "owner")).await.unwrap();
    w.denied(&w.alice, "vault_remove_member", json!({"vault_id": w.shared, "email": w.alice.user.email})).await;
    w.denied(&w.alice, "vault_leave", json!({"vault_id": w.shared})).await;
}

#[tokio::test]
async fn cloning_a_vault_copies_only_that_vaults_memories() {
    let w = world().await;
    // a stray memory of another vault pointing at a shared-vault entity (an older install could hold one)
    let db = w.app.db().await;
    db.test_raw()
        .query("CREATE memory SET owner = $o, vault = $v, subject = $s, text = 'STRAYFACT', type = 'world'")
        .bind(("o", w.bob.user.id.clone()))
        .bind(("v", rid::parse(&w.bob_personal).unwrap()))
        .bind(("s", rid::parse(&w.quinn).unwrap()))
        .await
        .unwrap()
        .check()
        .unwrap();
    let c = w.ok(&w.alice, "vault_clone", json!({"vault_id": w.shared})).await;
    let new_id = c["id"].as_str().unwrap();
    let all = s(&w.ok(&w.alice, "recall", json!({"query": "STRAYFACT mango", "vault_id": new_id, "time_range": ["2000-01-01T00:00:00Z", "2100-01-01T00:00:00Z"]})).await);
    assert!(all.contains("SHAREDFACT") && !all.contains("STRAYFACT"), "{all}");
}
