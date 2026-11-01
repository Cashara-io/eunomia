//! Agent skills (#93): `skill_save`, `skill_find`, `skill_get`, `skill_delete` through the registry and a
//! real in-memory SurrealDB. Semantic find uses a stub embeddings provider that maps words to concepts
//! (as `recall_semantic.rs` does), so no live key is needed.

mod common;

use axum::body::Body;
use axum::http::{header, Request};
use common::TestApp;
use eunomia_backend::embeddings::service as embeddings;
use eunomia_backend::models_user::{create_api_token_with, User};
use eunomia_backend::rid::{self, RecordIdExt};
use eunomia_backend::scopes;
use eunomia_backend::tools::registry;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const CONCEPTS: &[&[&str]] = &[&["rollback", "revert", "undo", "restore"], &["deploy", "release", "ship", "publish"], &["invoice", "billing", "payment"]];

fn concept_vector(text: &str) -> Vec<f32> {
    let mut v = vec![0.0_f32; embeddings::dim()];
    for word in text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 2) {
        let i = match CONCEPTS.iter().position(|c| c.contains(&word)) {
            Some(c) => c,
            None => 100 + word.bytes().fold(0usize, |h, b| (h * 31 + b as usize) % 1000),
        };
        v[i] += 1.0;
    }
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    v.iter().map(|x| x / n).collect()
}

/// An OpenAI-compatible `/v1/embeddings` returning [`concept_vector`]s.
async fn stub_embeddings() -> String {
    use axum::{routing::post, Json, Router};
    async fn embed(Json(body): Json<Value>) -> Json<Value> {
        let input: Vec<String> = match &body["input"] {
            Value::String(s) => vec![s.clone()],
            v => serde_json::from_value(v.clone()).unwrap_or_default(),
        };
        let data: Vec<Value> = input.iter().enumerate().map(|(i, t)| json!({"index": i, "embedding": concept_vector(t)})).collect();
        Json(json!({ "data": data }))
    }
    let router = Router::new().route("/v1/embeddings", post(embed));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{addr}/v1")
}

/// A test app whose user embeds through the stub.
async fn semantic_app() -> TestApp {
    let mut settings = common::test_settings();
    settings.openai_api_key = Some("sk-test".into());
    let app = TestApp::with_settings(settings).await;
    let base = stub_embeddings().await;
    let sql = format!("UPSERT app_settings:`{}` SET owner = {}, openai_base_url = '{base}'", rid::key_string(&app.user.id.key).unwrap(), app.user.id.to_string());
    app.db().await.test_raw().query(sql).await.unwrap().check().unwrap();
    app
}

async fn call_as(app: &TestApp, user: &User, tool: &str, args: Value) -> Value {
    eunomia_backend::authz::as_system(registry::call(&app.state, user, tool, args)).await.expect("tool call")
}

fn save_args(name: &str, description: &str) -> Value {
    json!({"name": name, "description": description, "body": format!("# {name}\n\n1. do the thing")})
}

fn names(out: &Value) -> Vec<String> {
    out["skills"].as_array().unwrap_or_else(|| panic!("{out}")).iter().map(|s| s["name"].as_str().unwrap().to_string()).collect()
}

