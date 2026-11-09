//! User-owned OAuth state. Conditional encrypted writes prevent stale refreshes restoring a disconnect.

use super::{
    oauth::{Metadata, OAuthClient, Pending},
    storage,
};
use crate::{
    connectors::service,
    error::{AppError, AppResult},
    state::OrgState,
};
use serde_json::json;
use surrealdb::types::RecordId;
use zeroize::Zeroizing;

fn retry() -> AppError {
    AppError::bad_request("Composio sign-in changed. Please try again.")
}

pub async fn begin(
    state: &OrgState,
    owner: &RecordId,
    redirect: &str,
    binding: &str,
) -> AppResult<String> {
    let publishing = state.composio_publishing.connection(&state.db, owner);
    let generation = publishing.generation();
    let row = storage::load(&state.db, owner).await?;
    let (pending, link) = OAuthClient::consumer()?.begin(redirect).await?;
    let mut credentials = crate::sources::base::Conn {
        credentials: service::credentials_of(&state.settings.encryption_key, &row)?,
        config: json!({}),
    };
    credentials.credentials["pending"] = serde_json::to_value(&pending).map_err(|_| retry())?;
    credentials.credentials["binding"] = json!(binding);
    let _commit = publishing.lock().await;
    publishing.check(generation)?;
    storage::replace(
        &state.db,
        &state.settings.encryption_key,
        owner,
        &row,
        &credentials.credentials,
    )
    .await?
    .ok_or_else(retry)?;
    Ok(link)
}

pub async fn finish(
    state: &OrgState,
    owner: &RecordId,
    binding: &str,
    oauth_state: &str,
    code: &str,
) -> AppResult<()> {
    let row = storage::load(&state.db, owner).await?;
    let mut credentials = crate::sources::base::Conn {
        credentials: service::credentials_of(&state.settings.encryption_key, &row)?,
        config: json!({}),
    };
    if credentials.credentials["binding"].as_str() != Some(binding) || binding.is_empty() {
        return Err(retry());
    }
    let pending: Pending =
        serde_json::from_value(credentials.credentials["pending"].clone()).map_err(|_| retry())?;
    if pending.state != oauth_state || pending.expires_at <= chrono::Utc::now().timestamp() {
        return Err(retry());
    }
    // Consume state before contacting the token endpoint. Replayed callbacks lose the CAS.
    credentials
        .credentials
        .as_object_mut()
        .ok_or_else(retry)?
        .remove("pending");
    credentials
        .credentials
        .as_object_mut()
        .ok_or_else(retry)?
        .remove("binding");
    let consumed = storage::replace(
        &state.db,
        &state.settings.encryption_key,
        owner,
        &row,
        &credentials.credentials,
    )
    .await?
    .ok_or_else(retry)?;
    let tokens = OAuthClient::consumer()?
        .exchange(&pending, oauth_state, code)
        .await?;
    let account = OAuthClient::consumer()?
        .identity(&tokens.access_token)
        .await?;
    credentials.credentials = json!({
        "access_token":tokens.access_token, "refresh_token":tokens.refresh_token,
        "expires_at":chrono::Utc::now().timestamp() + i64::try_from(tokens.expires_in.min(86400)).unwrap_or(0),
        "metadata":pending.metadata, "client_id":pending.client_id, "account":account,
    });
    storage::replace(
        &state.db,
        &state.settings.encryption_key,
        owner,
        &consumed,
        &credentials.credentials,
    )
    .await?
    .ok_or_else(retry)?;
    Ok(())
}

