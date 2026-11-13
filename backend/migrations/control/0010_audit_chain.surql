-- Hash-chained security audit (compliance PR03, docs/audit-chain.md).
-- Expand-only. Rows written before this migration keep `ledger` NONE: they stay readable and are
-- reported as written before the chain started. Nothing already written is rewritten.
-- `ledger`: `org:<32 hex>` for an org's events, `instance` for events with no org (failed sign-ins).
-- `pending`: true from the write until the row is sealed into its ledger's chain (seq, prev_hash, hash).
DEFINE FIELD OVERWRITE ledger ON audit_event TYPE option<string>;
DEFINE FIELD OVERWRITE pending ON audit_event TYPE option<bool>;
DEFINE FIELD OVERWRITE seq ON audit_event TYPE option<int>;
DEFINE FIELD OVERWRITE prev_hash ON audit_event TYPE option<string>;
DEFINE FIELD OVERWRITE hash ON audit_event TYPE option<string>;
DEFINE INDEX OVERWRITE audit_event_chain_idx ON audit_event FIELDS ledger, seq;
DEFINE INDEX OVERWRITE audit_event_pending_idx ON audit_event FIELDS pending;
DEFINE INDEX OVERWRITE audit_event_ledger_time_idx ON audit_event FIELDS ledger, created_at;

-- One row per ledger (id = the ledger): the newest sealed sequence number and its hash. Sealing
-- compares and sets this row in the same transaction, so concurrent writers make one chain.
DEFINE TABLE OVERWRITE audit_head SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE ledger ON audit_head TYPE string;
DEFINE FIELD OVERWRITE seq ON audit_head TYPE int;
DEFINE FIELD OVERWRITE hash ON audit_head TYPE string;
DEFINE FIELD OVERWRITE genesis_at ON audit_head TYPE datetime;
DEFINE FIELD OVERWRITE updated_at ON audit_head TYPE datetime DEFAULT time::now();

-- Points a verifier can start from or compare against: the genesis (seq 0), a retention cut (the
-- last pruned event), and periodic copies of the head.
DEFINE TABLE OVERWRITE audit_checkpoint SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD OVERWRITE ledger ON audit_checkpoint TYPE string;
DEFINE FIELD OVERWRITE seq ON audit_checkpoint TYPE int;
DEFINE FIELD OVERWRITE hash ON audit_checkpoint TYPE string;
DEFINE FIELD OVERWRITE kind ON audit_checkpoint TYPE string ASSERT $value IN ["genesis", "periodic", "retention"];
DEFINE FIELD OVERWRITE at ON audit_checkpoint TYPE datetime;
DEFINE FIELD OVERWRITE pruned ON audit_checkpoint TYPE option<int>;
DEFINE INDEX OVERWRITE audit_checkpoint_seq_idx ON audit_checkpoint FIELDS ledger, seq;
DEFINE INDEX OVERWRITE audit_checkpoint_at_idx ON audit_checkpoint FIELDS ledger, at;
