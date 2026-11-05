-- Indexes for lookups that scanned a whole table (2.0.8 performance pass). No data changes.
--
-- Relation tables were indexed only from their `in` end (the UNIQUE (in, out, ...) indexes), so every
-- read from the `out` end was a full scan: a record's incoming links (`cache.links_in*`, the `links`
-- tool and recall's graph hops over synced records), an entity's incoming relations
-- (`entities.edges_in_in_vault`, entities_graph, entity_merge) and the export's relations into a set.
-- The memory index serves "one subject's memories of one type" (an observation lookup on every
-- memory_write, vault copy folding).
-- CONCURRENTLY: built in the background like the other large-table indexes, so the migration commits
-- at once; queries use a table scan until the build finishes.
DEFINE INDEX IF NOT EXISTS linked_to_out_idx ON linked_to FIELDS out, rel CONCURRENTLY;
DEFINE INDEX IF NOT EXISTS relates_to_out_idx ON relates_to FIELDS out CONCURRENTLY;
DEFINE INDEX IF NOT EXISTS memory_subject_type_idx ON memory FIELDS subject, type CONCURRENTLY;
