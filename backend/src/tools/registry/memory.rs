//! Memories: recall and reflect (read-only), write, update, delete and consolidate.
//! Part of the tool registry (see `registry/mod.rs`); one `register` call per tool.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use super::{register, bad_args, parse_rid, parse_opt_rid, parse_vault_ids, to_tool_value, vault_ids_schema, ToolSpec};
use crate::cache::recall::MemoryType;
use crate::cache::tools as cache_tools;
use crate::entities::service::KINDS;
use crate::entities::tools as entities_tools;

pub(super) fn register_all(registry: &mut HashMap<&'static str, ToolSpec>) {
    // -- cache::tools: recall, reflect (read-only) ---------------------------

    #[derive(serde::Deserialize, Default)]
    struct RecallArgs {
        query: String,
        #[serde(default)]
        time_range: Option<Vec<String>>,
        #[serde(default = "default_recall_limit")]
        limit: usize,
        #[serde(default)]
        max_tokens: Option<usize>,
        #[serde(default)]
        types: Option<Vec<MemoryType>>,
        #[serde(default)]
        vault_id: Option<String>,
        #[serde(default)]
        vault_ids: Option<Vec<String>>,
    }
    fn default_recall_limit() -> usize {
        20
    }

    register(
        registry,
        "recall",
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "at most 2000 characters"},
                "time_range": {
                    "type": "array",
                    "items": {"type": "string"},
                    "minItems": 2,
                    "maxItems": 2,
                    "description": "[since, until] ISO 8601, since <= until",
                },
                "limit": {"type": "integer", "description": "1-100, default 20"},
                "max_tokens": {"type": "integer", "description": "1-100000"},
                "types": {
                    "type": "array",
                    "items": {"type": "string", "enum": ["world", "experience", "observation"]},
                    "description": "filter memory-sourced results by memory.type",
                },
                "vault_id": {"type": "string", "description": "recall from just this vault (a vault id or its name, e.g. \"Acme\"); omit to search your personal vault and every org vault you belong to"},
                "vault_ids": vault_ids_schema(),
            },
            "required": ["query"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: RecallArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let vaults = match parse_vault_ids(&a.vault_id, &a.vault_ids) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                cache_tools::recall_tool(
                    &state.db,
                    &state.settings,
                    owner,
                    &a.query,
                    a.time_range.as_deref(),
                    a.limit,
                    a.max_tokens,
                    a.types.as_deref(),
                    &vaults,
                )
                .await
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct ReflectArgs {
        query: String,
        #[serde(default)]
        vault_id: Option<String>,
        #[serde(default)]
        vault_ids: Option<Vec<String>>,
        #[serde(default = "default_reflect_limit")]
        limit: usize,
    }
    fn default_reflect_limit() -> usize {
        10
    }

    register(
        registry,
        "reflect",
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "vault_id": {"type": "string", "description": "reflect over just this vault (a vault id or its name, e.g. \"Acme\"); omit to use your personal vault and every org vault you belong to"},
                "vault_ids": vault_ids_schema(),
                "limit": {"type": "integer", "description": "how many recalled memories to consider"},
            },
            "required": ["query"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: ReflectArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let vaults = match parse_vault_ids(&a.vault_id, &a.vault_ids) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                cache_tools::reflect_tool(&state.db, &state.settings, owner, &a.query, &vaults, a.limit).await
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct MemoryWriteArgs {
        subject_name: String,
        subject_kind: String,
        text: String,
        #[serde(default)]
        source_record_id: Option<String>,
        #[serde(rename = "type", default = "default_memory_write_type")]
        mem_type: String,
        #[serde(default)]
        vault_id: Option<String>,
    }
    fn default_memory_write_type() -> String {
        "world".to_string()
    }

    register(
        registry,
        "memory_write",
        json!({
            "type": "object",
            "properties": {
                "subject_name": {"type": "string"},
                "subject_kind": {"type": "string", "enum": KINDS},
                "text": {"type": "string"},
                "source_record_id": {"type": "string"},
                "type": {"type": "string", "enum": ["world", "experience", "observation"]},
                "vault_id": {"type": "string", "description": "write into this vault instead of your personal one (a vault id or its name, e.g. \"Acme\")"},
            },
            "required": ["subject_name", "subject_kind", "text"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: MemoryWriteArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let vault_id = match parse_opt_rid("vault_id", &a.vault_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                let result = entities_tools::memory_write(
                    &state.db,
                    &state.settings,
                    owner,
                    &a.subject_name,
                    &a.subject_kind,
                    &a.text,
                    a.source_record_id.as_deref(),
                    &a.mem_type,
                    vault_id.as_ref(),
                )
                .await;
                Ok(to_tool_value(result))
            })
        }),
    );

    #[derive(serde::Deserialize, Default)]
    struct ConsolidateArgs {
        #[serde(default)]
        subject_id: Option<String>,
    }

    register(
        registry,
        "consolidate_observations",
        json!({
            "type": "object",
            "properties": {
                "subject_id": {"type": "string", "description": "consolidate just this entity; omit for all"},
            },
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: ConsolidateArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let subject_id = match parse_opt_rid("subject_id", &a.subject_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                let result =
                    entities_tools::consolidate_observations(&state.db, &state.settings, owner, subject_id.as_ref()).await;
                Ok(to_tool_value(result))
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct MemoryUpdateArgs {
        memory_id: String,
        #[serde(default)]
        text: Option<String>,
        #[serde(rename = "type", default)]
        mem_type: Option<String>,
    }

    register(
        registry,
        "memory_update",
        json!({
            "type": "object",
            "properties": {
                "memory_id": {"type": "string"},
                "text": {"type": "string"},
                "type": {"type": "string", "enum": ["world", "experience"]},
            },
            "required": ["memory_id"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: MemoryUpdateArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let rid = match parse_rid("memory_id", &a.memory_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                let result =
                    entities_tools::memory_update(&state.db, owner, &rid, a.text.as_deref(), a.mem_type.as_deref()).await;
                Ok(to_tool_value(result.and_then(|m| m.ok_or_else(|| crate::error::AppError::coded(crate::error::ErrorCode::MemoryNotFound, "memory not found")))))
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct MemoryIdArgs {
        memory_id: String,
    }

    register(
        registry,
        "memory_delete",
        json!({"type": "object", "properties": {"memory_id": {"type": "string"}}, "required": ["memory_id"]}),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: MemoryIdArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let rid = match parse_rid("memory_id", &a.memory_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                let result = entities_tools::memory_delete(&state.db, owner, &rid).await;
                Ok(to_tool_value(result.map(|deleted| json!({ "deleted": deleted }))))
            })
        }),
    );
}
