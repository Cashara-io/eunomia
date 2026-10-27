//! Uploaded documents: the original bytes in the org database's SurrealDB file bucket, the extracted
//! text indexed as ordinary `cache_record`s so search, recall, entity extraction and the vector index
//! treat them like synced data. Design and setup: docs/documents.md.
//!
//! - Upload validates the type, size and text, stores the bytes at an immutable path chosen here
//!   (`/{vault key}/{key}/{revision}/{safe name}`, so each vault's files sit under their own prefix),
//!   writes the `document` row (status `indexing`) and queues an `index_document` job. Nothing the
//!   caller sends becomes a path or a URL. Rows keep their path, so a document stored before 2.0.4
//!   (`/{key}/{revision}/{safe name}`, no vault prefix) still downloads; its next revision moves it.
//! - The job extracts the text, chunks it (line-aware, [`CHUNK_CHARS`]), ingests the chunks through
//!   `cache::ingest` (embedding, extraction jobs), then retires every other revision's chunks and marks
//!   the document `ready` (or `failed`, with the error; the original stays downloadable).
//! - Delete marks the document deleted, removes its chunks and what was extracted from them in one
//!   transaction, then the object, then the row.
//!
//! A document lives in one vault (the caller's default one unless a `vault_id` is given): every
//! operation authorizes on that vault, so its members read and write it and a guessed id of a vault the
//! caller is not in, or of another org, reads as absent. Its chunks are filed in the same vault (none,
//! that is the owner's, for the personal vault) and keep the uploader as their owner. A vault clone or
//! merge copies its documents (bytes, rows, chunks and vectors) into the new vault; deleting a vault
//! deletes them ([`copy_vault_documents`], [`purge_vault_documents`]).

use base64::Engine;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use surrealdb::types::{Bytes, Datetime, RecordId, SurrealValue};

use crate::authz::{self, Action};
use crate::cache::search::Envelope;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::jobs::{self, Job, JobError, NewJob};
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::state::{AppState, OrgState};
use crate::store::documents as q;

/// The cache source and record type of a document's chunks.
pub const SOURCE: &str = "documents";
pub const CHUNK_TYPE: &str = "document.chunk";
/// Chunk size in characters: well under the embedding input limit, a reasonable extraction call.
pub const CHUNK_CHARS: usize = 6000;
/// What `document_get` returns of the text before pointing at the export and download.
pub const GET_TEXT_CHARS: usize = 100_000;

/// Types accepted, by canonical media type.
pub const MEDIA_TYPES: &[&str] = &["text/plain", "text/markdown", "application/json", "text/csv", "application/pdf"];

/// The bucket backend URL for one org database: `base` (`EUNOMIA_DOCUMENTS_BACKEND`) with the
/// database name appended to the folder or key prefix, so orgs sharing a store never share keys.
/// Credentials are refused in the URL: `DEFINE BUCKET` text is visible to `INFO FOR DB`, so they come
/// from the database server's environment instead.
pub fn bucket_url(base: &str, db: &str) -> Result<String, String> {
    let base = base.trim();
    if !db.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || db.is_empty() {
        return Err(format!("unexpected database name {db:?}"));
    }
    if base == "memory" {
        return Ok("memory".into());
    }
    let mut url = url::Url::parse(base).map_err(|e| format!("{base:?} is not a URL ({e}); use memory, file:/path, s3://bucket, gs://bucket or az://container"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("put no credentials in the URL: set them in the database server's environment (AWS_ACCESS_KEY_ID, ...)".into());
    }
    match url.scheme() {
        "file" => {
            if url.query_pairs().next().is_some() || url.host_str().is_some_and(|h| !h.is_empty()) {
                return Err("a file backend is file:/absolute/path, with no host or options".into());
            }
            let path = url.path().trim_end_matches('/');
            if path.is_empty() || path.split('/').any(|seg| seg == "..") {
                return Err("a file backend needs an absolute path".into());
            }
            Ok(format!("file:{path}/{db}"))
        }
        "s3" | "s3+http" | "s3+https" | "gs" | "gcs" | "az" | "azure" => {
            let pairs: Vec<(String, String)> = url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
            if let Some((k, _)) = pairs.iter().find(|(k, _)| k != "region" && k != "prefix") {
                return Err(format!("only region and prefix may be set in the URL, not {k:?}; credentials come from the database server's environment"));
            }
            let prefix = pairs.iter().find(|(k, _)| k == "prefix").map(|(_, v)| v.trim_matches('/').to_string()).filter(|p| !p.is_empty());
            let prefix = match prefix {
                Some(p) => format!("{p}/{db}"),
                None => db.to_string(),
            };
            let mut out: Vec<(String, String)> = pairs.into_iter().filter(|(k, _)| k != "prefix").collect();
            out.push(("prefix".into(), prefix));
            url.query_pairs_mut().clear().extend_pairs(out);
            Ok(url.to_string())
        }
        other => Err(format!("unsupported backend scheme {other:?}; use memory, file:, s3:, s3+http:, s3+https:, gs: or az:")),
    }
}

/// The canonical media type for an upload: the declared type when it is a supported one, else (no type,
/// or a generic binary one) the file extension's. `None` means unsupported.
pub fn media_type(filename: &str, declared: Option<&str>) -> Option<&'static str> {
    let declared = declared.unwrap_or("").split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let canonical = match declared.as_str() {
        "text/x-markdown" | "text/md" => "text/markdown",
        "application/x-pdf" => "application/pdf",
        "text/json" => "application/json",
        "application/csv" => "text/csv",
        other => other,
    };
    if let Some(t) = MEDIA_TYPES.iter().find(|t| **t == canonical) {
        return Some(t);
    }
    if !(canonical.is_empty() || canonical == "application/octet-stream") {
        return None;
    }
    let ext = filename.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    Some(match ext.as_str() {
        "txt" | "text" | "log" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "json" => "application/json",
        "csv" => "text/csv",
        "pdf" => "application/pdf",
        _ => return None,
    })
}

/// The name kept for display and the download, without any directory part.
fn clean_filename(name: &str) -> AppResult<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    if base.is_empty() || base == "." || base == ".." {
        return Err(AppError::bad_request("filename is required"));
    }
    if base.chars().any(char::is_control) {
        return Err(AppError::bad_request("filename must not contain control characters"));
    }
    if base.chars().count() > 255 {
        return Err(AppError::bad_request("filename is longer than 255 characters"));
    }
    Ok(base.to_string())
}

