//! Statements for secret requests and the secrets Eunomia holds (`secrets::requests`, `secrets::held`,
//! migration 0026; docs/architecture/specs/secret-requests.md).
//!
//! Isolation's second layer (docs/architecture/tenancy.md): every statement that reads or changes a row
//! names the caller's vaults (`vault IN $vaults`), so even two orgs in one database could not see each
//! other's rows. Only [`HELD_FOR_USE`] returns `value_enc`, and only the broker's resolution and the
//! transform call it; every other statement reads the fields by name ([`held_meta!`]).

use super::{ControlStmt, Stmt};

pub const ALL: &[&Stmt] = &[
    &REQ_CREATE,
    &REQ_PENDING_SAME,
    &REQ_PENDING_COUNT,
    &REQ_GET,
    &REQ_LIST,
    &REQ_EXPIRE,
    &REQ_CLAIM,
    &REQ_UNCLAIM,
    &REQ_LINK,
    &REQ_DECLINE,
    &HELD_LIST,
    &HELD_GET,
    &HELD_BY_NAME,
    &HELD_FOR_USE,
    &HELD_CREATE,
    &HELD_CHILDREN,
    &HELD_DELETE,
    &HELD_REPLACE,
];

pub const CONTROL_ALL: &[&ControlStmt] = &[&ACTIVITY, &TOKEN_NAME, &CLIENT_NAME];

// ---------------------------------------------------------------- requests

pub const REQ_CREATE: Stmt = Stmt::new(
    "secret_requests.create",
    "CREATE secret_request SET vault = $vault, name = $name, label = $label, description = $description, \
     target_hosts = $hosts, placements = $placements, connector = $connector, requested_by = $requester, \
     actor_kind = $actor_kind, actor_id = $actor_id, client_name = $client_name, channel = $channel, \
     session = $session, replace = $replace, secret_ttl_days = $ttl_days, expires_at = $expires_at RETURN AFTER",
);

/// The same caller's pending request for the same slot (a retried call gets it back).
pub const REQ_PENDING_SAME: Stmt = Stmt::new(
    "secret_requests.pending_same",
    "SELECT * FROM secret_request WHERE vault = $vault AND name = $name AND actor_id = $actor_id AND status = 'pending' \
     AND expires_at > time::now() ORDER BY created_at DESC LIMIT 1",
);

pub const REQ_PENDING_COUNT: Stmt = Stmt::new(
    "secret_requests.pending_count",
    "SELECT count() AS n FROM secret_request WHERE requested_by = $requester AND status = 'pending' AND expires_at > time::now() \
     AND vault IN $vaults GROUP ALL",
);

pub const REQ_GET: Stmt = Stmt::new("secret_requests.get", "SELECT * FROM secret_request WHERE id = $id AND vault IN $vaults");

/// Newest first; `$status` NONE for every status.
pub const REQ_LIST: Stmt = Stmt::new(
    "secret_requests.list",
    "SELECT * FROM secret_request WHERE vault IN $vaults AND ($status = NONE OR status = $status) ORDER BY created_at DESC LIMIT 200",
);

/// Pending requests past their expiry become `expired` (on read: no job needed).
pub const REQ_EXPIRE: Stmt = Stmt::new(
    "secret_requests.expire",
    "UPDATE secret_request SET status = 'expired', decided_at = time::now() WHERE status = 'pending' AND expires_at <= time::now() \
     AND vault IN $vaults RETURN NONE",
);

/// Takes a pending, unexpired request for a fill: only one fill can win.
pub const REQ_CLAIM: Stmt = Stmt::new(
    "secret_requests.claim",
    "UPDATE $id SET status = 'filled', decided_at = time::now(), decided_by = $decider \
     WHERE status = 'pending' AND expires_at > time::now() AND vault IN $vaults RETURN AFTER",
);

/// Gives a claimed request back when storing the secret failed.
pub const REQ_UNCLAIM: Stmt = Stmt::new(
    "secret_requests.unclaim",
    "UPDATE $id SET status = 'pending', decided_at = NONE, decided_by = NONE WHERE status = 'filled' AND secret = NONE AND vault IN $vaults RETURN NONE",
);

