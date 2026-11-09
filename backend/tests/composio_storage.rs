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
        assert!(!storage::select(&db,&app.user.id,&saved,json!({"apps":[]}),json!({}),true).await.unwrap(),"a stale selection re-enabled a disconnect");
        assert_eq!(service::credentials_of(key, &disconnected).unwrap(),json!({}));
    }).await;
    if !mutation.is_empty() {
        assert!(isolation::rewritten() > 0, "mutation did not apply");
    }
    isolation::clear_mutations();
}

#[tokio::test]
async fn selection_during_refresh_preserves_rotated_tokens_and_choices() {
    if std::env::var("COMPOSIO_GUARD_MUTATION").as_deref() == Ok("refresh") {
        isolation::mutate("app.composio_credentials_replace", "AND credentials_encrypted = $previous", "AND credentials_encrypted = $previous AND updated_at = $updated");
    }
    let app = TestApp::new().await;
    let db = app.db().await;
    let key = &app.state.settings.encryption_key;
    let row = storage::load(&db, &app.user.id).await.unwrap();
    let leased = storage::replace(&db, key, &app.user.id, &row,
        &json!({"access_token":"old","refresh_token":"old-refresh","lease_until":i64::MAX})).await.unwrap().unwrap();
    assert!(storage::select(&db, &app.user.id, &leased,
        json!({"apps":[{"toolkit":"googledrive","account":"mine"}]}), json!({"default_vault":"vault:mine"}), true).await.unwrap());
    let rotated = storage::replace(&db, key, &app.user.id, &leased,
        &json!({"access_token":"new","refresh_token":"rotated-refresh"})).await.unwrap()
        .expect("selection discarded a rotated refresh token");
    assert_eq!(service::credentials_of(key, &rotated).unwrap()["refresh_token"], "rotated-refresh");
    assert_eq!(rotated.config["apps"][0]["account"], "mine");
    assert_eq!(rotated.routing["default_vault"], "vault:mine");
    assert!(rotated.enabled);
    assert!(storage::replace(&db, key, &app.user.id, &leased, &json!({"access_token":"stale"})).await.unwrap().is_none());
    isolation::clear_mutations();
}

#[tokio::test]
async fn disconnect_during_slow_document_upload_leaves_no_documents_or_records() {
    use eunomia_backend::{connectors::composio::account, documents};
    use std::sync::Arc;
    use tokio::sync::Notify;
    if std::env::var("COMPOSIO_GUARD_MUTATION").as_deref() == Ok("publish") {
        isolation::mutate("app.composio_active", "enabled = true AND credentials_encrypted = $previous AND credentials_encrypted != ''", "true");
    }
    let app = TestApp::new().await;
    let state = app.state.org(&app.user.org).await.unwrap();
    let owner = &app.user.id;
    sys(async {
        let row = storage::load(&state.db, owner).await.unwrap();
        let row = storage::replace(&state.db, &state.settings.encryption_key, owner, &row, &json!({"access_token":"token"})).await.unwrap().unwrap();
        assert!(storage::select(&state.db, owner, &row, json!({}), json!({}), true).await.unwrap());
        let row = storage::load(&state.db, owner).await.unwrap();
        let publishing = state.composio_publishing.connection(&state.db, owner);
        let staged = Arc::new(Notify::new());
        let resume = Arc::new(Notify::new());
        let publish = async {
            let result = documents::upload_checked(&state, owner, documents::Upload {
                filename: "slow.txt".into(), media_type: Some("text/plain".into()),
                bytes: b"slow upload".to_vec(), replace: None, vault: None,
            }, || async {
                // The object is staged. Hold the upload before its document commit.
                staged.notify_one();
                resume.notified().await;
                let guard = publishing.lock().await;
                storage::ensure_active(&state.db, owner, &row).await?;
                Ok(guard)
            }).await;
            assert!(result.is_err(), "upload published after disconnect");
            // This is the same pre-ingest fence as run_sync uses.
            assert!(storage::ensure_active(&state.db, owner, &row).await.is_err());
        };
        let disconnect = async {
            staged.notified().await;
            tokio::time::timeout(std::time::Duration::from_secs(2), account::disconnect(&state, owner))
                .await.expect("disconnect waited for object storage").unwrap();
            resume.notify_one();
        };
        tokio::time::timeout(std::time::Duration::from_secs(10), async { tokio::join!(publish, disconnect); }).await.unwrap();
        for table in ["document", "composio_file", "cache_record"] {
            let rows: Vec<serde_json::Value> = state.db.test_raw().query(format!("SELECT * FROM {table}")).await.unwrap().take(0).unwrap();
            assert!(rows.is_empty(), "{table} appeared after disconnect");
        }
        let docs = documents::list(&state, owner, None, false, 50, 0).await.unwrap();
        assert!(docs.results.is_empty());
    }).await;
    isolation::clear_mutations();
}
