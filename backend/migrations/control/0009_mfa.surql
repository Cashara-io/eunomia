-- Two-step verification (TOTP, RFC 6238) for password accounts, single-use recovery codes, the short-lived
-- sign-in challenge between the password and the second step, and the instance-wide "require MFA" switch.
-- Everything is new and optional: existing users, sessions and tokens read as "no second factor yet" and keep
-- working until an admin turns the switch on (see docs/security/mfa.md).

-- `mfa_secret_enc`: the confirmed TOTP secret, AES-GCM under ENCRYPTION_KEY (connectors/crypto.rs). NONE = MFA off.
-- `mfa_pending_enc`: a secret shown during enrolment that no valid code has confirmed yet; it never signs anyone in.
-- `mfa_last_step`: the last 30-second TOTP step accepted, so a code cannot be replayed inside its window.
DEFINE FIELD OVERWRITE mfa_secret_enc ON user TYPE option<string>;
DEFINE FIELD OVERWRITE mfa_enabled_at ON user TYPE option<datetime>;
DEFINE FIELD OVERWRITE mfa_pending_enc ON user TYPE option<string>;
DEFINE FIELD OVERWRITE mfa_pending_at ON user TYPE option<datetime>;
DEFINE FIELD OVERWRITE mfa_last_step ON user TYPE option<int>;
-- Codes tried in the current 15-minute window (right or wrong), counted before each one is checked, so
-- parallel requests cannot exceed the budget. A right code resets it. Shared by every replica.
DEFINE FIELD OVERWRITE mfa_attempts ON user TYPE option<int>;
DEFINE FIELD OVERWRITE mfa_attempts_since ON user TYPE option<datetime>;

-- When this browser session last proved a second factor (TOTP or a recovery code). NONE for a password session
-- from before MFA existed. `via` is how it signed in: `password` or `oidc` (NONE before this migration).
DEFINE FIELD OVERWRITE mfa_at ON session TYPE option<datetime>;
DEFINE FIELD OVERWRITE via ON session TYPE option<string>;

-- Recovery codes: only the SHA-256 of each code is stored. `used_at` is set once, atomically, when one is spent.
DEFINE TABLE OVERWRITE mfa_recovery_code SCHEMAFULL;
DEFINE FIELD OVERWRITE user ON mfa_recovery_code TYPE record<user>;
DEFINE FIELD OVERWRITE code_hash ON mfa_recovery_code TYPE string;
DEFINE FIELD OVERWRITE used_at ON mfa_recovery_code TYPE option<datetime>;
DEFINE FIELD OVERWRITE created_at ON mfa_recovery_code TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE mfa_recovery_code_user_idx ON mfa_recovery_code FIELDS user, code_hash;

-- A password sign-in that still owes its second step. The browser holds the random token (an HttpOnly cookie
-- scoped to /api/auth/mfa); only its SHA-256 is stored. It is not a session: it can only verify a code or
-- enrol, expires after a few minutes, and is deleted after too many wrong codes. `purpose`: `verify` (the
-- account has MFA) or `enrol` (MFA is required and the account has none yet).
DEFINE TABLE OVERWRITE mfa_challenge SCHEMAFULL;
DEFINE FIELD OVERWRITE user ON mfa_challenge TYPE record<user>;
DEFINE FIELD OVERWRITE token_hash ON mfa_challenge TYPE string;
DEFINE FIELD OVERWRITE purpose ON mfa_challenge TYPE string ASSERT $value IN ["verify", "enrol"];
DEFINE FIELD OVERWRITE via ON mfa_challenge TYPE string DEFAULT "password";
DEFINE FIELD OVERWRITE user_agent ON mfa_challenge TYPE string DEFAULT "";
-- codes tried on this challenge, counted before each is checked
DEFINE FIELD OVERWRITE failures ON mfa_challenge TYPE int DEFAULT 0;
DEFINE FIELD OVERWRITE expires_at ON mfa_challenge TYPE datetime;
DEFINE FIELD OVERWRITE created_at ON mfa_challenge TYPE datetime DEFAULT time::now();
DEFINE INDEX OVERWRITE mfa_challenge_hash_unique ON mfa_challenge FIELDS token_hash UNIQUE;
DEFINE INDEX OVERWRITE mfa_challenge_user_idx ON mfa_challenge FIELDS user;

-- Instance-wide security settings an admin changes in Settings. One row, `instance_setting:security`.
-- `require_mfa`: every password account must pass a second factor before it gets a session.
DEFINE TABLE OVERWRITE instance_setting SCHEMAFULL;
DEFINE FIELD OVERWRITE require_mfa ON instance_setting TYPE bool DEFAULT false;
DEFINE FIELD OVERWRITE updated_at ON instance_setting TYPE datetime DEFAULT time::now();
DEFINE FIELD OVERWRITE updated_by ON instance_setting TYPE option<record<user>>;
