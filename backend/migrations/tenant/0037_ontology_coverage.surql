-- Coverage and strict mode (docs/concepts.md#the-project-ontology).
-- Each reviewed kind, link type, property and metric keeps a vector of its name, synonyms and
-- description, so `ontology_coverage` can match a question's phrases to the approved ones. The vector is
-- made when an owner approves or describes the row, and lazily for older rows; `embedding_text` is the
-- text it was made from (a changed description makes a new one), `embedding_spec` the provider and model.
-- The HNSW indexes have the same shape as the memory, skill and record indexes (0022).
DEFINE FIELD OVERWRITE embedding ON ontology_kind TYPE option<array<float>>;
DEFINE FIELD OVERWRITE embedding_spec ON ontology_kind TYPE option<string>;
DEFINE FIELD OVERWRITE embedding_text ON ontology_kind TYPE option<string>;
DEFINE FIELD OVERWRITE embedding ON ontology_link TYPE option<array<float>>;
DEFINE FIELD OVERWRITE embedding_spec ON ontology_link TYPE option<string>;
DEFINE FIELD OVERWRITE embedding_text ON ontology_link TYPE option<string>;
DEFINE FIELD OVERWRITE embedding ON ontology_property TYPE option<array<float>>;
DEFINE FIELD OVERWRITE embedding_spec ON ontology_property TYPE option<string>;
DEFINE FIELD OVERWRITE embedding_text ON ontology_property TYPE option<string>;
DEFINE FIELD OVERWRITE embedding ON ontology_metric TYPE option<array<float>>;
DEFINE FIELD OVERWRITE embedding_spec ON ontology_metric TYPE option<string>;
DEFINE FIELD OVERWRITE embedding_text ON ontology_metric TYPE option<string>;
DEFINE INDEX OVERWRITE ontology_kind_embedding_idx ON ontology_kind FIELDS embedding HNSW DIMENSION 1536 DIST COSINE TYPE F32 CONCURRENTLY;
DEFINE INDEX OVERWRITE ontology_link_embedding_idx ON ontology_link FIELDS embedding HNSW DIMENSION 1536 DIST COSINE TYPE F32 CONCURRENTLY;
DEFINE INDEX OVERWRITE ontology_property_embedding_idx ON ontology_property FIELDS embedding HNSW DIMENSION 1536 DIST COSINE TYPE F32 CONCURRENTLY;
DEFINE INDEX OVERWRITE ontology_metric_embedding_idx ON ontology_metric FIELDS embedding HNSW DIMENSION 1536 DIST COSINE TYPE F32 CONCURRENTLY;

-- "Only approved kinds and links": when true, a write under a custom kind or a relation under a link
-- type that is not approved is refused and filed as a proposal. NONE (every existing vault) is off.
DEFINE FIELD OVERWRITE ontology_strict ON vault TYPE option<bool>;
