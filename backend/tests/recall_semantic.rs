//! Recall over remembered facts by meaning, one bounded relation hop, event time and history (#65),
//! end to end through the registry and a real in-memory SurrealDB. Embeddings come from a stub
//! provider that maps words to concepts, so a paraphrase with no word in common lands near its fact.

mod common;

use common::TestApp;
use eunomia_backend::embeddings::service as embeddings;
use serde_json::{json, Value};
use surrealdb::types::RecordId;

/// Words that mean the same thing share a concept; every other word is its own concept.
const CONCEPTS: &[&[&str]] = &[
    &["compilers", "compiler", "programming", "language", "tooling", "toolchains"],
    &["builds", "writes", "makes", "creates"],
    &["living", "job", "occupation", "work", "profession"],
    &["hiking", "walking", "trails"],
];

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
async fn app() -> TestApp {
    let mut settings = common::test_settings();
    settings.openai_api_key = Some("sk-test".into());
    let app = TestApp::with_settings(settings).await;
    let base = stub_embeddings().await;
    raw(&app, &format!("UPSERT app_settings:`{}` SET owner = {}, openai_base_url = '{base}'", owner_key(&app.user.id), app.user.id.id_string())).await;
    app
}

trait IdString {
    fn id_string(&self) -> String;
}
impl IdString for RecordId {
    fn id_string(&self) -> String {
        eunomia_backend::rid::RecordIdExt::to_string(self)
    }
}

fn owner_key(owner: &RecordId) -> String {
    eunomia_backend::rid::key_string(&owner.key).unwrap()
}

async fn raw(app: &TestApp, sql: &str) {
    app.db().await.test_raw().query(sql).await.unwrap().check().unwrap();
}

async fn embed_backlog(app: &TestApp) {
    let db = app.db().await;
    eunomia_backend::authz::as_system(embeddings::embed_memory_backlog(&db, &app.state.settings, &app.user.id, 100)).await.unwrap();
}

async fn write(app: &TestApp, subject: &str, kind: &str, text: &str, extra: Value) -> Value {
    let mut args = json!({"subject_name": subject, "subject_kind": kind, "text": text});
    args.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
    let out = app.tool("memory_write", args).await;
    assert!(out.get("error").is_none(), "{out}");
    out
}

async fn recall(app: &TestApp, args: Value) -> Vec<Value> {
    let out = app.tool("recall", args).await;
    assert!(out.get("error").is_none(), "{out}");
    out["results"].as_array().cloned().unwrap()
}

fn texts(hits: &[Value]) -> Vec<String> {
    hits.iter().map(|h| h["text"].as_str().unwrap().to_string()).collect()
}

#[tokio::test]
async fn a_paraphrase_finds_a_fact_in_a_shared_vault_by_meaning() {
    let app = app().await;
    let team = app.tool("vault_create", json!({"name": "Team"})).await;
    assert!(team.get("error").is_none(), "{team}");
    write(&app, "Grace Hopper", "person", "Builds compilers for a living", json!({"vault_id": "Team"})).await;
    write(&app, "Grace Hopper", "person", "Likes hiking on weekends", json!({"vault_id": "Team"})).await;
    embed_backlog(&app).await;

    // no word in common with the fact, and no entity name: only the memory semantic arm can find it
    let hits = recall(&app, json!({"query": "who writes programming language tooling?"})).await;
    let found = texts(&hits);
    assert_eq!(found.first().map(String::as_str), Some("Builds compilers for a living"), "{hits:?}");
    assert_eq!(hits[0]["vault_name"], "Team");
    assert!(!found.iter().any(|t| t.contains("hiking")), "an unrelated nearest neighbour is below the similarity floor: {found:?}");

    // another user of the org who is not a member of Team gets nothing from it
    let other = common::register(&app.state, "outsider@example.com").await;
    let out = eunomia_backend::authz::as_system(eunomia_backend::tools::registry::call(
        &app.state,
        &other,
        "recall",
        json!({"query": "who writes programming language tooling?"}),
    ))
    .await
    .unwrap();
    assert!(!out.to_string().contains("compilers"), "{out}");
}

async fn spec(app: &TestApp, id: &str) -> Option<String> {
    let mut res = app.db().await.test_raw().query(format!("SELECT VALUE embedding_spec FROM ONLY {id}")).await.unwrap();
    res.take::<Option<String>>(0).unwrap()
}

#[tokio::test]
async fn editing_a_fact_drops_its_vector_until_it_is_embedded_again() {
    let app = app().await;
    let out = write(&app, "Grace Hopper", "person", "Builds compilers for a living", json!({})).await;
    let id = out["memory"]["id"].as_str().unwrap().to_string();
    embed_backlog(&app).await;
    assert!(spec(&app, &id).await.is_some());
    let edited = app.tool("memory_update", json!({"memory_id": id, "text": "Likes hiking on weekends"})).await;
    assert!(edited.get("error").is_none(), "{edited}");
    assert_eq!(spec(&app, &id).await, None, "the text changed, so the old vector is gone");
    // and the old meaning no longer finds it
    let hits = recall(&app, json!({"query": "programming language tooling"})).await;
    assert!(texts(&hits).is_empty(), "{hits:?}");
    embed_backlog(&app).await;
    let hits = recall(&app, json!({"query": "walking trails"})).await;
    assert_eq!(texts(&hits), ["Likes hiking on weekends"]);
}