/// The object key's last segment: only the characters a bucket key allows, never `..`.
fn safe_segment(name: &str) -> String {
    let s: String = name.chars().take(100).map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).collect();
    let s = s.trim_start_matches('.').to_string();
    if s.is_empty() { "file".into() } else { s }
}

fn new_key() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::thread_rng();
    (0..20).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect()
}

fn object_path(vault: &RecordId, key: &str, revision: i64, filename: &str) -> String {
    format!("/{}/{key}/{revision}/{}", safe_segment(&key_of(vault)), safe_segment(filename))
}

/// A document id given by a caller: `document:<key>` with a plain key, else not found.
pub fn parse_id(id: &str) -> AppResult<RecordId> {
    let not_found = || AppError::coded(ErrorCode::DocumentNotFound, "Document not found.");
    let key = id.trim().strip_prefix("document:").ok_or_else(not_found)?;
    if key.is_empty() || key.len() > 64 || !key.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(not_found());
    }
    Ok(RecordId::from_table_key("document", key.to_string()))
}

fn key_of(id: &RecordId) -> String {
    crate::rid::key_string(id.key()).unwrap_or_default()
}

pub fn download_path(id: &str) -> String {
    format!("/api/documents/{id}/download")
}

// ---- rows and what callers see ----------------------------------------------------------------

#[derive(Debug, Clone, serde::Deserialize, SurrealValue)]
pub(crate) struct DocRow {
    id: RecordId,
    owner: RecordId,
    vault: RecordId,
    filename: String,
    media_type: String,
    size_bytes: i64,
    sha256: String,
    object_path: String,
    revision: i64,
    status: String,
    #[serde(default)]
    #[surreal(default)]
    error: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    chunk_count: i64,
    created_at: Datetime,
    updated_at: Datetime,
    #[serde(default)]
    #[surreal(default)]
    indexed_at: Option<Datetime>,
}

/// One document as callers see it.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DocumentOut {
    pub id: String,
    pub filename: String,
    pub media_type: String,
    pub size_bytes: i64,
    /// SHA-256 of the original bytes, hex.
    pub sha256: String,
    /// Bumped by a re-upload or a re-index; chunks name the revision they came from.
    pub revision: i64,
    /// `indexing`, `ready` or `failed` (`deleted` only while a deletion's cleanup is unfinished).
    pub status: String,
    /// Why indexing failed. Absent otherwise (a tool result with an `error` key reads as a failed call).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub chunk_count: i64,
    pub created_at: String,
    pub updated_at: String,
    pub indexed_at: Option<String>,
    /// Authenticated `GET` for the original bytes (same credentials as this call).
    pub download_url: String,
}

