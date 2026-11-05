-- Per-user: the name the email extraction prompt uses for the mailbox owner ("" = unknown).
DEFINE FIELD OVERWRITE owner_name ON app_settings TYPE string DEFAULT "";
-- A plain DEFAULT applies on create only: backfill existing rows so their next settings save passes the type check.
UPDATE app_settings SET owner_name = owner_name ?? "" WHERE owner_name = NONE;