#[tokio::test]
async fn naming_two_related_entities_returns_their_relation_and_a_hop_reaches_a_neighbours_facts() {
    let app = app().await;
    let ada = write(&app, "Ada Lovelace", "person", "Ada prefers async updates", json!({})).await;
    let acme = write(&app, "Acme Corp", "organisation", "Acme renewed their contract in March", json!({})).await;
    let other = app.tool("vault_create", json!({"name": "Elsewhere"})).await;
    let far = write(&app, "Babbage", "person", "Babbage designed the difference engine", json!({"vault_id": "Elsewhere"})).await;
    let (ada, acme, far) = (ada["entity"]["id"].as_str().unwrap(), acme["entity"]["id"].as_str().unwrap(), far["entity"]["id"].as_str().unwrap());
    assert!(other.get("error").is_none());
    raw(&app, &format!("RELATE {ada}->relates_to->{acme} SET label = 'works_at'; RELATE {ada}->relates_to->{far} SET label = 'knows'")).await;

    let hits = recall(&app, json!({"query": "How is Ada Lovelace connected to Acme Corp?"})).await;
    let rel = hits.iter().find(|h| h["kind"] == "relation").unwrap_or_else(|| panic!("{hits:?}"));
    assert_eq!(rel["text"], "Ada Lovelace works at Acme Corp");
    assert!(hits.iter().take(3).any(|h| h["kind"] == "relation"), "the edge joining the two named entities ranks near the top: {hits:?}");
    assert_eq!(rel["arms_hit"], 2, "graph and relation arms both count it");

    // one hop from Ada reaches Acme's facts; the edge into another vault is not followed
    let hits = recall(&app, json!({"query": "What about Ada Lovelace?"})).await;
    let found = texts(&hits);
    assert!(found.contains(&"Acme renewed their contract in March".to_string()), "{found:?}");
    assert!(!found.iter().any(|t| t.contains("difference engine") || t.contains("knows")), "{found:?}");
}

#[tokio::test]
async fn a_fact_dated_last_year_is_found_by_last_years_range_not_by_when_it_was_written() {
    let app = app().await;
    write(&app, "Acme Corp", "organisation", "Acme signed the pilot", json!({"occurred_at": "2025-03-14"})).await;
    write(&app, "Acme Corp", "organisation", "Acme asked for SSO", json!({})).await;
    let hits = recall(&app, json!({"query": "Acme", "time_range": ["2025-01-01", "2025-12-31T23:59:59Z"]})).await;
    let pilot = hits.iter().find(|h| h["text"] == "Acme signed the pilot").unwrap_or_else(|| panic!("{hits:?}"));
    assert!(pilot["occurred_at"].as_str().unwrap().starts_with("2025-03-14"), "{pilot}");
    // the temporal arm only dates last year's fact into the range
    let in_range = |h: &Value| h["arms_hit"].as_u64().unwrap() >= 2;
    assert!(in_range(pilot), "{pilot}");
    let sso = hits.iter().find(|h| h["text"] == "Acme asked for SSO");
    assert!(sso.is_none_or(|h| h["score"].as_f64().unwrap() < pilot["score"].as_f64().unwrap()), "{hits:?}");
    let bad = app.tool("memory_write", json!({"subject_name": "Acme Corp", "subject_kind": "organisation", "text": "x", "occurred_at": "last spring"})).await;
    assert!(bad.get("error").is_some(), "{bad}");
}

#[tokio::test]
async fn superseded_facts_come_back_only_when_history_is_asked_for() {
    let app = app().await;
    let old = write(&app, "Ada Lovelace", "person", "Ada works in the Sydney office", json!({})).await;
    write(&app, "Ada Lovelace", "person", "Ada works in the Melbourne office", json!({})).await;
    let old_id = old["memory"]["id"].as_str().unwrap();
    raw(&app, &format!("UPDATE {old_id} SET status = 'superseded'")).await;

    let hits = recall(&app, json!({"query": "Where does Ada Lovelace work?"})).await;
    assert!(!texts(&hits).iter().any(|t| t.contains("Sydney")), "{hits:?}");
    let hits = recall(&app, json!({"query": "Where does Ada Lovelace work?", "include_history": true})).await;
    let sydney = hits.iter().find(|h| h["text"] == "Ada works in the Sydney office").unwrap_or_else(|| panic!("{hits:?}"));
    assert_eq!(sydney["status"], "superseded");
    let melbourne = hits.iter().find(|h| h["text"] == "Ada works in the Melbourne office").unwrap();
    assert!(melbourne.get("status").is_none(), "{melbourne}");
}
