//! The project ontology: what a vault uses shows up observed, agents and the server's model propose,
//! only a vault owner in the browser decides, and decisions steer later writes.

mod common;

use common::{test_settings, TestApp, PASSWORD};
use eunomia_backend::config::Settings;
use eunomia_backend::rid;
use eunomia_backend::sources::mock::{route, serve};
use eunomia_backend::vaults::service as vaults;
use serde_json::{json, Value};

fn ok(v: Value) -> Value {
    assert!(v.get("error").is_none(), "{v}");
    v
}

fn kind<'a>(ontology: &'a Value, name: &str) -> &'a Value {
    ontology["kinds"].as_array().unwrap().iter().find(|k| k["kind"] == name).unwrap_or_else(|| panic!("no kind {name} in {ontology}"))
}

fn link<'a>(ontology: &'a Value, from: &str, label: &str, to: &str) -> &'a Value {
    ontology["links"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["from_kind"] == from && l["label"] == label && l["to_kind"] == to)
        .unwrap_or_else(|| panic!("no link {from} {label} {to} in {ontology}"))
}

async fn write(app: &TestApp, name: &str, kind: &str) -> Value {
    ok(app.tool("memory_write", json!({"subject_name": name, "subject_kind": kind, "text": format!("{name} is a {kind}")})).await)
}

async fn personal(app: &TestApp) -> String {
    ok(app.tool("ontology_get", json!({})).await)["vault"].as_str().unwrap().to_string()
}

/// A decision as the owner's browser session sends it.
async fn decide_kind(app: &TestApp, vault: &str, kind: &str, decision: Value) -> (u16, Value) {
    let (status, body) = app.http_session("POST", &format!("/api/vaults/{vault}/ontology/kinds/decide"), Some(json!({"kind": kind, "decision": decision}))).await;
    (status.as_u16(), body)
}

#[tokio::test]
async fn observed_kinds_and_links_show_with_counts_and_samples() {
    let app = TestApp::new().await;
    let ada = write(&app, "Ada", "person").await;
    let orders = write(&app, "orders_db", "db").await;
    let billing = write(&app, "billing_db", "db").await;
    for db in [&orders, &billing] {
        ok(app.tool("code_relate", json!({"from_id": ada["entity"]["id"], "to_id": db["entity"]["id"], "label": "owns"})).await);
    }

    let o = ok(app.tool("ontology_get", json!({})).await);
    let names: Vec<&str> = o["kinds"].as_array().unwrap().iter().map(|k| k["kind"].as_str().unwrap()).collect();
    assert_eq!(names, ["person", "organisation", "location", "repository", "file", "symbol", "db"], "built-ins first, then custom kinds");
    let person = kind(&o, "person");
    assert_eq!((person["built_in"].as_bool(), person["entities"].as_i64(), person["status"].as_str()), (Some(true), Some(1), Some("observed")));
    assert_eq!(person["samples"], json!(["Ada"]));
    let db = kind(&o, "db");
    assert_eq!((db["built_in"].as_bool(), db["entities"].as_i64(), db["status"].as_str()), (Some(false), Some(2), Some("observed")));
    assert_eq!(db["samples"], json!(["billing_db", "orders_db"]));
    assert_eq!(kind(&o, "symbol")["entities"], 0);

    let owns = link(&o, "person", "owns", "db");
    assert_eq!((owns["count"].as_i64(), owns["status"].as_str()), (Some(2), Some("observed")));
    assert_eq!(owns["samples"].as_array().unwrap().len(), 2, "{owns}");
    assert!(owns["samples"].as_array().unwrap().iter().all(|s| s["from"] == "Ada"), "{owns}");
    assert_eq!(o["links"].as_array().unwrap().len(), 1, "{o}");

    // the same view over REST
    let vault = o["vault"].as_str().unwrap();
    let (status, rest) = app.http("GET", &format!("/api/vaults/{vault}/ontology"), None, true).await;
    assert_eq!(status, 200, "{rest}");
    assert_eq!(rest, o);
}

#[tokio::test]
async fn proposals_wait_for_a_person_and_never_undo_a_decision() {
    let app = TestApp::new().await;
    write(&app, "Ada", "person").await;
    let vault = personal(&app).await;

    let out = ok(app
        .tool(
            "ontology_propose",
            json!({
                "kinds": [{"kind": "customer", "description": "A paying client", "reason": "the vault keeps clients"}],
                "links": [{"from_kind": "person", "label": "works_at", "to_kind": "customer"}],
            }),
        )
        .await);
    assert_eq!(out["kinds"], json!([{"kind": "customer", "status": "proposed", "merge_into": null}]));
    assert_eq!(out["links"][0]["status"], "proposed");
    let o = ok(app.tool("ontology_get", json!({})).await);
    let customer = kind(&o, "customer");
    assert_eq!((customer["status"].as_str(), customer["origin"].as_str(), customer["entities"].as_i64()), (Some("proposed"), Some("agent"), Some(0)));
    assert_eq!(customer["description"], "A paying client");
    assert_eq!(link(&o, "person", "works_at", "customer")["status"], "proposed");

    // a bad kind name fails the whole call, like memory_write
    let bad = app.tool("ontology_propose", json!({"kinds": [{"kind": "Bad Kind"}]})).await;
    assert!(bad["error"].as_str().unwrap_or_default().contains("invalid kind"), "{bad}");

    // the owner approves in the browser
    let (status, o) = decide_kind(&app, &vault, "customer", json!({"action": "approve"})).await;
    assert_eq!(status, 200, "{o}");
    assert_eq!(kind(&o, "customer")["status"], "approved");

    // a later proposal (agent or suggest) does not revert it or rewrite it
    let again = ok(app.tool("ontology_propose", json!({"kinds": [{"kind": "customer", "description": "something else", "merge_into": "organisation"}]})).await);
    assert_eq!(again["kinds"][0]["status"], "approved");
    let suggested = ok(app.tool("ontology_suggest", json!({})).await);
    assert_eq!(suggested["mode"], "profile_only", "{suggested}");
    let o = ok(app.tool("ontology_get", json!({})).await);
    let customer = kind(&o, "customer");
    assert_eq!((customer["status"].as_str(), customer["description"].as_str(), customer["merge_into"].as_str()), (Some("approved"), Some("A paying client"), None));

    // entity_types carries the review: an approved kind is listed before anything uses it
    let types = ok(app.tool("entity_types", json!({})).await);
    assert_eq!(types["custom"], json!([{"kind": "customer", "entities": 0, "status": "approved", "description": "A paying client", "spaces": ["Personal"]}]));

    // the link is approved too, and stays so
    let (status, o) = app
        .http_session("POST", &format!("/api/vaults/{vault}/ontology/links/decide"), Some(json!({"from_kind": "person", "label": "works_at", "to_kind": "customer", "decision": {"action": "approve"}})))
        .await;
    assert_eq!(status, 200, "{o}");
    assert_eq!(link(&o, "person", "works_at", "customer")["status"], "approved");
    ok(app.tool("ontology_propose", json!({"links": [{"from_kind": "person", "label": "works_at", "to_kind": "customer", "description": "x"}]})).await);
    let o = ok(app.tool("ontology_get", json!({})).await);
    assert_eq!(link(&o, "person", "works_at", "customer")["status"], "approved");
}

#[tokio::test]
async fn a_kind_rejected_into_another_sends_later_writes_there() {
    let app = TestApp::new().await;
    write(&app, "Acme", "client").await;
    write(&app, "Globex", "customer").await;
    let vault = personal(&app).await;

    let (status, o) = decide_kind(&app, &vault, "client", json!({"action": "reject", "merge_into": "customer"})).await;
    assert_eq!(status, 200, "{o}");
    let client = kind(&o, "client");
    assert_eq!((client["status"].as_str(), client["merge_into"].as_str()), (Some("rejected"), Some("customer")));

    let out = write(&app, "Initech", "client").await;
    assert_eq!(out["entity"]["kind"], "customer", "{out}");
    assert_eq!(out["kind_mapped"], json!({"from": "client", "to": "customer"}));
    let found = ok(app.tool("entities_search", json!({"query": "Initech"})).await);
    assert_eq!(found["results"][0]["kind"], "customer", "{found}");
    // an unaffected kind reports no mapping
    assert!(write(&app, "Hooli", "customer").await.get("kind_mapped").is_none());

    // the batch writer maps the same way
    let batch = ok(app.tool("memory_write_batch", json!({"items": [{"subject_name": "Umbrella", "subject_kind": "client", "text": "Umbrella is a client"}]})).await);
    assert_eq!(batch["results"][0]["kind_mapped"], json!({"from": "client", "to": "customer"}), "{batch}");

    // and so does the app's own create route, which says so too
    let (status, made) = app.http("POST", "/api/entities", Some(json!({"kind": "client", "name": "Vandelay"})), true).await;
    assert_eq!(status, 200, "{made}");
    assert_eq!((made["kind"].as_str(), made["name"].as_str()), (Some("customer"), Some("Vandelay")), "{made}");
    assert_eq!(made["kind_mapped"], json!({"from": "client", "to": "customer"}));
    let (_, plain) = app.http("POST", "/api/entities", Some(json!({"kind": "customer", "name": "Kramerica"})), true).await;
    assert!(plain.get("kind_mapped").is_none(), "{plain}");

    let types = ok(app.tool("entity_types", json!({})).await);
    let client = types["custom"].as_array().unwrap().iter().find(|k| k["kind"] == "client").unwrap();
    assert_eq!((client["status"].as_str(), client["merge_into"].as_str(), client["entities"].as_i64()), (Some("rejected"), Some("customer"), Some(1)));

    // rejecting the target too carries the merge on, so a merge never points at a rejected kind
    write(&app, "Ann", "person").await;
    let (status, o) = decide_kind(&app, &vault, "customer", json!({"action": "reject", "merge_into": "organisation"})).await;
    assert_eq!(status, 200, "{o}");
    assert_eq!(kind(&o, "client")["merge_into"], "organisation");
    let out = write(&app, "Massive Dynamic", "client").await;
    assert_eq!(out["entity"]["kind"], "organisation", "{out}");
    assert_eq!(out["kind_mapped"], json!({"from": "client", "to": "organisation"}));
}

#[tokio::test]
async fn a_kind_rejected_without_a_target_refuses_writes() {
    let app = TestApp::new().await;
    write(&app, "Use Postgres", "decision").await;
    let vault = personal(&app).await;
    assert_eq!(decide_kind(&app, &vault, "decision", json!({"action": "approve"})).await.0, 200);
    let (status, body) = decide_kind(&app, &vault, "junk", json!({"action": "reject"})).await;
    assert_eq!(status, 200, "{body}");

    let refused = app.tool("memory_write", json!({"subject_name": "x", "subject_kind": "junk", "text": "x"})).await;
    assert_eq!(refused["code"], "validation.invalid", "{refused}");
    let message = refused["error"].as_str().unwrap();
    assert!(message.contains("\"junk\" was rejected") && message.contains("decision") && message.contains("person"), "{message}");
    // the entity creation route answers 400 with the same words
    let (status, body) = app.http("POST", "/api/entities", Some(json!({"kind": "junk", "name": "x"})), true).await;
    assert_eq!(status, 400, "{body}");
    assert!(body.to_string().contains("decision"), "{body}");
    // nothing was written under it
    let o = ok(app.tool("ontology_get", json!({})).await);
    assert_eq!(kind(&o, "junk")["entities"], 0);
}

#[tokio::test]
async fn a_built_in_kind_can_be_described_and_approved_but_not_rejected() {
    let app = TestApp::new().await;
    let vault = personal(&app).await;
    let (status, body) = decide_kind(&app, &vault, "person", json!({"action": "reject", "merge_into": "organisation"})).await;
    assert_eq!(status, 400, "{body}");
    assert!(body.to_string().contains("built-in"), "{body}");

    let (status, o) = decide_kind(&app, &vault, "person", json!({"action": "describe", "description": "Someone we work with"})).await;
    assert_eq!(status, 200, "{o}");
    let person = kind(&o, "person");
    assert_eq!((person["status"].as_str(), person["description"].as_str(), person["origin"].as_str()), (Some("proposed"), Some("Someone we work with"), Some("human")));
    let (status, o) = decide_kind(&app, &vault, "person", json!({"action": "approve"})).await;
    assert_eq!(status, 200, "{o}");
    assert_eq!(kind(&o, "person")["status"], "approved");
    // a write under it is untouched
    assert_eq!(write(&app, "Ada", "person").await["entity"]["kind"], "person");
}

#[tokio::test]
async fn a_merge_target_must_be_a_kind_that_is_kept() {
    let app = TestApp::new().await;
    write(&app, "Acme", "client").await;
    write(&app, "Junk", "junk").await;
    let vault = personal(&app).await;
    assert_eq!(decide_kind(&app, &vault, "junk", json!({"action": "reject"})).await.0, 200);
    for (target, why) in [("client", "itself"), ("nowhere", "not a kind this vault uses"), ("junk", "rejected too"), ("Not A Kind", "invalid kind")] {
        let (status, body) = decide_kind(&app, &vault, "client", json!({"action": "reject", "merge_into": target})).await;
        assert_eq!(status, 400, "{target}: {body}");
        assert!(body.to_string().contains(why), "{target}: {body}");
    }
}

#[tokio::test]
async fn only_a_vault_owner_in_the_browser_decides() {
    let app = TestApp::new().await;
    let bob = common::register(&app.state, "bob@example.com").await;
    let team = ok(app.tool("vault_create", json!({"name": "Team", "kind": "org"})).await);
    let team_id = team["id"].as_str().unwrap().to_string();
    common::sys(async {
        let db = app.db().await;
        let v = rid::parse(&team_id).unwrap();
        vaults::invite_member(&db, app.control(), &app.user.id, &v, &bob.email, "member").await.unwrap();
        vaults::accept_invitation(&db, app.control(), &bob.id, &v).await.unwrap();
    })
    .await;
    ok(app.tool("memory_write", json!({"subject_name": "orders_db", "subject_kind": "db", "text": "orders_db is in Team", "vault_id": "Team"})).await);
    let path = format!("/api/vaults/{team_id}/ontology/kinds/decide");
    let body = json!({"kind": "db", "decision": {"action": "approve"}});

    // a member, in the browser
    let login = json!({"email": bob.email, "password": PASSWORD});
    let (_, cookie) = common::http(&app.router, "POST", "/api/auth/login", Some(login), None, None).await;
    let cookie = cookie.expect("login sets a cookie").split(';').next().unwrap().to_string();
    let ((status, refused), _) = common::http(&app.router, "POST", &path, Some(body.clone()), None, Some(&cookie)).await;
    assert_eq!(status, 403, "{refused}");
    // a member may read the ontology
    let ((status, _), _) = common::http(&app.router, "GET", &format!("/api/vaults/{team_id}/ontology"), None, None, Some(&cookie)).await;
    assert_eq!(status, 200);

    // the owner with a token (an agent may hold one)
    let (status, refused) = app.http("POST", &path, Some(body.clone()), true).await;
    assert_eq!(status, 403, "{refused}");

    // the owner in the browser
    let (status, o) = app.http_session("POST", &path, Some(body)).await;
    assert_eq!(status, 200, "{o}");
    assert_eq!(kind(&o, "db")["status"], "approved");
    // the personal vault's ontology is a separate one
    assert!(ok(app.tool("ontology_get", json!({})).await)["kinds"].as_array().unwrap().iter().all(|k| k["kind"] != "db"));
}

#[tokio::test]
async fn suggest_without_a_model_returns_the_profile_for_the_agent() {
    let app = TestApp::new().await;
    write(&app, "orders_db", "db").await;
    let out = ok(app.tool("ontology_suggest", json!({})).await);
    assert_eq!(out["mode"], "profile_only", "{out}");
    assert!(out["hint"].as_str().unwrap().contains("ontology_propose"), "{out}");
    assert_eq!(kind(&out["profile"], "db")["entities"], 1);
    // nothing was proposed
    assert_eq!(kind(&ok(app.tool("ontology_get", json!({})).await), "db")["status"], "observed");
}

#[tokio::test]
async fn suggest_with_a_model_records_its_proposals_and_converges() {
    let answer = json!({
        "kinds": [
            {"kind": "client", "description": "A company that pays us", "merge_into": "customer", "reason": "client and customer name the same thing"},
            {"kind": "customer", "description": "A paying company", "reason": "no description yet"},
            {"kind": "Not A Kind", "description": "dropped"},
            {"kind": "repository", "description": "dropped: a built-in kind this vault never used"},
        ],
        "links": [{"from_kind": "person", "label": "works_at", "to_kind": "customer", "description": "Where someone works", "reason": "used"}],
    });
    let completion = json!({"choices": [{"message": {"content": answer.to_string()}}]});
    let mock = serve(vec![route("POST", "/chat/completions", completion)]).await;
    let settings = Settings { openai_base_url: mock.base.clone(), openai_api_key: Some("sk-test".into()), openai_chat_model: "test-model".into(), ..test_settings() };
    let app = TestApp::with_settings(settings).await;
    write(&app, "Acme", "client").await;
    write(&app, "Globex", "customer").await;
    let vault = personal(&app).await;
    assert_eq!(decide_kind(&app, &vault, "customer", json!({"action": "approve"})).await.0, 200);

    let out = ok(app.tool("ontology_suggest", json!({})).await);
    assert_eq!((out["mode"].as_str(), out["skipped"].as_i64()), (Some("model"), Some(2)), "{out}");
    let o = &out["ontology"];
    let client = kind(o, "client");
    assert_eq!(
        (client["status"].as_str(), client["origin"].as_str(), client["merge_into"].as_str(), client["description"].as_str()),
        (Some("proposed"), Some("suggested"), Some("customer"), Some("A company that pays us"))
    );
    // the approved kind keeps its (empty) description: a suggestion never rewrites a decision
    assert_eq!((kind(o, "customer")["status"].as_str(), kind(o, "customer")["description"].as_str()), (Some("approved"), None));
    assert_eq!(link(o, "person", "works_at", "customer")["status"], "proposed");
    assert_eq!(kind(o, "repository")["status"], "observed");
    // a proposed merge does not move writes yet
    assert_eq!(write(&app, "Initech", "client").await["entity"]["kind"], "client");

    // the model saw the profile as data, with the vault's kinds and names in it
    let sent: Vec<_> = mock.requests().into_iter().filter(|r| r.path == "/chat/completions").collect();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].body.contains("<profile>") && sent[0].body.contains("Acme") && sent[0].body.contains("test-model"), "{}", sent[0].body);
    assert!(!sent[0].body.contains("symbol"), "unused built-in kinds stay out of the profile: {}", sent[0].body);

    // running it again converges: same rows, same statuses
    let again = ok(app.tool("ontology_suggest", json!({})).await);
    let (a, b) = (&out["ontology"]["kinds"], &again["ontology"]["kinds"]);
    let statuses = |v: &Value| v.as_array().unwrap().iter().map(|k| (k["kind"].clone(), k["status"].clone(), k["merge_into"].clone())).collect::<Vec<_>>();
    assert_eq!(statuses(a), statuses(b));
    assert_eq!(again["ontology"]["links"].as_array().unwrap().len(), out["ontology"]["links"].as_array().unwrap().len());
}

#[tokio::test]
async fn writes_still_work_on_an_org_database_before_the_migration() {
    let app = TestApp::new().await;
    app.db().await.test_raw().query("REMOVE TABLE ontology_kind").await.unwrap().check().unwrap();
    assert_eq!(write(&app, "orders_db", "db").await["entity"]["kind"], "db");
    let types = ok(app.tool("entity_types", json!({})).await);
    assert_eq!(types["custom"], json!([{"kind": "db", "entities": 1, "status": "observed", "spaces": ["Personal"]}]));
}


#[tokio::test]
async fn the_ontology_view_is_observed_only_on_an_org_database_before_the_migration() {
    let app = TestApp::new().await;
    let latte = write(&app, "Latte", "product").await["entity"]["id"].as_str().unwrap().to_string();
    let cafe = write(&app, "Bean Bar", "cafe").await["entity"]["id"].as_str().unwrap().to_string();
    ok(app.tool("code_relate", json!({"from_id": cafe, "to_id": latte, "label": "sells"})).await);
    app.db()
        .await
        .test_raw()
        .query("REMOVE TABLE ontology_kind; REMOVE TABLE ontology_link; REMOVE TABLE entity_property; REMOVE TABLE ontology_property; REMOVE TABLE ontology_metric")
        .await
        .unwrap()
        .check()
        .unwrap();
    let view = ok(app.tool("ontology_get", json!({})).await);
    assert_eq!((kind(&view, "product")["status"].as_str(), kind(&view, "product")["entities"].as_i64()), (Some("observed"), Some(1)), "{view}");
    assert_eq!(link(&view, "cafe", "sells", "product")["status"], "observed");
    let (status, body) = app.http("GET", &format!("/api/vaults/{}/ontology", view["vault"].as_str().unwrap()), None, true).await;
    assert_eq!(status, 200, "{body}");
    let near = ok(app.tool("entities_neighbourhood", json!({"entity_id": cafe})).await);
    assert_eq!(near["link_types"][0]["status"], "observed", "{near}");
}

#[tokio::test]
async fn reads_and_writes_still_work_on_an_org_database_before_the_properties_migration() {
    let app = TestApp::new().await;
    let latte = write(&app, "Latte", "product").await["entity"]["id"].as_str().unwrap().to_string();
    let cafe = write(&app, "Bean Bar", "cafe").await["entity"]["id"].as_str().unwrap().to_string();
    app.db().await.test_raw().query("REMOVE TABLE entity_property; REMOVE TABLE ontology_property").await.unwrap().check().unwrap();
    ok(app.tool("code_relate", json!({"from_id": cafe, "to_id": latte, "label": "sells"})).await);
    assert_eq!(ok(app.tool("entities_get", json!({"id": latte})).await)["properties"], json!([]));
    ok(app.tool("entities_neighbourhood", json!({"entity_id": cafe})).await);
    assert_eq!(kind(&ok(app.tool("ontology_get", json!({})).await), "product")["properties"], json!([]));
    let refused = app.tool("entity_set_property", json!({"entity_id": latte, "property": "price", "value": 4.5})).await;
    assert!(refused["error"].as_str().unwrap_or_default().contains("does not exist"), "{refused}");
    ok(app.tool("entity_delete", json!({"entity_id": latte})).await);
}

#[tokio::test]
async fn a_kind_row_from_before_synonyms_still_takes_decisions_after_the_upgrade() {
    let app = TestApp::new().await;
    ok(app.tool("ontology_propose", json!({"kinds": [{"kind": "customer", "description": "A paying client"}]})).await);
    let db = app.db().await;
    // the row as 0033 left it: no synonyms field at all
    db.test_raw().query("REMOVE FIELD synonyms ON ontology_kind; UPDATE ontology_kind UNSET synonyms;").await.unwrap().check().unwrap();
    db.test_raw().query(include_str!("../migrations/tenant/0034_ontology_properties.surql")).await.unwrap().check().unwrap();
    let vault = personal(&app).await;
    let (status, o) = decide_kind(&app, &vault, "customer", json!({"action": "approve"})).await;
    assert_eq!(status, 200, "{o}");
    assert_eq!((kind(&o, "customer")["status"].as_str(), kind(&o, "customer")["synonyms"].clone()), (Some("approved"), json!([])));
}

#[tokio::test]
async fn a_property_row_from_before_source_maps_still_takes_proposals_after_the_upgrade() {
    let app = TestApp::new().await;
    ok(app.tool("ontology_propose", json!({"properties": [price("money")]})).await);
    let db = app.db().await;
    // the row as 0034 left it: no source_map field at all
    db.test_raw()
        .query("REMOVE FIELD source_map[*].source ON ontology_property; REMOVE FIELD source_map[*].record_type ON ontology_property; REMOVE FIELD source_map[*].path ON ontology_property; REMOVE FIELD source_map ON ontology_property; UPDATE ontology_property UNSET source_map;")
        .await
        .unwrap()
        .check()
        .unwrap();
    db.test_raw().query(include_str!("../migrations/tenant/0036_ontology_automap.surql")).await.unwrap().check().unwrap();
    // a decision updates the old row: it fails on a typed field left NONE
    let vault = personal(&app).await;
    let (status, body) = decide_property(&app, &vault, "product", "price", json!({"action": "describe", "description": "What one costs"})).await;
    assert_eq!(status, 200, "{body}");
    let mapped = json!({"source": "document", "record_type": "document:abc", "path": "price AUD"});
    let again = ok(app.tool("ontology_propose", json!({"properties": [{"kind": "product", "name": "price", "datatype": "money", "source_map": [mapped.clone()]}]})).await);
    assert_eq!(again["properties"][0]["status"], "proposed");
    let o = ok(app.tool("ontology_get", json!({})).await);
    assert_eq!(property(&o, "product", "price")["source_map"], json!([mapped]));
}

#[tokio::test]
async fn a_link_row_from_before_link_sources_still_takes_decisions_and_columns_after_the_upgrade() {
    let app = TestApp::new().await;
    ok(app.tool("ontology_propose", json!({"links": [{"from_kind": "product", "label": "supplied_by", "to_kind": "organisation"}]})).await);
    let db = app.db().await;
    // the row as 0037 left it: no source_map field at all
    db.test_raw()
        .query("REMOVE FIELD source_map[*].source ON ontology_link; REMOVE FIELD source_map[*].record_type ON ontology_link; REMOVE FIELD source_map[*].path ON ontology_link; REMOVE FIELD source_map ON ontology_link; UPDATE ontology_link UNSET source_map;")
        .await
        .unwrap()
        .check()
        .unwrap();
    // an agent's proposal, with no columns, still writes before the migration (a rolling upgrade)
    ok(app.tool("ontology_propose", json!({"links": [{"from_kind": "product", "label": "supplied_by", "to_kind": "organisation", "reason": "again"}]})).await);
    db.test_raw().query(include_str!("../migrations/tenant/0038_ontology_link_source.surql")).await.unwrap().check().unwrap();
    let vault = personal(&app).await;
    let body = json!({"from_kind": "product", "label": "supplied_by", "to_kind": "organisation", "decision": {"action": "describe", "description": "Who sells it to us"}});
    let (status, body) = app.http_session("POST", &format!("/api/vaults/{vault}/ontology/links/decide"), Some(body)).await;
    assert_eq!(status, 200, "{body}");
    let mapped = json!({"source": "document", "record_type": "document:abc", "path": "supplier"});
    ok(app.tool("ontology_propose", json!({"links": [{"from_kind": "product", "label": "supplied_by", "to_kind": "organisation", "source_map": [mapped.clone()]}]})).await);
    let o = ok(app.tool("ontology_get", json!({})).await);
    assert_eq!(link(&o, "product", "supplied_by", "organisation")["source_map"], json!([mapped]));
}

// ------------------------------------------------------------ what a change affects: the neighbourhood

/// A small logistics chain in the personal vault: Ana drives Truck 7, which carries Shipment 88, which
/// fulfils Order 12, which is for Acme. Returns the entity ids by name.
async fn logistics(app: &TestApp) -> std::collections::HashMap<&'static str, String> {
    let mut ids = std::collections::HashMap::new();
    for (name, kind) in [("Ana", "person"), ("Truck 7", "truck"), ("Shipment 88", "shipment"), ("Order 12", "order"), ("Acme", "organisation")] {
        ids.insert(name, write(app, name, kind).await["entity"]["id"].as_str().unwrap().to_string());
    }
    for (from, label, to) in [("Ana", "drives", "Truck 7"), ("Truck 7", "carries", "Shipment 88"), ("Shipment 88", "fulfils", "Order 12"), ("Order 12", "for", "Acme")] {
        ok(app.tool("code_relate", json!({"from_id": ids[from], "to_id": ids[to], "label": label})).await);
    }
    ids
}

fn names(n: &Value) -> Vec<String> {
    let mut v: Vec<String> = n["nodes"].as_array().unwrap().iter().map(|n| n["name"].as_str().unwrap().to_string()).collect();
    v.sort();
    v
}

async fn as_user(app: &TestApp, user: &eunomia_backend::models_user::User, tool: &str, args: Value) -> Value {
    eunomia_backend::authz::as_system(eunomia_backend::tools::registry::call(&app.state, user, tool, args)).await.unwrap()
}

/// The names of the statements run on this thread while it is held (each query's `store.query` span).
#[derive(Clone, Default)]
struct Ops(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Ops {
    fn on_new_span(&self, attrs: &tracing::span::Attributes<'_>, _: &tracing::span::Id, _: tracing_subscriber::layer::Context<'_, S>) {
        struct Op(Option<String>);
        impl tracing::field::Visit for Op {
            fn record_str(&mut self, f: &tracing::field::Field, v: &str) {
                if f.name() == "op" {
                    self.0 = Some(v.to_string());
                }
            }
            fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
                if f.name() == "op" {
                    self.0 = Some(format!("{v:?}").trim_matches('"').to_string());
                }
            }
        }
        let mut op = Op(None);
        attrs.record(&mut op);
        if let Some(op) = op.0 {
            self.0.lock().unwrap().push(op);
        }
    }
}

#[tokio::test]
async fn the_neighbourhood_reads_only_the_link_types_it_found_not_the_whole_vault() {
    use tracing_subscriber::layer::SubscriberExt;
    let app = TestApp::new().await;
    let ids = logistics(&app).await;
    let vault = personal(&app).await;
    let (status, _) = app.http_session("POST", &format!("/api/vaults/{vault}/ontology/links/decide"), Some(json!({"from_kind": "truck", "label": "carries", "to_kind": "shipment", "decision": {"action": "approve"}}))).await;
    assert_eq!(status, 200);
    let ops = Ops::default();
    let n = {
        let _held = tracing::subscriber::set_default(tracing_subscriber::registry().with(ops.clone()));
        ok(app.tool("entities_neighbourhood", json!({"entity_id": ids["Truck 7"], "depth": 1})).await)
    };
    let ran = ops.0.lock().unwrap().clone();
    assert!(ran.iter().any(|o| o == "ontology.links_labelled"), "{ran:?}");
    assert!(!ran.iter().any(|o| o == "ontology.profile"), "the vault-wide profile is not on the chat's path: {ran:?}");
    let types: Vec<(String, String)> = n["link_types"].as_array().unwrap().iter().map(|l| (l["label"].as_str().unwrap().to_string(), l["status"].as_str().unwrap().to_string())).collect();
    assert_eq!(types, [("drives".to_string(), "observed".to_string()), ("carries".to_string(), "approved".to_string())], "{n}");
}

#[tokio::test]
async fn the_neighbourhood_follows_relations_both_ways_up_to_the_depth() {
    let app = TestApp::new().await;
    let ids = logistics(&app).await;
    for i in 1..=4 {
        ok(app.tool("memory_write", json!({"subject_name": "Truck 7", "subject_kind": "truck", "text": format!("Truck 7 report {i}")})).await);
    }

    // default depth 2 from the truck: one hop back to its driver, forward to the shipment, then the order
    let n = ok(app.tool("entities_neighbourhood", json!({"entity_id": ids["Truck 7"]})).await);
    assert_eq!(n["depth"], 2);
    assert_eq!(names(&n), ["Ana", "Order 12", "Shipment 88", "Truck 7"], "{n}");
    assert_eq!(n["nodes"][0]["name"], "Truck 7", "the start comes first");
    assert_eq!(n["truncated"], false);
    let edges: Vec<(String, String, String)> = n["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            let name = |id: &Value| ids.iter().find(|(_, v)| *v == id).unwrap().0.to_string();
            (name(&e["from"]), e["label"].as_str().unwrap().to_string(), name(&e["to"]))
        })
        .collect();
    for want in [("Ana", "drives", "Truck 7"), ("Truck 7", "carries", "Shipment 88"), ("Shipment 88", "fulfils", "Order 12")] {
        assert!(edges.contains(&(want.0.into(), want.1.into(), want.2.into())), "stored direction kept: {edges:?}");
    }
    assert_eq!(edges.len(), 3, "{edges:?}");
    let truck = &n["nodes"][0];
    assert_eq!(truck["kind"], "truck");
    let facts: Vec<&str> = truck["facts"].as_array().unwrap().iter().map(|f| f.as_str().unwrap()).collect();
    assert_eq!(facts.len(), 3, "the three most recent facts: {facts:?}");
    assert!(facts.contains(&"Truck 7 report 4") && !facts.contains(&"Truck 7 report 1"), "{facts:?}");

    // the depth clamps to 1..=3
    let deep = ok(app.tool("entities_neighbourhood", json!({"entity_id": ids["Truck 7"], "depth": 9})).await);
    assert_eq!((deep["depth"].as_i64(), names(&deep).len()), (Some(3), 5), "{deep}");
    let shallow = ok(app.tool("entities_neighbourhood", json!({"entity_id": ids["Truck 7"], "depth": 0})).await);
    assert_eq!(names(&shallow), ["Ana", "Shipment 88", "Truck 7"], "{shallow}");

    // the link types present, with the vault's review of each
    ok(app.tool("ontology_propose", json!({"links": [{"from_kind": "truck", "label": "carries", "to_kind": "shipment", "description": "A late truck delays what it carries"}]})).await);
    let n = ok(app.tool("entities_neighbourhood", json!({"entity_id": ids["Truck 7"], "depth": 1})).await);
    let types = n["link_types"].as_array().unwrap();
    assert_eq!(types.len(), 2, "only the types among these edges: {n}");
    let carries = types.iter().find(|l| l["label"] == "carries").unwrap();
    assert_eq!(
        (carries["from_kind"].as_str(), carries["to_kind"].as_str(), carries["status"].as_str(), carries["description"].as_str()),
        (Some("truck"), Some("shipment"), Some("proposed"), Some("A late truck delays what it carries"))
    );
    assert_eq!(types.iter().find(|l| l["label"] == "drives").unwrap()["status"], "observed");
}

