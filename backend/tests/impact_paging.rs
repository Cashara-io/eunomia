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
    let port = url::Url::parse(&url).unwrap().port().unwrap();
    assert!(![3000, 8000, 8001, 3600, 8600, 3700, 18470, 18471, 18472, 18473].contains(&port));
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
    for page in 0..7 {
        let mut args = args.clone(); args["page"] = json!(page);
        let out = ok(app.tool("show_view", args).await);
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
    // A model requests page two through the real chat tool loop, then replies
    // with an unsupported number. The checked tree must still suppress it.
    let mut second_page = args.clone(); second_page["page"] = json!(1);
    let sse = |delta: Value| format!("data: {}\n\ndata: [DONE]\n\n", json!({"choices":[{"delta":delta}]}));
    mock.set_routes(vec![
        route("POST", "/chat/completions", Value::Null).times(1).raw(&sse(json!({"tool_calls":[{"index":0,"id":"page_two","function":{"name":"show_view","arguments":second_page.to_string()}}]}))),
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
    assert_eq!(result["view"]["pagination"]["page"], 1);
    assert_eq!(result["view"]["pagination"]["total"], 100);
    assert_eq!(result["view"]["affected"][0]["entity"]["name"], "Item 016");
    let last = events.last().unwrap();
    assert!(last["reply"].as_str().unwrap().contains("[unverified number]"));
    assert!(!last["reply"].as_str().unwrap().contains("999777"));
    let history = common::sys(service::history(&db, &app.user.id, &thread_id)).await.unwrap().unwrap();
    assert_eq!(serde_json::to_value(history).unwrap().as_array().unwrap().last().unwrap()["tree"], last["tree"]);
    // The generic parameters and page result are also available over MCP.
    let (status, mcp) = app.http("POST", "/mcp", Some(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"show_view","arguments":second_page}})), true).await;
    assert_eq!(status, 200);
    let mcp_view = &mcp["result"]["structuredContent"];
    assert_eq!(mcp_view["pagination"]["page"], 1);
    assert_eq!(mcp_view["pagination"]["total"], 100);
    let ui = json!({"view":"ui","spec":{"root":"impact","elements":{"impact":{"type":"Impact","props":{"title":"Linked items","summary":"Saved links","data":{"$state":"/sources/items"}}}}},"sources":{"items":{"type":"impact","start_id":start,"headline":"Linked items","reach":{"depth":1,"kinds":["sample"],"labels":["touches"]},"page":1}}});
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
