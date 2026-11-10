//! Scope-bound transport limits and the total deadline for an upload body.

use axum::{body::{Body, Bytes}, http::StatusCode, response::{IntoResponse, Response}};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::ratelimit::{LargeBodies, LargeBody};
use http_body_util::BodyExt;

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

/// Read the small budget without a slot. Acquire it before retaining any excess bytes;
/// an unscoped MCP caller is passed only the small limit and can never escalate.
pub(super) async fn read(body: Body, limit: usize, bodies: &LargeBodies, user: String) -> AppResult<(Bytes, Option<LargeBody<'_>>)> {
    tokio::time::timeout(READ_TIMEOUT, async move {
        let mut body = body;
        let mut bytes = Vec::new();
        let mut slot = None;
        let normal_limit = crate::max_body_bytes();
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|_| AppError::bad_request("could not read upload body"))?;
            let Ok(chunk) = frame.into_data() else { continue };
            let size = bytes.len().saturating_add(chunk.len());
            if size > limit {
                return Err(AppError::coded(ErrorCode::DocumentTooLarge, format!("request body exceeds the {limit} byte transport limit")));
            }
            if size > normal_limit && slot.is_none() {
                slot = Some(bodies.acquire(user.clone()).ok_or_else(|| AppError::new(StatusCode::TOO_MANY_REQUESTS, "another large upload is in progress"))?);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok((Bytes::from(bytes), slot))
    }).await.map_err(|_| AppError::new(StatusCode::REQUEST_TIMEOUT, "upload body exceeded the 120 second read deadline"))?
}
