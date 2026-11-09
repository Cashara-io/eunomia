mod common;
use common::TestApp;
use serde_json::{Value, json};

async fn setup(app: &TestApp) -> String {
    app.tool("ontology_get", json!({})).await["vault"]
        .as_str()
        .unwrap()
        .to_string()
}
async fn propose(app: &TestApp, vault: &str, description: &str) {
    let current = app.tool("ontology_get", json!({"vault_id":vault})).await;
    let revision = current["kinds"].as_array().unwrap().iter().find(|k| k["kind"] == "shop").and_then(|k| k["revision"].as_i64()).unwrap_or(0);
    let out = app.tool("ontology_propose", json!({"vault_id":vault,"kinds":[{"kind":"shop","revision":revision,"description":description,"reason":"From stores.csv"}]})).await;
    assert!(out.get("error").is_none(), "{out}");
}
async fn activity(app: &TestApp, vault: &str) -> Vec<Value> {
    let (status, body) = app
        .http_session(
            "GET",
            &format!("/api/vaults/{vault}/ontology/activity"),
            None,
        )
        .await;
    assert_eq!(status, 200, "{body}");
    body["events"].as_array().unwrap().clone()
}
async fn decide(app: &TestApp, vault: &str, revision: i64, decision: Value) -> (u16, Value) {
    let (status, body) = app.http_session("POST", &format!("/api/vaults/{vault}/ontology/kinds/decide"), Some(json!({"kind":"shop","revision":revision,"reason":"Matches the source records","decision":decision}))).await;
    (status.as_u16(), body)
}