/// A tool call over REST with a bearer token (so the credential's vault restriction applies).
async fn rest(app: &TestApp, token: &str, tool: &str, args: Value) -> Value {
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/tools/{tool}"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(args.to_string()))
        .unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

#[tokio::test]
async fn save_then_find_by_keyword_then_get_the_body() {
    let app = TestApp::new().await;
    let saved = app.tool("skill_save", save_args("deploy-rollback", "How to roll back a bad deploy on the web tier")).await;
    assert_eq!((saved["name"].as_str(), saved["embedded"].as_bool()), (Some("deploy-rollback"), Some(false)), "no key, so no vector: {saved}");
    app.tool("skill_save", save_args("invoice-export", "Export last month's invoices")).await;

    let found = app.tool("skill_find", json!({"query": "roll back deploy"})).await;
    assert_eq!(names(&found), ["deploy-rollback"], "{found}");
    let hit = &found["skills"][0];
    assert!(hit.get("body").is_none() && hit["updated_at"].is_string() && hit["vault"].is_string(), "{hit}");

    let id = saved["id"].as_str().unwrap();
    let got = app.tool("skill_get", json!({"id": id})).await;
    assert_eq!(got["body"], "# deploy-rollback\n\n1. do the thing", "{got}");
    let by_name = app.tool("skill_get", json!({"name": "deploy-rollback"})).await;
    assert_eq!(by_name["id"], id);

    let gone = app.tool("skill_delete", json!({"id": id})).await;
    assert_eq!(gone["deleted"], id, "{gone}");
    assert!(app.tool("skill_get", json!({"id": id})).await.get("error").is_some());
    assert!(names(&app.tool("skill_find", json!({"query": "deploy"})).await).is_empty());
}

#[tokio::test]
async fn saving_a_name_again_replaces_the_skill_in_that_vault_only() {
    let app = TestApp::new().await;
    let team = app.tool("vault_create", json!({"name": "Team"})).await;
    assert!(team.get("error").is_none(), "{team}");
    let first = app.tool("skill_save", save_args("runbook", "old words")).await;
    let again = app.tool("skill_save", json!({"name": "runbook", "description": "new words", "body": "v2"})).await;
    assert_eq!(first["id"], again["id"], "same (vault, name): same skill");
    let in_team = app.tool("skill_save", json!({"name": "runbook", "description": "team words", "body": "t", "vault_id": "Team"})).await;
    assert_ne!(in_team["id"], first["id"]);

    let got = app.tool("skill_get", json!({"id": first["id"]})).await;
    assert_eq!((got["description"].as_str(), got["body"].as_str()), (Some("new words"), Some("v2")), "{got}");
    let all = app.tool("skill_find", json!({"query": "words"})).await;
    assert_eq!(all["skills"].as_array().unwrap().len(), 2, "{all}");
    let one = app.tool("skill_find", json!({"query": "words", "vault_id": "Team"})).await;
    assert_eq!(one["skills"][0]["id"], in_team["id"], "{one}");

    // the same name in two readable vaults is ambiguous by name, and the error names both ids
    let amb = app.tool("skill_get", json!({"name": "runbook"})).await;
    let msg = amb["error"].as_str().unwrap_or_else(|| panic!("{amb}"));
    assert!(msg.contains(first["id"].as_str().unwrap()) && msg.contains(in_team["id"].as_str().unwrap()), "{msg}");
    let pick = app.tool("skill_get", json!({"name": "runbook", "vault_id": "Team"})).await;
    assert_eq!(pick["body"], "t");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_saves_of_one_new_name_all_succeed_as_one_skill() {
    let app = TestApp::new().await;
    let saves = (0..8).map(|i| app.tool("skill_save", json!({"name": "race", "description": "d", "body": format!("v{i}")})));
    let saved = futures::future::join_all(saves).await;
    assert!(saved.iter().all(|s| s.get("error").is_none()), "the loser of the unique (vault, name) index replaces the winner: {saved:?}");
    let ids: std::collections::BTreeSet<String> = saved.iter().map(|s| s["id"].to_string()).collect();
    assert_eq!(ids.len(), 1, "one skill: {saved:?}");
}

/// Regression guard for the commit-conflict retry in `skill_save` (its upsert and its embedding write)
/// and `skill_delete`. On SurrealDB 3.3.5 every write to `skill` queues a background compaction of
/// `skill_embedding_idx` that commits in its own transaction, and a skill write that commits just after
/// it fails with a commit conflict (`db.conflict`). Without `tx::with_retry` that reaches the caller.
///
/// Concurrent writers cycle save, save again, save a third time (each a new description, so a new
/// vector) and delete through the tools, while a probe runs the same three statements bare (no retry)
/// and counts their conflicts. The run lasts until the probe has met `ENOUGH_BARE_CONFLICTS` conflicts
/// and the tools have done `MIN_CYCLES` cycles (or until `DEADLINE`), so the race provably fired many
/// times during the run, and every tool call in it must have succeeded and embedded (a lost embedding
/// write is no error to the caller, only `embedded: false`). The run starts on a fresh org, so it covers
/// the index's concurrent build as well as the compactions after it.
///
/// Measured on 3.3.5: with every retry removed, 20 runs in 20 failed (10 normal, 10 `TEST_HARDENED`),
/// each within about 60 cycles; with only one retry removed, 4 in 5 (delete), 5 in 5 (embedding) and
/// 5 in 5 (upsert). With the retry, 40 in 40 passed, a run taking about 9 to 20 s after setup on a loaded box.
/// `MIN_BARE_CONFLICTS` is only a floor that says the race still fires at all, checked once the run did
/// `MIN_CYCLES` cycles: a machine so stalled that it did fewer before `DEADLINE` cannot tell.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn skill_writes_retry_conflicts_with_hnsw_index_maintenance() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use eunomia_backend::store::skills as q;
    const WORKERS: usize = 4;
    const ENOUGH_BARE_CONFLICTS: usize = 12;
    const MIN_CYCLES: usize = 150;
    const MIN_BARE_CONFLICTS: usize = 3;
    const DEADLINE: Duration = Duration::from_secs(20);

    let app = semantic_app().await;
    let seed = app.tool("skill_save", save_args("seed", "deploy the web tier")).await;
    assert_eq!(seed["embedded"], true, "{seed}");
    let vault = rid::parse(seed["vault"].as_str().unwrap()).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let (cycles, bare_conflicts) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let failures: Arc<Mutex<Vec<String>>> = Arc::default();

    let mut tasks = Vec::new();
    for w in 0..WORKERS {
        let (state, user, stop, cycles, failures) = (app.state.clone(), app.user.clone(), stop.clone(), cycles.clone(), failures.clone());
        tasks.push(tokio::spawn(async move {
            let call = |tool: &'static str, args: Value| {
                let (state, user) = (state.clone(), user.clone());
                async move { eunomia_backend::authz::as_system(registry::call(&state, &user, tool, args)).await.expect("tool call") }
            };
            let mut i = 0;
            while !stop.load(SeqCst) {
                let name = format!("w{w}-{i}");
                let saved = call("skill_save", json!({"name": name, "description": format!("deploy step{w}x{i}"), "body": "b1"})).await;
                let again = call("skill_save", json!({"name": name, "description": format!("rollback step{w}x{i}"), "body": "b2"})).await;
                let third = call("skill_save", json!({"name": name, "description": format!("invoice step{w}x{i}"), "body": "b3"})).await;
                let deleted = call("skill_delete", json!({"id": saved["id"]})).await;
                let bad = [("save", &saved), ("save again", &again), ("save a third time", &third)]
                    .into_iter()
                    .filter(|(_, out)| out.get("error").is_some() || out["embedded"] != true)
                    .chain(std::iter::once(("delete", &deleted)).filter(|(_, out)| out.get("error").is_some()))
                    .map(|(what, out)| format!("{what} of {name}: {out}"))
                    .collect::<Vec<_>>();
                if !bad.is_empty() {
                    failures.lock().unwrap().extend(bad);
                    stop.store(true, SeqCst);
                }
                cycles.fetch_add(1, SeqCst);
                i += 1;
            }
        }));
    }
    {
        let (db, owner, stop, bare_conflicts) = (app.db().await, app.user.id.clone(), stop.clone(), bare_conflicts.clone());
        tasks.push(tokio::spawn(async move {
            let mut i = 0;
            let count = |r: Result<(), surrealdb::Error>, what: &str| match r {
                Err(e) if eunomia_backend::tx::is_conflict(&e) => {
                    bare_conflicts.fetch_add(1, SeqCst);
                }
                Err(e) => panic!("bare {what}: {e}"),
                Ok(()) => {}
            };
            while !stop.load(SeqCst) {
                let name = format!("probe-{i}");
                let up: Result<Vec<Value>, _> = async {
                    q::UPSERT.on(&db).bind(("owner", owner.clone())).bind(("vault", vault.clone())).bind(("name", name.clone())).bind(("description", "d")).bind(("body", "b")).await?.take(0)
                }
                .await;
                let id = match up {
                    Ok(rows) => rows.first().and_then(|r| r["id"].as_str()).map(|s| rid::parse(s).unwrap()),
                    Err(e) => {
                        count(Err(e), "upsert");
                        None
                    }
                };
                if let Some(id) = id {
                    let set = async {
                        q::SET_EMBEDDING
                            .on(&db)
                            .bind(("id", id.clone()))
                            .bind(("embedding", concept_vector(&format!("probe{i}"))))
                            .bind(("spec", "probe"))
                            .bind(("vault", vault.clone()))
                            .bind(("name", name.clone()))
                            .bind(("description", "d"))
                            .await?
                            .check()
                            .map(|_| ())
                    };
                    count(set.await, "set embedding");
                    let del = async { q::DELETE.on(&db).bind(("id", id.clone())).await?.check().map(|_| ()) };
                    count(del.await, "delete");
                }
                i += 1;
            }
        }));
    }

    let start = Instant::now();
    while !stop.load(SeqCst) && start.elapsed() < DEADLINE && (bare_conflicts.load(SeqCst) < ENOUGH_BARE_CONFLICTS || cycles.load(SeqCst) < MIN_CYCLES) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    stop.store(true, SeqCst);
    for t in tasks {
        t.await.unwrap();
    }
    let (cycles, bare) = (cycles.load(SeqCst), bare_conflicts.load(SeqCst));
    eprintln!("{cycles} tool cycles, {bare} bare conflicts in {:?}", start.elapsed());
    let failures = failures.lock().unwrap();
    assert!(
        failures.is_empty(),
        "a skill tool write failed while SurrealDB maintained skill_embedding_idx ({cycles} cycles, {bare} bare conflicts): \
         the commit-conflict retry (tx::with_retry) around skill_save's upsert, its embedding write or skill_delete has \
         regressed (an `embedded: false` is a lost embedding write): {failures:#?}"
    );
    if cycles < MIN_CYCLES {
        eprintln!("only {cycles} tool cycles before the deadline: too slow a run to check that the race still fires");
        return;
    }
    assert!(
        bare >= MIN_BARE_CONFLICTS,
        "the bare skill statements met only {bare} commit conflicts in {cycles} cycles: the index maintenance race no \
         longer fires here, so this test no longer guards the retry; re-check skill_embedding_idx and the SurrealDB version"
    );
}

