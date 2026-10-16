//! Entity-memory REST surface: list/get/graph plus create/edit/delete over
//! the person/organisation/location + memory + relates_to graph,
//! vault-scoped like `routers/vaults.rs`. Thin wrappers over
//! `entities::service` -- the same functions `entities::tools`'s functions
//! call -- so the manual add/edit UI and a future agent's tool calls stay in
//! lockstep.
//!
//! `GET /entities/graph`'s `kinds` filter takes the repeated-query-param
//! form (`?kinds=a&kinds=b`) via a manual `RawQuery` parse (see `parse_kinds`),
//! since `axum::extract::Query`'s `serde_urlencoded` backing doesn't collect
//! repeated keys into a `Vec` without `axum-extra`. Comma-separated
//! (`?kinds=a,b`) is also accepted. Every other behavior (status codes,
//! response shapes, field names, vault membership checks) matches exactly.

use axum::{
    extract::{Path, Query, RawQuery, State},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use surrealdb::types::RecordId;

use crate::entities::service;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::User;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/entities", get(list_entities).post(create_entity))
        .route("/entities/graph", get(entity_graph))
        .route("/entities/cloud", get(vector_cloud))
        .route("/entities/memory/{memory_id}", axum::routing::patch(update_memory).delete(delete_memory))
        .route("/entities/{entity_id}", get(get_entity).patch(update_entity).delete(delete_entity))
        .route("/entities/{entity_id}/memory", post(add_memory))
        .route("/entities/{entity_id}/relations", post(add_relation))
        .route("/entities/{entity_id}/merge", post(merge_entities))
}

