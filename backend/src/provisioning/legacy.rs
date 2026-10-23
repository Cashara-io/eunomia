//! The self-host move: a pre-tenancy install keeps everything in one database. On the first boot of
//! this version, if that database has users and `control.tenant` has no rows, copy it into the new
//! layout: one org (`Default`), every existing user a member (the first user the owner), an
//! `org_<uuid>` database holding the org tables, and the control database holding accounts,
//! credentials, OAuth, the job queue and capsules.
//!
//! Resumable: the org's `tenant` row says `moving` until the end, every copy is `INSERT IGNORE` by
//! record id, so a crash re-runs the same steps and skips what is already there. Verified: per-table
//! row counts must match before the org is marked `ready`. Invisible to users: record ids, sessions,
//! tokens and OAuth grants carry over unchanged. The old database is never deleted, but it is not
//! untouched either: it is brought to tenant schema 8 in place first (ledger rows, and the entity-name
//! merge of migration 0002). The log line at the end says so and how to remove it once you have checked.

use surrealdb::types::{RecordId, SurrealValue, Value};

use super::Provisioner;
use crate::config::Settings;
use crate::error::{AppError, AppResult};
use crate::pool::{self, ControlDb, OrgId};
use crate::store::{self, root};

/// Control tables to copy, in dependency-free order (no table has a hard reference to another).
/// `job_leader` is not copied: the control migration seeds its one row.
const CONTROL_COPY: &[&str] =
    &["user", "api_token", "session", "oauth_client", "oauth_grant", "oauth_code", "oauth_token", "audit_event", "job", "failure_capsule"];

/// Org tables to copy: every table in `store::TENANT_TABLES` (a test keeps the two in step, so a new
/// org table can't be left behind). Edges come last. A table the old database never had (an install
/// from before it existed) counts as empty.
const TENANT_COPY: &[&str] = &[
    "app_settings", "vault", "vault_member", "connector", "sync_status", "cache_record", "person", "organisation", "location", "repository", "file",
    "symbol", "entity", "memory", "chat_thread", "chat_message", "audit_log", "embed_cache", "pocket_recording", "document", "extension", "extension_event",
];
const TENANT_EDGES: &[&str] = &["linked_to", "relates_to"];

const BATCH: usize = 100;

#[derive(serde::Deserialize, SurrealValue)]
struct Count {
    count: i64,
}

/// Does the move if there is one to do. A no-op on a fresh install and on every boot after the move.
/// Two replicas booting together both get here; every step is resumable, so the one that loses a
/// commit race (or finds the row the other just created) runs the whole thing again, and the second
/// pass resumes or finds it done.
pub async fn move_if_needed(p: &Provisioner, control: &ControlDb, settings: &Settings) -> AppResult<()> {
    use crate::error::ErrorCode;
    let mut attempt = 1;
    loop {
        match move_once(p, control, settings).await {
            Err(e) if attempt < crate::tx::MAX_ATTEMPTS && matches!(e.code, ErrorCode::DbConflict | ErrorCode::DbDuplicate) => {
                tracing::warn!(attempt, "another process is moving the data too; re-checking");
                tokio::time::sleep(std::time::Duration::from_millis(100 * attempt as u64)).await;
                attempt += 1;
            }
            other => return other,
        }
    }
}