#[tokio::test]
async fn the_neighbourhood_never_leaves_the_vault_or_opens_one_the_caller_cannot_read() {
    let app = TestApp::new().await;
    let ids = logistics(&app).await;
    let team = ok(app.tool("vault_create", json!({"name": "Team", "kind": "org"})).await);
    let depot = ok(app.tool("memory_write", json!({"subject_name": "Depot", "subject_kind": "location", "text": "Depot is shared", "vault_id": "Team"})).await);
    let depot_id = depot["entity"]["id"].as_str().unwrap().to_string();
    // an edge from before relations were confined to one vault, into the Team vault
    app.db()
        .await
        .test_raw()
        .query("RELATE $a->relates_to->$b SET label = 'parks_at'")
        .bind(("a", rid::parse(&ids["Truck 7"]).unwrap()))
        .bind(("b", rid::parse(&depot_id).unwrap()))
        .await
        .unwrap()
        .check()
        .unwrap();
    let n = ok(app.tool("entities_neighbourhood", json!({"entity_id": ids["Truck 7"], "depth": 3})).await);
    assert!(!names(&n).contains(&"Depot".to_string()), "{n}");
    assert!(n["edges"].as_array().unwrap().iter().all(|e| e["label"] != "parks_at"), "{n}");
    assert!(!n.to_string().contains(&depot_id), "{n}");
    let from_depot = ok(app.tool("entities_neighbourhood", json!({"entity_id": depot_id})).await);
    assert_eq!(names(&from_depot), ["Depot"], "{from_depot}");

    // the entity must be in the vault named
    let wrong = app.tool("entities_neighbourhood", json!({"entity_id": ids["Truck 7"], "vault_id": "Team"})).await;
    assert_eq!(wrong["code"], "entity.not_found", "{wrong}");
    assert_eq!(ok(app.tool("entities_neighbourhood", json!({"entity_id": depot_id, "vault_id": team["id"]})).await)["nodes"][0]["name"], "Depot");

    // someone outside the personal vault gets not found, not the truck
    let bob = common::register(&app.state, "bob@example.com").await;
    assert_eq!(bob.org, app.user.org);
    let out = as_user(&app, &bob, "entities_neighbourhood", json!({"entity_id": ids["Truck 7"]})).await;
    assert_eq!(out["code"], "entity.not_found", "{out}");
    assert!(!out.to_string().contains("Truck 7"), "{out}");
}

/// `n` relations from `from`, labelled `label_0`, `label_1`, ..., each to `to` or, without one, to a new
/// entity of its own.
async fn fan_out(app: &TestApp, from: &str, label: &str, n: usize, to: Option<&str>) {
    app.db()
        .await
        .test_raw()
        .query(
            "LET $h = (SELECT owner, vault FROM ONLY $from);
             FOR $i IN 0..$n {
                 LET $t = $to ?? (CREATE ONLY entity SET owner = $h.owner, vault = $h.vault, kind = 'spoke', name = 'Spoke ' + <string> $i).id;
                 RELATE $from->relates_to->$t SET label = $label + '_' + <string> $i;
             };",
        )
        .bind(("from", rid::parse(from).unwrap()))
        .bind(("to", to.map(|t| rid::parse(t).unwrap())))
        .bind(("n", n as i64))
        .bind(("label", label.to_string()))
        .await
        .unwrap()
        .check()
        .unwrap();
}

#[tokio::test]
async fn a_neighbourhood_around_a_hub_is_capped_and_says_so() {
    use eunomia_backend::entities::neighbourhood::{MAX_EDGES, MAX_EDGES_PER_ENTITY, MAX_NODES};
    let app = TestApp::new().await;
    let id = |v: Value| v["entity"]["id"].as_str().unwrap().to_string();
    let edges = |n: &Value| n["edges"].as_array().unwrap().len();

    // more neighbours than the node cap: every edge still points at a node that came back
    let hub = id(write(&app, "Hub", "hub").await);
    fan_out(&app, &hub, "feeds", MAX_NODES + 50, None).await;
    let n = ok(app.tool("entities_neighbourhood", json!({"entity_id": hub, "depth": 1})).await);
    assert_eq!((n["nodes"].as_array().unwrap().len(), edges(&n), n["truncated"].as_bool()), (MAX_NODES, MAX_NODES - 1, Some(true)));

    // more relations from one entity than are read each way
    let (busy, other) = (id(write(&app, "Busy", "hub").await), id(write(&app, "Other", "hub").await));
    fan_out(&app, &busy, "pings", MAX_EDGES_PER_ENTITY + 100, Some(&other)).await;
    let n = ok(app.tool("entities_neighbourhood", json!({"entity_id": busy, "depth": 1})).await);
    assert_eq!((names(&n), edges(&n), n["truncated"].as_bool()), (vec!["Busy".to_string(), "Other".to_string()], MAX_EDGES_PER_ENTITY, Some(true)));

    // under the per-entity cap everywhere, over the total: start -> a, b; a and b each -> c many times
    let mut chain = Vec::new();
    for name in ["Start", "A", "B", "C"] {
        chain.push(id(write(&app, name, "hub").await));
    }
    let [start, a, b, c] = <[String; 4]>::try_from(chain).unwrap();
    fan_out(&app, &start, "uses", 1, Some(&a)).await;
    fan_out(&app, &start, "uses", 1, Some(&b)).await;
    let each = MAX_EDGES / 2 + 100;
    assert!(each < MAX_EDGES_PER_ENTITY);
    fan_out(&app, &a, "calls", each, Some(&c)).await;
    fan_out(&app, &b, "calls", each, Some(&c)).await;
    let n = ok(app.tool("entities_neighbourhood", json!({"entity_id": start, "depth": 3})).await);
    assert_eq!((names(&n).len(), edges(&n), n["truncated"].as_bool()), (4, MAX_EDGES, Some(true)), "{}", n["nodes"]);

    // within every cap nothing is cut
    let n = ok(app.tool("entities_neighbourhood", json!({"entity_id": start, "depth": 1})).await);
    assert_eq!((edges(&n), n["truncated"].as_bool()), (2, Some(false)));
}

// ------------------------------------------------------------ what a change affects: the view

/// The relation ids among the logistics chain, by label.
async fn edges_by_label(app: &TestApp, from: &str) -> std::collections::HashMap<String, String> {
    let n = ok(app.tool("entities_neighbourhood", json!({"entity_id": from, "depth": 3})).await);
    n["edges"].as_array().unwrap().iter().map(|e| (e["label"].as_str().unwrap().to_string(), e["id"].as_str().unwrap().to_string())).collect()
}

fn impact(start: &str, affected: Value) -> Value {
    json!({"view": "impact", "start_id": start, "headline": "  Truck 7 is two hours late.  ", "affected": affected})
}

fn refused(out: &Value, needle: &str) {
    assert_eq!(out["code"], "validation.invalid", "{out}");
    assert!(out["error"].as_str().unwrap_or_default().contains(needle), "the error names {needle}: {out}");
}

#[tokio::test]
async fn an_impact_view_comes_back_resolved_with_names_and_text() {
    let app = TestApp::new().await;
    let ids = logistics(&app).await;
    let rel = edges_by_label(&app, &ids["Truck 7"]).await;
    let token = format!("ghp_{}", "a1B2c3D4e5".repeat(4));
    let out = ok(app
        .tool(
            "show_view",
            impact(
                &ids["Truck 7"],
                json!([
                    {"entity_id": ids["Shipment 88"], "path": [rel["carries"]], "why": "It is on the truck."},
                    {"entity_id": ids["Order 12"], "path": [rel["carries"], rel["fulfils"]], "why": format!("Its shipment is late. {token}")},
                    // against the stored direction: Ana drives the truck
                    {"entity_id": ids["Ana"], "path": [rel["drives"]], "why": "Her shift runs over."},
                ]),
            ),
        )
        .await);
    assert_eq!((out["view"].as_str(), out["headline"].as_str()), (Some("impact"), Some("Truck 7 is two hours late.")), "{out}");
    assert_eq!(out["start"], json!({"id": ids["Truck 7"], "name": "Truck 7", "kind": "truck", "vault": personal(&app).await}));
    let order = &out["affected"][1];
    assert_eq!((order["entity"]["name"].as_str(), order["entity"]["kind"].as_str()), (Some("Order 12"), Some("order")));
    let steps: Vec<(String, String, String)> = order["path"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["from"]["name"].as_str().unwrap().into(), s["label"].as_str().unwrap().into(), s["to"]["name"].as_str().unwrap().into()))
        .collect();
    assert_eq!(steps, [("Truck 7".into(), "carries".into(), "Shipment 88".into()), ("Shipment 88".into(), "fulfils".into(), "Order 12".into())]);
    let ana = &out["affected"][2]["path"][0];
    assert_eq!((ana["from"]["name"].as_str(), ana["to"]["name"].as_str()), (Some("Ana"), Some("Truck 7")), "the stored direction is kept");
    assert!(!out.to_string().contains(&token), "secrets are dropped from the model's text: {out}");
    let text = out["text"].as_str().unwrap();
    assert!(text.starts_with("Truck 7 is two hours late.\n"), "{text}");
    assert!(text.contains("Order 12 (order), via Truck 7 carries Shipment 88, then Shipment 88 fulfils Order 12."), "{text}");

    let sub = ok(app
        .tool("show_view", json!({"view": "subgraph", "title": "How Ana reaches Acme", "entity_ids": [ids["Ana"], ids["Ana"]], "relation_ids": [rel["drives"], rel["for"], rel["drives"]]}))
        .await);
    let names: Vec<&str> = sub["entities"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Ana", "Truck 7", "Order 12", "Acme"], "a relation's ends are shown too, a repeated id once: {sub}");
    assert_eq!(sub["relations"].as_array().unwrap().len(), 2, "a repeated relation is shown once: {sub}");
    assert!(sub["text"].as_str().unwrap().contains("- Order 12 for Acme"), "{sub}");
}

#[tokio::test]
async fn a_view_with_an_invented_id_or_a_broken_path_is_refused_naming_it() {
    let app = TestApp::new().await;
    let ids = logistics(&app).await;
    let rel = edges_by_label(&app, &ids["Truck 7"]).await;
    let truck = &ids["Truck 7"];
    let shipment = |path: Value| impact(truck, json!([{"entity_id": ids["Shipment 88"], "path": path, "why": "x"}]));

    refused(&app.tool("show_view", impact("entity:invented1", json!([]))).await, "entity:invented1");
    let out = app.tool("show_view", impact(truck, json!([{"entity_id": "entity:invented2", "path": [rel["carries"]], "why": "x"}]))).await;
    refused(&out, "entity:invented2");
    refused(&app.tool("show_view", shipment(json!(["relates_to:invented3"]))).await, "relates_to:invented3");
    refused(&app.tool("show_view", shipment(json!([ids["Order 12"]]))).await, &ids["Order 12"]);
    // the first relation must touch the start
    let out = app.tool("show_view", impact(truck, json!([{"entity_id": ids["Order 12"], "path": [rel["fulfils"]], "why": "x"}]))).await;
    refused(&out, "does not start at");
    // each next relation must touch where the chain got to
    let out = app.tool("show_view", impact(truck, json!([{"entity_id": ids["Acme"], "path": [rel["carries"], rel["for"]], "why": "x"}]))).await;
    refused(&out, &format!("broken at relation {}", rel["for"]));
    // and it must end at the entity it explains
    let out = app.tool("show_view", impact(truck, json!([{"entity_id": ids["Order 12"], "path": [rel["carries"]], "why": "x"}]))).await;
    refused(&out, "ends at");
    refused(&app.tool("show_view", shipment(json!([]))).await, "is empty");
    // a chain that goes round in circles is not a path: no relation twice, no entity twice
    let out = app.tool("show_view", shipment(json!([rel["carries"], rel["carries"], rel["carries"]]))).await;
    refused(&out, &format!("uses relation {} twice", rel["carries"]));
    let rides = ok(app.tool("code_relate", json!({"from_id": ids["Shipment 88"], "to_id": truck, "label": "rides_on"})).await);
    let rides = rides["id"].as_str().unwrap().to_string();
    let out = app.tool("show_view", impact(truck, json!([{"entity_id": ids["Order 12"], "path": [rel["carries"], rides, rel["carries"], rel["fulfils"]], "why": "x"}]))).await;
    refused(&out, &format!("comes back to {truck} at relation {rides}"));
    // each affected entity once
    let twice = json!([
        {"entity_id": ids["Shipment 88"], "path": [rel["carries"]], "why": "x"},
        {"entity_id": ids["Shipment 88"], "path": [rel["carries"]], "why": "y"},
    ]);
    refused(&app.tool("show_view", impact(truck, twice)).await, &format!("{} is listed twice", ids["Shipment 88"]));
    // the caps, before any id is looked up
    let many: Vec<Value> = (0..=eunomia_backend::views::MAX_AFFECTED).map(|i| json!({"entity_id": format!("entity:many{i}"), "path": ["relates_to:x"], "why": "x"})).collect();
    refused(&app.tool("show_view", impact(truck, json!(many))).await, &format!("at most {} affected", eunomia_backend::views::MAX_AFFECTED));
    let long = vec![rel["carries"].clone(); eunomia_backend::views::MAX_PATH + 1];
    let out = app.tool("show_view", impact(truck, json!([{"entity_id": ids["Order 12"], "path": long, "why": "x"}]))).await;
    refused(&out, &format!("the path to {} has more than {} relations", ids["Order 12"], eunomia_backend::views::MAX_PATH));
    // an unknown view is a bad argument, not a new kind of answer
    let out = app.tool("show_view", json!({"view": "pie_chart", "title": "x"})).await;
    assert!(out["error"].as_str().unwrap().contains("unknown variant"), "{out}");
    // the arguments are the view itself: a view nested under `view` is not one
    let nested = app.tool("show_view", json!({"view": impact(truck, json!([]))})).await;
    assert!(nested["error"].as_str().unwrap().contains("invalid arguments"), "{nested}");
}

#[tokio::test]
async fn a_view_cannot_name_an_entity_the_caller_cannot_read() {
    let app = TestApp::new().await;
    let ids = logistics(&app).await;
    let bob = common::register(&app.state, "bob@example.com").await;
    assert_eq!(bob.org, app.user.org, "same database: only the vault check keeps Bob's entity out");
    let secret = ok(as_user(&app, &bob, "memory_write", json!({"subject_name": "Bob Private Depot", "subject_kind": "location", "text": "only Bob sees this"})).await);
    let hidden = secret["entity"]["id"].as_str().unwrap();
    let hood = ok(as_user(&app, &bob, "entities_neighbourhood", json!({"entity_id": hidden})).await);
    assert_eq!(hood["nodes"][0]["name"], "Bob Private Depot");

    for view in [impact(hidden, json!([])), json!({"view": "subgraph", "title": "x", "entity_ids": [ids["Ana"], hidden]})] {
        let out = app.tool("show_view", view).await;
        refused(&out, hidden);
        assert!(!out.to_string().contains("Bob Private Depot"), "{out}");
    }
}

#[tokio::test]
async fn a_view_cannot_show_a_relation_across_vaults_even_when_both_ends_are_readable() {
    let app = TestApp::new().await;
    let ids = logistics(&app).await;
    ok(app.tool("vault_create", json!({"name": "Team", "kind": "org"})).await);
    let depot = ok(app.tool("memory_write", json!({"subject_name": "Depot", "subject_kind": "location", "text": "Depot is shared", "vault_id": "Team"})).await);
    let depot_id = depot["entity"]["id"].as_str().unwrap().to_string();
    // an edge from before relations were confined to one vault
    let mut res = app
        .db()
        .await
        .test_raw()
        .query("LET $r = RELATE $a->relates_to->$b SET label = 'parks_at'; RETURN <string> $r[0].id;")
        .bind(("a", rid::parse(&ids["Truck 7"]).unwrap()))
        .bind(("b", rid::parse(&depot_id).unwrap()))
        .await
        .unwrap();
    let parks_at: String = res.take::<Option<String>>(1).unwrap().unwrap();
    let both = ok(app.tool("show_view", json!({"view": "subgraph", "title": "x", "entity_ids": [ids["Truck 7"], depot_id]})).await);
    assert_eq!(both["entities"].as_array().unwrap().len(), 2, "both ends are readable: {both}");

    let sub = app.tool("show_view", json!({"view": "subgraph", "title": "x", "entity_ids": [ids["Truck 7"]], "relation_ids": [parks_at]})).await;
    refused(&sub, &format!("unknown relation {parks_at}"));
    let out = app.tool("show_view", impact(&ids["Truck 7"], json!([{"entity_id": depot_id, "path": [parks_at], "why": "x"}]))).await;
    refused(&out, &format!("unknown relation {parks_at}"));
}