fn default_memory_type() -> String {
    "world".to_string()
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct EntityCreate {
    kind: String,
    name: String,
    #[serde(default)]
    aliases: Option<Vec<String>>,
    #[serde(default)]
    vault_id: Option<String>,
}

#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
struct EntityUpdate {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    aliases: Option<Vec<String>>,
    #[serde(default)]
    summary: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct MemoryCreate {
    text: String,
    #[serde(rename = "type", default = "default_memory_type")]
    mem_type: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct RelationCreate {
    to_id: String,
    label: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct MergeRequest {
    loser_id: String,
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    kind: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    offset: usize,
    vault_id: Option<String>,
}

fn default_limit() -> usize {
    50
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct GraphQuery {
    vault_id: Option<String>,
}

/// `kinds` can arrive either as a repeated query param (`?kinds=a&kinds=b`,
/// the browser/fetch convention this frontend uses) or comma-separated
/// (`?kinds=a,b`) -- accept both rather than pick one.
fn parse_kinds(raw_query: Option<&str>) -> Vec<String> {
    let Some(raw) = raw_query else { return Vec::new() };
    let pairs: Vec<(String, String)> = serde_urlencoded::from_str(raw).unwrap_or_default();
    pairs
        .into_iter()
        .filter(|(k, _)| k == "kinds")
        .flat_map(|(_, v)| v.split(',').map(str::to_string).collect::<Vec<_>>())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Path params are plain strings; parse here, same convention as `routers/vaults.rs`'s
/// `parse_vault_id`.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct CloudQuery {
    /// comma-separated vault ids; default: your personal vault
    #[serde(default)]
    vault_ids: Option<String>,
}

/// 3D PCA projection of the vectors in one or more vaults -- see `cache::cloud`.
#[utoipa::path(
    operation_id = "getVectorCloud",
    get,
    path = "/api/entities/cloud",
    tag = "entities",
    summary = "3D projection of vault vectors",
    params(CloudQuery),
    responses((status = 200, body = crate::cache::cloud::Cloud), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn vector_cloud(
    State(state): State<AppState>,
    user: User,
    Query(q): Query<CloudQuery>,
) -> AppResult<Json<crate::cache::cloud::Cloud>> {
    let state = state.org(&user.org).await?;
    let ids = q
        .vault_ids
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(parse_record_id)
        .collect::<AppResult<Vec<_>>>()?;
    Ok(Json(crate::cache::cloud::cloud(&state.db, &state.settings, &user.id, &ids).await?))
}

fn parse_record_id(id: &str) -> AppResult<RecordId> {
    crate::rid::parse(id).map_err(|_| AppError::not_found("not found"))
}

/// A built-in kind or a valid custom kind name (see `service::kind_table`).
fn known_kind_or_400(kind: &str) -> AppResult<()> {
    service::kind_table(kind).map(|_| ())
}

#[utoipa::path(
    operation_id = "listEntities",
    get,
    path = "/api/entities",
    tag = "entities",
    summary = "List entities",
    params(ListQuery),
    responses((status = 200, body = service::ListEntitiesOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list_entities(
    State(state): State<AppState>,
    user: User,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<service::ListEntitiesOut>> {
    let state = state.org(&user.org).await?;
    if let Some(k) = &q.kind {
        known_kind_or_400(k)?;
    }
    let vault_rid = q.vault_id.as_deref().map(parse_record_id).transpose()?;
    Ok(Json(
        service::list_entities(&state.db, &user.id, q.kind.as_deref(), vault_rid.as_ref(), Some(q.limit), q.offset)
            .await?,
    ))
}

#[utoipa::path(
    operation_id = "getEntityGraph",
    get,
    path = "/api/entities/graph",
    tag = "entities",
    summary = "Entity graph",
    params(GraphQuery, ("kinds" = Option<Vec<String>>, Query, description = "Repeated or comma separated entity kinds")),
    responses((status = 200, body = service::GraphOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn entity_graph(
    State(state): State<AppState>,
    user: User,
    Query(q): Query<GraphQuery>,
    RawQuery(raw): RawQuery,
) -> AppResult<Json<service::GraphOut>> {
    let state = state.org(&user.org).await?;
    let kinds_vec = parse_kinds(raw.as_deref());
    let kinds: Option<Vec<String>> = if kinds_vec.is_empty() { None } else { Some(kinds_vec) };
    if let Some(ks) = &kinds {
        for k in ks {
            known_kind_or_400(k)?;
        }
    }
    let vault_rid = q.vault_id.as_deref().map(parse_record_id).transpose()?;
    Ok(Json(service::graph(&state.db, &state.control, &user.id, kinds.as_deref(), vault_rid.as_ref()).await?))
}

#[utoipa::path(
    operation_id = "createEntity",
    post,
    path = "/api/entities",
    tag = "entities",
    summary = "Create or upsert an entity",
    request_body = EntityCreate,
    responses((status = 200, body = service::EntityOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn create_entity(
    State(state): State<AppState>,
    user: User,
    Json(body): Json<EntityCreate>,
) -> AppResult<Json<service::EntityOut>> {
    let state = state.org(&user.org).await?;
    known_kind_or_400(&body.kind)?;
    let vault_rid = body.vault_id.as_deref().map(parse_record_id).transpose()?;
    Ok(Json(
        service::upsert_entity(&state.db, &user.id, &body.kind, &body.name, body.aliases, vault_rid.as_ref()).await?,
    ))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
struct MemoryUpdate {
    #[serde(default)]
    text: Option<String>,
    #[serde(rename = "type", default)]
    mem_type: Option<String>,
}

#[utoipa::path(
    operation_id = "updateMemory",
    patch,
    path = "/api/entities/memory/{memory_id}",
    tag = "entities",
    summary = "Edit a memory",
    params(("memory_id" = String, Path)),
    request_body = MemoryUpdate,
    responses((status = 200, body = service::MemoryOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn update_memory(
    State(state): State<AppState>,
    user: User,
    Path(memory_id): Path<String>,
    Json(body): Json<MemoryUpdate>,
) -> AppResult<Json<service::MemoryOut>> {
    let state = state.org(&user.org).await?;
    let rid = parse_record_id(&memory_id)?;
    let memory = service::update_memory(&state.db, &user.id, &rid, body.text.as_deref(), body.mem_type.as_deref()).await?;
    memory.map(Json).ok_or_else(|| AppError::coded(ErrorCode::MemoryNotFound, "not found"))
}

#[utoipa::path(
    operation_id = "deleteMemory",
    delete,
    path = "/api/entities/memory/{memory_id}",
    tag = "entities",
    summary = "Delete a memory",
    params(("memory_id" = String, Path)),
    responses((status = 200, body = crate::openapi::DeletedBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn delete_memory(
    State(state): State<AppState>,
    user: User,
    Path(memory_id): Path<String>,
) -> AppResult<Json<Value>> {
    let state = state.org(&user.org).await?;
    let rid = parse_record_id(&memory_id)?;
    let deleted = service::delete_memory(&state.db, &user.id, &rid).await?;
    if !deleted {
        return Err(AppError::coded(ErrorCode::MemoryNotFound, "not found"));
    }
    Ok(Json(json!({ "deleted": true })))
}

#[utoipa::path(
    operation_id = "getEntity",
    get,
    path = "/api/entities/{entity_id}",
    tag = "entities",
    summary = "One entity with its memories and relations",
    params(("entity_id" = String, Path)),
    responses((status = 200, body = service::EntityDetail), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_entity(
    State(state): State<AppState>,
    user: User,
    Path(entity_id): Path<String>,
) -> AppResult<Json<service::EntityDetail>> {
    let state = state.org(&user.org).await?;
    let rid = parse_record_id(&entity_id)?;
    let entity = service::get_entity(&state.db, &state.control, &user.id, &rid).await?;
    entity.map(Json).ok_or_else(|| AppError::coded(ErrorCode::EntityNotFound, "not found"))
}

#[utoipa::path(
    operation_id = "updateEntity",
    patch,
    path = "/api/entities/{entity_id}",
    tag = "entities",
    summary = "Edit an entity",
    params(("entity_id" = String, Path)),
    request_body = EntityUpdate,
    responses((status = 200, body = service::EntityOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn update_entity(
    State(state): State<AppState>,
    user: User,
    Path(entity_id): Path<String>,
    Json(body): Json<EntityUpdate>,
) -> AppResult<Json<service::EntityOut>> {
    let state = state.org(&user.org).await?;
    let rid = parse_record_id(&entity_id)?;
    let entity =
        service::update_entity(&state.db, &user.id, &rid, body.name.as_deref(), body.aliases, body.summary.as_deref())
            .await?;
    entity.map(Json).ok_or_else(|| AppError::coded(ErrorCode::EntityNotFound, "not found"))
}

#[utoipa::path(
    operation_id = "deleteEntity",
    delete,
    path = "/api/entities/{entity_id}",
    tag = "entities",
    summary = "Delete an entity",
    params(("entity_id" = String, Path)),
    responses((status = 200, body = crate::openapi::DeletedBody), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn delete_entity(
    State(state): State<AppState>,
    user: User,
    Path(entity_id): Path<String>,
) -> AppResult<Json<Value>> {
    let state = state.org(&user.org).await?;
    let rid = parse_record_id(&entity_id)?;
    let deleted = service::delete_entity(&state.db, &user.id, &rid).await?;
    if !deleted {
        return Err(AppError::coded(ErrorCode::EntityNotFound, "not found"));
    }
    Ok(Json(json!({ "deleted": true })))
}

#[utoipa::path(
    operation_id = "addMemory",
    post,
    path = "/api/entities/{entity_id}/memory",
    tag = "entities",
    summary = "Add a memory to an entity",
    params(("entity_id" = String, Path)),
    request_body = MemoryCreate,
    responses((status = 200, body = service::MemoryOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn add_memory(
    State(state): State<AppState>,
    user: User,
    Path(entity_id): Path<String>,
    Json(body): Json<MemoryCreate>,
) -> AppResult<Json<service::MemoryOut>> {
    let state = state.org(&user.org).await?;
    let rid = parse_record_id(&entity_id)?;
    if service::get_entity(&state.db, &state.control, &user.id, &rid).await?.is_none() {
        return Err(AppError::coded(ErrorCode::EntityNotFound, "not found"));
    }
    Ok(Json(service::add_memory(&state.db, &user.id, &rid, &body.text, None, &body.mem_type).await?))
}

#[utoipa::path(
    operation_id = "addRelation",
    post,
    path = "/api/entities/{entity_id}/relations",
    tag = "entities",
    summary = "Relate two entities",
    params(("entity_id" = String, Path)),
    request_body = RelationCreate,
    responses((status = 200, body = service::RelationOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn add_relation(
    State(state): State<AppState>,
    user: User,
    Path(entity_id): Path<String>,
    Json(body): Json<RelationCreate>,
) -> AppResult<Json<service::RelationOut>> {
    let state = state.org(&user.org).await?;
    let rid = parse_record_id(&entity_id)?;
    if service::get_entity(&state.db, &state.control, &user.id, &rid).await?.is_none() {
        return Err(AppError::coded(ErrorCode::EntityNotFound, "not found"));
    }
    let to_rid = parse_record_id(&body.to_id)?;
    if service::get_entity(&state.db, &state.control, &user.id, &to_rid).await?.is_none() {
        return Err(AppError::coded(ErrorCode::EntityNotFound, "target entity not found"));
    }
    Ok(Json(service::add_relation(&state.db, &user.id, &rid, &to_rid, &body.label, None).await?))
}

#[utoipa::path(
    operation_id = "mergeEntities",
    post,
    path = "/api/entities/{entity_id}/merge",
    tag = "entities",
    summary = "Merge a duplicate entity into this one",
    params(("entity_id" = String, Path)),
    request_body = MergeRequest,
    responses((status = 200, body = service::EntityOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn merge_entities(
    State(state): State<AppState>,
    user: User,
    Path(entity_id): Path<String>,
    Json(body): Json<MergeRequest>,
) -> AppResult<Json<service::EntityOut>> {
    let state = state.org(&user.org).await?;
    let rid = parse_record_id(&entity_id)?;
    let loser_rid = parse_record_id(&body.loser_id)?;
    Ok(Json(service::merge_entities(&state.db, &user.id, &rid, &loser_rid).await?))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    list_entities,
    create_entity,
    entity_graph,
    vector_cloud,
    update_memory,
    delete_memory,
    get_entity,
    update_entity,
    delete_entity,
    add_memory,
    add_relation,
    merge_entities,
))]
pub struct Doc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limit_is_50() {
        assert_eq!(default_limit(), 50);
    }

    #[test]
    fn default_memory_type_is_world() {
        assert_eq!(default_memory_type(), "world");
    }

    #[test]
    fn known_kind_or_400_accepts_valid_kinds() {
        assert!(known_kind_or_400("person").is_ok());
        assert!(known_kind_or_400("symbol").is_ok());
    }

    #[test]
    fn known_kind_or_400_rejects_malformed_kind() {
        assert!(known_kind_or_400("alien").is_ok(), "a custom kind");
        for bad in ["", "Alien", "1db", "db-table", "x = 1 OR true; DELETE user; --", &"a".repeat(33)] {
            assert!(known_kind_or_400(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn parse_record_id_rejects_garbage() {
        assert!(parse_record_id("not-a-record-id!!").is_err());
    }

    #[test]
    fn parse_record_id_accepts_well_formed_id() {
        assert!(parse_record_id("person:abc123").is_ok());
    }
}
