//! Field profiling: what each field of a record type holds, worked out from a sample of its newest
//! records (or the rows of an uploaded CSV), computed on demand and never stored. Auto-mapping reads
//! it to propose kinds, typed properties and links ([`super::automap`]).
//!
//! A field is a JSON key path of the payload (`billing.country`), or a CSV column. Per field: a
//! guessed datatype, the share of records with no value, whether every value differs, how many
//! distinct values there are (counted to 26), the five commonest, the full set when there are 25 or
//! fewer (an enum candidate), a date format, and min and max for numbers. Objects are walked to
//! [`MAX_DEPTH`] levels; arrays are left out. At most [`MAX_PATHS`] fields per record type.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use surrealdb::types::{RecordId, SurrealValue};

use crate::error::AppResult;
use crate::pool::OrgDb;
use crate::store::ontology as q;

/// Records read per record type: the newest ones.
pub const SAMPLE_SIZE: usize = 1000;
/// How deep payload objects are walked: `a.b.c.d` at most.
pub const MAX_DEPTH: usize = 4;
/// Most fields profiled per record type, in the order they first appear.
pub const MAX_PATHS: usize = 60;
/// Most record types one auto-mapping run profiles.
pub const MAX_RECORD_TYPES: usize = 12;
/// Distinct values are counted up to this; 26 means "26 or more".
pub const DISTINCT_CAP: usize = 26;
const ENUM_MAX: usize = 25;
const TOP_VALUES: usize = 5;
const VALUE_CHARS: usize = 80;

/// What a field's values look like. The datatypes of a property, plus `id` for a key that names a
/// record (a link candidate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum InferredType {
    Text,
    Number,
    Integer,
    Money,
    Percent,
    Bool,
    Date,
    Datetime,
    Enum,
    Id,
}

#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
pub struct TopValue {
    pub value: String,
    pub count: usize,
}

/// One field of one record type, from a sample.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
pub struct FieldProfile {
    pub source: String,
    pub record_type: String,
    /// The payload key path (`billing.country`) or the CSV column.
    pub path: String,
    pub inferred_type: InferredType,
    /// The share of sampled records with no value here (missing, null or blank), 0 to 1.
    pub null_rate: f64,
    /// Every sampled value differs (and there is at least one).
    pub is_unique: bool,
    /// Distinct values in the sample, counted up to 26 (26 means 26 or more).
    pub distinct_count: usize,
    /// The commonest values, most frequent first, at most five.
    pub top_values: Vec<TopValue>,
    /// Every distinct value, for an enum candidate (25 or fewer).
    pub enum_values: Option<Vec<String>>,
    /// For dates: the notation the values are written in, such as `YYYY-MM-DD` or `DD/MM/YYYY`.
    pub date_format: Option<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub sample_size: usize,
}

/// One record type's fields.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct RecordTypeProfile {
    pub source: String,
    pub record_type: String,
    /// Records sampled.
    pub records: usize,
    pub fields: Vec<FieldProfile>,
    /// More fields than [`MAX_PATHS`]: the later ones were left out.
    pub paths_truncated: bool,
}

/// The rows a profile is made from: each one a JSON object (a payload, or a CSV row of column to
/// text, a blank cell as null).
#[derive(Debug, Clone)]
pub struct Sample {
    pub source: String,
    pub record_type: String,
    pub rows: Vec<Value>,
}

/// A scalar value as profiled; null, missing and blank are `None`.
#[derive(Debug, Clone, PartialEq)]
enum Scalar {
    Str(String),
    Num(f64),
    Bool(bool),
}

impl Scalar {
    fn of(v: &Value) -> Option<Scalar> {
        match v {
            Value::String(s) if !s.trim().is_empty() => Some(Scalar::Str(s.trim().to_string())),
            Value::Number(n) => n.as_f64().map(Scalar::Num),
            Value::Bool(b) => Some(Scalar::Bool(*b)),
            _ => None,
        }
    }

    fn text(&self) -> String {
        match self {
            Scalar::Str(s) => s.clone(),
            Scalar::Num(n) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{}", *n as i64),
            Scalar::Num(n) => n.to_string(),
            Scalar::Bool(b) => b.to_string(),
        }
    }
}

