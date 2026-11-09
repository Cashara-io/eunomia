-- Expand-only. Existing rows start at revision 1; observed-only definitions use revision 0.
DEFINE TABLE IF NOT EXISTS ontology_event SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS vault ON ontology_event TYPE record<vault>;
DEFINE FIELD IF NOT EXISTS seq ON ontology_event TYPE int;
DEFINE FIELD IF NOT EXISTS at ON ontology_event TYPE datetime DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS actor ON ontology_event TYPE string;
DEFINE FIELD IF NOT EXISTS actor_type ON ontology_event TYPE string;
DEFINE FIELD IF NOT EXISTS actor_name ON ontology_event TYPE string;
DEFINE FIELD IF NOT EXISTS action ON ontology_event TYPE string;
DEFINE FIELD IF NOT EXISTS definition_type ON ontology_event TYPE string;
DEFINE FIELD IF NOT EXISTS kind ON ontology_event TYPE string;
DEFINE FIELD IF NOT EXISTS name ON ontology_event TYPE string;
DEFINE FIELD IF NOT EXISTS target ON ontology_event TYPE string;
DEFINE FIELD IF NOT EXISTS revision ON ontology_event TYPE int;
DEFINE FIELD IF NOT EXISTS reason ON ontology_event TYPE option<string>;
DEFINE FIELD IF NOT EXISTS before ON ontology_event TYPE option<object> FLEXIBLE;
DEFINE FIELD IF NOT EXISTS after ON ontology_event TYPE option<object> FLEXIBLE;
DEFINE INDEX IF NOT EXISTS ontology_event_cursor ON ontology_event FIELDS vault, seq UNIQUE;
DEFINE FIELD IF NOT EXISTS ontology_seq ON vault TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS ontology_actor_name ON vault TYPE option<string>;
DEFINE FIELD IF NOT EXISTS ontology_actor ON vault TYPE option<string>;
DEFINE FIELD IF NOT EXISTS ontology_actor_type ON vault TYPE option<string>;

DEFINE FIELD IF NOT EXISTS revision ON ontology_kind TYPE int DEFAULT 1;
DEFINE FIELD IF NOT EXISTS activity_actor_name ON ontology_kind TYPE option<string>;
DEFINE FIELD IF NOT EXISTS activity_actor ON ontology_kind TYPE option<string>;
DEFINE FIELD IF NOT EXISTS activity_actor_type ON ontology_kind TYPE option<string>;
DEFINE FIELD IF NOT EXISTS activity_action ON ontology_kind TYPE option<string>;
DEFINE FIELD IF NOT EXISTS activity_reason ON ontology_kind TYPE option<string>;
UPDATE ontology_kind SET revision = revision ?? 1;
DEFINE EVENT IF NOT EXISTS ontology_activity ON ontology_kind
WHEN $event != "DELETE" AND $after.revision > 0 AND $after.revision != $before.revision THEN {
    LET $v = (UPDATE $after.vault SET ontology_seq = (ontology_seq ?? 0) + 1 RETURN AFTER)[0];
    CREATE ontology_event SET vault = $after.vault, seq = $v.ontology_seq,
        actor_name = $after.activity_actor_name ?? "System", actor = $after.activity_actor ?? "system", actor_type = $after.activity_actor_type ?? "system",
        action = IF $after.activity_action = "decided" {
            IF $after.status = "approved" { "approved" } ELSE IF $after.merge_into != NONE { "merged" } ELSE { "rejected" }
        } ELSE { $after.activity_action ?? "proposed" },
        definition_type = "kind", kind = $after.kind ?? $after.from_kind,
        name = $after.name ?? $after.label ?? $after.kind, target = <string> $after.id,
        revision = $after.revision, reason = $after.activity_reason,
        before = { description: $before.description, status: $before.status, merge_into: $before.merge_into, revision: $before.revision, synonyms: $before.synonyms, expr: $before.expr, datatype: $before.datatype, unit: $before.unit, enum_values: $before.enum_values, source_map: $before.source_map, reason: $before.reason },
        after = { description: $after.description, status: $after.status, merge_into: $after.merge_into, revision: $after.revision, synonyms: $after.synonyms, expr: $after.expr, datatype: $after.datatype, unit: $after.unit, enum_values: $after.enum_values, source_map: $after.source_map, reason: $after.reason };
};

