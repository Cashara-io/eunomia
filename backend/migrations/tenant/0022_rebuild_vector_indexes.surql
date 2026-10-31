-- Rebuild the three HNSW vector indexes once, on SurrealDB 3.3.5. 3.3.4 and 3.3.5 fixed HNSW bugs where
-- a delete could leave a removed point in the graph and break kNN results for good, and memories, skills
-- and cache records are deleted and superseded all the time, so an index built on 3.3.1 may be damaged.
-- No data changes: only the index is rebuilt from the rows.
--
-- The rebuild is DEFINE INDEX OVERWRITE with the unchanged definition (0019, 0021 and 0008): it retires
-- the old index (its entries are reclaimed in the background) and builds a new one under a fresh index
-- id from the table's rows, in the background (CONCURRENTLY) like the original definitions, so the
-- migration commits at once. Until the build finishes a kNN query underfills and recall, skill_find and
-- record search fall back to their exact scan. Not REBUILD INDEX: on 3.3.5 it fails with "index is
-- currently building" while a CONCURRENTLY build is still running, which is the case in a fresh org,
-- where this runs right after 0019 and 0021 define the indexes. Running these statements again is
-- harmless: each run is one more rebuild.
DEFINE INDEX OVERWRITE memory_embedding_idx ON memory FIELDS embedding HNSW DIMENSION 1536 DIST COSINE TYPE F32 CONCURRENTLY;
DEFINE INDEX OVERWRITE skill_embedding_idx ON skill FIELDS embedding HNSW DIMENSION 1536 DIST COSINE TYPE F32 CONCURRENTLY;
DEFINE INDEX OVERWRITE cache_record_embedding_idx ON cache_record FIELDS embedding HNSW DIMENSION 1536 DIST COSINE TYPE F32 CONCURRENTLY;