#[tokio::test]
async fn over_mcp_a_view_is_its_text_plus_the_structured_view() {
    let app = TestApp::new().await;
    let ids = logistics(&app).await;
    let rel = edges_by_label(&app, &ids["Truck 7"]).await;
    let mcp = |name: &str, args: Value| json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}});

    let args = impact(&ids["Truck 7"], json!([{"entity_id": ids["Shipment 88"], "path": [rel["carries"]], "why": "It is on the truck."}]));
    let (status, rpc) = app.http("POST", "/mcp", Some(mcp("show_view", args)), true).await;
    assert_eq!(status, 200, "{rpc}");
    let result = &rpc["result"];
    assert_eq!(result["isError"], false, "{rpc}");
    let text = result["content"][0]["text"].as_str().unwrap();
    assert_eq!(text, "Truck 7 is two hours late.\nStarting from Truck 7 (truck).\n1. Shipment 88 (shipment), via Truck 7 carries Shipment 88. It is on the truck.");
    let view = &result["structuredContent"];
    assert_eq!((view["view"].as_str(), view["affected"][0]["entity"]["name"].as_str()), (Some("impact"), Some("Shipment 88")), "{view}");
    assert_eq!(view["text"], text);

    // a refused view is an ordinary tool error; other tools keep their JSON text and no structure
    let (_, rpc) = app.http("POST", "/mcp", Some(mcp("show_view", impact("entity:invented", json!([])))), true).await;
    assert_eq!(rpc["result"]["isError"], true, "{rpc}");
    assert!(rpc["result"]["content"][0]["text"].as_str().unwrap().contains("entity:invented"), "{rpc}");
    assert!(rpc["result"].get("structuredContent").is_none(), "{rpc}");
    let (_, rpc) = app.http("POST", "/mcp", Some(mcp("entities_neighbourhood", json!({"entity_id": ids["Truck 7"]}))), true).await;
    let hood: Value = serde_json::from_str(rpc["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(hood["nodes"][0]["name"], "Truck 7");
    assert!(rpc["result"].get("structuredContent").is_none(), "{rpc}");
}

// ------------------------------------------------------------ what a change affects: the in-app chat

/// One chat message as the browser sends it, in a new thread; the SSE events that came back.
async fn chat(app: &TestApp, message: &str) -> (String, Vec<Value>) {
    let (_, thread) = app.http("POST", "/api/chat/threads", Some(json!({})), true).await;
    let thread_id = thread["id"].as_str().unwrap().to_string();
    let events = chat_in(app, &thread_id, message).await;
    (thread_id, events)
}

/// One chat message in `thread_id`; the SSE events that came back.
async fn chat_in(app: &TestApp, thread_id: &str, message: &str) -> Vec<Value> {
    use axum::body::Body;
    use axum::http::{header, Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/chat/threads/{thread_id}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {}", app.token))
        .body(Body::from(json!({"message": message}).to_string()))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let text = String::from_utf8(resp.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    text.lines().filter_map(|l| l.strip_prefix("data:")).filter_map(|d| serde_json::from_str(d.trim()).ok()).collect()
}

#[tokio::test]
async fn a_model_that_never_answers_ends_the_turn_with_an_error_and_the_thread_stays_usable() {
    let app = TestApp::with_settings(Settings { chat_model_timeout_secs: 1, ..test_settings() }).await;
    let sse = |chunk: Value| format!("data: {chunk}\n\ndata: [DONE]\n\n");
    let hello = json!({"choices": [{"delta": {"content": "Hello."}}]});
    let mock = serve(vec![route("POST", "/chat/completions", json!(null)).raw(&sse(hello.clone())).header("content-type", "text/event-stream").delay(120_000)]).await;
    let (status, _) = app.http("PATCH", "/api/settings", Some(json!({"openai_base_url": mock.base})), true).await;
    assert_eq!(status, 200);

    let started = std::time::Instant::now();
    let (thread_id, events) = chat(&app, "Are you there?").await;
    assert!(started.elapsed() < std::time::Duration::from_secs(30), "the turn ended on its own");
    let error = events.iter().find(|e| e["type"] == "error").unwrap_or_else(|| panic!("an error event: {events:?}"));
    assert!(error["message"].as_str().unwrap().contains("did not answer within 1 second,"), "{error}");
    assert!(!events.iter().any(|e| e["type"] == "done"), "{events:?}");

    // the next message in the same thread is answered
    mock.set_routes(vec![route("POST", "/chat/completions", json!(null)).raw(&sse(hello)).header("content-type", "text/event-stream")]);
    let events = chat_in(&app, &thread_id, "Are you there now?").await;
    assert_eq!(events.iter().find(|e| e["type"] == "done").unwrap_or_else(|| panic!("{events:?}"))["reply"], "Hello.");
}

#[tokio::test]
async fn the_chat_streams_the_resolved_view_with_the_show_view_result_only() {
    let app = TestApp::new().await;
    let ids = logistics(&app).await;
    let rel = edges_by_label(&app, &ids["Truck 7"]).await;

    // the model looks the truck up and draws the impact in one round, then answers
    let view = impact(&ids["Truck 7"], json!([{"entity_id": ids["Shipment 88"], "path": [rel["carries"]], "why": "It is on the truck."}]));
    let call = |i: usize, id: &str, name: &str, args: Value| json!({"index": i, "id": id, "function": {"name": name, "arguments": args.to_string()}});
    let tools = json!({"choices": [{"delta": {"tool_calls": [call(0, "c1", "entities_search", json!({"query": "Truck 7"})), call(1, "c2", "show_view", view)]}}]});
    let reply = json!({"choices": [{"delta": {"content": "Shipment 88 will be late."}}]});
    let sse = |chunk: Value| format!("data: {chunk}\n\ndata: [DONE]\n\n");
    let mock = serve(vec![
        route("POST", "/chat/completions", json!(null)).body("\"role\":\"tool\"").raw(&sse(reply)).header("content-type", "text/event-stream"),
        route("POST", "/chat/completions", json!(null)).raw(&sse(tools)).header("content-type", "text/event-stream"),
    ])
    .await;
    let (status, _) = app.http("PATCH", "/api/settings", Some(json!({"openai_base_url": mock.base})), true).await;
    assert_eq!(status, 200);

    let (thread_id, events) = chat(&app, "Truck 7 is late. What does that affect?").await;
    let results: Vec<&Value> = events.iter().filter(|e| e["type"] == "tool_result").collect();
    assert_eq!(results.len(), 2, "{events:?}");
    // each call and its result carry the model's id for the call, so the app pairs them by it
    let pairs = |kind: &str| -> Vec<(String, String)> {
        events.iter().filter(|e| e["type"] == kind).map(|e| (e["id"].as_str().unwrap_or_else(|| panic!("no id: {e}")).to_string(), e["name"].as_str().unwrap().to_string())).collect()
    };
    let want = vec![("c1".to_string(), "entities_search".to_string()), ("c2".to_string(), "show_view".to_string())];
    assert_eq!((pairs("tool_call"), pairs("tool_result")), (want.clone(), want));
    let search = results.iter().find(|e| e["name"] == "entities_search").unwrap();
    assert!(search.get("view").is_none(), "only show_view carries a view: {search}");
    let shown = &results.iter().find(|e| e["name"] == "show_view").unwrap()["view"];
    assert_eq!((shown["view"].as_str(), shown["start"]["name"].as_str()), (Some("impact"), Some("Truck 7")), "{shown}");
    assert_eq!(shown["affected"][0]["path"][0]["to"]["name"], "Shipment 88");
    let done = events.iter().find(|e| e["type"] == "done").unwrap();
    assert_eq!(done["reply"], "Shipment 88 will be late.");

    // the stored result is the same view, so reopening the thread draws it again
    let (_, history) = app.http("GET", &format!("/api/chat/threads/{thread_id}/history"), None, true).await;
    let stored: Vec<Value> = history.as_array().unwrap().iter().filter(|m| m["role"] == "tool").map(|m| serde_json::from_str(m["content"].as_str().unwrap()).unwrap()).collect();
    assert!(stored.iter().any(|r| r == shown), "{history}");
}

#[tokio::test]
async fn the_chat_cannot_save_its_own_conclusion_but_saves_what_the_user_said() {
    let app = TestApp::new().await;
    let ids = logistics(&app).await;
    let rel = edges_by_label(&app, &ids["Truck 7"]).await;
    let view = impact(&ids["Truck 7"], json!([{"entity_id": ids["Shipment 88"], "path": [rel["carries"]], "why": "It is on the truck."}]));
    let call = |i: usize, id: &str, name: &str, args: Value| json!({"index": i, "id": id, "function": {"name": name, "arguments": args.to_string()}});
    let sse = |chunk: Value| format!("data: {chunk}\n\ndata: [DONE]\n\n");
    let draw = json!({"choices": [{"delta": {"tool_calls": [call(0, "c1", "show_view", view)]}}]});
    // then it tries to remember the headline it drew (its own conclusion) and the user's own words
    let conclusion = json!({"subject_name": "Truck 7", "subject_kind": "truck", "text": "Truck 7 is two hours late."});
    let fact = json!({"subject_name": "Truck 7", "subject_kind": "truck", "text": "The driver of Truck 7 called in sick today."});
    let save = json!({"choices": [{"delta": {"tool_calls": [call(0, "c2", "memory_write", conclusion), call(1, "c3", "memory_write", fact)]}}]});
    let reply = json!({"choices": [{"delta": {"content": "Shipment 88 will be late."}}]});
    let mock = serve(vec![
        route("POST", "/chat/completions", json!(null)).body("Not saved").raw(&sse(reply)).header("content-type", "text/event-stream"),
        route("POST", "/chat/completions", json!(null)).body("\"role\":\"tool\"").raw(&sse(save)).header("content-type", "text/event-stream"),
        route("POST", "/chat/completions", json!(null)).raw(&sse(draw)).header("content-type", "text/event-stream"),
    ])
    .await;
    let (status, _) = app.http("PATCH", "/api/settings", Some(json!({"openai_base_url": mock.base})), true).await;
    assert_eq!(status, 200);

    let (thread_id, events) = chat(&app, "The driver of Truck 7 called in sick today. What does that affect?").await;
    assert_eq!(events.iter().find(|e| e["type"] == "done").unwrap()["reply"], "Shipment 88 will be late.", "{events:?}");
    let texts: Vec<Value> = app.db().await.test_raw().query("SELECT VALUE text FROM memory").await.unwrap().take(0).unwrap();
    assert!(!texts.contains(&json!("Truck 7 is two hours late.")), "the drawn headline is not saved: {texts:?}");
    assert!(texts.contains(&json!("The driver of Truck 7 called in sick today.")), "the user's own fact is: {texts:?}");
    let (_, history) = app.http("GET", &format!("/api/chat/threads/{thread_id}/history"), None, true).await;
    assert!(history.to_string().contains("Not saved"), "the model is told why: {history}");
}

/// The model's turns in order, each served once its request holds the turn's `after` fragment (the
/// tool result it answers; "" for the first). Later turns are matched first.
async fn chat_turns(app: &TestApp, turns: Vec<(&'static str, Value)>) -> eunomia_backend::sources::mock::Mock {
    let sse = |chunks: &Value| chunks.as_array().unwrap().iter().map(|c| format!("data: {c}\n\n")).collect::<String>() + "data: [DONE]\n\n";
    let routes = turns
        .iter()
        .rev()
        .map(|(after, t)| route("POST", "/chat/completions", json!(null)).body(after).raw(&sse(t)).header("content-type", "text/event-stream"))
        .collect();
    let mock = serve(routes).await;
    let (status, _) = app.http("PATCH", "/api/settings", Some(json!({"openai_base_url": mock.base})), true).await;
    assert_eq!(status, 200);
    mock
}

fn tool_call(i: usize, id: &str, name: &str, args: Value) -> Value {
    json!({"index": i, "id": id, "function": {"name": name, "arguments": args.to_string()}})
}

#[tokio::test]
async fn the_chat_saves_what_the_user_said_while_its_reply_says_it_is_saving_it() {
    let app = TestApp::new().await;
    let text = "Jo's phone number is 0412 345 678";
    let save = json!([
        {"choices": [{"delta": {"content": format!("Saved: {text}")}}]},
        {"choices": [{"delta": {"tool_calls": [tool_call(0, "c1", "memory_write", json!({"subject_name": "Jo", "subject_kind": "person", "text": text}))]}}]},
    ]);
    let done = json!([{"choices": [{"delta": {"content": "Done."}}]}]);
    let _mock = chat_turns(&app, vec![("", save), ("\"tool_call_id\":\"c1\"", done)]).await;

    let (thread_id, _) = chat(&app, "Jo's number is 0412 345 678").await;
    let texts: Vec<Value> = app.db().await.test_raw().query("SELECT VALUE text FROM memory").await.unwrap().take(0).unwrap();
    let (_, history) = app.http("GET", &format!("/api/chat/threads/{thread_id}/history"), None, true).await;
    assert!(texts.contains(&json!(text)), "the reply's own words are not a reason to refuse: {texts:?} {history}");
}

#[tokio::test]
async fn the_chat_may_change_a_memory_type_and_resend_its_unchanged_text() {
    let app = TestApp::new().await;
    let text = "Jo moved to the Brisbane office last spring";
    let written = ok(app.tool("memory_write", json!({"subject_name": "Jo", "subject_kind": "person", "text": text, "type": "world"})).await);
    let (entity, memory) = (written["entity"]["id"].as_str().unwrap(), written["memory"]["id"].as_str().unwrap());
    // the model reads Jo (so the text is in a tool result), then fixes the type and re-sends the same text
    let read = json!([{"choices": [{"delta": {"tool_calls": [tool_call(0, "c1", "entities_get", json!({"id": entity}))]}}]}]);
    let fix = json!([{"choices": [{"delta": {"tool_calls": [tool_call(0, "c2", "memory_update", json!({"memory_id": memory, "text": text, "type": "experience"}))]}}]}]);
    let done = json!([{"choices": [{"delta": {"content": "Done."}}]}]);
    let _mock = chat_turns(&app, vec![("", read), ("\"tool_call_id\":\"c1\"", fix), ("\"tool_call_id\":\"c2\"", done)]).await;

    let (thread_id, _) = chat(&app, "That Brisbane move happened, it is not a standing fact.").await;
    let (_, history) = app.http("GET", &format!("/api/chat/threads/{thread_id}/history"), None, true).await;
    let types: Vec<Value> = app.db().await.test_raw().query(format!("SELECT VALUE type FROM memory WHERE text = '{text}'")).await.unwrap().take(0).unwrap();
    assert_eq!(types, vec![json!("experience")], "{history}");
}

// ------------------------------------------------------------ typed properties and grounded links

/// A property decision as the owner's browser session sends it.
async fn decide_property(app: &TestApp, vault: &str, kind: &str, name: &str, decision: Value) -> (u16, Value) {
    let body = json!({"kind": kind, "name": name, "decision": decision});
    let (status, body) = app.http_session("POST", &format!("/api/vaults/{vault}/ontology/properties/decide"), Some(body)).await;
    (status.as_u16(), body)
}

fn property<'a>(ontology: &'a Value, kind_name: &str, name: &str) -> &'a Value {
    kind(ontology, kind_name)["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .unwrap_or_else(|| panic!("no property {name} on {kind_name} in {ontology}"))
}

fn price(datatype: &str) -> Value {
    json!({"kind": "product", "name": "price", "datatype": datatype, "unit": "AUD", "description": "What one costs", "synonyms": ["cost"]})
}

/// A latte (a `product`) and an approved `price` (money, AUD) on product. Returns the vault and the latte's id.
async fn latte_with_price(app: &TestApp) -> (String, String) {
    let latte = write(app, "Latte", "product").await["entity"]["id"].as_str().unwrap().to_string();
    let vault = personal(app).await;
    ok(app.tool("ontology_propose", json!({"properties": [price("money")]})).await);
    assert_eq!(decide_property(app, &vault, "product", "price", json!({"action": "approve"})).await.0, 200);
    (vault, latte)
}

fn err(v: &Value) -> &str {
    v["error"].as_str().unwrap_or_else(|| panic!("expected an error, got {v}"))
}

#[tokio::test]
async fn a_proposed_property_waits_for_the_owner_in_the_browser() {
    let app = TestApp::new().await;
    write(&app, "Latte", "product").await;
    let vault = personal(&app).await;

    let out = ok(app.tool("ontology_propose", json!({"properties": [price("money")]})).await);
    assert_eq!(out["properties"], json!([{"kind": "product", "name": "price", "datatype": "money", "status": "proposed", "merge_into": null}]));
    let p = property(&ok(app.tool("ontology_get", json!({})).await), "product", "price").clone();
    assert_eq!((p["status"].as_str(), p["origin"].as_str(), p["unit"].as_str(), p["values"].as_i64()), (Some("proposed"), Some("agent"), Some("AUD"), Some(0)));
    assert_eq!(p["synonyms"], json!(["cost"]));

    // a bad datatype or name fails the whole call
    let bad = app.tool("ontology_propose", json!({"properties": [{"kind": "product", "name": "size", "datatype": "colour"}]})).await;
    assert!(err(&bad).contains("datatype"), "{bad}");
    let bad = app.tool("ontology_propose", json!({"properties": [{"kind": "product", "name": "Unit Price", "datatype": "money"}]})).await;
    assert!(err(&bad).contains("invalid property name"), "{bad}");

    // the owner with a token (an agent may hold one) cannot decide
    let path = format!("/api/vaults/{vault}/ontology/properties/decide");
    let body = json!({"kind": "product", "name": "price", "decision": {"action": "approve"}});
    let (status, refused) = app.http("POST", &path, Some(body.clone()), true).await;
    assert_eq!(status, 403, "{refused}");
    assert_eq!(property(&ok(app.tool("ontology_get", json!({})).await), "product", "price")["status"], "proposed");

    // the owner in the browser approves; a later proposal does not change the decision
    let (status, o) = app.http_session("POST", &path, Some(body)).await;
    assert_eq!(status, 200, "{o}");
    assert_eq!(property(&o, "product", "price")["status"], "approved");
    let again = ok(app.tool("ontology_propose", json!({"properties": [price("text")]})).await);
    assert_eq!((again["properties"][0]["status"].as_str(), again["properties"][0]["datatype"].as_str()), (Some("approved"), Some("money")));

    // describing keeps the status; deciding a property nobody proposed is not found
    let (status, o) = decide_property(&app, &vault, "product", "price", json!({"action": "describe", "description": "Shelf price"})).await;
    assert_eq!(status, 200, "{o}");
    assert_eq!((property(&o, "product", "price")["description"].as_str(), property(&o, "product", "price")["status"].as_str()), (Some("Shelf price"), Some("approved")));
    assert_eq!(decide_property(&app, &vault, "product", "weight", json!({"action": "approve"})).await.0, 404);
}

#[tokio::test]
async fn a_value_needs_an_approved_property_and_must_fit_its_datatype() {
    let app = TestApp::new().await;
    let latte = write(&app, "Latte", "product").await["entity"]["id"].as_str().unwrap().to_string();
    let vault = personal(&app).await;
    let set = |property: &str, value: Value| app.tool("entity_set_property", json!({"entity_id": latte, "property": property, "value": value}));

    let unknown = set("price", json!(4.5)).await;
    assert!(err(&unknown).contains("propose it with ontology_propose"), "{unknown}");
    ok(app.tool("ontology_propose", json!({"properties": [
        price("money"),
        {"kind": "product", "name": "size", "datatype": "enum", "enum_values": ["Small", "Large"]},
        {"kind": "product", "name": "launched_on", "datatype": "date"},
    ]})).await);
    let proposed = set("price", json!(4.5)).await;
    assert!(err(&proposed).contains("proposed, not approved"), "{proposed}");

    for name in ["price", "size", "launched_on"] {
        assert_eq!(decide_property(&app, &vault, "product", name, json!({"action": "approve"})).await.0, 200);
    }
    for (name, value, says) in [
        ("price", json!("$4.50"), "must be a number"),
        ("price", json!(true), "must be a number"),
        ("size", json!("Medium"), "takes one of Small, Large"),
        ("launched_on", json!("7/11/2026"), "YYYY-MM-DD"),
        ("launched_on", json!("2026-02-30"), "YYYY-MM-DD"),
    ] {
        let refused = set(name, value).await;
        assert!(err(&refused).contains(says), "{name}: {refused}");
    }
    let mut res = app.db().await.test_raw().query("SELECT VALUE id FROM entity_property").await.unwrap();
    let written: Vec<Value> = res.take(0).unwrap();
    assert!(written.is_empty(), "a refused value writes nothing: {written:?}");

    let out = ok(set("price", json!("4.50")).await);
    assert_eq!((out["value"]["property"].as_str(), out["value"]["value"].as_f64(), out["value"]["unit"].as_str()), (Some("price"), Some(4.5), Some("AUD")));
    assert!(out.get("property_mapped").is_none(), "{out}");
    assert_eq!(ok(set("size", json!("large")).await)["value"]["value"], "Large", "an enum value is stored as listed");
    assert_eq!(ok(set("launched_on", json!("2026-11-07")).await)["value"]["value"], "2026-11-07");
}

#[tokio::test]
async fn a_rejected_property_sends_its_values_to_the_merge_target() {
    let app = TestApp::new().await;
    let (vault, latte) = latte_with_price(&app).await;
    ok(app.tool("ontology_propose", json!({"properties": [
        {"kind": "product", "name": "cost", "datatype": "number"},
        {"kind": "product", "name": "junk", "datatype": "text"},
    ]})).await);

    // a merge target must be a property of the kind that is kept
    assert_eq!(decide_property(&app, &vault, "product", "cost", json!({"action": "reject", "merge_into": "weight"})).await.0, 400);
    let (status, o) = decide_property(&app, &vault, "product", "cost", json!({"action": "reject", "merge_into": "price"})).await;
    assert_eq!(status, 200, "{o}");
    assert_eq!((property(&o, "product", "cost")["status"].as_str(), property(&o, "product", "cost")["merge_into"].as_str()), (Some("rejected"), Some("price")));

    let out = ok(app.tool("entity_set_property", json!({"entity_id": latte, "property": "cost", "value": 5, "unit": "USD"})).await);
    assert_eq!(out["property_mapped"], json!({"from": "cost", "to": "price"}));
    assert_eq!((out["value"]["property"].as_str(), out["value"]["value"].as_f64(), out["value"]["unit"].as_str()), (Some("price"), Some(5.0), Some("USD")));

    // rejected with no target: refused, naming the approved properties
    assert_eq!(decide_property(&app, &vault, "product", "junk", json!({"action": "reject"})).await.0, 200);
    let refused = app.tool("entity_set_property", json!({"entity_id": latte, "property": "junk", "value": "x"})).await;
    assert!(err(&refused).contains("was rejected") && err(&refused).contains("Approved properties of \"product\": price"), "{refused}");
}

#[tokio::test]
async fn the_newest_value_wins_on_read_and_the_older_ones_stay() {
    let app = TestApp::new().await;
    let (_, latte) = latte_with_price(&app).await;
    let cafe = write(&app, "Bean Bar", "cafe").await["entity"]["id"].as_str().unwrap().to_string();
    ok(app.tool("code_relate", json!({"from_id": cafe, "to_id": latte, "label": "sells"})).await);
    let set = |value: f64, as_of: &str| app.tool("entity_set_property", json!({"entity_id": latte, "property": "price", "value": value, "as_of": as_of}));
    ok(set(4.5, "2026-01-01").await);
    ok(set(5.0, "2026-06-01").await);
    // written last but true earlier: it is history, not the current value
    ok(set(4.0, "2025-12-01").await);

    let got = ok(app.tool("entities_get", json!({"id": latte})).await);
    assert_eq!(got["properties"].as_array().unwrap().len(), 1, "{got}");
    let current = &got["properties"][0];
    assert_eq!((current["property"].as_str(), current["value"].as_f64(), current["unit"].as_str()), (Some("price"), Some(5.0), Some("AUD")));
    assert!(current["as_of"].as_str().unwrap().starts_with("2026-06-01"), "{current}");

    let hood = ok(app.tool("entities_neighbourhood", json!({"entity_id": cafe})).await);
    let node = hood["nodes"].as_array().unwrap().iter().find(|n| n["id"] == latte.as_str()).unwrap();
    assert_eq!(node["properties"][0]["value"].as_f64(), Some(5.0), "{hood}");

    // a future as_of would pin a value no later write could replace, so it is refused and stores nothing
    let next_year = (chrono::Utc::now() + chrono::Duration::days(365)).format("%Y-%m-%d").to_string();
    let refused = set(0.0, &next_year).await;
    assert!(err(&refused).contains("as_of is in the future"), "{refused}");
    let in_two_days = (chrono::Utc::now() + chrono::Duration::days(2)).format("%Y-%m-%d").to_string();
    assert!(err(&set(0.0, &in_two_days).await).contains("as_of is in the future"));
    let in_an_hour = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    assert!(err(&set(0.0, &in_an_hour).await).contains("as_of is in the future"));

    let mut res = app.db().await.test_raw().query("SELECT VALUE value_num FROM entity_property ORDER BY as_of").await.unwrap();
    let history: Vec<f64> = res.take(0).unwrap();
    assert_eq!(history, [4.0, 4.5, 5.0]);
    let o = ok(app.tool("ontology_get", json!({})).await);
    assert_eq!(property(&o, "product", "price")["values"], 3);

    // a merged-away entity's values move to the winner; a deleted entity's go with it
    let dup = write(&app, "Caffe Latte", "product").await["entity"]["id"].as_str().unwrap().to_string();
    ok(app.tool("entity_set_property", json!({"entity_id": dup, "property": "price", "value": 6, "as_of": "2026-09-01"})).await);
    ok(app.tool("entity_merge", json!({"winner_id": latte, "loser_id": dup})).await);
    assert_eq!(ok(app.tool("entities_get", json!({"id": latte})).await)["properties"][0]["value"].as_f64(), Some(6.0));

    // today's date where the day starts before UTC's (Melbourne in the morning) is tomorrow in UTC: accepted
    let tomorrow_utc = (chrono::Utc::now() + chrono::Duration::days(1)).format("%Y-%m-%d").to_string();
    ok(set(5.5, &tomorrow_utc).await);
    assert_eq!(ok(app.tool("entities_get", json!({"id": latte})).await)["properties"][0]["value"].as_f64(), Some(5.5));
    ok(app.tool("entity_delete", json!({"entity_id": latte})).await);
    let mut res = app.db().await.test_raw().query("SELECT VALUE id FROM entity_property").await.unwrap();
    let left: Vec<Value> = res.take(0).unwrap();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test]
async fn an_unapproved_property_is_not_shown_and_values_stay_in_their_vault() {
    let app = TestApp::new().await;
    let (vault, latte) = latte_with_price(&app).await;
    ok(app.tool("entity_set_property", json!({"entity_id": latte, "property": "price", "value": 4.5})).await);
    // an approval taken back hides the value on read
    assert_eq!(decide_property(&app, &vault, "product", "price", json!({"action": "reject"})).await.0, 200);
    assert_eq!(ok(app.tool("entities_get", json!({"id": latte})).await)["properties"], json!([]));

    // the approval is the personal vault's: a product in Team has no price property
    ok(app.tool("vault_create", json!({"name": "Team", "kind": "org"})).await);
    let team_latte = ok(app.tool("memory_write", json!({"subject_name": "Latte", "subject_kind": "product", "text": "on the menu", "vault_id": "Team"})).await);
    let team_id = team_latte["entity"]["id"].as_str().unwrap();
    let refused = app.tool("entity_set_property", json!({"entity_id": team_id, "property": "price", "value": 4.5})).await;
    assert!(err(&refused).contains("does not exist in this vault"), "{refused}");

    // another member of the org who is not in the vault neither reads nor writes its values
    let bob = common::register(&app.state, "bob@example.com").await;
    let got = as_user(&app, &bob, "entity_set_property", json!({"entity_id": latte, "property": "price", "value": 1})).await;
    assert!(err(&got).contains("not found"), "{got}");
    let got = as_user(&app, &bob, "entities_get", json!({"id": latte})).await;
    assert!(got.get("properties").is_none(), "{got}");
}

#[tokio::test]
async fn a_rejected_link_type_refuses_the_relation_and_names_the_approved_ones() {
    let app = TestApp::new().await;
    let ada = write(&app, "Ada", "person").await["entity"]["id"].as_str().unwrap().to_string();
    let orders = write(&app, "orders_db", "db").await["entity"]["id"].as_str().unwrap().to_string();
    let vault = personal(&app).await;
    let path = format!("/api/vaults/{vault}/ontology/links/decide");
    let decide = |label: &str, action: &str| {
        let body = json!({"from_kind": "person", "label": label, "to_kind": "db", "decision": {"action": action}});
        app.http_session("POST", &path, Some(body))
    };
    assert_eq!(decide("owns", "reject").await.0, 200);
    assert_eq!(decide("maintains", "approve").await.0, 200);

    let refused = app.tool("code_relate", json!({"from_id": ada, "to_id": orders, "label": "owns"})).await;
    assert!(err(&refused).contains("person owns db was rejected") && err(&refused).contains("person maintains db"), "{refused}");
    assert_eq!(refused["code"], "ontology.rejected", "{refused}");
    let (status, body) = app.http("POST", &format!("/api/entities/{ada}/relations"), Some(json!({"to_id": orders, "label": "owns"})), true).await;
    assert_eq!(status, 400, "{body}");

    // an approved label and a label nobody reviewed both pass; the unknown one shows as observed
    ok(app.tool("code_relate", json!({"from_id": ada, "to_id": orders, "label": "maintains"})).await);
    ok(app.tool("code_relate", json!({"from_id": ada, "to_id": orders, "label": "reads"})).await);
    let o = ok(app.tool("ontology_get", json!({})).await);
    assert_eq!(link(&o, "person", "reads", "db")["status"], "observed");
    assert_eq!(link(&o, "person", "owns", "db")["count"], 0);

    // a code entity's parent link is grounded too
    let repo = ok(app.tool("code_entity_upsert", json!({"kind": "repository", "name": "eunomia"})).await)["id"].as_str().unwrap().to_string();
    let body = json!({"from_kind": "file", "label": "part_of", "to_kind": "repository", "decision": {"action": "reject"}});
    assert_eq!(app.http_session("POST", &format!("/api/vaults/{vault}/ontology/links/decide"), Some(body)).await.0, 200);
    // the file is still written; only its rejected parent link is left out
    let file = ok(app.tool("code_entity_upsert", json!({"kind": "file", "name": "src/main.rs", "parent_id": repo})).await);
    let got = ok(app.tool("entities_get", json!({"id": file["id"]})).await);
    assert_eq!(got["relations"].as_array().map(Vec::len), Some(0), "{got}");
}

#[tokio::test]
async fn suggest_proposes_properties_and_only_synonyms_written_in_each_kinds_data() {
    let answer = json!({
        "kinds": [
            {"kind": "product", "description": "Something the cafe sells", "synonyms": ["FW", "Long Black"], "reason": "written in a fact"},
            {"kind": "cafe", "description": "A place that sells coffee", "synonyms": ["FW"], "reason": "copied from product"},
        ],
        "links": [],
        "properties": [
            {"kind": "product", "name": "price", "datatype": "money", "unit": "AUD", "description": "What one costs", "synonyms": ["sticker price", "cost"], "reason": "facts give prices"},
            {"kind": "product", "name": "Bad Name", "datatype": "money"},
            {"kind": "product", "name": "size", "datatype": "enum"},
        ],
    });
    let completion = json!({"choices": [{"message": {"content": answer.to_string()}}]});
    let mock = serve(vec![route("POST", "/chat/completions", completion)]).await;
    let settings = Settings { openai_base_url: mock.base.clone(), openai_api_key: Some("sk-test".into()), openai_chat_model: "test-model".into(), ..test_settings() };
    let app = TestApp::with_settings(settings).await;
    ok(app.tool("memory_write", json!({"subject_name": "Flat White", "subject_kind": "product", "text": "Flat White, also called FW, has a cost of 4.50 AUD"})).await);
    write(&app, "Bean Bar", "cafe").await;

    let out = ok(app.tool("ontology_suggest", json!({})).await);
    assert_eq!((out["mode"].as_str(), out["skipped"].as_i64()), (Some("model"), Some(2)), "{out}");
    let o = &out["ontology"];
    let p = property(o, "product", "price");
    assert_eq!((p["status"].as_str(), p["origin"].as_str(), p["datatype"].as_str(), p["unit"].as_str()), (Some("proposed"), Some("suggested"), Some("money"), Some("AUD")));
    assert_eq!(p["synonyms"], json!(["cost"]), "only a synonym found in the kind's data");
    assert_eq!(kind(o, "product")["synonyms"], json!(["FW"]));
    assert_eq!(kind(o, "cafe")["synonyms"], json!([]), "a synonym from another kind's data is not copied");

    // the model read the facts as evidence
    let sent: Vec<_> = mock.requests().into_iter().filter(|r| r.path == "/chat/completions").collect();
    assert!(sent[0].body.contains("also called FW"), "{}", sent[0].body);
}

// ------------------------------------------------------------ certified metrics

/// A metric decision as the owner's browser session sends it.
async fn decide_metric(app: &TestApp, vault: &str, kind: &str, name: &str, decision: Value) -> (u16, Value) {
    let body = json!({"kind": kind, "name": name, "decision": decision});
    let (status, body) = app.http_session("POST", &format!("/api/vaults/{vault}/ontology/metrics/decide"), Some(body)).await;
    (status.as_u16(), body)
}

fn metric<'a>(ontology: &'a Value, kind_name: &str, name: &str) -> &'a Value {
    kind(ontology, kind_name)["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == name)
        .unwrap_or_else(|| panic!("no metric {name} on {kind_name} in {ontology}"))
}

fn eval_args(metric: &str, extra: Value) -> Value {
    let mut args = json!({"metric": metric});
    args.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
    args
}

/// A small cafe range in the personal vault. Products with approved `price` (money, AUD), `size` (enum),
/// `launched_on` (date) and `supplier` (text). Current prices: Latte 5 (4 until June), Mocha 6, Tea 3;
/// the Scone has no price. Returns the vault and the product ids by name.
async fn cafe_range(app: &TestApp) -> (String, std::collections::HashMap<&'static str, String>) {
    let mut ids = std::collections::HashMap::new();
    for name in ["Latte", "Mocha", "Tea", "Scone"] {
        ids.insert(name, write(app, name, "product").await["entity"]["id"].as_str().unwrap().to_string());
    }
    let vault = personal(app).await;
    ok(app.tool("ontology_propose", json!({"properties": [
        price("money"),
        {"kind": "product", "name": "size", "datatype": "enum", "enum_values": ["Small", "Large"]},
        {"kind": "product", "name": "launched_on", "datatype": "date"},
        {"kind": "product", "name": "supplier", "datatype": "text"},
    ]})).await);
    for name in ["price", "size", "launched_on", "supplier"] {
        assert_eq!(decide_property(app, &vault, "product", name, json!({"action": "approve"})).await.0, 200);
    }
    let set = |who: &str, property: &str, value: Value, as_of: &str| {
        app.tool("entity_set_property", json!({"entity_id": ids[who], "property": property, "value": value, "as_of": as_of}))
    };
    for (who, property, value, as_of) in [
        ("Latte", "price", json!(4.0), "2026-01-01"),
        ("Latte", "price", json!(5.0), "2026-06-01"),
        ("Mocha", "price", json!(6.0), "2026-02-01"),
        ("Tea", "price", json!(3.0), "2026-02-01"),
        ("Latte", "size", json!("Large"), "2026-01-01"),
        ("Mocha", "size", json!("Large"), "2026-01-01"),
        ("Tea", "size", json!("Small"), "2026-01-01"),
        ("Scone", "size", json!("Small"), "2026-01-01"),
        ("Latte", "launched_on", json!("2026-01-15"), "2026-01-15"),
        ("Mocha", "launched_on", json!("2026-01-20"), "2026-01-20"),
        ("Tea", "launched_on", json!("2026-03-05"), "2026-03-05"),
        ("Latte", "supplier", json!("Riverside Roasters"), "2026-01-01"),
        ("Mocha", "supplier", json!("Riverside Roasters"), "2026-01-01"),
        ("Tea", "supplier", json!("Leaf Co"), "2026-01-01"),
    ] {
        ok(set(who, property, value, as_of).await);
    }
    (vault, ids)
}

/// Proposes `metrics` (each {name, expr, ...} on product) and approves them in the browser.
async fn approved_metrics(app: &TestApp, vault: &str, metrics: Value) {
    let metrics: Vec<Value> = metrics.as_array().unwrap().iter().map(|m| { let mut m = m.clone(); m["kind"] = json!("product"); m }).collect();
    ok(app.tool("ontology_propose", json!({"metrics": metrics.clone()})).await);
    for m in &metrics {
        let (status, body) = decide_metric(app, vault, "product", m["name"].as_str().unwrap(), json!({"action": "approve"})).await;
        assert_eq!(status, 200, "{body}");
    }
}

fn ids_of(names: &[&str], ids: &std::collections::HashMap<&'static str, String>) -> Vec<String> {
    let mut out: Vec<String> = names.iter().map(|n| ids[n].clone()).collect();
    out.sort();
    out
}

#[tokio::test]
async fn a_metric_is_refused_unless_its_expression_fits_the_kinds_properties() {
    let app = TestApp::new().await;
    cafe_range(&app).await;
    let propose = |name: &str, expr: Value| app.tool("ontology_propose", json!({"metrics": [{"kind": "product", "name": name, "expr": expr}]}));
    let gt = |i: usize| json!({"property": "price", "op": "gt", "value": i});
    for (name, expr, says) in [
        ("avg_weight", json!({"agg": "avg", "property": "weight"}), "not a property of \"product\""),
        ("supplier_sum", json!({"agg": "sum", "property": "supplier"}), "needs a number, integer, money or percent property"),
        ("cheap", json!({"agg": "count", "filters": [{"property": "price", "op": "lt", "value": "cheap"}]}), "must be a number"),
        ("mixed", json!({"agg": "count", "filters": [{"property": "size", "op": "gt", "value": "Small"}]}), "takes eq, ne or in"),
        ("many", json!({"agg": "count", "filters": (0..11).map(gt).collect::<Vec<_>>()}), "at most 10 filters"),
        ("medium", json!({"agg": "count", "filters": [{"property": "size", "op": "eq", "value": "Medum"}]}), "nearest: "),
        // anything that reads as query text: in a property name, a filter value, or an extra field
        ("drop", json!({"agg": "sum", "property": "price; DELETE entity_property"}), "invalid property name"),
        ("inject", json!({"agg": "count", "filters": [{"property": "supplier", "op": "eq", "value": "x'; DELETE entity_property; --"}]}), "looks like query text"),
        ("raw", json!({"agg": "count", "sql": "SELECT * FROM entity_property"}), "unknown field"),
        ("Avg Price", json!({"agg": "avg", "property": "price"}), "invalid metric name"),
    ] {
        let refused = propose(name, expr).await;
        assert!(err(&refused).contains(says), "{name}: {refused}");
    }
    let mut res = app.db().await.test_raw().query("SELECT VALUE id FROM ontology_metric").await.unwrap();
    let written: Vec<Value> = res.take(0).unwrap();
    assert!(written.is_empty(), "a refused metric writes nothing: {written:?}");

    // a metric over a property proposed in the same call is checked against that property
    let out = ok(app.tool("ontology_propose", json!({
        "properties": [{"kind": "product", "name": "weight_g", "datatype": "integer"}],
        "metrics": [{"kind": "product", "name": "avg_weight", "expr": {"agg": "avg", "property": "weight_g"}, "description": "Average weight"}],
    })).await);
    assert_eq!(out["metrics"], json!([{"kind": "product", "name": "avg_weight", "status": "proposed", "merge_into": null}]));
}

#[tokio::test]
async fn only_a_vault_owner_in_the_browser_decides_a_metric() {
    let app = TestApp::new().await;
    let (vault, _) = cafe_range(&app).await;
    ok(app.tool("ontology_propose", json!({
        "properties": [{"kind": "product", "name": "weight_g", "datatype": "integer"}],
        "metrics": [
            {"kind": "product", "name": "avg_price", "expr": {"agg": "avg", "property": "price"}, "description": "What a product costs on average"},
            {"kind": "product", "name": "avg_weight", "expr": {"agg": "avg", "property": "weight_g"}},
        ],
    })).await);
    let o = ok(app.tool("ontology_get", json!({})).await);
    let m = metric(&o, "product", "avg_price");
    assert_eq!((m["status"].as_str(), m["origin"].as_str(), m["expr_words"].as_str(), m["unit"].as_str()), (Some("proposed"), Some("agent"), Some("Average price"), None));
    assert_eq!(m["expr"], json!({"agg": "avg", "property": "price"}));

    let path = format!("/api/vaults/{vault}/ontology/metrics/decide");
    let body = json!({"kind": "product", "name": "avg_price", "decision": {"action": "approve"}});
    let (status, refused) = app.http("POST", &path, Some(body.clone()), true).await;
    assert_eq!(status, 403, "a token (an agent may hold one) cannot decide: {refused}");
    assert_eq!(metric(&ok(app.tool("ontology_get", json!({})).await), "product", "avg_price")["status"], "proposed");
    let (status, o) = app.http_session("POST", &path, Some(body)).await;
    assert_eq!(status, 200, "{o}");
    assert_eq!(metric(&o, "product", "avg_price")["status"], "approved");

    // a metric over a property nobody approved cannot be approved: it would read nothing
    let (status, refused) = decide_metric(&app, &vault, "product", "avg_weight", json!({"action": "approve"})).await;
    assert_eq!(status, 400, "{refused}");
    assert!(refused.to_string().contains("approve the property \\\"weight_g\\\""), "{refused}");
    let (status, o) = decide_metric(&app, &vault, "product", "avg_weight", json!({"action": "describe", "description": "Grams, on average"})).await;
    assert_eq!(status, 200, "{o}");
    assert_eq!((metric(&o, "product", "avg_weight")["description"].as_str(), metric(&o, "product", "avg_weight")["status"].as_str()), (Some("Grams, on average"), Some("proposed")));
    // a merge target must be a metric of the kind that is kept
    assert_eq!(decide_metric(&app, &vault, "product", "avg_weight", json!({"action": "reject", "merge_into": "nothing_here"})).await.0, 400);
    assert_eq!(decide_metric(&app, &vault, "product", "nothing_here", json!({"action": "approve"})).await.0, 404);
}

#[tokio::test]
async fn metric_eval_computes_each_aggregate_from_the_latest_values() {
    let app = TestApp::new().await;
    let (vault, ids) = cafe_range(&app).await;
    approved_metrics(&app, &vault, json!([
        {"name": "total_price", "expr": {"agg": "sum", "property": "price"}},
        {"name": "avg_price", "expr": {"agg": "avg", "property": "price"}},
        {"name": "lowest_price", "expr": {"agg": "min", "property": "price"}},
        {"name": "highest_price", "expr": {"agg": "max", "property": "price"}},
        {"name": "products", "expr": {"agg": "count"}},
        {"name": "priced_products", "expr": {"agg": "count", "property": "price"}},
        {"name": "sizes", "expr": {"agg": "count_distinct", "property": "size"}},
        {"name": "large_share", "expr": {"agg": "ratio", "numerator": {"agg": "count", "filters": [{"property": "size", "op": "eq", "value": "large"}]}, "denominator": {"agg": "count"}}},
    ])).await;
    let run = |name: &str| app.tool("metric_eval", json!({"metric": name}));
    for (name, value, unit, from) in [
        ("total_price", 14.0, Some("AUD"), vec!["Latte", "Mocha", "Tea"]),
        ("avg_price", 4.666667, Some("AUD"), vec!["Latte", "Mocha", "Tea"]),
        ("lowest_price", 3.0, Some("AUD"), vec!["Latte", "Mocha", "Tea"]),
        ("highest_price", 6.0, Some("AUD"), vec!["Latte", "Mocha", "Tea"]),
        ("products", 4.0, None, vec!["Latte", "Mocha", "Tea", "Scone"]),
        ("priced_products", 3.0, None, vec!["Latte", "Mocha", "Tea"]),
        ("sizes", 2.0, None, vec!["Latte", "Mocha", "Tea", "Scone"]),
        ("large_share", 0.5, None, vec!["Latte", "Mocha", "Tea", "Scone"]),
    ] {
        let out = ok(run(name).await);
        assert_eq!(out["value"].as_f64(), Some(value), "{name}: {out}");
        assert_eq!(out["unit"].as_str(), unit, "{name}: {out}");
        assert_eq!((out["status"].as_str(), out["kind"].as_str(), out["n_entities"].as_u64()), (Some("approved"), Some("product"), Some(from.len() as u64)), "{name}: {out}");
        let got: Vec<String> = serde_json::from_value(out["entity_ids"].clone()).unwrap();
        assert_eq!(got, ids_of(&from, &ids), "{name}: the entities it was computed from");
        assert_eq!(out["truncated"], false);
    }
    let shown = ok(run("large_share").await);
    assert_eq!(shown["expr_words"], "Number of product where size is Large, divided by number of product");
}

#[tokio::test]
async fn metric_eval_reads_each_value_as_of_a_date_and_applies_filters() {
    let app = TestApp::new().await;
    let (vault, ids) = cafe_range(&app).await;
    approved_metrics(&app, &vault, json!([
        {"name": "total_price", "expr": {"agg": "sum", "property": "price"}},
        {"name": "riverside_avg_price", "expr": {"agg": "avg", "property": "price", "filters": [{"property": "supplier", "op": "eq", "value": "Riverside Roasters"}]}},
        {"name": "launched_total", "expr": {"agg": "sum", "property": "price", "time_property": "launched_on", "grain": "month"}},
        {"name": "sized", "expr": {"agg": "count", "property": "size"}},
    ])).await;
    let run = |name: &str, extra: Value| app.tool("metric_eval", eval_args(name, extra));

    // history from typed properties: the Latte was 4 until June
    assert_eq!(ok(run("total_price", json!({})).await)["value"].as_f64(), Some(14.0));
    let march = ok(run("total_price", json!({"as_of": "2026-03-01"})).await);
    assert_eq!((march["value"].as_f64(), march["as_of"].as_str()), (Some(13.0), Some("2026-03-01T23:59:59Z")), "{march}");
    // a bare date includes the values dated that day
    assert_eq!(ok(run("total_price", json!({"as_of": "2026-06-01"})).await)["value"].as_f64(), Some(14.0));
    let early = ok(run("total_price", json!({"as_of": "2026-01-10"})).await);
    assert_eq!((early["value"].as_f64(), early["n_entities"].as_u64()), (Some(4.0), Some(1)));
    assert_eq!(early["entity_ids"], json!([ids["Latte"]]));

    // the metric's filter, then an extra one ANDed with it
    let riverside = ok(run("riverside_avg_price", json!({})).await);
    assert_eq!((riverside["value"].as_f64(), riverside["n_entities"].as_u64()), (Some(5.5), Some(2)), "{riverside}");
    let large = ok(run("total_price", json!({"filters": [{"property": "size", "op": "in", "value": ["Large"]}]})).await);
    assert_eq!(large["value"].as_f64(), Some(11.0), "{large}");
    let words = large["expr_words"].as_str().unwrap();
    assert!(words.ends_with(" where size is one of Large") && !words.contains(", and where"), "{large}");
    let cheap = ok(run("total_price", json!({"filters": [{"property": "price", "op": "lt", "value": 5}]})).await);
    assert_eq!(cheap["value"].as_f64(), Some(3.0));
    // "is not Leaf Co" counts the Scone, which has no supplier at all
    let not_leaf = ok(run("sized", json!({"filters": [{"property": "supplier", "op": "ne", "value": "Leaf Co"}]})).await);
    assert_eq!((not_leaf["value"].as_f64(), not_leaf["n_entities"].as_u64()), (Some(3.0), Some(3)), "{not_leaf}");
    let mut picked: Vec<&str> = not_leaf["entity_ids"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    picked.sort();
    let mut want = vec![ids["Latte"].as_str(), ids["Mocha"].as_str(), ids["Scone"].as_str()];
    want.sort();
    assert_eq!(picked, want);

    // a series by the month each product launched, oldest first
    let series = ok(run("launched_total", json!({"group_by_grain": true})).await);
    assert_eq!(series["series"], json!([{"period": "2026-01", "value": 11.0, "n": 2}, {"period": "2026-03", "value": 3.0, "n": 1}]), "{series}");
    assert!(series["value"].is_null());
    let refused = run("total_price", json!({"group_by_grain": true})).await;
    assert!(err(&refused).contains("has no time_property"), "{refused}");
}

#[tokio::test]
async fn a_misspelt_filter_value_warns_with_the_nearest_values_instead_of_a_zero() {
    let app = TestApp::new().await;
    let (vault, _) = cafe_range(&app).await;
    approved_metrics(&app, &vault, json!([{"name": "total_price", "expr": {"agg": "sum", "property": "price"}}])).await;

    let out = ok(app.tool("metric_eval", json!({"metric": "total_price", "filters": [{"property": "size", "op": "eq", "value": "Lage"}]})).await);
    assert!(out["value"].is_null(), "an enum value listed nowhere is no answer, not 0: {out}");
    let warnings = out["warnings"].to_string();
    assert!(warnings.contains("nearest: Large"), "{out}");
    assert_eq!(out["n_entities"], 0);

    // a text value no entity holds names the stored values nearest it
    let out = ok(app.tool("metric_eval", json!({"metric": "total_price", "filters": [{"property": "supplier", "op": "eq", "value": "Riverside Roaster"}]})).await);
    assert_eq!(out["value"].as_f64(), Some(0.0));
    assert!(out["warnings"].to_string().contains("nearest stored values: Riverside Roasters"), "{out}");
}

#[tokio::test]
async fn only_an_approved_metric_runs_by_name() {
    let app = TestApp::new().await;
    let (vault, _) = cafe_range(&app).await;
    approved_metrics(&app, &vault, json!([{"name": "avg_price", "expr": {"agg": "avg", "property": "price"}}])).await;
    ok(app.tool("ontology_propose", json!({"metrics": [
        {"kind": "product", "name": "max_price", "expr": {"agg": "max", "property": "price"}},
        {"kind": "product", "name": "mean_price", "expr": {"agg": "avg", "property": "price"}},
        {"kind": "product", "name": "junk", "expr": {"agg": "count"}},
    ]})).await);

    // proposed: refused, unless the caller asks to try it, and then marked unapproved
    let refused = app.tool("metric_eval", json!({"metric": "max_price"})).await;
    assert!(err(&refused).contains("proposed, not approved") && err(&refused).contains("allow_unapproved"), "{refused}");
    let tried = ok(app.tool("metric_eval", json!({"metric": "max_price", "allow_unapproved": true})).await);
    assert_eq!((tried["value"].as_f64(), tried["status"].as_str()), (Some(6.0), Some("proposed")), "{tried}");
    assert!(tried["warnings"].to_string().contains("not approved"), "{tried}");

    // rejected into another metric: that one runs, and the result says so
    assert_eq!(decide_metric(&app, &vault, "product", "mean_price", json!({"action": "reject", "merge_into": "avg_price"})).await.0, 200);
    let mapped = ok(app.tool("metric_eval", json!({"metric": "mean_price"})).await);
    assert_eq!(mapped["metric_mapped"], json!({"from": "mean_price", "to": "avg_price"}));
    assert_eq!((mapped["metric"].as_str(), mapped["value"].as_f64(), mapped["status"].as_str()), (Some("avg_price"), Some(4.666667), Some("approved")));

    // rejected with no target, or unknown: refused, naming the approved metrics
    assert_eq!(decide_metric(&app, &vault, "product", "junk", json!({"action": "reject"})).await.0, 200);
    let refused = app.tool("metric_eval", json!({"metric": "junk", "allow_unapproved": true})).await;
    assert!(err(&refused).contains("was rejected") && err(&refused).contains("avg_price (product)"), "{refused}");
    assert_eq!(refused["code"], "ontology.rejected", "{refused}");
    let unknown = app.tool("metric_eval", json!({"metric": "median_price"})).await;
    assert!(err(&unknown).contains("no metric \"median_price\"") && err(&unknown).contains("avg_price (product)"), "{unknown}");

    // the metric is the personal vault's: another vault has none
    ok(app.tool("vault_create", json!({"name": "Team", "kind": "org"})).await);
    let elsewhere = app.tool("metric_eval", json!({"metric": "avg_price", "vault_id": "Team"})).await;
    assert!(err(&elsewhere).contains("no metric"), "{elsewhere}");
}

#[tokio::test]
async fn a_metric_lists_at_most_fifty_entity_ids_and_says_when_there_were_more() {
    let app = TestApp::new().await;
    let items: Vec<Value> = (0..55).map(|i| json!({"subject_name": format!("Widget {i}"), "subject_kind": "widget", "text": format!("Widget {i} is a widget")})).collect();
    ok(app.tool("memory_write_batch", json!({"items": items})).await);
    let vault = personal(&app).await;
    ok(app.tool("ontology_propose", json!({"metrics": [{"kind": "widget", "name": "widgets", "expr": {"agg": "count"}}]})).await);
    assert_eq!(decide_metric(&app, &vault, "widget", "widgets", json!({"action": "approve"})).await.0, 200);
    let out = ok(app.tool("metric_eval", json!({"metric": "widgets"})).await);
    assert_eq!((out["value"].as_f64(), out["n_entities"].as_u64(), out["truncated"].as_bool()), (Some(55.0), Some(55), Some(true)), "{out}");
    assert_eq!(out["entity_ids"].as_array().unwrap().len(), 50);
}

#[tokio::test]
async fn a_metric_view_runs_the_metric_again_and_never_takes_a_number() {
    let app = TestApp::new().await;
    let (vault, _) = cafe_range(&app).await;
    approved_metrics(&app, &vault, json!([{"name": "avg_price", "expr": {"agg": "avg", "property": "price"}, "description": "What a product costs on average"}])).await;

    // a number the model sends is not part of the view: the server's own run is
    let shown = ok(app.tool("show_view", json!({"view": "metric", "metric": "avg_price", "value": 999})).await);
    assert_eq!((shown["view"].as_str(), shown["metric"]["value"].as_f64(), shown["metric"]["unit"].as_str()), (Some("metric"), Some(4.666667), Some("AUD")), "{shown}");
    let mut names: Vec<&str> = shown["entities"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    names.sort();
    assert_eq!(names, ["Latte", "Mocha", "Tea"]);
    let text = shown["text"].as_str().unwrap();
    assert!(text.starts_with("avg_price: 4.67 AUD\nAverage price. What a product costs on average.\nAs of "), "{text}");
    assert!(text.contains("from 3 products") && text.contains("From: "), "{text}");

    let refused = app.tool("show_view", json!({"view": "metric", "metric": "made_up_metric"})).await;
    assert!(err(&refused).contains("no metric \"made_up_metric\""), "{refused}");

    // over MCP, the view reads as its text
    let rpc = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "show_view", "arguments": {"view": "metric", "metric": "avg_price"}}});
    let (_, rpc) = app.http("POST", "/mcp", Some(rpc), true).await;
    assert_eq!(rpc["result"]["content"][0]["text"], rpc["result"]["structuredContent"]["text"], "{rpc}");
    assert!(rpc["result"]["content"][0]["text"].as_str().unwrap().starts_with("avg_price: 4.67 AUD"), "{rpc}");
}

#[tokio::test]
async fn the_chat_answers_a_number_with_metric_eval_and_streams_the_metric_view() {
    let app = TestApp::new().await;
    let (vault, _) = cafe_range(&app).await;
    approved_metrics(&app, &vault, json!([{"name": "avg_price", "expr": {"agg": "avg", "property": "price"}}])).await;

    let call = |i: usize, id: &str, name: &str, args: Value| json!({"index": i, "id": id, "function": {"name": name, "arguments": args.to_string()}});
    let tools = json!({"choices": [{"delta": {"tool_calls": [
        call(0, "c1", "metric_eval", json!({"metric": "avg_price"})),
        call(1, "c2", "show_view", json!({"view": "metric", "metric": "avg_price"})),
    ]}}]});
    let reply = json!({"choices": [{"delta": {"content": "The average price is 4.67 AUD, from 3 products."}}]});
    let sse = |chunk: Value| format!("data: {chunk}\n\ndata: [DONE]\n\n");
    let mock = serve(vec![
        route("POST", "/chat/completions", json!(null)).body("\"role\":\"tool\"").raw(&sse(reply)).header("content-type", "text/event-stream"),
        route("POST", "/chat/completions", json!(null)).raw(&sse(tools)).header("content-type", "text/event-stream"),
    ])
    .await;
    let (status, _) = app.http("PATCH", "/api/settings", Some(json!({"openai_base_url": mock.base})), true).await;
    assert_eq!(status, 200);

    let (_, events) = chat(&app, "What is the average price of our products?").await;
    let names: Vec<&str> = events.iter().filter(|e| e["type"] == "tool_result").map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["metric_eval", "show_view"], "{events:?}");
    let shown = &events.iter().find(|e| e["type"] == "tool_result" && e["name"] == "show_view").unwrap()["view"];
    assert_eq!((shown["view"].as_str(), shown["metric"]["value"].as_f64()), (Some("metric"), Some(4.666667)), "{shown}");
    assert_eq!(events.iter().find(|e| e["type"] == "done").unwrap()["reply"], "The average price is 4.67 AUD, from 3 products.");
    // the model read the server's number and its entity ids in the metric_eval result
    let sent: Vec<_> = mock.requests().into_iter().filter(|r| r.path == "/chat/completions" && r.body.contains("\"role\":\"tool\"")).collect();
    assert!(sent.iter().any(|r| r.body.contains("4.666667") && r.body.contains("entity_ids")), "{:?}", sent.iter().map(|r| &r.body).collect::<Vec<_>>());
}

