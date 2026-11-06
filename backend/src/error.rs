//! One error type for the whole backend. Every error has a stable dotted
//! `ErrorCode` (documented in `docs/errors.md`) and leaves the process as an
//! RFC 9457 `application/problem+json` body carrying that code and the
//! request's trace id. Raw database or upstream text goes to the log only
//! (`AppError::source`), never into a response.

use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;

tokio::task_local! {
    /// Set by `registry::call` so the raw cause of a tool error (which `to_tool_value` hides) reaches the failure capsule.
    pub static TOOL_SOURCE: std::sync::Arc<std::sync::Mutex<Option<String>>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    AuthUnauthorized,
    AuthForbidden,
    AuthEmailTaken,
    AuthNotFound,
    AuthScope,
    AuthTokenExpired,
    AuthSessionExpired,
    AuthPasswordDisabled,
    RateLimited,
    TenantDenied,
    TenantNotFound,
    TenantSchemaBehind,
    TenantProvisioningDisabled,
    VaultNotFound,
    VaultForbidden,
    VaultNameTaken,
    EntityNotFound,
    MemoryNotFound,
    ChatThreadNotFound,
    ConnectorNotFound,
    ConnectorNotConnected,
    SourceNotFound,
    ExtensionNotFound,
    ExtensionDisabled,
    ExtensionBufferFull,
    ToolNotFound,
    ResourceNotFound,
    DocumentNotFound,
    DocumentTooLarge,
    DocumentUnsupportedType,
    DocumentStorageUnavailable,
    ChangesResync,
    ValidationInvalid,
    DbConflict,
    DbDuplicate,
    JobNoHandler,
    JobLeaseExpired,
    JobPanicked,
    Internal,
}

