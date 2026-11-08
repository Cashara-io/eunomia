//! An uploaded CSV as a table: profiled like one record type (each column a field), auto-mapped like
//! synced records, and, once an owner approved its kind and properties, imported row by row as
//! entities with typed values whose source is the document. A column an approved link type is mapped
//! from names each row's target: the import finds or creates that entity by name and writes the
//! relation.
//!
//! The document is still chunked and indexed as text (`documents`), so recall finds its rows either
//! way. Parsing is capped in bytes, columns and rows. The import is idempotent: an entity is found or
//! created by its name, and each value is keyed by (entity, property, document), so a re-run updates
//! values in place and never adds an entity or a row of history. A bad cell is reported with its line
//! and column and never stops the rest.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use super::profile::{self, RecordTypeProfile, Sample};
use super::property::{self, Datatype, PropertyName};
use super::{kind_rid, no_ontology_table, Kind, Origin, Status};
use crate::authz::Action;
use crate::entities::service::Lookups;
use crate::error::{AppError, AppResult};
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::state::OrgState;
use crate::store::ontology as q;

/// Largest CSV parsed as a table.
pub const MAX_TABLE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_COLUMNS: usize = 60;
pub const MAX_ROWS: usize = 5000;
/// Rows an import gets for one rate-limit unit, as a batch of facts gets ten.
pub const IMPORT_ROWS_PER_UNIT: usize = 500;
/// Bad cells listed in an import's answer; the count covers the rest.
const MAX_BAD_LISTED: usize = 100;
/// The source a CSV's fields carry in a profile and a property's `source_map`.
pub const SOURCE: &str = "document";

/// A parsed CSV: the header and each data row with the line it starts on.
#[derive(Debug, Clone)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<(u64, Vec<String>)>,
}

/// Parses `bytes` as a CSV with a header row, within the caps. Blank header cells become `column_N`
/// and a repeated header gets `_2`, `_3`, so every column has one name. Blank rows are skipped.
pub fn parse(bytes: &[u8]) -> AppResult<Table> {
    if bytes.len() > MAX_TABLE_BYTES {
        return Err(AppError::bad_request(format!("the CSV is {} bytes; a table is read up to {MAX_TABLE_BYTES} bytes: split the file", bytes.len())));
    }
    let bytes = bytes.strip_prefix("\u{feff}".as_bytes()).unwrap_or(bytes);
    let mut reader = csv::ReaderBuilder::new().flexible(true).trim(csv::Trim::All).from_reader(bytes);
    let header = reader.headers().map_err(|e| AppError::bad_request(format!("the CSV header could not be read: {e}")))?.clone();
    if header.len() > MAX_COLUMNS {
        return Err(AppError::bad_request(format!("the CSV has {} columns; a table is read up to {MAX_COLUMNS}", header.len())));
    }
    let mut columns: Vec<String> = Vec::new();
    for (i, h) in header.iter().enumerate() {
        let base = if h.is_empty() { format!("column_{}", i + 1) } else { h.chars().take(100).collect() };
        let mut name = base.clone();
        let mut n = 2;
        while columns.contains(&name) {
            name = format!("{base}_{n}");
            n += 1;
        }
        columns.push(name);
    }
    if columns.is_empty() {
        return Err(AppError::bad_request("the CSV has no header row"));
    }
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| AppError::bad_request(format!("the CSV could not be read: {e}")))?;
        if record.iter().all(str::is_empty) {
            continue;
        }
        if rows.len() == MAX_ROWS {
            return Err(AppError::bad_request(format!("the CSV has more than {MAX_ROWS} rows: split it into smaller files")));
        }
        let line = record.position().map_or(0, |p| p.line());
        rows.push((line, record.iter().map(str::to_string).collect()));
    }
    Ok(Table { columns, rows })
}

/// The table as a profiling sample: its first [`profile::SAMPLE_SIZE`] rows, one object each, a blank
/// cell as null. The record type is the document id.
pub fn sample(document_id: &str, table: &Table) -> Sample {
    let rows = table
        .rows
        .iter()
        .take(profile::SAMPLE_SIZE)
        .map(|(_, cells)| {
            let obj: serde_json::Map<String, Value> =
                table.columns.iter().enumerate().map(|(i, c)| (c.clone(), cells.get(i).filter(|v| !v.is_empty()).map_or(Value::Null, |v| json!(v)))).collect();
            Value::Object(obj)
        })
        .collect();
    Sample { source: SOURCE.to_string(), record_type: document_id.to_string(), rows }
}

