mod common;
use common::TestApp;
use serde_json::{json, Value};
fn ok(v: Value) -> Value { assert!(v.get("error").is_none(),"{v}"); v }
fn spec() -> Value { json!({"root":"a","elements":{"a":{"type":"Table","props":{"title":"Rows","summary":"Approved values","data":{"$state":"/sources/sample"}}}}}) }
async fn sample() -> (TestApp, String) {
    let app=TestApp::new().await;
    let vault=ok(app.tool("ontology_get",json!({})).await)["vault"].as_str().unwrap().to_string();
    ok(app.tool("ontology_propose",json!({"kinds":[{"kind":"sample"}],"properties":[{"kind":"sample","name":"amount","datatype":"money","unit":"AUD"},{"kind":"sample","name":"share","datatype":"percent","unit":"%"},{"kind":"sample","name":"label","datatype":"text"}]})).await);
    for (path,body) in [("kinds",json!({"kind":"sample","decision":{"action":"approve"}})),("properties",json!({"kind":"sample","name":"amount","decision":{"action":"approve"}})),("properties",json!({"kind":"sample","name":"share","decision":{"action":"approve"}})),("properties",json!({"kind":"sample","name":"label","decision":{"action":"approve"}}))] {
        let (status,body)=app.http_session("POST",&format!("/api/vaults/{vault}/ontology/{path}/decide"),Some(body)).await;
        assert_eq!(status,200,"{body}");
    }
    for (name,amount) in [("One",10.25),("Two",20.75)] {
        let entity=ok(app.tool("memory_write",json!({"subject_name":name,"subject_kind":"sample","text":"A sample"})).await)["entity"]["id"].clone();
        ok(app.tool("entity_set_property",json!({"entity_id":entity,"property":"amount","value":amount})).await);
        ok(app.tool("entity_set_property",json!({"entity_id":entity,"property":"share","value":35.7})).await);
        ok(app.tool("entity_set_property",json!({"entity_id":entity,"property":"label","value":if name == "One" { "Alpha" } else { "alpha" }})).await);
    }
    (app, vault)
}
#[tokio::test]
async fn dataset_resolves_real_typed_values_and_mcp_catalog() {
    let (app, vault) = sample().await;
    let args=json!({"view":"ui","spec":spec(),"sources":{"sample":{"type":"dataset","vault_id":vault,"kind":"sample","properties":["amount","share"],"filters":[{"property":"amount","op":"gt","value":10}]}}});
    let out=ok(app.tool("show_view",args.clone()).await);
    assert_eq!(out["data"]["sources"]["sample"]["totals"]["amount"]["sum"],31.0);
    assert_eq!(out["data"]["sources"]["sample"]["rows"].as_array().unwrap().len(),2);
    assert!(out["text"].as_str().unwrap().contains("35.7"));
    let (_,tools)=app.http("POST","/mcp",Some(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})),true).await;
    let tool=tools["result"]["tools"].as_array().unwrap().iter().find(|t|t["name"]=="show_view").unwrap();
    assert!(tool["inputSchema"]["properties"]["view"]["enum"].as_array().unwrap().contains(&json!("ui")));
    assert!(tool["description"].as_str().unwrap().contains("Histogram"));
    let mut bad=args.clone();bad["sources"]["sample"]["vault_id"]=json!("vault:missing");
    assert!(app.tool("show_view",bad).await.get("error").is_some());
    let mut bad=args.clone();bad["spec"]["elements"]["a"]["props"]["data"]=json!([{"amount":999}]);
    assert!(app.tool("show_view",bad).await.get("error").is_some());
    ok(app.tool("ontology_propose",json!({"metrics":[{"kind":"sample","name":"total_amount","expr":{"agg":"sum","property":"amount"},"unit":"AUD"}]})).await);
    let preview=ok(app.tool("show_view",json!({"view":"ui","spec":{"root":"a","elements":{"a":{"type":"Stat","props":{"title":"Proposed total","summary":"Not approved","data":{"$state":"/sources/m/metric/value"}}}}},"sources":{"m":{"type":"metric","metric":"total_amount","kind":"sample","vault_id":vault,"allow_unapproved":true}}})).await);
    assert_eq!(preview["data"]["sources"]["m"]["status"],"not approved");
    assert!(preview["text"].as_str().unwrap().contains("not approved"));
    let supplier=ok(app.tool("memory_write",json!({"subject_name":"Supplier","subject_kind":"organisation","text":"A supplier"})).await)["entity"]["id"].clone();
    ok(app.tool("ontology_propose",json!({"links":[{"from_kind":"sample","label":"supplied_by","to_kind":"organisation"}]})).await);
    let (status,body)=app.http_session("POST",&format!("/api/vaults/{vault}/ontology/links/decide"),Some(json!({"from_kind":"sample","label":"supplied_by","to_kind":"organisation","decision":{"action":"approve"}}))).await;
    assert_eq!(status,200,"{body}");
    for row in out["data"]["sources"]["sample"]["rows"].as_array().unwrap() {
        ok(app.tool("entity_relate",json!({"from_id":row["entity_id"],"label":"supplied_by","to_id":supplier})).await);
    }
    let mut grouped=args.clone();
    grouped["sources"]["sample"]["group_by"]=json!("link:supplied_by");
    grouped["sources"]["sample"]["aggregates"]=json!([{"op":"count"}]);
    let grouped=ok(app.tool("show_view",grouped).await);
    assert_eq!(grouped["data"]["sources"]["sample"]["rows"][0]["count"],2);
    assert_eq!(grouped["data"]["sources"]["sample"]["rows"][0]["link:supplied_by"]["name"],"Supplier");
    // Same entities and the same cut-off through both public paths, including empty sets.
    for empty in [false, true] {
        for (property, ops) in [("amount", vec!["count","sum","avg","min","max","median","count_distinct"]),("label",vec!["count","count_distinct"])] {
            for op in ops {
                let metric = format!("check_{op}_{property}");
                ok(app.tool("ontology_propose",json!({"metrics":[{"kind":"sample","name":metric,"expr":{"agg":op,"property":property}}]})).await);
                let filters = if empty { json!([{"property":"amount","op":"gt","value":1000}]) } else { json!([]) };
                let evaluated = ok(app.tool("metric_eval",json!({"vault_id":vault,"kind":"sample","metric":metric,"allow_unapproved":true,"filters":filters,"as_of":"2099-01-01"})).await);
                let mut query = args.clone();
                query["sources"]["sample"]["properties"] = json!([property]);
                query["sources"]["sample"]["filters"] = filters;
                query["sources"]["sample"]["as_of"] = json!("2099-01-01");
                query["sources"]["sample"]["aggregates"] = json!([{"op":op,"property":property}]);
                let dataset = ok(app.tool("show_view",query).await);
                assert_eq!(dataset["data"]["sources"]["sample"]["rows"][0][format!("{}_{}", if op == "count_distinct" { "distinct" } else { op }, property)].as_f64(), evaluated["value"].as_f64(),"{op} {property}, empty={empty}");
            }
        }
    }
    let mut text_sum=args.clone(); text_sum["sources"]["sample"]["properties"]=json!(["label"]); text_sum["sources"]["sample"]["aggregates"]=json!([{"op":"sum","property":"label"}]);
    assert!(app.tool("show_view",text_sum).await.to_string().contains("numeric aggregate needs numeric property"));
    let mut bad=args;bad["sources"]["sample"]["limit"]=json!(501);
    assert!(app.tool("show_view",bad).await.get("error").is_some());
}

