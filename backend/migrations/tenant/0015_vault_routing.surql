-- Connector routing (docs/connectors.md): which vault a sync files each record in.
-- `{ default_vault: "vault:..." (absent: the owner's personal vault), rules: [{ field, value, vault }] }`.
DEFINE FIELD OVERWRITE routing ON connector TYPE object FLEXIBLE DEFAULT {};
-- A synced record's vault. NONE is the owner's personal vault, read by the owner only (every record
-- before 2.0.3); a vault id is where routing filed it, read by that vault's active members.
DEFINE FIELD OVERWRITE vault ON cache_record TYPE option<record<vault>>;
-- What routing rules match on (`field:value`, lowercase: tags, repo, channel, title, ...), kept so
-- "Move existing data" can re-file a record without asking the provider again. NONE before 2.0.3.
DEFINE FIELD OVERWRITE route_keys ON cache_record TYPE option<array<string>>;
-- Reads by vault (records routed to the vaults a caller reads), and moving a record's derived facts and
-- relations (by their `source`) without scanning every memory and edge.
DEFINE INDEX OVERWRITE cache_record_vault_idx ON cache_record FIELDS vault CONCURRENTLY;
DEFINE INDEX OVERWRITE memory_source_idx ON memory FIELDS source CONCURRENTLY;
DEFINE INDEX OVERWRITE relates_to_source_idx ON relates_to FIELDS source CONCURRENTLY;
