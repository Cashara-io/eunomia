//! Browser-facing OAuth routes under `/api`: the consent page's data and
//! decision, and the "Connected apps" list in Settings. The protocol
//! endpoints themselves (`/oauth/*`, `/.well-known/*`) live in `src/oauth/`
//! and, being defined by RFCs, are not part of this OpenAPI document.

use surrealdb::types::SurrealValue;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get},
};
use serde::{Deserialize, Serialize};
use surrealdb::types::RecordId;
use crate::rid::RecordIdExt;

use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::User;
use crate::oauth::server::{self, AuthzFail, AuthzParams};
use crate::scopes;
use crate::state::AppState;
use utoipa::OpenApi;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/oauth/consent", get(consent_info).post(consent_decide))
        .route("/oauth/grants", get(list_grants))
        .route("/oauth/grants/{grant_id}", delete(revoke_grant))
}

#[derive(OpenApi)]
#[openapi(
    paths(consent_info, consent_decide, list_grants, revoke_grant),
    components(schemas(AuthzParams, ConsentInfo, ConsentClient, ConsentScope, ConsentDecision, ConsentResult, Grant))
)]
pub struct Doc;

#[derive(Serialize, utoipa::ToSchema)]
struct ConsentClient {
    name: String,
    logo_uri: Option<String>,
    client_uri: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct ConsentScope {
    scope: String,
    description: String,
}

#[derive(Serialize, utoipa::ToSchema)]
struct ConsentInfo {
    client: ConsentClient,
    /// Host the user is sent back to after deciding.
    redirect_host: String,
    /// True when the redirect goes to this computer only (localhost); the page warns about it.
    loopback: bool,
    scopes: Vec<ConsentScope>,
    user_email: String,
}

fn invalid(f: AuthzFail) -> AppError {
    AppError::coded(ErrorCode::ValidationInvalid, f.message().to_string())
}

#[utoipa::path(
    operation_id = "getOauthConsent",
    get,
    path = "/api/oauth/consent",
    tag = "oauth",
    summary = "Validate an authorization request and describe it for the consent page",
    params(("response_type" = Option<String>, Query), ("client_id" = Option<String>, Query), ("redirect_uri" = Option<String>, Query),
           ("code_challenge" = Option<String>, Query), ("code_challenge_method" = Option<String>, Query),
           ("state" = Option<String>, Query), ("scope" = Option<String>, Query), ("resource" = Option<String>, Query)),
    responses((status = 200, body = ConsentInfo), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn consent_info(State(state): State<AppState>, user: User, Query(p): Query<AuthzParams>) -> AppResult<Json<ConsentInfo>> {
    let v = server::validate_authz(&state, &p).await.map_err(invalid)?;
    let url = reqwest::Url::parse(&v.redirect_uri).map_err(|_| AppError::bad_request("invalid redirect_uri"))?;
    let host = match (url.host_str(), url.port()) {
        (Some(h), Some(port)) => format!("{h}:{port}"),
        (Some(h), None) => h.to_string(),
        (None, _) => format!("{}://", url.scheme()), // private-use scheme app
    };
    Ok(Json(ConsentInfo {
        client: ConsentClient { name: v.client.name, logo_uri: v.client.logo_uri, client_uri: v.client.client_uri },
        redirect_host: host,
        loopback: crate::oauth::cimd::is_loopback_host(&url),
        scopes: v.scopes.iter().map(|s| ConsentScope { scope: s.clone(), description: scopes::describe(s).into() }).collect(),
        user_email: user.email,
    }))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct ConsentDecision {
    #[serde(flatten)]
    request: AuthzParams,
    approve: bool,
}

#[derive(Serialize, utoipa::ToSchema)]
struct ConsentResult {
    /// Send the browser here: the client's redirect URI with the code (or the denial).
    redirect_to: String,
}

#[utoipa::path(
    operation_id = "decideOauthConsent",
    post,
    path = "/api/oauth/consent",
    tag = "oauth",
    summary = "Approve or deny an authorization request",
    request_body = ConsentDecision,
    responses((status = 200, body = ConsentResult), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn consent_decide(State(state): State<AppState>, user: User, Json(d): Json<ConsentDecision>) -> AppResult<Json<ConsentResult>> {
    // a token must not mint a longer-lived, differently scoped credential for an OAuth client
    crate::authz::require_session()?;
    let v = server::validate_authz(&state, &d.request).await.map_err(invalid)?;
    let redirect_to = if d.approve {
        let code = server::create_code(&state, &v, &user.id).await?;
        server::redirect_url(&state, &v.redirect_uri, v.state.as_deref(), &[("code", &code)])
    } else {
        server::fail_redirect(&state, &d.request, "access_denied", "The user denied the request")
    };
    let client: String = v.client.client_id.chars().take(256).collect();
    crate::audit::record_as_caller(&state.control, &user.id, "oauth.consent", &client, if d.approve { "ok" } else { "access_denied" }).await;
    Ok(Json(ConsentResult { redirect_to }))
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
struct Grant {
    id: String,
    client_id: String,
    client_name: String,
    client_logo: Option<String>,
    scope: Vec<String>,
    created_at: String,
    last_used_at: Option<String>,
}

#[utoipa::path(
    operation_id = "listOauthGrants",
    get,
    path = "/api/oauth/grants",
    tag = "oauth",
    summary = "List the apps connected through OAuth",
    responses((status = 200, body = Vec<Grant>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn list_grants(State(state): State<AppState>, user: User) -> AppResult<Json<Vec<Grant>>> {
    crate::authz::require_session()?;
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        id: RecordId,
        client_id: String,
        client_name: String,
        client_logo: Option<String>,
        scope: Vec<String>,
        created_at: surrealdb::types::Datetime,
        last_used_at: Option<surrealdb::types::Datetime>,
    }
    let mut res = crate::store::control::OAUTH_GRANT_LIST.on(&state.control).bind(("owner", user.id)).await?;
    let rows: Vec<Row> = res.take(0)?;
    Ok(Json(
        rows.into_iter()
            .map(|r| Grant {
                id: crate::rid::key_string(r.id.key()).unwrap_or_default(),
                client_id: r.client_id,
                client_name: r.client_name,
                client_logo: r.client_logo,
                scope: r.scope,
                created_at: r.created_at.to_string(),
                last_used_at: r.last_used_at.map(|d| d.to_string()),
            })
            .collect(),
    ))
}

#[utoipa::path(
    operation_id = "revokeOauthGrant",
    delete,
    path = "/api/oauth/grants/{grant_id}",
    tag = "oauth",
    summary = "Disconnect an app: revoke all its tokens",
    params(("grant_id" = String, Path)),
    responses((status = 200, body = crate::openapi::DeletedBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn revoke_grant(State(state): State<AppState>, user: User, Path(grant_id): Path<String>) -> AppResult<Json<crate::openapi::DeletedBody>> {
    crate::authz::require_session()?;
    let user_id = user.id.clone();
    let id = RecordId::from_table_key("oauth_grant", grant_id);
    let mut res = crate::store::control::OAUTH_GRANT_DELETE.on(&state.control).bind(("id", id.clone())).bind(("owner", user.id.clone())).await?;
    let removed: Vec<server::Gone> = res.take(0)?;
    if removed.is_empty() {
        return Err(AppError::coded(ErrorCode::ResourceNotFound, "No such connected app."));
    }
    crate::store::control::OAUTH_TOKENS_DELETE_FAMILY.on(&state.control).bind(("family", id.clone())).await?.check()?;
    state.credentials.forget(&id.to_string());
    crate::audit::record_as_caller(&state.control, &user_id, "oauth.grant_revoke", &id.to_string(), "ok").await;
    Ok(Json(crate::openapi::DeletedBody { deleted: true }))
}