#[tokio::test]
async fn dataset_denies_same_org_nonmember() {
    let (app, vault) = sample().await;
    let args=json!({"view":"ui","spec":spec(),"sources":{"sample":{"type":"dataset","vault_id":vault,"kind":"sample","properties":["amount","share"]}}});
    let other=common::register(&app.state,"outsider@example.com").await;
    assert_eq!(other.org, app.user.org);
    let denied=common::sys(eunomia_backend::tools::registry::call(&app.state,&other,"show_view",args.clone())).await.unwrap();
    assert!(denied.get("error").is_some(),"same-org nonmember cannot read the dataset: {denied}");
}

#[tokio::test]
async fn dataset_denies_vault_restricted_token() {
    let (app, vault) = sample().await;
    let args=json!({"view":"ui","spec":spec(),"sources":{"sample":{"type":"dataset","vault_id":vault,"kind":"sample","properties":["amount","share"]}}});
    let team = ok(app.tool("vault_create",json!({"name":"Team","kind":"org"})).await)["id"].as_str().unwrap().to_string();
    let only = eunomia_backend::rid::parse(&team).unwrap();
    let token = eunomia_backend::models_user::create_api_token_with(&app.state.control,&app.user.id,"limited", &["memory:read".into()],Some(&only),None).await.unwrap().token;
    let ((status,denied),_) = common::http(&app.router,"POST","/api/tools/show_view",Some(args.clone()),Some(&token),None).await;
    assert!(status == 403 || denied.get("error").is_some(),"{status}: {denied}");
}

