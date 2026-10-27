//! The per-vault change log (`changes.rs`, migration 0022): numbered rows for every write, read over
//! REST, the `changes_since` tool and a WebSocket, with the same vault rules as any vault route.

mod common;

use common::TestApp;
use eunomia_backend::models_user::create_api_token_with;
use eunomia_backend::rid;
use eunomia_backend::scopes;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

fn seqs(page: &Value) -> Vec<i64> {
    page["changes"].as_array().unwrap_or_else(|| panic!("{page}")).iter().map(|c| c["seq"].as_i64().unwrap()).collect()
}

fn ops(page: &Value) -> Vec<String> {
    page["changes"].as_array().unwrap().iter().map(|c| format!("{} {}", c["op"].as_str().unwrap(), c["kind"].as_str().unwrap())).collect()
}

async fn personal(app: &TestApp) -> String {
    let vaults = app.tool("vault_list", json!({})).await;
    vaults["vaults"].as_array().or(vaults["results"].as_array()).unwrap_or_else(|| panic!("{vaults}")).iter().find(|v| v["kind"] == "personal").unwrap()["id"].as_str().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_write_appends_one_numbered_row_without_content() {
    let app = TestApp::new().await;
    // concurrent writers in one vault: the seqs still come out 1..=n with no gap
    let writes = (0..12).map(|i| app.tool("memory_write", json!({"subject_name": format!("Person {}", i % 3), "subject_kind": "person", "text": format!("secret fact {i}")})));
    let written = futures::future::join_all(writes).await;
    assert!(written.iter().all(|w| w.get("error").is_none()), "{written:?}");
    let vault = personal(&app).await;
    let (status, page) = app.http("GET", &format!("/api/vaults/{vault}/changes?limit=1000"), None, true).await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(seqs(&page), (1..=15).collect::<Vec<_>>(), "3 entities and 12 memories: {page}");
    assert_eq!(ops(&page).iter().filter(|o| *o == "created entity").count(), 3);
    assert!(!page.to_string().contains("secret fact"), "no content in the log: {page}");
    assert_eq!(page["next"], 15);
    assert_eq!(page["changes"][0]["actor"], "system", "these tool calls ran as the system: {page}");

    // update, merge, delete, skills
    let mem = written[0]["memory"]["id"].as_str().unwrap();
    app.tool("memory_update", json!({"memory_id": mem, "text": "changed"})).await;
    app.tool("memory_delete", json!({"memory_id": mem})).await;
    let (a, b) = (written[0]["entity"]["id"].as_str().unwrap(), written[1]["entity"]["id"].as_str().unwrap());
    app.tool("entity_update", json!({"entity_id": a, "summary": "s"})).await;
    app.tool("entity_merge", json!({"winner_id": a, "loser_id": b})).await;
    let skill = app.tool("skill_save", json!({"name": "n", "description": "d", "body": "b"})).await;
    app.tool("skill_save", json!({"name": "n", "description": "d", "body": "b2"})).await;
    app.tool("skill_delete", json!({"id": skill["id"]})).await;
    let doc = app.tool("document_upload", json!({"filename": "a.md", "text": "hello"})).await;
    app.tool("document_delete", json!({"id": doc["id"]})).await;

    let (_, page) = app.http("GET", &format!("/api/vaults/{vault}/changes?after=15"), None, true).await;
    assert_eq!(
        ops(&page),
        [
            "updated memory", "deleted memory", "updated entity", "merged entity", "updated entity",
            "created skill", "updated skill", "deleted skill", "created document", "deleted document",
        ],
        "{page}"
    );
    assert_eq!(seqs(&page), (16..=25).collect::<Vec<_>>());
    assert_eq!(page["changes"][3]["id"], b, "the merged-away entity");

    // paging and the tool
    let (_, one) = app.http("GET", &format!("/api/vaults/{vault}/changes?after=3&limit=2"), None, true).await;
    assert_eq!((seqs(&one), one["next"].clone()), (vec![4, 5], json!(5)));
    let (_, none) = app.http("GET", &format!("/api/vaults/{vault}/changes?after=99"), None, true).await;
    assert_eq!((seqs(&none), none["next"].clone()), (vec![], json!(99)), "an empty page keeps the cursor");
    let tool = app.tool("changes_since", json!({"after": 24})).await;
    assert_eq!(seqs(&tool), [25], "{tool}");
}

#[tokio::test]
async fn a_vault_restricted_token_reads_its_own_vault_only() {
    let app = TestApp::new().await;
    let team = app.tool("vault_create", json!({"name": "Team"})).await["id"].as_str().unwrap().to_string();
    let vault = personal(&app).await;
    let granted = vec![scopes::MEMORY_WRITE.to_string()];
    let token = create_api_token_with(app.control(), &app.user.id, "project", &granted, Some(&rid::parse(&team).unwrap()), None).await.unwrap().token;

    // a write over REST with the restricted token: the actor is that token
    let ((status, w), _) = common::http(&app.router, "POST", "/api/tools/memory_write", Some(json!({"subject_name": "Ada", "subject_kind": "person", "text": "t", "vault_id": team})), Some(&token), None).await;
    assert!(status == 200 && w.get("error").is_none(), "{w}");
    let ((status, page), _) = common::http(&app.router, "GET", &format!("/api/vaults/{team}/changes"), None, Some(&token), None).await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(seqs(&page), [1, 2]);
    assert!(page["changes"][0]["actor"].as_str().unwrap().starts_with("api_token:"), "{page}");

    let ((status, refused), _) = common::http(&app.router, "GET", &format!("/api/vaults/{vault}/changes"), None, Some(&token), None).await;
    assert_eq!(status, 403, "{refused}");
    let ((_, tool), _) = common::http(&app.router, "POST", "/api/tools/changes_since", Some(json!({"vault_id": vault})), Some(&token), None).await;
    assert_eq!(tool["code"], "auth.scope", "{tool}");
    let ((_, tool), _) = common::http(&app.router, "POST", "/api/tools/changes_since", Some(json!({})), Some(&token), None).await;
    assert_eq!(seqs(&tool), [1, 2], "the default vault of a restricted token is its vault: {tool}");

    // another user is not a member of the vault
    let other = common::register(&app.state, "other@example.com").await;
    let other_token = eunomia_backend::models_user::create_api_token(app.control(), &other.id, "o").await.unwrap().token;
    let ((status, _), _) = common::http(&app.router, "GET", &format!("/api/vaults/{team}/changes"), None, Some(&other_token), None).await;
    assert_eq!(status, 403);
}

/// The app on a real port, for WebSocket clients.
async fn serve(app: &TestApp) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router.clone();
    tokio::spawn(async move { axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });
    format!("ws://{addr}")
}

