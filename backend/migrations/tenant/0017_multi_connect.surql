-- Multi-connect (docs/connectors.md): one user may connect the same kind more than once (two Gmail
-- accounts, two Pocket devices). `instance` tells a kind's connections apart: "" is the kind's first
-- connection (every connection made before 2.0.4, which keeps its sync cursor and its records as they
-- are), any other value is that connection's own record key. `label` is the name the user gives it.
DEFINE FIELD OVERWRITE instance ON connector TYPE string DEFAULT "";
DEFINE FIELD OVERWRITE label ON connector TYPE string DEFAULT "";
REMOVE INDEX IF EXISTS connector_owner_kind_unique ON connector;
DEFINE INDEX OVERWRITE connector_owner_kind_instance_unique ON connector FIELDS owner, kind, instance UNIQUE;
-- After the index, so the index sees the written value. A dozen rows per user at most.
UPDATE connector SET instance = "", label = "" WHERE instance = NONE OR label = NONE;
-- The connection a record was synced through. NONE: synced before 2.0.4, so the kind's first connection.
DEFINE FIELD OVERWRITE connector ON cache_record TYPE option<record<connector>>;