#[tokio::test]
async fn dataset_hides_cross_vault_links() {
    let (app, vault) = sample().await;
    let args=json!({"view":"ui","spec":spec(),"sources":{"sample":{"type":"dataset","vault_id":vault,"kind":"sample","properties":["amount","share"]}}});
    let team = ok(app.tool("vault_create",json!({"name":"Team","kind":"org"})).await)["id"].as_str().unwrap().to_string();
    let out=ok(app.tool("show_view",args.clone()).await);
    ok(app.tool("ontology_propose",json!({"links":[{"from_kind":"sample","label":"supplied_by","to_kind":"organisation"}]})).await);
    let (status, body)=app.http_session("POST", &format!("/api/vaults/{vault}/ontology/links/decide"),Some(json!({"from_kind":"sample","label":"supplied_by","to_kind":"organisation","decision":{"action":"approve"}}))).await;
    assert_eq!(status,200,"{body}");
    let foreign = ok(app.tool("memory_write",json!({"subject_name":"Foreign supplier","subject_kind":"organisation","text":"Elsewhere","vault_id":team})).await)["entity"]["id"].clone();
    // A legacy cross-vault edge must not supply grouped data, even if both vaults are readable.
    let db = app.db().await;
    let entity = eunomia_backend::rid::parse(out["data"]["sources"]["sample"]["rows"][0]["entity_id"].as_str().unwrap()).unwrap();
    let foreign_id = eunomia_backend::rid::parse(foreign.as_str().unwrap()).unwrap();
    db.test_raw().query("RELATE $from->relates_to->$to SET label = 'supplied_by'").bind(("from",entity)).bind(("to",foreign_id)).await.unwrap().check().unwrap();
    let mut cross = args.clone(); cross["sources"]["sample"]["properties"] = json!(["link:supplied_by"]); cross["sources"]["sample"].as_object_mut().unwrap().remove("filters");
    let cross = ok(app.tool("show_view",cross).await);
    assert!(!cross.to_string().contains("Foreign supplier"));
}

#[tokio::test]
async fn dataset_strict_kind_guard() {
    let (app, vault) = sample().await;
    let args=json!({"view":"ui","spec":spec(),"sources":{"sample":{"type":"dataset","vault_id":vault,"kind":"sample","properties":["amount","share"]}}});
    ok(app.tool("ontology_propose",json!({"kinds":[{"kind":"unreviewed"}],"properties":[{"kind":"unreviewed","name":"amount","datatype":"money","unit":"AUD"},{"kind":"unreviewed","name":"share","datatype":"percent","unit":"%"}]})).await);
    for name in ["amount", "share"] {
        let (status,body)=app.http_session("POST", &format!("/api/vaults/{vault}/ontology/properties/decide"),Some(json!({"kind":"unreviewed","name":name,"decision":{"action":"approve"}}))).await;
        assert_eq!(status,200,"{body}");
    }
    let mut unreviewed = args.clone(); unreviewed["sources"]["sample"]["kind"] = json!("unreviewed");
    ok(app.tool("show_view",unreviewed).await);
    let (status,body)=app.http_session("POST",&format!("/api/vaults/{vault}/ontology/strict"),Some(json!({"on":true}))).await;
    assert_eq!(status,200,"{body}");
    let mut unreviewed=args.clone();unreviewed["sources"]["sample"]["kind"]=json!("unreviewed");
    let denied = app.tool("show_view",unreviewed).await;
    assert!(denied.to_string().contains("strict mode requires an approved dataset kind"), "{denied}");
}

