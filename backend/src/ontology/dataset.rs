//! Bounded, typed datasets for generative UI. Reuses ontology access and filter checks.
use std::collections::{BTreeMap, HashMap};
use chrono::Datelike;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use surrealdb::types::{RecordId, SurrealValue};
#[derive(Debug, Deserialize, SurrealValue)]
struct LinkRow { entity: RecordId, target: RecordId, label: String }
use super::{Kind, Status, ViewStatus, metric, property};
use crate::{authz::Action, error::{AppError, AppResult}, pool::OrgDb, rid::RecordIdExt};

pub const MAX_ROWS: usize = 500;
const MAX_INPUT: usize = 5000;
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dataset {
    pub vault_id: String,
    pub kind: String,
    pub properties: Vec<String>,
    #[serde(default)] pub filters: Vec<metric::Filter>,
    pub group_by: Option<String>,
    pub bucket: Option<Bucket>,
    #[serde(default)] pub aggregates: Vec<Aggregate>,
    pub sort: Option<Sort>,
    pub limit: Option<usize>,
}
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all="snake_case")]
pub enum Bucket { Day, Week, Month, Quarter, Year }
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sort { pub property: String, #[serde(default)] pub descending: bool }
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Aggregate { pub name: String, pub op: Agg, pub property: Option<String> }
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all="snake_case")]
pub enum Agg { Count, Sum, Avg, Min, Max, Median, Distinct }
#[derive(Debug, Clone, Serialize)]
pub struct Column { pub key: String, pub datatype: String, pub unit: Option<String> }

