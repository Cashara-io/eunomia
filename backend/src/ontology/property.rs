//! Typed properties on a vault's kinds, and their values (migration 0034).
//!
//! A property (`price` on `product`, a `money` in AUD) is data the vault's ontology defines, never a
//! field in this code. It is proposed and decided like a kind, and the decision gates writes: only an
//! approved property takes a value, a rejected one with `merge_into` sends its values to that property,
//! and every value must parse as the property's datatype. Values are rows with a date and a source, so
//! the newest one is the current value and the older ones stay as history.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use super::{clean_text, decider, no_ontology_table, view, Kind, OntologyOut, Origin, Status};
use crate::authz::Action;
use crate::entities::neighbourhood::NodeRow;
use crate::entities::service::{accessible, cache_record_rid, require_entity_id};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::store::ontology as q;
use crate::tx::with_retry_dup;

/// What a property's values are. A datatype, not a business field: the vault says which property has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Datatype {
    Text,
    Number,
    Integer,
    Money,
    Percent,
    Bool,
    Date,
    Datetime,
    Enum,
}

impl Datatype {
    pub const ALL: [Datatype; 9] = [
        Datatype::Text,
        Datatype::Number,
        Datatype::Integer,
        Datatype::Money,
        Datatype::Percent,
        Datatype::Bool,
        Datatype::Date,
        Datatype::Datetime,
        Datatype::Enum,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Datatype::Text => "text",
            Datatype::Number => "number",
            Datatype::Integer => "integer",
            Datatype::Money => "money",
            Datatype::Percent => "percent",
            Datatype::Bool => "bool",
            Datatype::Date => "date",
            Datatype::Datetime => "datetime",
            Datatype::Enum => "enum",
        }
    }

    pub fn parse(s: &str) -> AppResult<Self> {
        let s = s.trim();
        Datatype::ALL.into_iter().find(|d| d.as_str() == s).ok_or_else(|| {
            let all: Vec<&str> = Datatype::ALL.iter().map(|d| d.as_str()).collect();
            AppError::bad_request(format!("datatype {s:?} is not one of {}", all.join(", ")))
        })
    }

    pub(super) fn numeric(self) -> bool {
        matches!(self, Datatype::Number | Datatype::Integer | Datatype::Money | Datatype::Percent)
    }
}

const MAX_NAME: usize = 64;
const MAX_UNIT: usize = 32;
const MAX_ENUM_VALUES: usize = 100;
const MAX_SYNONYMS: usize = 20;
const MAX_VALUE_TEXT: usize = 2000;
/// Most record fields or CSV columns one property is mapped from.
const MAX_SOURCE_FIELDS: usize = 20;
/// How far past now an `as_of` may be, for clock skew between the agent and the server.
const AS_OF_LEEWAY_MINUTES: i64 = 5;

/// A property name (`price`, `unit_price`): a lowercase letter, then lowercase letters, digits or `_`,
/// at most 64. One spelling per property, so an agent's write finds the row an owner approved.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PropertyName(String);

