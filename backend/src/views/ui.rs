//! json-render catalog boundary. Models arrange elements; only the server supplies data.
use std::collections::{BTreeMap, HashSet};
use serde::Deserialize;
use serde_json::{json, Value};
use surrealdb::types::RecordId;
use crate::{error::{AppError, AppResult}, ontology::dataset, pool::OrgDb};
use super::{View, ViewSpec};

pub const MAX_ELEMENTS: usize = 100;
pub const MAX_DEPTH: usize = 12;
pub const MAX_BYTES: usize = 64 * 1024;
#[derive(Debug, Clone, Deserialize)]
#[serde(tag="type", rename_all="snake_case", deny_unknown_fields)]
pub enum Source {
    Dataset { #[serde(flatten)] query: dataset::Dataset },
    Metric { #[serde(flatten)] args: crate::ontology::metric::EvalArgs },
    Scenario { scenario: crate::ontology::scenario::ScenarioArgs, headline: String },
    Impact { start_id: String, headline: String, #[serde(default)] affected: Vec<super::AffectedSpec> },
    Subgraph { title: String, entity_ids: Vec<String>, #[serde(default)] relation_ids: Vec<String> },
}
fn bad(s: impl Into<String>) -> AppError { AppError::bad_request(s) }
pub fn catalog() -> &'static Value {
    static CATALOG: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| serde_json::from_str(include_str!("catalog.generated.json")).expect("generated catalog"))
}
pub fn description() -> &'static str {
    static DESCRIPTION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DESCRIPTION.get_or_init(|| format!("Show an answer with server-resolved data. Keep the four legacy views, or use view ui with spec and named sources. Sources: dataset (vault_id, kind, properties, filters, group_by, bucket day/week/month/quarter/year, aggregates [{{name,op count/sum/avg/min/max/median/distinct,property?}}], sort {{property,descending}}, limit at most 500. To show detailed rows and totals, omit aggregates; totals are always computed separately); metric (metric_eval arguments); scenario (each named source is {{type: scenario, headline: words, scenario: {{vault_id, changes, metrics}}}}; headline belongs INSIDE that source, not at the top level); impact and subgraph (existing view arguments). A dataset returns rows, columns, status, truncated and warnings. Bind data with {{\"$state\":\"/sources/sales/rows\"}}. Stat binds a scalar pointer. Legacy blocks bind the whole source. Never send literal numbers, numeric strings or inline data. x, y and series name columns. All displayed numbers come from sources. No actions, repeats or visibility expressions. At most 100 elements, depth 12 and 64 KiB. The server returns useful text for terminal clients.\n{}", catalog()["prompt"].as_str().unwrap_or("")))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec { root: String, elements: BTreeMap<String, Element> }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Element { #[serde(rename="type")] kind: String, props: Value, #[serde(default)] children: Vec<String> }

pub fn validate(spec: &Value, data: Option<&Value>) -> AppResult<()> {
    if serde_json::to_vec(spec).map_err(|_| bad("invalid UI spec"))?.len() > MAX_BYTES { return Err(bad("UI spec exceeds 64 KiB")); }
    let spec: Spec = serde_json::from_value(spec.clone()).map_err(|e| bad(format!("invalid UI spec: {e}")))?;
    if spec.elements.is_empty() || spec.elements.len() > MAX_ELEMENTS { return Err(bad("UI needs 1 to 100 elements")); }
    fn walk(id: &str, spec: &Spec, depth: usize, visited: &mut HashSet<String>) -> AppResult<()> {
        if depth > MAX_DEPTH || !visited.insert(id.to_string()) { return Err(bad("UI cycle, shared child or depth exceeds 12")); }
        let e = spec.elements.get(id).ok_or_else(|| bad(format!("missing UI element {id}")))?;
        for c in &e.children { walk(c, spec, depth + 1, visited)?; }
        Ok(())
    }
    let mut visited = HashSet::new(); walk(&spec.root, &spec, 1, &mut visited)?;
    if visited.len() != spec.elements.len() { return Err(bad("UI contains unreachable elements")); }
    for e in spec.elements.values() {
        let def = catalog()["components"].get(&e.kind).ok_or_else(|| bad(format!("unknown component {}", e.kind)))?;
        let validator = jsonschema::validator_for(&def["props"]).map_err(|e| bad(format!("invalid catalog schema: {e}")))?;
        validator.validate(&e.props).map_err(|error| bad(format!("{}: {error}", e.kind)))?;
        // Every data-bearing component has exactly one data prop, which must be a source pointer.
        if let Some(binding) = e.props.get("data") {
            let path = binding["$state"].as_str().ok_or_else(|| bad("data must be a source binding"))?;
            if let Some(data) = data {
                let value = data.pointer(path).ok_or_else(|| bad(format!("binding points at nothing: {path}")))?;
                if value.is_array() && value.as_array().is_some_and(|v| v.len() > dataset::MAX_ROWS) { return Err(bad("bound rows exceed 500")); }
                let rows = value.as_array().or_else(|| value.get("rows").and_then(Value::as_array));
                let mut keys: Vec<&str> = ["x", "y", "group", "size", "parent"].iter().filter_map(|field| e.props[*field].as_str()).collect();
                keys.extend(e.props["series"].as_array().into_iter().flatten().filter_map(Value::as_str));
                keys.extend(e.props["columns"].as_array().into_iter().flatten().filter_map(|c| c["key"].as_str()));
                if let Some(rows) = rows {
                    for key in keys {
                        if rows.iter().any(|r| !r.is_object() || r.get(key).is_none()) {
                            return Err(bad(format!("unknown bound column {key}. For a table of detailed rows with totals, omit aggregates: totals are supplied automatically.")));
                        }
                    }
                }

            }
        }
        if !e.children.is_empty() && !matches!(e.kind.as_str(), "Stack" | "Grid" | "Card" | "Section" | "Tabs" | "Accordion") { return Err(bad("only layout components take children")); }
    }
    Ok(())
}
pub async fn resolve(db: &OrgDb, owner: &RecordId, spec: Value, sources: BTreeMap<String, Source>) -> AppResult<View> {
    validate(&spec, None)?;
    if sources.is_empty() || sources.len() > 8 { return Err(bad("UI needs 1 to 8 named sources")); }
    let mut resolved = serde_json::Map::new();
    for (name, source) in sources {
        if name.is_empty() || name.len() > 64 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') { return Err(bad("source name must use letters, digits and underscores")); }
        let value = match source {
            Source::Dataset { query } => dataset::resolve(db, owner, &query).await?,
            other => {
                let view = match other {
                    Source::Metric { args } => ViewSpec::Metric { metric: args.metric, kind: args.kind, vault_id: args.vault_id, filters: args.filters, as_of: args.as_of, group_by_grain: args.group_by_grain, allow_unapproved: args.allow_unapproved, related_to: args.related_to },
                    Source::Scenario { scenario, headline } => ViewSpec::Scenario { scenario, headline },
                    Source::Impact { start_id, headline, affected } => ViewSpec::Impact { start_id, headline, affected },
                    Source::Subgraph { title, entity_ids, relation_ids } => ViewSpec::Subgraph { title, entity_ids, relation_ids },
                    Source::Dataset { .. } => unreachable!(),
                };
                serde_json::to_value(Box::pin(super::show(db, owner, view)).await?).map_err(|_| bad("source did not serialise"))?
            }
        };
        let mut value = value;
        if value["view"] == "metric" {
            value["status"] = json!(if value["metric"]["status"] == "approved" { "approved" } else { "not approved" });
            value["warnings"] = value["metric"]["warnings"].clone();
        } else if value["view"] == "scenario" {
            value["warnings"] = value["result"]["warnings"].clone();
        }
        resolved.insert(name, value);
    }
    let data = json!({"sources": resolved});
    validate(&spec, Some(&data))?;
    if serde_json::to_vec(&data).map_err(|_| bad("invalid data"))?.len() > 2 * 1024 * 1024 { return Err(bad("resolved UI data exceeds 2 MiB")); }
    Ok(View::Ui { spec, data })
}
pub fn text(spec: &Value, data: &Value) -> String {
    let mut out = String::from("Analysis\n");
    if let Some(elements) = spec["elements"].as_object() {
        for e in elements.values() { if let Some(title) = e["props"]["title"].as_str() { out.push_str(title); out.push('\n'); } }
    }
    // Include all resolved rows, units and warnings. No model-written numeric summary is trusted.
    if let Some(sources) = data["sources"].as_object() {
        for (name, source) in sources {
            out.push_str(&format!("\nSource {name}:\n"));
            if let Some(text) = source["text"].as_str() { out.push_str(text); }
            else if let Some(rows) = source["rows"].as_array() {
                out.push_str(&format!("Status: {}. {} rows.{}\n", source["status"].as_str().unwrap_or("not approved"), rows.len(), if source["truncated"] == true { " Data truncated." } else { "" }));
                for row in rows {
                    if let Some(row) = row.as_object() {
                        let cells: Vec<String> = row.iter().filter(|(key,_)| key.as_str() != "entity_id").map(|(key,value)| {
                            let unit = source["columns"].as_array().into_iter().flatten().find(|c| c["key"] == key.as_str()).and_then(|c| c["unit"].as_str());
                            let shown = if let Some(n) = value.as_f64() { super::number(n, unit) } else if let Some(s) = value.as_str() { s.to_string() } else { value.to_string() };
                            format!("{key}: {shown}")
                        }).collect();
                        out.push_str(&format!("- {}\n", cells.join("; ")));
                    }
                }
                out.push_str(&format!("Totals: {}\nDistributions: {}\n", source["totals"], source["distributions"]));
                for warning in source["warnings"].as_array().into_iter().flatten().filter_map(Value::as_str) { out.push_str(&format!("Note: {warning}\n")); }
            } else { out.push_str(&source.to_string()); }
            out.push('\n');
        }
    }
    out
}
/// Source arguments are advertised beside the generated spec schema, so models see the nesting.
pub fn sources_schema() -> Value {
    let string = json!({"type":"string"});
    let strings = json!({"type":"array","items":{"type":"string"}});
    json!({"type":"object","minProperties":1,"maxProperties":8,"additionalProperties":{
        "type":"object",
        "properties":{
            "type":{"type":"string","enum":["dataset","metric","scenario","impact","subgraph"]},
            "vault_id":string,"kind":string,"properties":strings,"group_by":{"type":"string","description":"One property key, or link:label for an approved outgoing link"},
            "bucket":{"type":"string","enum":["day","week","month","quarter","year"]},
            "filters":{"type":"array","items":{"type":"object"}},
            "sort":{"type":"object","properties":{"property":string,"descending":{"type":"boolean"}},"required":["property"]},
            "limit":{"type":"integer","minimum":1,"maximum":500},
            "aggregates":{"type":"array","description":"Omit for detailed rows, even when a table needs totals. All datasets return totals separately.","items":{"type":"object","properties":{"name":string,"op":{"type":"string","enum":["count","sum","avg","min","max","median","distinct"]},"property":string},"required":["name","op"]}},
            "metric":string,"as_of":string,"group_by_grain":{"type":"boolean"},"allow_unapproved":{"type":"boolean"},"related_to":{"type":"object"},
            "headline":{"type":"string","description":"REQUIRED inside each scenario or impact source"},
            "scenario":{"type":"object","description":"scenario_run arguments: vault_id, changes, metrics, optional as_of, impact_from and depth"},
            "start_id":string,"affected":{"type":"array","items":{"type":"object"}},"title":string,"entity_ids":strings,"relation_ids":strings
        },"required":["type"]
    }})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_literals_unknown_props_missing_bindings_and_cycles() {
        let mut spec = json!({"root":"a","elements":{"a":{"type":"Stat","props":{"title":"Total","summary":"Total","data":{"$state":"/sources/a/value"}}}}});
        assert!(validate(&spec, Some(&json!({"sources":{"a":{"value":12}}}))).is_ok());
        assert!(validate(&spec, Some(&json!({"sources":{}}))).is_err());
        for literal in [json!(12), json!("12"), json!([12]), json!({"value":12})] {
            spec["elements"]["a"]["props"]["data"] = literal;
            assert!(validate(&spec, None).is_err());
        }
        spec["elements"]["a"] = json!({"type":"Stack","props":{},"children":["a"]});
        assert!(validate(&spec, None).is_err());
        spec["elements"]["a"] = json!({"type":"Unknown","props":{}});
        assert!(validate(&spec, None).is_err());
    }
    #[test]
    fn refuses_missing_columns_bad_props_depth_and_element_caps() {
        let spec = json!({"root":"a","elements":{"a":{"type":"Table","props":{"title":"Rows","summary":"Rows","columns":[{"key":"fake","label":"Fake"}],"data":{"$state":"/sources/sample"}}}}});
        assert!(validate(&spec,Some(&json!({"sources":{"sample":{"rows":[{"real":1}]}}}))).is_err());
        let mut elements = serde_json::Map::new();
        for i in 0..=MAX_DEPTH { elements.insert(i.to_string(),json!({"type":"Stack","props":{},"children":if i < MAX_DEPTH {vec![(i+1).to_string()]} else {vec![]}})); }
        assert!(validate(&json!({"root":"0","elements":elements}),None).is_err());
        for i in 0..=MAX_ELEMENTS { elements.insert(i.to_string(),json!({"type":"Stack","props":{},"children":[]})); }
        assert!(validate(&json!({"root":"0","elements":elements}),None).is_err());
        let spec = json!({"root":"a","elements":{"a":{"type":"Stat","props":{"title":"Total","summary":"Total","data":{"$state":"/sources/a"},"value":999}}}});
        assert!(validate(&spec,None).is_err());
    }
    #[test]
    fn rejects_caps() {
        let spec = json!({"root":"a","elements":{"a":{"type":"Text","props":{"text":"a".repeat(MAX_BYTES)}}}});
        assert!(validate(&spec, None).is_err());
    }
}
