//! Multi-connect (issue #100): one user connects the same kind more than once. Each connection syncs
//! on its own cursor, every record names the connection it came from, removing one connection leaves
//! the other, a webhook reaches the connection whose secret signed it, and nobody else can see or use a
//! user's connection.

mod common;

use std::sync::Once;

use axum::body::Body;
use common::{test_settings, TestApp};
use eunomia_backend::connectors::service::{self, Connector};
use eunomia_backend::models_user;
use eunomia_backend::rid::RecordIdExt;
use eunomia_backend::sources::base::owner_key_str;
use eunomia_backend::sources::mock::{route, serve};
use eunomia_backend::sources::registry;
use eunomia_backend::sources::scheduler::{due_syncs, sync_source};
use eunomia_backend::state::OrgState;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use surrealdb::types::RecordId;
use tower::ServiceExt;

fn allow_base_url() {
    static ONCE: Once = Once::new();
    // SAFETY: set once before any test in this binary reads it.
    ONCE.call_once(|| unsafe { std::env::set_var("EUNOMIA_ALLOW_CONNECTOR_BASE_URL", "1") });
}

async fn app() -> (TestApp, OrgState) {
    allow_base_url();
    let app = TestApp::with_settings(test_settings()).await;
    let org = app.state.org(&app.user.org).await.unwrap();
    (app, org)
}

fn issue(number: i64, title: &str, updated: &str) -> Value {
    json!({
        "number": number, "title": title, "state": "open", "body": "", "user": {"login": "ada"}, "labels": [],
        "assignees": [], "comments": 0, "repository": {"full_name": "acme/widget"},
        "html_url": format!("https://github.com/acme/widget/issues/{number}"),
        "created_at": "2024-01-01T00:00:00Z", "updated_at": updated,
    })
}

async fn set_up(org: &OrgState, row: Connector, base: &str, credentials: Value) -> Connector {
    service::update_connector(&org.db, &org.settings.encryption_key, row, Some(true), Some(json!({"base_url": base})), Some(credentials), None)
        .await
        .unwrap()
}

async fn cursor(org: &OrgState, owner: &RecordId, key: &str) -> String {
    let id = RecordId::new("sync_status", format!("{}:{key}", owner_key_str(owner)));
    let mut res = org.db.test_raw().query("SELECT VALUE cursor FROM ONLY $id").bind(("id", id)).await.unwrap();
    res.take::<Option<String>>(0).unwrap().unwrap_or_default()
}