#[tokio::test]
async fn another_user_or_org_cannot_find_get_or_delete_a_skill() {
    let app = TestApp::new().await;
    let saved = app.tool("skill_save", save_args("secret-runbook", "Private steps for the payroll run")).await;
    let id = saved["id"].as_str().unwrap();

    // an org mate who is not a member of the vault, and a user of another org
    let mate = common::register(&app.state, "mate@example.com").await;
    let outsider = common::register_personal(&app.state, "outsider@example.com").await;
    for user in [&mate, &outsider] {
        let found = call_as(&app, user, "skill_find", json!({"query": "payroll"})).await;
        assert!(names(&found).is_empty(), "{found}");
        for (tool, args) in [("skill_get", json!({"id": id})), ("skill_delete", json!({"id": id})), ("skill_get", json!({"name": "secret-runbook"}))] {
            let out = call_as(&app, user, tool, args).await;
            assert!(out.get("error").is_some() && !out.to_string().contains("payroll"), "{tool}: {out}");
        }
    }
    assert_eq!(app.tool("skill_get", json!({"id": id})).await["name"], "secret-runbook", "the delete attempts changed nothing");
}

#[tokio::test]
async fn a_vault_restricted_token_cannot_reach_another_vaults_skill() {
    let app = TestApp::new().await;
    let team = app.tool("vault_create", json!({"name": "Team"})).await;
    let team_id = team["id"].as_str().unwrap().to_string();
    let personal = app.tool("skill_save", save_args("personal-notes", "Only for me")).await;
    let shared = app.tool("skill_save", json!({"name": "team-notes", "description": "For the team", "body": "b", "vault_id": team_id})).await;

    let granted: Vec<String> = scopes::ALL.iter().map(|s| s.to_string()).collect();
    let team_rid = rid::parse(&team_id).unwrap();
    let token = create_api_token_with(app.control(), &app.user.id, "r", &granted, Some(&team_rid), None).await.unwrap().token;

    let found = rest(&app, &token, "skill_find", json!({"query": "notes"})).await;
    assert_eq!(names(&found), ["team-notes"], "{found}");
    assert!(rest(&app, &token, "skill_get", json!({"id": shared["id"]})).await.get("body").is_some());
    for (tool, args) in [
        ("skill_get", json!({"id": personal["id"]})),
        ("skill_delete", json!({"id": personal["id"]})),
        ("skill_save", json!({"name": "x", "description": "d", "body": "b", "vault_id": personal["vault"]})),
        ("skill_find", json!({"query": "notes", "vault_id": personal["vault"]})),
    ] {
        let out = rest(&app, &token, tool, args).await;
        assert!(out.get("error").is_some(), "{tool}: {out}");
    }
    assert_eq!(app.tool("skill_get", json!({"id": personal["id"]})).await["name"], "personal-notes");
}