impl ErrorCode {
    pub const ALL: &'static [ErrorCode] = &[
        ErrorCode::AuthUnauthorized,
        ErrorCode::AuthForbidden,
        ErrorCode::AuthEmailTaken,
        ErrorCode::AuthNotFound,
        ErrorCode::AuthScope,
        ErrorCode::AuthTokenExpired,
        ErrorCode::AuthSessionExpired,
        ErrorCode::AuthPasswordDisabled,
        ErrorCode::RateLimited,
        ErrorCode::TenantDenied,
        ErrorCode::TenantNotFound,
        ErrorCode::TenantSchemaBehind,
        ErrorCode::TenantProvisioningDisabled,
        ErrorCode::VaultNotFound,
        ErrorCode::VaultForbidden,
        ErrorCode::VaultNameTaken,
        ErrorCode::EntityNotFound,
        ErrorCode::MemoryNotFound,
        ErrorCode::ChatThreadNotFound,
        ErrorCode::ConnectorNotFound,
        ErrorCode::ConnectorNotConnected,
        ErrorCode::SourceNotFound,
        ErrorCode::ExtensionNotFound,
        ErrorCode::ExtensionDisabled,
        ErrorCode::ExtensionBufferFull,
        ErrorCode::ToolNotFound,
        ErrorCode::ResourceNotFound,
        ErrorCode::DocumentNotFound,
        ErrorCode::DocumentTooLarge,
        ErrorCode::DocumentUnsupportedType,
        ErrorCode::DocumentStorageUnavailable,
        ErrorCode::ChangesResync,
        ErrorCode::ValidationInvalid,
        ErrorCode::DbConflict,
        ErrorCode::DbDuplicate,
        ErrorCode::JobNoHandler,
        ErrorCode::JobLeaseExpired,
        ErrorCode::JobPanicked,
        ErrorCode::Internal,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::AuthUnauthorized => "auth.unauthorized",
            ErrorCode::AuthForbidden => "auth.forbidden",
            ErrorCode::AuthEmailTaken => "auth.email_taken",
            ErrorCode::AuthNotFound => "auth.not_found",
            ErrorCode::AuthScope => "auth.scope",
            ErrorCode::AuthTokenExpired => "auth.token_expired",
            ErrorCode::AuthSessionExpired => "auth.session_expired",
            ErrorCode::AuthPasswordDisabled => "auth.password_disabled",
            ErrorCode::RateLimited => "rate.limited",
            ErrorCode::TenantDenied => "tenant.denied",
            ErrorCode::TenantNotFound => "tenant.not_found",
            ErrorCode::TenantSchemaBehind => "tenant.schema_behind",
            ErrorCode::TenantProvisioningDisabled => "tenant.provisioning_disabled",
            ErrorCode::VaultNotFound => "vault.not_found",
            ErrorCode::VaultForbidden => "vault.forbidden",
            ErrorCode::VaultNameTaken => "vault.name_taken",
            ErrorCode::EntityNotFound => "entity.not_found",
            ErrorCode::MemoryNotFound => "memory.not_found",
            ErrorCode::ChatThreadNotFound => "chat.thread_not_found",
            ErrorCode::ConnectorNotFound => "connector.not_found",
            ErrorCode::ConnectorNotConnected => "connector.not_connected",
            ErrorCode::SourceNotFound => "source.not_found",
            ErrorCode::ExtensionNotFound => "extension.not_found",
            ErrorCode::ExtensionDisabled => "extension.disabled",
            ErrorCode::ExtensionBufferFull => "extension.buffer_full",
            ErrorCode::ToolNotFound => "tool.not_found",
            ErrorCode::ResourceNotFound => "resource.not_found",
            ErrorCode::DocumentNotFound => "document.not_found",
            ErrorCode::DocumentTooLarge => "document.too_large",
            ErrorCode::DocumentUnsupportedType => "document.unsupported_type",
            ErrorCode::DocumentStorageUnavailable => "document.storage_unavailable",
            ErrorCode::ChangesResync => "changes.resync",
            ErrorCode::ValidationInvalid => "validation.invalid",
            ErrorCode::DbConflict => "db.conflict",
            ErrorCode::DbDuplicate => "db.duplicate",
            ErrorCode::JobNoHandler => "job.no_handler",
            ErrorCode::JobLeaseExpired => "job.lease_expired",
            ErrorCode::JobPanicked => "job.panicked",
            ErrorCode::Internal => "internal",
        }
    }

    pub fn default_status(self) -> StatusCode {
        match self {
            ErrorCode::AuthUnauthorized | ErrorCode::AuthTokenExpired | ErrorCode::AuthSessionExpired => StatusCode::UNAUTHORIZED,
            ErrorCode::AuthForbidden | ErrorCode::AuthPasswordDisabled | ErrorCode::AuthScope | ErrorCode::TenantDenied | ErrorCode::VaultForbidden | ErrorCode::ExtensionDisabled => {
                StatusCode::FORBIDDEN
            }
            ErrorCode::RateLimited | ErrorCode::ExtensionBufferFull => StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::TenantSchemaBehind | ErrorCode::DocumentStorageUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::DocumentTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ErrorCode::DocumentUnsupportedType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ErrorCode::ChangesResync => StatusCode::GONE,
            ErrorCode::TenantProvisioningDisabled => StatusCode::NOT_IMPLEMENTED,
            ErrorCode::AuthEmailTaken | ErrorCode::VaultNameTaken | ErrorCode::DbConflict | ErrorCode::DbDuplicate => StatusCode::CONFLICT,
            ErrorCode::AuthNotFound
            | ErrorCode::VaultNotFound
            | ErrorCode::EntityNotFound
            | ErrorCode::MemoryNotFound
            | ErrorCode::ChatThreadNotFound
            | ErrorCode::ConnectorNotFound
            | ErrorCode::SourceNotFound
            | ErrorCode::ExtensionNotFound
            | ErrorCode::ToolNotFound
            | ErrorCode::TenantNotFound
            | ErrorCode::ResourceNotFound
            | ErrorCode::DocumentNotFound => StatusCode::NOT_FOUND,
            ErrorCode::ConnectorNotConnected | ErrorCode::ValidationInvalid => StatusCode::BAD_REQUEST,
            ErrorCode::JobNoHandler | ErrorCode::JobLeaseExpired | ErrorCode::JobPanicked | ErrorCode::Internal => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }

    /// Codes whose message may hold internals, so callers outside the process
    /// see a generic detail instead.
    fn hides_message(self) -> bool {
        matches!(self, ErrorCode::Internal | ErrorCode::TenantDenied)
    }

    /// A code for a bare status, for the `AppError::new(status, ..)` callers. A caller that
    /// means a more specific code (`vault.forbidden`) uses [`AppError::coded`].
    fn from_status(status: StatusCode) -> Self {
        match status {
            StatusCode::BAD_REQUEST => ErrorCode::ValidationInvalid,
            StatusCode::UNAUTHORIZED => ErrorCode::AuthUnauthorized,
            StatusCode::FORBIDDEN => ErrorCode::AuthForbidden,
            StatusCode::TOO_MANY_REQUESTS => ErrorCode::RateLimited,
            StatusCode::NOT_FOUND => ErrorCode::ResourceNotFound,
            StatusCode::CONFLICT => ErrorCode::DbConflict,
            _ => ErrorCode::Internal,
        }
    }
}