/// Connector ids of the owner's live records with `title`.
async fn record_connectors(org: &OrgState, owner: &RecordId) -> Vec<(String, Option<RecordId>)> {
    let mut res = org
        .db
        .test_raw()
        .query("SELECT title, connector FROM cache_record WHERE owner = $owner AND deleted = false ORDER BY title")
        .bind(("owner", owner.clone()))
        .await
        .unwrap();
    let rows: Vec<Value> = res.take(0).unwrap();
    rows.into_iter()
        .map(|r| {
            let conn = r["connector"].as_str().map(|s| eunomia_backend::rid::parse(s).unwrap());
            (r["title"].as_str().unwrap().to_string(), conn)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_connections_of_one_kind_sync_independently_and_removing_one_leaves_the_other() {
    let (app, org) = app().await;
    let owner = app.user.id.clone();
    let work = serve(vec![route("GET", "/issues", json!([issue(1, "Work issue", "2024-02-01T00:00:00Z")]))]).await;
    let home = serve(vec![route("GET", "/issues", json!([issue(2, "Home issue", "2024-03-01T00:00:00Z")]))]).await;

    let first = service::get_or_create_connector(&org.db, &owner, "github").await.unwrap();
    let first = set_up(&org, first, &work.base, json!({"personal_access_token": "ghp_work"})).await;
    let second = service::create_connection(&org.db, &owner, "github", "home account").await.unwrap();
    assert!(service::create_connection(&org.db, &owner, "github", "  ").await.is_err(), "an added connection needs a name");
    let second = set_up(&org, second, &home.base, json!({"personal_access_token": "ghp_home"})).await;
    let second_key = registry::sync_key(&eunomia_backend::sources::github::GitHubSource, &second);
    assert_eq!(second_key, format!("github~{}", second.instance));

    let mut due: Vec<String> = due_syncs(&org).await.unwrap().into_iter().map(|d| d.source).collect();
    due.sort();
    assert_eq!(due, vec!["github".to_string(), second_key.clone()], "each connection is its own sync");

    assert_eq!(sync_source(&org, &owner, "github", None).await["written"], 1);
    assert_eq!(sync_source(&org, &owner, &second_key, None).await["written"], 1);
    assert_eq!(work.requests()[0].header("authorization"), "Bearer ghp_work");
    assert_eq!(home.requests()[0].header("authorization"), "Bearer ghp_home", "each connection uses its own credentials");
    assert_eq!(cursor(&org, &owner, "github").await, "2024-02-01T00:00:00Z");
    assert_eq!(cursor(&org, &owner, &second_key).await, "2024-03-01T00:00:00Z", "separate cursors");
    assert_eq!(
        record_connectors(&org, &owner).await,
        vec![("Home issue".to_string(), Some(second.id.clone())), ("Work issue".to_string(), Some(first.id.clone()))],
        "each record names its connection"
    );

    // The list shows both connections; a kind still resolves to its first connection.
    let (status, list) = app.http("GET", "/api/connectors", None, true).await;
    assert_eq!(status, 200);
    let github: Vec<&Value> = list.as_array().unwrap().iter().filter(|c| c["kind"] == "github").collect();
    assert_eq!(github.len(), 2);
    assert_eq!(github[1]["label"], "home account");
    assert_eq!(github[1]["source_key"], second_key.as_str());
    let (_, by_kind) = app.http("GET", "/api/connectors/github", None, true).await;
    assert_eq!(by_kind["id"], first.id.to_string());
    let (_, sources) = app.http("GET", "/api/sources", None, true).await;
    let rows: Vec<&Value> = sources.as_array().unwrap().iter().filter(|s| s["provider"] == "github").collect();
    assert_eq!(rows.iter().map(|r| r["record_count"].as_i64().unwrap()).collect::<Vec<_>>(), vec![1, 1]);

    // Removing the added connection takes its records and status, and leaves the first one alone.
    let (status, out) = app.http("DELETE", &format!("/api/connectors/{}", second.id.to_string()), None, true).await;
    assert_eq!(status, 200, "{out}");
    assert_eq!(out["records"], 1);
    assert_eq!(record_connectors(&org, &owner).await, vec![("Work issue".to_string(), Some(first.id.clone()))]);
    assert_eq!(cursor(&org, &owner, &second_key).await, "");
    assert_eq!(cursor(&org, &owner, "github").await, "2024-02-01T00:00:00Z");
    let (status, _) = app.http("GET", &format!("/api/connectors/{}", second.id.to_string()), None, true).await;
    assert_eq!(status, 404);
    assert_eq!(due_syncs(&org).await.unwrap().len(), 0, "the first connection synced just now and the removed one is gone");
}

/// Records synced before 2.0.4 carry no connection: they belong to the kind's first connection, so its
/// "Delete data" still takes them and an added connection's does not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn records_from_before_multi_connect_belong_to_the_first_connection() {
    let (app, org) = app().await;
    let owner = app.user.id.clone();
    let first = service::get_or_create_connector(&org.db, &owner, "github").await.unwrap();
    let second = service::create_connection(&org.db, &owner, "github", "other").await.unwrap();
    let raw = org.db.test_raw();
    raw.query("CREATE cache_record:legacy SET owner = $owner, source = 'github', type = 'github.issue', external_id = '9', content_hash = 'h', ingested_at = time::now(), updated_at = time::now()")
        .bind(("owner", owner.clone()))
        .await
        .unwrap()
        .check()
        .unwrap();
    assert_eq!(registry::delete_data(&org.db, &owner, &second).await.unwrap()["records"], 0);
    assert_eq!(registry::delete_data(&org.db, &owner, &first).await.unwrap()["records"], 1);
}

/// One webhook URL serves every Up connection: the delivery lands on the one whose secret signed it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_webhook_reaches_the_connection_whose_secret_signed_it() {
    let (app, org) = app().await;
    let owner = app.user.id.clone();
    let txn = json!({"data": {"type": "transactions", "id": "t9",
        "attributes": {"description": "Coles", "createdAt": "2024-03-01T00:00:00+11:00", "status": "HELD",
                       "amount": {"value": "-5.00", "valueInBaseUnits": -500, "currencyCode": "AUD"}},
        "relationships": {}}});
    let mock = serve(vec![route("GET", "/transactions/t9", txn)]).await;
    let first = service::get_or_create_connector(&org.db, &owner, "up_bank").await.unwrap();
    set_up(&org, first, &mock.base, json!({"personal_access_token": "up:yeah:a", "webhook_secret_key": "secret-a"})).await;
    let second = service::create_connection(&org.db, &owner, "up_bank", "joint account").await.unwrap();
    let second = set_up(&org, second, &mock.base, json!({"personal_access_token": "up:yeah:b", "webhook_secret_key": "secret-b"})).await;

    let body = json!({"data": {"attributes": {"eventType": "TRANSACTION_CREATED"},
        "relationships": {"transaction": {"data": {"id": "t9"}, "links": {"related": format!("{}/transactions/t9", mock.base)}}}}})
    .to_string();
    let sign = |secret: &[u8]| {
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret).unwrap();
        mac.update(body.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    };
    let deliver = |sig: String| {
        let req = axum::http::Request::post(format!("/api/sources/up_bank/webhook/{}", owner.to_string()))
            .header("X-Up-Authenticity-Signature", sig)
            .body(Body::from(body.clone()))
            .unwrap();
        app.router.clone().oneshot(req)
    };

    let resp = deliver(sign(b"nobody")).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(record_connectors(&org, &owner).await.is_empty(), "an unsigned delivery is ignored");

    let resp = deliver(sign(b"secret-b")).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(record_connectors(&org, &owner).await, vec![("Coles".to_string(), Some(second.id.clone()))]);
    assert_eq!(mock.requests().last().unwrap().header("authorization"), "Bearer up:yeah:b", "fetched with that connection's token");
}

/// Another user (same org) can neither read, change, sync nor remove someone's connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_user_cannot_see_or_use_a_connection() {
    let (app, org) = app().await;
    let owner = app.user.id.clone();
    let mine = service::create_connection(&org.db, &owner, "github", "private").await.unwrap();
    let other = common::register(&app.state, "other@example.com").await;
    let token = models_user::create_api_token(&app.state.control, &other.id, "t").await.unwrap().token;
    let call = |method: &'static str, path: String, body: Option<Value>| {
        let (router, token) = (app.router.clone(), token.clone());
        async move { common::http(&router, method, &path, body, Some(&token), None).await.0 }
    };
    let id = mine.id.to_string();
    for (method, path, body) in [
        ("GET", format!("/api/connectors/{id}"), None),
        ("PUT", format!("/api/connectors/{id}"), Some(json!({"enabled": true}))),
        ("POST", format!("/api/connectors/{id}/test"), None),
        ("DELETE", format!("/api/connectors/{id}"), None),
        ("DELETE", format!("/api/connectors/{id}/data"), None),
        ("POST", format!("/api/connectors/{}/move", eunomia_backend::rid::key_string(mine.id.key()).unwrap()), None),
    ] {
        let (status, out) = call(method, path.clone(), body).await;
        assert_eq!(status, 404, "{method} {path}: {out}");
    }
    let (status, _) = call("POST", format!("/api/sources/github~{}/sync", mine.instance), None).await;
    assert_eq!(status, 404);
    let (_, list) = call("GET", "/api/connectors".into(), None).await;
    assert!(!list.to_string().contains(&id), "not listed for another user");
    assert!(service::resolve(&org.db, &owner, &id).await.is_ok(), "still there for its owner");
}
