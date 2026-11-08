//! Certified metrics on a vault's kinds (migration 0035).
//!
//! A metric (`avg_price` on `product`: the average of its `price`) is data the vault's ontology
//! defines, never a field in this code. Its `expr` is a typed aggregate over the kind's typed
//! properties, checked against them when it is proposed, decided and run: the server compiles it
//! itself, so no query text ever comes from an agent, a model or a person. Only an approved metric runs
//! by name; a proposed one runs only when the caller asks for an unapproved run, and the result says so.
//! Every number comes back with the entities it was computed from.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::Datelike;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use super::property::{clean_synonyms, clean_unit, parse_value, property_rid, to_json, Datatype, PropertyName, PropertyRow, Stored, ValidProperty};
use super::{clean_text, decider, no_ontology_table, resolve, view, Kind, OntologyOut, Origin, Status};
use crate::authz::Action;
use crate::entities::service::kind_table;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::store::ontology as q;
use crate::tx::with_retry_dup;

/// Most filters one expression holds, counting a ratio's sides; and most extra filters per run.
pub const MAX_FILTERS: usize = 10;
/// Most values one `in` filter lists.
const MAX_IN_VALUES: usize = 20;
/// Most entity ids a result lists.
pub const MAX_ENTITY_IDS: usize = 50;
/// Most periods a series holds.
const MAX_PERIODS: usize = 400;
const MAX_FILTER_TEXT: usize = 200;

/// How a metric combines its entities' values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Agg {
    Sum,
    Count,
    Avg,
    Min,
    Max,
    CountDistinct,
    /// One sub-expression divided by another.
    Ratio,
}

impl Agg {
    fn as_str(self) -> &'static str {
        match self {
            Agg::Sum => "sum",
            Agg::Count => "count",
            Agg::Avg => "avg",
            Agg::Min => "min",
            Agg::Max => "max",
            Agg::CountDistinct => "count_distinct",
            Agg::Ratio => "ratio",
        }
    }

    fn numeric_only(self) -> bool {
        matches!(self, Agg::Sum | Agg::Avg | Agg::Min | Agg::Max)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    In,
}

impl Op {
    fn as_str(self) -> &'static str {
        match self {
            Op::Eq => "eq",
            Op::Ne => "ne",
            Op::Gt => "gt",
            Op::Gte => "gte",
            Op::Lt => "lt",
            Op::Lte => "lte",
            Op::In => "in",
        }
    }

    fn ordered(self) -> bool {
        matches!(self, Op::Gt | Op::Gte | Op::Lt | Op::Lte)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Grain {
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

impl Grain {
    fn period(self, at: chrono::DateTime<chrono::Utc>) -> String {
        match self {
            Grain::Day => at.format("%Y-%m-%d").to_string(),
            Grain::Week => at.format("%G-W%V").to_string(),
            Grain::Month => at.format("%Y-%m").to_string(),
            Grain::Quarter => format!("{}-Q{}", at.year(), at.month0() / 3 + 1),
            Grain::Year => at.format("%Y").to_string(),
        }
    }

    fn word(self) -> &'static str {
        match self {
            Grain::Day => "day",
            Grain::Week => "week",
            Grain::Month => "month",
            Grain::Quarter => "quarter",
            Grain::Year => "year",
        }
    }
}

/// Keeps entities whose value of `property` compares to `value` by `op` (`in` takes a list).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    pub property: String,
    pub op: Op,
    /// In the property's datatype; a list for `in`.
    pub value: Value,
}

/// One side of a ratio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SubExpr {
    pub agg: Agg,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<Filter>,
}

/// What a metric computes, over the latest value of each property per entity. Never query text: an
/// unknown field is refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MetricExpr {
    pub agg: Agg,
    /// The measured property: needed by sum, avg, min, max and count_distinct; count counts the
    /// entities that have it, or every entity without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property: Option<String>,
    /// Ratio only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub numerator: Option<SubExpr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denominator: Option<SubExpr>,
    /// ANDed, and on a ratio applied to both sides.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filters: Vec<Filter>,
    /// A date or datetime property a series groups the entities by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_property: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grain: Option<Grain>,
}

fn metric_rid(vault: &RecordId, kind: &Kind, name: &PropertyName) -> RecordId {
    let parts = format!("{}|{}|{}", vault.to_string(), kind.as_str(), name.as_str());
    RecordId::from_table_key("ontology_metric", crate::tx::stable_key('m', &parts))
}

/// A metric name follows the property name rule.
fn metric_name(s: &str) -> AppResult<PropertyName> {
    PropertyName::parse(s).map_err(|e| AppError::bad_request(e.message.replace("property name", "metric name")))
}

fn words(name: &str) -> String {
    name.replace('_', " ")
}

// ---------------------------------------------------------------------------
// Checking an expression against the kind's properties
// ---------------------------------------------------------------------------

/// What the check needs of one property of the kind.
#[derive(Debug, Clone)]
pub(super) struct PropInfo {
    datatype: Datatype,
    unit: Option<String>,
    enum_values: Option<Vec<String>>,
    status: Status,
    merge_into: Option<String>,
}

impl PropInfo {
    fn from_row(r: &PropertyRow) -> AppResult<Self> {
        Ok(PropInfo {
            datatype: Datatype::parse(&r.datatype)?,
            unit: r.unit.clone(),
            enum_values: r.enum_values.clone(),
            status: Status::parse(&r.status)?,
            merge_into: r.merge_into.clone(),
        })
    }
}

type Props = HashMap<String, PropInfo>;

/// A filter value given as listed nowhere: the enum's values nearest it, so the caller can fix the
/// spelling rather than read nothing as 0.
#[derive(Debug, Clone, PartialEq)]
struct Unlisted {
    property: String,
    value: String,
    nearest: Vec<String>,
}

impl Unlisted {
    fn sentence(&self) -> String {
        format!(
            "no {:?} is {:?}: it is not one of its listed values (nearest: {})",
            self.property,
            self.value,
            self.nearest.join(", ")
        )
    }
}

enum FilterError {
    Bad(AppError),
    Unlisted(Unlisted),
}

impl From<AppError> for FilterError {
    fn from(e: AppError) -> Self {
        FilterError::Bad(e)
    }
}

/// One checked filter: values parsed as the property's datatype, kept as given for display.
#[derive(Debug, Clone)]
struct Cond {
    filter: Filter,
    datatype: Datatype,
    values: Vec<Stored>,
}

/// One checked side of a metric: what it combines, and its own filters.
#[derive(Debug, Clone)]
struct Side {
    agg: Agg,
    property: Option<String>,
    filters: Vec<Cond>,
}

/// A checked expression.
#[derive(Debug, Clone)]
struct Plan {
    /// One side, or a ratio's numerator then denominator.
    sides: Vec<Side>,
    ratio: bool,
    filters: Vec<Cond>,
    time: Option<(String, Grain)>,
}

/// Characters with a meaning in query text, which no filter value needs. Values are always parameters,
/// so this keeps an expression readable as data, not a guard on its own.
fn looks_like_query(s: &str) -> bool {
    [";", "--", "/*", "*/", "`", "$", "{", "}"].iter().any(|c| s.contains(c))
}

