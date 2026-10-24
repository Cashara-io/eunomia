-- Semantic recall over remembered facts (issue #65). A memory's `embedding` is written by the embed job
-- (and right after a write), under `embedding_spec`: the provider origin and model that made it, so a
-- query only meets vectors from the same model. NONE: not embedded yet (every memory before 2.0.6).
DEFINE FIELD OVERWRITE embedding ON memory TYPE option<array<float>>;
DEFINE FIELD OVERWRITE embedding_spec ON memory TYPE option<string>;
DEFINE INDEX OVERWRITE memory_embedding_idx ON memory FIELDS embedding HNSW DIMENSION 1536 DIST COSINE TYPE F32 CONCURRENTLY;
-- the embed job's backlog probe: an owner's memories with no vector yet
DEFINE INDEX OVERWRITE memory_owner_spec_idx ON memory FIELDS owner, embedding_spec CONCURRENTLY;
-- Any change to the text (an edit, a consolidation rewriting an observation) drops the vector, so a
-- stale one is never searched; the embed job makes a new one. Database side, so every code path and
-- code at schema N-1 keep it right.
DEFINE EVENT OVERWRITE memory_text_changed ON memory
    WHEN $event = "UPDATE" AND $before.text != $after.text AND $after.embedding != NONE
    THEN (UPDATE $after.id SET embedding = NONE, embedding_spec = NONE);
-- When the fact happened, if the writer said so (a fact imported today about last year), distinct from
-- `created_at` (when it was written). Recall's date range and recency use it, falling back to created_at.
DEFINE FIELD OVERWRITE occurred_at ON memory TYPE option<datetime>;
