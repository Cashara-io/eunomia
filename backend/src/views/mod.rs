//! `show_view`: the model's answer drawn as a view the app knows, instead of free text. A typed
//! catalogue, one [`ViewSpec`] variant per view: `impact` (what a change to one entity affects, each
//! affected entity with the chain of relations that reaches it and the model's reason), `subgraph`
//! (how some entities connect), `metric` (an approved metric's number) and `scenario` (a what-if: the
//! metrics before and after hypothetical changes). The server reruns `metric` and `scenario` itself, so
//! a number on screen is always its own.
//!
//! The server resolves every id and checks it before anything is drawn: each entity and relation must
//! exist in a vault the caller can read, and each impact path must be a connected chain of relations
//! from the start to its entity. A bad id is a tool error naming it, so the model can fix the call. Free
//! text is trimmed, capped and has secrets dropped as in a memory. The result is the resolved view, with
//! names and kinds from the database, plus [`Shown::text`], a plain rendering for a terminal.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use surrealdb::types::RecordId;

use crate::authz::Action;
use crate::entities::neighbourhood::{load_edges, load_nodes, EdgeRow, NodeRow};
use crate::entities::service::{accessible, require_entity_id};
use crate::error::{AppError, AppResult};
use crate::ontology::metric::{self, EvalArgs, Filter, MetricEvalOut};
use crate::ontology::scenario::{self, ScenarioArgs, ScenarioOut};
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;

/// The tool's name: MCP and the chat treat its result as a view.
pub const TOOL: &str = "show_view";

pub const MAX_AFFECTED: usize = 50;
/// Most relations in one impact path.
pub const MAX_PATH: usize = 8;
pub const MAX_SUBGRAPH_ENTITIES: usize = 150;
pub const MAX_SUBGRAPH_RELATIONS: usize = 300;
const MAX_HEADLINE: usize = 200;
const MAX_WHY: usize = 300;

/// What the model asks to draw.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "view", rename_all = "snake_case")]
pub enum ViewSpec {
    Impact {
        start_id: String,
        headline: String,
        #[serde(default)]
        affected: Vec<AffectedSpec>,
    },
    Subgraph {
        title: String,
        entity_ids: Vec<String>,
        #[serde(default)]
        relation_ids: Vec<String>,
    },
    /// The same arguments as `metric_eval`; there is no field for a value.
    Metric {
        metric: String,
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        vault_id: Option<String>,
        #[serde(default)]
        filters: Vec<Filter>,
        #[serde(default)]
        as_of: Option<String>,
        #[serde(default)]
        group_by_grain: bool,
        #[serde(default)]
        allow_unapproved: bool,
    },
    /// The same arguments as `scenario_run`, under `scenario`; there is no field for a number.
    Scenario { scenario: ScenarioArgs, headline: String },
}

