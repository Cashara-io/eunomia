//! Statements for the project ontology (`ontology`, migrations 0033 to 0036). See the module docs in store/mod.rs.

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

/// The link rows of `$vault` with a label in `$labels`: the link types a neighbourhood shows, without
/// the vault-wide [`PROFILE`].
pub const LINKS_LABELLED: Stmt =
    Stmt::new("ontology.links_labelled", "SELECT * FROM ontology_link WHERE vault = $vault AND label IN $labels");

/// Whether `$vault` takes only approved kinds and links (NONE before migration 0037, or never set).
pub const STRICT: Stmt = Stmt::new("ontology.strict", "SELECT VALUE ontology_strict FROM $vault");

pub const SET_STRICT: Stmt = Stmt::new("ontology.set_strict", "UPDATE $vault SET ontology_strict = $on RETURN NONE");

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

// The columns (`source_map`, migration 0038) are added in their own step, and only when given, so an
// agent's proposal still writes on a database one migration behind during a rolling upgrade.
pub const PROPOSE_LINK: Stmt = Stmt::at(
    "ontology.propose_link",
    r#"BEGIN TRANSACTION;
        LET $cur = (SELECT VALUE status FROM $id);
        IF array::len($cur) = 0 {
            CREATE $id SET vault = $vault, from_kind = $from_kind, label = $label, to_kind = $to_kind,
                description = $description, origin = $origin, reason = $reason
        } ELSE IF $cur[0] = "proposed" {
            UPDATE $id SET description = $description ?? description, reason = $reason ?? reason, updated_at = time::now()
        };
        IF array::len($source_map) > 0 AND (array::len($cur) = 0 OR $cur[0] = "proposed") {
            UPDATE $id SET source_map = array::union(source_map ?? [], $source_map)
        };
        RETURN (SELECT * FROM $id);
        COMMIT TRANSACTION;"#,
    4,
);

/// The open proposals agents filed in `$vault`, of every sort.
macro_rules! open_agent_proposals {
    () => {
        r#"(array::len(SELECT VALUE id FROM ontology_kind WHERE vault = $vault AND status = "proposed" AND origin = "agent")
            + array::len(SELECT VALUE id FROM ontology_link WHERE vault = $vault AND status = "proposed" AND origin = "agent")
            + array::len(SELECT VALUE id FROM ontology_property WHERE vault = $vault AND status = "proposed" AND origin = "agent")
            + array::len(SELECT VALUE id FROM ontology_metric WHERE vault = $vault AND status = "proposed" AND origin = "agent"))"#
    };
}

/// A strict-mode refusal's kind, filed as an agent's proposal unless the vault already has `$cap` open
/// agent proposals. Only creates the row, so an existing proposal keeps its reason. Returns whether the
/// kind is on file.
pub const FILE_KIND: Stmt = Stmt::at(
    "ontology.file_kind",
    concat!(
        r#"BEGIN TRANSACTION;
        LET $new = array::len(SELECT VALUE id FROM $id) = 0;
        LET $file = $new AND "#,
        open_agent_proposals!(),
        r#" < $cap;
        IF $file {
            CREATE $id SET vault = $vault, kind = $kind, origin = "agent", reason = $reason RETURN NONE
        };
        RETURN !$new OR $file;
        COMMIT TRANSACTION;"#
    ),
    4, // BEGIN 0, two LETs 1 and 2, IF 3, RETURN 4
);

