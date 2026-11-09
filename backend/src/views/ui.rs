//! json-render catalog boundary. Models arrange elements; only the server supplies data.
use std::collections::{BTreeMap, HashSet};
use serde::Deserialize;
use serde_json::{json, Value};
use surrealdb::types::RecordId;
mod prose;
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
    DESCRIPTION.get_or_init(|| format!("Show an answer with server-resolved data. Keep the four legacy views, or use view ui with spec and named sources. Sources: dataset (vault_id, kind, properties, filters, group_by, bucket day/week/month/quarter/year, aggregates [{{op count/sum/avg/min/max/median/distinct,property?}}], sort {{property,descending}}, limit at most 500. To show detailed rows and totals, omit aggregates; totals are always computed separately. Aggregate column keys are generated as op_property, or count without a property. Model aliases are refused); metric (metric_eval arguments); scenario (each named source is {{type: scenario, headline: words, scenario: {{vault_id, changes, metrics}}}}; headline belongs INSIDE that source, not at the top level); impact and subgraph (existing view arguments). A dataset returns rows, columns, totals, distributions, total_rows, status, truncated and warnings. Totals are nested by returned column key then operation: /totals/amount/sum without aggregates; /totals/sum_amount/sum after sum of amount. total_rows counts output rows or groups before the limit. Omit aggregates for Histogram, Scatter and detail tables: totals and distributions are already supplied from the detailed rows. Bind data with {{\"$state\":\"/sources/sales/rows\"}}. Stat binds a scalar pointer. Legacy blocks bind the whole source. Never send literal numeric props or inline data; bind measured values to sources. x, y and series name columns. In all written text, use bound placeholders for numbers: title \"Rows {{{{/sources/sales/total_rows}}}}\". Dotted paths such as {{{{sources.sales.rows[0].amount}}}} also work. The server resolves one exact typed numeric or date path and formats it with server units and currency. UI prose is literal plain text without Markdown or entity decoding. Separate placeholders from numeric tokens. Raw digits are allowed only in verbatim phrases with adjacent letter-bearing words in server strings predating the turn, or matching applied ISO dates or bucket labels. No identifier exemption. Number words remain unchecked. The server resolves chart columns and refuses ambiguous y choices. No actions, repeats or visibility expressions. At most 100 elements, depth 12 and 64 KiB. The server returns useful text for terminal clients.\n{}", catalog()["prompt"].as_str().unwrap_or("")))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec { root: String, elements: BTreeMap<String, Element> }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Element { #[serde(rename="type")] kind: String, props: Value, #[serde(default)] children: Vec<String> }

pub fn validate(spec: &Value, data: Option<&Value>) -> AppResult<()> {
    if serde_json::to_vec(spec).map_err(|_| bad("invalid UI spec"))?.len() > MAX_BYTES { return Err(bad("UI spec exceeds 64 KiB")); }
    if let Some(data) = data { prose::resolve(spec, data)?; }
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
    for (id, e) in &spec.elements {
        prose::identifier(id)?;
        for key in ["x", "y", "group", "size", "parent"].iter().filter_map(|key| e.props[*key].as_str())
            .chain(e.props["series"].as_array().into_iter().flatten().filter_map(Value::as_str))
            .chain(e.props["columns"].as_array().into_iter().flatten().filter_map(|c| c["key"].as_str()))
            .chain(e.props["formats"].as_object().into_iter().flat_map(|m| m.keys().map(String::as_str))) {
            // Link columns use the server namespace spelling, e.g. link:supplied_by.
            for part in key.split(':') { prose::identifier(part)?; }
        }
        let def = catalog()["components"].get(&e.kind).ok_or_else(|| bad(format!("unknown component {}", e.kind)))?;
        let validator = jsonschema::validator_for(&def["props"]).map_err(|e| bad(format!("invalid catalog schema: {e}")))?;
        validator.validate(&e.props).map_err(|error| bad(format!("{}: {error}", e.kind)))?;
        // Every data-bearing component has exactly one data prop, which must be a source pointer.
        if let Some(binding) = e.props.get("data") {
            let path = binding["$state"].as_str().ok_or_else(|| bad("data must be a source binding"))?;
            if let Some(data) = data {
                let value = data.pointer(path).ok_or_else(|| bad(format!("binding points at nothing: {path}")))?;
                if value.is_array() && value.as_array().is_some_and(|v| v.len() > dataset::MAX_ROWS) { return Err(bad("bound rows exceed 500")); }
                validate_binding(&e.kind, &e.props, path, value, data)?;

            }
        }
        if !e.children.is_empty() && !matches!(e.kind.as_str(), "Stack" | "Grid" | "Card" | "Section" | "Tabs" | "Accordion") { return Err(bad("only layout components take children")); }
    }
    Ok(())
}
// Resolve columns using metadata, never the availability of individual row measurements.
fn resolve_columns(spec: &mut Value, data: &Value) -> AppResult<()> {
    for e in spec["elements"].as_object_mut().ok_or_else(|| bad("invalid elements"))?.values_mut() {
        let kind = e["type"].as_str().unwrap_or("").to_string();
        if !matches!(kind.as_str(), "Line" | "Area" | "StackedArea" | "Bar" | "GroupedBar" | "StackedBar" | "Histogram" | "Scatter" | "Bubble" | "Pie" | "Donut" | "BoxPlot" | "Violin" | "CalendarHeatmap" | "Heatmap" | "Treemap" | "Partition" | "Radar" | "Funnel" | "Waterfall" | "Sparkline" | "Gauge" | "Bullet" | "Network" | "Tree" | "Combo" | "SmallMultiples" | "Sankey") { continue; }
        let path = e["props"]["data"]["$state"].as_str().unwrap_or("");
        let source = &data["sources"][path.split('/').nth(2).unwrap_or("")];
        let columns: Vec<&str> = source["columns"].as_array().into_iter().flatten().filter_map(|c| c["key"].as_str()).collect();
        let numerics: Vec<&str> = columns.iter().copied().filter(|k| numeric_column(source, k)).collect();
        let props = &mut e["props"];
        let distribution = match kind.as_str() { "Histogram" => Some(("start", "count")), "BoxPlot" => Some(("min", "median")), "Violin" => Some(("value", "count")), _ => None };
        if props["y"].is_null() {
            let y = if let Some((_, y)) = distribution { y } else if numerics.len() == 1 { numerics[0] } else {
                return Err(bad(format!("{kind} has ambiguous numeric columns; name y explicitly")));
            };
            props["y"] = json!(y);
        }
        if props["x"].is_null() {
            let y = props["y"].as_str().unwrap_or("");
            let candidates: Vec<_> = columns.iter().copied().filter(|k| *k != y && !numeric_column(source, k)).collect();
            let x = if let Some((x, _)) = distribution { x } else if matches!(kind.as_str(), "Scatter" | "Bubble") {
                return Err(bad(format!("{kind} requires an explicit numeric x column")));
            } else if candidates.len() == 1 { candidates[0] } else { "name" };
            props["x"] = json!(x);
        }
        if props["series"].is_null() { props["series"] = json!([props["y"]]); }
    }
    Ok(())
}
fn numeric_column(source: &Value, key: &str) -> bool {
    source["columns"].as_array().into_iter().flatten().any(|c| c["key"] == key &&
        matches!(c["datatype"].as_str(), Some("money" | "number" | "integer" | "percent")))
}
fn trusted_number(data: &Value, path: &str) -> bool {
    let decoded: Vec<_> = path.trim_start_matches('/').split('/').map(|s| s.replace("~1", "/").replace("~0", "~")).collect();
    let parts: Vec<_> = decoded.iter().map(String::as_str).collect();
    let ["sources", name, rest @ ..] = parts.as_slice() else { return false };
    let source = &data["sources"][*name];
    if !data.pointer(path).is_some_and(|v| v.is_number() || v.is_null()) { return false; }
    if source["columns"].is_array() {
        return match rest {
            ["rows", index, key] => index.parse::<usize>().is_ok() && numeric_column(source, key),
            ["totals", key, op] => numeric_column(source, key) || *op == "count" || *op == "distinct",
            ["distributions", key, _, _, _] => numeric_column(source, key),
            ["total_rows"] => true,
            _ => false,
        };
    }
    match source["view"].as_str() {
        Some("metric") => matches!(rest, ["metric", "value" | "n_entities"] | ["metric", "series", _, "value" | "n"]),
        Some("scenario") => matches!(rest, ["result", "baseline" | "scenario", _, "value" | "delta" | "delta_pct" | "n_entities"] | ["result", "n_changed"]),
        _ => false,
    }
}
fn validate_binding(kind: &str, props: &Value, path: &str, value: &Value, data: &Value) -> AppResult<()> {
    let source = path.split('/').nth(2).map(|name| &data["sources"][name]).unwrap_or(&Value::Null);
    let numeric = |key: &str, rows: &[Value]| -> AppResult<()> {
        if !numeric_column(source, key) || rows.iter().any(|r| !r[key].is_number() && !r[key].is_null()) {
            return Err(bad(format!("{kind} requires a trusted numeric column: {key}")));
        }
        Ok(())
    };
    if kind == "Stat" {
        if !trusted_number(data, path) { return Err(bad("Stat requires a typed numeric scalar binding")); }
        return Ok(());
    }
    if matches!(kind, "Impact" | "Subgraph" | "Metric" | "Scenario") {
        if value["view"].as_str() != Some(kind.to_lowercase().as_str()) { return Err(bad("legacy block needs its matching source")); }
        return Ok(());
    }
    if matches!(kind, "Meter" | "Comparison") {
        for key in if kind == "Meter" { vec!["value", "min", "max"] } else { vec!["before", "after"] } {
            if !trusted_number(data, &format!("{path}/{key}")) { return Err(bad("range and comparison fields require typed numeric bindings")); }
        }
        return Ok(());
    }
    let rows = value.as_array().or_else(|| value["rows"].as_array()).ok_or_else(|| bad("component requires bound rows"))?;
    let distribution = match kind { "Histogram" => Some(("bins", vec!["start", "end", "count"])), "BoxPlot" => Some(("box", vec!["min", "q1", "median", "q3", "max"])), "Violin" => Some(("density", vec!["value", "count"])), _ => None };
    if let Some((shape, fields)) = distribution {
        let parts: Vec<_> = path.split('/').collect();
        if parts.len() != 6 || parts[3] != "distributions" || parts[5] != shape || !numeric_column(source, parts[4]) || rows.iter().any(|r| fields.iter().any(|k| !r[k].is_number())) {
            return Err(bad("distribution chart requires server-computed distribution data"));
        }
        return Ok(());
    }
    let mut keys: Vec<&str> = ["x", "y", "group", "size", "parent"].iter().filter_map(|field| props[*field].as_str()).collect();
    keys.extend(props["series"].as_array().into_iter().flatten().filter_map(Value::as_str));
    keys.extend(props["columns"].as_array().into_iter().flatten().filter_map(|c| c["key"].as_str()));
    for key in keys {
        if rows.iter().any(|r| !r.is_object() || r.get(key).is_none()) { return Err(bad(format!("unknown bound column {key}"))); }
    }
    for c in props["columns"].as_array().into_iter().flatten() {
        if matches!(c["format"].as_str(), Some("number" | "money" | "percent")) || matches!(c["total"].as_str(), Some("sum" | "avg")) {
            numeric(c["key"].as_str().unwrap_or(""), rows)?;
        }
    }
    if matches!(kind, "Table" | "KeyValue" | "Entity" | "EntityList" | "Timeline") { return Ok(()); }
    if kind == "StatGroup" {
        if !source["columns"].is_array() || !source["columns"].as_array().into_iter().flatten().any(|c| numeric_column(source, c["key"].as_str().unwrap_or(""))) { return Err(bad("StatGroup requires typed numeric columns")); }
        return Ok(());
    }
    let y = props["y"].as_str().ok_or_else(|| bad("chart requires an explicit numeric y column; name y"))?;
    numeric(y, rows)?;
    for key in props["series"].as_array().into_iter().flatten().filter_map(Value::as_str).chain(props["size"].as_str()) { numeric(key, rows)?; }
    if matches!(kind, "Scatter" | "Bubble") { numeric(props["x"].as_str().unwrap_or(""), rows)?; }
    if matches!(kind, "Gauge" | "Bullet") { for key in ["min", "max"] { numeric(key, rows)?; } }
    if matches!(kind, "Network" | "Sankey") {
        let from = props["x"].as_str().unwrap_or("name");
        let to = props["group"].as_str().unwrap_or("target");
        if rows.iter().any(|r| !r[from].is_string() || !r[to].is_string()) { return Err(bad("network requires source and target node columns")); }
    }
    if matches!(kind, "Tree" | "Treemap" | "Partition") {
        let parent = props["parent"].as_str().ok_or_else(|| bad("hierarchy requires a parent column"))?;
        fn id(r: &Value) -> Option<&str> { r["entity_id"].as_str().or_else(|| r["id"].as_str()).or_else(|| r["name"].as_str()) }
        let parents: std::collections::HashMap<_, _> = rows.iter().map(|r| (id(r), r[parent].as_str())).collect();
        if parents.len() != rows.len() || parents.contains_key(&None) { return Err(bad("hierarchy requires unique nodes")); }
        for row in rows {
            let mut seen = HashSet::new(); let mut next = id(row);
            while let Some(key) = next {
                if !seen.insert(key) { return Err(bad("hierarchy contains a cycle")); }
                next = *parents.get(&Some(key)).ok_or_else(|| bad("hierarchy parent is missing"))?;
            }
        }
    }
    Ok(())
}
pub async fn resolve(db: &OrgDb, owner: &RecordId, mut spec: Value, sources: BTreeMap<String, Source>, before: surrealdb::types::Datetime) -> AppResult<View> {
    validate(&spec, None)?;
    if sources.is_empty() || sources.len() > 8 { return Err(bad("UI needs 1 to 8 named sources")); }
    let mut resolved = serde_json::Map::new();
    let mut source_prose = BTreeMap::new();
    let mut legacy_views = BTreeMap::new();
    for (name, source) in sources {
        if name.len() > 64 { return Err(bad("source name exceeds 64 bytes")); }
        prose::identifier(&name)?;
        let prose = match &source {
            Source::Subgraph { title, .. } => json!({"title":title}),
            Source::Scenario { headline, .. } => json!({"headline":headline}),
            Source::Impact { headline, affected, .. } => json!({"headline":headline,"affected":affected.iter().map(|a| json!({"why":a.why})).collect::<Vec<_>>()}),
            _ => Value::Null,
        };
        let value = match source {
            Source::Dataset { query } => {
                let mut value = dataset::resolve(db, owner, &query).await?;
                value["parameters"] = json!({"limit":query.limit,"as_of":query.as_of,"filters":query.filters});
                value["bucket_column"] = json!(query.bucket.and(query.group_by));
                value
            },
            other => {
                let view = match other {
                    Source::Metric { args } => ViewSpec::Metric { metric: args.metric, kind: args.kind, vault_id: args.vault_id, filters: args.filters, as_of: args.as_of, group_by_grain: args.group_by_grain, allow_unapproved: args.allow_unapproved, related_to: args.related_to },
                    Source::Scenario { scenario, headline } => ViewSpec::Scenario { scenario, headline },
                    Source::Impact { start_id, headline, affected } => ViewSpec::Impact { start_id, headline, affected },
                    Source::Subgraph { title, entity_ids, relation_ids } => ViewSpec::Subgraph { title, entity_ids, relation_ids },
                    Source::Dataset { .. } => unreachable!(),
                };
                let shown = Box::pin(super::show(db, owner, view)).await?;
                legacy_views.insert(name.clone(), shown.view.clone());
                serde_json::to_value(shown).map_err(|_| bad("source did not serialise"))?
            }
        };
        let mut value = value;
        if value["view"] == "metric" {
            value["status"] = json!(if value["metric"]["status"] == "approved" { "approved" } else { "not approved" });
            value["warnings"] = value["metric"]["warnings"].clone();
        } else if value["view"] == "scenario" {
            value["warnings"] = value["result"]["warnings"].clone();
        }
        source_prose.insert(name.clone(), prose);
        resolved.insert(name, value);
    }
    let mut data = json!({"sources": resolved});
    // Collect permissions before substituting any model text into resolved sources.
    let trusted = data.clone();
    let allowed = prose::eligible(db, owner, &trusted, before).await?;
    for (name, raw) in source_prose {
        let rendered = prose::resolve_allowed(&raw, &trusted, &allowed)?;
        let source = &mut data["sources"][&name];
        if let Some(fields) = rendered.as_object() {
            for (key, value) in fields {
                if key == "affected" {
                    for (i, affected) in value.as_array().into_iter().flatten().enumerate() {
                        source["affected"][i]["why"] = affected["why"].clone();
                    }
                } else { source[key] = value.clone(); }
            }
        }
        if let Some(view) = legacy_views.get_mut(&name) {
            match view {
                View::Subgraph { title, .. } => *title = source["title"].as_str().unwrap_or("").into(),
                View::Scenario { headline, .. } => *headline = source["headline"].as_str().unwrap_or("").into(),
                View::Impact { headline, affected, .. } => {
                    *headline = source["headline"].as_str().unwrap_or("").into();
                    for (i, a) in affected.iter_mut().enumerate() { a.why = source["affected"][i]["why"].as_str().unwrap_or("").into(); }
                },
                _ => (),
            }
            source["text"] = json!(view.text());
        }
    }
    resolve_columns(&mut spec, &data)?;
    validate(&spec, Some(&data))?;
    spec = prose::resolve_allowed(&spec, &trusted, &allowed)?;
    validate(&spec, None)?; // Expanded text must still fit the renderer schema and size limits.
    if serde_json::to_vec(&data).map_err(|_| bad("invalid data"))?.len() > 2 * 1024 * 1024 { return Err(bad("resolved UI data exceeds 2 MiB")); }
    Ok(View::Ui { spec, data })
}
pub fn text(spec: &Value, data: &Value) -> String {
    let mut out = String::from("Analysis\n");
    if let Some(elements) = spec["elements"].as_object() {
        fn append(value: &Value, out: &mut String) {
            match value {
                Value::Object(fields) => for (key, value) in fields {
                    if prose::field(key) && let Some(s) = value.as_str() { out.push_str("• "); out.push_str(s); out.push('\n'); }
                    else { append(value, out); }
                },
                Value::Array(values) => for value in values { append(value, out); },
                _ => (),
            }
        }
        for e in elements.values() { append(&e["props"], &mut out); }
    }
    // Include all resolved rows, units and warnings. No model-written numeric summary is trusted.
    if let Some(sources) = data["sources"].as_object() {
        for (index, source) in sources.values().enumerate() {
            out.push_str(&format!("\nSource {}:\n", index + 1));
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
            "aggregates":{"type":"array","description":"Omit for detailed rows, even when a table needs totals. All datasets return totals separately.","items":{"type":"object","additionalProperties":false,"properties":{"op":{"type":"string","enum":["count","sum","avg","min","max","median","distinct"]},"property":string},"required":["op"]}},
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
    fn identifiers_cannot_invent_displayed_numeric_spans() {
        for kind in ["Tabs", "Accordion", "Text"] {
            let mut spec = json!({"root":"999999","elements":{"999999":{"type":kind,"props":{}}}});
            if kind == "Text" { spec["elements"]["999999"]["props"] = json!({"text":"Words"}); }
            assert!(validate(&spec, None).unwrap_err().message.contains("identifier"));
        }
        for (key, value) in [
            ("x", json!("999999")), ("y", json!("999999")), ("group", json!("999999")),
            ("size", json!("999999")), ("parent", json!("999999")), ("series", json!(["999999"])),
            ("columns", json!([{"key":"999999","label":"Words"}])), ("formats", json!({"999999":"number"})),
        ] {
            let mut spec = json!({"root":"chart","elements":{"chart":{"type":"Line","props":{"title":"Values","summary":"Values","data":{"$state":"/sources/sales/rows"}}}}});
            spec["elements"]["chart"]["props"][key] = value;
            assert!(validate(&spec, None).unwrap_err().message.contains("identifier"), "{key}");
        }
        let data = json!({"sources":{"999999":{"rows":[]}}});
        let shown = text(&json!({}), &data);
        assert!(shown.contains("Source 1:"));
        assert!(!shown.contains("999999"));
        // Aggregate keys add an operation prefix to a valid property of up to 64 bytes.
        let key = format!("sum_{}", "a".repeat(64));
        let spec = json!({"root":"chart","elements":{"chart":{"type":"Line","props":{"title":"Values","summary":"Values","data":{"$state":"/sources/sales/rows"},"y":key}}}});
        assert!(validate(&spec, None).is_ok());
    }
    #[test]
    fn refuses_literals_unknown_props_missing_bindings_and_cycles() {
        let mut spec = json!({"root":"a","elements":{"a":{"type":"Stat","props":{"title":"Total","summary":"Total","data":{"$state":"/sources/a/metric/value"}}}}});
        assert!(validate(&spec, Some(&json!({"sources":{"a":{"view":"metric","metric":{"value":12}}}}))).is_ok());
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
    fn numeric_bypass_matrix() {
        let data = json!({"sources":{"s":{"title":"999","rows":[{"name":"999","amount":12,"other":"999","min":0,"max":20}],"columns":[{"key":"name","datatype":"text"},{"key":"amount","datatype":"number"},{"key":"other","datatype":"text"},{"key":"min","datatype":"number"},{"key":"max","datatype":"number"}],"distributions":{"amount":{"box":[{"min":12,"q1":12,"median":12,"q3":12,"max":12}]}}}}});
        let make = |kind: &str, path: &str, extra: Value| {
            let mut props = json!({"title":"Values","summary":"Approved values","data":{"$state":path}});
            props.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            json!({"root":"a","elements":{"a":{"type":kind,"props":props}}})
        };
        assert!(validate(&make("Stat", "/sources/s/rows/0/amount", json!({})), Some(&data)).is_ok());
        for path in ["/sources/s/title", "/sources/s/rows/0/name"] { assert!(validate(&make("Stat",path,json!({})),Some(&data)).is_err(), "{path}"); }
        for kind in ["Line","Area","Bar","StackedBar","Pie","Donut","Radar","Funnel","Waterfall","Sparkline","Gauge","Bullet","Heatmap","CalendarHeatmap","Combo","SmallMultiples"] {
            assert!(validate(&make(kind,"/sources/s/rows",json!({"x":"name","y":"name"})),Some(&data)).is_err(),"{kind}");
        }
        for (kind, extra) in [("Scatter",json!({"x":"name","y":"amount"})),("Bubble",json!({"x":"amount","y":"amount","size":"name"})),("Line",json!({"x":"name","y":"amount","series":["other"]})),("Table",json!({"columns":[{"key":"name","label":"Name","format":"number"}]})),("Table",json!({"columns":[{"key":"name","label":"Name","total":"sum"}]}))] {
            assert!(validate(&make(kind,"/sources/s/rows",extra),Some(&data)).is_err(),"{kind}");
        }
        for kind in ["Histogram","BoxPlot","Violin","Tree","Treemap","Partition","Network","Sankey","Meter","Comparison"] {
            assert!(validate(&make(kind,"/sources/s/rows",json!({"x":"name","y":"amount"})),Some(&data)).is_err(),"{kind}");
        }
        assert!(validate(&make("BoxPlot","/sources/s/distributions/amount/box",json!({})),Some(&data)).is_ok());
        for field in ["title","summary"] {
            let mut spec = make("Stat","/sources/s/rows/0/amount",json!({})); spec["elements"]["a"]["props"][field] = json!("Total 998");
            assert!(validate(&spec,Some(&data)).is_err());
        }
        for kind in ["Text","Caption","Badge","Callout","Heading"] {
            assert!(validate(&json!({"root":"a","elements":{"a":{"type":kind,"props":{"text":"999"}}}}),Some(&json!({}))).is_err());
        }
    }
    #[test]
    fn omitted_y_resolves_only_unambiguous_metadata() {
        let mut spec = json!({"root":"a","elements":{"a":{"type":"Line","props":{"title":"Amount","summary":"Amount","data":{"$state":"/sources/s/rows"},"x":"name"}}}});
        let mut data = json!({"sources":{"s":{"rows":[{"name":"Supplier","amount":null,"quantity":12}],"columns":[{"key":"amount","datatype":"money"},{"key":"quantity","datatype":"number"}]}}});
        assert!(resolve_columns(&mut spec, &data).unwrap_err().message.contains("name y"));
        data["sources"]["s"]["columns"] = json!([{"key":"amount","datatype":"money"}]);
        resolve_columns(&mut spec, &data).unwrap();
        assert_eq!(spec["elements"]["a"]["props"]["y"], "amount");
        assert_eq!(spec["elements"]["a"]["props"]["series"], json!(["amount"]));
        assert!(validate(&spec,Some(&data)).is_ok());
    }
    #[test]
    fn prose_prompt_contract_has_no_value_matching_exemption() {
        let prompt = description();
        assert!(prompt.contains("{{/sources/sales/total_rows}}"));
        assert!(prompt.contains("Raw digits"));
        assert!(prompt.contains("literal plain text"));
        assert!(prompt.contains("predating the turn"));
        assert!(!prompt.contains("digits must match"));
        assert!(!prompt.contains("digit quantities must match"));
        assert!(!prompt.contains("Alphanumeric identifiers such as Q3 are allowed"));
    }
    #[test]
    fn rejects_caps() {
        let spec = json!({"root":"a","elements":{"a":{"type":"Text","props":{"text":"a".repeat(MAX_BYTES)}}}});
        assert!(validate(&spec, None).is_err());
    }
}
