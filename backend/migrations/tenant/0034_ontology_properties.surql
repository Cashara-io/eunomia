-- Typed properties on a vault's kinds, and their values (docs/concepts.md#the-project-ontology).
-- A property is per-vault data the ontology defines: a name, a datatype and an optional unit on one kind.
-- It is reviewed like a kind: only an approved property takes values, and a rejected one with
-- merge_into sends its values to that property of the same kind. Ids are derived from (vault, kind,
-- name), as for kinds and links. Values are rows, never fields on the entity tables, so each keeps its
-- date, its source and the history before it.

-- Other names written in the vault's data for a kind or a property ("also called", an abbreviation).
DEFINE FIELD OVERWRITE synonyms ON ontology_kind TYPE array<string> DEFAULT [];
UPDATE ontology_kind SET synonyms = [] WHERE synonyms IS NONE;

DEFINE TABLE OVERWRITE ontology_property SCHEMAFULL;
DEFINE FIELD OVERWRITE vault ON ontology_property TYPE record<vault>;
DEFINE FIELD OVERWRITE kind ON ontology_property TYPE string ASSERT string::len($value) >= 1 AND string::len($value) <= 32;
DEFINE FIELD OVERWRITE name ON ontology_property TYPE string ASSERT string::len($value) >= 1 AND string::len($value) <= 64;
DEFINE FIELD OVERWRITE datatype ON ontology_property TYPE string
    ASSERT $value IN ["text", "number", "integer", "money", "percent", "bool", "date", "datetime", "enum"];
DEFINE FIELD OVERWRITE unit ON ontology_property TYPE option<string>;
DEFINE FIELD OVERWRITE enum_values ON ontology_property TYPE option<array<string>>;
DEFINE FIELD OVERWRITE description ON ontology_property TYPE option<string>;
DEFINE FIELD OVERWRITE synonyms ON ontology_property TYPE array<string> DEFAULT [];
DEFINE FIELD OVERWRITE status ON ontology_property TYPE string DEFAULT "proposed" ASSERT $value IN ["proposed", "approved", "rejected"];
-- rejected: the property of the same kind its values go to instead; proposed: the merge the proposer suggests
DEFINE FIELD OVERWRITE merge_into ON ontology_property TYPE option<string>;
DEFINE FIELD OVERWRITE origin ON ontology_property TYPE string ASSERT $value IN ["suggested", "agent", "human"];
DEFINE FIELD OVERWRITE reason ON ontology_property TYPE option<string>;
DEFINE FIELD OVERWRITE decided_by ON ontology_property TYPE option<record<user>>;
DEFINE FIELD OVERWRITE created_at ON ontology_property TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON ontology_property TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE ontology_property_vault_key_unique ON ontology_property FIELDS vault, kind, name UNIQUE;

-- One value of one property for one entity, as of a date. The latest as_of wins on read.
DEFINE TABLE OVERWRITE entity_property SCHEMAFULL;
DEFINE FIELD OVERWRITE vault ON entity_property TYPE record<vault>;
DEFINE FIELD OVERWRITE entity ON entity_property TYPE record<person | organisation | location | repository | file | symbol | entity>;
DEFINE FIELD OVERWRITE property ON entity_property TYPE record<ontology_property>;
DEFINE FIELD OVERWRITE value_num ON entity_property TYPE option<float>;
DEFINE FIELD OVERWRITE value_text ON entity_property TYPE option<string>;
DEFINE FIELD OVERWRITE value_bool ON entity_property TYPE option<bool>;
DEFINE FIELD OVERWRITE value_date ON entity_property TYPE option<datetime>;
DEFINE FIELD OVERWRITE unit ON entity_property TYPE option<string>;
DEFINE FIELD OVERWRITE as_of ON entity_property TYPE datetime;
DEFINE FIELD OVERWRITE source ON entity_property TYPE option<record<cache_record>>;
DEFINE FIELD OVERWRITE origin ON entity_property TYPE string ASSERT $value IN ["suggested", "agent", "human"];
DEFINE FIELD OVERWRITE created_at ON entity_property TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE entity_property_entity_idx ON entity_property FIELDS entity, property, as_of;
DEFINE INDEX OVERWRITE entity_property_property_idx ON entity_property FIELDS property;