#[derive(Debug, Clone, Deserialize)]
pub struct AffectedSpec {
    pub entity_id: String,
    /// Relation ids from the start to this entity, each in either stored direction.
    pub path: Vec<String>,
    pub why: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct EntityRef {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub vault: String,
}

/// One relation, in its stored direction.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Step {
    pub id: String,
    pub from: EntityRef,
    pub label: String,
    pub to: EntityRef,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Affected {
    pub entity: EntityRef,
    pub path: Vec<Step>,
    pub why: String,
}

/// A view, resolved and checked.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "view", rename_all = "snake_case")]
pub enum View {
    Impact { start: EntityRef, headline: String, affected: Vec<Affected> },
    Subgraph { title: String, entities: Vec<EntityRef>, relations: Vec<Step> },
    /// `metric` is the server's own run; `entities` are its `entity_ids` with names, the receipts.
    Metric { metric: MetricEvalOut, entities: Vec<EntityRef> },
    /// `result` is the server's own run of the what-if.
    Scenario { headline: String, result: ScenarioOut },
}

/// `show_view`'s result: the view's fields, plus `text`.
#[derive(Debug, Clone, Serialize)]
pub struct Shown {
    #[serde(flatten)]
    pub view: View,
    pub text: String,
}

/// Resolves and checks `spec` for `owner`.
pub async fn show(db: &OrgDb, owner: &RecordId, spec: ViewSpec) -> AppResult<Shown> {
    let view = match spec {
        ViewSpec::Impact { start_id, headline, affected } => {
            if affected.len() > MAX_AFFECTED {
                return Err(AppError::bad_request(format!("an impact view lists at most {MAX_AFFECTED} affected entities")));
            }
            if let Some(a) = affected.iter().find(|a| a.path.len() > MAX_PATH) {
                return Err(AppError::bad_request(format!("the path to {} has more than {MAX_PATH} relations", a.entity_id)));
            }
            let entity_ids = std::iter::once(start_id.as_str()).chain(affected.iter().map(|a| a.entity_id.as_str()));
            let relation_ids = affected.iter().flat_map(|a| a.path.iter().map(String::as_str));
            let graph = Resolved::load(db, owner, entity_ids, relation_ids).await?;
            let start = graph.entity(&start_id)?;
            let headline = clean("headline", &headline, MAX_HEADLINE)?;
            let mut listed = HashSet::new();
            let affected = affected
                .iter()
                .map(|a| {
                    let entity = graph.entity(&a.entity_id)?;
                    if entity.id == start.id {
                        return Err(AppError::bad_request(format!("{} is the start entity; list only what it affects", a.entity_id)));
                    }
                    if !listed.insert(entity.id.clone()) {
                        return Err(AppError::bad_request(format!("{} is listed twice in affected: list each entity once", a.entity_id)));
                    }
                    let path = graph.path(&start, &entity, &a.path)?;
                    Ok(Affected { entity, path, why: clean("why", &a.why, MAX_WHY)? })
                })
                .collect::<AppResult<Vec<_>>>()?;
            View::Impact { start, headline, affected }
        }
        ViewSpec::Subgraph { title, entity_ids, relation_ids } => {
            if entity_ids.len() > MAX_SUBGRAPH_ENTITIES || relation_ids.len() > MAX_SUBGRAPH_RELATIONS {
                return Err(AppError::bad_request(format!(
                    "a subgraph view shows at most {MAX_SUBGRAPH_ENTITIES} entities and {MAX_SUBGRAPH_RELATIONS} relations"
                )));
            }
            let graph = Resolved::load(db, owner, entity_ids.iter().map(String::as_str), relation_ids.iter().map(String::as_str)).await?;
            // a repeat carries nothing new, so it is dropped rather than refused
            let mut relations: Vec<Step> = Vec::new();
            for r in &relation_ids {
                let step = graph.step(r)?;
                if !relations.iter().any(|x| x.id == step.id) {
                    relations.push(step);
                }
            }
            // a relation's ends belong in the picture even when the model did not list them
            let mut entities: Vec<EntityRef> = Vec::new();
            let listed = entity_ids.iter().map(|e| graph.entity(e)).collect::<AppResult<Vec<_>>>()?;
            for e in listed.into_iter().chain(relations.iter().flat_map(|s| [s.from.clone(), s.to.clone()])) {
                if !entities.iter().any(|x| x.id == e.id) {
                    entities.push(e);
                }
            }
            View::Subgraph { title: clean("title", &title, MAX_HEADLINE)?, entities, relations }
        }
        ViewSpec::Metric { metric, kind, vault_id, filters, as_of, group_by_grain, allow_unapproved } => {
            let args = EvalArgs { metric, kind, vault_id, filters, as_of, group_by_grain, allow_unapproved };
            let out = metric::eval(db, owner, &args).await?;
            let ids = out.entity_ids.iter().map(String::as_str);
            let graph = Resolved::load(db, owner, ids, std::iter::empty()).await?;
            let entities = out.entity_ids.iter().filter_map(|id| graph.entity(id).ok()).collect();
            View::Metric { metric: out, entities }
        }
        ViewSpec::Scenario { scenario: args, headline } => {
            let headline = clean("headline", &headline, MAX_HEADLINE)?;
            View::Scenario { headline, result: scenario::run(db, owner, &args).await? }
        }
    };
    let text = view.text();
    Ok(Shown { view, text })
}

/// Free text from the model: trimmed, secrets replaced as in a memory, at most `max` characters.
fn clean(field: &str, s: &str, max: usize) -> AppResult<String> {
    let s = s.trim();
    if s.is_empty() {
        return Err(AppError::bad_request(format!("{field} is empty")));
    }
    let s = crate::redact::redact(s, &mut Vec::new());
    if s.chars().count() <= max {
        return Ok(s);
    }
    Ok(s.chars().take(max.saturating_sub(1)).collect::<String>().trim_end().to_string() + "\u{2026}")
}

fn unknown_entity(id: &str) -> AppError {
    AppError::bad_request(format!("unknown entity {id}: use an entity id from entities_search or entities_neighbourhood"))
}

fn unknown_relation(id: &str) -> AppError {
    AppError::bad_request(format!("unknown relation {id}: use an edge id from entities_neighbourhood"))
}

/// The entities and relations a view names, those the caller may read, keyed by the id as given.
struct Resolved {
    entities: HashMap<String, EntityRef>,
    relations: HashMap<String, (EdgeRow, EntityRef, EntityRef)>,
}

impl Resolved {
    /// Loads every id in two round trips plus one access check per vault. A relation is readable when
    /// both its ends are and share a vault: one from before relations were confined to a vault stays
    /// hidden, as in `entities_get` and the neighbourhood. Ids that do not parse fail here; ids that are missing or unreadable fail when
    /// the view looks them up, so the error names the first bad one in the view's order.
    async fn load<'a>(
        db: &OrgDb,
        owner: &RecordId,
        entity_ids: impl Iterator<Item = &'a str>,
        relation_ids: impl Iterator<Item = &'a str>,
    ) -> AppResult<Self> {
        let mut entity_rids = Vec::new();
        for id in entity_ids {
            let rid = crate::rid::parse(id).map_err(|_| unknown_entity(id))?;
            require_entity_id(&rid).map_err(|_| unknown_entity(id))?;
            entity_rids.push(rid);
        }
        let mut relation_rids = Vec::new();
        for id in relation_ids {
            let rid = crate::rid::parse(id).ok().filter(|r| r.table() == "relates_to").ok_or_else(|| unknown_relation(id))?;
            relation_rids.push(rid);
        }
        let edges = load_edges(db, relation_rids).await?;
        let ends = edges.values().flat_map(|e| [e.from.clone(), e.to.clone()]);
        let nodes = load_nodes(db, entity_rids.into_iter().chain(ends).collect()).await?;

        let mut readable: HashMap<String, bool> = HashMap::new();
        for vault in nodes.values().map(|n| &n.vault) {
            if let Entry::Vacant(slot) = readable.entry(vault.to_string()) {
                slot.insert(accessible(db, owner, vault, Action::ReadMemories).await?);
            }
        }
        let entity_ref = |n: &NodeRow| EntityRef { id: n.id.to_string(), name: n.name.clone(), kind: n.kind(), vault: n.vault.to_string() };
        let entities: HashMap<String, EntityRef> =
            nodes.values().filter(|n| readable[&n.vault.to_string()]).map(|n| (n.id.to_string(), entity_ref(n))).collect();
        let relations = edges
            .into_iter()
            .filter_map(|(id, e)| {
                let from = entities.get(&e.from.to_string())?.clone();
                let to = entities.get(&e.to.to_string())?.clone();
                (from.vault == to.vault).then_some((id, (e, from, to)))
            })
            .collect();
        Ok(Resolved { entities, relations })
    }