fn bad(s: impl Into<String>) -> AppError { AppError::bad_request(s) }
pub async fn resolve(db: &OrgDb, owner: &RecordId, args: &Dataset) -> AppResult<Value> {
    if args.properties.len() > 24 || args.aggregates.len() > 24 || args.limit.is_some_and(|n| n == 0 || n > MAX_ROWS) {
        return Err(bad("datasets allow 24 columns and aggregates and 1 to 500 output rows"));
    }
    let vault = crate::rid::parse(&args.vault_id).map_err(|_| bad("invalid dataset vault_id"))?;
    let vault = super::resolve(db, owner, Some(&vault), Action::ReadMemories).await?;
    let kind = Kind::parse(&args.kind)?;
    let ontology = super::get(db, owner, Some(&vault), Some(kind.as_str())).await?;
    let k = ontology.kinds.iter().find(|k| k.kind == args.kind).ok_or_else(|| bad("unknown dataset kind"))?;
    let approved = k.status == ViewStatus::Approved;
    if ontology.strict && !approved && !k.built_in { return Err(bad("strict mode requires an approved dataset kind")); }
    let needed = args.properties.iter().chain(args.group_by.iter()).chain(args.aggregates.iter().filter_map(|a| a.property.as_ref()));
    let mut columns = vec![];
    for name in needed {
        if columns.iter().any(|c: &Column| &c.key == name) { continue; }
        if let Some(label) = name.strip_prefix("link:") {
            let link = ontology.links.iter().find(|l| l.from_kind == args.kind && l.label == label).ok_or_else(|| bad("unknown dataset link"))?;
            if link.status != ViewStatus::Approved { return Err(bad("dataset link is not approved")); }
            columns.push(Column { key: name.clone(), datatype: "link".into(), unit: None });
            continue;
        }
        let p = k.properties.iter().find(|p| &p.name == name).ok_or_else(|| bad(format!("unknown dataset property {name}")))?;
        if p.status != Status::Approved { return Err(bad(format!("dataset property {name} is not approved"))); }
        columns.push(Column { key: name.clone(), datatype: p.datatype.as_str().into(), unit: p.unit.clone() });
    }
    if args.bucket.is_some() && !columns.iter().any(|c| Some(&c.key) == args.group_by.as_ref() && matches!(c.datatype.as_str(), "date" | "datetime")) {
        return Err(bad("time bucket needs a date or datetime group_by property"));
    }
    let selector = metric::Selector::new(db, &vault, &kind, &args.filters).await?;
    let mut ids: Vec<RecordId> = crate::store::ontology::UI_ENTITIES.on(db)
        .bind(("vault", vault.clone())).bind(("kind", args.kind.clone()))
        .bind(("table", crate::entities::service::kind_table(&args.kind)?.to_string())).await?.take(0)?;
    let input_truncated = ids.len() > MAX_INPUT;
    ids.truncate(MAX_INPUT);
    let links: Vec<LinkRow> = if columns.iter().any(|c| c.datatype == "link") {
        crate::store::ontology::UI_LINKS.on(db).bind(("vault", vault.clone())).bind(("ids", ids.clone())).await?.take(0)?
    } else { Vec::new() };
    if links.len() > MAX_INPUT { return Err(bad("dataset link targets exceed 5000, narrow the dataset")); }
    let target_ids: Vec<RecordId> = links.iter().map(|l| l.target.clone()).collect();
    let targets = crate::entities::neighbourhood::load_nodes(db, target_ids).await?;
    let values = property::latest_values(db, &vault, &ids).await?;
    let nodes = crate::entities::neighbourhood::load_nodes(db, ids.clone()).await?;
    let mut rows = Vec::new();
    for id in ids {
        let id = id.to_string();
        let held = values.get(&id).into_iter().flatten().map(|p| Ok((p.property.clone(), metric::Held {
            stored: property::parse_value(if matches!(p.datatype, property::Datatype::Enum) { property::Datatype::Text } else { p.datatype }, &p.property, None, &p.value)?, unit: p.unit.clone(),
        }))).collect::<AppResult<HashMap<_, _>>>()?;
        if !selector.picks(|p| held.get(p)) { continue; }
        let mut row = serde_json::Map::new();
        row.insert("entity_id".into(), json!(id));
        row.insert("name".into(), json!(nodes.get(&id).map(|n| n.name.as_str()).unwrap_or(&id)));
        for c in &columns {
            if let Some(label) = c.key.strip_prefix("link:") {
                let refs: Vec<Value> = links.iter().filter(|l| l.entity.to_string() == id && l.label == label).map(|l| json!({"entity_id":l.target.to_string(),"name":targets.get(&l.target.to_string()).map(|n|n.name.as_str()).unwrap_or("")})).collect();
                row.insert(c.key.clone(), json!(refs));
                continue;
            }
            let v = values.get(&id).into_iter().flatten().find(|p| p.property == c.key).map(|p| p.value.clone()).unwrap_or(Value::Null);
            row.insert(c.key.clone(), v);
        }
        if let Some(group) = args.group_by.as_ref().filter(|p| p.starts_with("link:")) {
            let refs = row.get(group).and_then(Value::as_array).cloned().unwrap_or_default();
            if refs.is_empty() { row.insert(group.clone(), Value::Null); rows.push(Value::Object(row)); }
            else { for target in refs { let mut r = row.clone(); r.insert(group.clone(), target); rows.push(Value::Object(r)); } }
        } else { rows.push(Value::Object(row)); }
        if rows.len() > MAX_INPUT { return Err(bad("expanded dataset exceeds 5000 rows")); }
    }
    let (mut rows, columns) = transform(rows, columns, args)?;
    if let Some(sort) = &args.sort {
        if !columns.iter().any(|c| c.key == sort.property) && sort.property != "name" { return Err(bad("unknown sort column")); }
        rows.sort_by(|a,b| { let order = compare(&a[&sort.property], &b[&sort.property]); if sort.descending { order.reverse() } else { order } });
    }
    let totals: serde_json::Map<String, Value> = columns.iter().map(|c| {
        let ns: Vec<f64> = rows.iter().filter_map(|r| r[&c.key].as_f64()).collect();
        let sum = ns.iter().sum::<f64>();
        (c.key.clone(), json!({"sum": if ns.is_empty() { None } else { Some(sum) }, "avg": if ns.is_empty() { None } else { Some(sum / ns.len() as f64) }, "count": ns.len()}))
    }).collect();
    let distributions: serde_json::Map<String, Value> = columns.iter().filter(|c| matches!(c.datatype.as_str(), "money" | "number" | "integer" | "percent")).map(|c| {
        let mut ns: Vec<f64> = rows.iter().filter_map(|r| r[&c.key].as_f64()).collect();
        ns.sort_by(f64::total_cmp);
        let n = ns.len();
        let value = if n == 0 { json!({"bins":[],"box":[],"density":[]}) } else {
            let lo = ns[0]; let hi = ns[n-1];
            let count = ((n as f64).sqrt().ceil() as usize).clamp(1,10);
            let step = (if hi == lo { 1.0 } else { hi-lo }) / count as f64;
            let mut counts = vec![0usize; count];
            for v in &ns { counts[(((v-lo)/step).floor() as usize).min(count-1)] += 1; }
            let quantile = |q: f64| { let i = (n-1) as f64 * q; let a = i.floor() as usize; ns[a] + (ns[(a+1).min(n-1)]-ns[a]) * i.fract() };
            let bins: Vec<Value> = counts.iter().enumerate().map(|(i,count)| json!({"start":lo+i as f64*step,"end":lo+(i+1) as f64*step,"count":count})).collect();
            let density: Vec<Value> = counts.iter().enumerate().map(|(i,count)| json!({"value":lo+(i as f64+0.5)*step,"count":count})).collect();
            json!({"bins":bins,"box":[{"min":lo,"max":hi,"q1":quantile(0.25),"median":quantile(0.5),"q3":quantile(0.75)}],"density":density})
        };
        (c.key.clone(), value)
    }).collect();
    let total_rows = rows.len();
    rows.truncate(args.limit.unwrap_or(MAX_ROWS));
    let truncated = input_truncated || rows.len() < total_rows;
    Ok(json!({ "rows": rows, "columns": columns, "totals": totals, "distributions": distributions, "total_rows": total_rows, "truncated": truncated,
        "status": if approved { "approved" } else { "not approved" },
        "warnings": if input_truncated { vec!["Input capped at 5000 entities. Aggregates describe only this partial input."] } else if truncated { vec!["Output rows truncated."] } else { vec![] } }))
}
fn compare(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a.as_f64(), b.as_f64()) { (Some(a),Some(b)) => a.total_cmp(&b), _ => a.to_string().cmp(&b.to_string()) }
}
fn period(value: &Value, bucket: Bucket) -> AppResult<Value> {
    if value.is_null() { return Ok(Value::Null); }
    let s = value.as_str().ok_or_else(|| bad("date value must be text"))?;
    let date = chrono::NaiveDate::parse_from_str(s.get(..10).unwrap_or(s), "%Y-%m-%d").map_err(|_| bad("invalid date value"))?;
    Ok(json!(match bucket {
        Bucket::Day => date.to_string(), Bucket::Week => date.format("%G-W%V").to_string(),
        Bucket::Month => date.format("%Y-%m").to_string(), Bucket::Quarter => format!("{}-Q{}", date.year(), date.month0()/3+1),
        Bucket::Year => date.year().to_string(),
    }))
}
fn transform(rows: Vec<Value>, columns: Vec<Column>, args: &Dataset) -> AppResult<(Vec<Value>, Vec<Column>)> {
    if args.aggregates.is_empty() {
        if args.group_by.is_some() { return Err(bad("group_by needs aggregates")); }
        return Ok((rows, columns));
    }
    let mut groups: BTreeMap<String, (Value, Vec<Value>)> = BTreeMap::new();
    for row in rows {
        let mut key = args.group_by.as_ref().map(|p| row[p].clone()).unwrap_or(Value::Null);
        if let Some(bucket) = args.bucket { key = period(&key, bucket)?; }
        groups.entry(key.to_string()).or_insert_with(|| (key, vec![])).1.push(row);
    }
    if groups.is_empty() && args.group_by.is_none() { groups.insert("null".into(), (Value::Null, vec![])); }
    let mut out_columns: Vec<Column> = columns.iter().filter(|c| Some(&c.key) == args.group_by.as_ref()).cloned().collect();
    for a in &args.aggregates {
        property::PropertyName::parse(&a.name)?;
        if out_columns.iter().any(|c| c.key == a.name) { return Err(bad("duplicate aggregate name")); }
        let source = a.property.as_ref().and_then(|p| columns.iter().find(|c| &c.key == p));
        if !matches!(a.op, Agg::Count) && source.is_none() { return Err(bad("aggregate needs a property")); }
        if matches!(a.op, Agg::Sum | Agg::Avg | Agg::Min | Agg::Max | Agg::Median) && !source.is_some_and(|c| matches!(c.datatype.as_str(), "money" | "number" | "integer" | "percent")) { return Err(bad("numeric aggregate needs numeric property")); }
        let count = matches!(a.op, Agg::Count | Agg::Distinct);
        out_columns.push(Column { key: a.name.clone(), datatype: if count { "number".into() } else { source.unwrap().datatype.clone() }, unit: if count { None } else { source.and_then(|c| c.unit.clone()) } });
    }
    let rows = groups.into_values().map(|(key, rows)| {
        let mut out = serde_json::Map::new();
        if let Some(p) = &args.group_by { out.insert(p.clone(), key); }
        for a in &args.aggregates {
            let values: Vec<&Value> = rows.iter().filter_map(|r| a.property.as_ref().map(|p| &r[p])).filter(|v| !v.is_null()).collect();
            let mut nums: Vec<f64> = values.iter().filter_map(|v| v.as_f64()).collect(); nums.sort_by(f64::total_cmp);
            let n = nums.len();
            let value = match a.op {
                Agg::Count => json!(if a.property.is_some() { values.len() } else { rows.len() }),
                Agg::Distinct => json!(values.iter().map(|v| v.to_string()).collect::<std::collections::HashSet<_>>().len()),
                _ if n == 0 => Value::Null,
                Agg::Sum => json!(nums.iter().sum::<f64>()), Agg::Avg => json!(nums.iter().sum::<f64>() / n as f64),
                Agg::Min => json!(nums[0]), Agg::Max => json!(nums[n-1]),
                Agg::Median => json!(if n.is_multiple_of(2) { nums[n/2-1]/2.0 + nums[n/2]/2.0 } else { nums[n/2] }),
            };
            out.insert(a.name.clone(), value);
        }
        Value::Object(out)
    }).collect();
    Ok((rows, out_columns))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_grouping_nulls_and_all_aggregates() {
        let rows = vec![json!({"supplier":"A","amount":10.25}),json!({"supplier":"A","amount":20.75}),json!({"supplier":"A","amount":null}),json!({"supplier":"B","amount":9})];
        let args: Dataset = serde_json::from_value(json!({"vault_id":"vault:test","kind":"sample","properties":["supplier","amount"],"group_by":"supplier","aggregates":[
            {"name":"sum","op":"sum","property":"amount"},{"name":"avg","op":"avg","property":"amount"},{"name":"min","op":"min","property":"amount"},{"name":"max","op":"max","property":"amount"},{"name":"median","op":"median","property":"amount"},{"name":"distinct","op":"distinct","property":"amount"},{"name":"count","op":"count"}
        ]})).unwrap();
        let columns = vec![Column{key:"supplier".into(),datatype:"text".into(),unit:None},Column{key:"amount".into(),datatype:"money".into(),unit:Some("AUD".into())}];
        let (rows, cols) = transform(rows,columns,&args).unwrap();
        assert_eq!(rows[0],json!({"supplier":"A","sum":31.0,"avg":15.5,"min":10.25,"max":20.75,"median":15.5,"distinct":2,"count":3}));
        assert_eq!(cols[1].unit.as_deref(),Some("AUD"));
        assert_eq!(cols[6].unit,None);
    }
    #[test]
    fn every_time_bucket_and_null() {
        for (bucket,want) in [(Bucket::Day,"2026-11-09"),(Bucket::Week,"2026-W46"),(Bucket::Month,"2026-11"),(Bucket::Quarter,"2026-Q4"),(Bucket::Year,"2026")] {
            assert_eq!(period(&json!("2026-11-09T10:00:00Z"),bucket).unwrap(),json!(want));
            assert_eq!(period(&Value::Null,bucket).unwrap(),Value::Null);
        }
    }
}
