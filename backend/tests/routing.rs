//! Connector routing (issue #93): a connector files its records in a destination vault, or splits them
//! across vaults by rules; vault members read what was routed to their vault and nobody else does; a
//! routing change applies to later syncs only, and "Move existing data" re-files the rest, facts and
//! relations included, without duplicates. Plus `vault_ids` reads across several vaults.
//!
//!   owner    has the connectors; owns the "Build Paperless" work vault and a "Side" vault
//!   member   an active member of Build Paperless
//!   outsider in the same org, in neither vault

mod common;

use std::sync::Once;

use common::{http, test_settings, TestApp};
use eunomia_backend::models_user::{self, User};
use eunomia_backend::rid::{self, RecordIdExt};
use eunomia_backend::sources::heypocket::HeyPocketSource;
use eunomia_backend::sources::mock::{route, serve};
use eunomia_backend::sources::registry;
use eunomia_backend::sources::scheduler::sync_source;
use eunomia_backend::state::OrgState;
use eunomia_backend::tools::registry as tools;
use eunomia_backend::vaults::service as vaults;
use serde_json::{json, Value};
use surrealdb::types::RecordId;

fn allow_base_url() {
    static ONCE: Once = Once::new();
    // SAFETY: set once before any test in this binary reads it, from the test threads' first step.
    ONCE.call_once(|| unsafe { std::env::set_var("EUNOMIA_ALLOW_CONNECTOR_BASE_URL", "1") });
}

struct P {
    user: User,
    token: String,
}

struct W {
    app: TestApp,
    org: OrgState,
    owner: P,
    member: P,
    outsider: P,
    work: String,
    side: String,
    personal: String,
}

