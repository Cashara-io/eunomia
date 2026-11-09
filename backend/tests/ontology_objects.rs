mod common;
use common::TestApp;
use serde_json::{json,Value};
use surrealdb::types::{RecordId, SurrealValue};

#[derive(SurrealValue)]
struct FixtureEntity { id: RecordId, name: String }
#[derive(SurrealValue)]
struct FixturePoint { id: RecordId, entity: RecordId, value: f64 }
#[derive(SurrealValue)]
struct TimedPoint { id: RecordId, value: f64, as_of: surrealdb::types::Datetime, recorded: surrealdb::types::Datetime }


async fn fixture(app:&TestApp) -> (String,Vec<String>) {
    let (status,created)=app.http("POST","/api/vaults",Some(json!({"name":"Banksia Basket Group"})),true).await;
    assert_eq!(status,200,"{created}");
    let vault=created["id"].as_str().unwrap().to_string();
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
    let kinds=get(&app,&vault,"").await;
    assert_eq!(kinds["kinds"][0]["kind"],"product");
    assert_eq!(kinds["kinds"][0]["entities"],32);
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
    for suffix in ["".to_string(),"?kind=product".to_string(),format!("/{}",ids[0])] {
        let (status,out)=common::http(&app.router,"GET",&format!("/api/vaults/{vault}/ontology/objects{suffix}"),None,Some(&token),None).await.0;
        assert_eq!(status,403,"{out}");
    }
    struct ClearMutations;
    impl Drop for ClearMutations { fn drop(&mut self) {eunomia_backend::isolation::clear_mutations();} }
    let _clear=ClearMutations;
    eunomia_backend::isolation::mutate("vaults.membership_active", "user = $user", "true");
    for suffix in ["".to_string(),"?kind=product".to_string(),format!("/{}",ids[0])] {
        let (status,out)=common::http(&app.router,"GET",&format!("/api/vaults/{vault}/ontology/objects{suffix}"),None,Some(&token),None).await.0;
        assert_eq!(status,200,"membership guard mutant must expose the object: {out}");
    }
    eunomia_backend::isolation::clear_mutations();

}

#[tokio::test]
async fn token_scopes_and_vault_restrictions_are_enforced() {
    let app=TestApp::new().await;
    let (vault,ids)=fixture(&app).await;
    let (_,other)=app.http("POST","/api/vaults",Some(json!({"name":"Other space"})),true).await;
    let other_id=eunomia_backend::rid::parse(other["id"].as_str().unwrap()).unwrap();
    let limited=eunomia_backend::models_user::create_api_token_with(&app.state.control,&app.user.id,"wrong scope",&["connectors".into()],None,None).await.unwrap().token;
    let restricted=eunomia_backend::models_user::create_api_token_with(&app.state.control,&app.user.id,"wrong vault",&["memory:read".into()],Some(&other_id),None).await.unwrap().token;
    let mut statuses=Vec::new();
    for token in [limited,restricted] {
        for suffix in ["".to_string(),"?kind=product".to_string(),format!("/{}",ids[0])] {
            let (status,_)=common::http(&app.router,"GET",&format!("/api/vaults/{vault}/ontology/objects{suffix}"),None,Some(&token),None).await.0;
            statuses.push(status.as_u16());
        }
    }
    assert_eq!(statuses,vec![403;6],"kind selector, directory and profile must enforce both credential gates");
    let (status,_) = app.http("GET",&format!("/api/vaults/{}/ontology/objects/{}",other["id"].as_str().unwrap(),ids[0]),None,true).await;
    assert_eq!(status,404,"profile id cannot cross spaces");
}

