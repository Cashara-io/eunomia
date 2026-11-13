//! The `subject_export` job: everything in the approved scope about one person, as a zip archive.
//!
//! The archive holds `manifest.json` (categories, counts, what was redacted and what is left out) and
//! one JSON file per category, plus `documents/<id>/<name>` for each of the person's document files.
//! Scope and redaction rules are in docs/privacy/subject-rights.md. In short:
//!
//! - Content in a space the person can no longer read is left out, even what they wrote there.
//! - In a shared space only what the person authored is included (other members' facts are theirs).
//! - A chat message that names a space the person can no longer read is replaced by a notice (#230).
//! - Other people's email addresses in the person's audit events are replaced.
//! - Credentials are never included: tokens, connected apps and sources by name only.

use std::collections::HashSet;
use std::io::Write;

use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use surrealdb::types::{Bytes, RecordId, SurrealValue};

use crate::error::{AppError, AppResult, ErrorCode};
use crate::jobs::{self, Job, JobError, NewJob};
use crate::rid::RecordIdExt;
use crate::state::{AppState, OrgState};
use crate::store::subject as q;

/// The tenant schema that has the `subject_export` table (tenant migration 0045).
pub const SCHEMA_VERSION: i64 = 45;

/// The largest archive kept (`EUNOMIA_EXPORT_MAX_MB`, default 64). A bigger one fails with a clear error.
fn max_bytes() -> usize {
    std::env::var("EUNOMIA_EXPORT_MAX_MB").ok().and_then(|v| v.trim().parse::<usize>().ok()).filter(|n| *n > 0).unwrap_or(64) * 1024 * 1024
}

/// What the exports list shows (never the archive).
#[derive(Debug, Clone, serde::Serialize, Deserialize, SurrealValue, utoipa::ToSchema)]
pub struct ExportRow {
    pub id: String,
    #[serde(default)]
    #[surreal(default)]
    pub subject: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    pub requested_by: Option<String>,
    /// `pending`, `ready` or `failed`.
    pub status: String,
    /// Categories and counts (empty until ready).
    #[serde(default)]
    #[surreal(default)]
    #[schema(value_type = Object)]
    pub manifest: Value,
    #[serde(default)]
    #[surreal(default)]
    pub size_bytes: i64,
    #[serde(default)]
    #[surreal(default)]
    pub error: Option<String>,
    #[schema(value_type = String)]
    pub created_at: surrealdb::types::Datetime,
    #[schema(value_type = String)]
    pub expires_at: surrealdb::types::Datetime,
}

pub fn require_schema(state: &OrgState) -> AppResult<()> {
    if state.db.schema_version() < SCHEMA_VERSION {
        return Err(AppError::coded(ErrorCode::TenantSchemaBehind, "This workspace is still upgrading. Try the export again shortly."));
    }
    Ok(())
}

/// Queues an export of `subject`'s data, asked for by `by` (the person, or an admin for them). The
/// row lives in the person's home workspace, so it is listed in their Settings > Data whoever asked.
/// An admin-started export is the person's: the admin sees its status and counts, and downloads it only
/// while the account is suspended, with a reason ([`archive`]). Either way the person's ledger records it.
pub async fn request_export(app: &AppState, subject: &RecordId, by: &RecordId) -> AppResult<ExportRow> {
    let home = crate::models_user::org_of(&app.control, subject).await?;
    let state = app.org(&home).await?;
    require_schema(&state)?;
    #[derive(Deserialize, SurrealValue)]
    struct Created {
        id: String,
    }
    let created: Option<Created> = q::EXPORT_CREATE.on(&state.db).bind(("u", subject.clone())).bind(("by", by.clone())).await?.take(0)?;
    let id = created.ok_or_else(|| AppError::internal("export row was not created"))?.id;
    let job = NewJob::new(jobs::kind::SUBJECT_EXPORT, by.clone(), format!("{}:{id}", jobs::kind::SUBJECT_EXPORT))
        .in_org(home)
        .payload(json!({ "export": id }));
    jobs::enqueue(&state.control, job).await?;
    if subject == by {
        crate::audit::record_as_caller(&state.control, by, "subject.export_request", &id, "ok").await;
    } else {
        // in the person's own ledger (their export lists it as started by an admin) and the admin's
        crate::audit::record_as_caller_detail(&state.control, subject, "subject.export_request_admin", &id, "ok", &format!("by={}", by.to_string())).await;
    }
    q::EXPORT_LIST
        .on(&state.db)
        .bind(("u", by.clone()))
        .await?
        .take::<Vec<ExportRow>>(0)?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| AppError::internal("export row vanished"))
}

