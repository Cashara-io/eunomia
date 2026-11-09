mod common;
use common::TestApp;
use serde_json::{json, Value};
fn ok(v: Value) -> Value { assert!(v.get("error").is_none(),"{v}"); v }
fn spec() -> Value { json!({"root":"a","elements":{"a":{"type":"Table","props":{"title":"Rows","summary":"Approved values","data":{"$state":"/sources/sample"}}}}}) }
#[tokio::test]
async fn dataset_resolves_real_typed_values_and_mcp_catalog() {
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
    grouped["sources"]["sample"]["aggregates"]=json!([{"name":"count","op":"count"}]);
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
                query["sources"]["sample"]["aggregates"] = json!([{"name":"result","op":op,"property":property}]);
                let dataset = ok(app.tool("show_view",query).await);
                assert_eq!(dataset["data"]["sources"]["sample"]["rows"][0]["result"].as_f64(), evaluated["value"].as_f64(),"{op} {property}, empty={empty}");
            }
        }
    }
    let other=common::register(&app.state,"outsider@example.com").await;
    assert_eq!(other.org, app.user.org);
    let denied=common::sys(eunomia_backend::tools::registry::call(&app.state,&other,"show_view",args.clone())).await.unwrap();
    assert!(denied.get("error").is_some(),"same-org nonmember cannot read the dataset: {denied}");
    let team = ok(app.tool("vault_create",json!({"name":"Team","kind":"org"})).await)["id"].as_str().unwrap().to_string();
    let only = eunomia_backend::rid::parse(&team).unwrap();
    let token = eunomia_backend::models_user::create_api_token_with(&app.state.control,&app.user.id,"limited", &["memory:read".into()],Some(&only),None).await.unwrap().token;
    let ((status,denied),_) = common::http(&app.router,"POST","/api/tools/show_view",Some(args.clone()),Some(&token),None).await;
    assert!(status == 403 || denied.get("error").is_some(),"{status}: {denied}");
    let foreign = ok(app.tool("memory_write",json!({"subject_name":"Foreign supplier","subject_kind":"organisation","text":"Elsewhere","vault_id":team})).await)["entity"]["id"].clone();
    // A legacy cross-vault edge must not supply grouped data, even if both vaults are readable.
    let db = app.db().await;
    let entity = eunomia_backend::rid::parse(out["data"]["sources"]["sample"]["rows"][0]["entity_id"].as_str().unwrap()).unwrap();
    let foreign_id = eunomia_backend::rid::parse(foreign.as_str().unwrap()).unwrap();
    db.test_raw().query("RELATE $from->relates_to->$to SET label = 'supplied_by'").bind(("from",entity)).bind(("to",foreign_id)).await.unwrap().check().unwrap();
    let mut cross = args.clone(); cross["sources"]["sample"]["properties"] = json!(["link:supplied_by"]); cross["sources"]["sample"].as_object_mut().unwrap().remove("filters");
    let cross = ok(app.tool("show_view",cross).await);
    assert!(!cross.to_string().contains("Foreign supplier"));
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
    let mut bad=args;bad["sources"]["sample"]["limit"]=json!(501);
    assert!(app.tool("show_view",bad).await.get("error").is_some());
}
