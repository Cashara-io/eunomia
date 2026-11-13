//! The `subject_export` job: what Eunomia holds about one person, as a zip archive with a manifest.
//! Who may have which archive (docs/privacy/subject-rights.md):
//!
//! - **The person's own export** (`scope = self`): everything about them in every workspace they
//!   belong to, their personal space, private chat and synced records included. Only they start it and
//!   only they download it, never an admin, suspended or not. Each workspace's part is stored in that
//!   workspace's own database (one `subject_export` row per workspace, the same id); the download
//!   assembles them for the person.
//! - **An admin's export** (`scope = organisation`): only the admin's workspace's data about the
//!   person: what they wrote in its shared spaces, their spaces there, their account record, tokens and
//!   connected apps by name, and audit events about them. Not their personal space, private chat,
//!   synced records or any other workspace. Stored in that workspace's database; the admin downloads
//!   it. The person's own activity records that it was made.
//!
//! Archives expire after seven days and are deleted by the scheduler. Redaction rules for the person's
//! own export: content in a space they can no longer read is left out, only what they authored in
//! shared spaces is included, chat naming an unreadable space is replaced by a notice (#230), and other
//! people's emails in their audit events are replaced. Credentials are never included.

use std::collections::HashSet;
use std::io::Write;

use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use surrealdb::types::{Bytes, RecordId, SurrealValue};

use crate::error::{AppError, AppResult, ErrorCode};
use crate::jobs::{self, Job, JobError, NewJob};
use crate::pool::OrgId;
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
    /// `self` (the person's own, every workspace) or `organisation` (an admin's, one workspace).
    #[serde(default)]
    #[surreal(default)]
    pub scope: String,
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

