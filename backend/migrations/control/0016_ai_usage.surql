-- Model spend per organisation (`ai/`, docs/deployment.md "Running a hosted service", "Model spend"). Every table
-- and field is new: an organisation with nothing set has no allowance, the server's key and AI switched on, which is
-- every organisation on a self-hosted install, and behaves exactly as before.

-- What an organisation has said about model use, and what the billing service allows it. One row per
-- organisation, `org_ai:<org key>`, written by whichever side owns the field:
--   owner or admin (`PUT /api/org/ai/...`): enabled, base_url, api_key_enc, chat_model, embedding_model
--   billing service (operator API):          allowance_micros
--   the job worker (`ai::hold`):             hold_until
DEFINE TABLE OVERWRITE org_ai SCHEMAFULL;
DEFINE FIELD OVERWRITE org ON org_ai TYPE record<org>;
-- false: no model is called for this organisation's data with any key (capability C31)
DEFINE FIELD OVERWRITE enabled ON org_ai TYPE bool DEFAULT true;
-- the organisation's own provider; "" for each: none. The key is encrypted like a person's own key and never returned.
DEFINE FIELD OVERWRITE base_url ON org_ai TYPE string DEFAULT "";
DEFINE FIELD OVERWRITE api_key_enc ON org_ai TYPE string DEFAULT "";
DEFINE FIELD OVERWRITE chat_model ON org_ai TYPE string DEFAULT "";
DEFINE FIELD OVERWRITE embedding_model ON org_ai TYPE string DEFAULT "";
-- the monthly allowance of spend on the server's key, in millionths of a US dollar (calendar month, UTC); NONE: unlimited
DEFINE FIELD OVERWRITE allowance_micros ON org_ai TYPE option<int> ASSERT $value = NONE OR $value >= 0;
-- set by a worker that paused a job for lack of allowance: the claim leaves this organisation's model jobs alone until then
DEFINE FIELD OVERWRITE hold_until ON org_ai TYPE option<datetime>;
DEFINE FIELD OVERWRITE updated_at ON org_ai TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_by ON org_ai TYPE option<record<user>>;
DEFINE INDEX OVERWRITE org_ai_org_idx ON org_ai FIELDS org UNIQUE;
DEFINE INDEX OVERWRITE org_ai_hold_idx ON org_ai FIELDS hold_until;

-- Counters, not calls: one row per organisation, day, purpose, model and whose key paid, upserted by a
-- deterministic id (`ai::meter`). Counts and cost only: no prompt, no text, no key.
DEFINE TABLE OVERWRITE ai_usage SCHEMAFULL;
DEFINE FIELD OVERWRITE org ON ai_usage TYPE record<org>;
DEFINE FIELD OVERWRITE day ON ai_usage TYPE string;
DEFINE FIELD OVERWRITE month ON ai_usage TYPE string;
DEFINE FIELD OVERWRITE purpose ON ai_usage TYPE string;
DEFINE FIELD OVERWRITE model ON ai_usage TYPE string;
-- `server`: our key (costs money, counts against the allowance); `org` and `user`: their own key (counted, never costed)
DEFINE FIELD OVERWRITE source ON ai_usage TYPE string ASSERT $value IN ["server", "org", "user"];
DEFINE FIELD OVERWRITE calls ON ai_usage TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE input_tokens ON ai_usage TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE output_tokens ON ai_usage TYPE int DEFAULT 0;
-- how many of the calls had their tokens counted by this server instead of reported by the provider
DEFINE FIELD OVERWRITE estimated_calls ON ai_usage TYPE int DEFAULT 0;
-- millionths of a US dollar, from the price table at the time of the call; 0 for a key that is not ours
DEFINE FIELD OVERWRITE cost_micros ON ai_usage TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE updated_at ON ai_usage TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE ai_usage_org_month_idx ON ai_usage FIELDS org, month;
DEFINE INDEX OVERWRITE ai_usage_month_idx ON ai_usage FIELDS month;

-- Prices the operator set, per model and per million tokens in US dollars. A model with no row here uses the
-- defaults built into the server; a model in neither uses a deliberately high fallback (`ai::prices`).
DEFINE TABLE OVERWRITE ai_price SCHEMAFULL;
DEFINE FIELD OVERWRITE model ON ai_price TYPE string;
DEFINE FIELD OVERWRITE input_per_m ON ai_price TYPE float ASSERT $value >= 0;
DEFINE FIELD OVERWRITE output_per_m ON ai_price TYPE float ASSERT $value >= 0;
DEFINE FIELD OVERWRITE updated_at ON ai_price TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE ai_price_model_idx ON ai_price FIELDS model UNIQUE;
