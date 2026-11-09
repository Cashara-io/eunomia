//! Permission-checked object directory and profile. No whole-kind values leave the list query.
use std::collections::{BTreeMap, BTreeSet};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use surrealdb::types::{RecordId, SurrealValue};
use crate::{authz::Action, error::{AppError, AppResult}, pool::OrgDb, rid::RecordIdExt, state::OrgState, store::objects as q};
use super::{Kind, metric, property::{self, Datatype, PropertyRow}};

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListArgs {
    pub kind: String,
    pub search: Option<String>,
    pub sort: Option<String>,
    #[serde(default)] pub descending: bool,
    pub filter: Option<String>,
    pub op: Option<String>,
    pub value: Option<String>,
    #[serde(default)] pub offset: usize,
    pub limit: Option<usize>,
}
#[derive(Deserialize, SurrealValue)]
struct PageRow { id: RecordId, name: String }
#[derive(Deserialize, SurrealValue)]
struct Page { total: usize, rows: Vec<PageRow> }
#[derive(Deserialize, SurrealValue)]
struct SourceRow { entity: RecordId, sources: Vec<RecordId> }
#[derive(Serialize)]
pub struct Column { name: String, datatype: String, unit: Option<String> }

async fn metadata(db: &OrgDb, vault: &RecordId, kind: &str) -> AppResult<(Vec<PropertyRow>, bool)> {
    let mut res = q::META.on(db).bind(("vault", vault.clone())).bind(("kind", kind.to_string())).await?;
    let props = res.take(0)?;
    let kinds: Vec<Value> = res.take(1)?;
    Ok((props, kinds.iter().any(|k| k["status"] == "approved")))
}
fn column(p: &PropertyRow) -> Column { Column { name: p.name.clone(), datatype: p.datatype.clone(), unit: p.unit.clone() } }

pub async fn list(db: &OrgDb, user: &RecordId, vault: &RecordId, args: &ListArgs) -> AppResult<Value> {
    let vault = super::resolve(db, user, Some(vault), Action::ReadMemories).await?;
    let kind = Kind::parse(&args.kind)?;
    let table = crate::entities::service::kind_table(kind.as_str())?;
    let limit = args.limit.unwrap_or(25);
    if !(1..=100).contains(&limit) || args.offset > 1_000_000 || args.search.as_ref().is_some_and(|s| s.len() > 200) {
        return Err(AppError::bad_request("use 1 to 100 rows, an offset below 1000000 and a search below 200 characters"));
    }
    let (props, approved) = metadata(db, &vault, kind.as_str()).await?;
    let find = |name: &str| props.iter().find(|p| p.name == name).ok_or_else(|| AppError::bad_request("choose an approved property"));
    let sort = args.sort.as_deref().unwrap_or("name");
    let sort_property = if sort == "name" { None } else { Some(find(sort)?.id.clone()) };
    let filter = args.filter.as_deref().map(find).transpose()?;
    let op = args.op.as_deref().unwrap_or("");
    if !["", "eq", "gte", "lte", "missing"].contains(&op) || (op != "" && filter.is_none()) {
        return Err(AppError::bad_request("filter needs an approved property and eq, gte, lte or missing"));
    }
    let value = if op == "" || op == "missing" { Value::Null } else {
        let p = filter.ok_or_else(|| AppError::bad_request("filter property missing"))?;
        let text = args.value.as_deref().ok_or_else(|| AppError::bad_request("filter value missing"))?;
        let datatype = Datatype::parse(&p.datatype)?;
        let stored = property::parse_value(datatype, &p.name, None, &json!(text))?;
        property::to_json(datatype, &stored).ok_or_else(|| AppError::bad_request("invalid filter value"))?
    };
    let page: Page = q::PAGE.on(db).bind(("vault", vault.clone())).bind(("table", table.to_string())).bind(("kind", kind.as_str().to_string()))
        .bind(("search", args.search.as_deref().unwrap_or("").to_lowercase())).bind(("sort_name", sort == "name"))
        .bind(("sort_property", sort_property)).bind(("filter_property", filter.map(|p| p.id.clone())))
        .bind(("filter_op", op.to_string())).bind(("filter_value", value)).bind(("descending", args.descending))
        .bind(("offset", args.offset)).bind(("limit", limit)).await?.take(3)?;
    let ids: Vec<RecordId> = page.rows.iter().map(|r| r.id.clone()).collect();
    let names: BTreeSet<String> = props.iter().map(|p| p.name.clone()).collect();
    let values = metric::latest_values(db, &vault, &kind, &names, &ids, chrono::Utc::now()).await?;
    let sources: Vec<SourceRow> = q::SOURCES.on(db).bind(("vault", vault.clone())).bind(("ids", ids)).await?.take(0)?;
    let sources: BTreeMap<String, usize> = sources.into_iter().map(|r| (r.entity.to_string(), r.sources.len())).collect();
    let rows: Vec<Value> = page.rows.into_iter().map(|r| {
        let id = r.id.to_string();
        let held = values.get(&id);
        let cells: BTreeMap<String, Value> = props.iter().map(|p| {
            let h = held.and_then(|h| h.get(&p.name));
            let v = h.and_then(|h| property::to_json(Datatype::parse(&p.datatype).ok()?, &h.stored)).unwrap_or(Value::Null);
            (p.name.clone(), json!({"value":v,"unit":h.and_then(|h|h.unit.clone()).or_else(||p.unit.clone())}))
        }).collect();
        json!({"id":id,"name":r.name,"values":cells,"source_count":sources.get(&id).copied().unwrap_or(0)})
    }).collect();
    Ok(json!({"rows":rows,"total":page.total,"offset":args.offset,"limit":limit,"columns":props.iter().map(column).collect::<Vec<_>>(),"kind_approved":approved}))
}

