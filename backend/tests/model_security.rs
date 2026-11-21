//! Model credentials stay bound to their endpoint (ported from main's fcf9a6f): the server key only goes
//! to the server's own base URL, a user URL only gets that user's key, embeddings honour the selected
//! model and are cached per provider and model, and a key that cannot be decrypted is never sent.

mod common;

use common::{bare_state, register, test_settings, TestApp};
use eunomia_backend::config::Settings;
use eunomia_backend::connectors::crypto;
use eunomia_backend::embeddings::service as embeddings;
use eunomia_backend::sources::mock::{route, serve, Mock};
use serde_json::{json, Value};

fn vectors() -> Value {
    json!({"data": [{"index": 0, "embedding": vec![0.01_f32; embeddings::DIM]}]})
}

async fn embedder() -> Mock {
    serve(vec![route("POST", "/embeddings", vectors())]).await
}

async fn embed(app: &TestApp, text: &str) -> eunomia_backend::error::AppResult<Vec<Vec<f32>>> {
    embeddings::embed(&app.db().await, &app.state.settings, &[text.to_string()], &app.user.id).await
}

async fn save(app: &TestApp, body: Value) -> (u16, Value) {
    let (status, v) = app.http("PATCH", "/api/settings", Some(body), true).await;
    (status.as_u16(), v)
}

fn with_server(base: &str, key: Option<&str>) -> Settings {
    Settings { openai_base_url: base.into(), openai_api_key: key.map(String::from), ..test_settings() }
}

#[tokio::test]
async fn the_server_key_is_only_sent_to_the_servers_own_url() {
    let server = embedder().await;
    let other = embedder().await;
    let app = TestApp::with_settings(with_server(&server.base, Some("sk-server"))).await;

    embed(&app, "to the server").await.unwrap();
    assert_eq!(server.requests()[0].header("authorization"), "Bearer sk-server");

    // the user points Settings elsewhere without a key: that endpoint gets no credential at all
    assert_eq!(save(&app, json!({"openai_base_url": other.base})).await.0, 200);
    embed(&app, "to the user's url").await.unwrap();
    assert_eq!(other.requests()[0].header("authorization"), "Bearer not-needed");

    // with their own key, that key (and only that key) goes there
    assert_eq!(save(&app, json!({"openai_api_key": "sk-user"})).await.0, 200);
    embed(&app, "with a user key").await.unwrap();
    assert_eq!(other.requests()[1].header("authorization"), "Bearer sk-user");
    assert!(server.requests().len() == 1, "the server saw nothing more");
}

#[tokio::test]
async fn settings_show_the_effective_base_url_and_a_new_user_follows_the_server() {
    let server = embedder().await;
    let app = TestApp::with_settings(with_server(&server.base, None)).await;
    let (status, s) = app.http("GET", "/api/settings", None, true).await;
    assert_eq!(status, 200);
    assert_eq!(s["openai_base_url"], json!(server.base));
    assert_eq!(s["embedding_model"], json!(embeddings::DEFAULT_MODEL));
}

#[tokio::test]
async fn the_selected_embedding_model_is_sent_and_vectors_are_cached_per_model() {
    let server = embedder().await;
    let app = TestApp::with_settings(with_server(&server.base, None)).await;
    embed(&app, "same text").await.unwrap();
    embed(&app, "same text").await.unwrap(); // cached
    assert_eq!(server.requests().len(), 1);
    assert!(server.requests()[0].body.contains(embeddings::DEFAULT_MODEL), "{}", server.requests()[0].body);

    assert_eq!(save(&app, json!({"embedding_model": "nomic-embed-text"})).await.0, 200);
    embed(&app, "same text").await.unwrap();
    assert_eq!(server.requests().len(), 2, "another model never reuses the first model's vector");
    assert!(server.requests()[1].body.contains("nomic-embed-text"));
}

#[tokio::test]
async fn unsupported_openai_models_are_rejected_on_save() {
    let app = TestApp::with_settings(with_server("https://api.openai.com/v1", Some("sk-server"))).await;
    let (status, body) = save(&app, json!({"embedding_model": "text-embedding-3-large"})).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(save(&app, json!({"embedding_model": "text-embedding-ada-002"})).await.0, 200);
    // another endpoint's models can't be known up front
    let other = embedder().await;
    assert_eq!(save(&app, json!({"openai_base_url": other.base, "embedding_model": "anything-1024"})).await.0, 200);
}

#[tokio::test]
async fn malformed_embedding_responses_are_rejected_and_never_cached() {
    let wrong_dim = serve(vec![route("POST", "/embeddings", json!({"data": [{"index": 0, "embedding": [0.1, 0.2]}]}))]).await;
    let app = TestApp::with_settings(with_server(&wrong_dim.base, None)).await;
    assert!(embed(&app, "x").await.is_err());
    assert!(embed(&app, "x").await.is_err());
    assert_eq!(wrong_dim.requests().len(), 2, "a bad response is not memoized");

    let short = serve(vec![route("POST", "/embeddings", json!({"data": []}))]).await;
    let app = TestApp::with_settings(with_server(&short.base, None)).await;
    assert!(embed(&app, "x").await.is_err());
}

#[tokio::test]
async fn a_key_that_cannot_be_decrypted_is_never_sent() {
    let server = embedder().await;
    let app = TestApp::with_settings(with_server(&server.base, Some("sk-server"))).await;
    // ciphertext written under some other ENCRYPTION_KEY
    let foreign = crypto::encrypt("another-encryption-key-0123456789", "sk-theirs");
    assert_eq!(save(&app, json!({"openai_api_key": "placeholder"})).await.0, 200);
    app.db().await.test_raw().query("UPDATE app_settings SET openai_api_key_encrypted = $k").bind(("k", foreign)).await.unwrap().check().unwrap();

    assert!(embed(&app, "x").await.is_err());
    assert!(!embeddings::available(&app.db().await, &app.state.settings, &app.user.id).await);
    assert!(server.requests().is_empty(), "nothing reached any provider");
    // the models list degrades to a code instead of an error
    let (status, models) = app.http("GET", "/api/settings/openai-models", None, true).await;
    assert_eq!(status, 200);
    assert_eq!(models["error"], json!("credential_unreadable"));
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn short_encryption_keys_are_refused_on_a_fresh_install_but_not_over_existing_data() {
    let state = bare_state().await;
    let short = Settings { encryption_key: "short".into(), ..test_settings() };
    let err = crypto::guard_key(&short, &state.control).await.unwrap_err();
    assert!(err.message.contains("too short"), "{}", err.message);
    register(&state, "owner@example.com").await;
    crypto::guard_key(&short, &state.control).await.unwrap();
}
