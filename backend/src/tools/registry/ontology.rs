//! The project ontology: read it, have the server's model suggest, propose, write a value of an
//! approved property, and run an approved metric. Deciding is REST only (`routers/ontology.rs`), so an agent cannot approve its own
//! proposal.
//! Part of the tool registry (see `registry/mod.rs`); one `register` call per tool.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use super::{bad_args, kind_schema, parse_opt_rid, parse_rid, register, to_tool_value, ToolSpec};
use crate::ontology::automap::{self, AutomapArgs};
use crate::ontology::metric::{self, EvalArgs};
use crate::ontology::tabular::{self, ImportArgs};
use crate::ontology::property::{self, Datatype};
use crate::ontology::{self, Proposals};

fn synonyms_schema() -> serde_json::Value {
    json!({"type": "array", "maxItems": 20, "items": {"type": "string"}, "description": "other names for it written in the vault's own data (an abbreviation, an 'also called'); never guessed"})
}

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
                        "synonyms": synonyms_schema(),
                        "reason": {"type": "string", "description": "why, for the person who reviews it"},
                    },
                    "required": ["kind"],
                },
            },
            "properties": {
                "type": "array",
                "maxItems": ontology::MAX_PROPOSED_PROPERTIES,
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": kind_schema(),
                        "name": {"type": "string", "pattern": "^[a-z][a-z0-9_]{0,63}$", "description": "the property's name on that kind, e.g. price, size, start_date"},
                        "datatype": {"type": "string", "enum": Datatype::ALL.map(Datatype::as_str)},
                        "unit": {"type": "string", "description": "for money a currency code (AUD), for a measure its unit (kg, minutes)"},
                        "enum_values": {"type": "array", "items": {"type": "string"}, "description": "an enum's allowed values (enum only)"},
                        "description": {"type": "string", "description": "what it holds, in one sentence"},
                        "synonyms": synonyms_schema(),
                        "merge_into": {"type": "string", "description": "propose folding this property into that one of the same kind (the one to keep)"},
                        "reason": {"type": "string", "description": "why, for the person who reviews it"},
                        "source_map": {
                            "type": "array",
                            "maxItems": 20,
                            "description": "the record fields or CSV columns its values come from, from ontology_automap's profiles; a CSV import reads it to know which column feeds this property",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "source": {"type": "string", "description": "the records' source (stripe), or document for an uploaded CSV"},
                                    "record_type": {"type": "string", "description": "the record type (stripe.charge), or the CSV's document id"},
                                    "path": {"type": "string", "description": "the payload key path (billing.country) or the CSV column"},
                                },
                                "required": ["source", "record_type", "path"],
                            },
                        },
                    },
                    "required": ["kind", "name", "datatype"],
                },
            },
            "metrics": {
                "type": "array",
                "maxItems": ontology::MAX_PROPOSED_METRICS,
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": kind_schema(),
                        "name": {"type": "string", "pattern": "^[a-z][a-z0-9_]{0,63}$", "description": "the metric's name on that kind, e.g. avg_price, open_order_value"},
                        "expr": expr_schema(),
                        "unit": {"type": "string", "description": "when it differs from the measured property's (a ratio's, say)"},
                        "description": {"type": "string", "description": "what the number means, in one sentence"},
                        "synonyms": synonyms_schema(),
                        "merge_into": {"type": "string", "description": "propose folding this metric into that one of the same kind (the one to keep)"},
                        "reason": {"type": "string", "description": "why, for the person who reviews it"},
                    },
                    "required": ["kind", "name", "expr"],
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

fn filters_schema(what: &str, max: usize) -> serde_json::Value {
    json!({
        "type": "array",
        "maxItems": max,
        "description": what,
        "items": {
            "type": "object",
            "properties": {
                "property": {"type": "string", "description": "a property of the metric's kind"},
                "op": {"type": "string", "enum": ["eq", "ne", "gt", "gte", "lt", "lte", "in"], "description": "gt, gte, lt and lte for numbers and dates only"},
                "value": {"description": "in the property's datatype (an enum value as listed, a date as YYYY-MM-DD); a list for in"},
            },
            "required": ["property", "op", "value"],
        },
    })
}