impl PropertyName {
    pub fn parse(s: &str) -> AppResult<Self> {
        let s = s.trim();
        let ok = s.len() <= MAX_NAME
            && s.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !ok {
            return Err(AppError::bad_request(format!(
                "invalid property name {s:?}: a lowercase letter, then lowercase letters, digits or _, at most {MAX_NAME}"
            )));
        }
        Ok(PropertyName(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub(super) fn property_rid(vault: &RecordId, kind: &Kind, name: &PropertyName) -> RecordId {
    let parts = format!("{}|{}|{}", vault.to_string(), kind.as_str(), name.0);
    RecordId::from_table_key("ontology_property", crate::tx::stable_key('p', &parts))
}

/// Other names for a kind or property: trimmed, 1 to 64 characters, secrets dropped, each once
/// (ignoring case), never the name itself, at most [`MAX_SYNONYMS`].
pub(super) fn clean_synonyms(name: &str, synonyms: &[String]) -> AppResult<Vec<String>> {
    let mut seen: HashSet<String> = HashSet::from([name.to_lowercase()]);
    let mut out = Vec::new();
    for s in synonyms {
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        if s.chars().count() > MAX_NAME {
            return Err(AppError::bad_request(format!("a synonym is at most {MAX_NAME} characters, got {s:?}")));
        }
        if seen.insert(s.to_lowercase()) {
            out.push(crate::redact::redact(s, &mut Vec::new()));
        }
    }
    if out.len() > MAX_SYNONYMS {
        return Err(AppError::bad_request(format!("at most {MAX_SYNONYMS} synonyms")));
    }
    Ok(out)
}

pub(super) fn clean_unit(unit: Option<&str>) -> AppResult<Option<String>> {
    let Some(u) = unit.map(str::trim).filter(|u| !u.is_empty()) else { return Ok(None) };
    if u.chars().count() > MAX_UNIT {
        return Err(AppError::bad_request(format!("a unit is at most {MAX_UNIT} characters, got {u:?}")));
    }
    Ok(Some(u.to_string()))
}

/// A record field or CSV column a property's values come from: `source` and `record_type` name the
/// records (`stripe`, `stripe.charge`), or `document` and the document id for an uploaded CSV, and
/// `path` the payload key path or the column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SourceField {
    pub source: String,
    pub record_type: String,
    pub path: String,
}

fn clean_source_map(fields: &[SourceField]) -> AppResult<Vec<SourceField>> {
    if fields.len() > MAX_SOURCE_FIELDS {
        return Err(AppError::bad_request(format!("a property is mapped from at most {MAX_SOURCE_FIELDS} fields")));
    }
    let mut out: Vec<SourceField> = Vec::new();
    for f in fields {
        let trim = |s: &str, what: &str| {
            let s = s.trim();
            if s.is_empty() || s.chars().count() > 200 {
                return Err(AppError::bad_request(format!("a source_map {what} is 1 to 200 characters")));
            }
            Ok(s.to_string())
        };
        let f = SourceField { source: trim(&f.source, "source")?, record_type: trim(&f.record_type, "record_type")?, path: trim(&f.path, "path")? };
        if !out.contains(&f) {
            out.push(f);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Proposals
// ---------------------------------------------------------------------------

/// A proposed property, as an agent or the model sends it.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PropertyProposal {
    pub kind: String,
    pub name: String,
    pub datatype: String,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub enum_values: Option<Vec<String>>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub synonyms: Vec<String>,
    /// Proposes folding this property into another of the same kind (the one to keep).
    #[serde(default)]
    pub merge_into: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    /// The record fields or CSV columns its values come from.
    #[serde(default)]
    pub source_map: Vec<SourceField>,
}

pub(super) struct ValidProperty {
    pub(super) kind: Kind,
    pub(super) name: PropertyName,
    pub(super) datatype: Datatype,
    pub(super) unit: Option<String>,
    pub(super) enum_values: Option<Vec<String>>,
    description: Option<String>,
    pub(super) synonyms: Vec<String>,
    merge_into: Option<PropertyName>,
    reason: Option<String>,
    pub(super) source_map: Vec<SourceField>,
}

impl PropertyProposal {
    pub(super) fn validate(&self) -> AppResult<ValidProperty> {
        let kind = Kind::parse(&self.kind)?;
        let name = PropertyName::parse(&self.name)?;
        let datatype = Datatype::parse(&self.datatype)?;
        let enum_values = match (datatype, &self.enum_values) {
            (Datatype::Enum, Some(values)) => {
                let values = clean_synonyms("", values)
                    .map_err(|e| AppError::bad_request(e.message.replace("synonym", "enum value")))?;
                if values.is_empty() || values.len() > MAX_ENUM_VALUES {
                    return Err(AppError::bad_request(format!("an enum property lists 1 to {MAX_ENUM_VALUES} enum_values")));
                }
                Some(values)
            }
            (Datatype::Enum, None) => return Err(AppError::bad_request(format!("{:?} is an enum: list its enum_values", name.0))),
            (_, Some(v)) if !v.is_empty() => return Err(AppError::bad_request(format!("enum_values are for an enum property, not {}", datatype.as_str()))),
            _ => None,
        };
        let merge_into = self.merge_into.as_deref().map(str::trim).filter(|m| !m.is_empty()).map(PropertyName::parse).transpose()?;
        if merge_into.as_ref() == Some(&name) {
            return Err(AppError::bad_request(format!("{:?} cannot merge into itself", name.0)));
        }
        Ok(ValidProperty {
            synonyms: clean_synonyms(&name.0, &self.synonyms)?,
            kind,
            name,
            datatype,
            unit: clean_unit(self.unit.as_deref())?,
            enum_values,
            description: clean_text("description", self.description.as_deref())?,
            merge_into,
            reason: clean_text("reason", self.reason.as_deref())?,
            source_map: clean_source_map(&self.source_map)?,
        })
    }
}

/// A proposed property after the call, with the row's status (a decided one keeps its decision).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ProposedProperty {
    pub kind: String,
    pub name: String,
    pub datatype: Datatype,
    pub status: Status,
    pub merge_into: Option<String>,
}

pub(super) async fn apply_properties(db: &OrgDb, vault: &RecordId, props: Vec<ValidProperty>, origin: Origin) -> AppResult<Vec<ProposedProperty>> {
    let mut out = Vec::new();
    for p in props {
        let id = property_rid(vault, &p.kind, &p.name);
        let row: PropertyRow = with_retry_dup(|| async {
            let mut res = q::PROPOSE_PROPERTY
                .on(db)
                .bind(("id", id.clone()))
                .bind(("vault", vault.clone()))
                .bind(("kind", p.kind.as_str().to_string()))
                .bind(("name", p.name.0.clone()))
                .bind(("datatype", p.datatype.as_str().to_string()))
                .bind(("unit", p.unit.clone()))
                .bind(("enum_values", p.enum_values.clone()))
                .bind(("description", p.description.clone()))
                .bind(("synonyms", p.synonyms.clone()))
                .bind(("merge_into", p.merge_into.as_ref().map(|m| m.0.clone())))
                .bind(("origin", origin.as_str().to_string()))
                .bind(("reason", p.reason.clone()))
                .bind(("source_map", serde_json::to_value(&p.source_map).unwrap_or_default()))
                .await?;
            let rows: Vec<PropertyRow> = res.take(q::PROPOSE_PROPERTY.slot)?;
            rows.into_iter().next().ok_or_else(|| surrealdb::Error::query("ontology property upsert returned no row".into(), None))
        })
        .await?;
        out.push(ProposedProperty { kind: row.kind, name: row.name, datatype: Datatype::parse(&row.datatype)?, status: Status::parse(&row.status)?, merge_into: row.merge_into });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Rows and the view
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, SurrealValue)]
pub(super) struct PropertyRow {
    pub(super) id: RecordId,
    pub(super) kind: String,
    pub(super) name: String,
    pub(super) datatype: String,
    #[serde(default)]
    #[surreal(default)]
    pub(super) unit: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    pub(super) enum_values: Option<Vec<String>>,
    #[serde(default)]
    #[surreal(default)]
    description: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    synonyms: Vec<String>,
    pub(super) status: String,
    #[serde(default)]
    #[surreal(default)]
    pub(super) merge_into: Option<String>,
    origin: String,
    #[serde(default)]
    #[surreal(default)]
    reason: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    source_map: Value,
}

impl PropertyRow {
    pub(super) fn source_map(&self) -> Vec<SourceField> {
        serde_json::from_value(self.source_map.clone()).unwrap_or_default()
    }
}

#[derive(Debug, Deserialize, SurrealValue)]
struct SampleValue {
    #[serde(default)]
    #[surreal(default)]
    num: Option<f64>,
    #[serde(default)]
    #[surreal(default)]
    text: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    boolean: Option<bool>,
    #[serde(default)]
    #[surreal(default)]
    date: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    unit: Option<String>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct ValueStats {
    property: RecordId,
    #[serde(default)]
    #[surreal(default)]
    count: i64,
    #[serde(default)]
    #[surreal(default)]
    samples: Vec<SampleValue>,
}

/// One property of a kind, as the ontology view shows it.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PropertyItem {
    pub name: String,
    pub datatype: Datatype,
    pub unit: Option<String>,
    pub enum_values: Option<Vec<String>>,
    pub description: Option<String>,
    pub synonyms: Vec<String>,
    pub status: Status,
    /// Rejected: the property of the same kind its values go to. Proposed: the merge the proposer suggests.
    pub merge_into: Option<String>,
    pub origin: Origin,
    pub reason: Option<String>,
    /// Values written for it in the vault, history included.
    pub values: i64,
    /// A few of them, as text with their unit.
    pub samples: Vec<String>,
    /// The record fields or CSV columns it was mapped from.
    pub source_map: Vec<SourceField>,
}

/// The vault's properties by kind, each kind's by name. None on a database before migration 0034.
pub(super) async fn properties_by_kind(db: &OrgDb, vault: &RecordId) -> AppResult<BTreeMap<String, Vec<PropertyItem>>> {
    let mut res = match q::PROPERTIES.on(db).bind(("vault", vault.clone())).await {
        Err(e) if no_ontology_table(&e) => return Ok(BTreeMap::new()),
        res => res?,
    };
    let rows: Vec<PropertyRow> = res.take(0)?;
    let stats: Vec<ValueStats> = res.take(1)?;
    let mut stats: HashMap<String, ValueStats> = stats.into_iter().map(|s| (s.property.to_string(), s)).collect();
    let mut out: BTreeMap<String, Vec<PropertyItem>> = BTreeMap::new();
    for r in rows {
        let datatype = Datatype::parse(&r.datatype)?;
        let (values, samples) = match stats.remove(&r.id.to_string()) {
            Some(s) => {
                let unit = r.unit.clone();
                let samples = s
                    .samples
                    .into_iter()
                    .filter_map(|v| {
                        let stored = Stored { num: v.num, text: v.text, boolean: v.boolean, date: v.date };
                        Some(with_unit(display(datatype, &stored)?, v.unit.as_deref().or(unit.as_deref())))
                    })
                    .collect();
                (s.count, samples)
            }
            None => (0, Vec::new()),
        };
        let source_map = r.source_map();
        out.entry(r.kind.clone()).or_default().push(PropertyItem {
            source_map,
            datatype,
            status: Status::parse(&r.status)?,
            origin: Origin::parse(&r.origin)?,
            name: r.name,
            unit: r.unit,
            enum_values: r.enum_values,
            description: r.description,
            synonyms: r.synonyms,
            merge_into: r.merge_into,
            reason: r.reason,
            values,
            samples,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Review: a person decides
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum PropertyDecision {
    Approve,
    /// Values for the property go to `merge_into` (a property of the same kind) from now on; without
    /// it they are refused.
    Reject {
        #[serde(default)]
        merge_into: Option<String>,
    },
    /// Set or (blank) clear the description; the status stays as it is.
    Describe { description: String },
}

async fn load(db: &OrgDb, id: &RecordId) -> AppResult<Option<PropertyRow>> {
    let rows: Vec<PropertyRow> = q::PROPERTY.on(db).bind(("id", id.clone())).await?.take(0)?;
    Ok(rows.into_iter().next())
}

fn no_property(kind: &Kind, name: &PropertyName) -> AppError {
    AppError::coded(ErrorCode::ResourceNotFound, format!("no property {:?} on the kind {:?} in this vault", name.0, kind.as_str()))
}

pub async fn decide_property(db: &OrgDb, user: &RecordId, vault: &RecordId, kind: &str, name: &str, decision: PropertyDecision) -> AppResult<OntologyOut> {
    decider(db, user, vault).await?;
    let kind = Kind::parse(kind)?;
    let name = PropertyName::parse(name)?;
    let id = property_rid(vault, &kind, &name);
    if load(db, &id).await?.is_none() {
        return Err(no_property(&kind, &name));
    }
    let (status, merge_into) = match decision {
        PropertyDecision::Describe { description } => {
            let description = clean_text("description", Some(&description))?;
            with_retry_dup(|| async { q::DESCRIBE_PROPERTY.on(db).bind(("id", id.clone())).bind(("description", description.clone())).await }).await?;
            return view(db, vault).await;
        }
        PropertyDecision::Approve => (Status::Approved, None),
        PropertyDecision::Reject { merge_into } => {
            let target = merge_into.as_deref().map(str::trim).filter(|m| !m.is_empty()).map(PropertyName::parse).transpose()?;
            if let Some(t) = &target {
                if t == &name {
                    return Err(AppError::bad_request(format!("{:?} cannot merge into itself", name.0)));
                }
                match load(db, &property_rid(vault, &kind, t)).await? {
                    None => return Err(AppError::bad_request(format!("{:?} is not a property of {:?}: propose it first", t.0, kind.as_str()))),
                    Some(row) if Status::parse(&row.status)? == Status::Rejected => {
                        return Err(AppError::bad_request(format!("{:?} is rejected too: merge into a property that is kept", t.0)))
                    }
                    Some(_) => {}
                }
            }
            (Status::Rejected, target)
        }
    };
    with_retry_dup(|| async {
        q::DECIDE_PROPERTY
            .on(db)
            .bind(("id", id.clone()))
            .bind(("vault", vault.clone()))
            .bind(("kind", kind.as_str().to_string()))
            .bind(("name", name.0.clone()))
            .bind(("status", status.as_str().to_string()))
            .bind(("merge_into", merge_into.as_ref().map(|m| m.0.clone())))
            .bind(("user", user.clone()))
            .await
    })
    .await?;
    view(db, vault).await
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

/// A value as stored: one of the four columns is set.
#[derive(Debug, Default, Clone, PartialEq)]
pub(super) struct Stored {
    pub(super) num: Option<f64>,
    pub(super) text: Option<String>,
    pub(super) boolean: Option<bool>,
    pub(super) date: Option<Datetime>,
}

fn expected(datatype: Datatype, name: &str, got: &Value) -> AppError {
    let what = match datatype {
        Datatype::Text => "text".to_string(),
        Datatype::Number | Datatype::Money | Datatype::Percent => "a number, like 4.5 (no currency sign or % in the value: the unit says that)".to_string(),
        Datatype::Integer => "a whole number".to_string(),
        Datatype::Bool => "true or false".to_string(),
        Datatype::Date => "an ISO date, YYYY-MM-DD".to_string(),
        Datatype::Datetime => "an ISO 8601 datetime with a zone, like 2026-11-07T09:30:00Z".to_string(),
        Datatype::Enum => "one of its enum_values".to_string(),
    };
    AppError::bad_request(format!("{name:?} is a {} property: the value must be {what}, got {got}", datatype.as_str()))
}

/// `value` read as `datatype`, or why not. Enum values match ignoring case and are stored as listed.
pub(super) fn parse_value(datatype: Datatype, name: &str, enum_values: Option<&[String]>, value: &Value) -> AppResult<Stored> {
    let bad = || expected(datatype, name, value);
    let text = value.as_str().map(str::trim);
    Ok(match datatype {
        Datatype::Text => {
            let s = text.filter(|s| !s.is_empty()).ok_or_else(bad)?;
            if s.chars().count() > MAX_VALUE_TEXT {
                return Err(AppError::bad_request(format!("a text value is at most {MAX_VALUE_TEXT} characters")));
            }
            Stored { text: Some(crate::redact::redact(s, &mut Vec::new())), ..Stored::default() }
        }
        Datatype::Number | Datatype::Integer | Datatype::Money | Datatype::Percent => {
            let n = match value {
                Value::Number(n) => n.as_f64(),
                Value::String(s) => s.trim().parse::<f64>().ok(),
                _ => None,
            }
            .filter(|n| n.is_finite())
            .ok_or_else(bad)?;
            if datatype == Datatype::Integer && (n.fract() != 0.0 || n.abs() > 9_007_199_254_740_991.0) {
                return Err(bad());
            }
            Stored { num: Some(n), ..Stored::default() }
        }
        Datatype::Bool => {
            let b = match value {
                Value::Bool(b) => Some(*b),
                Value::String(s) => match s.trim() {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                },
                _ => None,
            };
            Stored { boolean: Some(b.ok_or_else(bad)?), ..Stored::default() }
        }
        Datatype::Date => {
            let d = text.and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()).ok_or_else(bad)?;
            let at = d.and_hms_opt(0, 0, 0).ok_or_else(bad)?.and_utc();
            Stored { date: Some(Datetime::from(at)), ..Stored::default() }
        }
        Datatype::Datetime => {
            let at = text.and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).ok_or_else(bad)?;
            Stored { date: Some(Datetime::from(at.with_timezone(&chrono::Utc))), ..Stored::default() }
        }
        Datatype::Enum => {
            let s = text.ok_or_else(bad)?;
            let allowed = enum_values.unwrap_or_default();
            let Some(hit) = allowed.iter().find(|v| v.eq_ignore_ascii_case(s)) else {
                return Err(AppError::bad_request(format!("{name:?} takes one of {}, got {value}", allowed.join(", "))));
            };
            Stored { text: Some(hit.clone()), ..Stored::default() }
        }
    })
}

/// A stored value as JSON in its datatype's shape: a number, a bool, a string, a date as YYYY-MM-DD.
pub(super) fn to_json(datatype: Datatype, v: &Stored) -> Option<Value> {
    Some(match datatype {
        Datatype::Integer => Value::from(v.num? as i64),
        d if d.numeric() => serde_json::Number::from_f64(v.num?).map(Value::Number)?,
        Datatype::Bool => Value::Bool(v.boolean?),
        Datatype::Date => Value::String(v.date?.into_inner().format("%Y-%m-%d").to_string()),
        Datatype::Datetime => Value::String(v.date?.into_inner().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        _ => Value::String(v.text.clone()?),
    })
}

fn display(datatype: Datatype, v: &Stored) -> Option<String> {
    Some(match (datatype, to_json(datatype, v)?) {
        (Datatype::Money, _) => format!("{:.2}", v.num?),
        (_, Value::String(s)) => s,
        (_, other) => other.to_string(),
    })
}

fn with_unit(text: String, unit: Option<&str>) -> String {
    match unit {
        Some(u) => format!("{text} {u}"),
        None => text,
    }
}

/// One property's current value on an entity.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PropertyValue {
    pub property: String,
    pub datatype: Datatype,
    /// A number, a bool or a string (a date is YYYY-MM-DD, a datetime RFC 3339).
    pub value: Value,
    /// The value's unit, else the property's.
    pub unit: Option<String>,
    /// When the value was true, RFC 3339.
    pub as_of: String,
    /// The synced record it came from.
    pub source: Option<String>,
}

/// A write to a rejected property that went to its merge target instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PropertyMapped {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetPropertyOut {
    pub entity_id: String,
    pub kind: String,
    pub value: PropertyValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub property_mapped: Option<PropertyMapped>,
}

/// What `entity_set_property` was asked to write.
pub struct SetProperty<'a> {
    pub entity_id: &'a RecordId,
    pub property: &'a str,
    pub value: &'a Value,
    pub unit: Option<&'a str>,
    pub as_of: Option<&'a str>,
    pub source_record_id: Option<&'a str>,
}

fn not_approved(kind: &Kind, name: &PropertyName) -> AppError {
    AppError::bad_request(format!(
        "the property {:?} on {:?} is proposed, not approved: a vault owner approves it in the Eunomia app (the vault's Kinds and links), then write the value again",
        name.0,
        kind.as_str()
    ))
}

async fn refuse_unknown(db: &OrgDb, vault: &RecordId, kind: &Kind, name: &PropertyName, why: &str) -> AppError {
    let approved: Vec<String> = match q::APPROVED_PROPERTIES.on(db).bind(("vault", vault.clone())).bind(("kind", kind.as_str().to_string())).await {
        Ok(mut res) => res.take(0).unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let named = if approved.is_empty() { String::new() } else { format!(" Approved properties of {:?}: {}.", kind.as_str(), approved.join(", ")) };
    AppError::bad_request(format!(
        "the property {:?} on {:?} {why}: propose it with ontology_propose (properties: kind, name, datatype, unit) and wait for a vault owner to approve it.{named}",
        name.0,
        kind.as_str()
    ))
}

/// The approved property a write of `asked` on `kind` lands on: itself, or the merge target of a
/// rejected one (`Some` mapping). Refused for a property that is unknown, proposed, or rejected with no
/// approved target. What `entity_set_property` and a CSV import both check.
pub(super) async fn resolve_approved(db: &OrgDb, vault: &RecordId, kind: &Kind, asked: &PropertyName) -> AppResult<(PropertyRow, Option<PropertyMapped>)> {
    let lookup = |name: &PropertyName| {
        let id = property_rid(vault, kind, name);
        async move {
            match q::PROPERTY.on(db).bind(("id", id)).await {
                Err(e) if no_ontology_table(&e) => Ok(None),
                res => Ok::<_, AppError>(res?.take::<Vec<PropertyRow>>(0)?.into_iter().next()),
            }
        }
    };
    let Some(row) = lookup(asked).await? else {
        return Err(refuse_unknown(db, vault, kind, asked, "does not exist in this vault").await);
    };
    match Status::parse(&row.status)? {
        Status::Approved => Ok((row, None)),
        Status::Proposed => Err(not_approved(kind, asked)),
        Status::Rejected => {
            let Some(target) = row.merge_into.as_deref().map(PropertyName::parse).transpose()? else {
                return Err(refuse_unknown(db, vault, kind, asked, "was rejected for this vault").await);
            };
            match lookup(&target).await? {
                Some(t) if Status::parse(&t.status)? == Status::Approved => Ok((t, Some(PropertyMapped { from: asked.0.clone(), to: target.0.clone() }))),
                Some(t) if Status::parse(&t.status)? == Status::Proposed => Err(not_approved(kind, &target)),
                _ => Err(refuse_unknown(db, vault, kind, &target, "was rejected for this vault").await),
            }
        }
    }
}

/// When a value was true: `given` (an ISO 8601 date or datetime) or now. Refused in the future: the
/// newest as_of is the current value and single values cannot be deleted, so a future date would pin a
/// value that no later write could replace.
pub(super) fn parse_as_of(given: Option<&str>) -> AppResult<chrono::DateTime<chrono::Utc>> {
    let given = given.map(str::trim).filter(|s| !s.is_empty());
    let as_of = match given {
        Some(s) => crate::cache::search::parse_datetime("as_of", s)?,
        None => chrono::Utc::now(),
    };
    // A bare date reads as midnight UTC, which is still ahead of now for half the world on that day
    // (Melbourne until 11am), so it gets a day of leeway; a datetime gets a few minutes of clock skew.
    let leeway = if given.is_some_and(|s| !s.contains('T')) { chrono::Duration::days(1) } else { chrono::Duration::minutes(AS_OF_LEEWAY_MINUTES) };
    if as_of > chrono::Utc::now() + leeway {
        return Err(AppError::bad_request("as_of is in the future: give when the value became true, today or earlier"));
    }
    Ok(as_of)
}

/// A value checked as `entity_set_property` checks it, before anything is written.
pub(super) struct Checked {
    pub(super) row: PropertyRow,
    pub(super) mapped: Option<PropertyMapped>,
    pub(super) datatype: Datatype,
    pub(super) stored: Stored,
    pub(super) unit: Option<String>,
}

/// `value` for `property` of `kind` in `vault`: the property must be approved (or rejected into an
/// approved one, which takes the value) and the value must parse as its datatype.
pub(super) async fn check_value(db: &OrgDb, vault: &RecordId, kind: &Kind, property: &str, value: &Value, unit: Option<&str>) -> AppResult<Checked> {
    let asked = PropertyName::parse(property)?;
    let (row, mapped) = resolve_approved(db, vault, kind, &asked).await?;
    let datatype = Datatype::parse(&row.datatype)?;
    let stored = parse_value(datatype, &row.name, row.enum_values.as_deref(), value)?;
    Ok(Checked { row, mapped, datatype, stored, unit: clean_unit(unit)? })
}

/// Writes one value of an approved property on an entity the caller may write to. Refused for a
/// property that is unknown, proposed, or rejected with no merge target; a rejected one with a target
/// writes to that property and says so. The value must parse as the property's datatype.
pub async fn set_property(db: &OrgDb, owner: &RecordId, input: SetProperty<'_>) -> AppResult<SetPropertyOut> {
    require_entity_id(input.entity_id)?;
    let not_found = || AppError::coded(ErrorCode::EntityNotFound, format!("entity {} not found", input.entity_id.to_string()));
    let entity: NodeRow = crate::store::get(db, input.entity_id).await?.ok_or_else(not_found)?;
    if !accessible(db, owner, &entity.vault, Action::WriteMemories).await? {
        return Err(not_found());
    }
    let vault = entity.vault.clone();
    let kind = Kind::parse(&entity.kind())?;
    let Checked { row, mapped, datatype, stored, unit } = check_value(db, &vault, &kind, input.property, input.value, input.unit).await?;
    let as_of = parse_as_of(input.as_of)?;
    let source = input.source_record_id.map(|r| cache_record_rid(owner, r));
    let written: Vec<ValueRow> = q::SET_VALUE
        .on(db)
        .bind(("vault", vault.clone()))
        .bind(("entity", entity.id.clone()))
        .bind(("property", row.id.clone()))
        .bind(("value_num", stored.num))
        .bind(("value_text", stored.text.clone()))
        .bind(("value_bool", stored.boolean))
        .bind(("value_date", stored.date))
        .bind(("unit", unit))
        .bind(("as_of", Datetime::from(as_of)))
        .bind(("source", source))
        .bind(("origin", Origin::Agent.as_str().to_string()))
        .await?
        .take(0)?;
    let written = written.into_iter().next().ok_or_else(|| AppError::internal("property value write returned no row"))?;
    let approved = ApprovedProperty { name: row.name, datatype, unit: row.unit };
    let value = written.out(&approved).ok_or_else(|| AppError::internal("property value did not round-trip"))?;
    Ok(SetPropertyOut { entity_id: entity.id.to_string(), kind: kind.as_str().to_string(), value, property_mapped: mapped })
}

#[derive(Debug, Deserialize, SurrealValue)]
struct ApprovedRow {
    id: RecordId,
    name: String,
    datatype: String,
    #[serde(default)]
    #[surreal(default)]
    unit: Option<String>,
}

struct ApprovedProperty {
    name: String,
    datatype: Datatype,
    unit: Option<String>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct ValueRow {
    #[serde(default)]
    #[surreal(default)]
    entity: Option<RecordId>,
    #[serde(default)]
    #[surreal(default)]
    property: Option<RecordId>,
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
    as_of: Datetime,
    #[serde(default)]
    #[surreal(default)]
    source: Option<RecordId>,
}

impl ValueRow {
    fn out(&self, p: &ApprovedProperty) -> Option<PropertyValue> {
        let stored = Stored { num: self.value_num, text: self.value_text.clone(), boolean: self.value_bool, date: self.value_date };
        Some(PropertyValue {
            property: p.name.clone(),
            datatype: p.datatype,
            value: to_json(p.datatype, &stored)?,
            unit: self.unit.clone().or_else(|| p.unit.clone()),
            as_of: self.as_of.into_inner().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            source: self.source.as_ref().map(|s| s.to_string()),
        })
    }
}

/// The current value of each approved property for each of `ids` in `vault` (the caller checked
/// access), by entity id, properties by name. A value of a property not approved now is left out.
/// Empty on a database before migration 0034.
pub async fn latest_values(db: &OrgDb, vault: &RecordId, ids: &[RecordId]) -> AppResult<HashMap<String, Vec<PropertyValue>>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let mut res = match q::VALUES_OF.on(db).bind(("vault", vault.clone())).bind(("ids", ids.to_vec())).await {
        Err(e) if no_ontology_table(&e) => return Ok(HashMap::new()),
        res => res?,
    };
    let approved: Vec<ApprovedRow> = res.take(0)?;
    let rows: Vec<ValueRow> = res.take(1)?;
    let approved: HashMap<String, ApprovedProperty> = approved
        .into_iter()
        .filter_map(|a| Some((a.id.to_string(), ApprovedProperty { datatype: Datatype::parse(&a.datatype).ok()?, name: a.name, unit: a.unit })))
        .collect();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut out: HashMap<String, Vec<PropertyValue>> = HashMap::new();
    // newest first, so the first row of each (entity, property) is its current value
    for r in rows {
        let (Some(entity), Some(property)) = (r.entity.as_ref().map(|e| e.to_string()), r.property.as_ref().map(|p| p.to_string())) else { continue };
        let Some(p) = approved.get(&property) else { continue };
        if !seen.insert((entity.clone(), property)) {
            continue;
        }
        if let Some(v) = r.out(p) {
            out.entry(entity).or_default().push(v);
        }
    }
    for values in out.values_mut() {
        values.sort_by(|a, b| a.property.cmp(&b.property));
    }
    Ok(out)
}

/// Drops a deleted entity's values. Best effort on a database before migration 0034, which has none.
pub async fn delete_values_of(db: &OrgDb, entity: &RecordId) -> AppResult<()> {
    match q::DELETE_VALUES_OF.on(db).bind(("id", entity.clone())).await {
        Err(e) if no_ontology_table(&e) => Ok(()),
        res => {
            res?.check()?;
            Ok(())
        }
    }
}

/// Moves a merged-away entity's values onto the entity it was merged into.
pub async fn move_values(db: &OrgDb, winner: &RecordId, loser: &RecordId) -> AppResult<()> {
    match q::MOVE_VALUES.on(db).bind(("winner", winner.clone())).bind(("loser", loser.clone())).await {
        Err(e) if no_ontology_table(&e) => Ok(()),
        res => {
            res?.check()?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(d: Datatype, v: Value) -> AppResult<Stored> {
        parse_value(d, "p", Some(&["Small".to_string(), "Large".to_string()]), &v)
    }

    #[test]
    fn values_must_parse_as_their_datatype() {
        assert_eq!(parse(Datatype::Money, json!(4.5)).unwrap().num, Some(4.5));
        assert_eq!(parse(Datatype::Money, json!(" 4.50 ")).unwrap().num, Some(4.5));
        assert!(parse(Datatype::Money, json!("$4.50")).is_err());
        assert!(parse(Datatype::Number, json!(true)).is_err());
        assert!(parse(Datatype::Integer, json!(2.5)).is_err());
        assert_eq!(parse(Datatype::Integer, json!(3)).unwrap().num, Some(3.0));
        assert_eq!(parse(Datatype::Bool, json!("true")).unwrap().boolean, Some(true));
        assert!(parse(Datatype::Bool, json!("yes")).is_err());
        assert!(parse(Datatype::Date, json!("7/11/2026")).is_err());
        assert!(parse(Datatype::Date, json!("2026-02-30")).is_err());
        assert!(parse(Datatype::Date, json!("2026-11-07")).unwrap().date.is_some());
        assert!(parse(Datatype::Datetime, json!("2026-11-07")).is_err());
        assert_eq!(parse(Datatype::Enum, json!("large")).unwrap().text.as_deref(), Some("Large"));
        assert!(parse(Datatype::Enum, json!("medium")).is_err());
        assert!(parse(Datatype::Text, json!("   ")).is_err());
    }

    #[test]
    fn values_come_back_in_their_datatypes_shape() {
        let date = parse(Datatype::Date, json!("2026-11-07")).unwrap();
        assert_eq!(to_json(Datatype::Date, &date), Some(json!("2026-11-07")));
        assert_eq!(to_json(Datatype::Integer, &Stored { num: Some(3.0), ..Stored::default() }), Some(json!(3)));
        assert_eq!(display(Datatype::Money, &Stored { num: Some(4.5), ..Stored::default() }).as_deref(), Some("4.50"));
    }

    #[test]
    fn names_and_proposals_are_checked() {
        assert!(PropertyName::parse("Price").is_err());
        assert!(PropertyName::parse("unit price").is_err());
        assert!(PropertyName::parse(&"p".repeat(65)).is_err());
        assert_eq!(PropertyName::parse(" unit_price ").unwrap().as_str(), "unit_price");
        let p = |datatype: &str, enum_values: Option<Vec<String>>| PropertyProposal {
            kind: "product".into(),
            name: "size".into(),
            datatype: datatype.into(),
            unit: None,
            enum_values,
            description: None,
            synonyms: vec!["Size".into(), " cup size ".into(), "cup size".into()],
            merge_into: None,
            reason: None,
            source_map: vec![],
        };
        assert!(p("enum", None).validate().is_err());
        assert!(p("text", Some(vec!["a".into()])).validate().is_err());
        assert!(p("colour", None).validate().is_err());
        let ok = p("enum", Some(vec!["Small".into(), "Large".into()])).validate().unwrap();
        assert_eq!(ok.synonyms, ["cup size"], "the name itself and repeats are dropped");
    }
}
