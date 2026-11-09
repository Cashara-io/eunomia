//! Bounded metadata projections. Never select record content, credentials, config, payload or cursors.
use super::{ControlStmt, Stmt};
pub const CONTROL_ALL: &[&ControlStmt] = &[&ORGS, &CONTROL_LEDGER, &FAILURES, &INSTALL, &SYNC_JOBS, &EMBED_JOBS];
pub const ALL: &[&Stmt] = &[&LEDGER, &CONNECTIONS, &SYNC, &SETTINGS, &MEMORY_BACKLOG, &CACHE_BACKLOG];
pub const ORGS: ControlStmt = ControlStmt::new("diagnostics.orgs", "SELECT org, schema_version, status FROM tenant ORDER BY id LIMIT 1001");
pub const CONTROL_LEDGER: ControlStmt = ControlStmt::new("diagnostics.control_ledger", "SELECT version FROM _migration ORDER BY version");
pub const INSTALL: ControlStmt = ControlStmt::new("diagnostics.install", "SELECT created_at FROM user ORDER BY created_at LIMIT 1");
pub async fn version(db: &crate::pool::ControlDb) -> surrealdb::Result<String> {
    Ok(db.raw(super::RawKey(())).version().await?.to_string())
}
pub const FAILURES: ControlStmt = ControlStmt::new("diagnostics.failures", "SELECT id, kind, attempts, status, last_error_code, updated_at FROM job WHERE last_error_code != NONE ORDER BY updated_at DESC LIMIT 50");
pub const LEDGER: Stmt = Stmt::new("diagnostics.ledger", "SELECT version FROM _migration ORDER BY version");
pub const CONNECTIONS: Stmt = Stmt::new("diagnostics.connections", "SELECT id, owner, kind, instance, enabled FROM connector ORDER BY id LIMIT 1001");
pub const SYNC: Stmt = Stmt::new("diagnostics.sync", "SELECT last_run, last_ok, last_error, consecutive_failures, string::len(cursor) AS cursor_chars, last_report.written AS written, last_report.skipped AS skipped, last_report.failed AS failed FROM sync_status WHERE id = $id");
pub const SETTINGS: Stmt = Stmt::new("diagnostics.settings", "SELECT owner, sync_check_connectivity, openai_base_url FROM app_settings ORDER BY id LIMIT 1001");
pub const MEMORY_BACKLOG: Stmt = Stmt::new("diagnostics.memory_backlog", "SELECT count() AS count FROM memory WHERE embedding_spec IS NONE GROUP ALL");
pub const CACHE_BACKLOG: Stmt = Stmt::new("diagnostics.cache_backlog", "SELECT count() AS count FROM cache_record WHERE deleted = false AND embedding IS NONE GROUP ALL");

pub const SYNC_JOBS: ControlStmt = ControlStmt::new("diagnostics.sync_jobs", "SELECT id, status, attempts, created_at, updated_at, last_error_code FROM job WHERE org = $org AND owner = $owner AND kind = 'sync' AND payload.source = $source ORDER BY created_at DESC LIMIT 20");

pub const EMBED_JOBS: ControlStmt = ControlStmt::new("diagnostics.embed_jobs", "SELECT org, kind, status, count() AS count FROM job WHERE kind IN ['embed', 'index_document'] AND status IN ['ready', 'running', 'dead'] GROUP BY org, kind, status LIMIT 1001");