    fn entity(&self, id: &str) -> AppResult<EntityRef> {
        let key = crate::rid::parse(id).map(|r| r.to_string()).unwrap_or_default();
        self.entities.get(&key).cloned().ok_or_else(|| unknown_entity(id))
    }

    fn step(&self, id: &str) -> AppResult<Step> {
        let key = crate::rid::parse(id).map(|r| r.to_string()).unwrap_or_default();
        let (e, from, to) = self.relations.get(&key).ok_or_else(|| unknown_relation(id))?;
        Ok(Step { id: e.id.to_string(), from: from.clone(), label: e.label.clone(), to: to.clone() })
    }

    /// `ids` as a chain of relations that starts at `start` and ends at `end`, each relation walked in
    /// either direction, never twice, and never back to an entity it already passed.
    fn path(&self, start: &EntityRef, end: &EntityRef, ids: &[String]) -> AppResult<Vec<Step>> {
        if ids.is_empty() {
            return Err(AppError::bad_request(format!("the path to {} is empty: list the relation ids from the start to it", end.id)));
        }
        let mut at = start.id.clone();
        let mut passed = HashSet::from([start.id.clone()]);
        let mut steps: Vec<Step> = Vec::new();
        for id in ids {
            let step = self.step(id)?;
            if steps.iter().any(|s| s.id == step.id) {
                return Err(AppError::bad_request(format!("the path to {} uses relation {id} twice", end.id)));
            }
            at = if step.from.id == at {
                step.to.id.clone()
            } else if step.to.id == at {
                step.from.id.clone()
            } else if steps.is_empty() {
                return Err(AppError::bad_request(format!("the path to {} does not start at the start entity {}: relation {id} does not touch it", end.id, start.id)));
            } else {
                return Err(AppError::bad_request(format!("the path to {} is broken at relation {id}: it does not touch {at}", end.id)));
            };
            if !passed.insert(at.clone()) {
                return Err(AppError::bad_request(format!("the path to {} comes back to {at} at relation {id}: list the shortest chain", end.id)));
            }
            steps.push(step);
        }
        if at != end.id {
            return Err(AppError::bad_request(format!("the path to {} ends at {at} instead", end.id)));
        }
        Ok(steps)
    }
}