#[derive(Debug)]
pub struct AppError {
    pub code: ErrorCode,
    pub status: StatusCode,
    /// For `internal` and `tenant.denied` this can hold raw upstream text;
    /// use `detail()` for anything that leaves the process.
    pub message: String,
    /// Raw cause (database or upstream text). Logged, never sent.
    pub source: Option<String>,
}

impl AppError {
    pub fn coded(code: ErrorCode, message: impl Into<String>) -> Self {
        AppError { code, status: code.default_status(), message: message.into(), source: None }
    }

    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        AppError { code: ErrorCode::from_status(status), status, message: message.into(), source: None }
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::coded(ErrorCode::AuthUnauthorized, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::coded(ErrorCode::ResourceNotFound, message)
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::coded(ErrorCode::ValidationInvalid, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::coded(ErrorCode::Internal, message)
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// The message that is safe to show a client or agent.
    pub fn detail(&self) -> &str {
        if self.code.hides_message() {
            "Internal server error."
        } else {
            &self.message
        }
    }

    /// The `{error, code, trace_id}` shape tools return. The raw cause is parked for the failure capsule.
    pub fn to_tool_value(&self) -> serde_json::Value {
        if let Some(s) = &self.source {
            let _ = TOOL_SOURCE.try_with(|c| *c.lock().unwrap() = Some(s.clone()));
        }
        json!({ "error": self.detail(), "code": self.code.as_str(), "trace_id": crate::telemetry::current_trace_id() })
    }

    /// Logs the cause on the current span. Called once, where the error leaves the process.
    fn log(&self) {
        let source = self.source.as_deref().unwrap_or(&self.message);
        if self.code == ErrorCode::TenantDenied {
            tracing::error!(code = self.code.as_str(), %source, "tenant isolation denied a query");
        } else if self.status.is_server_error() {
            tracing::error!(code = self.code.as_str(), %source, "request failed");
        } else {
            tracing::debug!(code = self.code.as_str(), %source, "request rejected");
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        self.log();
        let info = crate::capsules::FailureInfo { code: self.code, source: self.source.clone().unwrap_or_else(|| self.message.clone()) };
        let body = json!({
            "type": "about:blank",
            "title": self.status.canonical_reason().unwrap_or("Error"),
            "status": self.status.as_u16(),
            "detail": self.detail(),
            "code": self.code.as_str(),
            "trace_id": crate::telemetry::current_trace_id(),
        });
        let mut resp = (self.status, [(header::CONTENT_TYPE, "application/problem+json")], body.to_string()).into_response();
        resp.extensions_mut().insert(info);
        resp
    }
}

impl From<surrealdb::Error> for AppError {
    fn from(err: surrealdb::Error) -> Self {
        let raw = err.to_string();
        let lower = raw.to_lowercase();
        let (code, message) = match &err {
            _ if crate::tx::is_conflict(&err) => (ErrorCode::DbConflict, "The write conflicted with another; retry it."),
            _ if lower.contains(crate::store::CROSSING_MARK) => (ErrorCode::TenantDenied, "Internal server error."),
            _ if err.is_not_allowed() => (ErrorCode::TenantDenied, "Internal server error."),
            _ if lower.contains("you don't have permission") || lower.contains("not enough permissions") => {
                (ErrorCode::TenantDenied, "Internal server error.")
            }
            _ if err.is_already_exists() || lower.contains("already contains") => (ErrorCode::DbDuplicate, "That record already exists."),
            _ => (ErrorCode::Internal, "Internal server error."),
        };
        AppError::coded(code, message).with_source(raw)
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_code_is_documented() {
        let docs = include_str!("../../docs/errors.md");
        for code in ErrorCode::ALL {
            assert!(docs.contains(&format!("`{}`", code.as_str())), "docs/errors.md is missing {}", code.as_str());
        }
    }

    #[test]
    fn all_lists_every_variant_once() {
        let mut names: Vec<_> = ErrorCode::ALL.iter().map(|c| c.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), ErrorCode::ALL.len());
        // exhaustive match: adding a variant fails to compile until it is handled here and in ALL
        for c in ErrorCode::ALL {
            match c {
                ErrorCode::AuthUnauthorized | ErrorCode::AuthForbidden | ErrorCode::AuthEmailTaken | ErrorCode::AuthNotFound
                | ErrorCode::AuthScope | ErrorCode::AuthTokenExpired | ErrorCode::AuthSessionExpired | ErrorCode::AuthPasswordDisabled | ErrorCode::RateLimited
                | ErrorCode::TenantDenied | ErrorCode::TenantNotFound | ErrorCode::TenantSchemaBehind
                | ErrorCode::TenantProvisioningDisabled | ErrorCode::VaultNotFound | ErrorCode::VaultForbidden | ErrorCode::VaultNameTaken | ErrorCode::EntityNotFound
                | ErrorCode::MemoryNotFound | ErrorCode::ChatThreadNotFound | ErrorCode::ConnectorNotFound
                | ErrorCode::ConnectorNotConnected | ErrorCode::SourceNotFound | ErrorCode::ExtensionNotFound | ErrorCode::ExtensionDisabled | ErrorCode::ExtensionBufferFull | ErrorCode::ToolNotFound
                | ErrorCode::ResourceNotFound | ErrorCode::DocumentNotFound | ErrorCode::DocumentTooLarge
                | ErrorCode::DocumentUnsupportedType | ErrorCode::DocumentStorageUnavailable | ErrorCode::ChangesResync | ErrorCode::ValidationInvalid | ErrorCode::DbConflict
                | ErrorCode::DbDuplicate | ErrorCode::JobNoHandler | ErrorCode::JobLeaseExpired | ErrorCode::JobPanicked
                | ErrorCode::Internal => {}
            }
        }
    }

    #[test]
    fn bare_statuses_map_to_their_own_codes() {
        let code = |s: StatusCode| AppError::new(s, "x").code.as_str();
        assert_eq!(code(StatusCode::TOO_MANY_REQUESTS), "rate.limited");
        assert_eq!(code(StatusCode::FORBIDDEN), "auth.forbidden");
        assert_eq!(code(StatusCode::BAD_GATEWAY), "internal");
    }

    #[test]
    fn internal_detail_never_leaks_the_source() {
        let e = AppError::internal("select * from secret: boom");
        assert_eq!(e.detail(), "Internal server error.");
        assert!(!e.to_tool_value().to_string().contains("secret"));
    }
}
