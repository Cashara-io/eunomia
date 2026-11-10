//! Fresh embedded databases only. The registry is shared by chat, MCP and browser tools.
mod common;
use common::TestApp;
use serde_json::{json, Value};
use std::collections::HashSet;
fn ok(v: Value) -> Value { assert!(v.get("error").is_none(), "{v}"); v }
async fn entity(app: &TestApp, name: &str, kind: &str) -> String {
    ok(app.tool("memory_write", json!({"subject_name":name,"subject_kind":kind,"text":"Fixture item"})).await)["entity"]["id"].as_str().unwrap().into()
}
#[tokio::test]
async fn impact_pages_count_and_reach_every_item_through_the_tool() {
    assert_eq!(common::test_settings().surreal_url, "mem://", "this fixture must use the embedded database");
    use eunomia_backend::sources::mock::{route, serve};
    let mock = serve(vec![route("GET", "/fixture", json!({})).header("x-eunomia-fixture", "impact-196")]).await;
    let url = format!("{}/fixture", mock.base);
    let proof = eunomia_backend::llm_net::client(&url).await.unwrap().get(&url).send().await.unwrap();
    assert_eq!(proof.headers()["x-eunomia-fixture"], "impact-196");
    let app = TestApp::with_settings(eunomia_backend::config::Settings {
        openai_base_url: mock.base.clone(), openai_chat_model: "fixture-model".into(), ..common::test_settings()
    }).await;
    let start = entity(&app, "Incident", "incident").await;
    let mut expected = HashSet::new();
    for i in 0..100 {
        let item = entity(&app, &format!("Item {i:03}"), "sample").await;
        ok(app.tool("entity_relate", json!({"from_id":start,"to_id":item,"label":"touches"})).await);
        expected.insert(item);
    }
    let unrelated = entity(&app, "Unrelated", "sample").await;
    ok(app.tool("entity_relate", json!({"from_id":start,"to_id":unrelated,"label":"other"})).await);
    let args = json!({"view":"impact","start_id":start,"headline":"Items linked to this incident","reach":{"depth":1,"kinds":["sample"],"labels":["touches"]}});
    let mut seen = HashSet::new();
    let mut cursor = Value::Null;
    let mut second_cursor = Value::Null;
    for page in 0..7 {
        let mut args = args.clone(); args["cursor"] = cursor;
        let out = ok(app.tool("show_view", args).await);
        if page == 0 { second_cursor = out["pagination"]["next_cursor"].clone(); }
        cursor = out["pagination"]["next_cursor"].clone();
        assert_eq!(out["pagination"]["total"], 100);
        assert_eq!(out["pagination"]["page"], page);
        assert_eq!(out["pagination"]["has_more"], page < 6);
        assert!(out["text"].as_str().unwrap().contains("100 linked items"));
        assert!(serde_json::to_vec(&out).unwrap().len() < 16_000, "default page must stay small");
        assert_eq!(out["affected"].as_array().unwrap().len(), if page == 6 { 4 } else { 16 });
        for row in out["affected"].as_array().unwrap() {
            assert!(seen.insert(row["entity"]["id"].as_str().unwrap().to_string()));
            assert_eq!(row["path"].as_array().unwrap().len(), 1);
            assert_eq!(row["path"][0]["label"], "touches");
        }
    }
    assert_eq!(seen, expected);
    // Echoed selection text in a real chat tool result must not authorise the reply.
    let mut second_page = args.clone(); second_page["cursor"] = second_cursor.clone();
    for kind in ["sample999777", "samplex999777"] {
        let mut echoed_page = args.clone(); echoed_page["reach"]["kinds"] = json!([kind]);
        let sse = |delta: Value| format!("data: {}\n\ndata: [DONE]\n\n", json!({"choices":[{"delta":delta}]}));
        mock.set_routes(vec![
            route("POST", "/chat/completions", Value::Null).times(1).raw(&sse(json!({"tool_calls":[{"index":0,"id":"page_two","function":{"name":"show_view","arguments":echoed_page.to_string()}}]}))),
            route("POST", "/chat/completions", Value::Null).raw(&sse(json!({"content":"There are 999777 affected items."}))),
        ]);
        use eunomia_backend::chat::service;
        let db = app.db().await;
        let thread = common::sys(service::create_thread(&db, &app.user.id, None)).await.unwrap();
        let thread_id = eunomia_backend::rid::parse(&thread.id).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        common::sys(service::send_stream(app.state.org(&app.user.org).await.unwrap(), app.user.clone(), thread_id.clone(), "Fetch the next page".into(), tx)).await;
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await { events.push(serde_json::to_value(event).unwrap()); }
        let result = events.iter().find(|e| e["type"] == "tool_result" && e["name"] == "show_view").unwrap();
        assert_eq!(result["view"]["pagination"]["total"], 0);
        assert_eq!(result["view"]["affected"], json!([]));
        let last = events.last().unwrap();
        assert!(last["reply"].as_str().unwrap().contains("[unverified number]"));
        assert!(!last["reply"].as_str().unwrap().contains("999777"));
        let history = common::sys(service::history(&db, &app.user.id, &thread_id)).await.unwrap().unwrap();
        assert_eq!(serde_json::to_value(history).unwrap().as_array().unwrap().last().unwrap()["tree"], last["tree"]);
    }
    // The generic parameters and page result are also available over MCP.
    let (status, mcp) = app.http("POST", "/mcp", Some(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"show_view","arguments":second_page}})), true).await;
    assert_eq!(status, 200);
    let mcp_view = &mcp["result"]["structuredContent"];
    assert_eq!(mcp_view["pagination"]["page"], 1);
    assert_eq!(mcp_view["pagination"]["total"], 100);
    let ui = json!({"view":"ui","spec":{"root":"impact","elements":{"impact":{"type":"Impact","props":{"title":"Linked items","summary":"Saved links","data":{"$state":"/sources/items"}}}}},"sources":{"items":{"type":"impact","start_id":start,"headline":"Linked items","reach":{"depth":1,"kinds":["sample"],"labels":["touches"]},"cursor":second_cursor}}});
    let shown = ok(app.tool("show_view", ui.clone()).await);
    assert_eq!(shown["data"]["sources"]["items"]["pagination"]["total"], 100);
    let mut unchecked = ui;
    unchecked["sources"]["items"]["headline"] = json!("There are 999777 linked items");
    assert!(app.tool("show_view", unchecked).await.get("error").is_some());
    let mut raw_headline = args.clone();
    raw_headline["headline"] = json!("There are 999777 linked items");
    let checked = ok(app.tool("show_view", raw_headline).await);
    assert!(!checked["headline"].as_str().unwrap().contains("999777"));
    assert!(checked["headline"].as_str().unwrap().contains("[unverified number]"));
    let mut invented_kind = args.clone();
    invented_kind["reach"]["kinds"] = json!(["sample999777"]);
    invented_kind["headline"] = json!("There are 999777 linked items");
    let checked = ok(app.tool("show_view", invented_kind).await);
    assert_eq!(checked["pagination"]["total"], 0);
    assert!(!checked["headline"].as_str().unwrap().contains("999777"), "query text must not authorise prose numbers");
    let mut invalid = args.clone(); invalid["page"] = json!(7);
    assert!(app.tool("show_view", invalid).await.get("error").is_some());
    let outsider = common::register(&app.state, "outside-impact@example.test").await;
    let denied = common::sys(eunomia_backend::tools::registry::call(&app.state, &outsider, "show_view", args.clone())).await.unwrap();
    assert!(denied.get("error").is_some(), "same-org nonmember must not see the selection");
    let mut mixed = args.clone();
    mixed["affected"] = json!([{"entity_id":start,"path":[],"why":"Invalid mixed mode"}]);
    assert!(app.tool("show_view", mixed).await.get("error").is_some());
    let mut missing = args; missing["start_id"] = json!("entity:missing");
    assert!(app.tool("show_view", missing).await.get("error").is_some());
}


