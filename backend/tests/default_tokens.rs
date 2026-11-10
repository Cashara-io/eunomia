//! Unqualified MCP calls use the new token's own space, including source records and documents.
mod common;

use axum::{body::Body, http::Request};
use common::TestApp;
use eunomia_backend::{cache::search::{self as cache, Envelope, LinkSpec, Placement}, rid, tools::registry};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

async fn mint(app: &TestApp, vault: Option<&str>) -> String {
    let mut body = json!({"name":"default-agent"});
    if let Some(vault) = vault { body["vault_id"] = vault.into(); }
    let (status, issued) = app.http_session("POST", "/api/auth/tokens", Some(body)).await;
    assert_eq!(status, 200);
    issued["token"].as_str().unwrap().to_string()
}

fn call<'a>(app: &'a TestApp, token: &'a str, name: &'a str, args: Value) -> impl std::future::Future<Output = Value> + 'a {
    // The router call carries a large future. Keep each call off the default 2 MiB test stack.
    Box::pin(async move {
        let req = Request::builder().method("POST").uri("/mcp")
            .header("authorization", format!("Bearer {token}")).header("content-type", "application/json")
            .body(Body::from(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}}).to_string())).unwrap();
        let res = app.router.clone().oneshot(req).await.unwrap();
        let status = res.status();
        assert!(status == 200 || status == 403, "{name}: {status}");
        let body: Value = serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
        if status == 403 {
            return json!({"code":body["code"].as_str().or(body["error"]["data"]["code"].as_str()).expect("scope refusal code"),
                "error":body["detail"].as_str().or(body["error"]["message"].as_str()).expect("clear scope refusal")});
        }
        if let Some(value) = body["result"].get("structuredContent") { return value.clone(); }
        serde_json::from_str(body["result"]["content"][0]["text"].as_str().expect("tool result"))
            .unwrap_or_else(|_| panic!("{name} must return JSON or structured content"))
    })
}

async fn ok(app: &TestApp, token: &str, name: &str, args: Value) -> Value {
    let v = call(app, token, name, args).await;
    assert!(v.get("error").is_none(), "{name} failed: {v}");
    v
}

#[tokio::test]
async fn unqualified_record_tools_scope_to_personal_or_the_explicitly_bound_team() {
    let app = TestApp::new().await;
    let team = app.tool("vault_create", json!({"name":"Private team", "kind":"org"})).await["id"].as_str().unwrap().to_string();
    let db = app.db().await;
    let mut personal_record = None;
    for (key, text, routed, links) in [
        ("team", "TEAM_CANARY needle", true, vec![]),
        ("neighbour", "PERSONAL_NEIGHBOUR needle", false, vec![]),
        ("personal", "PERSONAL_CANARY needle", false, vec![LinkSpec {target:"fixture:note:neighbour".into(),rel:"related".into()}, LinkSpec {target:"fixture:note:team".into(),rel:"related".into()}]),
    ] {
        let placement = routed.then(|| Placement { vault:Some(rid::parse(&team).unwrap()), writable:Arc::new(vec![rid::parse(&team).unwrap()]), ..Default::default() });
        let env = Envelope { id:format!("fixture:note:{key}"), source:"fixture".into(), type_:"fixture.note".into(), external_id:key.into(), title:text.into(), body_text:text.into(), placement, links, ..Default::default() };
        let (stored, _) = common::sys(cache::upsert(&db, &app.user.id, &env)).await.unwrap();
        if key == "personal" { personal_record = stored.rid; }
    }
    for (destination, allowed, hidden, id) in [(None,"PERSONAL_CANARY","TEAM_CANARY","personal"), (Some(team.as_str()),"TEAM_CANARY","PERSONAL_CANARY","team")] {
        let token = mint(&app, destination).await;
        for (name, args) in [
            ("list", json!({"type":"fixture.note"})),
            ("search", json!({"query":"needle", "mode":"keyword"})),
            ("get", json!({"id":format!("fixture:note:{id}")})),
        ] {
            let value = ok(&app, &token, name, args).await.to_string();
            assert!(value.contains(allowed), "{name}: {value}");
            assert!(!value.contains(hidden), "{name} escaped its token's space");
        }
        let other = if destination.is_none() { "team" } else { "personal" };
        assert_eq!(call(&app, &token, "get", json!({"id":format!("fixture:note:{other}")})).await["error"], "not found");
        let links = ok(&app, &token, "links", json!({"id":format!("fixture:note:{id}")})).await;
        assert!(!links.to_string().contains(&format!("fixture:note:{other}")));
    }
    let token = mint(&app, None).await;
    db.test_raw().query("UPDATE $record SET deleted = true").bind(("record",personal_record.unwrap())).await.unwrap().check().unwrap();
    assert_eq!(call(&app,&token,"get",json!({"id":"fixture:note:personal"})).await["error"],"not found");
    for (name,args) in [("list",json!({})),("search",json!({"query":"needle","mode":"keyword"})),("links",json!({"id":"fixture:note:neighbour"}))] {
        let result = ok(&app,&token,name,args).await.to_string();
        assert!(!result.contains("fixture:note:personal"),"{name} hides the tombstone");
    }
    assert!(ok(&app,&token,"get",json!({"id":"fixture:note:neighbour"})).await.to_string().contains("PERSONAL_NEIGHBOUR"));
}

