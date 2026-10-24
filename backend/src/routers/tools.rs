//! REST surface for the tool registry -- the catalogue is public (schema
//! only, no data); calling a tool requires auth since it reads the caller's
//! owned data.
//!
//! `all_tools()` and `call()` pick up new registry entries automatically.

use axum::{
    extract::{Extension, Path, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::Value;

use crate::models_user::User;
use crate::state::AppState;
use crate::tools::registry;

pub fn router() -> Router<AppState> {
    Router::new().route("/tools", get(catalogue)).route("/tools/{name}", post(invoke))
}

#[utoipa::path(
    operation_id = "listTools",
    get,
    path = "/api/tools",
    tag = "tools",
    summary = "Names of the available tools",
    responses((status = 200, body = Vec<String>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(()),
)]
async fn catalogue() -> Json<Value> {
    // Names only: the tools' JSON schemas are served by the MCP `tools/list` call.
    let names: Vec<&str> = registry::all_tools().keys().copied().collect();
    Json(serde_json::json!(names))
}

#[utoipa::path(
    operation_id = "invokeTool",
    post,
    path = "/api/tools/{name}",
    tag = "tools",
    summary = "Invoke a tool",
    params(("name" = String, Path)),
    request_body(content = Object, description = "Tool arguments"),
    // open body: each tool defines its own result shape
    responses((status = 200, body = Object), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn invoke(
    State(state): State<AppState>,
    user: User,
    meter: Option<Extension<crate::gate::Meter>>,
    Path(name): Path<String>,
    body: Option<Json<Value>>,
) -> Response {
    if !registry::all_tools().contains_key(name.as_str()) {
        return crate::error::AppError::coded(crate::error::ErrorCode::ToolNotFound, format!("unknown tool {name:?}")).into_response();
    }
    let args = body.map(|Json(v)| v).unwrap_or(Value::Object(serde_json::Map::new()));
    // the gate charged one request; a batch tool costs more (`registry::extra_cost`)
    if let Some(Extension(m)) = &meter
        && let Err(wait) = m.charge(registry::extra_cost(&name, &args))
    {
        return crate::gate::rate_limited(wait);
    }
    match registry::call(&state, &user, &name, args).await {
        Ok(result) => Json(result).into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    catalogue,
    invoke,
))]
pub struct Doc;