fn property_of<'a>(props: &'a Props, kind: &Kind, name: &str, role: &str) -> AppResult<(String, &'a PropInfo)> {
    let name = PropertyName::parse(name)?.as_str().to_string();
    let Some(p) = props.get(&name) else {
        let mut known: Vec<&str> = props.iter().filter(|(_, p)| p.status != Status::Rejected).map(|(n, _)| n.as_str()).collect();
        known.sort();
        let listed = if known.is_empty() { "it has none yet".to_string() } else { format!("its properties are {}", known.join(", ")) };
        return Err(AppError::bad_request(format!(
            "{role}: {name:?} is not a property of {:?} in this vault ({listed}): propose it with ontology_propose first",
            kind.as_str()
        )));
    };
    if p.status == Status::Rejected {
        let instead = p.merge_into.as_deref().map(|m| format!(": use {m:?}")).unwrap_or_default();
        return Err(AppError::bad_request(format!("{role}: the property {name:?} was rejected for this vault{instead}")));
    }
    Ok((name, p))
}

/// The listed values nearest `given`, by edit distance ignoring case.
fn nearest(given: &str, listed: &[String]) -> Vec<String> {
    let g = given.to_lowercase();
    let mut scored: Vec<(usize, &String)> = listed.iter().map(|v| (edit_distance(&g, &v.to_lowercase()), v)).collect();
    scored.sort();
    scored.into_iter().take(3).map(|(_, v)| v.clone()).collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + usize::from(ca != *cb)).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

fn check_filter(props: &Props, kind: &Kind, f: &Filter) -> Result<Cond, FilterError> {
    let (name, p) = property_of(props, kind, &f.property, "a filter")?;
    let dt = p.datatype;
    let orderable = dt.numeric() || matches!(dt, Datatype::Date | Datatype::Datetime);
    if f.op.ordered() && !orderable {
        return Err(AppError::bad_request(format!("a filter on {name:?} ({}) takes eq, ne or in, not {}", dt.as_str(), f.op.as_str())).into());
    }
    let given: Vec<&Value> = match (&f.value, f.op) {
        (Value::Array(items), Op::In) if !items.is_empty() && items.len() <= MAX_IN_VALUES => items.iter().collect(),
        (_, Op::In) => return Err(AppError::bad_request(format!("an in filter lists 1 to {MAX_IN_VALUES} values")).into()),
        (Value::Array(_), _) => return Err(AppError::bad_request(format!("only an in filter takes a list; {} takes one value", f.op.as_str())).into()),
        (v, _) => vec![v],
    };
    let mut values = Vec::new();
    let mut shown = Vec::new();
    for v in given {
        if let Some(s) = v.as_str() {
            if s.chars().count() > MAX_FILTER_TEXT {
                return Err(AppError::bad_request(format!("a filter value is at most {MAX_FILTER_TEXT} characters")).into());
            }
            if looks_like_query(s) {
                return Err(AppError::bad_request(format!(
                    "the filter value {s:?} looks like query text: a value cannot hold ; -- /* */ ` $ {{ }} (values are compared as data, never run)"
                ))
                .into());
            }
        }
        if dt == Datatype::Enum {
            let listed = p.enum_values.clone().unwrap_or_default();
            let text = v.as_str().map(str::trim).unwrap_or_default();
            if !listed.iter().any(|l| l.eq_ignore_ascii_case(text)) {
                return Err(FilterError::Unlisted(Unlisted { property: name, value: text.to_string(), nearest: nearest(text, &listed) }));
            }
        }
        let stored = parse_value(dt, &name, p.enum_values.as_deref(), v)?;
        shown.push(to_json(dt, &stored).unwrap_or(Value::Null));
        values.push(stored);
    }
    let value = if f.op == Op::In { Value::Array(shown) } else { shown.into_iter().next().unwrap_or(Value::Null) };
    Ok(Cond { filter: Filter { property: name, op: f.op, value }, datatype: dt, values })
}

fn hard(e: FilterError) -> AppError {
    match e {
        FilterError::Bad(e) => e,
        FilterError::Unlisted(u) => AppError::bad_request(format!("{}; use one of its listed values", u.sentence())),
    }
}

fn check_side(props: &Props, kind: &Kind, agg: Agg, property: Option<&str>, filters: &[Filter], role: &str) -> AppResult<Side> {
    let property = property.map(str::trim).filter(|p| !p.is_empty());
    let property = match (agg, property) {
        (Agg::Ratio, _) => return Err(AppError::bad_request(format!("{role}: a ratio's sides are sum, count, avg, min, max or count_distinct"))),
        (Agg::Count, None) => None,
        (a, None) => return Err(AppError::bad_request(format!("{role}: {} needs a property", a.as_str()))),
        (a, Some(p)) => {
            let (name, info) = property_of(props, kind, p, role)?;
            if a.numeric_only() && !info.datatype.numeric() {
                return Err(AppError::bad_request(format!(
                    "{role}: {} needs a number, integer, money or percent property; {name:?} is {}",
                    a.as_str(),
                    info.datatype.as_str()
                )));
            }
            Some(name)
        }
    };
    let filters = filters.iter().map(|f| check_filter(props, kind, f).map_err(hard)).collect::<AppResult<Vec<_>>>()?;
    Ok(Side { agg, property, filters })
}

fn check(props: &Props, kind: &Kind, e: &MetricExpr) -> AppResult<Plan> {
    let sub_filters = e.numerator.iter().chain(&e.denominator).map(|s| s.filters.len()).sum::<usize>();
    if e.filters.len() + sub_filters > MAX_FILTERS {
        return Err(AppError::bad_request(format!("a metric has at most {MAX_FILTERS} filters")));
    }
    let (sides, ratio) = match e.agg {
        Agg::Ratio => {
            let (Some(n), Some(d)) = (&e.numerator, &e.denominator) else {
                return Err(AppError::bad_request("a ratio needs a numerator and a denominator"));
            };
            if e.property.is_some() {
                return Err(AppError::bad_request("a ratio measures through its numerator and denominator, not a property"));
            }
            let n = check_side(props, kind, n.agg, n.property.as_deref(), &n.filters, "the numerator")?;
            let d = check_side(props, kind, d.agg, d.property.as_deref(), &d.filters, "the denominator")?;
            (vec![n, d], true)
        }
        agg => {
            if e.numerator.is_some() || e.denominator.is_some() {
                return Err(AppError::bad_request("only a ratio has a numerator and a denominator"));
            }
            (vec![check_side(props, kind, agg, e.property.as_deref(), &[], "the metric")?], false)
        }
    };
    let filters = e.filters.iter().map(|f| check_filter(props, kind, f).map_err(hard)).collect::<AppResult<Vec<_>>>()?;
    let time = match (&e.time_property, e.grain) {
        (Some(t), grain) => {
            let (name, info) = property_of(props, kind, t, "the time property")?;
            if !matches!(info.datatype, Datatype::Date | Datatype::Datetime) {
                return Err(AppError::bad_request(format!("the time property must be a date or datetime; {name:?} is {}", info.datatype.as_str())));
            }
            Some((name, grain.unwrap_or(Grain::Month)))
        }
        (None, Some(_)) => return Err(AppError::bad_request("a grain needs a time_property to group by")),
        (None, None) => None,
    };
    Ok(Plan { sides, ratio, filters, time })
}

