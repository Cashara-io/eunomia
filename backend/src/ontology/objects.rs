//! Permission-checked object directory and profile. No whole-kind values leave the list query.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use surrealdb::types::{RecordId, SurrealValue};
use crate::{authz::Action, error::{AppError, AppResult}, pool::OrgDb, rid::RecordIdExt, state::OrgState, store::objects as q};
use super::{Kind, metric, property::{self, Datatype, PropertyRow}};

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListArgs {
    #[serde(default)] pub kind: String,
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
struct SortValue {
    entity: RecordId, property: RecordId,
    value_num: Option<f64>, value_text: Option<String>, value_bool: Option<bool>, value_date: Option<surrealdb::types::Datetime>,
}
impl SortValue {
    fn value(&self, datatype:&str) -> Value {
        match datatype {
            "number"|"integer"|"money"|"percent"=>self.value_num.map(|n|json!(n)),
            "bool"=>self.value_bool.map(Value::Bool),
            "date"|"datetime"=>self.value_date.map(|d|json!(d.into_inner().to_rfc3339_opts(chrono::SecondsFormat::AutoSi,true))),
            "text"|"enum"=>self.value_text.clone().map(Value::String),
            _=>None,
        }.unwrap_or(Value::Null)
    }
}
fn compare(a:&Value,b:&Value) -> std::cmp::Ordering {
    match (a,b) {
        (Value::Number(a),Value::Number(b))=>a.as_f64().unwrap_or(0.0).total_cmp(&b.as_f64().unwrap_or(0.0)),
        (Value::String(a),Value::String(b))=>a.cmp(b),
        (Value::Bool(a),Value::Bool(b))=>a.cmp(b),
        _=>std::cmp::Ordering::Equal,
    }
}
#[derive(Deserialize, SurrealValue)]
struct LinkMeta { label: String, to_kind: String }
#[derive(Deserialize, SurrealValue)]
struct SourceRow { entity: RecordId, sources: Vec<RecordId> }
#[derive(Serialize)]
pub struct Column { name: String, datatype: String, unit: Option<String> }

async fn metadata(db: &OrgDb, vault: &RecordId, kind: &str) -> AppResult<(Vec<PropertyRow>, bool, Vec<LinkMeta>)> {
    let mut res = q::META.on(db).bind(("vault", vault.clone())).bind(("kind", kind.to_string())).await?;
    let props = res.take(0)?;
    let kinds: Vec<Value> = res.take(1)?;
    let links: Vec<LinkMeta> = res.take(2)?;
    Ok((props, kinds.iter().any(|k| k["status"] == "approved"), links))
}
fn column(p: &PropertyRow) -> Column { Column { name: p.name.clone(), datatype: p.datatype.clone(), unit: p.unit.clone() } }

pub async fn list(db: &OrgDb, user: &RecordId, vault: &RecordId, args: &ListArgs) -> AppResult<Value> {
    let vault = super::resolve(db, user, Some(vault), Action::ReadMemories).await?;
    if args.kind.trim().is_empty() {
        let mut result=q::KINDS.on(db).bind(("vault",vault.clone())).await?;
        let counts:Vec<Value>=result.take(0)?;
        let registered:Vec<Value>=result.take(1)?;
        let mut kinds:BTreeMap<String,Value>=counts.into_iter().filter_map(|k|Some((k["kind"].as_str()?.to_string(),k))).collect();
        for k in &registered {
            if let Some(name)=k["kind"].as_str() {kinds.entry(name.to_string()).or_insert_with(||json!({"kind":name,"entities":0}));}
        }
        let kinds:Vec<_>=kinds.into_values().filter(|k|k["entities"].as_u64().unwrap_or(0)>0 || registered.iter().any(|r|r["kind"]==k["kind"])).collect();
        return Ok(json!({"kinds":kinds}));
    }
    let kind = Kind::parse(&args.kind)?;
    let table = crate::entities::service::kind_table(kind.as_str())?;
    let limit = args.limit.unwrap_or(25);
    if !(1..=100).contains(&limit) || args.offset > 1_000_000 || args.search.as_ref().is_some_and(|s| s.chars().count() > 200) {
        return Err(AppError::bad_request("use 1 to 100 rows, an offset below 1000000 and a search below 200 characters"));
    }
    let (props, approved, link_types) = metadata(db, &vault, kind.as_str()).await?;
    let labels:Vec<_>=link_types.iter().map(|l|l.label.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    let find = |name: &str| props.iter().find(|p| p.name == name).ok_or_else(|| AppError::bad_request("choose an approved property"));
    let sort = args.sort.as_deref().unwrap_or("name");
    let sort_property = if sort == "name" { None } else { Some(find(sort)?.id.clone()) };
    let filter = args.filter.as_deref().map(find).transpose()?;
    let op = args.op.as_deref().unwrap_or("");
    if !["", "eq", "gte", "lte", "missing"].contains(&op) || (!op.is_empty() && filter.is_none()) {
        return Err(AppError::bad_request("filter needs an approved property and eq, gte, lte or missing"));
    }
    let value = if op.is_empty() || op == "missing" { Value::Null } else {
        let p = filter.ok_or_else(|| AppError::bad_request("filter property missing"))?;
        let text = args.value.as_deref().ok_or_else(|| AppError::bad_request("filter value missing"))?;
        let datatype = Datatype::parse(&p.datatype)?;
        let stored = property::parse_value(datatype, &p.name, None, &json!(text))?;
        if let Some(date) = stored.date { json!(date.into_inner().to_rfc3339_opts(chrono::SecondsFormat::AutoSi,true)) }
        else { property::to_json(datatype, &stored).ok_or_else(|| AppError::bad_request("invalid filter value"))? }
    };
    let mut rows: Vec<PageRow> = q::PAGE.on(db).bind(("vault",vault.clone())).bind(("table",table.to_string()))
        .bind(("kind",kind.as_str().to_string())).await?.take(0)?;
    let needed: Vec<RecordId> = sort_property.iter().cloned().chain(filter.map(|p|p.id.clone())).collect();
    let sort_values: Vec<SortValue> = if needed.is_empty() {Vec::new()} else {
        q::SORT_VALUES.on(db).bind(("vault",vault.clone())).bind(("props",needed)).await?.take(0)?
    };
    let mut latest: HashMap<(String,String),Value> = HashMap::new();
    let types:HashMap<_,_>=props.iter().map(|p|(p.id.to_string(),p.datatype.as_str())).collect();
    for v in sort_values {
        let property=v.property.to_string();
        let value=v.value(types.get(&property).copied().unwrap_or(""));
        latest.entry((v.entity.to_string(),property)).or_insert(value);
    }
    let get = |id:&RecordId,p:&RecordId|latest.get(&(id.to_string(),p.to_string())).cloned().unwrap_or(Value::Null);
    let search = args.search.as_deref().unwrap_or("").to_lowercase();
    rows.retain(|r| {
        if !r.name.to_lowercase().contains(&search) {return false;}
        let v=filter.map(|p|get(&r.id,&p.id)).unwrap_or(Value::Null);
        match op { ""=>true,"missing"=>v.is_null(),"eq"=>!v.is_null()&&compare(&v,&value).is_eq(),"gte"=>!v.is_null()&&compare(&v,&value).is_ge(),"lte"=>!v.is_null()&&compare(&v,&value).is_le(),_=>false }
    });
    let mut keyed: Vec<_> = rows.into_iter().map(|r| {
        let id=r.id.to_string();
        let key=sort_property.as_ref().map(|p|get(&r.id,p)).unwrap_or_else(||json!(r.name));
        (r,key,id)
    }).collect();
    keyed.sort_by(|(_,a,aid),(_,b,bid)| {
        let order=if args.descending {compare(b,a)} else {compare(a,b)};
        a.is_null().cmp(&b.is_null()).then(order).then_with(||aid.cmp(bid))
    });
    let total=keyed.len();
    let rows: Vec<_> = keyed.into_iter().skip(args.offset).take(limit).map(|(r,_,_)|r).collect();
    let ids: Vec<RecordId> = rows.iter().map(|r| r.id.clone()).collect();
    let names: BTreeSet<String> = props.iter().map(|p| p.name.clone()).collect();
    let values = metric::latest_values(db, &vault, &kind, &names, &ids, chrono::Utc::now()).await?;
    let sources: Vec<SourceRow> = q::SOURCES.on(db).bind(("vault", vault.clone())).bind(("ids", ids)).await?.take(0)?;
    let sources: BTreeMap<String, usize> = sources.into_iter().map(|r| (r.entity.to_string(), r.sources.len())).collect();
    #[derive(Deserialize, SurrealValue)]
    struct LinkValue { entity: RecordId, target: RecordId, label: String }
    let links: Vec<LinkValue> = q::PAGE_LINKS.on(db).bind(("vault",vault.clone()))
        .bind(("ids",rows.iter().map(|r|r.id.clone()).collect::<Vec<_>>())).bind(("labels",labels.clone())).bind(("allowed",link_types.iter().map(|t|format!("{}|{}",t.label,t.to_kind)).collect::<Vec<_>>())).await?.take(0)?;
    let targets = crate::entities::neighbourhood::load_nodes(db,links.iter().map(|l|l.target.clone()).collect()).await?;
    let mut columns: Vec<Column> = props.iter().map(column).collect();
    columns.extend(labels.iter().map(|l|Column {name:format!("link:{l}"),datatype:"link".into(),unit:None}));
    let rows: Vec<Value> = rows.into_iter().map(|r| {
        let id = r.id.to_string();
        let held = values.get(&id);
        let mut cells: BTreeMap<String, Value> = props.iter().map(|p| {
            let h = held.and_then(|h| h.get(&p.name));
            let v = h.and_then(|h| property::to_json(Datatype::parse(&p.datatype).ok()?, &h.stored)).unwrap_or(Value::Null);
            (p.name.clone(), json!({"value":v,"unit":h.and_then(|h|h.unit.clone()).or_else(||p.unit.clone())}))
        }).collect();
        for label in &labels {
            let matches:Vec<_>=links.iter().filter(|l|l.entity.to_string()==id && &l.label==label).collect();
            let names:Vec<_>=matches.iter().take(20).filter_map(|l|targets.get(&l.target.to_string()))
                .filter(|n|n.vault==vault && link_types.iter().any(|t|t.label==*label && t.to_kind==n.kind())).map(|n|n.name.clone()).collect();
            let text=if names.is_empty(){None}else{Some(format!("{}{}",names.join(", "),if matches.len()>20 {" · More links in profile"}else{""}))};
            cells.insert(format!("link:{label}"),json!({"value":text,"unit":null}));
        }
        json!({"id":id,"name":r.name,"values":cells,"source_count":sources.get(&id).copied().unwrap_or(0)})
    }).collect();
    Ok(json!({"rows":rows,"total":total,"offset":args.offset,"limit":limit,"columns":columns,"kind_approved":approved}))
}

pub async fn profile(state: &OrgState, user: &RecordId, vault: &RecordId, id: &RecordId, offset: usize) -> AppResult<Value> {
    let vault = super::resolve(&state.db, user, Some(vault), Action::ReadMemories).await?;
    crate::entities::service::require_entity_id(id)?;
    let node: crate::entities::neighbourhood::NodeRow = crate::store::get(&state.db, id).await?.ok_or_else(|| AppError::not_found("object unavailable"))?;
    if node.vault != vault { return Err(AppError::not_found("object unavailable")); }
    let kind = Kind::parse(&node.kind())?;
    let (props, approved, _) = metadata(&state.db, &vault, kind.as_str()).await?;
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
    let mut properties: Vec<Value> = props.iter().map(|p| {
        let rows: Vec<&Value> = current.iter().filter(|v|v["property"] == p.name).collect();
        let v = rows.first().copied();
        let value = v.map(|v| match p.datatype.as_str() {
            "number"|"integer"|"money"|"percent" => v["value_num"].clone(),
            "bool"=>v["value_bool"].clone(),
            "date"|"datetime"=> if v["value_date"] == "NONE" { Value::Null } else {v["value_date"].clone()},
            _=>v["value_text"].clone(),
        }).unwrap_or(Value::Null);
        let conflicting = v.is_some_and(|v|v["conflicting"] == true);
        let source = v.and_then(|v|v["source"].as_str()).filter(|s|*s!="NONE" && !s.is_empty());
        let state = if v.is_none() {"missing"} else if conflicting {"conflicting"} else if value.is_null() {"unavailable"} else {"recorded"};
        json!({"name":p.name,"datatype":p.datatype,"unit":v.and_then(|v|v["unit"].as_str()).map(String::from).or_else(||p.unit.clone()),"value":value,"as_of":v.map(|v|&v["as_of"]),"recorded_at":v.map(|v|&v["recorded_at"]),"source":source,"state":state})
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
                    Some(json!({"line":row.position().map(|p|p.line() as usize).unwrap_or(i+2),"cells":headers.iter().zip(row.iter()).map(|(h,c)|(h.clone(),json!(c))).collect::<BTreeMap<_,_>>()}))
                }).take(2).collect();
                source["document"] = json!({"id":source_id,"filename":doc.filename,"row":if matches.len()==1 {matches.first().cloned()} else {None},"note":"Current document. Original imported row number and revision were not retained; this row is matched by object name."});
                source["state"] = json!("available");
            }
            Ok(_) => {},
            Err(e) if e.status.is_server_error() => { source["error"]=json!("Document storage unavailable"); },
            Err(_) => {},
        }
    }
    for p in &mut properties {
        let source = sources.iter().find(|s|s["id"]==p["source"]);
        p["source_state"] = source.map(|s|s["state"].clone()).unwrap_or(json!("not recorded"));
        p["source_label"] = source.map(|s| {
            if let Some(filename)=s["document"]["filename"].as_str() {
                match s["document"]["row"]["line"].as_u64() {Some(line)=>json!(format!("{filename}, row {line}")),None=>json!(filename)}
            } else {
                let label=s["record"]["title"].as_str().filter(|s|!s.is_empty()).or_else(||s["record"]["external_id"].as_str().filter(|s|!s.is_empty()));
                label.map(|s|json!(s)).unwrap_or_else(||if s["state"]=="available" {json!("Inspect source")}else{json!("Source unavailable")})
            }
        }).unwrap_or(json!("Source not recorded"));
        if p["state"] != "recorded" { continue; }
        if let Some(record) = sources.iter().find(|s|s["id"]==p["source"]).and_then(|s|s["record"].as_object()) {
            let updated = record.get("updated_at").and_then(Value::as_str).and_then(|s|chrono::DateTime::parse_from_rfc3339(s).ok());
            let recorded_at = p["recorded_at"].as_str().and_then(|s|chrono::DateTime::parse_from_rfc3339(s).ok());
            if updated.zip(recorded_at).is_some_and(|(updated,recorded_at)|updated>recorded_at) { p["state"]=json!("stale"); }
        }
    }
    let metrics = metric::metrics_by_kind(&state.db, &vault).await?.remove(kind.as_str()).unwrap_or_default();
    let mut numbers = Vec::new();
    for m in metrics.iter().filter(|m|m.status == super::Status::Approved) {
        let args = metric::EvalArgs { metric:m.name.clone(),kind:Some(kind.as_str().into()),vault_id:Some(vault.to_string()),..Default::default() };
        let run = match metric::eval_object(&state.db, &vault, &args, id).await {
            Ok(mut run)=>{
                let warnings:Vec<_> = properties.iter().filter(|p|run["input_properties"].as_array().is_some_and(|names|names.contains(&p["name"])))
                    .filter(|p|(p["state"]!="recorded" && p["state"]!="missing") || p["source_state"]=="unavailable")
                    .map(|p| {
                        let state=if p["source_state"]=="unavailable" {"recorded with an unavailable source"}else{p["state"].as_str().unwrap_or("unavailable")};
                        json!(format!("Input {} is {state}. Inspect its sources before relying on this result.",p["name"].as_str().unwrap_or("property")))
                    }).collect();
                if let Some(out)=run["warnings"].as_array_mut(){out.extend(warnings);}
                json!({"run":run})
            },Err(e)=>json!({"unavailable":e.message})
        };
        numbers.push(json!({"name":m.name,"definition":m.expr_words,"result":run}));
    }
    Ok(json!({"id":id.to_string(),"name":node.name,"kind":kind.as_str(),"summary":node.summary,"kind_approved":approved,
        "properties":properties,"numbers":numbers,"links":links,"link_counts":link_counts,"history":history,"history_total":totals.first().map(|v|&v["total"]),"sources":sources,"memories":memories,"more":more,"offset":offset,
        "warnings":["Freshness rules and review history are not recorded. Imported values may be updated in place; this is not a complete change history."]}))
}
