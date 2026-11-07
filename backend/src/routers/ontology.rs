//! A vault's project ontology over REST: read it, have the server's model suggest, and decide. The
//! decide routes are the only way to approve or reject, and they take a browser session only.

use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use surrealdb::types::RecordId;

use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::User;
use crate::ontology::{self, property::PropertyDecision, suggest::SuggestOut, KindDecision, LinkDecision, OntologyOut};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/vaults/{vault_id}/ontology", get(get_ontology))
        .route("/vaults/{vault_id}/ontology/suggest", post(suggest_ontology))
        .route("/vaults/{vault_id}/ontology/kinds/decide", post(decide_kind))
        .route("/vaults/{vault_id}/ontology/links/decide", post(decide_link))
        .route("/vaults/{vault_id}/ontology/properties/decide", post(decide_property))
}

fn parse_vault_id(vault_id: &str) -> AppResult<RecordId> {
    crate::rid::parse(vault_id).map_err(|_| AppError::coded(ErrorCode::VaultNotFound, "Vault not found."))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct KindDecideBody {
    kind: String,
    decision: KindDecision,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct LinkDecideBody {
    from_kind: String,
    label: String,
    to_kind: String,
    decision: LinkDecision,
}

#[derive(Deserialize, utoipa::ToSchema)]
struct PropertyDecideBody {
    kind: String,
    name: String,
    decision: PropertyDecision,
}

#[utoipa::path(
    operation_id = "getVaultOntology",
    get,
    path = "/api/vaults/{vault_id}/ontology",
    tag = "vaults",
    summary = "The vault's entity kinds and link types, observed and reviewed",
    params(("vault_id" = String, Path)),
    responses((status = 200, body = OntologyOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_ontology(State(state): State<AppState>, user: User, Path(vault_id): Path<String>) -> AppResult<Json<OntologyOut>> {
    let state = state.org(&user.org).await?;
    let vault = parse_vault_id(&vault_id)?;
    Ok(Json(ontology::get(&state.db, &user.id, Some(&vault)).await?))
}

#[utoipa::path(
    operation_id = "suggestVaultOntology",
    post,
    path = "/api/vaults/{vault_id}/ontology/suggest",
    tag = "vaults",
    summary = "Have the server's model propose kinds, merges and link types (the profile alone without a model)",
    params(("vault_id" = String, Path)),
    responses((status = 200, body = SuggestOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn suggest_ontology(State(state): State<AppState>, user: User, Path(vault_id): Path<String>) -> AppResult<Json<SuggestOut>> {
    let state = state.org(&user.org).await?;
    let vault = parse_vault_id(&vault_id)?;
    Ok(Json(ontology::suggest::suggest(&state.db, &state.settings, &user.id, Some(&vault)).await?))
}

#[utoipa::path(
    operation_id = "decideVaultOntologyKind",
    post,
    path = "/api/vaults/{vault_id}/ontology/kinds/decide",
    tag = "vaults",
    summary = "Approve, reject (optionally merging into another kind) or describe a kind (owner, browser session only)",
    params(("vault_id" = String, Path)),
    request_body = KindDecideBody,
    responses((status = 200, body = OntologyOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn decide_kind(State(state): State<AppState>, user: User, Path(vault_id): Path<String>, Json(body): Json<KindDecideBody>) -> AppResult<Json<OntologyOut>> {
    let state = state.org(&user.org).await?;
    let vault = parse_vault_id(&vault_id)?;
    Ok(Json(ontology::decide_kind(&state.db, &user.id, &vault, &body.kind, body.decision).await?))
}

#[utoipa::path(
    operation_id = "decideVaultOntologyLink",
    post,
    path = "/api/vaults/{vault_id}/ontology/links/decide",
    tag = "vaults",
    summary = "Approve, reject or describe a link type (owner, browser session only)",
    params(("vault_id" = String, Path)),
    request_body = LinkDecideBody,
    responses((status = 200, body = OntologyOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn decide_link(State(state): State<AppState>, user: User, Path(vault_id): Path<String>, Json(body): Json<LinkDecideBody>) -> AppResult<Json<OntologyOut>> {
    let state = state.org(&user.org).await?;
    let vault = parse_vault_id(&vault_id)?;
    Ok(Json(ontology::decide_link(&state.db, &user.id, &vault, &body.from_kind, &body.label, &body.to_kind, body.decision).await?))
}

#[utoipa::path(
    operation_id = "decideVaultOntologyProperty",
    post,
    path = "/api/vaults/{vault_id}/ontology/properties/decide",
    tag = "vaults",
    summary = "Approve, reject (optionally merging into another property of the kind) or describe a property (owner, browser session only)",
    params(("vault_id" = String, Path)),
    request_body = PropertyDecideBody,
    responses((status = 200, body = OntologyOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn decide_property(State(state): State<AppState>, user: User, Path(vault_id): Path<String>, Json(body): Json<PropertyDecideBody>) -> AppResult<Json<OntologyOut>> {
    let state = state.org(&user.org).await?;
    let vault = parse_vault_id(&vault_id)?;
    Ok(Json(ontology::property::decide_property(&state.db, &user.id, &vault, &body.kind, &body.name, body.decision).await?))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(get_ontology, suggest_ontology, decide_kind, decide_link, decide_property))]
pub struct Doc;