/// The exports in `user`'s home workspace that are theirs or that they asked for.
pub async fn list_exports(state: &OrgState, user: &RecordId) -> AppResult<Vec<ExportRow>> {
    require_schema(state)?;
    Ok(q::EXPORT_LIST.on(&state.db).bind(("u", user.clone())).await?.take(0)?)
}

/// The archive of a ready export `user` may read: their own; one they asked for about someone else
/// only while that account is suspended and with a `reason` (recorded in both ledgers). Else `None`.
pub async fn archive(app: &AppState, state: &OrgState, user: &RecordId, id: &str, reason: Option<&str>) -> AppResult<Option<Vec<u8>>> {
    require_schema(state)?;
    let Ok(rid) = crate::rid::parse(id) else { return Ok(None) };
    if rid.table() != "subject_export" {
        return Ok(None);
    }
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        subject: RecordId,
        status: String,
        #[serde(default)]
        #[surreal(default)]
        archive: Option<Bytes>,
    }
    let rows: Vec<Row> = q::EXPORT_GET.on(&state.db).bind(("id", rid)).bind(("u", user.clone())).await?.take(0)?;
    let Some(row) = rows.into_iter().next() else { return Ok(None) };
    if &row.subject != user {
        #[derive(Deserialize, SurrealValue)]
        struct S {
            #[serde(default)]
            #[surreal(default)]
            suspended_at: Option<surrealdb::types::Datetime>,
        }
        let s: Option<S> = crate::store::get_control(&app.control, &row.subject).await?;
        if s.and_then(|s| s.suspended_at).is_none() {
            return Err(AppError::coded(
                ErrorCode::AuthForbidden,
                "This export belongs to the person it is about: it is in their Settings > Data. An admin can download it only while their account is suspended.",
            ));
        }
        let reason = reason.map(str::trim).filter(|r| !r.is_empty()).ok_or_else(|| AppError::bad_request("Say why you are downloading someone else's data (the reason is recorded)."))?;
        let detail = format!("by={} reason={}", user.to_string(), reason);
        crate::audit::record_as_caller_detail(&app.control, &row.subject, "subject.export_download_admin", id, "ok", &detail).await;
        crate::audit::record_as_caller_detail(&app.control, user, "subject.export_download_admin", id, "ok", &detail).await;
    }
    if row.status != "ready" {
        return Err(AppError::coded(ErrorCode::SubjectExportNotReady, "This export isn't ready yet. Check its status and try again."));
    }
    Ok(row.archive.map(|b| b.to_vec()))
}

pub async fn export_job(state: AppState, job: Job) -> Result<(), JobError> {
    let id = job.payload.get("export").and_then(Value::as_str).ok_or_else(|| JobError::permanent(ErrorCode::ValidationInvalid, "job payload has no export"))?;
    let rid = crate::rid::parse(id).map_err(|_| JobError::permanent(ErrorCode::ValidationInvalid, "bad export id"))?;
    let org = state.org(&job.org_id()?).await?;
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        subject: RecordId,
        requested_by: RecordId,
        status: String,
    }
    let rows: Vec<Row> = q::EXPORT_JOB_ROW.on(&org.db).bind(("id", rid.clone())).await.map_err(AppError::from)?.take(0).map_err(AppError::from)?;
    let Some(row) = rows.into_iter().next() else { return Ok(()) }; // expired or erased meanwhile
    if row.status != "pending" {
        return Ok(());
    }
    let finish = |status: &'static str, manifest: Value, archive: Option<Vec<u8>>, error: Option<String>| {
        let size = archive.as_ref().map_or(0, |a| a.len() as i64);
        q::EXPORT_FINISH
            .on(&org.db)
            .bind(("id", rid.clone()))
            .bind(("status", status))
            .bind(("manifest", manifest))
            .bind(("archive", archive.map(Bytes::from)))
            .bind(("size", size))
            .bind(("error", error))
    };
    let by = if row.subject == row.requested_by { "self" } else { "admin" };
    match build(&state, &row.subject, by).await {
        Ok((bytes, manifest)) if bytes.len() <= max_bytes() => {
            finish("ready", manifest, Some(bytes), None).await.map_err(AppError::from)?.check().map_err(AppError::from)?;
        }
        Ok((bytes, manifest)) => {
            let msg = format!(
                "The export is {} MB, more than this server keeps ({} MB). Ask the operator to raise EUNOMIA_EXPORT_MAX_MB.",
                bytes.len() / (1024 * 1024),
                max_bytes() / (1024 * 1024)
            );
            finish("failed", manifest, None, Some(msg)).await.map_err(AppError::from)?.check().map_err(AppError::from)?;
        }
        Err(e) => {
            finish("failed", json!({}), None, Some("The export could not be built. Try again; if it keeps failing, ask the operator to check the server log.".into()))
                .await
                .map_err(AppError::from)?
                .check()
                .map_err(AppError::from)?;
            return Err(JobError::permanent(e.code, e.source.unwrap_or(e.message)));
        }
    }
    Ok(())
}

