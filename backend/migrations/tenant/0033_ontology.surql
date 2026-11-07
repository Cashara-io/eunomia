-- Project ontology (docs/concepts.md#the-project-ontology): the kinds and link types a vault's owners
-- reviewed. A kind or link with no row here is only observed in the vault's entities and relations.
-- Proposals come from the server's model (`suggested`), an agent (`agent`) or a person (`human`);
-- only a person in the browser approves or rejects one. Ids are derived from the key (see
-- ontology/mod.rs), so two proposers of the same kind meet on one row.

DEFINE TABLE OVERWRITE ontology_kind SCHEMAFULL;
DEFINE FIELD OVERWRITE vault ON ontology_kind TYPE record<vault>;
DEFINE FIELD OVERWRITE kind ON ontology_kind TYPE string ASSERT string::len($value) >= 1 AND string::len($value) <= 32;
DEFINE FIELD OVERWRITE description ON ontology_kind TYPE option<string>;
DEFINE FIELD OVERWRITE status ON ontology_kind TYPE string DEFAULT "proposed" ASSERT $value IN ["proposed", "approved", "rejected"];
-- rejected: where writes under this kind go instead; proposed: the merge the proposer suggests
DEFINE FIELD OVERWRITE merge_into ON ontology_kind TYPE option<string>;
DEFINE FIELD OVERWRITE origin ON ontology_kind TYPE string ASSERT $value IN ["suggested", "agent", "human"];
DEFINE FIELD OVERWRITE reason ON ontology_kind TYPE option<string>;
DEFINE FIELD OVERWRITE decided_by ON ontology_kind TYPE option<record<user>>;
DEFINE FIELD OVERWRITE created_at ON ontology_kind TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON ontology_kind TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE ontology_kind_vault_kind_unique ON ontology_kind FIELDS vault, kind UNIQUE;

DEFINE TABLE OVERWRITE ontology_link SCHEMAFULL;
DEFINE FIELD OVERWRITE vault ON ontology_link TYPE record<vault>;
DEFINE FIELD OVERWRITE from_kind ON ontology_link TYPE string ASSERT string::len($value) >= 1 AND string::len($value) <= 32;
DEFINE FIELD OVERWRITE label ON ontology_link TYPE string ASSERT string::len($value) >= 1 AND string::len($value) <= 64;
DEFINE FIELD OVERWRITE to_kind ON ontology_link TYPE string ASSERT string::len($value) >= 1 AND string::len($value) <= 32;
DEFINE FIELD OVERWRITE description ON ontology_link TYPE option<string>;
DEFINE FIELD OVERWRITE status ON ontology_link TYPE string DEFAULT "proposed" ASSERT $value IN ["proposed", "approved", "rejected"];
DEFINE FIELD OVERWRITE origin ON ontology_link TYPE string ASSERT $value IN ["suggested", "agent", "human"];
DEFINE FIELD OVERWRITE reason ON ontology_link TYPE option<string>;
DEFINE FIELD OVERWRITE decided_by ON ontology_link TYPE option<record<user>>;
DEFINE FIELD OVERWRITE created_at ON ontology_link TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON ontology_link TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE ontology_link_vault_key_unique ON ontology_link FIELDS vault, from_kind, label, to_kind UNIQUE;
