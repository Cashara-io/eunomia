//! Connector CRUD + enable/disable logic (the connector rows only; app settings live in
//! `routers::settings`).
//!
//! Every function takes an explicit `owner` (the user's `RecordId`) and scopes
//! its SurrealDB query to that owner -- connectors are per-user, not global
//! singletons.

use surrealdb::types::SurrealValue;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use surrealdb::types::{Datetime, RecordId};

use crate::connectors::crypto;
use crate::pool::OrgDb;
use crate::store;
use crate::tx::with_retry_dup;
use crate::error::{AppError, AppResult};

/// Connector kinds the CRUD surface manages -- each one backed by a source
/// in `sources::registry` that really syncs it (asserted by the registry's
/// tests). "demo" and "open_connector" remain valid in the schema for old
/// rows but are no longer offered.
pub const CONNECTOR_KINDS: &[&str] = &[
    "up_bank",
    "pocketai",
    "github",
    "slack",
    "notion",
    "linear",
    "gmail",
    "google_calendar",
    "discord",
    "spotify",
    "todoist",
    "stripe",
];

#[derive(Debug, Clone, Deserialize, SurrealValue, Serialize)]
pub struct Connector {
    pub id: RecordId,
    pub owner: RecordId,
    pub kind: String,
    #[serde(default)]
    #[surreal(default)]
    pub enabled: bool,
    #[serde(default)]
    #[surreal(default)]
    pub config: Value,
    #[serde(default)]
    #[surreal(default)]
    pub credentials_encrypted: String,
    pub updated_at: Datetime,
    /// Where its syncs file records (`sources::routing::Routing`).
    #[serde(default)]
    #[surreal(default)]
    pub routing: Value,
}

pub async fn get_connector(db: &OrgDb, owner: &RecordId, kind: &str) -> AppResult<Option<Connector>> {
    let mut res = store::app::CONNECTOR_BY_KIND
        .on(db)
        .bind(("owner", owner.clone()))
        .bind(("kind", kind.to_string()))
        .await?;
    let rows: Vec<Connector> = res.take(0)?;
    Ok(rows.into_iter().next())
}

pub async fn get_or_create_connector(db: &OrgDb, owner: &RecordId, kind: &str) -> AppResult<Connector> {
    // Unique (owner, kind): a racing creator fails the insert and the retry's re-read finds its row.
    let row = with_retry_dup(|| async {
        let mut res = store::app::CONNECTOR_BY_KIND
            .on(db)
            .bind(("owner", owner.clone()))
            .bind(("kind", kind.to_string()))
            .await?
            .check()?;
        if let Some(row) = res.take::<Vec<Connector>>(0)?.into_iter().next() {
            return Ok(Some(row));
        }
        let mut res = store::app::CONNECTOR_CREATE
            .on(db)
            .bind(("owner", owner.clone()))
            .bind(("kind", kind.to_string()))
            .await?
            .check()?;
        Ok(res.take::<Vec<Connector>>(0)?.into_iter().next())
    })
    .await?;
    row.ok_or_else(|| AppError::internal("connector insert returned no row"))
}

pub async fn list_connectors(db: &OrgDb, owner: &RecordId) -> AppResult<Vec<Connector>> {
    let mut out = Vec::with_capacity(CONNECTOR_KINDS.len());
    for kind in CONNECTOR_KINDS {
        out.push(get_or_create_connector(db, owner, kind).await?);
    }
    Ok(out)
}

