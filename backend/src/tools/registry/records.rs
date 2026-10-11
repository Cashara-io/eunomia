//! Synced source records: search, get, list, links (read-only).
//! Part of the tool registry (see `registry/mod.rs`); one `register` call per tool.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};

use super::{register, IdArgs, bad_args, parse_vault_ids, vault_ids_schema, ToolSpec};
use crate::tools::generic;

pub(super) fn register_all(registry: &mut HashMap<&'static str, ToolSpec>) {
    // -- tools::generic: search, get, list, links (read-only) ---------------

    #[derive(serde::Deserialize, Default)]
    struct SearchArgs {
        query: String,
        #[serde(default)]
        sources: Option<Vec<String>>,
        #[serde(default)]
        types: Option<Vec<String>>,
        #[serde(default)]
        since: Option<String>,
        #[serde(default)]
        until: Option<String>,
        #[serde(default = "default_mode")]
        mode: String,
        #[serde(default = "default_search_limit")]
        limit: i64,
        #[serde(default)]
        offset: i64,
        #[serde(default)]
        vault_id: Option<String>,
        #[serde(default)]
        vault_ids: Option<Vec<String>>,
    }
    fn default_mode() -> String {
        "hybrid".to_string()
    }
    fn default_search_limit() -> i64 {
        20
    }

    register(
        registry,
        "search",
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "sources": {"type": "array", "items": {"type": "string"}},
                "types": {"type": "array", "items": {"type": "string"}},
                "since": {"type": "string", "description": "ISO 8601 date or datetime"},
                "until": {"type": "string", "description": "ISO 8601 date or datetime"},
                "mode": {"type": "string", "enum": ["keyword", "semantic", "hybrid"]},
                "limit": {"type": "integer", "description": "at most 100, default 20"},
                "offset": {"type": "integer", "description": "offset + limit at most 1000; page while has_more"},
                "vault_id": {"type": "string", "description": "search just this vault's records (a vault id or its name); omit for your personal vault and every org vault you belong to"},
                "vault_ids": vault_ids_schema(),
            },
            "required": ["query"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: SearchArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let vaults = match parse_vault_ids(&a.vault_id, &a.vault_ids) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                generic::search(
                    &state.db,
                    &state.settings,
                    owner,
                    &a.query,
                    a.sources.as_deref(),
                    a.types.as_deref(),
                    a.since.as_deref(),
                    a.until.as_deref(),
                    &a.mode,
                    a.limit,
                    a.offset,
                    &vaults,
                )
                .await
            })
        }),
    );

    register(
        registry,
        "get",
        json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: IdArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                generic::get(&state.db, owner, &a.id).await
            })
        }),
    );

    #[derive(serde::Deserialize, Default)]
    struct ListArgs {
        #[serde(rename = "type", default)]
        type_: Option<String>,
        #[serde(default)]
        filters: Option<Value>,
        #[serde(default = "default_sort")]
        sort: String,
        #[serde(default = "default_list_limit")]
        limit: i64,
        #[serde(default)]
        offset: i64,
    }
    fn default_sort() -> String {
        "-occurred_at".to_string()
    }
    fn default_list_limit() -> i64 {
        50
    }

    register(
        registry,
        "list",
        json!({
            "type": "object",
            "properties": {
                "type": {"type": "string"},
                "filters": {
                    "type": "object",
                    "description": "{field: value} equality, or field__ne/__gt/__gte/__lt/__lte; fields: occurred_at, \
ingested_at, updated_at (ISO 8601), title, type, source, id, external_id, or payload__<field>[__<subfield>]",
                },
                "sort": {"type": "string", "description": "a filter field, -field for descending"},
                "limit": {"type": "integer"},
                "offset": {"type": "integer"},
            },
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: ListArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let filters = a.filters.as_ref().and_then(|v| v.as_object());
                generic::list(&state.db, owner, a.type_.as_deref(), filters, &a.sort, a.limit, a.offset).await
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct LinksArgs {
        id: String,
        #[serde(default)]
        rel: Option<String>,
    }

    register(
        registry,
        "links",
        json!({
            "type": "object",
            "properties": {"id": {"type": "string"}, "rel": {"type": "string"}},
            "required": ["id"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: LinksArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                generic::links(&state.db, owner, &a.id, a.rel.as_deref()).await
            })
        }),
    );
}