impl W {
    async fn call(&self, p: &P, name: &str, args: Value) -> Value {
        match Box::pin(common::sys(tools::call(&self.app.state, &p.user, name, args))).await {
            Ok(v) => v,
            Err(e) => json!({ "error": e.message }),
        }
    }
    async fn ok(&self, p: &P, name: &str, args: Value) -> Value {
        let v = self.call(p, name, args).await;
        assert!(v.get("error").is_none(), "{name} failed: {v}");
        v
    }
    async fn http(&self, p: &P, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let ((s, b), _) = http(&self.app.router, method, path, body, Some(&p.token), None).await;
        (s.as_u16(), b)
    }
    /// Sets `p`'s routing for connector `kind` through the REST route.
    async fn route_to(&self, p: &P, kind: &str, routing: Value) {
        let (status, body) = self.http(p, "PUT", &format!("/api/connectors/{kind}"), Some(json!({"routing": routing}))).await;
        assert_eq!(status, 200, "{body}");
    }
    /// The ids `p` finds with the `search` tool.
    async fn search(&self, p: &P, query: &str, extra: Value) -> Vec<String> {
        let mut args = json!({"query": query, "mode": "keyword"});
        args.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
        let out = self.ok(p, "search", args).await;
        out["results"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect()
    }
    /// The texts `p` recalls (records and memories), each with its vault name.
    async fn recall(&self, p: &P, query: &str, extra: Value) -> Vec<(String, String)> {
        let mut args = json!({"query": query});
        args.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
        let out = self.ok(p, "recall", args).await;
        out["results"].as_array().unwrap().iter().map(|r| (r["text"].as_str().unwrap().to_string(), r["vault_name"].as_str().unwrap().to_string())).collect()
    }
    /// The vault each of `owner`'s records from `source` is filed in ("" = personal), by external id.
    async fn filed(&self, source: &str) -> Vec<(String, String)> {
        let mut res = self
            .org
            .db
            .test_raw()
            .query("SELECT external_id, vault FROM cache_record WHERE owner = $o AND source = $s ORDER BY external_id")
            .bind(("o", self.owner.user.id.clone()))
            .bind(("s", source.to_string()))
            .await
            .unwrap();
        let rows: Vec<Value> = res.take(0).unwrap();
        rows.iter().map(|r| (r["external_id"].as_str().unwrap().to_string(), r["vault"].as_str().unwrap_or("").to_string())).collect()
    }
    async fn count(&self, sql: &str) -> i64 {
        let mut res = self.org.db.test_raw().query(format!("{sql} GROUP ALL")).bind(("work", rid::parse(&self.work).unwrap())).bind(("personal", rid::parse(&self.personal).unwrap())).await.unwrap();
        res.take::<Option<Value>>(0).unwrap().and_then(|v| v["count"].as_i64()).unwrap_or(0)
    }
}

async fn person(app: &TestApp, user: User) -> P {
    let token = models_user::create_api_token(&app.state.control, &user.id, "t").await.unwrap().token;
    P { user, token }
}

async fn world() -> W {
    allow_base_url();
    let app = TestApp::with_settings(test_settings()).await;
    let org = app.state.org(&app.user.org).await.unwrap();
    let owner = person(&app, app.user.clone()).await;
    let member = person(&app, common::register(&app.state, "member@example.com").await).await;
    let outsider = person(&app, common::register(&app.state, "outsider@example.com").await).await;
    let blank = String::new;
    let mut w = W { app, org, owner, member, outsider, work: blank(), side: blank(), personal: blank() };
    w.work = w.ok(&w.owner, "vault_create", json!({"name": "Build Paperless", "kind": "org"})).await["id"].as_str().unwrap().into();
    w.side = w.ok(&w.owner, "vault_create", json!({"name": "Side", "kind": "org"})).await["id"].as_str().unwrap().into();
    w.ok(&w.owner, "vault_invite", json!({"vault_id": w.work, "email": "member@example.com"})).await;
    common::sys(vaults::accept_invitation(&w.org.db, &w.member.user.id, &rid::parse(&w.work).unwrap())).await.unwrap();
    w.personal = common::sys(vaults::personal_vault_id(&w.org.db, &w.owner.user.id)).await.unwrap().to_string();
    w
}

/// Points `owner`'s connector `kind` at a mock provider.
async fn connect(w: &W, kind: &str, base: &str, credentials: Value) {
    eunomia_backend::connectors::service::upsert_connector(
        &w.org.db,
        &w.org.settings.encryption_key,
        &w.owner.user.id,
        kind,
        Some(true),
        Some(json!({"base_url": base, "token_url": format!("{base}/oauth/token")})),
        Some(credentials),
    )
    .await
    .unwrap();
}

fn issue(number: i64, title: &str) -> Value {
    json!({
        "number": number, "title": title, "state": "open", "body": format!("{title}: the importer drops the last CSV row."),
        "user": {"login": "ada"}, "labels": [], "assignees": [], "comments": 0, "repository": {"full_name": "acme/paperless"},
        "html_url": format!("https://github.com/acme/paperless/issues/{number}"), "created_at": "2024-01-01T00:00:00Z", "updated_at": "2024-02-01T00:00:00Z",
    })
}

/// A Pocket recording as its API returns it (list entry and detail folded together).
fn recording(id: &str, title: &str, tags: &[&str], summary: &str) -> Value {
    json!({
        "id": id, "title": title, "recording_at": "2024-05-02T09:00:00Z", "duration": 600,
        "tags": tags.iter().map(|t| json!({"id": t, "name": t})).collect::<Vec<_>>(),
        "summarizations": {"s": {"v2": {"summary": {"markdown": summary}}}},
        "transcript": {"segments": []},
    })
}

/// Serves `recordings` the way Pocket's API does: the list, then each one's detail.
async fn pocket(recordings: &[Value]) -> eunomia_backend::sources::mock::Mock {
    let mut routes = vec![route("GET", "/public/recordings", json!({"data": recordings, "pagination": {"has_more": false}}))];
    for r in recordings {
        routes.push(route("GET", &format!("/public/recordings/{}", r["id"].as_str().unwrap()), json!({"data": r})));
    }
    serve(routes).await
}

/// A vault-specific source: every GitHub issue lands in the Build Paperless vault, which its members
/// read (search, get, recall) and nobody else does; the owner still sees it there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_vault_specific_source_files_every_record_in_its_destination() {
    let w = world().await;
    let mock = serve(vec![route("GET", "/issues", json!([issue(7, "Paperless importer")]))]).await;
    connect(&w, "github", &mock.base, json!({"personal_access_token": "ghp"})).await;
    w.route_to(&w.owner, "github", json!({"default_vault": w.work})).await;
    let (_, got) = w.http(&w.owner, "GET", "/api/connectors/github", None).await;
    assert_eq!(got["routing"]["default_vault"], w.work.as_str(), "{got}");
    assert_eq!(got["routing_fields"], json!(["repo", "title"]));

