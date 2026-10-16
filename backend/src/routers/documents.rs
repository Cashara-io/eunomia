//! Document routes: thin wrappers over `documents/` (the same functions the `document_*` tools call).
//! Upload takes the raw file as the body (no multipart), so this router gets its own body limit
//! (`EUNOMIA_DOCUMENTS_MAX_BYTES`, see `lib.rs`). Downloads and exports are attachments and are
//! recorded in the audit log by document id, without content.

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

use crate::documents::{self, DocumentDeleted, DocumentDetail, DocumentList, DocumentOut, Upload};
use crate::error::{AppError, AppResult};
use crate::models_user::User;
use crate::state::{AppState, OrgState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/documents", get(list_documents).post(upload_document))
        .route("/documents/{document_id}", get(get_document).delete(delete_document))
        .route("/documents/{document_id}/download", get(download_document))
        .route("/documents/{document_id}/export", get(export_document))
        .route("/documents/{document_id}/reindex", post(reindex_document))
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(list_documents, upload_document, get_document, delete_document, download_document, export_document, reindex_document),
    components(schemas(DocumentOut, DocumentList, DocumentDetail, DocumentDeleted, documents::ChunkRef))
)]
pub struct Doc;

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    /// At most 200, default 50.
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
    /// The vault (`vault:...`); default: the caller's personal vault (a vault-restricted token's own).
    #[serde(default)]
    vault_id: Option<String>,
}