#[tokio::test]
async fn dataset_cutoff_currency_and_mixed_units() {
    let (app, vault) = sample().await;
    let args=json!({"view":"ui","spec":spec(),"sources":{"sample":{"type":"dataset","vault_id":vault,"kind":"sample","properties":["amount"],"as_of":"2026-01-01"}}});
    let current=ok(app.tool("show_view",json!({"view":"ui","spec":spec(),"sources":{"sample":{"type":"dataset","vault_id":vault,"kind":"sample","properties":["amount"]}}})).await);
    let id=current["data"]["sources"]["sample"]["rows"][0]["entity_id"].clone();
    for (as_of,value,unit) in [("2026-01-01T12:00:00+11:00",12,"USD"),("2026-01-02T00:00:00+11:00",13,"USD"),("2026-01-02T12:00:00+11:00",99,"AUD")] {
        ok(app.tool("entity_set_property",json!({"entity_id":id,"property":"amount","value":value,"unit":unit,"as_of":as_of})).await);
    }
    let out=ok(app.tool("show_view",args.clone()).await);
    // A bare date ends in UTC, so Melbourne midnight on the following day is included.
    assert_eq!(out["data"]["sources"]["sample"]["totals"]["amount"]["sum"],13.0);
    ok(app.tool("ontology_propose",json!({"metrics":[{"kind":"sample","name":"cutoff_count","expr":{"agg":"count","property":"amount"}}]})).await);
    let metric=ok(app.tool("metric_eval",json!({"vault_id":vault,"kind":"sample","metric":"cutoff_count","allow_unapproved":true,"as_of":"2026-01-01"})).await);
    assert_eq!(out["data"]["sources"]["sample"]["totals"]["amount"]["count"].as_f64(),metric["value"].as_f64());
    assert_eq!(out["data"]["sources"]["sample"]["columns"][0]["unit"],"USD");
    let mut grouped=args.clone(); grouped["sources"]["sample"]["aggregates"]=json!([{"op":"sum","property":"amount"}]);
    let out=ok(app.tool("show_view",grouped).await);
    assert_eq!(out["data"]["sources"]["sample"]["columns"][0]["unit"],"USD");
    let other=current["data"]["sources"]["sample"]["rows"][1]["entity_id"].clone();
    ok(app.tool("entity_set_property",json!({"entity_id":other,"property":"amount","value":3,"unit":"AUD","as_of":"2026-01-01T12:00:00+11:00"})).await);
    let denied=app.tool("show_view",args).await;
    assert!(denied.to_string().contains("incompatible units"),"{denied}");
}

