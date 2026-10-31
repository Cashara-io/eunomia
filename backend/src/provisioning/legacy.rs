//! The self-host move: a pre-tenancy install keeps everything in one database. On the first boot of
//! this version, if that database has users and `control.tenant` has no rows, copy it into the new
//! layout: one org (`Default`), every existing user a member (the first user the owner), an
//! `org_<uuid>` database holding the org tables, and the control database holding accounts,
//! credentials, OAuth, the job queue and capsules.
//!
//! Resumable: the org's `tenant` row says `moving` until the end, every copy is `INSERT IGNORE` by
//! record id, so a crash re-runs the same steps and skips what is already there. Verified: per-table
//! row counts must match (less the rows skipped as invalid, below) before the org is marked `ready`.
//!
//! Invalid rows are skipped, as before SurrealDB 3.3.4: from 3.3.4 `INSERT IGNORE` raises an ASSERT
//! failure, a type error or a THROW instead of dropping the row, which would stop the whole move on one
//! bad row. A batch that fails that way is retried row by row; each row that still fails is left out,
//! logged (table, record id and the error with any value redacted, never the row) and counted. The
//! counts go in the final log line, the returned [`MoveReport`] and a `legacy.move` audit event. Invisible to users: record ids, sessions,
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
    "symbol", "entity", "memory", "chat_thread", "chat_message", "audit_log", "embed_cache", "pocket_recording", "document", "extension", "extension_event", "skill", "change", "change_seq",
    "redact_backfill",
];
const TENANT_EDGES: &[&str] = &["linked_to", "relates_to"];

const BATCH: usize = 100;

/// A row the move left out because the new schema refused it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedRow {
    pub table: String,
    /// `table:key`, as the old database has it.
    pub id: String,
    /// SurrealDB's error with every value in it redacted (see [`redact_values`]).
    pub error: String,
}

/// What a move did. Empty for a boot with nothing to move.
#[derive(Debug, Default, Clone)]
pub struct MoveReport {
    /// Rows left out as invalid, in copy order.
    pub skipped: Vec<SkippedRow>,
}

impl MoveReport {
    /// Skipped rows per table, for the log line and the audit event.
    pub fn skipped_by_table(&self) -> std::collections::BTreeMap<&str, usize> {
        let mut m = std::collections::BTreeMap::new();
        for r in &self.skipped {
            *m.entry(r.table.as_str()).or_insert(0) += 1;
        }
        m
    }

    fn summary(&self) -> String {
        if self.skipped.is_empty() {
            return "no row skipped".into();
        }
        let per: Vec<String> = self.skipped_by_table().iter().map(|(t, n)| format!("{t}: {n}")).collect();
        format!("{} invalid rows skipped ({}); the log lists each record id and error", self.skipped.len(), per.join(", "))
    }
}

#[derive(serde::Deserialize, SurrealValue)]
struct Count {
    count: i64,
}

