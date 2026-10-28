-- Per-user: probe for an internet connection before a scheduled sync and skip the run quietly while
-- offline (default on).
DEFINE FIELD OVERWRITE sync_check_connectivity ON app_settings TYPE bool DEFAULT true;
-- A plain DEFAULT applies on create only: rows that already exist stay NONE, and their next UPDATE
-- (any settings save) would then fail the bool type check. One row per user.
-- 0011 added `chat_model` the same way without a backfill, so a row older than it has the same gap.
-- Both go in one statement: an UPDATE type-checks every field of the row, so fixing one at a time fails
-- on the other. `??` keeps a value that is already set.
UPDATE app_settings SET sync_check_connectivity = sync_check_connectivity ?? true, chat_model = chat_model ?? ""
    WHERE sync_check_connectivity = NONE OR chat_model = NONE;
