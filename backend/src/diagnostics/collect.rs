use serde::Deserialize;
use serde_json::{json, Value};
use surrealdb::types::{RecordId, SurrealValue};
use crate::{error::AppResult, state::AppState, store::diagnostics as q};
use crate::rid::RecordIdExt;

#[derive(Deserialize, SurrealValue)]
struct Org { org: RecordId, schema_version: i64, status: String }
#[derive(Deserialize, SurrealValue)]
struct Connection { id: RecordId, owner: RecordId, kind: String, instance: String, enabled: bool }
#[derive(Deserialize, SurrealValue)]
struct Setting { owner: RecordId, #[surreal(default)] openai_base_url: String, #[surreal(default)] sync_check_connectivity: Option<bool> }

pub async fn collect(state: &AppState) -> AppResult<Vec<(String, String)>> {
    let control_ledger: Vec<Value> = q::CONTROL_LEDGER.on(&state.control).await?.take(0)?;
    let version: String = q::version(&state.control).await?;
    let install: Vec<Value> = q::INSTALL.on(&state.control).await?.take(0)?;
    let orgs: Vec<Org> = q::ORGS.on(&state.control).await?.take(0)?;
    let mut migrations = Vec::new();
    let mut connections = Vec::new();
    let mut settings = Vec::new();
    let mut backlog = Vec::new();
    let mut notes = Vec::new();
    if orgs.len() > 1000 { notes.push("Organisation list capped at 1000"); }
    for org in orgs.into_iter().take(1000) {
        let id = super::opaque(&org.org.to_string());
        let Some(org_id) = crate::rid::key_string(org.org.key()).and_then(|key| crate::pool::OrgId::parse(&key)) else { continue; };
        let db = match state.pool.for_org(&org_id).await {
            Ok(db) => db,
            Err(_) => { migrations.push(json!({"org": id, "routing_version": org.schema_version, "ledger": null, "note": "Organisation database unavailable"})); continue; }
        };
        let ledger: Vec<Value> = q::LEDGER.on(&db).await?.take(0)?;
        migrations.push(json!({"org": id, "routing_version": org.schema_version, "ledger": ledger,
            "status": if org.status == "ready" { "ready" } else { "not_ready" }}));
        let rows: Vec<Connection> = q::CONNECTIONS.on(&db).await?.take(0)?;
        if rows.len() > 1000 { notes.push("Connection list capped at 1000 per organisation"); }
        for conn in rows.into_iter().take(1000usize.saturating_sub(connections.len())) {
            let kind = if crate::connectors::service::CONNECTOR_KINDS.contains(&conn.kind.as_str()) { conn.kind.as_str() } else { "unknown" };
            let source = if kind == "pocketai" { "heypocket" } else { kind };
            let key = if conn.instance.is_empty() { source.to_string() } else { format!("{source}~{}", conn.instance) };
            let sync_id = RecordId::from_table_key("sync_status", format!("{}:{key}", crate::sources::base::owner_key_str(&conn.owner)));
            let mut sync: Value = q::SYNC.on(&db).bind(("id", sync_id)).await?.take::<Vec<Value>>(0)?.into_iter().next().unwrap_or(json!({}));
            for field in ["consecutive_failures", "cursor_chars", "written", "skipped", "failed"] {
                sync[field] = sync[field].as_u64().map_or(Value::Null, |n| json!(n));
            }
            sync["last_error"] = super::privacy::error_summary(sync["last_error"].as_str().unwrap_or(""));
            let history_key = super::opaque(&format!("{}/{}/{}", org_id.key(), conn.owner.to_string(), key));
            let mut jobs: Vec<Value> = q::SYNC_JOBS.on(&state.control).bind(("org", org_id.key()))
                .bind(("owner", conn.owner.clone())).bind(("source", key)).await?.take(0)?;
            for job in &mut jobs {
                job["id"] = json!(super::opaque(&job["id"].to_string()));
                let status = job["status"].as_str().unwrap_or("");
                job["status"] = json!(if ["ready", "running", "done", "dead"].contains(&status) { status } else { "unknown" });
                let code = job["last_error_code"].as_str().unwrap_or("");
                job["last_error_code"] = json!(crate::error::ErrorCode::ALL.iter().find(|c| c.as_str() == code).map(|c| c.as_str()));
            }
            connections.push(json!({"org": id, "id": super::opaque(&conn.id.to_string()), "source_type": kind,
                "status": if !conn.enabled { "disabled" } else if !sync["last_error"].is_null() { "error" } else if sync["last_ok"].is_null() { "not_synced" } else { "ok" },
                "sync": sync, "recent_history": super::history::recent(&history_key), "recent_jobs": jobs}));
        }
        let rows: Vec<Setting> = q::SETTINGS.on(&db).await?.take(0)?;
        for row in rows.into_iter().take(1000usize.saturating_sub(settings.len())) {
            settings.push(json!({"org": id, "owner": super::opaque(&row.owner.to_string()), "sync_check_connectivity": row.sync_check_connectivity.unwrap_or(true),
                "model_endpoint_host": super::host(&row.openai_base_url), "values": {
                    "embedding_model": "[redacted]", "chat_model": "[redacted]", "owner_name": "[redacted]", "sync_intervals": "[redacted]",
                    "theme": "[redacted]", "openai_api_key_encrypted": "[redacted]", "observations_mission": "[redacted]", "memory_skill": "[redacted]"}}));
        }
        let memory: Vec<Value> = q::MEMORY_BACKLOG.on(&db).await?.take(0)?;
        let cache: Vec<Value> = q::CACHE_BACKLOG.on(&db).await?.take(0)?;
        backlog.push(json!({"org": id, "memory_missing_embedding_spec": memory.first().map(|v| &v["count"]).unwrap_or(&json!(0)),
            "records_missing_embedding": cache.first().map(|v| &v["count"]).unwrap_or(&json!(0))}));
    }
    if connections.len() == 1000 { notes.push("Connection metadata capped at 1000 across this instance"); }
    if settings.len() == 1000 { notes.push("Settings metadata capped at 1000 across this instance"); }
    let mut failures: Vec<Value> = q::FAILURES.on(&state.control).await?.take(0)?;
    for failure in &mut failures {
        failure["id"] = json!(super::opaque(&failure["id"].to_string()));
        // Codes and kinds are application enums. Never export arbitrary database strings.
        let code = failure["last_error_code"].as_str().unwrap_or("");
        failure["last_error_code"] = json!(crate::error::ErrorCode::ALL.iter().find(|c| c.as_str() == code).map(|c| c.as_str()).unwrap_or("unknown"));
        failure["kind"] = json!(if ["sync", "embed", "extract", "consolidate", "prune_capsules", "prune_auth", "migrate_tenant", "index_document", "supersede", "redact_backfill"].contains(&failure["kind"].as_str().unwrap_or("")) { failure["kind"].clone() } else { json!("unknown") });
    }
    let mut embedding_jobs: Vec<Value> = q::EMBED_JOBS.on(&state.control).await?.take(0)?;
    if embedding_jobs.len() > 1000 { notes.push("Embedding job groups capped at 1000"); }
    embedding_jobs.truncate(1000);
    for row in &mut embedding_jobs {
        row["org"] = row["org"].as_str().map_or(Value::Null, |id| json!(super::opaque(id)));
    }
    let summary = json!({"app_version": crate::config::APP_VERSION, "generated_at": chrono::Utc::now().to_rfc3339(),
        "install_date_estimate": install.first().map(|v| &v["created_at"]), "install_date_source": "oldest account, when available",
        "os": std::env::consts::OS, "arch": std::env::consts::ARCH, "uptime_seconds": super::START.elapsed().as_secs(),
        "surrealdb_version": version, "image_tags": null, "data_volume_free_bytes": null,
        "container_metadata": "See containers/compose-ps.json and containers/collection.json when available",
        "timezone": chrono::Local::now().format("%Z %:z").to_string(),
        "control_migrations": control_ledger, "tenant_migrations": migrations, "settings": settings,
        "server_setting_keys": ["JWT_SECRET", "SURREAL_URL", "SURREAL_USER", "SURREAL_PASS", "SURREAL_NS", "SURREAL_DB", "OPENAI_API_KEY", "OPENAI_BASE_URL", "OPENAI_CHAT_MODEL", "ENCRYPTION_KEY", "EMBEDDINGS_BACKEND", "CORS_ALLOWED_ORIGINS", "LOG_LEVEL", "UPDATE_STATUS_DIR", "BIND_ADDR", "PUBLIC_URL", "EUNOMIA_DOCUMENTS_BACKEND", "EUNOMIA_DOCUMENTS_MAX_BYTES", "EUNOMIA_CONNECTIVITY_PROBE", "AUTH_MODE", "OIDC_ISSUER", "OIDC_CLIENT_ID", "OIDC_CLIENT_SECRET", "SECRETS_HELPER_URL", "SECRETS_HELPER_KEY", "EUNOMIA_COMPOSE_PROFILES", "EUNOMIA_CHAT_MODEL_TIMEOUT_SECS", "SIGNUP", "ALLOW_PRIVATE_LLM_URL", "EUNOMIA_ROLE"],
        "server_setting_values": "[redacted]", "feature_flags": {"provisioning": cfg!(feature = "provisioning"), "secrets_helper_enabled": state.settings.secrets_helper_enabled, "documents_enabled": !state.settings.documents_backend.is_empty()},
        "model_endpoint_host": super::host(&state.settings.openai_base_url), "notes": notes});
    let ready = crate::ready_problem(state).await.is_none();
    Ok(vec![super::json_file("summary.json", summary.clone()), ("summary.md".into(), format!("# Eunomia diagnostics\n\n```json\n{}\n```\n", super::json_text(summary))),
        super::json_file("connectors.json", json!({"connections": connections, "history_note": "Up to 20 runs per connection, from the latest 2000 syncs in this process. History resets on restart. Last sync checkpoint and counts are persisted."})),
        super::json_file("health.json", json!({"healthz": crate::healthz().await.0, "readyz": {"ok": ready}, "health_note": "This version exposes /healthz and /readyz, not /health", "recent_job_failures": failures, "embedding_backlog": backlog, "embedding_jobs": embedding_jobs}))])
}