DEFINE FIELD IF NOT EXISTS revision ON ontology_link TYPE int DEFAULT 1;
DEFINE FIELD IF NOT EXISTS activity_actor_name ON ontology_link TYPE option<string>;
DEFINE FIELD IF NOT EXISTS activity_actor ON ontology_link TYPE option<string>;
DEFINE FIELD IF NOT EXISTS activity_actor_type ON ontology_link TYPE option<string>;
DEFINE FIELD IF NOT EXISTS activity_action ON ontology_link TYPE option<string>;
DEFINE FIELD IF NOT EXISTS activity_reason ON ontology_link TYPE option<string>;
UPDATE ontology_link SET revision = revision ?? 1;
DEFINE EVENT IF NOT EXISTS ontology_activity ON ontology_link
WHEN $event != "DELETE" AND $after.revision > 0 AND $after.revision != $before.revision THEN {
    LET $v = (UPDATE $after.vault SET ontology_seq = (ontology_seq ?? 0) + 1 RETURN AFTER)[0];
    CREATE ontology_event SET vault = $after.vault, seq = $v.ontology_seq,
        actor_name = $after.activity_actor_name ?? "System", actor = $after.activity_actor ?? "system", actor_type = $after.activity_actor_type ?? "system",
        action = IF $after.activity_action = "decided" {
            IF $after.status = "approved" { "approved" } ELSE IF $after.merge_into != NONE { "merged" } ELSE { "rejected" }
        } ELSE { $after.activity_action ?? "proposed" },
        definition_type = "link", kind = $after.from_kind,
        name = string::concat($after.from_kind, " ", $after.label, " ", $after.to_kind), target = <string> $after.id,
        revision = $after.revision, reason = $after.activity_reason,
        before = { description: $before.description, status: $before.status, merge_into: $before.merge_into, revision: $before.revision, synonyms: $before.synonyms, expr: $before.expr, datatype: $before.datatype, unit: $before.unit, enum_values: $before.enum_values, source_map: $before.source_map, reason: $before.reason },
        after = { description: $after.description, status: $after.status, merge_into: $after.merge_into, revision: $after.revision, synonyms: $after.synonyms, expr: $after.expr, datatype: $after.datatype, unit: $after.unit, enum_values: $after.enum_values, source_map: $after.source_map, reason: $after.reason };
};

DEFINE FIELD IF NOT EXISTS revision ON ontology_property TYPE int DEFAULT 1;
DEFINE FIELD IF NOT EXISTS activity_actor_name ON ontology_property TYPE option<string>;
DEFINE FIELD IF NOT EXISTS activity_actor ON ontology_property TYPE option<string>;
DEFINE FIELD IF NOT EXISTS activity_actor_type ON ontology_property TYPE option<string>;
DEFINE FIELD IF NOT EXISTS activity_action ON ontology_property TYPE option<string>;
DEFINE FIELD IF NOT EXISTS activity_reason ON ontology_property TYPE option<string>;
UPDATE ontology_property SET revision = revision ?? 1;
DEFINE EVENT IF NOT EXISTS ontology_activity ON ontology_property
WHEN $event != "DELETE" AND $after.revision > 0 AND $after.revision != $before.revision THEN {
    LET $v = (UPDATE $after.vault SET ontology_seq = (ontology_seq ?? 0) + 1 RETURN AFTER)[0];
    CREATE ontology_event SET vault = $after.vault, seq = $v.ontology_seq,
        actor_name = $after.activity_actor_name ?? "System", actor = $after.activity_actor ?? "system", actor_type = $after.activity_actor_type ?? "system",
        action = IF $after.activity_action = "decided" {
            IF $after.status = "approved" { "approved" } ELSE IF $after.merge_into != NONE { "merged" } ELSE { "rejected" }
        } ELSE { $after.activity_action ?? "proposed" },
        definition_type = "property", kind = $after.kind ?? $after.from_kind,
        name = $after.name ?? $after.label ?? $after.kind, target = <string> $after.id,
        revision = $after.revision, reason = $after.activity_reason,
        before = { description: $before.description, status: $before.status, merge_into: $before.merge_into, revision: $before.revision, synonyms: $before.synonyms, expr: $before.expr, datatype: $before.datatype, unit: $before.unit, enum_values: $before.enum_values, source_map: $before.source_map, reason: $before.reason },
        after = { description: $after.description, status: $after.status, merge_into: $after.merge_into, revision: $after.revision, synonyms: $after.synonyms, expr: $after.expr, datatype: $after.datatype, unit: $after.unit, enum_values: $after.enum_values, source_map: $after.source_map, reason: $after.reason };
};