async fn move_once(p: &Provisioner, control: &ControlDb, settings: &Settings) -> AppResult<()> {
    let mut res = store::dynamic_control(control, "legacy.tenants", "SELECT org, status FROM tenant").await?;
    #[derive(serde::Deserialize, SurrealValue)]
    struct T {
        org: RecordId,
        status: String,
    }
    let tenants: Vec<T> = res.take(0)?;
    if let Some(t) = tenants.iter().find(|t| t.status == "moving") {
        let org = org_of_record(&t.org)?;
        tracing::warn!(%org, "resuming an interrupted data move");
        return run(p, control, settings, org).await;
    }
    if !tenants.is_empty() {
        return Ok(());
    }
    // A short-lived root session for the look at the old database; dropped before the move opens its own.
    let has_users = {
        let rt = p.open().await?;
        p.legacy_has_users(&rt, &settings.surreal_db).await?
    };
    if !has_users {
        return Ok(());
    }
    // The move creates the org's database password; under an empty key that would be encrypted with the public zero key.
    if settings.encryption_key.is_empty() && !crate::connectors::crypto::empty_key_allowed() {
        return Err(AppError::internal(
            "ENCRYPTION_KEY is empty and this install has data to move into the org layout. Set ENCRYPTION_KEY (openssl rand -base64 32) \
             before starting; also set ENCRYPTION_KEY_LEGACY_EMPTY=1 if connector secrets were saved under the empty key. \
             EUNOMIA_ALLOW_EMPTY_ENCRYPTION_KEY=1 overrides this (dev only). See docs/deployment.md, \"Rotating ENCRYPTION_KEY\".",
        ));
    }
    tracing::warn!(db = %settings.surreal_db, "moving this install's single database into the org layout");
    run(p, control, settings, OrgId::from_label(&format!("legacy:{}:{}", settings.surreal_ns, settings.surreal_db))).await
}

fn org_of_record(r: &RecordId) -> AppResult<OrgId> {
    use crate::rid::RecordIdExt;
    crate::rid::key_string(r.key()).and_then(|k| OrgId::parse(&k)).ok_or_else(|| AppError::internal("tenant row has a bad org id"))
}

async fn run(p: &Provisioner, control: &ControlDb, settings: &Settings, org: OrgId) -> AppResult<()> {
    // One root session for the whole move, dropped when it ends.
    let rt = p.open().await?;
    let legacy = p.session(&rt, &settings.surreal_db).await?;
    // Bring a 2.x export or an older schema to the last shape the legacy database may have. This is the one
    // thing written to the old database: the `_migration` ledger, the schema up to version 8 and, if two
    // entities in a vault share a name, their merge (migration 0002). No row is copied back or deleted.
    crate::migrate::apply_up_to(&legacy, crate::migrate::LEGACY_TENANT_VERSION).await?;
    let ctrl = p.session(&rt, pool::CONTROL_DB).await?;

    // The org and its (still `moving`) routing row first, so a crash from here on resumes.
    store::tenant::ORG_CREATE.on(control).bind(("id", org.record())).bind(("name", "Default")).await?.check()?;
    let (db, pass) = p.claim_tenant(control, &org, "moving").await?;
    p.build_database(&rt, &db, &pass).await?;
    let dst = p.session(&rt, &db).await?;

    // Accounts and credentials, then membership: oldest user owns.
    let present = tables_of(&legacy).await?;
    for table in CONTROL_COPY.iter().filter(|t| present.contains(**t)) {
        copy_table(&legacy, &ctrl, table, false).await?;
    }
    store::control::USER_EMAIL_LC_BACKFILL.on(control).await?.check()?;
    add_memberships(control, &ctrl, &org).await?;
    for table in TENANT_COPY.iter().filter(|t| present.contains(**t)) {
        copy_table(&legacy, &dst, table, false).await?;
    }
    for table in TENANT_EDGES.iter().filter(|t| present.contains(**t)) {
        copy_table(&legacy, &dst, table, true).await?;
    }

    // Verify before declaring done.
    let mut mismatched = Vec::new();
    for (table, to) in CONTROL_COPY.iter().map(|t| (t, &ctrl)).chain(TENANT_COPY.iter().chain(TENANT_EDGES).map(|t| (t, &dst))) {
        let a = if present.contains(*table) { count(&legacy, table).await? } else { 0 };
        let b = count(to, table).await?;
        if a != b {
            mismatched.push(format!("{table}: {a} in the old database, {b} in the new"));
        }
    }
    if !mismatched.is_empty() {
        return Err(AppError::internal(format!("the data move did not verify: {}", mismatched.join("; "))));
    }
    p.write_tenant(control, &org, &db, &pass, crate::migrate::LATEST_TENANT, "ready").await?;
    let (users, mems) = (count(&ctrl, "user").await?, count(&ctrl, "membership").await?);
    tracing::warn!(
        %org, users, memberships = mems, db = %db,
        "data move verified and complete. The old database `{old}` was kept: no row in it was copied back or deleted, but it was \
         brought to schema 8 in place (migration ledger, and entity names merged if they collided). Once you have checked the app, \
         remove it with: surreal sql --user <root> --pass <pass> --ns {ns} --hide-welcome <<< 'REMOVE DATABASE `{old}`;'",
        old = settings.surreal_db, ns = settings.surreal_ns
    );
    Ok(())
}