#[tokio::test]
async fn reads_still_work_on_an_org_database_before_the_metrics_migration() {
    let app = TestApp::new().await;
    write(&app, "Latte", "product").await;
    app.db().await.test_raw().query("REMOVE TABLE ontology_metric").await.unwrap().check().unwrap();
    assert_eq!(kind(&ok(app.tool("ontology_get", json!({})).await), "product")["metrics"], json!([]));
    let refused = app.tool("metric_eval", json!({"metric": "avg_price"})).await;
    assert!(err(&refused).contains("no metric"), "{refused}");
}

#[tokio::test]
async fn suggest_proposes_metrics_the_kinds_properties_can_compute() {
    let answer = json!({
        "properties": [{"kind": "product", "name": "price", "datatype": "money", "unit": "AUD"}],
        "metrics": [
            {"kind": "product", "name": "avg_price", "description": "What a product costs on average", "expr": {"agg": "avg", "property": "price"}, "reason": "prices are in the facts"},
            {"kind": "product", "name": "avg_weight", "expr": {"agg": "avg", "property": "weight"}},
            {"kind": "product", "name": "raw", "expr": {"agg": "count", "sql": "SELECT 1"}},
        ],
    });
    let completion = json!({"choices": [{"message": {"content": answer.to_string()}}]});
    let mock = serve(vec![route("POST", "/chat/completions", completion)]).await;
    let settings = Settings { openai_base_url: mock.base.clone(), openai_api_key: Some("sk-test".into()), openai_chat_model: "test-model".into(), ..test_settings() };
    let app = TestApp::with_settings(settings).await;
    write(&app, "Latte", "product").await;

    let out = ok(app.tool("ontology_suggest", json!({})).await);
    // a metric over a property that does not exist, or with a field that is not part of an expression, is dropped
    assert_eq!((out["mode"].as_str(), out["skipped"].as_i64()), (Some("model"), Some(2)), "{out}");
    let m = metric(&out["ontology"], "product", "avg_price");
    assert_eq!((m["status"].as_str(), m["origin"].as_str(), m["expr_words"].as_str()), (Some("proposed"), Some("suggested"), Some("Average price")));
    let sent: Vec<_> = mock.requests().into_iter().filter(|r| r.path == "/chat/completions").collect();
    assert!(sent[0].body.contains("count_distinct"), "the prompt explains metrics: {}", sent[0].body);
}

// ------------------------------------------------------------ auto-mapping and CSV import

/// A synced record of `source`/`type` with `payload`, as a connector would store it.
async fn seed_record(app: &TestApp, source: &str, type_: &str, external_id: &str, payload: Value) {
    let env: eunomia_backend::cache::search::Envelope = serde_json::from_value(json!({
        "id": format!("{source}:{type_}:{external_id}"), "source": source, "type": type_, "external_id": external_id,
        "title": external_id, "body_text": format!("{type_} {external_id}"), "payload": payload,
    }))
    .unwrap();
    eunomia_backend::cache::search::upsert(&app.db().await, &app.user.id, &env).await.unwrap();
}

/// Six Stripe charges (two with no description, one refunded) and two customers they point at.
async fn stripe_records(app: &TestApp) {
    for c in ["cus_1", "cus_2"] {
        seed_record(app, "stripe", "stripe.customer", c, json!({"email": format!("{c}@example.com")})).await;
    }
    for i in 0..6 {
        let payload = json!({
            "amount": 10.0 + i as f64 * 2.5, "currency": "AUD", "status": if i == 5 { "refunded" } else { "succeeded" },
            "paid": i != 5, "customer": if i % 2 == 0 { "cus_1" } else { "cus_2" }, "created": format!("2026-11-0{}", i + 1),
            "description": if i < 2 { Value::Null } else { json!(format!("Order {i}")) },
            "billing": {"address": {"city": "Melbourne"}},
        });
        seed_record(app, "stripe", "stripe.charge", &format!("ch_{i}"), payload).await;
    }
}

fn field<'a>(profiles: &'a Value, record_type: &str, path: &str) -> &'a Value {
    let p = profiles.as_array().unwrap().iter().find(|p| p["record_type"] == record_type).unwrap_or_else(|| panic!("no {record_type} in {profiles}"));
    p["fields"].as_array().unwrap().iter().find(|f| f["path"] == path).unwrap_or_else(|| panic!("no {path} in {p}"))
}

fn model_app_settings(mock: &eunomia_backend::sources::mock::Mock) -> Settings {
    Settings { openai_base_url: mock.base.clone(), openai_api_key: Some("sk-test".into()), openai_chat_model: "test-model".into(), ..test_settings() }
}

#[tokio::test]
async fn automap_without_a_model_profiles_the_records_fields_for_the_agent() {
    let app = TestApp::new().await;
    stripe_records(&app).await;
    let out = ok(app.tool("ontology_automap", json!({"source": "stripe"})).await);
    assert_eq!(out["mode"], "profile_only", "{out}");
    assert!(out["hint"].as_str().unwrap().contains("ontology_propose"));
    let profiles = &out["profiles"];
    let amount = field(profiles, "stripe.charge", "amount");
    assert_eq!((amount["inferred_type"].as_str(), amount["min"].as_f64(), amount["max"].as_f64()), (Some("number"), Some(10.0), Some(22.5)));
    let status = field(profiles, "stripe.charge", "status");
    assert_eq!((status["inferred_type"].as_str(), status["enum_values"].clone()), (Some("enum"), json!(["refunded", "succeeded"])));
    assert_eq!(field(profiles, "stripe.charge", "description")["null_rate"], 0.333);
    let created = field(profiles, "stripe.charge", "created");
    assert_eq!((created["inferred_type"].as_str(), created["date_format"].as_str()), (Some("date"), Some("YYYY-MM-DD")));
    assert_eq!(field(profiles, "stripe.charge", "paid")["inferred_type"], "bool");
    assert_eq!(field(profiles, "stripe.charge", "billing.address.city")["distinct_count"], 1, "nested paths are walked");
    let customer = field(profiles, "stripe.charge", "customer");
    assert_eq!((customer["inferred_type"].as_str(), customer["is_unique"].as_bool(), customer["sample_size"].as_i64()), (Some("id"), Some(false), Some(6)));
    assert_eq!(field(profiles, "stripe.customer", "email")["is_unique"], true);
    // the customer key was checked against the customer records: every value matches
    let keys = out["key_matches"].as_array().unwrap();
    let key = keys.iter().find(|k| k["field"] == "customer").unwrap_or_else(|| panic!("{out}"));
    assert_eq!((key["target_record_type"].as_str(), key["matched"].as_i64(), key["ratio"].as_f64()), (Some("stripe.customer"), Some(2), Some(1.0)));
    // nothing was proposed, and a narrower call profiles only that record type
    assert_eq!(ok(app.tool("ontology_get", json!({})).await)["kinds"].as_array().unwrap().iter().filter(|k| k["status"] == "proposed").count(), 0);
    let one = ok(app.tool("ontology_automap", json!({"record_type": "stripe.customer"})).await);
    assert_eq!(one["profiles"].as_array().unwrap().len(), 1);
}

const PRICE_LIST: &str = "product,supplier,price AUD,unit,lead_time_days,status,warehouse\n\
Arabica beans 1kg,Bean Co,32.50,kg,5,Active,W1\n\
Oat milk 1L,Dairy Free Ltd,3.80,L,2,Active,W2\n\
Paper cups 8oz,Pack Pty,0.12,each,9,Active,W1\n\
Chai syrup,Bean Co,14.00,bottle,7,Retired,W3\n";

async fn upload_csv(app: &TestApp, name: &str, text: &str) -> String {
    ok(app.tool("document_upload", json!({"filename": name, "text": text})).await)["id"].as_str().unwrap().to_string()
}

fn automap_answer(doc: &str) -> Value {
    let field = |f: &str| json!([f]);
    json!({"record_types": [{
        "source": "document", "record_type": doc, "kind": "supply_item", "description": "Something a supplier sells us",
        "name_field": "product",
        "properties": [
            {"name": "price", "datatype": "money", "unit": "AUD", "description": "What one unit costs", "fields": field("price AUD")},
            {"name": "unit", "datatype": "text", "fields": field("unit")},
            {"name": "lead_time_days", "datatype": "integer", "unit": "days", "fields": field("lead_time_days")},
            {"name": "status", "datatype": "enum", "enum_values": ["Active", "Retired"], "fields": field("status")},
            {"name": "status_again", "datatype": "text", "fields": field("status")},
            {"name": "Bad Name", "datatype": "text", "fields": field("warehouse")},
        ],
        "links": [
            {"field": "supplier", "label": "supplied_by", "to_kind": "organisation", "target": null, "description": "Who sells it"},
            {"field": "warehouse", "label": "stored_in", "to_kind": "warehouse", "target": null},
        ],
    }, {"source": "nowhere", "record_type": "x", "kind": "x"}]})
}

#[tokio::test]
async fn automap_with_a_model_proposes_and_keeps_only_links_whose_values_match() {
    // the model's answer names the document, so it is set once the CSV is uploaded
    let mock = serve(vec![route("POST", "/chat/completions", json!({}))]).await;
    let app = TestApp::with_settings(model_app_settings(&mock)).await;
    let doc = upload_csv(&app, "price-list.csv", PRICE_LIST).await;
    mock.set_routes(vec![route("POST", "/chat/completions", json!({"choices": [{"message": {"content": automap_answer(&doc).to_string()}}]}))]);
    write(&app, "Bean Co", "organisation").await;
    write(&app, "Pack Pty", "organisation").await;

    let out = ok(app.tool("ontology_automap", json!({"document_id": doc})).await);
    assert_eq!(out["mode"], "model", "{out}");
    // the unknown record type, the property whose only field was already taken and the bad name are left out
    assert_eq!(out["skipped"], 3, "{out}");
    let o = &out["ontology"];
    let k = kind(o, "supply_item");
    assert_eq!((k["status"].as_str(), k["origin"].as_str()), (Some("proposed"), Some("suggested")));
    let p = property(o, "supply_item", "price");
    assert_eq!((p["status"].as_str(), p["datatype"].as_str(), p["unit"].as_str()), (Some("proposed"), Some("money"), Some("AUD")));
    assert_eq!(p["source_map"], json!([{"source": "document", "record_type": doc, "path": "price AUD"}]));
    assert!(kind(o, "supply_item")["properties"].as_array().unwrap().iter().all(|p| p["name"] != "status_again"));
    // two of the three suppliers are organisations in the vault: kept, with the share as its reason
    let kept = link(o, "supply_item", "supplied_by", "organisation");
    assert_eq!((kept["status"].as_str(), kept["reason"].as_str()), (Some("proposed"), Some("67% of values match (2 of 3)")));
    // no warehouse kind exists: it is proposed with the link, and an import creates each one named
    assert_eq!(kind(o, "warehouse")["status"], "proposed", "{o}");
    let stored = link(o, "supply_item", "stored_in", "warehouse");
    assert_eq!(stored["source_map"], json!([{"source": "document", "record_type": doc, "path": "warehouse"}]));
    let checks = out["links"].as_array().unwrap();
    let created = checks.iter().find(|c| c["label"] == "stored_in").unwrap();
    assert_eq!((created["kept"].as_bool(), created["matched"].as_i64()), (Some(true), Some(0)), "{created}");
    // a kind that has entities, none of them named in the column: dropped
    write(&app, "Northside Depot", "depot").await;
    let mut answer = automap_answer(&doc);
    answer["record_types"][0]["links"] = json!([{"field": "warehouse", "label": "kept_at", "to_kind": "depot", "target": null}]);
    mock.set_routes(vec![route("POST", "/chat/completions", json!({"choices": [{"message": {"content": answer.to_string()}}]}))]);
    let again = ok(app.tool("ontology_automap", json!({"document_id": doc})).await);
    let dropped = again["links"].as_array().unwrap().iter().find(|c| c["label"] == "kept_at").unwrap().clone();
    assert_eq!((dropped["kept"].as_bool(), dropped["reason"].as_str()), (Some(false), Some("no value matches (0 of 3)")), "{again}");
    assert!(again["ontology"]["links"].as_array().unwrap().iter().all(|l| l["label"] != "kept_at"));
    let mapped = &out["record_types"][0];
    assert_eq!((mapped["kind"].as_str(), mapped["name_field"].as_str()), (Some("supply_item"), Some("product")));
    assert_eq!(mapped["unmapped"], json!([]), "{mapped}");
    // the model read the profile as data
    let sent: Vec<_> = mock.requests().into_iter().filter(|r| r.path == "/chat/completions").collect();
    assert!(sent[0].body.contains("<profiles>") && sent[0].body.contains("Arabica beans 1kg"), "{}", sent[0].body);
}

#[tokio::test]
async fn automap_checks_a_record_key_against_the_target_records_ids() {
    let mock = serve(vec![route("POST", "/chat/completions", json!({}))]).await;
    let app = TestApp::with_settings(model_app_settings(&mock)).await;
    stripe_records(&app).await;
    seed_record(&app, "stripe", "stripe.refund", "re_1", json!({"charge": "ch_missing", "amount": 5})).await;
    let answer = json!({"record_types": [
        {"source": "stripe", "record_type": "stripe.charge", "kind": "charge", "name_field": null,
         "properties": [{"name": "amount", "datatype": "money", "unit": "AUD", "fields": ["amount"]}],
         "links": [{"field": "customer", "label": "paid_by", "to_kind": "customer", "target": {"source": "stripe", "record_type": "stripe.customer"}}]},
        {"source": "stripe", "record_type": "stripe.refund", "kind": "refund", "name_field": null, "properties": [],
         "links": [{"field": "charge", "label": "refunds", "to_kind": "charge", "target": {"source": "stripe", "record_type": "stripe.charge"}}]},
    ]});
    mock.set_routes(vec![route("POST", "/chat/completions", json!({"choices": [{"message": {"content": answer.to_string()}}]}))]);
    // the vault already has this link type, spelt with a space: the model's paid_by joins it
    ok(app.tool("ontology_propose", json!({"links": [{"from_kind": "charge", "label": "paid by", "to_kind": "customer"}]})).await);
    let out = ok(app.tool("ontology_automap", json!({"source": "stripe"})).await);
    let o = &out["ontology"];
    assert_eq!(link(o, "charge", "paid by", "customer")["reason"], "100% of values match (2 of 2)");
    assert!(o["links"].as_array().unwrap().iter().all(|l| l["label"] != "paid_by"), "no near twin: {o}");
    assert!(o["links"].as_array().unwrap().iter().all(|l| l["label"] != "refunds"), "a refund of a charge that is not there: {o}");
    let mapped = out["record_types"].as_array().unwrap().iter().find(|t| t["kind"] == "charge").unwrap();
    assert!(mapped["unmapped"].as_array().unwrap().contains(&json!("status")), "{mapped}");
}

/// The price list mapped and approved as an owner would: kind supply_item with price, lead_time_days and status.
async fn approved_price_list(app: &TestApp, doc: &str) -> String {
    let vault = personal(app).await;
    let source = |path: &str| json!([{"source": "document", "record_type": doc, "path": path}]);
    ok(app
        .tool(
            "ontology_propose",
            json!({
                "kinds": [{"kind": "supply_item"}],
                "properties": [
                    {"kind": "supply_item", "name": "price", "datatype": "money", "unit": "AUD", "source_map": source("price AUD")},
                    {"kind": "supply_item", "name": "lead_time_days", "datatype": "integer", "source_map": source("lead_time_days")},
                    {"kind": "supply_item", "name": "status", "datatype": "enum", "enum_values": ["Active", "Retired"], "source_map": source("status")},
                    {"kind": "supply_item", "name": "unit", "datatype": "text", "source_map": source("unit")},
                ],
            }),
        )
        .await);
    vault
}

async fn approve_price_list(app: &TestApp, vault: &str) {
    assert_eq!(decide_kind(app, vault, "supply_item", json!({"action": "approve"})).await.0, 200);
    for p in ["price", "lead_time_days", "status"] {
        assert_eq!(decide_property(app, vault, "supply_item", p, json!({"action": "approve"})).await.0, 200);
    }
}

async fn import(app: &TestApp, doc: &str) -> Value {
    app.tool("document_import_rows", json!({"document_id": doc, "kind": "supply_item", "name_column": "product"})).await
}

/// The vault's supply_item entities, `id` and `name` each.
async fn supply_items(app: &TestApp) -> Vec<Value> {
    let rows: Vec<Value> =
        app.db().await.test_raw().query("SELECT <string> id AS id, name FROM entity WHERE kind = 'supply_item'").await.unwrap().take(0).unwrap();
    rows
}

#[tokio::test]
async fn a_csv_profiles_and_imports_only_after_the_owner_approves() {
    let app = TestApp::new().await;
    let doc = upload_csv(&app, "price-list.csv", PRICE_LIST).await;
    // the profile, as the document view shows it
    let (status, prof) = app.http("GET", &format!("/api/documents/{doc}/profile"), None, true).await;
    assert_eq!(status, 200, "{prof}");
    assert_eq!((prof["rows"].as_i64(), prof["columns"].as_array().map(Vec::len)), (Some(4), Some(7)));
    let price = prof["profile"]["fields"].as_array().unwrap().iter().find(|f| f["path"] == "price AUD").unwrap();
    assert_eq!((price["inferred_type"].as_str(), price["min"].as_f64(), price["max"].as_f64()), (Some("number"), Some(0.12), Some(32.5)));
    assert_eq!(prof["import"]["ready"], false);

    // before anything is approved the import is refused, and writes nothing
    let refused = import(&app, &doc).await;
    assert!(err(&refused).contains("not approved"), "{refused}");
    let vault = approved_price_list(&app, &doc).await;
    let refused = import(&app, &doc).await;
    assert!(err(&refused).contains("not approved"), "a proposed kind is not enough: {refused}");
    assert_eq!(decide_kind(&app, &vault, "supply_item", json!({"action": "approve"})).await.0, 200);
    let refused = import(&app, &doc).await;
    assert!(err(&refused).contains("no approved property"), "{refused}");
    assert!(supply_items(&app).await.is_empty());

    approve_price_list(&app, &vault).await;
    let (_, prof) = app.http("GET", &format!("/api/documents/{doc}/profile"), None, true).await;
    assert_eq!((prof["import"]["ready"].as_bool(), prof["import"]["kind"].as_str(), prof["import"]["name_column"].as_str()), (Some(true), Some("supply_item"), Some("product")));

    let out = ok(import(&app, &doc).await);
    assert_eq!((out["rows"].as_i64(), out["created"].as_i64(), out["updated"].as_i64(), out["values"].as_i64()), (Some(4), Some(4), Some(0), Some(12)), "{out}");
    let unit = out["columns"].as_array().unwrap().iter().find(|c| c["column"] == "unit").unwrap();
    assert!(unit["skipped"].as_str().unwrap().contains("proposed, not approved"), "a proposed property is left out: {unit}");
    let beans = supply_items(&app).await.into_iter().find(|e| e["name"] == "Arabica beans 1kg").expect("imported");
    let got = ok(app.tool("entities_get", json!({"id": beans["id"]})).await);
    let price = got["properties"].as_array().unwrap().iter().find(|p| p["property"] == "price").unwrap().clone();
    assert_eq!((price["value"].as_f64(), price["unit"].as_str(), price["source"].as_str()), (Some(32.5), Some("AUD"), Some(doc.as_str())), "{got}");

    // a metric over the imported values
    ok(app.tool("ontology_propose", json!({"metrics": [{"kind": "supply_item", "name": "avg_price", "expr": {"agg": "avg", "property": "price"}}]})).await);
    assert_eq!(decide_metric(&app, &vault, "supply_item", "avg_price", json!({"action": "approve"})).await.0, 200);
    let avg = ok(app.tool("metric_eval", json!({"metric": "avg_price"})).await);
    assert!((avg["value"].as_f64().unwrap() - 12.605).abs() < 1e-9 && avg["n_entities"] == 4, "{avg}");

    // a new revision of the file: the re-run updates values in place and makes no entity or value row twice
    let changed = PRICE_LIST.replace("32.50", "35.00");
    ok(app.tool("document_upload", json!({"filename": "price-list.csv", "text": changed, "document_id": doc})).await);
    let again = ok(import(&app, &doc).await);
    assert_eq!((again["created"].as_i64(), again["updated"].as_i64(), again["values"].as_i64()), (Some(0), Some(4), Some(12)), "{again}");
    assert_eq!(supply_items(&app).await.len(), 4);
    let rows: Vec<Value> = app.db().await.test_raw().query("SELECT * FROM entity_property").await.unwrap().take(0).unwrap();
    assert_eq!(rows.len(), 12);
    let got = ok(app.tool("entities_get", json!({"id": beans["id"]})).await);
    assert_eq!(got["properties"].as_array().unwrap().iter().find(|p| p["property"] == "price").unwrap()["value"], 35.0);

    // deleting the document takes its imported values with it
    ok(app.tool("document_delete", json!({"id": doc})).await);
    let rows: Vec<Value> = app.db().await.test_raw().query("SELECT * FROM entity_property").await.unwrap().take(0).unwrap();
    assert!(rows.is_empty());
}

#[tokio::test]
async fn a_bad_cell_is_reported_on_its_line_and_the_rest_still_imports() {
    let app = TestApp::new().await;
    let csv = "product,supplier,price AUD,unit,lead_time_days,status,warehouse\n\
Arabica beans 1kg,Bean Co,$32.50,kg,5,Active,W1\n\
,Nobody,1.00,kg,1,Active,W1\n\
Oat milk 1L,Dairy Free Ltd,3.80,L,two,Paused,W2\n\
Paper cups 8oz,Pack Pty,0.12\n\
Chai syrup,Bean Co,14.00,bottle,7,retired,W3\n";
    let doc = upload_csv(&app, "messy.csv", csv).await;
    let vault = approved_price_list(&app, &doc).await;
    approve_price_list(&app, &vault).await;
    let out = ok(import(&app, &doc).await);
    assert_eq!((out["created"].as_i64(), out["bad_count"].as_i64()), (Some(3), Some(4)), "{out}");
    let bad: Vec<(i64, Option<&str>)> = out["bad"].as_array().unwrap().iter().map(|b| (b["line"].as_i64().unwrap(), b["column"].as_str())).collect();
    assert_eq!(bad, [(3, Some("product")), (4, Some("lead_time_days")), (4, Some("status")), (5, None)], "{out}");
    assert!(out["bad"][1]["error"].as_str().unwrap().contains("number"), "{out}");
    // the good cells of a bad row still land; an enum value matches ignoring case
    let items = supply_items(&app).await;
    let chai = items.iter().find(|e| e["name"] == "Chai syrup").unwrap();
    let got = ok(app.tool("entities_get", json!({"id": chai["id"]})).await);
    assert_eq!(got["properties"].as_array().unwrap().iter().find(|p| p["property"] == "status").unwrap()["value"], "Retired");
}

#[tokio::test]
async fn australian_money_percents_and_day_first_dates_import_and_a_repeated_name_is_reported() {
    let app = TestApp::new().await;
    let csv = "product,price AUD,discount,listed\n\
Arabica beans 1kg,\"$1,200.50\",10%,25/12/2026\n\
Paper cups 8oz,A$0.12,2.5 %,01/02/2026\n\
Arabica beans 1kg,$9.99,0%,02/02/2026\n\
Teapot,US$5.00,abc%,03/02/2026\n\
Kettle,\"1.234,56\",5%,04/02/2026\n";
    let doc = upload_csv(&app, "au-prices.csv", csv).await;
    let vault = personal(&app).await;
    let source = |path: &str| json!([{"source": "document", "record_type": doc, "path": path}]);
    ok(app
        .tool(
            "ontology_propose",
            json!({
                "kinds": [{"kind": "supply_item"}],
                "properties": [
                    {"kind": "supply_item", "name": "price", "datatype": "money", "unit": "AUD", "source_map": source("price AUD")},
                    {"kind": "supply_item", "name": "discount", "datatype": "percent", "source_map": source("discount")},
                    {"kind": "supply_item", "name": "listed", "datatype": "date", "source_map": source("listed")},
                ],
            }),
        )
        .await);
    assert_eq!(decide_kind(&app, &vault, "supply_item", json!({"action": "approve"})).await.0, 200);
    for p in ["price", "discount", "listed"] {
        assert_eq!(decide_property(&app, &vault, "supply_item", p, json!({"action": "approve"})).await.0, 200);
    }
    let out = ok(import(&app, &doc).await);
    // the third row repeats the first's name, so it is reported and left out, never merged silently
    // a US dollar amount into an AUD price, a decimal comma, and a non-number percent are refused as
    // written, never stored as another number
    let bad: Vec<(i64, Option<&str>)> = out["bad"].as_array().unwrap().iter().map(|b| (b["line"].as_i64().unwrap(), b["column"].as_str())).collect();
    assert_eq!(bad, [(4, Some("product")), (5, Some("price AUD")), (5, Some("discount")), (6, Some("price AUD"))], "{out}");
    assert!(out["bad"][0]["error"].as_str().unwrap().contains("repeats the name"), "{out}");
    assert!(out["bad"][1]["error"].as_str().unwrap().contains("US$5.00"), "the cell as written: {out}");
    assert!(out["bad"][2]["error"].as_str().unwrap().contains("abc%"), "the cell as written: {out}");
    assert_eq!((out["created"].as_i64(), out["values"].as_i64()), (Some(4), Some(9)), "{out}");

    let value = |got: &Value, p: &str| got["properties"].as_array().unwrap().iter().find(|v| v["property"] == p).unwrap()["value"].clone();
    let items = supply_items(&app).await;
    let beans = ok(app.tool("entities_get", json!({"id": items.iter().find(|e| e["name"] == "Arabica beans 1kg").unwrap()["id"]})).await);
    assert_eq!((value(&beans, "price").as_f64(), value(&beans, "discount").as_f64()), (Some(1200.5), Some(10.0)), "{beans}");
    assert!(value(&beans, "listed").as_str().unwrap().starts_with("2026-12-25"), "{beans}");
    let cups = ok(app.tool("entities_get", json!({"id": items.iter().find(|e| e["name"] == "Paper cups 8oz").unwrap()["id"]})).await);
    assert_eq!((value(&cups, "price").as_f64(), value(&cups, "discount").as_f64()), (Some(0.12), Some(2.5)), "{cups}");
    // day first, as the column's other dates show: 1 February, not 2 January
    assert!(value(&cups, "listed").as_str().unwrap().starts_with("2026-02-01"), "{cups}");
}