/// [`FILE_KIND`] for a link type.
pub const FILE_LINK: Stmt = Stmt::at(
    "ontology.file_link",
    concat!(
        r#"BEGIN TRANSACTION;
        LET $new = array::len(SELECT VALUE id FROM $id) = 0;
        LET $file = $new AND "#,
        open_agent_proposals!(),
        r#" < $cap;
        IF $file {
            CREATE $id SET vault = $vault, from_kind = $from_kind, label = $label, to_kind = $to_kind, origin = "agent", reason = $reason RETURN NONE
        };
        RETURN !$new OR $file;
        COMMIT TRANSACTION;"#
    ),
    4,
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

/// The approved link types from `$kind` in `$vault`: a CSV import writes those a column feeds.
pub const APPROVED_LINKS_FROM: Stmt = Stmt::new(
    "ontology.approved_links_from",
    r#"SELECT * FROM ontology_link WHERE vault = $vault AND from_kind = $kind AND status = "approved" ORDER BY label, to_kind"#,
);

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
                origin = $origin, reason = $reason, source_map = $source_map RETURN AFTER)
        } ELSE IF $cur[0] = "proposed" {
            (UPDATE $id SET datatype = $datatype, unit = $unit ?? unit, enum_values = $enum_values ?? enum_values,
                description = $description ?? description, synonyms = array::union(synonyms ?? [], $synonyms),
                merge_into = $merge_into ?? merge_into, reason = $reason ?? reason,
                source_map = array::union(source_map ?? [], $source_map), updated_at = time::now() RETURN AFTER)
        } ELSE {
            (UPDATE $id SET source_map = array::union(source_map ?? [], $source_map) RETURN AFTER)
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
/// 1; `$table` picks the kind's table, `$kind` the custom kind) and, per entity and property in
/// `$props`, its latest value that was true by `$as_of` (slot 2): one indexed lookup each on
/// (entity, property, as_of), so a long history is never read. Fixed text: every input is a parameter.
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
     RETURN array::flatten(array::flatten($ids.map(|$e| $props.map(|$p| (
        SELECT entity, property, value_num, value_text, value_bool, value_date, as_of, created_at FROM entity_property
        WHERE entity = $e AND property = $p AND vault = $vault AND as_of <= $as_of ORDER BY as_of DESC, created_at DESC LIMIT 1
     )))));"#,
);

/// The approved terms of `$vault` that a question's phrases are matched against, each with the spec and
/// text of its stored vector: kinds (slot 0), properties (slot 1), metrics (slot 2), link types (slot 3).
pub const COVERAGE_TERMS: Stmt = Stmt::new(
    "ontology.coverage_terms",
    r#"SELECT id, kind AS name, NONE AS of_kind, description, synonyms, embedding_spec, embedding_text FROM ontology_kind
        WHERE vault = $vault AND status = "approved";
     SELECT id, name, kind AS of_kind, description, synonyms, embedding_spec, embedding_text FROM ontology_property
        WHERE vault = $vault AND status = "approved";
     SELECT id, name, kind AS of_kind, description, synonyms, embedding_spec, embedding_text FROM ontology_metric
        WHERE vault = $vault AND status = "approved";
     SELECT id, label AS name, string::concat(from_kind, " to ", to_kind) AS of_kind, description, [] AS synonyms,
        embedding_spec, embedding_text FROM ontology_link WHERE vault = $vault AND status = "approved";"#,
);

/// A term's vector, the provider and model it was made with, and the text it was made from.
pub const SET_TERM_EMBEDDING: Stmt =
    Stmt::new("ontology.set_term_embedding", "UPDATE $id SET embedding = $embedding, embedding_spec = $spec, embedding_text = $text RETURN NONE");

/// The approved terms of `$vault` nearest `$vector` (same `$spec`), by the HNSW indexes, with their
/// cosine distance. K is 20, not 3, so terms of the org's other vaults cannot crowd this vault's out
/// even if the vault condition is applied after the search rather than inside it.
pub const COVERAGE_NEAREST: Stmt = Stmt::new(
    "ontology.coverage_nearest",
    r#"SELECT id, vector::distance::knn() AS distance FROM ontology_kind
        WHERE embedding <|20,40|> $vector AND vault = $vault AND status = "approved" AND embedding_spec = $spec ORDER BY distance;
     SELECT id, vector::distance::knn() AS distance FROM ontology_property
        WHERE embedding <|20,40|> $vector AND vault = $vault AND status = "approved" AND embedding_spec = $spec ORDER BY distance;
     SELECT id, vector::distance::knn() AS distance FROM ontology_metric
        WHERE embedding <|20,40|> $vector AND vault = $vault AND status = "approved" AND embedding_spec = $spec ORDER BY distance;
     SELECT id, vector::distance::knn() AS distance FROM ontology_link
        WHERE embedding <|20,40|> $vector AND vault = $vault AND status = "approved" AND embedding_spec = $spec ORDER BY distance;"#,
);

