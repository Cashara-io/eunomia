//! Statements for the org registry and the tenant routing table (control database).
//! See the module docs in store/mod.rs.

use super::ControlStmt;

pub const ALL: &[&ControlStmt] = &[&BY_ORG, &LIST, &ORG_CREATE, &ORG_NAME, &CLAIM, &UPSERT, &SET_STATE, &ORG_COUNT, &MEMBERSHIP_COUNT, &PASS_ALL, &PASS_READY, &SET_PASS];

/// Where an org's data lives and how to sign in to it.
pub const BY_ORG: ControlStmt =
    ControlStmt::new("tenant.by_org", "SELECT db, db_user, db_pass_enc, schema_version, status FROM tenant WHERE id = $id");

pub const LIST: ControlStmt = ControlStmt::new("tenant.list", "SELECT org, db, schema_version, status FROM tenant ORDER BY created_at, id");

pub const ORG_CREATE: ControlStmt = ControlStmt::new("tenant.org_create", "UPSERT $id SET name = $name RETURN AFTER");

pub const ORG_NAME: ControlStmt = ControlStmt::new("tenant.org_name", "SELECT name FROM ONLY $id");

/// First writer wins, so two processes provisioning the same org agree on one generated password.
pub const CLAIM: ControlStmt = ControlStmt::new(
    "tenant.claim",
    "INSERT IGNORE INTO tenant { id: $id, org: $org, db: $db, db_user: $db_user, db_pass_enc: $db_pass_enc, \
     schema_version: 0, status: $status } RETURN NONE",
);

pub const UPSERT: ControlStmt = ControlStmt::new(
    "tenant.upsert",
    "UPSERT $id SET org = $org, db = $db, db_user = $db_user, db_pass_enc = $db_pass_enc, \
     schema_version = $version, status = $status, updated_at = time::now()",
);

pub const SET_STATE: ControlStmt =
    // a closed workspace (its only member was erased) stays closed: a migration never reopens it
    ControlStmt::new(
        "tenant.set_state",
        "UPDATE $id SET status = IF status = 'closed' THEN status ELSE $status END, schema_version = $version, updated_at = time::now()",
    );

pub const ORG_COUNT: ControlStmt = ControlStmt::new("tenant.org_count", "SELECT count() FROM org GROUP ALL");

pub const MEMBERSHIP_COUNT: ControlStmt = ControlStmt::new("tenant.membership_count", "SELECT count() FROM membership GROUP ALL");

/// Every org's encrypted database password (boot-time key rotation only).
pub const PASS_ALL: ControlStmt = ControlStmt::new("tenant.pass_all", "SELECT id, db_pass_enc FROM tenant");

/// Ready orgs' databases and encrypted passwords (boot-time replacement of passwords stored under the empty key).
pub const PASS_READY: ControlStmt = ControlStmt::new("tenant.pass_ready", "SELECT id, db, db_pass_enc FROM tenant WHERE status = 'ready'");

pub const SET_PASS: ControlStmt = ControlStmt::new("tenant.set_pass", "UPDATE $id SET db_pass_enc = $db_pass_enc");