/// Every copied user becomes a member of the org; the oldest is the owner. Re-runs skip existing rows.
/// Memberships go through `MEMBERSHIP_ADD`, the statement signup uses, so they get the same `slot`.
/// Copied jobs and capsules are stamped with the org too.
async fn add_memberships(control: &ControlDb, ctrl: &crate::db::Db, org: &OrgId) -> AppResult<()> {
    let users: Vec<RecordId> = store::control::USER_IDS_OLDEST_FIRST.on(control).await?.take(0)?;
    for u in users {
        let have: Vec<RecordId> = root(ctrl, "legacy.membership_exists", "SELECT VALUE id FROM membership WHERE user = $u AND org = $org")
            .bind(("u", u.clone()))
            .bind(("org", org.record()))
            .await?
            .take(0)?;
        if have.is_empty() {
            // a concurrent boot doing the same move may create it between the check and the add; the retry re-checks
            crate::tx::with_retry_dup(|| async {
                store::control::MEMBERSHIP_ADD.on(control).bind(("user", u.clone())).bind(("org", org.record())).await?.check()
            })
            .await?;
        }
    }
    root(ctrl, "legacy.stamp_org", "UPDATE job SET org = $key WHERE org = NONE; UPDATE failure_capsule SET org = $key WHERE org = NONE;")
        .bind(("key", org.key()))
        .await?
        .check()?;
    Ok(())
}

/// The tables the old database defines.
async fn tables_of(db: &crate::db::Db) -> AppResult<std::collections::HashSet<String>> {
    let info: Option<serde_json::Value> = root(db, "legacy.info_db", "INFO FOR DB").await?.take(0)?;
    Ok(info.as_ref().and_then(|i| i.get("tables")).and_then(|t| t.as_object()).map(|t| t.keys().cloned().collect()).unwrap_or_default())
}

async fn count(db: &crate::db::Db, table: &str) -> AppResult<i64> {
    let mut res = root(db, "legacy.count", format!("SELECT count() FROM {table} GROUP ALL")).await?;
    Ok(res.take::<Vec<Count>>(0)?.first().map_or(0, |c| c.count))
}

// ponytail: rows round-trip as SurrealQL values through this process, 100 at a time, and a resumed move rescans
// from the first id (INSERT IGNORE makes that safe, not fast). Fine for self-host sizes; stream or checkpoint
// the cursor per table if an install ever has millions of rows.
/// Copy every row of `table` in id order, `BATCH` at a time. Fields the new (schemafull) table no
/// longer defines are left out: 3.x refuses a row with an unknown field, and INSERT IGNORE would then
/// skip the whole row (1.3+ observations carry `obs_subject`, which 2.0 dropped).
async fn copy_table(src: &crate::db::Db, dst: &crate::db::Db, table: &str, edge: bool) -> AppResult<()> {
    let keep = defined_fields(dst, table).await?;
    let mut dropped = std::collections::BTreeSet::new();
    let mut last: Option<RecordId> = None;
    loop {
        let ids: Vec<RecordId> = match &last {
            Some(l) => root(src, "legacy.ids", format!("SELECT VALUE id FROM {table} WHERE id > $last ORDER BY id LIMIT {BATCH}")).bind(("last", l.clone())),
            None => root(src, "legacy.ids", format!("SELECT VALUE id FROM {table} ORDER BY id LIMIT {BATCH}")),
        }
        .await?
        .take(0)?;
        let Some(tail) = ids.last().cloned() else { break };
        let mut rows: Vec<Value> = root(src, "legacy.rows", "SELECT * FROM $ids").bind(("ids", ids)).await?.take(0)?;
        if let Some(keep) = &keep {
            for row in rows.iter_mut() {
                if let Value::Object(o) = row {
                    o.retain(|k, _| {
                        let ok = matches!(k.as_str(), "id" | "in" | "out") || keep.contains(k);
                        if !ok {
                            dropped.insert(k.clone());
                        }
                        ok
                    });
                }
            }
        }
        let relation = if edge { "RELATION " } else { "" };
        root(dst, "legacy.insert", format!("INSERT {relation}IGNORE INTO {table} $rows RETURN NONE")).bind(("rows", rows)).await?.check()?;
        last = Some(tail);
    }
    if !dropped.is_empty() {
        tracing::warn!(table, fields = ?dropped, "data move: left out fields this version no longer has");
    }
    Ok(())
}

