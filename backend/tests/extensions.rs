//! Extensions, phase 1 (docs/extensions.md): the registry, the webhook inbox with its secret and
//! signature, the claim/ack/nack consumer API, ownership, the buffer bound, rotation and deletion.
//! End to end through the real router and an in-memory SurrealDB.

mod common;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use common::TestApp;
use eunomia_backend::extensions::sign;
use eunomia_backend::models_user::create_api_token_with;
use eunomia_backend::scopes;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

/// Every test caps an inbox at 20 open events so the bound is cheap to reach; no other test sends more.
const CAP: usize = 20;

fn setup() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe { std::env::set_var("EXTENSION_BUFFER_MAX", CAP.to_string()) });
}

async fn app() -> TestApp {
    setup();
    TestApp::new().await
}

async fn api(app: &TestApp, token: &str, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    common::http(&app.router, method, path, body, Some(token), None).await.0
}

/// One delivery to the public inbox. `headers` are extra request headers.
async fn hook(app: &TestApp, key: &str, body: &str, headers: &[(&str, String)]) -> (StatusCode, Value) {
    let mut req = Request::builder().method("POST").uri(format!("/hooks/ext/{key}")).header(header::CONTENT_TYPE, "application/json");
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let resp = app.router.clone().oneshot(req.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn bearer(secret: &str) -> Vec<(&'static str, String)> {
    vec![("authorization", format!("Bearer {secret}"))]
}

fn signed(secret: &str, body: &str) -> Vec<(&'static str, String)> {
    vec![("x-eunomia-signature", sign(secret, body.as_bytes()))]
}

/// Creates an extension through the API; returns (its JSON, the one-time secret).
async fn create(app: &TestApp, token: &str, name: &str) -> (Value, String) {
    let (status, body) = api(app, token, "POST", "/api/extensions", Some(json!({"name": name}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    (body["extension"].clone(), body["secret"].as_str().expect("secret").to_string())
}

fn key_of(ext: &Value) -> String {
    ext["webhook_url"].as_str().unwrap().rsplit('/').next().unwrap().to_string()
}

fn id_of(ext: &Value) -> String {
    ext["id"].as_str().unwrap().to_string()
}

async fn deliver_n(app: &TestApp, ext: &Value, secret: &str, n: usize) {
    for i in 0..n {
        let (status, body) = hook(app, &key_of(ext), &json!({"n": i}).to_string(), &bearer(secret)).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    }
}

async fn claim(app: &TestApp, token: &str, ext: &Value, max: i64, lease: i64) -> Vec<Value> {
    let (status, body) =
        api(app, token, "POST", &format!("/api/extensions/{}/events/claim", id_of(ext)), Some(json!({"max": max, "lease_secs": lease}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["events"].as_array().unwrap().clone()
}

fn ids(events: &[Value]) -> Vec<Value> {
    events.iter().map(|e| e["id"].clone()).collect()
}

async fn counts(app: &TestApp, token: &str, ext: &Value) -> (i64, i64, i64, i64) {
    let (status, body) = api(app, token, "GET", &format!("/api/extensions/{}", id_of(ext)), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let n = |k: &str| body[k].as_i64().unwrap();
    (n("pending"), n("claimed"), n("done"), n("dead"))
}

/// Pretend every lease ran out.
async fn lapse_leases(app: &TestApp) {
    let db = app.db().await;
    db.test_raw().query("UPDATE extension_event SET claimed_until = time::now() - 1s WHERE status = 'claimed'").await.unwrap().check().unwrap();
}

#[tokio::test]
async fn create_shows_the_secret_once_and_a_webhook_url() {
    let app = app().await;
    let (ext, secret) = create(&app, &app.token, "Notes importer").await;
    assert!(secret.starts_with("exs_") && secret.len() > 40);
    assert!(ext["id"].as_str().unwrap().starts_with("extension:"));
    let url = ext["webhook_url"].as_str().unwrap();
    assert!(url.starts_with("http://localhost:8001/hooks/ext/"), "{url}");
    assert_eq!((ext["enabled"].as_bool(), ext["pending"].as_i64(), ext["vault_id"].is_null()), (Some(true), Some(0), true));
    assert_eq!(ext["secret_hint"].as_str().unwrap(), &secret[secret.len() - 4..]);

    // the secret is in no later response
    for path in ["/api/extensions".to_string(), format!("/api/extensions/{}", id_of(&ext))] {
        let (status, body) = api(&app, &app.token, "GET", &path, None).await;
        assert_eq!(status, StatusCode::OK);
        assert!(!body.to_string().contains(&secret), "{path} leaked the secret");
    }
    // and not in the database in the clear
    let db = app.db().await;
    let mut res = db.test_raw().query("SELECT secret_enc FROM extension").await.unwrap();
    let rows: Vec<Value> = res.take(0).unwrap();
    assert!(!rows[0].to_string().contains(&secret));

    // validation
    let (status, body) = api(&app, &app.token, "POST", "/api/extensions", Some(json!({"name": "  "}))).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::BAD_REQUEST, Some("validation.invalid")));
    let (status, body) = api(&app, &app.token, "POST", "/api/extensions", Some(json!({"name": "x", "vault_id": "vault:nope"}))).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("vault.forbidden")), "{body}");
}

#[tokio::test]
async fn target_vault_is_checked_and_can_be_changed() {
    let app = app().await;
    let personal = app.tool("vault_list", json!({})).await["results"][0]["id"].as_str().unwrap().to_string();
    let (status, body) = api(&app, &app.token, "POST", "/api/extensions", Some(json!({"name": "x", "vault_id": personal}))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["extension"]["vault_id"].as_str(), Some(personal.as_str()));
    let id = id_of(&body["extension"]);
    let (_, patched) = api(&app, &app.token, "PATCH", &format!("/api/extensions/{id}"), Some(json!({"vault_id": ""}))).await;
    assert!(patched["vault_id"].is_null(), "{patched}");
}

#[tokio::test]
async fn bearer_and_hmac_deliveries_are_buffered() {
    let app = app().await;
    let (ext, secret) = create(&app, &app.token, "Hooks").await;
    let key = key_of(&ext);

    let (status, body) = hook(&app, &key, r#"{"kind":"bearer"}"#, &bearer(&secret)).await;
    assert_eq!((status, body["status"].as_str()), (StatusCode::ACCEPTED, Some("accepted")), "{body}");
    let payload = r#"{"kind":"hmac","items":[1,2,3]}"#;
    let (status, body) = hook(&app, &key, payload, &signed(&secret, payload)).await;
    assert_eq!((status, body["status"].as_str()), (StatusCode::ACCEPTED, Some("accepted")), "{body}");

    let (pending, claimed, done, dead) = counts(&app, &app.token, &ext).await;
    assert_eq!((pending, claimed, done, dead), (2, 0, 0, 0));
    let (_, one) = api(&app, &app.token, "GET", &format!("/api/extensions/{}", id_of(&ext)), None).await;
    assert!(one["last_delivery_at"].is_string(), "{one}");

    let events = claim(&app, &app.token, &ext, 10, 60).await;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["body"], json!({"kind": "bearer"}));
    assert_eq!(events[1]["body"], json!({"kind": "hmac", "items": [1, 2, 3]}));
    assert_eq!(events[0]["attempts"], 1);
}

#[tokio::test]
async fn bad_credentials_are_401_and_all_look_alike() {
    let app = app().await;
    let (ext, secret) = create(&app, &app.token, "Hooks").await;
    let key = key_of(&ext);
    let body = r#"{"a":1}"#;

    let mut answers = Vec::new();
    answers.push(hook(&app, &key, body, &bearer("exs_wrong")).await); // wrong bearer
    answers.push(hook(&app, &key, body, &[("x-eunomia-signature", sign("exs_wrong", body.as_bytes()))]).await); // wrong key
    answers.push(hook(&app, &key, body, &[("x-eunomia-signature", sign(&secret, b"another body"))]).await); // body changed
    answers.push(hook(&app, &key, body, &[("x-eunomia-signature", "sha256=nothex".into())]).await);
    answers.push(hook(&app, &key, body, &[("x-eunomia-signature", sign(&secret, body.as_bytes()).replace("sha256=", ""))]).await); // no prefix
    // a bad signature is not rescued by a good bearer
    let mut both = bearer(&secret);
    both.push(("x-eunomia-signature", "sha256=00".into()));
    answers.push(hook(&app, &key, body, &both).await);
    answers.push(hook(&app, &key, body, &[]).await); // nothing at all
    // unknown extension of a real owner, a malformed key, an unknown owner
    let owner_prefix = key.rsplit_once('_').unwrap().0;
    answers.push(hook(&app, &format!("{owner_prefix}_0123456789abcdef"), body, &bearer(&secret)).await);
    answers.push(hook(&app, "not-a-key", body, &bearer(&secret)).await);
    answers.push(hook(&app, "nosuchuser_0123456789abcdef", body, &bearer(&secret)).await);

    for (i, (status, b)) in answers.iter().enumerate() {
        assert_eq!((*status, b["code"].as_str()), (StatusCode::UNAUTHORIZED, Some("auth.unauthorized")), "case {i}: {b}");
        assert_eq!(b["detail"], answers[0].1["detail"], "case {i} says something different");
    }
    assert_eq!(counts(&app, &app.token, &ext).await, (0, 0, 0, 0), "nothing was buffered");

    // a personal API token is not the extension's secret
    let (status, _) = hook(&app, &key, body, &bearer(&app.token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn bodies_must_be_json_within_the_bound_and_retries_are_recognised() {
    let app = app().await;
    let (ext, secret) = create(&app, &app.token, "Hooks").await;
    let key = key_of(&ext);

    let (status, body) = hook(&app, &key, "not json", &bearer(&secret)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::BAD_REQUEST, Some("validation.invalid")), "{body}");
    let big = format!("{{\"x\":\"{}\"}}", "a".repeat(300 * 1024));
    let (status, body) = hook(&app, &key, &big, &bearer(&secret)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    let just_under = format!("{{\"x\":\"{}\"}}", "a".repeat(250 * 1024));
    let (status, _) = hook(&app, &key, &just_under, &bearer(&secret)).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    // any JSON value is accepted, not just objects
    let (status, _) = hook(&app, &key, "[1,2]", &bearer(&secret)).await;
    assert_eq!(status, StatusCode::ACCEPTED);

    // the same delivery id twice stores one event
    let mut h = bearer(&secret);
    h.push(("x-delivery-id", "delivery-1".into()));
    let (_, first) = hook(&app, &key, r#"{"a":1}"#, &h).await;
    let (status, second) = hook(&app, &key, r#"{"a":1}"#, &h).await;
    assert_eq!((first["status"].as_str(), second["status"].as_str(), status), (Some("accepted"), Some("duplicate"), StatusCode::ACCEPTED));
    assert_eq!(first["id"], second["id"]);
    assert_eq!(counts(&app, &app.token, &ext).await.0, 3);
    // a header that is not a sane id is refused
    let (status, _) = hook(&app, &key, "{}", &[("authorization", format!("Bearer {secret}")), ("x-delivery-id", "x".repeat(200))]).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn disabled_extension_refuses_after_authentication() {
    let app = app().await;
    let (ext, secret) = create(&app, &app.token, "Hooks").await;
    let key = key_of(&ext);
    let path = format!("/api/extensions/{}", id_of(&ext));
    let (status, patched) = api(&app, &app.token, "PATCH", &path, Some(json!({"enabled": false, "name": "Renamed", "description": "d"}))).await;
    assert_eq!((status, patched["enabled"].as_bool(), patched["name"].as_str()), (StatusCode::OK, Some(false), Some("Renamed")), "{patched}");

    let (status, body) = hook(&app, &key, "{}", &bearer(&secret)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::FORBIDDEN, Some("extension.disabled")), "{body}");
    // a wrong secret learns nothing about it being off
    let (status, _) = hook(&app, &key, "{}", &bearer("exs_wrong")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    api(&app, &app.token, "PATCH", &path, Some(json!({"enabled": true}))).await;
    let (status, _) = hook(&app, &key, "{}", &bearer(&secret)).await;
    assert_eq!(status, StatusCode::ACCEPTED);
}

#[tokio::test]
async fn claim_respects_max_and_the_lease() {
    let app = app().await;
    let (ext, secret) = create(&app, &app.token, "Batch").await;
    deliver_n(&app, &ext, &secret, 7).await;

    let first = claim(&app, &app.token, &ext, 3, 60).await;
    assert_eq!(first.len(), 3);
    assert_eq!(first.iter().map(|e| e["body"]["n"].as_i64().unwrap()).collect::<Vec<_>>(), vec![0, 1, 2], "oldest first");
    assert_eq!(counts(&app, &app.token, &ext).await, (4, 3, 0, 0));

    // a second claim during the lease gets the others, never the leased ones
    let second = claim(&app, &app.token, &ext, 100, 60).await;
    assert_eq!(second.len(), 4);
    for e in &second {
        assert!(!ids(&first).contains(&e["id"]), "an event was leased twice");
    }
    assert!(claim(&app, &app.token, &ext, 100, 60).await.is_empty(), "everything is leased");
    assert_eq!(counts(&app, &app.token, &ext).await, (0, 7, 0, 0));

    // acking the first batch leaves the second leased
    let (status, ack) = api(&app, &app.token, "POST", &format!("/api/extensions/{}/events/ack", id_of(&ext)), Some(json!({"ids": ids(&first)}))).await;
    assert_eq!((status, ack["acked"].as_u64()), (StatusCode::OK, Some(3)), "{ack}");
    assert_eq!(counts(&app, &app.token, &ext).await, (0, 4, 3, 0));
    // acking again (or something never leased) counts nothing
    let (_, again) = api(&app, &app.token, "POST", &format!("/api/extensions/{}/events/ack", id_of(&ext)), Some(json!({"ids": ids(&first)}))).await;
    assert_eq!(again["acked"], 0);

    // the lease runs out: the unacked four are handed out again, with a higher attempt count
    lapse_leases(&app).await;
    assert_eq!(counts(&app, &app.token, &ext).await, (4, 0, 3, 0), "a lapsed lease counts as pending");
    let redo = claim(&app, &app.token, &ext, 2, 60).await;
    assert_eq!(redo.len(), 2);
    assert!(redo.iter().all(|e| e["attempts"] == 2));
    assert_eq!(redo.iter().map(|e| e["body"]["n"].as_i64().unwrap()).collect::<Vec<_>>(), vec![3, 4]);

    // bad arguments
    let path = format!("/api/extensions/{}/events/claim", id_of(&ext));
    for body in [json!({"max": 0}), json!({"max": 501}), json!({"lease_secs": 1}), json!({"lease_secs": 7200})] {
        let (status, b) = api(&app, &app.token, "POST", &path, Some(body.clone())).await;
        assert_eq!((status, b["code"].as_str()), (StatusCode::BAD_REQUEST, Some("validation.invalid")), "{body}");
    }
    // defaults
    let (status, b) = api(&app, &app.token, "POST", &path, Some(json!({}))).await;
    assert_eq!((status, b["lease_secs"].as_i64()), (StatusCode::OK, Some(300)));
}

#[tokio::test]
async fn nack_requeues_and_the_fifth_attempt_is_dead() {
    let app = app().await;
    let (ext, secret) = create(&app, &app.token, "Flaky").await;
    deliver_n(&app, &ext, &secret, 2).await;
    let nack = format!("/api/extensions/{}/events/nack", id_of(&ext));

    for attempt in 1..=4 {
        let batch = claim(&app, &app.token, &ext, 10, 60).await;
        assert_eq!(batch.len(), 2, "attempt {attempt}");
        assert!(batch.iter().all(|e| e["attempts"] == attempt));
        let (status, out) = api(&app, &app.token, "POST", &nack, Some(json!({"ids": ids(&batch), "error": "boom"}))).await;
        assert_eq!((status, out["requeued"].as_u64(), out["dead"].as_u64()), (StatusCode::OK, Some(2), Some(0)), "{out}");
    }
    // nacking something not leased does nothing
    let (_, out) = api(&app, &app.token, "POST", &nack, Some(json!({"ids": ["extension_event:nothing"]}))).await;
    assert_eq!((out["requeued"].as_u64(), out["dead"].as_u64()), (Some(0), Some(0)));

    let last = claim(&app, &app.token, &ext, 10, 60).await;
    assert!(last.iter().all(|e| e["attempts"] == 5));
    let (_, out) = api(&app, &app.token, "POST", &nack, Some(json!({"ids": [last[0]["id"]], "error": "still broken"}))).await;
    assert_eq!((out["requeued"].as_u64(), out["dead"].as_u64()), (Some(0), Some(1)), "{out}");
    // the other one is acked instead
    api(&app, &app.token, "POST", &format!("/api/extensions/{}/events/ack", id_of(&ext)), Some(json!({"ids": [last[1]["id"]]}))).await;

    assert_eq!(counts(&app, &app.token, &ext).await, (0, 0, 1, 1));
    assert!(claim(&app, &app.token, &ext, 10, 60).await.is_empty(), "a dead event is never handed out again");
    let (status, dead) = api(&app, &app.token, "GET", &format!("/api/extensions/{}/events?status=dead", id_of(&ext)), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!((dead.as_array().unwrap().len(), dead[0]["last_error"].as_str(), dead[0]["attempts"].as_i64()), (1, Some("still broken"), Some(5)));
    let (status, _) = api(&app, &app.token, "GET", &format!("/api/extensions/{}/events?status=bogus", id_of(&ext)), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn lapsed_leases_on_the_last_attempt_dead_letter_the_event() {
    let app = app().await;
    let (ext, secret) = create(&app, &app.token, "Crashy").await;
    deliver_n(&app, &ext, &secret, 1).await;
    for _ in 0..5 {
        assert_eq!(claim(&app, &app.token, &ext, 10, 60).await.len(), 1);
        lapse_leases(&app).await; // the consumer crashed every time
    }
    // the next claim finds nothing to hand out, and dead-letters the one that is out of attempts
    assert!(claim(&app, &app.token, &ext, 10, 60).await.is_empty());
    assert_eq!(counts(&app, &app.token, &ext).await, (0, 0, 0, 1));
}

#[tokio::test]
async fn concurrent_claims_never_share_an_event() {
    let app = app().await;
    let (ext, secret) = create(&app, &app.token, "Race").await;
    deliver_n(&app, &ext, &secret, 12).await;
    let (a, b, c) = tokio::join!(
        claim(&app, &app.token, &ext, 5, 60),
        claim(&app, &app.token, &ext, 5, 60),
        claim(&app, &app.token, &ext, 5, 60),
    );
    let mut all: Vec<Value> = [a, b, c].concat().iter().map(|e| e["id"].clone()).collect();
    let total = all.len();
    all.sort_by_key(|v| v.to_string());
    all.dedup();
    assert_eq!(all.len(), total, "an event went to two consumers");
    assert_eq!(total, 12, "all of them were leased");
}

#[tokio::test]
async fn other_users_cannot_see_or_claim_an_extension() {
    let app = app().await;
    let (ext, secret) = create(&app, &app.token, "Mine").await;
    deliver_n(&app, &ext, &secret, 2).await;
    let id = id_of(&ext);

    // a user in the same org, and one in an org of their own
    let same_org = common::register(&app.state, "other@example.com").await;
    let other_org = common::register_personal(&app.state, "third@example.com").await;
    for (who, user) in [("same org", same_org), ("other org", other_org)] {
        let token = eunomia_backend::models_user::create_api_token(&app.state.control, &user.id, "t").await.unwrap().token;
        let (status, list) = api(&app, &token, "GET", "/api/extensions", None).await;
        assert_eq!((status, list.as_array().map(Vec::len)), (StatusCode::OK, Some(0)), "{who}");
        let calls: [(&str, String, Option<Value>); 8] = [
            ("GET", format!("/api/extensions/{id}"), None),
            ("PATCH", format!("/api/extensions/{id}"), Some(json!({"name": "stolen"}))),
            ("DELETE", format!("/api/extensions/{id}"), None),
            ("POST", format!("/api/extensions/{id}/rotate-secret"), None),
            ("GET", format!("/api/extensions/{id}/events"), None),
            ("POST", format!("/api/extensions/{id}/events/claim"), Some(json!({}))),
            ("POST", format!("/api/extensions/{id}/events/ack"), Some(json!({"ids": ["extension_event:x"]}))),
            ("POST", format!("/api/extensions/{id}/events/nack"), Some(json!({"ids": ["extension_event:x"]}))),
        ];
        for (method, path, body) in calls {
            let (status, b) = api(&app, &token, method, &path, body).await;
            assert_eq!((status, b["code"].as_str()), (StatusCode::NOT_FOUND, Some("extension.not_found")), "{who}: {method} {path}: {b}");
        }
        // their secret is not mine
        let (theirs, their_secret) = create(&app, &token, "Theirs").await;
        let (status, _) = hook(&app, &key_of(&ext), "{}", &bearer(&their_secret)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{who}");
        let (status, _) = hook(&app, &key_of(&theirs), "{}", &bearer(&secret)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{who}");
    }
    // the service itself filters by owner, below the id check in the router
    let db = app.db().await;
    let me = app.user.id.clone();
    let stranger = common::register(&app.state, "fourth@example.com").await.id;
    let rid = eunomia_backend::rid::parse(&id).unwrap();
    assert!(eunomia_backend::extensions::get(&db, &me, &rid).await.is_ok());
    assert!(eunomia_backend::extensions::get(&db, &stranger, &rid).await.is_err());

    // mine is untouched
    let (name, counts) = (api(&app, &app.token, "GET", &format!("/api/extensions/{id}"), None).await.1, counts(&app, &app.token, &ext).await);
    assert_eq!((name["name"].as_str(), counts), (Some("Mine"), (2, 0, 0, 0)));
    assert_eq!(claim(&app, &app.token, &ext, 10, 60).await.len(), 2);
}

#[tokio::test]
async fn scopes_decide_who_may_manage_and_who_may_consume() {
    let app = app().await;
    let mk = |scopes: Vec<String>, vault: Option<surrealdb::types::RecordId>| {
        let (control, user) = (app.state.control.clone(), app.user.id.clone());
        async move { create_api_token_with(&control, &user, "t", &scopes, vault.as_ref(), None).await.unwrap().token }
    };
    let read = mk(vec![scopes::MEMORY_READ.into()], None).await;
    let write = mk(vec![scopes::MEMORY_WRITE.into()], None).await;
    let connectors = mk(vec![scopes::CONNECTORS.into()], None).await;
    let (ext, secret) = create(&app, &connectors, "Scoped").await; // the connectors scope manages extensions
    deliver_n(&app, &ext, &secret, 2).await;
    let id = id_of(&ext);

    let code = |(s, b): (StatusCode, Value)| (s, b["code"].as_str().map(String::from));
    // manage: connectors only
    for t in [&read, &write] {
        assert_eq!(code(api(&app, t, "GET", "/api/extensions", None).await), (StatusCode::FORBIDDEN, Some("auth.scope".into())));
        assert_eq!(code(api(&app, t, "POST", "/api/extensions", Some(json!({"name": "x"}))).await), (StatusCode::FORBIDDEN, Some("auth.scope".into())));
    }
    // consume: memory:write; read may only look
    let claim_path = format!("/api/extensions/{id}/events/claim");
    assert_eq!(code(api(&app, &connectors, "POST", &claim_path, Some(json!({}))).await), (StatusCode::FORBIDDEN, Some("auth.scope".into())));
    assert_eq!(code(api(&app, &read, "POST", &claim_path, Some(json!({}))).await), (StatusCode::FORBIDDEN, Some("auth.scope".into())));
    assert_eq!(api(&app, &read, "GET", &format!("/api/extensions/{id}/events"), None).await.0, StatusCode::OK);
    assert_eq!(api(&app, &write, "POST", &claim_path, Some(json!({"max": 1}))).await.0, StatusCode::OK);
    // no credential at all
    let (status, _) = common::http(&app.router, "POST", &claim_path, Some(json!({})), None, None).await.0;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // a vault-restricted token is an account-level no
    let vault = eunomia_backend::rid::parse(app.tool("vault_list", json!({})).await["results"][0]["id"].as_str().unwrap()).unwrap();
    let restricted = mk(scopes::ALL.iter().map(|s| s.to_string()).collect(), Some(vault)).await;
    assert_eq!(api(&app, &restricted, "POST", &claim_path, Some(json!({}))).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_buffer_is_bounded() {
    let app = app().await;
    let (ext, secret) = create(&app, &app.token, "Busy").await;
    deliver_n(&app, &ext, &secret, CAP).await;
    let (status, body) = hook(&app, &key_of(&ext), "{}", &bearer(&secret)).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::TOO_MANY_REQUESTS, Some("extension.buffer_full")), "{body}");
    // another extension has its own room
    let (other, other_secret) = create(&app, &app.token, "Quiet").await;
    deliver_n(&app, &other, &other_secret, 1).await;

    // leased events still take room; acked ones free it
    let batch = claim(&app, &app.token, &ext, 5, 60).await;
    assert_eq!(hook(&app, &key_of(&ext), "{}", &bearer(&secret)).await.0, StatusCode::TOO_MANY_REQUESTS);
    api(&app, &app.token, "POST", &format!("/api/extensions/{}/events/ack", id_of(&ext)), Some(json!({"ids": ids(&batch)}))).await;
    for _ in 0..5 {
        assert_eq!(hook(&app, &key_of(&ext), "{}", &bearer(&secret)).await.0, StatusCode::ACCEPTED);
    }
    assert_eq!(hook(&app, &key_of(&ext), "{}", &bearer(&secret)).await.0, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn rotating_the_secret_invalidates_the_old_one() {
    let app = app().await;
    let (ext, old) = create(&app, &app.token, "Rotating").await;
    let key = key_of(&ext);
    deliver_n(&app, &ext, &old, 1).await;

    let (status, rotated) = api(&app, &app.token, "POST", &format!("/api/extensions/{}/rotate-secret", id_of(&ext)), None).await;
    assert_eq!(status, StatusCode::OK, "{rotated}");
    let new = rotated["secret"].as_str().unwrap().to_string();
    assert_ne!(new, old);
    assert_eq!(rotated["extension"]["webhook_url"], ext["webhook_url"], "the URL stays");
    assert_eq!(rotated["extension"]["secret_hint"].as_str().unwrap(), &new[new.len() - 4..]);

    assert_eq!(hook(&app, &key, "{}", &bearer(&old)).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(hook(&app, &key, "{}", &signed(&old, "{}")).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(hook(&app, &key, "{}", &bearer(&new)).await.0, StatusCode::ACCEPTED);
    assert_eq!(hook(&app, &key, "{}", &signed(&new, "{}")).await.0, StatusCode::ACCEPTED);
    assert_eq!(counts(&app, &app.token, &ext).await.0, 3, "events from before the rotation are kept");
}

#[tokio::test]
async fn deleting_removes_the_extension_and_its_events() {
    let app = app().await;
    let (ext, secret) = create(&app, &app.token, "Doomed").await;
    let (keep, keep_secret) = create(&app, &app.token, "Kept").await;
    deliver_n(&app, &ext, &secret, 3).await;
    deliver_n(&app, &keep, &keep_secret, 2).await;
    claim(&app, &app.token, &ext, 1, 60).await;

    let (status, body) = api(&app, &app.token, "DELETE", &format!("/api/extensions/{}", id_of(&ext)), None).await;
    assert_eq!((status, body["ok"].as_bool()), (StatusCode::OK, Some(true)));
    let (status, body) = api(&app, &app.token, "GET", &format!("/api/extensions/{}", id_of(&ext)), None).await;
    assert_eq!((status, body["code"].as_str()), (StatusCode::NOT_FOUND, Some("extension.not_found")));
    assert_eq!(hook(&app, &key_of(&ext), "{}", &bearer(&secret)).await.0, StatusCode::UNAUTHORIZED, "the inbox is gone");
    let (status, _) = api(&app, &app.token, "DELETE", &format!("/api/extensions/{}", id_of(&ext)), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let db = app.db().await;
    let mut res = db.test_raw().query("SELECT <string> extension AS extension FROM extension_event").await.unwrap();
    let left: Vec<Value> = res.take(0).unwrap();
    assert_eq!(left.len(), 2, "only the other extension's events remain: {left:?}");
    assert!(left.iter().all(|r| r["extension"].as_str() == Some(id_of(&keep).as_str())));
    assert_eq!(counts(&app, &app.token, &keep).await.0, 2);
}

#[tokio::test]
async fn old_finished_events_are_pruned_and_live_ones_are_not() {
    let app = app().await;
    let (ext, secret) = create(&app, &app.token, "Old").await;
    deliver_n(&app, &ext, &secret, 4).await;
    let batch = claim(&app, &app.token, &ext, 3, 60).await;
    api(&app, &app.token, "POST", &format!("/api/extensions/{}/events/ack", id_of(&ext)), Some(json!({"ids": ids(&batch)}))).await;
    let db = app.db().await;
    // two done events are 8 days old, one is fresh; the pending one is ancient but still waiting
    db.test_raw()
        .query("UPDATE (SELECT VALUE id FROM extension_event WHERE status = 'done' LIMIT 2) SET updated_at = time::now() - 8d; UPDATE extension_event SET updated_at = time::now() - 90d WHERE status = 'pending'")
        .await
        .unwrap()
        .check()
        .unwrap();
    eunomia_backend::extensions::prune(&db).await.unwrap();
    assert_eq!(counts(&app, &app.token, &ext).await, (1, 0, 1, 0));
}

#[tokio::test]
async fn management_calls_are_audited() {
    let app = app().await;
    let (ext, _) = create(&app, &app.token, "Audited").await;
    let id = id_of(&ext);
    api(&app, &app.token, "PATCH", &format!("/api/extensions/{id}"), Some(json!({"name": "Audited 2"}))).await;
    api(&app, &app.token, "POST", &format!("/api/extensions/{id}/rotate-secret"), None).await;
    api(&app, &app.token, "DELETE", &format!("/api/extensions/{id}"), None).await;

    let mut res = app
        .state
        .control
        .test_raw()
        .query("SELECT action, target, outcome, detail FROM audit_event WHERE action CONTAINS 'extension.' ORDER BY created_at")
        .await
        .unwrap();
    let rows: Vec<Value> = res.take(0).unwrap();
    let actions: Vec<&str> = rows.iter().map(|r| r["action"].as_str().unwrap()).collect();
    assert_eq!(actions, vec!["extension.create", "extension.update", "extension.rotate_secret", "extension.delete"]);
    assert!(rows.iter().all(|r| r["target"].as_str() == Some(id.as_str()) && r["outcome"] == "ok"));
    assert!(!serde_json::to_string(&rows).unwrap().contains("exs_"), "no secret in the audit trail");
}
