//! The per-user live stream (`GET /api/events/ws`, routers/changes.rs, #132): one socket per browser
//! tab that carries both the change logs of every vault the user can read and what happens to the
//! vaults themselves, so an open page sees an agent's MCP writes without a reload.

mod common;

use common::TestApp;
use eunomia_backend::models_user;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The app on a real port, for WebSocket clients.
async fn serve(app: &TestApp) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router.clone();
    tokio::spawn(async move { axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });
    format!("ws://{addr}")
}

async fn connect(base: &str, token: &str) -> Ws {
    let mut req = format!("{base}/api/events/ws").into_client_request().unwrap();
    req.headers_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    tokio_tungstenite::connect_async(req).await.expect("connect").0
}

/// The next message that is not a heartbeat.
async fn next(ws: &mut Ws) -> Value {
    loop {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(10), ws.next()).await.expect("a message within 10 s").expect("open").expect("ok");
        if let Message::Text(t) = msg {
            let v: Value = serde_json::from_str(&t).unwrap();
            if v["type"] != "heartbeat" {
                return v;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_socket_carries_vault_events_and_content_changes() {
    let app = TestApp::new().await;
    let base = serve(&app).await;
    let mut ws = connect(&base, &app.token).await;
    let hello = next(&mut ws).await;
    assert_eq!(hello["type"], "hello", "{hello}");
    assert_eq!(hello["vaults"].as_object().map(|o| o.len()), Some(1), "the personal vault: {hello}");

    // an MCP-style vault_create is one `vault` event, and the next thing is the new vault's first change
    let team = app.tool("vault_create", json!({"name": "Team"})).await["id"].as_str().unwrap().to_string();
    let ev = next(&mut ws).await;
    assert_eq!((ev["type"].clone(), ev["op"].clone(), ev["vault"].clone()), (json!("vault"), json!("created"), json!(team)), "{ev}");
    let w = app.tool("memory_write", json!({"subject_name": "Ada", "subject_kind": "person", "text": "f", "vault_id": team})).await;
    let ev = next(&mut ws).await;
    assert_eq!((ev["type"].clone(), ev["kind"].clone(), ev["vault"].clone(), ev["seq"].clone()), (json!("change"), json!("entity"), json!(team), json!(1)), "{ev}");
    let ev = next(&mut ws).await;
    assert_eq!((ev["kind"].clone(), ev["id"].clone()), (json!("memory"), w["memory"]["id"].clone()), "{ev}");

    // memory_delete: exactly one change, for that memory
    app.tool("memory_delete", json!({"memory_id": w["memory"]["id"]})).await;
    let ev = next(&mut ws).await;
    assert_eq!((ev["op"].clone(), ev["kind"].clone(), ev["id"].clone()), (json!("deleted"), json!("memory"), w["memory"]["id"].clone()), "{ev}");

    app.tool("vault_rename", json!({"vault_id": team, "name": "Team 2"})).await;
    let ev = next(&mut ws).await;
    assert_eq!((ev["type"].clone(), ev["op"].clone()), (json!("vault"), json!("renamed")), "{ev}");

    app.tool("vault_delete", json!({"vault_id": team})).await;
    let ev = next(&mut ws).await;
    assert_eq!((ev["type"].clone(), ev["op"].clone(), ev["vault"].clone()), (json!("vault"), json!("deleted"), json!(team)), "{ev}");

    // a write in the deleted vault's place no longer reaches the socket; one in the personal vault does
    app.tool("memory_write", json!({"subject_name": "Bo", "subject_kind": "person", "text": "x"})).await;
    let ev = next(&mut ws).await;
    assert_eq!(ev["type"], "change", "{ev}");
    assert_ne!(ev["vault"], json!(team), "{ev}");
    ws.close(None).await.unwrap();

    // reconnect: hello reports the personal vault's current seq, so a client that missed writes knows
    app.tool("memory_write", json!({"subject_name": "Bo", "subject_kind": "person", "text": "while away"})).await;
    let mut ws = connect(&base, &app.token).await;
    let hello = next(&mut ws).await;
    let seqs: Vec<i64> = hello["vaults"].as_object().unwrap().values().map(|v| v.as_i64().unwrap()).collect();
    assert_eq!(seqs, [3], "2 rows before the disconnect, 1 while away: {hello}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_invitee_hears_the_invitation_and_a_removed_member_hears_the_removal() {
    let app = TestApp::new().await;
    let base = serve(&app).await;
    let bob = common::register(&app.state, "bob@example.com").await;
    let bob_token = models_user::create_api_token(app.control(), &bob.id, "bob").await.unwrap().token;
    let team = app.tool("vault_create", json!({"name": "Team"})).await["id"].as_str().unwrap().to_string();

    let mut ws = connect(&base, &bob_token).await;
    assert_eq!(next(&mut ws).await["type"], "hello");
    app.tool("vault_invite", json!({"vault_id": team, "email": "bob@example.com"})).await;
    let ev = next(&mut ws).await;
    assert_eq!((ev["op"].clone(), ev["vault"].clone()), (json!("member_invited"), json!(team)), "{ev}");

    // not a member yet: the team's content changes are not sent
    app.tool("memory_write", json!({"subject_name": "Ada", "subject_kind": "person", "text": "f", "vault_id": team})).await;
    app.tool("vault_remove_member", json!({"vault_id": team, "email": "bob@example.com"})).await;
    let ev = next(&mut ws).await;
    assert_eq!((ev["type"].clone(), ev["op"].clone()), (json!("vault"), json!("member_removed")), "{ev}");
}

async fn ontology_next(ws: &mut Ws) -> Value {
    loop { let event = next(ws).await; if event["type"] == "ontology" { return event; } }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ontology_resume_replays_exactly_the_missed_events_and_honours_vault_scope() {
    let app = TestApp::new().await;
    let base = serve(&app).await;
    let vault = app.tool("ontology_get", json!({})).await["vault"].as_str().unwrap().to_string();
    let other = app.tool("vault_create", json!({"name":"Other"})).await["id"].as_str().unwrap().to_string();
    let restricted = models_user::create_api_token_with(app.control(), &app.user.id, "Other vault only", &["memory:read".into()], Some(&eunomia_backend::rid::parse(&other).unwrap()), None).await.unwrap().token;
    let mut scoped = connect(&base, &restricted).await;
    let hello = next(&mut scoped).await;
    assert!(hello["ontology"].get(&vault).is_none(), "{hello}");
    let mut ws = connect(&base, &app.token).await;
    assert_eq!(next(&mut ws).await["type"], "hello");
    for name in ["shop", "supplier"] {
        let out = app.tool("ontology_propose", json!({"vault_id":vault,"kinds":[{"kind":name}]})).await;
        assert!(out.get("error").is_none(), "{out}");
    }
    assert_eq!(ontology_next(&mut ws).await["event"]["seq"], 1);
    assert_eq!(ontology_next(&mut ws).await["event"]["seq"], 2);
    ws.close(None).await.unwrap();
    for name in ["order", "shipment"] {
        app.tool("ontology_propose", json!({"vault_id":vault,"kinds":[{"kind":name}]})).await;
    }
    let resume = url::form_urlencoded::Serializer::new(String::new()).append_pair("ontology_after", &json!({&vault:2}).to_string()).finish();
    let mut req = format!("{base}/api/events/ws?{resume}").into_client_request().unwrap();
    req.headers_mut().insert("authorization", format!("Bearer {}", app.token).parse().unwrap());
    let mut resumed = tokio_tungstenite::connect_async(req).await.unwrap().0;
    assert_eq!(next(&mut resumed).await["type"], "hello");
    assert_eq!(ontology_next(&mut resumed).await["event"]["seq"], 3);
    assert_eq!(ontology_next(&mut resumed).await["event"]["seq"], 4);
    assert!(tokio::time::timeout(std::time::Duration::from_millis(1200), ontology_next(&mut resumed)).await.is_err(), "no duplicate replay");
    assert!(tokio::time::timeout(std::time::Duration::from_millis(1200), ontology_next(&mut scoped)).await.is_err(), "no other-vault event");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ontology_membership_is_checked_on_subscribe_and_again_before_delivery() {
    let app = TestApp::new().await;
    let base = serve(&app).await;
    let vault = app.tool("ontology_get", json!({})).await["vault"].as_str().unwrap().to_string();
    let bob = common::register(&app.state, "activity-outsider@example.com").await;
    let token = models_user::create_api_token(app.control(), &bob.id, "Outsider").await.unwrap().token;
    let mut ws = connect(&base, &token).await;
    assert!(next(&mut ws).await["ontology"].get(&vault).is_none());
    app.tool("ontology_propose", json!({"vault_id":vault,"kinds":[{"kind":"shop"}]})).await;
    assert!(tokio::time::timeout(std::time::Duration::from_millis(1200), ontology_next(&mut ws)).await.is_err());
    // Simulate a membership update on another replica, with no local announcement.
    let db = app.db().await;
    db.test_raw().query("CREATE vault_member SET vault = $vault, user = $user, role = 'member'")
        .bind(("vault", eunomia_backend::rid::parse(&vault).unwrap())).bind(("user", bob.id.clone()))
        .await.unwrap().check().unwrap();
    assert_eq!(ontology_next(&mut ws).await["event"]["seq"], 1);
    db.test_raw().query("DELETE vault_member WHERE vault = $vault AND user = $user")
        .bind(("vault", eunomia_backend::rid::parse(&vault).unwrap())).bind(("user", bob.id))
        .await.unwrap().check().unwrap();
    app.tool("ontology_propose", json!({"vault_id":vault,"kinds":[{"kind":"supplier"}]})).await;
    assert!(tokio::time::timeout(std::time::Duration::from_millis(1500), ontology_next(&mut ws)).await.is_err(), "revoked member gets no event");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_socket_opened_during_the_schema_upgrade_reconnects_for_ontology_events() {
    use futures::SinkExt;
    let app = TestApp::new().await;
    app.control().test_raw().query("UPDATE $tenant SET schema_version = 40")
        .bind(("tenant", app.user.org.tenant_record())).await.unwrap().check().unwrap();
    app.state.pool.evict(&app.user.org);
    let base = serve(&app).await;
    let mut ws = connect(&base, &app.token).await;
    assert_eq!(next(&mut ws).await["ontology"], json!({}));
    app.control().test_raw().query("UPDATE $tenant SET schema_version = 41")
        .bind(("tenant", app.user.org.tenant_record())).await.unwrap().check().unwrap();
    app.state.pool.evict(&app.user.org);
    ws.send(Message::Ping(Default::default())).await.unwrap();
    let closed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(Ok(message)) = ws.next().await {
            if let Message::Close(Some(close)) = message { return u16::from(close.code); }
        }
        0
    }).await.unwrap();
    assert_eq!(closed, 4410);
    let mut resumed = connect(&base, &app.token).await;
    assert!(!next(&mut resumed).await["ontology"].as_object().unwrap().is_empty());
}
