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

/// `show_view`'s input schema: one object per view, told apart by `view`.
#[inline(never)]
fn schema() -> serde_json::Value {
    let impact = json!({
        "type": "object",
        "description": "What a change, delay or problem with one entity affects",
        "properties": {
            "view": {"type": "string", "enum": ["impact"]},
            "start_id": {"type": "string", "description": "the entity that changed"},
            "headline": {"type": "string", "description": "one sentence: what happened (at most 200 characters)"},
            "affected": {
                "type": "array",
                "maxItems": views::MAX_AFFECTED,
                "items": {
                    "type": "object",
                    "properties": {
                        "entity_id": {"type": "string", "description": "an affected entity"},
                        "path": id_list("the relation (edge) ids from the start to this entity, in order; each may run either way"),
                        "why": {"type": "string", "description": "one sentence: why this entity is affected (at most 300 characters)"},
                    },
                    "required": ["entity_id", "path", "why"],
                },
            },
        },
        "required": ["view", "start_id", "headline", "affected"],
    });
    let subgraph = json!({
        "type": "object",
        "description": "How some entities connect",
        "properties": {
            "view": {"type": "string", "enum": ["subgraph"]},
            "title": {"type": "string"},
            "entity_ids": id_list("the entities to show"),
            "relation_ids": id_list("the relations (edge ids) between them to show; their ends are shown too"),
        },
        "required": ["view", "title", "entity_ids"],
    });
    json!({
        "type": "object",
        "properties": {"view": {"description": "the view to draw: `impact` or `subgraph`", "anyOf": [impact, subgraph]}},
        "required": ["view"],
    })
}

pub(super) fn register_all(registry: &mut HashMap<&'static str, ToolSpec>) {
    #[derive(serde::Deserialize)]
    struct ShowArgs {
        view: ViewSpec,
    }

    register(
        registry,
        views::TOOL,
        schema(),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: ShowArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                Ok(to_tool_value(views::show(&state.db, owner, a.view).await))
            })
        }),
    );
}