/// A short distributed lease is itself encrypted and acquired by CAS. Every lease release must match
/// the ciphertext acquired, so a disconnect or a newer sign-in wins over an in-flight refresh.
pub async fn access_token(state: &OrgState, owner: &RecordId) -> AppResult<Zeroizing<String>> {
    for _ in 0..100 {
        let row = storage::load(&state.db, owner).await?;
        let mut credentials = crate::sources::base::Conn {
            credentials: service::credentials_of(&state.settings.encryption_key, &row)?,
            config: json!({}),
        };
        let now = chrono::Utc::now().timestamp();
        if credentials.credentials["expires_at"].as_i64().unwrap_or(0) > now + 60 {
            return credentials.credentials["access_token"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(|s| Zeroizing::new(s.to_string()))
                .ok_or_else(retry);
        }
        if credentials.credentials["lease_until"].as_i64().unwrap_or(0) > now {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            continue;
        }
        let metadata: Metadata =
            serde_json::from_value(credentials.credentials["metadata"].clone())
                .map_err(|_| retry())?;
        let refresh = Zeroizing::new(
            credentials.credentials["refresh_token"]
                .as_str()
                .ok_or_else(retry)?
                .to_string(),
        );
        let client_id = credentials.credentials["client_id"]
            .as_str()
            .ok_or_else(retry)?
            .to_string();
        credentials.credentials["lease_until"] = json!(now + 120);
        let Some(leased) = storage::replace(
            &state.db,
            &state.settings.encryption_key,
            owner,
            &row,
            &credentials.credentials,
        )
        .await?
        else {
            continue;
        };
        let refreshed = OAuthClient::consumer()?
            .refresh(&metadata, &client_id, &refresh)
            .await;
        credentials
            .credentials
            .as_object_mut()
            .ok_or_else(retry)?
            .remove("lease_until");
        match refreshed {
            Ok(tokens) => {
                credentials.credentials["access_token"] = json!(tokens.access_token);
                if let Some(refresh) = &tokens.refresh_token {
                    credentials.credentials["refresh_token"] = json!(refresh);
                }
                credentials.credentials["expires_at"] =
                    json!(now + i64::try_from(tokens.expires_in.min(86400)).unwrap_or(0));
                storage::replace(
                    &state.db,
                    &state.settings.encryption_key,
                    owner,
                    &leased,
                    &credentials.credentials,
                )
                .await?
                .ok_or_else(retry)?;
                return Ok(Zeroizing::new(tokens.access_token.clone()));
            }
            Err(error) => {
                storage::replace(
                    &state.db,
                    &state.settings.encryption_key,
                    owner,
                    &leased,
                    &credentials.credentials,
                )
                .await?;
                return Err(error);
            }
        }
    }
    Err(AppError::bad_request(
        "Composio sign-in is being refreshed. Please try again shortly.",
    ))
}

pub async fn disconnect(state: &OrgState, owner: &RecordId) -> AppResult<bool> {
    let publishing = state.composio_publishing.connection(&state.db, owner);
    publishing.invalidate();
    let row = storage::load(&state.db, owner).await?;
    // Delete locally even when the provider is unavailable. A leased refresh cannot restore this state.
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        storage::delete_tokens(&state.db, owner, &row.id),
    ).await.map_err(|_| AppError::internal("Composio disconnect timed out. Please retry."))??;
    // Only local commits can hold this gate. Never wait for provider or object-storage I/O.
    let drained = tokio::time::timeout(std::time::Duration::from_secs(1), publishing.lock())
        .await.map_err(|_| AppError::internal("Composio commits are stopping. Please retry disconnect."))?;
    drop(drained);
    let Ok(credentials) = service::credentials_of(&state.settings.encryption_key, &row) else {
        return Ok(false);
    };
    let credentials = crate::sources::base::Conn { credentials, config: json!({}) };
    let Some(raw) = credentials.credentials.get("metadata") else {
        return Ok(false);
    };
    let Ok(metadata) = serde_json::from_value::<Metadata>(raw.clone()) else {
        return Ok(false);
    };
    let token = credentials.credentials["refresh_token"]
        .as_str()
        .or_else(|| credentials.credentials["access_token"].as_str())
        .unwrap_or("");
    let Ok(client) = OAuthClient::consumer() else {
        return Ok(false);
    };
    Ok(tokio::time::timeout(std::time::Duration::from_secs(1), client.revoke(
        &metadata,
        credentials.credentials["client_id"].as_str().unwrap_or(""),
        token,
    )).await.ok().and_then(Result::ok).unwrap_or(false))
}