/// The key paths of `rows` (each as its key segments), in first-seen order, at most [`MAX_PATHS`];
/// true when more were left out.
fn paths(rows: &[Value]) -> (Vec<Vec<String>>, bool) {
    fn walk(v: &Value, prefix: &mut Vec<String>, seen: &mut HashSet<Vec<String>>, out: &mut Vec<Vec<String>>, truncated: &mut bool) {
        let Value::Object(map) = v else { return };
        for (k, child) in map {
            prefix.push(k.clone());
            match child {
                Value::Object(_) if prefix.len() < MAX_DEPTH => walk(child, prefix, seen, out, truncated),
                Value::Object(_) | Value::Array(_) => {}
                _ => {
                    if !seen.contains(prefix.as_slice()) {
                        if out.len() < MAX_PATHS {
                            seen.insert(prefix.clone());
                            out.push(prefix.clone());
                        } else {
                            *truncated = true;
                        }
                    }
                }
            }
            prefix.pop();
        }
    }
    let (mut seen, mut out, mut truncated) = (HashSet::new(), Vec::new(), false);
    for r in rows {
        walk(r, &mut Vec::new(), &mut seen, &mut out, &mut truncated);
    }
    (out, truncated)
}

fn at<'a>(row: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(row, |v, k| v.get(k))
}

/// Profiles every field of `sample`.
pub fn profile(sample: &Sample) -> RecordTypeProfile {
    let (paths, paths_truncated) = paths(&sample.rows);
    let fields = paths
        .iter()
        .map(|p| {
            let values: Vec<Option<Scalar>> = sample.rows.iter().map(|r| at(r, p).and_then(Scalar::of)).collect();
            field(&sample.source, &sample.record_type, &p.join("."), &values)
        })
        .collect();
    RecordTypeProfile { source: sample.source.clone(), record_type: sample.record_type.clone(), records: sample.rows.len(), fields, paths_truncated }
}

/// The distinct values of field `path` in `sample`, as text, for a link's value check.
pub fn values_at(sample: &Sample, path: &str) -> Vec<String> {
    let (paths, _) = paths(&sample.rows);
    let Some(p) = paths.iter().find(|p| p.join(".") == path) else { return Vec::new() };
    let mut seen = HashSet::new();
    sample.rows.iter().filter_map(|r| at(r, p).and_then(Scalar::of)).map(|s| s.text()).filter(|s| seen.insert(s.clone())).collect()
}

/// A number written as text: plain digits, an optional sign and decimal point.
pub(crate) fn plain_number(s: &str) -> Option<f64> {
    let ok = !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')) && s.chars().any(|c| c.is_ascii_digit());
    if ok { s.parse::<f64>().ok().filter(|n| n.is_finite()) } else { None }
}

/// An amount with a currency mark: `$4.50`, `A$4.50`, `AUD 4.50`, `4.50 AUD`.
pub(crate) fn money(s: &str) -> Option<f64> {
    let t = s.trim();
    let stripped = t
        .strip_prefix("A$")
        .or_else(|| t.strip_prefix("US$"))
        .or_else(|| t.strip_prefix('$'))
        .or_else(|| t.strip_prefix('€'))
        .or_else(|| t.strip_prefix('£'))
        .map(str::to_string)
        .or_else(|| {
            let (a, b) = t.split_once(' ')?;
            let code = |c: &str| c.len() == 3 && c.chars().all(|ch| ch.is_ascii_uppercase());
            if code(a) { Some(b.to_string()) } else if code(b) { Some(a.to_string()) } else { None }
        })?;
    plain_number(stripped.trim())
}

const DATE_FORMATS: &[(&str, &str)] = &[("%Y-%m-%d", "YYYY-MM-DD"), ("%d/%m/%Y", "DD/MM/YYYY"), ("%m/%d/%Y", "MM/DD/YYYY"), ("%Y/%m/%d", "YYYY/MM/DD"), ("%d-%m-%Y", "DD-MM-YYYY"), ("%d %b %Y", "D MMM YYYY")];
const DATETIME_FORMATS: &[(&str, &str)] = &[("%Y-%m-%d %H:%M:%S", "YYYY-MM-DD hh:mm:ss"), ("%Y-%m-%dT%H:%M:%S", "YYYY-MM-DDThh:mm:ss")];

/// The chrono notation every date in `values` parses in (day first wins, as in `date_format`), so an
/// import can turn them into ISO dates.
pub(crate) fn date_notation(values: &[&str]) -> Option<&'static str> {
    DATE_FORMATS.iter().find(|(fmt, _)| values.iter().all(|s| chrono::NaiveDate::parse_from_str(s, fmt).is_ok())).map(|(fmt, _)| *fmt)
}