#[tokio::test]
async fn a_column_two_properties_claim_is_skipped_until_the_owner_picks() {
    let app = TestApp::new().await;
    let doc = upload_csv(&app, "price-list.csv", PRICE_LIST).await;
    let vault = approved_price_list(&app, &doc).await;
    approve_price_list(&app, &vault).await;
    // an agent maps the price column onto a second property too: while it is only proposed, the
    // approved mapping still holds
    let source = json!([{"source": "document", "record_type": doc, "path": "price AUD"}]);
    ok(app.tool("ontology_propose", json!({"properties": [{"kind": "supply_item", "name": "list_price", "datatype": "money", "unit": "AUD", "source_map": source}]})).await);
    let out = ok(import(&app, &doc).await);
    assert!(out["columns"].as_array().unwrap().iter().any(|c| c["column"] == "price AUD" && c["property"] == "price" && c["skipped"].is_null()), "{out}");
    assert_eq!(decide_property(&app, &vault, "supply_item", "list_price", json!({"action": "approve"})).await.0, 200);
    let out = ok(import(&app, &doc).await);
    let price = out["columns"].as_array().unwrap().iter().find(|c| c["column"] == "price AUD").unwrap();
    assert!(price["skipped"].as_str().unwrap().contains("choose one with column_map"), "{out}");
    let (_, prof) = app.http("GET", &format!("/api/documents/{doc}/profile"), None, true).await;
    let planned = prof["import"]["columns"].as_array().unwrap().iter().find(|c| c["column"] == "price AUD").unwrap().clone();
    assert!(planned["property"].is_null(), "no default for a column two properties claim: {planned}");
    // the owner picks one, and the column imports
    let picked = ok(app
        .tool("document_import_rows", json!({"document_id": doc, "kind": "supply_item", "name_column": "product", "column_map": {"price AUD": "price"}}))
        .await);
    assert!(picked["columns"].as_array().unwrap().iter().any(|c| c["column"] == "price AUD" && c["property"] == "price" && c["skipped"].is_null()), "{picked}");
}

#[tokio::test]
async fn an_import_holds_the_row_and_column_caps() {
    let app = TestApp::new().await;
    let rows = format!("product,price AUD\n{}", (0..=eunomia_backend::ontology::tabular::MAX_ROWS).map(|i| format!("p{i},1\n")).collect::<String>());
    let doc = upload_csv(&app, "long.csv", &rows).await;
    let vault = approved_price_list(&app, &doc).await;
    approve_price_list(&app, &vault).await;
    assert!(err(&import(&app, &doc).await).contains("more than 5000 rows"));
    assert!(supply_items(&app).await.is_empty(), "nothing is half imported");
    let header = (0..=eunomia_backend::ontology::tabular::MAX_COLUMNS).map(|i| format!("c{i}")).collect::<Vec<_>>().join(",");
    let wide = upload_csv(&app, "wide.csv", &format!("{header}\n")).await;
    let refused = app.tool("ontology_automap", json!({"document_id": wide})).await;
    assert!(err(&refused).contains("61 columns"), "{refused}");
    // a file that is not a CSV is refused by name
    let md = ok(app.tool("document_upload", json!({"filename": "notes.md", "text": "# notes"})).await)["id"].as_str().unwrap().to_string();
    assert!(err(&app.tool("document_import_rows", json!({"document_id": md, "kind": "supply_item", "name_column": "x"})).await).contains("not a CSV"));
}

#[tokio::test]
async fn extraction_writes_approved_custom_kinds_and_never_rejected_or_proposed_ones() {
    let answer = json!({
        "people": [],
        "other": [
            {"kind": "supply_item", "name": "Arabica beans 1kg", "facts": ["Arabica beans 1kg come from Bean Co."]},
            {"kind": "junk", "name": "Spam thing", "facts": ["junk fact"]},
            {"kind": "draft_kind", "name": "Draft thing", "facts": ["draft fact"]},
        ],
    });
    let mock = serve(vec![route("POST", "/chat/completions", json!({"choices": [{"message": {"content": answer.to_string()}}]}))]).await;
    let app = TestApp::with_settings(model_app_settings(&mock)).await;
    let vault = personal(&app).await;
    ok(app.tool("ontology_propose", json!({"kinds": [{"kind": "supply_item", "description": "Something a supplier sells us"}, {"kind": "draft_kind"}]})).await);
    assert_eq!(decide_kind(&app, &vault, "supply_item", json!({"action": "approve"})).await.0, 200);
    assert_eq!(decide_kind(&app, &vault, "junk", json!({"action": "reject"})).await.0, 200);

    let db = app.db().await;
    let record = eunomia_backend::entities::extract::ExtractRecord {
        id: "notes:note:1".into(),
        title: "Order notes".into(),
        body_text: "We order Arabica beans 1kg from Bean Co every week, and the spam thing too.".into(),
        occurred_at: None,
        vault: None,
        source: "notes".into(),
        payload: Value::Null,
        owner_name: None,
    };
    // as the extraction job runs it: with the system's credential
    eunomia_backend::authz::as_system(eunomia_backend::entities::extract::extract_entities(&db, &app.state.settings, &app.user.id, &record)).await.unwrap();
    let sent: Vec<_> = mock.requests().into_iter().filter(|r| r.path == "/chat/completions").collect();
    let prompt = &sent[0].body;
    assert!(prompt.contains("supply_item: Something a supplier sells us"), "{prompt}");
    assert!(!prompt.contains("junk") && !prompt.contains("draft_kind"), "{prompt}");
    let o = ok(app.tool("ontology_get", json!({})).await);
    assert_eq!(kind(&o, "supply_item")["entities"], 1);
    assert_eq!(kind(&o, "junk")["entities"], 0);
    assert_eq!(kind(&o, "draft_kind")["entities"], 0);
}

// ------------------------------------------------------------ the what-if sandbox

/// The cafe range with `total_price` (sum), `avg_price` (avg) and `products` (count) approved.
async fn cafe_with_metrics(app: &TestApp) -> (String, std::collections::HashMap<&'static str, String>) {
    let (vault, ids) = cafe_range(app).await;
    approved_metrics(app, &vault, json!([
        {"name": "total_price", "expr": {"agg": "sum", "property": "price"}},
        {"name": "avg_price", "expr": {"agg": "avg", "property": "price"}},
        {"name": "products", "expr": {"agg": "count"}},
    ])).await;
    (vault, ids)
}

fn what_if(changes: Value, extra: Value) -> Value {
    let mut args = json!({"changes": changes, "metrics": ["total_price", "avg_price", "products"]});
    args.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
    args
}

/// A metric's baseline, scenario and delta values.
type Move = (Option<f64>, Option<f64>, Option<f64>);

/// Each metric's move, by name.
fn moves(out: &Value) -> std::collections::HashMap<String, Move> {
    let base = out["baseline"].as_array().unwrap();
    let after = out["scenario"].as_array().unwrap();
    base.iter().zip(after).map(|(b, s)| (b["metric"].as_str().unwrap().to_string(), (b["value"].as_f64(), s["value"].as_f64(), s["delta"].as_f64()))).collect()
}

fn changed_ids(out: &Value) -> Vec<String> {
    let mut v: Vec<String> = out["changed"].as_array().unwrap().iter().map(|c| c["entity_id"].as_str().unwrap().to_string()).collect();
    v.sort();
    v
}

#[tokio::test]
async fn a_what_if_takes_a_metric_by_name_and_kind_as_its_schema_says() {
    let schema = eunomia_backend::tools::registry::find("scenario_run").unwrap().schema;
    let forms = schema["properties"]["metrics"]["items"]["anyOf"].as_array().cloned().unwrap_or_default();
    assert!(forms.iter().any(|f| f["type"] == "string"), "{schema}");
    assert!(forms.iter().any(|f| f["type"] == "object" && f["required"] == json!(["metric", "kind"])), "the {{metric, kind}} form is advertised: {schema}");
    let app = TestApp::new().await;
    let (_, ids) = cafe_with_metrics(&app).await;
    let args = json!({"changes": [{"remove": {"entity_id": ids["Tea"]}}], "metrics": [{"metric": "total_price", "kind": "product"}]});
    let out = ok(app.tool("scenario_run", args).await);
    assert_eq!(moves(&out)["total_price"], (Some(14.0), Some(11.0), Some(-3.0)), "{out}");
}

#[tokio::test]
async fn a_what_if_moves_each_metric_by_the_amount_worked_out_by_hand() {
    let app = TestApp::new().await;
    let (_, ids) = cafe_with_metrics(&app).await;
    let run = |changes: Value| app.tool("scenario_run", what_if(changes, json!({})));

    // the real numbers: Latte 5 + Mocha 6 + Tea 3 = 14 over 3 priced products; 4 products in all
    // shift: the latte up 30 cents gives 14.3, an average of 14.3 / 3
    let out = ok(run(json!([{"shift": {"kind": "product", "property": "price", "delta": 0.3, "entity_ids": [ids["Latte"]]}}])).await);
    let m = moves(&out);
    assert_eq!(m["total_price"], (Some(14.0), Some(14.3), Some(0.3)), "{out}");
    assert_eq!(m["avg_price"], (Some(4.666667), Some(4.766667), Some(0.1)), "{out}");
    assert_eq!(m["products"], (Some(4.0), Some(4.0), Some(0.0)), "{out}");
    assert_eq!(out["scenario"][0]["delta_pct"].as_f64(), Some(2.14), "0.3 of 14 is 2.14%: {out}");
    assert_eq!(out["changed"], json!([{"entity_id": ids["Latte"], "name": "Latte", "kind": "product", "property": "price", "before": 5.0, "after": 5.3, "unit": "AUD"}]));
    assert_eq!(out["changes"], json!(["Latte price up 0.30 AUD"]));

    // scale: Riverside Roasters' two products up 10% gives 5.5 + 6.6 + 3 = 15.1
    let riverside = json!([{"property": "supplier", "op": "eq", "value": "Riverside Roasters"}]);
    let out = ok(run(json!([{"scale": {"kind": "product", "property": "price", "factor": 1.1, "filters": riverside}}])).await);
    assert_eq!(moves(&out)["total_price"], (Some(14.0), Some(15.1), Some(1.1)), "{out}");
    assert_eq!(out["changes"][0], "Product price up 10% on 2 products where supplier is Riverside Roasters");

    // set: the tea at 4 gives 15
    let out = ok(run(json!([{"set": {"entity_id": ids["Tea"], "property": "price", "value": 4}}])).await);
    assert_eq!(moves(&out)["total_price"], (Some(14.0), Some(15.0), Some(1.0)), "{out}");
    assert_eq!(out["changes"], json!(["Tea price set to 4.00 AUD"]));

    // in order: the latte's 5.3 is then scaled with the mocha's 6, so 5.83 + 6.6 + 3 = 15.43
    let out = ok(run(json!([
        {"shift": {"kind": "product", "property": "price", "delta": 0.3, "entity_ids": [ids["Latte"]]}},
        {"scale": {"kind": "product", "property": "price", "factor": 1.1, "filters": [{"property": "size", "op": "eq", "value": "Large"}]}},
    ])).await);
    assert_eq!(moves(&out)["total_price"].1, Some(15.43), "{out}");
    let latte = out["changed"].as_array().unwrap().iter().find(|c| c["name"] == "Latte").unwrap();
    assert_eq!((latte["before"].as_f64(), latte["after"].as_f64()), (Some(5.0), Some(5.83)), "before is the real value, after the final one");
    assert_eq!(out["n_changed"], 2);
}

#[tokio::test]
async fn a_scale_changes_only_the_entities_its_filters_and_relations_pick() {
    let app = TestApp::new().await;
    let (_, ids) = cafe_with_metrics(&app).await;
    let scale = |pick: Value| {
        let mut s = json!({"kind": "product", "property": "price", "factor": 2});
        s.as_object_mut().unwrap().extend(pick.as_object().cloned().unwrap());
        app.tool("scenario_run", what_if(json!([{"scale": s}]), json!({})))
    };
    let out = ok(scale(json!({"filters": [{"property": "size", "op": "eq", "value": "large"}]})).await);
    assert_eq!(changed_ids(&out), ids_of(&["Latte", "Mocha"], &ids), "{out}");
    let out = ok(scale(json!({"filters": [{"property": "price", "op": "lt", "value": 5.5}, {"property": "size", "op": "in", "value": ["Small", "Large"]}]})).await);
    assert_eq!(changed_ids(&out), ids_of(&["Latte", "Tea"], &ids), "{out}");
    // the scone has no price, so even with every product picked it is not changed, and the warning says so
    let out = ok(scale(json!({})).await);
    assert_eq!(changed_ids(&out), ids_of(&["Latte", "Mocha", "Tea"], &ids));
    assert!(out["warnings"].to_string().contains("1 picked product have no price value"), "{out}");
    // a filter value listed nowhere is refused, not read as picking nothing
    assert!(err(&scale(json!({"filters": [{"property": "size", "op": "eq", "value": "Medum"}]})).await).contains("nearest"));

    // related_to: only the products linked to Bean Co by supplied_by
    let bean_co = write(&app, "Bean Co", "organisation").await["entity"]["id"].as_str().unwrap().to_string();
    for (from, label) in [("Latte", "supplied by"), ("Tea", "supplied by"), ("Mocha", "competes with")] {
        ok(app.tool("code_relate", json!({"from_id": ids[from], "to_id": bean_co, "label": label})).await);
    }
    let out = ok(scale(json!({"related_to": {"entity_id": bean_co, "label": "supplied_by"}})).await);
    assert_eq!(changed_ids(&out), ids_of(&["Latte", "Tea"], &ids), "{out}");
    assert_eq!(out["changes"][0], "Product price up 100% on 2 products supplied by Bean Co");
    assert_eq!(moves(&out)["total_price"].1, Some(22.0), "10 + 6 + 6");
    let out = ok(scale(json!({"related_to": {"entity_id": bean_co}})).await);
    assert_eq!(changed_ids(&out), ids_of(&["Latte", "Mocha", "Tea"], &ids), "any relation without a label");

    // a supplier no product is linked to: the change says so in words, and warns
    let leaf = write(&app, "Leaf Co", "organisation").await["entity"]["id"].as_str().unwrap().to_string();
    let out = ok(scale(json!({"related_to": {"entity_id": leaf, "label": "supplied_by"}})).await);
    assert!(changed_ids(&out).is_empty(), "{out}");
    assert_eq!(out["changes"][0], "Product price up 100%: no products are linked to Leaf Co");
    assert!(out["warnings"].to_string().contains("no products are linked to Leaf Co by supplied by links"), "{out}");
}

#[tokio::test]
async fn a_removed_entity_leaves_the_metrics_and_is_never_reached() {
    let app = TestApp::new().await;
    let (_, ids) = cafe_with_metrics(&app).await;
    let bean_co = write(&app, "Bean Co", "organisation").await["entity"]["id"].as_str().unwrap().to_string();
    for from in ["Latte", "Mocha"] {
        ok(app.tool("code_relate", json!({"from_id": ids[from], "to_id": bean_co, "label": "supplied_by"})).await);
    }
    let reached = |out: &Value| -> Vec<String> {
        let mut v: Vec<String> = out["reached"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap().to_string()).collect();
        v.sort();
        v
    };
    // without the removal, Bean Co reaches both its products
    let out = ok(app.tool("scenario_run", what_if(json!([{"set": {"entity_id": ids["Tea"], "property": "price", "value": 3}}]), json!({"impact_from": [bean_co], "depth": 1}))).await);
    assert_eq!(reached(&out), ["Latte", "Mocha"], "{out}");
    let latte = out["reached"].as_array().unwrap().iter().find(|r| r["name"] == "Latte").unwrap();
    assert_eq!((latte["depth"].as_u64(), latte["via"].as_str(), latte["from"].as_str()), (Some(1), Some("Latte supplied by Bean Co"), Some(bean_co.as_str())));
    assert_eq!(latte["path"].as_array().unwrap().len(), 1);

    let out = ok(app.tool("scenario_run", what_if(json!([{"remove": {"entity_id": ids["Mocha"]}}]), json!({"impact_from": [bean_co], "depth": 1}))).await);
    let m = moves(&out);
    assert_eq!(m["products"], (Some(4.0), Some(3.0), Some(-1.0)), "{out}");
    assert_eq!(m["total_price"], (Some(14.0), Some(8.0), Some(-6.0)), "{out}");
    assert_eq!(reached(&out), ["Latte"], "the mocha is gone, so it is not reached: {out}");
    assert_eq!(out["removed"], json!([{"entity_id": ids["Mocha"], "name": "Mocha", "kind": "product"}]));
    assert_eq!(out["changes"], json!(["Mocha removed"]));
    // what the removed entity was linked to is reached from it
    let out = ok(app.tool("scenario_run", what_if(json!([{"remove": {"entity_id": ids["Mocha"]}}]), json!({"depth": 1}))).await);
    assert_eq!(reached(&out), ["Bean Co"], "{out}");
}

/// Rows in every table a what-if could touch.
async fn row_counts(app: &TestApp) -> Value {
    let mut res = app
        .db()
        .await
        .test_raw()
        .query("RETURN { values: count(SELECT id FROM entity_property), entities: count(SELECT id FROM entity), orgs: count(SELECT id FROM organisation), relations: count(SELECT id FROM relates_to), memories: count(SELECT id FROM memory), properties: count(SELECT id FROM ontology_property), metrics: count(SELECT id FROM ontology_metric) }")
        .await
        .unwrap();
    res.take::<Option<Value>>(0).unwrap().unwrap()
}

#[tokio::test]
async fn a_what_if_writes_nothing() {
    let app = TestApp::new().await;
    let (_, ids) = cafe_with_metrics(&app).await;
    let bean_co = write(&app, "Bean Co", "organisation").await["entity"]["id"].as_str().unwrap().to_string();
    ok(app.tool("code_relate", json!({"from_id": ids["Latte"], "to_id": bean_co, "label": "supplied_by"})).await);
    let before = row_counts(&app).await;
    let changes = json!([
        {"set": {"entity_id": ids["Tea"], "property": "price", "value": 9}},
        {"scale": {"kind": "product", "property": "price", "factor": 3, "related_to": {"entity_id": bean_co}}},
        {"shift": {"kind": "product", "property": "price", "delta": -1}},
        {"remove": {"entity_id": bean_co}},
        {"remove": {"entity_id": ids["Mocha"]}},
    ]);
    ok(app.tool("scenario_run", what_if(changes.clone(), json!({"impact_from": [ids["Latte"]]}))).await);
    ok(app.tool("show_view", json!({"view": "scenario", "headline": "Prices move", "scenario": what_if(changes, json!({}))})).await);
    assert_eq!(row_counts(&app).await, before);
    assert_eq!(before["values"].as_u64(), Some(14), "{before}");
    // the real numbers are the real ones still
    assert_eq!(ok(app.tool("metric_eval", json!({"metric": "total_price"})).await)["value"].as_f64(), Some(14.0));
    assert_eq!(ok(app.tool("entities_get", json!({"id": ids["Tea"]})).await)["properties"][1]["value"], json!(3.0));
}

#[tokio::test]
async fn a_what_if_refuses_what_a_write_would_refuse() {
    let app = TestApp::new().await;
    let (_, ids) = cafe_with_metrics(&app).await;
    ok(app.tool("ontology_propose", json!({"properties": [{"kind": "product", "name": "weight_g", "datatype": "integer"}]})).await);
    let run = |change: Value| app.tool("scenario_run", what_if(json!([change]), json!({})));
    for (change, says) in [
        (json!({"set": {"entity_id": ids["Tea"], "property": "weight_g", "value": 200}}), "proposed, not approved"),
        (json!({"set": {"entity_id": ids["Tea"], "property": "calories", "value": 200}}), "does not exist in this vault"),
        (json!({"set": {"entity_id": ids["Tea"], "property": "price", "value": "$4.50"}}), "the value must be a number"),
        (json!({"set": {"entity_id": ids["Tea"], "property": "size", "value": "Medium"}}), "takes one of Small, Large"),
        (json!({"scale": {"kind": "product", "property": "supplier", "factor": 1.1}}), "scale changes a number, integer, money or percent property"),
        (json!({"shift": {"kind": "product", "property": "size", "delta": 1}}), "shift changes a number"),
        (json!({"scale": {"kind": "product", "property": "weight_g", "factor": 2}}), "proposed, not approved"),
        (json!({"scale": {"kind": "product", "property": "price", "factor": -1}}), "between 0 and 1000"),
        (json!({"scale": {"kind": "product", "property": "price", "factor": 2, "entity_ids": ["entity:nothere"]}}), "unknown entity"),
        (json!({"remove": {"entity_id": "entity:nothere"}}), "unknown entity"),
        (json!({"drop": {"entity_id": ids["Tea"]}}), "unknown variant"),
    ] {
        let out = run(change.clone()).await;
        assert!(err(&out).contains(says), "{change}: {out}");
    }
    let one = json!({"set": {"entity_id": ids["Tea"], "property": "price", "value": 4}});
    let many: Vec<Value> = (0..21).map(|_| one.clone()).collect();
    assert!(err(&app.tool("scenario_run", what_if(json!(many), json!({}))).await).contains("1 to 20 changes"));
    assert!(err(&app.tool("scenario_run", what_if(json!([]), json!({}))).await).contains("1 to 20 changes"));
    assert!(err(&app.tool("scenario_run", what_if(json!([one]), json!({"metrics": ["no_such_metric"]}))).await).contains("no metric \"no_such_metric\""));
}

#[tokio::test]
async fn an_unapproved_metric_runs_in_a_what_if_only_when_asked() {
    let app = TestApp::new().await;
    let (_, ids) = cafe_with_metrics(&app).await;
    ok(app.tool("ontology_propose", json!({"metrics": [{"kind": "product", "name": "highest_price", "expr": {"agg": "max", "property": "price"}}]})).await);
    let args = |allow: bool| json!({"changes": [{"set": {"entity_id": ids["Tea"], "property": "price", "value": 9}}], "metrics": ["highest_price"], "allow_unapproved": allow});
    assert!(err(&app.tool("scenario_run", args(false)).await).contains("proposed, not approved"));
    let out = ok(app.tool("scenario_run", args(true)).await);
    assert_eq!(moves(&out)["highest_price"], (Some(6.0), Some(9.0), Some(3.0)), "{out}");
    assert_eq!(out["scenario"][0]["status"], "proposed");
}

#[tokio::test]
async fn a_scale_past_500_entities_changes_500_and_says_so() {
    let app = TestApp::new().await;
    let vault = personal(&app).await;
    ok(app.tool("ontology_propose", json!({"properties": [price("money")]})).await);
    assert_eq!(decide_property(&app, &vault, "product", "price", json!({"action": "approve"})).await.0, 200);
    approved_metrics(&app, &vault, json!([{"name": "total_price", "expr": {"agg": "sum", "property": "price"}}])).await;
    for batch in 0..6 {
        let items: Vec<Value> = (0..100).map(|i| json!({"subject_name": format!("Cup {}", batch * 100 + i), "subject_kind": "product", "text": "a cup"})).collect();
        ok(app.tool("memory_write_batch", json!({"items": items})).await);
    }
    // a price of 1 on every cup, written as entity_set_property writes it
    let mut res = app
        .db()
        .await
        .test_raw()
        .query("LET $p = (SELECT VALUE id FROM ONLY ontology_property WHERE name = 'price' LIMIT 1); FOR $e IN (SELECT VALUE id FROM entity WHERE kind = 'product') { CREATE entity_property SET vault = $e.vault, entity = $e, property = $p, value_num = 1, as_of = time::now() - 1h, origin = 'agent' }; RETURN count(SELECT id FROM entity_property)")
        .await
        .unwrap();
    assert_eq!(res.take::<Option<i64>>(2).unwrap(), Some(600));
    let out = ok(app.tool("scenario_run", json!({"changes": [{"scale": {"kind": "product", "property": "price", "factor": 2}}], "metrics": ["total_price"]})).await);
    assert_eq!(out["n_changed"], 500, "{}", out["warnings"]);
    assert_eq!(out["changed"].as_array().unwrap().len(), 50);
    assert_eq!(moves(&out)["total_price"], (Some(600.0), Some(1100.0), Some(500.0)), "500 doubled, 100 as they were");
    assert!(out["warnings"].to_string().contains("this change picks 600 products; a what-if changes at most 500"), "{}", out["warnings"]);
}

#[tokio::test]
async fn a_what_if_reads_only_its_own_vault() {
    let app = TestApp::new().await;
    let (_, ids) = cafe_with_metrics(&app).await;
    ok(app.tool("vault_create", json!({"name": "Team", "kind": "org"})).await);
    let team = ok(app.tool("memory_write", json!({"subject_name": "Team latte", "subject_kind": "product", "text": "on the menu", "vault_id": "Team"})).await);
    let team_id = team["entity"]["id"].as_str().unwrap().to_string();
    // a Team entity in a personal what-if, and a personal one in a Team what-if, are both unknown
    for (args, says) in [
        (json!({"changes": [{"remove": {"entity_id": team_id}}]}), "unknown entity"),
        (json!({"changes": [{"set": {"entity_id": ids["Tea"], "property": "price", "value": 1}}], "vault_id": "Team"}), "unknown entity"),
        (json!({"changes": [{"remove": {"entity_id": ids["Tea"]}}], "impact_from": [team_id]}), "unknown entity"),
        (json!({"changes": [{"remove": {"entity_id": ids["Tea"]}}], "metrics": ["total_price"], "vault_id": "Team"}), "unknown entity"),
    ] {
        let out = app.tool("scenario_run", args.clone()).await;
        assert!(err(&out).contains(says), "{args}: {out}");
    }
    // a member of the org who is not in the personal vault cannot run one there
    let bob = common::register(&app.state, "bob@example.com").await;
    let vault = personal(&app).await;
    let got = as_user(&app, &bob, "scenario_run", json!({"changes": [{"remove": {"entity_id": ids["Tea"]}}], "vault_id": vault})).await;
    assert!(got.get("error").is_some() && !got.to_string().contains("Tea"), "{got}");
}

#[tokio::test]
async fn a_scenario_view_is_the_servers_own_run_and_refuses_a_bad_scenario() {
    let app = TestApp::new().await;
    let (_, ids) = cafe_with_metrics(&app).await;
    let scenario = json!({"changes": [{"shift": {"kind": "product", "property": "price", "delta": 0.3, "entity_ids": [ids["Latte"]]}}], "metrics": ["total_price"]});
    let out = ok(app.tool("show_view", json!({"view": "scenario", "headline": "What if the latte costs 30 cents more?", "scenario": scenario})).await);
    assert_eq!((out["view"].as_str(), out["headline"].as_str()), (Some("scenario"), Some("What if the latte costs 30 cents more?")));
    assert_eq!(out["result"]["scenario"][0]["value"].as_f64(), Some(14.3), "{out}");
    let text = out["text"].as_str().unwrap();
    assert!(text.contains("What if: Latte price up 0.30 AUD."), "{text}");
    assert!(text.contains("- total price: 14.00 \u{2192} 14.30 AUD, up 0.30 AUD (2.1%)"), "{text}");
    assert!(text.contains("Changed: Latte price 5.00 AUD \u{2192} 5.30 AUD."), "{text}");
    assert!(text.ends_with("Nothing was saved: this is a what-if."), "{text}");

    // a number from the model has nowhere to go, and a scenario that fails its checks draws nothing
    let mut forged = json!({"view": "scenario", "headline": "x", "scenario": {"changes": [], "metrics": ["total_price"], "baseline": [{"metric": "total_price", "value": 99}]}});
    assert!(err(&app.tool("show_view", forged.clone()).await).contains("unknown field `baseline`"));
    forged["scenario"] = json!({"changes": [{"set": {"entity_id": ids["Tea"], "property": "price", "value": "free"}}]});
    assert!(err(&app.tool("show_view", forged).await).contains("must be a number"));
}

#[tokio::test]
async fn the_chat_runs_a_what_if_and_streams_the_scenario_view() {
    let app = TestApp::new().await;
    let (_, ids) = cafe_with_metrics(&app).await;
    let scenario = json!({"changes": [{"scale": {"kind": "product", "property": "price", "factor": 1.1, "filters": [{"property": "supplier", "op": "eq", "value": "Riverside Roasters"}]}}], "metrics": ["total_price"]});
    let call = |i: usize, id: &str, name: &str, args: Value| json!({"index": i, "id": id, "function": {"name": name, "arguments": args.to_string()}});
    let view = json!({"view": "scenario", "headline": "Riverside Roasters up 10%", "scenario": scenario.clone()});
    let tools = json!({"choices": [{"delta": {"tool_calls": [call(0, "c1", "scenario_run", scenario), call(1, "c2", "show_view", view)]}}]});
    let reply = json!({"choices": [{"delta": {"content": "Total price goes from 14 to 15.10 AUD. Nothing was saved."}}]});
    let sse = |chunk: Value| format!("data: {chunk}\n\ndata: [DONE]\n\n");
    // the second round only answers when the scenario's own numbers reached the model
    let mock = serve(vec![
        route("POST", "/chat/completions", json!(null)).body("\\\"value\\\":15.1").raw(&sse(reply)).header("content-type", "text/event-stream"),
        route("POST", "/chat/completions", json!(null)).raw(&sse(tools)).header("content-type", "text/event-stream"),
    ])
    .await;
    let (status, _) = app.http("PATCH", "/api/settings", Some(json!({"openai_base_url": mock.base})), true).await;
    assert_eq!(status, 200);

    let (_, events) = chat(&app, "What if Riverside Roasters puts prices up 10%?").await;
    let results: Vec<&Value> = events.iter().filter(|e| e["type"] == "tool_result").collect();
    let names: Vec<&str> = results.iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["scenario_run", "show_view"], "{events:?}");
    let shown = &results[1]["view"];
    assert_eq!((shown["view"].as_str(), shown["result"]["scenario"][0]["value"].as_f64()), (Some("scenario"), Some(15.1)), "{shown}");
    assert_eq!(changed_ids(&shown["result"]), ids_of(&["Latte", "Mocha"], &ids));
    let done = events.iter().find(|e| e["type"] == "done").unwrap();
    assert_eq!(done["reply"], "Total price goes from 14 to 15.10 AUD. Nothing was saved.", "{events:?}");
}

/// A price list with `n` rows, each a different item.
fn price_rows(n: usize) -> String {
    let mut csv = String::from("product,supplier,price AUD,unit,lead_time_days,status,warehouse\n");
    for i in 0..n {
        csv.push_str(&format!("Item {i},Supplier {},{}.50,kg,{},{},W{}\n", i % 40, i % 90 + 1, i % 30, if i % 7 == 0 { "Retired" } else { "Active" }, i % 5));
    }
    csv
}

#[tokio::test]
async fn an_import_costs_one_rate_limit_unit_per_500_rows() {
    use eunomia_backend::ratelimit::RateConfig;
    let app = TestApp::new().await;
    let big = upload_csv(&app, "big.csv", &price_rows(2600)).await;
    let vault = approved_price_list(&app, &big).await;
    approve_price_list(&app, &vault).await;
    let import_on = |router: axum::Router, doc: String| {
        let token = app.token.clone();
        async move {
            let args = json!({"document_id": doc, "kind": "supply_item", "name_column": "product"});
            common::http(&router, "POST", "/api/tools/document_import_rows", Some(args), Some(&token), None).await.0
        }
    };
    let limited = || eunomia_backend::app_with(app.state.clone(), RateConfig { token_per_min: 5, user_per_min: 0, ..RateConfig::default() });
    // 2,600 rows cost 6 units, one more than a 5 a minute token has: refused before anything is written
    let (_, out) = import_on(limited(), big.clone()).await;
    assert_eq!(out["code"], "rate.limited", "{out}");
    assert!(supply_items(&app).await.is_empty());
    // 1,000 rows cost 2
    let small = upload_csv(&app, "small.csv", &price_rows(1000)).await;
    let (_, out) = import_on(limited(), small).await;
    assert_eq!(ok(out)["created"], 1000);
}

/// The time a 5,000-row import takes, and that a re-run updates every row in place. Ignored by
/// default: `cargo test --test ontology a_5000_row_import -- --ignored --nocapture` prints the times.
#[tokio::test]
#[ignore]
async fn a_5000_row_import_is_timed_and_reruns_in_place() {
    let app = TestApp::new().await;
    let doc = upload_csv(&app, "big.csv", &price_rows(5000)).await;
    let vault = approved_price_list(&app, &doc).await;
    approve_price_list(&app, &vault).await;
    let started = std::time::Instant::now();
    let out = ok(import(&app, &doc).await);
    let first = started.elapsed();
    assert_eq!((out["created"].as_i64(), out["values"].as_i64(), out["bad_count"].as_i64()), (Some(5000), Some(15000), Some(0)), "{out}");
    let started = std::time::Instant::now();
    let again = ok(import(&app, &doc).await);
    let second = started.elapsed();
    assert_eq!((again["created"].as_i64(), again["updated"].as_i64(), again["values"].as_i64()), (Some(0), Some(5000), Some(15000)), "{again}");
    let rows: Vec<Value> = app.db().await.test_raw().query("SELECT count() AS n FROM entity_property GROUP ALL").await.unwrap().take(0).unwrap();
    assert_eq!(rows[0]["n"], 15000);
    eprintln!("5000-row import: first run {first:?}, re-run {second:?}");
}

// ------------------------------------------------------------ coverage, readiness and strict mode

/// The cafe range with `product` approved and, on it: `avg_price` approved (also called "mean price",
/// "Average selling price of a product"), `gross_margin` proposed, `shelf_life_days` a rejected property
/// and `cheapest` a rejected metric. Returns the vault.
async fn cafe_for_coverage(app: &TestApp) -> String {
    let (vault, _) = cafe_range(app).await;
    assert_eq!(decide_kind(app, &vault, "product", json!({"action": "approve"})).await.0, 200);
    ok(app.tool("ontology_propose", json!({
        "properties": [{"kind": "product", "name": "shelf_life_days", "datatype": "integer", "description": "How many days a product keeps"}],
        "metrics": [
            {"kind": "product", "name": "avg_price", "expr": {"agg": "avg", "property": "price"}, "description": "Average selling price of a product", "synonyms": ["mean price"]},
            {"kind": "product", "name": "gross_margin", "expr": {"agg": "sum", "property": "price"}, "description": "Gross margin on what we sell"},
            {"kind": "product", "name": "cheapest", "expr": {"agg": "min", "property": "price"}, "description": "Lowest unit spend of a product"},
        ],
    })).await);
    assert_eq!(decide_metric(app, &vault, "product", "avg_price", json!({"action": "approve"})).await.0, 200);
    assert_eq!(decide_metric(app, &vault, "product", "cheapest", json!({"action": "reject"})).await.0, 200);
    assert_eq!(decide_property(app, &vault, "product", "shelf_life_days", json!({"action": "reject"})).await.0, 200);
    vault
}

