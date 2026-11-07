-- Auto-mapping (docs/concepts.md#the-project-ontology): the record fields and CSV columns each property
-- was mapped from, and values imported from an uploaded CSV.
-- source_map: one {source, record_type, path} per field the property holds, so an import knows which
-- column feeds which property. Old rows get an empty list, so later updates never meet NONE.
DEFINE FIELD OVERWRITE source_map ON ontology_property TYPE array<object> DEFAULT [];
DEFINE FIELD OVERWRITE source_map[*].source ON ontology_property TYPE string;
DEFINE FIELD OVERWRITE source_map[*].record_type ON ontology_property TYPE string;
DEFINE FIELD OVERWRITE source_map[*].path ON ontology_property TYPE string;
UPDATE ontology_property SET source_map = [] WHERE source_map IS NONE;

-- A value imported from a CSV names the document as its source (a synced value still names its record).
DEFINE FIELD OVERWRITE source ON entity_property TYPE option<record<cache_record | document>>;
