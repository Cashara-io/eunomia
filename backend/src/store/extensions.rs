//! Statements for extensions and their webhook buffer (docs/extensions.md). See the module docs in store/mod.rs.
//!
//! Every statement names the owner, as the other tenant statements do: the extension row is checked
//! once by `GET`, and the event statements repeat the owner so a mistake in one cannot cross users.

use super::Stmt;

pub const ALL: &[&Stmt] = &[
    &CREATE,
    &GET,
    &LIST,
    &UPDATE,
    &ROTATE,
    &DELETE_EVENTS,
    &DELETE,
    &COUNTS,
    &LAST_DELIVERY,
    &OPEN_COUNT,
    &INSERT_EVENT,
    &PEEK,
    &CLAIM,
    &ACK,
    &NACK,
    &LIST_EVENTS,
    &PRUNE,
];

pub const CREATE: Stmt = Stmt::new(
    "extensions.create",
    "CREATE $id SET owner = $owner, name = $name, description = $description, vault = $vault, \
     secret_enc = $secret_enc, secret_hint = $secret_hint RETURN AFTER",
);

pub const GET: Stmt = Stmt::new("extensions.get", "SELECT * FROM extension WHERE id = $id AND owner = $owner");

pub const LIST: Stmt = Stmt::new("extensions.list", "SELECT * FROM extension WHERE owner = $owner ORDER BY created_at, id");

pub const UPDATE: Stmt = Stmt::new(
    "extensions.update",
    "UPDATE $id SET name = $name, description = $description, vault = $vault, enabled = $enabled, updated_at = time::now() \
     WHERE owner = $owner RETURN AFTER",
);

/// A new secret replaces the old one at once; the old one stops verifying with this statement.
pub const ROTATE: Stmt = Stmt::new(
    "extensions.rotate",
    "UPDATE $id SET secret_enc = $secret_enc, secret_hint = $secret_hint, secret_rotated_at = time::now(), updated_at = time::now() \
     WHERE owner = $owner RETURN AFTER",
);

pub const DELETE_EVENTS: Stmt =
    Stmt::new("extensions.delete_events", "DELETE extension_event WHERE extension = $id AND owner = $owner RETURN NONE");

pub const DELETE: Stmt = Stmt::new("extensions.delete", "DELETE extension WHERE id = $id AND owner = $owner RETURN BEFORE");

/// Events per extension and state for one owner. A claim whose lease ran out counts as pending
/// (`lapsed`), because the next claim hands it out again.
pub const COUNTS: Stmt = Stmt::new(
    "extensions.counts",
    "SELECT extension, status, (claimed_until != NONE AND claimed_until < time::now()) AS lapsed, count() AS n \
     FROM extension_event WHERE owner = $owner GROUP BY extension, status, lapsed",
);

pub const LAST_DELIVERY: Stmt = Stmt::new(
    "extensions.last_delivery",
    "SELECT VALUE received_at FROM extension_event WHERE extension = $ext AND owner = $owner ORDER BY received_at DESC LIMIT 1",
);

/// Events that still take room in the buffer: waiting or leased.
pub const OPEN_COUNT: Stmt = Stmt::new(
    "extensions.open_count",
    "SELECT count() AS n FROM extension_event WHERE extension = $ext AND owner = $owner AND status IN ['pending', 'claimed'] GROUP ALL",
);

/// The record id is derived from the extension and the delivery id, so a redelivery inserts nothing.
pub const INSERT_EVENT: Stmt = Stmt::new(
    "extensions.insert_event",
    "INSERT IGNORE INTO extension_event { id: $id, extension: $ext, owner: $owner, delivery_id: $delivery, body: $body, size: $size } RETURN VALUE id",
);

/// The oldest claimable events: waiting, or leased with the lease run out.
pub const PEEK: Stmt = Stmt::new(
    "extensions.peek",
    "SELECT id, size FROM extension_event WHERE extension = $ext AND owner = $owner \
     AND (status = 'pending' OR (status = 'claimed' AND claimed_until < time::now())) \
     ORDER BY received_at, id LIMIT $n",
);

/// One transaction: dead-letter events whose last lease ran out with no attempts left, then lease
/// the peeked ones that are still claimable. The conditional UPDATE re-checks status, so two
/// consumers racing for an event conflict on commit (retried by `tx::with_retry`) and the loser
/// re-reads: an event is leased to one consumer at a time. Same shape as `jobs.claim`.
pub const CLAIM: Stmt = Stmt::at(
    "extensions.claim",
    "BEGIN TRANSACTION; \
     UPDATE extension_event SET status = 'dead', claimed_until = NONE, last_error = 'lease expired', updated_at = time::now() \
         WHERE extension = $ext AND owner = $owner AND status = 'claimed' AND claimed_until < time::now() AND attempts >= $max_attempts; \
     UPDATE $ids SET status = 'claimed', claimed_until = time::now() + <duration>$lease, attempts += 1, updated_at = time::now() \
         WHERE extension = $ext AND owner = $owner AND attempts < $max_attempts \
         AND (status = 'pending' OR (status = 'claimed' AND claimed_until < time::now())) RETURN AFTER; \
     COMMIT TRANSACTION;",
    2, // BEGIN 0, dead-letter 1, lease 2
);

pub const ACK: Stmt = Stmt::new(
    "extensions.ack",
    "UPDATE $ids SET status = 'done', claimed_until = NONE, updated_at = time::now() \
     WHERE extension = $ext AND owner = $owner AND status = 'claimed' RETURN VALUE id",
);

/// Back to pending, or dead when the attempts are used up.
pub const NACK: Stmt = Stmt::new(
    "extensions.nack",
    "UPDATE $ids SET status = IF attempts >= $max_attempts THEN 'dead' ELSE 'pending' END, claimed_until = NONE, \
         last_error = $error, updated_at = time::now() \
     WHERE extension = $ext AND owner = $owner AND status = 'claimed' RETURN id, status",
);

pub const LIST_EVENTS: Stmt = Stmt::new(
    "extensions.list_events",
    "SELECT * FROM extension_event WHERE extension = $ext AND owner = $owner AND ($status = NONE OR status = $status) \
     ORDER BY received_at DESC, id LIMIT $limit",
);

/// Housekeeping across the org database: finished and dead events past their retention.
pub const PRUNE: Stmt = Stmt::new(
    "extensions.prune",
    "DELETE extension_event WHERE (status = 'done' AND updated_at < time::now() - <duration>$done_age) \
     OR (status = 'dead' AND updated_at < time::now() - <duration>$dead_age) RETURN NONE",
);