/// `part_of` reads as "part of".
fn words(label: &str) -> String {
    label.replace('_', " ")
}

fn sentence(s: &Step) -> String {
    format!("{} {} {}", s.from.name, words(&s.label), s.to.name)
}

impl View {
    /// The view as plain text, for a terminal or any client that shows only text.
    pub fn text(&self) -> String {
        match self {
            View::Impact { start, headline, affected } => {
                let mut out = format!("{headline}\nStarting from {} ({}).", start.name, words(&start.kind));
                if affected.is_empty() {
                    out.push_str("\nNothing else is affected.");
                }
                for (i, a) in affected.iter().enumerate() {
                    let via = a.path.iter().map(sentence).collect::<Vec<_>>().join(", then ");
                    out.push_str(&format!("\n{}. {} ({}), via {via}. {}", i + 1, a.entity.name, words(&a.entity.kind), a.why));
                }
                out
            }
            View::Subgraph { title, entities, relations } => {
                let names = entities.iter().map(|e| format!("{} ({})", e.name, words(&e.kind))).collect::<Vec<_>>().join(", ");
                let mut out = format!("{title}\nEntities: {names}.");
                for r in relations {
                    out.push_str(&format!("\n- {}", sentence(r)));
                }
                out
            }
            View::Metric { metric: m, entities } => metric_text(m, entities),
            View::Scenario { headline, result } => scenario_text(headline, result),
        }
    }
}