fn vault_mention() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"vault:[A-Za-z0-9_]+").expect("valid regex"))
}

fn email_pattern() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}").expect("valid regex"))
}

/// True when `text` names a space that is not in `readable` (#230: the person no longer has access).
pub fn names_unreadable_space(text: &str, readable: &HashSet<String>) -> bool {
    vault_mention().find_iter(text).any(|m| !readable.contains(m.as_str()))
}

/// `text` with every email address but `own` replaced, and how many were.
pub fn redact_other_emails(text: &str, own: &str) -> (String, usize) {
    let mut n = 0;
    let out = email_pattern().replace_all(text, |c: &regex::Captures| {
        let m = &c[0];
        if m.eq_ignore_ascii_case(own) {
            m.to_string()
        } else {
            n += 1;
            "[another person]".to_string()
        }
    });
    (out.into_owned(), n)
}

const CHAT_NOTICE: &str = "[Left out of this export: this message came from a space you no longer have access to.]";

fn take_vec(res: &mut surrealdb::IndexedResults, i: usize) -> AppResult<Vec<Value>> {
    Ok(res.take::<Vec<Value>>(i)?)
}

type Zip = zip::ZipWriter<std::io::Cursor<Vec<u8>>>;

/// Writes one category file and lists it in the manifest.
fn put(zip: &mut Zip, categories: &mut Vec<Value>, workspace: Option<&str>, name: &str, file: &str, count: usize, value: Value) -> AppResult<()> {
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file(file, opts).map_err(|e| AppError::internal(e.to_string()))?;
    let body = serde_json::to_vec_pretty(&value).map_err(|e| AppError::internal(e.to_string()))?;
    zip.write_all(&body).map_err(|e| AppError::internal(e.to_string()))?;
    categories.push(json!({ "name": name, "workspace": workspace, "file": file, "count": count }));
    Ok(())
}

#[derive(Default)]
struct Redactions {
    unreadable_spaces: usize,
    chat: usize,
    files_included: usize,
    files_missing: usize,
}