/// Run explicitly. Uses a disposable, migrated in-memory SurrealDB and the real dataset path.
#[tokio::test]
#[ignore = "scale proof: 5000 entities and 50000 property history rows"]
async fn dataset_scale_grouped_and_bucketed() {
    let (app, vault) = sample().await;
    ok(app.tool("ontology_propose",json!({"properties":[{"kind":"sample","name":"date","datatype":"date"}]})).await);
    let (status,body)=app.http_session("POST",&format!("/api/vaults/{vault}/ontology/properties/decide"),Some(json!({"kind":"sample","name":"date","decision":{"action":"approve"}}))).await;
    assert_eq!(status,200,"{body}");
    let db=app.db().await;
    let vault_id=eunomia_backend::rid::parse(&vault).unwrap();
    let amount: Vec<surrealdb::types::RecordId>=db.test_raw().query("SELECT VALUE id FROM ontology_property WHERE vault=$vault AND kind='sample' AND name='amount'").bind(("vault",vault_id.clone())).await.unwrap().take(0).unwrap();
    let date: Vec<surrealdb::types::RecordId>=db.test_raw().query("SELECT VALUE id FROM ontology_property WHERE vault=$vault AND kind='sample' AND name='date'").bind(("vault",vault_id.clone())).await.unwrap().take(0).unwrap();
    let prop=|name: &str| if name == "amount" { amount[0].clone() } else { date[0].clone() };
    // Remove the two tiny-fixture entities so the timed input is exactly five thousand.
    db.test_raw().query("DELETE entity_property WHERE vault = $vault; DELETE entity WHERE vault = $vault;").bind(("vault",vault_id.clone())).await.unwrap().check().unwrap();
    for start in (0..5000).step_by(100) {
        let mut sql=String::new();
        for i in start..start+100 {
            sql.push_str(&format!("CREATE entity:scale_{i} SET owner=$owner, vault=$vault, kind='sample', name='Scale {i}';"));
            for version in 0..5 {
                for (property,value) in [("amount",format!("value_num={}",i+version)),("date",format!("value_date=d'2026-{:02}-01T00:00:00Z'",i%12+1))] {
                    sql.push_str(&format!("CREATE entity_property SET vault=$vault, entity=entity:scale_{i}, property=${property}, {value}, unit=NONE, as_of=d'2026-01-0{}T00:00:00Z', origin='human';",version+1));
                }
            }
        }
        db.test_raw().query(sql).bind(("vault",vault_id.clone())).bind(("owner",app.user.id.clone())).bind(("amount",prop("amount"))).bind(("date",prop("date"))).await.unwrap().check().unwrap();
    }
    let mut counts=db.test_raw().query("RETURN (SELECT count() AS n FROM entity WHERE vault=$vault GROUP ALL)[0].n; RETURN (SELECT count() AS n FROM entity_property WHERE vault=$vault GROUP ALL)[0].n;").bind(("vault",vault_id.clone())).await.unwrap();
    let entities: Option<i64>=counts.take(0).unwrap();
    let entities=entities.unwrap();
    let histories: Option<i64>=counts.take(1).unwrap();
    let histories=histories.unwrap();
    assert_eq!((entities,histories),(5000,50000));
    println!("SCALE entities={entities} property_history_rows={histories}");
    let mut plans=db.test_raw().query("SELECT VALUE id FROM entity WHERE vault=$vault AND kind='sample' ORDER BY id LIMIT 5001 EXPLAIN; SELECT entity, property, value_num FROM entity_property WHERE entity=entity:scale_0 AND property=$amount AND vault=$vault AND as_of <= d'2026-12-31T00:00:00Z' ORDER BY as_of DESC, created_at DESC LIMIT 1 EXPLAIN;").bind(("vault",vault_id.clone())).bind(("amount",prop("amount"))).await.unwrap().check().unwrap();
    for index in 0..2 {
        let plan: surrealdb::types::Value=plans.take(index).unwrap();
        println!("PLAN {index}: {plan:?}");
        let plan = format!("{plan:?}");
        let index_name = if index == 0 { "entity_vault_kind_idx" } else { "entity_property_entity_idx" };
        assert!(plan.contains("IndexScan") && plan.contains(index_name) && !plan.contains("TableScan"),"{plan}");
    }
    for bucket in [false,true] {
        let source=if bucket { json!({"type":"dataset","vault_id":vault,"kind":"sample","properties":["amount","date"],"group_by":"date","bucket":"month","aggregates":[{"op":"sum","property":"amount"}],"as_of":"2026-12-31"}) } else { json!({"type":"dataset","vault_id":vault,"kind":"sample","properties":["amount"],"group_by":"amount","aggregates":[{"op":"count"}],"as_of":"2026-12-31"}) };
        let started=std::time::Instant::now();
        let out=ok(app.tool("show_view",json!({"view":"ui","spec":spec(),"sources":{"sample":source}})).await);
        println!("SCALE bucket={bucket} elapsed_ms={} groups={} truncated={}",started.elapsed().as_millis(),out["data"]["sources"]["sample"]["total_rows"],out["data"]["sources"]["sample"]["truncated"]);
        assert_eq!(out["data"]["sources"]["sample"]["total_rows"],if bucket { 12 } else { 5000 });
        let total_key=if bucket { "sum_amount" } else { "count" };
        assert_eq!(out["data"]["sources"]["sample"]["totals"][total_key]["sum"].as_f64(),Some(if bucket { 12_517_500.0 } else { 5000.0 }));
    }
}

