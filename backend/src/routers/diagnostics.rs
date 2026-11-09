//! Browser-session-only support download. The collector owns privacy and size limits.
use axum::{routing::post, Router};
use crate::{diagnostics::download, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new().route("/diagnostics", post(download))
}