fn ts(d: &Datetime) -> String {
    (*d).into_inner().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

impl From<&DocRow> for DocumentOut {
    fn from(r: &DocRow) -> Self {
        let id = r.id.to_string();
        DocumentOut {
            download_url: download_path(&id),
            id,
            filename: r.filename.clone(),
            media_type: r.media_type.clone(),
            size_bytes: r.size_bytes,
            sha256: r.sha256.clone(),
            revision: r.revision,
            status: r.status.clone(),
            error: r.error.clone(),
            chunk_count: r.chunk_count,
            created_at: ts(&r.created_at),
            updated_at: ts(&r.updated_at),
            indexed_at: r.indexed_at.as_ref().map(ts),
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DocumentList {
    pub results: Vec<DocumentOut>,
    pub total: i64,
    pub has_more: bool,
    /// Whether this server stores documents at all (`EUNOMIA_DOCUMENTS_BACKEND` is set).
    pub storage: bool,
    /// The largest upload accepted, in bytes.
    pub max_bytes: i64,
    /// Accepted media types.
    pub media_types: Vec<String>,
}

/// Where one chunk sits in its document.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ChunkRef {
    /// The chunk's record id for `get` (also its vector's reference).
    pub chunk_id: String,
    pub part: i64,
    /// Character offsets of the chunk in the extracted text, end exclusive.
    pub char_start: i64,
    pub char_end: i64,
    pub has_embedding: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DocumentDetail {
    pub document: DocumentOut,
    /// The extracted text of the current revision (up to `max_chars`; `text_truncated` says when cut).
    pub text: String,
    pub text_truncated: bool,
    pub chunks: Vec<ChunkRef>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DocumentDeleted {
    pub deleted: bool,
    /// Chunks removed from retrieval, and memories that had been extracted from them.
    pub chunks: i64,
    pub memories: i64,
}

// ---- storage ----------------------------------------------------------------------------------

fn storage_off() -> AppError {
    AppError::coded(
        ErrorCode::DocumentStorageUnavailable,
        "Document storage is not configured on this server: set EUNOMIA_DOCUMENTS_BACKEND (see docs/documents.md).",
    )
}

fn storage_error(e: surrealdb::Error) -> AppError {
    AppError::coded(ErrorCode::DocumentStorageUnavailable, "Document storage is unavailable right now; try again, or ask the operator to check the database's file bucket (docs/documents.md).")
        .with_source(e.to_string())
}

async fn put_object(db: &OrgDb, path: &str, bytes: Vec<u8>) -> AppResult<()> {
    q::OBJECT_PUT.on(db).bind(("path", path.to_string())).bind(("bytes", Bytes::from(bytes))).await.map_err(storage_error)?;
    Ok(())
}

async fn get_object(db: &OrgDb, path: &str) -> AppResult<Option<Vec<u8>>> {
    let mut res = q::OBJECT_GET.on(db).bind(("path", path.to_string())).await.map_err(storage_error)?;
    Ok(res.take::<Option<Bytes>>(0).map_err(storage_error)?.map(|b| b.to_vec()))
}

async fn delete_object(db: &OrgDb, path: &str) -> AppResult<()> {
    q::OBJECT_DELETE.on(db).bind(("path", path.to_string())).await.map_err(storage_error)?;
    Ok(())
}

// ---- reads ------------------------------------------------------------------------------------

fn not_found() -> AppError {
    AppError::coded(ErrorCode::DocumentNotFound, "Document not found.")
}

/// The vault a list or upload names (`vault_id`), else the caller's default one (a vault-restricted
/// token's own vault, otherwise the personal vault), authorized for `action`.
async fn target_vault(db: &OrgDb, user: &RecordId, vault_id: Option<&str>, action: Action) -> AppResult<RecordId> {
    let vault = match vault_id {
        Some(v) => crate::rid::parse(v).map_err(|_| AppError::coded(ErrorCode::VaultNotFound, "Vault not found."))?,
        None => crate::vaults::service::default_vault_id(db, user).await?,
    };
    Ok(authz::authorize(db, user, action, &vault).await?.vault().clone())
}

/// A document by id, with no access check (jobs, and a chunk the caller already read).
async fn fetch(db: &OrgDb, id: &RecordId, include_deleted: bool) -> AppResult<DocRow> {
    let stmt = if include_deleted { &q::GET_ANY } else { &q::GET };
    let mut res = stmt.on(db).bind(("id", id.clone())).await?;
    let rows: Vec<DocRow> = res.take(0)?;
    rows.into_iter().next().ok_or_else(not_found)
}

/// A document `user` may `action` on: its vault decides. One in a vault the caller is not a member of
/// reads as absent; a credential lacking the scope, or restricted to another vault, is still refused.
async fn load(db: &OrgDb, user: &RecordId, id: &RecordId, include_deleted: bool, action: Action) -> AppResult<DocRow> {
    authz::require_scope(action.scope())?;
    let doc = fetch(db, id, include_deleted).await?;
    match authz::authorize(db, user, action, &doc.vault).await {
        Ok(_) => Ok(doc),
        Err(e) if e.code == ErrorCode::VaultForbidden => Err(not_found()),
        Err(e) => Err(e),
    }
}

pub async fn list(state: &OrgState, user: &RecordId, vault_id: Option<&str>, limit: i64, offset: i64) -> AppResult<DocumentList> {
    let vault = target_vault(&state.db, user, vault_id, Action::ReadMemories).await?;
    let (limit, offset) = (limit.clamp(1, 200), offset.max(0));
    let mut res = q::LIST.on(&state.db).bind(("vault", vault.clone())).bind(("limit", limit)).bind(("offset", offset)).await?;
    let rows: Vec<DocRow> = res.take(0)?;
    #[derive(serde::Deserialize, SurrealValue)]
    struct Count {
        count: i64,
    }
    let mut res = q::COUNT.on(&state.db).bind(("vault", vault)).await?;
    let total = res.take::<Vec<Count>>(0)?.first().map_or(0, |c| c.count);
    let results: Vec<DocumentOut> = rows.iter().map(DocumentOut::from).collect();
    let has_more = offset + (results.len() as i64) < total;
    Ok(DocumentList {
        results,
        total,
        has_more,
        storage: !state.settings.documents_backend.is_empty(),
        max_bytes: state.settings.documents_max_bytes as i64,
        media_types: MEDIA_TYPES.iter().map(|t| t.to_string()).collect(),
    })
}

#[derive(serde::Deserialize, SurrealValue)]
struct ChunkRow {
    id: RecordId,
    #[serde(default)]
    #[surreal(default)]
    body_text: String,
    #[serde(default)]
    #[surreal(default)]
    payload: Value,
    #[serde(default)]
    #[surreal(default)]
    has_embedding: bool,
}

fn int(v: &Value, k: &str) -> i64 {
    v.get(k).and_then(Value::as_i64).unwrap_or(0)
}

async fn chunks(db: &OrgDb, doc: &DocRow) -> AppResult<Vec<ChunkRow>> {
    let mut res = q::CHUNKS
        .on(db)
        .bind(("owner", doc.owner.clone()))
        .bind(("document", doc.id.to_string()))
        .bind(("revision", doc.revision))
        .await?;
    let mut rows: Vec<ChunkRow> = res.take(0)?;
    rows.sort_by_key(|r| int(&r.payload, "part"));
    Ok(rows)
}

pub async fn get(state: &OrgState, user: &RecordId, id: &str, max_chars: Option<usize>) -> AppResult<DocumentDetail> {
    let doc = load(&state.db, user, &parse_id(id)?, false, Action::ReadMemories).await?;
    let rows = chunks(&state.db, &doc).await?;
    let max = max_chars.unwrap_or(GET_TEXT_CHARS).clamp(1, 1_000_000);
    let mut text = String::new();
    let mut truncated = false;
    for r in &rows {
        let room = max.saturating_sub(text.chars().count());
        if r.body_text.chars().count() > room {
            text.extend(r.body_text.chars().take(room));
            truncated = true;
            break;
        }
        text.push_str(&r.body_text);
    }
    let chunks = rows
        .iter()
        .map(|r| ChunkRef {
            chunk_id: crate::cache::search::literal(&r.id),
            part: int(&r.payload, "part"),
            char_start: int(&r.payload, "char_start"),
            char_end: int(&r.payload, "char_end"),
            has_embedding: r.has_embedding,
        })
        .collect();
    Ok(DocumentDetail { document: DocumentOut::from(&doc), text, text_truncated: truncated, chunks })
}

/// The original bytes (checked against the stored SHA-256) and the row they belong to. The caller
/// records the disclosure in the audit log.
pub async fn download(state: &OrgState, user: &RecordId, id: &str) -> AppResult<(DocumentOut, Vec<u8>)> {
    let doc = load(&state.db, user, &parse_id(id)?, false, Action::ReadMemories).await?;
    let bytes = stored_bytes(&state.db, &doc).await?;
    Ok((DocumentOut::from(&doc), bytes))
}

/// `doc`'s original bytes, checked against its SHA-256.
async fn stored_bytes(db: &OrgDb, doc: &DocRow) -> AppResult<Vec<u8>> {
    let bytes = get_object(db, &doc.object_path)
        .await?
        .ok_or_else(|| AppError::coded(ErrorCode::DocumentStorageUnavailable, "The stored file is missing from document storage.").with_source(doc.object_path.clone()))?;
    if sha256_hex(&bytes) != doc.sha256 {
        return Err(AppError::internal("stored document bytes do not match their sha256").with_source(doc.id.to_string()));
    }
    Ok(bytes)
}

/// The versioned export manifest: the document, its chunks with their text and location, and each
/// chunk's vector (or an explicit `missing`) with the embedding settings. `vectors: false` leaves the
/// numbers out (an agent's context does not need 1536 floats a chunk).
pub async fn export(state: &OrgState, user: &RecordId, id: &str, vectors: bool) -> AppResult<Value> {
    let doc = load(&state.db, user, &parse_id(id)?, false, Action::ReadMemories).await?;
    #[derive(serde::Deserialize, SurrealValue)]
    struct VecRow {
        id: RecordId,
        #[serde(default)]
        #[surreal(default)]
        body_text: String,
        #[serde(default)]
        #[surreal(default)]
        payload: Value,
        #[serde(default)]
        #[surreal(default)]
        embedding: Option<Vec<f32>>,
    }
    let mut res = q::CHUNK_VECTORS
        .on(&state.db)
        .bind(("owner", doc.owner.clone()))
        .bind(("document", doc.id.to_string()))
        .bind(("revision", doc.revision))
        .await?;
    let mut rows: Vec<VecRow> = res.take(0)?;
    rows.sort_by_key(|r| int(&r.payload, "part"));
    let spec = crate::embeddings::service::spec_for(&state.db, &state.settings, &doc.owner).await;
    let (embedded, missing) = rows.iter().fold((0, 0), |(e, m), r| if r.embedding.is_some() { (e + 1, m) } else { (e, m + 1) });
    let chunks: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let mut c = json!({
                "chunk_id": crate::cache::search::literal(&r.id),
                "part": int(&r.payload, "part"),
                "char_start": int(&r.payload, "char_start"),
                "char_end": int(&r.payload, "char_end"),
                "text": r.body_text,
                "embedding_state": if r.embedding.is_some() { "present" } else { "missing" },
            });
            if vectors {
                c["embedding"] = json!(r.embedding);
            }
            c
        })
        .collect();
    let out = DocumentOut::from(&doc);
    Ok(json!({
        "format": "eunomia.document-export",
        "version": 1,
        "document": out,
        "embedding": {
            "dimension": crate::embeddings::service::dim(),
            "provider": spec.as_ref().map(|s| s.origin.clone()).ok(),
            "model": spec.as_ref().map(|s| s.model.clone()).ok(),
            "provenance": "the owner's current embedding settings; stored vectors do not record which model made them",
            "chunks_embedded": embedded,
            "chunks_missing": missing,
        },
        "vectors_included": vectors,
        "chunks": chunks,
    }))
}

// ---- writes -----------------------------------------------------------------------------------

/// What an upload carries.
pub struct Upload {
    pub filename: String,
    /// The declared media type; the extension decides when it is absent or generic.
    pub media_type: Option<String>,
    pub bytes: Vec<u8>,
    /// A document to publish these bytes as a new revision of (it stays in its vault).
    pub replace: Option<String>,
    /// The vault a new document goes in; none: the caller's default vault.
    pub vault: Option<String>,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Bytes from a tool call: `text` (UTF-8) or `content_base64`, exactly one.
pub fn tool_bytes(text: Option<String>, content_base64: Option<String>) -> AppResult<Vec<u8>> {
    match (text, content_base64) {
        (Some(t), None) => Ok(t.into_bytes()),
        (None, Some(b)) => {
            let cleaned: String = b.chars().filter(|c| !c.is_whitespace()).collect();
            base64::engine::general_purpose::STANDARD.decode(cleaned).map_err(|e| AppError::bad_request(format!("content_base64 is not valid base64: {e}")))
        }
        _ => Err(AppError::bad_request("pass exactly one of text or content_base64")),
    }
}

/// Type, size and content checks; returns the canonical media type.
fn validate(settings: &crate::config::Settings, filename: &str, declared: Option<&str>, bytes: &[u8]) -> AppResult<&'static str> {
    if bytes.is_empty() {
        return Err(AppError::bad_request("the document is empty"));
    }
    if bytes.len() > settings.documents_max_bytes {
        return Err(AppError::coded(
            ErrorCode::DocumentTooLarge,
            format!("the document is {} bytes; the limit is {} bytes", bytes.len(), settings.documents_max_bytes),
        ));
    }
    let Some(media) = media_type(filename, declared) else {
        return Err(AppError::coded(
            ErrorCode::DocumentUnsupportedType,
            format!("unsupported document type {:?}; supported: plain text, Markdown, JSON, CSV and text-based PDF", declared.unwrap_or(filename)),
        ));
    };
    if media == "application/pdf" {
        if !bytes.starts_with(b"%PDF-") {
            return Err(AppError::bad_request("the file is not a PDF (no %PDF- header)"));
        }
    } else {
        let text = std::str::from_utf8(bytes).map_err(|_| AppError::bad_request("the document is not valid UTF-8 text"))?;
        if media == "application/json" && serde_json::from_str::<serde::de::IgnoredAny>(text).is_err() {
            return Err(AppError::bad_request("the document is not valid JSON"));
        }
    }
    Ok(media)
}

fn require_storage(state: &OrgState) -> AppResult<()> {
    if state.settings.documents_backend.is_empty() { Err(storage_off()) } else { Ok(()) }
}

/// Store the bytes and queue indexing. A `replace`d document gets a new revision; its old object is
/// removed once the new one is in place.
pub async fn upload(state: &OrgState, user: &RecordId, up: Upload) -> AppResult<DocumentOut> {
    let db = &state.db;
    let previous = match &up.replace {
        Some(id) => Some(load(db, user, &parse_id(id)?, false, Action::WriteMemories).await?),
        None => None,
    };
    let vault = match &previous {
        Some(p) => p.vault.clone(),
        None => target_vault(db, user, up.vault.as_deref(), Action::WriteMemories).await?,
    };
    let filename = clean_filename(&up.filename)?;
    let media = validate(&state.settings, &filename, up.media_type.as_deref(), &up.bytes)?;
    require_storage(state)?;
    let sha = sha256_hex(&up.bytes);
    let size = up.bytes.len() as i64;

    // a new revision stays with the document's owner, whose chunks it replaces
    let (id, revision, owner) = match &previous {
        Some(p) => (p.id.clone(), p.revision + 1, p.owner.clone()),
        None => (RecordId::from_table_key("document", new_key()), 1, user.clone()),
    };
    let path = object_path(&vault, &key_of(&id), revision, &filename);
    put_object(db, &path, up.bytes).await?;

    let written: AppResult<Vec<DocRow>> = async {
        let mut res = match &previous {
            None => q::CREATE.on(db).bind(("vault", vault.clone())),
            Some(p) => q::REVISE.on(db).bind(("revision", revision)).bind(("expected", p.revision)),
        }
        .bind(("id", id.clone()))
        .bind(("owner", owner.clone()))
        .bind(("filename", filename.clone()))
        .bind(("media_type", media.to_string()))
        .bind(("size_bytes", size))
        .bind(("sha256", sha.clone()))
        .bind(("object_path", path.clone()))
        .await?;
        Ok(res.take(0)?)
    }
    .await;
    let row = match written.map(|rows| rows.into_iter().next()) {
        Ok(Some(row)) => row,
        other => {
            let _ = delete_object(db, &path).await;
            return Err(match other {
                Err(e) => e,
                _ => AppError::coded(ErrorCode::DbConflict, "the document changed while uploading; retry"),
            });
        }
    };
    if let Some(p) = previous.filter(|p| p.object_path != path)
        && let Err(e) = delete_object(db, &p.object_path).await
    {
        tracing::warn!(document = %row.id.to_string(), error = e.source.as_deref().unwrap_or(&e.message), "the previous revision's file could not be removed");
    }
    let op = if row.revision > 1 { crate::changes::UPDATED } else { crate::changes::CREATED };
    crate::changes::record(db, &vault, op, crate::changes::DOCUMENT, &row.id).await;
    enqueue_index(state, &row).await?;
    Ok(DocumentOut::from(&row))
}

/// Extract and index the stored bytes again as a new revision (after a failure, or a change in how
/// text is extracted). The chunks of the previous revision stay searchable until it is ready.
pub async fn reindex(state: &OrgState, user: &RecordId, id: &str) -> AppResult<DocumentOut> {
    let db = &state.db;
    let doc = load(db, user, &parse_id(id)?, false, Action::WriteMemories).await?;
    require_storage(state)?;
    let mut res = q::REVISE
        .on(db)
        .bind(("id", doc.id.clone()))
        .bind(("owner", doc.owner.clone()))
        .bind(("filename", doc.filename.clone()))
        .bind(("media_type", doc.media_type.clone()))
        .bind(("size_bytes", doc.size_bytes))
        .bind(("sha256", doc.sha256.clone()))
        .bind(("object_path", doc.object_path.clone()))
        .bind(("revision", doc.revision + 1))
        .bind(("expected", doc.revision))
        .await?;
    let row: DocRow = res.take::<Vec<DocRow>>(0)?.into_iter().next().ok_or_else(|| AppError::coded(ErrorCode::DbConflict, "the document changed meanwhile; retry"))?;
    enqueue_index(state, &row).await?;
    Ok(DocumentOut::from(&row))
}

/// Revoke first (the row reads as deleted and its chunks leave retrieval in one transaction), then the
/// object, then the row. A failure part way leaves a `deleted` row; deleting again finishes it.
pub async fn delete(state: &OrgState, user: &RecordId, id: &str) -> AppResult<DocumentDeleted> {
    let doc = load(&state.db, user, &parse_id(id)?, true, Action::WriteMemories).await?;
    let removed = purge(&state.db, &doc, !state.settings.documents_backend.is_empty()).await?;
    crate::changes::record(&state.db, &doc.vault, crate::changes::DELETED, crate::changes::DOCUMENT, &doc.id).await;
    Ok(DocumentDeleted { deleted: true, chunks: int(&removed, "records"), memories: int(&removed, "memories") })
}

/// Deletes `doc` (see [`delete`]); `objects: false` leaves storage alone (it is not configured).
async fn purge(db: &OrgDb, doc: &DocRow, objects: bool) -> AppResult<Value> {
    q::MARK_DELETED.on(db).bind(("id", doc.id.clone())).bind(("owner", doc.owner.clone())).await?.check()?;
    let removed = remove_chunks(db, &doc.owner, &doc.id, -1, -1, -1).await?;
    if objects {
        delete_object(db, &doc.object_path).await?;
    }
    q::REMOVE.on(db).bind(("id", doc.id.clone())).bind(("owner", doc.owner.clone())).await?.check()?;
    Ok(removed)
}

/// Deletes every document of `vault` like [`delete`] does, bytes included: before a vault is deleted,
/// and when a failed clone or merge is discarded. A failure part way stops it and the caller fails, so
/// a retry finishes the job and never leaves a row whose bytes are gone (a deleted row reads as absent).
/// Storage errors are not skipped: rows exist here only because a file was stored.
pub async fn purge_vault_documents(db: &OrgDb, vault: &RecordId) -> AppResult<usize> {
    let mut res = q::IN_VAULT.on(db).bind(("vault", vault.clone())).await?;
    let docs: Vec<DocRow> = res.take(0)?;
    for doc in &docs {
        purge(db, doc, true).await?;
    }
    Ok(docs.len())
}

/// Copies every live document of `src` into `dest` (a vault being built by a clone or merge, which no
/// one can see yet) as `owner`'s: the bytes under `dest`'s prefix, the row, and the current revision's
/// chunks with their vectors (filed in `dest`, nothing re-embedded or re-extracted). Returns each source
/// chunk's copy, so the copied memories that were extracted from a chunk point at its copy.
/// Each row is written before its bytes, so a failure or a crash part way leaves nothing that
/// [`purge_vault_documents`] (the discard of a failed copy) does not find. A document still being
/// indexed is copied as `failed`, to be reindexed from its copied bytes.
#[allow(clippy::mutable_key_type)] // RecordId hashes by value; the interior mutability is never touched
pub async fn copy_vault_documents(db: &OrgDb, owner: &RecordId, src: &RecordId, dest: &RecordId) -> AppResult<std::collections::HashMap<RecordId, RecordId>> {
    let mut chunk_map = std::collections::HashMap::new();
    let mut res = q::IN_VAULT.on(db).bind(("vault", src.clone())).await?;
    let docs: Vec<DocRow> = res.take(0)?;
    for doc in docs.iter().filter(|d| d.status != "deleted") {
        let bytes = stored_bytes(db, doc).await?;
        let key = new_key();
        let id = RecordId::from_table_key("document", key.clone());
        let path = object_path(dest, &key, doc.revision, &doc.filename);
        let (status, error) = match doc.status.as_str() {
            "indexing" => ("failed".to_string(), Some("copied while it was being indexed: reindex it".to_string())),
            _ => (doc.status.clone(), doc.error.clone()),
        };
        q::COPY
            .on(db)
            .bind(("id", id.clone()))
            .bind(("owner", owner.clone()))
            .bind(("vault", dest.clone()))
            .bind(("filename", doc.filename.clone()))
            .bind(("media_type", doc.media_type.clone()))
            .bind(("size_bytes", doc.size_bytes))
            .bind(("sha256", doc.sha256.clone()))
            .bind(("object_path", path.clone()))
            .bind(("revision", doc.revision))
            .bind(("status", status.clone()))
            .bind(("error", error))
            .bind(("chunk_count", if status == "ready" { doc.chunk_count } else { 0 }))
            .bind(("indexed_at", doc.indexed_at))
            .await?
            .check()?;
        put_object(db, &path, bytes).await?;
        if status != "ready" {
            continue;
        }
        #[derive(serde::Deserialize, SurrealValue)]
        struct Chunk {
            id: RecordId,
            #[serde(default)]
            #[surreal(default)]
            title: String,
            #[serde(default)]
            #[surreal(default)]
            body_text: String,
            #[serde(default)]
            #[surreal(default)]
            occurred_at: Option<Datetime>,
            #[serde(default)]
            #[surreal(default)]
            payload: Value,
            #[serde(default)]
            #[surreal(default)]
            embedding: Option<Vec<f32>>,
        }
        let mut res = q::CHUNKS_TO_COPY
            .on(db)
            .bind(("owner", doc.owner.clone()))
            .bind(("document", doc.id.to_string()))
            .bind(("revision", doc.revision))
            .await?;
        let chunks: Vec<Chunk> = res.take(0)?;
        for c in chunks {
            let mut payload = c.payload;
            payload["document_id"] = json!(id.to_string());
            let env = chunk_envelope(&key, doc.revision, int(&payload, "part"), c.title, c.body_text, c.occurred_at, payload, Some(dest));
            crate::cache::search::upsert(db, owner, &env).await?;
            if let Some(v) = c.embedding {
                crate::cache::search::set_embedding(db, owner, &env.id, v).await?;
            }
            chunk_map.insert(c.id, crate::cache::search::rid(owner, &env.id));
        }
    }
    Ok(chunk_map)
}

/// Removes `doc`'s chunks: all but revision `keep`, or only revision `only`, or only revisions older
/// than `below` (each `-1` when unused).
async fn remove_chunks(db: &OrgDb, owner: &RecordId, doc: &RecordId, keep: i64, only: i64, below: i64) -> AppResult<Value> {
    let mut res = crate::tx::with_retry(|| async {
        q::DELETE_CHUNKS
            .on(db)
            .bind(("owner", owner.clone()))
            .bind(("document", doc.to_string()))
            .bind(("keep", keep))
            .bind(("only", only))
            .bind(("below", below))
            .await
    })
    .await?;
    Ok(res.take::<Option<Value>>(q::DELETE_CHUNKS.slot)?.unwrap_or(Value::Null))
}

// ---- indexing ---------------------------------------------------------------------------------

/// Queues indexing as the document's owner (whose chunks they are), whoever in the vault asked.
async fn enqueue_index(state: &OrgState, doc: &DocRow) -> AppResult<()> {
    let id = doc.id.to_string();
    let key = format!("{}:{}:{}", jobs::kind::INDEX_DOCUMENT, id, doc.revision);
    let job = NewJob::new(jobs::kind::INDEX_DOCUMENT, doc.owner.clone(), key).in_org(state.db.org()).payload(json!({ "document": id, "revision": doc.revision }));
    jobs::enqueue(&state.control, job).await?;
    Ok(())
}

/// The text of a stored document. PDFs run on a blocking thread and a panicking parser is a failure,
/// not a crash.
pub async fn extract_text(media_type: &str, bytes: Vec<u8>) -> Result<String, String> {
    if media_type != "application/pdf" {
        return String::from_utf8(bytes).map_err(|_| "the document is not valid UTF-8 text".to_string());
    }
    let text = tokio::task::spawn_blocking(move || pdf_extract::extract_text_from_mem(&bytes))
        .await
        .map_err(|_| "the PDF could not be read (the parser failed)".to_string())?
        .map_err(|e| format!("the PDF could not be read: {e}"))?;
    // pdf layout pads lines with spaces and stacks blank lines
    let mut out = String::new();
    let mut blank = 0;
    for line in text.lines().map(str::trim_end) {
        blank = if line.is_empty() { blank + 1 } else { 0 };
        if blank <= 1 {
            out.push_str(line);
            out.push('\n');
        }
    }
    Ok(out.trim().to_string())
}

/// Splits `text` into consecutive pieces of at most `max` characters, cutting after a newline when
/// one is in reach so lines stay whole. Returns `(char_start, char_end, piece)`; the pieces
/// concatenate back to `text` exactly.
pub fn chunk(text: &str, max: usize) -> Vec<(usize, usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let hard = (start + max).min(chars.len());
        let end = if hard == chars.len() {
            hard
        } else {
            // the last newline in the window, unless that leaves a tiny chunk
            match chars[start..hard].iter().rposition(|c| *c == '\n') {
                Some(i) if i + 1 >= max / 4 => start + i + 1,
                _ => hard,
            }
        };
        out.push((start, end, chars[start..end].iter().collect()));
        start = end;
    }
    out
}

fn finish_err(e: AppError) -> JobError {
    JobError::from(e)
}

/// The `index_document` job: extract, chunk, ingest, retire other revisions, mark ready. Safe to run
/// twice (chunk ids are derived from the document, revision and part, and a stale or finished
/// revision is skipped).
pub async fn index_job(state: AppState, job: Job) -> Result<(), JobError> {
    let doc_id = job.payload.get("document").and_then(Value::as_str).unwrap_or_default().to_string();
    let revision = job.payload.get("revision").and_then(Value::as_i64).unwrap_or(0);
    let id = parse_id(&doc_id).map_err(|_| JobError::permanent(ErrorCode::ValidationInvalid, "job payload names no document"))?;
    let state = state.org(&job.org_id()?).await?;
    let owner = job.owner.clone();
    let doc = match fetch(&state.db, &id, false).await {
        Ok(d) if d.owner == owner => d,
        Ok(_) => return Ok(()),
        Err(e) if e.code == ErrorCode::DocumentNotFound => return Ok(()),
        Err(e) => return Err(finish_err(e)),
    };
    if doc.revision != revision || doc.status != "indexing" {
        return Ok(()); // a newer revision has its own job, or this one already finished
    }
    let last_attempt = job.attempts >= job.max_attempts;
    let outcome = match index(&state, &owner, &doc).await {
        Ok(n) => Ok(("ready", None, n)),
        // the content cannot be indexed: fail now, the original stays downloadable
        Err(IndexError::Content(why)) => Ok(("failed", Some(why), 0)),
        Err(IndexError::Transient(e)) if last_attempt => Err((Some(format!("indexing failed: {}", e.message)), e)),
        Err(IndexError::Transient(e)) => return Err(finish_err(e)),
    };
    let (status, error, n, err) = match outcome {
        Ok((status, error, n)) => (status, error, n, None),
        Err((error, e)) => ("failed", error, 0, Some(e)),
    };
    // Retire OLDER revisions before publishing this one, so search never holds two revisions at once:
    // removing older chunks is right whatever happens meanwhile (a newer revision is never touched).
    if status == "ready" {
        remove_chunks(&state.db, &owner, &doc.id, -1, -1, doc.revision).await.map_err(finish_err)?;
    }
    let current = finish(&state, &owner, &doc, status, error, n).await?;
    // Current and failed: nothing of it stays searchable. Overtaken by a newer revision meanwhile
    // (a re-upload during indexing): only its own chunks go.
    let remove = match (current, status) {
        (true, "ready") => None,
        (true, _) => Some((-1, -1)),
        (false, _) => Some((-1, doc.revision)),
    };
    if let Some((keep, only)) = remove {
        remove_chunks(&state.db, &owner, &doc.id, keep, only, -1).await.map_err(finish_err)?;
    }
    match err {
        Some(e) => Err(finish_err(e)),
        None => Ok(()),
    }
}

/// Records the outcome for `doc`'s revision; false when that revision is no longer the current one.
async fn finish(state: &OrgState, owner: &RecordId, doc: &DocRow, status: &str, error: Option<String>, chunks: usize) -> Result<bool, JobError> {
    let mut res = q::FINISH
        .on(&state.db)
        .bind(("id", doc.id.clone()))
        .bind(("owner", owner.clone()))
        .bind(("status", status.to_string()))
        .bind(("error", error))
        .bind(("chunk_count", chunks as i64))
        .bind(("revision", doc.revision))
        .await
        .map_err(AppError::from)?;
    let rows: Vec<Value> = res.take(0).map_err(AppError::from)?;
    Ok(!rows.is_empty())
}

enum IndexError {
    /// The document itself cannot be indexed (not text, no text in it): permanent.
    Content(String),
    /// Storage or the database: retried.
    Transient(AppError),
}

async fn index(state: &OrgState, owner: &RecordId, doc: &DocRow) -> Result<usize, IndexError> {
    let bytes = get_object(&state.db, &doc.object_path)
        .await
        .map_err(IndexError::Transient)?
        .ok_or_else(|| IndexError::Content("the stored file is missing from document storage".into()))?;
    let text = extract_text(&doc.media_type, bytes).await.map_err(IndexError::Content)?;
    if text.trim().is_empty() {
        return Err(IndexError::Content("no text could be extracted (scanned PDFs and images are not supported)".into()));
    }
    let pieces = chunk(&text, CHUNK_CHARS);
    let n = pieces.len();
    let id = doc.id.to_string();
    let key = key_of(&doc.id);
    // the personal vault's records are filed in none (read by their owner); any other's in it
    let personal = crate::vaults::service::personal_vault_id(&state.db, owner).await.map_err(IndexError::Transient)?;
    let vault = (doc.vault != personal).then_some(&doc.vault);
    let envelopes: Vec<Envelope> = pieces
        .into_iter()
        .enumerate()
        .map(|(i, (start, end, piece))| {
            let title = if n == 1 { doc.filename.clone() } else { format!("{} (part {}/{n})", doc.filename, i + 1) };
            let payload = json!({
                "document_id": id, "revision": doc.revision, "part": i, "char_start": start, "char_end": end, "filename": doc.filename,
            });
            chunk_envelope(&key, doc.revision, i as i64, title, piece, Some(doc.created_at), payload, vault)
        })
        .collect();
    let raws: Vec<Value> = envelopes.iter().map(|e| serde_json::to_value(e).unwrap_or(Value::Null)).collect();
    // the placement is not serialized: put it back on each mapped envelope
    let placement = envelopes.first().and_then(|e| e.placement.clone());
    let report = crate::cache::ingest::ingest(state, owner, SOURCE, &raws, |raw| {
        serde_json::from_value::<Envelope>(raw.clone()).map(|e| vec![Envelope { placement: placement.clone(), ..e }]).map_err(|e| e.to_string())
    })
    .await
    .map_err(IndexError::Transient)?;
    if report.failed > 0 {
        return Err(IndexError::Transient(AppError::internal(format!("{} of {n} chunks could not be stored", report.failed)).with_source(report.errors.join("; "))));
    }
    Ok(n)
}

/// One chunk of document `key`'s `revision` as a cache envelope, filed in `vault` (`None`: the owner's
/// personal vault).
#[allow(clippy::too_many_arguments)]
fn chunk_envelope(key: &str, revision: i64, part: i64, title: String, body_text: String, occurred_at: Option<Datetime>, payload: Value, vault: Option<&RecordId>) -> Envelope {
    let external_id = format!("{key}:{revision}:{part}");
    Envelope {
        id: format!("{SOURCE}:{CHUNK_TYPE}:{external_id}"),
        source: SOURCE.into(),
        type_: CHUNK_TYPE.into(),
        external_id,
        title,
        body_text,
        occurred_at,
        url: String::new(),
        payload,
        deleted: false,
        links: Vec::new(),
        placement: vault.map(|v| crate::cache::search::Placement { vault: Some(v.clone()), ..Default::default() }),
        connector: None,
    }
}

/// A document's metadata by id for `get` on one of its chunks (no vault check: the chunk was already
/// read under the caller's own rules). `None` when it is gone.
pub async fn summary(db: &OrgDb, id: &str) -> AppResult<Option<DocumentOut>> {
    let Ok(id) = parse_id(id) else { return Ok(None) };
    match fetch(db, &id, false).await {
        Ok(d) => Ok(Some(DocumentOut::from(&d))),
        Err(e) if e.code == ErrorCode::DocumentNotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// The document reference a retrieval hit on a chunk carries: which document, revision and part,
/// where in the text, and how to download the original. `None` for any other record.
pub fn chunk_ref(record_id: &str, source: &str, payload: &Value) -> Option<Value> {
    if source != SOURCE {
        return None;
    }
    let doc = payload.get("document_id")?.as_str()?;
    Some(json!({
        "document_id": doc,
        "chunk_id": record_id,
        "filename": payload.get("filename").cloned().unwrap_or(Value::Null),
        "revision": payload.get("revision").cloned().unwrap_or(Value::Null),
        "part": payload.get("part").cloned().unwrap_or(Value::Null),
        "char_start": payload.get("char_start").cloned().unwrap_or(Value::Null),
        "char_end": payload.get("char_end").cloned().unwrap_or(Value::Null),
        "download_url": download_path(doc),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_urls_get_the_database_and_refuse_credentials() {
        let db = "org_0123abcd";
        assert_eq!(bucket_url("memory", db).unwrap(), "memory");
        assert_eq!(bucket_url("file:/documents", db).unwrap(), "file:/documents/org_0123abcd");
        assert_eq!(bucket_url("file:///data/docs/", db).unwrap(), "file:/data/docs/org_0123abcd");
        assert_eq!(bucket_url("s3://my-bucket?region=eu-west-1", db).unwrap(), "s3://my-bucket?region=eu-west-1&prefix=org_0123abcd");
        assert_eq!(bucket_url("s3+http://storage.internal:9000/docs?prefix=eunomia/", db).unwrap(), "s3+http://storage.internal:9000/docs?prefix=eunomia%2Forg_0123abcd");
        assert!(bucket_url("s3://key:secret@host/bucket", db).is_err());
        assert!(bucket_url("s3://bucket?access_key=a&secret_key=b", db).is_err());
        assert!(bucket_url("file:relative", db).is_err() || bucket_url("file:relative", db).unwrap().starts_with("file:/"));
        assert_eq!(bucket_url("file:/a/../b", db).unwrap(), "file:/b/org_0123abcd", "the URL parser resolves dot segments");
        assert!(bucket_url("http://example.com", db).is_err());
        assert!(bucket_url("file:/documents", "org;DROP").is_err());
    }

    #[test]
    fn media_types_come_from_the_declared_type_then_the_extension() {
        assert_eq!(media_type("a.md", Some("text/markdown; charset=utf-8")), Some("text/markdown"));
        assert_eq!(media_type("a.md", None), Some("text/markdown"));
        assert_eq!(media_type("a.md", Some("application/octet-stream")), Some("text/markdown"));
        assert_eq!(media_type("a.PDF", Some("")), Some("application/pdf"));
        assert_eq!(media_type("a.txt", Some("text/plain")), Some("text/plain"));
        assert_eq!(media_type("a.csv", Some("application/vnd.ms-excel")), None);
        assert_eq!(media_type("a.exe", None), None);
        assert_eq!(media_type("a.docx", None), None);
    }

    #[test]
    fn filenames_lose_directories_and_object_keys_stay_safe() {
        assert_eq!(clean_filename("../../etc/passwd").unwrap(), "passwd");
        assert_eq!(clean_filename("C:\\Users\\me\\notes.md").unwrap(), "notes.md");
        assert!(clean_filename("  ").is_err() && clean_filename("..").is_err() && clean_filename("a\nb").is_err());
        assert_eq!(safe_segment("project notes (v2).md"), "project_notes__v2_.md");
        assert_eq!(safe_segment("..hidden"), "hidden");
        assert_eq!(safe_segment("ü"), "_");
        let vault = RecordId::from_table_key("vault", "v1");
        assert_eq!(object_path(&vault, "abc", 2, "My File.md"), "/v1/abc/2/My_File.md");
    }

    #[test]
    fn ids_must_be_plain_document_ids() {
        assert!(parse_id("document:abc123").is_ok());
        for bad in ["memory:abc", "document:", "document:a-b", "document:`x`", "abc"] {
            assert_eq!(parse_id(bad).unwrap_err().code, ErrorCode::DocumentNotFound, "{bad}");
        }
    }

    #[test]
    fn chunks_are_bounded_line_aware_and_rebuild_the_text() {
        let text = (0..400).map(|i| format!("line {i} of the notes")).collect::<Vec<_>>().join("\n");
        let parts = chunk(&text, 500);
        assert!(parts.len() > 1);
        assert_eq!(parts.iter().map(|p| p.2.as_str()).collect::<String>(), text);
        for (i, (s, e, piece)) in parts.iter().enumerate() {
            assert!(piece.chars().count() <= 500 && e - s == piece.chars().count());
            if i + 1 < parts.len() {
                assert!(piece.ends_with('\n'), "cut at a line end");
                assert_eq!(*e, parts[i + 1].0);
            }
        }
        // one long line is cut hard, multibyte characters intact
        let long = "é".repeat(1200);
        let parts = chunk(&long, 500);
        assert_eq!(parts.iter().map(|p| p.2.chars().count()).collect::<Vec<_>>(), vec![500, 500, 200]);
        assert!(chunk("", 500).is_empty());
    }

    #[test]
    fn tool_bytes_need_exactly_one_valid_input() {
        assert_eq!(tool_bytes(Some("hi".into()), None).unwrap(), b"hi");
        assert_eq!(tool_bytes(None, Some("aGk=\n".into())).unwrap(), b"hi");
        assert!(tool_bytes(None, Some("***".into())).is_err());
        assert!(tool_bytes(None, None).is_err() && tool_bytes(Some("a".into()), Some("YQ==".into())).is_err());
    }

    #[test]
    fn a_chunk_hit_names_its_document() {
        let p = json!({"document_id": "document:abc", "revision": 2, "part": 1, "char_start": 10, "char_end": 20, "filename": "n.md"});
        let r = chunk_ref("documents:document.chunk:abc:2:1", SOURCE, &p).unwrap();
        assert_eq!(r["download_url"], "/api/documents/document:abc/download");
        assert_eq!(r["revision"], 2);
        assert!(chunk_ref("x", "github", &p).is_none());
    }
}
