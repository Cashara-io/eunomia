//! Read models for the ontology directory and bounded object profiles.
use super::Stmt;

pub const META: Stmt = Stmt::new("objects.meta", r#"
SELECT * FROM ontology_property WHERE vault = $vault AND kind = $kind AND status = 'approved' ORDER BY name;
SELECT status FROM ontology_kind WHERE vault = $vault AND kind = $kind;
SELECT label FROM ontology_link WHERE vault = $vault AND from_kind = $kind AND status = 'approved';
"#);

// Candidate identities only. Rust applies typed comparisons before slicing the output page.
pub const PAGE: Stmt = Stmt::new("objects.page", r#"
RETURN IF $table = 'entity' { (SELECT id, name FROM entity WHERE vault = $vault AND kind = $kind) }
    ELSE IF $table = 'person' { (SELECT id, name FROM person WHERE vault = $vault) }
    ELSE IF $table = 'organisation' { (SELECT id, name FROM organisation WHERE vault = $vault) }
    ELSE IF $table = 'location' { (SELECT id, name FROM location WHERE vault = $vault) }
    ELSE IF $table = 'repository' { (SELECT id, name FROM repository WHERE vault = $vault) }
    ELSE IF $table = 'file' { (SELECT id, name FROM file WHERE vault = $vault) }
    ELSE { (SELECT id, name FROM symbol WHERE vault = $vault) };
"#);

// Two property histories at most, read in one batch. Avoids a DB subquery per candidate.
pub const SORT_VALUES: Stmt = Stmt::new("objects.sort_values", r#"
SELECT id, entity, property, value_num, value_text, value_bool, value_date, as_of, created_at
    FROM entity_property WHERE vault = $vault AND property IN $props AND as_of <= time::now()
    ORDER BY as_of DESC, created_at DESC, id DESC;
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

pub const ALL: &[&Stmt] = &[&META, &PAGE, &SOURCES, &HISTORY, &SOURCE_RECORDS, &CURRENT, &SORT_VALUES, &KINDS];

pub const CURRENT: Stmt = Stmt::new("objects.current", r#"
RETURN array::flatten($props.map(|$p| (
    SELECT <string>(id) AS id, property.name AS property, value_num, value_text, value_bool,
        (SELECT value_num, value_text, value_bool, value_date, unit FROM entity_property
            WHERE entity = $parent.entity AND property = $parent.property AND vault = $vault AND as_of = $parent.as_of
            GROUP BY value_num, value_text, value_bool, value_date, unit LIMIT 2) AS alternatives,
        <string>(value_date) AS value_date, unit, <string>(as_of) AS as_of,
        <string>(created_at) AS recorded_at, <string>(source) AS source
        FROM entity_property WHERE entity = $id AND property = $p AND vault = $vault AND as_of <= time::now()
        ORDER BY as_of DESC, recorded_at DESC, id DESC LIMIT 1
)));
"#);

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
