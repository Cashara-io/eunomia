//! The per-vault change log (`changes.rs`, migration 0023): numbered rows for every write, read over
//! REST, the `changes_since` tool and a WebSocket, with the same vault rules as any vault route.

mod common;

use common::TestApp;
use eunomia_backend::changes::{self, Retention};
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

    // update, merge, delete, skills: each call must succeed, or the log below is missing its row
    let ok = |name: &'static str, r: Value| {
        assert!(r.get("error").is_none() && r.get("code").is_none(), "{name}: {r}");
        r
    };
    let mem = written[0]["memory"]["id"].as_str().unwrap();
    ok("memory_update", app.tool("memory_update", json!({"memory_id": mem, "text": "changed"})).await);
    ok("memory_delete", app.tool("memory_delete", json!({"memory_id": mem})).await);
    let (a, b) = (written[0]["entity"]["id"].as_str().unwrap(), written[1]["entity"]["id"].as_str().unwrap());
    ok("entity_update", app.tool("entity_update", json!({"entity_id": a, "summary": "s"})).await);
    ok("entity_merge", app.tool("entity_merge", json!({"winner_id": a, "loser_id": b})).await);
    let skill = ok("skill_save", app.tool("skill_save", json!({"name": "n", "description": "d", "body": "b"})).await);
    ok("skill_save", app.tool("skill_save", json!({"name": "n", "description": "d", "body": "b2"})).await);
    ok("skill_delete", app.tool("skill_delete", json!({"id": skill["id"]})).await);
    let doc = ok("document_upload", app.tool("document_upload", json!({"filename": "a.md", "text": "hello"})).await);
    ok("document_delete", app.tool("document_delete", json!({"id": doc["id"]})).await);

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
    let (_, none) = app.http("GET", &format!("/api/vaults/{vault}/changes?after=25"), None, true).await;
    assert_eq!((seqs(&none), none["next"].clone()), (vec![], json!(25)), "an empty page keeps the cursor");
    // a cursor past the newest change (a restored database) is not silently an empty page
    let (status, ahead) = app.http("GET", &format!("/api/vaults/{vault}/changes?after=99"), None, true).await;
    assert_eq!((status.as_u16(), ahead["code"].clone()), (410, json!("changes.resync")), "{ahead}");
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