impl Plan {
    /// The expression as checked: names trimmed, enum values as listed, dates as YYYY-MM-DD.
    fn expr(&self) -> MetricExpr {
        let filters = |c: &[Cond]| c.iter().map(|c| c.filter.clone()).collect::<Vec<_>>();
        let sub = |s: &Side| SubExpr { agg: s.agg, property: s.property.clone(), filters: filters(&s.filters) };
        let (time_property, grain) = match &self.time {
            Some((t, g)) => (Some(t.clone()), Some(*g)),
            None => (None, None),
        };
        if self.ratio {
            MetricExpr {
                agg: Agg::Ratio,
                property: None,
                numerator: Some(sub(&self.sides[0])),
                denominator: Some(sub(&self.sides[1])),
                filters: filters(&self.filters),
                time_property,
                grain,
            }
        } else {
            MetricExpr { agg: self.sides[0].agg, property: self.sides[0].property.clone(), numerator: None, denominator: None, filters: filters(&self.filters), time_property, grain }
        }
    }

    /// Every property the plan reads.
    fn properties(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for s in &self.sides {
            out.extend(s.property.clone());
            out.extend(s.filters.iter().map(|c| c.filter.property.clone()));
        }
        out.extend(self.filters.iter().map(|c| c.filter.property.clone()));
        out.extend(self.time.iter().map(|(t, _)| t.clone()));
        out
    }
}

// ---------------------------------------------------------------------------
// The expression in plain words
// ---------------------------------------------------------------------------

fn value_words(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(value_words).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

fn filter_words(f: &Filter) -> String {
    let op = match f.op {
        Op::Eq => "is",
        Op::Ne => "is not",
        Op::Gt => "is over",
        Op::Gte => "is at least",
        Op::Lt => "is under",
        Op::Lte => "is at most",
        Op::In => "is one of",
    };
    format!("{} {op} {}", words(&f.property), value_words(&f.value))
}

fn side_words(kind: &str, agg: Agg, property: Option<&str>, filters: &[Filter]) -> String {
    let p = property.map(words).unwrap_or_default();
    let mut out = match (agg, property) {
        (Agg::Sum, _) => format!("Sum of {p}"),
        (Agg::Avg, _) => format!("Average {p}"),
        (Agg::Min, _) => format!("Lowest {p}"),
        (Agg::Max, _) => format!("Highest {p}"),
        (Agg::CountDistinct, _) => format!("Number of different {p} values"),
        (Agg::Count, Some(_)) => format!("Number of {} with a {p}", words(kind)),
        (Agg::Count, None) | (Agg::Ratio, _) => format!("Number of {}", words(kind)),
    };
    if !filters.is_empty() {
        out.push_str(" where ");
        out.push_str(&filters.iter().map(filter_words).collect::<Vec<_>>().join(" and "));
    }
    out
}

/// "Sum of price where status is late", "Number of order where status is late, divided by number of
/// order", with ", by month of ordered_on" for a series.
pub fn expr_words(kind: &str, e: &MetricExpr) -> String {
    let mut out = match (&e.numerator, &e.denominator) {
        (Some(n), Some(d)) if e.agg == Agg::Ratio => {
            let n = side_words(kind, n.agg, n.property.as_deref(), &n.filters);
            let d = side_words(kind, d.agg, d.property.as_deref(), &d.filters);
            let mut d = d.chars();
            let d = d.next().map(|c| c.to_lowercase().collect::<String>() + d.as_str()).unwrap_or_default();
            let mut s = format!("{n}, divided by {d}");
            if !e.filters.is_empty() {
                s.push_str(", both where ");
                s.push_str(&e.filters.iter().map(filter_words).collect::<Vec<_>>().join(" and "));
            }
            s
        }
        _ => side_words(kind, e.agg, e.property.as_deref(), &e.filters),
    };
    if let Some(t) = &e.time_property {
        out.push_str(&format!(", by {} of {}", e.grain.unwrap_or(Grain::Month).word(), words(t)));
    }
    out
}

// ---------------------------------------------------------------------------
// Proposals
// ---------------------------------------------------------------------------

/// A proposed metric, as an agent or the model sends it.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetricProposal {
    pub kind: String,
    pub name: String,
    pub expr: MetricExpr,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub synonyms: Vec<String>,
    /// Proposes folding this metric into another of the same kind (the one to keep).
    #[serde(default)]
    pub merge_into: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

pub(super) struct ValidMetric {
    kind: Kind,
    name: PropertyName,
    expr: MetricExpr,
    unit: Option<String>,
    description: Option<String>,
    synonyms: Vec<String>,
    merge_into: Option<PropertyName>,
    reason: Option<String>,
}

/// The property rows of `kind` in `vault`, by name. None on a database before migration 0034.
async fn props_of(db: &OrgDb, vault: &RecordId, kind: &Kind) -> AppResult<Props> {
    let rows: Vec<PropertyRow> = match q::PROPERTIES_OF_KIND.on(db).bind(("vault", vault.clone())).bind(("kind", kind.as_str().to_string())).await {
        Err(e) if no_ontology_table(&e) => return Ok(Props::new()),
        res => res?.take(0)?,
    };
    rows.iter().map(|r| Ok((r.name.clone(), PropInfo::from_row(r)?))).collect()
}

impl MetricProposal {
    /// Checked against `props`, the kind's properties (stored, plus any proposed in the same call).
    pub(super) fn validate(&self, props: &Props) -> AppResult<ValidMetric> {
        let kind = Kind::parse(&self.kind)?;
        let name = metric_name(&self.name)?;
        let plan = check(props, &kind, &self.expr).map_err(|e| AppError::bad_request(format!("metric {:?}: {}", name.as_str(), e.message)))?;
        let merge_into = self.merge_into.as_deref().map(str::trim).filter(|m| !m.is_empty()).map(metric_name).transpose()?;
        if merge_into.as_ref() == Some(&name) {
            return Err(AppError::bad_request(format!("{:?} cannot merge into itself", name.as_str())));
        }
        Ok(ValidMetric {
            synonyms: clean_synonyms(name.as_str(), &self.synonyms)?,
            expr: plan.expr(),
            kind,
            name,
            unit: clean_unit(self.unit.as_deref())?,
            description: clean_text("description", self.description.as_deref())?,
            merge_into,
            reason: clean_text("reason", self.reason.as_deref())?,
        })
    }
}

/// Checks `metrics` against the properties of their kinds in `vault` and the `properties` proposed
/// alongside them. One bad metric fails the lot.
pub(super) async fn validate_all(db: &OrgDb, vault: &RecordId, metrics: &[MetricProposal], properties: &[ValidProperty]) -> AppResult<Vec<ValidMetric>> {
    let mut by_kind: HashMap<String, Props> = HashMap::new();
    let mut out = Vec::new();
    for m in metrics {
        let kind = Kind::parse(&m.kind)?;
        if !by_kind.contains_key(kind.as_str()) {
            let mut props = props_of(db, vault, &kind).await?;
            for p in properties.iter().filter(|p| p.kind == kind) {
                props.entry(p.name.as_str().to_string()).or_insert_with(|| PropInfo {
                    datatype: p.datatype,
                    unit: p.unit.clone(),
                    enum_values: p.enum_values.clone(),
                    status: Status::Proposed,
                    merge_into: None,
                });
            }
            by_kind.insert(kind.as_str().to_string(), props);
        }
        out.push(m.validate(&by_kind[kind.as_str()])?);
    }
    Ok(out)
}

/// A proposed metric after the call, with the row's status (a decided one keeps its decision).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ProposedMetric {
    pub kind: String,
    pub name: String,
    pub status: Status,
    pub merge_into: Option<String>,
}

