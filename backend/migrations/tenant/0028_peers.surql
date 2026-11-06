-- Peer vaults (docs/peer-vaults.md): a vault shared with another Eunomia instance, kept in step by an
-- encrypted exchange of the vault's change log.

-- This org database's identity towards other instances: one row, `peer_instance:self`.
DEFINE TABLE OVERWRITE peer_instance SCHEMAFULL;
-- 32 hex digits, random, made on first use
DEFINE FIELD OVERWRITE instance_id ON peer_instance TYPE string;
DEFINE FIELD OVERWRITE created_at ON peer_instance TYPE datetime DEFAULT time::now();

-- A one-time share code an owner made for a vault. The code itself is never stored: only its secret,
-- encrypted at rest (both sides key a MAC with it), wiped once the code is used.
DEFINE TABLE OVERWRITE peer_share SCHEMAFULL;
DEFINE FIELD OVERWRITE vault ON peer_share TYPE record<vault>;
DEFINE FIELD OVERWRITE role ON peer_share TYPE string ASSERT $value IN ["edit", "read"];
DEFINE FIELD OVERWRITE secret_enc ON peer_share TYPE option<string>;
DEFINE FIELD OVERWRITE created_by ON peer_share TYPE record<user>;
DEFINE FIELD OVERWRITE created_at ON peer_share TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE expires_at ON peer_share TYPE datetime;
DEFINE FIELD OVERWRITE used_at ON peer_share TYPE option<datetime>;
-- the link the code turned into
DEFINE FIELD OVERWRITE link ON peer_share TYPE option<string>;
DEFINE INDEX OVERWRITE peer_share_vault_idx ON peer_share FIELDS vault;

-- One shared vault between this instance and one other: `host` holds the original, `guest` a copy.
-- The key is the link id both sides share (32 hex digits).
DEFINE TABLE OVERWRITE peer_link SCHEMAFULL;
DEFINE FIELD OVERWRITE vault ON peer_link TYPE record<vault>;
DEFINE FIELD OVERWRITE direction ON peer_link TYPE string ASSERT $value IN ["host", "guest"];
-- what the guest may do: "edit" (its changes come back) or "read"
DEFINE FIELD OVERWRITE role ON peer_link TYPE string ASSERT $value IN ["edit", "read"];
-- the link key, encrypted at rest
DEFINE FIELD OVERWRITE key_enc ON peer_link TYPE string;
-- the local user the link writes as (the code's maker on the host, the joiner on the guest)
DEFINE FIELD OVERWRITE user ON peer_link TYPE record<user>;
DEFINE FIELD OVERWRITE peer_instance ON peer_link TYPE string;
DEFINE FIELD OVERWRITE peer_name ON peer_link TYPE string DEFAULT "";
DEFINE FIELD OVERWRITE peer_org ON peer_link TYPE string DEFAULT "";
-- the other side's addresses, best first; `addr` is the one that answered last
DEFINE FIELD OVERWRITE peer_addrs ON peer_link TYPE array<string> DEFAULT [];
DEFINE FIELD OVERWRITE addr ON peer_link TYPE option<string>;
-- this side's change-log seq the other side has acknowledged (guest), and the other side's seq applied here
DEFINE FIELD OVERWRITE out_cursor ON peer_link TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE in_cursor ON peer_link TYPE int DEFAULT 0;
-- up_to_date | syncing | unreachable | integrity_problem | revoked
DEFINE FIELD OVERWRITE status ON peer_link TYPE string DEFAULT "syncing";
DEFINE FIELD OVERWRITE last_error ON peer_link TYPE option<string>;
DEFINE FIELD OVERWRITE last_attempt_at ON peer_link TYPE option<datetime>;
DEFINE FIELD OVERWRITE last_sync_at ON peer_link TYPE option<datetime>;
DEFINE FIELD OVERWRITE integrity_failures ON peer_link TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE local_digest ON peer_link TYPE option<string>;
DEFINE FIELD OVERWRITE remote_digest ON peer_link TYPE option<string>;
DEFINE FIELD OVERWRITE digests_match ON peer_link TYPE option<bool>;
-- counts of the last exchange, for the status line
DEFINE FIELD OVERWRITE last_report ON peer_link TYPE option<object> FLEXIBLE;
DEFINE FIELD OVERWRITE created_at ON peer_link TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE peer_link_vault_idx ON peer_link FIELDS vault;

-- Which local item an item of the other side is. Keyed `peer_item:[<link key>, <local id>]`.
DEFINE TABLE OVERWRITE peer_item SCHEMAFULL;
DEFINE FIELD OVERWRITE link ON peer_item TYPE string;
DEFINE FIELD OVERWRITE vault ON peer_item TYPE record<vault>;
DEFINE FIELD OVERWRITE kind ON peer_item TYPE string ASSERT $value IN ["entity", "memory", "relation", "skill"];
-- the local item's record id, as text
DEFINE FIELD OVERWRITE local ON peer_item TYPE string;
-- the other side's id for it, once known
DEFINE FIELD OVERWRITE remote ON peer_item TYPE option<string>;
-- "<instance id>:<record id>" of the instance the item was first written on (NONE: this one)
DEFINE FIELD OVERWRITE origin ON peer_item TYPE option<string>;
-- content hash both sides last agreed on (the base for conflict detection)
DEFINE FIELD OVERWRITE hash ON peer_item TYPE string;
DEFINE FIELD OVERWRITE updated_at ON peer_item TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE peer_item_link_remote_idx ON peer_item FIELDS link, remote;
DEFINE INDEX OVERWRITE peer_item_vault_local_idx ON peer_item FIELDS vault, local;
DEFINE INDEX OVERWRITE peer_item_vault_origin_idx ON peer_item FIELDS vault, origin;
