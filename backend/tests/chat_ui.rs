mod common;
use common::TestApp;
use serde_json::{json, Value};
fn ok(v: Value) -> Value { assert!(v.get("error").is_none(),"{v}"); v }
fn spec() -> Value { json!({"root":"a","elements":{"a":{"type":"Table","props":{"title":"Rows","summary":"Approved values","data":{"$state":"/sources/sample"}}}}}) }
async fn sample() -> (TestApp, String) {
    sample_with(TestApp::new().await).await
}
async fn sample_with(app: TestApp) -> (TestApp, String) {
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
async fn show_view_resolves_chart_columns_and_bound_prose() {
    let (app,vault) = sample().await;
    let mut args = json!({"view":"ui","spec":{"root":"a","elements":{"a":{"type":"Line","props":{"title":"Total {{/sources/sample/totals/amount/sum}}","summary":"Amount","data":{"$state":"/sources/sample/rows"}}}}},"sources":{"sample":{"type":"dataset","vault_id":vault,"kind":"sample","properties":["amount"],"limit":5}}});
    let out = ok(app.tool("show_view",args.clone()).await);
    assert_eq!(out["spec"]["elements"]["a"]["props"]["title"],"Total 31.00 AUD");
    assert!(out["text"].as_str().unwrap().contains("Total 31.00 AUD"));
    assert_eq!(out["spec"]["elements"]["a"]["props"]["x"],"name");
    assert_eq!(out["spec"]["elements"]["a"]["props"]["y"],"amount");
    assert_eq!(out["spec"]["elements"]["a"]["props"]["series"],json!(["amount"]));
    args["sources"]["sample"]["properties"] = json!(["amount","share"]);
    assert!(app.tool("show_view",args).await.to_string().contains("name y"));
}

#[tokio::test]
async fn prose_substitution_reaches_spec_and_terminal_summary() {
    let (app, vault) = sample().await;
    let mut args = json!({"view":"ui","spec":{"root":"a","elements":{"a":{"type":"Text","props":{"text":"Total {{sources.sales.totals.amount.sum}}"}}}},"sources":{"sales":{"type":"dataset","vault_id":vault,"kind":"sample","properties":["amount","share"]}}});
    let out = ok(app.tool("show_view", args.clone()).await);
    assert_eq!(out["spec"]["elements"]["a"]["props"]["text"], "Total 31.00 AUD");
    assert!(out["text"].as_str().unwrap().contains("Total 31.00 AUD"));
    assert!(!out["text"].as_str().unwrap().contains("{{"));
    for raw in ["Sales rose 35.7%", "Value {{sources.sales.rows[0].missing}}", "Value {{sources.sales.rows[0].name}}"] {
        args["spec"]["elements"]["a"]["props"]["text"] = json!(raw);
        let error = app.tool("show_view", args.clone()).await;
        assert!(error.get("error").is_some(), "{raw}: {error}");
        assert!(error.to_string().contains("placeholder"), "{error}");
    }
}

#[tokio::test]
async fn invented_query_kind_is_not_a_server_text_token() {
    let (app, vault) = sample().await;
    let args = json!({"view":"ui","spec":{"root":"a","elements":{"a":{"type":"Text","props":{"text":"x12000"}}}},"sources":{"sales":{"type":"dataset","vault_id":vault,"kind":"x12000","properties":[]}}});
    let error = app.tool("show_view", args).await;
    assert!(error.get("error").is_some(), "{error}");
    assert!(error.to_string().contains("unsupported raw digits"), "{error}");
}

#[tokio::test]
async fn turn_snapshot_refuses_new_names_and_text_values() {
    use eunomia_backend::views::TURN_STARTED;
    use surrealdb::types::Datetime;
    let (app, vault) = sample().await;
    let before = Datetime::default();
    let id = ok(app.tool("memory_write",json!({"subject_name":"Truck 7","subject_kind":"sample","text":"New this turn"})).await)["entity"]["id"].clone();
    ok(app.tool("entity_set_property",json!({"entity_id":id,"property":"label","value":"ISO 27001"})).await);
    let make = |title: &str| json!({"view":"ui","spec":{"root":"a","elements":{"a":{"type":"Text","props":{"text":title}}}},"sources":{"sample":{"type":"dataset","vault_id":vault,"kind":"sample","properties":["label"]}}});
    for phrase in ["Truck 7 deliveries", "ISO 27001 compliance"] {
        let args = make(phrase);
        let denied = TURN_STARTED.scope(before, app.tool("show_view",args.clone())).await;
        assert!(denied.get("error").is_some(), "new string authorised digits: {denied}");
        ok(app.tool("show_view",args).await); // The same phrase is eligible next request.
    }
    for name in ["12%", "12"] {
        ok(app.tool("memory_write",json!({"subject_name":name,"subject_kind":"organisation","text":"Bare numeric name"})).await);
        let mut args = make(&format!("Revenue rose {name}"));
        args["sources"]["sample"]["kind"] = json!("organisation");
        args["sources"]["sample"]["properties"] = json!([]);
        let denied = app.tool("show_view",args).await;
        assert!(denied.get("error").is_some(), "bare digits authorised: {denied}");
    }
}

#[tokio::test]
async fn whole_spans_guard_ui_and_terminal_prose() {
    let (app, vault) = sample().await;
    for name in ["Orders 12", "345 deliveries"] {
        ok(app.tool("memory_write", json!({"subject_name":name,"subject_kind":"sample","text":"Existing name"})).await);
    }
    let make = |text: &str| json!({"view":"ui","spec":{"root":"a","elements":{"a":{"type":"Text","props":{"text":text}}}},"sources":{"sales":{"type":"dataset","vault_id":vault,"kind":"sample","properties":["amount","share"]}}});
    for raw in ["Orders 12,345 deliveries", "Orders 12 345 deliveries", "−{{/sources/sales/total_rows}}", "{{/sources/sales/total_rows}}\u{034f}%", "{{/sources/sales/total_rows}}.5"] {
        let denied = app.tool("show_view", make(raw)).await;
        assert!(denied.get("error").is_some(), "{raw}: {denied}");
    }
    for raw in ["Count {{/sources/sales/total_rows}}.", "Count {{/sources/sales/total_rows}}, then", "Total {{/sources/sales/totals/amount/sum}}", "Share {{/sources/sales/totals/share/sum}}"] {
        let out = ok(app.tool("show_view", make(raw)).await);
        let rendered = out["spec"]["elements"]["a"]["props"]["text"].as_str().unwrap();
        assert!(out["text"].as_str().unwrap().contains(rendered), "{out}");
        assert!(!rendered.contains("{{"));
    }
}

#[tokio::test]
async fn display_normalisation_modifiers_and_source_aliases_are_guarded() {
    let (app, vault) = sample().await;
    let make = |text: &str| json!({"view":"ui","spec":{"root":"copy","elements":{"copy":{"type":"Text","props":{"text":text}}}},"sources":{"sales":{"type":"dataset","vault_id":vault,"kind":"sample","properties":["amount","share"]}}});
    let raw = "Ｃｏｕｎｔ \u{202e}{{/sources/sales/total_rows}}\u{202c}";
    let out = ok(app.tool("show_view", make(raw)).await);
    assert_eq!(out["spec"]["elements"]["copy"]["props"]["text"], "Count 2");
    assert!(out["text"].as_str().unwrap().contains("• Count 2\n"));
    assert!(!out["text"].as_str().unwrap().contains(['\u{202e}', '\u{202c}']));
    for text in ["− {{/sources/sales/total_rows}}", "{{/sources/sales/total_rows}} %", "{{/sources/sales/total_rows}}\u{00a0}€", "$  {{/sources/sales/total_rows}}"] {
        let denied = app.tool("show_view", make(text)).await;
        assert!(denied.get("error").is_some(), "{text}: {denied}");
    }
    for alias in ["999999", "12sales", "sales_999999"] {
        let mut args = make("Sales");
        let source = args["sources"].as_object_mut().unwrap().remove("sales").unwrap();
        args["sources"][alias] = source;
        let denied = app.tool("show_view", args).await;
        assert!(denied.get("error").is_some(), "{alias}: {denied}");
        assert!(denied.to_string().contains("identifier"), "{denied}");
    }
    let mut args = make("Sales");
    let source = args["sources"].as_object_mut().unwrap().remove("sales").unwrap();
    args["sources"]["sales_q3"] = source;
    let out = ok(app.tool("show_view", args).await);
    assert!(out["text"].as_str().unwrap().contains("Source 1:"));
    assert!(!out["text"].as_str().unwrap().contains("sales_q3"));
}

#[tokio::test]
async fn selectors_resolve_approved_digit_properties_and_reject_model_aliases() {
    let (app, vault) = sample().await;
    for (name, datatype) in [("cost_2026", "number"), ("amount_2026", "number"), ("region_2026", "text")] {
        ok(app.tool("ontology_propose", json!({"properties":[{"kind":"sample","name":name,"datatype":datatype}]})).await);
        let (status, body) = app.http_session("POST", &format!("/api/vaults/{vault}/ontology/properties/decide"), Some(json!({"kind":"sample","name":name,"decision":{"action":"approve"}}))).await;
        assert_eq!(status, 200, "{body}");
    }
    let rows = ok(app.tool("show_view", json!({"view":"ui","spec":spec(),"sources":{"sample":{"type":"dataset","vault_id":vault,"kind":"sample","properties":["amount"]}}})).await);
    for row in rows["data"]["sources"]["sample"]["rows"].as_array().unwrap() {
        for (property, value) in [("cost_2026", json!(12)), ("amount_2026", json!(24)), ("region_2026", json!("North"))] {
            ok(app.tool("entity_set_property", json!({"entity_id":row["entity_id"],"property":property,"value":value})).await);
        }
    }
    let make = |kind: &str, extra: Value, source: Value| {
        let mut props = json!({"title":"Values","summary":"Values","data":{"$state":"/sources/sales/rows"}});
        props.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        json!({"view":"ui","spec":{"root":"chart","elements":{"chart":{"type":kind,"props":props}}},"sources":{"sales":source}})
    };
    let source = json!({"type":"dataset","vault_id":vault,"kind":"sample","properties":["cost_2026"]});
    for extra in [json!({"y":"cost_2026"}), json!({})] {
        let out = ok(app.tool("show_view", make("Bar", extra, source.clone())).await);
        assert_eq!(out["spec"]["elements"]["chart"]["props"]["y"], "cost_2026");
        assert_eq!(out["spec"]["elements"]["chart"]["props"]["series"], json!(["cost_2026"]));
        assert_eq!(out["data"]["sources"]["sales"]["totals"]["name"]["count"], 2.0);
    }
    let table_source = json!({"type":"dataset","vault_id":vault,"kind":"sample","properties":["amount_2026"],"sort":{"property":"amount_2026"}});
    let table = ok(app.tool("show_view", make("Table", json!({"columns":[{"key":"amount_2026","label":"Model label"}]}), table_source)).await);
    assert_eq!(table["spec"]["elements"]["chart"]["props"]["columns"][0]["label"], "amount 2026");
    let grouped = json!({"type":"dataset","vault_id":vault,"kind":"sample","properties":["cost_2026"],"group_by":"region_2026","aggregates":[{"op":"sum","property":"cost_2026"}],"sort":{"property":"sum_cost_2026"}});
    let out = ok(app.tool("show_view", make("Bar", json!({"x":"region_2026","group":"region_2026"}), grouped)).await);
    assert_eq!(out["spec"]["elements"]["chart"]["props"]["x"], "region_2026");
    assert_eq!(out["spec"]["elements"]["chart"]["props"]["y"], "sum_cost_2026");
    assert_eq!(out["data"]["sources"]["sales"]["rows"][0]["sum_cost_2026"], 24.0);
    let denied = app.tool("show_view", make("Bar", json!({"y":"unknown_2026"}), source.clone())).await;
    assert!(denied.to_string().contains("unknown bound column unknown_2026"), "{denied}");
    let mut unknown_sort = source.clone();
    unknown_sort["sort"] = json!({"property":"unknown_2026"});
    let denied = app.tool("show_view", make("Bar", json!({}), unknown_sort)).await;
    assert!(denied.to_string().contains("unknown sort column"), "{denied}");
    let absent_group_column = json!({"type":"dataset","vault_id":vault,"kind":"sample","properties":["cost_2026"],"aggregates":[{"op":"sum","property":"cost_2026"}],"sort":{"property":"name"}});
    let denied = app.tool("show_view", make("Table", json!({}), absent_group_column)).await;
    assert!(denied.to_string().contains("unknown sort column"), "{denied}");
    let mut args = make("Bar", json!({"y":"cost_2026"}), source);
    let source = args["sources"].as_object_mut().unwrap().remove("sales").unwrap();
    args["sources"]["revenue_999"] = source;
    args["spec"]["elements"]["chart"]["props"]["data"]["$state"] = json!("/sources/revenue_999/rows");
    let denied = app.tool("show_view", args).await;
    assert!(denied.to_string().contains("identifier"), "{denied}");
}

#[tokio::test]
async fn grouped_metrics_rank_properties_and_links_and_batch_views() {
    let (app, vault) = sample().await;
    for (name, label, amount) in [("Three", "Alpha", 30.0), ("Four", "Beta", 12.0)] {
        let entity = ok(app.tool("memory_write",json!({"subject_name":name,"subject_kind":"sample","text":"A sample"})).await)["entity"]["id"].clone();
        for (property, value) in [("amount",json!(amount)),("label",json!(label))] {
            ok(app.tool("entity_set_property",json!({"entity_id":entity,"property":property,"value":value})).await);
        }
    }
    ok(app.tool("ontology_propose",json!({"metrics":[{"kind":"sample","name":"total_amount","expr":{"agg":"sum","property":"amount"}}]})).await);
    let (s, b) = app.http_session("POST",&format!("/api/vaults/{vault}/ontology/metrics/decide"),Some(json!({"kind":"sample","name":"total_amount","decision":{"action":"approve"}}))).await;
    assert_eq!(s,200,"{b}");
    let args = json!({"metric":"total_amount","kind":"sample","vault_id":vault,"group_by":"label","sort":{"property":"value","descending":true},"top_n":2});
    let ranked = ok(app.tool("metric_eval", args.clone()).await);
    assert_eq!(ranked["unit"], "AUD");
    let groups = ranked["groups"].as_array().unwrap();
    assert_eq!(groups.len(),2);
    assert_eq!((groups[0]["group"].clone(),groups[0]["value"].clone()),(json!("Alpha (also: alpha)"),json!(61.0)));
    assert_eq!((groups[1]["group"].clone(),groups[1]["value"].clone()),(json!("Beta"),json!(12.0)));
    assert_eq!(groups[0]["entity_ids"].as_array().unwrap().len(),3);
    assert_eq!(groups[1]["entity_ids"].as_array().unwrap().len(),1);
    assert_eq!(ranked["n_entities"],4); // before top_n
    let mut ascending = args.clone(); ascending["sort"]["descending"] = json!(false); ascending["top_n"] = json!(1);
    let lowest = ok(app.tool("metric_eval",ascending).await);
    assert_eq!(lowest["groups"][0]["group"],"Beta");
    assert_eq!(lowest["groups"][0]["value"],12.0);
    // Same schema and result over MCP, including Claude Code's text result.
    let (_, rpc) = app.http("POST","/mcp",Some(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"metric_eval","arguments":args}})),true).await;
    let terminal: Value = serde_json::from_str(rpc["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(terminal["groups"],ranked["groups"]);
    let mut view = args.clone(); view["view"] = json!("metric");
    let shown = ok(app.tool("show_view",view).await);
    assert!(shown["text"].as_str().unwrap().contains("61.00 AUD"));
    let mut source = args.clone(); source["type"] = json!("metric");
    let batch = ok(app.tool("show_view",json!({"view":"ui","spec":{"root":"root","elements":{
        "root":{"type":"Stack","props":{},"children":["a","b"]},
        "a":{"type":"Table","props":{"title":"Members {{sources.ranked.metric.groups[0].n_entities}}","summary":"Ranked totals","data":{"$state":"/sources/ranked/metric/groups"}}},
        "b":{"type":"Stat","props":{"title":"All sales","summary":"All sales","data":{"$state":"/sources/total/metric/value"}}}
    }},"sources":{"ranked":source,"total":{"type":"metric","metric":"total_amount","kind":"sample","vault_id":vault}}})).await);
    assert_eq!(batch["spec"]["elements"]["a"]["props"]["title"],"Members 3");
    assert_eq!(batch["data"]["sources"]["ranked"]["metric"]["groups"],ranked["groups"]);
    assert_eq!(batch["data"]["sources"]["total"]["metric"]["value"],73.0);
    // Ratios must be recomputed over group totals, rather than averaging row ratios.
    for (i,id) in groups[0]["entity_ids"].as_array().unwrap().iter().enumerate() {
        ok(app.tool("entity_set_property",json!({"entity_id":id,"property":"share","value":if i == 0 { 10 } else { 30 }})).await);
    }
    ok(app.tool("ontology_propose",json!({"metrics":[{"kind":"sample","name":"amount_ratio","unit":"%","expr":{"agg":"ratio","numerator":{"agg":"sum","property":"amount"},"denominator":{"agg":"sum","property":"share"}}}]})).await);
    let ratio = ok(app.tool("metric_eval",json!({"metric":"amount_ratio","kind":"sample","vault_id":vault,"allow_unapproved":true,"group_by":"label","filters":[{"property":"label","op":"eq","value":"Alpha"}]})).await);
    let alpha = ratio["groups"].as_array().unwrap().iter().find(|r| r["group"] == "Alpha (also: alpha)").unwrap();
    assert!((alpha["value"].as_f64().unwrap() - 87.142857).abs() < 1e-9);
    assert_eq!(ratio["unit"],"%");
    let supplier = ok(app.tool("memory_write",json!({"subject_name":"Supplier","subject_kind":"organisation","text":"A supplier"})).await)["entity"]["id"].clone();
    ok(app.tool("ontology_propose",json!({"links":[{"from_kind":"sample","label":"supplied_by","to_kind":"organisation"}]})).await);
    let mut linked = args.clone(); linked["group_by"] = json!("link:supplied_by");
    assert!(app.tool("metric_eval",linked.clone()).await.get("error").is_some());
    let (s,b) = app.http_session("POST",&format!("/api/vaults/{vault}/ontology/links/decide"),Some(json!({"from_kind":"sample","label":"supplied_by","to_kind":"organisation","decision":{"action":"approve"}}))).await;
    assert_eq!(s,200,"{b}");
    for id in groups[0]["entity_ids"].as_array().unwrap() { ok(app.tool("entity_relate",json!({"from_id":id,"label":"supplied_by","to_id":supplier})).await); }
    let linked = ok(app.tool("metric_eval",linked).await);
    assert_eq!(linked["groups"][0]["group"]["entity_id"],supplier);
    assert_eq!(linked["groups"][0]["value"],61.0);
    assert_eq!(linked["groups"][1]["group"],Value::Null);
    assert_eq!(linked["groups"][1]["value"],12.0);
    assert_eq!(linked["groups"][0]["entity_ids"],groups[0]["entity_ids"]);
    for extra in [json!({"group_by":"unknown"}), json!({"group_by":"label","group_by_grain":true}), json!({"group_by":"label","top_n":0}), json!({"group_by":"label","sort":{"property":"unknown"}})] {
        let mut bad = args.clone(); bad.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        assert!(app.tool("metric_eval",bad).await.get("error").is_some());
    }
}

fn model_reply(delta: Value) -> String {
    format!("data: {}\n\ndata: [DONE]\n\n",json!({"choices":[{"delta":delta}]}))
}
async fn scripted_chat(mock: &eunomia_backend::sources::mock::Mock) -> (TestApp, String) {
    let settings = eunomia_backend::config::Settings { openai_base_url:mock.base.clone(), openai_chat_model:"gpt-6-luna".into(), ..common::test_settings() };
    sample_with(TestApp::with_settings(settings).await).await
}
async fn chat_events(app: &TestApp, message: &str) -> (Vec<Value>, String) {
    use eunomia_backend::{chat::service, rid};
    let db = app.db().await;
    let thread = common::sys(service::create_thread(&db,&app.user.id,None)).await.unwrap();
    let id = rid::parse(&thread.id).unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    common::sys(service::send_stream(app.state.org(&app.user.org).await.unwrap(),app.user.clone(),id.clone(),message.into(),tx)).await;
    let mut events = Vec::new();
    while let Some(e) = rx.recv().await { events.push(serde_json::to_value(e).unwrap()); }
    let history = common::sys(service::history(&db,&app.user.id,&id)).await.unwrap().unwrap();
    (events,serde_json::to_string(&history).unwrap())
}
#[tokio::test]
async fn exhausted_chat_budget_runs_a_final_answer_without_tools() {
    use eunomia_backend::sources::mock::{route,serve};
    let tool = model_reply(json!({"tool_calls":[{"index":0,"id":"next","function":{"name":"metric_eval","arguments":json!({"metric":"total_amount","kind":"sample","allow_unapproved":true}).to_string()}}]}));
    let answer = "The total is $31.00. I could not finish the store ranking.";
    let mock = serve(vec![route("POST","/chat/completions",Value::Null).raw(&tool).times(10),route("POST","/chat/completions",Value::Null).raw(&model_reply(json!({"content":answer})))]).await;
    let (app,_) = scripted_chat(&mock).await;
    ok(app.tool("ontology_propose",json!({"metrics":[{"kind":"sample","name":"total_amount","expr":{"agg":"sum","property":"amount"}}]})).await);
    let (events,history) = chat_events(&app,"Compare the departments and rank the stores").await;
    assert_eq!(events.last().unwrap()["reply"],answer,"{events:?}");
    assert!(history.contains(answer));
    let requests: Vec<_> = mock.requests().into_iter().filter(|r| r.path == "/chat/completions").collect();
    assert_eq!(requests.len(),11);
    let final_request: Value = serde_json::from_str(&requests.last().unwrap().body).unwrap();
    assert_eq!(final_request["tool_choice"],"none");
    assert!(final_request.get("tools").is_none());
    for request in requests { assert_eq!(serde_json::from_str::<Value>(&request.body).unwrap()["model"],"gpt-6-luna"); }
}
#[tokio::test]
async fn unsupported_chat_numbers_and_placeholders_never_stream_or_persist() {
    use eunomia_backend::sources::mock::{route,serve};
    for bad in ["Bakery sales were $999,777.00.","Bakery sales were $0.00.","There were {{sources.sales.total_rows}} observations.","There were {{sources.sales.total_rows observations.","Sales were ９９９７７７."] {
        let mock = serve(vec![route("POST","/chat/completions",Value::Null).raw(&model_reply(json!({"content":bad})))]).await;
        let (app,_) = scripted_chat(&mock).await;
        let (events,history) = chat_events(&app,"Show the bakery sales").await;
        assert!(!history.contains(bad),"{history}");
        for event in &events {
            if let Some(text) = event["delta"].as_str().or(event["reply"].as_str()) {
                assert!(!text.contains("999") && !text.contains("$0") && !text.contains("{{"),"{text}");
            }
        }
        assert!(events.last().unwrap()["reply"].as_str().unwrap().contains("Some numbers couldn't be checked"));
    }
}
#[tokio::test]
async fn checked_tree_is_identical_in_text_done_and_reopened_history() {
    use eunomia_backend::sources::mock::{route,serve};
    for (raw, question, expected) in [
        ("Count 12![3](/icon.svg)", "Explain 123", "Count [unverified number]"),
        ("<https://example.com/999>", "Give a link", "https://example.com/999"),
    ] {
        let mock = serve(vec![route("POST","/chat/completions",Value::Null).raw(&model_reply(json!({"content":raw})))]).await;
        let (app,_) = scripted_chat(&mock).await;
        let (events,history) = chat_events(&app,question).await;
        let text_events: Vec<_> = events.iter().filter(|e| e["type"] == "text").collect();
        assert_eq!(text_events.len(), 1, "no unchecked model deltas are forwarded");
        let done = events.last().unwrap();
        assert_eq!(done["type"], "done");
        assert!(done["reply"].as_str().unwrap().starts_with(expected));
        assert_eq!(text_events[0]["tree"], done["tree"]);
        let history: Value = serde_json::from_str(&history).unwrap();
        assert_eq!(history.as_array().unwrap().last().unwrap()["tree"], done["tree"]);
        let (status, exported) = app.http_session("GET", "/api/export", None).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        let exported = exported["chat_history"].as_array().unwrap().last().unwrap();
        assert_eq!(exported["tree"], done["tree"]);
        assert_eq!(exported["content"], done["reply"]);
    }
}

#[tokio::test]
async fn chat_resolves_valid_histogram_placeholders_and_allows_verified_numbers() {
    use eunomia_backend::sources::mock::{route,serve};
    let mock = serve(vec![]).await;
    let (app,vault) = scripted_chat(&mock).await;
    let view = json!({"view":"ui","spec":{"root":"h","elements":{"h":{"type":"Histogram","props":{"title":"Distribution","summary":"Amounts","data":{"$state":"/sources/sales/distributions/amount/bins"}}}}},"sources":{"sales":{"type":"dataset","vault_id":vault,"kind":"sample","properties":["amount"]}}});
    let tool = model_reply(json!({"tool_calls":[{"index":0,"id":"histogram","function":{"name":"show_view","arguments":view.to_string()}}]}));
    mock.set_routes(vec![route("POST","/chat/completions",Value::Null).raw(&tool).times(1),route("POST","/chat/completions",Value::Null).raw(&model_reply(json!({"content":"There were {{sources.sales.total_rows}} observations. The total was $31.00."})))]);
    let (events,history) = chat_events(&app,"Show the amount distribution").await;
    let answer = events.last().unwrap()["reply"].as_str().unwrap();
    assert_eq!(answer,"There were 2 observations. The total was $31.00.","{events:?}");
    assert!(history.contains(answer));
    assert!(!history.contains("{{sources"));
}

#[tokio::test]
async fn chat_uses_whole_numeric_spans_and_only_current_turn_evidence() {
    use eunomia_backend::sources::mock::{route,serve};
    let mock = serve(vec![]).await;
    let (app,_) = scripted_chat(&mock).await;
    for (raw, question, expected) in [
        ("The supplied amount was $615,548.00.","Explain the supplied $615548",true),
        ("The supplied amount was $615,549.00.","Explain the supplied 615548",false),
        ("The supplied count was 12.","Explain 1 and 2",false),
        ("The supplied count was 9007199254740993.","Explain 9007199254740992",false),
        ("The supplied amount was -12.","Explain 12",false),
        ("The supplied count was １２.","Explain 12",true),
        ("The count is ｛｛sources.sales.total_rows｝｝.","Explain the count",false),
    ] {
        mock.set_routes(vec![route("POST","/chat/completions",Value::Null).raw(&model_reply(json!({"content":raw})))]);
        let (events,_) = chat_events(&app,question).await;
        let answer = events.last().unwrap()["reply"].as_str().unwrap();
        assert_eq!(!answer.contains("Some numbers couldn't be checked"),expected,"{raw}: {events:?}");
    }
}

#[tokio::test]
async fn bakery_metric_result_does_not_authorise_a_made_up_zero() {
    use eunomia_backend::sources::mock::{route,serve};
    let mock = serve(vec![]).await;
    let (app,_) = scripted_chat(&mock).await;
    let rows = ok(app.tool("show_view",json!({"view":"ui","spec":spec(),"sources":{"sample":{"type":"dataset","vault_id":ok(app.tool("ontology_get",json!({})).await)["vault"],"kind":"sample","properties":["amount"]}}})).await);
    for row in rows["data"]["sources"]["sample"]["rows"].as_array().unwrap() {
        let value = if row["name"] == "One" { 615527.25 } else { 20.75 };
        ok(app.tool("entity_set_property",json!({"entity_id":row["entity_id"],"property":"amount","value":value})).await);
    }
    ok(app.tool("ontology_propose",json!({"metrics":[{"kind":"sample","name":"bakery_sales","expr":{"agg":"sum","property":"amount"}}]})).await);
    let args = json!({"metric":"bakery_sales","kind":"sample","allow_unapproved":true});
    assert_eq!(ok(app.tool("metric_eval",args.clone()).await)["value"],615548.0);
    let call = model_reply(json!({"tool_calls":[{"index":0,"id":"bakery","function":{"name":"metric_eval","arguments":args.to_string()}}]}));
    mock.set_routes(vec![route("POST","/chat/completions",Value::Null).raw(&call).times(1),route("POST","/chat/completions",Value::Null).raw(&model_reply(json!({"content":"Bakery had no records and sales were $0.00."})))]);
    let (events,history) = chat_events(&app,"Show weekly bakery sales").await;
    assert!(events.last().unwrap()["reply"].as_str().unwrap().contains("Some numbers couldn't be checked"),"{events:?}");
    assert!(!history.contains("$0.00"));
    assert!(!events.iter().any(|e| e["delta"].as_str().is_some_and(|s| s.contains("$0.00"))));
}

#[tokio::test]
async fn grouped_values_equal_filtered_values_for_every_aggregate() {
    use rand::{Rng, SeedableRng};
    let (app, vault) = sample().await;
    let mut rng = rand::rngs::StdRng::seed_from_u64(208);
    for i in 0..16 {
        let name = format!("Generated {i}");
        let id = ok(app.tool("memory_write",json!({"subject_name":name,"subject_kind":"sample","text":"Generated group parity fixture"})).await)["entity"]["id"].clone();
        let label = ["ALPHA", "alpha", "Alpha", "Beta", "BETA"][rng.gen_range(0..5)];
        for (property,value) in [("label",json!(label)),("amount",json!(rng.gen_range(-1000..1000) as f64 / 4.0)),("share",json!(rng.gen_range(1..100)))] {
            ok(app.tool("entity_set_property",json!({"entity_id":id,"property":property,"value":value})).await);
        }
    }
    for agg in ["count","sum","avg","min","max","median","count_distinct","ratio"] {
        let name = format!("parity_{agg}");
        let expr = if agg == "ratio" { json!({"agg":"ratio","numerator":{"agg":"sum","property":"amount"},"denominator":{"agg":"sum","property":"share"}}) } else { json!({"agg":agg,"property":"amount"}) };
        ok(app.tool("ontology_propose",json!({"metrics":[{"kind":"sample","name":name,"expr":expr}]})).await);
        let args = json!({"metric":name,"kind":"sample","vault_id":vault,"allow_unapproved":true,"group_by":"label"});
        let grouped = ok(app.tool("metric_eval",args.clone()).await);
        assert_eq!(grouped["groups"].as_array().unwrap().len(),2,"{agg}: {grouped}");
        for group in grouped["groups"].as_array().unwrap() {
            let mut filtered = args.clone(); filtered.as_object_mut().unwrap().remove("group_by");
            filtered["filters"] = json!([{"property":"label","op":"eq","value":group["variants"][0].as_str().map(Value::from).unwrap_or_else(|| group["group"].clone())}]);
            let filtered = ok(app.tool("metric_eval",filtered).await);
            assert_eq!(group["value"],filtered["value"],"{agg}: {group}");
            assert_eq!(group["n_entities"],filtered["n_entities"]);
        }
    }
}

#[tokio::test]
async fn overlapping_link_groups_warn_tools_and_views() {
    use eunomia_backend::sources::mock::{route,serve};
    let mock = serve(vec![]).await;
    let (app,vault) = scripted_chat(&mock).await;
    ok(app.tool("ontology_propose",json!({"metrics":[{"kind":"sample","name":"overlap_amount","expr":{"agg":"sum","property":"amount"}}],"links":[{"from_kind":"sample","label":"supplied_by","to_kind":"organisation"}]})).await);
    let (s,b) = app.http_session("POST",&format!("/api/vaults/{vault}/ontology/links/decide"),Some(json!({"from_kind":"sample","label":"supplied_by","to_kind":"organisation","decision":{"action":"approve"}}))).await; assert_eq!(s,200,"{b}");
    let entity = ok(app.tool("entities_search",json!({"query":"One","kind":"sample"})).await)["results"][0]["id"].clone();
    ok(app.tool("entity_set_property",json!({"entity_id":entity,"property":"amount","value":100})).await);
    for name in ["First supplier","Second supplier"] {
        let supplier = ok(app.tool("memory_write",json!({"subject_name":name,"subject_kind":"organisation","text":"Supplier"})).await)["entity"]["id"].clone();
        ok(app.tool("entity_relate",json!({"from_id":entity,"label":"supplied_by","to_id":supplier})).await);
    }
    let args = json!({"metric":"overlap_amount","kind":"sample","vault_id":vault,"allow_unapproved":true,"group_by":"link:supplied_by"});
    let metric = ok(app.tool("metric_eval",args.clone()).await);
    assert_eq!(metric["groups"].as_array().unwrap().iter().filter(|g| g["value"] == 100.0).count(),2);
    let note = "1 entity contributes to more than one group; groups overlap and must not be summed";
    assert!(metric["warnings"].as_array().unwrap().contains(&json!(note)),"{metric}");
    let mut view = args.clone(); view["view"] = json!("metric");
    assert!(ok(app.tool("show_view",view).await)["text"].as_str().unwrap().contains(note));
    let mut source = args.clone(); source["type"] = json!("metric");
    let mut grouped_spec = spec(); grouped_spec["elements"]["a"]["props"]["data"] = json!({"$state":"/sources/sample/metric/groups"});
    let view = ok(app.tool("show_view",json!({"view":"ui","spec":grouped_spec,"sources":{"sample":source}})).await);
    assert!(view["data"]["sources"]["sample"]["warnings"].as_array().unwrap().contains(&json!(note)));
    assert!(view["text"].as_str().unwrap().contains(note));
    let call = model_reply(json!({"tool_calls":[{"index":0,"id":"overlap","function":{"name":"metric_eval","arguments":args.to_string()}}]}));
    mock.set_routes(vec![route("POST","/chat/completions",Value::Null).raw(&call).times(1),route("POST","/chat/completions",Value::Null).raw(&model_reply(json!({"content":"The supplier groups overlap and must not be summed."})))]);
    let (events,_) = chat_events(&app,"Compare the supplier groups").await;
    assert_eq!(events.last().unwrap()["type"],"done");
    let request: Value = serde_json::from_str(&mock.requests().last().unwrap().body).unwrap();
    assert!(request["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool" && m["content"].as_str().is_some_and(|s| s.contains(note))));
}

#[tokio::test]
async fn configured_provider_model_is_used_including_budget_final_turn() {
    use eunomia_backend::sources::mock::{route,serve};
    let call = model_reply(json!({"tool_calls":[{"index":0,"id":"next","function":{"name":"ontology_get","arguments":"{}"}}]}));
    let mock = serve(vec![route("POST","/chat/completions",Value::Null).raw(&call).times(10),route("POST","/chat/completions",Value::Null).raw(&model_reply(json!({"content":"Here are the available results."})))]).await;
    let settings = eunomia_backend::config::Settings { openai_base_url:mock.base.clone(), openai_chat_model:"self-hosted-model".into(), ..common::test_settings() };
    let app = TestApp::with_settings(settings).await;
    let (events,_) = chat_events(&app,"Describe our ontology").await;
    assert_eq!(events.last().unwrap()["reply"],"Here are the available results.");
    assert_eq!(mock.requests().len(),11);
    for request in mock.requests() { assert_eq!(serde_json::from_str::<Value>(&request.body).unwrap()["model"],"self-hosted-model"); }
}

#[tokio::test]
async fn disconnect_cancels_budget_final_turn_without_persisting() {
    use eunomia_backend::{chat::service, sources::mock::{route,serve}};
    let call = model_reply(json!({"tool_calls":[{"index":0,"id":"next","function":{"name":"ontology_get","arguments":"{}"}}]}));
    let mock = serve(vec![route("POST","/chat/completions",Value::Null).raw(&call).times(10),route("POST","/chat/completions",Value::Null).delay(1500).raw(&model_reply(json!({"content":"This must never be saved."})))]).await;
    let (app,_) = scripted_chat(&mock).await;
    let db = app.db().await;
    let thread = common::sys(service::create_thread(&db,&app.user.id,None)).await.unwrap();
    let id = eunomia_backend::rid::parse(&thread.id).unwrap();
    let (tx,rx) = tokio::sync::mpsc::channel(256);
    let state = app.state.org(&app.user.org).await.unwrap();
    let user = app.user.clone(); let send_id = id.clone();
    let task = tokio::spawn(common::sys(async move { service::send_stream(state,user,send_id,"Describe our ontology".into(),tx).await }));
    tokio::time::timeout(std::time::Duration::from_secs(15),async {
        while mock.requests().len() < 11 { tokio::time::sleep(std::time::Duration::from_millis(10)).await; }
    }).await.unwrap();
    let before = common::sys(service::history(&db,&app.user.id,&id)).await.unwrap().unwrap();
    drop(rx); // Stop aborts the SSE fetch, which drops this receiver too.
    tokio::time::timeout(std::time::Duration::from_millis(500),task).await.expect("model future must be cancelled promptly").unwrap();
    let after = common::sys(service::history(&db,&app.user.id,&id)).await.unwrap().unwrap();
    assert_eq!(serde_json::to_value(before).unwrap(),serde_json::to_value(after).unwrap());
}

#[tokio::test]
async fn cancelled_ordinary_tool_batch_then_send_has_valid_history() {
    use eunomia_backend::{chat::service, sources::mock::{route,serve}};
    let mock = serve(vec![]).await;
    let (app,_) = scripted_chat(&mock).await;
    let embeddings_before = mock.requests().iter().filter(|r| r.path == "/embeddings").count();
    let call = model_reply(json!({"tool_calls":[
        {"index":0,"id":"completed","function":{"name":"ontology_get","arguments":"{}"}},
        {"index":1,"id":"pending","function":{"name":"recall","arguments":json!({"query":"cancel probe"}).to_string()}}
    ]}));
    mock.set_routes(vec![route("POST","/chat/completions",Value::Null).raw(&call), route("POST","/embeddings",json!({"data":[]})).delay(3000)]);
    let db = app.db().await;
    let thread = common::sys(service::create_thread(&db,&app.user.id,None)).await.unwrap();
    let id = eunomia_backend::rid::parse(&thread.id).unwrap();
    let (tx,mut rx) = tokio::sync::mpsc::channel(256);
    let state = app.state.org(&app.user.org).await.unwrap();
    let user = app.user.clone(); let send_id = id.clone();
    let task = tokio::spawn(common::sys(async move { service::send_stream(state,user,send_id,"Find the cancel probe".into(),tx).await }));
    tokio::time::timeout(std::time::Duration::from_secs(10),async {
        loop {
            let event = serde_json::to_value(rx.recv().await.unwrap()).unwrap();
            if event["type"] == "tool_result" && event["id"] == "completed" { break; }
        }
        while mock.requests().iter().filter(|r| r.path == "/embeddings").count() <= embeddings_before { tokio::time::sleep(std::time::Duration::from_millis(5)).await; }
    }).await.unwrap();
    drop(rx);
    tokio::time::timeout(std::time::Duration::from_millis(500),task).await.unwrap().unwrap();
    let history = common::sys(service::history(&db,&app.user.id,&id)).await.unwrap().unwrap();
    assert_eq!(history.len(),1,"An unfinished batch must never be persisted: {history:?}");
    mock.set_routes(vec![route("POST","/chat/completions",Value::Null).raw(&model_reply(json!({"content":"Ready to continue."})))]);
    let (tx,mut rx) = tokio::sync::mpsc::channel(256);
    common::sys(service::send_stream(app.state.org(&app.user.org).await.unwrap(),app.user.clone(),id,"Continue".into(),tx)).await;
    let mut done = false;
    while let Some(event) = rx.recv().await { if serde_json::to_value(event).unwrap()["type"] == "done" { done = true; } }
    assert!(done);
    let replay: Value = serde_json::from_str(&mock.requests().last().unwrap().body).unwrap();
    assert!(replay["messages"].as_array().unwrap().iter().all(|m| m.get("tool_calls").is_none() && m["role"] != "tool"),"{replay}");
}

#[tokio::test]
async fn merged_group_spellings_are_visible_in_tool_and_views() {
    let (app,vault) = sample().await;
    let rows = ok(app.tool("show_view",json!({"view":"ui","spec":spec(),"sources":{"sample":{"type":"dataset","vault_id":vault,"kind":"sample","properties":["amount"]}}})).await);
    for (i,row) in rows["data"]["sources"]["sample"]["rows"].as_array().unwrap().iter().enumerate() {
        ok(app.tool("entity_set_property",json!({"entity_id":row["entity_id"],"property":"label","value":if i == 0 { "ABC" } else { "abc" }})).await);
    }
    ok(app.tool("ontology_propose",json!({"metrics":[{"kind":"sample","name":"spelling_sum","expr":{"agg":"sum","property":"amount"}}]})).await);
    let args = json!({"metric":"spelling_sum","kind":"sample","vault_id":vault,"allow_unapproved":true,"group_by":"label"});
    let result = ok(app.tool("metric_eval",args.clone()).await);
    assert_eq!(result["groups"][0]["group"],"ABC (also: abc)");
    assert_eq!(result["groups"][0]["variants"],json!(["ABC","abc"]));
    assert_eq!(result["groups"][0]["value"],31.0);
    let mut view = args; view["view"] = json!("metric");
    assert!(ok(app.tool("show_view",view).await)["text"].as_str().unwrap().contains("ABC (also: abc)"));
}
