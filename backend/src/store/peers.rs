//! Statements for peer vaults (`peers/`, migration 0028). See the module docs in store/mod.rs.
//!
//! The caller authorizes the vault (REST) or holds a verified link (the peer routes); the statements
//! filter by the vault or the link.

use super::Stmt;

pub const ALL: &[&Stmt] = &[
    &INSTANCE,
    &CREATE_SHARE,
    &SHARES_OF_VAULT,
    &CLAIM_SHARE,
    &DELETE_SHARE,
    &CREATE_LINK,
    &LINKS_OF_VAULT,
    &GUEST_LINKS,
    &SAVE_PROGRESS,
    &SET_STATUS,
    &DELETE_LINK,
    &ITEM_BY_REMOTE,
    &ITEMS_BY_LOCAL,
    &ITEM_BY_ORIGIN,
    &UPSERT_ITEM,
    &DELETE_ITEMS_OF_LOCAL,
    &DELETE_ITEM,
    &VAULT_CONTENT,
    &DELETE_RELATION,
];

/// This org's instance id, made on first use (`$new` is only taken when there is none yet).
pub const INSTANCE: Stmt = Stmt::new(
    "peers.instance",
    "UPSERT ONLY peer_instance:self SET instance_id = instance_id ?? $new RETURN VALUE instance_id",
);

pub const CREATE_SHARE: Stmt = Stmt::new(
    "peers.create_share",
    "CREATE ONLY $id SET vault = $vault, role = $role, secret_enc = $secret_enc, created_by = $user, expires_at = $expires_at RETURN AFTER",
);

/// Codes for a vault not yet used and not expired.
pub const SHARES_OF_VAULT: Stmt = Stmt::new(
    "peers.shares_of_vault",
    "SELECT id, role, created_at, expires_at FROM peer_share WHERE vault = $vault AND used_at IS NONE AND expires_at > time::now() ORDER BY created_at DESC",
);

/// Marks a code used and wipes its secret, only if it still was unused and unexpired: two joins racing
/// with one code cannot both win. Returns the row as it was before.
pub const CLAIM_SHARE: Stmt = Stmt::new(
    "peers.claim_share",
    "UPDATE $id SET used_at = time::now(), secret_enc = NONE, link = $link WHERE used_at IS NONE AND expires_at > time::now() RETURN BEFORE",
);

pub const DELETE_SHARE: Stmt = Stmt::new("peers.delete_share", "DELETE $id WHERE vault = $vault RETURN BEFORE");

pub const CREATE_LINK: Stmt = Stmt::new(
    "peers.create_link",
    "CREATE ONLY $id SET vault = $vault, direction = $direction, role = $role, key_enc = $key_enc, user = $user, \
     peer_instance = $peer_instance, peer_name = $peer_name, peer_org = $peer_org, peer_addrs = $peer_addrs, addr = $addr RETURN AFTER",
);

pub const LINKS_OF_VAULT: Stmt = Stmt::new("peers.links_of_vault", "SELECT * FROM peer_link WHERE vault = $vault ORDER BY created_at");

/// Guest links whose last try is older than `$age` (or never): the scheduler syncs them.
pub const GUEST_LINKS: Stmt = Stmt::new(
    "peers.guest_links",
    r#"SELECT VALUE id FROM peer_link WHERE direction = "guest" AND status != "revoked" AND (last_attempt_at IS NONE OR last_attempt_at < time::now() - <duration>$age)"#,
);

/// After an exchange: cursors, the address that answered, digests, counts and the status words.
pub const SAVE_PROGRESS: Stmt = Stmt::new(
    "peers.save_progress",
    "UPDATE $id SET out_cursor = $out_cursor, in_cursor = $in_cursor, addr = $addr, status = $status, last_error = $last_error, \
     last_attempt_at = time::now(), last_sync_at = IF $ok THEN time::now() ELSE last_sync_at END, \
     integrity_failures += $integrity, local_digest = $local_digest ?? local_digest, remote_digest = $remote_digest ?? remote_digest, \
     digests_match = $digests_match ?? digests_match, last_report = $report",
);

pub const SET_STATUS: Stmt = Stmt::new(
    "peers.set_status",
    "UPDATE $id SET status = $status, last_error = $last_error, last_attempt_at = time::now()",
);

/// A link and its item map.
pub const DELETE_LINK: Stmt = Stmt::new("peers.delete_link", "DELETE peer_item WHERE link = $key; DELETE $id");

pub const ITEM_BY_REMOTE: Stmt = Stmt::new("peers.item_by_remote", "SELECT * FROM peer_item WHERE link = $link AND remote = $remote LIMIT 1");

/// Every link's map row for one local item of the vault.
pub const ITEMS_BY_LOCAL: Stmt = Stmt::new("peers.items_by_local", "SELECT * FROM peer_item WHERE vault = $vault AND local = $local");

pub const ITEM_BY_ORIGIN: Stmt = Stmt::new("peers.item_by_origin", "SELECT * FROM peer_item WHERE vault = $vault AND origin = $origin LIMIT 1");

/// `$remote` and `$origin` keep what the row had when NONE.
pub const UPSERT_ITEM: Stmt = Stmt::new(
    "peers.upsert_item",
    "UPSERT $id SET link = $link, vault = $vault, kind = $kind, local = $local, remote = $remote ?? remote, origin = $origin ?? origin, \
     hash = $hash, updated_at = time::now()",
);

pub const DELETE_ITEM: Stmt = Stmt::new("peers.delete_item", "DELETE $id");

pub const DELETE_ITEMS_OF_LOCAL: Stmt = Stmt::new("peers.delete_items_of_local", "DELETE peer_item WHERE vault = $vault AND local = $local");

/// Everything of a vault that peer sync carries, for its digest: entities per table (slots 0-6, the
/// order of `entities::service::ENTITY_TABLES`), raw memories (7), relations (8) and skills (9).
pub const VAULT_CONTENT: Stmt = Stmt::new(
    "peers.vault_content",
    "SELECT id, kind, name, aliases, summary FROM person WHERE vault = $vault; \
     SELECT id, kind, name, aliases, summary FROM organisation WHERE vault = $vault; \
     SELECT id, kind, name, aliases, summary FROM location WHERE vault = $vault; \
     SELECT id, kind, name, aliases, summary FROM repository WHERE vault = $vault; \
     SELECT id, kind, name, aliases, summary FROM file WHERE vault = $vault; \
     SELECT id, kind, name, aliases, summary FROM symbol WHERE vault = $vault; \
     SELECT id, kind, name, aliases, summary FROM entity WHERE vault = $vault; \
     SELECT id, text, type, subject, subject.kind AS subject_kind, subject.name AS subject_name FROM memory WHERE vault = $vault AND type != \"observation\"; \
     SELECT id, label, in AS src, out AS dst, in.kind AS src_kind, in.name AS src_name, out.kind AS dst_kind, out.name AS dst_name FROM relates_to WHERE in.vault = $vault; \
     SELECT id, name, description, body FROM skill WHERE vault = $vault",
);

/// One relation (there is no relation delete in `entities::service`: edges otherwise go with an entity).
pub const DELETE_RELATION: Stmt = Stmt::new("peers.delete_relation", "DELETE $id");
