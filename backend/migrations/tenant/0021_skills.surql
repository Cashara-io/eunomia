-- Agent skills (issue #93): a skill an agent learned (SKILL.md style: name, description, markdown body),
-- scoped to a vault and embedded (name + description) so another agent can find it by meaning.
-- Not the built-in memory skill (`app_settings.memory_skill`), which is one instruction for the whole install.
DEFINE TABLE OVERWRITE skill SCHEMAFULL;
DEFINE FIELD OVERWRITE vault ON skill TYPE record<vault>;
-- the author
DEFINE FIELD OVERWRITE owner ON skill TYPE record<user>;
DEFINE FIELD OVERWRITE name ON skill TYPE string;
DEFINE FIELD OVERWRITE description ON skill TYPE string;
DEFINE FIELD OVERWRITE body ON skill TYPE string;
DEFINE FIELD OVERWRITE created_at ON skill TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON skill TYPE datetime DEFAULT time::now();
-- as on memory (0019): the vector and the provider and model that made it; NONE: not embedded
DEFINE FIELD OVERWRITE embedding ON skill TYPE option<array<float>>;
DEFINE FIELD OVERWRITE embedding_spec ON skill TYPE option<string>;
DEFINE INDEX OVERWRITE skill_vault_name_idx ON skill FIELDS vault, name UNIQUE;
DEFINE INDEX OVERWRITE skill_embedding_idx ON skill FIELDS embedding HNSW DIMENSION 1536 DIST COSINE TYPE F32 CONCURRENTLY;
-- A new name or description drops the vector, so a stale one is never searched (database side, like memory_text_changed).
DEFINE EVENT OVERWRITE skill_text_changed ON skill
    WHEN $event = "UPDATE" AND ($before.name != $after.name OR $before.description != $after.description) AND $after.embedding != NONE
    THEN (UPDATE $after.id SET embedding = NONE, embedding_spec = NONE);
