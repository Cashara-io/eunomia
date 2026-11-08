//! The what-if sandbox (`scenario_run`): "what if the latte price went up 30 cents", "what if Truck 7
//! never arrives". The model says what changes and which metrics matter; the server checks each change
//! as `entity_set_property` checks a value, puts the changes over the real values in memory, runs each
//! metric on the real values and on the changed ones through the same arithmetic as `metric_eval`,
//! and lists what the changed entities reach. Nothing is written: the overlay lives for one call.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use surrealdb::types::RecordId;

use super::metric::{self, EvalArgs, Filter, Held, Overlay, Selector, Values};
use super::property::{check_value, resolve_approved, to_json, Datatype, PropertyName, Stored};
use super::{resolve, Kind, Status};
use crate::authz::Action;
use crate::entities::neighbourhood::{load_nodes, relations_of, walk, NodeRow, DEFAULT_DEPTH, MAX_DEPTH};
use crate::entities::service::require_entity_id;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::views::number;

pub const MAX_CHANGES: usize = 20;
pub const MAX_METRICS: usize = 10;
pub const MAX_IMPACT_FROM: usize = 5;
/// Most entities one scale or shift changes; past it the rest keep their real values.
pub const MAX_TOUCHED: usize = 500;
/// Most changed entities `reached` is traced from, besides `impact_from`.
const MAX_TRACED: usize = 50;
/// Most changed values the result lists; `n_changed` counts them all.
pub const MAX_LISTED: usize = 50;
const MAX_FACTOR: f64 = 1000.0;
const BUDGET: Duration = Duration::from_secs(20);

/// A hypothetical value for one entity's property.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SetChange {
    pub entity_id: String,
    pub property: String,
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// Picks only the entities linked to `entity_id`, by a relation labelled `label` when given.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RelatedTo {
    pub entity_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Every entity of `kind` the filters pick: its `property` times `factor`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScaleChange {
    pub kind: String,
    pub property: String,
    /// Only these entities of the kind (at most [`MAX_TOUCHED`]), e.g. just the latte.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entity_ids: Vec<String>,
    pub factor: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<Filter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related_to: Option<RelatedTo>,
}

/// Every entity of `kind` the filters pick: its `property` plus `delta`, in the property's unit.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShiftChange {
    pub kind: String,
    pub property: String,
    /// Only these entities of the kind (at most [`MAX_TOUCHED`]), e.g. just the latte.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entity_ids: Vec<String>,
    pub delta: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<Filter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related_to: Option<RelatedTo>,
}

/// The entity is gone from the picture: it counts in no metric.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveChange {
    pub entity_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    Set(SetChange),
    Scale(ScaleChange),
    Shift(ShiftChange),
    Remove(RemoveChange),
}

/// A metric by name, or by name and kind when several kinds have one of that name.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum MetricRef {
    Name(String),
    Named {
        metric: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
    },
}

/// What `scenario_run` was asked, and what a `scenario` view reruns.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of: Option<String>,
    pub changes: Vec<Change>,
    #[serde(default)]
    pub metrics: Vec<MetricRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub impact_from: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<i64>,
    #[serde(default)]
    pub allow_unapproved: bool,
}

/// One metric's number, on the real values or the what-if's.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetricValue {
    pub metric: String,
    pub kind: String,
    pub expr_words: String,
    pub value: Option<f64>,
    pub unit: Option<String>,
    pub n_entities: usize,
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScenarioValue {
    #[serde(flatten)]
    pub value: MetricValue,
    /// The what-if's value minus the real one; None when either has no value.
    pub delta: Option<f64>,
    /// `delta` as a percentage of the real value; None when that is 0 or missing.
    pub delta_pct: Option<f64>,
}

/// One property value the what-if changed: the real value as of the run, and the what-if's.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Changed {
    pub entity_id: String,
    pub name: String,
    pub kind: String,
    pub property: String,
    pub before: Option<Value>,
    pub after: Value,
    pub unit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Removed {
    pub entity_id: String,
    pub name: String,
    pub kind: String,
}

