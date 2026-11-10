//! In-memory graph fixtures and scripted completions. No live provider is called.
mod common;
use common::TestApp;
use serde_json::{Value, json};
use std::collections::BTreeSet;
fn ok(v: Value) -> Value {
    assert!(v.get("error").is_none(), "{v}");
    v
}
async fn entity(app: &TestApp, name: &str, kind: &str) -> String {
    ok(app
        .tool(
            "memory_write",
            json!({"subject_name":name,"subject_kind":kind,"text":"Diagram fixture"}),
        )
        .await)["entity"]["id"]
        .as_str()
        .unwrap()
        .into()
}
async fn link(app: &TestApp, a: &str, b: &str, label: &str) {
    ok(app
        .tool(
            "entity_relate",
            json!({"from_id":a,"to_id":b,"label":label}),
        )
        .await);
}
fn request(kind: &str, id: &str) -> Value {
    json!({"view":"diagram","diagram":{"selection":{"type":kind,"focus_id":id,"hops":"two"}}})
}
fn ids(rows: &Value) -> BTreeSet<String> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|n| n["id"].as_str().unwrap().into())
        .collect()
}
#[tokio::test]
async fn neighbourhood_path_impact_kinds_and_terminal_share_real_data() {
    assert_eq!(common::test_settings().surreal_url, "mem://");
    let app = TestApp::new().await;
    let a = entity(&app, "Origin", "organisation").await;
    let b = entity(&app, "Order", "order").await;
    let c = entity(&app, "Store", "location").await;
    let d = entity(&app, "Other order", "order").await;
    link(&app, &a, &b, "supplies").await;
    link(&app, &b, &c, "delivered_to").await;
    link(&app, &a, &d, "supplies").await;
    link(&app, &d, &c, "delivered_to").await;
    let neighbours = ok(app
        .tool("entities_neighbourhood", json!({"entity_id":a,"depth":2}))
        .await);
    let out = ok(app.tool("show_view", request("neighbourhood", &a)).await);
    let (status, mcp) = app.http("POST", "/mcp", Some(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"show_view","arguments":request("neighbourhood", &a)}})), true).await;
    assert_eq!(status, 200, "{mcp}");
    assert_eq!(mcp["result"]["structuredContent"], out);
    assert_eq!(mcp["result"]["content"][0]["text"], out["text"]);
    assert_eq!(ids(&out["diagram"]["nodes"]), ids(&neighbours["nodes"]));
    assert_eq!(ids(&out["diagram"]["edges"]), ids(&neighbours["edges"]));
    for e in out["diagram"]["edges"].as_array().unwrap() {
        let original = neighbours["edges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == e["id"])
            .unwrap();
        for k in ["from", "to", "label"] {
            assert_eq!(e[k], original[k]);
        }
    }
    assert_eq!(out["diagram"]["total_nodes"], 4);
    assert_eq!(out["diagram"]["total_edges"], 4);
    assert_eq!(out["diagram"]["partial"], false);
    assert!(
        out["text"]
            .as_str()
            .unwrap()
            .contains("Order delivered to Store")
    );
    assert_eq!(
        out,
        ok(app.tool("show_view", request("neighbourhood", &a)).await),
        "layout is deterministic"
    );
    let path = json!({"view":"diagram","diagram":{"selection":{"type":"path","from_id":a,"to_id":c,"hops":"two","direction":"out"}}});
    let found = ok(app.tool("show_view", path.clone()).await);
    assert_eq!(
        ids(&found["diagram"]["edges"]),
        ids(&neighbours["edges"]),
        "all shortest alternatives use only real links"
    );
    let mut reverse = path.clone();
    reverse["diagram"]["selection"]["from_id"] = json!(c);
    reverse["diagram"]["selection"]["to_id"] = json!(a);
    let none = ok(app.tool("show_view", reverse).await);
    assert_eq!(none["diagram"]["edges"], json!([]));
    assert!(
        none["text"]
            .as_str()
            .unwrap()
            .contains("No path found within the search limits (2 hops)")
    );
    let mut short = path;
    short["diagram"]["selection"]["hops"] = json!("one");
    assert!(
        ok(app.tool("show_view", short).await)["text"]
            .as_str()
            .unwrap()
            .contains("No path found within the search limits (1 hop)")
    );
    let impact = ok(app
        .tool(
            "show_view",
            json!({"view":"impact","start_id":a,"headline":"Linked items","reach":{"depth":2}}),
        )
        .await);
    let diagram = ok(app.tool("show_view", request("impact", &a)).await);
    assert_eq!(
        diagram["diagram"]["total_nodes"].as_u64().unwrap() - 1,
        impact["pagination"]["total"].as_u64().unwrap()
    );
    let kinds=ok(app.tool("show_view",json!({"view":"diagram","diagram":{"selection":{"type":"kinds","vault_id":neighbours["vault"],"kind":"order"}}})).await);
    let ontology = ok(app
        .tool("ontology_get", json!({"vault_id":neighbours["vault"]}))
        .await);
    for n in kinds["diagram"]["nodes"].as_array().unwrap() {
        let k = ontology["kinds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["kind"] == n["kind"])
            .unwrap();
        assert_eq!(n["count"], k["entities"]);
    }
    let ui=ok(app.tool("show_view",json!({"view":"ui","sources":{"graph":{"type":"diagram","diagram":{"selection":{"type":"neighbourhood","focus_id":a}}}},"spec":{"root":"graph","elements":{"graph":{"type":"Diagram","props":{"title":"Connections","summary":"Recorded links","data":{"$state":"/sources/graph"}}}}}})).await);
    assert_eq!(ui["data"]["sources"]["graph"]["diagram"], out["diagram"]);
    for field in ["label", "value", "x", "colour", "nodes", "edges"] {
        let mut bad = request("neighbourhood", &a);
        bad["diagram"][field] = json!(999777);
        assert!(app.tool("show_view", bad).await.get("error").is_some());
    }
    let mut bad = request("neighbourhood", &a);
    bad["title"] = json!("Invented 999777");
    assert!(app.tool("show_view", bad).await.get("error").is_some());
    let mut bad = request("neighbourhood", &a);
    bad["diagram"]["selection"]["depth"] = json!(999777);
    assert!(app.tool("show_view", bad).await.get("error").is_some());
}
#[tokio::test]
async fn references_and_restricted_tokens_cannot_leave_the_vault() {
    let app = TestApp::new().await;
    let a = entity(&app, "Private", "person").await;
    let team = ok(app
        .tool("vault_create", json!({"name":"Other","kind":"org"}))
        .await)["id"]
        .as_str()
        .unwrap()
        .to_string();
    let b=ok(app.tool("memory_write",json!({"subject_name":"Other vault","subject_kind":"person","text":"Elsewhere","vault_id":team})).await)["entity"]["id"].as_str().unwrap().to_string();
    let cross =
        json!({"view":"diagram","diagram":{"selection":{"type":"path","from_id":a,"to_id":b}}});
    assert!(app.tool("show_view", cross).await.get("error").is_some());
    for ref_id in ["entity:missing", b.as_str()] {
        let mut arg = request("neighbourhood", &a);
        arg["diagram"]["emphasis"] = json!([ref_id]);
        assert!(app.tool("show_view", arg).await.get("error").is_some());
    }
    let other = common::register(&app.state, "diagram-outsider@example.test").await;
    assert!(
        common::sys(eunomia_backend::tools::registry::call(
            &app.state,
            &other,
            "show_view",
            request("neighbourhood", &a)
        ))
        .await
        .unwrap()
        .get("error")
        .is_some()
    );
    let only = eunomia_backend::rid::parse(&team).unwrap();
    let token = eunomia_backend::models_user::create_api_token_with(
        &app.state.control,
        &app.user.id,
        "limited",
        &["memory:read".into()],
        Some(&only),
        None,
    )
    .await
    .unwrap()
    .token;
    for kind in ["neighbourhood", "impact"] {
        let ((status, out), _) = common::http(
            &app.router,
            "POST",
            "/api/tools/show_view",
            Some(request(kind, &a)),
            Some(&token),
            None,
        )
        .await;
        assert!(
            status == 403 || out.get("error").is_some(),
            "{status} {out}"
        );
    }
    let ((status, out), _) = common::http(
        &app.router,
        "POST",
        "/api/tools/show_view",
        Some(request("neighbourhood", &b)),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(status, 200, "{out}");
    ok(out);
}
#[tokio::test]
async fn caps_group_omitted_nodes_and_report_partial_counts() {
    let app = TestApp::new().await;
    let a = entity(&app, "Hub", "organisation").await;
    for i in 0..20 {
        let n = entity(&app, &format!("Item {i}"), "order").await;
        link(&app, &a, &n, "supplies").await;
    }
    let d = ok(app.tool("show_view", request("neighbourhood", &a)).await)["diagram"].clone();
    assert_eq!(d["nodes"].as_array().unwrap().len(), 12);
    assert_eq!(d["total_nodes"], 21);
    assert_eq!(d["partial"], true);
    assert_eq!(d["total_partial"], false);
    assert_eq!(d["groups"], json!([{"kind":"order","count":9}]));
    assert_eq!(d["omitted_edges"], 9);
}
#[tokio::test]
async fn scripted_chat_stores_exact_diagram_and_checks_prose_numbers() {
    use eunomia_backend::{
        chat::service,
        sources::mock::{route, serve},
    };
    let mock = serve(vec![]).await;
    let app = TestApp::with_settings(eunomia_backend::config::Settings {
        openai_base_url: mock.base.clone(),
        openai_chat_model: "scripted".into(),
        ..common::test_settings()
    })
    .await;
    let a = entity(&app, "Origin", "organisation").await;
    let b = entity(&app, "Store", "location").await;
    link(&app, &a, &b, "supplies").await;
    let args = request("neighbourhood", &a);
    let sse = |delta: Value| {
        format!(
            "data: {}\n\ndata: [DONE]\n\n",
            json!({"choices":[{"delta":delta}]})
        )
    };
    mock.set_routes(vec![route("POST","/chat/completions",Value::Null).times(1).raw(&sse(json!({"tool_calls":[{"index":0,"id":"diagram","function":{"name":"show_view","arguments":args.to_string()}}]}))),route("POST","/chat/completions",Value::Null).raw(&sse(json!({"content":"There are 2 objects and 999777 deliveries."})))]);
    let db = app.db().await;
    let t = common::sys(service::create_thread(&db, &app.user.id, None))
        .await
        .unwrap();
    let tid = eunomia_backend::rid::parse(&t.id).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    common::sys(service::send_stream(
        app.state.org(&app.user.org).await.unwrap(),
        app.user.clone(),
        tid.clone(),
        "Show the connections".into(),
        tx,
    ))
    .await;
    let mut events = vec![];
    while let Some(e) = rx.recv().await {
        events.push(serde_json::to_value(e).unwrap());
    }
    let live = events
        .iter()
        .find(|e| e["type"] == "tool_result" && e["name"] == "show_view")
        .unwrap()["view"]
        .clone();
    let history = serde_json::to_value(
        common::sys(service::history(&db, &app.user.id, &tid))
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let stored = history
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "tool")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(stored["content"].as_str().unwrap()).unwrap(),
        live
    );
    let reply = events.last().unwrap()["reply"].as_str().unwrap();
    assert!(reply.contains("2 objects"), "{reply}");
    assert!(reply.contains("[unverified number]"));
    assert!(!reply.contains("999777"));
}

#[tokio::test]
async fn annotation_metric_is_approved_and_scoped_to_its_object() {
    let app = TestApp::new().await;
    let a = entity(&app, "One order", "order").await;
    let b = entity(&app, "Another order", "order").await;
    link(&app, &a, &b, "related").await;
    let n = ok(app
        .tool("entities_neighbourhood", json!({"entity_id":a}))
        .await);
    let vault = n["vault"].as_str().unwrap();
    ok(app.tool("ontology_propose",json!({"metrics":[{"kind":"order","name":"object_count","expr":{"agg":"count"},"unit":"orders"}]})).await);
    let mut arg = request("neighbourhood", &a);
    arg["diagram"]["annotations"] = json!([{"entity_id":a,"metric":"object_count"}]);
    assert!(
        app.tool("show_view", arg.clone())
            .await
            .get("error")
            .is_some(),
        "proposed metric refused"
    );
    let (status, body) = app
        .http_session(
            "POST",
            &format!("/api/vaults/{vault}/ontology/metrics/decide"),
            Some(json!({"kind":"order","name":"object_count","decision":{"action":"approve"}})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let out = ok(app.tool("show_view", arg.clone()).await);
    let node = out["diagram"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == a)
        .unwrap();
    assert_eq!(node["metrics"][0]["value"], 1.0);
    assert_eq!(node["metrics"][0]["unit"], "orders");
    assert!(out["text"].as_str().unwrap().contains("1 orders"));
    let ui=ok(app.tool("show_view",json!({"view":"ui","sources":{"graph":{"type":"diagram","diagram":arg["diagram"]}},"spec":{"root":"text","elements":{"text":{"type":"Text","props":{"text":"Object count {{/sources/graph/diagram/nodes/0/metrics/0/value}}"}}}}})).await);
    assert_eq!(
        ui["spec"]["elements"]["text"]["props"]["text"],
        "Object count 1 orders"
    );
}

#[tokio::test]
async fn dense_graph_caps_edges_without_inventing_group_links() {
    let app = TestApp::new().await;
    let mut nodes = vec![];
    for i in 0..7 {
        nodes.push(entity(&app, &format!("Object {i}"), "order").await);
    }
    for a in &nodes {
        for b in &nodes {
            if a != b {
                link(&app, a, b, "related").await;
            }
        }
    }
    let out = ok(app
        .tool("show_view", request("neighbourhood", &nodes[0]))
        .await);
    assert_eq!(out["diagram"]["nodes"].as_array().unwrap().len(), 7);
    assert_eq!(out["diagram"]["edges"].as_array().unwrap().len(), 24);
    assert_eq!(out["diagram"]["total_edges"], 42);
    assert_eq!(out["diagram"]["omitted_edges"], 18);
    assert_eq!(out["diagram"]["groups"], json!([]));
    assert_eq!(out["diagram"]["partial"], true);
}

#[tokio::test]
async fn traversal_budget_keeps_totals_as_lower_bounds() {
    let app = TestApp::new().await;
    let a = entity(&app, "Bounded hub", "organisation").await;
    for i in 0..155 {
        let n = entity(&app, &format!("Reached {i}"), "order").await;
        link(&app, &a, &n, "supplies").await;
    }
    let source = ok(app
        .tool("entities_neighbourhood", json!({"entity_id":a,"depth":1}))
        .await);
    let mut arg = request("neighbourhood", &a);
    arg["diagram"]["selection"]["hops"] = json!("one");
    use eunomia_backend::entities::neighbourhood::LARGEST_DIAGRAM_QUERY;
    let (out, max_rows) = LARGEST_DIAGRAM_QUERY.scope(Default::default(), async {
        let out = ok(app.tool("show_view", arg).await);
        (out, LARGEST_DIAGRAM_QUERY.with(|n| n.get()))
    }).await;
    assert_eq!(max_rows, 33, "a high-degree hub must be paged as 32 rows plus one lookahead");
    assert_eq!(source["truncated"], true);
    assert_eq!(out["diagram"]["total_partial"], true);
    assert_eq!(out["diagram"]["partial"], true);
    assert_eq!(
        out["diagram"]["total_nodes"].as_u64().unwrap() as usize,
        source["nodes"].as_array().unwrap().len()
    );
    assert!(out["text"].as_str().unwrap().contains("At least"));
}

#[cfg(feature = "test-support")]
#[tokio::test]
async fn dense_hubs_return_an_honest_partial_result_at_the_deadline() {
    use eunomia_backend::views::diagram::{Request, resolve_search_deadline_for_test};
    use std::time::{Duration, Instant};
    let app = TestApp::new().await;
    let a = entity(&app, "First hub", "organisation").await;
    let b = entity(&app, "Second hub", "organisation").await;
    let mut middle = Vec::new();
    for i in 0..18 { middle.push(entity(&app, &format!("Spoke {i}"), "order").await); }
    for from in &middle {
        link(&app, &a, from, "supplies").await;
        link(&app, from, &b, "delivered_to").await;
        for to in &middle { if from != to { link(&app, from, to, "related_to").await; } }
    }
    let request = || serde_json::from_value::<Request>(json!({"selection":{"type":"path","from_id":a,"to_id":b,"hops":"three"}})).unwrap();
    let db = app.db().await;
    let started = Instant::now();
    let (partial, issued, late) = eunomia_backend::store::DEADLINE_PROBE.scope(Default::default(), async {
        let partial = common::sys(resolve_search_deadline_for_test(&db, &app.user.id, request(), Duration::ZERO)).await.unwrap();
        let (issued, late) = eunomia_backend::store::DEADLINE_PROBE.with(|p| (p.borrow().issued, p.borrow().after_deadline));
        (partial, issued, late)
    }).await;
    assert_eq!(late, 0, "no query may be submitted after the search deadline");
    assert_eq!(issued, 0, "an expired search must not submit even its first query");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(partial.partial && partial.total_partial);
    assert!(partial.notice.as_deref().unwrap().contains("No path found within the search limits"));
    assert_eq!(partial.nodes.len(), 2);
    assert!(partial.edges.is_empty());
    let started = Instant::now();
    use eunomia_backend::entities::neighbourhood::{FETCHED_EDGE_ROWS, LARGEST_DIAGRAM_QUERY};
    let (normal, max_rows, fetched, late) = LARGEST_DIAGRAM_QUERY.scope(Default::default(), FETCHED_EDGE_ROWS.scope(Default::default(), eunomia_backend::store::DEADLINE_PROBE.scope(Default::default(), async {
        let normal = common::sys(eunomia_backend::views::diagram::resolve(&db, &app.user.id, request())).await.unwrap();
        (normal, LARGEST_DIAGRAM_QUERY.with(|n| n.get()), FETCHED_EDGE_ROWS.with(|n| n.get()), eunomia_backend::store::DEADLINE_PROBE.with(|p| p.borrow().after_deadline))
    }))).await;
    assert!(max_rows > 0 && max_rows <= 33, "at most 32 edge rows plus one lookahead, or 32 node rows per query: {max_rows}");
    assert!(fetched <= 2401, "total fetched-row budget plus one lookahead: {fetched}");
    assert_eq!(late, 0);
    assert!(started.elapsed() < Duration::from_secs(4));
    assert!(normal.nodes.len() <= 12 && normal.edges.len() <= 24);
}

#[cfg(feature = "test-support")]
#[tokio::test]
async fn database_submission_fence_does_not_send_an_expired_query() {
    let app = TestApp::new().await;
    let db = app.db().await;
    let deadline = tokio::time::Instant::now();
    eunomia_backend::store::DEADLINE_PROBE.scope(std::cell::RefCell::new(eunomia_backend::store::DeadlineProbe {
        deadline: Some(deadline), ..Default::default()
    }), async {
        let result = eunomia_backend::store::entities::DIAGRAM_EDGES.on(&db).until(deadline)
            .bind(("id", app.user.id.clone())).bind(("outgoing", true)).bind(("limit", 33usize)).bind(("start", 0usize)).await;
        assert!(result.is_err());
        eunomia_backend::store::DEADLINE_PROBE.with(|p| {
            assert_eq!(p.borrow().issued, 0);
            assert_eq!(p.borrow().after_deadline, 0);
        });
    }).await;
}
