//! Golden snapshots for every tool in the registry, run against a real
//! in-memory SurrealDB. The tool list comes from the registry itself, so a new
//! tool without a golden fails `every_registered_tool_has_a_golden`. No network
//! calls: tools that need an OpenAI key snapshot their no-key behaviour.
//!
//! First run / intentional change: `INSTA_UPDATE=always cargo test --test golden_tools`
//! then review the diff under `tests/snapshots`.

mod common;

use std::collections::BTreeSet;

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
    async fn snap(&mut self, label: &str, tool: &str, args: Value) -> Value {
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
    }
}

fn s(v: &Value, ptr: &str) -> String {
    v.pointer(ptr).and_then(Value::as_str).unwrap_or_else(|| panic!("no string at {ptr} in {v}")).to_string()
}

#[test]
fn every_registered_tool_has_a_golden() {
    // boxed: one await per tool makes this future large, and pinned on the test thread's stack it
    // overflows a debug build once a few more tools are added. And on its own 8 MiB thread: the
    // in-memory SurrealDB 3.3.5 engine runs every query on this thread, and in a debug build one of
    // them goes past the default 2 MiB test stack (it passes with 3 MiB; 3.3.1 fitted in 2 MiB).
    let run = std::thread::Builder::new()
        .name("goldens".into())
        .stack_size(8 << 20)
        .spawn(|| tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(Box::pin(goldens())))
        .unwrap()
        .join();
    if let Err(panic) = run {
        std::panic::resume_unwind(panic);
    }
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
    g.snap("memory_write__custom_kind", "memory_write",
        json!({"subject_name":"orders_db","subject_kind":"db","text":"orders_db runs Postgres 16"})).await;
    g.snap("entity_types", "entity_types", json!({})).await;
    // -- bulk writes (after entity_types, so they change no snapshot above): one result per item, a bad one fails alone --
    g.snap("memory_write_batch", "memory_write_batch", json!({"items": [
        {"subject_name": "Import page 1", "subject_kind": "file", "text": "page one says hello"},
        {"subject_name": "Import page 2", "subject_kind": "file"},
        {"subject_name": "Import page 3", "subject_kind": "file", "text": "page three says goodbye", "type": "experience"},
    ]})).await;

    // -- agent skills (last, so they change no snapshot above; keyword-only: no embedding key) --
    let skill = Box::pin(g.snap("skill_save", "skill_save", json!({"name": "anvil-budget", "description": "How to read the anvil budget", "body": "# Anvil budget\n\n1. Open the sheet."}))).await;
    Box::pin(g.snap("skill_find", "skill_find", json!({"query": "anvil"}))).await;
    Box::pin(g.snap("skill_get", "skill_get", json!({"id": s(&skill, "/id")}))).await;
    Box::pin(g.snap("skill_delete", "skill_delete", json!({"id": s(&skill, "/id")}))).await;

    let registered: BTreeSet<String> = registry::all_tools().keys().map(|k| k.to_string()).collect();
    let missing: Vec<_> = registered.difference(&g.covered).collect();
    let extra: Vec<_> = g.covered.difference(&registered).collect();
    assert!(missing.is_empty(), "tools without a golden snapshot: {missing:?}");
    assert!(extra.is_empty(), "goldens for tools that are not registered: {extra:?}");
    println!("golden tools covered: {}", g.covered.len());
}
