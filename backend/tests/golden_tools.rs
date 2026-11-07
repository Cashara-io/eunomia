//! Golden snapshots for every tool in the registry, run against a real
//! in-memory SurrealDB. The tool list comes from the registry itself, so a new
//! tool without a golden fails `every_registered_tool_has_a_golden`. No network
//! calls: tools that need an OpenAI key snapshot their no-key behaviour.
//!
//! First run / intentional change: `INSTA_UPDATE=always cargo test --test golden_tools`
//! then review the diff under `tests/snapshots`.

mod common;

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;

use common::{Normalizer, TestApp};
use eunomia_backend::sources::registry as sources;
use eunomia_backend::tools::registry;
use serde_json::{json, Value};

struct Golden {
    app: TestApp,
    norm: Normalizer,
    covered: BTreeSet<String>,
}

impl Golden {
    /// Call `tool`, snapshot the normalized output as `label`, return the raw output.
    ///
    /// Not an `async fn`: the future is built in this short frame and handed back boxed, so the long
    /// `goldens` poll frame holds a pointer per call. As an `async fn`, a debug build kept a copy of every
    /// call's future (~5 KB each) in that frame, ~550 KiB on top of the tool call's own stack.
    fn snap<'a>(&'a mut self, label: &'a str, tool: &'a str, args: Value) -> Pin<Box<dyn Future<Output = Value> + 'a>> {
        Box::pin(async move {
            let out = self.app.tool(tool, args).await;
            let mut stable = self.norm.apply(&out);
            // Equal-scored hits come back in random-id order; sort them so the golden is stable.
            for key in ["results", "memories"] {
                if let Some(Value::Array(items)) = stable.get_mut(key) {
                    for field in ["text", "name"] {
                        if items.iter().all(|i| i.get(field).is_some()) {
                            items.sort_by_key(|i| i[field].to_string());
                            break;
                        }
                    }
                    // Scores and indexes depend on the (random-id) tie-break rank, so they are not stable.
                    for item in items.iter_mut() {
                        for noisy in ["score", "index"] {
                            if let Some(sc) = item.get_mut(noisy) {
                                *sc = json!("<unstable>");
                            }
                        }
                    }
                }
            }
            insta::assert_json_snapshot!(label, stable);
            self.covered.insert(tool.to_string());
            out
        })
    }
}

fn s(v: &Value, ptr: &str) -> String {
    v.pointer(ptr).and_then(Value::as_str).unwrap_or_else(|| panic!("no string at {ptr} in {v}")).to_string()
}

#[test]
fn every_registered_tool_has_a_golden() {
    // One await per tool makes the `goldens` future large; polled on a default 2 MiB test-thread stack
    // it overflows a debug build once a few more tools are added. Run it on a thread with room instead.
    std::thread::Builder::new()
        .name("golden_tools".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(goldens());
        })
        .unwrap()
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
}

/// Invitations, roles and ownership, in a vault of their own so the vaults above are untouched.
/// Its own function: inlined into `goldens` the future is large enough to overflow a debug stack.
fn role_goldens<'a>(g: &'a mut Golden, bob: &'a eunomia_backend::models_user::User) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>> {
    Box::pin(async move {
    let h = Box::pin(g.snap("vault_create__handover", "vault_create", json!({"name": "Handover", "kind": "org"}))).await;
    let handover = s(&h, "/id");
    Box::pin(g.snap("vault_invite__handover", "vault_invite", json!({"vault_id": handover, "email": bob.email}))).await;
    // the owner has no invitation of their own to answer
    Box::pin(g.snap("vault_invitations", "vault_invitations", json!({}))).await;
    Box::pin(g.snap("vault_accept_invitation", "vault_accept_invitation", json!({"vault_id": handover}))).await;
    Box::pin(g.snap("vault_decline_invitation", "vault_decline_invitation", json!({"vault_id": handover}))).await;
    Box::pin(g.snap("vault_get", "vault_get", json!({"vault_id": handover}))).await;
    Box::pin(g.snap("vault_check", "vault_check", json!({"vault_id": handover}))).await;
    Box::pin(g.snap("vault_repair", "vault_repair", json!({"vault_id": handover}))).await;
    Box::pin(g.snap("vault_set_role", "vault_set_role", json!({"vault_id": handover, "email": bob.email, "role": "owner"}))).await;
    Box::pin(g.snap("vault_set_role__last_owner", "vault_set_role", json!({"vault_id": handover, "email": g.app.user.email.clone(), "role": "member"}))).await;
    Box::pin(g.snap("vault_transfer_ownership", "vault_transfer_ownership", json!({"vault_id": handover, "email": bob.email}))).await;

    })
}