/// An entity within reach of a changed one (or of `impact_from`). Plain data: whether the change
/// affects it is the model's call.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reached {
    pub entity_id: String,
    pub name: String,
    pub kind: String,
    /// Relations from the nearest start.
    pub depth: u8,
    /// The last relation on the way, as a sentence: "Truck 7 carries Shipment 88".
    pub via: String,
    /// The start it was reached from, and the relation ids from there to it.
    pub from: String,
    pub path: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScenarioOut {
    pub vault: String,
    pub as_of: String,
    /// The changes in plain words, in order: "Product price up 10% on 2 products supplied by Riverside Roasters".
    pub changes: Vec<String>,
    pub baseline: Vec<MetricValue>,
    pub scenario: Vec<ScenarioValue>,
    /// At most [`MAX_LISTED`].
    pub changed: Vec<Changed>,
    pub n_changed: usize,
    pub removed: Vec<Removed>,
    pub reached: Vec<Reached>,
    pub reached_truncated: bool,
    pub warnings: Vec<String>,
}

fn unknown_entity(id: &str) -> AppError {
    AppError::coded(ErrorCode::EntityNotFound, format!("unknown entity {id} in this vault: use an entity id from entities_search"))
}

fn words(s: &str) -> String {
    s.replace('_', " ")
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

pub(super) fn plural(n: usize, kind: &str) -> String {
    let w = words(kind);
    if n == 1 { w } else { format!("{w}s") }
}

/// Relation labels match ignoring case, and `_` matches a space: "supplied_by" finds "supplied by".
fn same_label(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim().to_lowercase().replace('_', " ");
    norm(a) == norm(b)
}

fn tidy(v: f64) -> f64 {
    let r = (v * 1e9).round() / 1e9;
    if r == 0.0 { 0.0 } else { r }
}

#[derive(Debug, Clone, Copy)]
enum How {
    Scale(f64),
    Shift(f64),
}

/// A bulk change's entities: those of the kind the filters and the relation pick.
struct Pick {
    selector: Selector,
    /// `entity_ids`, when given.
    only: Option<HashSet<String>>,
    /// Ids linked to the `related_to` entity, and how they are linked to it.
    related: Option<Related>,
}

pub(super) struct Related {
    pub(super) ids: HashSet<String>,
    /// "supplied by Riverside Roasters"
    pub(super) said: String,
    pub(super) target: String,
    pub(super) label: Option<String>,
}

impl Related {
    /// The entities linked to `r.entity_id` (one of `named`, so in the vault), by relations labelled
    /// `r.label` when given.
    pub(super) async fn load(db: &OrgDb, vault: &RecordId, named: &Named, r: &RelatedTo, warnings: &mut Vec<String>) -> AppResult<Self> {
        let target = named.get(&r.entity_id)?;
        let (edges, cut) = relations_of(db, vault, &target.id).await?;
        if cut {
            warnings.push(format!("{} has more relations than one call reads: only the first ones picked entities", target.name));
        }
        let label = r.label.as_deref().map(str::trim).filter(|l| !l.is_empty());
        let edges: Vec<_> = edges.into_iter().filter(|e| label.is_none_or(|l| same_label(&e.label, l))).collect();
        let ids: HashSet<String> = edges.iter().map(|e| if e.from == target.id { e.to.to_string() } else { e.from.to_string() }).collect();
        // "supplied by Riverside Roasters" when the picked entities are the relation's `from` end
        let said = match (label, edges.first().filter(|_| edges.iter().all(|e| e.to == target.id))) {
            (Some(_), Some(e)) => format!("{} {}", words(&e.label), target.name),
            (Some(l), None) => format!("linked to {} by {}", target.name, words(l)),
            (None, _) => format!("linked to {}", target.name),
        };
        Ok(Related { ids, said, target: target.name.clone(), label: label.map(words) })
    }

    /// The warning for a kind none of whose entities are linked to the target.
    pub(super) fn none_linked(&self, kind: &str) -> String {
        let by = self.label.as_ref().map(|l| format!(" by {l} links")).unwrap_or_default();
        format!("no {} are linked to {}{by}", plural(2, kind), self.target)
    }
}

/// One change, checked.
enum Step {
    Set { entity: NodeRow, kind: Kind, property: String, datatype: Datatype, unit: Option<String>, held: Held },
    Bulk { kind: Kind, property: String, datatype: Datatype, unit: Option<String>, how: How, pick: Pick },
    Remove { entity: NodeRow },
}

/// The entities a scenario names, loaded once; each must be in the scenario's vault.
pub(super) struct Named(HashMap<String, NodeRow>);

impl Named {
    pub(super) async fn load<'a>(db: &OrgDb, vault: &RecordId, ids: impl IntoIterator<Item = &'a str>) -> AppResult<Self> {
        let mut rids = Vec::new();
        for id in ids {
            let rid = crate::rid::parse(id).map_err(|_| unknown_entity(id))?;
            require_entity_id(&rid).map_err(|_| unknown_entity(id))?;
            rids.push(rid);
        }
        let mut nodes = load_nodes(db, rids).await?;
        // an entity in another vault is as unknown here as one that does not exist
        nodes.retain(|_, n| &n.vault == vault);
        Ok(Named(nodes))
    }

    pub(super) fn get(&self, id: &str) -> AppResult<NodeRow> {
        let key = crate::rid::parse(id).map(|r| r.to_string()).unwrap_or_default();
        self.0.get(&key).cloned().ok_or_else(|| unknown_entity(id))
    }
}