#[utoipa::path(
    operation_id = "listDocuments",
    get,
    path = "/api/documents",
    tag = "documents",
    summary = "List uploaded documents, newest first",
    params(ListQuery),
    responses((status = 200, body = DocumentList), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_documents(State(state): State<AppState>, user: User, Query(q): Query<ListQuery>) -> AppResult<Json<DocumentList>> {
    let state = state.org(&user.org).await?;
    Ok(Json(documents::list(&state, &user.id, q.vault_id.as_deref(), q.limit.unwrap_or(50), q.offset.unwrap_or(0)).await?))
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct UploadQuery {
    /// The file's name (shown, and used for the download); its extension decides the type when the
    /// body's Content-Type is missing or generic.
    filename: Option<String>,
    /// Overrides the request's Content-Type.
    #[serde(default)]
    content_type: Option<String>,
    /// Publish the body as a new revision of this document instead of a new document.
    #[serde(default)]
    replace: Option<String>,
    /// The vault a new document goes in (`vault:...`); default: the caller's personal vault (a vault-restricted token's own).
    #[serde(default)]
    vault_id: Option<String>,
}

#[utoipa::path(
    operation_id = "uploadDocument",
    post,
    path = "/api/documents",
    tag = "documents",
    summary = "Upload a document (the request body is the file)",
    params(UploadQuery),
    request_body(content = String, description = "The file's bytes", content_type = "application/octet-stream"),
    responses((status = 200, body = DocumentOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn upload_document(State(state): State<AppState>, user: User, Query(q): Query<UploadQuery>, headers: HeaderMap, body: Bytes) -> AppResult<Json<DocumentOut>> {
    let state = state.org(&user.org).await?;
    let filename = q.filename.filter(|f| !f.trim().is_empty()).ok_or_else(|| AppError::bad_request("the filename query parameter is required"))?;
    let declared = q.content_type.or_else(|| headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_string));
    let args = json!({ "filename": filename, "replace": q.replace, "vault_id": q.vault_id, "bytes": body.len(), "via": "http" });
    let up = Upload { filename, media_type: declared, bytes: body.to_vec(), replace: q.replace, vault: q.vault_id };
    let out = documents::upload(&state, &user.id, up).await;
    record(&state, &user, "document_upload", &args, &out).await;
    Ok(Json(out?))
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct GetQuery {
    /// How much of the extracted text to return, default 100000 characters.
    #[serde(default)]
    max_chars: Option<usize>,
}

#[utoipa::path(
    operation_id = "getDocument",
    get,
    path = "/api/documents/{document_id}",
    tag = "documents",
    summary = "One document with its extracted text and chunk references",
    params(("document_id" = String, Path), GetQuery),
    responses((status = 200, body = DocumentDetail), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_document(State(state): State<AppState>, user: User, Path(document_id): Path<String>, Query(q): Query<GetQuery>) -> AppResult<Json<DocumentDetail>> {
    let state = state.org(&user.org).await?;
    Ok(Json(documents::get(&state, &user.id, &document_id, q.max_chars).await?))
}

#[utoipa::path(
    operation_id = "deleteDocument",
    delete,
    path = "/api/documents/{document_id}",
    tag = "documents",
    summary = "Delete a document, its stored file, its chunks and what was extracted from them",
    params(("document_id" = String, Path)),
    responses((status = 200, body = DocumentDeleted), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn delete_document(State(state): State<AppState>, user: User, Path(document_id): Path<String>) -> AppResult<Json<DocumentDeleted>> {
    let state = state.org(&user.org).await?;
    let out = documents::delete(&state, &user.id, &document_id).await;
    audit(&state, &user, "document_delete", &document_id, &out).await;
    Ok(Json(out?))
}

#[utoipa::path(
    operation_id = "reindexDocument",
    post,
    path = "/api/documents/{document_id}/reindex",
    tag = "documents",
    summary = "Extract and index the stored file again, as a new revision",
    params(("document_id" = String, Path)),
    responses((status = 200, body = DocumentOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn reindex_document(State(state): State<AppState>, user: User, Path(document_id): Path<String>) -> AppResult<Json<DocumentOut>> {
    let state = state.org(&user.org).await?;
    let out = documents::reindex(&state, &user.id, &document_id).await;
    audit(&state, &user, "document_reindex", &document_id, &out).await;
    Ok(Json(out?))
}

// open body: the original bytes as an attachment, not consumed through the typed client
#[utoipa::path(
    operation_id = "downloadDocument",
    get,
    path = "/api/documents/{document_id}/download",
    tag = "documents",
    summary = "Download the original file, byte for byte",
    params(("document_id" = String, Path)),
    responses((status = 200, description = "The original bytes", content_type = "application/octet-stream", body = String), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn download_document(State(state): State<AppState>, user: User, Path(document_id): Path<String>) -> AppResult<Response> {
    let state = state.org(&user.org).await?;
    let out = documents::download(&state, &user.id, &document_id).await;
    audit(&state, &user, "document_download", &document_id, &out).await;
    let (doc, bytes) = out?;
    let content_type = if doc.media_type.starts_with("text/") { format!("{}; charset=utf-8", doc.media_type) } else { doc.media_type.clone() };
    Ok(attachment(&content_type, &doc.filename, bytes))
}

// open body: a downloadable manifest (Content-Disposition attachment), not consumed through the typed client
#[utoipa::path(
    operation_id = "exportDocument",
    get,
    path = "/api/documents/{document_id}/export",
    tag = "documents",
    summary = "Download the document's export manifest: chunks, locations and vectors",
    params(("document_id" = String, Path)),
    responses((status = 200, body = Object), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn export_document(State(state): State<AppState>, user: User, Path(document_id): Path<String>) -> AppResult<Response> {
    let state = state.org(&user.org).await?;
    let out = documents::export(&state, &user.id, &document_id, true).await;
    audit(&state, &user, "document_export", &document_id, &out).await;
    let manifest = out?;
    let name = format!("{}.export.json", manifest["document"]["filename"].as_str().unwrap_or("document"));
    let body = serde_json::to_vec_pretty(&manifest).map_err(|e| AppError::internal(e.to_string()))?;
    Ok(attachment("application/json", &name, body))
}

/// The owner's audit row and the control ledger event, by document id only.
async fn audit<T>(state: &OrgState, user: &User, action: &str, document_id: &str, out: &AppResult<T>) {
    record(state, user, action, &json!({ "id": document_id, "via": "http" }), out).await;
}

async fn record<T>(state: &OrgState, user: &User, action: &str, args: &serde_json::Value, out: &AppResult<T>) {
    let (outcome, code) = match out {
        Ok(_) => ("ok", "ok"),
        Err(e) => ("error", e.code.as_str()),
    };
    crate::tools::registry::record_audit(state, &user.id, action, args, outcome, code).await;
}

/// `attachment` with the name both as plain ASCII and RFC 5987 UTF-8, and no sniffing.
fn attachment(content_type: &str, filename: &str, body: Vec<u8>) -> Response {
    let ascii: String = filename.chars().map(|c| if (c.is_ascii_graphic() && c != '"' && c != '\\') || c == ' ' { c } else { '_' }).collect();
    let encoded: String = filename
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect();
    let disposition = format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}");
    let mut resp = body.into_response();
    let headers = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(content_type) {
        headers.insert(header::CONTENT_TYPE, v);
    }
    if let Ok(v) = HeaderValue::from_str(&disposition) {
        headers.insert(header::CONTENT_DISPOSITION, v);
    }
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_names_are_safe_in_the_header() {
        let r = attachment("text/plain", "notes \"q3\" é.md", vec![1]);
        let d = r.headers()[header::CONTENT_DISPOSITION].to_str().unwrap();
        assert_eq!(d, "attachment; filename=\"notes _q3_ _.md\"; filename*=UTF-8''notes%20%22q3%22%20%C3%A9.md");
        assert_eq!(r.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    }
}