    let out = sync_source(&w.org, &w.owner.user.id, "github", None).await;
    assert_eq!(out["written"], 1, "{out}");
    assert_eq!(w.filed("github").await, [("acme/paperless#7".to_string(), w.work.clone())]);

    let own_id = "github:github.issue:acme/paperless#7".to_string();
    assert_eq!(w.search(&w.owner, "importer", json!({})).await, std::slice::from_ref(&own_id));
    // the member names it with its owner's key: unambiguous in a shared vault
    let shared = w.search(&w.member, "importer", json!({})).await;
    assert_eq!(shared.len(), 1, "{shared:?}");
    assert!(shared[0].ends_with(&own_id) && shared[0] != own_id, "{shared:?}");
    let got = w.ok(&w.member, "get", json!({"id": shared[0]})).await;
    assert_eq!(got["title"], "Paperless importer");
    let recalled = w.recall(&w.member, "paperless importer", json!({})).await;
    assert!(recalled.iter().any(|(t, v)| t.contains("Paperless importer") && v == "Build Paperless"), "{recalled:?}");
    let listed = w.ok(&w.member, "list", json!({})).await;
    assert_eq!(listed["total"], 1, "{listed}");

    // the outsider: nothing by search, list, recall, get (either id) or links
    assert!(w.search(&w.outsider, "importer", json!({})).await.is_empty());
    assert_eq!(w.ok(&w.outsider, "list", json!({})).await["total"], 0);
    assert!(w.recall(&w.outsider, "paperless importer", json!({})).await.is_empty());
    for id in [&shared[0], &own_id] {
        assert_eq!(w.call(&w.outsider, "get", json!({"id": id})).await["error"], "not found");
        assert_eq!(w.ok(&w.outsider, "links", json!({"id": id})).await["links"], json!([]));
    }
    // nor can the outsider read the work vault by naming it
    assert!(w.call(&w.outsider, "recall", json!({"query": "importer", "vault_id": w.work})).await.get("error").is_some());
    assert!(w.call(&w.outsider, "search", json!({"query": "importer", "vault_ids": [w.work]})).await.get("error").is_some());
}