#[tokio::test]
async fn quoted_and_prefixed_starts_never_count_themselves() {
    assert_eq!(common::test_settings().surreal_url, "mem://");
    let app = TestApp::new().await;
    let id = entity(&app, "Alone", "sample").await;
    let key = id.split_once(':').unwrap().1;
    for start in [id.clone(), format!("`entity`:{key}"), format!("r'{id}'"), format!("r\"{id}\""), format!("`entity`:`{key}`")] {
        let out = ok(app.tool("show_view", json!({"view":"impact","start_id":start,"headline":"Linked items","reach":{"depth":1}})).await);
        assert_eq!(out["pagination"]["total"], 0, "{start}: {out}");
        assert_eq!(out["affected"], json!([]));
        assert_eq!(out["linked"], false);
    }
}

#[tokio::test]
async fn dense_frontier_bounds_actual_fetched_rows_and_labels_partial_total() {
    assert_eq!(common::test_settings().surreal_url, "mem://");
    let app = TestApp::new().await;
    let start = entity(&app, "Start", "sample").await;
    let db = app.db().await;
    let vault = ok(app.tool("ontology_get", json!({})).await)["vault"].as_str().unwrap().to_owned();
    let vault = eunomia_backend::rid::parse(&vault).unwrap();
    // A thousand-node frontier, each with forty-two neighbouring links.
    // The start's generated key is used by the seed's hub links below.
    eunomia_backend::store::entities::seed_view_fixture(&db, &app.user.id, &vault, 1000).await;
    eunomia_backend::store::entities::seed_dense_view_links(&db, &eunomia_backend::rid::parse(&start).unwrap(), 1000).await;
    use eunomia_backend::entities::neighbourhood::FETCHED_EDGE_ROWS;
    FETCHED_EDGE_ROWS.scope(std::cell::Cell::new(0), async {
        let out = app.tool("show_view", json!({"view":"impact","start_id":start,"headline":"Linked items","reach":{"depth":2}})).await;
        eprintln!("dense frontier fetched {} edge rows", FETCHED_EDGE_ROWS.with(std::cell::Cell::get));
        assert!(FETCHED_EDGE_ROWS.with(std::cell::Cell::get) <= 40_000, "fetched {} rows", FETCHED_EDGE_ROWS.with(std::cell::Cell::get));
        assert_eq!(out["pagination"]["total_partial"], true, "{out}");
        assert!(out["text"].as_str().unwrap().contains("At least 1000 linked items"));
    }).await;
}