/// Partial update of a connector's config/credentials, scoped to `owner`.
/// Credentials are merged (not replaced) with whatever is already on file,
/// so saving one refreshed secret does not drop the others.
pub async fn upsert_connector(
    db: &OrgDb,
    encryption_key: &str,
    owner: &RecordId,
    kind: &str,
    enabled: Option<bool>,
    config: Option<Value>,
    credentials: Option<Value>,
) -> AppResult<Connector> {
    let row = get_or_create_connector(db, owner, kind).await?;

    let merged_config = match config {
        Some(patch) => Some(merge_objects(&row.config, &patch)),
        None => None,
    };

    let merged_credentials_encrypted = match credentials {
        Some(patch) => {
            let existing_raw = crypto::decrypt(encryption_key, &row.credentials_encrypted)?;
            let existing: Value = if existing_raw.is_empty() {
                Value::Object(Default::default())
            } else {
                serde_json::from_str(&existing_raw).map_err(|e| AppError::internal(e.to_string()))?
            };
            let merged = merge_objects(&existing, &patch);
            let serialized = serde_json::to_string(&merged).map_err(|e| AppError::internal(e.to_string()))?;
            Some(crypto::encrypt(encryption_key, &serialized))
        }
        None => None,
    };

    if enabled.is_none() && merged_config.is_none() && merged_credentials_encrypted.is_none() {
        return Ok(row);
    }

    let mut set_clauses = Vec::new();
    if enabled.is_some() {
        set_clauses.push("enabled = $enabled");
    }
    if merged_config.is_some() {
        set_clauses.push("config = $config");
    }
    if merged_credentials_encrypted.is_some() {
        set_clauses.push("credentials_encrypted = $credentials_encrypted");
    }
    set_clauses.push("updated_at = time::now()");
    let query = format!("UPDATE $id SET {} RETURN AFTER", set_clauses.join(", "));

    // dynamic: the SET clause list depends on which fields are present
    let mut q = store::dynamic(db, "app.connector_update", query).bind(("id", row.id.clone()));
    if let Some(v) = enabled {
        q = q.bind(("enabled", v));
    }
    if let Some(v) = merged_config {
        q = q.bind(("config", v));
    }
    if let Some(v) = merged_credentials_encrypted {
        q = q.bind(("credentials_encrypted", v));
    }
    let mut res = q.await?;
    let rows: Vec<Connector> = res.take(0)?;
    rows.into_iter().next().ok_or_else(|| AppError::internal("connector update returned no row"))
}

pub async fn credentials_for(db: &OrgDb, encryption_key: &str, owner: &RecordId, kind: &str) -> AppResult<Value> {
    let Some(row) = get_connector(db, owner, kind).await? else {
        return Ok(Value::Object(Default::default()));
    };
    let raw = crypto::decrypt(encryption_key, &row.credentials_encrypted)?;
    if raw.is_empty() {
        Ok(Value::Object(Default::default()))
    } else {
        serde_json::from_str(&raw).map_err(|e| AppError::internal(e.to_string()))
    }
}

/// Shallow merge of two JSON objects, `patch` winning on key conflicts.
/// Non-object inputs are treated as empty objects.
fn merge_objects(base: &Value, patch: &Value) -> Value {
    let mut merged = base.as_object().cloned().unwrap_or_default();
    if let Some(patch_obj) = patch.as_object() {
        for (k, v) in patch_obj {
            merged.insert(k.clone(), v.clone());
        }
    }
    Value::Object(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_objects_patch_wins_on_conflict() {
        let base = json!({"a": 1, "b": 2});
        let patch = json!({"b": 20, "c": 3});
        let merged = merge_objects(&base, &patch);
        assert_eq!(merged, json!({"a": 1, "b": 20, "c": 3}));
    }

    #[test]
    fn merge_objects_treats_missing_base_as_empty() {
        let base = Value::Null;
        let patch = json!({"a": 1});
        let merged = merge_objects(&base, &patch);
        assert_eq!(merged, json!({"a": 1}));
    }

    #[test]
    fn connector_kinds_excludes_demo() {
        assert!(!CONNECTOR_KINDS.contains(&"demo"));
        assert!(!CONNECTOR_KINDS.contains(&"open_connector"));
        assert_eq!(CONNECTOR_KINDS.len(), 12);
        for k in ["up_bank", "pocketai", "github", "slack", "notion", "linear", "gmail", "google_calendar", "discord", "spotify", "todoist", "stripe"] {
            assert!(CONNECTOR_KINDS.contains(&k), "missing {k}");
        }
    }
}
