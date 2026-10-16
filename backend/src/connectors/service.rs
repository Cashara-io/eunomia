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
use crate::rid::RecordIdExt;
use crate::store;
use crate::tx::with_retry_dup;
use crate::error::{AppError, AppResult, ErrorCode};

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
    /// "" for a kind's first connection (every connection made before 2.0.4), otherwise this
    /// connection's own record key. See `sources::registry::sync_key`.
    #[serde(default)]
    #[surreal(default)]
    pub instance: String,
    /// The name the user gave this connection (an email address, a device name); "" if none.
    #[serde(default)]
    #[surreal(default)]
    pub label: String,
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

impl Connector {
    /// Whether this is its kind's first connection, which also owns the records synced before 2.0.4.
    pub fn is_first(&self) -> bool {
        self.instance.is_empty()
    }
}

/// Longest connection label kept.
const MAX_LABEL_CHARS: usize = 100;

/// `owner`'s connection of `kind` with this `instance` ("" for the kind's first connection).
pub async fn get_connector(db: &OrgDb, owner: &RecordId, kind: &str, instance: &str) -> AppResult<Option<Connector>> {
    let mut res = store::app::CONNECTORS_OF_KIND
        .on(db)
        .bind(("owner", owner.clone()))
        .bind(("kind", kind.to_string()))
        .await?;
    let rows: Vec<Connector> = res.take(0)?;
    Ok(rows.into_iter().find(|c| c.instance == instance))
}

async fn create(db: &OrgDb, owner: &RecordId, kind: &str, instance: &str, label: &str) -> surrealdb::Result<Option<Connector>> {
    let key = if instance.is_empty() { uuid::Uuid::new_v4().simple().to_string() } else { instance.to_string() };
    let mut res = store::app::CONNECTOR_CREATE
        .on(db)
        .bind(("id", RecordId::from_table_key("connector", key)))
        .bind(("owner", owner.clone()))
        .bind(("kind", kind.to_string()))
        .bind(("instance", instance.to_string()))
        .bind(("label", label.to_string()))
        .await?
        .check()?;
    Ok(res.take::<Vec<Connector>>(0)?.into_iter().next())
}

/// `owner`'s first connection of `kind`, created (empty, disabled) if missing.
pub async fn get_or_create_connector(db: &OrgDb, owner: &RecordId, kind: &str) -> AppResult<Connector> {
    // Unique (owner, kind, instance): a racing creator fails the insert and the retry's re-read finds its row.
    let row = with_retry_dup(|| async {
        let mut res = store::app::CONNECTORS_OF_KIND
            .on(db)
            .bind(("owner", owner.clone()))
            .bind(("kind", kind.to_string()))
            .await?
            .check()?;
        if let Some(row) = res.take::<Vec<Connector>>(0)?.into_iter().find(Connector::is_first) {
            return Ok(Some(row));
        }
        create(db, owner, kind, "", "").await
    })
    .await?;
    row.ok_or_else(|| AppError::internal("connector insert returned no row"))
}

/// A further connection of `kind` for `owner` ("Add another"), empty and disabled until set up. Its
/// record key doubles as its `instance`.
pub async fn create_connection(db: &OrgDb, owner: &RecordId, kind: &str, label: &str) -> AppResult<Connector> {
    if !CONNECTOR_KINDS.contains(&kind) {
        return Err(AppError::coded(ErrorCode::ConnectorNotFound, "unknown connector"));
    }
    let label = clean_label(label)?;
    if label.is_empty() {
        return Err(AppError::bad_request("name the new connection, for example the account's email address"));
    }
    let instance = uuid::Uuid::new_v4().simple().to_string();
    create(db, owner, kind, &instance, &label).await?.ok_or_else(|| AppError::internal("connector insert returned no row"))
}

