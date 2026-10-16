//! Data export: a single JSON download of the caller's personal vault: entities of every kind with
//! their memories and relations, plus their chat history. The document's `scope` key says what is in
//! and what is left out (other vaults, cache records, connectors, settings, audit log, uploaded documents). It is built in
//! memory with one query per table.

use surrealdb::types::SurrealValue;
use std::collections::HashMap;

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use surrealdb::types::{Datetime, RecordId};
use crate::rid::RecordIdExt;

use crate::pool::{ControlDb, OrgDb};
use crate::store;
use crate::error::{AppError, AppResult};
use crate::models_user::User;
use crate::state::AppState;

/// Every entity table: the built-in kinds' own, then the one custom kinds share.
use crate::entities::service::ENTITY_TABLES as KINDS;

pub fn router() -> Router<AppState> {
    Router::new().route("/export", get(export_data))
}

#[derive(Debug, Deserialize, SurrealValue)]
struct EntityRow {
    id: RecordId,
    /// set on a custom-kind row only; a built-in kind is its table
    #[serde(default)]
    #[surreal(default)]
    kind: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    name: String,
    #[serde(default)]
    #[surreal(default)]
    aliases: Vec<String>,
    #[serde(default)]
    #[surreal(default)]
    summary: String,
    owner: Option<RecordId>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct MemoryRow {
    id: RecordId,
    subject: RecordId,
    #[serde(default)]
    #[surreal(default)]
    text: String,
    owner: Option<RecordId>,
    #[serde(default, rename = "type")]
    #[surreal(default, rename = "type")]
    kind: String,
    #[serde(default)]
    #[surreal(default)]
    proof_count: i64,
    status: Option<String>,
    created_at: Option<Datetime>,
    updated_at: Option<Datetime>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct RelationRow {
    id: RecordId,
    #[serde(rename = "in")]
    #[surreal(rename = "in")]
    in_: RecordId,
    out: RecordId,
    #[serde(default)]
    #[surreal(default)]
    label: String,
    owner: Option<RecordId>,
    created_at: Option<Datetime>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct ChatMessageRow {
    id: RecordId,
    thread_id: RecordId,
    role: String,
    content: String,
    #[serde(default)]
    #[surreal(default)]
    tool_calls: Option<Vec<Value>>,
    #[serde(default)]
    #[surreal(default)]
    tool_call_id: Option<String>,
    created_at: Option<Datetime>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct UserEmailRow {
    id: RecordId,
    email: String,
}

async fn fetch_emails(db: &ControlDb, ids: &[RecordId]) -> AppResult<HashMap<String, String>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut dedup: Vec<RecordId> = Vec::new();
    for id in ids {
        if !dedup.contains(id) {
            dedup.push(id.clone());
        }
    }
    let mut res = store::control::EXPORT_USER_EMAILS
        .on(db)
        .bind(("ids", dedup))
        .await?;
    let rows: Vec<UserEmailRow> = res.take(0)?;
    Ok(rows.into_iter().map(|r| (r.id.to_string(), r.email)).collect())
}

fn datetime_str(d: &Option<Datetime>) -> Value {
    json!(d)
}

// open body: a downloadable dump (Content-Disposition attachment), not consumed through the typed client
#[utoipa::path(
    operation_id = "exportData",
    get,
    path = "/api/export",
    tag = "export",
    summary = "Download the personal vault as JSON",
    responses((status = 200, body = Object), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn export_data(State(state): State<AppState>, user: User) -> AppResult<Response> {
    let state = state.org(&user.org).await?;
    let document = build_export(&state.db, &state.control, &user).await?;
    let body = serde_json::to_string_pretty(&document).map_err(|e| AppError::internal(e.to_string()))?;

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json".to_string()),
            (header::CONTENT_DISPOSITION, "attachment; filename=\"eunomia-export.json\"".to_string()),
        ],
        body,
    )
        .into_response())
}

/// The export document for `user`'s personal vault. Shared with `eunomia replay`.
pub async fn build_export(db: &OrgDb, control: &ControlDb, user: &User) -> AppResult<Value> {
    let vault = crate::vaults::service::personal_vault_id(db, &user.id).await?;

    // One query per table, not per entity: the entities of every kind, then the memories and the
    // relations (both directions) of all of them at once, grouped back onto their entity.
    struct Bundle {
        kind: String,
        row: EntityRow,
        memories: Vec<MemoryRow>,
        outgoing: Vec<RelationRow>,
        incoming: Vec<RelationRow>,
    }

    let mut bundles: Vec<Bundle> = Vec::new();
    for kind in KINDS {
        let query = format!("SELECT * FROM {kind} WHERE vault = $vault ORDER BY name");
        // dynamic: the table name is the entity kind
        let rows: Vec<EntityRow> = match store::dynamic(db, "app.export_entities", query).bind(("vault", vault.clone())).await {
            Err(e) if crate::entities::service::no_custom_table(&e) => Vec::new(),
            res => res?.take(0)?,
        };
        bundles.extend(rows.into_iter().map(|row| Bundle { kind: row.kind.clone().unwrap_or_else(|| kind.to_string()), row, memories: Vec::new(), outgoing: Vec::new(), incoming: Vec::new() }));
    }

    let ids: Vec<RecordId> = bundles.iter().map(|b| b.row.id.clone()).collect();
    let at: HashMap<String, usize> = ids.iter().enumerate().map(|(i, id)| (id.to_string(), i)).collect();
    if !ids.is_empty() {
        let memories: Vec<MemoryRow> = store::app::EXPORT_MEMORIES.on(db).bind(("ids", ids.clone())).await?.take(0)?;
        for m in memories {
            if let Some(i) = at.get(&m.subject.to_string()) {
                bundles[*i].memories.push(m);
            }
        }
        let outgoing: Vec<RelationRow> = store::app::EXPORT_RELATIONS_OUT.on(db).bind(("ids", ids.clone())).await?.take(0)?;
        for r in outgoing {
            if let Some(i) = at.get(&r.in_.to_string()) {
                bundles[*i].outgoing.push(r);
            }
        }
        let incoming: Vec<RelationRow> = store::app::EXPORT_RELATIONS_IN.on(db).bind(("ids", ids)).await?.take(0)?;
        for r in incoming {
            if let Some(i) = at.get(&r.out.to_string()) {
                bundles[*i].incoming.push(r);
            }
        }
    }

    let owner_ids: Vec<RecordId> = bundles
        .iter()
        .flat_map(|b| {
            let owners = b.memories.iter().map(|m| &m.owner).chain(b.outgoing.iter().chain(&b.incoming).map(|r| &r.owner));
            std::iter::once(&b.row.owner).chain(owners).flatten().cloned().collect::<Vec<_>>()
        })
        .collect();

    let emails = fetch_emails(control, &owner_ids).await?;
    let email_for = |id: &Option<RecordId>| -> Value {
        id.as_ref()
            .and_then(|i| emails.get(&i.to_string()))
            .map(|e| json!(e))
            .unwrap_or(Value::Null)
    };

    let entities_out: Vec<Value> = bundles
        .into_iter()
        .map(|b| {
            let memory: Vec<Value> = b
                .memories
                .iter()
                .map(|m| {
                    json!({
                        "id": m.id.to_string(),
                        "text": m.text,
                        "type": m.kind,
                        "proof_count": m.proof_count,
                        "status": m.status,
                        "created_at": datetime_str(&m.created_at),
                        "updated_at": datetime_str(&m.updated_at),
                        "owner_email": email_for(&m.owner),
                    })
                })
                .collect();

            let relations: Vec<Value> = b
                .outgoing
                .iter()
                .map(|r| {
                    json!({
                        "id": r.id.to_string(),
                        "in": r.in_.to_string(),
                        "out": r.out.to_string(),
                        "label": r.label,
                        "direction": "out",
                        "created_at": datetime_str(&r.created_at),
                        "owner_email": email_for(&r.owner),
                    })
                })
                .chain(b.incoming.iter().map(|r| {
                    json!({
                        "id": r.id.to_string(),
                        "in": r.in_.to_string(),
                        "out": r.out.to_string(),
                        "label": r.label,
                        "direction": "in",
                        "created_at": datetime_str(&r.created_at),
                        "owner_email": email_for(&r.owner),
                    })
                }))
                .collect();

            json!({
                "id": b.row.id.to_string(),
                "kind": b.kind,
                "name": b.row.name,
                "aliases": b.row.aliases,
                "summary": b.row.summary,
                "owner_email": email_for(&b.row.owner),
                "memory": memory,
                "relations": relations,
            })
        })
        .collect();

    let mut chat_res = store::app::CHAT_MESSAGES_FOR_OWNER
        .on(db)
        .bind(("owner", user.id.clone()))
        .await?;
    let chat_rows: Vec<ChatMessageRow> = chat_res.take(0)?;
    let chat_history: Vec<Value> = chat_rows
        .iter()
        .map(|m| {
            json!({
                "id": m.id.to_string(),
                "thread_id": m.thread_id.to_string(),
                "role": m.role,
                "content": m.content,
                "tool_calls": m.tool_calls,
                "tool_call_id": m.tool_call_id,
                "created_at": datetime_str(&m.created_at),
            })
        })
        .collect();

    let tables = [KINDS, &["memory", "relates_to", "chat_message"]].concat();
    Ok(json!({
        "exported_at": Utc::now().to_rfc3339(),
        "user": { "id": user.id.to_string(), "email": user.email },
        "scope": {
            "vault": "personal",
            "tables": tables,
            "left_out": ["other vaults you belong to (export each separately)", "cache_record (synced source data, re-sync to rebuild)", "connectors and settings", "audit_log", "uploaded documents (download each from Documents; their files are not in this export)"],
        },
        "entities": entities_out,
        "chat_history": chat_history,
    }))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    export_data,
))]
pub struct Doc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datetime_str_is_null_when_absent() {
        assert_eq!(datetime_str(&None), Value::Null);
    }

    #[test]
    fn kinds_are_every_entity_table() {
        assert_eq!(KINDS, ["person", "organisation", "location", "repository", "file", "symbol", "entity"]);
    }
}