/// [`COVERAGE_NEAREST`] by an exact scan of the vault's approved terms, for while an index is still
/// building (a kNN query then finds nothing).
pub const COVERAGE_NEAREST_EXACT: Stmt = Stmt::new(
    "ontology.coverage_nearest_exact",
    r#"SELECT id, 1 - vector::similarity::cosine(embedding, $vector) AS distance FROM ontology_kind
        WHERE vault = $vault AND status = "approved" AND embedding_spec = $spec ORDER BY distance LIMIT 3;
     SELECT id, 1 - vector::similarity::cosine(embedding, $vector) AS distance FROM ontology_property
        WHERE vault = $vault AND status = "approved" AND embedding_spec = $spec ORDER BY distance LIMIT 3;
     SELECT id, 1 - vector::similarity::cosine(embedding, $vector) AS distance FROM ontology_metric
        WHERE vault = $vault AND status = "approved" AND embedding_spec = $spec ORDER BY distance LIMIT 3;
     SELECT id, 1 - vector::similarity::cosine(embedding, $vector) AS distance FROM ontology_link
        WHERE vault = $vault AND status = "approved" AND embedding_spec = $spec ORDER BY distance LIMIT 3;"#,
);

/// Every record type of `$vault` with its record count (personal vault: also the owner's records filed
/// in none), uploaded document chunks left out. Auto-mapping profiles each.
pub const RECORD_TYPES: Stmt = Stmt::new(
    "ontology.record_types",
    r#"SELECT source, type, count() AS count FROM cache_record
        WHERE (vault IN $vaults OR ($own AND vault = NONE AND owner = $owner)) AND deleted = false AND source != "documents"
        GROUP BY source, type ORDER BY source, type LIMIT 100"#,
);

/// The newest `$limit` payloads of one record type in `$vault` (as for [`RECORD_TYPES`]).
pub const RECORD_SAMPLE: Stmt = Stmt::new(
    "ontology.record_sample",
    r#"SELECT external_id, payload, updated_at FROM cache_record
        WHERE (vault IN $vaults OR ($own AND vault = NONE AND owner = $owner)) AND deleted = false AND source = $source AND type = $type
        ORDER BY updated_at DESC LIMIT $limit"#,
);

/// The records in `$vault` (as for [`RECORD_TYPES`]) whose external id is one of `$values`, with their
/// type: a link's value check, against every record type at once.
// ponytail: no index on external_id, so this scans the vault's records once per checked field; fine
// for an owner's one-off auto-map run.
pub const MATCH_EXTERNAL_IDS: Stmt = Stmt::new(
    "ontology.match_external_ids",
    r#"SELECT source, type, external_id FROM cache_record
        WHERE (vault IN $vaults OR ($own AND vault = NONE AND owner = $owner)) AND deleted = false AND source != "documents"
        AND external_id IN $values"#,
);

/// Which of `$values` (lowercased) are the name or an alias of an entity of one kind in `$vault`
/// (`$table` picks the kind's table, `$kind` the custom kind): a link's value check.
/// The entities of every kind in `$vault` whose name key (the lowercase name) is one of `$keys`, with
/// their kind: what a link column's values already name, whatever kind made them.
pub const NAMED_ANY_KIND: Stmt = Stmt::new(
    "ontology.named_any_kind",
    r#"RETURN array::flatten([
        (SELECT id, name, name_key, "person" AS kind FROM person WHERE vault = $vault AND name_key IN $keys),
        (SELECT id, name, name_key, "organisation" AS kind FROM organisation WHERE vault = $vault AND name_key IN $keys),
        (SELECT id, name, name_key, "location" AS kind FROM location WHERE vault = $vault AND name_key IN $keys),
        (SELECT id, name, name_key, "repository" AS kind FROM repository WHERE vault = $vault AND name_key IN $keys),
        (SELECT id, name, name_key, "file" AS kind FROM file WHERE vault = $vault AND name_key IN $keys),
        (SELECT id, name, name_key, "symbol" AS kind FROM symbol WHERE vault = $vault AND name_key IN $keys),
        (SELECT id, name, name_key, kind FROM entity WHERE vault = $vault AND name_key IN $keys)
    ]);"#,
);

