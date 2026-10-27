pub mod audit;
pub mod auth;
pub mod authz;
pub mod cache;
pub mod capsules;
pub mod changes;
pub mod chat;
pub mod config;
pub mod connectors;
pub mod db;
pub mod docs;
pub mod documents;
pub mod embeddings;
pub mod entities;
pub mod error;
pub mod extensions;
pub mod gate;
#[cfg(feature = "test-support")]
pub mod isolation;
pub mod jobs;
pub mod llm_net;
pub mod migrate;
pub mod models_user;
pub mod oauth;
pub mod oidc;
pub mod openapi;
pub mod pool;
pub mod provisioning;
pub mod redact;
pub mod rid;
pub mod ratelimit;
pub mod replay;
pub mod routers;
pub mod scopes;
pub mod sources;
pub mod state;
pub mod store;
pub mod telemetry;
pub mod tools;
pub mod tx;
pub mod usage;
pub mod vaults;

use axum::http::{header, HeaderName, HeaderValue, Method};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::state::AppState;

/// The full HTTP app (CORS included), shared by `main` and the tests.
pub fn app(state: AppState) -> axum::Router {
    app_with(state, ratelimit::RateConfig::from_env())
}

/// [`app`] with explicit rate limits (the tests use tiny ones).
pub fn app_with(state: AppState, limits: ratelimit::RateConfig) -> axum::Router {
    let allowed_origins: Vec<HeaderValue> = state
        .settings
        .cors_allowed_origins
        .split(',')
        .map(str::trim)
        .filter(|o| !o.is_empty())
        .filter_map(|o| o.parse().ok())
        .collect();

    let cors = CorsLayer::new()
        .allow_origin(allowed_origins)
        .allow_credentials(true)
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::PUT, Method::DELETE])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION, HeaderName::from_static("traceparent")])
        .expose_headers([HeaderName::from_static("x-trace-id")]);

    let api = axum::Router::new()
        .route("/openapi.json", axum::routing::get(openapi::openapi_json))
        .merge(routers::auth::router())
        .merge(routers::chat::router())
        .merge(routers::settings::router())
        .merge(routers::audit::router())
        .merge(routers::export::router())
        .merge(routers::debug::router())
        .merge(routers::update::router())
        .merge(routers::https::router())
        .merge(routers::vaults::router())
        .merge(routers::changes::router())
        .merge(routers::connectors::router())
        .merge(routers::connectors::snapshot_router())
        .merge(routers::entities::router())
        .merge(routers::tools::router())
        .merge(routers::sources::router())
        .merge(routers::sources::webhook_router())
        .merge(routers::extensions::router())
        .merge(routers::oauth::router())
        .merge(routers::oidc::router());

    // Browser-based MCP clients (for example MCP Inspector) run on any origin and
    // authenticate with a bearer token only, so these routes answer CORS for any
    // origin and never allow credentials: no cookie is ever sent or honoured here.
    // The ambient-credential routes (`/api`, which accept the session cookie) keep
    // the single-origin credentialed CORS above.
    let open_cors = CorsLayer::new()
        .allow_origin(AllowOrigin::any())
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers([
            header::CONTENT_TYPE,
            header::AUTHORIZATION,
            header::ACCEPT,
            HeaderName::from_static("mcp-protocol-version"),
            HeaderName::from_static("mcp-session-id"),
            HeaderName::from_static("traceparent"),
        ])
        .expose_headers([header::WWW_AUTHENTICATE, HeaderName::from_static("x-trace-id"), HeaderName::from_static("mcp-session-id")]);

    // CORS wraps the gate so a preflight is answered before authentication.
    let gate = gate::Gate::new(state.clone(), limits);
    let capture_state = state.clone();
    let guarded = |routes: axum::Router<AppState>, cors: CorsLayer| {
        routes
            .layer(axum::middleware::from_fn_with_state(capture_state.clone(), capsules::capture))
            .layer(axum::middleware::from_fn_with_state(gate.clone(), gate::gate))
            .layer(axum::middleware::from_fn(telemetry::trace_request))
            .layer(cors)
    };

    let open = axum::Router::new().route("/healthz", axum::routing::get(healthz)).route("/readyz", axum::routing::get(readyz)).merge(routers::mcp::router()).merge(routers::extensions::hook_router()).merge(oauth::router());
    // One ceiling for every request body, ahead of everything else (a handler that reads the body
    // itself, like the webhook, is bounded too); raise it with the env var. Document uploads are the
    // exception: their router has its own ceiling, the document size limit plus room for the request.
    let max_body = max_body_bytes();
    let max_upload = state.settings.documents_max_bytes.saturating_add(64 * 1024).max(max_body);
    let limited = |routes: axum::Router<AppState>, max: usize| {
        routes.layer(axum::extract::DefaultBodyLimit::max(max)).layer(tower_http::limit::RequestBodyLimitLayer::new(max))
    };
    limited(guarded(open, open_cors).merge(guarded(axum::Router::new().nest("/api", api), cors.clone())), max_body)
        .merge(limited(guarded(axum::Router::new().nest("/api", routers::documents::router()), cors), max_upload))
        .with_state(state)
}

