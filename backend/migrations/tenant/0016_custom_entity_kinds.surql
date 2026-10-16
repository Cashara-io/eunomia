-- Agent-defined entity kinds (docs/concepts.md#entity-kinds). The six built-in kinds keep their own tables.
-- Every other kind ("db", "table", "decision", ...) lives in ONE table, `entity`, told apart by `kind`:
-- a kind name comes from an agent, so it never becomes a table name. The app validates `kind` (a lowercase
-- slug, not a built-in) and caps how many custom kinds a vault holds.
DEFINE TABLE OVERWRITE entity SCHEMAFULL;
DEFINE FIELD OVERWRITE owner ON entity TYPE record<user>;
DEFINE FIELD OVERWRITE vault ON entity TYPE record<vault>;
DEFINE FIELD OVERWRITE kind ON entity TYPE string;
DEFINE FIELD OVERWRITE name ON entity TYPE string;
DEFINE FIELD OVERWRITE aliases ON entity TYPE array<string> DEFAULT [];
DEFINE FIELD OVERWRITE summary ON entity TYPE string DEFAULT "";
DEFINE FIELD OVERWRITE created_at ON entity TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON entity TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE name_key ON entity TYPE string VALUE string::lowercase(name);
DEFINE FIELD OVERWRITE alias_keys ON entity TYPE array<string> VALUE (aliases ?? []).map(|$a| string::lowercase($a));
-- a name is unique per kind in a vault, as it is for the built-in kinds
DEFINE INDEX OVERWRITE entity_vault_kind_name_key_unique ON entity FIELDS vault, kind, name_key UNIQUE;
DEFINE INDEX OVERWRITE entity_alias_keys_idx ON entity FIELDS alias_keys;
DEFINE INDEX OVERWRITE entity_vault_kind_idx ON entity FIELDS vault, kind;
-- memories and relations may hang off a custom-kind entity too (existing rows all still fit the wider type)
DEFINE FIELD OVERWRITE subject ON memory TYPE record<person | organisation | location | repository | file | symbol | entity>;
DEFINE TABLE OVERWRITE relates_to SCHEMAFULL TYPE RELATION
    FROM person | organisation | location | repository | file | symbol | entity
    TO person | organisation | location | repository | file | symbol | entity;