fn matched_names(out: &Value) -> Vec<(String, String, String)> {
    out["matched"].as_array().unwrap().iter().map(|m| (m["phrase"].as_str().unwrap().into(), m["term"].as_str().unwrap().into(), m["name"].as_str().unwrap().into())).collect()
}

#[tokio::test]
async fn coverage_without_a_model_matches_names_and_synonyms_of_approved_terms_only() {
    let app = TestApp::new().await;
    cafe_for_coverage(&app).await;
    let out = ok(app.tool("ontology_coverage", json!({"question": "What is the mean price of our products?"})).await);
    assert_eq!(out["mode"], "words");
    assert_eq!(out["coverage"].as_f64(), Some(1.0), "{out}");
    let s = |a: &str, b: &str, c: &str| (a.to_string(), b.to_string(), c.to_string());
    assert_eq!(matched_names(&out), [s("mean price", "metric", "avg_price"), s("products", "kind", "product")], "a synonym and an approved kind");
    assert!(out["matched"][0]["distance"].as_f64() == Some(0.0));

    // a proposed metric and a rejected property never count
    let out = ok(app.tool("ontology_coverage", json!({"question": "What is our gross margin on oat milk?"})).await);
    assert_eq!((out["coverage"].as_f64(), out["uncovered"].clone()), (Some(0.0), json!(["gross margin", "oat milk"])), "{out}");
    assert!(out["suggestion"].as_str().unwrap().contains("ontology_propose"), "{out}");
    let out = ok(app.tool("ontology_coverage", json!({"question": "What is the shelf life of a product?"})).await);
    assert_eq!((out["coverage"].as_f64(), out["uncovered"].clone()), (Some(0.5), json!(["shelf life"])), "{out}");
    assert!(out["suggestion"].as_str().unwrap().contains("on product"), "names the kind to propose on: {out}");
}

/// A stand-in model: the chat picks phrases by a fixed table, and embeddings are word counts hashed
/// into the vector, so texts that share words are near and others far.
async fn coverage_model(phrases: Vec<(&'static str, Value)>, embeddings_work: bool) -> String {
    use axum::{routing::post, Json, Router};
    use std::hash::{Hash, Hasher};
    fn vector(text: &str) -> Vec<f32> {
        let mut v = vec![0f32; 1536];
        for w in text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            w.hash(&mut h);
            v[(h.finish() % 1536) as usize] += 1.0;
        }
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
        v.into_iter().map(|x| x / n).collect()
    }
    let phrases = std::sync::Arc::new(phrases);
    let chat = {
        let phrases = phrases.clone();
        move |body: String| {
            let phrases = phrases.clone();
            async move {
                let list = phrases.iter().find(|(needle, _)| body.contains(needle)).map(|(_, p)| p.clone()).unwrap_or(json!([]));
                Json(json!({"choices": [{"message": {"content": json!({"phrases": list}).to_string()}}]}))
            }
        }
    };
    let embed = move |Json(body): Json<Value>| async move {
        if !embeddings_work {
            return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": {"message": "down"}})));
        }
        let data: Vec<Value> = body["input"].as_array().unwrap().iter().enumerate().map(|(i, t)| json!({"index": i, "embedding": vector(t.as_str().unwrap())})).collect();
        (axum::http::StatusCode::OK, Json(json!({"data": data})))
    };
    let router = Router::new().route("/v1/chat/completions", post(chat)).route("/v1/embeddings", post(embed));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}/v1")
}

#[tokio::test]
async fn coverage_with_a_model_matches_by_meaning_and_never_by_a_proposed_or_rejected_term() {
    let base = coverage_model(vec![
        ("selling price", json!(["average selling price", "products"])),
        ("unit spend", json!(["lowest unit spend"])),
        ("our gross margin", json!(["gross margin"])),
    ], true)
    .await;
    let app = TestApp::with_settings(Settings { openai_base_url: base, openai_api_key: Some("sk-test".into()), openai_chat_model: "test-model".into(), ..test_settings() }).await;
    cafe_for_coverage(&app).await;

    let out = ok(app.tool("ontology_coverage", json!({"question": "What's the average selling price of our products?"})).await);
    assert_eq!((out["mode"].as_str(), out["coverage"].as_f64()), (Some("model"), Some(1.0)), "{out}");
    let by_meaning = &out["matched"][0];
    assert_eq!((by_meaning["term"].as_str(), by_meaning["name"].as_str()), (Some("metric"), Some("avg_price")), "{out}");
    let d = by_meaning["distance"].as_f64().unwrap();
    assert!(d > 0.0 && d <= 0.6, "matched by embedding distance: {out}");
    assert_eq!(out["matched"][1]["name"], "product");
    // the approved terms keep their vectors
    let rows: Vec<Value> = app.db().await.test_raw().query("SELECT VALUE name FROM ontology_metric WHERE embedding_spec != NONE").await.unwrap().take(0).unwrap();
    assert_eq!(rows, [json!("avg_price")], "only approved terms are embedded");

    // the proposed gross_margin and the rejected cheapest ("Lowest unit spend") are as near as can be, and never match
    let out = ok(app.tool("ontology_coverage", json!({"question": "What is our gross margin?"})).await);
    assert_eq!((out["coverage"].as_f64(), out["uncovered"].clone()), (Some(0.0), json!(["gross margin"])), "{out}");
    let out = ok(app.tool("ontology_coverage", json!({"question": "What is our lowest unit spend?"})).await);
    assert_eq!(out["uncovered"], json!(["lowest unit spend"]), "{out}");
}

#[tokio::test]
async fn readiness_counts_each_sort_by_status_and_names_the_blockers() {
    let app = TestApp::new().await;
    let vault = cafe_for_coverage(&app).await;
    write(&app, "Order 1", "order").await;
    ok(app.tool("ontology_propose", json!({"kinds": [{"kind": "order"}], "properties": [
        {"kind": "product", "name": "weight", "datatype": "number"},
        {"kind": "product", "name": "colour", "datatype": "text"},
    ]})).await);
    assert_eq!(decide_kind(&app, &vault, "order", json!({"action": "approve"})).await.0, 200);
    // a proposed metric over weight, which an owner then rejects
    ok(app.tool("ontology_propose", json!({"metrics": [{"kind": "product", "name": "avg_weight", "expr": {"agg": "avg", "property": "weight"}}]})).await);
    assert_eq!(decide_property(&app, &vault, "product", "weight", json!({"action": "reject"})).await.0, 200);
    // an approved metric over an approved property that holds no values yet
    ok(app.tool("ontology_propose", json!({"properties": [{"kind": "product", "name": "units_on_hand", "datatype": "integer"}], "metrics": [{"kind": "product", "name": "total_units_on_hand", "expr": {"agg": "sum", "property": "units_on_hand"}}]})).await);
    assert_eq!(decide_property(&app, &vault, "product", "units_on_hand", json!({"action": "approve"})).await.0, 200);
    assert_eq!(decide_metric(&app, &vault, "product", "total_units_on_hand", json!({"action": "approve"})).await.0, 200);

    let out = ok(app.tool("ontology_readiness", json!({})).await);
    assert_eq!(out["kinds"], json!({"approved": 2, "proposed": 0, "rejected": 0, "observed": 0}), "{out}");
    assert_eq!(out["properties"], json!({"approved": 5, "proposed": 1, "rejected": 2, "observed": 0}), "{out}");
    assert_eq!(out["metrics"], json!({"approved": 2, "proposed": 2, "rejected": 1, "observed": 0}), "{out}");
    assert_eq!(out["ready"], false);
    // the approved kind order has no metric, and that is not a blocker
    assert_eq!(
        out["blockers"],
        json!([
            "1 property is proposed and needs an owner",
            "2 metrics are proposed and need an owner",
            "metric total units on hand on product uses units on hand, which has no values yet: import them or set them",
        ]),
        "{out}"
    );
    let (status, rest) = app.http("GET", &format!("/api/vaults/{vault}/ontology/readiness"), None, true).await;
    assert_eq!((status.as_u16(), &rest["blockers"]), (200, &out["blockers"]));

    // once nothing waits and every approved metric has its values, the space is ready
    assert_eq!(decide_property(&app, &vault, "product", "colour", json!({"action": "reject"})).await.0, 200);
    for name in ["gross_margin", "avg_weight"] {
        assert_eq!(decide_metric(&app, &vault, "product", name, json!({"action": "reject"})).await.0, 200);
    }
    let found = ok(app.tool("entities_search", json!({"query": "Latte"})).await);
    let latte = found["results"].as_array().unwrap().iter().find(|r| r["name"] == "Latte").unwrap()["id"].as_str().unwrap().to_string();
    ok(app.tool("entity_set_property", json!({"entity_id": latte, "property": "units_on_hand", "value": 4})).await);
    let out = ok(app.tool("ontology_readiness", json!({})).await);
    assert_eq!((out["ready"].as_bool(), out["blockers"].clone()), (Some(true), json!([])), "{out}");
}

async fn set_strict(app: &TestApp, vault: &str, on: bool) -> (u16, Value) {
    let (status, body) = app.http_session("POST", &format!("/api/vaults/{vault}/ontology/strict"), Some(json!({"on": on}))).await;
    (status.as_u16(), body)
}

#[tokio::test]
async fn strict_mode_is_an_owners_switch_in_the_browser_only() {
    let app = TestApp::new().await;
    let vault = personal(&app).await;
    let (status, _) = app.http("POST", &format!("/api/vaults/{vault}/ontology/strict"), Some(json!({"on": true})), true).await;
    assert_eq!(status, 403, "an agent's token cannot turn it on");
    assert_eq!(ok(app.tool("ontology_get", json!({})).await)["strict"], false, "off by default");
    let (status, o) = set_strict(&app, &vault, true).await;
    assert_eq!((status, o["strict"].as_bool()), (200, Some(true)), "{o}");
    let bob = common::register(&app.state, "bob@example.com").await;
    let team_id = ok(app.tool("vault_create", json!({"name": "Team", "kind": "org"})).await)["id"].as_str().unwrap().to_string();
    common::sys(async {
        let db = app.db().await;
        let v = rid::parse(&team_id).unwrap();
        vaults::invite_member(&db, app.control(), &app.user.id, &v, &bob.email, "member").await.unwrap();
        vaults::accept_invitation(&db, app.control(), &bob.id, &v).await.unwrap();
    })
    .await;
    let login = json!({"email": bob.email, "password": PASSWORD});
    let (_, cookie) = common::http(&app.router, "POST", "/api/auth/login", Some(login), None, None).await;
    let cookie = cookie.expect("login sets a cookie").split(';').next().unwrap().to_string();
    let ((status, _), _) = common::http(&app.router, "POST", &format!("/api/vaults/{team_id}/ontology/strict"), Some(json!({"on": true})), None, Some(&cookie)).await;
    assert_eq!(status, 403, "a member who does not own the vault cannot");
    let ((status, _), _) = common::http(&app.router, "GET", &format!("/api/vaults/{team_id}/ontology/readiness"), None, None, Some(&cookie)).await;
    assert_eq!(status, 200, "a member may read readiness");
    assert_eq!(set_strict(&app, &team_id, true).await.0, 200, "the owner in the browser can");
    assert_eq!(ok(app.tool("ontology_get", json!({})).await)["strict"], true, "the personal vault stays as it was set");
}

#[tokio::test]
async fn strict_mode_refuses_unknown_kinds_and_links_files_proposals_and_lets_built_ins_through() {
    let app = TestApp::new().await;
    let vault = personal(&app).await;
    // off: nothing changes
    let widget = write(&app, "Widget 1", "widget").await["entity"]["id"].as_str().unwrap().to_string();
    let ana = write(&app, "Ana", "person").await["entity"]["id"].as_str().unwrap().to_string();
    ok(app.tool("code_relate", json!({"from_id": ana, "to_id": widget, "label": "designed"})).await);
    ok(app.tool("ontology_propose", json!({"kinds": [{"kind": "gadget"}]})).await);
    assert_eq!(write(&app, "Gadget 1", "gadget").await["entity"]["kind"], "gadget", "a proposed kind still takes writes while off");
    assert_eq!(decide_kind(&app, &vault, "widget", json!({"action": "approve"})).await.0, 200);
    assert_eq!(decide_link(&app, &vault, "person", "designed", "widget").await, 200);

    assert_eq!(set_strict(&app, &vault, true).await.0, 200);
    // an unknown kind and a proposed one are refused, naming the approved ones
    for kind in ["sprocket", "gadget"] {
        let out = app.tool("memory_write", json!({"subject_name": "Thing", "subject_kind": kind, "text": "A thing"})).await;
        assert_eq!(out["code"], "ontology.rejected", "{out}");
        let msg = err(&out);
        assert!(msg.contains("only approved kinds") && msg.contains("person") && msg.contains("widget"), "{msg}");
    }
    let o = ok(app.tool("ontology_get", json!({})).await);
    let sprocket = kind(&o, "sprocket");
    assert_eq!((sprocket["status"].as_str(), sprocket["origin"].as_str(), sprocket["entities"].as_i64()), (Some("proposed"), Some("agent"), Some(0)), "filed for the owner: {o}");
    // an approved kind and a built-in pass
    assert_eq!(write(&app, "Widget 2", "widget").await["entity"]["kind"], "widget");
    let bob = write(&app, "Bob", "person").await["entity"]["id"].as_str().unwrap().to_string();
    // an unknown link type is refused and filed; an approved one passes
    let out = app.tool("code_relate", json!({"from_id": bob, "to_id": widget, "label": "broke"})).await;
    assert_eq!(out["code"], "ontology.rejected", "{out}");
    assert!(err(&out).contains("person designed widget"), "names the approved link: {out}");
    let o = ok(app.tool("ontology_get", json!({})).await);
    assert_eq!((link(&o, "person", "broke", "widget")["status"].as_str(), link(&o, "person", "broke", "widget")["count"].as_i64()), (Some("proposed"), Some(0)));
    ok(app.tool("code_relate", json!({"from_id": bob, "to_id": widget, "label": "designed"})).await);

    // off again: the unknown kind is written
    assert_eq!(set_strict(&app, &vault, false).await.0, 200);
    assert_eq!(write(&app, "Sprocket 1", "sprocket").await["entity"]["kind"], "sprocket");
}

async fn decide_link(app: &TestApp, vault: &str, from: &str, label: &str, to: &str) -> u16 {
    let (status, _) = app.http_session("POST", &format!("/api/vaults/{vault}/ontology/links/decide"), Some(json!({"from_kind": from, "label": label, "to_kind": to, "decision": {"action": "approve"}}))).await;
    status.as_u16()
}

#[tokio::test]
async fn extraction_in_a_strict_vault_skips_links_that_are_not_approved_and_files_them() {
    let answer = json!({
        "people": [{"name": "Ana Lee", "facts": ["Ana Lee buys from Bean Co."]}],
        "organisations": [{"name": "Bean Co", "facts": ["Bean Co roasts coffee."]}],
        "other": [{"kind": "supply_item", "name": "Arabica beans 1kg", "facts": ["Arabica beans 1kg come from Bean Co."]}],
        "relations": [
            {"from": "Bean Co", "from_kind": "organisation", "label": "supplies", "to": "Arabica beans 1kg", "to_kind": "supply_item"},
            {"from": "Ana Lee", "from_kind": "person", "label": "buys_from", "to": "Bean Co", "to_kind": "organisation"},
        ],
    });
    let mock = serve(vec![route("POST", "/chat/completions", json!({"choices": [{"message": {"content": answer.to_string()}}]}))]).await;
    let app = TestApp::with_settings(model_app_settings(&mock)).await;
    let vault = personal(&app).await;
    ok(app.tool("ontology_propose", json!({"kinds": [{"kind": "supply_item"}]})).await);
    assert_eq!(decide_kind(&app, &vault, "supply_item", json!({"action": "approve"})).await.0, 200);
    assert_eq!(decide_link(&app, &vault, "organisation", "supplies", "supply_item").await, 200);
    assert_eq!(set_strict(&app, &vault, true).await.0, 200);

    let db = app.db().await;
    let record = eunomia_backend::entities::extract::ExtractRecord {
        id: "notes:note:2".into(),
        title: "Supplier notes".into(),
        body_text: "Ana Lee buys Arabica beans 1kg from Bean Co.".into(),
        occurred_at: None,
        vault: None,
        source: "notes".into(),
        payload: Value::Null,
        owner_name: None,
    };
    eunomia_backend::authz::as_system(eunomia_backend::entities::extract::extract_entities(&db, &app.state.settings, &app.user.id, &record)).await.unwrap();
    let labels: Vec<Value> = db.test_raw().query("SELECT VALUE label FROM relates_to").await.unwrap().take(0).unwrap();
    assert_eq!(labels, [json!("supplies")], "the approved link is written, the other left out");
    let o = ok(app.tool("ontology_get", json!({})).await);
    assert_eq!(kind(&o, "supply_item")["entities"], 1, "the rest of the record is kept");
    assert_eq!(link(&o, "person", "buys_from", "organisation")["status"], "proposed", "and the refused link is filed: {o}");
}

#[tokio::test]
async fn strict_mode_lets_the_code_map_link_its_own_kinds_and_files_nothing_for_them() {
    let app = TestApp::new().await;
    let vault = personal(&app).await;
    let widget = write(&app, "Widget 1", "widget").await["entity"]["id"].as_str().unwrap().to_string();
    assert_eq!(decide_kind(&app, &vault, "widget", json!({"action": "approve"})).await.0, 200);
    assert_eq!(set_strict(&app, &vault, true).await.0, 200);

    let id = |v: Value| ok(v)["id"].as_str().unwrap().to_string();
    let repo = id(app.tool("code_entity_upsert", json!({"kind": "repository", "name": "eunomia"})).await);
    let file = id(app.tool("code_entity_upsert", json!({"kind": "file", "name": "main.rs", "parent_id": repo})).await);
    let main = id(app.tool("code_entity_upsert", json!({"kind": "symbol", "name": "main", "parent_id": file})).await);
    let run = id(app.tool("code_entity_upsert", json!({"kind": "symbol", "name": "run", "parent_id": file})).await);
    ok(app.tool("code_relate", json!({"from_id": main, "to_id": run, "label": "calls"})).await);
    ok(app.tool("code_relate", json!({"from_id": file, "to_id": repo, "label": "imports"})).await);

    let db = app.db().await;
    let mut labels: Vec<String> = db.test_raw().query("SELECT VALUE label FROM relates_to").await.unwrap().take(0).unwrap();
    labels.sort();
    assert_eq!(labels, ["calls", "defined_in", "defined_in", "imports", "part_of"]);
    let filed: Vec<String> = db.test_raw().query("SELECT VALUE label FROM ontology_link").await.unwrap().take(0).unwrap();
    assert!(filed.is_empty(), "nothing is filed for the code map: {filed:?}");

    // a link to a custom kind still needs an approved link type
    let out = app.tool("code_relate", json!({"from_id": main, "to_id": widget, "label": "renders"})).await;
    assert_eq!(out["code"], "ontology.rejected", "{out}");

    // and a label that is not the code map's is reviewed even between code kinds: people written as
    // files cannot be related as reports_to without the owner seeing it
    let alice = id(app.tool("code_entity_upsert", json!({"kind": "file", "name": "Alice"})).await);
    let bob = id(app.tool("code_entity_upsert", json!({"kind": "file", "name": "Bob"})).await);
    let out = app.tool("code_relate", json!({"from_id": alice, "to_id": bob, "label": "reports_to"})).await;
    assert_eq!(out["code"], "ontology.rejected", "{out}");
    let mut filed: Vec<String> = db.test_raw().query("SELECT VALUE label FROM ontology_link").await.unwrap().take(0).unwrap();
    filed.sort();
    assert_eq!(filed, ["renders", "reports_to"], "both refusals are filed for the owner to review");
}

#[tokio::test]
async fn a_strict_refusal_keeps_the_reason_on_an_existing_proposal() {
    let app = TestApp::new().await;
    let vault = personal(&app).await;
    ok(app.tool("ontology_propose", json!({
        "kinds": [{"kind": "gadget", "reason": "Sam wants gadgets tracked"}],
        "links": [{"from_kind": "person", "label": "owns", "to_kind": "organisation", "reason": "Sam wants owners tracked"}],
    })).await);
    let ana = write(&app, "Ana", "person").await["entity"]["id"].as_str().unwrap().to_string();
    let acme = write(&app, "Acme", "organisation").await["entity"]["id"].as_str().unwrap().to_string();
    assert_eq!(set_strict(&app, &vault, true).await.0, 200);

    let out = app.tool("memory_write", json!({"subject_name": "Gadget 1", "subject_kind": "gadget", "text": "A gadget"})).await;
    assert_eq!(out["code"], "ontology.rejected", "{out}");
    let out = app.tool("code_relate", json!({"from_id": ana, "to_id": acme, "label": "owns"})).await;
    assert_eq!(out["code"], "ontology.rejected", "{out}");
    let db = app.db().await;
    let reasons: Vec<String> = db.test_raw().query("SELECT VALUE reason FROM ontology_kind WHERE kind = 'gadget'").await.unwrap().take(0).unwrap();
    assert_eq!(reasons, ["Sam wants gadgets tracked"]);
    let reasons: Vec<String> = db.test_raw().query("SELECT VALUE reason FROM ontology_link WHERE label = 'owns'").await.unwrap().take(0).unwrap();
    assert_eq!(reasons, ["Sam wants owners tracked"]);
}

#[tokio::test]
async fn strict_mode_stops_filing_proposals_once_the_owner_has_a_backlog() {
    let app = TestApp::new().await;
    let vault = personal(&app).await;
    let names: Vec<String> = (0..50u8).map(|i| format!("waiting_{}{}", (b'a' + i / 26) as char, (b'a' + i % 26) as char)).collect();
    ok(app.tool("ontology_propose", json!({"kinds": names.iter().map(|k| json!({"kind": k})).collect::<Vec<_>>()})).await);
    assert_eq!(set_strict(&app, &vault, true).await.0, 200);
    let sprocket = || app.tool("memory_write", json!({"subject_name": "Thing", "subject_kind": "sprocket", "text": "A thing"}));
    let filed = || async { app.db().await.test_raw().query("SELECT VALUE kind FROM ontology_kind WHERE kind = 'sprocket'").await.unwrap().take::<Vec<String>>(0).unwrap() };

    let out = sprocket().await;
    assert_eq!(out["code"], "ontology.rejected", "{out}");
    assert!(err(&out).contains("backlog of 50"), "{out}");
    assert!(filed().await.is_empty(), "not filed while 50 wait");

    // one decided: the next refusal files again
    assert_eq!(decide_kind(&app, &vault, &names[0], json!({"action": "approve"})).await.0, 200);
    let out = sprocket().await;
    assert!(err(&out).contains("It was filed"), "{out}");
    assert_eq!(filed().await, ["sprocket"]);
}

#[tokio::test]
async fn coverage_finds_a_vaults_term_when_another_vault_holds_nearer_ones() {
    let base = coverage_model(vec![("roasts beans", json!(["company that roasts beans"]))], true).await;
    let app = TestApp::with_settings(Settings { openai_base_url: base, openai_api_key: Some("sk-test".into()), openai_chat_model: "test-model".into(), ..test_settings() }).await;
    let vault = cafe_for_coverage(&app).await;
    ok(app.tool("ontology_propose", json!({"kinds": [{"kind": "supplier", "description": "company that roasts beans"}]})).await);
    assert_eq!(decide_kind(&app, &vault, "supplier", json!({"action": "approve"})).await.0, 200);
    // another vault in the org holds more kinds than one kNN read takes, each nearer the phrase
    let team = ok(app.tool("vault_create", json!({"name": "Team", "kind": "org"})).await)["id"].as_str().unwrap().to_string();
    let near = ["alpha", "bravo", "charlie", "delta", "echo"];
    let kinds: Vec<Value> = near.iter().map(|k| json!({"kind": k, "description": "company that roasts beans, company that roasts beans"})).collect();
    ok(app.tool("ontology_propose", json!({"vault_id": team, "kinds": kinds})).await);
    for k in near {
        assert_eq!(decide_kind(&app, &team, k, json!({"action": "approve"})).await.0, 200);
    }
    let question = "Which company that roasts beans do we use?";
    let out = ok(app.tool("ontology_coverage", json!({"vault_id": team, "question": question})).await);
    assert_eq!(out["matched"][0]["term"], "kind", "the team's terms have vectors: {out}");

    let out = ok(app.tool("ontology_coverage", json!({"vault_id": vault, "question": question})).await);
    assert_eq!((out["mode"].as_str(), out["coverage"].as_f64()), (Some("model"), Some(1.0)), "{out}");
    assert_eq!(out["matched"][0]["name"], "supplier", "{out}");
    // with no vault named both are checked; the nearer match answers, and the other is listed
    let out = ok(app.tool("ontology_coverage", json!({"question": question})).await);
    assert_eq!((out["vault"].as_str(), out["also"][0]["vault"].as_str()), (Some(team.as_str()), Some(vault.as_str())), "{out}");
}

#[tokio::test]
async fn coverage_matches_by_words_when_embeddings_fail() {
    let base = coverage_model(vec![("mean price", json!(["mean price", "products"]))], false).await;
    let app = TestApp::with_settings(Settings { openai_base_url: base, openai_api_key: Some("sk-test".into()), openai_chat_model: "test-model".into(), ..test_settings() }).await;
    cafe_for_coverage(&app).await;
    let out = ok(app.tool("ontology_coverage", json!({"question": "What is the mean price of our products?"})).await);
    assert_eq!((out["mode"].as_str(), out["coverage"].as_f64()), (Some("words"), Some(1.0)), "{out}");
    assert_eq!(out["matched"][0]["name"], "avg_price", "{out}");
    assert!(out["warning"].as_str().is_some_and(|w| w.contains("name and synonym")), "{out}");
}

#[test]
fn the_ontology_tool_eval_list_names_only_registered_tools() {
    let evals: Value = serde_json::from_str(include_str!("../../docs/evals/ontology-tools.json")).unwrap();
    let cases = evals["cases"].as_array().unwrap();
    assert!(cases.iter().any(|c| c["order"][0] == "ontology_coverage" && c["order"].as_array().unwrap().contains(&json!("metric_eval"))));
    for c in cases {
        for tool in c["order"].as_array().unwrap().iter().chain(c["never"].as_array().unwrap()) {
            let name = tool.as_str().unwrap();
            assert!(eunomia_backend::tools::registry::find(name).is_some(), "{}: no tool {name}", c["id"]);
        }
    }
}

// ------------------------------------------------------------ a question that names no space

/// An org space "Northside Bikes" beside the personal one: approved kind `part` with an approved
/// money `price` (Groupset 1249, Chain 79.95) and an approved `avg_price` ("average price"). The
/// personal space has none of it. Returns the org vault's id.
async fn northside(app: &TestApp) -> String {
    let team = ok(app.tool("vault_create", json!({"name": "Northside Bikes", "kind": "org"})).await)["id"].as_str().unwrap().to_string();
    let mut ids = Vec::new();
    for name in ["Groupset", "Chain"] {
        let w = ok(app.tool("memory_write", json!({"subject_name": name, "subject_kind": "part", "text": format!("{name} is a part we stock"), "vault_id": "Northside Bikes"})).await);
        ids.push(w["entity"]["id"].as_str().unwrap().to_string());
    }
    ok(app.tool("ontology_propose", json!({"vault_id": "Northside Bikes", "kinds": [{"kind": "part"}],
        "properties": [{"kind": "part", "name": "price", "datatype": "money", "unit": "AUD"}],
        "metrics": [{"kind": "part", "name": "avg_price", "expr": {"agg": "avg", "property": "price"}, "synonyms": ["average price"]}]})).await);
    assert_eq!(decide_kind(app, &team, "part", json!({"action": "approve"})).await.0, 200);
    assert_eq!(decide_property(app, &team, "part", "price", json!({"action": "approve"})).await.0, 200);
    assert_eq!(decide_metric(app, &team, "part", "avg_price", json!({"action": "approve"})).await.0, 200);
    for (id, price) in ids.iter().zip([1249.0, 79.95]) {
        ok(app.tool("entity_set_property", json!({"entity_id": id, "property": "price", "value": price})).await);
    }
    team
}

#[tokio::test]
async fn a_number_question_that_names_no_space_reaches_the_org_space_that_has_its_metric() {
    let app = TestApp::new().await;
    let team = northside(&app).await;
    // someone else's space with a metric of the same name is never checked or named
    let bob = common::register(&app.state, "bob@example.com").await;
    ok(as_user(&app, &bob, "vault_create", json!({"name": "Secret Co", "kind": "org"})).await);
    ok(as_user(&app, &bob, "memory_write", json!({"subject_name": "Widget", "subject_kind": "part", "text": "a part", "vault_id": "Secret Co"})).await);
    ok(as_user(&app, &bob, "ontology_propose", json!({"vault_id": "Secret Co", "properties": [{"kind": "part", "name": "price", "datatype": "money"}],
        "metrics": [{"kind": "part", "name": "avg_price", "expr": {"agg": "avg", "property": "price"}}]})).await);

    let out = ok(app.tool("ontology_coverage", json!({"question": "What is the average price of our parts?"})).await);
    assert_eq!((out["vault"].as_str(), out["vault_name"].as_str(), out["coverage"].as_f64()), (Some(team.as_str()), Some("Northside Bikes"), Some(1.0)), "{out}");
    // naming the personal space still checks only it
    let personal_vault = personal(&app).await;
    let mine = ok(app.tool("ontology_coverage", json!({"question": "What is the average price of our parts?", "vault_id": personal_vault})).await);
    assert_eq!((mine["vault_name"].as_str(), mine["coverage"].as_f64()), (Some("Personal"), Some(0.0)), "{mine}");

    // the other tools read the default space, and name where the thing asked for is
    let pass = "is in Northside Bikes: pass vault_id \"Northside Bikes\"";
    let out = app.tool("metric_eval", json!({"metric": "avg_price"})).await;
    assert!(err(&out).contains(&format!("avg_price {pass}")), "{out}");
    let out = app.tool("ontology_get", json!({"kind": "part"})).await;
    assert!(err(&out).contains(&format!("part {pass}")), "{out}");
    let out = app.tool("scenario_run", json!({"changes": [{"scale": {"kind": "part", "property": "price", "factor": 1.1}}], "metrics": ["avg_price"]})).await;
    assert!(err(&out).contains(&format!("Everything it names {pass}")), "{out}");
    for out in [app.tool("metric_eval", json!({"metric": "avg_price"})).await, app.tool("ontology_get", json!({"kind": "part"})).await] {
        assert!(!out.to_string().contains("Secret Co"), "a space the caller cannot read is never named: {out}");
    }
    // with the space coverage returned, they answer
    let out = ok(app.tool("metric_eval", json!({"metric": "avg_price", "vault_id": team})).await);
    assert_eq!((out["value"].as_f64(), out["n_entities"].as_u64()), (Some(664.475), Some(2)), "{out}");
    let got = ok(app.tool("ontology_get", json!({"kind": "part", "vault_id": "Northside Bikes"})).await);
    assert_eq!(got["kinds"].as_array().unwrap().len(), 1, "{got}");
    // bob is never pointed at Northside Bikes
    let out = as_user(&app, &bob, "metric_eval", json!({"metric": "avg_price"})).await;
    assert!(!out.to_string().contains("Northside"), "{out}");
}

#[tokio::test]
async fn the_chat_answers_a_number_from_the_space_coverage_returned() {
    let app = TestApp::new().await;
    let team = northside(&app).await;
    let question = "What is the average price of our parts?";
    let turn = |c: Value| json!([{"choices": [{"delta": {"tool_calls": [c]}}]}]);
    let _mock = chat_turns(&app, vec![
        ("", turn(tool_call(0, "c1", "ontology_coverage", json!({"question": question})))),
        // the model first forgets the space, then passes the one coverage returned
        ("\"tool_call_id\":\"c1\"", turn(tool_call(0, "c2", "metric_eval", json!({"metric": "avg_price"})))),
        ("\"tool_call_id\":\"c2\"", turn(tool_call(0, "c3", "metric_eval", json!({"metric": "avg_price", "vault_id": team})))),
        // Server formatting must itself be in this turn's evidence before prose quotes it.
        ("\"tool_call_id\":\"c3\"", turn(tool_call(0, "c4", "show_view", json!({"view":"metric","metric":"avg_price","vault_id":team})))),
        ("\"tool_call_id\":\"c4\"", json!([{"choices": [{"delta": {"content": "The average part price is $664.48, from 2 parts."}}]}])),
    ])
    .await;
    let (thread_id, events) = chat(&app, question).await;
    assert_eq!(events.iter().find(|e| e["type"] == "done").unwrap_or_else(|| panic!("{events:?}"))["reply"], "The average part price is $664.48, from 2 parts.");
    let (_, history) = app.http("GET", &format!("/api/chat/threads/{thread_id}/history"), None, true).await;
    let results: Vec<Value> = history.as_array().unwrap().iter().filter(|m| m["role"] == "tool").map(|m| serde_json::from_str(m["content"].as_str().unwrap()).unwrap()).collect();
    assert_eq!((results[0]["vault"].as_str(), results[0]["vault_name"].as_str()), (Some(team.as_str()), Some("Northside Bikes")), "coverage found the org space: {results:?}");
    assert!(err(&results[1]).contains("avg_price is in Northside Bikes"), "{results:?}");
    assert_eq!(results[2]["value"].as_f64(), Some(664.475), "{results:?}");
}