/// The one notation every value parses in, and whether it is a datetime. Day first wins over month
/// first when both fit (Australian dates).
fn date_format(values: &[&str]) -> Option<(&'static str, bool)> {
    if values.iter().all(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok()) {
        return Some(("RFC 3339", true));
    }
    for (fmt, label) in DATETIME_FORMATS {
        if values.iter().all(|s| chrono::NaiveDateTime::parse_from_str(s, fmt).is_ok()) {
            return Some((label, true));
        }
    }
    DATE_FORMATS.iter().find(|(fmt, _)| values.iter().all(|s| chrono::NaiveDate::parse_from_str(s, fmt).is_ok())).map(|(_, label)| (*label, false))
}

/// A field name that says it holds a key: `id`, `customer_id`, `customerId`, `ID`.
fn id_name(path: &str) -> bool {
    let last = path.rsplit('.').next().unwrap_or(path);
    let lower = last.to_lowercase();
    lower == "id" || lower.ends_with("_id") || lower.ends_with(" id") || (last.ends_with("Id") && last.len() > 2) || (last.ends_with("ID") && last.len() > 2)
}

/// A value shaped like a key: a UUID, or a short prefix and an underscore before the rest (`cus_1`).
fn id_shaped(s: &str) -> bool {
    let uuid = s.len() == 36 && s.chars().filter(|c| *c == '-').count() == 4 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    let prefixed = s.split_once('_').is_some_and(|(p, rest)| {
        (2..=8).contains(&p.len()) && p.chars().all(|c| c.is_ascii_lowercase()) && !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric())
    });
    uuid || prefixed
}

fn field(source: &str, record_type: &str, path: &str, values: &[Option<Scalar>]) -> FieldProfile {
    let present: Vec<&Scalar> = values.iter().flatten().collect();
    let texts: Vec<String> = present.iter().map(|s| s.text()).collect();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for t in &texts {
        *counts.entry(t.as_str()).or_default() += 1;
    }
    let distinct = counts.len();
    let is_unique = !present.is_empty() && distinct == present.len();
    let strs: Vec<&str> = present.iter().filter_map(|s| if let Scalar::Str(t) = s { Some(t.as_str()) } else { None }).collect();
    let all_strs = strs.len() == present.len();
    let nums: Option<Vec<f64>> = present
        .iter()
        .map(|s| match s {
            Scalar::Num(n) => Some(*n),
            Scalar::Str(t) => plain_number(t),
            Scalar::Bool(_) => None,
        })
        .collect();
    let mut date = None;
    let inferred = if present.is_empty() {
        InferredType::Text
    } else if present.iter().all(|s| matches!(s, Scalar::Bool(_)) || matches!(s, Scalar::Str(t) if t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("false"))) {
        InferredType::Bool
    } else if id_name(path) && present.iter().all(|s| matches!(s, Scalar::Num(n) if n.fract() == 0.0) || matches!(s, Scalar::Str(t) if !t.contains(char::is_whitespace))) {
        InferredType::Id
    } else if let Some(nums) = &nums {
        if nums.iter().all(|n| n.fract() == 0.0) && !texts.iter().any(|t| t.contains('.')) { InferredType::Integer } else { InferredType::Number }
    } else if all_strs && strs.iter().all(|s| s.ends_with('%') && plain_number(s.trim_end_matches('%').trim()).is_some()) {
        InferredType::Percent
    } else if all_strs && strs.iter().all(|s| money(s).is_some()) {
        InferredType::Money
    } else if let Some((fmt, is_datetime)) = all_strs.then(|| date_format(&strs)).flatten() {
        date = Some(fmt.to_string());
        if is_datetime { InferredType::Datetime } else { InferredType::Date }
    } else if all_strs && strs.iter().all(|s| id_shaped(s)) {
        InferredType::Id
    } else if all_strs && distinct <= ENUM_MAX && distinct * 2 <= present.len() {
        InferredType::Enum
    } else {
        InferredType::Text
    };
    let numbers: Option<Vec<f64>> = match inferred {
        InferredType::Number | InferredType::Integer => nums,
        InferredType::Money => Some(strs.iter().filter_map(|s| money(s)).collect()),
        InferredType::Percent => Some(strs.iter().filter_map(|s| plain_number(s.trim_end_matches('%').trim())).collect()),
        _ => None,
    };
    let min = numbers.as_ref().and_then(|n| n.iter().copied().reduce(f64::min));
    let max = numbers.as_ref().and_then(|n| n.iter().copied().reduce(f64::max));
    // a value may hold a secret (a key pasted in a note): it is redacted before anyone reads it
    let shown = |s: &str| crate::redact::redact(&s.chars().take(VALUE_CHARS).collect::<String>(), &mut Vec::new());
    let mut top: Vec<(&str, usize)> = counts.iter().map(|(k, v)| (*k, *v)).collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let top_values = top.iter().take(TOP_VALUES).map(|(v, n)| TopValue { value: shown(v), count: *n }).collect();
    let enum_values = (inferred == InferredType::Enum).then(|| {
        let mut all: Vec<String> = counts.keys().map(|v| shown(v)).collect();
        all.sort();
        all
    });
    let null_rate = if values.is_empty() { 0.0 } else { ((values.len() - present.len()) as f64 / values.len() as f64 * 1000.0).round() / 1000.0 };
    FieldProfile {
        source: source.to_string(),
        record_type: record_type.to_string(),
        path: path.to_string(),
        inferred_type: inferred,
        null_rate,
        is_unique,
        distinct_count: distinct.min(DISTINCT_CAP),
        top_values,
        enum_values,
        date_format: date,
        min,
        max,
        sample_size: values.len(),
    }
}

