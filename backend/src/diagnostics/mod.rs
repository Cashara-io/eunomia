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
const README: &str = "Eunomia support diagnostics\n\nReview every file before sending this zip to support.\n\nsummary.json and summary.md: versions, migration ledgers, opaque IDs, feature flags, redacted settings and endpoint host only. Unknown metadata is null.\nconnectors.json: status, checkpoint times, cursor character counts, ingest and permanent provider-skip counts when available, partial-fetch flags, retry delays and recent process-local sync history.\nbackend.log: recent operational metadata, up to 5000 lines or 5 MiB. Events can be dropped under contention; drop counts are included.\ncontainers/: compose service status and up to 300 privacy-filtered log lines per service, when the installed updater supports diagnostics.\nhealth.json: /healthz, readiness, last 50 job failures by code and embedding backlog counts.\n\nIntentionally excluded: credentials, tokens, emails, account and organisation names, connection labels, cursor values, requests, memory, document and chat text, provider response bodies and free-text errors. Error categories replace provider messages. Logs retain timestamps, levels, allowlisted event targets, fixed message templates and event-specific bounded integer fields only. Other messages are redacted.\n\nThe archive is streamed from a private temporary file and capped at 25 MiB, including ZIP overhead. LIMITS.txt lists any files omitted by the cap. Collection lists are also bounded; summary notes explain those limits.\nNo data is sent to support automatically.\n";

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
    files.push(("backend.log".into(), logs::LOGS.snapshot()));
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

const FILE_LIMIT: u64 = 1024 * 1024;

// These files are writable by another container. Never follow a final symlink or block on
// a pipe/device, and recheck the opened descriptor rather than trusting path metadata.
fn read_regular(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new().read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW).open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > FILE_LIMIT {
        return Err(std::io::Error::other("not a bounded regular file"));
    }
    let mut text = String::new();
    file.take(FILE_LIMIT + 1).read_to_string(&mut text)?;
    if text.len() as u64 > FILE_LIMIT { return Err(std::io::Error::other("file grew beyond limit")); }
    Ok(privacy::scrub(&text))
}

async fn read_before(path: &Path, deadline: tokio::time::Instant, read: fn(&Path) -> std::io::Result<String>) -> Option<String> {
    let path = path.to_path_buf();
    tokio::time::timeout_at(deadline, tokio::task::spawn_blocking(move || read(&path))).await.ok()?.ok()?.ok()
}
async fn read_bounded(path: &Path, deadline: tokio::time::Instant) -> Option<String> {
    read_before(path, deadline, read_regular).await
}

struct RequestMarker { path: std::path::PathBuf, nonce: String }
impl Drop for RequestMarker {
    fn drop(&mut self) {
        // Drop must never perform IO on an async worker or hold up release of BUNDLE.
        let path = self.path.clone();
        let nonce = self.nonce.clone();
        tokio::task::spawn_blocking(move || {
            if read_regular(&path).ok().as_deref() == Some(&nonce) { let _ = std::fs::remove_file(path); }
        });
    }
}

pub async fn container_files(dir: &Path, wait: Duration) -> Vec<(String, String)> {
    let deadline = tokio::time::Instant::now() + wait;
    match tokio::time::timeout_at(deadline, collect_containers(dir, deadline)).await {
        Ok(files) => files,
        Err(_) => container_note("Container logs unavailable or incomplete: collection reached its deadline. The updater may be stopped, busy, or from 2.2.0 or earlier. The rest of this bundle is complete."),
    }
}
fn container_note(message: &str) -> Vec<(String, String)> { vec![("containers/README.txt".into(), message.into())] }

async fn collect_containers(dir: &Path, deadline: tokio::time::Instant) -> Vec<(String, String)> {
    if !tokio::fs::metadata(dir).await.is_ok_and(|m| m.is_dir()) { return container_note("Container logs unavailable: the updater status directory is not mounted. The rest of this bundle is complete."); }
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let path = dir.join("diagnostics-requested");
    let _marker = RequestMarker { path: path.clone(), nonce: nonce.clone() };
    let temp = dir.join(format!(".diagnostics-{nonce}"));
    if tokio::fs::write(&temp, &nonce).await.is_err() || tokio::fs::rename(&temp, &path).await.is_err() {
        let _ = tokio::fs::remove_file(&temp).await;
        return container_note("Container logs unavailable: could not write the updater request. Check directory permissions.");
    }
    let output = dir.join("diagnostics");
    loop {
        if read_bounded(&output.join("ready"), deadline).await.as_deref() == Some(&nonce) {
            let mut files = Vec::new();
            let mut omitted = Vec::new();
            for name in ["compose-ps.json", "collection.json", "backend.log", "frontend.log", "surrealdb.log", "updater.log", "backup.log", "caddy.log", "secrets-helper.log", "surreal-upgrade.log"] {
                if let Some(text) = read_bounded(&output.join(name), deadline).await { files.push((format!("containers/{name}"), if name.ends_with(".log") { logs::filter_container(&text) } else { text })); }
                else { omitted.push(name); }
            }
            files.extend(container_note(&format!("Container logs keep only allowlisted event templates and bounded integer fields. Other messages are redacted. Collected by the installed release's updater. Files omitted because they were unavailable, unsafe, oversized or exceeded the collection deadline: {}.", omitted.join(", "))));
            return files;
        }
        tokio::time::sleep_until((tokio::time::Instant::now() + Duration::from_millis(25)).min(deadline)).await;
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(download))]
pub struct Doc;