#[tokio::test]
async fn a_plain_word_of_one_approved_property_counts_as_a_match() {
    let app = TestApp::new().await;
    let vault = personal(&app).await;
    write(&app, "Drip line", "product").await;
    ok(app.tool("ontology_propose", json!({"kinds": [{"kind": "product"}],
        "properties": [{"kind": "product", "name": "unit_cost", "datatype": "money", "unit": "AUD"}, {"kind": "product", "name": "sell_price", "datatype": "money", "unit": "AUD"}]})).await);
    assert_eq!(decide_kind(&app, &vault, "product", json!({"action": "approve"})).await.0, 200);
    for p in ["unit_cost", "sell_price"] {
        assert_eq!(decide_property(&app, &vault, "product", p, json!({"action": "approve"})).await.0, 200);
    }
    // the gate's case: "costs" gave 0.0 though unit_cost is approved
    let out = ok(app.tool("ontology_coverage", json!({"question": "What are our costs?"})).await);
    assert_eq!((out["coverage"].as_f64(), matched_names(&out)), (Some(1.0), vec![("costs".into(), "property".into(), "unit_cost".into())]), "{out}");
    // a second property with the word makes it a guess, so neither is taken
    ok(app.tool("ontology_propose", json!({"properties": [{"kind": "product", "name": "freight_cost", "datatype": "money"}]})).await);
    assert_eq!(decide_property(&app, &vault, "product", "freight_cost", json!({"action": "approve"})).await.0, 200);
    let out = ok(app.tool("ontology_coverage", json!({"question": "What are our costs?"})).await);
    assert_eq!((out["coverage"].as_f64(), out["uncovered"].clone()), (Some(0.0), json!(["costs"])), "{out}");
}

/// A space of bob's that the test user cannot read, holding `entity` and a proposed ontology.
async fn bobs_secret_space(app: &TestApp, entity: &str) -> (eunomia_backend::models_user::User, String) {
    let bob = common::register(&app.state, "bob@example.com").await;
    let secret = ok(as_user(app, &bob, "vault_create", json!({"name": "Secret Co", "kind": "org"})).await)["id"].as_str().unwrap().to_string();
    ok(as_user(app, &bob, "memory_write", json!({"subject_name": entity, "subject_kind": "organisation", "text": "a supplier", "vault_id": "Secret Co"})).await);
    ok(as_user(app, &bob, "ontology_propose", json!({"vault_id": "Secret Co", "kinds": [{"kind": "rainfall_reading"}]})).await);
    (bob, secret)
}

#[tokio::test]
async fn a_question_no_space_covers_goes_to_the_space_that_holds_what_it_names() {
    let app = TestApp::new().await;
    let team = northside(&app).await;
    ok(app.tool("memory_write", json!({"subject_name": "Coastal Irrigation Co", "subject_kind": "organisation", "text": "Coastal Irrigation Co supplies our drip line", "vault_id": "Northside Bikes"})).await);
    let (_, secret) = bobs_secret_space(&app, "Coastal Irrigation Co").await;
    let no_leak = |out: &Value| assert!(!out.to_string().contains("Secret Co") && !out.to_string().contains(&secret), "a space the caller cannot read is never named: {out}");

    // no phrase matches an approved term anywhere; the org space holds what the question names
    let out = ok(app.tool("ontology_coverage", json!({"question": "What is the rainfall at Coastal Irrigation?"})).await);
    assert_eq!((out["coverage"].as_f64(), out["vault"].as_str(), out["chosen_by"].as_str()), (Some(0.0), Some(team.as_str()), Some("entities")), "not the personal space: {out}");
    assert!(out["suggestion"].as_str().unwrap().contains("\"Northside Bikes\" was checked because it holds what the question names"), "{out}");
    no_leak(&out);

    // nothing named is held anywhere: the space that has an approved ontology
    let out = ok(app.tool("ontology_coverage", json!({"question": "What is the rainfall in Geelong?"})).await);
    assert_eq!((out["vault"].as_str(), out["chosen_by"].as_str()), (Some(team.as_str()), Some("ontology")), "{out}");
    no_leak(&out);

    // a match still decides, and naming a space still checks only it
    let out = ok(app.tool("ontology_coverage", json!({"question": "What is the average price of our parts?"})).await);
    assert_eq!((out["vault"].as_str(), out["chosen_by"].as_str()), (Some(team.as_str()), Some("coverage")), "{out}");
    let mine = personal(&app).await;
    let out = ok(app.tool("ontology_coverage", json!({"question": "What is the rainfall at Coastal Irrigation?", "vault_id": mine})).await);
    assert_eq!((out["vault"].as_str(), out["chosen_by"].as_str()), (Some(mine.as_str()), Some("vault_id")), "{out}");
}