pub(super) async fn apply_metrics(db: &OrgDb, vault: &RecordId, metrics: Vec<ValidMetric>, origin: Origin) -> AppResult<Vec<ProposedMetric>> {
    let mut out = Vec::new();
    for m in metrics {
        let id = metric_rid(vault, &m.kind, &m.name);
        let expr = serde_json::to_value(&m.expr).map_err(|e| AppError::internal(format!("metric expr did not serialise: {e}")))?;
        let row: MetricRow = with_retry_dup(|| async {
            let mut res = q::PROPOSE_METRIC
                .on(db)
                .bind(("id", id.clone()))
                .bind(("vault", vault.clone()))
                .bind(("kind", m.kind.as_str().to_string()))
                .bind(("name", m.name.as_str().to_string()))
                .bind(("description", m.description.clone()))
                .bind(("synonyms", m.synonyms.clone()))
                .bind(("expr", expr.clone()))
                .bind(("unit", m.unit.clone()))
                .bind(("merge_into", m.merge_into.as_ref().map(|n| n.as_str().to_string())))
                .bind(("origin", origin.as_str().to_string()))
                .bind(("reason", m.reason.clone()))
                .await?;
            let rows: Vec<MetricRow> = res.take(q::PROPOSE_METRIC.slot)?;
            rows.into_iter().next().ok_or_else(|| surrealdb::Error::query("ontology metric upsert returned no row".into(), None))
        })
        .await?;
        out.push(ProposedMetric { kind: row.kind, name: row.name, status: Status::parse(&row.status)?, merge_into: row.merge_into });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Rows and the view
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, SurrealValue)]
struct MetricRow {
    kind: String,
    name: String,
    #[serde(default)]
    #[surreal(default)]
    description: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    synonyms: Vec<String>,
    #[serde(default)]
    #[surreal(default)]
    expr: Value,
    #[serde(default)]
    #[surreal(default)]
    unit: Option<String>,
    status: String,
    #[serde(default)]
    #[surreal(default)]
    merge_into: Option<String>,
    origin: String,
    #[serde(default)]
    #[surreal(default)]
    reason: Option<String>,
}

impl MetricRow {
    fn expr(&self) -> AppResult<MetricExpr> {
        serde_json::from_value(self.expr.clone()).map_err(|e| AppError::internal(format!("metric {:?} has an unreadable expr: {e}", self.name)))
    }
}

/// One metric of a kind, as the ontology view shows it. Its value is never here: `metric_eval` runs it.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MetricItem {
    pub name: String,
    pub description: Option<String>,
    pub synonyms: Vec<String>,
    pub expr: MetricExpr,
    /// The expression in plain words, e.g. "Sum of price where status is late".
    pub expr_words: String,
    pub unit: Option<String>,
    pub status: Status,
    /// Rejected: the metric of the same kind that runs instead. Proposed: the merge the proposer suggests.
    pub merge_into: Option<String>,
    pub origin: Origin,
    pub reason: Option<String>,
}

