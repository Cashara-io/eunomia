//! Statements for the control database: accounts, credentials, OAuth, the audit ledger and org
//! membership. See the module docs in store/mod.rs.

use super::ControlStmt;

pub const ALL: &[&ControlStmt] = &[
    &READY_PING,
    &READY_VERSION,
    &AUTH_SESSION_CREATE,
    &AUTH_SESSION_FIND,
    &AUTH_SESSION_TOUCH,
    &AUTH_SESSION_REVOKE_BY_SID,
    &AUTH_SESSION_REVOKE,
    &AUTH_SESSION_LIST,
    &AUTH_USER_ID_BY_EMAIL,
    &AUTH_USER_CREATE,
    &AUTH_USER_BY_EMAIL,
    &AUTH_USER_COUNT,
    &AUTH_USER_ONBOARDED,
    &AUTH_USER_DELETE,
    &AUTH_TOKEN_DELETE,
    &AUTH_TOKEN_RENAME,
    &AUTH_TOKEN_CREATE,
    &AUTH_TOKEN_LIST,
    &AUTH_TOKEN_BY_HASH,
    &AUTH_TOKEN_TOUCH,
    &EXPORT_USER_EMAILS,
    &SOURCES_USER_IDS,
    &OAUTH_CLIENT_GET,
    &OAUTH_CLIENT_UPSERT,
    &OAUTH_CLIENT_PRUNE,
    &OAUTH_CLIENT_TOUCH,
    &OAUTH_CLIENT_DCR_COUNT,
    &OAUTH_CODE_CREATE,
    &OAUTH_CODE_TAKE,
    &OAUTH_CODE_REDEEMED,
    &OAUTH_CODE_LINK_GRANT,
    &OAUTH_GRANT_CREATE,
    &OAUTH_GRANT_LIST,
    &OAUTH_GRANT_DELETE,
    &OAUTH_GRANT_TOUCH,
    &OAUTH_TOKEN_CREATE,
    &OAUTH_TOKEN_BY_HASH,
    &OAUTH_TOKEN_SPEND,
    &OAUTH_TOKEN_DELETE,
    &OAUTH_TOKENS_DELETE_FAMILY,
    &OAUTH_PRUNE,
    &AUDIT_EVENT_CREATE,
    &AUDIT_PRUNE,
    &SESSION_PRUNE,
    &ORG_OF_USER,
    &ORG_FIRST_OWNER,
    &ORG_MEMBERS,
    &EMAIL_IN_ORG,
    &MEMBERSHIP_ADD,
    &USER_EMAIL_LC_BACKFILL,
    &USER_IDS_OLDEST_FIRST,
    &MEMBERSHIP_DELETE_USER,
];

pub const AUTH_SESSION_CREATE: ControlStmt =
    ControlStmt::new(
    "app.auth_session_create",
    "CREATE session SET owner = $owner, sid = $sid, user_agent = $user_agent, expires_at = $expires_at",
);

pub const AUTH_SESSION_FIND: ControlStmt = ControlStmt::new(
    "app.auth_session_find",
    "SELECT *, (expires_at != NONE AND expires_at <= time::now()) AS expired FROM session \
     WHERE sid = $sid AND owner = $owner AND revoked = false LIMIT 1",
);

/// Slides `expires_at` forward, but only when it is more than an hour behind
/// `$new_exp` (so at most one extension per hour of use).
pub const AUTH_SESSION_TOUCH: ControlStmt = ControlStmt::new(
    "app.auth_session_touch",
    "UPDATE $id SET last_seen_at = time::now(), \
     expires_at = IF expires_at = NONE OR expires_at < $threshold THEN $new_exp ELSE expires_at END",
);

pub const AUTH_SESSION_REVOKE_BY_SID: ControlStmt =
    ControlStmt::new("app.auth_session_revoke_by_sid", "UPDATE session SET revoked = true WHERE sid = $sid");

pub const AUTH_SESSION_REVOKE: ControlStmt = ControlStmt::new("app.auth_session_revoke", "UPDATE $id SET revoked = true");

pub const AUTH_SESSION_LIST: ControlStmt = ControlStmt::new(
    "app.auth_session_list",
    "SELECT id, sid, user_agent, created_at, last_seen_at FROM session \
     WHERE owner = $owner AND revoked = false ORDER BY last_seen_at DESC",
);

pub const AUTH_USER_ID_BY_EMAIL: ControlStmt =
    ControlStmt::new("app.auth_user_id_by_email", "SELECT id FROM user WHERE email_lc = $email LIMIT 1");