/// A metric's number as people write it: money to the cent, whole numbers bare, others to two places.
pub fn number(value: f64, unit: Option<&str>) -> String {
    let money = unit.is_some_and(|u| u.len() == 3 && u.chars().all(|c| c.is_ascii_uppercase()));
    let n = if money {
        format!("{value:.2}")
    } else if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        let s = format!("{value:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    };
    match unit {
        Some(u) => format!("{n} {u}"),
        None => n,
    }
}

fn metric_text(m: &MetricEvalOut, entities: &[EntityRef]) -> String {
    let value = match (&m.series, m.value) {
        (Some(_), _) => "a series".to_string(),
        (None, Some(v)) => number(v, m.unit.as_deref()),
        (None, None) => "no value".to_string(),
    };
    let mut out = format!("{}: {value}", m.metric);
    if m.status != crate::ontology::Status::Approved {
        out.push_str(" (not approved yet)");
    }
    out.push_str(&format!("\n{}.", m.expr_words));
    if let Some(d) = &m.description {
        let stop = if d.ends_with(['.', '!', '?']) { "" } else { "." };
        out.push_str(&format!(" {d}{stop}"));
    }
    let date = m.as_of.get(..10).unwrap_or(&m.as_of);
    let noun = if m.n_entities == 1 { words(&m.kind) } else { format!("{}s", words(&m.kind)) };
    out.push_str(&format!("\nAs of {date}, from {} {noun}.", m.n_entities));
    for p in m.series.iter().flatten() {
        let v = p.value.map(|v| number(v, m.unit.as_deref())).unwrap_or_else(|| "no value".into());
        out.push_str(&format!("\n- {}: {v} ({})", p.period, p.n));
    }
    if !entities.is_empty() {
        let more = if m.truncated { format!(" and {} more", m.n_entities - entities.len()) } else { String::new() };
        out.push_str(&format!("\nFrom: {}{more}.", entities.iter().map(|e| e.name.as_str()).collect::<Vec<_>>().join(", ")));
    }
    for w in &m.warnings {
        out.push_str(&format!("\nNote: {w}"));
    }
    out
}

/// A number as [`number`] writes it for `unit`, without the unit.
fn figure(v: f64, unit: Option<&str>) -> String {
    let s = number(v, unit);
    match unit {
        Some(u) => s.strip_suffix(&format!(" {u}")).unwrap_or(&s).to_string(),
        None => s,
    }
}

/// "15.22 \u{2192} 15.67 AUD, up 0.45 AUD (2.9%)": the move in words, never by colour alone.
pub fn movement(before: Option<f64>, after: Option<f64>, delta_pct: Option<f64>, unit: Option<&str>) -> String {
    let tail = unit.map(|u| format!(" {u}")).unwrap_or_default();
    let (a, b) = match (before, after) {
        (Some(a), Some(b)) => (a, b),
        (None, Some(b)) => return format!("no value before, {}{tail} after", figure(b, unit)),
        (Some(a), None) => return format!("{}{tail} before, no value after", figure(a, unit)),
        (None, None) => return "no value".into(),
    };
    let d = b - a;
    if d.abs() < 1e-9 {
        return format!("{}{tail}, no change", figure(a, unit));
    }
    let way = if d > 0.0 { "up" } else { "down" };
    let pct = delta_pct.map(|p| format!(" ({}%)", number((p.abs() * 10.0).round() / 10.0, None))).unwrap_or_default();
    format!("{} \u{2192} {}{tail}, {way} {}{tail}{pct}", figure(a, unit), figure(b, unit), figure(d.abs(), unit))
}

fn shown_value(v: &serde_json::Value, unit: Option<&str>) -> String {
    match v.as_f64() {
        Some(n) => number(n, unit),
        None => v.as_str().map(String::from).unwrap_or_else(|| v.to_string()),
    }
}

fn scenario_text(headline: &str, r: &ScenarioOut) -> String {
    let mut out = format!("{headline}\nWhat if: {}.", r.changes.join("; "));
    for (b, s) in r.baseline.iter().zip(&r.scenario) {
        let approved = if b.status == crate::ontology::Status::Approved { "" } else { " (not approved yet)" };
        out.push_str(&format!("\n- {}{approved}: {}", words(&b.metric), movement(b.value, s.value.value, s.delta_pct, b.unit.as_deref())));
    }
    if !r.changed.is_empty() {
        let listed = r
            .changed
            .iter()
            .map(|c| {
                let before = c.before.as_ref().map(|v| shown_value(v, c.unit.as_deref())).unwrap_or_else(|| "none".into());
                format!("{} {} {before} \u{2192} {}", c.name, words(&c.property), shown_value(&c.after, c.unit.as_deref()))
            })
            .collect::<Vec<_>>()
            .join("; ");
        let more = r.n_changed.saturating_sub(r.changed.len());
        let more = if more > 0 { format!(" and {more} more") } else { String::new() };
        out.push_str(&format!("\nChanged: {listed}{more}."));
    }
    if !r.reached.is_empty() {
        let listed = r.reached.iter().map(|e| format!("{} ({}, via {})", e.name, words(&e.kind), e.via)).collect::<Vec<_>>().join("; ");
        out.push_str(&format!("\nReaches: {listed}."));
    }
    for w in &r.warnings {
        out.push_str(&format!("\nNote: {w}"));
    }
    out.push_str("\nNothing was saved: this is a what-if.");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: &str, name: &str, kind: &str) -> EntityRef {
        EntityRef { id: id.into(), name: name.into(), kind: kind.into(), vault: "vault:v".into() }
    }

    #[test]
    fn an_impact_view_reads_as_sentences() {
        let (truck, shipment, order) = (e("entity:t", "Truck 7", "truck"), e("entity:s", "Shipment 88", "shipment"), e("entity:o", "Order 12", "order"));
        let carries = Step { id: "relates_to:1".into(), from: truck.clone(), label: "carries".into(), to: shipment.clone() };
        let fulfils = Step { id: "relates_to:2".into(), from: shipment.clone(), label: "part_of".into(), to: order.clone() };
        let view = View::Impact {
            start: truck,
            headline: "Truck 7 is late.".into(),
            affected: vec![
                Affected { entity: shipment, path: vec![carries.clone()], why: "It is on the truck.".into() },
                Affected { entity: order, path: vec![carries, fulfils], why: "Its shipment is late.".into() },
            ],
        };
        assert_eq!(
            view.text(),
            "Truck 7 is late.\nStarting from Truck 7 (truck).\n\
             1. Shipment 88 (shipment), via Truck 7 carries Shipment 88. It is on the truck.\n\
             2. Order 12 (order), via Truck 7 carries Shipment 88, then Shipment 88 part of Order 12. Its shipment is late."
        );
    }

    #[test]
    fn free_text_is_trimmed_capped_and_loses_secrets() {
        assert!(clean("why", "  ", 10).unwrap_err().message.contains("why is empty"));
        assert_eq!(clean("why", "  short  ", 10).unwrap(), "short");
        let capped = clean("why", &"x".repeat(50), 10).unwrap();
        assert_eq!(capped.chars().count(), 10);
        assert!(capped.ends_with('\u{2026}'));
        let token = format!("ghp_{}", "a1B2c3D4e5".repeat(4));
        let out = clean("why", &format!("the key is {token}"), 300).unwrap();
        assert!(!out.contains(&token) && out.contains("[redacted"), "{out}");
    }
}
