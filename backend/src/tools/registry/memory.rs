//! Memories: recall and reflect (read-only), write, update, delete and consolidate.
//! Part of the tool registry (see `registry/mod.rs`); one `register` call per tool.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use super::{register, bad_args, parse_rid, parse_opt_rid, parse_vault_ids, to_tool_value, vault_ids_schema, ToolSpec};
use crate::cache::recall::MemoryType;
use crate::cache::tools as cache_tools;
use super::kind_schema;
use crate::entities::tools as entities_tools;

/// `memory_write_batch`'s input schema, built in its own frame: `register_all` builds every schema
/// inline, and in a debug build one more nested `json!` there overflows a test thread's stack.
#[inline(never)]
fn batch_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "minItems": 1,
                "maxItems": super::MAX_BATCH_ITEMS,
                "description": "the facts, each written exactly as `memory_write` would write it",
                "items": {
                    "type": "object",
                    "properties": {
                        "subject_name": {"type": "string"},
                        "subject_kind": kind_schema(),
                        "text": {"type": "string"},
                        "source_record_id": {"type": "string"},
                        "type": {"type": "string", "enum": ["world", "experience", "observation"]},
                        "vault_id": {"type": "string", "description": "this item's vault, overriding the batch's"},
                    "occurred_at": {"type": "string", "description": "when the fact happened (ISO 8601), if not now"},
                    },
                    "required": ["subject_name", "subject_kind", "text"],
                },
            },
            "vault_id": {"type": "string", "description": "write every item into this vault instead of your personal one (a vault id or its name)"},
        },
        "required": ["items"],
    })
}

/// Writes in flight at once inside one `memory_write_batch` call.
const BATCH_CONCURRENCY: usize = 4;

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
        #[serde(default)]
        include_history: bool,
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
                "include_history": {"type": "boolean", "description": "also return facts a later fact superseded, marked status \"superseded\" (for what was true before); default false: current facts only"},
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
                    a.include_history,
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
        #[serde(default)]
        occurred_at: Option<String>,
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
                "subject_kind": kind_schema(),
                "text": {"type": "string"},
                "source_record_id": {"type": "string"},
                "type": {"type": "string", "enum": ["world", "experience", "observation"]},
                "vault_id": {"type": "string", "description": "write into this vault instead of your personal one (a vault id or its name, e.g. \"Acme\")"},
                "occurred_at": {"type": "string", "description": "when the fact happened or became true (ISO 8601 date or datetime), if not now: e.g. a decision made last March. Recall's time_range and recency use it"},
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
                    state,
                    owner,
                    &a.subject_name,
                    &a.subject_kind,
                    &a.text,
                    a.source_record_id.as_deref(),
                    &a.mem_type,
                    vault_id.as_ref(),
                    a.occurred_at.as_deref(),
                )
                .await;
                Ok(to_tool_value(result))
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct BatchItem {
        subject_name: String,
        subject_kind: String,
        text: String,
        #[serde(default)]
        source_record_id: Option<String>,
        #[serde(rename = "type", default = "default_memory_write_type")]
        mem_type: String,
        #[serde(default)]
        vault_id: Option<String>,
        #[serde(default)]
        occurred_at: Option<String>,
    }
    #[derive(serde::Deserialize)]
    struct MemoryWriteBatchArgs {
        items: Vec<serde_json::Value>,
        #[serde(default)]
        vault_id: Option<String>,
    }

    register(
        registry,
        "memory_write_batch",
        batch_schema(),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                use futures::StreamExt;
                let a: MemoryWriteBatchArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                if a.items.is_empty() || a.items.len() > super::MAX_BATCH_ITEMS {
                    return Ok(crate::error::AppError::bad_request(format!("items: 1 to {} facts per call", super::MAX_BATCH_ITEMS)).to_tool_value());
                }
                // a few writes at a time: facts about one subject serialise on its lock anyway, and a
                // wide fan-out would only trade 429s for database write conflicts. Each write's (large)
                // future is boxed rather than held inline in the stream.
                let results: Vec<serde_json::Value> = futures::stream::iter(a.items.into_iter().enumerate())
                    .map(|(index, raw)| {
                        let default_vault = a.vault_id.clone();
                        Box::pin(async move {
                            let item: BatchItem = match serde_json::from_value(raw) {
                                Ok(i) => i,
                                Err(e) => return json!({"index": index, "ok": false, "error": e.to_string(), "code": "validation.invalid"}),
                            };
                            let vault = match parse_opt_rid("vault_id", &item.vault_id.or(default_vault)) {
                                Ok(v) => v,
                                Err(e) => return json!({"index": index, "ok": false, "error": e["error"], "code": e["code"]}),
                            };
                            let out = entities_tools::memory_write(
                                state,
                                owner,
                                &item.subject_name,
                                &item.subject_kind,
                                &item.text,
                                item.source_record_id.as_deref(),
                                &item.mem_type,
                                vault.as_ref(),
                                item.occurred_at.as_deref(),
                            )
                            .await;
                            match out {
                                Ok(o) => {
                                    let mut v = json!({"index": index, "ok": true, "entity_id": o.entity.id, "memory_id": o.memory.id});
                                    if !o.memory.redacted.is_empty() {
                                        v["redacted"] = json!(o.memory.redacted);
                                    }
                                    v
                                }
                                Err(e) => {
                                    let v = e.to_tool_value();
                                    json!({"index": index, "ok": false, "error": v["error"], "code": v["code"]})
                                }
                            }
                        })
                    })
                    .buffered(BATCH_CONCURRENCY)
                    .collect()
                    .await;
                let written = results.iter().filter(|r| r["ok"] == true).count();
                Ok(json!({"written": written, "failed": results.len() - written, "results": results}))
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
