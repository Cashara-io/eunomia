//! Credential-boundary proof. Guard mutations run this same test and must make it fail.
mod common;

use common::{TestApp, sys};
use eunomia_backend::{
    connectors::{composio::storage, service},
    isolation, rid,
};
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn composio_credentials_are_owner_isolated_encrypted_and_deleted_on_disconnect() {
    let mutation = std::env::var("COMPOSIO_GUARD_MUTATION").unwrap_or_default();
    match mutation.as_str() {
        "owner" => isolation::mutate("app.composio_credentials_replace", "owner = $owner", "true"),
        "disconnect" => isolation::mutate(
            "app.composio_disconnect",
            "credentials_encrypted = ''",
            "credentials_encrypted = credentials_encrypted",
        ),
        "" => {}
        _ => panic!("Unknown mutation"),
    }
    let app = TestApp::new().await;
    sys(async {
        let db = app.db().await;
        let key = &app.state.settings.encryption_key;
        let original = storage::load(&db, &app.user.id).await.unwrap();
        let credentials = json!({"access_token":"composio-canary-token","refresh_token":"composio-canary-refresh"});
        let saved = storage::replace(&db, key, &app.user.id, &original, &credentials).await.unwrap().unwrap();
        assert!(saved.credentials_encrypted.starts_with("enc:v1:"));
        assert!(!saved.credentials_encrypted.contains("canary"));
        assert_eq!(service::credentials_of(key, &saved).unwrap(), credentials);
        let other = rid::parse("user:other-composio-user").unwrap();
        let replaced = storage::replace(&db, key, &other, &saved, &json!({"access_token":"other-token"})).await.unwrap();
        assert!(replaced.is_none(), "another user changed the credentials");
        // A stale writer cannot replace a newer token after refresh rotation.
        assert!(storage::replace(&db, key, &app.user.id, &original, &credentials).await.unwrap().is_none());
        storage::delete_tokens(&db, &other, &saved.id).await.unwrap();
        assert_eq!(service::credentials_of(key, &storage::load(&db, &app.user.id).await.unwrap()).unwrap(),credentials);
        storage::delete_tokens(&db, &app.user.id, &saved.id).await.unwrap();
        let disconnected = storage::load(&db, &app.user.id).await.unwrap();
        assert!(disconnected.credentials_encrypted.is_empty(), "disconnect retained credentials");
        assert!(!disconnected.enabled);
        assert_eq!(service::credentials_of(key, &disconnected).unwrap(),json!({}));
    }).await;
    if !mutation.is_empty() {
        assert!(isolation::rewritten() > 0, "mutation did not apply");
    }
    isolation::clear_mutations();
}
