//! Group membership is loaded once. Each group uses the existing metric arithmetic.
use super::*;
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Group {
    pub group: Value,
    pub value: Option<f64>,
    pub n_entities: usize,
    pub entity_ids: Vec<String>,
    pub truncated: bool,
}
pub(super) struct Grouping {
    members: BTreeMap<String, (Value, BTreeSet<String>)>,
    sort: Option<super::super::dataset::Sort>,
    top_n: Option<usize>,
}
impl Grouping {
    pub(super) async fn load(db: &OrgDb, vault: &RecordId, prepared: &Prepared, args: &EvalArgs, as_of: chrono::DateTime<chrono::Utc>) -> AppResult<Option<Self>> {
        let Some(group) = &args.group_by else { return Ok(None) };
        if args.top_n.is_some_and(|n| n == 0 || n > 500) || args.sort.as_ref().is_some_and(|s| !matches!(s.property.as_str(), "group" | "value")) {
            return Err(AppError::bad_request("grouped metrics allow top_n from 1 to 500 and sort property group or value"));
        }
        let ids: Vec<RecordId> = prepared.entities.iter().map(|id| crate::rid::parse(id).expect("loaded entity id")).collect();
        let mut members: BTreeMap<String, (Value, BTreeSet<String>)> = BTreeMap::new();
        let mut add = |key: Value, id: &str| { members.entry(key.to_string()).or_insert_with(|| (key, BTreeSet::new())).1.insert(id.into()); };
        if let Some(label) = group.strip_prefix("link:") {
            let mut metadata = q::UI_METADATA.on(db).bind(("vault", vault.clone())).bind(("kind", prepared.kind.as_str().to_string()))
                .bind(("names", Vec::<String>::new())).bind(("labels", vec![label.to_string()])).await?;
            let definitions: Vec<super::super::LinkRow> = metadata.take(2)?;
            if !definitions.iter().any(|l| l.label == label && l.status == "approved") { return Err(AppError::bad_request("group_by needs an approved outgoing link")); }
            #[derive(Deserialize, SurrealValue)]
            struct Link { entity: RecordId, target: RecordId }
            let links: Vec<Link> = q::UI_LINKS.on(db).bind(("vault", vault.clone())).bind(("ids", ids.clone())).bind(("labels", vec![label.to_string()])).await?.take(0)?;
            if links.len() > 5000 { return Err(AppError::bad_request("grouped metric link targets exceed 5000; narrow the metric")); }
            let nodes = crate::entities::neighbourhood::load_nodes(db, links.iter().map(|l| l.target.clone()).collect()).await?;
            let mut linked = HashSet::new();
            for link in links {
                if !nodes.get(&link.target.to_string()).is_some_and(|node| definitions.iter().any(|l| l.status == "approved" && l.to_kind == node.kind())) { continue; }
                let id = link.entity.to_string();
                linked.insert(id.clone());
                let target = link.target.to_string();
                add(json!({"entity_id":target,"name":nodes.get(&target).map(|n| n.name.as_str()).unwrap_or("")}), &id);
            }
            for id in &prepared.entities { if !linked.contains(id) { add(Value::Null, id); } }
        } else {
            let name = PropertyName::parse(group)?;
            let prop = prepared.props.get(name.as_str()).filter(|p| p.status == Status::Approved).ok_or_else(|| AppError::bad_request("group_by needs an approved property"))?;
            let values = latest_values(db, vault, &prepared.kind, &BTreeSet::from([name.as_str().to_string()]), &ids, as_of).await?;
            for id in &prepared.entities {
                let key = values.get(id).and_then(|v| v.get(name.as_str())).and_then(|h| super::super::property::to_json(prop.datatype, &h.stored)).unwrap_or(Value::Null);
                add(key, id);
            }
        }
        Ok(Some(Self { members, sort: args.sort.clone(), top_n: args.top_n }))
    }
    pub(super) fn run(&self, plan: &Plan, refs: &[&String], values: &Values, props: &Props, warnings: &mut Vec<String>) -> (Vec<Group>, BTreeSet<String>) {
        let mut all_used = BTreeSet::new();
        let mut groups = Vec::new();
        for (key, members) in self.members.values() {
            let refs: Vec<_> = refs.iter().copied().filter(|id| members.contains(*id)).collect();
            let (value, used) = value_of(plan, &refs, values, props, warnings);
            if used.is_empty() { continue; }
            all_used.extend(used.iter().cloned());
            groups.push(Group { group: key.clone(), value, n_entities: used.len(), truncated: used.len() > MAX_ENTITY_IDS, entity_ids: used.into_iter().take(MAX_ENTITY_IDS).collect() });
        }
        if let Some(sort) = &self.sort {
            groups.sort_by(|a,b| {
                let order = if sort.property == "value" {
                    match (a.value,b.value) { (Some(a),Some(b)) => a.total_cmp(&b), (Some(_),None) => return std::cmp::Ordering::Less, (None,Some(_)) => return std::cmp::Ordering::Greater, _ => std::cmp::Ordering::Equal }
                } else { super::super::dataset::compare(&a.group, &b.group) };
                (if sort.descending { order.reverse() } else { order }).then_with(|| a.group.to_string().cmp(&b.group.to_string()))
            });
        }
        if let Some(n) = self.top_n { groups.truncate(n); }
        (groups, all_used)
    }
}