/// A property name from a column header: lowercase letters, digits and `_` (`Price AUD` -> `price_aud`).
pub fn property_name_for(column: &str) -> String {
    let mut out = String::new();
    for c in column.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let out = out.trim_matches('_').to_string();
    let out = if out.chars().next().is_some_and(|c| c.is_ascii_lowercase()) { out } else { format!("c_{out}") };
    out.chars().take(64).collect::<String>().trim_end_matches('_').to_string()
}

// ---------------------------------------------------------------------------
// Profile and import plan
// ---------------------------------------------------------------------------

/// How one column would be imported.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ColumnPlan {
    pub column: String,
    /// The property it feeds, from its `source_map`.
    pub property: Option<String>,
    pub status: Option<Status>,
    /// The approved link type whose targets it names ("supplied_by supplier"), when no property takes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

/// An approved link type from the imported kind.
#[derive(Debug, Clone, Deserialize, SurrealValue)]
struct LinkFrom {
    label: String,
    to_kind: String,
    #[serde(default)]
    #[surreal(default)]
    source_map: Value,
}

impl LinkFrom {
    fn words(&self) -> String {
        format!("{} {}", self.label, self.to_kind)
    }

    fn fed_by(&self, document_id: &str, column: &str) -> bool {
        serde_json::from_value::<Vec<property::SourceField>>(self.source_map.clone())
            .unwrap_or_default()
            .iter()
            .any(|f| f.source == SOURCE && f.record_type == document_id && f.path == column)
    }
}

async fn approved_links_from(db: &OrgDb, vault: &RecordId, kind: &str) -> AppResult<Vec<LinkFrom>> {
    match q::APPROVED_LINKS_FROM.on(db).bind(("vault", vault.clone())).bind(("kind", kind.to_string())).await {
        Err(e) if no_ontology_table(&e) => Ok(Vec::new()),
        res => Ok(res?.take(0)?),
    }
}

/// The approved link type `column` names targets of: the one mapped from it, else the only one whose
/// target kind is the column's name (a `supplier` column, a `supplier` kind).
fn link_for<'a>(links: &'a [LinkFrom], document_id: &str, column: &str) -> Option<&'a LinkFrom> {
    links.iter().find(|l| l.fed_by(document_id, column)).or_else(|| {
        let name = property_name_for(column);
        match links.iter().filter(|l| l.to_kind == name).collect::<Vec<_>>().as_slice() {
            [one] => Some(*one),
            _ => None,
        }
    })
}

/// What an import of the CSV would do now.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ImportPlan {
    /// The kind its mapped properties belong to, once auto-mapping proposed them.
    pub kind: Option<String>,
    /// That kind's review status; none when it was never reviewed.
    pub kind_status: Option<Status>,
    pub columns: Vec<ColumnPlan>,
    /// The column that names each row's entity: the first one no property or link is mapped from.
    pub name_column: Option<String>,
    /// The kind is approved and at least one mapped property is: `document_import_rows` will run.
    pub ready: bool,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TableProfile {
    pub document_id: String,
    pub filename: String,
    pub rows: usize,
    pub columns: Vec<String>,
    pub profile: RecordTypeProfile,
    pub import: ImportPlan,
}

async fn kind_status(db: &OrgDb, vault: &RecordId, kind: &Kind) -> AppResult<Option<Status>> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        status: String,
    }
    let rows: Vec<Row> = match q::KIND.on(db).bind(("id", kind_rid(vault, kind))).await {
        Err(e) if no_ontology_table(&e) => return Ok(None),
        res => res?.take(0)?,
    };
    rows.into_iter().next().map(|r| Status::parse(&r.status)).transpose()
}

