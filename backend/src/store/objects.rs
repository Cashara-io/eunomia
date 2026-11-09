//! Read models for the ontology directory and bounded object profiles.
use super::Stmt;

pub const META: Stmt = Stmt::new("objects.meta", r#"
SELECT * FROM ontology_property WHERE vault = $vault AND kind = $kind AND status = 'approved' ORDER BY name;
SELECT status FROM ontology_kind WHERE vault = $vault AND kind = $kind;
"#);

// Only sort/filter values are loaded for the whole kind. Display values and provenance are
// fetched for the page, using the indexed shared latest-value loader.
pub const PAGE: Stmt = Stmt::new("objects.page", r#"
LET $entities = IF $table = 'entity' { (SELECT id, name FROM entity WHERE vault = $vault AND kind = $kind) }
    ELSE IF $table = 'person' { (SELECT id, name FROM person WHERE vault = $vault) }
    ELSE IF $table = 'organisation' { (SELECT id, name FROM organisation WHERE vault = $vault) }
    ELSE IF $table = 'location' { (SELECT id, name FROM location WHERE vault = $vault) }
    ELSE IF $table = 'repository' { (SELECT id, name FROM repository WHERE vault = $vault) }
    ELSE IF $table = 'file' { (SELECT id, name FROM file WHERE vault = $vault) }
    ELSE { (SELECT id, name FROM symbol WHERE vault = $vault) };
LET $candidates = SELECT id, name,
    (SELECT VALUE value_num ?? value_text ?? value_bool ?? value_date FROM entity_property
        WHERE entity = $parent.id AND property = $sort_property AND vault = $vault AND as_of <= time::now()
        ORDER BY as_of DESC, created_at DESC LIMIT 1)[0] AS sort_value,
    (SELECT VALUE value_num ?? value_text ?? value_bool ?? value_date FROM entity_property
        WHERE entity = $parent.id AND property = $filter_property AND vault = $vault AND as_of <= time::now()
        ORDER BY as_of DESC, created_at DESC LIMIT 1)[0] AS filter_value
    FROM $entities WHERE ($search = '' OR string::contains(string::lowercase(name), $search));
LET $matching = SELECT *, IF $sort_name THEN name ELSE sort_value END AS ordering,
    sort_value = NONE AS missing FROM $candidates
    WHERE $filter_op = '' OR ($filter_op = 'missing' AND filter_value = NONE)
    OR ($filter_op = 'eq' AND filter_value = $filter_value)
    OR ($filter_op = 'gte' AND filter_value >= $filter_value)
    OR ($filter_op = 'lte' AND filter_value <= $filter_value);
RETURN { total: array::len($matching), rows: IF $descending {
    (SELECT id, name, ordering, missing FROM $matching ORDER BY missing ASC, ordering DESC, id ASC LIMIT $limit START $offset)
} ELSE {
    (SELECT id, name, ordering, missing FROM $matching ORDER BY missing ASC, ordering ASC, id ASC LIMIT $limit START $offset)
} };
"#);

pub const SOURCES: Stmt = Stmt::new("objects.sources", r#"
SELECT entity, array::distinct(source) AS sources FROM entity_property
    WHERE entity IN $ids AND vault = $vault AND source != NONE GROUP BY entity;
"#);

pub const HISTORY: Stmt = Stmt::new("objects.history", r#"
SELECT <string>(id) AS id, property.name AS property, value_num, value_text, value_bool,
    <string>(value_date) AS value_date, unit, <string>(as_of) AS as_of,
    <string>(created_at) AS recorded_at, <string>(source) AS source
    FROM entity_property WHERE entity = $id AND vault = $vault
    ORDER BY as_of DESC, recorded_at DESC, id DESC LIMIT 101 START $offset;
SELECT <string>(id) AS id, label, <string>(in) AS from_id, in.name AS from_name,
    <string>(out) AS to_id, out.name AS to_name, <string>(created_at) AS recorded_at
    FROM relates_to WHERE (in = $id OR out = $id) AND in.vault = $vault AND out.vault = $vault
    ORDER BY id LIMIT 101 START $offset;
SELECT <string>(id) AS id, text, <string>(created_at) AS recorded_at, type
    FROM memory WHERE subject = $id AND vault = $vault ORDER BY created_at DESC, id LIMIT 101 START $offset;
SELECT count() AS total FROM entity_property WHERE entity = $id AND vault = $vault GROUP ALL;
SELECT label, in = $id AS outgoing, count() AS total FROM relates_to
    WHERE (in = $id OR out = $id) AND in.vault = $vault AND out.vault = $vault GROUP BY label, outgoing;
"#);

pub const SOURCE_RECORDS: Stmt = Stmt::new("objects.source_records", r#"
SELECT <string>(id) AS id, title, source, external_id, payload, <string>(updated_at) AS updated_at
    FROM cache_record WHERE id IN $ids AND vault = $vault AND deleted != true LIMIT 100;
"#);

pub const ALL: &[&Stmt] = &[&META, &PAGE, &SOURCES, &HISTORY, &SOURCE_RECORDS, &CURRENT];

pub const CURRENT: Stmt = Stmt::new("objects.current", r#"
RETURN array::flatten($props.map(|$p| (
    SELECT <string>(id) AS id, property.name AS property, value_num, value_text, value_bool,
        <string>(value_date) AS value_date, unit, <string>(as_of) AS as_of,
        <string>(created_at) AS recorded_at, <string>(source) AS source
        FROM entity_property WHERE entity = $id AND property = $p AND vault = $vault AND as_of <= time::now()
        ORDER BY as_of DESC, recorded_at DESC, id DESC LIMIT 2
)));
"#);