/// Queues the person's own export (`admin` none: every workspace, kept for them) or an admin's export
/// of their workspace `admin.1` about the person (kept in that workspace, for the admin).
pub async fn request_export(app: &AppState, subject: &RecordId, admin: Option<(&RecordId, OrgId)>) -> AppResult<ExportRow> {
    let (org, by, scope) = match admin {
        None => (crate::models_user::org_of(&app.control, subject).await?, subject.clone(), "self"),
        Some((a, org)) => (org, a.clone(), "organisation"),
    };
    let state = app.org(&org).await?;
    require_schema(&state)?;
    #[derive(Deserialize, SurrealValue)]
    struct Created {
        id: String,
    }
    let created: Option<Created> = q::EXPORT_CREATE.on(&state.db).bind(("u", subject.clone())).bind(("by", by.clone())).bind(("scope", scope)).await?.take(0)?;
    let id = created.ok_or_else(|| AppError::internal("export row was not created"))?.id;
    let job = NewJob::new(jobs::kind::SUBJECT_EXPORT, by.clone(), format!("{}:{id}", jobs::kind::SUBJECT_EXPORT))
        .in_org(org)
        .payload(json!({ "export": id }));
    jobs::enqueue(&state.control, job).await?;
    if admin.is_none() {
        crate::audit::record_as_caller(&state.control, &by, "subject.export_request", &id, "ok").await;
    } else {
        // in the person's own activity (Settings > Data) and in the workspace's log, which operators see
        let admin_email = crate::models_user::email_of(&app.control, &by).await.unwrap_or_default();
        let detail = format!("by={} workspace={}", admin_email, org.key());
        crate::audit::record_as_caller_detail(&state.control, subject, "subject.export_request_admin", &id, "ok", &detail).await;
        crate::audit::record_as_caller_detail(&state.control, &by, "subject.export_request_admin", &id, "ok", &detail).await;
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

/// The exports in `user`'s home workspace listed for them: their own, and the ones they started as an admin.
pub async fn list_exports(state: &OrgState, user: &RecordId) -> AppResult<Vec<ExportRow>> {
    require_schema(state)?;
    Ok(q::EXPORT_LIST.on(&state.db).bind(("u", user.clone())).await?.take(0)?)
}

/// The archive `user` may download from their workspace `state`, else `None`: their own export
/// (assembled from its part in every workspace they still belong to) or an admin export they started.
pub async fn archive(app: &AppState, state: &OrgState, user: &RecordId, id: &str) -> AppResult<Option<(Vec<u8>, String)>> {
    require_schema(state)?;
    let Ok(rid) = crate::rid::parse(id) else { return Ok(None) };
    if rid.table() != "subject_export" {
        return Ok(None);
    }
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        #[serde(default)]
        #[surreal(default)]
        scope: String,
        status: String,
        #[serde(default)]
        #[surreal(default)]
        archive: Option<Bytes>,
    }
    let rows: Vec<Row> = q::EXPORT_GET.on(&state.db).bind(("id", rid.clone())).bind(("u", user.clone())).await?.take(0)?;
    let Some(row) = rows.into_iter().next() else { return Ok(None) };
    if row.status != "ready" {
        return Err(AppError::coded(ErrorCode::SubjectExportNotReady, "This export isn't ready yet. Check its status and try again."));
    }
    let Some(home) = row.archive else { return Ok(None) };
    if row.scope != "self" {
        return Ok(Some((home.to_vec(), row.scope)));
    }
    // the person's own: the home part, then each other workspace's part from that workspace's database
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    copy_entries(&mut out, &home.to_vec())?;
    let orgs: Vec<RecordId> = q::USER_ORGS.on(&app.control).bind(("u", user.clone())).await?.take(0)?;
    for org in orgs {
        let Some(id) = crate::rid::key_string(&org.key).and_then(|k| OrgId::parse(&k)) else { continue };
        if id == state.db.org() {
            continue;
        }
        let Ok(other) = app.org(&id).await else { continue };
        #[derive(Deserialize, SurrealValue)]
        struct Part {
            #[serde(default)]
            #[surreal(default)]
            archive: Option<Bytes>,
        }
        let part: Vec<Part> = q::EXPORT_PART_GET.on(&other.db).bind(("id", rid.clone())).bind(("u", user.clone())).await?.take(0)?;
        if let Some(bytes) = part.into_iter().next().and_then(|p| p.archive) {
            copy_entries(&mut out, &bytes.to_vec())?;
        }
    }
    let bytes = out.finish().map_err(|e| AppError::internal(e.to_string()))?.into_inner();
    Ok(Some((bytes, row.scope)))
}

/// Copies every entry of the zip `from` into `to`, as stored.
fn copy_entries(to: &mut Zip, from: &[u8]) -> AppResult<()> {
    let mut src = zip::ZipArchive::new(std::io::Cursor::new(from.to_vec())).map_err(|e| AppError::internal(e.to_string()))?;
    for i in 0..src.len() {
        let f = src.by_index_raw(i).map_err(|e| AppError::internal(e.to_string()))?;
        to.raw_copy_file(f).map_err(|e| AppError::internal(e.to_string()))?;
    }
    Ok(())
}

pub async fn export_job(state: AppState, job: Job) -> Result<(), JobError> {
    let id = job.payload.get("export").and_then(Value::as_str).ok_or_else(|| JobError::permanent(ErrorCode::ValidationInvalid, "job payload has no export"))?;
    let rid = crate::rid::parse(id).map_err(|_| JobError::permanent(ErrorCode::ValidationInvalid, "bad export id"))?;
    let org = state.org(&job.org_id()?).await?;
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        subject: RecordId,
        #[serde(default)]
        #[surreal(default)]
        scope: String,
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
    let built = if row.scope == "organisation" { build_organisation(&state, &org, &row.subject).await } else { build_own(&state, &org, &row.subject, &rid).await };
    match built {
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
    /// Other people's emails replaced in the person's audit events.
    audit: usize,
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

/// The account-level files of the person's own export: account, sign-ins, workspaces, tokens,
/// sessions, connected apps, their audit events and usage.
async fn account_part(app: &AppState, subject: &RecordId, zip: &mut Zip, categories: &mut Vec<Value>) -> AppResult<(String, usize)> {
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
    put(zip, categories, None, "account", "account.json", 1, json!({ "account": account, "sign_in_identities": identities, "organisations": org_memberships }))?;
    put(zip, categories, None, "api_tokens", "api_tokens.json", tokens.len(), json!(tokens))?;
    put(zip, categories, None, "sessions", "sessions.json", sessions.len(), json!(sessions))?;
    put(zip, categories, None, "connected_apps", "connected_apps.json", apps.len(), json!(apps))?;
    put(zip, categories, None, "audit_events", "audit_events.json", audit.len(), json!(audit))?;
    put(zip, categories, None, "usage", "usage.json", usage.len(), json!(usage))?;
    Ok((email, audit_redacted))
}

fn manifest_for(subject: &RecordId, email: &str, requested_by: &str, workspaces: Vec<Value>, categories: Vec<Value>, red: &Redactions, left_out: &[&str]) -> Value {
    json!({
        "format": "eunomia-subject-export/3",
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "subject": { "id": subject.to_string(), "email": email },
        "requested_by": requested_by,
        "workspaces": workspaces,
        "categories": categories,
        "document_files": { "included": red.files_included, "missing": red.files_missing },
        "redactions": {
            "spaces_no_longer_readable": red.unreadable_spaces,
            "chat_messages_replaced": red.chat,
            "other_peoples_emails_in_audit_events": red.audit,
        },
        "left_out": left_out,
    })
}

fn finish_zip(mut zip: Zip, manifest: &Value) -> AppResult<Vec<u8>> {
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("manifest.json", opts).map_err(|e| AppError::internal(e.to_string()))?;
    zip.write_all(&serde_json::to_vec_pretty(manifest).map_err(|e| AppError::internal(e.to_string()))?).map_err(|e| AppError::internal(e.to_string()))?;
    Ok(zip.finish().map_err(|e| AppError::internal(e.to_string()))?.into_inner())
}

const OWN_LEFT_OUT: &[&str] = &[
    "Facts, entities and documents other members wrote in shared spaces (they are theirs).",
    "Secret values: API token hashes, passwords, connector credentials, password-manager references and provider keys.",
    "Connector configuration and your settings (they hold encrypted provider keys).",
    "Embeddings (numbers derived from your text; the text itself is included).",
    "Recording audio files (transcripts and summaries are included).",
    "Backups taken before this export (see docs/privacy/subject-rights.md).",
];

/// The person's own export: the account part and the home workspace's part go in the home row (the
/// returned archive); every other workspace's part goes into that workspace's own database under the
/// same id. The manifest (in the home part) lists them all.
async fn build_own(app: &AppState, home: &OrgState, subject: &RecordId, id: &RecordId) -> AppResult<(Vec<u8>, Value)> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let mut categories = Vec::new();
    let (email, audit_redacted) = account_part(app, subject, &mut zip, &mut categories).await?;
    let mut red = Redactions { audit: audit_redacted, ..Redactions::default() };
    let mut workspaces = Vec::new();
    let orgs: Vec<RecordId> = q::USER_ORGS.on(&app.control).bind(("u", subject.clone())).await?.take(0)?;
    for org in orgs {
        let Some(oid) = crate::rid::key_string(&org.key).and_then(|k| OrgId::parse(&k)) else { continue };
        if oid == home.db.org() {
            workspace_part(home, subject, &mut zip, &mut categories, &mut red).await?;
            workspaces.push(json!({ "workspace": oid.key(), "included": true, "stored_in": "this workspace" }));
            continue;
        }
        match app.org(&oid).await {
            Ok(state) => {
                // this workspace's part stays in this workspace's database
                let mut part = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
                workspace_part(&state, subject, &mut part, &mut categories, &mut red).await?;
                let bytes = part.finish().map_err(|e| AppError::internal(e.to_string()))?.into_inner();
                q::EXPORT_PART_PUT
                    .on(&state.db)
                    .bind(("id", id.clone()))
                    .bind(("u", subject.clone()))
                    .bind(("manifest", json!({ "workspace": oid.key() })))
                    .bind(("size", bytes.len() as i64))
                    .bind(("archive", Bytes::from(bytes)))
                    .await?
                    .check()?;
                workspaces.push(json!({ "workspace": oid.key(), "included": true, "stored_in": "that workspace" }));
            }
            Err(e) => workspaces.push(json!({ "workspace": oid.key(), "included": false, "why": e.message })),
        }
    }
    let manifest = manifest_for(subject, &email, "self", workspaces, categories, &red, OWN_LEFT_OUT);
    Ok((finish_zip(zip, &manifest)?, manifest))
}

/// An admin's export of one workspace's data about the person: what they wrote in its shared spaces
/// (with the document files), their spaces there, their account record, tokens and connected apps by
/// name, and audit events about them. No personal space, private chat, synced records or other workspace.
async fn build_organisation(app: &AppState, state: &OrgState, subject: &RecordId) -> AppResult<(Vec<u8>, Value)> {
    let db = &state.db;
    let key = db.org().key();
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let mut categories = Vec::new();
    let ws = Some(key.as_str());
    let mut res = q::ACCOUNT.on(&app.control).bind(("u", subject.clone())).await?;
    let account: Option<Value> = res.take(0)?;
    let account = account.ok_or_else(|| AppError::coded(ErrorCode::AuthNotFound, "That account doesn't exist."))?;
    let email = account.get("email").and_then(Value::as_str).unwrap_or_default().to_string();
    let record = json!({ "email": email, "created_at": account.get("created_at") });
    let _ = take_vec(&mut res, 1)?;
    let _ = take_vec(&mut res, 2)?;
    let tokens: Vec<Value> = take_vec(&mut res, 3)?.into_iter().map(|t| json!({ "name": t.get("name"), "created_at": t.get("created_at"), "last_used_at": t.get("last_used_at") })).collect();
    let _ = take_vec(&mut res, 4)?;
    let apps: Vec<Value> = take_vec(&mut res, 5)?.into_iter().map(|a| json!({ "name": a.get("client_name"), "created_at": a.get("created_at") })).collect();
    let audit: Vec<Value> = q::ORG_AUDIT_ABOUT.on(&app.control).bind(("u", subject.clone())).await?.take(0)?;

    let shared: Vec<RecordId> = q::ORG_SHARED_VAULTS.on(db).await?.take(0)?;
    let spaces: Vec<Value> = q::MEMBERSHIPS.on(db).bind(("u", subject.clone())).await?.take::<Vec<Value>>(0)?.into_iter().filter(|m| m.get("kind").and_then(Value::as_str) == Some("org")).collect();
    let mut res = q::AUTHORED.on(db).bind(("u", subject.clone())).bind(("readable", shared)).await?;
    let mut entities = Vec::new();
    for i in 0..7 {
        entities.extend(take_vec(&mut res, i)?);
    }
    let memories = take_vec(&mut res, 7)?;
    let relations = take_vec(&mut res, 8)?;
    let skills = take_vec(&mut res, 9)?;
    let documents = take_vec(&mut res, 10)?;

    put(&mut zip, &mut categories, ws, "account", "account.json", 1, record)?;
    put(&mut zip, &mut categories, ws, "spaces", "spaces.json", spaces.len(), json!(spaces))?;
    put(&mut zip, &mut categories, ws, "entities", "entities.json", entities.len(), json!(entities))?;
    put(&mut zip, &mut categories, ws, "memories", "memories.json", memories.len(), json!(memories))?;
    put(&mut zip, &mut categories, ws, "relations", "relations.json", relations.len(), json!(relations))?;
    put(&mut zip, &mut categories, ws, "skills", "skills.json", skills.len(), json!(skills))?;
    put(&mut zip, &mut categories, ws, "api_tokens", "api_tokens.json", tokens.len(), json!(tokens))?;
    put(&mut zip, &mut categories, ws, "connected_apps", "connected_apps.json", apps.len(), json!(apps))?;
    put(&mut zip, &mut categories, ws, "audit_events", "audit_events.json", audit.len(), json!(audit))?;
    let mut red = Redactions::default();
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    if !state.settings.documents_backend.is_empty() {
        for d in &documents {
            let (Some(id), Some(path)) = (d.get("id").and_then(Value::as_str), d.get("object_path").and_then(Value::as_str)) else { continue };
            let name = d.get("filename").and_then(Value::as_str).unwrap_or("document");
            let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect();
            let doc_key = id.split(':').nth(1).unwrap_or(id);
            match crate::documents::get_object(db, path).await {
                Ok(Some(bytes)) => {
                    zip.start_file(format!("documents/{doc_key}/{safe}"), opts).map_err(|e| AppError::internal(e.to_string()))?;
                    zip.write_all(&bytes).map_err(|e| AppError::internal(e.to_string()))?;
                    red.files_included += 1;
                }
                _ => red.files_missing += 1,
            }
        }
    }
    let docs: Vec<Value> = documents.into_iter().map(|mut d| {
        if let Some(o) = d.as_object_mut() {
            o.remove("object_path");
        }
        d
    }).collect();
    put(&mut zip, &mut categories, ws, "documents", "documents.json", docs.len(), json!(docs))?;
    let left_out = [
        "Their personal space, private chat, synced records, sources, recordings and settings.",
        "Every other workspace they belong to.",
        "Secret values of any kind.",
    ];
    let manifest = manifest_for(subject, &email, "organisation", vec![json!({ "workspace": key, "included": true })], categories, &red, &left_out);
    Ok((finish_zip(zip, &manifest)?, manifest))
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