// ---------------------------------------------------------------------------
// Samples from the database
// ---------------------------------------------------------------------------

/// Which records belong to `vault`: those filed in it, plus, for the caller's personal vault, the
/// caller's own records filed in none.
pub(super) struct Scope {
    pub(super) vaults: Vec<RecordId>,
    pub(super) own: bool,
    pub(super) owner: RecordId,
}

impl Scope {
    pub(super) async fn of(db: &OrgDb, owner: &RecordId, vault: &RecordId) -> AppResult<Self> {
        let personal = crate::vaults::service::personal_vault_id(db, owner).await?;
        Ok(Scope { vaults: vec![vault.clone()], own: &personal == vault, owner: owner.clone() })
    }

    pub(super) fn bind<'a>(&self, q: crate::store::Q<'a>) -> crate::store::Q<'a> {
        q.bind(("vaults", self.vaults.clone())).bind(("own", self.own)).bind(("owner", self.owner.clone()))
    }
}

#[derive(Debug, Deserialize, SurrealValue)]
struct TypeRow {
    source: String,
    #[serde(rename = "type")]
    #[surreal(rename = "type")]
    type_: String,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct PayloadRow {
    #[serde(default)]
    #[surreal(default)]
    payload: Value,
}

/// A sample of the newest records of each record type in scope, narrowed to `source` and
/// `record_type` when given; at most [`MAX_RECORD_TYPES`] types.
pub(super) async fn record_samples(db: &OrgDb, scope: &Scope, source: Option<&str>, record_type: Option<&str>) -> AppResult<Vec<Sample>> {
    let types: Vec<TypeRow> = scope.bind(q::RECORD_TYPES.on(db)).await?.take(0)?;
    let mut out = Vec::new();
    let wanted: Vec<TypeRow> =
        types.into_iter().filter(|t| source.is_none_or(|s| s == t.source) && record_type.is_none_or(|r| r == t.type_)).take(MAX_RECORD_TYPES).collect();
    for t in wanted {
        let rows: Vec<PayloadRow> = scope
            .bind(q::RECORD_SAMPLE.on(db))
            .bind(("source", t.source.clone()))
            .bind(("type", t.type_.clone()))
            .bind(("limit", SAMPLE_SIZE as i64))
            .await?
            .take(0)?;
        out.push(Sample { source: t.source, record_type: t.type_, rows: rows.into_iter().map(|r| r.payload).collect() });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample(rows: Vec<Value>) -> Sample {
        Sample { source: "s".into(), record_type: "t".into(), rows }
    }

    fn f<'a>(p: &'a RecordTypeProfile, path: &str) -> &'a FieldProfile {
        p.fields.iter().find(|f| f.path == path).unwrap_or_else(|| panic!("no {path}"))
    }