async fn plan(db: &OrgDb, vault: &RecordId, document_id: &str, columns: &[String]) -> AppResult<ImportPlan> {
    let by_kind = property::properties_by_kind(db, vault).await?;
    let fed_by = |p: &property::PropertyItem, col: &str| p.source_map.iter().any(|f| f.source == SOURCE && f.record_type == document_id && f.path == col);
    // the kind most of this document's columns were mapped onto
    let kind = by_kind
        .iter()
        .map(|(k, props)| (k, columns.iter().filter(|c| props.iter().any(|p| fed_by(p, c))).count()))
        .filter(|(_, n)| *n > 0)
        .max_by_key(|(_, n)| *n)
        .map(|(k, _)| k.clone());
    let props = kind.as_ref().and_then(|k| by_kind.get(k)).cloned().unwrap_or_default();
    let plans: Vec<ColumnPlan> = columns
        .iter()
        .map(|c| {
            let fed: Vec<_> = props.iter().filter(|p| fed_by(p, c)).collect();
            let approved: Vec<_> = fed.iter().filter(|p| p.status == Status::Approved).collect();
            // a column two approved properties claim has no default: the import asks for a column_map
            let p = match (approved.as_slice(), fed.as_slice()) {
                ([one], _) => Some(**one),
                ([], [one]) => Some(*one),
                _ => None,
            };
            ColumnPlan { column: c.clone(), property: p.map(|p| p.name.clone()), status: p.map(|p| p.status), link: None }
        })
        .collect();
    let links = match &kind {
        Some(k) => approved_links_from(db, vault, k).await?,
        None => Vec::new(),
    };
    let mut plans = plans;
    for c in plans.iter_mut().filter(|c| c.property.is_none()) {
        c.link = link_for(&links, document_id, &c.column).map(LinkFrom::words);
    }
    let kind_status = match &kind {
        Some(k) => kind_status(db, vault, &Kind::parse(k)?).await?,
        None => None,
    };
    let name_column = plans.iter().find(|c| c.property.is_none() && c.link.is_none()).or(plans.first()).map(|c| c.column.clone());
    let ready = kind_status == Some(Status::Approved) && plans.iter().any(|c| c.status == Some(Status::Approved));
    Ok(ImportPlan { kind, kind_status, columns: plans, name_column, ready })
}