#[tokio::test]
async fn impact_cursor_survives_inserts_and_deletes_before_the_boundary() {
    assert_eq!(common::test_settings().surreal_url, "mem://");
    let app = TestApp::new().await;
    let start = entity(&app, "Start", "sample").await;
    let mut expected = HashSet::new();
    for i in 0..40 {
        let id = entity(&app, &format!("Item {i:03}"), "sample").await;
        ok(app.tool("entity_relate", json!({"from_id":start,"to_id":id,"label":"touches"})).await);
        expected.insert(id);
    }
    let args = json!({"view":"impact","start_id":start,"headline":"Linked items","reach":{"depth":1}});
    let first = ok(app.tool("show_view", args.clone()).await);
    let mut seen: HashSet<String> = first["affected"].as_array().unwrap().iter().map(|v| v["entity"]["id"].as_str().unwrap().to_owned()).collect();
    let cursor = first["pagination"]["next_cursor"].as_str().expect("server cursor").to_owned();
    // Delete both an earlier item and the boundary itself. Offsets would now skip a survivor.
    for row in [&first["affected"][0], &first["affected"][15]] {
        let removed = row["entity"]["id"].as_str().unwrap();
        ok(app.tool("entity_delete", json!({"entity_id":removed})).await);
        expected.remove(removed);
        seen.remove(removed);
    }
    let db = app.db().await;
    let vault = eunomia_backend::rid::parse(first["start"]["vault"].as_str().unwrap()).unwrap();
    let added = "entity:000inserted";
    eunomia_backend::store::entities::seed_view_item(&db, &app.user.id, &vault, &eunomia_backend::rid::parse(added).unwrap()).await;
    ok(app.tool("entity_relate", json!({"from_id":start,"to_id":added,"label":"touches"})).await);
    let mut wrong_selection = args.clone();
    wrong_selection["reach"]["depth"] = json!(2);
    wrong_selection["cursor"] = json!(cursor);
    assert!(app.tool("show_view", wrong_selection).await.get("error").is_some());
    let mut invalid = args.clone(); invalid["cursor"] = json!("not-a-cursor");
    assert!(app.tool("show_view", invalid).await.get("error").is_some());
    let mut next = Some(cursor);
    while let Some(cursor) = next {
        let mut request = args.clone(); request["cursor"] = json!(cursor);
        let out = ok(app.tool("show_view", request).await);
        for row in out["affected"].as_array().unwrap() {
            assert!(seen.insert(row["entity"]["id"].as_str().unwrap().to_owned()), "repeated item");
        }
        next = out["pagination"]["next_cursor"].as_str().map(str::to_owned);
    }
    assert!(expected.is_subset(&seen), "no surviving original item may be skipped");
}

