//! Scope-bound transport limits and the total deadline for an upload body.

use axum::{body::{Body, Bytes}, http::StatusCode, response::{IntoResponse, Response}};
use crate::error::{AppError, AppResult, ErrorCode};

const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

pub(super) fn allowed() -> bool {
    crate::authz::current().is_some_and(|c| c.allows(crate::scopes::for_tool("document_upload", false)))
}

pub(super) fn preflight(limit: usize, upload: bool) -> Response {
    ([("x-eunomia-body-limit", limit.to_string()), ("x-eunomia-upload-allowed", if upload { "1" } else { "0" }.to_string())], StatusCode::OK).into_response()
}

pub(super) fn scope_denied() -> AppError {
    AppError::coded(ErrorCode::AuthScope, "document upload requires memory:write")
}

pub(super) async fn read(body: Body, limit: usize) -> AppResult<Bytes> {
    tokio::time::timeout(READ_TIMEOUT, axum::body::to_bytes(body, limit)).await
        .map_err(|_| AppError::new(StatusCode::REQUEST_TIMEOUT, "upload body exceeded the 120 second read deadline"))?
        .map_err(|_| AppError::coded(ErrorCode::DocumentTooLarge, format!("request body exceeds the {limit} byte transport limit")))
}