/// Does the move if there is one to do. A no-op on a fresh install and on every boot after the move.
/// Two replicas booting together both get here; every step is resumable, so the one that loses a
/// commit race (or finds the row the other just created) runs the whole thing again, and the second
/// pass resumes or finds it done.
pub async fn move_if_needed(p: &Provisioner, control: &ControlDb, settings: &Settings) -> AppResult<MoveReport> {
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

async fn move_once(p: &Provisioner, control: &ControlDb, settings: &Settings) -> AppResult<MoveReport> {
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
        return Ok(MoveReport::default());
    }
    // A short-lived root session for the look at the old database; dropped before the move opens its own.
    let has_users = {
        let rt = p.open().await?;
        p.legacy_has_users(&rt, &settings.surreal_db).await?
    };
    if !has_users {
        return Ok(MoveReport::default());
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

async fn run(p: &Provisioner, control: &ControlDb, settings: &Settings, org: OrgId) -> AppResult<MoveReport> {
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
    // Every run copies every table from the first id, so a resumed move counts its skipped rows afresh.
    let mut report = MoveReport::default();
    let present = tables_of(&legacy).await?;
    for table in CONTROL_COPY.iter().filter(|t| present.contains(**t)) {
        report.skipped.extend(copy_table(&legacy, &ctrl, table, false).await?);
    }
    store::control::USER_EMAIL_LC_BACKFILL.on(control).await?.check()?;
    add_memberships(control, &ctrl, &org).await?;
    for table in TENANT_COPY.iter().filter(|t| present.contains(**t)) {
        report.skipped.extend(copy_table(&legacy, &dst, table, false).await?);
    }
    for table in TENANT_EDGES.iter().filter(|t| present.contains(**t)) {
        report.skipped.extend(copy_table(&legacy, &dst, table, true).await?);
    }

    // Verify before declaring done: every row arrived or was skipped as invalid.
    let skipped = report.skipped_by_table();
    let mut mismatched = Vec::new();
    for (table, to) in CONTROL_COPY.iter().map(|t| (t, &ctrl)).chain(TENANT_COPY.iter().chain(TENANT_EDGES).map(|t| (t, &dst))) {
        let a = if present.contains(*table) { count(&legacy, table).await? } else { 0 };
        let s = skipped.get(*table).copied().unwrap_or(0) as i64;
        // the move's own audit event (written once it is done) is not a copied row
        let b = if *table == "audit_event" { count(to, "audit_event WHERE action != 'legacy.move'").await? } else { count(to, table).await? };
        if a - s != b {
            mismatched.push(format!("{table}: {a} in the old database, {s} skipped as invalid, {b} in the new"));
        }
    }
    if !mismatched.is_empty() {
        return Err(AppError::internal(format!("the data move did not verify: {}", mismatched.join("; "))));
    }
    p.write_tenant(control, &org, &db, &pass, crate::migrate::LATEST_TENANT, "ready").await?;
    let (users, mems) = (count(&ctrl, "user").await?, count(&ctrl, "membership").await?);
    let summary = report.summary();
    // After `ready`, so a resumed move never meets its own event in the copied `audit_event` count.
    // Best effort, like every audit write; the log line below says the same.
    let actor = crate::authz::Actor { kind: "system", id: "legacy-move".into() };
    let target = format!("org:{}", org.key());
    crate::audit::record(control, crate::audit::Event { user: None, actor: &actor, action: "legacy.move", target: &target, outcome: "ok", detail: &summary }).await;
    tracing::warn!(
        %org, users, memberships = mems, db = %db, skipped_rows = report.skipped.len(), skipped = %summary,
        "data move verified and complete. The old database `{old}` was kept: no row in it was copied back or deleted, but it was \
         brought to schema 8 in place (migration ledger, and entity names merged if they collided). Once you have checked the app, \
         remove it with: surreal sql --user <root> --pass <pass> --ns {ns} --hide-welcome <<< 'REMOVE DATABASE `{old}`;'",
        old = settings.surreal_db, ns = settings.surreal_ns
    );
    Ok(report)
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
/// Copy every row of `table` in id order, `BATCH` at a time, and return the rows left out as invalid.
/// Fields the new (schemafull) table no longer defines are left out first: 3.x refuses a row with an
/// unknown field (1.3+ observations carry `obs_subject`, which 2.0 dropped).
async fn copy_table(src: &crate::db::Db, dst: &crate::db::Db, table: &str, edge: bool) -> AppResult<Vec<SkippedRow>> {
    let keep = defined_fields(dst, table).await?;
    let mut dropped = std::collections::BTreeSet::new();
    let mut skipped = Vec::new();
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
        let sql = format!("INSERT {relation}IGNORE INTO {table} $rows RETURN NONE");
        // The fast path: one statement per batch. A failed statement inserts nothing (3.3.5 rolls it back).
        match insert(dst, &sql, rows.clone()).await {
            Ok(()) => {}
            Err(e) if !skippable(&e) => return Err(e.into()),
            Err(_) => {
                // Some row in the batch is invalid: find it (or them) one row at a time.
                for row in rows {
                    let id = row_id(&row);
                    match insert(dst, &sql, vec![row]).await {
                        Ok(()) => {}
                        Err(e) if !skippable(&e) => return Err(e.into()),
                        Err(e) => {
                            let error = redact_values(&e.to_string());
                            tracing::warn!(table, id = %id, error = %error, "data move: skipped a row the new schema refuses");
                            skipped.push(SkippedRow { table: table.to_string(), id, error });
                        }
                    }
                }
            }
        }
        last = Some(tail);
    }
    if !dropped.is_empty() {
        tracing::warn!(table, fields = ?dropped, "data move: left out fields this version no longer has");
    }
    if !skipped.is_empty() {
        tracing::warn!(table, skipped = skipped.len(), "data move: skipped invalid rows in this table");
    }
    Ok(skipped)
}

async fn insert(dst: &crate::db::Db, sql: &str, rows: Vec<Value>) -> surrealdb::Result<()> {
    root(dst, "legacy.insert", sql).bind(("rows", rows)).await.and_then(|r| r.check()).map(|_| ())
}

/// A row-level refusal (ASSERT, type, THROW, a unique index), which the old `INSERT IGNORE` skipped.
/// Not a commit conflict (the move's retry handles it) and not a lost connection: those stop the move,
/// so a row is never counted as invalid because the server was briefly unreachable.
fn skippable(e: &surrealdb::Error) -> bool {
    !(crate::tx::is_conflict(e) || e.is_connection())
}

fn row_id(row: &Value) -> String {
    use crate::rid::RecordIdExt;
    match row {
        Value::Object(o) => match o.get("id") {
            Some(Value::RecordId(r)) => r.to_string(),
            _ => "(no id)".into(),
        },
        _ => "(not an object)".into(),
    }
}

/// SurrealDB's errors quote the refused value ("Found 'x' for field `email`", "Expected `int` but found
/// `'x'`"), and a row can hold a secret, so every quoted string and every value after "found" becomes
/// `<value>`. Field names, record ids, types and the ASSERT clause (schema, not data) stay. Long
/// messages are cut at 300 characters.
pub fn redact_values(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut chars = msg.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // a quote that opens a value (not the apostrophe in "Couldn't")
            '\'' | '"' if !out.chars().last().is_some_and(char::is_alphanumeric) => {
                // skip to the matching quote, honouring backslash escapes
                let mut escaped = false;
                for n in chars.by_ref() {
                    if escaped {
                        escaped = false;
                    } else if n == '\\' {
                        escaped = true;
                    } else if n == c {
                        break;
                    }
                }
                out.push_str("<value>");
            }
            '`' if out.to_lowercase().ends_with("found ") => {
                for n in chars.by_ref() {
                    if n == '`' {
                        break;
                    }
                }
                out.push_str("`<value>`");
            }
            _ => out.push(c),
        }
    }
    if out.chars().count() > 300 {
        out = out.chars().take(300).collect::<String>() + "...";
    }
    out
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
    fn errors_are_logged_without_the_refused_value() {
        let type_err = "Couldn't coerce value for field `n` of `t:b`: Expected `int` but found `'hunter2'`";
        assert_eq!(redact_values(type_err), "Couldn't coerce value for field `n` of `t:b`: Expected `int` but found `<value>`");
        let assert_err = "Found 'ghp_secret' for field `e`, with record `t:e`, but field must conform to: string::is_email($value)";
        assert_eq!(redact_values(assert_err), "Found <value> for field `e`, with record `t:e`, but field must conform to: string::is_email($value)");
        let nested = r#"Expected `string` but found `{ a: "it's", b: 'x\'y' }`"#;
        assert_eq!(redact_values(nested), "Expected `string` but found `<value>`");
        let number = "Expected `string` but found `4242`";
        assert_eq!(redact_values(number), "Expected `string` but found `<value>`");
        assert!(redact_values(&"x".repeat(1000)).len() < 310);
    }

    #[test]
    fn every_org_table_is_moved() {
        let moved: std::collections::BTreeSet<&str> = TENANT_COPY.iter().chain(TENANT_EDGES).copied().collect();
        let tables: std::collections::BTreeSet<&str> = store::TENANT_TABLES.iter().copied().collect();
        assert_eq!(moved, tables, "TENANT_COPY + TENANT_EDGES must be exactly store::TENANT_TABLES");
    }
}