#[tokio::test]
async fn metric_evidence_cursor_reaches_all_contributors() {
    assert_eq!(common::test_settings().surreal_url, "mem://");
    let app = TestApp::new().await;
    entity(&app, "First", "sample").await;
    let vault = ok(app.tool("ontology_get", json!({})).await)["vault"].as_str().unwrap().to_owned();
    let db = app.db().await;
    eunomia_backend::store::entities::seed_view_fixture(&db, &app.user.id, &eunomia_backend::rid::parse(&vault).unwrap(), 1239).await;
    ok(app.tool("ontology_propose", json!({"vault_id":vault,"properties":[{"kind":"sample","name":"score","datatype":"number"}],"metrics":[{"kind":"sample","name":"item_count","expr":{"agg":"sum","property":"score"}}]})).await);
    let (status, body) = app.http_session("POST", &format!("/api/vaults/{vault}/ontology/properties/decide"), Some(json!({"kind":"sample","name":"score","decision":{"action":"approve"}}))).await;
    assert_eq!(status, 200, "{body}");
    eunomia_backend::store::entities::seed_view_values(&db, &eunomia_backend::rid::parse(&vault).unwrap()).await;
    let mut args = json!({"view":"metric","vault_id":vault,"metric":"item_count","allow_unapproved":true,"as_of":"2026-01-01T00:00:00.987654321Z"});
    let mut next = Value::Null;
    let mut seen = HashSet::new();
    loop {
        let mut request = args.clone();
        if !next.is_null() { request["evidence_cursor"] = next; }
        let out = ok(app.tool("show_view", request).await);
        assert_eq!(out["metric"]["n_entities"], 1240);
        assert_eq!(out["metric"]["value"], 1240.0);
        for row in out["entities"].as_array().unwrap() { assert!(seen.insert(row["id"].as_str().unwrap().to_owned())); }
        if out["evidence"]["next_cursor"].is_null() { break; }
        next = out["evidence"]["next_cursor"].clone();
        args = out["evidence"]["args"].clone(); args["view"] = json!("metric");
    }
    assert_eq!(seen.len(), 1240);
}

