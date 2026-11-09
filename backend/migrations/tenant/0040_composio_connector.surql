-- Expand the existing connector kind constraint. Credentials and configuration reuse existing fields.
DEFINE FIELD OVERWRITE kind ON connector TYPE string ASSERT $value IN ["up_bank","pocketai","open_connector","github","slack","notion","linear","gmail","google_calendar","discord","spotify","todoist","stripe","demo","ed","composio"];