/// A mixed Pocket source: recordings tagged "work" go to Build Paperless, the rest stay personal.
/// Later syncs keep each record where it was filed; a destination change applies to new recordings only.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mixed_pocket_source_splits_work_and_personal_recordings() {
    let w = world().await;
    let mock = pocket(&[
        recording("rec_work", "Paperless standup", &["work"], "Agreed the zanzibar importer fix."),
        recording("rec_home", "Dentist call", &[], "Booked the kiwi appointment."),
    ])
    .await;
    connect(&w, "pocketai", &mock.base, json!({"api_key": "pk"})).await;
    w.route_to(&w.owner, "pocketai", json!({"rules": [{"field": "tag", "value": "Work", "vault": w.work}]})).await;

    let out = sync_source(&w.org, &w.owner.user.id, "heypocket", None).await;
    assert_eq!(out["failed"], 0, "{out}");
    assert_eq!(w.filed("heypocket").await, [("rec_home".to_string(), String::new()), ("rec_work".to_string(), w.work.clone())]);

    // the owner reads both; the member only the work one; the outsider neither
    assert_eq!(w.search(&w.owner, "zanzibar", json!({})).await.len(), 1);
    assert_eq!(w.search(&w.owner, "kiwi", json!({})).await.len(), 1);
    assert_eq!(w.search(&w.member, "zanzibar", json!({})).await.len(), 1);
    assert!(w.search(&w.member, "kiwi", json!({})).await.is_empty(), "a personal recording stays the owner's");
    assert!(w.search(&w.outsider, "zanzibar", json!({})).await.is_empty() && w.search(&w.outsider, "kiwi", json!({})).await.is_empty());
    let member_recall = w.recall(&w.member, "zanzibar kiwi", json!({})).await;
    assert!(member_recall.iter().any(|(t, _)| t.contains("zanzibar")) && !member_recall.iter().any(|(t, _)| t.contains("kiwi")), "{member_recall:?}");
    let owner_recall = w.recall(&w.owner, "zanzibar kiwi", json!({})).await;
    assert!(owner_recall.contains(&("Paperless standup\nAgreed the zanzibar importer fix.\n\nTags: work".to_string(), "Build Paperless".to_string())), "{owner_recall:?}");
    assert!(owner_recall.iter().any(|(t, v)| t.contains("kiwi") && v != "Build Paperless"), "{owner_recall:?}");
    // the member's `get` of the work recording carries the stored recording, its owner's
    let id = &w.search(&w.member, "zanzibar", json!({})).await[0];
    assert_eq!(w.ok(&w.member, "get", json!({"id": id})).await["recording"]["tags"], json!(["work"]));

    // a later sync: the work recording changed, plus a new work one and a new personal one
    let mock = pocket(&[
        recording("rec_work", "Paperless standup", &["work"], "Agreed the zanzibar importer fix, shipping Friday."),
        recording("rec_work2", "Paperless retro", &["work"], "The mango migration went well."),
        recording("rec_home2", "Gym plan", &[], "Papaya smoothies after leg day."),
    ])
    .await;
    connect(&w, "pocketai", &mock.base, json!({"api_key": "pk"})).await;
    let out = sync_source(&w.org, &w.owner.user.id, "heypocket", None).await;
    assert_eq!(out["failed"], 0, "{out}");
    let filed = w.filed("heypocket").await;
    assert_eq!(
        filed,
        [("rec_home", ""), ("rec_home2", ""), ("rec_work", w.work.as_str()), ("rec_work2", w.work.as_str())].map(|(a, b)| (a.to_string(), b.to_string()))
    );
    assert!(w.search(&w.member, "friday", json!({})).await.len() == 1, "the update stayed in the work vault");

    // the destination changes: everything new goes to Side; what is stored stays put
    w.route_to(&w.owner, "pocketai", json!({"default_vault": w.side})).await;
    let mock = pocket(&[recording("rec_later", "Paperless planning", &["work"], "Plan the durian release.")]).await;
    connect(&w, "pocketai", &mock.base, json!({"api_key": "pk"})).await;
    sync_source(&w.org, &w.owner.user.id, "heypocket", None).await;
    let filed = w.filed("heypocket").await;
    assert!(filed.contains(&("rec_later".to_string(), w.side.clone())), "{filed:?}");
    assert!(filed.contains(&("rec_work".to_string(), w.work.clone())) && filed.contains(&("rec_home".to_string(), String::new())), "{filed:?}");
    assert!(w.search(&w.member, "durian", json!({})).await.is_empty(), "the member is not in Side");
}

