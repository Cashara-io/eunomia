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
    let out = as_user(&app, &bob, "entities_neighbourhood", json!({"entity_id": ids["Truck 7"]})).await;
    assert_eq!(out["code"], "entity.not_found", "{out}");
    assert!(!out.to_string().contains("Truck 7"), "{out}");
}