/// Peer vault tools without a second instance: a share code (its random parts masked), the vault's
/// peers, a join with a bad code and a sync of an unknown link (both errors).
fn peer_goldens<'a>(g: &'a mut Golden) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>> {
    Box::pin(async move {
        // an address the code can carry whatever this machine's network is
        unsafe { std::env::set_var("EUNOMIA_PEER_ADDRS", "https://golden.example.com") };
        let v = Box::pin(g.snap("vault_create__peer", "vault_create", json!({"name": "Peer Share", "kind": "org"}))).await;
        let vault = s(&v, "/id");
        let mask = |mut out: Value| {
            for key in ["code", "addresses", "fingerprint", "id"] {
                if out.get(key).is_some() {
                    out[key] = json!(format!("<{key}>"));
                }
            }
            if let Some(Value::Array(codes)) = out.get_mut("codes") {
                for c in codes {
                    c["id"] = json!("<id>");
                }
            }
            out
        };
        let share = g.app.tool("vault_share_code", json!({"vault_id": vault, "role": "read"})).await;
        assert!(s(&share, "/code").starts_with("eunomia1:"), "{share}");
        insta::assert_json_snapshot!("vault_share_code", g.norm.apply(&mask(share)));
        g.covered.insert("vault_share_code".into());
        let peers = g.app.tool("vault_peers", json!({"vault_id": vault})).await;
        insta::assert_json_snapshot!("vault_peers", g.norm.apply(&mask(peers)));
        g.covered.insert("vault_peers".into());
        Box::pin(g.snap("vault_join_shared__bad_code", "vault_join_shared", json!({"share_code": "eunomia1:nope"}))).await;
        Box::pin(g.snap("vault_peer_sync__unknown", "vault_peer_sync", json!({"link_id": "0".repeat(32)}))).await;
    })
}

