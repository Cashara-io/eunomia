//! The project ontology: read it, have the server's model suggest, propose. Deciding is REST only
//! (`routers/ontology.rs`), so an agent cannot approve its own proposal.
//! Part of the tool registry (see `registry/mod.rs`); one `register` call per tool.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use super::{bad_args, kind_schema, parse_opt_rid, register, to_tool_value, ToolSpec};
use crate::ontology::{self, KindProposal, LinkProposal};

/// `ontology_propose`'s input schema, in its own frame like `memory::batch_schema`.
#[inline(never)]
fn propose_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "vault_id": {"type": "string", "description": "the vault whose ontology you propose for (a vault id or its name); omit for your personal vault"},
            "kinds": {
                "type": "array",
                "maxItems": ontology::MAX_PROPOSED_KINDS,
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": kind_schema(),
                        "description": {"type": "string", "description": "what this kind is, in one sentence (at most 500 characters)"},
                        "merge_into": {"type": "string", "description": "propose folding this kind into that one (the kind to keep)"},
                        "reason": {"type": "string", "description": "why, for the person who reviews it"},
                    },
                    "required": ["kind"],
                },
            },
            "links": {
                "type": "array",
                "maxItems": ontology::MAX_PROPOSED_LINKS,
                "items": {
                    "type": "object",
                    "properties": {
                        "from_kind": kind_schema(),
                        "label": {"type": "string", "description": "the relation label, as `code_relate` writes it, e.g. works_at or calls"},
                        "to_kind": kind_schema(),
                        "description": {"type": "string"},
                        "reason": {"type": "string"},
                    },
                    "required": ["from_kind", "label", "to_kind"],
                },
            },
        },
    })
}

fn vault_only_schema(what: &str) -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "vault_id": {"type": "string", "description": format!("{what} (a vault id or its name); omit for your personal vault")},
        },
    })
}

pub(super) fn register_all(registry: &mut HashMap<&'static str, ToolSpec>) {
    #[derive(serde::Deserialize, Default)]
    struct VaultArgs {
        #[serde(default)]
        vault_id: Option<String>,
    }

    register(
        registry,
        "ontology_get",
        vault_only_schema("the vault to read"),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: VaultArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let vault = match parse_opt_rid("vault_id", &a.vault_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                Ok(to_tool_value(ontology::get(&state.db, owner, vault.as_ref()).await))
            })
        }),
    );

    register(
        registry,
        "ontology_suggest",
        vault_only_schema("the vault to review"),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: VaultArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let vault = match parse_opt_rid("vault_id", &a.vault_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                Ok(to_tool_value(ontology::suggest::suggest(&state.db, &state.settings, owner, vault.as_ref()).await))
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct ProposeArgs {
        #[serde(default)]
        vault_id: Option<String>,
        #[serde(default)]
        kinds: Vec<KindProposal>,
        #[serde(default)]
        links: Vec<LinkProposal>,
    }

    register(
        registry,
        "ontology_propose",
        propose_schema(),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: ProposeArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let vault = match parse_opt_rid("vault_id", &a.vault_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                Ok(to_tool_value(ontology::propose(&state.db, owner, vault.as_ref(), &a.kinds, &a.links).await))
            })
        }),
    );
}