pub const REQ_LINK: Stmt = Stmt::new("secret_requests.link", "UPDATE $id SET secret = $secret WHERE vault IN $vaults RETURN NONE");

pub const REQ_DECLINE: Stmt = Stmt::new(
    "secret_requests.decline",
    "UPDATE $id SET status = 'declined', decided_at = time::now(), decided_by = $decider \
     WHERE status = 'pending' AND vault IN $vaults RETURN AFTER",
);

// ---------------------------------------------------------------- held secrets

/// Every field of a held secret but its value.
macro_rules! held_meta {
    () => {
        "id, vault, name, label, allowed_hosts, placements, allowed_tools, allowed_actors, derived_from, op, request, \
         created_by, created_at, updated_at, expires_at"
    };
}

pub const HELD_LIST: Stmt =
    Stmt::new("secret_requests.held_list", concat!("SELECT ", held_meta!(), " FROM vault_secret WHERE vault IN $vaults ORDER BY created_at, id"));

pub const HELD_GET: Stmt = Stmt::new("secret_requests.held_get", concat!("SELECT ", held_meta!(), " FROM vault_secret WHERE id = $id AND vault IN $vaults"));

pub const HELD_BY_NAME: Stmt = Stmt::new(
    "secret_requests.held_by_name",
    concat!("SELECT ", held_meta!(), " FROM vault_secret WHERE vault = $vault AND name = $name AND vault IN $vaults"),
);

/// The one statement that reads the ciphertext: resolution and transforms only.
pub const HELD_FOR_USE: Stmt = Stmt::new(
    "secret_requests.held_for_use",
    concat!("SELECT ", held_meta!(), ", value_enc FROM vault_secret WHERE vault = $vault AND name = $name AND vault IN $vaults"),
);

pub const HELD_CREATE: Stmt = Stmt::new(
    "secret_requests.held_create",
    "CREATE vault_secret SET vault = $vault, name = $name, label = $label, value_enc = $value_enc, allowed_hosts = $hosts, \
     placements = $placements, allowed_tools = $tools, allowed_actors = $actors, derived_from = $derived_from, op = $op, \
     request = $request, created_by = $creator, expires_at = $expires_at RETURN VALUE id",
);

/// The held secrets made directly from any of `$ids`.
pub const HELD_CHILDREN: Stmt =
    Stmt::new("secret_requests.held_children", "SELECT VALUE id FROM vault_secret WHERE derived_from CONTAINSANY $ids AND vault IN $vaults");

pub const HELD_DELETE: Stmt = Stmt::new("secret_requests.held_delete", "DELETE vault_secret WHERE id IN $ids AND vault IN $vaults RETURN NONE");

pub const HELD_REPLACE: Stmt = Stmt::new(
    "secret_requests.held_replace",
    "UPDATE $id SET value_enc = $value_enc, updated_at = time::now() WHERE vault IN $vaults RETURN VALUE id",
);

// ---------------------------------------------------------------- control

/// One org's `secret.*` events, newest first (the Connectors page's resolution log).
pub const ACTIVITY: ControlStmt = ControlStmt::new(
    "control.secret_activity",
    "SELECT id, action, actor_kind, actor_id, outcome, trace_id, detail, created_at FROM audit_event \
     WHERE action IN $actions AND target = $org AND ($before = NONE OR created_at < $before) ORDER BY created_at DESC LIMIT $limit",
);

/// A personal access token's name, for the pop-up's "who asked".
pub const TOKEN_NAME: ControlStmt = ControlStmt::new("control.secret_token_name", "SELECT VALUE name FROM api_token WHERE id = $id AND owner = $owner");

/// An OAuth client's name, by its client id.
pub const CLIENT_NAME: ControlStmt = ControlStmt::new("control.secret_client_name", "SELECT VALUE name FROM oauth_client WHERE client_id = $client LIMIT 1");
