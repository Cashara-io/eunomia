-- Expand the existing connector kind constraint. Credentials and configuration reuse existing fields.
DEFINE FIELD OVERWRITE kind ON connector TYPE string ASSERT $value IN ["up_bank","pocketai","open_connector","github","slack","notion","linear","gmail","google_calendar","discord","spotify","todoist","stripe","demo","ed","composio"];

-- File identities survive a failed ingest so retries revise the same document.
DEFINE TABLE composio_file SCHEMAFULL;
DEFINE FIELD owner ON composio_file TYPE record<user>;
DEFINE FIELD connector ON composio_file TYPE record<connector>;
DEFINE FIELD external_id ON composio_file TYPE string;
DEFINE FIELD document_id ON composio_file TYPE string;
DEFINE INDEX composio_file_identity ON composio_file FIELDS owner, connector, external_id UNIQUE;