/// The vault's metrics by kind, each kind's by name. None on a database before migration 0035.
pub(super) async fn metrics_by_kind(db: &OrgDb, vault: &RecordId) -> AppResult<BTreeMap<String, Vec<MetricItem>>> {
    let rows: Vec<MetricRow> = match q::METRICS.on(db).bind(("vault", vault.clone())).await {
        Err(e) if no_ontology_table(&e) => return Ok(BTreeMap::new()),
        res => res?.take(0)?,
    };
    let mut out: BTreeMap<String, Vec<MetricItem>> = BTreeMap::new();
    for r in rows {
        let expr = r.expr()?;
        out.entry(r.kind.clone()).or_default().push(MetricItem {
            expr_words: expr_words(&r.kind, &expr),
            expr,
            status: Status::parse(&r.status)?,
            origin: Origin::parse(&r.origin)?,
            name: r.name,
            description: r.description,
            synonyms: r.synonyms,
            unit: r.unit,
            merge_into: r.merge_into,
            reason: r.reason,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Review: a person decides
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum MetricDecision {
    /// The metric runs by name from now on. Its properties must be approved first.
    Approve,
    /// Runs of the metric go to `merge_into` (a metric of the same kind) from now on; without it they
    /// are refused.
    Reject {
        #[serde(default)]
        merge_into: Option<String>,
    },
    /// Set or (blank) clear the description; the status stays as it is.
    Describe { description: String },
}

async fn load(db: &OrgDb, id: &RecordId) -> AppResult<Option<MetricRow>> {
    let rows: Vec<MetricRow> = match q::METRIC.on(db).bind(("id", id.clone())).await {
        Err(e) if no_ontology_table(&e) => return Ok(None),
        res => res?.take(0)?,
    };
    Ok(rows.into_iter().next())
}

fn no_metric(kind: &Kind, name: &PropertyName) -> AppError {
    AppError::coded(ErrorCode::ResourceNotFound, format!("no metric {:?} on the kind {:?} in this vault", name.as_str(), kind.as_str()))
}

pub async fn decide_metric(db: &OrgDb, user: &RecordId, vault: &RecordId, kind: &str, name: &str, decision: MetricDecision) -> AppResult<OntologyOut> {
    decider(db, user, vault).await?;
    let kind = Kind::parse(kind)?;
    let name = metric_name(name)?;
    let id = metric_rid(vault, &kind, &name);
    let Some(row) = load(db, &id).await? else { return Err(no_metric(&kind, &name)) };
    let (status, merge_into) = match decision {
        MetricDecision::Describe { description } => {
            let description = clean_text("description", Some(&description))?;
            with_retry_dup(|| async { q::DESCRIBE_METRIC.on(db).bind(("id", id.clone())).bind(("description", description.clone())).await }).await?;
            return view(db, vault).await;
        }
        MetricDecision::Approve => {
            // an approved metric must run: its expression still fits the kind, over approved properties
            let props = props_of(db, vault, &kind).await?;
            let plan = check(&props, &kind, &row.expr()?).map_err(|e| AppError::bad_request(format!("metric {:?} cannot be approved: {}", name.as_str(), e.message)))?;
            if let Some(p) = plan.properties().into_iter().find(|p| props.get(p).is_some_and(|i| i.status != Status::Approved)) {
                return Err(AppError::bad_request(format!("approve the property {p:?} of {:?} first: the metric reads it", kind.as_str())));
            }
            (Status::Approved, None)
        }
        MetricDecision::Reject { merge_into } => {
            let target = merge_into.as_deref().map(str::trim).filter(|m| !m.is_empty()).map(metric_name).transpose()?;
            if let Some(t) = &target {
                if t == &name {
                    return Err(AppError::bad_request(format!("{:?} cannot merge into itself", name.as_str())));
                }
                match load(db, &metric_rid(vault, &kind, t)).await? {
                    None => return Err(AppError::bad_request(format!("{:?} is not a metric of {:?}: propose it first", t.as_str(), kind.as_str()))),
                    Some(r) if Status::parse(&r.status)? == Status::Rejected => {
                        return Err(AppError::bad_request(format!("{:?} is rejected too: merge into a metric that is kept", t.as_str())))
                    }
                    Some(_) => {}
                }
            }
            (Status::Rejected, target)
        }
    };
    with_retry_dup(|| async {
        q::DECIDE_METRIC
            .on(db)
            .bind(("id", id.clone()))
            .bind(("vault", vault.clone()))
            .bind(("kind", kind.as_str().to_string()))
            .bind(("name", name.as_str().to_string()))
            .bind(("status", status.as_str().to_string()))
            .bind(("merge_into", merge_into.as_ref().map(|m| m.as_str().to_string())))
            .bind(("user", user.clone()))
            .await
    })
    .await?;
    view(db, vault).await
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

/// What `metric_eval` was asked to run.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalArgs {
    pub metric: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub vault_id: Option<String>,
    /// ANDed with the metric's own.
    #[serde(default)]
    pub filters: Vec<Filter>,
    /// The values as of this date or datetime (default now): each entity's latest value by then.
    #[serde(default)]
    pub as_of: Option<String>,
    #[serde(default)]
    pub group_by_grain: bool,
    #[serde(default)]
    pub allow_unapproved: bool,
}

/// A run of a rejected metric that went to its merge target instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MetricMapped {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Period {
    pub period: String,
    pub value: Option<f64>,
    /// Entities in the period that the value was computed from.
    pub n: usize,
}

/// A metric's number, with the entities it came from.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetricEvalOut {
    pub metric: String,
    pub kind: String,
    pub vault: String,
    pub description: Option<String>,
    pub expr_words: String,
    /// None when there is nothing to compute it from (an average of no values, a filter value listed
    /// nowhere, a denominator of 0) and in a series.
    pub value: Option<f64>,
    pub unit: Option<String>,
    /// The values were each entity's latest by this time, RFC 3339.
    pub as_of: String,
    /// Entities the number was computed from.
    pub n_entities: usize,
    /// Their ids, at most [`MAX_ENTITY_IDS`]; `truncated` says when there were more.
    pub entity_ids: Vec<String>,
    pub truncated: bool,
    /// The metric's review: `approved`, or `proposed` for a run with `allow_unapproved`.
    pub status: Status,
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metric_mapped: Option<MetricMapped>,
    /// With `group_by_grain`: the value per period of the time property, oldest first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub series: Option<Vec<Period>>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct InputRow {
    entity: RecordId,
    property: RecordId,
    #[serde(default)]
    #[surreal(default)]
    value_num: Option<f64>,
    #[serde(default)]
    #[surreal(default)]
    value_text: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    value_bool: Option<bool>,
    #[serde(default)]
    #[surreal(default)]
    value_date: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    unit: Option<String>,
}

/// One entity's value of one property as of the run.
#[derive(Debug, Clone)]
pub(super) struct Held {
    pub(super) stored: Stored,
    pub(super) unit: Option<String>,
}

/// Entity id to property name to its value.
pub(super) type Values = HashMap<String, HashMap<String, Held>>;

/// Values a what-if puts in place of the real ones, and entities it takes out, for [`Prepared::run`].
/// Entity ids are unique across kinds, so one overlay serves every metric of a scenario.
#[derive(Debug, Default, Clone)]
pub(super) struct Overlay {
    pub(super) values: Values,
    pub(super) removed: HashSet<String>,
}

fn not_found(name: &str, approved: &[String]) -> AppError {
    let named = if approved.is_empty() { " It has no approved metrics yet.".to_string() } else { format!(" Approved metrics: {}.", approved.join(", ")) };
    AppError::coded(
        ErrorCode::ResourceNotFound,
        format!("no metric {name:?} in this vault: propose one with ontology_propose (metrics) and wait for a vault owner to approve it.{named}"),
    )
}

async fn approved_names(db: &OrgDb, vault: &RecordId) -> AppResult<Vec<String>> {
    Ok(metrics_by_kind(db, vault)
        .await?
        .into_iter()
        .flat_map(|(kind, ms)| ms.into_iter().filter(|m| m.status == Status::Approved).map(move |m| format!("{} ({kind})", m.name)))
        .collect())
}

fn unapproved(name: &str, kind: &str) -> AppError {
    AppError::bad_request(format!(
        "the metric {name:?} on {kind:?} is proposed, not approved: a vault owner approves it in the Eunomia app (the vault's Kinds and links). To try it before that, pass allow_unapproved: true; the result then says it is unapproved"
    ))
}

/// Filters on one kind's properties, checked as a metric's own are, that pick the entities a what-if
/// changes.
pub(super) struct Selector {
    conds: Vec<Cond>,
}

impl Selector {
    /// Checked against `kind`'s properties in `vault`. A value listed nowhere is refused, naming the
    /// nearest listed ones, rather than quietly picking nothing.
    pub(super) async fn new(db: &OrgDb, vault: &RecordId, kind: &Kind, filters: &[Filter]) -> AppResult<Self> {
        if filters.len() > MAX_FILTERS {
            return Err(AppError::bad_request(format!("at most {MAX_FILTERS} filters")));
        }
        let props = props_of(db, vault, kind).await?;
        let conds = filters.iter().map(|f| check_filter(&props, kind, f).map_err(hard)).collect::<AppResult<Vec<_>>>()?;
        Ok(Selector { conds })
    }

    pub(super) fn properties(&self) -> impl Iterator<Item = &String> {
        self.conds.iter().map(|c| &c.filter.property)
    }

    /// Whether an entity whose values `held` returns passes every filter.
    pub(super) fn picks<'a>(&self, held: impl Fn(&str) -> Option<&'a Held>) -> bool {
        self.conds.iter().all(|c| matches(c, held(&c.filter.property)))
    }

    /// "status is Active and price is over 10", or None without filters.
    pub(super) fn words(&self) -> Option<String> {
        (!self.conds.is_empty()).then(|| self.conds.iter().map(|c| filter_words(&c.filter)).collect::<Vec<_>>().join(" and "))
    }
}

/// The end of a bare date's day, so "as of 2026-06-01" includes values dated that day.
pub(super) fn parse_as_of(s: Option<&str>) -> AppResult<chrono::DateTime<chrono::Utc>> {
    let Some(s) = s.map(str::trim).filter(|s| !s.is_empty()) else { return Ok(chrono::Utc::now()) };
    let at = crate::cache::search::parse_datetime("as_of", s)?;
    Ok(if s.contains('T') { at } else { at + chrono::Duration::days(1) - chrono::Duration::milliseconds(1) })
}

/// Whether a value passes a filter. An entity with no value "is not" anything, so it passes `ne` and
/// no other op.
fn matches(c: &Cond, held: Option<&Held>) -> bool {
    let Some(v) = held.map(|h| &h.stored) else { return c.filter.op == Op::Ne };
    let cmp = |want: &Stored| -> Option<std::cmp::Ordering> {
        match c.datatype {
            d if d.numeric() => v.num?.partial_cmp(&want.num?),
            Datatype::Date | Datatype::Datetime => Some(v.date?.into_inner().cmp(&want.date?.into_inner())),
            Datatype::Bool => Some(v.boolean?.cmp(&want.boolean?)),
            _ => Some(v.text.as_deref()?.to_lowercase().cmp(&want.text.as_deref()?.to_lowercase())),
        }
    };
    use std::cmp::Ordering::*;
    match c.filter.op {
        Op::Eq => cmp(&c.values[0]) == Some(Equal),
        Op::Ne => cmp(&c.values[0]) != Some(Equal),
        Op::Gt => cmp(&c.values[0]) == Some(Greater),
        Op::Gte => matches!(cmp(&c.values[0]), Some(Greater | Equal)),
        Op::Lt => cmp(&c.values[0]) == Some(Less),
        Op::Lte => matches!(cmp(&c.values[0]), Some(Less | Equal)),
        Op::In => c.values.iter().any(|w| cmp(w) == Some(Equal)),
    }
}

/// A side's value over `entities`, and the entities it used.
struct Computed {
    value: Option<f64>,
    used: Vec<String>,
    /// Values left out for a unit other than the property's.
    other_units: BTreeMap<String, usize>,
}

fn compute(side: &Side, filters: &[Cond], entities: &[&String], values: &Values, props: &Props) -> Computed {
    let unit = side.property.as_ref().and_then(|p| props.get(p)).and_then(|p| p.unit.clone());
    let mut used = Vec::new();
    let mut nums = Vec::new();
    let mut distinct = HashSet::new();
    let mut other_units: BTreeMap<String, usize> = BTreeMap::new();
    for e in entities {
        let held = values.get(*e);
        let get = |p: &str| held.and_then(|h| h.get(p));
        if !filters.iter().chain(&side.filters).all(|c| matches(c, get(&c.filter.property))) {
            continue;
        }
        let Some(p) = &side.property else {
            used.push((*e).clone());
            continue;
        };
        let Some(h) = get(p) else { continue };
        if side.agg.numeric_only() {
            if let (Some(u), Some(want)) = (&h.unit, &unit)
                && u != want
            {
                *other_units.entry(u.clone()).or_default() += 1;
                continue;
            }
            let Some(n) = h.stored.num else { continue };
            nums.push(n);
        } else if side.agg == Agg::CountDistinct {
            let s = &h.stored;
            distinct.insert(format!("{:?}|{:?}|{:?}|{:?}", s.num, s.text.as_deref().map(str::to_lowercase), s.boolean, s.date));
        }
        used.push((*e).clone());
    }
    let value = match side.agg {
        Agg::Count => Some(used.len() as f64),
        Agg::CountDistinct => Some(distinct.len() as f64),
        Agg::Sum => Some(nums.iter().sum()),
        Agg::Avg if !nums.is_empty() => Some(nums.iter().sum::<f64>() / nums.len() as f64),
        Agg::Min => nums.iter().copied().reduce(f64::min),
        Agg::Max => nums.iter().copied().reduce(f64::max),
        Agg::Avg | Agg::Ratio => None,
    };
    Computed { value, used, other_units }
}

fn round(v: f64) -> f64 {
    let r = (v * 1e6).round() / 1e6;
    if r == 0.0 { 0.0 } else { r }
}

/// The plan's value over `entities`: one side, or numerator over denominator.
fn value_of(plan: &Plan, entities: &[&String], values: &Values, props: &Props, warnings: &mut Vec<String>) -> (Option<f64>, BTreeSet<String>) {
    let sides: Vec<Computed> = plan.sides.iter().map(|s| compute(s, &plan.filters, entities, values, props)).collect();
    for (s, c) in plan.sides.iter().zip(&sides) {
        for (unit, n) in &c.other_units {
            let p = s.property.as_deref().unwrap_or_default();
            let w = format!("left out {n} {p:?} value(s) in {unit}, not the property's unit");
            if !warnings.contains(&w) {
                warnings.push(w);
            }
        }
    }
    let used: BTreeSet<String> = sides.iter().flat_map(|c| c.used.iter().cloned()).collect();
    let value = if plan.ratio {
        match (sides[0].value, sides[1].value) {
            (Some(n), Some(d)) if d != 0.0 => Some(n / d),
            _ => None,
        }
    } else {
        sides[0].value
    };
    (value.map(round), used)
}

/// Text filters that no entity's value matches, with the stored values nearest the one given, since a
/// misspelt value reads as nothing at all. (An enum value listed nowhere is caught before the run.)
fn anchor_warnings(conds: &[&Cond], entities: &[&String], values: &Values) -> Vec<String> {
    let mut out = Vec::new();
    for c in conds {
        if c.datatype != Datatype::Text || !matches!(c.filter.op, Op::Eq | Op::In) {
            continue;
        }
        let p = &c.filter.property;
        if entities.iter().any(|e| matches(c, values.get(*e).and_then(|h| h.get(p)))) {
            continue;
        }
        let seen: BTreeSet<String> = entities.iter().filter_map(|e| values.get(*e)?.get(p)?.stored.text.clone()).collect();
        let seen: Vec<String> = seen.into_iter().collect();
        let given = value_words(&c.filter.value);
        if seen.is_empty() {
            out.push(format!("no entity has a {p:?} value yet, so the filter {p} = {given:?} matched nothing"));
        } else {
            out.push(format!("no {p:?} is {given:?} (nearest stored values: {}): check the spelling before reading this as 0", nearest(&given, &seen).join(", ")));
        }
    }
    out
}

/// Runs a metric by name for `owner`. Only an approved metric runs, unless `allow_unapproved` lets a
/// proposed one run (marked so); a rejected metric with a merge target runs that one.
pub async fn eval(db: &OrgDb, owner: &RecordId, args: &EvalArgs) -> AppResult<MetricEvalOut> {
    let vault_id = args.vault_id.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(|v| crate::rid::parse(v).map_err(|_| AppError::coded(ErrorCode::VaultNotFound, "Vault not found."))).transpose()?;
    let vault = resolve(db, owner, vault_id.as_ref(), Action::ReadMemories).await?;
    prepare(db, &vault, args).await?.run(None)
}

/// Each entity of `kind` in `vault`, sorted, and its latest value of each of `names` as of `as_of`.
pub(super) async fn load_values(db: &OrgDb, vault: &RecordId, kind: &Kind, names: &BTreeSet<String>, as_of: chrono::DateTime<chrono::Utc>) -> AppResult<(Vec<String>, Values)> {
    let by_id: HashMap<String, String> = names.iter().map(|n| (property_rid(vault, kind, &PropertyName::parse(n).expect("checked")).to_string(), n.clone())).collect();
    let prop_ids: Vec<RecordId> = names.iter().map(|n| property_rid(vault, kind, &PropertyName::parse(n).expect("checked"))).collect();
    let mut res = q::METRIC_INPUTS
        .on(db)
        .bind(("vault", vault.clone()))
        .bind(("table", kind_table(kind.as_str())?.to_string()))
        .bind(("kind", kind.as_str().to_string()))
        .bind(("props", prop_ids))
        .bind(("as_of", Datetime::from(as_of)))
        .await?;
    let ids: Vec<RecordId> = res.take(1)?;
    let rows: Vec<InputRow> = match res.take(2) {
        Err(e) if no_ontology_table(&e) => Vec::new(),
        r => r?,
    };
    let mut entities: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
    entities.sort();
    let known: HashSet<&String> = entities.iter().collect();
    let mut values: Values = HashMap::new();
    // newest first: the first row of each (entity, property) is its value as of the run
    for r in rows {
        let entity = r.entity.to_string();
        let Some(name) = by_id.get(&r.property.to_string()) else { continue };
        if !known.contains(&entity) {
            continue;
        }
        let stored = Stored { num: r.value_num, text: r.value_text, boolean: r.value_bool, date: r.value_date };
        values.entry(entity).or_default().entry(name.clone()).or_insert(Held { stored, unit: r.unit });
    }
    Ok((entities, values))
}

/// A metric found, gated, checked and loaded for one run: everything but the arithmetic, which
/// [`Prepared::run`] does on the real values or on a what-if's.
pub(super) struct Prepared {
    out: MetricEvalOut,
    plan: Plan,
    props: Props,
    kind: Kind,
    entities: Vec<String>,
    values: Values,
    series: bool,
    /// A filter value listed nowhere: `out` already says nothing was computed.
    nothing: bool,
}

/// Finds and loads the metric `args` names in `vault` (the caller checked access).
pub(super) async fn prepare(db: &OrgDb, vault: &RecordId, args: &EvalArgs) -> AppResult<Prepared> {
    let vault = vault.clone();
    let asked = metric_name(&args.metric)?;
    let row = match args.kind.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
        Some(k) => {
            let kind = Kind::parse(k)?;
            load(db, &metric_rid(&vault, &kind, &asked)).await?
        }
        None => {
            let rows: Vec<MetricRow> = match q::METRICS_NAMED.on(db).bind(("vault", vault.clone())).bind(("name", asked.as_str().to_string())).await {
                Err(e) if no_ontology_table(&e) => Vec::new(),
                res => res?.take(0)?,
            };
            if rows.len() > 1 {
                let kinds: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
                return Err(AppError::bad_request(format!("{:?} is a metric of several kinds ({}): pass kind", asked.as_str(), kinds.join(", "))));
            }
            rows.into_iter().next()
        }
    };
    let Some(row) = row else { return Err(not_found(asked.as_str(), &approved_names(db, &vault).await?)) };
    let kind = Kind::parse(&row.kind)?;
    let mut warnings = Vec::new();
    let gate = |r: &MetricRow, warnings: &mut Vec<String>| -> AppResult<Status> {
        match Status::parse(&r.status)? {
            Status::Approved => Ok(Status::Approved),
            Status::Proposed if args.allow_unapproved => {
                warnings.push(format!("{:?} is proposed, not approved: no vault owner has checked this number yet", r.name));
                Ok(Status::Proposed)
            }
            Status::Proposed => Err(unapproved(&r.name, &r.kind)),
            Status::Rejected => Ok(Status::Rejected),
        }
    };
    let (row, status, mapped) = match gate(&row, &mut warnings)? {
        Status::Rejected => {
            let target = row.merge_into.as_deref().map(metric_name).transpose()?;
            let refused = || async {
                let approved = approved_names(db, &vault).await.unwrap_or_default();
                let named = if approved.is_empty() { String::new() } else { format!(" Approved metrics: {}.", approved.join(", ")) };
                AppError::coded(ErrorCode::OntologyRejected, format!("the metric {:?} was rejected for this vault.{named}", row.name))
            };
            let Some(target) = target else { return Err(refused().await) };
            let Some(t) = load(db, &metric_rid(&vault, &kind, &target)).await? else { return Err(refused().await) };
            match gate(&t, &mut warnings)? {
                Status::Rejected => return Err(refused().await),
                s => {
                    let mapped = MetricMapped { from: row.name.clone(), to: t.name.clone() };
                    (t, s, Some(mapped))
                }
            }
        }
        s => (row, s, None),
    };

    let props = props_of(db, &vault, &kind).await?;
    let expr = row.expr()?;
    let plan = check(&props, &kind, &expr).map_err(|e| AppError::bad_request(format!("the metric {:?} no longer fits the ontology: {}", row.name, e.message)))?;
    if args.filters.len() > MAX_FILTERS {
        return Err(AppError::bad_request(format!("at most {MAX_FILTERS} extra filters")));
    }
    let mut unlisted = Vec::new();
    let mut extra = Vec::new();
    for f in &args.filters {
        match check_filter(&props, &kind, f) {
            Ok(c) => extra.push(c),
            Err(FilterError::Unlisted(u)) => unlisted.push(u),
            Err(FilterError::Bad(e)) => return Err(e),
        }
    }
    if args.group_by_grain && plan.time.is_none() {
        return Err(AppError::bad_request(format!("{:?} has no time_property, so it has no series: run it without group_by_grain", row.name)));
    }
    for p in plan.properties().iter().chain(extra.iter().map(|c| &c.filter.property)) {
        if props.get(p).is_some_and(|i| i.status != Status::Approved) {
            let w = format!("the property {p:?} is not approved, so it holds no values yet");
            if !warnings.contains(&w) {
                warnings.push(w);
            }
        }
    }
    let as_of = parse_as_of(args.as_of.as_deref())?;
    let unit = row.unit.clone().or_else(|| match plan.ratio {
        false if plan.sides[0].agg.numeric_only() => plan.sides[0].property.as_ref().and_then(|p| props.get(p)).and_then(|p| p.unit.clone()),
        _ => None,
    });
    let mut out = MetricEvalOut {
        metric: row.name.clone(),
        kind: kind.as_str().to_string(),
        vault: vault.to_string(),
        description: row.description.clone(),
        expr_words: expr_words(kind.as_str(), &expr),
        value: None,
        unit,
        as_of: as_of.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        n_entities: 0,
        entity_ids: Vec::new(),
        truncated: false,
        status,
        warnings,
        metric_mapped: mapped,
        series: None,
    };
    if !extra.is_empty() {
        let joined = extra.iter().map(|c| filter_words(&c.filter)).collect::<Vec<_>>().join(" and ");
        let glue = if out.expr_words.contains(" where ") { " and " } else { " where " };
        out.expr_words.push_str(&format!("{glue}{joined}"));
    }
    let names: BTreeSet<String> = plan.properties().into_iter().chain(extra.iter().map(|c| c.filter.property.clone())).collect();
    let mut all_filters = plan.filters.clone();
    all_filters.extend(extra);
    let plan = Plan { filters: all_filters, ..plan };
    let mut prepared = Prepared { out, plan, props, kind, entities: Vec::new(), values: HashMap::new(), series: args.group_by_grain, nothing: false };
    if !unlisted.is_empty() {
        // a filter value listed nowhere matches nothing: say so instead of returning 0
        prepared.out.warnings.extend(unlisted.iter().map(|u| format!("{}. Nothing was computed: this is not a 0", u.sentence())));
        if args.group_by_grain {
            prepared.out.series = Some(Vec::new());
        }
        prepared.nothing = true;
        return Ok(prepared);
    }
    (prepared.entities, prepared.values) = load_values(db, &vault, &prepared.kind, &names, as_of).await?;
    Ok(prepared)
}

