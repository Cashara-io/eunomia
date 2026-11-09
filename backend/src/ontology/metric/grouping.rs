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
    members: HashMap<String, (Value, BTreeSet<String>)>,
    sort: Option<super::super::dataset::Sort>,
    top_n: Option<usize>,
}
pub(super) fn text_key(text: &str) -> String { text.to_lowercase() }

impl Grouping {
    pub(super) async fn load(db: &OrgDb, vault: &RecordId, prepared: &Prepared, args: &EvalArgs, as_of: chrono::DateTime<chrono::Utc>) -> AppResult<Option<Self>> {
        let Some(group) = &args.group_by else { return Ok(None) };
        if args.top_n.is_some_and(|n| n == 0 || n > 500) || args.sort.as_ref().is_some_and(|s| !matches!(s.property.as_str(), "group" | "value")) {
            return Err(AppError::bad_request("grouped metrics allow top_n from 1 to 500 and sort property group or value"));
        }
        let ids: Vec<RecordId> = prepared.entities.iter().map(|id| crate::rid::parse(id).expect("loaded entity id")).collect();
        let mut members: HashMap<String, (Value, BTreeSet<String>)> = HashMap::new();
        let mut add = |key: Value, id: &str| {
            let normal = match &key {
                Value::String(s) => Value::String(text_key(s)).to_string(),
                _ => key.to_string(),
            };
            let member = members.entry(normal).or_insert_with(|| (key.clone(), BTreeSet::new()));
            // Stable spelling, regardless of database row order.
            if let (Value::String(candidate), Value::String(shown)) = (&key, &member.0)
                && candidate < shown { member.0 = key; }
            member.1.insert(id.into());
        };
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
        let by_id: HashMap<_, _> = refs.iter().map(|id| (id.as_str(), *id)).collect();
        let mut memberships: HashMap<&str, usize> = HashMap::new();
        for (key, members) in self.members.values() {
            let refs: Vec<_> = members.iter().filter_map(|id| by_id.get(id.as_str()).copied()).collect();
            let (value, used) = value_of(plan, &refs, values, props, warnings);
            if used.is_empty() { continue; }
            for id in &used { *memberships.entry(by_id[id.as_str()].as_str()).or_default() += 1; }
            all_used.extend(used.iter().cloned());
            groups.push(Group { group: key.clone(), value, n_entities: used.len(), truncated: used.len() > MAX_ENTITY_IDS, entity_ids: used.into_iter().take(MAX_ENTITY_IDS).collect() });
        }
        let overlapping = memberships.values().filter(|n| **n > 1).count();
        if overlapping > 0 {
            let subject = if overlapping == 1 { "entity contributes" } else { "entities contribute" };
            warnings.push(format!("{overlapping} {subject} to more than one group; groups overlap and must not be summed"));
        }
        // Always sort before limiting, including when the caller omitted sort.
        let default_sort = super::super::dataset::Sort { property: "value".into(), descending: true };
        {
            let sort = self.sort.as_ref().unwrap_or(&default_sort);
            groups.sort_by(|a,b| {
                let order = if sort.property == "value" {
                    match (a.value,b.value) { (Some(a),Some(b)) => a.total_cmp(&b), (Some(_),None) => return std::cmp::Ordering::Less, (None,Some(_)) => return std::cmp::Ordering::Greater, _ => std::cmp::Ordering::Equal }
                } else { super::super::dataset::compare(&a.group, &b.group) };
                (if sort.descending { order.reverse() } else { order }).then_with(|| a.group.to_string().cmp(&b.group.to_string()))
            });
        }
        let limit = self.top_n.unwrap_or(1000).min(1000);
        if groups.len() > limit {
            warnings.push(format!("{} more groups not shown", groups.len() - limit));
            groups.truncate(limit);
        }
        (groups, all_used)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn many_groups_are_ranked_globally_capped_and_reported() {
        for size in [1001, 50_000] {
        let ids: Vec<String> = (0..size).map(|i| format!("entity:{i:05}")).collect();
        let members = ids.iter().enumerate().map(|(i,id)| (format!("{i:05}"),(json!(i),BTreeSet::from([id.clone()])))).collect();
        let grouping = Grouping { members, sort: Some(serde_json::from_value(json!({"property":"group","descending":true})).unwrap()), top_n: None };
        let plan = check(&Props::new(), &Kind::parse("sample").unwrap(), &serde_json::from_value(json!({"agg":"count"})).unwrap()).unwrap();
        let started = std::time::Instant::now(); let mut warnings = Vec::new();
        let (groups,used) = grouping.run(&plan,&ids.iter().collect::<Vec<_>>(),&Values::new(),&Props::new(),&mut warnings);
        assert_eq!(groups.len(),1000);
        assert_eq!(groups[0].group,json!(size - 1));
        assert_eq!(groups[999].group,json!(size - 1000));
        assert_eq!(used.len(),size);
        assert!(warnings.iter().any(|w| w == &format!("{} more groups not shown", size - 1000)),"{warnings:?}");
        assert!(started.elapsed() < std::time::Duration::from_secs(5),"grouping took {:?}",started.elapsed());
        }
    }
}