/// Every entity id a scenario names: changed, removed, picked, related to, or traced from.
fn named_ids(args: &ScenarioArgs) -> impl Iterator<Item = &str> {
    args.changes
        .iter()
        .filter_map(|c| match c {
            Change::Set(s) => Some(s.entity_id.as_str()),
            Change::Remove(r) => Some(r.entity_id.as_str()),
            Change::Scale(_) | Change::Shift(_) => None,
        })
        .chain(args.changes.iter().flat_map(|c| {
            let (only, related) = match c {
                Change::Scale(ScaleChange { entity_ids, related_to, .. }) | Change::Shift(ShiftChange { entity_ids, related_to, .. }) => (entity_ids.as_slice(), related_to.as_ref()),
                _ => (&[][..], None),
            };
            only.iter().map(String::as_str).chain(related.map(|r| r.entity_id.as_str()))
        }))
        .chain(args.impact_from.iter().map(String::as_str))
}

/// Whether everything the scenario names is in `vault`: its entities, its metrics and the properties
/// its bulk changes change (whatever their status). A what-if that failed in the default vault is
/// pointed at the readable vaults this holds for.
async fn fits(db: &OrgDb, vault: RecordId, args: &ScenarioArgs) -> AppResult<bool> {
    let ids: Vec<RecordId> = named_ids(args).filter_map(|id| crate::rid::parse(id).ok()).collect();
    let nodes = load_nodes(db, ids.clone()).await?;
    if ids.iter().any(|id| nodes.get(&id.to_string()).is_none_or(|n| n.vault != vault)) {
        return Ok(false);
    }
    for m in &args.metrics {
        let (name, kind) = match m {
            MetricRef::Name(n) => (n.as_str(), None),
            MetricRef::Named { metric, kind } => (metric.as_str(), kind.as_deref()),
        };
        if !metric::exists(db, vault.clone(), name, kind).await? {
            return Ok(false);
        }
    }
    for c in &args.changes {
        let (Change::Scale(ScaleChange { kind, property, .. }) | Change::Shift(ShiftChange { kind, property, .. })) = c else { continue };
        let Ok(kind) = Kind::parse(kind) else { return Ok(false) };
        if !metric::props_of(db, &vault, &kind).await?.contains_key(property.trim()) {
            return Ok(false);
        }
    }
    Ok(true)
}

struct Wanted<'a> {
    entity_ids: &'a [String],
    filters: &'a [Filter],
    related_to: Option<&'a RelatedTo>,
}