#[tokio::test]
async fn bad_input_is_a_validation_error_not_a_server_error() {
    let app = TestApp::new().await;
    let long = "x".repeat(1025);
    let huge = "x".repeat(64 * 1024 + 1);
    let cases = [
        json!({"name": "Bad Name", "description": "d", "body": "b"}),
        json!({"name": "-lead", "description": "d", "body": "b"}),
        json!({"name": "a".repeat(65), "description": "d", "body": "b"}),
        json!({"name": "ok", "description": "  ", "body": "b"}),
        json!({"name": "ok", "description": long, "body": "b"}),
        json!({"name": "ok", "description": "d", "body": ""}),
        json!({"name": "ok", "description": "d", "body": huge}),
        json!({"name": "ok", "description": "d", "body": "b", "vault_id": "nope"}),
    ];
    for args in cases {
        let out = app.tool("skill_save", args.clone()).await;
        assert!(out["error"].is_string() && out["code"] != "internal", "{out} for {}", args.to_string().chars().take(80).collect::<String>());
    }
    assert!(app.tool("skill_save", json!({"name": "ok"})).await["error"].as_str().unwrap().starts_with("invalid arguments"));
    assert_eq!(app.tool("skill_get", json!({})).await["code"], "validation.invalid");
    assert_eq!(app.tool("skill_get", json!({"id": "memory:abc"})).await["code"], "validation.invalid");
    assert_eq!(app.tool("skill_find", json!({"query": " "})).await["code"], "validation.invalid");
    // and the edge values are accepted
    let edge = app.tool("skill_save", json!({"name": "a".repeat(64), "description": "d".repeat(1024), "body": "b".repeat(64 * 1024)})).await;
    assert!(edge.get("error").is_none(), "{edge}");
}