DEFINE FIELD IF NOT EXISTS revision ON ontology_metric TYPE int DEFAULT 1;
DEFINE FIELD IF NOT EXISTS activity_actor_name ON ontology_metric TYPE option<string>;
DEFINE FIELD IF NOT EXISTS activity_actor ON ontology_metric TYPE option<string>;
DEFINE FIELD IF NOT EXISTS activity_actor_type ON ontology_metric TYPE option<string>;
DEFINE FIELD IF NOT EXISTS activity_action ON ontology_metric TYPE option<string>;
DEFINE FIELD IF NOT EXISTS activity_reason ON ontology_metric TYPE option<string>;
UPDATE ontology_metric SET revision = revision ?? 1;
DEFINE EVENT IF NOT EXISTS ontology_activity ON ontology_metric
WHEN $event != "DELETE" AND $after.revision > 0 AND $after.revision != $before.revision THEN {
    LET $v = (UPDATE $after.vault SET ontology_seq = (ontology_seq ?? 0) + 1 RETURN AFTER)[0];
    CREATE ontology_event SET vault = $after.vault, seq = $v.ontology_seq,
        actor_name = $after.activity_actor_name ?? "System", actor = $after.activity_actor ?? "system", actor_type = $after.activity_actor_type ?? "system",
        action = IF $after.activity_action = "decided" {
            IF $after.status = "approved" { "approved" } ELSE IF $after.merge_into != NONE { "merged" } ELSE { "rejected" }
        } ELSE { $after.activity_action ?? "proposed" },
        definition_type = "metric", kind = $after.kind ?? $after.from_kind,
        name = $after.name ?? $after.label ?? $after.kind, target = <string> $after.id,
        revision = $after.revision, reason = $after.activity_reason,
        before = { description: $before.description, status: $before.status, merge_into: $before.merge_into, revision: $before.revision, synonyms: $before.synonyms, expr: $before.expr, datatype: $before.datatype, unit: $before.unit, enum_values: $before.enum_values, source_map: $before.source_map, reason: $before.reason },
        after = { description: $after.description, status: $after.status, merge_into: $after.merge_into, revision: $after.revision, synonyms: $after.synonyms, expr: $after.expr, datatype: $after.datatype, unit: $after.unit, enum_values: $after.enum_values, source_map: $after.source_map, reason: $after.reason };
};

DEFINE EVENT IF NOT EXISTS ontology_strict_activity ON vault
WHEN $event = "UPDATE" AND $after.ontology_strict != $before.ontology_strict THEN {
    LET $v = (UPDATE $after.id SET ontology_seq = (ontology_seq ?? 0) + 1 RETURN AFTER)[0];
    CREATE ontology_event SET vault = $after.id, seq = $v.ontology_seq,
        actor_name = $after.ontology_actor_name ?? "System", actor = $after.ontology_actor ?? "system", actor_type = $after.ontology_actor_type ?? "system",
        action = "strict_changed", definition_type = "policy", kind = "", name = "Only approved kinds and links",
        target = <string> $after.id, revision = 0,
        before = { strict: $before.ontology_strict ?? false }, after = { strict: $after.ontology_strict };
};
