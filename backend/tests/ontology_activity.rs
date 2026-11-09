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
    let out = app.tool("ontology_propose", json!({"vault_id":vault,"kinds":[{"kind":"shop","description":description,"reason":"From stores.csv"}]})).await;
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
    assert_eq!(events[2]["before"]["status"], "proposed");
    assert_eq!(events[2]["after"]["status"], "approved");
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
async fn typed_values_and_import_batches_record_only_committed_writes() {
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
    assert_eq!(written["after"]["value_num"], 12.5);
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
    let out = app.tool("ontology_propose", json!({"vault_id":vault,"metrics":[{"kind":"shop","name":"total","description":"Changed while open","expr":{"agg":"sum","property":"price"}}]})).await;
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
    db.test_raw().query("DEFINE FIELD OVERWRITE reason ON ontology_event TYPE option<string> ASSERT $value != 'force event failure'")
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