#[tokio::test]
async fn find_by_meaning_when_embeddings_are_available() {
    let app = semantic_app().await;
    let saved = app.tool("skill_save", save_args("deploy-rollback", "Undo a release that went wrong")).await;
    assert_eq!(saved["embedded"], true, "{saved}");
    app.tool("skill_save", save_args("invoice-export", "Export billing data for finance")).await;

    // no word in common with the skill: only the vector can find it, and the unrelated skill stays below the floor
    let found = app.tool("skill_find", json!({"query": "revert the ship"})).await;
    assert_eq!(names(&found), ["deploy-rollback"], "{found}");

    // a new description drops the vector in the database itself (the event), whichever path wrote it
    let id = saved["id"].as_str().unwrap();
    let db = app.db().await;
    let has_vector = || async { db.test_raw().query(format!("SELECT VALUE embedding_spec FROM ONLY {id}")).await.unwrap().take::<Option<String>>(0).unwrap().is_some() };
    assert!(has_vector().await);
    db.test_raw().query(format!("UPDATE {id} SET description = 'Pay the quarterly payment'")).await.unwrap().check().unwrap();
    assert!(!has_vector().await, "the old vector is gone with the old description");
    // and saving again embeds the new text
    let again = app.tool("skill_save", save_args("deploy-rollback", "Pay the quarterly payment")).await;
    assert_eq!(again["embedded"], true);
    assert!(has_vector().await);
    let found = app.tool("skill_find", json!({"query": "billing"})).await;
    assert!(names(&found).contains(&"invoice-export".to_string()), "{found}");
}