pub const MATCH_NAMES: Stmt = Stmt::new(
    "ontology.match_names",
    r#"LET $rows = IF $table = "person" { (SELECT name, aliases FROM person WHERE vault = $vault) }
        ELSE IF $table = "organisation" { (SELECT name, aliases FROM organisation WHERE vault = $vault) }
        ELSE IF $table = "location" { (SELECT name, aliases FROM location WHERE vault = $vault) }
        ELSE IF $table = "repository" { (SELECT name, aliases FROM repository WHERE vault = $vault) }
        ELSE IF $table = "file" { (SELECT name, aliases FROM file WHERE vault = $vault) }
        ELSE IF $table = "symbol" { (SELECT name, aliases FROM symbol WHERE vault = $vault) }
        ELSE { (SELECT name, aliases FROM entity WHERE vault = $vault AND kind = $kind) };
     RETURN array::distinct(array::flatten($rows.map(|$r| array::concat([$r.name], $r.aliases ?? []))).map(|$n| string::lowercase($n)))
        .filter(|$n| $n IN $values);"#,
);

/// The approved custom kinds of `$vault`, newest decision first, at most `$limit`: what extraction
/// may write besides people, organisations and locations.
pub const EXTRACTION_KINDS: Stmt = Stmt::new(
    "ontology.extraction_kinds",
    r#"SELECT kind, description FROM ontology_kind WHERE vault = $vault AND status = "approved" AND kind NOT IN $built_in
        ORDER BY updated_at DESC LIMIT $limit"#,
);

/// One kind's row by its derived id: an import checks the kind is approved.
pub const KIND: Stmt = Stmt::new("ontology.kind", "SELECT kind, status FROM $id");

/// A batch of imported values, each keyed by (entity, property, document) in `$values[*].id`: a re-run
/// of the import updates them in place instead of adding history.
pub const UPSERT_IMPORTED_VALUES: Stmt = Stmt::new(
    "ontology.upsert_imported_values",
    "FOR $v IN $values { \
        UPSERT $v.id SET vault = $vault, entity = $v.entity, property = $v.property, value_num = $v.value_num, \
        value_text = $v.value_text, value_bool = $v.value_bool, value_date = $v.value_date, unit = NONE, as_of = $as_of, \
        source = $source, origin = $origin RETURN NONE; \
     };",
);

/// The values a deleted document's import wrote go with it.
pub const DELETE_IMPORTED_VALUES: Stmt = Stmt::new("ontology.delete_imported_values", "DELETE entity_property WHERE source = $document");

pub const ALL: &[&Stmt] = &[
    &PROFILE,
    &REGISTRY,
    &LINKS_LABELLED,
    &STRICT,
    &SET_STRICT,
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
    &APPROVED_LINKS_FROM,
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
    &COVERAGE_TERMS,
    &SET_TERM_EMBEDDING,
    &COVERAGE_NEAREST,
    &COVERAGE_NEAREST_EXACT,
    &RECORD_TYPES,
    &RECORD_SAMPLE,
    &MATCH_EXTERNAL_IDS,
    &MATCH_NAMES,
    &NAMED_ANY_KIND,
    &EXTRACTION_KINDS,
    &KIND,
    &UPSERT_IMPORTED_VALUES,
    &DELETE_IMPORTED_VALUES,
];
