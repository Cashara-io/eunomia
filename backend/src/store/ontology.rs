//! Statements for the project ontology (`ontology`, migrations 0033, 0034 and 0035). See the module docs in store/mod.rs.

use super::Stmt;

/// The kind of a `relates_to` endpoint: a custom-kind entity's `kind`, else its table. Casts are not
/// functions, so this stays inside the hardened server's `--allow-funcs` list (no `record::tb`).
macro_rules! end_kind {
    ($end:literal) => {
        concat!("(", $end, ".kind ?? string::split(<string> ", $end, ", \":\")[0])")
    };
}

macro_rules! built_in {
    ($t:literal) => {
        concat!(
            "{ kind: \"", $t, "\", count: (SELECT count() AS n FROM ", $t, " WHERE vault = $vault GROUP ALL)[0].n ?? 0, ",
            "samples: (SELECT VALUE name FROM ", $t, " WHERE vault = $vault ORDER BY name LIMIT 3) }"
        )
    };
}

/// What `$vault` holds, for the ontology view: every built-in kind and every custom kind with its
/// entity count and a few names (slot 1), then every link type in use, grouped by (from kind, label,
/// to kind), with its count and a few example pairs (slot 2). The pairs are an aggregate, not a
/// subquery per group: inside a closure, a subquery's `in.vault` filter matches nothing on 3.3.5.
// ponytail: the link half reads every relates_to edge of the org (no index on the endpoint's vault);
// fine for a review page, not for a hot path.
pub const PROFILE: Stmt = Stmt::new(
    "ontology.profile",
    concat!(
        "LET $custom = (SELECT kind, count() AS count FROM entity WHERE vault = $vault GROUP BY kind); ",
        "RETURN array::concat([",
        built_in!("person"), ", ",
        built_in!("organisation"), ", ",
        built_in!("location"), ", ",
        built_in!("repository"), ", ",
        built_in!("file"), ", ",
        built_in!("symbol"),
        "], $custom.map(|$k| ({ kind: $k.kind, count: $k.count, ",
        "samples: (SELECT VALUE name FROM entity WHERE vault = $vault AND kind = $k.kind ORDER BY name LIMIT 3) }))); ",
        "SELECT label, ", end_kind!("in"), " AS from_kind, ", end_kind!("out"), " AS to_kind, count() AS count, ",
        "array::slice(array::distinct({ from_name: in.name, to_name: out.name }), 0, 3) AS samples ",
        "FROM relates_to WHERE in.vault = $vault GROUP BY label, from_kind, to_kind;"
    ),
);
pub const PROFILE_KINDS_SLOT: usize = 1;
pub const PROFILE_LINKS_SLOT: usize = 2;

/// The registry rows of `$vault`: kinds (slot 0), then links (slot 1).
pub const REGISTRY: Stmt = Stmt::new(
    "ontology.registry",
    "SELECT * FROM ontology_kind WHERE vault = $vault ORDER BY kind; \
     SELECT * FROM ontology_link WHERE vault = $vault ORDER BY from_kind, label, to_kind;",
);

/// The kind rows of every vault in `$vaults`, for `entity_types`.
pub const KINDS_IN: Stmt =
    Stmt::new("ontology.kinds_in", "SELECT vault, kind, description, status, merge_into FROM ontology_kind WHERE vault IN $vaults ORDER BY kind");

/// One kind's decision, by its derived id: the one read a write under a custom kind pays.
pub const GROUND: Stmt = Stmt::new("ontology.ground", "SELECT status, merge_into FROM $id");