pub const AUTH_USER_CREATE: ControlStmt =
    ControlStmt::new("app.auth_user_create", "CREATE user SET email = $email, email_lc = $email, password_hash = $password_hash RETURN AFTER");

pub const AUTH_USER_BY_EMAIL: ControlStmt =
    ControlStmt::new("app.auth_user_by_email", "SELECT * FROM user WHERE email_lc = $email LIMIT 1");

pub const AUTH_USER_COUNT: ControlStmt = ControlStmt::new("app.auth_user_count", "SELECT count() FROM user GROUP ALL");

pub const AUTH_USER_DELETE: ControlStmt = ControlStmt::new("app.auth_user_delete", "DELETE $id");

pub const AUTH_TOKEN_DELETE: ControlStmt = ControlStmt::new(
    "app.auth_token_delete",
    "DELETE api_token WHERE id = $id AND owner = $owner RETURN BEFORE",
);

pub const AUTH_TOKEN_RENAME: ControlStmt = ControlStmt::new(
    "app.auth_token_rename",
    "UPDATE api_token SET name = $name WHERE id = $id AND owner = $owner \
     RETURN id, name, created_at, last_used_at, scopes, vault, expires_at",
);

pub const AUTH_USER_ONBOARDED: ControlStmt = ControlStmt::new("app.auth_user_onboarded", "UPDATE $id SET onboarded_at = time::now()");

pub const AUTH_TOKEN_CREATE: ControlStmt = ControlStmt::new(
    "app.auth_token_create",
    "CREATE api_token SET owner = $owner, name = $name, token_hash = $hash, scopes = $scopes, \
     vault = $vault, expires_at = $expires_at RETURN AFTER",
);

pub const AUTH_TOKEN_LIST: ControlStmt = ControlStmt::new(
    "app.auth_token_list",
    "SELECT id, name, created_at, last_used_at, scopes, vault, expires_at FROM api_token \
     WHERE owner = $owner ORDER BY created_at DESC",
);

pub const AUTH_TOKEN_BY_HASH: ControlStmt =
    ControlStmt::new(
        "app.auth_token_by_hash",
        "SELECT *, (expires_at != NONE AND expires_at <= time::now()) AS expired FROM api_token \
         WHERE token_hash = $hash LIMIT 1",
    );

pub const AUTH_TOKEN_TOUCH: ControlStmt = ControlStmt::new("app.auth_token_touch", "UPDATE $id SET last_used_at = time::now()");

pub const EXPORT_USER_EMAILS: ControlStmt = ControlStmt::new("app.export_user_emails", "SELECT id, email FROM user WHERE id IN $ids");

pub const SOURCES_USER_IDS: ControlStmt = ControlStmt::new("app.sources_user_ids", "SELECT id FROM user");

pub const OAUTH_CLIENT_GET: ControlStmt = ControlStmt::new("app.oauth_client_get", "SELECT * FROM oauth_client WHERE client_id = $client_id LIMIT 1");

pub const OAUTH_CLIENT_UPSERT: ControlStmt = ControlStmt::new(
    "app.oauth_client_upsert",
    "UPSERT $id SET client_id = $client_id, name = $name, logo_uri = $logo_uri, client_uri = $client_uri, \
     redirect_uris = $redirect_uris, kind = $kind, fetched_at = time::now(), expires_at = $expires_at",
);

/// A registered (DCR) client is dropped after 24 hours if nobody ever authorized it, and after 30 days
/// without use (`fetched_at` is bumped on every token issue) even if someone did, so open registration
/// cannot grow the table forever. Existing grants keep refreshing: refresh does not look the client up.
pub const OAUTH_CLIENT_PRUNE: ControlStmt = ControlStmt::new(
    "app.oauth_client_prune",
    "DELETE oauth_client WHERE kind = 'dcr' AND (fetched_at < time::now() - 30d \
     OR (fetched_at < time::now() - 24h AND client_id NOT IN (SELECT VALUE client_id FROM oauth_grant)))",
);

pub const OAUTH_CLIENT_TOUCH: ControlStmt =
    ControlStmt::new("app.oauth_client_touch", "UPDATE oauth_client SET fetched_at = time::now() WHERE client_id = $client_id AND kind = 'dcr'");

pub const OAUTH_CLIENT_DCR_COUNT: ControlStmt =
    ControlStmt::new("app.oauth_client_dcr_count", "SELECT count() FROM oauth_client WHERE kind = 'dcr' GROUP ALL");