#[tokio::test]
async fn a_question_no_space_covers_says_so_and_lists_the_readable_spaces() {
    let app = TestApp::new().await;
    ok(app.tool("vault_create", json!({"name": "Empty Co", "kind": "org"})).await);
    let (_, secret) = bobs_secret_space(&app, "Geelong").await;
    let out = ok(app.tool("ontology_coverage", json!({"question": "What is the rainfall in Geelong?"})).await);
    assert_eq!(out["chosen_by"], "none", "{out}");
    let spaces: Vec<&str> = out["spaces"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(spaces, ["Personal", "Empty Co"], "{out}");
    let s = out["suggestion"].as_str().unwrap();
    assert!(s.starts_with("No space you can read covers this question") && s.contains("\"Personal\", \"Empty Co\"") && s.contains("Ask the user"), "{s}");
    assert!(!out.to_string().contains("Secret Co") && !out.to_string().contains(&secret), "{out}");

    // with one readable space nothing changes
    let solo = TestApp::new().await;
    let out = ok(solo.tool("ontology_coverage", json!({"question": "What is the rainfall in Geelong?"})).await);
    assert_eq!((out["chosen_by"].as_str(), out.get("spaces")), (Some("coverage"), None), "{out}");
}

// ------------------------------------------------------------ a write that names no space

/// The gate's owner: a personal space plus "Geelong Garden Supply" with approved kind `product` and
/// strict mode on. Returns the org vault's id.
async fn strict_garden_supply(app: &TestApp) -> String {
    let team = ok(app.tool("vault_create", json!({"name": "Geelong Garden Supply", "kind": "org"})).await)["id"].as_str().unwrap().to_string();
    ok(app.tool("memory_write", json!({"subject_name": "Drip line", "subject_kind": "product", "text": "We sell drip line", "vault_id": "Geelong Garden Supply"})).await);
    assert_eq!(decide_kind(app, &team, "product", json!({"action": "approve"})).await.0, 200);
    assert_eq!(set_strict(app, &team, true).await.0, 200);
    team
}

fn chickens(extra: Value) -> Value {
    let mut args = json!({"subject_name": "Chickens", "subject_kind": "livestock", "text": "We now sell chickens."});
    args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    args
}

#[tokio::test]
async fn a_new_kind_with_no_space_named_is_refused_while_an_org_space_is_strict() {
    let app = TestApp::new().await;
    // a kind the personal space already has, written before the org space existed
    write(&app, "Sourdough", "recipe").await;
    let team = strict_garden_supply(&app).await;

    // the gate's write: no vault_id, a kind the personal space does not have
    let out = app.tool("memory_write", chickens(json!({}))).await;
    assert_eq!(out["code"], "validation.invalid", "{out}");
    let msg = err(&out);
    assert!(msg.contains("vault_id") && msg.contains("\"Geelong Garden Supply\"") && msg.contains("\"Personal\" if it is private"), "names the spaces to pick from: {msg}");
    let found = ok(app.tool("entities_search", json!({"query": "Chickens"})).await);
    assert_eq!(found["total"], 0, "nothing was written: {found}");
    // the batch says so per item, and the rest still writes
    let out = ok(app.tool("memory_write_batch", json!({"items": [chickens(json!({})), {"subject_name": "Ana", "subject_kind": "person", "text": "Ana runs the yard"}]})).await);
    assert_eq!((out["written"].as_u64(), out["results"][0]["code"].as_str(), out["results"][1]["vault_name"].as_str()), (Some(1), Some("validation.invalid"), Some("Personal")), "{out}");

    // named, each space decides for itself: the org space refuses and files it, the personal one takes it
    let out = app.tool("memory_write", chickens(json!({"vault_id": "Geelong Garden Supply"}))).await;
    assert_eq!(out["code"], "ontology.rejected", "{out}");
    let out = ok(app.tool("memory_write", chickens(json!({"vault_id": "Personal"}))).await);
    assert_eq!((out["entity"]["kind"].as_str(), out["vault_name"].as_str()), (Some("livestock"), Some("Personal")), "{out}");
    // then the personal space has the kind, so a later write with no vault goes there as before
    let out = ok(app.tool("memory_write", json!({"subject_name": "Ducks", "subject_kind": "livestock", "text": "Ducks too"})).await);
    assert_eq!(out["vault_name"], "Personal", "{out}");

    // a kind the personal space has, and a built-in kind, still write with no vault_id
    assert_eq!(write(&app, "Focaccia", "recipe").await["vault_name"], "Personal");
    assert_eq!(write(&app, "Jesse", "person").await["vault_name"], "Personal");
    let org = ok(app.tool("memory_write", json!({"subject_name": "Hose", "subject_kind": "product", "text": "We sell hose", "vault_id": team})).await);
    assert_eq!(org["vault_name"], "Geelong Garden Supply", "{org}");

    // entity_types with no vault says which space each kind is in
    let types = ok(app.tool("entity_types", json!({})).await);
    let spaces = |k: &str| types["custom"].as_array().unwrap().iter().find(|c| c["kind"] == k).unwrap_or_else(|| panic!("{k} in {types}"))["spaces"].clone();
    assert_eq!((spaces("product"), spaces("recipe"), spaces("livestock")), (json!(["Geelong Garden Supply"]), json!(["Personal"]), json!(["Personal"])), "{types}");

    // strict mode off: a new kind with no vault goes to the personal space as before
    assert_eq!(set_strict(&app, &team, false).await.0, 200);
    assert_eq!(write(&app, "Hens", "poultry").await["vault_name"], "Personal");
}

#[tokio::test]
async fn with_no_strict_org_space_a_new_kind_with_no_space_named_writes_as_before() {
    let app = TestApp::new().await;
    ok(app.tool("vault_create", json!({"name": "Geelong Garden Supply", "kind": "org"})).await);
    let out = ok(app.tool("memory_write", chickens(json!({}))).await);
    assert_eq!((out["entity"]["kind"].as_str(), out["vault_name"].as_str()), (Some("livestock"), Some("Personal")), "{out}");
}

// ------------------------------------------------------------ the gate's bike shop: links and row maths

const PARTS: &str = include_str!("fixtures/parts.csv");

/// The model's map of parts.csv: kind part, its typed columns, the supplier column as a link to a
/// supplier kind no one has proposed yet, and two metrics with per-row maths.
fn parts_answer(doc: &str) -> Value {
    let p = |name: &str, datatype: &str, unit: Option<&str>, col: &str| json!({"name": name, "datatype": datatype, "unit": unit, "fields": [col]});
    json!({"record_types": [{
        "source": "document", "record_type": doc, "kind": "part", "name_field": "part",
        "properties": [
            p("price", "money", Some("AUD"), "price AUD"), p("cost", "money", Some("AUD"), "cost AUD"),
            p("stock", "integer", None, "stock"), p("reorder_level", "integer", None, "reorder_level"),
            {"name": "status", "datatype": "enum", "enum_values": ["Active", "On order", "Discontinued"], "fields": ["status"]},
            p("last_ordered", "date", None, "last_ordered"),
        ],
        "links": [{"field": "supplier", "label": "supplied_by", "to_kind": "supplier", "target": null}],
        "metrics": [
            {"name": "total_stock_value", "description": "What the stock on hand cost us", "expr": {"agg": "sum", "row": {"op": "mul", "left": "stock", "right": "cost"}}},
            {"name": "gross_margin", "description": "The share of the price we keep", "expr": {"agg": "ratio",
                "numerator": {"agg": "sum", "row": {"op": "sub", "left": "price", "right": "cost"}}, "denominator": {"agg": "sum", "property": "price"}}},
            {"name": "bad_value", "expr": {"agg": "sum", "row": {"op": "mul", "left": "price", "right": "cost"}}},
        ],
    }]})
}

#[tokio::test]
async fn automap_links_parts_to_suppliers_the_import_creates_once_and_row_maths_answer_the_gate() {
    let mock = serve(vec![route("POST", "/chat/completions", json!({}))]).await;
    let app = TestApp::with_settings(model_app_settings(&mock)).await;
    let doc = upload_csv(&app, "parts.csv", PARTS).await;
    mock.set_routes(vec![route("POST", "/chat/completions", json!({"choices": [{"message": {"content": parts_answer(&doc).to_string()}}]}))]);

    // auto-map proposes the supplier kind with the link, since no supplier exists yet
    let out = ok(app.tool("ontology_automap", json!({"document_id": doc})).await);
    let o = &out["ontology"];
    assert_eq!((kind(o, "supplier")["status"].as_str(), kind(o, "supplier")["origin"].as_str()), (Some("proposed"), Some("suggested")), "{o}");
    let l = link(o, "part", "supplied_by", "supplier");
    assert_eq!(l["source_map"], json!([{"source": "document", "record_type": doc, "path": "supplier"}]), "{l}");
    let check = out["links"].as_array().unwrap().iter().find(|c| c["label"] == "supplied_by").unwrap();
    assert_eq!((check["kept"].as_bool(), check["reason"].as_str()), (Some(true), Some("no supplier exists yet: an import creates the 4 named in supplier once each")), "{check}");
    // the metrics with row maths are proposed and read naturally; money times money is not
    let words = |name: &str| metric(o, "part", name)["expr_words"].as_str().unwrap().to_string();
    assert_eq!(words("total_stock_value"), "Sum of stock × cost");
    assert_eq!(words("gross_margin"), "Sum of (price − cost), divided by sum of price");
    assert_eq!(metric(o, "part", "gross_margin")["reason"], "computed from the parts.csv records' properties", "the file's name, never its id: {o}");
    assert!(kind(o, "part")["metrics"].as_array().unwrap().iter().all(|m| m["name"] != "bad_value"), "{o}");

    // the owner approves the map
    let vault = personal(&app).await;
    for k in ["part", "supplier"] {
        assert_eq!(decide_kind(&app, &vault, k, json!({"action": "approve"})).await.0, 200);
    }
    for p in ["price", "cost", "stock", "reorder_level", "status", "last_ordered"] {
        assert_eq!(decide_property(&app, &vault, "part", p, json!({"action": "approve"})).await.0, 200, "{p}");
    }
    for m in ["total_stock_value", "gross_margin"] {
        assert_eq!(decide_metric(&app, &vault, "part", m, json!({"action": "approve"})).await.0, 200, "{m}");
    }
    assert_eq!(decide_link(&app, &vault, "part", "supplied_by", "supplier").await, 200);
    // a later proposal cannot move the columns of a link the owner already decided
    let moved = json!([{"source": "document", "record_type": doc, "path": "price AUD"}]);
    ok(app.tool("ontology_propose", json!({"links": [{"from_kind": "part", "label": "supplied_by", "to_kind": "supplier", "source_map": moved}]})).await);
    let o = ok(app.tool("ontology_get", json!({})).await);
    assert_eq!(link(&o, "part", "supplied_by", "supplier")["source_map"], json!([{"source": "document", "record_type": doc, "path": "supplier"}]), "{o}");
    let (_, profile) = app.http("GET", &format!("/api/documents/{doc}/profile"), None, true).await;
    let plan = profile["import"]["columns"].as_array().unwrap().iter().find(|c| c["column"] == "supplier").unwrap().clone();
    assert_eq!((plan["link"].as_str(), profile["import"]["name_column"].as_str()), (Some("supplied_by supplier"), Some("part")), "{profile}");

    // the import creates each supplier once ("shimano oceania" is Shimano Oceania) and links every part
    let args = json!({"document_id": doc, "kind": "part", "name_column": "part"});
    let first = ok(app.tool("document_import_rows", args.clone()).await);
    assert_eq!((first["created"].as_u64(), first["targets_created"].as_u64(), first["linked"].as_u64(), first["bad_count"].as_u64()), (Some(25), Some(4), Some(25), Some(0)), "{first}");
    let supplier_col = first["columns"].as_array().unwrap().iter().find(|c| c["column"] == "supplier").unwrap().clone();
    assert_eq!(supplier_col["link"], "supplied_by supplier", "{first}");
    let again = ok(app.tool("document_import_rows", args).await);
    assert_eq!((again["created"].as_u64(), again["updated"].as_u64(), again["targets_created"].as_u64(), again["linked"].as_u64()), (Some(0), Some(25), Some(0), Some(0)), "a re-run adds nothing: {again}");
    let o = ok(app.tool("ontology_get", json!({})).await);
    assert_eq!(kind(&o, "supplier")["entities"], 4, "{o}");
    assert_eq!(link(&o, "part", "supplied_by", "supplier")["count"], 25);
    let found = ok(app.tool("entities_search", json!({"query": "Shimano Oceania", "kind": "supplier"})).await);
    let shimano = found["results"].as_array().unwrap().iter().find(|r| r["name"] == "Shimano Oceania").unwrap_or_else(|| panic!("{found}"))["id"].as_str().unwrap().to_string();

    // the numbers, worked out by hand from parts.csv
    let eval = |m: &str| app.tool("metric_eval", json!({"metric": m}));
    let tsv = ok(eval("total_stock_value").await);
    assert_eq!((tsv["value"].as_f64(), tsv["unit"].as_str(), tsv["n_entities"].as_u64()), (Some(14062.25), Some("AUD"), Some(25)), "{tsv}");
    assert_eq!(ok(eval("gross_margin").await)["value"].as_f64(), Some(0.30265));

    // Shimano raises cost 8%: the 7 Shimano parts, picked by the link, move both metrics
    let changes = json!([{"scale": {"kind": "part", "property": "cost", "factor": 1.08, "related_to": {"entity_id": shimano, "label": "supplied_by"}}}]);
    let out = ok(app.tool("scenario_run", json!({"changes": changes, "metrics": ["total_stock_value", "gross_margin"]})).await);
    assert_eq!(out["n_changed"], 7, "{out}");
    assert_eq!(out["changes"][0], "Part cost up 8% on 7 parts supplied by Shimano Oceania");
    let after = |m: &str| out["scenario"].as_array().unwrap().iter().find(|s| s["metric"] == m).unwrap()["value"].as_f64();
    assert_eq!((after("total_stock_value"), after("gross_margin")), (Some(14400.97), Some(0.288303)), "{out}");
}

#[tokio::test]
async fn row_maths_is_typed_checked_for_units_and_skips_a_row_that_divides_by_zero() {
    let app = TestApp::new().await;
    let doc = upload_csv(&app, "parts.csv", PARTS).await;
    let vault = personal(&app).await;
    let source = |col: &str| json!([{"source": "document", "record_type": doc, "path": col}]);
    ok(app.tool("ontology_propose", json!({"kinds": [{"kind": "part"}], "properties": [
        {"kind": "part", "name": "price", "datatype": "money", "unit": "AUD", "source_map": source("price AUD")},
        {"kind": "part", "name": "cost", "datatype": "money", "unit": "AUD", "source_map": source("cost AUD")},
        {"kind": "part", "name": "stock", "datatype": "integer", "source_map": source("stock")},
        {"kind": "part", "name": "status", "datatype": "text", "source_map": source("status")},
    ]})).await);
    assert_eq!(decide_kind(&app, &vault, "part", json!({"action": "approve"})).await.0, 200);
    for p in ["price", "cost", "stock", "status"] {
        assert_eq!(decide_property(&app, &vault, "part", p, json!({"action": "approve"})).await.0, 200);
    }
    ok(app.tool("document_import_rows", json!({"document_id": doc, "kind": "part", "name_column": "part"})).await);

    let propose = |name: &str, expr: Value| app.tool("ontology_propose", json!({"metrics": [{"kind": "part", "name": name, "expr": expr}]}));
    let row = |op: &str, l: Value, r: Value| json!({"op": op, "left": l, "right": r});
    for (expr, says) in [
        (json!({"agg": "sum", "row": row("mul", json!("price"), json!("cost"))}), "money times money"),
        (json!({"agg": "sum", "row": row("sub", json!("price"), json!("stock"))}), "do not add or subtract"),
        (json!({"agg": "sum", "row": row("div", json!("stock"), json!("price"))}), "dividing by money"),
        (json!({"agg": "sum", "row": row("mul", json!("status"), json!(2))}), "is text"),
        (json!({"agg": "sum", "row": row("mul", json!("weight"), json!(2))}), "not a property"),
        (json!({"agg": "count", "row": row("mul", json!("stock"), json!(2))}), "takes no row"),
        (json!({"agg": "sum", "property": "cost", "row": row("mul", json!("stock"), json!(2))}), "not both"),
        (json!({"agg": "sum", "row": row("add", json!(1), row("add", json!(1), row("add", json!(1), row("add", json!(1), json!("stock")))))}), "at most 3 deep"),
    ] {
        let out = propose("refused", expr.clone()).await;
        assert!(err(&out).contains(says), "{expr}: {out}");
    }
    // query text never gets in: a row is typed data, not a formula string
    for expr in [json!({"agg": "sum", "row": "stock * cost"}), json!({"agg": "sum", "row": {"op": "mul", "left": "stock", "right": "cost", "sql": "1"}})] {
        assert!(propose("refused", expr.clone()).await.get("error").is_some(), "{expr}");
    }

    // money times a number is money; a row that divides by 0 is left out and counted
    ok(propose("stock_value_at_price", json!({"agg": "sum", "row": row("mul", json!("stock"), json!("price"))})).await);
    ok(propose("cost_per_unit", json!({"agg": "avg", "row": row("div", json!("cost"), json!("stock"))})).await);
    ok(propose("margin_at_10_off", json!({"agg": "max", "row": row("sub", row("mul", json!("price"), json!(0.9)), json!("cost"))})).await);
    for m in ["stock_value_at_price", "cost_per_unit", "margin_at_10_off"] {
        assert_eq!(decide_metric(&app, &vault, "part", m, json!({"action": "approve"})).await.0, 200);
    }
    let at_price = ok(app.tool("metric_eval", json!({"metric": "stock_value_at_price"})).await);
    assert_eq!((at_price["value"].as_f64(), at_price["unit"].as_str()), (Some(21332.55), Some("AUD")), "{at_price}");
    let per_unit = ok(app.tool("metric_eval", json!({"metric": "cost_per_unit"})).await);
    assert_eq!((per_unit["value"].as_f64(), per_unit["n_entities"].as_u64()), (Some(149.413095), Some(21)), "{per_unit}");
    assert!(per_unit["warnings"].to_string().contains("left out 4 entities whose row divides by 0"), "{per_unit}");
    let off = ok(app.tool("metric_eval", json!({"metric": "margin_at_10_off"})).await);
    assert_eq!((off["expr_words"].as_str(), off["unit"].as_str()), (Some("Highest of ((price × 0.9) − cost)"), Some("AUD")), "{off}");
    assert_eq!(off["value"].as_f64(), Some(219.1), "1249 × 0.9 − 905, the groupset");

    // a row too large for a number is left out and counted, never summed as infinity or null
    ok(propose("cost_times_huge", json!({"agg": "sum", "row": row("mul", json!("cost"), json!(1e308))})).await);
    assert_eq!(decide_metric(&app, &vault, "part", "cost_times_huge", json!({"action": "approve"})).await.0, 200);
    let huge = ok(app.tool("metric_eval", json!({"metric": "cost_times_huge"})).await);
    assert!(huge["warnings"].to_string().contains("left out 25 entities whose row is too large to compute"), "{huge}");
    assert_eq!(huge["n_entities"].as_u64(), Some(0), "{huge}");
}

#[tokio::test]
async fn a_property_named_like_the_column_wins_over_a_link_guessed_from_the_kind_name() {
    let app = TestApp::new().await;
    let doc = upload_csv(&app, "parts.csv", PARTS).await;
    let vault = personal(&app).await;
    ok(app.tool("ontology_propose", json!({
        "kinds": [{"kind": "part"}, {"kind": "supplier"}],
        "properties": [
            {"kind": "part", "name": "supplier", "datatype": "text"},
            {"kind": "part", "name": "price", "datatype": "money", "unit": "AUD", "source_map": [{"source": "document", "record_type": doc, "path": "price AUD"}]},
        ],
        "links": [{"from_kind": "part", "label": "supplied_by", "to_kind": "supplier"}],
    })).await);
    for k in ["part", "supplier"] {
        assert_eq!(decide_kind(&app, &vault, k, json!({"action": "approve"})).await.0, 200);
    }
    for p in ["supplier", "price"] {
        assert_eq!(decide_property(&app, &vault, "part", p, json!({"action": "approve"})).await.0, 200);
    }
    assert_eq!(decide_link(&app, &vault, "part", "supplied_by", "supplier").await, 200);
    let (_, profile) = app.http("GET", &format!("/api/documents/{doc}/profile"), None, true).await;
    let plan = profile["import"]["columns"].as_array().unwrap().iter().find(|c| c["column"] == "supplier").unwrap().clone();
    assert_eq!((plan["property"].as_str(), plan["link"].as_str()), (Some("supplier"), None), "the preview says what the import does: {profile}");
    let out = ok(app.tool("document_import_rows", json!({"document_id": doc, "kind": "part", "name_column": "part"})).await);
    let col = out["columns"].as_array().unwrap().iter().find(|c| c["column"] == "supplier").unwrap().clone();
    assert_eq!((col["property"].as_str(), col["link"].as_str()), (Some("supplier"), None), "{out}");
    assert_eq!((out["targets_created"].as_u64(), out["linked"].as_u64()), (Some(0), Some(0)), "{out}");
}

// ------------------------------------------------------------ the second gate: business links, no twin suppliers

#[tokio::test]
async fn entity_relate_links_business_things_saves_its_reason_once_and_is_grounded() {
    let app = TestApp::new().await;
    let vault = personal(&app).await;
    let truck = write(&app, "Truck 4", "truck").await["entity"]["id"].as_str().unwrap().to_string();
    let order = write(&app, "GGS-5012", "order").await["entity"]["id"].as_str().unwrap().to_string();
    let token = format!("ghp_{}", "a1B2c3D4e5".repeat(4));
    let carries = json!({"from_id": truck, "label": "carries", "to_id": order, "reason": format!("It is on Monday's run sheet. {token}")});
    let first = ok(app.tool("entity_relate", carries.clone()).await);
    assert_eq!((first["in"].as_str(), first["label"].as_str(), first["out"].as_str()), (Some(truck.as_str()), Some("carries"), Some(order.as_str())), "{first}");
    assert!(first["reason_memory_id"].is_string() && first["redacted"].as_array().is_some_and(|r| !r.is_empty()), "{first}");
    let again = ok(app.tool("entity_relate", carries).await);
    assert_eq!((again["id"].as_str(), again.get("reason_memory_id")), (first["id"].as_str(), None), "the same link twice is one relation and one fact: {again}");
    let got = ok(app.tool("entities_get", json!({"id": truck})).await);
    let facts: Vec<&str> = got["memory"].as_array().unwrap().iter().map(|m| m["text"].as_str().unwrap()).collect();
    assert_eq!(facts.len(), 2, "{facts:?}");
    assert!(facts.iter().any(|f| f.starts_with("Truck 4 carries GGS-5012: It is on Monday's run sheet.")), "{facts:?}");
    assert!(!got.to_string().contains(&token), "the reason's secret is redacted: {got}");
    // impact now has a link to follow
    let n = ok(app.tool("entities_neighbourhood", json!({"entity_id": truck})).await);
    assert_eq!(names(&n), ["GGS-5012", "Truck 4"], "{n}");

    // strict: an unreviewed link type is refused, filed for the owner, and saves no reason fact
    assert_eq!(decide_link(&app, &vault, "truck", "carries", "order").await, 200);
    assert_eq!(set_strict(&app, &vault, true).await.0, 200);
    let out = app.tool("entity_relate", json!({"from_id": truck, "label": "hauls", "to_id": order, "reason": "A new word for it."})).await;
    assert_eq!(out["code"], "ontology.rejected", "{out}");
    assert!(err(&out).contains("truck carries order"), "names the approved link: {out}");
    let o = ok(app.tool("ontology_get", json!({})).await);
    assert_eq!(link(&o, "truck", "hauls", "order")["status"], "proposed", "{o}");
    assert_eq!(ok(app.tool("entities_get", json!({"id": truck})).await)["memory"].as_array().unwrap().len(), 2, "no fact for a refused link");
    ok(app.tool("entity_relate", json!({"from_id": truck, "label": "carries", "to_id": order})).await);
}

#[tokio::test]
async fn an_impact_view_from_an_entity_with_no_links_says_there_is_nothing_to_follow() {
    let app = TestApp::new().await;
    let truck = write(&app, "Truck 4", "truck").await["entity"]["id"].as_str().unwrap().to_string();
    let bare = ok(app.tool("show_view", json!({"view": "impact", "start_id": truck, "headline": "Truck 4 is a day late."})).await);
    assert_eq!(bare["linked"], false, "{bare}");
    let text = bare["text"].as_str().unwrap();
    assert!(text.contains("Truck 4 has no saved links to follow") && !text.contains("Nothing else is affected"), "{text}");

    let order = write(&app, "GGS-5012", "order").await["entity"]["id"].as_str().unwrap().to_string();
    ok(app.tool("entity_relate", json!({"from_id": truck, "label": "carries", "to_id": order})).await);
    let linked = ok(app.tool("show_view", json!({"view": "impact", "start_id": truck, "headline": "Truck 4 is a day late."})).await);
    assert_eq!(linked["linked"], true, "{linked}");
    assert!(linked["text"].as_str().unwrap().ends_with("Nothing else is affected."), "{linked}");
}

const LINK_GARDEN: &str = "product,supplier,price AUD\n\
Drip Line 13mm,Coastal Irrigation Co,$24.00\n\
Sprinkler Head,coastal irrigation co,$8.50\n\
Pine Bark Mulch,Barwon Soil & Mulch,$75.00\n\
Garden Twine,Twine Works,$4.00\n";

fn garden_answer(doc: &str) -> Value {
    json!({"record_types": [{
        "source": "document", "record_type": doc, "kind": "product", "name_field": "product",
        "properties": [{"name": "price", "datatype": "money", "unit": "AUD", "fields": ["price AUD"]}],
        "links": [{"field": "supplier", "label": "supplied_by", "to_kind": "supplier", "target": null}],
        "metrics": [],
    }]})
}

#[tokio::test]
async fn automap_and_import_link_a_supplier_column_to_the_organisation_already_there_and_skip_a_name_two_entities_share() {
    let mock = serve(vec![route("POST", "/chat/completions", json!({}))]).await;
    let app = TestApp::with_settings(model_app_settings(&mock)).await;
    let vault = personal(&app).await;
    // what document indexing made before the CSV came
    let coastal = ok(app.tool("memory_write", json!({"subject_name": "Coastal Irrigation Co", "subject_kind": "organisation", "text": "Coastal Irrigation Co plans a price rise"})).await)["entity"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    write(&app, "Barwon Soil & Mulch", "organisation").await;
    write(&app, "Barwon Soil & Mulch", "customer").await;
    let doc = upload_csv(&app, "products.csv", LINK_GARDEN).await;
    mock.set_routes(vec![route("POST", "/chat/completions", json!({"choices": [{"message": {"content": garden_answer(&doc).to_string()}}]}))]);

    // auto-map looks past the supplier kind: one value names the organisation, one names two entities
    let out = ok(app.tool("ontology_automap", json!({"document_id": doc})).await);
    let check = out["links"].as_array().unwrap().iter().find(|c| c["label"] == "supplied_by").unwrap().clone();
    assert_eq!(check["reused"], json!([{"column": "supplier", "name": "Coastal Irrigation Co", "kind": "organisation", "entity_id": coastal}]), "{check}");
    assert_eq!(check["ambiguous"], json!([{"column": "supplier", "name": "Barwon Soil & Mulch", "kinds": ["organisation", "customer"]}]), "{check}");
    assert_eq!((check["matched"].as_u64(), check["kept"].as_bool()), (Some(1), Some(true)), "{check}");
    let reason = check["reason"].as_str().unwrap();
    assert!(reason.starts_with("no supplier exists yet: an import creates the 1 named in supplier once each"), "{reason}");
    assert!(reason.contains("organisation Coastal Irrigation Co") && reason.contains("Barwon Soil & Mulch: organisation and customer"), "{reason}");

    for k in ["product", "supplier"] {
        assert_eq!(decide_kind(&app, &vault, k, json!({"action": "approve"})).await.0, 200);
    }
    assert_eq!(decide_property(&app, &vault, "product", "price", json!({"action": "approve"})).await.0, 200);
    assert_eq!(decide_link(&app, &vault, "product", "supplied_by", "supplier").await, 200);

    let (_, preview) = app.http("GET", &format!("/api/documents/{doc}/profile"), None, true).await;
    assert_eq!(preview["import"]["reused"], check["reused"]);
    assert_eq!(preview["import"]["ambiguous"], check["ambiguous"]);

    // strict: linking a product to an organisation is a link type no owner approved yet, so those cells wait
    assert_eq!(set_strict(&app, &vault, true).await.0, 200);
    let args = json!({"document_id": doc, "kind": "product", "name_column": "product"});
    let first = ok(app.tool("document_import_rows", args.clone()).await);
    assert_eq!((first["created"].as_u64(), first["targets_created"].as_u64(), first["linked"].as_u64(), first["bad_count"].as_u64()), (Some(4), Some(1), Some(1), Some(2)), "{first}");
    assert!(first["bad"][0]["error"].as_str().unwrap().contains("is an existing organisation"), "{first}");
    assert_eq!(first["reused"], json!([]), "{first}");
    assert_eq!(first["ambiguous"], json!([{"column": "supplier", "name": "Barwon Soil & Mulch", "kinds": ["organisation", "customer"]}]), "{first}");

    // the owner approves it: both spellings link to the one organisation, still an organisation
    assert_eq!(decide_link(&app, &vault, "product", "supplied_by", "organisation").await, 200);
    let second = ok(app.tool("document_import_rows", args.clone()).await);
    assert_eq!((second["created"].as_u64(), second["targets_created"].as_u64(), second["linked"].as_u64(), second["bad_count"].as_u64()), (Some(0), Some(0), Some(2), Some(0)), "{second}");
    assert_eq!(second["reused"], json!([{"column": "supplier", "name": "Coastal Irrigation Co", "kind": "organisation", "entity_id": coastal}]), "{second}");
    let third = ok(app.tool("document_import_rows", args).await);
    assert_eq!((third["targets_created"].as_u64(), third["linked"].as_u64()), (Some(0), Some(0)), "a re-run adds nothing: {third}");

    let o = ok(app.tool("ontology_get", json!({})).await);
    assert_eq!(kind(&o, "supplier")["entities"], 1, "only Twine Works is a new supplier: {o}");
    let found = ok(app.tool("entities_search", json!({"query": "Coastal Irrigation Co"})).await);
    let hits: Vec<(&str, &str)> = found["results"].as_array().unwrap().iter().map(|r| (r["name"].as_str().unwrap(), r["kind"].as_str().unwrap())).collect();
    assert_eq!(hits, [("Coastal Irrigation Co", "organisation")], "one entity, its kind kept: {found}");
    let n = ok(app.tool("entities_neighbourhood", json!({"entity_id": coastal})).await);
    assert_eq!(names(&n), ["Coastal Irrigation Co", "Drip Line 13mm", "Sprinkler Head"], "{n}");
}

const GARDEN: &str = include_str!("fixtures/garden.csv");

/// garden.csv as products linked to their suppliers, with a percent gross margin and stock value at cost.
async fn garden(app: &TestApp) -> std::collections::HashMap<String, String> {
    let doc = upload_csv(app, "garden.csv", GARDEN).await;
    let vault = personal(app).await;
    let source = |col: &str| json!([{"source": "document", "record_type": doc, "path": col}]);
    ok(app.tool("ontology_propose", json!({
        "kinds": [{"kind": "product"}, {"kind": "supplier"}],
        "properties": [
            {"kind": "product", "name": "price", "datatype": "money", "unit": "AUD", "source_map": source("price AUD")},
            {"kind": "product", "name": "cost", "datatype": "money", "unit": "AUD", "source_map": source("cost AUD")},
            {"kind": "product", "name": "stock", "datatype": "integer", "source_map": source("stock")},
        ],
        "links": [{"from_kind": "product", "label": "supplied_by", "to_kind": "supplier", "source_map": source("supplier")}],
        "metrics": [
            {"kind": "product", "name": "gross_margin", "unit": "percent", "expr": {"agg": "ratio",
                "numerator": {"agg": "sum", "row": {"op": "mul", "left": {"op": "sub", "left": "price", "right": "cost"}, "right": "stock"}},
                "denominator": {"agg": "sum", "row": {"op": "mul", "left": "price", "right": "stock"}}}},
            {"kind": "product", "name": "stock_value", "expr": {"agg": "sum", "row": {"op": "mul", "left": "stock", "right": "cost"}}},
        ],
    })).await);
    for k in ["product", "supplier"] {
        assert_eq!(decide_kind(app, &vault, k, json!({"action": "approve"})).await.0, 200);
    }
    for p in ["price", "cost", "stock"] {
        assert_eq!(decide_property(app, &vault, "product", p, json!({"action": "approve"})).await.0, 200);
    }
    for m in ["gross_margin", "stock_value"] {
        assert_eq!(decide_metric(app, &vault, "product", m, json!({"action": "approve"})).await.0, 200);
    }
    assert_eq!(decide_link(app, &vault, "product", "supplied_by", "supplier").await, 200);
    let out = ok(app.tool("document_import_rows", json!({"document_id": doc, "kind": "product", "name_column": "product"})).await);
    assert_eq!((out["created"].as_u64(), out["targets_created"].as_u64(), out["linked"].as_u64()), (Some(8), Some(3), Some(8)), "{out}");
    let mut ids = std::collections::HashMap::new();
    for name in ["Barwon Soil & Mulch", "Coastal Irrigation Co"] {
        let found = ok(app.tool("entities_search", json!({"query": name, "kind": "supplier"})).await);
        let hit = found["results"].as_array().unwrap().iter().find(|r| r["name"] == name).unwrap_or_else(|| panic!("{found}"));
        ids.insert(name.to_string(), hit["id"].as_str().unwrap().to_string());
    }
    ids
}

#[tokio::test]
async fn a_ratio_uses_the_same_entities_on_both_sides_and_a_metric_narrows_to_one_suppliers_products() {
    let app = TestApp::new().await;
    let suppliers = garden(&app).await;
    let (barwon, coastal) = (suppliers["Barwon Soil & Mulch"].clone(), suppliers["Coastal Irrigation Co"].clone());
    let eval = |args: Value| app.tool("metric_eval", args);

    // worked out from garden.csv: Turf Underlay has no cost, so it is left out of both sides of the margin
    let gm = ok(eval(json!({"metric": "gross_margin"})).await);
    assert_eq!((gm["value"].as_f64(), gm["unit"].as_str(), gm["n_entities"].as_u64()), (Some(38.062307), Some("%"), Some(7)), "a fraction shown as a percent is times 100: {gm}");
    assert!(gm["warnings"].to_string().contains("left out 1 entity missing a value one side needs"), "{gm}");
    // a sum is unchanged: the blank cost is simply not added
    let sv = ok(eval(json!({"metric": "stock_value"})).await);
    assert_eq!((sv["value"].as_f64(), sv["unit"].as_str(), sv["n_entities"].as_u64()), (Some(3246.65), Some("AUD"), Some(7)), "{sv}");
    assert!(!sv["warnings"].to_string().contains("one side needs"), "{sv}");

    // narrowed by the link: Barwon's 3 products, the lower-case spelling included, 2 of them costed
    let on_barwon = ok(eval(json!({"metric": "gross_margin", "related_to": {"entity_id": barwon, "label": "supplied_by"}})).await);
    assert_eq!((on_barwon["value"].as_f64(), on_barwon["n_entities"].as_u64()), (Some(46.615087), Some(2)), "{on_barwon}");
    assert!(on_barwon["expr_words"].as_str().unwrap().ends_with(", on 3 products supplied by Barwon Soil & Mulch"), "{on_barwon}");
    assert!(on_barwon["warnings"].to_string().contains("left out 1 entity missing a value one side needs"), "{on_barwon}");
    let any_label = ok(eval(json!({"metric": "gross_margin", "related_to": {"entity_id": barwon}})).await);
    assert_eq!(any_label["value"], on_barwon["value"], "{any_label}");
    // an entity nothing of the kind links to gives no value, never a 0; an unknown id is refused
    let leaf = write(&app, "Leaf Co", "organisation").await["entity"]["id"].as_str().unwrap().to_string();
    let none = ok(eval(json!({"metric": "gross_margin", "related_to": {"entity_id": leaf, "label": "supplied_by"}})).await);
    assert_eq!((none["value"].clone(), none["n_entities"].as_u64()), (Value::Null, Some(0)), "{none}");
    assert!(none["warnings"].to_string().contains("no products are linked to Leaf Co by supplied by links, so nothing was computed"), "{none}");
    assert!(err(&eval(json!({"metric": "gross_margin", "related_to": {"entity_id": "entity:nowhere"}})).await).contains("unknown entity"));

    // the metric view reruns it with the same pick and writes the percent as one
    let view = ok(app.tool("show_view", json!({"view": "metric", "metric": "gross_margin", "related_to": {"entity_id": barwon, "label": "supplied_by"}})).await);
    let text = view["text"].as_str().unwrap();
    assert!(text.starts_with("gross_margin: 46.6%\n") && text.contains("on 3 products supplied by Barwon Soil & Mulch."), "{text}");

    // the what-if inherits both: Coastal's costs up 10% moves the margin from 38.06% to 33.90%
    let args = json!({"changes": [{"scale": {"kind": "product", "property": "cost", "factor": 1.1, "related_to": {"entity_id": coastal, "label": "supplied_by"}}}], "metrics": ["gross_margin"]});
    let out = ok(app.tool("scenario_run", args.clone()).await);
    let (b, s) = (&out["baseline"][0], &out["scenario"][0]);
    assert_eq!((b["value"].as_f64(), s["value"].as_f64(), s["delta"].as_f64(), s["unit"].as_str()), (Some(38.062307), Some(33.895036), Some(-4.167271), Some("%")), "{out}");
    let view = ok(app.tool("show_view", json!({"view": "scenario", "headline": "Coastal raises costs", "scenario": args})).await);
    assert!(view["text"].as_str().unwrap().contains("- gross margin: 38.1% \u{2192} 33.9%, down 4.2 points (exact: -4.167271 points)\n"), "{view}");
}

#[tokio::test]
async fn gate2_rest_cannot_seed_a_new_personal_kind_past_a_strict_space() {
    let app = TestApp::new().await;
    strict_garden_supply(&app).await;
    let token = eunomia_backend::models_user::create_api_token_with(
        &app.state.control, &app.user.id, "writer", &["memory:write".into()], None, None,
    ).await.unwrap().token;
    let body = json!({"kind": "livestock", "name": "Chickens"});
    let ((status, out), _) = common::http(&app.router, "POST", "/api/entities", Some(body.clone()), Some(&token), None).await;
    assert_eq!(status, 400, "{out}");
    let message = out["detail"].as_str().unwrap();
    assert!(message.contains("vault_id") && message.contains("Geelong Garden Supply") && message.contains("Personal"), "{out}");
    let (status, out) = app.http_session("POST", "/api/entities", Some(body.clone())).await;
    assert_eq!(status, 400, "the browser gets the same error: {out}");
    assert_eq!(out["detail"], message);
    let refused = app.tool("memory_write", chickens(json!({}))).await;
    assert!(refused.get("error").is_some(), "REST did not seed the kind: {refused}");
    let db = app.db().await;
    assert!(common::sys(eunomia_backend::entities::service::upsert_entity_created(
        &db, &app.user.id, "livestock", "Chickens", None, None,
    )).await.is_err());
    let mine = personal(&app).await;
    let mut explicit = body;
    explicit["vault_id"] = json!(mine);
    let ((status, out), _) = common::http(&app.router, "POST", "/api/entities", Some(explicit.clone()), Some(&token), None).await;
    assert_eq!(status, 200, "{out}");
    assert_eq!(out["vault"], mine);
    explicit["name"] = json!("Ducks");
    let (status, out) = app.http_session("POST", "/api/entities", Some(explicit)).await;
    assert_eq!(status, 200, "selected-space Memory page creation: {out}");
    assert_eq!(out["vault"], mine);
}

#[tokio::test]
async fn gate2_short_names_are_not_hidden_by_personal_substring_matches() {
    let app = TestApp::new().await;
    let team = ok(app.tool("vault_create", json!({"name": "Team", "kind": "org"})).await)["id"].as_str().unwrap().to_string();
    let mine = rid::parse(&personal(&app).await).unwrap();
    let db = app.db().await;
    common::sys(async {
        for i in 0..60 {
            eunomia_backend::entities::service::upsert_entity(&db, &app.user.id, "file", &format!("Space heater {i}"), None, Some(&mine)).await.unwrap();
        }
    }).await;
    ok(app.tool("memory_write", json!({"subject_name": "ACE", "subject_kind": "organisation", "text": "Our supplier", "vault_id": team})).await);
    bobs_secret_space(&app, "ACE").await;
    let out = ok(app.tool("ontology_coverage", json!({"question": "Where is ACE?"})).await);
    assert_eq!(out["chosen_by"], "entities", "{out}");
    assert_eq!(out["vault"], team, "{out}");
    ok(app.tool("memory_write", json!({"subject_name": "BP", "subject_kind": "organisation", "text": "Another supplier", "vault_id": team})).await);
    let out = ok(app.tool("ontology_coverage", json!({"question": "Where is BP?"})).await);
    assert_eq!(out["chosen_by"], "entities", "{out}");
    assert_eq!(out["vault"], team, "{out}");
}

#[tokio::test]
async fn gate2_named_space_lookup_uses_name_key_indexes() {
    fn index_scan(node: &Value) -> Option<&Value> {
        if node["operator"] == "IndexScan" {
            return Some(node);
        }
        node["children"].as_array()?.iter().find_map(index_scan)
    }
    let app = TestApp::new().await;
    let vault = rid::parse(&personal(&app).await).unwrap();
    let db = app.db().await;
    for table in eunomia_backend::entities::service::ENTITY_TABLES {
        for prefix in [false, true] {
            let plan: Vec<Value> = eunomia_backend::store::entities::explain_name_lookup(&db, table, prefix)
                .bind(("vault", vault.clone())).bind(("key", "coastal irrigation"))
                .bind(("lower", "coastal irrigation ")).bind(("upper", "coastal irrigation!"))
                .await.unwrap().take(0).unwrap();
            let shown = serde_json::to_string(&plan).unwrap();
            assert!(shown.contains("IndexScan") && shown.contains("name_key"), "{table}, prefix={prefix}: {shown}");
            assert!(!shown.contains("TableScan"), "{shown}");
            let scan = index_scan(&plan[0]).expect("index scan");
            assert!(scan["attributes"]["index"].as_str().unwrap().contains("name_key"), "{shown}");
            assert_eq!(scan["attributes"]["limit"], "8", "the index walk itself must stop: {shown}");
            assert!(!shown.contains("Sort"), "names must come from the index in order: {shown}");
            if prefix {
                assert!(scan["attributes"]["access"].as_str().unwrap().contains("MoreThanEqual 'coastal irrigation '"), "{shown}");
            }
            println!("{table}, prefix={prefix}: {shown}");
        }
    }
}

#[tokio::test]
async fn gate2_a_long_question_caps_name_lookups_and_never_searches_memory_text() {
    let app = TestApp::new().await;
    ok(app.tool("vault_create", json!({"name": "Team", "kind": "org"})).await);
    let question = (0..100).map(|i| format!("word{i}")).collect::<Vec<_>>().join(" ");
    use tracing_subscriber::layer::SubscriberExt;
    let ops = Ops::default();
    {
        let _held = tracing::subscriber::set_default(tracing_subscriber::registry().with(ops.clone()));
        ok(app.tool("ontology_coverage", json!({"question": question})).await);
    }
    let queries = ops.0.lock().unwrap();
    assert_eq!(queries.iter().filter(|q| *q == "entities.names_by_prefix").count(), 16, "{queries:?}");
    assert!(!queries.iter().any(|q| q == "entities.search"), "no general or memory-text search: {queries:?}");
}

#[tokio::test]
async fn gate2_name_lookup_honours_vault_restricted_tokens() {
    let app = TestApp::new().await;
    let team = ok(app.tool("vault_create", json!({"name": "Team", "kind": "org"})).await)["id"].as_str().unwrap().to_string();
    let other = ok(app.tool("vault_create", json!({"name": "Other", "kind": "org"})).await)["id"].as_str().unwrap().to_string();
    ok(app.tool("memory_write", json!({"subject_name": "ACE", "subject_kind": "organisation", "text": "Supplier", "vault_id": other})).await);
    let only = rid::parse(&team).unwrap();
    let token = eunomia_backend::models_user::create_api_token_with(
        &app.state.control, &app.user.id, "reader", &["memory:read".into()], Some(&only), None,
    ).await.unwrap().token;
    let ((status, out), _) = common::http(&app.router, "POST", "/api/tools/ontology_coverage", Some(json!({"question": "Where is ACE?"})), Some(&token), None).await;
    assert_eq!(status, 200, "{out}");
    assert_eq!(out["vault"], team, "{out}");
    assert!(!out.to_string().contains(&other) && !out.to_string().contains("Personal"), "{out}");
}

#[tokio::test]
async fn relation_boundary_refuses_overlong_labels_in_strict_mode() {
    let app = TestApp::new().await;
    let a = write(&app, "Alice", "person").await["entity"]["id"].clone();
    let b = write(&app, "Bob", "person").await["entity"]["id"].clone();
    assert_eq!(set_strict(&app, &personal(&app).await, true).await.0, 200);
    for tool in ["entity_relate", "code_relate"] {
        let out = app.tool(tool, json!({"from_id": a, "to_id": b, "label": "x".repeat(65)})).await;
        assert!(out.get("error").is_some(), "overlong label was accepted");
    }
    assert_eq!(ok(app.tool("entities_get", json!({"id": a})).await)["relations"], json!([]));
}

#[tokio::test]
async fn a_padded_label_is_stored_as_the_approved_label_it_was_grounded_as() {
    let app = TestApp::new().await;
    let vault = personal(&app).await;
    let a = write(&app, "Alice", "person").await["entity"]["id"].clone();
    let b = write(&app, "Bob", "person").await["entity"]["id"].clone();
    ok(app.tool("ontology_propose", json!({"links": [{"from_kind": "person", "label": "knows", "to_kind": "person"}]})).await);
    assert_eq!(decide_link(&app, &vault, "person", "knows", "person").await, 200);
    assert_eq!(set_strict(&app, &vault, true).await.0, 200);
    let padded = format!("  knows{}", " ".repeat(65));
    ok(app.tool("code_relate", json!({"from_id": a, "to_id": b, "label": padded})).await);
    ok(app.tool("entity_relate", json!({"from_id": a, "to_id": b, "label": padded, "reason": "old friends"})).await);
    let rels = ok(app.tool("entities_get", json!({"id": a})).await)["relations"].clone();
    let labels: Vec<&str> = rels.as_array().unwrap().iter().filter_map(|r| r["label"].as_str()).collect();
    assert_eq!(labels, ["knows"], "one relation, stored under the approved label: {rels}");

    // an edge stored padded before labels were trimmed is found again, not duplicated
    let c = write(&app, "Cleo", "person").await["entity"]["id"].as_str().unwrap().to_string();
    let a = a.as_str().unwrap();
    app.db().await.test_raw().query(format!("RELATE {a}->relates_to->{c} SET label = ' knows ', owner = $u"))
        .bind(("u", app.user.id.clone())).await.unwrap().check().unwrap();
    ok(app.tool("code_relate", json!({"from_id": a, "to_id": c, "label": "knows"})).await);
    ok(app.tool("entity_relate", json!({"from_id": a, "to_id": c, "label": "knows", "reason": "met at work"})).await);
    let rels = ok(app.tool("entities_get", json!({"id": a})).await)["relations"].clone();
    let to_cleo = rels.as_array().unwrap().iter().filter(|r| r.to_string().contains(&c)).count();
    assert_eq!(to_cleo, 1, "the legacy padded edge is reused: {rels}");
}

#[tokio::test]
async fn relation_boundary_refuses_secret_labels_before_storage_or_proposals() {
    let app = TestApp::new().await;
    let a = write(&app, "Alice", "person").await["entity"]["id"].clone();
    let b = write(&app, "Bob", "person").await["entity"]["id"].clone();
    let fake_token = format!("ghp_{}", "FAKE0".repeat(8));
    for strict in [false, true] {
        assert_eq!(set_strict(&app, &personal(&app).await, strict).await.0, 200);
        for tool in ["entity_relate", "code_relate"] {
            let out = app.tool(tool, json!({"from_id": a, "to_id": b, "label": fake_token})).await;
            assert!(out.get("error").is_some(), "secret label was accepted");
            assert!(!out.to_string().contains(&fake_token), "secret echoed in refusal");
        }
    }
    assert_eq!(ok(app.tool("entities_get", json!({"id": a})).await)["relations"], json!([]));
    assert!(!ok(app.tool("ontology_get", json!({})).await).to_string().contains(&fake_token));
}

#[tokio::test]
async fn relation_reason_concurrent_retries_and_a_missing_reason_converge() {
    let app = TestApp::new().await;
    let a = write(&app, "Alice", "person").await["entity"]["id"].clone();
    let b = write(&app, "Bob", "person").await["entity"]["id"].clone();
    let args = json!({"from_id": a, "to_id": b, "label": "knows", "reason": "They met at work."});
    let (one, two) = tokio::join!(app.tool("entity_relate", args.clone()), app.tool("entity_relate", args.clone()));
    assert_eq!(ok(one)["id"], ok(two)["id"]);
    let got = ok(app.tool("entities_get", json!({"id": a})).await);
    assert_eq!(got["relations"].as_array().unwrap().len(), 1);
    assert_eq!(got["memory"].as_array().unwrap().len(), 2);
    // Simulate an old partial write, or deletion of the reason, then retry.
    app.db().await.test_raw().query("DELETE memory WHERE text = 'Alice knows Bob: They met at work.'").await.unwrap().check().unwrap();
    ok(app.tool("entity_relate", args).await);
    assert_eq!(ok(app.tool("entities_get", json!({"id": a})).await)["memory"].as_array().unwrap().len(), 2, "retry restores exactly one reason");
}

#[tokio::test]
async fn relation_reason_failure_rolls_back_the_relation_and_retry_succeeds() {
    let app = TestApp::new().await;
    let a = write(&app, "Alice", "person").await["entity"]["id"].clone();
    let b = write(&app, "Bob", "person").await["entity"]["id"].clone();
    let db = app.db().await;
    db.test_raw().query("DEFINE EVENT refuse_reason ON memory WHEN $event = 'CREATE' THEN { THROW 'forced reason failure'; }").await.unwrap().check().unwrap();
    let args = json!({"from_id": a, "to_id": b, "label": "knows", "reason": "They met at work."});
    assert!(app.tool("entity_relate", args.clone()).await.get("error").is_some());
    assert_eq!(ok(app.tool("entities_get", json!({"id": a})).await)["relations"], json!([]), "a failed reason must roll back its relation");
    db.test_raw().query("REMOVE EVENT refuse_reason ON memory").await.unwrap().check().unwrap();
    ok(app.tool("entity_relate", args).await);
    let got = ok(app.tool("entities_get", json!({"id": a})).await);
    assert_eq!(got["relations"].as_array().unwrap().len(), 1);
    assert_eq!(got["memory"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn rejected_named_targets_are_skipped_or_resolved_in_automap_preview_and_import() {
    let mock = serve(vec![route("POST", "/chat/completions", json!({}))]).await;
    let app = TestApp::with_settings(model_app_settings(&mock)).await;
    let vault = personal(&app).await;
    write(&app, "Acme", "vendor").await;
    assert_eq!(decide_kind(&app, &vault, "vendor", json!({"action": "reject"})).await.0, 200);
    let doc = upload_csv(&app, "products.csv", "product,supplier,price AUD\nWidget,Acme,$2.00\n").await;
    mock.set_routes(vec![route("POST", "/chat/completions", json!({"choices": [{"message": {"content": garden_answer(&doc).to_string()}}]}))]);
    let auto = ok(app.tool("ontology_automap", json!({"document_id": doc})).await);
    assert_eq!(auto["links"][0]["reused"], json!([]));
    assert_eq!(auto["links"][0]["skipped"][0]["name"], "Acme");
    for k in ["product", "supplier"] {
        assert_eq!(decide_kind(&app, &vault, k, json!({"action": "approve"})).await.0, 200);
    }
    assert_eq!(decide_property(&app, &vault, "product", "price", json!({"action": "approve"})).await.0, 200);
    assert_eq!(decide_link(&app, &vault, "product", "supplied_by", "supplier").await, 200);
    let (_, profile) = app.http("GET", &format!("/api/documents/{doc}/profile"), None, true).await;
    assert_eq!(profile["import"]["skipped"][0]["name"], "Acme");
    let args = json!({"document_id": doc, "kind": "product", "name_column": "product"});
    let out = ok(app.tool("document_import_rows", args.clone()).await);
    assert_eq!(out["linked"], 0);
    assert_eq!(out["targets_created"], 0);
    assert_eq!(out["skipped"][0]["name"], "Acme");
    assert_eq!(decide_kind(&app, &vault, "vendor", json!({"action": "reject", "merge_into": "organisation"})).await.0, 200);
    let absent = ok(app.tool("document_import_rows", args.clone()).await);
    assert_eq!(absent["linked"], 0);
    assert_eq!(absent["skipped"][0]["name"], "Acme");
    let replacement = write(&app, "Acme", "organisation").await["entity"]["id"].clone();
    let out = ok(app.tool("document_import_rows", args).await);
    assert_eq!(out["linked"], 1);
    assert_eq!(out["reused"][0]["entity_id"], replacement);
    let auto = ok(app.tool("ontology_automap", json!({"document_id": doc})).await);
    assert_eq!(auto["links"][0]["reused"][0]["entity_id"], replacement);
}

#[tokio::test]
async fn preview_column_precedence_matches_import_for_competing_properties_and_links() {
    let app = TestApp::new().await;
    let doc = upload_csv(&app, "products.csv", "product,supplier,price\nWidget,Acme,2\n").await;
    let vault = personal(&app).await;
    let source = |col: &str| json!([{"source": "document", "record_type": doc, "path": col}]);
    ok(app.tool("ontology_propose", json!({
        "kinds": [{"kind": "product"}, {"kind": "supplier"}],
        "properties": [
            {"kind": "product", "name": "price", "datatype": "number", "source_map": source("price")},
            {"kind": "product", "name": "supplier_text", "datatype": "text", "source_map": source("supplier")}
        ],
        "links": [{"from_kind": "product", "label": "supplied_by", "to_kind": "supplier", "source_map": source("supplier")}]
    })).await);
    for k in ["product", "supplier"] {
        assert_eq!(decide_kind(&app, &vault, k, json!({"action": "approve"})).await.0, 200);
    }
    assert_eq!(decide_property(&app, &vault, "product", "price", json!({"action": "approve"})).await.0, 200);
    assert_eq!(decide_link(&app, &vault, "product", "supplied_by", "supplier").await, 200);
    let args = json!({"document_id": doc, "kind": "product", "name_column": "product"});
    let (_, profile) = app.http("GET", &format!("/api/documents/{doc}/profile"), None, true).await;
    let planned = &profile["import"]["columns"][1];
    assert!(planned["property"].is_null());
    assert_eq!(planned["link"], "supplied_by supplier");
    assert_eq!(ok(app.tool("document_import_rows", args.clone()).await)["columns"][0]["link"], planned["link"]);
    ok(app.tool("ontology_propose", json!({"properties": [
        {"kind": "product", "name": "supplier_other", "datatype": "text", "source_map": source("supplier")}
    ]})).await);
    for p in ["supplier_text", "supplier_other"] {
        assert_eq!(decide_property(&app, &vault, "product", p, json!({"action": "approve"})).await.0, 200);
    }
    let (_, profile) = app.http("GET", &format!("/api/documents/{doc}/profile"), None, true).await;
    let planned = &profile["import"]["columns"][1];
    assert!(planned["property"].is_null() && planned["link"].is_null());
    let out = ok(app.tool("document_import_rows", args).await);
    assert!(out["columns"][0]["property"].is_null() && out["columns"][0]["link"].is_null());
    assert!(out["columns"][0]["skipped"].as_str().unwrap().contains("column_map"));
}

#[tokio::test]
async fn gate3_empty_phrases_choose_named_entities_then_approved_ontology() {
    let base = coverage_model(vec![], true).await;
    let app = TestApp::with_settings(Settings { openai_base_url: base, openai_api_key: Some("sk-test".into()), openai_chat_model: "test-model".into(), ..test_settings() }).await;
    let team = ok(app.tool("vault_create", json!({"name": "Workshop", "kind": "org"})).await)["id"].clone();
    for name in ["Label Printer 2", "Ute 3"] {
        ok(app.tool("memory_write", json!({"subject_name": name, "subject_kind": "equipment", "text": "Kept in the workshop", "vault_id": team})).await);
    }
    for question in ["What is happening with Label Printer 2?", "Tell me about Ute 3"] {
        let out = ok(app.tool("ontology_coverage", json!({"question": question})).await);
        assert_eq!((out["mode"].as_str(), out["coverage"].as_f64()), (Some("model"), Some(0.0)), "{out}");
        assert_eq!(out["chosen_by"], "entities", "{out}");
        assert_eq!(out["vault"], team, "{out}");
    }
    ok(app.tool("ontology_propose", json!({"vault_id": team, "kinds": [{"kind": "equipment"}]})).await);
    assert_eq!(decide_kind(&app, team.as_str().unwrap(), "equipment", json!({"action": "approve"})).await.0, 200);
    let out = ok(app.tool("ontology_coverage", json!({"question": "What is happening?"})).await);
    assert_eq!(out["chosen_by"], "ontology", "{out}");
    assert_eq!(out["vault"], team, "{out}");
}

#[tokio::test]
async fn gate3_empty_phrases_without_entities_or_ontology_list_readable_spaces() {
    let base = coverage_model(vec![], true).await;
    let app = TestApp::with_settings(Settings { openai_base_url: base, openai_api_key: Some("sk-test".into()), openai_chat_model: "test-model".into(), ..test_settings() }).await;
    let team = ok(app.tool("vault_create", json!({"name": "Workshop", "kind": "org"})).await)["id"].clone();
    let out = ok(app.tool("ontology_coverage", json!({"question": "What is happening?"})).await);
    assert_eq!(out["chosen_by"], "none", "{out}");
    let spaces: Vec<&str> = out["spaces"].as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(spaces, ["Personal", "Workshop"], "{out}");
    let suggestion = out["suggestion"].as_str().unwrap();
    assert!(suggestion.contains("Personal") && suggestion.contains("Workshop"), "{out}");
    let explicit = ok(app.tool("ontology_coverage", json!({"question": "What is happening?", "vault_id": team})).await);
    assert_eq!(explicit["chosen_by"], "vault_id", "{explicit}");
    let base = coverage_model(vec![], true).await;
    let solo = TestApp::with_settings(Settings { openai_base_url: base, openai_api_key: Some("sk-test".into()), openai_chat_model: "test-model".into(), ..test_settings() }).await;
    let out = ok(solo.tool("ontology_coverage", json!({"question": "What is happening?"})).await);
    assert_eq!(out["chosen_by"], "none", "{out}");
    assert_eq!(out["spaces"][0]["name"], "Personal", "{out}");
}

#[tokio::test]
async fn gate3_scenario_warnings_use_readable_metric_words() {
    let app = TestApp::new().await;
    garden(&app).await;
    let out = ok(app.tool("show_view", json!({"view": "scenario", "headline": "Costs rise", "scenario": {
        "changes": [{"scale": {"kind": "product", "property": "cost", "factor": 1.1}}], "metrics": ["gross_margin"]
    }})).await);
    let warnings = out["result"]["warnings"].as_array().unwrap();
    assert!(warnings.iter().any(|w| w.as_str().unwrap().starts_with("gross margin:")), "{out}");
    assert!(warnings.iter().all(|w| !w.as_str().unwrap().contains("gross_margin")), "{out}");
    assert!(!out["text"].as_str().unwrap().contains("gross_margin"), "{out}");
}

#[tokio::test]
async fn gate3_mcp_scenario_text_and_structured_content_keep_raw_small_moves() {
    let app = TestApp::new().await;
    let vault = personal(&app).await;
    ok(app.tool("ontology_propose", json!({
        "kinds": [{"kind": "sample"}],
        "properties": [{"kind": "sample", "name": "score", "datatype": "percent"}],
        "metrics": [{"kind": "sample", "name": "average_score", "unit": "percent", "expr": {"agg": "avg", "property": "score"}}]
    })).await);
    assert_eq!(decide_kind(&app, &vault, "sample", json!({"action": "approve"})).await.0, 200);
    assert_eq!(decide_property(&app, &vault, "sample", "score", json!({"action": "approve"})).await.0, 200);
    assert_eq!(decide_metric(&app, &vault, "sample", "average_score", json!({"action": "approve"})).await.0, 200);
    let id = write(&app, "Sample", "sample").await["entity"]["id"].clone();
    ok(app.tool("entity_set_property", json!({"entity_id": id, "property": "score", "value": 35.7})).await);
    let args = json!({"view": "scenario", "headline": "Score changes", "scenario": {
        "changes": [{"set": {"entity_id": id, "property": "score", "value": 35.351}}], "metrics": ["average_score"]
    }});
    let rpc = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "show_view", "arguments": args.clone()}});
    let (status, out) = app.http("POST", "/mcp", Some(rpc), true).await;
    assert_eq!(status, 200, "{out}");
    let shown = &out["result"]["structuredContent"];
    assert_eq!(shown["result"]["baseline"][0]["value"], 35.7, "{out}");
    assert_eq!(shown["result"]["scenario"][0]["value"], 35.351, "{out}");
    assert_eq!(shown["result"]["scenario"][0]["delta"], -0.349, "{out}");
    let expected = "35.70% \u{2192} 35.35%, down 0.35 points (exact: -0.349 points)";
    assert!(shown["text"].as_str().unwrap().contains(expected), "{out}");
    assert!(out["result"]["content"][0]["text"].as_str().unwrap().contains(expected), "{out}");

    let call = json!({"index": 0, "id": "c1", "function": {"name": "show_view", "arguments": args.to_string()}});
    let sse = |chunk: Value| format!("data: {chunk}\n\ndata: [DONE]\n\n");
    let reply = |text: &str| json!({"choices": [{"delta": {"content": text}}]});
    let mock = serve(vec![
        route("POST", "/chat/completions", json!(null)).body("(exact: -0.349 points)").raw(&sse(reply("Exact delta received."))).header("content-type", "text/event-stream"),
        route("POST", "/chat/completions", json!(null)).body("\"role\":\"tool\"").raw(&sse(reply("Exact delta missing."))).header("content-type", "text/event-stream"),
        route("POST", "/chat/completions", json!(null)).raw(&sse(json!({"choices": [{"delta": {"tool_calls": [call]}}]}))).header("content-type", "text/event-stream"),
    ]).await;
    assert_eq!(app.http("PATCH", "/api/settings", Some(json!({"openai_base_url": mock.base})), true).await.0, 200);
    let (_, events) = chat(&app, "What if the score changes?").await;
    assert_eq!(events.iter().find(|e| e["type"] == "done").unwrap()["reply"], "Exact delta received.", "{events:?}");
}