#[tokio::test]
async fn foreign_linked_neighbourhood_is_unreadable_with_or_without_selector() {
    let app = TestApp::new().await;
    let foreign = app.tool("vault_create",json!({"name":"Foreign graph","kind":"org"})).await["id"].clone();
    let a = app.tool("memory_write",json!({"vault_id":foreign,"subject_name":"FOREIGN node","subject_kind":"person","text":"FOREIGN fact"})).await["entity"]["id"].clone();
    let b = app.tool("memory_write",json!({"vault_id":foreign,"subject_name":"FOREIGN neighbour","subject_kind":"person","text":"FOREIGN neighbour fact"})).await["entity"]["id"].clone();
    app.tool("entity_relate",json!({"from_id":a,"to_id":b,"label":"knows"})).await;
    let graph = app.tool("entities_neighbourhood",json!({"vault_id":foreign,"entity_id":a})).await;
    assert_eq!(graph["nodes"].as_array().unwrap().len(),2);
    assert_eq!(graph["edges"].as_array().unwrap().len(),1);
    // Exercise the unchanged service too, bypassing the new dispatcher preflight.
    // This distinguishes the old safe mismatch response from a foreign-vault read bypass.
    let bound = app.tool("ontology_get",json!({})).await["vault"].as_str().unwrap().to_string();
    let own = app.tool("memory_write",json!({"subject_name":"Own node","subject_kind":"person","text":"Own fact"})).await["entity"]["id"].clone();
    let caller = eunomia_backend::authz::Caller {
        actor:eunomia_backend::authz::Actor {kind:"token",id:"fixture".into()},
        scopes:vec!["memory:read".into()],vault:Some(rid::parse(&bound).unwrap()),expires_at:None,
    };
    let db = app.db().await;
    let foreign_vault = rid::parse(foreign.as_str().unwrap()).unwrap();
    let foreign_entity = rid::parse(a.as_str().unwrap()).unwrap();
    for selector in [None,Some(&foreign_vault)] {
        let error = eunomia_backend::authz::with_caller(caller.clone(),eunomia_backend::entities::neighbourhood::neighbourhood(&db,&app.user.id,&foreign_entity,None,selector)).await.unwrap_err();
        assert_eq!(error.code,eunomia_backend::error::ErrorCode::AuthScope);
    }
    let own_entity = rid::parse(own.as_str().unwrap()).unwrap();
    let mismatch = eunomia_backend::authz::with_caller(caller,eunomia_backend::entities::neighbourhood::neighbourhood(&db,&app.user.id,&own_entity,None,Some(&foreign_vault))).await.unwrap_err();
    assert_eq!(mismatch.code,eunomia_backend::error::ErrorCode::EntityNotFound);
    let token = mint(&app,None).await;
    for args in [json!({"entity_id":a}),json!({"entity_id":a,"vault_id":foreign})] {
        let result = call(&app,&token,"entities_neighbourhood",args).await;
        assert_eq!(result["code"],"auth.scope","{result}");
        assert!(result.get("nodes").is_none() && result.get("edges").is_none());
        assert!(!result.to_string().contains("FOREIGN"));
    }
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn default_personal_token_uploads_searches_and_resolves_its_document() {
    use eunomia_backend::jobs::{WorkerConfig, handlers, worker};
    let app = TestApp::with_settings(eunomia_backend::config::Settings { embeddings_backend:"stub".into(), ..common::test_settings() }).await;
    let token = mint(&app, None).await;
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let cfg = WorkerConfig { id:"default-token-docs".into(), poll:std::time::Duration::from_millis(20), ..WorkerConfig::default() };
    tokio::spawn(worker::run(app.state.clone(), handlers::registry(), cfg, stopped));
    let text = "The lighthouse restoration budget is 48,000 euros, approved by the harbour board.";
    let uploaded = ok(&app, &token, "document_upload", json!({"filename":"planning.txt","text":text})).await;
    let doc = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let doc = ok(&app, &token, "document_get", json!({"id":uploaded["id"]})).await;
            if doc["document"]["status"] == "ready" { break doc; }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    let chunk = &doc["chunks"][0]["chunk_id"];
    let search = ok(&app, &token, "search", json!({"query":"lighthouse restoration","mode":"keyword"})).await;
    assert_eq!(search["results"][0]["document"]["document_id"], uploaded["id"]);
    let get = ok(&app, &token, "get", json!({"id":chunk})).await;
    assert_eq!(get["document"]["document_id"], uploaded["id"]);
    assert!(ok(&app, &token, "recall", json!({"query":"lighthouse restoration"})).await.to_string().contains(chunk.as_str().unwrap()));
    stop.send(true).unwrap();
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn every_mcp_tool_with_a_default_token_stays_personal_or_has_an_intended_refusal() {
    walk_bound_token(false, false).await;
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn every_mcp_tool_with_a_team_token_stays_bound_or_has_an_intended_refusal() {
    walk_bound_token(true, false).await;
}

#[tokio::test(flavor="multi_thread", worker_threads=2)]
async fn every_mcp_tool_with_a_restricted_admin_token_stays_bound_or_has_an_intended_refusal() {
    walk_bound_token(true, true).await;
}

fn walk_bound_token(team_bound: bool, admin: bool) -> impl std::future::Future<Output = ()> {
    // The combined walk holds many query futures and fixtures. Heap-allocate its
    // state so polling and dropping it fit the default test-thread stack, like dynamic_queries.
    Box::pin(walk_bound_token_scenario(team_bound, admin))
}

async fn walk_bound_token_scenario(team_bound: bool, admin: bool) {
    use eunomia_backend::{jobs::{WorkerConfig, handlers, worker}, scopes};
    use std::collections::BTreeSet;
    let app = Box::pin(TestApp::with_settings(eunomia_backend::config::Settings { embeddings_backend:"stub".into(), public_url:"https://token-walk.example.test".into(), ..common::test_settings() })).await;
    // Prepare the token and foreign records in a separate heap-allocated phase.
    let (bound, team, token, outside, neighbour, edge, foreign_skill, foreign_request) = Box::pin(async {
        let personal = Box::pin(app.tool("ontology_get", json!({}))).await["vault"].as_str().unwrap().to_string();
        let created_team = Box::pin(app.tool("vault_create", json!({"name":"Walk team", "kind":"org"}))).await["id"].as_str().unwrap().to_string();
        let bound = if team_bound { created_team.clone() } else { personal.clone() };
        let team = if team_bound { personal.clone() } else { created_team.clone() };
        let mut body = json!({"name":"walk-token"});
        if team_bound { body["vault_id"] = bound.clone().into(); }
        if admin { body["scopes"] = json!(["memory:read", "memory:write", "vaults:admin"]); }
        let (status, issued) = Box::pin(app.http_session("POST", "/api/auth/tokens", Some(body))).await;
        assert_eq!(status, 200);
        let token = issued["token"].as_str().unwrap().to_string();
        if admin {
            let colleague = common::register(&app.state, "colleague@example.test").await;
            Box::pin(app.tool("vault_invite", json!({"vault_id":bound,"email":colleague.email}))).await;
            Box::pin(common::sys(eunomia_backend::vaults::service::accept_invitation(&app.db().await, &app.state.control, &colleague.id, &rid::parse(&bound).unwrap()))).await.unwrap();
        }
        let request = Request::builder().method("POST").uri("/mcp")
            .header("authorization",format!("Bearer {token}")).header("content-type","application/json")
            .body(Body::from(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}).to_string())).unwrap();
        let response = app.router.clone().oneshot(request).await.unwrap();
        let catalog: Value = serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
        let tools = catalog["result"]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|t|t["name"]=="vault_get"));
        for tool in tools {
            assert!(!tool["inputSchema"]["required"].as_array().is_some_and(|r|r.iter().any(|v|v=="vault_id")),
                "{} must advertise the credential's implicit vault",tool["name"]);
        }
        let get = tools.iter().find(|t|t["name"]=="get").unwrap();
        assert!(get["inputSchema"]["required"].as_array().unwrap().contains(&json!("id")),"ordinary required arguments stay required");
        // Use existing, linked foreign objects. A bound-vault id plus a foreign selector alone
        // cannot distinguish a scope guard from a lookup in the wrong vault.
        let outside = Box::pin(app.tool("memory_write", json!({"vault_id":team,"subject_name":"OUTSIDE_SPACE_CANARY needle","subject_kind":"outside_only_canary","text":"OUTSIDE_SPACE_CANARY"}))).await;
        let neighbour = Box::pin(app.tool("memory_write", json!({"vault_id":team,"subject_name":"OUTSIDE_NEIGHBOUR","subject_kind":"person","text":"OUTSIDE_NEIGHBOUR fact"}))).await;
        let edge = Box::pin(app.tool("entity_relate", json!({"from_id":outside["entity"]["id"],"to_id":neighbour["entity"]["id"],"label":"knows"}))).await;
        let foreign_record = Envelope {id:"fixture:note:foreign".into(),source:"fixture".into(),type_:"fixture.note".into(),external_id:"foreign".into(),title:"OUTSIDE_SPACE_CANARY".into(),body_text:"OUTSIDE_SPACE_CANARY".into(),placement:Some(Placement { vault:Some(rid::parse(&team).unwrap()), writable:Arc::new(vec![rid::parse(&team).unwrap()]), ..Default::default() }),links:vec![LinkSpec {target:"fixture:note:foreign-neighbour".into(),rel:"related".into()}],..Default::default()};
        let mut foreign_neighbour = foreign_record.clone();
        foreign_neighbour.id = "fixture:note:foreign-neighbour".into();
        foreign_neighbour.external_id = "foreign-neighbour".into();
        foreign_neighbour.links.clear();
        for record in [&foreign_neighbour, &foreign_record] {
            Box::pin(common::sys(cache::upsert(&app.db().await,&app.user.id,record))).await.unwrap();
        }
        let foreign_skill = Box::pin(app.tool("skill_save",json!({"vault_id":team,"name":"outside-skill","description":"OUTSIDE_SPACE_CANARY","body":"OUTSIDE_SPACE_CANARY"}))).await;
        let foreign_request = Box::pin(app.tool("secret_request",json!({"vault_id":team,"name":"outside_pending","description":"OUTSIDE_SPACE_CANARY","target":{"hosts":["api.example.test"]}}))).await;
        (bound, team, token, outside, neighbour, edge, foreign_skill, foreign_request)
    }).await;
    // Keep document and worker setup out of the walk loops' future state.
    let (first, second, stop, doc, foreign_doc, foreign_chunk, foreign_link, team_cursor) = Box::pin(async {
        let first = ok(&app,&token,"memory_write",json!({"subject_name":"Personal One needle","subject_kind":"person","text":"PERSONAL needle"})).await;
        let second = ok(&app,&token,"memory_write",json!({"subject_name":"Personal Two needle","subject_kind":"person","text":"PERSONAL neighbour"})).await;
        let record = Envelope {id:"fixture:note:personal".into(),source:"fixture".into(),type_:"fixture.note".into(),external_id:"personal".into(),title:"PERSONAL needle".into(),body_text:"PERSONAL needle".into(),placement:Some(Placement { vault:Some(rid::parse(&bound).unwrap()), writable:Arc::new(vec![rid::parse(&bound).unwrap()]), ..Default::default() }),..Default::default()};
        Box::pin(common::sys(cache::upsert(&app.db().await,&app.user.id,&record))).await.unwrap();
        let (stop, stopped) = tokio::sync::watch::channel(false);
        tokio::spawn(worker::run(app.state.clone(),handlers::registry(),WorkerConfig{id:"default-walk".into(),poll:std::time::Duration::from_millis(20),..WorkerConfig::default()},stopped));
        let doc = ok(&app,&token,"document_upload",json!({"filename":"menu.csv","text":"product,price\nTea,2.00\nCoffee,3.00\n"})).await;
        tokio::time::timeout(std::time::Duration::from_secs(15),async {
            while ok(&app,&token,"document_get",json!({"id":doc["id"]})).await["document"]["status"] != "ready" {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }).await.unwrap();
        let foreign_doc = Box::pin(app.tool("document_upload",json!({"vault_id":team,"filename":"outside.csv","text":"product,price\nOUTSIDE_SPACE_CANARY,999\n"}))).await;
        tokio::time::timeout(std::time::Duration::from_secs(15),async {
            while Box::pin(app.tool("document_get",json!({"id":foreign_doc["id"]}))).await["document"]["status"] != "ready" {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }).await.unwrap();
        let foreign_chunk = Box::pin(app.tool("document_get",json!({"id":foreign_doc["id"]}))).await["chunks"][0]["chunk_id"].clone();
        // Prove the foreign fixtures are readable by the owner and contain a real graph.
        let graph = Box::pin(app.tool("entities_neighbourhood",json!({"entity_id":outside["entity"]["id"],"vault_id":team}))).await;
        assert_eq!(graph["nodes"].as_array().unwrap().len(),2);
        assert_eq!(graph["edges"].as_array().unwrap().len(),1);
        assert!(Box::pin(app.tool("get",json!({"id":"fixture:note:foreign"}))).await.to_string().contains("OUTSIDE_SPACE_CANARY"));
        assert_eq!(Box::pin(app.tool("links",json!({"id":"fixture:note:foreign"}))).await["links"].as_array().unwrap().len(),1);
        let foreign_link = "11111111111111111111111111111111";
        // A host link never opens a connection during sync_now. Keep the fixture in memory.
        app.db().await.test_raw().query("CREATE $id SET vault = $vault, direction = 'host', role = 'read', key_enc = 'unused-fixture-key', user = $user, peer_instance = 'outside-fixture'")
            .bind(("id",rid::parse(&format!("peer_link:{foreign_link}")).unwrap())).bind(("vault",rid::parse(&team).unwrap())).bind(("user",app.user.id.clone()))
            .await.unwrap().check().unwrap();
        assert!(Box::pin(app.tool("vault_peer_sync",json!({"link_id":foreign_link}))).await.get("error").is_none());
        let team_cursor = Box::pin(app.tool("changes_since",json!({"vault_id":team}))).await["next"].clone();
        (first, second, stop, doc, foreign_doc, foreign_chunk, foreign_link, team_cursor)
    }).await;
    let a = &first["entity"]["id"]; let b = &second["entity"]["id"]; let fact = &first["memory"]["id"];
    let x = &outside["entity"]["id"];
    let y = &neighbour["entity"]["id"];
    let (product, skill, asked) = Box::pin(async {
        Box::pin(app.tool("ontology_propose",json!({"vault_id":bound,"kinds":[{"kind":"product"}],"properties":[{"kind":"product","name":"price","datatype":"money","unit":"AUD","source_map":[{"source":"document","record_type":doc["id"],"path":"price"}]}],"metrics":[{"kind":"product","name":"total_price","expr":{"agg":"sum","property":"price"}}]}))).await;
        for (route,body) in [("kinds",json!({"kind":"product","decision":{"action":"approve"}})),("properties",json!({"kind":"product","name":"price","decision":{"action":"approve"}})),("metrics",json!({"kind":"product","name":"total_price","decision":{"action":"approve"}}))] {
            assert_eq!(Box::pin(app.http_session("POST",&format!("/api/vaults/{bound}/ontology/{route}/decide"),Some(body))).await.0,200);
        }
        let product = ok(&app,&token,"memory_write",json!({"subject_kind":"product","subject_name":"Fixture product","text":"PERSONAL product"})).await["entity"].clone();
        let skill = ok(&app,&token,"skill_save",json!({"name":"personal-fixture","description":"PERSONAL","body":"PERSONAL needle"})).await;
        let asked = ok(&app,&token,"secret_request",json!({"name":"pending_fixture","description":"Fixture request","target":{"hosts":["api.example.test"]}})).await;
        // Without a model the tool returns an empty no-op before it looks up any subject.
        // Also exercise the service used when a model exists: it checks the vault before
        // loading facts or calling the model, and must reject this real foreign subject.
        let consolidation_caller = eunomia_backend::authz::Caller {
            actor:eunomia_backend::authz::Actor {kind:"token",id:"fixture".into()},
            scopes:vec!["memory:write".into()],vault:Some(rid::parse(&bound).unwrap()),expires_at:None,
        };
        let denied = Box::pin(eunomia_backend::authz::with_caller(consolidation_caller,eunomia_backend::entities::consolidate::consolidate_subject(&app.db().await,&app.state.settings,&app.user.id,&rid::parse(x.as_str().unwrap()).unwrap(),None))).await.unwrap_err();
        assert_eq!(denied.code,eunomia_backend::error::ErrorCode::AuthScope);
        (product, skill, asked)
    }).await;
    let pending = &asked["handle"]["$secret"];
    // Foreign-object and selector checks have their own small poll frame.
    Box::pin(async {
        let foreign_cases = vec![
            ("get",json!({"id":"fixture:note:foreign"}),"record"),
            ("get",json!({"id":foreign_chunk}),"record"),
            ("links",json!({"id":"fixture:note:foreign"}),"links"),
            ("list",json!({"filters":{"id":"fixture:note:foreign"}}),"list"),
            ("entities_get",json!({"id":x}),"scope"),
            ("entities_neighbourhood",json!({"entity_id":x}),"scope"),
            ("entity_update",json!({"entity_id":x,"summary":"must not change"}),"scope"),
            ("entity_delete",json!({"entity_id":x}),"scope"),
            ("entity_set_property",json!({"entity_id":x,"property":"price","value":1}),"scope"),
            ("memory_update",json!({"memory_id":outside["memory"]["id"],"text":"must not change"}),"scope"),
            ("memory_delete",json!({"memory_id":outside["memory"]["id"]}),"scope"),
            ("consolidate_observations",json!({"subject_id":x}),"no_model"),
            ("code_entity_upsert",json!({"kind":"file","name":"foreign-parent","parent_id":x}),"scope"),
            ("code_relate",json!({"from_id":x,"to_id":y,"label":"knows"}),"scope"),
            ("code_relate",json!({"from_id":a,"to_id":y,"label":"knows"}),"scope"),
            ("entity_relate",json!({"from_id":x,"to_id":b,"label":"knows"}),"scope"),
            ("entity_relate",json!({"from_id":a,"to_id":y,"label":"knows"}),"scope"),
            ("entity_merge",json!({"winner_id":x,"loser_id":b}),"scope"),
            ("entity_merge",json!({"winner_id":a,"loser_id":y}),"scope"),
            ("metric_eval",json!({"metric":"total_price","related_to":{"entity_id":x}}),"unknown"),
            ("scenario_run",json!({"changes":[{"set":{"entity_id":x,"property":"price","value":1}}]}),"unknown"),
            ("scenario_run",json!({"changes":[{"remove":{"entity_id":x}}]}),"unknown"),
            ("scenario_run",json!({"changes":[{"scale":{"kind":"product","property":"price","factor":2,"entity_ids":[x]}}]}),"unknown"),
            ("scenario_run",json!({"changes":[{"shift":{"kind":"product","property":"price","delta":2,"related_to":{"entity_id":x}}}]}),"unknown"),
            ("scenario_run",json!({"changes":[{"scale":{"kind":"product","property":"price","factor":2}}],"impact_from":[x]}),"unknown"),
            ("show_view",json!({"view":"subgraph","title":"Test","entity_ids":[x]}),"scope"),
            ("show_view",json!({"view":"subgraph","title":"Test","entity_ids":[],"relation_ids":[edge["id"]]}),"scope"),
            ("show_view",json!({"view":"impact","start_id":x,"headline":"Test","affected":[]}),"scope"),
            ("show_view",json!({"view":"metric","metric":"total_price","related_to":{"entity_id":x}}),"unknown"),
            ("show_view",json!({"view":"scenario","headline":"Test","scenario":{"changes":[{"remove":{"entity_id":x}}]}}),"unknown"),
            ("document_get",json!({"id":foreign_doc["id"]}),"scope"),
            ("document_download",json!({"id":foreign_doc["id"]}),"scope"),
            ("document_export",json!({"id":foreign_doc["id"]}),"scope"),
            ("document_delete",json!({"id":foreign_doc["id"]}),"scope"),
            ("document_upload",json!({"document_id":foreign_doc["id"],"filename":"overwrite.csv","text":"must not change"}),"scope"),
            ("ontology_automap",json!({"document_id":foreign_doc["id"]}),"scope"),
            ("document_import_rows",json!({"document_id":foreign_doc["id"],"kind":"product","name_column":"product"}),"scope"),
            ("skill_get",json!({"id":foreign_skill["id"]}),"absent"),
            ("skill_delete",json!({"id":foreign_skill["id"]}),"absent"),
            ("secret_request_status",json!({"request_id":foreign_request["request_id"]}),"request"),
            ("vault_peer_sync",json!({"link_id":foreign_link}),"scope"),
            ("secret_transform",json!({"op":"base64","inputs":{"value":foreign_request["handle"]}}),"unavailable"),
            ("http_request",json!({"url":"https://api.example.test/","headers":{"Authorization":foreign_request["handle"]}}),"unavailable"),
        ];
        let mut id_walked = BTreeSet::new();
        for (name,args,behaviour) in foreign_cases {
            id_walked.insert(name);
            // The id-only call is essential: entity, memory and document ids are global.
            // Repeat with an explicit foreign selector wherever the tool accepts one.
            let mut variants = vec![args.clone()];
            if registry::all_tools()[name].schema["properties"].get("vault_id").is_some() {
                let mut explicit = args;
                explicit["vault_id"] = team.clone().into();
                variants.push(explicit);
            }
            for (i,args) in variants.into_iter().enumerate() {
                let v = call(&app,&token,name,args).await;
                match if i == 1 { "scope" } else { behaviour } {
                    "scope" => assert_eq!(v["code"],"auth.scope","foreign id {name}: {v}"),
                    // Cache queries include the bound vault in the WHERE clause. Foreign ids
                    // therefore behave exactly like missing ids, including empty link lists.
                    "record" => {
                        // The registry adds its standard error code and trace id, but no data.
                        assert_eq!(v["error"],"not found","{name}: {v}");
                        assert_eq!(v["code"],"validation.invalid","{name}: {v}");
                        assert!(v["trace_id"].as_str().is_some_and(|s| !s.is_empty()));
                        assert_eq!(v.as_object().unwrap().len(),3);
                    }
                    "links" => assert_eq!(v,json!({"links":[]}),"{name}"),
                    "list" => { assert_eq!(v["results"],json!([])); assert_eq!(v["total"],0); }
                    // Scenario/metric Named::load retains only entities in the authorised
                    // default vault. Its unknown-entity error contains only the supplied id.
                    "unknown" => {
                        assert_eq!(v["code"],"entity.not_found","{name}: {v}");
                        assert!(v["error"].as_str().unwrap().contains("unknown entity"),"{name}: {v}");
                    }
                    // Skills deliberately hide existence from callers outside the vault.
                    "absent" => assert_eq!(v["error"],format!("no skill {}",foreign_skill["id"].as_str().unwrap()),"{name}: {v}"),
                    // Request queries filter both the bound vault and the requesting person.
                    "request" => assert_eq!(v["error"],"No such secret request of yours.","{name}: {v}"),
                    // No model means no subject lookup or write. The direct service check
                    // above proves the guarded model-enabled path too.
                    "no_model" => {
                        for key in ["consolidated","skipped","errors"] { assert_eq!(v[key],json!([]),"{name}: {v}"); }
                        assert!(v["note"].as_str().unwrap().starts_with("Eunomia has no model configured"));
                        assert_eq!(v.as_object().unwrap().len(),4);
                    }
                    "unavailable" => assert_eq!(v["code"],"secret.unavailable","{name}: {v}"),
                    _ => unreachable!(),
                }
                eprintln!("TOKEN_WALK foreign_id {name} explicit={i}: {}",if i == 1 { "scope" } else { behaviour });
                assert!(!v.to_string().contains("OUTSIDE_SPACE_CANARY") && !v.to_string().contains("OUTSIDE_NEIGHBOUR"),"{name} disclosed a foreign object");
            }
        }
        // source_record_id is a provenance label, never a lookup or mutation target.
        // Existing-entity targets are covered above; memory_write creates/finds by name.
        // Keep new id-bearing tools from silently escaping the foreign-object walk.
        for (name,spec) in registry::all_tools() {
            let properties = spec.schema["properties"].as_object().unwrap();
            let object_id = properties.keys().any(|k| matches!(k.as_str(),"id"|"entity_id"|"subject_id"|"memory_id"|"parent_id"|"from_id"|"to_id"|"winner_id"|"loser_id"|"document_id"|"request_id"|"link_id"|"entity_ids"|"related_to"|"impact_from"));
            assert!(!object_id || id_walked.contains(name),"{name} needs a foreign-object case");
        }
    }).await;
    // Run the complete local and admin catalogue walk without an inline combined future.
    Box::pin(async {
        // The batch-level default must not hide an item's attempted vault override.
        let batch = call(&app,&token,"memory_write_batch",json!({"items":[{"vault_id":team,"subject_name":"Escape item","subject_kind":"person","text":"must not write"}]})).await;
        assert_eq!(batch["written"],0);
        assert_eq!(batch["results"][0]["code"],"auth.scope","{batch}");
        let cases = vec![
            ("docs",json!({})), ("list",json!({})), ("search",json!({"query":"needle","mode":"keyword"})),
            ("get",json!({"id":"fixture:note:personal"})), ("links",json!({"id":"fixture:note:personal"})),
            ("vault_list",json!({})), ("vault_get",json!({})), ("vault_members",json!({})), ("vault_check",json!({})), ("vault_invitations",json!({})), ("vault_peers",json!({})),
            ("memory_write",json!({"subject_name":"Personal Extra","subject_kind":"person","text":"PERSONAL"})),
            ("memory_write_batch",json!({"items":[{"subject_name":"Personal Batch","subject_kind":"person","text":"PERSONAL"}]})),
            ("recall",json!({"query":"needle"})), ("reflect",json!({"query":"needle"})),
            ("entities_search",json!({"query":"needle"})), ("entities_get",json!({"id":a})), ("entities_graph",json!({})),
            ("entities_neighbourhood",json!({"entity_id":a})), ("entity_types",json!({})),
            ("entity_update",json!({"entity_id":a,"summary":"PERSONAL"})),
            ("code_entity_upsert",json!({"kind":"repository","name":"personal-repository"})),
            ("code_relate",json!({"from_id":a,"to_id":b,"label":"works_with"})),
            ("entity_relate",json!({"from_id":a,"to_id":b,"label":"knows","reason":"PERSONAL"})),
            ("ontology_get",json!({})), ("ontology_coverage",json!({"question":"What is the total price?"})), ("ontology_readiness",json!({})),
            ("ontology_propose",json!({"kinds":[{"kind":"personal_note"}]})), ("ontology_suggest",json!({})),
            ("entity_set_property",json!({"entity_id":product["id"],"property":"price","value":2})),
            ("metric_eval",json!({"metric":"total_price"})),
            ("scenario_run",json!({"changes":[{"scale":{"kind":"product","property":"price","factor":1.5}}],"metrics":["total_price"]})),
            ("show_view",json!({"view":"subgraph","title":"PERSONAL","entity_ids":[a,b]})),
            ("ontology_automap",json!({"document_id":doc["id"]})),
            ("document_import_rows",json!({"document_id":doc["id"],"kind":"product","name_column":"product"})),
            ("document_list",json!({})), ("document_get",json!({"id":doc["id"]})), ("document_download",json!({"id":doc["id"]})), ("document_export",json!({"id":doc["id"]})),
            ("document_upload",json!({"filename":"extra.txt","text":"PERSONAL needle"})),
            ("skill_find",json!({"query":"PERSONAL"})), ("skill_list",json!({})), ("skill_get",json!({"id":skill["id"]})),
            ("skill_save",json!({"name":"extra-skill","description":"PERSONAL","body":"PERSONAL"})), ("skill_delete",json!({"id":skill["id"]})),
            ("changes_since",json!({})),
            ("secret_request",json!({"name":"extra_pending","description":"Fixture request","target":{"hosts":["api.example.test"]}})),
            ("secret_request_status",json!({"request_id":asked["request_id"]})),
            ("secret_transform",json!({"op":"base64","inputs":{"value":{"$secret":pending}}})),
            ("http_request",json!({"url":"https://api.example.test/","headers":{"Authorization":{"$secret":pending}}})),
            ("consolidate_observations",json!({"subject_id":a})),
            ("memory_update",json!({"memory_id":fact,"text":"PERSONAL revised"})),
            ("entity_merge",json!({"winner_id":a,"loser_id":b})),
            ("memory_delete",json!({"memory_id":fact})), ("entity_delete",json!({"entity_id":a})), ("document_delete",json!({"id":doc["id"]})),
        ];
        let mut walked = BTreeSet::new();
        for (name,args) in cases {
            assert!(walked.insert(name));
            // Every tool advertising a vault selector must reject the same valid foreign vault.
            if registry::all_tools()[name].schema["properties"].get("vault_id").is_some() {
                let mut foreign = args.clone();
                foreign["vault_id"] = team.clone().into();
                let refused = call(&app,&token,name,foreign).await;
                // A batch tool reports the refusal per item: nothing written, every item refused.
                if let Some(results) = refused["results"].as_array() {
                    assert_eq!(refused["written"], 0, "foreign {name}: {refused}");
                    eprintln!("TOKEN_WALK selector {name}: per_item");
                    assert!(!results.is_empty() && results.iter().all(|r| r["code"] == "auth.scope"), "foreign {name}: {refused}");
                } else {
                    assert_eq!(refused["code"], "auth.scope", "foreign {name}: {refused}");
                    eprintln!("TOKEN_WALK selector {name}: auth.scope");
                }
            }
            if registry::all_tools()[name].schema["properties"].get("vault_ids").is_some() {
                let mut mixed = args.clone();
                mixed["vault_ids"] = json!([bound,team]);
                assert_eq!(call(&app,&token,name,mixed).await["code"],"auth.scope","mixed vaults {name}");
            }
            let value = call(&app,&token,name,args).await;
            if matches!(name,"secret_transform"|"http_request") {
                assert_eq!(value["code"],"secret.unavailable","{name}: {value}");
                assert!(value["error"].as_str().is_some_and(|m| !m.is_empty()));
            } else { assert!(value.get("error").is_none(),"{name}: {value}"); }
            let public = value.to_string();
            assert!(!public.contains("OUTSIDE_SPACE_CANARY") && !public.contains("outside_only_canary") && !public.contains(&team),"{name} escaped its binding");
        }
        for (name,args) in [("vault_get",json!({"vault_id":team})),("search",json!({"vault_id":team,"query":"needle"})),("memory_write",json!({"vault_id":team,"subject_name":"Escape","subject_kind":"person","text":"must not write"})),("document_upload",json!({"vault_id":team,"filename":"escape.txt","text":"must not write"}))] {
            let ((_,value),_) = Box::pin(common::http(&app.router,"POST",&format!("/api/tools/{name}"),Some(args),Some(&token),None)).await;
            assert_eq!(value["code"],"auth.scope","explicit {name} must not override the credential: {value}");
        }
        // Valid arguments reach the binding guard even when administrative scope is granted.
        // Membership refusals use the sole owner, so the walk cannot remove its own authority.
        let admin_cases = [
            ("vault_create", json!({"name":"Escape"}), Some("bound")),
            ("vault_clone", json!({}), Some("bound")),
            ("vault_merge", json!({"vault_id_a":bound,"vault_id_b":team}), Some("bound")),
            ("vault_join_shared", json!({"share_code":"unused-fixture"}), Some("bound")),
            ("vault_repair", json!({}), None),
            ("vault_rename", json!({"name":"Renamed bound"}), None),
            ("vault_share_code", json!({}), None),
            ("vault_invite", json!({"email":app.user.email}), Some("already a member")),
            ("vault_remove_member", json!({"email":app.user.email}), Some("last owner")),
            ("vault_set_role", json!({"email":app.user.email,"role":"owner"}), None),
            ("vault_transfer_ownership", json!({"email":app.user.email}), Some("already own")),
            ("vault_accept_invitation", json!({}), Some("no pending invitation")),
            ("vault_decline_invitation", json!({}), Some("no pending invitation")),
            ("vault_peer_sync", json!({"link_id":"00000000000000000000000000000000"}), Some("No such shared space link.")),
            ("vault_leave", json!({}), Some("last owner")),
            // Last: deleting the bound team is permitted with admin scope.
            ("vault_delete", json!({}), None),
        ];
        for (name,args,expected) in admin_cases {
            assert_eq!(scopes::for_tool(name,registry::is_read_only(name)), scopes::VAULTS_ADMIN);
            assert!(walked.insert(name));
            if registry::all_tools()[name].schema["properties"].get("vault_id").is_some() {
                let mut foreign = args.clone();
                foreign["vault_id"] = team.clone().into();
                assert_eq!(call(&app,&token,name,foreign).await["code"], "auth.scope", "foreign {name}");
                eprintln!("TOKEN_WALK selector {name}: auth.scope");
            }
            let v = call(&app,&token,name,args).await;
            if !admin || expected == Some("bound") {
                assert_eq!(v["code"],"auth.scope","{name}: {v}");
            } else if let Some(message) = expected {
                assert!(v["error"].as_str().is_some_and(|s| s.contains(message)), "{name}: {v}");
                assert_ne!(v["code"], "auth.scope", "admin must exercise the service guard");
            } else { assert!(v.get("error").is_none(), "{name}: {v}"); }
            assert!(!v.to_string().contains("OUTSIDE_SPACE_CANARY"), "{name} leaked the foreign space");
        }
        assert_eq!(walked,registry::all_tools().keys().copied().collect());
        assert_eq!(Box::pin(app.tool("changes_since",json!({"vault_id":team}))).await["next"],team_cursor,"no unqualified write changed the team space");
    }).await;
    stop.send(true).unwrap();
}
