-- Documents per vault (docs/documents.md): lists read one vault's documents, and a vault clone, merge
-- or delete finds them, by `vault`. Their files sit under the vault's own key prefix in the org
-- database's bucket; a document stored before 2.0.4 keeps its `object_path` (no prefix) and still
-- downloads, so no file moves here.
DEFINE INDEX OVERWRITE document_vault_idx ON document FIELDS vault CONCURRENTLY;
