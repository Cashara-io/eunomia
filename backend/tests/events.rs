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
    let bob_token = models_user::create_api_token_with(app.control(), &bob.id, "bob", &eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect::<Vec<_>>(), None, None).await.unwrap().token;
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
    app.db().await.test_raw().query("CREATE vault:a_empty_replay SET name = 'Empty replay space', kind = 'org'; CREATE vault_member SET vault = vault:a_empty_replay, user = $user, role = 'owner'")
        .bind(("user",app.user.id.clone())).await.unwrap().check().unwrap();
    let resume = url::form_urlencoded::Serializer::new(String::new()).append_pair("ontology_after", &json!({&vault:2}).to_string()).finish();
    let mut req = format!("{base}/api/events/ws?{resume}").into_client_request().unwrap();
    req.headers_mut().insert("authorization", format!("Bearer {}", app.token).parse().unwrap());
    let mut resumed = tokio_tungstenite::connect_async(req).await.unwrap().0;
    let hello = next(&mut resumed).await;
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["ontology"]["vault:a_empty_replay"], 0);
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
    let token = models_user::create_api_token_with(app.control(), &bob.id, "Outsider", &eunomia_backend::scopes::ALL.iter().map(|s| s.to_string()).collect::<Vec<_>>(), None, None).await.unwrap().token;
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

async fn resume(base: &str, token: &str, vault: &str, seq: i64) -> Ws {
    let query = url::form_urlencoded::Serializer::new(String::new()).append_pair("ontology_after", &json!({vault:seq}).to_string()).finish();
    let mut req = format!("{base}/api/events/ws?{query}").into_client_request().unwrap();
    req.headers_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    tokio_tungstenite::connect_async(req).await.unwrap().0
}

