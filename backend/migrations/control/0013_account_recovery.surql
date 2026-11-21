-- Password recovery by email (`recovery.rs`, `routers/recovery.rs`, docs/deployment.md "Email").
-- The reset link's token lives on the account it resets, not in a table of its own: one open link per
-- account (asking again replaces it, so only the newest link works), and everything about an account
-- goes with the account when it is erased. Only the token's sha256 is stored, never the token.

-- sha256 of the open reset link's token; NONE when no link is open or after it was used
DEFINE FIELD OVERWRITE reset_token_hash ON user TYPE option<string>;
-- the link stops working at this time (30 minutes after it was sent)
DEFINE FIELD OVERWRITE reset_expires_at ON user TYPE option<datetime>;
-- when the last link was sent: a new one is not issued within a minute of it, on any replica
DEFINE FIELD OVERWRITE reset_sent_at ON user TYPE option<datetime>;
DEFINE INDEX OVERWRITE user_reset_token_idx ON user FIELDS reset_token_hash;
