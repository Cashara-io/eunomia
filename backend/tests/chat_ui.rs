mod common;
use common::TestApp;
use serde_json::{json, Value};
fn ok(v: Value) -> Value { assert!(v.get("error").is_none(),"{v}"); v }
fn spec() -> Value { json!({"root":"a","elements":{"a":{"type":"Table","props":{"title":"Rows","summary":"Approved values","data":{"$state":"/sources/sample"}}}}}) }
#[tokio::test]
async fn dataset_resolves_real_typed_values_and_mcp_catalog() {
    let app=TestApp::new().await;
    let vault=ok(app.tool("ontology_get",json!({})).await)["vault"].as_str().unwrap().to_string();
    ok(app.tool("ontology_propose",json!({"kinds":[{"kind":"sample"}],"properties":[{"kind":"sample","name":"amount","datatype":"money","unit":"AUD"},{"kind":"sample","name":"share","datatype":"percent","unit":"%"}]})).await);
    for (path,body) in [("kinds",json!({"kind":"sample","decision":{"action":"approve"}})),("properties",json!({"kind":"sample","name":"amount","decision":{"action":"approve"}})),("properties",json!({"kind":"sample","name":"share","decision":{"action":"approve"}}))] {
        let (status,body)=app.http_session("POST",&format!("/api/vaults/{vault}/ontology/{path}/decide"),Some(body)).await;
        assert_eq!(status,200,"{body}");
    }
    for (name,amount) in [("One",10.25),("Two",20.75)] {
        let entity=ok(app.tool("memory_write",json!({"subject_name":name,"subject_kind":"sample","text":"A sample"})).await)["entity"]["id"].clone();
        ok(app.tool("entity_set_property",json!({"entity_id":entity,"property":"amount","value":amount})).await);
        ok(app.tool("entity_set_property",json!({"entity_id":entity,"property":"share","value":35.7})).await);
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
    let other=common::register(&app.state,"outsider@example.com").await;
    let denied=common::sys(eunomia_backend::tools::registry::call(&app.state,&other,"show_view",args.clone())).await.unwrap();
    assert!(denied.get("error").is_some(),"another org cannot read the dataset: {denied}");
    ok(app.tool("ontology_propose",json!({"kinds":[{"kind":"unreviewed"}]})).await);
    let (status,body)=app.http_session("POST",&format!("/api/vaults/{vault}/ontology/strict"),Some(json!({"on":true}))).await;
    assert_eq!(status,200,"{body}");
    let mut unreviewed=args.clone();unreviewed["sources"]["sample"]["kind"]=json!("unreviewed");
    assert!(app.tool("show_view",unreviewed).await.get("error").is_some());
    let mut bad=args;bad["sources"]["sample"]["limit"]=json!(501);
    assert!(app.tool("show_view",bad).await.get("error").is_some());
}
