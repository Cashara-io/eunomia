mod common;
use common::TestApp;
use serde_json::{json,Value};

async fn fixture(app:&TestApp) -> (String,Vec<String>) {
    let vault = app.tool("ontology_get",json!({})).await["vault"].as_str().unwrap().to_string();
    let out = app.tool("ontology_propose",json!({"vault_id":vault,"kinds":[{"kind":"product"}],"properties":[{"kind":"product","name":"price","datatype":"money","unit":"AUD"},{"kind":"product","name":"margin","datatype":"percent"},{"kind":"product","name":"release","datatype":"date"}],"metrics":[{"kind":"product","name":"total_price","expr":{"agg":"sum","property":"price"}}]})).await;
    assert!(out.get("error").is_none(),"{out}");
    for (route,body) in [("kinds",json!({"kind":"product","decision":{"action":"approve"}})),("properties",json!({"kind":"product","name":"price","decision":{"action":"approve"}})),("properties",json!({"kind":"product","name":"margin","decision":{"action":"approve"}})),("properties",json!({"kind":"product","name":"release","decision":{"action":"approve"}})),("metrics",json!({"kind":"product","name":"total_price","decision":{"action":"approve"}}))] {
        let (status,value)=app.http_session("POST",&format!("/api/vaults/{vault}/ontology/{route}/decide"),Some(body)).await;
        assert_eq!(status,200,"{value}");
    }
    let mut ids=vec![];
    for i in 0..32 {
        let (_,entity)=app.http("POST","/api/entities",Some(json!({"kind":"product","name":format!("Product {i:02}"),"vault_id":vault})),true).await;
        let id=entity["id"].as_str().unwrap().to_string();
        if i<31 {
            let out=app.tool("entity_set_property",json!({"entity_id":id,"property":"price","value":i})).await;
            assert!(out.get("error").is_none(),"{out}");
        }
        ids.push(id);
    }
    (vault,ids)
}
async fn get(app:&TestApp,vault:&str,query:&str)->Value {
    let (status,out)=app.http("GET",&format!("/api/vaults/{vault}/ontology/objects{query}"),None,true).await;
    assert_eq!(status,200,"{out}");out
}
#[tokio::test]
async fn paging_typed_sort_filters_missing_and_object_metric_scope() {
    let app=TestApp::new().await;
    let (vault,ids)=fixture(&app).await;
    let page=get(&app,&vault,"?kind=product&sort=price&descending=true&limit=5&offset=5").await;
    assert_eq!(page["total"],32);
    assert_eq!(page["rows"][0]["values"]["price"]["value"],25.0);
    assert_eq!(page["rows"][0]["values"]["price"]["unit"],"AUD");
    assert_eq!(page["rows"][0]["source_count"],0);
    let filtered=get(&app,&vault,"?kind=product&filter=price&op=gte&value=29&sort=price&limit=1&offset=1").await;
    assert_eq!(filtered["total"],2);
    assert_eq!(filtered["rows"][0]["values"]["price"]["value"],30.0);
    let missing=get(&app,&vault,"?kind=product&filter=price&op=missing").await;
    assert_eq!(missing["total"],1);
    assert!(missing["rows"][0]["values"]["price"]["value"].is_null());
    let zero=get(&app,&vault,"?kind=product&filter=price&op=eq&value=0").await;
    assert_eq!(zero["total"],1);
    assert_eq!(zero["rows"][0]["values"]["price"]["value"],0.0);
    let profile=get(&app,&vault,&format!("/{}",ids[12])).await;
    assert_eq!(profile["numbers"][0]["result"]["run"]["value"],12.0);
    assert_eq!(profile["numbers"][0]["result"]["run"]["entity_ids"],json!([ids[12]]));
    assert_eq!(profile["numbers"][0]["result"]["run"]["n_entities"],1);
    for property in ["margin","release"] { assert_eq!(profile["properties"].as_array().unwrap().iter().find(|p|p["name"]==property).unwrap()["state"],"missing"); }
    for bad in ["?kind=product&sort=unknown","?kind=product&limit=101","?kind=product&filter=price&op=gte&value=no"] {
        let (status,_)=app.http("GET",&format!("/api/vaults/{vault}/ontology/objects{bad}"),None,true).await;assert_eq!(status,400);
    }
}

#[tokio::test]
async fn aggregates_refuse_non_members_and_restricted_credentials() {
    let app=TestApp::new().await;
    let (vault,ids)=fixture(&app).await;
    let other=common::register(&app.state,"outsider@example.com").await;
    let token=eunomia_backend::models_user::create_api_token(&app.state.control,&other.id,"outsider").await.unwrap().token;
    for suffix in ["?kind=product".to_string(),format!("/{}",ids[0])] {
        let (status,out)=common::http(&app.router,"GET",&format!("/api/vaults/{vault}/ontology/objects{suffix}"),None,Some(&token),None).await.0;
        assert_eq!(status,403,"{out}");
    }
}

#[tokio::test]
async fn token_scopes_and_vault_restrictions_are_enforced() {
    let app=TestApp::new().await;
    let (vault,ids)=fixture(&app).await;
    let (_,other)=app.http("POST","/api/vaults",Some(json!({"name":"Other space"})),true).await;
    let other_id=eunomia_backend::rid::parse(other["id"].as_str().unwrap()).unwrap();
    let limited=eunomia_backend::models_user::create_api_token_with(&app.state.control,&app.user.id,"wrong scope",&["settings:read".into()],None,None).await.unwrap().token;
    let restricted=eunomia_backend::models_user::create_api_token_with(&app.state.control,&app.user.id,"wrong vault",&["memory:read".into()],Some(&other_id),None).await.unwrap().token;
    for token in [limited,restricted] {
        for suffix in ["?kind=product".to_string(),format!("/{}",ids[0])] {
            let (status,out)=common::http(&app.router,"GET",&format!("/api/vaults/{vault}/ontology/objects{suffix}"),None,Some(&token),None).await.0;
            assert_eq!(status,403,"{out}");
        }
    }
    let (status,_) = app.http("GET",&format!("/api/vaults/{}/ontology/objects/{}",other["id"].as_str().unwrap(),ids[0]),None,true).await;
    assert_eq!(status,404,"profile id cannot cross spaces");
}