pub const OAUTH_CODE_CREATE: ControlStmt = ControlStmt::new(
    "app.oauth_code_create",
    "CREATE oauth_code SET code_hash = $code_hash, owner = $owner, client_id = $client_id, redirect_uri = $redirect_uri, \
     code_challenge = $code_challenge, scope = $scope, resource = $resource, expires_at = time::now() + 60s",
);

/// Single use: the first redemption marks the row (it is kept, see `OAUTH_PRUNE`); a second finds nothing here.
pub const OAUTH_CODE_TAKE: ControlStmt = ControlStmt::new(
    "app.oauth_code_take",
    "UPDATE oauth_code SET redeemed_at = time::now() WHERE code_hash = $code_hash AND redeemed_at IS NONE RETURN BEFORE",
);

/// The marker of an already redeemed code: who owns it and which grant it produced (none if redemption failed).
pub const OAUTH_CODE_REDEEMED: ControlStmt = ControlStmt::new(
    "app.oauth_code_redeemed",
    "SELECT owner, grant_id FROM oauth_code WHERE code_hash = $code_hash AND redeemed_at IS NOT NONE LIMIT 1",
);

pub const OAUTH_CODE_LINK_GRANT: ControlStmt =
    ControlStmt::new("app.oauth_code_link_grant", "UPDATE oauth_code SET grant_id = $grant_id WHERE code_hash = $code_hash");

pub const OAUTH_GRANT_CREATE: ControlStmt = ControlStmt::new(
    "app.oauth_grant_create",
    "CREATE oauth_grant SET owner = $owner, client_id = $client_id, client_name = $client_name, \
     client_logo = $client_logo, scope = $scope, resource = $resource",
);

pub const OAUTH_GRANT_LIST: ControlStmt = ControlStmt::new(
    "app.oauth_grant_list",
    "SELECT id, client_id, client_name, client_logo, scope, created_at, last_used_at FROM oauth_grant \
     WHERE owner = $owner ORDER BY created_at DESC",
);

/// Returns the deleted row only when `$owner` owns it; callers then drop the tokens.
pub const OAUTH_GRANT_DELETE: ControlStmt =
    ControlStmt::new("app.oauth_grant_delete", "DELETE oauth_grant WHERE id = $id AND owner = $owner RETURN BEFORE");

pub const OAUTH_GRANT_TOUCH: ControlStmt = ControlStmt::new(
    "app.oauth_grant_touch",
    "UPDATE $id SET last_used_at = time::now() WHERE last_used_at IS NONE OR last_used_at < time::now() - 1m",
);

pub const OAUTH_TOKEN_CREATE: ControlStmt = ControlStmt::new(
    "app.oauth_token_create",
    "CREATE oauth_token SET kind = $kind, token_hash = $token_hash, family = $family, scope = $scope, \
     expires_at = time::now() + <duration> $ttl",
);

/// Joins the family so one query serves both bearer verification and refresh.
pub const OAUTH_TOKEN_BY_HASH: ControlStmt = ControlStmt::new(
    "app.oauth_token_by_hash",
    "SELECT id, kind, family, expires_at < time::now() AS expired, used_at, family.owner AS owner, \
     family.client_id AS client_id, (scope ?? family.scope) AS scope, family.resource AS resource \
     FROM oauth_token WHERE token_hash = $token_hash LIMIT 1",
);

/// Marks a refresh token spent. Empty result means it was already spent (a race or a replay).
pub const OAUTH_TOKEN_SPEND: ControlStmt = ControlStmt::new(
    "app.oauth_token_spend",
    "UPDATE oauth_token SET used_at = time::now() WHERE id = $id AND used_at IS NONE RETURN BEFORE",
);

pub const OAUTH_TOKEN_DELETE: ControlStmt = ControlStmt::new("app.oauth_token_delete", "DELETE $id");

pub const OAUTH_TOKENS_DELETE_FAMILY: ControlStmt =
    ControlStmt::new("app.oauth_tokens_delete_family", "DELETE oauth_token WHERE family = $family");

pub const OAUTH_PRUNE: ControlStmt = ControlStmt::new(
    "app.oauth_prune",
    "DELETE oauth_token WHERE expires_at < time::now() - 1d; DELETE oauth_code WHERE expires_at < time::now() - 10m",
);

/// Append-only: this module only ever creates audit_event rows.
pub const AUDIT_EVENT_CREATE: ControlStmt = ControlStmt::new(
    "app.audit_event_create",
    "CREATE audit_event SET user = $user, actor_kind = $actor_kind, actor_id = $actor_id, action = $action, \
     target = $target, outcome = $outcome, trace_id = $trace_id, detail = $detail",
);