/// The approved kinds of `$vault`, to name in the refusal of a rejected one.
pub const APPROVED_KINDS: Stmt =
    Stmt::new("ontology.approved_kinds", r#"SELECT VALUE kind FROM ontology_kind WHERE vault = $vault AND status = "approved" ORDER BY kind"#);

/// Whether `$kind` can take merged writes in `$vault`: its row's status (slot 0) and whether any entity
/// has it (slot 1).
pub const TARGET_STATE: Stmt = Stmt::new(
    "ontology.target_state",
    "SELECT VALUE status FROM $id; SELECT VALUE id FROM entity WHERE vault = $vault AND kind = $kind LIMIT 1;",
);

/// A proposal: creates the row, or refreshes a still-proposed one (a field left out keeps its value).
/// A row a person approved or rejected is returned as it is, so a proposal never undoes a decision.
pub const PROPOSE_KIND: Stmt = Stmt::at(
    "ontology.propose_kind",
    r#"BEGIN TRANSACTION;
        LET $cur = (SELECT VALUE status FROM $id);
        LET $row = IF array::len($cur) = 0 {
            (CREATE $id SET vault = $vault, kind = $kind, description = $description, merge_into = $merge_into,
                synonyms = $synonyms, origin = $origin, reason = $reason RETURN AFTER)
        } ELSE IF $cur[0] = "proposed" {
            (UPDATE $id SET description = $description ?? description, merge_into = $merge_into ?? merge_into,
                synonyms = array::union(synonyms ?? [], $synonyms), reason = $reason ?? reason, updated_at = time::now() RETURN AFTER)
        } ELSE {
            (SELECT * FROM $id)
        };
        RETURN $row;
        COMMIT TRANSACTION;"#,
    3, // BEGIN 0, two LETs 1 and 2, RETURN 3
);

pub const PROPOSE_LINK: Stmt = Stmt::at(
    "ontology.propose_link",
    r#"BEGIN TRANSACTION;
        LET $cur = (SELECT VALUE status FROM $id);
        LET $row = IF array::len($cur) = 0 {
            (CREATE $id SET vault = $vault, from_kind = $from_kind, label = $label, to_kind = $to_kind,
                description = $description, origin = $origin, reason = $reason RETURN AFTER)
        } ELSE IF $cur[0] = "proposed" {
            (UPDATE $id SET description = $description ?? description, reason = $reason ?? reason,
                updated_at = time::now() RETURN AFTER)
        } ELSE {
            (SELECT * FROM $id)
        };
        RETURN $row;
        COMMIT TRANSACTION;"#,
    3,
);

/// A person's approval or rejection of a kind (a row is made for a kind only observed so far). Kinds
/// rejected into this one follow it to its new target, so a merge never points at a rejected kind.
pub const DECIDE_KIND: Stmt = Stmt::new(
    "ontology.decide_kind",
    r#"BEGIN TRANSACTION;
        IF array::len(SELECT VALUE id FROM $id) = 0 {
            CREATE $id SET vault = $vault, kind = $kind, origin = "human"
        };
        UPDATE $id SET status = $status, merge_into = $merge_into, decided_by = $user, updated_at = time::now();
        IF $status = "rejected" {
            UPDATE ontology_kind SET merge_into = $merge_into, updated_at = time::now()
                WHERE vault = $vault AND status = "rejected" AND merge_into = $kind
        };
        COMMIT TRANSACTION;"#,
);

pub const DESCRIBE_KIND: Stmt = Stmt::new(
    "ontology.describe_kind",
    r#"BEGIN TRANSACTION;
        IF array::len(SELECT VALUE id FROM $id) = 0 {
            CREATE $id SET vault = $vault, kind = $kind, origin = "human"
        };
        UPDATE $id SET description = $description, updated_at = time::now();
        COMMIT TRANSACTION;"#,
);

pub const DECIDE_LINK: Stmt = Stmt::new(
    "ontology.decide_link",
    r#"BEGIN TRANSACTION;
        IF array::len(SELECT VALUE id FROM $id) = 0 {
            CREATE $id SET vault = $vault, from_kind = $from_kind, label = $label, to_kind = $to_kind, origin = "human"
        };
        UPDATE $id SET status = $status, decided_by = $user, updated_at = time::now();
        COMMIT TRANSACTION;"#,
);

pub const DESCRIBE_LINK: Stmt = Stmt::new(
    "ontology.describe_link",
    r#"BEGIN TRANSACTION;
        IF array::len(SELECT VALUE id FROM $id) = 0 {
            CREATE $id SET vault = $vault, from_kind = $from_kind, label = $label, to_kind = $to_kind, origin = "human"
        };
        UPDATE $id SET description = $description, updated_at = time::now();
        COMMIT TRANSACTION;"#,
);

/// One link type's decision, by its derived id: the one read a relation write pays.
pub const GROUND_LINK: Stmt = Stmt::new("ontology.ground_link", "SELECT VALUE status FROM $id");

/// The approved labels from `$from_kind` to `$to_kind` in `$vault`, to name in the refusal of a rejected one.
pub const APPROVED_LINKS: Stmt = Stmt::new(
    "ontology.approved_links",
    r#"SELECT VALUE label FROM ontology_link WHERE vault = $vault AND from_kind = $from_kind AND to_kind = $to_kind AND status = "approved" ORDER BY label"#,
);

/// The property rows of `$vault` (slot 0), then per property its value count and a few values (slot 1).
// ponytail: the stats read every value row of the vault; fine for a review page, not for a hot path.
pub const PROPERTIES: Stmt = Stmt::new(
    "ontology.properties",
    "SELECT * FROM ontology_property WHERE vault = $vault ORDER BY kind, name; \
     SELECT property, count() AS count, \
        array::slice(array::distinct({ num: value_num, text: value_text, boolean: value_bool, date: value_date, unit: unit }), 0, 3) AS samples \
        FROM entity_property WHERE vault = $vault GROUP BY property;",
);

/// The newest facts of `$vault` with their subject's kind, for the suggest profile: evidence for
/// properties and synonyms.
pub const RECENT_FACTS: Stmt = Stmt::new(
    "ontology.recent_facts",
    concat!(
        "SELECT text, ", end_kind!("subject"), " AS kind FROM memory WHERE vault = $vault AND type != \"observation\" ",
        "ORDER BY created_at DESC LIMIT 400"
    ),
);

/// One property, by its derived id.
pub const PROPERTY: Stmt = Stmt::new("ontology.property", "SELECT * FROM $id");

/// The approved property names of `$kind` in `$vault`, to name in a refusal.
pub const APPROVED_PROPERTIES: Stmt = Stmt::new(
    "ontology.approved_properties",
    r#"SELECT VALUE name FROM ontology_property WHERE vault = $vault AND kind = $kind AND status = "approved" ORDER BY name"#,
);

/// A property proposal, as for a kind: a decided row is returned as it is.
pub const PROPOSE_PROPERTY: Stmt = Stmt::at(
    "ontology.propose_property",
    r#"BEGIN TRANSACTION;
        LET $cur = (SELECT VALUE status FROM $id);
        LET $row = IF array::len($cur) = 0 {
            (CREATE $id SET vault = $vault, kind = $kind, name = $name, datatype = $datatype, unit = $unit,
                enum_values = $enum_values, description = $description, synonyms = $synonyms, merge_into = $merge_into,
                origin = $origin, reason = $reason RETURN AFTER)
        } ELSE IF $cur[0] = "proposed" {
            (UPDATE $id SET datatype = $datatype, unit = $unit ?? unit, enum_values = $enum_values ?? enum_values,
                description = $description ?? description, synonyms = array::union(synonyms ?? [], $synonyms),
                merge_into = $merge_into ?? merge_into, reason = $reason ?? reason, updated_at = time::now() RETURN AFTER)
        } ELSE {
            (SELECT * FROM $id)
        };
        RETURN $row;
        COMMIT TRANSACTION;"#,
    3,
);

/// A person's approval or rejection of a property. Properties rejected into this one follow it to its
/// new target, as kinds do.
pub const DECIDE_PROPERTY: Stmt = Stmt::new(
    "ontology.decide_property",
    r#"BEGIN TRANSACTION;
        UPDATE $id SET status = $status, merge_into = $merge_into, decided_by = $user, updated_at = time::now();
        IF $status = "rejected" {
            UPDATE ontology_property SET merge_into = $merge_into, updated_at = time::now()
                WHERE vault = $vault AND kind = $kind AND status = "rejected" AND merge_into = $name
        };
        COMMIT TRANSACTION;"#,
);

pub const DESCRIBE_PROPERTY: Stmt =
    Stmt::new("ontology.describe_property", "UPDATE $id SET description = $description, updated_at = time::now()");

/// One value of one property for one entity.
pub const SET_VALUE: Stmt = Stmt::new(
    "ontology.set_value",
    "CREATE entity_property SET vault = $vault, entity = $entity, property = $property, value_num = $value_num, \
     value_text = $value_text, value_bool = $value_bool, value_date = $value_date, unit = $unit, as_of = $as_of, \
     source = $source, origin = $origin RETURN AFTER",
);

/// The approved properties of `$vault` (slot 0) and every value of the entities in `$ids` there,
/// newest first (slot 1).
pub const VALUES_OF: Stmt = Stmt::new(
    "ontology.values_of",
    r#"SELECT id, name, datatype, unit FROM ontology_property WHERE vault = $vault AND status = "approved";
     SELECT entity, property, value_num, value_text, value_bool, value_date, unit, as_of, source, created_at
        FROM entity_property WHERE entity IN $ids AND vault = $vault ORDER BY as_of DESC, created_at DESC;"#,
);

/// A deleted entity's values go with it.
pub const DELETE_VALUES_OF: Stmt = Stmt::new("ontology.delete_values_of", "DELETE entity_property WHERE entity = $id");

/// A merged-away entity's values move to the entity it was merged into.
pub const MOVE_VALUES: Stmt = Stmt::new("ontology.move_values", "UPDATE entity_property SET entity = $winner WHERE entity = $loser");

/// The metric rows of `$vault`, by kind and name.
pub const METRICS: Stmt = Stmt::new("ontology.metrics", "SELECT * FROM ontology_metric WHERE vault = $vault ORDER BY kind, name");

/// One metric, by its derived id.
pub const METRIC: Stmt = Stmt::new("ontology.metric", "SELECT * FROM $id");

/// The metrics named `$name` in `$vault`, on any kind, for `metric_eval` without a kind.
pub const METRICS_NAMED: Stmt =
    Stmt::new("ontology.metrics_named", "SELECT * FROM ontology_metric WHERE vault = $vault AND name = $name ORDER BY kind");

/// The approved metric names of `$kind` in `$vault`, to name in a refusal.
pub const APPROVED_METRICS: Stmt = Stmt::new(
    "ontology.approved_metrics",
    r#"SELECT VALUE name FROM ontology_metric WHERE vault = $vault AND kind = $kind AND status = "approved" ORDER BY name"#,
);

/// The property rows of `$kind` in `$vault`, to check a metric's expression against.
pub const PROPERTIES_OF_KIND: Stmt =
    Stmt::new("ontology.properties_of_kind", "SELECT * FROM ontology_property WHERE vault = $vault AND kind = $kind ORDER BY name");

/// A metric proposal, as for a property: a decided row is returned as it is.
pub const PROPOSE_METRIC: Stmt = Stmt::at(
    "ontology.propose_metric",
    r#"BEGIN TRANSACTION;
        LET $cur = (SELECT VALUE status FROM $id);
        LET $row = IF array::len($cur) = 0 {
            (CREATE $id SET vault = $vault, kind = $kind, name = $name, description = $description, synonyms = $synonyms,
                expr = $expr, unit = $unit, merge_into = $merge_into, origin = $origin, reason = $reason RETURN AFTER)
        } ELSE IF $cur[0] = "proposed" {
            (UPDATE $id SET expr = $expr, unit = $unit ?? unit, description = $description ?? description,
                synonyms = array::union(synonyms ?? [], $synonyms), merge_into = $merge_into ?? merge_into,
                reason = $reason ?? reason, updated_at = time::now() RETURN AFTER)
        } ELSE {
            (SELECT * FROM $id)
        };
        RETURN $row;
        COMMIT TRANSACTION;"#,
    3,
);

/// A person's approval or rejection of a metric. Metrics rejected into this one follow it to its new
/// target, as properties do.
pub const DECIDE_METRIC: Stmt = Stmt::new(
    "ontology.decide_metric",
    r#"BEGIN TRANSACTION;
        UPDATE $id SET status = $status, merge_into = $merge_into, decided_by = $user, updated_at = time::now();
        IF $status = "rejected" {
            UPDATE ontology_metric SET merge_into = $merge_into, updated_at = time::now()
                WHERE vault = $vault AND kind = $kind AND status = "rejected" AND merge_into = $name
        };
        COMMIT TRANSACTION;"#,
);

pub const DESCRIBE_METRIC: Stmt =
    Stmt::new("ontology.describe_metric", "UPDATE $id SET description = $description, updated_at = time::now()");

/// What one metric evaluation reads, whatever the metric: the entities of the kind in `$vault` (slot
/// 1; `$table` picks the kind's table, `$kind` the custom kind) and every value of the properties in
/// `$props` there that was true by `$as_of`, newest first (slot 2), so the first row of each (entity,
/// property) is its value as of then. Fixed text: every input is a parameter.
// ponytail: reads every value of the metric's properties in the vault and filters in Rust; fine
// until a vault holds hundreds of thousands of values of one property.
pub const METRIC_INPUTS: Stmt = Stmt::new(
    "ontology.metric_inputs",
    r#"LET $ids = IF $table = "person" { (SELECT VALUE id FROM person WHERE vault = $vault) }
        ELSE IF $table = "organisation" { (SELECT VALUE id FROM organisation WHERE vault = $vault) }
        ELSE IF $table = "location" { (SELECT VALUE id FROM location WHERE vault = $vault) }
        ELSE IF $table = "repository" { (SELECT VALUE id FROM repository WHERE vault = $vault) }
        ELSE IF $table = "file" { (SELECT VALUE id FROM file WHERE vault = $vault) }
        ELSE IF $table = "symbol" { (SELECT VALUE id FROM symbol WHERE vault = $vault) }
        ELSE { (SELECT VALUE id FROM entity WHERE vault = $vault AND kind = $kind) };
     RETURN $ids;
     SELECT entity, property, value_num, value_text, value_bool, value_date, as_of, created_at FROM entity_property
        WHERE property IN $props AND vault = $vault AND as_of <= $as_of ORDER BY as_of DESC, created_at DESC;"#,
);

pub const ALL: &[&Stmt] = &[
    &PROFILE,
    &REGISTRY,
    &KINDS_IN,
    &GROUND,
    &APPROVED_KINDS,
    &TARGET_STATE,
    &PROPOSE_KIND,
    &PROPOSE_LINK,
    &DECIDE_KIND,
    &DESCRIBE_KIND,
    &DECIDE_LINK,
    &DESCRIBE_LINK,
    &GROUND_LINK,
    &APPROVED_LINKS,
    &PROPERTIES,
    &RECENT_FACTS,
    &PROPERTY,
    &APPROVED_PROPERTIES,
    &PROPOSE_PROPERTY,
    &DECIDE_PROPERTY,
    &DESCRIBE_PROPERTY,
    &SET_VALUE,
    &VALUES_OF,
    &DELETE_VALUES_OF,
    &MOVE_VALUES,
    &METRICS,
    &METRIC,
    &METRICS_NAMED,
    &APPROVED_METRICS,
    &PROPERTIES_OF_KIND,
    &PROPOSE_METRIC,
    &DECIDE_METRIC,
    &DESCRIBE_METRIC,
    &METRIC_INPUTS,
];