async fn seed_activity(app: &TestApp, vault: &str, count: i64) {
    app.db().await.test_raw().query(r#"BEGIN TRANSACTION;
        FOR $i IN 1..=$count {
            CREATE ontology_event SET vault = $vault, seq = $i, actor = 'system', actor_type = 'system', actor_name = 'System',
                action = 'proposed', definition_type = 'kind', kind = 'shop', name = 'shop', target = 'ontology_kind:shop', revision = $i;
        };
        UPDATE $vault SET ontology_seq = $count;
        COMMIT TRANSACTION;"#)
        .bind(("vault", eunomia_backend::rid::parse(vault).unwrap())).bind(("count", count))
        .await.unwrap().check().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_guard_confidential_values_never_reach_stored_activity_live_frames_or_replay() {
    let app = TestApp::new().await;
    let vault = app.tool("ontology_get", json!({})).await["vault"].as_str().unwrap().to_string();
    let canary = "confidential-note-canary-207";
    let out = app.tool("ontology_propose", json!({"kinds":[{"kind":"shop","description":canary,"reason":canary}],"properties":[{"kind":"shop","name":"note","datatype":"text","description":canary,"reason":canary,"enum_values":[]}],"links":[{"from_kind":"shop","label":"near","to_kind":"location","description":canary,"reason":canary}],"metrics":[{"kind":"shop","name":"total","expr":{"agg":"count"},"description":canary,"reason":canary}]})).await;
    assert!(out.get("error").is_none(), "{out}");
    let (status, body) = app.http_session("POST", &format!("/api/vaults/{vault}/ontology/properties/decide"), Some(json!({"kind":"shop","name":"note","revision":1,"decision":{"action":"approve"}}))).await;
    assert_eq!(status,200,"{body}");
    let entity = app.tool("memory_write",json!({"subject_name":canary,"subject_kind":"shop","text":canary})).await;
    let base = serve(&app).await;
    let mut ws = connect(&base, &app.token).await;
    assert_eq!(next(&mut ws).await["type"],"hello");
    let out = app.tool("entity_set_property",json!({"entity_id":entity["entity"]["id"],"property":"note","value":canary})).await;
    assert!(out.get("error").is_none(),"{out}");
    let frame = ontology_next(&mut ws).await;
    assert!(!frame.to_string().contains(canary),"{frame}");
    let stored: Vec<Value> = app.db().await.test_raw().query("SELECT * FROM ontology_event").await.unwrap().take(0).unwrap();
    assert_eq!(stored.len(),6);
    assert!(!serde_json::to_string(&stored).unwrap().contains(canary),"{stored:?}");
    // An old row from an earlier writer must also be sanitised on replay and HTTP.
    app.db().await.test_raw().query("UPDATE ontology_event SET after = { value_text: $canary, description: $canary, values: 1 }; UPDATE ontology_event SET definition_type = 'import', name = $canary WHERE action = 'value_written'; UPDATE ontology_event SET reason = $canary WHERE action = 'proposed'")
        .bind(("canary",canary.to_string())).await.unwrap().check().unwrap();
    let mut replay = resume(&base,&app.token,&vault,0).await;
    assert_eq!(next(&mut replay).await["type"],"hello");
    for _ in 0..6 {
        let frame = ontology_next(&mut replay).await;
        assert!(!frame.to_string().contains(canary),"{frame}");
    }
    let (_, body) = app.http_session("GET",&format!("/api/vaults/{vault}/ontology/activity"),None).await;
    assert!(!body.to_string().contains(canary),"{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_guard_replay_yields_between_pages_and_revocation_stops_the_next_page() {
    use futures::SinkExt;
    let app = TestApp::new().await;
    let vault = app.tool("vault_create",json!({"name":"Replay"})).await["id"].as_str().unwrap().to_string();
    let bob = common::register(&app.state,"replay-member@example.com").await;
    let token = models_user::create_api_token_with(app.control(), &bob.id,"Member", &["memory:read".into()], Some(&eunomia_backend::rid::parse(&vault).unwrap()), None).await.unwrap().token;
    let db = app.db().await;
    let rid = eunomia_backend::rid::parse(&vault).unwrap();
    db.test_raw().query("CREATE vault_member SET vault = $vault, user = $user, role = 'member'")
        .bind(("vault",rid.clone())).bind(("user",bob.id.clone())).await.unwrap().check().unwrap();
    seed_activity(&app,&vault,201).await;
    let base = serve(&app).await;
    let mut ws = resume(&base,&token,&vault,0).await;
    assert_eq!(next(&mut ws).await["type"],"hello");
    for seq in 1..=100 { assert_eq!(ontology_next(&mut ws).await["event"]["seq"],seq); }
    // Control traffic must be served before the second page, without draining the backlog.
    ws.send(Message::Ping(vec![2,0,7].into())).await.unwrap();
    let message = tokio::time::timeout(std::time::Duration::from_secs(2),ws.next()).await.unwrap().unwrap().unwrap();
    assert!(matches!(message, Message::Pong(_)),"{message:?}");
    // No local bus announcement: the next replay pass must query membership again.
    db.test_raw().query("DELETE vault_member WHERE vault = $vault AND user = $user")
        .bind(("vault",rid)).bind(("user",bob.id)).await.unwrap().check().unwrap();
    let leaked = tokio::time::timeout(std::time::Duration::from_secs(12),async {
        while let Some(Ok(message)) = ws.next().await {
            if let Message::Text(text) = message {
                let frame: Value = serde_json::from_str(&text).unwrap();
                if frame["type"] == "ontology" { return true; }
            }
        }
        false
    }).await.unwrap_or(false);
    assert!(!leaked,"revoked member received another page");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_guard_large_replay_requests_http_resync_then_continues_live_delivery() {
    let app = TestApp::new().await;
    let vault = app.tool("ontology_get",json!({})).await["vault"].as_str().unwrap().to_string();
    seed_activity(&app,&vault,1001).await;
    let base = serve(&app).await;
    let mut ws = resume(&base,&app.token,&vault,0).await;
    assert_eq!(next(&mut ws).await["type"],"hello");
    let frame = next(&mut ws).await;
    assert_eq!(frame,json!({"type":"ontology_resync","vault":vault,"seq":1001}));
    let (status, page) = app.http_session("GET",&format!("/api/vaults/{vault}/ontology/activity?newest=true&limit=1"),None).await;
    assert_eq!(status,200);
    assert_eq!(page["events"][0]["seq"],1001);
    app.tool("ontology_propose",json!({"kinds":[{"kind":"supplier"}]})).await;
    assert_eq!(ontology_next(&mut ws).await["event"]["seq"],1002);
}