async fn goldens() {
    let mut g = Golden { app: TestApp::new().await, norm: Normalizer::default(), covered: BTreeSet::new() };
    let bob = common::register(&g.app.state, "bob@example.com").await;

    // Deterministic offline records through the demo source's own mapper + the real ingest path.
    let demo = sources::get("demo").unwrap();
    let raws: Vec<Value> = [
        ("t1", "Woolworths", "Groceries", -4250, "2026-01-05T09:00:00Z"),
        ("t2", "Netflix", "Subscriptions", -1699, "2026-01-06T09:00:00Z"),
        ("t3", "Salary", "Income", 300000, "2026-01-07T09:00:00Z"),
    ]
    .iter()
    .map(|(id, d, c, amt, at)| {
        json!({"_kind":"txn","id":id,"account":"Spending","description":d,"category":c,"amount_cents":amt,"created_at":at})
    })
    .collect();
    let report = sources::ingest(&g.app.state.org(&g.app.user.org).await.unwrap(), &g.app.user.id, &raws, demo.as_ref()).await.unwrap();
    assert_eq!(report.written, 3, "{:?}", report.errors);

    // -- docs, generic cache tools --
    g.snap("docs", "docs", json!({})).await;
    g.snap("docs__unknown_topic", "docs", json!({"topic": "nope"})).await;
    let found = g.snap("search", "search", json!({"query": "Woolworths", "mode": "keyword"})).await;
    let record_id = s(&found, "/results/0/id");
    g.snap("get", "get", json!({"id": record_id})).await;
    g.snap("list", "list", json!({"type": "up.transaction", "sort": "occurred_at", "limit": 10})).await;
    g.snap("links", "links", json!({"id": record_id})).await;

    // -- entities and memories --
    let w1 = g.snap("memory_write", "memory_write",
        json!({"subject_name":"Alice","subject_kind":"person","text":"Alice likes green tea"})).await;
    let alice = s(&w1, "/entity/id");
    let mem1 = s(&w1, "/memory/id");
    g.snap("memory_write__second", "memory_write",
        json!({"subject_name":"Alice","subject_kind":"person","text":"Alice works at Acme","type":"experience"})).await;
    let w3 = g.snap("memory_write__org", "memory_write",
        json!({"subject_name":"Acme","subject_kind":"organisation","text":"Acme builds anvils"})).await;
    let acme = s(&w3, "/entity/id");
    g.snap("memory_write__observation", "memory_write",
        json!({"subject_name":"Alice","subject_kind":"person","text":"Alice is a tea drinker at Acme","type":"observation"})).await;
    g.snap("entities_search", "entities_search", json!({"query": "alice"})).await;
    g.snap("entities_get", "entities_get", json!({"id": alice})).await;
    g.snap("entities_graph", "entities_graph", json!({})).await;
    g.snap("recall", "recall", json!({"query": "tea"})).await;
    g.snap("reflect", "reflect", json!({"query": "tea"})).await;
    g.snap("consolidate_observations", "consolidate_observations", json!({})).await;
    g.snap("memory_update", "memory_update", json!({"memory_id": mem1, "text": "Alice likes oolong tea"})).await;
    g.snap("entity_update", "entity_update", json!({"entity_id": alice, "aliases": ["Al"], "summary": "Tea person"})).await;

    // code graph
    let repo = g.snap("code_entity_upsert", "code_entity_upsert",
        json!({"kind":"repository","name":"eunomia","summary":"memory layer"})).await;
    let repo_id = s(&repo, "/id");
    let file = g.snap("code_entity_upsert__file", "code_entity_upsert",
        json!({"kind":"file","name":"main.rs","parent_id": repo_id})).await;
    g.snap("code_relate", "code_relate", json!({"from_id": s(&file, "/id"), "to_id": repo_id, "label": "imports"})).await;

    // merge + deletes
    let dup = g.snap("memory_write__duplicate_entity", "memory_write",
        json!({"subject_name":"Alicia","subject_kind":"person","text":"Alicia is the same person"})).await;
    g.snap("entity_merge", "entity_merge", json!({"winner_id": alice, "loser_id": s(&dup, "/entity/id")})).await;
    g.snap("memory_delete", "memory_delete", json!({"memory_id": mem1})).await;
    g.snap("entity_delete", "entity_delete", json!({"entity_id": acme})).await;

    // -- vaults --
    let v = g.snap("vault_create", "vault_create", json!({"name": "Team", "kind": "org"})).await;
    let team = s(&v, "/id");
    g.snap("vault_list", "vault_list", json!({})).await;
    g.snap("vault_rename", "vault_rename", json!({"vault_id": team, "name": "Team Renamed"})).await;
    g.snap("vault_invite", "vault_invite", json!({"vault_id": team, "email": bob.email})).await;
    g.snap("vault_members", "vault_members", json!({"vault_id": team})).await;
    let clone = g.snap("vault_clone", "vault_clone", json!({"vault_id": team, "name": "Team Copy"})).await;
    let v2 = g.snap("vault_create__second", "vault_create", json!({"name": "Other", "kind": "org"})).await;
    g.snap("vault_merge", "vault_merge", json!({"vault_id_a": team, "vault_id_b": s(&v2, "/id"), "name": "Merged"})).await;
    g.snap("vault_remove_member", "vault_remove_member", json!({"vault_id": team, "email": bob.email})).await;
    g.snap("vault_leave", "vault_leave", json!({"vault_id": team})).await;
    g.snap("vault_delete", "vault_delete", json!({"vault_id": s(&clone, "/id")})).await;

    // -- documents (the index job run by hand: no worker in this test, so nothing else moves) --
    let up = g.snap("document_upload", "document_upload", json!({"filename": "anvil-budget.md", "text": "# Anvil budget\n\nThe anvil budget doubles in March."})).await;
    let doc = s(&up, "/id");
    let jobs = eunomia_backend::jobs::claim(g.app.control(), "golden", 10, std::time::Duration::from_secs(30), 100).await.unwrap();
    for job in jobs.into_iter().filter(|j| j.kind == eunomia_backend::jobs::kind::INDEX_DOCUMENT) {
        Box::pin(common::sys(eunomia_backend::documents::index_job(g.app.state.clone(), job))).await.unwrap();
    }
    g.snap("document_get", "document_get", json!({"id": doc})).await;
    g.snap("document_list", "document_list", json!({})).await;
    g.snap("document_download", "document_download", json!({"id": doc})).await;
    g.snap("document_export", "document_export", json!({"id": doc})).await;
    g.snap("document_delete", "document_delete", json!({"id": doc})).await;

    // -- an agent's own entity kind (last, so it changes no snapshot above) --
    let orders = g.snap("memory_write__custom_kind", "memory_write",
        json!({"subject_name":"orders_db","subject_kind":"db","text":"orders_db runs Postgres 16"})).await;
    g.snap("entity_types", "entity_types", json!({})).await;
    // -- the project ontology (after entity_types; proposals change no snapshot above) --
    Box::pin(g.snap("ontology_propose", "ontology_propose", json!({
        "kinds": [{"kind": "db", "description": "A database a service reads or writes", "synonyms": ["database"]}],
        "links": [{"from_kind": "person", "label": "owns", "to_kind": "db"}],
        "properties": [{"kind": "db", "name": "size_gb", "datatype": "number", "unit": "GB", "description": "How much it stores"}],
    }))).await;
    Box::pin(g.snap("ontology_get", "ontology_get", json!({}))).await;
    Box::pin(g.snap("ontology_suggest", "ontology_suggest", json!({}))).await;
    // a value of a proposed property is refused; once an owner approved it (done here in the database), it is kept
    let orders_id = s(&orders, "/entity/id");
    let set = json!({"entity_id": orders_id, "property": "size_gb", "value": 120, "as_of": "2026-11-01"});
    Box::pin(g.snap("entity_set_property__proposed", "entity_set_property", set.clone())).await;
    g.app.db().await.test_raw().query("UPDATE ontology_property SET status = 'approved' WHERE name = 'size_gb'").await.unwrap().check().unwrap();
    Box::pin(g.snap("entity_set_property", "entity_set_property", set)).await;
    // a metric over it: a proposed one runs only on request, an approved one by name
    Box::pin(g.snap("ontology_propose__metric", "ontology_propose", json!({
        "metrics": [{"kind": "db", "name": "total_size_gb", "expr": {"agg": "sum", "property": "size_gb"}, "description": "How much the databases store"}],
    }))).await;
    let run = json!({"metric": "total_size_gb", "as_of": "2026-11-02"});
    Box::pin(g.snap("metric_eval__proposed", "metric_eval", run.clone())).await;
    g.app.db().await.test_raw().query("UPDATE ontology_metric SET status = 'approved' WHERE name = 'total_size_gb'").await.unwrap().check().unwrap();
    Box::pin(g.snap("metric_eval", "metric_eval", run)).await;
    // a what-if over it: the metric before and after the databases grow by half, nothing saved
    let what_if = json!({"changes": [{"scale": {"kind": "db", "property": "size_gb", "factor": 1.5}}], "metrics": ["total_size_gb"], "as_of": "2026-11-02"});
    Box::pin(g.snap("scenario_run", "scenario_run", what_if.clone())).await;
    Box::pin(g.snap("show_view__scenario", "show_view", json!({"view": "scenario", "headline": "What if the databases grow by half?", "scenario": what_if}))).await;
    Box::pin(g.snap("scenario_run__unknown_property", "scenario_run", json!({"changes": [{"scale": {"kind": "person", "property": "name", "factor": 2}}]}))).await;
    // -- auto-mapping: a CSV profiled without a model, its rows refused until an owner approved the mapping, then imported --
    let csv = g.snap("document_upload__csv", "document_upload", json!({"filename": "menu.csv", "text": "product,price\nLatte,4.50\nMocha,oops\n"})).await;
    let csv_id = s(&csv, "/id");
    Box::pin(g.snap("ontology_automap", "ontology_automap", json!({"document_id": csv_id}))).await;
    let rows = json!({"document_id": csv_id, "kind": "menu_item", "name_column": "product"});
    Box::pin(g.snap("document_import_rows__unapproved", "document_import_rows", rows.clone())).await;
    Box::pin(g.snap("ontology_propose__mapped", "ontology_propose", json!({
        "kinds": [{"kind": "menu_item"}],
        "properties": [{"kind": "menu_item", "name": "price", "datatype": "money", "unit": "AUD", "source_map": [{"source": "document", "record_type": csv_id, "path": "price"}]}],
    }))).await;
    g.app.db().await.test_raw().query("UPDATE ontology_kind SET status = 'approved' WHERE kind = 'menu_item'; UPDATE ontology_property SET status = 'approved' WHERE kind = 'menu_item'").await.unwrap().check().unwrap();
    Box::pin(g.snap("document_import_rows", "document_import_rows", rows)).await;
    // -- bulk writes (after entity_types, so they change no snapshot above): one result per item, a bad one fails alone --
    g.snap("memory_write_batch", "memory_write_batch", json!({"items": [
        {"subject_name": "Import page 1", "subject_kind": "file", "text": "page one says hello"},
        {"subject_name": "Import page 2", "subject_kind": "file"},
        {"subject_name": "Import page 3", "subject_kind": "file", "text": "page three says goodbye", "type": "experience"},
    ]})).await;

    // -- agent skills (last, so they change no snapshot above; keyword-only: no embedding key) --
    let skill = Box::pin(g.snap("skill_save", "skill_save", json!({"name": "anvil-budget", "description": "How to read the anvil budget", "body": "# Anvil budget\n\n1. Open the sheet."}))).await;
    Box::pin(g.snap("skill_find", "skill_find", json!({"query": "anvil"}))).await;
    Box::pin(g.snap("skill_list", "skill_list", json!({"limit": 10}))).await;
    Box::pin(g.snap("skill_get", "skill_get", json!({"id": s(&skill, "/id")}))).await;
    Box::pin(g.snap("skill_delete", "skill_delete", json!({"id": s(&skill, "/id")}))).await;
    // -- the change log: the first writes above, oldest first --
    Box::pin(g.snap("changes_since", "changes_since", json!({"after": 0, "limit": 3}))).await;

    // -- secret requests (after the change log: they add no change rows). Only a person in a browser can
    // fill a request, so the transform and the outbound call snapshot the unfilled handle's refusal --
    let req = Box::pin(g.snap("secret_request", "secret_request", json!({"name": "golden_key", "label": "Golden key", "description": "to call the example API", "target": {"hosts": ["api.example.com"]}}))).await;
    Box::pin(g.snap("secret_request_status", "secret_request_status", json!({"request_id": s(&req, "/request_id")}))).await;
    let handle = s(&req, "/handle/$secret");
    Box::pin(g.snap("secret_transform", "secret_transform", json!({"op": "template", "inputs": {"key": handle}, "params": {"template": "Bearer {{key}}"}}))).await;
    Box::pin(g.snap("http_request", "http_request", json!({"url": "https://api.example.com/v1/ping", "headers": {"Authorization": {"$secret": handle}}}))).await;

    // -- invitations and roles (last, so they change no snapshot above) --
    role_goldens(&mut g, &bob).await;
    // -- peer vaults (last; no second instance: what works without one) --
    peer_goldens(&mut g).await;
    // -- what a change affects (last, so the ids it shows first change no snapshot above): the code graph
    // above has a file that `imports` and is `part_of` its repository --
    let hood = Box::pin(g.snap("entities_neighbourhood", "entities_neighbourhood", json!({"entity_id": repo_id}))).await;
    let (file_id, part_of) = (s(&hood, "/nodes/1/id"), s(&hood, "/edges/1/id"));
    Box::pin(g.snap("show_view", "show_view", json!({
        "view": "impact", "start_id": repo_id, "headline": "The repository moves to a new host.",
        "affected": [{"entity_id": file_id, "path": [part_of], "why": "It is part of the repository."}],
    }))).await;

    let registered: BTreeSet<String> = registry::all_tools().keys().map(|k| k.to_string()).collect();
    let missing: Vec<_> = registered.difference(&g.covered).collect();
    let extra: Vec<_> = g.covered.difference(&registered).collect();
    assert!(missing.is_empty(), "tools without a golden snapshot: {missing:?}");
    assert!(extra.is_empty(), "goldens for tools that are not registered: {extra:?}");
    println!("golden tools covered: {}", g.covered.len());
}