    #[test]
    fn types_null_rates_uniqueness_and_ranges() {
        let rows: Vec<Value> = (0..10)
            .map(|i| {
                json!({
                    "id": format!("ch_{i}"), "amount": 10.5 + i as f64, "qty": i, "paid": i % 2 == 0,
                    "status": if i < 7 { "succeeded" } else { "failed" }, "created": format!("2026-11-0{}", i % 9 + 1),
                    "note": if i < 4 { Value::Null } else { json!(format!("note {i}")) }, "price": format!("${}.50", i + 1),
                    "share": format!("{}%", i * 10), "customer": "cus_1",
                })
            })
            .collect();
        let p = profile(&sample(rows));
        assert_eq!(p.records, 10);
        let id = f(&p, "id");
        assert_eq!((id.inferred_type, id.is_unique, id.null_rate), (InferredType::Id, true, 0.0));
        let amount = f(&p, "amount");
        assert_eq!((amount.inferred_type, amount.min, amount.max), (InferredType::Number, Some(10.5), Some(19.5)));
        assert_eq!((f(&p, "qty").inferred_type, f(&p, "qty").max), (InferredType::Integer, Some(9.0)));
        assert_eq!(f(&p, "paid").inferred_type, InferredType::Bool);
        let status = f(&p, "status");
        assert_eq!(status.inferred_type, InferredType::Enum);
        assert_eq!(status.enum_values.as_deref(), Some(&["failed".to_string(), "succeeded".to_string()][..]));
        assert_eq!(status.top_values[0], TopValue { value: "succeeded".into(), count: 7 });
        assert!(!status.is_unique);
        let created = f(&p, "created");
        assert_eq!((created.inferred_type, created.date_format.as_deref()), (InferredType::Date, Some("YYYY-MM-DD")));
        let note = f(&p, "note");
        assert_eq!((note.null_rate, note.inferred_type), (0.4, InferredType::Text));
        assert_eq!((f(&p, "price").inferred_type, f(&p, "price").min, f(&p, "price").max), (InferredType::Money, Some(1.5), Some(10.5)));
        assert_eq!((f(&p, "share").inferred_type, f(&p, "share").max), (InferredType::Percent, Some(90.0)));
        let customer = f(&p, "customer");
        assert_eq!((customer.inferred_type, customer.distinct_count, customer.is_unique), (InferredType::Id, 1, false));
    }

    #[test]
    fn date_formats_prefer_day_first_and_datetimes_are_told_apart() {
        let p = profile(&sample(vec![json!({"d": "03/04/2026", "t": "2026-11-07T09:30:00Z", "us": "12/31/2026"}), json!({"d": "25/12/2026", "t": "2026-11-08T10:00:00+11:00", "us": "01/15/2026"})]));
        assert_eq!(f(&p, "d").date_format.as_deref(), Some("DD/MM/YYYY"));
        assert_eq!((f(&p, "t").inferred_type, f(&p, "t").date_format.as_deref()), (InferredType::Datetime, Some("RFC 3339")));
        assert_eq!(f(&p, "us").date_format.as_deref(), Some("MM/DD/YYYY"));
    }

    #[test]
    fn many_distinct_values_are_capped_and_not_an_enum() {
        let rows: Vec<Value> = (0..40).map(|i| json!({"city": format!("Town {}", i % 30)})).collect();
        let city = f(&profile(&sample(rows)), "city").clone();
        assert_eq!((city.distinct_count, city.inferred_type, city.enum_values), (DISTINCT_CAP, InferredType::Text, None));
        assert_eq!(city.top_values.len(), 5);
    }

    #[test]
    fn paths_are_walked_to_the_depth_cap_and_capped_in_number() {
        let deep = json!({"a": {"b": {"c": {"d": 1, "e": {"too_deep": 2}}}}, "list": [1, 2], "top": "x"});
        let p = profile(&sample(vec![deep]));
        let paths: Vec<&str> = p.fields.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["a.b.c.d", "top"], "arrays and anything past four levels are left out");
        let wide: serde_json::Map<String, Value> = (0..MAX_PATHS + 5).map(|i| (format!("k{i:03}"), json!(i))).collect();
        let p = profile(&sample(vec![Value::Object(wide)]));
        assert_eq!((p.fields.len(), p.paths_truncated), (MAX_PATHS, true));
    }

    #[test]
    fn a_secret_in_a_value_is_redacted() {
        let token = format!("ghp_{}", "a1B2c3D4e5".repeat(4));
        let p = profile(&sample(vec![json!({"note": token.clone()}), json!({"note": token.clone()}), json!({"note": token.clone()})]));
        let note = f(&p, "note");
        assert!(note.top_values.iter().all(|v| !v.value.contains(&token)), "{note:?}");
        assert!(note.enum_values.iter().flatten().all(|v| !v.contains(&token)), "{note:?}");
    }

    #[test]
    fn values_at_gives_each_distinct_value_once() {
        let s = sample(vec![json!({"c": "cus_1"}), json!({"c": "cus_2"}), json!({"c": "cus_1"}), json!({"c": null})]);
        assert_eq!(values_at(&s, "c"), ["cus_1", "cus_2"]);
        assert!(values_at(&s, "nope").is_empty());
    }
}