/// "Move existing data" re-files records synced before the rules, with their facts and relations,
/// into the same-named entities in the destination; a fact the destination already has is not copied
/// twice; a second run moves nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn moving_existing_data_refiles_records_facts_and_relations_once() {
    let w = world().await;
    let owner = &w.owner.user.id;
    registry::ingest(
        &w.org,
        owner,
        &[recording("rec_w", "Paperless kickoff", &["work"], "Kickoff for the pomelo importer."), recording("rec_h", "Family dinner", &[], "Lychee dessert.")],
        &HeyPocketSource,
    )
    .await
    .unwrap();
    assert_eq!(w.filed("heypocket").await, [("rec_h".to_string(), String::new()), ("rec_w".to_string(), String::new())]);
    // facts and a relation drawn from the work recording (an agent citing it; extraction does the same)
    let src = "heypocket:heypocket.recording:rec_w";
    let ada = w.ok(&w.owner, "memory_write", json!({"subject_name": "Ada Lovelace", "subject_kind": "person", "text": "Ada leads the pomelo importer.", "source_record_id": src})).await;
    w.ok(&w.owner, "memory_write", json!({"subject_name": "Ada Lovelace", "subject_kind": "person", "text": "Ada prefers Rust.", "source_record_id": src})).await;
    let acme = w.ok(&w.owner, "memory_write", json!({"subject_name": "Acme", "subject_kind": "organisation", "text": "Acme funds the pomelo importer.", "source_record_id": src})).await;
    w.ok(&w.owner, "code_relate", json!({"from_id": ada["entity"]["id"], "to_id": acme["entity"]["id"], "label": "works_at", "source_record_id": src})).await;
    // the work vault already knows one of those facts
    w.ok(&w.owner, "memory_write", json!({"subject_name": "Ada Lovelace", "subject_kind": "person", "text": "Ada prefers Rust.", "vault_id": w.work})).await;
    let personal_facts = w.count("SELECT count() FROM memory WHERE vault = $personal").await;

    w.route_to(&w.owner, "pocketai", json!({"rules": [{"field": "tag", "value": "work", "vault": w.work}]})).await;
    // a routing change alone moves nothing
    assert_eq!(w.filed("heypocket").await[1].1, "");
    let (status, report) = w.http(&w.owner, "POST", "/api/connectors/pocketai/move", None).await;
    assert_eq!(status, 200, "{report}");
    assert_eq!(report, json!({"records": 1, "memories": 3, "relations": 1, "left": 0}));
    assert_eq!(w.filed("heypocket").await, [("rec_h".to_string(), String::new()), ("rec_w".to_string(), w.work.clone())]);

    // the facts are in the work vault once each, on its Ada and Acme; none are left in personal
    assert_eq!(w.count("SELECT count() FROM memory WHERE vault = $work AND text = 'Ada prefers Rust.'").await, 1, "not duplicated");
    assert_eq!(w.count("SELECT count() FROM memory WHERE vault = $work AND text = 'Ada leads the pomelo importer.' AND subject.vault = $work").await, 1);
    assert_eq!(w.count("SELECT count() FROM memory WHERE vault = $personal").await, personal_facts - 3);
    assert_eq!(w.count("SELECT count() FROM person WHERE vault = $work AND name = 'Ada Lovelace'").await, 1);
    assert_eq!(w.count("SELECT count() FROM relates_to WHERE label = 'works_at' AND in.vault = $work AND out.vault = $work").await, 1);
    assert_eq!(w.count("SELECT count() FROM relates_to WHERE label = 'works_at' AND in.vault = $personal").await, 0);
    // and the member now recalls them
    let recalled = w.recall(&w.member, "pomelo importer", json!({})).await;
    assert!(recalled.iter().any(|(t, v)| t == "Ada leads the pomelo importer." && v == "Build Paperless"), "{recalled:?}");

    // idempotent
    let (_, again) = w.http(&w.owner, "POST", "/api/connectors/pocketai/move", None).await;
    assert_eq!(again, json!({"records": 0, "memories": 0, "relations": 0, "left": 0}));
    assert_eq!(w.count("SELECT count() FROM memory WHERE vault = $work").await, 3);
}