impl Prepared {
    /// The metric over the loaded values, or over them with `overlay`'s values in place and its removed
    /// entities left out: the same arithmetic either way.
    pub(super) fn run(&self, overlay: Option<&Overlay>) -> AppResult<MetricEvalOut> {
        let mut out = self.out.clone();
        if self.nothing {
            return Ok(out);
        }
        let plan = &self.plan;
        let kind = &self.kind;
        let props = &self.props;
        let mut values = std::borrow::Cow::Borrowed(&self.values);
        let mut refs: Vec<&String> = self.entities.iter().collect();
        if let Some(o) = overlay {
            refs.retain(|e| !o.removed.contains(*e));
            let reads = plan.properties();
            let known: HashSet<&String> = self.entities.iter().collect();
            for (e, held) in &o.values {
                if !known.contains(e) {
                    continue;
                }
                for (p, h) in held.iter().filter(|(p, _)| reads.contains(*p)) {
                    values.to_mut().entry(e.clone()).or_default().insert(p.clone(), h.clone());
                }
            }
        }
        let values = values.as_ref();
        let conds: Vec<&Cond> = plan.filters.iter().chain(plan.sides.iter().flat_map(|s| &s.filters)).collect();
        out.warnings.extend(anchor_warnings(&conds, &refs, values));

        let used = if let (true, Some((t, grain))) = (self.series, &plan.time) {
            let mut periods: BTreeMap<String, Vec<&String>> = BTreeMap::new();
            let mut undated = 0;
            for e in &refs {
                match values.get(*e).and_then(|h| h.get(t)).and_then(|h| h.stored.date) {
                    Some(d) => periods.entry(grain.period(d.into_inner())).or_default().push(e),
                    None => undated += 1,
                }
            }
            if periods.len() > MAX_PERIODS {
                return Err(AppError::bad_request(format!("the series has more than {MAX_PERIODS} periods: use a longer grain")));
            }
            let mut series = Vec::new();
            let mut used = BTreeSet::new();
            for (period, members) in periods {
                let (value, u) = value_of(plan, &members, values, props, &mut out.warnings);
                series.push(Period { period, value, n: u.len() });
                used.extend(u);
            }
            if undated > 0 && !used.is_empty() {
                out.warnings.push(format!("{undated} {} with no {t:?} are left out of the series", words(kind.as_str())));
            }
            out.series = Some(series);
            used
        } else {
            let (value, used) = value_of(plan, &refs, values, props, &mut out.warnings);
            out.value = value;
            if plan.ratio && value.is_none() && !used.is_empty() {
                out.warnings.push("the denominator is 0, so the ratio has no value".into());
            }
            used
        };
        if used.is_empty() {
            out.warnings.push(format!("no {} matched as of {}: nothing was computed from any entity", words(kind.as_str()), out.as_of));
        }
        out.n_entities = used.len();
        out.truncated = used.len() > MAX_ENTITY_IDS;
        out.entity_ids = used.into_iter().take(MAX_ENTITY_IDS).collect();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn props() -> Props {
        let p = |datatype: Datatype, enum_values: Option<Vec<&str>>| PropInfo {
            datatype,
            unit: (datatype == Datatype::Money).then(|| "AUD".to_string()),
            enum_values: enum_values.map(|v| v.into_iter().map(String::from).collect()),
            status: Status::Approved,
            merge_into: None,
        };
        HashMap::from([
            ("price".to_string(), p(Datatype::Money, None)),
            ("label".to_string(), p(Datatype::Text, None)),
            ("status".to_string(), p(Datatype::Enum, Some(vec!["Delayed", "Delivered", "Pending"]))),
            ("shipped_on".to_string(), p(Datatype::Date, None)),
        ])
    }

    fn expr(v: Value) -> MetricExpr {
        serde_json::from_value(v).unwrap()
    }

    fn refused(v: Value) -> String {
        let kind = Kind::parse("product").unwrap();
        match serde_json::from_value::<MetricExpr>(v) {
            Err(e) => e.to_string(),
            Ok(e) => check(&props(), &kind, &e).expect_err("should be refused").message,
        }
    }

    #[test]
    fn an_expression_is_checked_against_the_kinds_properties() {
        assert!(refused(json!({"agg": "sum", "property": "weight"})).contains("not a property"));
        assert!(refused(json!({"agg": "sum", "property": "label"})).contains("needs a number"));
        assert!(refused(json!({"agg": "avg"})).contains("needs a property"));
        assert!(refused(json!({"agg": "sum", "property": "price", "filters": [{"property": "price", "op": "gt", "value": "cheap"}]})).contains("must be a number"));
        assert!(refused(json!({"agg": "count", "filters": [{"property": "label", "op": "gt", "value": "a"}]})).contains("takes eq, ne or in"));
        let eleven: Vec<Value> = (0..11).map(|i| json!({"property": "price", "op": "gt", "value": i})).collect();
        assert!(refused(json!({"agg": "count", "filters": eleven})).contains("at most 10 filters"));
        assert!(refused(json!({"agg": "ratio", "numerator": {"agg": "count"}})).contains("numerator and a denominator"));
        assert!(refused(json!({"agg": "count", "grain": "month"})).contains("needs a time_property"));
        assert!(refused(json!({"agg": "count", "time_property": "price"})).contains("date or datetime"));
        let delayd = refused(json!({"agg": "count", "filters": [{"property": "status", "op": "eq", "value": "Delayd"}]}));
        assert!(delayd.contains("nearest: Delayed"), "{delayd}");
    }

    #[test]
    fn query_text_never_gets_in() {
        assert!(refused(json!({"agg": "sum", "property": "price; DELETE entity_property"})).contains("invalid property name"));
        assert!(refused(json!({"agg": "count", "filters": [{"property": "label", "op": "eq", "value": "x'; DELETE entity_property; --"}]})).contains("looks like query text"));
        assert!(refused(json!({"agg": "count", "filters": [{"property": "label) OR (1=1", "op": "eq", "value": "x"}]})).contains("invalid property name"));
        // a field that is not part of the expression, such as raw query text, is refused when read
        assert!(refused(json!({"agg": "count", "sql": "SELECT * FROM entity_property"})).contains("unknown field"));
        assert!(refused(json!({"agg": "count", "filters": [{"property": "label", "op": "like", "value": "%"}]})).contains("unknown variant"));
        assert!(metric_name("avg_price; --").is_err());
        assert!(metric_name("Avg Price").unwrap_err().message.contains("metric name"));
    }

    #[test]
    fn a_checked_expression_is_stored_normalised() {
        let kind = Kind::parse("shipment").unwrap();
        let e = expr(json!({"agg": "sum", "property": " price ", "filters": [
            {"property": "status", "op": "in", "value": ["delayed", "PENDING"]},
            {"property": "shipped_on", "op": "gte", "value": "2026-01-01"},
        ], "time_property": "shipped_on"}));
        let plan = check(&props(), &kind, &e).unwrap();
        let back = plan.expr();
        assert_eq!(back.property.as_deref(), Some("price"));
        assert_eq!(back.filters[0].value, json!(["Delayed", "Pending"]));
        assert_eq!(back.grain, Some(Grain::Month));
        assert_eq!(expr_words("shipment", &back), "Sum of price where status is one of Delayed, Pending and shipped on is at least 2026-01-01, by month of shipped on");
    }

    #[test]
    fn expressions_read_as_plain_words() {
        let ratio = expr(json!({"agg": "ratio", "numerator": {"agg": "count", "filters": [{"property": "status", "op": "eq", "value": "Delayed"}]}, "denominator": {"agg": "count"}}));
        assert_eq!(expr_words("shipment", &ratio), "Number of shipment where status is Delayed, divided by number of shipment");
        assert_eq!(expr_words("product", &expr(json!({"agg": "avg", "property": "price"}))), "Average price");
    }

    #[test]
    fn periods_and_nearest_values() {
        let at = chrono::DateTime::parse_from_rfc3339("2026-11-08T10:00:00Z").unwrap().with_timezone(&chrono::Utc);
        assert_eq!(
            [Grain::Day, Grain::Week, Grain::Month, Grain::Quarter, Grain::Year].map(|g| g.period(at)),
            ["2026-11-08", "2026-W45", "2026-11", "2026-Q4", "2026"]
        );
        let listed = ["Delayed", "Delivered", "Pending", "Lost"].map(String::from);
        assert_eq!(nearest("delayd", &listed)[0], "Delayed");
        assert_eq!(edit_distance("kitten", "sitting"), 3);
    }
}