#[cfg(feature = "test-support")]
pub use archive::build as test_archive;

#[cfg(test)]
mod io_tests {
    use super::*;
    use std::os::unix::fs::symlink;
    static IO_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[tokio::test]
    async fn unsafe_files_are_omitted_with_deadline_and_next_bundle_succeeds() {
        let _test = IO_TESTS.lock().await;
        for kind in ["fifo", "symlink", "directory", "huge"] {
            for name in ["ready", "backend.log"] {
                let dir = tempfile::tempdir().unwrap();
                let out = dir.path().join("diagnostics");
                std::fs::create_dir(&out).unwrap();
                let bad = out.join(name);
                match kind {
                    "fifo" => { assert!(std::process::Command::new("mkfifo").arg(&bad).status().unwrap().success()); }
                    "symlink" => {
                        let target = dir.path().join("private-fixture");
                        std::fs::write(&target, "SYMLINK_CANARY").unwrap();
                        symlink(target, &bad).unwrap();
                    }
                    "directory" => std::fs::create_dir(&bad).unwrap(),
                    "huge" => std::fs::File::create(&bad).unwrap().set_len(FILE_LIMIT + 1).unwrap(),
                    _ => unreachable!(),
                }
                assert!(read_bounded(&bad, tokio::time::Instant::now() + Duration::from_millis(200)).await.is_none(), "accepted {kind}");
                let root = dir.path().to_path_buf();
                let writer = tokio::spawn(async move {
                    if name == "backend.log" {
                        let request = root.join("diagnostics-requested");
                        while !request.exists() { tokio::task::yield_now().await; }
                        let nonce = tokio::fs::read_to_string(request).await.unwrap();
                        tokio::fs::write(root.join("diagnostics/ready"), nonce).await.unwrap();
                    }
                });
                let start = Instant::now();
                let guard = BUNDLE.lock().await;
                let files = container_files(dir.path(), Duration::from_millis(100)).await;
                drop(guard);
                writer.await.unwrap();
                assert!(start.elapsed() < Duration::from_secs(1), "{kind} blocked collection");
                assert!(files.iter().any(|(n,t)| n == "containers/README.txt" && (t.contains("omitted") || t.contains("deadline"))));
                let (_archive, _) = archive::build(files, archive::SIZE_CAP).unwrap();
                // Same process-wide lock as download, and a second collection produces a bundle.
                let second = BUNDLE.try_lock().expect("bundle lock leaked");
                let files = container_files(&dir.path().join("missing"), Duration::from_millis(100)).await;
                archive::build(files, archive::SIZE_CAP).unwrap();
                drop(second);
            }
        }
    }

    fn slow_read(_: &Path) -> std::io::Result<String> {
        std::thread::sleep(Duration::from_millis(250));
        Ok("late".into())
    }
    fn panic_read(_: &Path) -> std::io::Result<String> { panic!("simulated filesystem failure") }

    #[tokio::test]
    async fn slow_or_panicking_reads_release_bundle_lock() {
        let _test = IO_TESTS.lock().await;
        for read in [slow_read as fn(&Path) -> std::io::Result<String>, panic_read] {
            let guard = BUNDLE.lock().await;
            let start = Instant::now();
            let text = read_before(Path::new("unused"), tokio::time::Instant::now() + Duration::from_millis(20), read).await;
            let files = text.map(|t| vec![("containers/backend.log".into(), t)]).unwrap_or_else(|| container_note("Container log omitted: read failed or reached the deadline."));
            drop(guard);
            assert!(start.elapsed() < Duration::from_millis(200), "blocking read bypassed deadline");
            assert!(files[0].1.contains("omitted"));
            archive::build(files, archive::SIZE_CAP).unwrap();
            let second = BUNDLE.try_lock().expect("bundle lock leaked after read failure");
            archive::build(container_note("second bundle"), archive::SIZE_CAP).unwrap();
            drop(second);
        }
    }
}
