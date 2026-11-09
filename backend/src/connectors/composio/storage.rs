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
        "UPDATE $id SET credentials_encrypted = $encrypted, updated_at = time::now() WHERE owner = $owner AND kind = 'composio' AND credentials_encrypted = $previous RETURN AFTER")
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

/// Each connection fences local commits. Provider calls must run outside the gate.
/// Weak entries are pruned so disconnected users do not accumulate locks.
#[derive(Default)]
pub struct Publishing(std::sync::Mutex<ConnectionLocks>);

type ConnectionLocks = std::collections::HashMap<(crate::pool::OrgId, RecordId), std::sync::Weak<Connection>>;

impl Publishing {
    pub fn connection(&self, db: &OrgDb, owner: &RecordId) -> std::sync::Arc<Connection> {
        let mut locks = self.0.lock().unwrap();
        locks.retain(|_, lock| lock.strong_count() > 0);
        let key = (db.org(), owner.clone());
        if let Some(lock) = locks.get(&key).and_then(std::sync::Weak::upgrade) { return lock; }
        let lock = std::sync::Arc::new(Connection::default());
        locks.insert(key, std::sync::Arc::downgrade(&lock));
        lock
    }
}

pub async fn ensure_active(db: &OrgDb, owner: &RecordId, row: &Connector) -> AppResult<()> {
    let mut result = store::dynamic(db, "app.composio_active", "SELECT VALUE id FROM $id WHERE owner = $owner AND enabled = true AND credentials_encrypted = $previous AND credentials_encrypted != ''")
        .bind(("id", row.id.clone())).bind(("owner", owner.clone()))
        .bind(("previous", row.credentials_encrypted.clone())).await?;
    if result.take::<Vec<RecordId>>(0)?.is_empty() {
        return Err(AppError::bad_request("Composio disconnected or changed during sync. Reconnect Composio or retry."));
    }
    Ok(())
}

#[derive(Default)]
pub struct Connection {
    gate: tokio::sync::Mutex<()>,
    generation: std::sync::atomic::AtomicU64,
}

impl Connection {
    pub async fn lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.gate.lock().await
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn invalidate(&self) {
        self.generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn check(&self, generation: u64) -> AppResult<()> {
        if self.generation() != generation {
            return Err(AppError::bad_request("Composio disconnected during this request. Please retry."));
        }
        Ok(())
    }
}
