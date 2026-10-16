//! Uploaded documents: upload, list, get, download, export, delete (`documents/`).
//! Part of the tool registry (see `registry/mod.rs`); one `register` call per tool. Bytes never travel
//! in a tool result: `document_download` and `document_export` hand out the authenticated HTTP routes.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};

use super::{bad_args, register, to_tool_value, ToolSpec};
use crate::documents::{self, Upload};

#[derive(serde::Deserialize)]
struct IdArgs {
    id: String,
}

fn http_route(settings: &crate::config::Settings, path: &str) -> Value {
    json!({ "method": "GET", "path": path, "url": format!("{}{path}", settings.public_url) })
}

pub(super) fn register_all(registry: &mut HashMap<&'static str, ToolSpec>) {
    #[derive(serde::Deserialize)]
    struct UploadArgs {
        filename: String,
        #[serde(default)]
        content_type: Option<String>,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        content_base64: Option<String>,
        #[serde(default)]
        document_id: Option<String>,
        #[serde(default)]
        vault_id: Option<String>,
    }

    register(
        registry,
        "document_upload",
        json!({
            "type": "object",
            "properties": {
                "filename": {"type": "string", "description": "e.g. project-notes.md; the extension picks the type when content_type is omitted"},
                "content_type": {"type": "string", "enum": documents::MEDIA_TYPES},
                "text": {"type": "string", "description": "the document as UTF-8 text (plain text, Markdown, JSON, CSV)"},
                "content_base64": {"type": "string", "description": "the document's bytes in base64 (for a PDF), instead of text"},
                "document_id": {"type": "string", "description": "upload a new version of this document instead of a new one"},
                "vault_id": {"type": "string", "description": "put a new document in this vault (a vault id or its name, e.g. \"Acme\"); omit for your personal vault"},
            },
            "required": ["filename"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: UploadArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let bytes = match documents::tool_bytes(a.text, a.content_base64) {
                    Ok(b) => b,
                    Err(e) => return Ok(e.to_tool_value()),
                };
                let up = Upload { filename: a.filename, media_type: a.content_type, bytes, replace: a.document_id, vault: a.vault_id };
                Ok(to_tool_value(documents::upload(state, owner, up).await))
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct ListArgs {
        #[serde(default = "default_limit")]
        limit: i64,
        #[serde(default)]
        offset: i64,
        #[serde(default)]
        vault_id: Option<String>,
    }
    fn default_limit() -> i64 {
        50
    }

    register(
        registry,
        "document_list",
        json!({
            "type": "object",
            "properties": {
                "limit": {"type": "integer", "description": "at most 200, default 50"},
                "offset": {"type": "integer"},
                "vault_id": {"type": "string", "description": "list this vault's documents (a vault id or its name); omit for your personal vault"},
            },
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: ListArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                Ok(to_tool_value(documents::list(state, owner, a.vault_id.as_deref(), a.limit, a.offset).await))
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct GetArgs {
        id: String,
        #[serde(default)]
        max_chars: Option<usize>,
    }

    register(
        registry,
        "document_get",
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "a document id (document:...)"},
                "max_chars": {"type": "integer", "description": "how much of the extracted text to return, default 100000"},
            },
            "required": ["id"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: GetArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                Ok(to_tool_value(documents::get(state, owner, &a.id, a.max_chars).await))
            })
        }),
    );

    register(
        registry,
        "document_download",
        json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: IdArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                // the same checks as the download itself; the bytes go over HTTP, never through the tool
                let detail = match documents::get(state, owner, &a.id, Some(1)).await {
                    Ok(d) => d,
                    Err(e) => return Ok(e.to_tool_value()),
                };
                let d = detail.document;
                Ok(json!({
                    "document_id": d.id,
                    "filename": d.filename,
                    "media_type": d.media_type,
                    "size_bytes": d.size_bytes,
                    "sha256": d.sha256,
                    "download": http_route(&state.settings, &d.download_url),
                    "auth": "send the same Authorization: Bearer token you use for MCP",
                }))
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct ExportArgs {
        id: String,
        #[serde(default)]
        include_vectors: bool,
    }

    register(
        registry,
        "document_export",
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string"},
                "include_vectors": {"type": "boolean", "description": "inline every chunk's 1536 numbers (default false: fetch them over HTTP from vectors_url)"},
            },
            "required": ["id"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: ExportArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let mut manifest = match documents::export(state, owner, &a.id, a.include_vectors).await {
                    Ok(m) => m,
                    Err(e) => return Ok(e.to_tool_value()),
                };
                let path = format!("/api/documents/{}/export", manifest["document"]["id"].as_str().unwrap_or_default());
                manifest["vectors_url"] = http_route(&state.settings, &path);
                Ok(manifest)
            })
        }),
    );

    register(
        registry,
        "document_delete",
        json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: IdArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                Ok(to_tool_value(documents::delete(state, owner, &a.id).await))
            })
        }),
    );
}
