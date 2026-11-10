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
            "view": {"type": "string", "enum": ["impact", "subgraph", "metric", "scenario", "ui"], "description": "`impact`: what a change, delay or problem with one entity affects. `subgraph`: how some entities connect. `metric`: an approved metric's number, run again by the server. `scenario`: a what-if, its metrics before and after, run again by the server"},
            "spec": views::ui::catalog()["schema"].clone(),
            "sources": views::ui::sources_schema(),
            "start_id": {"type": "string", "description": "impact: the entity that changed"},
            "headline": {"type": "string", "description": "impact: one sentence, what happened; scenario: one sentence, what the what-if asks (at most 200 characters)"},
            "scenario": {"type": "object", "description": "scenario: the same arguments you gave scenario_run (changes, metrics, vault_id, as_of, impact_from, depth)"},
            "reach": views::impact::schema(),
            "cursor": {"type":"string","description":"impact with reach: previous next_cursor; repeat the same reach. Sorted by immutable record ID. Items added after the first page may not appear. total_partial means the total is a lower bound: say at least N"},
            "evidence_cursor": {"type":"string","description":"metric: previous evidence.next_cursor; repeat evidence.args for the next contributors. Items added after the first page may not appear"},
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
            "metric": {"type": "string", "description": "metric: the metric's name; the other metric fields are as for metric_eval"},
            "kind": {"type": "string", "description": "metric: its kind, when several kinds have one of that name"},
            "vault_id": {"type": "string", "description": "metric: the vault, as for metric_eval"},
            "filters": {"type": "array", "items": {"type": "object"}, "description": "metric: the extra filters you gave metric_eval"},
            "related_to": super::ontology::related_schema("metric: the related_to you gave metric_eval"),
            "as_of": {"type": "string", "description": "metric: as for metric_eval"},
            "group_by": {"type":"string","description":"Approved property or link:label. Returns groups with value, entity_ids and n_entities. Cannot combine with group_by_grain."},
            "sort": {"type":"object","properties":{"property":{"type":"string","enum":["group","value"]},"descending":{"type":"boolean"}},"required":["property"],"additionalProperties":false},
            "top_n": {"type":"integer","minimum":1,"maximum":500},
            "group_by_grain": {"type": "boolean", "description": "metric: draw the series"},
            "allow_unapproved": {"type": "boolean", "description": "metric: only to show a proposed metric the user asked to try; it is drawn as not approved"},
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
                let mut args = args;
                // a scenario's own vault_id may be a name, like every top-level one
                if let Some(inner) = args.get_mut("scenario")
                    && let Err(e) = super::resolve_vault_names(&state.db, owner, inner).await
                {
                    return Ok(json!({ "error": e }));
                }
                if let Some(sources) = args.get_mut("sources").and_then(serde_json::Value::as_object_mut) {
                    for source in sources.values_mut() {
                        if let Err(e) = super::resolve_vault_names(&state.db, owner, source).await { return Ok(json!({"error":e})); }
                        if let Some(scenario) = source.get_mut("scenario")
                            && let Err(e) = super::resolve_vault_names(&state.db, owner, scenario).await { return Ok(json!({"error":e})); }
                    }
                }
                let spec: ViewSpec = match serde_json::from_value(args) {
                    Ok(s) => s,
                    Err(e) => return Ok(bad_args(e)),
                };
                Ok(to_tool_value(views::show(&state.db, owner, spec).await))
            })
        }),
    );
}