/// The profile of an uploaded CSV the caller may read, and what an import would do now.
pub async fn document_profile(state: &OrgState, user: &RecordId, id: &str) -> AppResult<TableProfile> {
    let doc = crate::documents::load_csv(state, user, id, Action::ReadMemories).await?;
    let table = parse(&doc.bytes)?;
    let doc_id = doc.id.to_string();
    let profile = profile::profile(&sample(&doc_id, &table));
    let import = plan(&state.db, &doc.vault, &doc_id, &table.columns).await?;
    Ok(TableProfile { document_id: doc_id, filename: doc.filename, rows: table.rows.len(), columns: table.columns, profile, import })
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct ImportArgs {
    pub document_id: String,
    /// The approved kind each row becomes.
    pub kind: String,
    /// The column holding each row's entity name.
    pub name_column: String,
    /// Column to property name; without it, each column's mapped property (its `source_map`), else
    /// the property named like the column.
    #[serde(default)]
    pub column_map: Option<BTreeMap<String, String>>,
    /// When the values were true (ISO 8601), if not now.
    #[serde(default)]
    pub as_of: Option<String>,
}

/// One column of an import: the property it wrote, or why it was left out.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ColumnUse {
    pub column: String,
    pub property: Option<String>,
    /// The link type whose targets it named ("supplied_by supplier").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

/// A row or cell that was not imported.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct BadCell {
    /// The file's line (the header is line 1).
    pub line: u64,
    /// The column, or none when the whole row was left out.
    pub column: Option<String>,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ImportOut {
    pub document_id: String,
    pub kind: String,
    /// Data rows in the file.
    pub rows: usize,
    /// Entities made by this run, and ones it found and updated.
    pub created: usize,
    pub updated: usize,
    /// Values written (each new or updated in place).
    pub values: usize,
    /// Relations this run wrote (a re-run writes none), and the link targets it created by name.
    pub linked: usize,
    pub targets_created: usize,
    pub columns: Vec<ColumnUse>,
    /// Rows and cells left out, at most 100 listed.
    pub bad: Vec<BadCell>,
    pub bad_count: usize,
}

fn not_ready(kind: &Kind, why: &str) -> AppError {
    AppError::bad_request(format!(
        "the kind {:?} {why}: a vault owner approves the kind and its properties in the Eunomia app (the vault's Kinds and links), then import again",
        kind.as_str()
    ))
}

/// Imports the CSV's rows as entities of an approved `kind` with values of its approved properties.
/// Refused before the kind is approved, or when no column feeds an approved property.
pub async fn import_rows(state: &OrgState, user: &RecordId, args: &ImportArgs, origin: Origin) -> AppResult<ImportOut> {
    let db = &state.db;
    let doc = crate::documents::load_csv(state, user, &args.document_id, Action::WriteMemories).await?;
    let kind = Kind::parse(&args.kind)?;
    match kind_status(db, &doc.vault, &kind).await? {
        Some(Status::Approved) => {}
        Some(Status::Rejected) => return Err(not_ready(&kind, "was rejected for this vault")),
        _ => return Err(not_ready(&kind, "is not approved for this vault yet")),
    }
    let table = parse(&doc.bytes)?;
    // the request already paid one unit; each further IMPORT_ROWS_PER_UNIT rows cost one more
    crate::gate::charge_more(table.rows.len().div_ceil(IMPORT_ROWS_PER_UNIT).saturating_sub(1))?;
    let doc_id = doc.id.to_string();
    let name_at = table
        .columns
        .iter()
        .position(|c| c == args.name_column.trim())
        .ok_or_else(|| AppError::bad_request(format!("no column {:?}: the columns are {}", args.name_column, table.columns.join(", "))))?;
    let as_of = Datetime::from(property::parse_as_of(args.as_of.as_deref())?);

    // which property each column feeds, checked as entity_set_property checks it
    let rows: Vec<property::PropertyRow> = q::PROPERTIES_OF_KIND.on(db).bind(("vault", doc.vault.clone())).bind(("kind", kind.as_str().to_string())).await?.take(0)?;
    let links = approved_links_from(db, &doc.vault, kind.as_str()).await?;
    let mut columns = Vec::new();
    let mut targets: Vec<(usize, property::PropertyRow, Datatype)> = Vec::new();
    let mut link_targets: Vec<(usize, LinkTarget)> = Vec::new();
    for (i, col) in table.columns.iter().enumerate().filter(|(i, _)| *i != name_at) {
        // a column no property takes may name the targets of an approved link type
        let as_link = match &args.column_map {
            Some(map) => map
                .get(col)
                .filter(|v| !rows.iter().any(|r| &r.name == *v))
                .and_then(|v| links.iter().find(|l| same_label(&l.label, v) || l.words() == v.trim())),
            None => {
                let approved = |r: &&property::PropertyRow| r.status == Status::Approved.as_str();
                let claimed = rows.iter().filter(approved).any(|r| r.source_map().iter().any(|f| f.source == SOURCE && f.record_type == doc_id && &f.path == col));
                // a property named like the column wins over a link guessed from the kind's name
                let named = rows.iter().filter(approved).any(|r| r.name == property_name_for(col));
                match (claimed, named) {
                    (true, _) => None,
                    (false, true) => links.iter().find(|l| l.fed_by(&doc_id, col)),
                    (false, false) => link_for(&links, &doc_id, col),
                }
            }
        };
        if let Some(l) = as_link {
            match LinkTarget::check(db, &doc.vault, &kind, l).await {
                Ok(t) => {
                    columns.push(ColumnUse { column: col.clone(), property: None, link: Some(l.words()), skipped: None });
                    link_targets.push((i, t));
                }
                Err(e) => columns.push(ColumnUse { column: col.clone(), property: None, link: Some(l.words()), skipped: Some(e.message) }),
            }
            continue;
        }
        let wanted = match &args.column_map {
            Some(map) => map.get(col).cloned(),
            None => {
                let claimed: Vec<&str> = rows
                    .iter()
                    .filter(|r| r.status == Status::Approved.as_str())
                    .filter(|r| r.source_map().iter().any(|f| f.source == SOURCE && f.record_type == doc_id && &f.path == col))
                    .map(|r| r.name.as_str())
                    .collect();
                // two properties claiming one column (an agent can add a mapping to an approved one) is
                // the owner's call, never a silent pick
                if claimed.len() > 1 {
                    let skipped = format!("is mapped to {}: choose one with column_map", claimed.join(" and "));
                    columns.push(ColumnUse { column: col.clone(), property: None, link: None, skipped: Some(skipped) });
                    continue;
                }
                claimed.first().map(|n| n.to_string()).or_else(|| Some(property_name_for(col)).filter(|n| rows.iter().any(|r| &r.name == n)))
            }
        };
        let Some(wanted) = wanted else {
            columns.push(ColumnUse { column: col.clone(), property: None, link: None, skipped: Some("no property or link is mapped from it".into()) });
            continue;
        };
        let resolved = match PropertyName::parse(&wanted) {
            Ok(name) => property::resolve_approved(db, &doc.vault, &kind, &name).await,
            Err(e) => Err(e),
        };
        match resolved {
            Ok((row, _)) => {
                let datatype = Datatype::parse(&row.datatype)?;
                columns.push(ColumnUse { column: col.clone(), property: Some(row.name.clone()), link: None, skipped: None });
                targets.push((i, row, datatype));
            }
            Err(e) => columns.push(ColumnUse { column: col.clone(), property: Some(wanted), link: None, skipped: Some(e.message) }),
        }
    }
    if targets.is_empty() && link_targets.is_empty() {
        return Err(not_ready(&kind, "has no approved property that a column of this CSV feeds"));
    }

    // the notation each date column is written in, as profiling reads it (day first for Australian dates)
    let notations: HashMap<usize, &'static str> = targets
        .iter()
        .filter(|(_, _, d)| *d == Datatype::Date)
        .filter_map(|(i, _, _)| {
            let values: Vec<&str> = table.rows.iter().filter_map(|(_, c)| c.get(*i)).map(|v| v.trim()).filter(|v| !v.is_empty()).collect();
            super::profile::date_notation(&values).map(|f| (*i, f))
        })
        .collect();
    let mut out = ImportOut {
        document_id: doc_id.clone(),
        kind: kind.as_str().to_string(),
        rows: table.rows.len(),
        created: 0,
        updated: 0,
        values: 0,
        linked: 0,
        targets_created: 0,
        columns,
        bad: Vec::new(),
        bad_count: 0,
    };
    // each target once per run, by its name in any case
    let mut found: HashMap<(usize, String), RecordId> = HashMap::new();
    let mut seen: HashMap<String, ()> = HashMap::new();
    let mut names: HashMap<String, u64> = HashMap::new();
    let mut pending: Vec<ImportedValue> = Vec::new();
    // one read of the kind's aliases, so a new name is not looked up as an alias row by row (a scan)
    let alias_keys: HashSet<String> = {
        let table = crate::entities::service::kind_table(kind.as_str())?;
        let lists: Vec<Vec<String>> = crate::store::entities::alias_keys_of(db, table).bind(("vault", doc.vault.clone())).bind(("kind", kind.as_str().to_string())).await?.take(0)?;
        lists.into_iter().flatten().collect()
    };
    for (line, cells) in &table.rows {
        if cells.len() != table.columns.len() {
            bad(&mut out, *line, None, format!("has {} cells; the header has {}", cells.len(), table.columns.len()));
            continue;
        }
        let name = cells[name_at].trim();
        if name.is_empty() {
            bad(&mut out, *line, Some(&table.columns[name_at]), "no name: the row was left out".into());
            continue;
        }
        // two rows with one name would land on one entity, the later silently overwriting the earlier
        if let Some(first) = names.get(&name.to_lowercase()) {
            bad(&mut out, *line, Some(&table.columns[name_at]), format!("repeats the name {name:?} from line {first}: the row was left out"));
            continue;
        }
        names.insert(name.to_lowercase(), *line);
        // load_csv checked the vault for writing and the kind is approved, so no per-row authz or grounding
        let lookups = if alias_keys.contains(&name.to_lowercase()) { Lookups::NameAndAlias } else { Lookups::NameOnly };
        let (entity, created) = crate::entities::service::upsert_in_vault(db, user, kind.as_str(), name, None, &doc.vault, lookups).await?;
        let first_time = seen.insert(entity.id.clone(), ()).is_none();
        match (created, first_time) {
            (true, _) => out.created += 1,
            (false, true) => out.updated += 1,
            _ => {}
        }
        let entity: RecordId = crate::rid::parse(&entity.id).map_err(|_| AppError::internal("entity id did not round-trip"))?;
        for (i, row, datatype) in &targets {
            let cell = cells[*i].trim();
            if cell.is_empty() {
                continue;
            }
            let cell = local_cell(*datatype, cell, row.unit.as_deref(), notations.get(i).copied());
            let stored = match property::parse_value(*datatype, &row.name, row.enum_values.as_deref(), &json!(cell)) {
                Ok(s) => s,
                Err(e) => {
                    bad(&mut out, *line, Some(&table.columns[*i]), e.message);
                    continue;
                }
            };
            let key = format!("{}|{}|{doc_id}", entity.to_string(), row.id.to_string());
            pending.push(ImportedValue {
                id: RecordId::from_table_key("entity_property", crate::tx::stable_key('i', &key)),
                entity: entity.clone(),
                property: row.id.clone(),
                value_num: stored.num,
                value_text: stored.text,
                value_bool: stored.boolean,
                value_date: stored.date,
            });
            out.values += 1;
        }
        for (i, link) in &link_targets {
            let cell = cells[*i].trim();
            if cell.is_empty() {
                continue;
            }
            let key = (*i, cell.to_lowercase());
            let target = match found.get(&key) {
                Some(t) => t.clone(),
                None => match crate::entities::service::upsert_in_vault(db, user, &link.kind, cell, None, &doc.vault, Lookups::NameAndAlias).await {
                    Ok((t, created)) => {
                        out.targets_created += usize::from(created);
                        let t: RecordId = crate::rid::parse(&t.id).map_err(|_| AppError::internal("entity id did not round-trip"))?;
                        found.insert(key, t.clone());
                        t
                    }
                    Err(e) => {
                        bad(&mut out, *line, Some(&table.columns[*i]), format!("could not find or create the {} {cell:?}: {}", link.kind.replace('_', " "), e.message));
                        continue;
                    }
                },
            };
            match crate::entities::service::relate_in_vault(db, user, &doc.vault, &entity, &target, &link.label).await {
                Ok(new) => out.linked += usize::from(new),
                Err(e) => bad(&mut out, *line, Some(&table.columns[*i]), format!("could not link to {cell:?}: {}", e.message)),
            }
        }
        if pending.len() >= IMPORT_BATCH {
            write_values(db, &doc.vault, &doc.id, as_of, origin, std::mem::take(&mut pending)).await?;
        }
    }
    write_values(db, &doc.vault, &doc.id, as_of, origin, pending).await?;
    Ok(out)
}

/// Relations labelled like `a` and `b` are one link type: case, spaces and `_` aside.
fn same_label(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim().to_lowercase().replace('_', " ").split_whitespace().collect::<Vec<_>>().join(" ");
    norm(a) == norm(b)
}

/// Where a link column's cells go: entities of `kind`, related by `label`.
struct LinkTarget {
    kind: String,
    label: String,
}

impl LinkTarget {
    /// The approved link type `l` from `kind`, its target kind grounded once (a rejected kind's merge
    /// target, or refused in strict mode).
    async fn check(db: &OrgDb, vault: &RecordId, kind: &Kind, l: &LinkFrom) -> AppResult<Self> {
        let to = match super::ground(db, vault, &l.to_kind).await? {
            Some(mapped) => mapped.to,
            None => l.to_kind.clone(),
        };
        super::ground_link(db, vault, kind.as_str(), &l.label, &to).await?;
        Ok(LinkTarget { kind: to, label: l.label.clone() })
    }
}

/// Values one import statement writes.
const IMPORT_BATCH: usize = 500;

/// One imported value, keyed by (entity, property, document) in `id`.
#[derive(Debug, Clone, SurrealValue)]
struct ImportedValue {
    id: RecordId,
    entity: RecordId,
    property: RecordId,
    value_num: Option<f64>,
    value_text: Option<String>,
    value_bool: Option<bool>,
    value_date: Option<Datetime>,
}

/// Writes `values` in one statement, each updated in place on a re-run.
async fn write_values(db: &OrgDb, vault: &RecordId, document: &RecordId, as_of: Datetime, origin: Origin, values: Vec<ImportedValue>) -> AppResult<()> {
    if values.is_empty() {
        return Ok(());
    }
    crate::tx::with_retry_dup(|| async {
        q::UPSERT_IMPORTED_VALUES
            .on(db)
            .bind(("values", values.clone()))
            .bind(("vault", vault.clone()))
            .bind(("as_of", as_of))
            .bind(("source", document.clone()))
            .bind(("origin", origin.as_str().to_string()))
            .await
    })
    .await?
    .check()?;
    Ok(())
}

/// A cell as a spreadsheet writes it, in the form `parse_value` takes: `$1,200.50` or `A$4` for money,
/// `10%` for a percent, and a date in the column's own notation (`25/12/2026`) as an ISO date. Anything
/// that does not fit, or would change meaning, is passed on unchanged, so its error names the cell as
/// written.
fn local_cell(datatype: Datatype, cell: &str, unit: Option<&str>, notation: Option<&str>) -> String {
    match datatype {
        Datatype::Money => local_money(cell, unit).unwrap_or_else(|| cell.to_string()),
        Datatype::Percent => cell
            .strip_suffix('%')
            .map(str::trim)
            .filter(|n| super::profile::plain_number(n).is_some())
            .map_or_else(|| cell.to_string(), str::to_string),
        Datatype::Date => notation
            .and_then(|f| chrono::NaiveDate::parse_from_str(cell, f).ok())
            .map_or_else(|| cell.to_string(), |d| d.format("%Y-%m-%d").to_string()),
        _ => cell.to_string(),
    }
}

/// Currencies written with a bare `$`.
const DOLLARS: &[&str] = &["AUD", "USD", "NZD", "CAD", "SGD", "HKD", "TWD", "FJD", "BSD", "BBD", "BZD", "BND", "JMD", "TTD", "XCD", "NAD", "LRD", "SBD"];

/// The amount in a money cell, when reading it keeps its meaning: a currency mark or code must match
/// the property's unit (a bare `$` is taken as that unit), and a comma must be a thousands separator.
fn local_money(cell: &str, unit: Option<&str>) -> Option<String> {
    let t = cell.trim();
    let code = |c: &str| c.len() == 3 && c.chars().all(|ch| ch.is_ascii_uppercase());
    let (amount, currency) = if let Some(r) = t.strip_prefix("A$") {
        (r, Some("AUD"))
    } else if let Some(r) = t.strip_prefix("US$") {
        (r, Some("USD"))
    } else if let Some(r) = t.strip_prefix('$') {
        // a bare $ is any dollar, so it is taken as the property's unit only when that is a dollar too
        if unit.is_some_and(|u| !DOLLARS.contains(&u.trim().to_ascii_uppercase().as_str())) {
            return None;
        }
        (r, None)
    } else if let Some(r) = t.strip_prefix('€') {
        (r, Some("EUR"))
    } else if let Some(r) = t.strip_prefix('£') {
        (r, Some("GBP"))
    } else if let Some((a, b)) = t.split_once(' ').filter(|(a, b)| code(a) || code(b)) {
        if code(a) { (b, Some(a)) } else { (a, Some(b)) }
    } else {
        (t, None)
    };
    if let (Some(c), Some(u)) = (currency, unit)
        && !c.eq_ignore_ascii_case(u.trim())
    {
        return None;
    }
    let plain = super::profile::without_thousands(amount.trim())?;
    super::profile::plain_number(&plain).map(|n| n.to_string())
}

fn bad(out: &mut ImportOut, line: u64, column: Option<&str>, error: String) {
    out.bad_count += 1;
    if out.bad.len() < MAX_BAD_LISTED {
        out.bad.push(BadCell { line, column: column.map(str::to_string), error });
    }
}

/// Drops the values an import of `document` wrote, when the document is deleted. Best effort on a
/// database before migration 0034, which has none.
pub async fn delete_imported_values(db: &OrgDb, document: &RecordId) -> AppResult<()> {
    match q::DELETE_IMPORTED_VALUES.on(db).bind(("document", document.clone())).await {
        Err(e) if no_ontology_table(&e) => Ok(()),
        res => {
            res?.check()?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_money_cell_is_read_only_when_its_meaning_is_kept() {
        let m = |cell: &str, unit: Option<&str>| super::local_money(cell, unit);
        assert_eq!(m("$1,200.50", Some("AUD")).as_deref(), Some("1200.5"));
        assert_eq!(m("A$4", Some("aud")).as_deref(), Some("4"));
        assert_eq!(m("-1,200", None).as_deref(), Some("-1200"));
        assert_eq!(m("+1,200", None).as_deref(), Some("1200"));
        assert_eq!(m("€5", Some("EUR")).as_deref(), Some("5"));
        // each of these would otherwise be stored as a different number
        for cell in ["0,500", "-,200", "1,5", "1,20", "12,34,567", "1.234,56", "US$5"] {
            assert_eq!(m(cell, Some("AUD")), None, "{cell}");
        }
        assert_eq!(m("$5", Some("EUR")), None, "a bare $ is not a euro");
        assert_eq!(m("AUD 5", Some("kg")), None);
    }

    use super::*;

    #[test]
    fn headers_get_one_name_each_and_blank_rows_are_skipped() {
        let t = parse("\u{feff}name, price ,,price\nLatte,4.5,x,1\n,,,\nMocha,5,,2\n".as_bytes()).unwrap();
        assert_eq!(t.columns, ["name", "price", "column_3", "price_2"]);
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.rows[1], (4, vec!["Mocha".to_string(), "5".into(), "".into(), "2".into()]));
    }

    #[test]
    fn the_byte_column_and_row_caps_hold() {
        let big = vec![b'a'; MAX_TABLE_BYTES + 1];
        assert!(parse(&big).unwrap_err().message.contains("bytes"));
        let wide = (0..=MAX_COLUMNS).map(|i| format!("c{i}")).collect::<Vec<_>>().join(",");
        assert!(parse(wide.as_bytes()).unwrap_err().message.contains("columns"));
        let at_cap = format!("n\n{}", "x\n".repeat(MAX_ROWS));
        assert_eq!(parse(at_cap.as_bytes()).unwrap().rows.len(), MAX_ROWS);
        let over = format!("n\n{}", "x\n".repeat(MAX_ROWS + 1));
        assert!(parse(over.as_bytes()).unwrap_err().message.contains("rows"));
    }

    #[test]
    fn a_csv_profiles_like_a_record_type() {
        let t = parse(b"product,price,status,listed\nLatte,4.50,Active,2026-11-01\nMocha,5.00,Active,2026-11-02\nChai,,Retired,2026-11-03\nTea,3.80,Active,2026-11-04\n").unwrap();
        let p = profile::profile(&sample("document:abc", &t));
        assert_eq!((p.source.as_str(), p.record_type.as_str(), p.records), ("document", "document:abc", 4));
        let price = p.fields.iter().find(|f| f.path == "price").unwrap();
        assert_eq!((price.inferred_type, price.null_rate, price.min, price.max), (profile::InferredType::Number, 0.25, Some(3.8), Some(5.0)));
        let status = p.fields.iter().find(|f| f.path == "status").unwrap();
        assert_eq!(status.inferred_type, profile::InferredType::Enum);
        assert_eq!(p.fields.iter().find(|f| f.path == "listed").unwrap().date_format.as_deref(), Some("YYYY-MM-DD"));
        assert!(p.fields.iter().find(|f| f.path == "product").unwrap().is_unique);
    }

    #[test]
    fn column_headers_become_property_names() {
        assert_eq!(property_name_for("Price AUD"), "price_aud");
        assert_eq!(property_name_for(" lead_time (days) "), "lead_time_days");
        assert_eq!(property_name_for("2026 total"), "c_2026_total");
    }
}