/// `MAX_REQUEST_BODY_BYTES`, default 1 MiB.
pub fn max_body_bytes() -> usize {
    std::env::var("MAX_REQUEST_BODY_BYTES").ok().and_then(|v| v.trim().parse().ok()).filter(|n| *n > 0).unwrap_or(1 << 20)
}

async fn healthz() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "status": "ok" }))
}

/// Readiness: the control database answers and its migrations are current. `/healthz` stays a plain
/// liveness check (the process is up); this is the one a load balancer should gate traffic on.
#[utoipa::path(
    operation_id = "getReadyz",
    get,
    path = "/readyz",
    tag = "meta",
    summary = "Readiness: the control database answers and its schema is current",
    responses((status = 200, body = openapi::OkBody), (status = 503, description = "Not ready", body = openapi::Problem, content_type = "application/problem+json")),
    security(()),
)]
pub async fn readyz(axum::extract::State(state): axum::extract::State<state::AppState>) -> axum::response::Response {
    use axum::response::IntoResponse;
    let cached = state.ready_cache.lock().ok().and_then(|c| c.as_ref().filter(|(at, _)| at.elapsed() < std::time::Duration::from_secs(2)).map(|(_, p)| p.clone()));
    let problem = match cached {
        Some(p) => p,
        None => {
            let p = ready_problem(&state).await;
            if let Ok(mut c) = state.ready_cache.lock() {
                *c = Some((std::time::Instant::now(), p.clone()));
            }
            p
        }
    };
    match problem {
        None => {
            let warning = connectors::crypto::validate_key(&state.settings.encryption_key).err().map(|why| format!("{why} See docs/deployment.md."));
            axum::Json(openapi::OkBody { ok: true, warning }).into_response()
        }
        Some(detail) => {
            tracing::warn!(%detail, "not ready");
            // built by hand, not through AppError: an `internal` error hides its detail, and a probe polling
            // a down database should not fill the failure-capsule table
            let body = serde_json::json!({
                "type": "about:blank", "title": "Service Unavailable", "status": 503, "detail": detail,
                "code": error::ErrorCode::Internal.as_str(), "trace_id": telemetry::current_trace_id(),
            });
            (axum::http::StatusCode::SERVICE_UNAVAILABLE, [(axum::http::header::CONTENT_TYPE, "application/problem+json")], body.to_string()).into_response()
        }
    }
}

/// The two database checks behind `/readyz`; `None` means ready.
async fn ready_problem(state: &state::AppState) -> Option<String> {
    use surrealdb::types::SurrealValue;
    #[derive(serde::Deserialize, SurrealValue)]
    struct Row {
        version: i64,
    }
    let newest = migrate::CONTROL_MIGRATIONS.iter().map(|m| i64::from(m.0)).max().unwrap_or(0);
    match store::control::READY_PING.on(&state.control).await {
        Err(e) => {
            tracing::warn!(error = %e, "readyz: control ping failed");
            Some("The control database did not answer.".to_string())
        }
        Ok(_) => match store::control::READY_VERSION.on(&state.control).await.and_then(|mut r| r.take::<Vec<Row>>(0)) {
            Err(e) => {
                tracing::warn!(error = %e, "readyz: reading the migration ledger failed");
                Some("Could not read the control migration ledger.".to_string())
            }
            Ok(rows) => {
                let applied = rows.first().map_or(0, |r| r.version);
                (applied != newest).then(|| format!("The control database is at migration {applied}, this build expects {newest}."))
            }
        },
    }
}
