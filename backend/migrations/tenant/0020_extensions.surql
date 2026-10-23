-- Extensions (docs/extensions.md): a user's own scripts talk to Eunomia over HTTP. An extension has a
-- webhook inbox (POST /hooks/ext/{key}, authenticated by a per-extension secret) whose deliveries are
-- buffered as `extension_event` rows; the script claims them in batches under a lease, then acks them.
-- Numbered 0020: 0019 is taken by another open change.
DEFINE TABLE OVERWRITE extension SCHEMAFULL;
DEFINE FIELD OVERWRITE owner ON extension TYPE record<user>;
DEFINE FIELD OVERWRITE name ON extension TYPE string;
DEFINE FIELD OVERWRITE description ON extension TYPE string DEFAULT "";
-- where the script is expected to write; NONE is the owner's personal vault
DEFINE FIELD OVERWRITE vault ON extension TYPE option<record<vault>>;
DEFINE FIELD OVERWRITE enabled ON extension TYPE bool DEFAULT true;
-- the secret, encrypted with ENCRYPTION_KEY (HMAC verification needs it back); shown once at creation
DEFINE FIELD OVERWRITE secret_enc ON extension TYPE string;
-- the last four characters, so the list can tell two secrets apart
DEFINE FIELD OVERWRITE secret_hint ON extension TYPE string DEFAULT "";
DEFINE FIELD OVERWRITE secret_rotated_at ON extension TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE created_at ON extension TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON extension TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE extension_owner_idx ON extension FIELDS owner;

DEFINE TABLE OVERWRITE extension_event SCHEMAFULL;
DEFINE FIELD OVERWRITE extension ON extension_event TYPE record<extension>;
DEFINE FIELD OVERWRITE owner ON extension_event TYPE record<user>;
-- the sender's delivery id (or one we made up); the record id is derived from it, so a redelivery is a no-op
DEFINE FIELD OVERWRITE delivery_id ON extension_event TYPE string;
-- the raw JSON text of the request body, exactly as received
DEFINE FIELD OVERWRITE body ON extension_event TYPE string;
DEFINE FIELD OVERWRITE size ON extension_event TYPE int DEFAULT 0;
-- pending -> claimed (leased) -> done; nack returns it to pending, and after too many attempts it is dead
DEFINE FIELD OVERWRITE status ON extension_event TYPE string DEFAULT "pending" ASSERT $value IN ["pending", "claimed", "done", "dead"];
DEFINE FIELD OVERWRITE claimed_until ON extension_event TYPE option<datetime>;
DEFINE FIELD OVERWRITE attempts ON extension_event TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE last_error ON extension_event TYPE option<string>;
DEFINE FIELD OVERWRITE received_at ON extension_event TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_at ON extension_event TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE extension_event_queue_idx ON extension_event FIELDS extension, status, received_at;
DEFINE INDEX OVERWRITE extension_event_recent_idx ON extension_event FIELDS extension, received_at;
DEFINE INDEX OVERWRITE extension_event_owner_idx ON extension_event FIELDS owner;