fn expr_schema() -> serde_json::Value {
    let side = json!({
        "type": "object",
        "properties": {
            "agg": {"type": "string", "enum": ["sum", "count", "avg", "min", "max", "count_distinct"]},
            "property": {"type": "string"},
            "filters": filters_schema("this side's own filters", metric::MAX_FILTERS),
        },
        "required": ["agg"],
    });
    json!({
        "type": "object",
        "description": "what the metric computes from the kind's typed properties; never query text",
        "properties": {
            "agg": {"type": "string", "enum": ["sum", "count", "avg", "min", "max", "count_distinct", "ratio"], "description": "sum, avg, min and max need a number, integer, money or percent property; count counts entities (with property: those that have it); ratio divides numerator by denominator"},
            "property": {"type": "string", "description": "the measured property of the kind"},
            "numerator": side.clone(),
            "denominator": side,
            "filters": filters_schema("ANDed; at most 10 in all", metric::MAX_FILTERS),
            "time_property": {"type": "string", "description": "a date or datetime property, for a series by grain"},
            "grain": {"type": "string", "enum": ["day", "week", "month", "quarter", "year"]},
        },
        "required": ["agg"],
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
    register_eval(registry);
    register_automap(registry);
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
        #[serde(flatten)]
        proposals: Proposals,
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
                Ok(to_tool_value(ontology::propose(&state.db, owner, vault.as_ref(), &a.proposals).await))
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct SetArgs {
        entity_id: String,
        property: String,
        value: serde_json::Value,
        #[serde(default)]
        unit: Option<String>,
        #[serde(default)]
        as_of: Option<String>,
        #[serde(default)]
        source_record_id: Option<String>,
    }

    register(
        registry,
        "entity_set_property",
        json!({
            "type": "object",
            "properties": {
                "entity_id": {"type": "string", "description": "the entity, from entities_search or memory_write"},
                "property": {"type": "string", "description": "an approved property of the entity's kind (ontology_get lists them)"},
                "value": {"description": "in the property's datatype: a number for number, integer, money and percent (4.5, not \"$4.50\"), true or false, a string for text and enum, YYYY-MM-DD for a date, RFC 3339 for a datetime"},
                "unit": {"type": "string", "description": "the value's unit when it differs from the property's"},
                "as_of": {"type": "string", "description": "when the value was true (ISO 8601 date or datetime), if not now"},
                "source_record_id": {"type": "string", "description": "the synced record the value came from"},
            },
            "required": ["entity_id", "property", "value"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: SetArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let entity = match parse_rid("entity_id", &a.entity_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                let input = property::SetProperty {
                    entity_id: &entity,
                    property: &a.property,
                    value: &a.value,
                    unit: a.unit.as_deref(),
                    as_of: a.as_of.as_deref(),
                    source_record_id: a.source_record_id.as_deref(),
                };
                Ok(to_tool_value(property::set_property(&state.db, owner, input).await))
            })
        }),
    );
}

/// `metric_eval`'s input schema, in its own frame like `propose_schema`.
#[inline(never)]
fn eval_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "metric": {"type": "string", "description": "the metric's name, from ontology_get"},
            "kind": {"type": "string", "description": "its kind, when several kinds have a metric of that name"},
            "vault_id": {"type": "string", "description": "the vault (a vault id or its name); omit for your personal vault"},
            "filters": filters_schema("extra filters for this run, ANDed with the metric's own", metric::MAX_FILTERS),
            "as_of": {"type": "string", "description": "use each entity's latest value as of this ISO 8601 date or datetime (a date includes its whole day); default now"},
            "group_by_grain": {"type": "boolean", "description": "a series by the metric's time_property and grain instead of one value"},
            "allow_unapproved": {"type": "boolean", "description": "run a proposed metric to try it; the result says it is unapproved. Never quote such a number as checked"},
        },
        "required": ["metric"],
    })
}

pub(super) fn register_eval(registry: &mut HashMap<&'static str, ToolSpec>) {
    register(
        registry,
        "metric_eval",
        eval_schema(),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: EvalArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                Ok(to_tool_value(metric::eval(&state.db, owner, &a).await))
            })
        }),
    );
}

/// `ontology_automap` and `document_import_rows`: map records and a CSV, then import the CSV's rows.
fn register_automap(registry: &mut HashMap<&'static str, ToolSpec>) {
    register(
        registry,
        "ontology_automap",
        json!({
            "type": "object",
            "properties": {
                "source": {"type": "string", "description": "only this source's synced records, e.g. stripe"},
                "record_type": {"type": "string", "description": "only this record type, e.g. stripe.charge"},
                "document_id": {"type": "string", "description": "map an uploaded CSV instead of synced records"},
                "vault_id": {"type": "string", "description": "the vault to map into (a vault id or its name); omit for a CSV's own vault, else your personal vault"},
            },
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: AutomapArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                Ok(to_tool_value(automap::automap(state, &state.settings, owner, &a).await))
            })
        }),
    );

    register(
        registry,
        "document_import_rows",
        json!({
            "type": "object",
            "properties": {
                "document_id": {"type": "string", "description": "an uploaded CSV"},
                "kind": {"type": "string", "description": "the approved kind each row becomes, e.g. product"},
                "name_column": {"type": "string", "description": "the column that names each row's entity, e.g. product"},
                "column_map": {
                    "type": "object",
                    "additionalProperties": {"type": "string"},
                    "description": "column to property name, e.g. {\"price AUD\": \"price\"}; omit to use each property's source_map from ontology_automap, else the property named like the column",
                },
                "as_of": {"type": "string", "description": "when the values were true (ISO 8601 date or datetime), if not now"},
            },
            "required": ["document_id", "kind", "name_column"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: ImportArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                Ok(to_tool_value(tabular::import_rows(state, owner, &a, crate::ontology::Origin::Agent).await))
            })
        }),
    );
}