async fn next_change(ws: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin)) -> Value {
    loop {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(10), ws.next()).await.expect("a change within 10 s").expect("open").expect("ok");
        if let Message::Text(t) = msg {
            return serde_json::from_str(&t).unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_websocket_sends_the_backlog_then_live_changes() {
    let app = TestApp::new().await;
    let base = serve(&app).await;
    let team = app.tool("vault_create", json!({"name": "Team"})).await["id"].as_str().unwrap().to_string();
    for i in 0..3 {
        app.tool("memory_write", json!({"subject_name": "Ada", "subject_kind": "person", "text": format!("f{i}"), "vault_id": team})).await;
    }

    let mut req = format!("{base}/api/vaults/{team}/changes/ws?after=1").into_client_request().unwrap();
    req.headers_mut().insert("authorization", format!("Bearer {}", app.token).parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.expect("connect");
    assert_eq!(next_change(&mut ws).await["seq"], 2, "the backlog after the cursor");
    assert_eq!(next_change(&mut ws).await["seq"], 3);
    assert_eq!(next_change(&mut ws).await["seq"], 4);
    app.tool("memory_write", json!({"subject_name": "Ada", "subject_kind": "person", "text": "live", "vault_id": team})).await;
    let live = next_change(&mut ws).await;
    assert_eq!((live["seq"].clone(), live["op"].clone(), live["kind"].clone()), (json!(5), json!("created"), json!("memory")), "{live}");
    // a write in another vault is not sent
    app.tool("memory_write", json!({"subject_name": "Bo", "subject_kind": "person", "text": "elsewhere"})).await;
    app.tool("memory_write", json!({"subject_name": "Ada", "subject_kind": "person", "text": "again", "vault_id": team})).await;
    assert_eq!(next_change(&mut ws).await["seq"], 6);
    ws.close(None).await.unwrap();

    // a restricted token for another vault, and a cookie from a foreign page, never get a socket
    let personal = personal(&app).await;
    let token = create_api_token_with(app.control(), &app.user.id, "p", &[scopes::MEMORY_WRITE.to_string()], Some(&rid::parse(&team).unwrap()), None).await.unwrap().token;
    let mut req = format!("{base}/api/vaults/{personal}/changes/ws").into_client_request().unwrap();
    req.headers_mut().insert("authorization", format!("Bearer {token}").parse().unwrap());
    let err = tokio_tungstenite::connect_async(req).await.expect_err("refused");
    assert!(err.to_string().contains("403"), "{err}");
    let mut req = format!("{base}/api/vaults/{team}/changes/ws").into_client_request().unwrap();
    req.headers_mut().insert("cookie", app.session_cookie().await.parse().unwrap());
    req.headers_mut().insert("origin", "https://evil.example".parse().unwrap());
    let err = tokio_tungstenite::connect_async(req).await.expect_err("refused");
    assert!(err.to_string().contains("403"), "{err}");
}
