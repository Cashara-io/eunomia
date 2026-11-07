-- Certified metrics on a vault's kinds (docs/concepts.md#the-project-ontology): a named number the
-- server computes from typed property values, such as the average `price` of `product`. A metric is
-- per-vault data, reviewed like a property: only an approved one runs by name, and a rejected one with
-- merge_into runs that metric of the same kind instead. `expr` is a typed aggregate the server checks
-- and compiles itself (ontology/metric.rs), never query text. Ids are derived from (vault, kind, name).

DEFINE TABLE OVERWRITE ontology_metric SCHEMAFULL;
DEFINE FIELD OVERWRITE vault ON ontology_metric TYPE record<vault>;
DEFINE FIELD OVERWRITE kind ON ontology_metric TYPE string ASSERT string::len($value) >= 1 AND string::len($value) <= 32;
DEFINE FIELD OVERWRITE name ON ontology_metric TYPE string ASSERT string::len($value) >= 1 AND string::len($value) <= 64;
DEFINE FIELD OVERWRITE description ON ontology_metric TYPE option<string>;
DEFINE FIELD OVERWRITE synonyms ON ontology_metric TYPE array<string> DEFAULT [];
-- {agg, property?, numerator?, denominator?, filters: [{property, op, value}], time_property?, grain?}
DEFINE FIELD OVERWRITE expr ON ontology_metric TYPE object FLEXIBLE;
DEFINE FIELD OVERWRITE unit ON ontology_metric TYPE option<string>;
DEFINE FIELD OVERWRITE status ON ontology_metric TYPE string DEFAULT "proposed" ASSERT $value IN ["proposed", "approved", "rejected"];
-- rejected: the metric of the same kind that runs instead; proposed: the merge the proposer suggests
DEFINE FIELD OVERWRITE merge_into ON ontology_metric TYPE option<string>;
DEFINE FIELD OVERWRITE origin ON ontology_metric TYPE string ASSERT $value IN ["suggested", "agent", "human"];
DEFINE FIELD OVERWRITE reason ON ontology_metric TYPE option<string>;
DEFINE FIELD OVERWRITE decided_by ON ontology_metric TYPE option<record<user>>;
DEFINE FIELD OVERWRITE created_at ON ontology_metric TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON ontology_metric TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE ontology_metric_vault_key_unique ON ontology_metric FIELDS vault, kind, name UNIQUE;