pub async fn profile(state: &OrgState, user: &RecordId, vault: &RecordId, id: &RecordId, offset: usize) -> AppResult<Value> {
    let vault = super::resolve(&state.db, user, Some(vault), Action::ReadMemories).await?;
    crate::entities::service::require_entity_id(id)?;
    let node: crate::entities::neighbourhood::NodeRow = crate::store::get(&state.db, id).await?.ok_or_else(|| AppError::not_found("object unavailable"))?;
    if node.vault != vault { return Err(AppError::not_found("object unavailable")); }
    let kind = Kind::parse(&node.kind())?;
    let (props, approved) = metadata(&state.db, &vault, kind.as_str()).await?;
    let mut res = q::HISTORY.on(&state.db).bind(("vault", vault.clone())).bind(("id", id.clone())).bind(("offset", offset)).await?;
    let mut history: Vec<Value> = res.take(0)?;
    let mut links: Vec<Value> = res.take(1)?;
    let mut memories: Vec<Value> = res.take(2)?;
    let totals: Vec<Value> = res.take(3)?;
    let link_counts: Vec<Value> = res.take(4)?;
    let more = json!({"history":history.len()>100,"links":links.len()>100,"memories":memories.len()>100});
    history.truncate(100); links.truncate(100); memories.truncate(100);
    let current: Vec<Value> = q::CURRENT.on(&state.db).bind(("vault",vault.clone())).bind(("id",id.clone()))
        .bind(("props",props.iter().map(|p|p.id.clone()).collect::<Vec<_>>())).await?.take(0)?;
    let properties: Vec<Value> = props.iter().map(|p| {
        let rows: Vec<&Value> = current.iter().filter(|v|v["property"] == p.name).collect();
        let v = rows.first().copied();
        let value = v.map(|v| match p.datatype.as_str() {
            "number"|"integer"|"money"|"percent" => v["value_num"].clone(),
            "bool"=>v["value_bool"].clone(),
            "date"|"datetime"=> if v["value_date"] == "NONE" { Value::Null } else {v["value_date"].clone()},
            _=>v["value_text"].clone(),
        }).unwrap_or(Value::Null);
        let conflicting = rows.get(1).is_some_and(|other|v.is_some_and(|v|v["as_of"] == other["as_of"] && ["value_num","value_text","value_bool","value_date","unit"].iter().any(|k|v[*k]!=other[*k])));
        let source = v.and_then(|v|v["source"].as_str()).filter(|s|*s!="NONE" && !s.is_empty());
        let state = if v.is_none() {"missing"} else if conflicting {"conflicting"} else if value.is_null() {"unavailable"} else {"recorded"};
        json!({"name":p.name,"datatype":p.datatype,"unit":v.and_then(|v|v["unit"].as_str()).map(String::from).or_else(||p.unit.clone()),"value":value,"as_of":v.map(|v|&v["as_of"]),"source":source,"state":state})
    }).collect();
    let source_ids: BTreeSet<String> = properties.iter().filter_map(|v|v["source"].as_str().map(String::from)).chain(history.iter().filter_map(|h|h["source"].as_str().filter(|s|!s.is_empty() && *s != "NONE").map(String::from))).collect();
    let ids: Vec<RecordId> = source_ids.iter().filter_map(|s|crate::rid::parse(s).ok()).collect();
    let records: Vec<Value> = q::SOURCE_RECORDS.on(&state.db).bind(("vault", vault.clone())).bind(("ids", ids)).await?.take(0)?;
    let mut sources: Vec<Value> = source_ids.iter().map(|source| {
        let record = records.iter().find(|r|r["id"] == *source);
        json!({"id":source,"record":record,"state":if record.is_some(){"available"}else{"unavailable"}})
    }).collect();
    // Imported values retain the document, but older imports did not retain a row number.
    // A unique name match locates the current file's row without inventing historical provenance.
    for source in &mut sources {
        let Some(source_id) = source["id"].as_str().filter(|s|s.starts_with("document:")).map(String::from) else { continue };
        match crate::documents::load_csv(state, user, &source_id, Action::ReadMemories).await {
            Ok(doc) if doc.vault == vault => {
                let mut reader = csv::ReaderBuilder::new().flexible(true).from_reader(doc.bytes.as_slice());
                let headers = reader.headers().map(|h|h.iter().map(String::from).collect::<Vec<_>>()).unwrap_or_default();
                let matches: Vec<_> = reader.records().enumerate().filter_map(|(i,row)| {
                    let row = row.ok()?;
                    if !row.iter().any(|cell|cell.trim().eq_ignore_ascii_case(&node.name)) { return None; }
                    Some(json!({"line":i+2,"cells":headers.iter().zip(row.iter()).map(|(h,c)|(h.clone(),json!(c))).collect::<BTreeMap<_,_>>()}))
                }).take(2).collect();
                source["document"] = json!({"id":source_id,"filename":doc.filename,"row":if matches.len()==1 {matches.first().cloned()} else {None},"note":"Current document. Original imported row number and revision were not retained; this row is matched by object name."});
                source["state"] = json!("available");
            }
            Ok(_) => {},
            Err(e) if e.status.is_server_error() => return Err(e),
            Err(_) => {},
        }
    }
    let metrics = metric::metrics_by_kind(&state.db, &vault).await?.remove(kind.as_str()).unwrap_or_default();
    let mut numbers = Vec::new();
    for m in metrics.iter().filter(|m|m.status == super::Status::Approved).take(24) {
        let args = metric::EvalArgs { metric:m.name.clone(),kind:Some(kind.as_str().into()),vault_id:Some(vault.to_string()),..Default::default() };
        let run = match metric::eval_object(&state.db, &vault, &args, id).await { Ok(run)=>json!({"run":run}),Err(e)=>json!({"unavailable":e.message}) };
        numbers.push(json!({"name":m.name,"definition":m.expr_words,"result":run}));
    }
    Ok(json!({"id":id.to_string(),"name":node.name,"kind":kind.as_str(),"summary":node.summary,"kind_approved":approved,
        "properties":properties,"numbers":numbers,"links":links,"link_counts":link_counts,"history":history,"history_total":totals.first().map(|v|&v["total"]),"sources":sources,"memories":memories,"more":more,"offset":offset,
        "warnings":["Approval covers the kind definition, not every value. Freshness rules and review history are not recorded. Imported values may be updated in place; this is not a complete change history."]}))
}