#[tokio::test]
async fn dataset_filter_limit() {
    let (app, vault) = sample().await;
    let filters=vec![json!({"property":"amount","op":"gt","value":0});11];
    let denied=app.tool("show_view",json!({"view":"ui","spec":spec(),"sources":{"sample":{"type":"dataset","vault_id":vault,"kind":"sample","properties":["amount"],"filters":filters}}})).await;
    assert!(denied.to_string().contains("10 filters"),"{denied}");
}

#[tokio::test]
async fn metric_stored_unit_override_matches_main_fixture() {
    let app = TestApp::new().await;
    let fixture: Value = serde_json::from_str(include_str!("fixtures/pr192-main-metrics.json")).unwrap();
    let case = fixture.as_array().unwrap().iter().find(|c| c["stored_unit"] == "USD").unwrap();
    let vault = ok(app.tool("ontology_get", json!({})).await)["vault"].clone();
    ok(app.tool("ontology_propose", json!({"kinds":[{"kind":"sample"}],"properties":[{"kind":"sample","name":"amount","datatype":"money","unit":case["property_unit"]}]})).await);
    for (path, body) in [("kinds",json!({"kind":"sample","decision":{"action":"approve"}})),("properties",json!({"kind":"sample","name":"amount","decision":{"action":"approve"}}))] {
        let (status, body) = app.http_session("POST", &format!("/api/vaults/{}/ontology/{path}/decide",vault.as_str().unwrap()), Some(body)).await;
        assert_eq!(status,200,"{body}");
    }
    let entity = ok(app.tool("memory_write",json!({"subject_name":"Unit override","subject_kind":"sample","text":"Sample"})).await)["entity"]["id"].clone();
    ok(app.tool("entity_set_property",json!({"entity_id":entity,"property":"amount","value":case["values"][0],"unit":case["stored_unit"]})).await);
    for (agg, expected) in case["main"].as_object().unwrap() {
        ok(app.tool("ontology_propose",json!({"metrics":[{"kind":"sample","name":format!("test_{agg}"),"expr":{"agg":agg,"property":"amount"}}]})).await);
        let out = ok(app.tool("metric_eval",json!({"metric":format!("test_{agg}"),"kind":"sample","vault_id":vault,"allow_unapproved":true})).await);
        assert_eq!(out["value"].as_f64().map(f64::to_bits), expected.as_f64().map(f64::to_bits),"{agg}: {out}");
        assert_eq!(out["n_entities"],1);
    }
    let out = ok(app.tool("show_view",json!({"view":"ui","spec":spec(),"sources":{"sample":{"type":"dataset","vault_id":vault,"kind":"sample","properties":["amount"],"limit":5}}})).await);
    assert_eq!(out["data"]["sources"]["sample"]["columns"][0]["unit"],"USD");
}

#[tokio::test]
async fn show_view_resolves_chart_columns_and_applied_limit() {
    let (app,vault) = sample().await;
    let mut args = json!({"view":"ui","spec":{"root":"a","elements":{"a":{"type":"Line","props":{"title":"Top 5 suppliers","summary":"Amount","data":{"$state":"/sources/sample/rows"}}}}},"sources":{"sample":{"type":"dataset","vault_id":vault,"kind":"sample","properties":["amount"],"limit":5}}});
    let out = ok(app.tool("show_view",args.clone()).await);
    assert_eq!(out["spec"]["elements"]["a"]["props"]["x"],"name");
    assert_eq!(out["spec"]["elements"]["a"]["props"]["y"],"amount");
    assert_eq!(out["spec"]["elements"]["a"]["props"]["series"],json!(["amount"]));
    args["sources"]["sample"]["properties"] = json!(["amount","share"]);
    assert!(app.tool("show_view",args).await.to_string().contains("name y"));
}