/// One workspace's part of the archive, under `workspaces/<key>/`.
async fn workspace_part(state: &OrgState, subject: &RecordId, zip: &mut Zip, categories: &mut Vec<Value>, red: &mut Redactions) -> AppResult<()> {
    let db = &state.db;
    let key = state.db.org().key();
    let dir = format!("workspaces/{key}");
    let ws = Some(key.as_str());
    let spaces: Vec<Value> = q::MEMBERSHIPS.on(db).bind(("u", subject.clone())).await?.take(0)?;
    let readable_ids: Vec<RecordId> = spaces
        .iter()
        .filter(|m| m.get("status").and_then(Value::as_str) == Some("active"))
        .filter_map(|m| m.get("vault").and_then(Value::as_str).and_then(|v| crate::rid::parse(v).ok()))
        .collect();
    let readable: HashSet<String> = readable_ids.iter().map(|v| v.to_string()).collect();
    red.unreadable_spaces += spaces.iter().filter(|m| m.get("status").and_then(Value::as_str) != Some("active")).count();

    let mut res = q::AUTHORED.on(db).bind(("u", subject.clone())).bind(("readable", readable_ids.clone())).await?;
    let mut entities = Vec::new();
    for i in 0..7 {
        entities.extend(take_vec(&mut res, i)?);
    }
    let memories = take_vec(&mut res, 7)?;
    let relations = take_vec(&mut res, 8)?;
    let skills = take_vec(&mut res, 9)?;
    let documents = take_vec(&mut res, 10)?;

    let mut res = q::OWNED.on(db).bind(("u", subject.clone())).bind(("readable", readable_ids)).await?;
    let records = take_vec(&mut res, 0)?;
    let threads = take_vec(&mut res, 1)?;
    let mut messages = take_vec(&mut res, 2)?;
    let tool_log = take_vec(&mut res, 3)?;
    let sources = take_vec(&mut res, 4)?;
    let recordings = take_vec(&mut res, 5)?;
    let extensions = take_vec(&mut res, 6)?;

    // #230: chat that repeats a space the person can no longer read is replaced by a notice
    for m in messages.iter_mut() {
        let text = serde_json::to_string(m).unwrap_or_default();
        if names_unreadable_space(&text, &readable) {
            m["content"] = json!(CHAT_NOTICE);
            m["tool_calls"] = Value::Null;
            red.chat += 1;
        }
    }

    put(zip, categories, ws, "spaces", &format!("{dir}/spaces.json"), spaces.len(), json!(spaces))?;
    put(zip, categories, ws, "entities", &format!("{dir}/entities.json"), entities.len(), json!(entities))?;
    put(zip, categories, ws, "memories", &format!("{dir}/memories.json"), memories.len(), json!(memories))?;
    put(zip, categories, ws, "relations", &format!("{dir}/relations.json"), relations.len(), json!(relations))?;
    put(zip, categories, ws, "skills", &format!("{dir}/skills.json"), skills.len(), json!(skills))?;
    put(zip, categories, ws, "synced_records", &format!("{dir}/synced_records.json"), records.len(), json!(records))?;
    put(zip, categories, ws, "chat", &format!("{dir}/chat.json"), messages.len(), json!({ "threads": threads, "messages": messages }))?;
    put(zip, categories, ws, "tool_log", &format!("{dir}/tool_log.json"), tool_log.len(), json!(tool_log))?;
    put(zip, categories, ws, "sources", &format!("{dir}/sources.json"), sources.len(), json!(sources))?;
    put(zip, categories, ws, "recordings", &format!("{dir}/recordings.json"), recordings.len(), json!(recordings))?;
    put(zip, categories, ws, "extensions", &format!("{dir}/extensions.json"), extensions.len(), json!(extensions))?;

    // the document files themselves
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    if !state.settings.documents_backend.is_empty() {
        for d in &documents {
            let (Some(id), Some(path)) = (d.get("id").and_then(Value::as_str), d.get("object_path").and_then(Value::as_str)) else { continue };
            let name = d.get("filename").and_then(Value::as_str).unwrap_or("document");
            let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect();
            let doc_key = id.split(':').nth(1).unwrap_or(id);
            match crate::documents::get_object(db, path).await {
                Ok(Some(bytes)) => {
                    zip.start_file(format!("{dir}/documents/{doc_key}/{safe}"), opts).map_err(|e| AppError::internal(e.to_string()))?;
                    zip.write_all(&bytes).map_err(|e| AppError::internal(e.to_string()))?;
                    red.files_included += 1;
                }
                _ => red.files_missing += 1,
            }
        }
    }
    let docs_meta: Vec<Value> = documents
        .iter()
        .map(|d| {
            let mut d = d.clone();
            if let Some(o) = d.as_object_mut() {
                o.remove("object_path");
            }
            d
        })
        .collect();
    put(zip, categories, ws, "documents", &format!("{dir}/documents.json"), docs_meta.len(), json!(docs_meta))
}