#[tokio::test]
async fn uneven_frontier_keeps_every_node_when_the_graph_fits_the_budget() {
    assert_eq!(common::test_settings().surreal_url, "mem://");
    let app = TestApp::new().await;
    let start = entity(&app, "Root", "sample").await;
    let mut hub = String::new();
    for i in 0..32 {
        let child = entity(&app, &format!("Child {i}"), "sample").await;
        ok(app.tool("entity_relate", json!({"from_id":start,"to_id":child,"label":"touches"})).await);
        if i == 0 { hub = child; }
    }
    for i in 0..100 {
        let leaf = entity(&app, &format!("Leaf {i}"), "sample").await;
        ok(app.tool("entity_relate", json!({"from_id":hub,"to_id":leaf,"label":"touches"})).await);
    }
    use eunomia_backend::entities::neighbourhood::FETCHED_EDGE_ROWS;
    FETCHED_EDGE_ROWS.scope(std::cell::Cell::new(0), async {
        let near = ok(app.tool("entities_neighbourhood", json!({"entity_id":start,"depth":2})).await);
        assert_eq!(near["nodes"].as_array().unwrap().len(), 133);
        assert_eq!(near["truncated"], false);
        assert!(FETCHED_EDGE_ROWS.with(std::cell::Cell::get) <= 2400);
    }).await;
    let impact = ok(app.tool("show_view", json!({"view":"impact","start_id":start,"headline":"Linked items","reach":{"depth":2}})).await);
    assert_eq!(impact["pagination"]["total"], 132);
    assert_eq!(impact["pagination"]["total_partial"], false);
    let scenario = ok(app.tool("scenario_run", json!({"changes":[{"remove":{"entity_id":start}}],"metrics":[],"impact_from":[start],"depth":2})).await);
    assert_eq!(scenario["reached"].as_array().unwrap().len(), 132, "{scenario}");
    assert_eq!(scenario["reached_truncated"], false);
}

#[tokio::test]
async fn hostile_cursors_fail_before_traversing_the_graph() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use eunomia_backend::entities::neighbourhood::FETCHED_EDGE_ROWS;
    assert_eq!(common::test_settings().surreal_url, "mem://");
    let app = TestApp::new().await;
    let start = entity(&app, "Cursor root", "sample").await;
    for i in 0..17 {
        let child = entity(&app, &format!("Cursor child {i}"), "sample").await;
        ok(app.tool("entity_relate", json!({"from_id":start,"to_id":child,"label":"touches"})).await);
    }
    let args = json!({"view":"impact","start_id":start,"headline":"Linked items","reach":{"depth":1}});
    let first = ok(app.tool("show_view", args.clone()).await);
    let cursor: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(first["pagination"]["next_cursor"].as_str().unwrap()).unwrap()).unwrap();
    let mut edited = cursor.clone();
    edited["page"] = json!(1_000_000);
    edited["key"] = json!("'; SLEEP 999999s; --");
    let mut request = args.clone();
    request["cursor"] = json!(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&edited).unwrap()));
    FETCHED_EDGE_ROWS.scope(std::cell::Cell::new(0), async {
        let out = ok(app.tool("show_view", request).await);
        assert!(FETCHED_EDGE_ROWS.with(std::cell::Cell::get) <= 18, "an edited ordinal or key must not expand the query");
        assert_eq!(out["pagination"]["page"], 1_000_000);
        assert_eq!(out["pagination"]["total"], 17);
    }).await;
    for (field, hostile) in [
        ("page", json!(u64::MAX)), ("page", json!(-1)), ("page", json!("999999999999999999999999")),
        ("key", json!(["wrong type"])), ("key", json!("x".repeat(2048))),
        ("scope", json!({"wrong":"type"})),
    ] {
        let mut edited = cursor.clone(); edited[field] = hostile;
        let mut request = args.clone(); request["cursor"] = json!(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&edited).unwrap()));
        FETCHED_EDGE_ROWS.scope(std::cell::Cell::new(0), async {
            let out = app.tool("show_view", request.clone()).await;
            assert_eq!(out["code"], "validation.invalid", "{out}");
            assert_eq!(FETCHED_EDGE_ROWS.with(std::cell::Cell::get), 0, "invalid cursor must not run the graph query");
            let (status, _) = app.http_session("POST", "/api/tools/show_view", Some(request)).await;
            assert_ne!(status, 500);
        }).await;
    }
}