async fn pick(db: &OrgDb, vault: &RecordId, named: &Named, kind: &Kind, wanted: Wanted<'_>, warnings: &mut Vec<String>) -> AppResult<Pick> {
    let Wanted { entity_ids, filters, related_to } = wanted;
    let selector = Selector::new(db, vault, kind, filters).await?;
    if entity_ids.len() > MAX_TOUCHED {
        return Err(AppError::bad_request(format!("entity_ids lists at most {MAX_TOUCHED} entities")));
    }
    let only = if entity_ids.is_empty() {
        None
    } else {
        let mut ids = HashSet::new();
        for id in entity_ids {
            let n = named.get(id)?;
            if n.kind() != kind.as_str() {
                return Err(AppError::bad_request(format!("{} ({id}) is a {}, not a {}", n.name, words(&n.kind()), words(kind.as_str()))));
            }
            ids.insert(n.id.to_string());
        }
        Some(ids)
    };
    let related = match related_to {
        None => None,
        Some(r) => Some(Related::load(db, vault, named, r, warnings).await?),
    };
    Ok(Pick { selector, only, related })
}

async fn check_change(db: &OrgDb, vault: &RecordId, named: &Named, change: &Change, warnings: &mut Vec<String>) -> AppResult<Step> {
    let bulk = |kind: &str, property: &str, how: How| (kind.to_string(), property.to_string(), how);
    let (kind, property, how, wanted) = match change {
        Change::Set(s) => {
            let entity = named.get(&s.entity_id)?;
            let kind = Kind::parse(&entity.kind())?;
            let checked = check_value(db, vault, &kind, &s.property, &s.value, s.unit.as_deref()).await?;
            if let Some(m) = &checked.mapped {
                warnings.push(format!("{:?} was merged into {:?} for this vault, so the what-if changed {:?}", m.from, m.to, m.to));
            }
            let held = Held { stored: checked.stored, unit: checked.unit };
            return Ok(Step::Set { entity, kind, property: checked.row.name, datatype: checked.datatype, unit: checked.row.unit, held });
        }
        Change::Remove(r) => return Ok(Step::Remove { entity: named.get(&r.entity_id)? }),
        Change::Scale(s) => {
            if !s.factor.is_finite() || !(0.0..=MAX_FACTOR).contains(&s.factor) {
                return Err(AppError::bad_request(format!("a scale factor is between 0 and {MAX_FACTOR} (1.1 is up 10%), got {}", s.factor)));
            }
            let (k, p, h) = bulk(&s.kind, &s.property, How::Scale(s.factor));
            (k, p, h, Wanted { entity_ids: &s.entity_ids, filters: &s.filters, related_to: s.related_to.as_ref() })
        }
        Change::Shift(s) => {
            if !s.delta.is_finite() {
                return Err(AppError::bad_request("a shift delta is a finite number, like 0.3 or -2"));
            }
            let (k, p, h) = bulk(&s.kind, &s.property, How::Shift(s.delta));
            (k, p, h, Wanted { entity_ids: &s.entity_ids, filters: &s.filters, related_to: s.related_to.as_ref() })
        }
    };
    let kind = Kind::parse(&kind)?;
    let (row, mapped) = resolve_approved(db, vault, &kind, &PropertyName::parse(&property)?).await?;
    let datatype = Datatype::parse(&row.datatype)?;
    if !datatype.numeric() {
        let verb = if matches!(how, How::Scale(_)) { "scale" } else { "shift" };
        return Err(AppError::bad_request(format!(
            "{verb} changes a number, integer, money or percent property; {:?} on {:?} is {}: use set for one entity instead",
            row.name,
            kind.as_str(),
            datatype.as_str()
        )));
    }
    if let Some(m) = &mapped {
        warnings.push(format!("{:?} was merged into {:?} for this vault, so the what-if changed {:?}", m.from, m.to, m.to));
    }
    let pick = pick(db, vault, named, &kind, wanted, warnings).await?;
    Ok(Step::Bulk { kind, property: row.name, datatype, unit: row.unit, how, pick })
}

/// The what-if's picture so far: the overlay over the real values.
fn current<'a>(overlay: &'a Overlay, base: &'a Values, entity: &str, property: &str) -> Option<&'a Held> {
    overlay.values.get(entity).and_then(|m| m.get(property)).or_else(|| base.get(entity).and_then(|m| m.get(property)))
}

fn shown(datatype: Datatype, held: &Held, unit: Option<&str>) -> String {
    match (datatype.numeric(), held.stored.num) {
        (true, Some(n)) => number(n, held.unit.as_deref().or(unit)),
        _ => to_json(datatype, &held.stored).map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string())).unwrap_or_default(),
    }
}

