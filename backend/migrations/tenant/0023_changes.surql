-- A numbered change log per vault (docs/projects.md): one row for each write that changes what a vault
-- holds, so a client can catch up from a cursor (`GET /api/vaults/{vault}/changes?after=`) or follow
-- live (`.../changes/ws`). No content: a reader fetches the item itself, so secrets never land here.
DEFINE TABLE OVERWRITE change SCHEMAFULL;
DEFINE FIELD OVERWRITE vault ON change TYPE record<vault>;
-- strictly increasing per vault with no gaps: taken from `change_seq` in the same transaction
DEFINE FIELD OVERWRITE seq ON change TYPE int;
DEFINE FIELD OVERWRITE op ON change TYPE string ASSERT $value IN ["created", "updated", "deleted", "merged"];
DEFINE FIELD OVERWRITE kind ON change TYPE string ASSERT $value IN ["memory", "entity", "relation", "skill", "document"];
-- the changed item's record id, as text (it may be gone already)
DEFINE FIELD OVERWRITE item ON change TYPE string;
-- who wrote it: a user, token or OAuth client id, or "system" for background jobs
DEFINE FIELD OVERWRITE actor ON change TYPE string;
DEFINE FIELD OVERWRITE at ON change TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE change_vault_seq_idx ON change FIELDS vault, seq UNIQUE;

-- One counter row per vault (keyed by the vault's key): the last seq handed out.
DEFINE TABLE OVERWRITE change_seq SCHEMAFULL;
DEFINE FIELD OVERWRITE n ON change_seq TYPE int DEFAULT 0;
