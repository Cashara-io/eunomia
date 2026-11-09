//! Read models for the ontology directory and bounded object profiles.
use super::Stmt;

pub const META: Stmt = Stmt::new("objects.meta", r#"
SELECT * FROM ontology_property WHERE vault = $vault AND kind = $kind AND status = 'approved' ORDER BY name;
SELECT status FROM ontology_kind WHERE vault = $vault AND kind = $kind;
SELECT label, to_kind FROM ontology_link WHERE vault = $vault AND from_kind = $kind AND status = 'approved';
"#);

/// The value field is chosen from approved server metadata, never query text.
pub fn value_field(datatype: &str) -> &'static str {
    match datatype {
        "number" | "integer" | "money" | "percent" => "value_num",
        "bool" => "value_bool",
        "date" | "datetime" => "value_date",
        _ => "value_text",
    }
}

// Histories are traversed in typed index order. Only current points survive, so old
// values cannot move an object across a page. Missing values are a separate, last phase.
pub fn page_sql(table: &str, sort_field: Option<&str>, filter_field: Option<&str>, op: &str, descending: bool, same_property: bool, searching: bool) -> String {
    let identity = if table == "entity" { "vault = $vault AND kind = $kind" } else { "vault = $vault" };
    let linked_identity = if table == "entity" { "entity.vault = $vault AND entity.kind = $kind".to_string() }
        else { format!("entity.vault = $vault AND string::split(<string>entity, ':')[0] = '{table}'") };
    // Scalar closure variables let the planner use the compound latest-point index.
    let current = |entity: &str, property: &str| format!("([{entity}].map(|$e| [{property}].map(|$p| ({})[0])[0])[0])", super::ontology::LATEST_POINT);
    let latest = current("$this.entity", "$this.property");
    let history = format!("vault = $vault AND {linked_identity} AND as_of <= $as_of AND id = {latest}.id");
    let filter = |entity: &str| match (filter_field, op) {
        (Some(field), "missing") => format!("{}.{field} = NONE", current(entity, "$filter_property")),
        (Some(field), "eq" | "gte" | "lte") => {
            let cmp = match op { "gte" => ">=", "lte" => "<=", _ => "=" };
            format!("([{}].map(|$v| $v.{field} != NONE AND $v.{field} {cmp} $filter_value)[0])", current(entity, "$filter_property"))
        }
        _ => "true".to_string(),
    };
    let search = if searching { " AND string::contains(string::lowercase(name), $search)" } else { "" };
    let linked_search = if searching { " AND string::contains(string::lowercase(entity.name), $search)" } else { "" };
    let base_where = format!("{identity}{search} AND {}", filter("$this.id"));
    let total = match (filter_field, op) {
        (Some(field), "eq" | "gte" | "lte") => {
            let cmp = match op { "gte" => ">=", "lte" => "<=", _ => "=" };
            format!("(SELECT count() AS n FROM entity_property WHERE property = $filter_property AND {field} {cmp} $filter_value AND {field} != NONE AND {history}{linked_search} GROUP ALL)[0].n ?? 0")
        }
        _ => format!("(SELECT count() AS n FROM {table} WHERE {base_where} GROUP ALL)[0].n ?? 0"),
    };
    let direction = if descending { "DESC" } else { "ASC" };
    if let Some(field) = sort_field {
        let extra_filter = if same_property && matches!(op, "eq" | "gte" | "lte") {
            let cmp = match op { "gte" => ">=", "lte" => "<=", _ => "=" };
            format!("{field} {cmp} $filter_value")
        } else { filter("$this.entity") };
        let present = if same_property && matches!(op, "eq" | "gte") { "true".to_string() } else { format!("{field} != NONE") };
        let sort_where = format!("property = $sort_property AND {present} AND {history}{linked_search} AND {extra_filter}");
        format!(r#"
            LET $page = (SELECT entity AS id, entity.name AS name, {field} FROM entity_property
                WHERE {sort_where} ORDER BY {field} {direction}, entity {direction} LIMIT $limit START $offset);
            LET $missing = IF array::len($page) < $limit {{
                (SELECT id, name FROM {table} WHERE {base_where} AND {}.{field} = NONE
                    ORDER BY id LIMIT ($limit - array::len($page)) START math::max([0, $offset - ((SELECT count() AS n FROM entity_property WHERE {sort_where} GROUP ALL)[0].n ?? 0)]));
            }} ELSE {{ [] }};
            RETURN {{rows:array::concat($page, $missing), total:{total}}};
        "#, current("$this.id", "$sort_property"))
    } else if let (Some(field), "eq" | "gte" | "lte") = (filter_field, op) {
        let cmp = match op { "gte" => ">=", "lte" => "<=", _ => "=" };
        format!(r#"
            RETURN {{ rows:(SELECT entity AS id, entity.name AS name FROM entity_property
                WHERE property = $filter_property AND {field} {cmp} $filter_value AND {field} != NONE AND {history}
                    {linked_search}
                ORDER BY name {direction}, entity {direction} LIMIT $limit START $offset), total:{total} }};
        "#)
    } else {
        format!(r#"
            RETURN {{ rows:(SELECT id, name FROM {table} WHERE {base_where}
                ORDER BY name {direction}, id {direction} LIMIT $limit START $offset), total:{total} }};
        "#)
    }
}

pub const SOURCES: Stmt = Stmt::new("objects.sources", r#"
SELECT entity, array::distinct(source) AS sources FROM entity_property
    WHERE entity IN $ids AND vault = $vault AND source != NONE GROUP BY entity;
"#);

pub const HISTORY: Stmt = Stmt::new("objects.history", r#"
SELECT <string>(id) AS id, property.name AS property, value_num, value_text, value_bool,
    <string>(value_date) AS value_date, unit, <string>(as_of) AS as_of,
    <string>(created_at) AS recorded_at, <string>(source) AS source
    FROM entity_property WHERE entity = $id AND vault = $vault
    ORDER BY recorded_at DESC, id DESC LIMIT 101 START $offset;
SELECT <string>(id) AS id, label, <string>(in) AS from_id, in.name AS from_name,
    <string>(out) AS to_id, out.name AS to_name, <string>(created_at) AS recorded_at
    FROM relates_to WHERE (in = $id OR out = $id) AND in.vault = $vault AND out.vault = $vault
    ORDER BY recorded_at DESC, id LIMIT 101 START $offset;
SELECT <string>(id) AS id, text, <string>(created_at) AS recorded_at, type
    FROM memory WHERE subject = $id AND vault = $vault ORDER BY created_at DESC, id LIMIT 101 START $offset;
SELECT count() AS total FROM entity_property WHERE entity = $id AND vault = $vault GROUP ALL;
SELECT label, in = $id AS outgoing, count() AS total FROM relates_to
    WHERE (in = $id OR out = $id) AND in.vault = $vault AND out.vault = $vault GROUP BY label, outgoing;
"#);

pub const SOURCE_RECORDS: Stmt = Stmt::new("objects.source_records", r#"
SELECT <string>(id) AS id, title, source, external_id, payload, <string>(updated_at) AS updated_at
    FROM cache_record WHERE id IN $ids AND vault = $vault AND deleted != true;
"#);

pub const ALL: &[&Stmt] = &[&META, &SOURCES, &HISTORY, &SOURCE_RECORDS, &KINDS, &PAGE_LINKS, &CONFLICTS];

pub const KINDS: Stmt = Stmt::new("objects.kinds", r#"
RETURN array::concat((SELECT kind, count() AS entities FROM entity WHERE vault = $vault GROUP BY kind), [
    {kind:'person', entities:(SELECT count() AS n FROM person WHERE vault=$vault GROUP ALL)[0].n ?? 0},
    {kind:'organisation', entities:(SELECT count() AS n FROM organisation WHERE vault=$vault GROUP ALL)[0].n ?? 0},
    {kind:'location', entities:(SELECT count() AS n FROM location WHERE vault=$vault GROUP ALL)[0].n ?? 0},
    {kind:'repository', entities:(SELECT count() AS n FROM repository WHERE vault=$vault GROUP ALL)[0].n ?? 0},
    {kind:'file', entities:(SELECT count() AS n FROM file WHERE vault=$vault GROUP ALL)[0].n ?? 0},
    {kind:'symbol', entities:(SELECT count() AS n FROM symbol WHERE vault=$vault GROUP ALL)[0].n ?? 0}
]);
SELECT kind, status FROM ontology_kind WHERE vault = $vault ORDER BY kind;
"#);

// At most twenty displayed names plus a sentinel per row/type, even on a large hub.
pub const PAGE_LINKS: Stmt = Stmt::new("objects.page_links", r#"
RETURN array::flatten($ids.map(|$e| array::flatten($labels.map(|$label| (
    SELECT in AS entity, out AS target, label FROM relates_to
    WHERE in = $e AND in.vault = $vault AND out.vault = $vault AND label = $label
        AND string::concat(label, '|', out.kind ?? string::split(<string>out, ':')[0]) IN $allowed
    ORDER BY target LIMIT 21
)))));
"#);

// Conflict checks bind the latest timestamp once per property. Keeping the subquery
// out of CURRENT's row projection avoids re-evaluating it for unrelated history rows.
pub const CONFLICTS: Stmt = Stmt::new("objects.conflicts", r#"
RETURN $points.map(|$p| ({
    name: $p.name,
    conflicting: array::len(array::distinct((SELECT VALUE {
        num:value_num, text:value_text, boolean:value_bool, date:value_date, unit:unit ?? property.unit
    } FROM entity_property WHERE entity = $id AND property = $p.property AND vault = $vault AND as_of = $p.as_of))) > 1
}));
"#);