/// Runs a what-if for `owner`: nothing is written, and the call stops after its time budget. One
/// that names no vault and fails in the default vault names the readable vaults that hold what it
/// names.
pub async fn run(db: &OrgDb, owner: &RecordId, args: &ScenarioArgs) -> AppResult<ScenarioOut> {
    let vault_id = args
        .vault_id
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| crate::rid::parse(v).map_err(|_| AppError::coded(ErrorCode::VaultNotFound, "Vault not found.")))
        .transpose()?;
    let vault = resolve(db, owner, vault_id.as_ref(), Action::ReadMemories).await?;
    let out = match tokio::time::timeout(BUDGET, run_within_budget(db, &vault, args)).await {
        Ok(out) => out,
        Err(_) => Err(AppError::bad_request(format!(
            "the what-if took longer than {} seconds: change fewer entities or ask for fewer metrics",
            BUDGET.as_secs()
        ))),
    };
    match out {
        Err(e) if vault_id.is_none() && !fits(db, vault.clone(), args).await? => {
            let homes = super::scope::elsewhere(db, owner, &vault, |v| fits(db, v, args)).await?;
            Err(super::scope::found_in(e, "Everything it names", &homes))
        }
        out => out,
    }
}

async fn run_within_budget(db: &OrgDb, vault: &RecordId, args: &ScenarioArgs) -> AppResult<ScenarioOut> {
    let vault = vault.clone();
    if args.changes.is_empty() || args.changes.len() > MAX_CHANGES {
        return Err(AppError::bad_request(format!("a what-if makes 1 to {MAX_CHANGES} changes, got {}", args.changes.len())));
    }
    if args.metrics.len() > MAX_METRICS {
        return Err(AppError::bad_request(format!("a what-if runs at most {MAX_METRICS} metrics")));
    }
    if args.impact_from.len() > MAX_IMPACT_FROM {
        return Err(AppError::bad_request(format!("impact_from lists at most {MAX_IMPACT_FROM} entities")));
    }
    let as_of = metric::parse_as_of(args.as_of.as_deref())?;
    let named = Named::load(db, &vault, named_ids(args)).await?;
    let mut warnings: Vec<String> = Vec::new();

    let mut steps = Vec::new();
    for (i, c) in args.changes.iter().enumerate() {
        let step = check_change(db, &vault, &named, c, &mut warnings).await.map_err(|e| AppError { message: format!("change {}: {}", i + 1, e.message), ..e })?;
        steps.push(step);
    }
    // the metrics are checked before any values are read, so a bad name fails fast
    let mut prepared = Vec::new();
    for m in &args.metrics {
        let (metric, kind) = match m {
            MetricRef::Name(n) => (n.clone(), None),
            MetricRef::Named { metric, kind } => (metric.clone(), kind.clone()),
        };
        let eval = EvalArgs { metric, kind, as_of: args.as_of.clone(), allow_unapproved: args.allow_unapproved, ..EvalArgs::default() };
        prepared.push(metric::prepare(db, &vault, &eval).await?);
    }

    // each kind a change touches: its entities and the values the changes read
    let mut reads: BTreeMap<String, (Kind, BTreeSet<String>)> = BTreeMap::new();
    for s in &steps {
        let (kind, names): (&Kind, Vec<String>) = match s {
            Step::Set { kind, property, .. } => (kind, vec![property.clone()]),
            Step::Bulk { kind, property, pick, .. } => (kind, std::iter::once(property.clone()).chain(pick.selector.properties().cloned()).collect()),
            Step::Remove { .. } => continue,
        };
        reads.entry(kind.as_str().to_string()).or_insert_with(|| (kind.clone(), BTreeSet::new())).1.extend(names);
    }
    let mut loaded: HashMap<String, (Vec<String>, Values)> = HashMap::new();
    for (name, (kind, names)) in &reads {
        loaded.insert(name.clone(), metric::load_values(db, &vault, kind, names, as_of).await?);
    }

    let mut overlay = Overlay::default();
    // (entity, property) in the order first changed, with what the result needs to show it
    let mut touched: Vec<(String, String, String, Datatype, Option<String>)> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut changes = Vec::new();
    let mut removed = Vec::new();
    let mut note = |e: &str, kind: &Kind, p: &str, dt: Datatype, unit: Option<String>| {
        if seen.insert((e.to_string(), p.to_string())) {
            touched.push((e.to_string(), kind.as_str().to_string(), p.to_string(), dt, unit));
        }
    };
    for step in &steps {
        match step {
            Step::Set { entity, kind, property, datatype, unit, held } => {
                let id = entity.id.to_string();
                changes.push(format!("{} {} set to {}", entity.name, words(property), shown(*datatype, held, unit.as_deref())));
                overlay.values.entry(id.clone()).or_default().insert(property.clone(), held.clone());
                note(&id, kind, property, *datatype, held.unit.clone().or_else(|| unit.clone()));
            }
            Step::Remove { entity } => {
                overlay.removed.insert(entity.id.to_string());
                changes.push(format!("{} removed", entity.name));
                removed.push(Removed { entity_id: entity.id.to_string(), name: entity.name.clone(), kind: entity.kind() });
            }
            Step::Bulk { kind, property, datatype, unit, how, pick } => {
                let (entities, base) = &loaded[kind.as_str()];
                let mut picked: Vec<(String, f64, Option<String>)> = Vec::new();
                let (mut no_value, mut other_unit) = (0usize, 0usize);
                for e in entities {
                    let outside = |set: Option<&HashSet<String>>| set.is_some_and(|ids| !ids.contains(e));
                    if overlay.removed.contains(e) || outside(pick.only.as_ref()) || outside(pick.related.as_ref().map(|r| &r.ids)) {
                        continue;
                    }
                    if !pick.selector.picks(|p| current(&overlay, base, e, p)) {
                        continue;
                    }
                    let Some(h) = current(&overlay, base, e, property).filter(|h| h.stored.num.is_some()) else {
                        no_value += 1;
                        continue;
                    };
                    // a shift is in the property's unit, so a value kept in another unit is left alone
                    if matches!(how, How::Shift(_)) && h.unit.is_some() && h.unit != *unit {
                        other_unit += 1;
                        continue;
                    }
                    picked.push((e.clone(), h.stored.num.unwrap_or_default(), h.unit.clone()));
                }
                let matched = picked.len();
                if matched > MAX_TOUCHED {
                    warnings.push(format!(
                        "this change picks {matched} {}; a what-if changes at most {MAX_TOUCHED}, so the rest keep their real {} and the numbers count them unchanged",
                        plural(matched, kind.as_str()),
                        words(property)
                    ));
                    picked.truncate(MAX_TOUCHED);
                }
                if no_value > 0 {
                    warnings.push(format!("{no_value} picked {} have no {} value, so the change left them out", plural(no_value, kind.as_str()), words(property)));
                }
                if other_unit > 0 {
                    warnings.push(format!("{other_unit} {} keep {} in another unit than {}, so the shift left them alone", plural(other_unit, kind.as_str()), words(property), unit.as_deref().unwrap_or("the property's")));
                }
                let mut rounded = false;
                for (e, n, held_unit) in &picked {
                    let mut v = match how {
                        How::Scale(f) => n * f,
                        How::Shift(d) => n + d,
                    };
                    if *datatype == Datatype::Integer {
                        rounded |= v.fract() != 0.0;
                        v = v.round();
                    }
                    let held = Held { stored: Stored { num: Some(tidy(v)), ..Stored::default() }, unit: held_unit.clone() };
                    overlay.values.entry(e.clone()).or_default().insert(property.clone(), held);
                    note(e, kind, property, *datatype, held_unit.clone().or_else(|| unit.clone()));
                }
                if rounded {
                    warnings.push(format!("{} is a whole number, so the changed values were rounded", words(property)));
                }
                let amount = match how {
                    How::Scale(f) if *f >= 1.0 => format!("up {}%", number(tidy((f - 1.0) * 100.0), None)),
                    How::Scale(f) => format!("down {}%", number(tidy((1.0 - f) * 100.0), None)),
                    How::Shift(d) if *d >= 0.0 => format!("up {}", number(*d, unit.as_deref())),
                    How::Shift(d) => format!("down {}", number(-d, unit.as_deref())),
                };
                let one = match (&pick.only, picked.as_slice()) {
                    (Some(_), [(e, ..)]) => named.0.get(e).map(|n| n.name.clone()),
                    _ => None,
                };
                // a related_to entity none of the kind is linked to: say that, not "on 0 products"
                if let Some(r) = pick.related.as_ref().filter(|r| !entities.iter().any(|e| r.ids.contains(e))) {
                    let kinds = plural(2, kind.as_str());
                    let said = format!("{} {} {amount}: no {kinds} are linked to {}", capital(&words(kind.as_str())), words(property), r.target);
                    warnings.push(format!("{}, so \"{said}\" changed nothing: pick the {kinds} with filters or entity_ids instead, or link them first", r.none_linked(kind.as_str())));
                    changes.push(said);
                    continue;
                }
                let mut said = match one {
                    Some(name) => format!("{name} {} {amount}", words(property)),
                    None => format!("{} {} {amount} on {} {}", capital(&words(kind.as_str())), words(property), picked.len(), plural(picked.len(), kind.as_str())),
                };
                if let Some(r) = &pick.related {
                    said.push_str(&format!(" {}", r.said));
                }
                if let Some(w) = pick.selector.words() {
                    said.push_str(&format!(" where {w}"));
                }
                if picked.is_empty() {
                    warnings.push(format!("change \"{said}\" matched nothing, so it changed nothing"));
                }
                changes.push(said);
            }
        }
    }

    let mut baseline = Vec::new();
    let mut scenario = Vec::new();
    for p in &prepared {
        let real = p.run(None)?;
        let what_if = p.run(Some(&overlay))?;
        for w in real.warnings.iter().chain(what_if.warnings.iter().filter(|w| !real.warnings.contains(w))) {
            let w = format!("{}: {w}", words(&real.metric));
            if !warnings.contains(&w) {
                warnings.push(w);
            }
        }
        let value = |o: &metric::MetricEvalOut| MetricValue {
            metric: o.metric.clone(),
            kind: o.kind.clone(),
            expr_words: o.expr_words.clone(),
            value: o.value,
            unit: o.unit.clone(),
            n_entities: o.n_entities,
            status: o.status,
        };
        let delta = match (real.value, what_if.value) {
            (Some(a), Some(b)) => Some(tidy(b - a)),
            _ => None,
        };
        let delta_pct = match (real.value, delta) {
            (Some(a), Some(d)) if a != 0.0 => Some((d / a.abs() * 10_000.0).round() / 100.0),
            _ => None,
        };
        baseline.push(value(&real));
        scenario.push(ScenarioValue { value: value(&what_if), delta, delta_pct });
    }
    if !scenario.is_empty() && scenario.iter().zip(&baseline).all(|(s, b)| s.value.value == b.value && s.value.n_entities == b.n_entities) {
        warnings.push("none of these metrics moved: they do not read the changed values or entities".into());
    }
    if args.metrics.is_empty() {
        warnings.push("no metrics were asked for, so this lists only the changes and what they reach".into());
    }

    let n_changed = touched.len();
    let listed: Vec<_> = touched.iter().take(MAX_LISTED).collect();
    let names = load_nodes(db, listed.iter().filter_map(|(e, ..)| crate::rid::parse(e).ok()).collect()).await?;
    let changed = listed
        .iter()
        .filter_map(|(e, kind, p, dt, unit)| {
            let base = loaded.get(kind).and_then(|(_, v)| v.get(e)).and_then(|m| m.get(p));
            let after = overlay.values.get(e).and_then(|m| m.get(p))?;
            Some(Changed {
                entity_id: e.clone(),
                name: names.get(e).map(|n| n.name.clone()).unwrap_or_default(),
                kind: kind.clone(),
                property: p.clone(),
                before: base.and_then(|h| to_json(*dt, &h.stored)),
                after: to_json(*dt, &after.stored)?,
                unit: unit.clone(),
            })
        })
        .collect();

    // reached: from the removed entities, then impact_from, then the changed ones
    let mut starts: Vec<RecordId> = Vec::new();
    let mut add = |id: &str| {
        if let Ok(r) = crate::rid::parse(id)
            && !starts.contains(&r)
        {
            starts.push(r);
        }
    };
    removed.iter().for_each(|r| add(&r.entity_id));
    for id in &args.impact_from {
        add(&named.get(id)?.id.to_string());
    }
    let mut once = HashSet::new();
    let ids: Vec<&String> = touched.iter().map(|(e, ..)| e).filter(|e| once.insert(*e)).collect();
    if ids.len() > MAX_TRACED {
        warnings.push(format!("reached is traced from the first {MAX_TRACED} changed entities, not all {}", ids.len()));
    }
    ids.into_iter().take(MAX_TRACED).for_each(|e| add(e));
    let depth = args.depth.unwrap_or(DEFAULT_DEPTH as i64).clamp(1, MAX_DEPTH as i64) as u8;
    let walked = walk(db, &vault, &starts, depth).await?;
    let mut nodes = load_nodes(db, walked.order.clone()).await?;
    nodes.retain(|_, n| n.vault == vault);
    let name = |id: &str| nodes.get(id).map(|n| n.name.clone()).unwrap_or_default();
    let mut reached: Vec<Reached> = walked
        .order
        .iter()
        .map(|r| r.to_string())
        .filter(|id| walked.hops.get(id).is_some_and(|h| *h > 0) && nodes.contains_key(id))
        .filter_map(|id| {
            let mut path = Vec::new();
            let mut at = id.clone();
            while walked.hops.get(&at).is_some_and(|h| *h > 0) {
                let e = walked.via.get(&at)?;
                path.push(e.id.to_string());
                at = if e.to.to_string() == at { e.from.to_string() } else { e.to.to_string() };
            }
            path.reverse();
            let last = walked.via.get(&id)?;
            let n = &nodes[&id];
            Some(Reached {
                via: format!("{} {} {}", name(&last.from.to_string()), words(&last.label), name(&last.to.to_string())),
                entity_id: id.clone(),
                name: n.name.clone(),
                kind: n.kind(),
                depth: walked.hops[&id],
                from: at,
                path,
            })
        })
        .collect();
    reached.sort_by(|a, b| (a.depth, &a.name).cmp(&(b.depth, &b.name)));

    Ok(ScenarioOut {
        vault: vault.to_string(),
        as_of: as_of.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        changes,
        baseline,
        scenario,
        changed,
        n_changed,
        removed,
        reached,
        reached_truncated: walked.truncated,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_change_is_one_of_four_shapes_and_nothing_else() {
        let read = |v: Value| serde_json::from_value::<Change>(v);
        assert!(matches!(read(json!({"set": {"entity_id": "entity:a", "property": "price", "value": 5}})).unwrap(), Change::Set(_)));
        assert!(matches!(read(json!({"scale": {"kind": "product", "property": "price", "factor": 1.1}})).unwrap(), Change::Scale(_)));
        assert!(matches!(read(json!({"shift": {"kind": "product", "property": "price", "delta": 0.3}})).unwrap(), Change::Shift(_)));
        assert!(matches!(read(json!({"remove": {"entity_id": "entity:a"}})).unwrap(), Change::Remove(_)));
        assert!(read(json!({"delete": {"entity_id": "entity:a"}})).is_err());
        assert!(read(json!({"scale": {"kind": "product", "property": "price", "factor": 1.1, "sql": "x"}})).is_err());
        let m: Vec<MetricRef> = serde_json::from_value(json!(["avg_price", {"metric": "late", "kind": "shipment"}])).unwrap();
        assert!(matches!(&m[1], MetricRef::Named { kind: Some(k), .. } if k == "shipment"));
    }

    #[test]
    fn labels_match_with_spaces_or_underscores() {
        assert!(same_label("supplied by", "Supplied_By"));
        assert!(!same_label("supplied by", "supplies"));
        assert_eq!(tidy(34.5 * 1.1), 37.95);
        assert_eq!(plural(2, "product"), "products");
        assert_eq!(capital("product"), "Product");
    }
}
