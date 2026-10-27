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

    // secrets in the description and body are redacted and reported together
    let args = json!({"name": "db-login", "description": "log in to postgres://app:hunter2@db", "body": "export KEY=AKIAIOSFODNN7EXAMPLE"});
    let saved = app.tool("skill_save", args).await;
    assert_eq!(saved["redacted"], json!([{"kind": "url password", "count": 1}, {"kind": "aws access key", "count": 1}]), "{saved}");
    let got = app.tool("skill_get", json!({"name": "db-login"})).await;
    assert_eq!(got["body"], "export KEY=[redacted: aws access key]");
    assert_eq!(got["description"], "log in to postgres://app:[redacted: url password]@db");
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
