//! Entities: search, get, graph (read-only), update, delete and merge.
//! Part of the tool registry (see `registry/mod.rs`); one `register` call per tool.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use super::{register, IdArgs, bad_args, parse_rid, parse_opt_rid, parse_vault_ids, to_tool_value, vault_ids_schema, ToolSpec};
use crate::entities::service::KINDS;
use crate::entities::tools as entities_tools;

pub(super) fn register_all(registry: &mut HashMap<&'static str, ToolSpec>) {
    // -- entities::tools (read-only + mutating) ------------------------------

    #[derive(serde::Deserialize, Default)]
    struct EntitiesSearchArgs {
        query: String,
        #[serde(default)]
        kind: Option<String>,
        #[serde(default = "default_entities_search_limit")]
        limit: usize,
        #[serde(default)]
        offset: usize,
        #[serde(default)]
        vault_id: Option<String>,
        #[serde(default)]
        vault_ids: Option<Vec<String>>,
    }
    fn default_entities_search_limit() -> usize {
        50
    }

    register(
        registry,
        "entities_search",
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "kind": {"type": "string", "enum": KINDS},
                "limit": {"type": "integer"},
                "offset": {"type": "integer"},
                "vault_id": {"type": "string", "description": "search just this vault (a vault id or its name, e.g. \"Acme\"); omit to search your personal vault and every org vault you belong to"},
                "vault_ids": vault_ids_schema(),
            },
            "required": ["query"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: EntitiesSearchArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let vaults = match parse_vault_ids(&a.vault_id, &a.vault_ids) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                let result = entities_tools::entities_search(
                    &state.db,
                    owner,
                    &a.query,
                    a.kind.as_deref(),
                    a.limit,
                    a.offset,
                    &vaults,
                )
                .await;
                Ok(to_tool_value(result))
            })
        }),
    );

    register(
        registry,
        "entities_get",
        json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: IdArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let rid = match parse_rid("id", &a.id) {
                    Ok(r) => r,
                    Err(e) => return Ok(e),
                };
                match entities_tools::entities_get(&state.db, &state.control, owner, &rid).await {
                    Ok(Some(entity)) => Ok(serde_json::to_value(entity).unwrap_or_else(|e| json!({ "error": e.to_string() }))),
                    Ok(None) => Ok(crate::error::AppError::coded(crate::error::ErrorCode::EntityNotFound, "not found").to_tool_value()),
                    Err(e) => Ok(e.to_tool_value()),
                }
            })
        }),
    );

    #[derive(serde::Deserialize, Default)]
    struct EntitiesGraphArgs {
        #[serde(default)]
        kinds: Option<Vec<String>>,
        #[serde(default)]
        vault_id: Option<String>,
    }

    register(
        registry,
        "entities_graph",
        json!({
            "type": "object",
            "properties": {
                "kinds": {
                    "type": "array",
                    "items": {"type": "string", "enum": KINDS},
                    "description": "restrict to these entity kinds (e.g. just the code kinds); omit for the full graph",
                },
                "vault_id": {"type": "string", "description": "use this vault instead of your personal one (a vault id or its name, e.g. \"Acme\")"},
            },
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: EntitiesGraphArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let vault_id = match parse_opt_rid("vault_id", &a.vault_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                let result = entities_tools::entities_graph(&state.db, &state.control, owner, a.kinds.as_deref(), vault_id.as_ref()).await;
                Ok(to_tool_value(result))
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct EntityUpdateArgs {
        entity_id: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        aliases: Option<Vec<String>>,
        #[serde(default)]
        summary: Option<String>,
    }

    register(
        registry,
        "entity_update",
        json!({
            "type": "object",
            "properties": {
                "entity_id": {"type": "string"},
                "name": {"type": "string"},
                "aliases": {"type": "array", "items": {"type": "string"}},
                "summary": {"type": "string"},
            },
            "required": ["entity_id"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: EntityUpdateArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let rid = match parse_rid("entity_id", &a.entity_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                let result = entities_tools::entity_update(
                    &state.db, owner, &rid, a.name.as_deref(), a.aliases, a.summary.as_deref(),
                )
                .await;
                Ok(to_tool_value(result.and_then(|e| e.ok_or_else(|| crate::error::AppError::coded(crate::error::ErrorCode::EntityNotFound, "entity not found")))))
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct EntityIdArgs {
        entity_id: String,
    }

    register(
        registry,
        "entity_delete",
        json!({"type": "object", "properties": {"entity_id": {"type": "string"}}, "required": ["entity_id"]}),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: EntityIdArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let rid = match parse_rid("entity_id", &a.entity_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                let result = entities_tools::entity_delete(&state.db, owner, &rid).await;
                Ok(to_tool_value(result.map(|deleted| json!({ "deleted": deleted }))))
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct EntityMergeArgs {
        winner_id: String,
        loser_id: String,
    }

    register(
        registry,
        "entity_merge",
        json!({
            "type": "object",
            "properties": {
                "winner_id": {"type": "string", "description": "the entity to keep"},
                "loser_id": {"type": "string", "description": "the duplicate to merge in and delete"},
            },
            "required": ["winner_id", "loser_id"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: EntityMergeArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let winner_id = match parse_rid("winner_id", &a.winner_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                let loser_id = match parse_rid("loser_id", &a.loser_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                let result = entities_tools::entity_merge(&state.db, owner, &winner_id, &loser_id).await;
                Ok(to_tool_value(result))
            })
        }),
    );
}
