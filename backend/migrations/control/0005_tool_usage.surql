-- Tool-call counters (src/usage.rs): one row per hour, user, credential, channel, session and tool,
-- upserted by a deterministic id on every tool call. Counts only: no arguments and no content.
-- Unlike audit_event it is mutable (the counters go up) and is pruned by TOOL_USAGE_RETENTION_DAYS.
DEFINE TABLE OVERWRITE tool_usage SCHEMAFULL;
DEFINE FIELD OVERWRITE org ON tool_usage TYPE string;
DEFINE FIELD OVERWRITE bucket ON tool_usage TYPE datetime;
DEFINE FIELD OVERWRITE user ON tool_usage TYPE record<user>;
DEFINE FIELD OVERWRITE actor_kind ON tool_usage TYPE string ASSERT $value IN ["user", "token", "oauth"];
DEFINE FIELD OVERWRITE actor_id ON tool_usage TYPE string DEFAULT "";
DEFINE FIELD OVERWRITE channel ON tool_usage TYPE string ASSERT $value IN ["rest", "mcp", "chat"];
DEFINE FIELD OVERWRITE session ON tool_usage TYPE string DEFAULT "";
DEFINE FIELD OVERWRITE tool ON tool_usage TYPE string;
DEFINE FIELD OVERWRITE calls ON tool_usage TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE errors ON tool_usage TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE total_ms ON tool_usage TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE updated_at ON tool_usage TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE tool_usage_org_idx ON tool_usage FIELDS org, bucket;
DEFINE INDEX OVERWRITE tool_usage_user_idx ON tool_usage FIELDS user, bucket;
