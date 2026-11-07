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
    assert_eq!(types["custom"], json!([{"kind": "customer", "entities": 0, "status": "approved", "description": "A paying client"}]));

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
    assert_eq!(types["custom"], json!([{"kind": "db", "entities": 1, "status": "observed"}]));
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

/// One chat message as the browser sends it; the SSE events that came back.
async fn chat(app: &TestApp, message: &str) -> (String, Vec<Value>) {
    use axum::body::Body;
    use axum::http::{header, Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let (_, thread) = app.http("POST", "/api/chat/threads", Some(json!({})), true).await;
    let thread_id = thread["id"].as_str().unwrap().to_string();
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
    (thread_id, text.lines().filter_map(|l| l.strip_prefix("data:")).filter_map(|d| serde_json::from_str(d.trim()).ok()).collect())
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
    assert!(large["expr_words"].as_str().unwrap().ends_with("and where size is one of Large"), "{large}");
    let cheap = ok(run("total_price", json!({"filters": [{"property": "price", "op": "lt", "value": 5}]})).await);
    assert_eq!(cheap["value"].as_f64(), Some(3.0));

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
    // no warehouse matches anything: dropped
    assert!(o["links"].as_array().unwrap().iter().all(|l| l["label"] != "stored_in"), "{o}");
    let checks = out["links"].as_array().unwrap();
    let dropped = checks.iter().find(|c| c["label"] == "stored_in").unwrap();
    assert_eq!((dropped["kept"].as_bool(), dropped["matched"].as_i64(), dropped["reason"].as_str()), (Some(false), Some(0), Some("no value matches (0 of 3)")));
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
    let out = ok(app.tool("ontology_automap", json!({"source": "stripe"})).await);
    let o = &out["ontology"];
    assert_eq!(link(o, "charge", "paid_by", "customer")["reason"], "100% of values match (2 of 2)");
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
    assert_eq!((out["created"].as_i64(), out["bad_count"].as_i64()), (Some(3), Some(5)), "{out}");
    let bad: Vec<(i64, Option<&str>)> = out["bad"].as_array().unwrap().iter().map(|b| (b["line"].as_i64().unwrap(), b["column"].as_str())).collect();
    assert_eq!(bad, [(2, Some("price AUD")), (3, Some("product")), (4, Some("lead_time_days")), (4, Some("status")), (5, None)], "{out}");
    assert!(out["bad"][0]["error"].as_str().unwrap().contains("a number"), "{out}");
    // the good cells of a bad row still land; an enum value matches ignoring case
    let items = supply_items(&app).await;
    let chai = items.iter().find(|e| e["name"] == "Chai syrup").unwrap();
    let got = ok(app.tool("entities_get", json!({"id": chai["id"]})).await);
    assert_eq!(got["properties"].as_array().unwrap().iter().find(|p| p["property"] == "status").unwrap()["value"], "Retired");
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
