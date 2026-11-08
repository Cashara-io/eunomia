-- Link types learn their column (docs/concepts.md#the-project-ontology): the record fields and CSV
-- columns a link type was mapped from, as a property's source_map, so an import finds or creates the
-- target entity a cell names and writes the relation. Old rows get an empty list, so later updates
-- never meet NONE.
DEFINE FIELD OVERWRITE source_map ON ontology_link TYPE array<object> DEFAULT [];
DEFINE FIELD OVERWRITE source_map[*].source ON ontology_link TYPE string;
DEFINE FIELD OVERWRITE source_map[*].record_type ON ontology_link TYPE string;
DEFINE FIELD OVERWRITE source_map[*].path ON ontology_link TYPE string;
UPDATE ontology_link SET source_map = [] WHERE source_map IS NONE;