#[tokio::test]
async fn relations_and_what_a_vault_clone_copies_are_logged() {
    let app = TestApp::new().await;
    let team = app.tool("vault_create", json!({"name": "Team"})).await["id"].as_str().unwrap().to_string();
    let a = app.tool("memory_write", json!({"subject_name": "Ada", "subject_kind": "person", "text": "a", "vault_id": team})).await;
    let b = app.tool("memory_write", json!({"subject_name": "Acme", "subject_kind": "organisation", "text": "b", "vault_id": team})).await;
    let rel = app.tool("code_relate", json!({"from_id": a["entity"]["id"], "to_id": b["entity"]["id"], "label": "works_at"})).await;
    assert!(rel.get("error").is_none(), "{rel}");
    let (_, page) = app.http("GET", &format!("/api/vaults/{team}/changes"), None, true).await;
    assert_eq!(ops(&page).last().unwrap(), "created relation", "{page}");

    let clone = app.tool("vault_clone", json!({"vault_id": team, "name": "Copy"})).await["id"].as_str().unwrap().to_string();
    let (_, page) = app.http("GET", &format!("/api/vaults/{clone}/changes"), None, true).await;
    assert_eq!(seqs(&page), [1, 2, 3, 4, 5], "{page}");
    let mut kinds = ops(&page);
    kinds.sort();
    assert_eq!(kinds, ["created entity", "created entity", "created memory", "created memory", "created relation"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_idle_socket_is_closed_once_its_token_is_revoked() {
    eunomia_backend::routers::changes::TEST_RECHECK_SECS.store(1, std::sync::atomic::Ordering::Relaxed);
    let app = TestApp::new().await;
    let base = serve(&app).await;
    let vault = personal(&app).await;
    let created = create_api_token_with(app.control(), &app.user.id, "w", &[scopes::MEMORY_READ.to_string()], None, None).await.unwrap();
    let mut req = format!("{base}/api/vaults/{vault}/changes/ws").into_client_request().unwrap();
    req.headers_mut().insert("authorization", format!("Bearer {}", created.token).parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.expect("connect");
    let cookie = app.session_cookie().await;
    let path = format!("/api/auth/tokens/{}", eunomia_backend::rid::RecordIdExt::to_string(&created.id));
    let ((status, body), _) = common::http(&app.router, "DELETE", &path, None, None, Some(&cookie)).await;
    assert_eq!(status, 200, "{body}");
    // no write happens: the timer alone finds the token gone
    let closed = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        while let Some(Ok(msg)) = ws.next().await {
            if let Message::Close(frame) = msg {
                return frame.map(|f| u16::from(f.code));
            }
        }
        None
    })
    .await
    .expect("closed within 20 s");
    assert_eq!(closed, Some(1008));
}

async fn page(app: &TestApp, vault: &str, after: i64) -> (u16, Value) {
    let (status, page) = app.http("GET", &format!("/api/vaults/{vault}/changes?after={after}&limit=1000"), None, true).await;
    (status.as_u16(), page)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retention_keeps_the_newest_rows_and_a_cursor_below_them_must_resync() {
    let app = TestApp::new().await;
    let base = serve(&app).await;
    let vault = personal(&app).await;
    for i in 0..10 {
        app.tool("memory_write", json!({"subject_name": "Ada", "subject_kind": "person", "text": format!("f{i}")})).await;
    }
    let db = app.db().await;
    // one entity and ten memories: 11 rows, keep the newest 4
    assert_eq!(changes::prune(&db, Retention { days: 0, max_rows: 4 }).await.unwrap(), 7);
    assert_eq!(seqs(&page(&app, &vault, 0).await.1), [8, 9, 10, 11], "after=0 reads from the oldest kept row");
    assert_eq!(seqs(&page(&app, &vault, 7).await.1), [8, 9, 10, 11], "a cursor right below the window is fine");
    let (status, old) = page(&app, &vault, 3).await;
    assert_eq!((status, old["code"].clone()), (410, json!("changes.resync")), "{old}");
    assert!(old["detail"].as_str().unwrap().contains("the oldest kept is 8"), "{old}");
    assert_eq!(page(&app, &vault, 11).await, (200, json!({"changes": [], "next": 11})), "up to date");
    assert_eq!(page(&app, &vault, 12).await.1["code"], "changes.resync", "past the newest change");
    let tool = app.tool("changes_since", json!({"after": 3})).await;
    assert_eq!(tool["code"], "changes.resync", "{tool}");
    // a stale cursor gets no socket: 410 before the upgrade
    let mut req = format!("{base}/api/vaults/{vault}/changes/ws?after=3").into_client_request().unwrap();
    req.headers_mut().insert("authorization", format!("Bearer {}", app.token).parse().unwrap());
    let err = tokio_tungstenite::connect_async(req).await.expect_err("refused");
    assert!(err.to_string().contains("410"), "{err}");

    // seqs go on from the counter, never restart; a second trim removes only what the new row pushed out
    app.tool("memory_write", json!({"subject_name": "Ada", "subject_kind": "person", "text": "after the trim"})).await;
    assert_eq!(seqs(&page(&app, &vault, 11).await.1), [12]);
    assert_eq!(changes::prune(&db, Retention { days: 0, max_rows: 4 }).await.unwrap(), 1);
    assert_eq!(changes::prune(&db, Retention { days: 0, max_rows: 4 }).await.unwrap(), 0, "idempotent");
    assert_eq!(seqs(&page(&app, &vault, 0).await.1), [9, 10, 11, 12]);
}

#[tokio::test]
async fn retention_by_age_and_its_defaults() {
    let app = TestApp::new().await;
    let vault = personal(&app).await;
    let team = app.tool("vault_create", json!({"name": "Team"})).await["id"].as_str().unwrap().to_string();
    for i in 0..5 {
        app.tool("memory_write", json!({"subject_name": "Ada", "subject_kind": "person", "text": format!("f{i}")})).await;
        app.tool("memory_write", json!({"subject_name": "Bo", "subject_kind": "person", "text": format!("t{i}"), "vault_id": team})).await;
    }
    let db = app.db().await;
    // the personal vault's first three rows are 100 days old; the team vault's are all new
    db.test_raw()
        .query("UPDATE change SET at = time::now() - 100d WHERE vault = $v AND seq <= 3")
        .bind(("v", rid::parse(&vault).unwrap()))
        .await
        .unwrap()
        .check()
        .unwrap();
    assert_eq!(changes::prune(&db, Retention { days: 0, max_rows: 0 }).await.unwrap(), 0, "0 and 0 keep everything");
    assert_eq!(changes::prune(&db, Retention { days: 90, max_rows: 0 }).await.unwrap(), 3);
    assert_eq!(seqs(&page(&app, &vault, 0).await.1), [4, 5, 6]);
    assert_eq!(page(&app, &vault, 2).await.1["code"], "changes.resync");
    assert_eq!(seqs(&page(&app, &team, 0).await.1), [1, 2, 3, 4, 5, 6], "another vault's new rows stay");

    // everything old: the log is empty, and only a cursor at the newest change is still in it
    db.test_raw().query("UPDATE change SET at = time::now() - 100d").await.unwrap().check().unwrap();
    assert_eq!(changes::prune(&db, Retention { days: 90, max_rows: 0 }).await.unwrap(), 9);
    assert_eq!(page(&app, &vault, 0).await, (200, json!({"changes": [], "next": 0})));
    assert_eq!(page(&app, &vault, 6).await, (200, json!({"changes": [], "next": 6})), "it saw everything: nothing to resync");
    assert_eq!(page(&app, &vault, 5).await.1["code"], "changes.resync", "it missed seq 6");

    // the environment: defaults when absent or unreadable, 0 turns a limit off
    let env = |pairs: &'static [(&'static str, &'static str)]| move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
    assert_eq!(Retention::from_vars(env(&[])), Retention { days: 90, max_rows: 100_000 });
    assert_eq!(Retention::from_vars(env(&[("CHANGE_LOG_RETENTION_DAYS", "0"), ("CHANGE_LOG_MAX_ROWS", "0")])), Retention { days: 0, max_rows: 0 });
    assert_eq!(Retention::from_vars(env(&[("CHANGE_LOG_RETENTION_DAYS", " 7 "), ("CHANGE_LOG_MAX_ROWS", "500")])), Retention { days: 7, max_rows: 500 });
    assert_eq!(Retention::from_vars(env(&[("CHANGE_LOG_RETENTION_DAYS", "soon"), ("CHANGE_LOG_MAX_ROWS", "-5")])), Retention { days: 90, max_rows: 100_000 });
}
