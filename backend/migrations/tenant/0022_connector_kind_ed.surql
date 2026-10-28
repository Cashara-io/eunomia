-- Ed Discussion connector: "ed" joins the allowed connector kinds (0001's list; "open_connector" and "demo"
-- stay valid for old rows).
DEFINE FIELD OVERWRITE kind ON connector TYPE string ASSERT $value IN ["up_bank","pocketai","open_connector","github","slack","notion","linear","gmail","google_calendar","discord","spotify","todoist","stripe","demo","ed"];