/// Retention only: the one place the app deletes `audit_event` rows (see `audit::prune`).
pub const AUDIT_PRUNE: ControlStmt = ControlStmt::new("app.audit_prune", "DELETE audit_event WHERE created_at < time::now() - <duration>$age");

/// Sessions that expired, or were revoked and last seen, longer than `$age` ago.
pub const SESSION_PRUNE: ControlStmt = ControlStmt::new(
    "app.session_prune",
    "DELETE session WHERE (expires_at != NONE AND expires_at < time::now() - <duration>$age) \
     OR (revoked = true AND last_seen_at < time::now() - <duration>$age)",
);

// orgs and memberships

/// A user's home org: their oldest membership. Every credential resolves to it.
pub const ORG_OF_USER: ControlStmt =
    ControlStmt::new("control.org_of_user", "SELECT org, role FROM membership WHERE user = $user ORDER BY created_at, id LIMIT 1");

pub const ORG_FIRST_OWNER: ControlStmt = ControlStmt::new(
    "control.org_first_owner",
    "SELECT user FROM membership WHERE org = $org AND role = 'owner' ORDER BY created_at, id LIMIT 1",
);

/// Every user in an org (what the schedulers iterate, in place of "every user").
pub const ORG_MEMBERS: ControlStmt = ControlStmt::new("control.org_members", "SELECT user AS id FROM membership WHERE org = $org");

/// An invitee is looked up by email inside the inviter's org only, so an email cannot be probed across orgs.
pub const EMAIL_IN_ORG: ControlStmt = ControlStmt::new(
    "control.email_in_org",
    "SELECT id FROM user WHERE email_lc = $email AND id IN (SELECT VALUE user FROM membership WHERE org = $org) LIMIT 1",
);

/// Adds `$user` to `$org`: as its owner if it has none yet, else as a member. `slot` is unique per
/// (org, user) for members and one fixed value per org for the owner, so two signups racing for an
/// org's first owner cannot both win (the loser's commit fails and is retried, see `assign_org`).
pub const MEMBERSHIP_ADD: ControlStmt = ControlStmt::new(
    "control.membership_add",
    r#"BEGIN TRANSACTION;
        LET $role = IF array::len((SELECT VALUE id FROM membership WHERE org = $org AND role = 'owner' LIMIT 1)) = 0 { 'owner' } ELSE { 'member' };
        CREATE membership SET user = $user, org = $org, role = $role,
            slot = IF $role = 'owner' { string::concat(<string>$org, '/owner') } ELSE { string::concat(<string>$org, '/', <string>$user) }
            RETURN AFTER;
        COMMIT TRANSACTION;"#,
);

/// The self-host move inserts legacy `user` rows after the control migrations ran, so they arrive with
/// `email_lc = NONE`. Same rule as control migration 0004 (lowercased, trimmed, oldest wins, a later
/// duplicate keeps an unreachable placeholder), but only for rows still NONE, so re-running is harmless.
pub const USER_EMAIL_LC_BACKFILL: ControlStmt = ControlStmt::new(
    "control.user_email_lc_backfill",
    r#"FOR $u IN (SELECT id, email FROM user WHERE email_lc = NONE ORDER BY created_at, id) {
        LET $lc = string::lowercase(string::trim($u.email));
        UPDATE $u.id SET email_lc = IF array::len((SELECT VALUE id FROM user WHERE email_lc = $lc)) = 0 { $lc } ELSE { string::concat('duplicate:', <string>$u.id) };
    };"#,
);

/// The self-host move: every user, oldest first (the first becomes the owner).
pub const USER_IDS_OLDEST_FIRST: ControlStmt = ControlStmt::new("control.user_ids_oldest_first", "SELECT VALUE id FROM user ORDER BY created_at, id");

/// Signup cleanup: every membership of a user whose signup failed.
pub const MEMBERSHIP_DELETE_USER: ControlStmt = ControlStmt::new("control.membership_delete_user", "DELETE membership WHERE user = $user");

/// `/readyz`: the control database answers.
pub const READY_PING: ControlStmt = ControlStmt::new("app.ready_ping", "RETURN 1");

/// `/readyz`: the newest control migration applied.
pub const READY_VERSION: ControlStmt = ControlStmt::new("app.ready_version", "SELECT version FROM _migration ORDER BY version DESC LIMIT 1");