/// Builds the archive and its manifest for `subject`: the account once, then every workspace they
/// belong to (the same scope as an erasure).
pub async fn build(app: &AppState, subject: &RecordId, requested_by: &str) -> AppResult<(Vec<u8>, Value)> {
    let mut res = q::ACCOUNT.on(&app.control).bind(("u", subject.clone())).await?;
    let account: Option<Value> = res.take(0)?;
    let account = account.ok_or_else(|| AppError::coded(ErrorCode::AuthNotFound, "That account doesn't exist."))?;
    let email = account.get("email").and_then(Value::as_str).unwrap_or_default().to_string();
    let identities = take_vec(&mut res, 1)?;
    let org_memberships = take_vec(&mut res, 2)?;
    let tokens = take_vec(&mut res, 3)?;
    let sessions = take_vec(&mut res, 4)?;
    let apps = take_vec(&mut res, 5)?;
    let mut audit = take_vec(&mut res, 6)?;
    let usage = take_vec(&mut res, 7)?;

    // other people's emails in the person's own audit trail
    let mut audit_redacted = 0;
    for e in audit.iter_mut() {
        if let Some(d) = e.get("detail").and_then(Value::as_str) {
            let (clean, n) = redact_other_emails(d, &email);
            audit_redacted += n;
            e["detail"] = json!(clean);
        }
    }

    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let mut categories = Vec::new();
    put(&mut zip, &mut categories, None, "account", "account.json", 1, json!({ "account": account, "sign_in_identities": identities, "organisations": org_memberships }))?;
    put(&mut zip, &mut categories, None, "api_tokens", "api_tokens.json", tokens.len(), json!(tokens))?;
    put(&mut zip, &mut categories, None, "sessions", "sessions.json", sessions.len(), json!(sessions))?;
    put(&mut zip, &mut categories, None, "connected_apps", "connected_apps.json", apps.len(), json!(apps))?;
    put(&mut zip, &mut categories, None, "audit_events", "audit_events.json", audit.len(), json!(audit))?;
    put(&mut zip, &mut categories, None, "usage", "usage.json", usage.len(), json!(usage))?;

    let mut red = Redactions::default();
    let mut workspaces = Vec::new();
    let orgs: Vec<RecordId> = q::USER_ORGS.on(&app.control).bind(("u", subject.clone())).await?.take(0)?;
    for org in orgs {
        let Some(id) = crate::rid::key_string(&org.key).and_then(|k| crate::pool::OrgId::parse(&k)) else { continue };
        match app.org(&id).await {
            Ok(state) => {
                workspace_part(&state, subject, &mut zip, &mut categories, &mut red).await?;
                workspaces.push(json!({ "workspace": id.key(), "included": true }));
            }
            Err(e) => workspaces.push(json!({ "workspace": id.key(), "included": false, "why": e.message })),
        }
    }

    let manifest = json!({
        "format": "eunomia-subject-export/2",
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "subject": { "id": subject.to_string(), "email": email },
        "requested_by": requested_by,
        "workspaces": workspaces,
        "categories": categories,
        "document_files": { "included": red.files_included, "missing": red.files_missing },
        "redactions": {
            "spaces_no_longer_readable": red.unreadable_spaces,
            "chat_messages_replaced": red.chat,
            "other_peoples_emails_in_audit_events": audit_redacted,
            "rule": "Content in spaces you can no longer read is left out. In shared spaces only what you wrote is included. Chat messages that name a space you can no longer read are replaced by a notice. Other people's email addresses in your audit events are replaced.",
        },
        "left_out": [
            "Facts, entities and documents other members wrote in shared spaces (they are theirs).",
            "Secret values: API token hashes, passwords, connector credentials, password-manager references and provider keys.",
            "Connector configuration and your settings (they hold encrypted provider keys).",
            "Embeddings (numbers derived from your text; the text itself is included).",
            "Recording audio files (transcripts and summaries are included).",
            "Backups taken before this export (see docs/privacy/subject-rights.md).",
        ],
    });
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("manifest.json", opts).map_err(|e| AppError::internal(e.to_string()))?;
    zip.write_all(&serde_json::to_vec_pretty(&manifest).map_err(|e| AppError::internal(e.to_string()))?).map_err(|e| AppError::internal(e.to_string()))?;
    let bytes = zip.finish().map_err(|e| AppError::internal(e.to_string()))?.into_inner();
    Ok((bytes, manifest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_naming_a_space_you_left_is_flagged() {
        let readable: HashSet<String> = ["vault:mine".to_string()].into_iter().collect();
        assert!(!names_unreadable_space("saved to vault:mine", &readable));
        assert!(names_unreadable_space("{\"vault\":\"vault:theirs\"}", &readable));
        assert!(!names_unreadable_space("no space here", &readable));
    }

    #[test]
    fn other_peoples_emails_are_replaced_and_your_own_kept() {
        let (out, n) = redact_other_emails("email=bob@example.com by Alice@Example.com", "alice@example.com");
        assert_eq!(out, "email=[another person] by Alice@Example.com");
        assert_eq!(n, 1);
    }
}
