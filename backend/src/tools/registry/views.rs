//! `show_view`: draw the model's answer as a view (`crate::views`).
//! Part of the tool registry (see `registry/mod.rs`); one `register` call per tool.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use super::{bad_args, register, to_tool_value, ToolSpec};
use crate::views::{self, ViewSpec};

fn id_list(what: &str) -> serde_json::Value {
    json!({"type": "array", "items": {"type": "string"}, "description": what})
}

/// `show_view`'s input schema: the view itself, told apart by `view`. Flat rather than an `anyOf` per
/// view, because models send a view's fields at the top level.
#[inline(never)]
fn schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "view": {"type": "string", "enum": ["impact", "subgraph"], "description": "`impact`: what a change, delay or problem with one entity affects. `subgraph`: how some entities connect"},
            "start_id": {"type": "string", "description": "impact: the entity that changed"},
            "headline": {"type": "string", "description": "impact: one sentence, what happened (at most 200 characters)"},
            "affected": {
                "type": "array",
                "description": "impact: each affected entity once",
                "maxItems": views::MAX_AFFECTED,
                "items": {
                    "type": "object",
                    "properties": {
                        "entity_id": {"type": "string", "description": "an affected entity"},
                        "path": {
                            "type": "array",
                            "items": {"type": "string"},
                            "maxItems": views::MAX_PATH,
                            "description": "the relation (edge) ids from the start to this entity, in order; each may run either way; no relation twice, and never back to an entity already passed",
                        },
                        "why": {"type": "string", "description": "one sentence: why this entity is affected (at most 300 characters)"},
                    },
                    "required": ["entity_id", "path", "why"],
                },
            },
            "title": {"type": "string", "description": "subgraph: what the picture shows"},
            "entity_ids": id_list("subgraph: the entities to show"),
            "relation_ids": id_list("subgraph: the relations (edge ids) between them to show; their ends are shown too"),
        },
        "required": ["view"],
    })
}

pub(super) fn register_all(registry: &mut HashMap<&'static str, ToolSpec>) {
    register(
        registry,
        views::TOOL,
        schema(),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let spec: ViewSpec = match serde_json::from_value(args) {
                    Ok(s) => s,
                    Err(e) => return Ok(bad_args(e)),
                };
                Ok(to_tool_value(views::show(&state.db, owner, spec).await))
            })
        }),
    );
}