/// The top-level fields `table` defines in `db`, or None when the table takes any field (not SCHEMAFULL).
async fn defined_fields(db: &crate::db::Db, table: &str) -> AppResult<Option<std::collections::HashSet<String>>> {
    let info: Option<serde_json::Value> = root(db, "legacy.info_db", "INFO FOR DB").await?.take(0)?;
    let def = info.as_ref().and_then(|i| i.pointer(&format!("/tables/{table}"))).and_then(|d| d.as_str()).unwrap_or("");
    if !def.contains("SCHEMAFULL") {
        return Ok(None);
    }
    let t: Option<serde_json::Value> = root(db, "legacy.info_table", format!("INFO FOR TABLE {table}")).await?.take(0)?;
    let fields = t.as_ref().and_then(|t| t.get("fields")).and_then(|f| f.as_object());
    Ok(Some(fields.map(|f| f.keys().map(|k| k.split(['.', '[']).next().unwrap_or(k).to_string()).collect()).unwrap_or_default()))
}

impl Provisioner {
    /// Is there an old single database holding at least one user?
    pub(super) async fn legacy_has_users(&self, rt: &crate::db::Db, db: &str) -> AppResult<bool> {
        if db == pool::CONTROL_DB {
            return Ok(false);
        }
        let ns = self.session_ns(rt).await?;
        let info: Option<serde_json::Value> = root(&ns, "legacy.info_ns", "INFO FOR NS STRUCTURE").await?.take(0)?;
        let exists = info
            .as_ref()
            .and_then(|v| v.get("databases"))
            .and_then(|d| d.as_array())
            .is_some_and(|dbs| dbs.iter().any(|d| d.get("name").and_then(|n| n.as_str()) == Some(db)));
        if !exists {
            return Ok(false);
        }
        let s = self.session(rt, db).await?;
        let info: Option<serde_json::Value> = root(&s, "legacy.info_db", "INFO FOR DB STRUCTURE").await?.take(0)?;
        let has_user_table = info
            .as_ref()
            .and_then(|v| v.get("tables"))
            .and_then(|t| t.as_array())
            .is_some_and(|ts| ts.iter().any(|t| t.get("name").and_then(|n| n.as_str()) == Some("user")));
        Ok(has_user_table && count(&s, "user").await? > 0)
    }

    async fn session_ns(&self, rt: &crate::db::Db) -> surrealdb::Result<crate::db::Db> {
        let s = rt.clone();
        s.use_ns(&self.settings.surreal_ns).await?;
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_org_table_is_moved() {
        let moved: std::collections::BTreeSet<&str> = TENANT_COPY.iter().chain(TENANT_EDGES).copied().collect();
        let tables: std::collections::BTreeSet<&str> = store::TENANT_TABLES.iter().copied().collect();
        assert_eq!(moved, tables, "TENANT_COPY + TENANT_EDGES must be exactly store::TENANT_TABLES");
    }
}
