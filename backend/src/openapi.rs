//! The OpenAPI 3.1 document for the REST API, served at `GET /api/openapi.json`
//! and committed as `backend/openapi.json` (a test fails when they drift).
//! Each router module owns its `Doc` (the `#[utoipa::path]` annotations sit on
//! the handlers); this file merges them and adds the shared pieces.
//! `/mcp` is JSON-RPC and deliberately not described here.

use axum::Json;
use serde::Serialize;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi, ToSchema};

use crate::auth::SESSION_COOKIE;
use crate::routers;

/// RFC 9457 `application/problem+json`, as built in `error.rs`.
#[derive(Serialize, ToSchema)]
pub struct Problem {
    /// Always `about:blank`.
    pub r#type: String,
    /// The HTTP reason phrase.
    pub title: String,
    pub status: u16,
    pub detail: String,
    /// Stable dotted error code, see `docs/errors.md`.
    pub code: String,
    pub trace_id: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct OkBody {
    pub ok: bool,
    /// Set on `/readyz` when the server is up but running on the public empty `ENCRYPTION_KEY`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct DeletedBody {
    pub deleted: bool,
}

#[derive(Serialize, ToSchema)]
pub struct RemovedBody {
    pub removed: bool,
}

#[derive(Serialize, ToSchema)]
pub struct LeftBody {
    pub left: bool,
}

#[derive(Serialize, ToSchema)]
pub struct DeclinedBody {
    pub declined: bool,
}

struct Security;

impl Modify for Security {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme("cookie", SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new(SESSION_COOKIE))));
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).description(Some("API token")).build()),
        );
    }
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Eunomia API", version = "1"),
    paths(openapi_json, crate::readyz),
    components(schemas(Problem, OkBody, DeletedBody, RemovedBody, LeftBody, DeclinedBody)),
    modifiers(&Security),
)]
struct Root;

/// The merged document.
pub fn spec() -> utoipa::openapi::OpenApi {
    let mut doc = Root::openapi();
    for part in [
        routers::auth::Doc::openapi(),
        routers::audit::Doc::openapi(),
        routers::chat::Doc::openapi(),
        routers::connectors::Doc::openapi(),
        routers::debug::Doc::openapi(),
        routers::documents::Doc::openapi(),
        routers::entities::Doc::openapi(),
        routers::export::Doc::openapi(),
        routers::https::Doc::openapi(),
        routers::oauth::Doc::openapi(),
        routers::oidc::Doc::openapi(),
        routers::settings::Doc::openapi(),
        routers::sources::Doc::openapi(),
        routers::tools::Doc::openapi(),
        routers::update::Doc::openapi(),
        routers::vaults::Doc::openapi(),
    ] {
        doc.merge(part);
    }
    doc
}

/// The committed file's exact text.
pub fn spec_json() -> String {
    let mut s = serde_json::to_string_pretty(&spec()).expect("openapi serialises");
    s.push('\n');
    s
}

#[utoipa::path(
    operation_id = "getOpenapi",
    get,
    path = "/api/openapi.json",
    tag = "meta",
    summary = "This document",
    responses((status = 200, description = "OpenAPI 3.1 document", body = Object)),
    security(()),
)]
pub async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(spec())
}