#[tokio::test]
async fn revisions_reasons_history_and_stale_decisions_are_durable() {
    let app = TestApp::new().await;
    let vault = setup(&app).await;
    propose(&app, &vault, "Original description").await;
    let events = activity(&app, &vault).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["action"], "proposed");
    assert_eq!(events[0]["revision"], 1);
    propose(&app, &vault, "An agent changed this").await;
    let (status, body) = decide(&app, &vault, 1, json!({"action":"approve"})).await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["code"], "ontology.stale_revision");
    assert_eq!(
        activity(&app, &vault).await.len(),
        2,
        "failed decisions append nothing"
    );
    let (status, body) = decide(&app, &vault, 2, json!({"action":"approve"})).await;
    assert_eq!(status, 200, "{body}");
    let events = activity(&app, &vault).await;
    assert_eq!(events[2]["action"], "approved");
    assert_eq!(events[2]["reason"], "Matches the source records");
    assert_eq!(events[2]["actor_type"], "user");
    assert_eq!(events[2]["actor_name"], app.user.email);
    assert_eq!(events[2]["before"]["revision"], 2);
    assert_eq!(events[2]["after"]["revision"], 3);
    let (status, _) = decide(
        &app,
        &vault,
        3,
        json!({"action":"describe","description":"Reviewed description"}),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = decide(
        &app,
        &vault,
        4,
        json!({"action":"reject","merge_into":"organisation"}),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = app
        .http_session(
            "POST",
            &format!("/api/vaults/{vault}/ontology/strict"),
            Some(json!({"on":true})),
        )
        .await;
    assert_eq!(status, 200);
    let events = activity(&app, &vault).await;
    let actions: Vec<_> = events
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert_eq!(
        actions,
        [
            "proposed",
            "proposed",
            "approved",
            "described",
            "merged",
            "strict_changed"
        ]
    );
    let seqs: Vec<_> = events.iter().map(|e| e["seq"].as_i64().unwrap()).collect();
    assert_eq!(seqs, [1, 2, 3, 4, 5, 6]);
    // Both cursor directions and combined filters are server-side.
    let (_, page) = app.http_session("GET", &format!("/api/vaults/{vault}/ontology/activity?after=2&limit=1&actor_type=user&definition_type=kind&kind=shop&name=shop&action=approved"), None).await;
    assert_eq!(page["events"].as_array().unwrap().len(), 1);
    assert_eq!(page["events"][0]["seq"], 3);
    let (_, page) = app
        .http_session(
            "GET",
            &format!("/api/vaults/{vault}/ontology/activity?newest=true&before=5&limit=2"),
            None,
        )
        .await;
    assert_eq!(page["events"][0]["seq"], 4);
    assert_eq!(page["next"], 3);
    assert_eq!(page["more"], true);
}

#[tokio::test]
async fn every_definition_type_records_proposals_and_rejections() {
    let app = TestApp::new().await;
    let vault = setup(&app).await;
    let out = app
        .tool(
            "ontology_propose",
            json!({"vault_id":vault,
        "kinds":[{"kind":"shop"}],
        "properties":[{"kind":"shop","name":"price","datatype":"number"}],
        "metrics":[{"kind":"shop","name":"total","expr":{"agg":"count"}}],
        "links":[{"from_kind":"shop","label":"near","to_kind":"shop"}]}),
        )
        .await;
    assert!(out.get("error").is_none(), "{out}");
    for (typ, fields) in [
        ("properties", json!({"kind":"shop","name":"price"})),
        ("metrics", json!({"kind":"shop","name":"total"})),
        (
            "links",
            json!({"from_kind":"shop","label":"near","to_kind":"shop"}),
        ),
    ] {
        let mut body = fields;
        body["revision"] = json!(1);
        body["reason"] = json!("Not supported by source data");
        body["decision"] = json!({"action":"reject"});
        let (status, out) = app
            .http_session(
                "POST",
                &format!("/api/vaults/{vault}/ontology/{typ}/decide"),
                Some(body),
            )
            .await;
        assert_eq!(status, 200, "{out}");
    }
    let events = activity(&app, &vault).await;
    assert_eq!(events.len(), 7, "{events:?}");
    assert_eq!(
        events
            .iter()
            .filter(|e| e["action"] == "rejected" && e["reason"] == "Not supported by source data")
            .count(),
        3
    );
}

#[tokio::test]
async fn typed_values_and_import_summaries_record_only_committed_writes() {
    let app = TestApp::new().await;
    let vault = setup(&app).await;
    let out = app.tool("ontology_propose", json!({"vault_id":vault,"kinds":[{"kind":"shop"}],"properties":[{"kind":"shop","name":"price","datatype":"number"}]})).await;
    assert!(out.get("error").is_none(), "{out}");
    assert_eq!(
        decide(&app, &vault, 1, json!({"action":"approve"})).await.0,
        200
    );
    let (status, out) = app
        .http_session(
            "POST",
            &format!("/api/vaults/{vault}/ontology/properties/decide"),
            Some(
                json!({"kind":"shop","name":"price","revision":1,"decision":{"action":"approve"}}),
            ),
        )
        .await;
    assert_eq!(status, 200, "{out}");
    let entity = app.tool("memory_write", json!({"vault_id":vault,"subject_name":"Central","subject_kind":"shop","text":"Our shop"})).await;
    let out = app
        .tool(
            "entity_set_property",
            json!({"entity_id":entity["entity"]["id"],"property":"price","value":12.5}),
        )
        .await;
    assert!(out.get("error").is_none(), "{out}");
    let doc = app.tool("document_upload", json!({"vault_id":vault,"filename":"shops.csv","text":"name,price\nNorth,20\nSouth,30\n"})).await;
    assert!(doc.get("error").is_none(), "{doc}");
    let out = app.tool("document_import_rows", json!({"document_id":doc["id"],"kind":"shop","name_column":"name","column_map":{"price":"price"}})).await;
    assert!(out.get("error").is_none(), "{out}");
    let events = activity(&app, &vault).await;
    let written = events
        .iter()
        .find(|e| e["action"] == "value_written")
        .unwrap();
    assert_eq!(written["after"], json!({"values":1}));
    let imported = events.iter().find(|e| e["action"] == "imported").unwrap();
    assert_eq!(imported["after"]["values"], 2);
}

#[tokio::test]
async fn a_stale_metric_gets_a_conflict_before_dependency_validation() {
    let app = TestApp::new().await;
    let vault = setup(&app).await;
    let args = json!({"vault_id":vault,"kinds":[{"kind":"shop"}],"properties":[{"kind":"shop","name":"price","datatype":"number"}],"metrics":[{"kind":"shop","name":"total","expr":{"agg":"sum","property":"price"}}]});
    let out = app.tool("ontology_propose", args).await;
    assert!(out.get("error").is_none(), "{out}");
    let out = app.tool("ontology_propose", json!({"vault_id":vault,"metrics":[{"kind":"shop","name":"total","revision":1,"description":"Changed while open","expr":{"agg":"sum","property":"price"}}]})).await;
    assert!(out.get("error").is_none(), "{out}");
    let (status, out) = app
        .http_session(
            "POST",
            &format!("/api/vaults/{vault}/ontology/metrics/decide"),
            Some(
                json!({"kind":"shop","name":"total","revision":1,"decision":{"action":"approve"}}),
            ),
        )
        .await;
    assert_eq!(status, 409, "{out}");
    assert_eq!(out["code"], "ontology.stale_revision");
}

#[tokio::test]
async fn a_failed_event_append_rolls_back_the_definition_and_cursor() {
    let app = TestApp::new().await;
    let vault = setup(&app).await;
    let db = app.db().await;
    db.test_raw().query("DEFINE FIELD OVERWRITE name ON ontology_event TYPE string ASSERT $value != 'shop'")
        .await.unwrap().check().unwrap();
    let out = app
        .tool(
            "ontology_propose",
            json!({"vault_id":vault,"kinds":[{"kind":"shop","reason":"force event failure"}]}),
        )
        .await;
    assert!(out.get("error").is_some(), "{out}");
    let ontology = app.tool("ontology_get", json!({"vault_id":vault})).await;
    assert!(
        !ontology["kinds"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k["kind"] == "shop")
    );
    assert!(activity(&app, &vault).await.is_empty());
    assert_eq!(
        eunomia_backend::ontology::activity::head(
            &db,
            &eunomia_backend::rid::parse(&vault).unwrap()
        )
        .await
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn an_n_minus_one_import_refuses_before_creating_any_objects() {
    let app = TestApp::new().await;
    let doc = app
        .tool(
            "document_upload",
            json!({"filename":"shops.csv","text":"name,price\nNorth,20\n"}),
        )
        .await;
    assert!(doc.get("error").is_none(), "{doc}");
    app.control()
        .test_raw()
        .query("UPDATE $tenant SET schema_version = 41")
        .bind(("tenant", app.user.org.tenant_record()))
        .await
        .unwrap()
        .check()
        .unwrap();
    app.state.pool.evict(&app.user.org);
    let out = app
        .tool(
            "document_import_rows",
            json!({"document_id":doc["id"],"kind":"shop","name_column":"name"}),
        )
        .await;
    assert!(
        out["error"]
            .as_str()
            .unwrap_or_default()
            .contains("upgrading"),
        "{out}"
    );
    let db = app.db().await;
    let objects: Vec<serde_json::Value> = db
        .test_raw()
        .query("SELECT * FROM entity")
        .await
        .unwrap()
        .take(0)
        .unwrap();
    assert!(objects.is_empty());
}

#[tokio::test]
async fn link_histories_distinguish_the_target_kind() {
    let app = TestApp::new().await;
    let vault = setup(&app).await;
    let out = app
        .tool(
            "ontology_propose",
            json!({"vault_id":vault,"links":[
                {"from_kind":"shop","label":"near","to_kind":"location"},
                {"from_kind":"shop","label":"near","to_kind":"organisation"}
            ]}),
        )
        .await;
    assert!(out.get("error").is_none(), "{out}");
    let (status, out) = app.http_session("GET", &format!("/api/vaults/{vault}/ontology/activity?definition_type=link&kind=shop&name=shop%20near%20location&review_only=true"), None).await;
    assert_eq!(status, 200, "{out}");
    assert_eq!(out["events"].as_array().unwrap().len(), 1);
    assert_eq!(out["events"][0]["name"], "shop near location");
}

#[tokio::test]
async fn review_guard_proposal_transactions_refuse_stale_or_missing_revisions_after_person_edits() {
    let app = TestApp::new().await;
    let vault = setup(&app).await;
    let items = [
        ("kinds", json!({"kind":"shop"}), "kinds"),
        ("links", json!({"from_kind":"shop","label":"near","to_kind":"location"}), "links"),
        ("properties", json!({"kind":"shop","name":"note","datatype":"text"}), "properties"),
        ("metrics", json!({"kind":"shop","name":"total","expr":{"agg":"count"}}), "metrics"),
    ];
    for (list, fields, route) in items {
        let mut args = json!({"vault_id":vault});
        args[list] = json!([fields]);
        let out = app.tool("ontology_propose", args.clone()).await;
        assert!(out.get("error").is_none(), "{out}");
        let mut edit = fields.clone();
        edit["revision"] = json!(1);
        edit["decision"] = json!({"action":"describe","description":"Person's newer description"});
        let (status, body) = app.http_session("POST", &format!("/api/vaults/{vault}/ontology/{route}/decide"), Some(edit)).await;
        assert_eq!(status, 200, "{body}");
        let before = activity(&app, &vault).await;
        for revision in [None, Some(1)] {
            let mut stale = fields.clone();
            stale["description"] = json!("Agent's stale description");
            if let Some(revision) = revision { stale["revision"] = json!(revision); }
            args[list] = json!([stale]);
            let out = app.tool("ontology_propose", args.clone()).await;
            assert_eq!(out["code"], "ontology.stale_revision", "{out}");
        }
        // Exercise the store transactions directly as well, bypassing service preflight checks.
        use eunomia_backend::store::ontology as q;
        let statements = match list {
            "kinds" => [&q::DESCRIBE_KIND, &q::DECIDE_KIND],
            "links" => [&q::DESCRIBE_LINK, &q::DECIDE_LINK],
            "properties" => [&q::DESCRIBE_PROPERTY, &q::DECIDE_PROPERTY],
            _ => [&q::DESCRIBE_METRIC, &q::DECIDE_METRIC],
        };
        let id = eunomia_backend::rid::parse(before.last().unwrap()["target"].as_str().unwrap()).unwrap();
        let db = app.db().await;
        for stmt in statements {
            for expected in [None, Some(1)] {
                let result = stmt.on(&db).bind(("id",id.clone())).bind(("expected_revision",expected)).await;
                let error = result.and_then(|res| res.check()).expect_err("stale or missing revision must fail inside the transaction");
                assert!(error.to_string().contains("ontology.stale_revision"),"{error}");
            }
        }
        assert_eq!(activity(&app, &vault).await, before, "CAS failure appends nothing");
        let latest = app.tool("ontology_get", json!({"vault_id":vault})).await;
        let entries = if list == "properties" || list == "metrics" { &latest["kinds"].as_array().unwrap().iter().find(|k| k["kind"] == "shop").unwrap()[list] } else { &latest[list] };
        let item = entries.as_array().unwrap().iter().find(|k| if list == "kinds" { k["kind"] == "shop" } else if list == "links" { k["label"] == "near" } else { k["name"] == fields["name"] }).unwrap();
        assert_eq!(item["description"], "Person's newer description");
        assert_eq!(item["revision"], 2);
        let mut fresh = fields;
        fresh["revision"] = json!(2);
        fresh["description"] = json!("Agent reviewed the new version");
        args[list] = json!([fresh]);
        let out = app.tool("ontology_propose", args).await;
        assert!(out.get("error").is_none(), "{out}");
    }
}

#[tokio::test]
async fn review_guard_imports_emit_one_summary_for_links_only_and_multiple_value_chunks() {
    let app = TestApp::new().await;
    let vault = setup(&app).await;
    app.tool("ontology_propose", json!({"kinds":[{"kind":"shop"}],"properties":[{"kind":"shop","name":"note","datatype":"text"}],"links":[{"from_kind":"shop","label":"near","to_kind":"location","source_map":[{"source":"document","record_type":"any","path":"location"}]}]})).await;
    for (route, fields) in [("kinds",json!({"kind":"shop"})), ("properties",json!({"kind":"shop","name":"note"})), ("links",json!({"from_kind":"shop","label":"near","to_kind":"location"}))] {
        let mut body = fields;
        body["revision"] = json!(1);
        body["decision"] = json!({"action":"approve"});
        let (status, out) = app.http_session("POST", &format!("/api/vaults/{vault}/ontology/{route}/decide"), Some(body)).await;
        assert_eq!(status, 200, "{out}");
    }
    for (filename, text, map, expected_values, expected_links) in [
        ("links.csv", "name,location\nNorth,Melbourne\nSouth,Sydney\n".to_string(),json!({"location":"near"}),0,2),
        ("confidential-import-canary.csv", format!("name,note\n{}", (0..501).map(|i| format!("Shop {i},confidential-import-canary\n")).collect::<String>()),json!({"note":"note"}),501,0),
    ] {
        let doc = app.tool("document_upload", json!({"filename":filename,"text":text})).await;
        let out = app.tool("document_import_rows", json!({"document_id":doc["id"],"kind":"shop","name_column":"name","column_map":map})).await;
        assert!(out.get("error").is_none(), "{out}");
        let db = app.db().await;
        let events: Vec<Value> = db.test_raw().query("SELECT * FROM ontology_event WHERE action = 'imported' AND target = $source")
            .bind(("source", doc["id"].as_str().unwrap().to_string())).await.unwrap().take(0).unwrap();
        assert_eq!(events.len(),1, "{events:?}");
        assert_eq!(events[0]["after"]["values"],expected_values);
        assert_eq!(events[0]["after"]["links"],expected_links);
        assert_eq!(events[0]["after"]["entities"],out["created"].as_u64().unwrap()+out["updated"].as_u64().unwrap()+out["targets_created"].as_u64().unwrap());
        assert!(!serde_json::to_string(&events).unwrap().contains("confidential-import-canary"));
    }
}


/// Includes timestamps, provenance, revisions and the durable cursor, so an UPDATE that
/// happens to leave the definition unchanged still fails the retry check.
async fn proposal_storage(app: &TestApp) -> Vec<Vec<Value>> {
    let db = app.db().await;
    let mut res = db.test_raw().query("SELECT * FROM ontology_kind ORDER BY id; SELECT * FROM ontology_link ORDER BY id; SELECT * FROM ontology_property ORDER BY id; SELECT * FROM ontology_metric ORDER BY id; SELECT * FROM ontology_event ORDER BY seq; SELECT id, ontology_seq FROM vault ORDER BY id;")
        .await.unwrap();
    (0..6).map(|slot| res.take(slot).unwrap()).collect()
}

#[tokio::test]
async fn identical_proposals_are_read_only_and_changed_definitions_still_need_a_revision() {
    let app = TestApp::new().await;
    let vault = setup(&app).await;
    let source = json!([{"source":"document","record_type":"shops.csv","path":"category"}]);
    let items = [
        ("kinds", json!({"kind":"shop","description":"A shop","synonyms":["store"],"merge_into":"organisation","reason":"Source records"}),
            json!({"description":"Changed","synonyms":["outlet"],"merge_into":"location","reason":"New reason"})),
        ("links", json!({"from_kind":"shop","label":"near","to_kind":"location","description":"Nearby","source_map":source,"reason":"Source records"}),
            json!({"description":"Changed","source_map":[{"source":"document","record_type":"shops.csv","path":"address"}],"reason":"New reason"})),
        ("properties", json!({"kind":"shop","name":"category","datatype":"enum","enum_values":["food","clothes"],"unit":"category","description":"Category","synonyms":["type"],"merge_into":"group","source_map":source,"reason":"Source records"}),
            json!({"datatype":"text","enum_values":["food","books"],"unit":"group","description":"Changed","synonyms":["class"],"merge_into":"department","source_map":[{"source":"document","record_type":"shops.csv","path":"type"}],"reason":"New reason"})),
        ("metrics", json!({"kind":"shop","name":"total","expr":{"agg":"count"},"unit":"shops","description":"Shop count","synonyms":["stores"],"merge_into":"count_all","reason":"Source records"}),
            json!({"expr":{"agg":"count","property":"category"},"unit":"stores","description":"Changed","synonyms":["outlets"],"merge_into":"count_stores","reason":"New reason"})),
    ];
    for (list, fields, changes) in items {
        let mut args = json!({"vault_id":vault});
        args[list] = json!([fields]);
        let created = app.tool("ontology_propose", args.clone()).await;
        assert!(created.get("error").is_none(), "{created}");
        assert_eq!(created[list][0]["revision"], 1);
        let before = proposal_storage(&app).await;
        for revision in [None, Some(0), Some(1), Some(999)] {
            let mut retry = fields.clone();
            if let Some(revision) = revision { retry["revision"] = json!(revision); }
            args[list] = json!([retry]);
            let out = app.tool("ontology_propose", args.clone()).await;
            assert_eq!(out, created, "{out}");
            let (status, out) = app.http("POST", "/api/tools/ontology_propose", Some(args.clone()), true).await;
            assert_eq!(status, 200, "{out}");
            assert_eq!(out, created);
            let rpc = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ontology_propose","arguments":args}});
            let (status, out) = app.http("POST", "/mcp", Some(rpc), true).await;
            assert_eq!(status, 200, "{out}");
            let result: Value = serde_json::from_str(out["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
            assert_eq!(result, created, "{out}");
            assert_eq!(proposal_storage(&app).await, before, "identical {list} retry must not write");
        }
        for (field, value) in changes.as_object().unwrap() {
            let mut changed = fields.clone();
            changed[field] = value.clone();
            if field == "datatype" { changed.as_object_mut().unwrap().remove("enum_values"); }
            for revision in [None, Some(0), Some(999)] {
                let mut stale = changed.clone();
                if let Some(revision) = revision { stale["revision"] = json!(revision); }
                args[list] = json!([stale]);
                let out = app.tool("ontology_propose", args.clone()).await;
                assert_eq!(out["code"], "ontology.stale_revision", "{list}.{field}: {out}");
            }
            assert_eq!(proposal_storage(&app).await, before, "refused {list}.{field} must not write");
        }
        let mut fresh = fields;
        fresh["revision"] = json!(1);
        fresh["description"] = json!("Reviewed change");
        args[list] = json!([fresh.clone()]);
        let updated = app.tool("ontology_propose", args.clone()).await;
        assert!(updated.get("error").is_none(), "{updated}");
        assert_eq!(updated[list][0]["revision"], 2);
        fresh.as_object_mut().unwrap().remove("revision");
        args[list] = json!([fresh]);
        let before = proposal_storage(&app).await;
        assert_eq!(app.tool("ontology_propose", args).await, updated);
        assert_eq!(proposal_storage(&app).await, before);
    }
}


#[tokio::test]
async fn identical_retries_of_decided_definitions_preserve_the_decision_and_revision() {
    let app = TestApp::new().await;
    let vault = setup(&app).await;
    let args = json!({"vault_id":vault,
        "kinds":[{"kind":"shop"}],
        "links":[{"from_kind":"shop","label":"near","to_kind":"location"}],
        "properties":[{"kind":"shop","name":"note","datatype":"text"}],
        "metrics":[{"kind":"shop","name":"total","expr":{"agg":"count"}}]});
    let out = app.tool("ontology_propose", args.clone()).await;
    assert!(out.get("error").is_none(), "{out}");
    for list in ["kinds", "links", "properties", "metrics"] {
        let mut fields = args[list][0].clone();
        fields["revision"] = json!(1);
        fields["decision"] = json!({"action":"approve"});
        let (status, out) = app.http_session("POST", &format!("/api/vaults/{vault}/ontology/{list}/decide"), Some(fields)).await;
        assert_eq!(status, 200, "{out}");
    }
    let before = proposal_storage(&app).await;
    let out = app.tool("ontology_propose", args).await;
    assert!(out.get("error").is_none(), "{out}");
    for list in ["kinds", "links", "properties", "metrics"] {
        assert_eq!(out[list][0]["status"], "approved");
        assert_eq!(out[list][0]["revision"], 2);
    }
    assert_eq!(proposal_storage(&app).await, before);
}