/// `vault_ids` reads several vaults together, by id or name, in recall, reflect, search and entities_search.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn vault_ids_read_several_vaults_together() {
    let w = world().await;
    let owner = &w.owner.user.id;
    w.route_to(&w.owner, "pocketai", json!({"rules": [{"field": "title", "value": "paperless", "vault": w.work}, {"field": "title", "value": "side", "vault": w.side}]})).await;
    registry::ingest(
        &w.org,
        owner,
        &[
            recording("r1", "Paperless sync", &[], "The quince report is due."),
            recording("r2", "Side project", &[], "The quince prototype works."),
            recording("r3", "Personal note", &[], "The quince jam recipe."),
        ],
        &HeyPocketSource,
    )
    .await
    .unwrap();
    w.ok(&w.owner, "memory_write", json!({"subject_name": "Quince Corp", "subject_kind": "organisation", "text": "Quince Corp is a work client.", "vault_id": w.work})).await;
    w.ok(&w.owner, "memory_write", json!({"subject_name": "Quince Club", "subject_kind": "organisation", "text": "Quince Club is a side thing.", "vault_id": "Side"})).await;

    let vaults_of = |hits: Vec<(String, String)>| {
        let mut v: Vec<String> = hits.into_iter().map(|(_, v)| v).collect();
        v.sort();
        v.dedup();
        v
    };
    let both = w.recall(&w.owner, "quince", json!({"vault_ids": [w.work, "Side"]})).await;
    assert_eq!(vaults_of(both.clone()), ["Build Paperless", "Side"], "{both:?}");
    assert_eq!(both.len(), 4, "a record and a fact in each: {both:?}");
    assert_eq!(vaults_of(w.recall(&w.owner, "quince", json!({"vault_ids": ["Build Paperless"]})).await), ["Build Paperless"]);
    // vault_id and vault_ids together read both
    assert_eq!(vaults_of(w.recall(&w.owner, "quince", json!({"vault_id": "Side", "vault_ids": [w.work]})).await), ["Build Paperless", "Side"]);
    // the default scope: personal plus every org vault
    assert_eq!(w.recall(&w.owner, "quince", json!({})).await.len(), 5);

    assert_eq!(w.search(&w.owner, "quince", json!({"vault_ids": [w.work, w.side]})).await.len(), 2);
    assert_eq!(w.search(&w.owner, "quince", json!({"vault_ids": [w.personal]})).await, ["heypocket:heypocket.recording:r3"]);
    let ents = w.ok(&w.owner, "entities_search", json!({"query": "quince", "vault_ids": ["Build Paperless", "Side"]})).await;
    assert_eq!(ents["total"], 2, "{ents}");
    let ents = w.ok(&w.owner, "entities_search", json!({"query": "quince", "vault_ids": ["Side"]})).await;
    assert_eq!(ents["total"], 1, "{ents}");
    let reflected = w.ok(&w.owner, "reflect", json!({"query": "quince", "vault_ids": [w.work, w.side]})).await;
    assert_eq!(reflected["memories"].as_array().map(Vec::len), Some(4), "{reflected}");
    // a vault the caller is not in is refused, not ignored
    assert!(w.call(&w.member, "recall", json!({"query": "quince", "vault_ids": [w.work, w.side]})).await.get("error").is_some());
}

