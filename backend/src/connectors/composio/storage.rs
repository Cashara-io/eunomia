//! Owner-scoped compare-and-swap for encrypted Composio state. No token enters SQL text.

use crate::connectors::{
    crypto,
    service::{self, Connector},
};
use crate::error::{AppError, AppResult};
use crate::pool::OrgDb;
use crate::store;
use serde_json::Value;
use surrealdb::types::RecordId;
use zeroize::Zeroizing;

pub async fn load(db: &OrgDb, owner: &RecordId) -> AppResult<Connector> {
    service::get_or_create_connector(db, owner, "composio").await
}

/// Returns None on a concurrent update or disconnect. Never re-creates a deleted connection.
pub async fn replace(
    db: &OrgDb,
    key: &str,
    owner: &RecordId,
    row: &Connector,
    credentials: &Value,
) -> AppResult<Option<Connector>> {
    let plaintext = Zeroizing::new(
        serde_json::to_string(credentials)
            .map_err(|_| AppError::internal("Could not save Composio sign-in."))?,
    );
    let encrypted = crypto::encrypt(key, &plaintext);
    let mut result = store::dynamic(db, "app.composio_credentials_replace",
        "UPDATE $id SET credentials_encrypted = $encrypted, updated_at = time::now() WHERE owner = $owner AND kind = 'composio' AND credentials_encrypted = $previous AND updated_at = $updated RETURN AFTER")
        .bind(("id",row.id.clone())).bind(("owner",owner.clone())).bind(("previous",row.credentials_encrypted.clone()))
        .bind(("updated",row.updated_at)).bind(("encrypted",encrypted)).await?.check()?;
    Ok(result.take::<Vec<Connector>>(0)?.into_iter().next())
}

pub async fn delete_tokens(db: &OrgDb, owner: &RecordId, id: &RecordId) -> AppResult<()> {
    store::dynamic(db, "app.composio_disconnect",
        "UPDATE $id SET credentials_encrypted = '', enabled = false, updated_at = time::now() WHERE owner = $owner AND kind = 'composio'")
        .bind(("id",id.clone())).bind(("owner",owner.clone())).await?.check()?;
    Ok(())
}

/// App choices and vault routing change together, only for the still-connected generation.
pub async fn select(
    db: &OrgDb,
    owner: &RecordId,
    row: &Connector,
    config: Value,
    routing: Value,
    enabled: bool,
) -> AppResult<bool> {
    if row.credentials_encrypted.is_empty() {
        return Ok(false);
    }
    let mut res=store::dynamic(db,"app.composio_selection",
        "UPDATE $id SET config = $config, routing = $routing, enabled = $enabled, updated_at = time::now() WHERE owner = $owner AND kind = 'composio' AND credentials_encrypted = $previous AND updated_at = $updated RETURN AFTER")
        .bind(("id",row.id.clone())).bind(("owner",owner.clone())).bind(("previous",row.credentials_encrypted.clone())).bind(("updated",row.updated_at))
        .bind(("config",config)).bind(("routing",routing)).bind(("enabled",enabled)).await?.check()?;
    Ok(!res.take::<Vec<Connector>>(0)?.is_empty())
}
