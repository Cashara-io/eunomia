//! Statements for the control database: accounts, credentials, OAuth, the audit ledger and org
//! membership. See the module docs in store/mod.rs.

use super::ControlStmt;

pub const ALL: &[&ControlStmt] = &[
    &MEMBERSHIP_ROLE,
    &SECRET_LOG,
    &SECRET_SINCE,
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
    &AUTH_USER_BY_ID,
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
    &AUDIT_PENDING_LEDGERS,
    &AUDIT_PENDING,
    &AUDIT_PENDING_COUNT,
    &AUDIT_LEGACY_COUNT,
    &AUDIT_HEAD,
    &AUDIT_HEADS,
    &AUDIT_SEAL,
    &AUDIT_CHAIN_PAGE,
    &AUDIT_FIRST_SEQ,
    &AUDIT_FIRST_SEQ_SINCE,
    &AUDIT_LAST_SEQ_BEFORE,
    &AUDIT_CHECKPOINTS,
    &AUDIT_LATEST_CHECKPOINT,
    &AUDIT_CHECKPOINT_CREATE,
    &AUDIT_RETENTION_CUT,
    &AUDIT_EVENT_FIND,
    &ACCESS_TOKENS,
    &ACCESS_APPS,
    &ACCESS_SESSIONS,
    &TOOL_USAGE_RECORD,
    &TOOL_USAGE_PRUNE,
    &TOOL_USAGE_REPORT,
    &TOOL_USAGE_USERS,
    &TOOL_USAGE_TOKENS,
    &TOOL_USAGE_CLIENTS,
    &SESSION_PRUNE,
    &ORG_OF_USER,
    &ORG_FIRST_OWNER,
    &ORG_MEMBERS,
    &EMAIL_IN_ORG,
    &MEMBERSHIP_ADD,
    &USER_EMAIL_LC_BACKFILL,
    &USER_IDS_OLDEST_FIRST,
    &MEMBERSHIP_DELETE_USER,
    &IDENTITY_FIND,
    &IDENTITY_CREATE,
    &IDENTITY_TOUCH,
    &IDENTITY_LIST_FOR_USER,
    &IDENTITY_DELETE_USER,
    &AUTH_SESSION_REVOKE_OWNER,
    &ADMIN_AUDIT_PAGE,
    &ADMIN_PEOPLE,
    &ADMIN_LAST_SEEN,
    &ADMIN_TOKEN_NAMES,
    &ADMIN_TOKEN_COUNTS,
    &USER_ADMIN_FLAG,
    &USER_SET_ADMIN,
    &USER_SET_SUSPENDED,
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
/// `$new_exp` (so at most one extension per hour of use). `last_seen_at` moves at most once a
/// minute: a session in use does not write its row on every request.
pub const AUTH_SESSION_TOUCH: ControlStmt = ControlStmt::new(
    "app.auth_session_touch",
    "UPDATE $id SET last_seen_at = time::now(), \
     expires_at = IF expires_at = NONE OR expires_at < $threshold THEN $new_exp ELSE expires_at END \
     WHERE last_seen_at IS NONE OR last_seen_at < time::now() - 1m OR expires_at = NONE OR expires_at < $threshold",
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

pub const AUTH_USER_BY_ID: ControlStmt = ControlStmt::new("app.auth_user_by_id", "SELECT * FROM user WHERE id = $id LIMIT 1");

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

/// At most once a minute per token, like `OAUTH_GRANT_TOUCH`: a busy token shared by many clients
/// would otherwise write its row on every request.
pub const AUTH_TOKEN_TOUCH: ControlStmt = ControlStmt::new(
    "app.auth_token_touch",
    "UPDATE $id SET last_used_at = time::now() WHERE last_used_at IS NONE OR last_used_at < time::now() - 1m",
);

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
    "SELECT id, kind, family, expires_at, expires_at < time::now() AS expired, used_at, family.owner AS owner, \
     family.client_id AS client_id, family.client_name AS client_name, (scope ?? family.scope) AS scope, family.resource AS resource \
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

/// Append-only: this module only ever creates audit_event rows. The row is `pending` until
/// [`AUDIT_SEAL`] chains it (`audit::chain`); `$id` is minted by the writer, so a replay of the same
/// event is a duplicate, never a second row.
pub const AUDIT_EVENT_CREATE: ControlStmt = ControlStmt::new(
    "app.audit_event_create",
    "CREATE $id SET user = $user, actor_kind = $actor_kind, actor_id = $actor_id, action = $action, \
     target = $target, outcome = $outcome, trace_id = $trace_id, detail = $detail, detail_digest = $detail_digest, \
     digest_key = $digest_key, created_at = $created_at, ledger = $ledger, pending = true RETURN NONE",
);

/// Retention for rows written before the chain started (no `ledger`). Chained rows are cut per
/// ledger with a checkpoint ([`AUDIT_RETENTION_CUT`]).
pub const AUDIT_PRUNE: ControlStmt =
    ControlStmt::new("app.audit_prune", "DELETE audit_event WHERE ledger = NONE AND created_at < time::now() - <duration>$age");

/// Ledgers with rows waiting to be sealed (the replay pass). Backed by audit_event_pending_idx.
pub const AUDIT_PENDING_LEDGERS: ControlStmt =
    ControlStmt::new("control.audit_pending_ledgers", "SELECT ledger FROM audit_event WHERE pending = true GROUP BY ledger");

/// The oldest rows of one ledger still waiting to be sealed, in write order.
pub const AUDIT_PENDING: ControlStmt = ControlStmt::new(
    "control.audit_pending",
    "SELECT id, ledger, created_at, user, actor_kind, actor_id, action, target, outcome, trace_id, detail, detail_digest, digest_key FROM audit_event \
     WHERE pending = true AND ledger = $ledger ORDER BY created_at, id LIMIT $limit",
);

/// How many rows of one ledger wait to be sealed.
pub const AUDIT_PENDING_COUNT: ControlStmt = ControlStmt::new(
    "control.audit_pending_count",
    "SELECT count() AS n FROM audit_event WHERE pending = true AND ledger = $ledger GROUP ALL",
);

/// Rows written before the chain started (they have no ledger and no hash).
pub const AUDIT_LEGACY_COUNT: ControlStmt =
    ControlStmt::new("control.audit_legacy_count", "SELECT count() AS n FROM audit_event WHERE ledger = NONE GROUP ALL");

/// One ledger's head: the newest sealed sequence number and its hash.
pub const AUDIT_HEAD: ControlStmt =
    ControlStmt::new("control.audit_head", "SELECT ledger, seq, hash, genesis_at, updated_at FROM audit_head WHERE id = $head");

/// Every ledger's head.
pub const AUDIT_HEADS: ControlStmt =
    ControlStmt::new("control.audit_heads", "SELECT ledger, seq, hash, genesis_at, updated_at FROM audit_head ORDER BY ledger");

/// Seals `$rows` (consecutive sequence numbers, hashes computed by the caller) onto the ledger whose
/// head is still at `$expect` (-1: no head yet, so this also writes the genesis checkpoint). A head
/// that moved, or a row that is no longer pending, throws `audit.chain_moved` and nothing is written:
/// the caller re-reads and tries again. Two writers can never both extend the same head.
pub const AUDIT_SEAL: ControlStmt = ControlStmt::new(
    "control.audit_seal",
    "BEGIN TRANSACTION; \
     LET $h = (SELECT VALUE seq FROM audit_head WHERE id = $head)[0] ?? -1; \
     IF $h != $expect { THROW 'audit.chain_moved' }; \
     IF $genesis_hash != NONE { CREATE audit_checkpoint SET ledger = $ledger, seq = 0, hash = $genesis_hash, kind = 'genesis', at = $genesis_at }; \
     FOR $r IN $rows { \
         LET $u = UPDATE $r.id SET seq = $r.seq, prev_hash = $r.prev_hash, hash = $r.hash, pending = NONE WHERE pending = true RETURN id; \
         IF array::len($u) = 0 { THROW 'audit.chain_moved' }; \
     }; \
     IF $genesis_hash != NONE { CREATE $head SET ledger = $ledger, seq = $seq, hash = $hash, genesis_at = $genesis_at, updated_at = time::now() RETURN NONE } \
     ELSE { UPDATE $head SET seq = $seq, hash = $hash, updated_at = time::now() RETURN NONE }; \
     COMMIT TRANSACTION;",
);

/// Sealed rows of one ledger with `$after < seq <= $until`, in chain order.
pub const AUDIT_CHAIN_PAGE: ControlStmt = ControlStmt::new(
    "control.audit_chain_page",
    "SELECT id, ledger, seq, prev_hash, hash, created_at, user, actor_kind, actor_id, action, target, outcome, trace_id, detail, \
     detail_digest, digest_key FROM audit_event WHERE ledger = $ledger AND seq > $after AND seq <= $until ORDER BY seq LIMIT $limit",
);

/// The lowest sealed sequence number still stored for a ledger.
pub const AUDIT_FIRST_SEQ: ControlStmt = ControlStmt::new(
    "control.audit_first_seq",
    "SELECT seq FROM audit_event WHERE ledger = $ledger AND seq != NONE ORDER BY seq LIMIT 1",
);

/// The first sealed event at or after `$from` (an export's start).
pub const AUDIT_FIRST_SEQ_SINCE: ControlStmt = ControlStmt::new(
    "control.audit_first_seq_since",
    "SELECT seq FROM audit_event WHERE ledger = $ledger AND seq != NONE AND created_at >= $from ORDER BY seq LIMIT 1",
);

/// The last sealed event before `$to` (an export's end, or a retention cut).
pub const AUDIT_LAST_SEQ_BEFORE: ControlStmt = ControlStmt::new(
    "control.audit_last_seq_before",
    "SELECT seq, hash FROM audit_event WHERE ledger = $ledger AND seq != NONE AND created_at < $to ORDER BY seq DESC LIMIT 1",
);

/// One ledger's checkpoints, oldest sequence first.
pub const AUDIT_CHECKPOINTS: ControlStmt = ControlStmt::new(
    "control.audit_checkpoints",
    "SELECT ledger, seq, hash, kind, at, pruned FROM audit_checkpoint WHERE ledger = $ledger ORDER BY seq, at",
);

/// The newest checkpoint of a ledger (the periodic pass writes one when the head has moved since).
pub const AUDIT_LATEST_CHECKPOINT: ControlStmt = ControlStmt::new(
    "control.audit_latest_checkpoint",
    "SELECT ledger, seq, hash, kind, at, pruned FROM audit_checkpoint WHERE ledger = $ledger ORDER BY at DESC LIMIT 1",
);

pub const AUDIT_CHECKPOINT_CREATE: ControlStmt = ControlStmt::new(
    "control.audit_checkpoint_create",
    "CREATE audit_checkpoint SET ledger = $ledger, seq = $seq, hash = $hash, kind = $kind, at = time::now() RETURN NONE",
);

/// Retention for a chained ledger: records the last pruned event (sequence and hash) as a
/// checkpoint, then deletes it and everything before it, in one transaction. The chain stays
/// verifiable from the checkpoint.
pub const AUDIT_RETENTION_CUT: ControlStmt = ControlStmt::new(
    "control.audit_retention_cut",
    "BEGIN TRANSACTION; \
     LET $n = (SELECT count() AS n FROM audit_event WHERE ledger = $ledger AND seq != NONE AND seq <= $seq GROUP ALL)[0].n ?? 0; \
     CREATE audit_checkpoint SET ledger = $ledger, seq = $seq, hash = $hash, kind = 'retention', at = time::now(), pruned = $n; \
     DELETE audit_event WHERE ledger = $ledger AND seq != NONE AND seq <= $seq; \
     COMMIT TRANSACTION;",
);

/// The newest sealed event with this action and target in a ledger (an access review's completion).
pub const AUDIT_EVENT_FIND: ControlStmt = ControlStmt::new(
    "control.audit_event_find",
    "SELECT seq, hash, created_at, pending, detail, detail_digest, digest_key FROM audit_event WHERE action = $action AND target = $target AND ledger = $ledger \
     ORDER BY created_at DESC LIMIT 1",
);

// access reviews (routers/access_reviews.rs): who holds which credential

/// API tokens of `$ids`: names, scopes and dates, never the hash.
pub const ACCESS_TOKENS: ControlStmt = ControlStmt::new(
    "control.access_tokens",
    "SELECT id, owner, name, scopes, vault, created_at, last_used_at, expires_at FROM api_token WHERE owner IN $ids ORDER BY created_at, id",
);

/// Apps (OAuth clients) `$ids` connected, by name.
pub const ACCESS_APPS: ControlStmt = ControlStmt::new(
    "control.access_apps",
    "SELECT id, owner, client_id, client_name, scope, created_at, last_used_at FROM oauth_grant WHERE owner IN $ids ORDER BY created_at, id",
);

/// Live browser sessions per person in `$ids`.
pub const ACCESS_SESSIONS: ControlStmt = ControlStmt::new(
    "control.access_sessions",
    "SELECT owner, count() AS n FROM session WHERE owner IN $ids AND revoked = false AND (expires_at = NONE OR expires_at > time::now()) GROUP BY owner",
);

/// Counts one tool call: upserts the hour/user/credential/channel/session/tool row (its id is derived
/// from those, see `usage::row_id`) and adds to its counters in one atomic statement.
pub const TOOL_USAGE_RECORD: ControlStmt = ControlStmt::new(
    "app.tool_usage_record",
    "UPSERT $id SET org = $org, bucket = $bucket, user = $user, actor_kind = $actor_kind, actor_id = $actor_id, \
     channel = $channel, session = $sess, tool = $tool, calls = (calls ?? 0) + 1, errors = (errors ?? 0) + $errors, \
     total_ms = (total_ms ?? 0) + $ms, updated_at = time::now() RETURN NONE",
);

/// Retention only: the one place the app deletes `tool_usage` rows (see `usage::prune`).
pub const TOOL_USAGE_PRUNE: ControlStmt = ControlStmt::new("app.tool_usage_prune", "DELETE tool_usage WHERE bucket < time::now() - <duration>$age");

/// The usage report's six aggregates over one org (or one user of it) since `$since`: by person, by
/// credential, by channel, by session, by tool, and per hour.
pub const TOOL_USAGE_REPORT: ControlStmt = ControlStmt::new(
    "app.tool_usage_report",
    "SELECT user, math::sum(calls) AS calls, math::sum(errors) AS errors, math::sum(total_ms) AS total_ms FROM tool_usage \
       WHERE org = $org AND bucket >= $since AND ($user IS NONE OR user = $user) GROUP BY user; \
     SELECT user, actor_kind, actor_id, math::sum(calls) AS calls, math::sum(errors) AS errors, math::sum(total_ms) AS total_ms FROM tool_usage \
       WHERE org = $org AND bucket >= $since AND ($user IS NONE OR user = $user) GROUP BY user, actor_kind, actor_id; \
     SELECT channel, math::sum(calls) AS calls, math::sum(errors) AS errors, math::sum(total_ms) AS total_ms FROM tool_usage \
       WHERE org = $org AND bucket >= $since AND ($user IS NONE OR user = $user) GROUP BY channel; \
     SELECT user, channel, session, math::sum(calls) AS calls, math::sum(errors) AS errors, math::sum(total_ms) AS total_ms FROM tool_usage \
       WHERE org = $org AND bucket >= $since AND session != '' AND ($user IS NONE OR user = $user) GROUP BY user, channel, session; \
     SELECT tool, math::sum(calls) AS calls, math::sum(errors) AS errors, math::sum(total_ms) AS total_ms FROM tool_usage \
       WHERE org = $org AND bucket >= $since AND ($user IS NONE OR user = $user) GROUP BY tool; \
     SELECT bucket, math::sum(calls) AS calls, math::sum(errors) AS errors FROM tool_usage \
       WHERE org = $org AND bucket >= $since AND ($user IS NONE OR user = $user) GROUP BY bucket",
);

/// Names for the report's rows: the people, their API tokens and the OAuth clients behind them.
pub const TOOL_USAGE_USERS: ControlStmt = ControlStmt::new("app.tool_usage_users", "SELECT id, email FROM user WHERE id IN $ids");
pub const TOOL_USAGE_TOKENS: ControlStmt = ControlStmt::new("app.tool_usage_tokens", "SELECT id, name FROM api_token WHERE owner IN $ids");
pub const TOOL_USAGE_CLIENTS: ControlStmt = ControlStmt::new("app.tool_usage_clients", "SELECT client_id, name FROM oauth_client WHERE client_id IN $ids");

/// Sessions that expired, or were revoked and last seen, longer than `$age` ago.
pub const SESSION_PRUNE: ControlStmt = ControlStmt::new(
    "app.session_prune",
    "DELETE session WHERE (expires_at != NONE AND expires_at < time::now() - <duration>$age) \
     OR (revoked = true AND last_seen_at < time::now() - <duration>$age)",
);

// orgs and memberships

/// A user's home org: their oldest membership. Every credential resolves to it.
pub const ORG_OF_USER: ControlStmt =
    ControlStmt::new(
        "control.org_of_user",
        "SELECT org, role, user.suspended_at AS suspended_at FROM membership WHERE user = $user ORDER BY created_at, id LIMIT 1",
    );

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

/// The user a provider identity signs in as.
pub const IDENTITY_FIND: ControlStmt =
    ControlStmt::new("control.identity_find", "SELECT user FROM identity WHERE issuer = $issuer AND subject = $subject LIMIT 1");

/// Links a provider identity to a user; the unique (issuer, subject) index makes a lost race an error.
pub const IDENTITY_CREATE: ControlStmt = ControlStmt::new(
    "control.identity_create",
    "CREATE identity SET user = $user, issuer = $issuer, subject = $subject, email = $email, last_login_at = time::now()",
);

pub const IDENTITY_TOUCH: ControlStmt = ControlStmt::new(
    "control.identity_touch",
    "UPDATE identity SET last_login_at = time::now(), email = $email WHERE issuer = $issuer AND subject = $subject",
);

/// The providers a user can sign in with (Settings shows which).
pub const IDENTITY_LIST_FOR_USER: ControlStmt = ControlStmt::new("control.identity_list_for_user", "SELECT issuer FROM identity WHERE user = $user");

/// Signup cleanup: every identity of a user whose signup failed.
pub const IDENTITY_DELETE_USER: ControlStmt = ControlStmt::new("control.identity_delete_user", "DELETE identity WHERE user = $user");

/// Ends every browser session of a user (linking a provider to an existing account signs its other sessions out).
pub const AUTH_SESSION_REVOKE_OWNER: ControlStmt =
    ControlStmt::new("control.auth_session_revoke_owner", "UPDATE session SET revoked = true WHERE owner = $owner AND revoked = false");

// workspace admin (routers/admin.rs)

/// A page of the security ledger for one org, newest first: rows of its members plus, when
/// `$instance` (the caller may see what belongs to no org, `authz::sees_instance`), the anonymous
/// rows (failed sign-ins), never the per-tool-call `tool.*` rows (Usage covers those).
/// `$prefix` and `$actions` narrow it to one kind of event, `$exclude` leaves some out; empty means all.
pub const ADMIN_AUDIT_PAGE: ControlStmt = ControlStmt::new(
    "control.admin_audit_page",
    "SELECT id, user, user.email AS email, actor_kind, action, target, outcome, detail, created_at FROM audit_event \
     WHERE ((user IS NONE AND $instance = true) OR user IN (SELECT VALUE user FROM membership WHERE org = $org)) \
     AND !string::starts_with(action, 'tool.') \
     AND ($prefix = '' OR string::starts_with(action, $prefix)) \
     AND (array::len($actions) = 0 OR action IN $actions) \
     AND (array::len($exclude) = 0 OR action NOTINSIDE $exclude) \
     ORDER BY created_at DESC LIMIT $limit START $offset",
);

/// Every account in an org, oldest first.
pub const ADMIN_PEOPLE: ControlStmt = ControlStmt::new(
    "control.admin_people",
    "SELECT id, email, created_at, admin, suspended_at FROM user WHERE id IN (SELECT VALUE user FROM membership WHERE org = $org) ORDER BY created_at, id",
);

/// When each of `$ids` was last seen: browser sessions, then API tokens (the caller takes the latest).
pub const ADMIN_LAST_SEEN: ControlStmt = ControlStmt::new(
    "control.admin_last_seen",
    "SELECT owner, last_seen_at AS at FROM session WHERE owner IN $ids; \
     SELECT owner, last_used_at AS at FROM api_token WHERE owner IN $ids AND last_used_at IS NOT NONE",
);

/// Names of API tokens by id, for the audit log's sentences (a revoked token's row is gone).
pub const ADMIN_TOKEN_NAMES: ControlStmt = ControlStmt::new("control.admin_token_names", "SELECT id, name FROM api_token WHERE id IN $ids");

/// How many API tokens each of `$ids` holds (People shows the count; an owner with none has no row).
pub const ADMIN_TOKEN_COUNTS: ControlStmt =
    ControlStmt::new("control.admin_token_counts", "SELECT owner, count() AS n FROM api_token WHERE owner IN $ids GROUP BY owner");

/// Whether an admin granted this account admin rights in the app (`authz::is_admin`).
pub const USER_ADMIN_FLAG: ControlStmt = ControlStmt::new("control.user_admin_flag", "SELECT admin FROM user WHERE id = $id LIMIT 1");

/// Grants (`true`) or takes back (`NONE`) admin rights granted in the app.
pub const USER_SET_ADMIN: ControlStmt =
    ControlStmt::new("control.user_set_admin", "UPDATE user SET admin = $admin WHERE id = $id RETURN id");

/// Suspends (now) or restores an account. Suspending again keeps the first time.
pub const USER_SET_SUSPENDED: ControlStmt = ControlStmt::new(
    "control.user_set_suspended",
    "UPDATE user SET suspended_at = IF $suspended THEN (suspended_at ?? time::now()) ELSE NONE END WHERE id = $id RETURN id",
);

// the credential broker (routers/secrets.rs)

/// The caller's role in one org.
pub const MEMBERSHIP_ROLE: ControlStmt = ControlStmt::new("control.membership_role", "SELECT role FROM membership WHERE user = $user AND org = $org");

/// One org's `secret.resolve` events, newest first, older than `$before` when it is set.
/// Backed by audit_event_secret_idx (control 0007).
pub const SECRET_LOG: ControlStmt = ControlStmt::new(
    "control.secret_log",
    "SELECT id, actor_kind, actor_id, outcome, trace_id, detail, created_at FROM audit_event \
     WHERE action = 'secret.resolve' AND target = $org AND ($before = NONE OR created_at < $before) ORDER BY created_at DESC LIMIT $limit",
);

/// One org's `secret.resolve` events since `$since`, for the per-reference counts.
pub const SECRET_SINCE: ControlStmt = ControlStmt::new(
    "control.secret_since",
    "SELECT id, actor_kind, actor_id, outcome, trace_id, detail, created_at FROM audit_event \
     WHERE action = 'secret.resolve' AND target = $org AND created_at > $since",
);
