-- Secret requests (docs/architecture/specs/secret-requests.md): an agent asks for a secret, the person
-- pastes it into Eunomia's own pop-up, and the agent gets only a handle, eu://<vault key>/<name>. The value
-- is held only as enc:v1: AES-256-GCM ciphertext under ENCRYPTION_KEY (connectors/crypto.rs), which is not
-- in the database, and no API returns it. Both tables are SCHEMAFULL: a write cannot add a field.

-- A request for a secret, from an agent's credential, waiting for (or answered by) the person behind it.
DEFINE TABLE OVERWRITE secret_request SCHEMAFULL;
DEFINE FIELD OVERWRITE vault ON secret_request TYPE record<vault>;
DEFINE FIELD OVERWRITE name ON secret_request TYPE string ASSERT string::len($value) >= 1 AND string::len($value) <= 64;
-- written by the agent, redacted (#111) before they are stored
DEFINE FIELD OVERWRITE label ON secret_request TYPE string DEFAULT "" ASSERT string::len($value) <= 120;
DEFINE FIELD OVERWRITE description ON secret_request TYPE string DEFAULT "" ASSERT string::len($value) <= 1000;
DEFINE FIELD OVERWRITE target_hosts ON secret_request TYPE array<string> DEFAULT [];
DEFINE FIELD OVERWRITE placements ON secret_request TYPE array<string> DEFAULT ["header"];
DEFINE FIELD OVERWRITE connector ON secret_request TYPE option<string>;
DEFINE FIELD OVERWRITE requested_by ON secret_request TYPE record<user>;
DEFINE FIELD OVERWRITE actor_kind ON secret_request TYPE string DEFAULT "";
DEFINE FIELD OVERWRITE actor_id ON secret_request TYPE string DEFAULT "";
DEFINE FIELD OVERWRITE client_name ON secret_request TYPE string DEFAULT "";
DEFINE FIELD OVERWRITE channel ON secret_request TYPE string DEFAULT "";
DEFINE FIELD OVERWRITE session ON secret_request TYPE string DEFAULT "";
DEFINE FIELD OVERWRITE status ON secret_request TYPE string DEFAULT "pending" ASSERT $value IN ["pending", "filled", "declined", "expired"];
DEFINE FIELD OVERWRITE replace ON secret_request TYPE bool DEFAULT false;
DEFINE FIELD OVERWRITE secret_ttl_days ON secret_request TYPE option<int>;
DEFINE FIELD OVERWRITE expires_at ON secret_request TYPE datetime;
DEFINE FIELD OVERWRITE created_at ON secret_request TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE decided_at ON secret_request TYPE option<datetime>;
DEFINE FIELD OVERWRITE decided_by ON secret_request TYPE option<record<user>>;
DEFINE FIELD OVERWRITE secret ON secret_request TYPE option<record<vault_secret>>;
DEFINE INDEX OVERWRITE secret_request_vault_name_idx ON secret_request FIELDS vault, name, status;
DEFINE INDEX OVERWRITE secret_request_requested_by_idx ON secret_request FIELDS requested_by, status;

-- A secret Eunomia holds for agents: pasted into a request, or derived from others by a transform.
DEFINE TABLE OVERWRITE vault_secret SCHEMAFULL;
DEFINE FIELD OVERWRITE vault ON vault_secret TYPE record<vault>;
DEFINE FIELD OVERWRITE name ON vault_secret TYPE string ASSERT string::len($value) >= 1 AND string::len($value) <= 64;
DEFINE FIELD OVERWRITE label ON vault_secret TYPE string DEFAULT "";
-- the value, only ever as ciphertext: a plaintext value is refused by the database too
DEFINE FIELD OVERWRITE value_enc ON vault_secret TYPE string ASSERT string::starts_with($value, "enc:v1:");
-- what the person approved: hosts it may be sent to (empty: none), and where in a request
DEFINE FIELD OVERWRITE allowed_hosts ON vault_secret TYPE array<string> DEFAULT [];
DEFINE FIELD OVERWRITE placements ON vault_secret TYPE array<string> DEFAULT ["header"];
DEFINE FIELD OVERWRITE allowed_tools ON vault_secret TYPE array<string> DEFAULT ["http_request", "secret_transform"];
-- actor ids that may use it; empty: any credential of a member of the vault
DEFINE FIELD OVERWRITE allowed_actors ON vault_secret TYPE array<string> DEFAULT [];
-- lineage: the held secrets a transform made it from (empty for a pasted one), and the operation
DEFINE FIELD OVERWRITE derived_from ON vault_secret TYPE array<record<vault_secret>> DEFAULT [];
DEFINE FIELD OVERWRITE op ON vault_secret TYPE option<string>;
DEFINE FIELD OVERWRITE request ON vault_secret TYPE option<record<secret_request>>;
DEFINE FIELD OVERWRITE created_by ON vault_secret TYPE record<user>;
DEFINE FIELD OVERWRITE created_at ON vault_secret TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON vault_secret TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE expires_at ON vault_secret TYPE option<datetime>;
DEFINE INDEX OVERWRITE vault_secret_vault_name_idx ON vault_secret FIELDS vault, name UNIQUE;
DEFINE INDEX OVERWRITE vault_secret_derived_from_idx ON vault_secret FIELDS derived_from;