fn clean_label(label: &str) -> AppResult<String> {
    let label = label.trim();
    if label.chars().count() > MAX_LABEL_CHARS {
        return Err(AppError::bad_request(format!("a connection name is at most {MAX_LABEL_CHARS} characters")));
    }
    Ok(label.to_string())
}

/// The connection `r` names for `owner`: a kind (its first connection, created if missing) or a
/// connector id (`connector:abc` or `abc`). Another user's connection is not found.
pub async fn resolve(db: &OrgDb, owner: &RecordId, r: &str) -> AppResult<Connector> {
    if CONNECTOR_KINDS.contains(&r) {
        return get_or_create_connector(db, owner, r).await;
    }
    let id = crate::rid::parse(r)
        .ok()
        .filter(|id| id.table() == "connector")
        .unwrap_or_else(|| RecordId::from_table_key("connector", r));
    let row: Option<Connector> = store::get(db, &id).await?;
    row.filter(|c| &c.owner == owner && CONNECTOR_KINDS.contains(&c.kind.as_str()))
        .ok_or_else(|| AppError::coded(ErrorCode::ConnectorNotFound, "unknown connector"))
}

/// Every connection of `owner`: each kind's first connection (created if missing), then the ones added
/// after it, in [`CONNECTOR_KINDS`] order.
pub async fn list_connectors(db: &OrgDb, owner: &RecordId) -> AppResult<Vec<Connector>> {
    let mut res = store::app::CONNECTORS_OF_OWNER.on(db).bind(("owner", owner.clone())).await?;
    let mut rows: Vec<Connector> = res.take(0)?;
    rows.retain(|c| CONNECTOR_KINDS.contains(&c.kind.as_str()));
    for kind in CONNECTOR_KINDS {
        if !rows.iter().any(|c| c.kind == *kind && c.is_first()) {
            rows.push(get_or_create_connector(db, owner, kind).await?);
        }
    }
    let order = |c: &Connector| CONNECTOR_KINDS.iter().position(|k| *k == c.kind).unwrap_or(usize::MAX);
    rows.sort_by(|a, b| (order(a), !a.is_first(), &a.label, &a.instance).cmp(&(order(b), !b.is_first(), &b.label, &b.instance)));
    Ok(rows)
}

/// Partial update of `owner`'s first connection of `kind` ([`update_connector`]).
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
    update_connector(db, encryption_key, row, enabled, config, credentials, None).await
}

/// Partial update of one connection's config, credentials and label. Credentials are merged (not
/// replaced) with whatever is already on file, so saving one refreshed secret does not drop the others.
pub async fn update_connector(
    db: &OrgDb,
    encryption_key: &str,
    row: Connector,
    enabled: Option<bool>,
    config: Option<Value>,
    credentials: Option<Value>,
    label: Option<String>,
) -> AppResult<Connector> {
    let merged_config = match config {
        Some(patch) => Some(merge_objects(&row.config, &patch)),
        None => None,
    };

    let merged_credentials_encrypted = match credentials {
        Some(patch) => {
            let existing = credentials_of(encryption_key, &row)?;
            let merged = merge_objects(&existing, &patch);
            let serialized = serde_json::to_string(&merged).map_err(|e| AppError::internal(e.to_string()))?;
            Some(crypto::encrypt(encryption_key, &serialized))
        }
        None => None,
    };
    let label = label.as_deref().map(clean_label).transpose()?;

    if enabled.is_none() && merged_config.is_none() && merged_credentials_encrypted.is_none() && label.is_none() {
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
    if label.is_some() {
        set_clauses.push("label = $label");
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
    if let Some(v) = label {
        q = q.bind(("label", v));
    }
    let mut res = q.await?;
    let rows: Vec<Connector> = res.take(0)?;
    rows.into_iter().next().ok_or_else(|| AppError::internal("connector update returned no row"))
}

/// One connection's decrypted credentials (an empty object when none are saved).
pub fn credentials_of(encryption_key: &str, row: &Connector) -> AppResult<Value> {
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