/// Routing writes only to vaults the connector's owner is in: saving a rule for another vault is
/// refused, and a rule whose vault the owner has since left falls back to the default vault (a stored
/// record in that vault is re-filed on its next sync).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn routing_never_writes_into_a_vault_its_owner_is_not_in() {
    let w = world().await;
    // the outsider cannot route into Build Paperless, nor make it a default
    let (status, body) = w.http(&w.outsider, "PUT", "/api/connectors/pocketai", Some(json!({"routing": {"rules": [{"field": "tag", "value": "work", "vault": w.work}]}}))).await;
    assert_eq!(status, 400, "{body}");
    let (status, _) = w.http(&w.outsider, "PUT", "/api/connectors/pocketai", Some(json!({"routing": {"default_vault": w.work}}))).await;
    assert_eq!(status, 400);
    let (status, _) = w.http(&w.owner, "PUT", "/api/connectors/pocketai", Some(json!({"routing": {"rules": [{"field": "repo", "value": "x", "vault": w.work}]}}))).await;
    assert_eq!(status, 400, "Pocket has no repo field");

    // the member routes their Pocket into Build Paperless, then is removed from it
    let member = &w.member.user.id;
    w.route_to(&w.member, "pocketai", json!({"rules": [{"field": "tag", "value": "work", "vault": w.work}]})).await;
    registry::ingest(&w.org, member, &[recording("m1", "Paperless notes", &["work"], "Member fig notes.")], &HeyPocketSource).await.unwrap();
    let filed = |id: &'static str| {
        let org = &w.org;
        async move {
            let mut res = org.db.test_raw().query("SELECT VALUE vault FROM ONLY cache_record WHERE external_id = $x LIMIT 1").bind(("x", id)).await.unwrap();
            res.take::<Option<RecordId>>(0).unwrap().map(|v| v.to_string()).unwrap_or_default()
        }
    };
    assert_eq!(filed("m1").await, w.work);
    assert_eq!(w.search(&w.owner, "fig", json!({})).await.len(), 1, "the vault's owner reads the member's routed record");
    w.ok(&w.owner, "vault_remove_member", json!({"vault_id": w.work, "email": "member@example.com"})).await;

    // the rule now falls back to the member's default (personal) vault
    registry::ingest(&w.org, member, &[recording("m2", "Paperless more", &["work"], "Member date notes.")], &HeyPocketSource).await.unwrap();
    assert_eq!(filed("m2").await, "", "not written into a vault the owner left");
    // and the record already there is re-filed when it syncs again
    registry::ingest(&w.org, member, &[recording("m1", "Paperless notes", &["work"], "Member fig notes, revised.")], &HeyPocketSource).await.unwrap();
    assert_eq!(filed("m1").await, "");
    assert!(w.search(&w.owner, "fig", json!({})).await.is_empty());
    assert_eq!(w.search(&w.member, "fig", json!({})).await, ["heypocket:heypocket.recording:m1"]);
}

/// After a vault merge, connectors that filed into either source vault can be pointed at the merged one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connectors_can_be_repointed_after_a_merge() {
    let w = world().await;
    w.route_to(&w.owner, "github", json!({"default_vault": w.work, "rules": [{"field": "repo", "value": "acme/side", "vault": w.side}]})).await;
    w.route_to(&w.owner, "pocketai", json!({"rules": [{"field": "tag", "value": "side", "vault": w.side}]})).await;
    w.route_to(&w.owner, "slack", json!({"default_vault": w.personal})).await;
    let merged = w.ok(&w.owner, "vault_merge", json!({"vault_id_a": w.work, "vault_id_b": w.side, "name": "Everything"})).await;
    let merged = merged["id"].as_str().unwrap();
    let (status, out) = w.http(&w.owner, "POST", "/api/connectors/repoint", Some(json!({"from": [w.work, w.side], "to": merged}))).await;
    assert_eq!(status, 200, "{out}");
    assert_eq!(out["connectors"], 2);
    let (_, gh) = w.http(&w.owner, "GET", "/api/connectors/github", None).await;
    assert_eq!(gh["routing"], json!({"default_vault": merged, "rules": [{"field": "repo", "value": "acme/side", "vault": merged}]}));
    let (_, slack) = w.http(&w.owner, "GET", "/api/connectors/slack", None).await;
    assert_eq!(slack["routing"]["default_vault"], w.personal.as_str());
    // only to a vault the caller is in
    let (status, _) = w.http(&w.outsider, "POST", "/api/connectors/repoint", Some(json!({"from": [w.work], "to": merged}))).await;
    assert_eq!(status, 403);
}