/// An isolated, in-memory Banksia-sized fixture for screenshots and endpoint timings.
/// Run the compiled test binary with --ignored --nocapture serve_banksia_sized_fixture.
#[tokio::test]
#[ignore]
async fn serve_banksia_sized_fixture() {
    // Reopen the same disposable remote fixture after a database/backend restart.
    // This keeps fixture writes out of cold measurements and leaves the DB caches cold.
    if std::env::var_os("ONTOLOGY_REUSE_FIXTURE").is_some() {
        let output=std::env::var("ONTOLOGY_FIXTURE_DIR").unwrap();
        let info:Value=serde_json::from_str(&std::fs::read_to_string(format!("{output}/fixture.json")).unwrap()).unwrap();
        let mut settings=common::test_settings();
        assert!(settings.surreal_url != "mem://", "reopening requires the disposable remote database");
        settings.surreal_ns=info["namespace"].as_str().unwrap().to_string();
        let state=eunomia_backend::state::AppState::build(&settings,common::engine_config()).await.unwrap();
        let port=std::env::var("ONTOLOGY_FIXTURE_PORT").unwrap_or_else(|_|"18860".into());
        let listener=tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await.unwrap();
        println!("Reopened Banksia-sized fixture on {port}");
        axum::serve(listener,eunomia_backend::app(state).into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
        return;
    }
    let app=TestApp::new().await;
    let (vault,ids)=fixture(&app).await;
    assert_eq!(app.http_session("POST","/api/settings/complete-onboarding",Some(json!({}))).await.0,200);
    let name = |i:usize| match i {0=>"Almond breakfast basket".to_string(),1=>"Barista coffee basket".to_string(),_=>format!("Basket range {i:03}")};
    for (i,id) in ids.iter().enumerate() {
        assert_eq!(app.http("PATCH",&format!("/api/entities/{id}"),Some(json!({"name":name(i)})),true).await.0,200);
    }
    let proposal=app.tool("ontology_propose",json!({"vault_id":vault,"properties":[
        {"kind":"product","name":"cost","datatype":"money","unit":"AUD"},
        {"kind":"product","name":"units","datatype":"integer"}
    ],"metrics":[{"kind":"product","name":"gross_profit","unit":"AUD","expr":{"agg":"sum","row":{"op":"mul","left":{"op":"sub","left":"price","right":"cost"},"right":"units"}}}]})).await;
    assert!(proposal.get("error").is_none(),"{proposal}");
    for (route,field) in [("properties","cost"),("properties","units"),("metrics","gross_profit")] {
        assert_eq!(app.http_session("POST",&format!("/api/vaults/{vault}/ontology/{route}/decide"),Some(json!({"kind":"product","name":field,"decision":{"action":"approve"}}))).await.0,200);
    }
    let mut text="Product,price,cost,units,margin,release\n".to_string();
    for i in 0..300 {
        let amount=if i==0 {0.0} else {24.9+(i%17) as f64};
        let price=if i==31 {String::new()} else {format!("{amount:.2}")};
        let margin=if i==0 || i==31 {String::new()} else {format!("{:.8}",(amount-14.9)/amount)};
        text.push_str(&format!("{},{price},14.90,100,{margin},2026-10-01\n",name(i)));
    }
    let csv=app.tool("document_upload",json!({"filename":"banksia-products.csv","vault_id":vault,"text":text})).await;
    let imported=app.tool("document_import_rows",json!({"document_id":csv["id"],"kind":"product","name_column":"Product"})).await;
    assert!(imported.get("error").is_none(),"{imported}");
    assert_eq!(app.http("POST",&format!("/api/entities/{}/memory",ids[1]),Some(json!({"text":"Banksia summer range. Review supplier pricing before the next order."})),true).await.0,200);
    let db=app.db().await;
    let vault_id=eunomia_backend::rid::parse(&vault).unwrap();
    for (kind,count,start) in [("stock_record",12000,0),("weekly_sales",6240,0),("supplier",338,0)] {
        for batch in (0..count).step_by(500) {
            let rows:Vec<_>=(batch..(batch+500).min(count)).map(|i| {
                let i=i+start;
                FixtureEntity { id:RecordId::new("entity",format!("{kind}_{i}")),name:format!("{} {i:05}",kind.replace('_'," ")) }
            }).collect();
            db.test_raw().query("INSERT INTO entity $rows.map(|$r| {id:$r.id,vault:$vault,owner:$owner,kind:$kind,name:$r.name,aliases:[],summary:''});")
                .bind(("rows",rows)).bind(("vault",vault_id.clone())).bind(("owner",app.user.id.clone())).bind(("kind",kind.to_string())).await.unwrap().check().unwrap();
        }
    }
    // Both large kinds carry typed values, including zero, for realistic page work.
    for (kind, count) in [("stock_record", 12000), ("weekly_sales", 6240)] {
        let proposed = app.tool("ontology_propose", json!({"vault_id":vault,
            "kinds":[{"kind":kind}],
            "properties":[{"kind":kind,"name":"units","datatype":"integer","unit":"units"}]
        })).await;
        assert!(proposed.get("error").is_none(), "{proposed}");
        for (route, body) in [
            ("kinds", json!({"kind":kind,"decision":{"action":"approve"}})),
            ("properties", json!({"kind":kind,"name":"units","decision":{"action":"approve"}})),
        ] {
            assert_eq!(app.http_session("POST", &format!("/api/vaults/{vault}/ontology/{route}/decide"), Some(body)).await.0, 200);
        }
        let rows:Vec<_>=(0..count).map(|i|FixturePoint {
            id:RecordId::new("entity_property",format!("{kind}_{i}")),
            entity:RecordId::new("entity",format!("{kind}_{i}")),value:i as f64,
        }).collect();
        db.test_raw().query("LET $p=(SELECT VALUE id FROM ontology_property WHERE vault=$vault AND kind=$kind AND name='units')[0]; INSERT INTO entity_property $rows.map(|$r| {id:$r.id,vault:$vault,entity:$r.entity,property:$p,value_num:$r.value,as_of:time::now(),origin:'agent'});")
            .bind(("vault",vault_id.clone())).bind(("kind",kind.to_string())).bind(("rows",rows)).await.unwrap().check().unwrap();
    }
    let second=eunomia_backend::rid::parse(&ids[1]).unwrap();
    db.test_raw().query("RELATE $from->relates_to->$to SET label='substitute_for',owner=$owner;")
        .bind(("from",eunomia_backend::rid::parse(&ids[0]).unwrap())).bind(("to",second)).bind(("owner",app.user.id.clone())).await.unwrap().check().unwrap();
    // Include sub-cent money in the number screenshots without changing the source-row metric.
    let out=app.tool("entity_set_property",json!({"entity_id":ids[2],"property":"price","value":0.0049})).await;
    assert!(out.get("error").is_none(),"{out}");
    let output=std::env::var("ONTOLOGY_FIXTURE_DIR").unwrap_or_else(|_|"/private/tmp/eunomia-ontology2".into());
    std::fs::create_dir_all(&output).unwrap();
    std::fs::write(format!("{output}/fixture.json"),json!({"namespace":app.state.settings.surreal_ns,"space":vault,"object":ids[1],"subcent_object":ids[2],"entities":18878,"products":300,"stock_records":12000,"weekly_sales":6240}).to_string()).unwrap();
    if std::env::var_os("ONTOLOGY_EXPLAIN").is_some() {
        let p:Option<surrealdb::types::RecordId>=db.test_raw().query("SELECT VALUE id FROM ontology_property WHERE vault=$vault AND kind='stock_record' AND name='units'")
            .bind(("vault",vault_id.clone())).await.unwrap().take::<Vec<surrealdb::types::RecordId>>(0).unwrap().into_iter().next();
        let mut plans=serde_json::Map::new();
        for (case,sort,filter) in [("name",None,false),("sort",Some("value_num"),false),("filter",Some("value_num"),true)] {
            let sql=eunomia_backend::store::objects::page_sql("entity",sort,if filter{Some("value_num")}else{None},if filter{"gte"}else{""},sort.is_some(),filter,false);
            let start=sql.find("(SELECT").unwrap()+1;
            let end=if sort.is_some(){sql.find(';').unwrap()-1}else{sql.find("), total:").unwrap()};
            let explain=format!("{} EXPLAIN FULL;",&sql[start..end]);
            let plan:Option<Value>=db.test_raw().query(explain).bind(("vault",vault_id.clone())).bind(("kind","stock_record"))
                .bind(("sort_property",p.clone())).bind(("filter_property",p.clone())).bind(("filter_value",11900))
                .bind(("as_of",surrealdb::types::Datetime::from(chrono::Utc::now()))).bind(("limit",25)).bind(("offset",0)).await.unwrap().take(0).unwrap();
            plans.insert(case.into(),plan.unwrap());
        }
        let plan:Option<Value>=db.test_raw().query("SELECT * FROM entity_property WHERE entity=$e AND property=$p AND vault=$vault AND as_of<=$as_of ORDER BY as_of DESC,created_at DESC,id DESC LIMIT 1 EXPLAIN FULL;")
            .bind(("e",surrealdb::types::RecordId::new("entity","stock_record_1"))).bind(("p",p)).bind(("vault",vault_id.clone()))
            .bind(("as_of",surrealdb::types::Datetime::from(chrono::Utc::now()))).await.unwrap().take(0).unwrap();
        plans.insert("latest".into(),plan.unwrap());
        std::fs::write(format!("{output}/explain.json"),serde_json::to_string_pretty(&plans).unwrap()).unwrap();
    }
    let port=std::env::var("ONTOLOGY_FIXTURE_PORT").unwrap_or_else(|_|"18760".into());
    let listener=tokio::net::TcpListener::bind(format!("127.0.0.1:{port}")).await.unwrap();
    println!("Isolated Banksia-sized fixture ready on {port}");
    axum::serve(listener,app.router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
}

#[tokio::test]
async fn typed_dates_and_distinct_property_states_and_record_sources() {
    let app=TestApp::new().await;
    let (vault,ids)=fixture(&app).await;
    for (property,value) in [("release",json!("2026-10-01")),("margin",json!(0.25))] {
        let out=app.tool("entity_set_property",json!({"entity_id":ids[0],"property":property,"value":value})).await;
        assert!(out.get("error").is_none(),"{out}");
    }
    let proposed=app.tool("ontology_propose",json!({"vault_id":vault,"properties":[{"kind":"product","name":"units","datatype":"integer"},{"kind":"product","name":"active","datatype":"bool"},{"kind":"product","name":"captured","datatype":"datetime"}]})).await;
    assert!(proposed.get("error").is_none(),"{proposed}");
    for name in ["units","active","captured"] {
        assert_eq!(app.http_session("POST",&format!("/api/vaults/{vault}/ontology/properties/decide"),Some(json!({"kind":"product","name":name,"decision":{"action":"approve"}}))).await.0,200);
    }
    for (entity,property,value) in [(&ids[0],"units",json!(0)),(&ids[1],"units",json!(12)),(&ids[0],"active",json!(false)),(&ids[1],"active",json!(true))] {
        let out=app.tool("entity_set_property",json!({"entity_id":entity,"property":property,"value":value})).await;
        assert!(out.get("error").is_none(),"{out}");
    }
    for query in ["?kind=product&filter=units&op=eq&value=0","?kind=product&filter=units&op=eq&value=12","?kind=product&filter=active&op=eq&value=false"] {
        assert_eq!(get(&app,&vault,query).await["total"],1,"{query}");
    }
    // A whole second must sort before its fractional part, including across a page.
    for (entity, value) in [(&ids[0], "2026-10-01T00:00:00Z"), (&ids[1], "2026-10-01T00:00:00.125Z")] {
        let out = app.tool("entity_set_property", json!({"entity_id":entity,"property":"captured","value":value})).await;
        assert!(out.get("error").is_none(), "{out}");
    }
    let ordered = get(&app, &vault, "?kind=product&sort=captured&limit=1&offset=1").await;
    assert_eq!(ordered["rows"][0]["id"], ids[1]);
    let at_second = get(&app, &vault, "?kind=product&filter=captured&op=lte&value=2026-10-01T00:00:00Z").await;
    assert_eq!(at_second["total"], 1);
    let date=get(&app,&vault,"?kind=product&filter=release&op=eq&value=2026-10-01").await;
    assert_eq!(date["total"],1);
    assert_eq!(date["rows"][0]["values"]["release"]["value"],"2026-10-01");
    assert_eq!(date["rows"][0]["values"]["margin"]["value"],0.25);
    let db=app.db().await;
    let entity=eunomia_backend::rid::parse(&ids[0]).unwrap();
    let v=eunomia_backend::rid::parse(&vault).unwrap();
    db.test_raw().query("CREATE cache_record:proof SET vault=$vault,owner=$owner,source='fixture',type='product',external_id='product-00',title='Product source',content_hash='proof',ingested_at=time::now(),updated_at=time::now(),payload={row:2,price:99}; UPDATE entity_property SET source=cache_record:proof,as_of=d'2026-01-01T00:00:00Z' WHERE entity=$entity AND property.name='price';")
        .bind(("vault",v.clone())).bind(("owner",app.user.id.clone())).bind(("entity",entity.clone())).await.unwrap().check().unwrap();
    let profile=get(&app,&vault,&format!("/{}",ids[0])).await;
    let price=profile["properties"].as_array().unwrap().iter().find(|p|p["name"]=="price").unwrap();
    assert_eq!(price["state"],"stale");
    assert_eq!(profile["sources"][0]["record"]["payload"]["row"],2);
    db.test_raw().query("UPDATE cache_record:proof SET title='';").await.unwrap().check().unwrap();
    let untitled=get(&app,&vault,&format!("/{}",ids[0])).await;
    let source=untitled["properties"].as_array().unwrap().iter().find(|p|p["name"]=="price").unwrap();
    assert_eq!(source["source_label"],"product-00");
    assert_eq!(source["source_state"],"available");
    let directory=get(&app,&vault,"?kind=product&search=Product%2000").await;
    assert_eq!(directory["rows"][0]["source_count"],1);
    db.test_raw().query("UPDATE cache_record:proof SET updated_at=d'2026-04-01T00:00:00Z';").await.unwrap().check().unwrap();
    let historical=get(&app,&vault,&format!("/{}",ids[0])).await;
    assert_eq!(historical["properties"].as_array().unwrap().iter().find(|p|p["name"]=="price").unwrap()["state"],"recorded","an old as-of time alone does not make a freshly recorded historical value stale");
    db.test_raw().query("LET $p=(SELECT VALUE id FROM ontology_property WHERE vault=$vault AND name='price')[0]; CREATE entity_property:agreement SET vault=$vault,entity=$entity,property=$p,value_num=0,as_of=d'2026-01-01T00:00:00Z',origin='agent';")
        .bind(("vault",v.clone())).bind(("entity",entity.clone())).await.unwrap().check().unwrap();
    let agreement=get(&app,&vault,&format!("/{}",ids[0])).await;
    assert_eq!(agreement["properties"].as_array().unwrap().iter().find(|p|p["name"]=="price").unwrap()["state"],"recorded","an inherited AUD unit agrees with an explicit AUD unit");
    db.test_raw().query("LET $p=(SELECT VALUE id FROM ontology_property WHERE vault=$vault AND name='price')[0]; CREATE entity_property:conflict SET vault=$vault,entity=$entity,property=$p,value_num=99,as_of=d'2026-01-01T00:00:00Z',origin='agent';")
        .bind(("vault",v.clone())).bind(("entity",entity.clone())).await.unwrap().check().unwrap();
    db.test_raw().query("LET $p=(SELECT VALUE id FROM ontology_property WHERE vault=$vault AND name='price')[0]; CREATE entity_property:conflict_same SET vault=$vault,entity=$entity,property=$p,value_num=99,as_of=d'2026-01-01T00:00:00Z',origin='agent';")
        .bind(("vault",v.clone())).bind(("entity",entity.clone())).await.unwrap().check().unwrap();
    let profile=get(&app,&vault,&format!("/{}",ids[0])).await;
    assert_eq!(profile["properties"].as_array().unwrap().iter().find(|p|p["name"]=="price").unwrap()["state"],"conflicting");
    db.test_raw().query("DELETE entity_property WHERE entity=$entity AND property.name='price'; LET $p=(SELECT VALUE id FROM ontology_property WHERE vault=$vault AND name='price')[0]; CREATE entity_property:unavailable SET vault=$vault,entity=$entity,property=$p,as_of=time::now(),origin='agent';")
        .bind(("vault",v)).bind(("entity",entity)).await.unwrap().check().unwrap();
    let profile=get(&app,&vault,&format!("/{}",ids[0])).await;
    assert_eq!(profile["properties"].as_array().unwrap().iter().find(|p|p["name"]=="price").unwrap()["state"],"unavailable");
    assert_eq!(get(&app,&vault,"?kind=product&filter=price&op=lte&value=0").await["total"],0,"unavailable and missing values do not satisfy a numeric range");
}

#[tokio::test]
async fn named_link_columns_deduplicate_labels_and_honour_approved_target_kinds() {
    let app=TestApp::new().await;
    let (vault,ids)=fixture(&app).await;
    let out=app.tool("ontology_propose",json!({"vault_id":vault,"kinds":[{"kind":"supplier"}],"links":[
        {"from_kind":"product","label":"supplied_by","to_kind":"supplier"},
        {"from_kind":"product","label":"supplied_by","to_kind":"organisation"}
    ]})).await;
    assert!(out.get("error").is_none(),"{out}");
    for kind in ["supplier","organisation"] {
        assert_eq!(app.http_session("POST",&format!("/api/vaults/{vault}/ontology/kinds/decide"),Some(json!({"kind":kind,"decision":{"action":"approve"}}))).await.0,200);
        let (status,out)=app.http_session("POST",&format!("/api/vaults/{vault}/ontology/links/decide"),Some(json!({"from_kind":"product","label":"supplied_by","to_kind":kind,"decision":{"action":"approve"}}))).await;
        assert_eq!(status,200,"{out}");
    }
    let db=app.db().await;
    for (kind,name) in [("supplier","Fixture supplier"),("organisation","Fixture organisation"),("unmodelled","Unapproved target kind")] {
        let (status,target)=app.http("POST","/api/entities",Some(json!({"kind":kind,"name":name,"vault_id":vault})),true).await;
        assert_eq!(status,200,"{target}");
        db.test_raw().query("RELATE $from->relates_to->$to SET label='supplied_by',owner=$owner;")
            .bind(("from",eunomia_backend::rid::parse(&ids[0]).unwrap())).bind(("to",eunomia_backend::rid::parse(target["id"].as_str().unwrap()).unwrap()))
            .bind(("owner",app.user.id.clone())).await.unwrap().check().unwrap();
    }
    let list=get(&app,&vault,"?kind=product&search=Product%2000").await;
    assert_eq!(list["columns"].as_array().unwrap().iter().filter(|c|c["name"]=="link:supplied_by").count(),1);
    let names=list["rows"][0]["values"]["link:supplied_by"]["value"].as_str().unwrap();
    assert!(names.contains("Fixture supplier") && names.contains("Fixture organisation"),"{names}");
    assert!(!names.contains("Unapproved target kind"));
    let targets:Vec<_>=(0..5002).map(|i|FixtureEntity { id:RecordId::new("entity",format!("hub_supplier_{i}")),name:format!("Hub supplier {i}") }).collect();
    db.test_raw().query("FOR $r IN $targets { LET $to=$r.id; CREATE $to SET vault=$vault,owner=$owner,kind='supplier',name=$r.name,aliases=[],summary=''; RELATE $from->relates_to->$to SET label='supplied_by',owner=$owner; };")
        .bind(("targets",targets)).bind(("vault",eunomia_backend::rid::parse(&vault).unwrap())).bind(("owner",app.user.id.clone())).bind(("from",eunomia_backend::rid::parse(&ids[0]).unwrap())).await.unwrap().check().unwrap();
    let hub=get(&app,&vault,"?kind=product&search=Product%2000").await;
    let cell=hub["rows"][0]["values"]["link:supplied_by"]["value"].as_str().unwrap();
    assert!(cell.contains("More links in profile"),"large hubs must disclose bounded names");
    assert!(cell.len()<1500,"the directory must not return every hub target");
}

#[tokio::test]
async fn latest_profile_directory_metric_parity_with_fractional_ties() {
    check_latest_parity().await;
}

#[tokio::test]
#[ignore = "deliberate latest-order guard mutation"]
#[should_panic(expected = "native as_of, recorded_at, then record id must win")]
async fn latest_order_guard_mutation_is_detected() {
    struct Restore;
    impl Drop for Restore { fn drop(&mut self) { eunomia_backend::isolation::clear_mutations(); } }
    let _restore=Restore;
    eunomia_backend::isolation::mutate("ontology.latest_values", "ORDER BY as_of DESC", "ORDER BY as_of ASC");
    check_latest_parity().await;
}

async fn check_latest_parity() {
    let app=TestApp::new().await;
    let (vault,ids)=fixture(&app).await;
    let db=app.db().await;
    let rows:Vec<_>=[
        ("second",10.0,"2020-01-01T00:00:00Z","2020-01-01T00:00:00Z"),
        ("fraction",20.0,"2020-01-01T00:00:00.125Z","2020-01-01T00:00:00Z"),
        ("recorded_a",30.0,"2020-01-01T00:00:00.125Z","2020-01-01T00:00:00.125Z"),
        ("recorded_b",40.0,"2020-01-01T00:00:00.125Z","2020-01-01T00:00:00.125Z"),
        ("future",99.0,"2099-01-01T00:00:00Z","2020-01-01T00:00:00Z"),
    ].into_iter().map(|(id,value,as_of,recorded)|TimedPoint {
        id:RecordId::new("entity_property",id),value,
        as_of:as_of.parse::<chrono::DateTime<chrono::Utc>>().unwrap().into(),
        recorded:recorded.parse::<chrono::DateTime<chrono::Utc>>().unwrap().into(),
    }).collect();
    db.test_raw().query(r#"
        LET $p=(SELECT VALUE id FROM ontology_property WHERE vault=$vault AND name='price')[0];
        DELETE entity_property WHERE entity=$entity AND property=$p;
        FOR $r IN $rows { LET $id=$r.id; CREATE $id SET entity=$entity,property=$p,vault=$vault,value_num=$r.value,as_of=$r.as_of,created_at=$r.recorded,origin='agent'; };
    "#).bind(("rows",rows)).bind(("vault",eunomia_backend::rid::parse(&vault).unwrap()))
        .bind(("entity",eunomia_backend::rid::parse(&ids[0]).unwrap())).await.unwrap().check().unwrap();
    let profile=get(&app,&vault,&format!("/{}",ids[0])).await;
    let property=profile["properties"].as_array().unwrap().iter().find(|p|p["name"]=="price").unwrap();
    assert_eq!(property["value"],40.0,"native as_of, recorded_at, then record id must win");
    let directory=get(&app,&vault,"?kind=product&filter=price&op=eq&value=40&sort=price").await;
    assert_eq!(directory["total"],1);
    assert_eq!(directory["rows"][0]["values"]["price"]["value"],property["value"]);
    assert_eq!(directory["rows"][0]["id"],ids[0]);
    assert_eq!(profile["numbers"][0]["result"]["run"]["value"],property["value"]);
    assert_eq!(profile["numbers"][0]["result"]["run"]["input_properties"],json!(["price"]));
}

#[tokio::test]
async fn approved_enum_filter_accepts_only_allowed_options() {
    let app=TestApp::new().await;
    let (vault,ids)=fixture(&app).await;
    let out=app.tool("ontology_propose",json!({"vault_id":vault,"properties":[{"kind":"product","name":"colour","datatype":"enum","enum_values":["red","blue"]}]})).await;
    assert!(out.get("error").is_none(),"{out}");
    assert_eq!(app.http_session("POST",&format!("/api/vaults/{vault}/ontology/properties/decide"),Some(json!({"kind":"product","name":"colour","decision":{"action":"approve"}}))).await.0,200);
    let out=app.tool("entity_set_property",json!({"entity_id":ids[0],"property":"colour","value":"blue"})).await;
    assert!(out.get("error").is_none(),"{out}");
    let page=get(&app,&vault,"?kind=product&filter=colour&op=eq&value=blue&sort=colour").await;
    assert_eq!(page["total"],1);
    assert_eq!(page["rows"][0]["id"],ids[0]);
    let (status,_)=app.http("GET",&format!("/api/vaults/{vault}/ontology/objects?kind=product&filter=colour&op=eq&value=green"),None,true).await;
    assert_eq!(status,400);
}

#[tokio::test]
async fn built_in_kind_typed_pages_use_only_allowed_functions() {
    let app=TestApp::new().await;
    let (status,vault)=app.http("POST","/api/vaults",Some(json!({"name":"People scores"})),true).await;
    assert_eq!(status,200,"{vault}");
    let vault=vault["id"].as_str().unwrap();
    let out=app.tool("ontology_propose",json!({"vault_id":vault,"kinds":[{"kind":"person"}],"properties":[{"kind":"person","name":"score","datatype":"number"}]})).await;
    assert!(out.get("error").is_none(),"{out}");
    for (route,body) in [("kinds",json!({"kind":"person","decision":{"action":"approve"}})),("properties",json!({"kind":"person","name":"score","decision":{"action":"approve"}}))] {
        assert_eq!(app.http_session("POST",&format!("/api/vaults/{vault}/ontology/{route}/decide"),Some(body)).await.0,200);
    }
    let (status,person)=app.http("POST","/api/entities",Some(json!({"vault_id":vault,"kind":"person","name":"Mira"})),true).await;
    assert_eq!(status,200,"{person}");
    let out=app.tool("entity_set_property",json!({"entity_id":person["id"],"property":"score","value":5})).await;
    assert!(out.get("error").is_none(),"{out}");
    let page=get(&app,vault,"?kind=person&sort=score&filter=score&op=eq&value=5").await;
    assert_eq!(page["total"],1);
    assert_eq!(page["rows"][0]["id"],person["id"]);
}
