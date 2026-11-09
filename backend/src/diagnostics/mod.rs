//! Instance-admin support bundles. No customer prose crosses this boundary.
mod archive;
mod collect;
pub mod history;
pub mod logs;
pub mod privacy;

use std::{path::Path, sync::LazyLock, time::{Duration, Instant}};
use axum::{body::Body, extract::State, response::Response};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use crate::{error::{AppError, AppResult}, models_user::User, state::AppState};

pub static START: LazyLock<Instant> = LazyLock::new(Instant::now);
static BUNDLE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
const README: &str = "Eunomia support diagnostics\n\nReview every file before sending this zip to support.\n\nsummary.json and summary.md: versions, migration ledgers, opaque IDs, feature flags, redacted settings and endpoint host only. Unknown metadata is null.\nconnectors.json: status, checkpoint times, cursor character counts, result counts and recent process-local sync history.\nbackend.log: recent operational metadata, up to 5000 lines or 5 MiB.\ncontainers/: compose service status and up to 300 metadata-only log lines per service, when the installed updater supports diagnostics.\nhealth.json: /healthz, readiness, last 50 job failures by code and embedding backlog counts.\n\nIntentionally excluded: credentials, tokens, emails, account and organisation names, connection labels, cursor values, requests, memory, document and chat text, provider response bodies and free-text errors. Error categories replace provider messages. Logs retain code locations, levels and numeric measurements only.\n\nThe archive is streamed from a private temporary file and capped at 25 MiB, including ZIP overhead. LIMITS.txt lists any files omitted by the cap. Collection lists are also bounded; summary notes explain those limits.\nNo data is sent to support automatically.\n";

pub fn opaque(value: &str) -> String { hex::encode(&Sha256::digest(value.as_bytes())[..12]) }
pub fn host(value: &str) -> Option<String> { url::Url::parse(value).ok()?.host_str().map(str::to_string) }
pub fn json_text(mut value: Value) -> String {
    privacy::scrub_value(&mut value);
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "null".into())
}
pub fn json_file(name: &str, value: Value) -> (String, String) { (name.into(), json_text(value)) }

#[utoipa::path(
    operation_id = "downloadDiagnostics", post, path = "/api/diagnostics", tag = "support",
    summary = "Download a privacy-filtered support zip (instance admin, browser session)",
    responses((status = 200, description = "ZIP, capped at 25 MiB", body = String, content_type = "application/zip"),
        (status = "default", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
pub(crate) async fn download(State(state): State<AppState>, user: User) -> AppResult<Response> {
    crate::routers::update::require_admin(&state, &user, "download diagnostics. Ask your instance admin for help").await?;
    crate::authz::require_session()?;
    let _guard = BUNDLE.try_lock().map_err(|_| AppError::new(axum::http::StatusCode::CONFLICT, "Another diagnostics download is running. Try again shortly."))?;
    let mut files = tokio::time::timeout(Duration::from_secs(30), collect::collect(&state)).await
        .map_err(|_| AppError::internal("Diagnostics collection timed out. Try again when the database is available."))??;
    let containers = container_files(Path::new(&state.settings.update_status_dir), Duration::from_secs(60)).await;
    // Include container-derived metadata in both summaries too, without trusting arbitrary text.
    let mut summary: Value = serde_json::from_str(&files[0].1).unwrap_or(json!({}));
    if let Some((_, ps)) = containers.iter().find(|(name, _)| name == "containers/compose-ps.json")
        && let Ok(ps) = serde_json::from_str::<Value>(ps)
    {
        summary["image_tags"] = ps;
    }
    if let Some((_, collection)) = containers.iter().find(|(name, _)| name == "containers/collection.json")
        && let Ok(c) = serde_json::from_str::<Value>(collection)
    {
        summary["data_volume_free_bytes"] = c["data_volume_free_bytes"].as_u64().map_or(Value::Null, |n| json!(n));
    }
    files[0] = json_file("summary.json", summary.clone());
    files[1] = ("summary.md".into(), format!("# Eunomia diagnostics\n\n```json\n{}\n```\n", json_text(summary)));
    files.push(("README.txt".into(), README.into()));
    files.push(("backend.log".into(), logs::LOGS.lock().map(|l| l.snapshot()).unwrap_or_default()));
    files.extend(containers);
    let (file, capped) = tokio::task::spawn_blocking(move || archive::build(files, archive::SIZE_CAP)).await
        .map_err(|_| AppError::internal("Diagnostics archive task failed."))??;
    crate::audit::record_as_caller_detail(&state.control, &user.id, "server.diagnostics", "", "ok", if capped { "size_capped" } else { "" }).await;
    let reader = tokio::fs::File::from_std(file.reopen().map_err(|_| AppError::internal("Could not read diagnostics archive."))?);
    // Holding NamedTempFile in the stream deletes it on completion or a disconnected download.
    let stream = futures::stream::try_unfold((reader, file), |(mut reader, file)| async move {
        let mut bytes = vec![0u8; 64 * 1024];
        let n = reader.read(&mut bytes).await?;
        if n == 0 { return Ok::<_, std::io::Error>(None); }
        bytes.truncate(n);
        Ok(Some((bytes, (reader, file))))
    });
    Response::builder().header("content-type", "application/zip")
        .header("content-disposition", "attachment; filename=\"eunomia-diagnostics.zip\"")
        .header("cache-control", "no-store").header("x-diagnostics-capped", if capped { "true" } else { "false" })
        .body(Body::from_stream(stream)).map_err(|_| AppError::internal("Could not stream diagnostics."))
}

struct RequestMarker { path: std::path::PathBuf, nonce: String }
impl Drop for RequestMarker {
    fn drop(&mut self) {
        if std::fs::read_to_string(&self.path).ok().as_deref() == Some(&self.nonce) { let _ = std::fs::remove_file(&self.path); }
    }
}
async fn read_bounded(path: &Path) -> Option<String> {
    if tokio::fs::symlink_metadata(path).await.ok()?.file_type().is_symlink() { return None; }
    let file = tokio::fs::File::open(path).await.ok()?;
    let mut text = String::new();
    file.take(1024 * 1024).read_to_string(&mut text).await.ok()?;
    Some(privacy::scrub(&text))
}

pub async fn container_files(dir: &Path, wait: Duration) -> Vec<(String, String)> {
    let note = |message: &str| vec![("containers/README.txt".into(), message.into())];
    if !dir.is_dir() { return note("Container logs unavailable: the updater status directory is not mounted. The rest of this bundle is complete."); }
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let path = dir.join("diagnostics-requested");
    // Never follow a pre-existing marker symlink. Atomic replacement also avoids partial requests.
    let temp = dir.join(format!(".diagnostics-{nonce}"));
    if tokio::fs::write(&temp, &nonce).await.is_err() || tokio::fs::rename(&temp, &path).await.is_err() {
        let _ = tokio::fs::remove_file(&temp).await;
        return note("Container logs unavailable: could not write the updater request. Check directory permissions.");
    }
    let _marker = RequestMarker { path, nonce: nonce.clone() };
    let deadline = tokio::time::Instant::now() + wait;
    while tokio::time::Instant::now() < deadline {
        let output = dir.join("diagnostics");
        if read_bounded(&output.join("ready")).await.as_deref() == Some(&nonce) {
            let mut files = Vec::new();
            for name in ["compose-ps.json", "collection.json", "backend.log", "frontend.log", "surrealdb.log", "updater.log", "backup.log", "caddy.log", "secrets-helper.log", "surreal-upgrade.log"] {
                if let Some(text) = read_bounded(&output.join(name)).await { files.push((format!("containers/{name}"), text)); }
            }
            files.extend(note("Container logs contain metadata only. Free text is omitted for privacy. Empty logs can mean an unavailable service. Collected by the installed release's updater."));
            return files;
        }
        tokio::time::sleep(Duration::from_millis(250).min(wait)).await;
    }
    note("Container logs unavailable: the updater did not answer within 60 seconds. It may be stopped, busy, or from 2.2.0 or earlier. Update to a release containing diagnostics collection. The rest of this bundle is complete.")
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(download))]
pub struct Doc;

#[cfg(feature = "test-support")]
pub use archive::build as test_archive;
