//! The operator API (`operator.rs`): `/api/operator/v1/...`, for the billing service of a hosted Cupel.
//!
//! Mounted only while `EUNOMIA_OPERATOR_TOKEN` is set (`lib.rs`); with it unset there is no such route and
//! the answer is 404. The gate (`gate::operator_gate`) lets through only a request that carries that token,
//! and every handler also asks for [`OperatorAuth`], so a route added here cannot be reached any other way.

use axum::{
    extract::{FromRequestParts, Path, Query, State},
    http::request::Parts,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::error::{AppError, AppResult};
use crate::operator::{self, Capacity, ClaimResent, CreateOrg, CreateOrgResult, OrgList, OrgSummary, PriceTableOut, PricesUpdate, SuspendOrg, UpdateOrg};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/operator/v1/orgs", get(list_orgs).post(create_org))
        .route("/operator/v1/orgs/{org_id}", get(get_org).patch(update_org))
        .route("/operator/v1/orgs/{org_id}/suspend", post(suspend_org))
        .route("/operator/v1/orgs/{org_id}/resume", post(resume_org))
        .route("/operator/v1/orgs/{org_id}/owner-claim/resend", post(resend_claim))
        .route("/operator/v1/capacity", get(get_capacity))
        .route("/operator/v1/ai/prices", get(get_prices).put(put_prices))
}

/// Proof that the gate saw the operator token on this request.
pub struct OperatorAuth;

impl<S: Send + Sync> FromRequestParts<S> for OperatorAuth {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<crate::gate::OperatorCall>().map(|_| OperatorAuth).ok_or_else(|| AppError::unauthorized("Not authenticated."))
    }
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct ListQuery {
    /// Page size, 1 to 500 (default 100).
    limit: Option<i64>,
    /// Organisations to skip, oldest first.
    offset: Option<i64>,
}

#[utoipa::path(
    operation_id = "operatorCreateOrg",
    post,
    path = "/api/operator/v1/orgs",
    tag = "operator",
    summary = "Create an organisation for a buyer, with its owner claim and optional AI allowance (idempotent on external_ref)",
    description = "Provisions the organisation's database and makes a single-use owner claim for `owner_email`: whoever opens `claim_link` creates their account (password or single sign-on) and becomes the organisation's owner. The same `external_ref` is the same organisation, so a retried call returns it and never makes a second one (`created: false`, and no link: resend the claim). When this server sends email the link is also emailed to the owner; `emailed` says whether the mail server accepted it. \
                   Refused: an owner email that already has an account (`operator.owner_in_other_org`), an address in `EUNOMIA_ADMIN_EMAILS` (`operator.owner_email_reserved`), the same reference with a different owner (`operator.reference_reused`), and a full server (`operator.at_capacity`).",
    request_body = CreateOrg,
    responses((status = 200, body = CreateOrgResult), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("operator" = [])),
)]
async fn create_org(_: OperatorAuth, State(state): State<AppState>, Json(body): Json<CreateOrg>) -> AppResult<Json<CreateOrgResult>> {
    Ok(Json(operator::create(&state, body).await?))
}

#[utoipa::path(
    operation_id = "operatorListOrgs",
    get,
    path = "/api/operator/v1/orgs",
    tag = "operator",
    summary = "List organisations with plan state, seat usage, owner and this month's model spend",
    description = "Control-database metadata only: this API never reads anything from an organisation's own database.",
    params(ListQuery),
    responses((status = 200, body = OrgList), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("operator" = [])),
)]
async fn list_orgs(_: OperatorAuth, State(state): State<AppState>, Query(q): Query<ListQuery>) -> AppResult<Json<OrgList>> {
    Ok(Json(operator::list(&state, q.limit, q.offset).await?))
}

#[utoipa::path(
    operation_id = "operatorGetOrg",
    get,
    path = "/api/operator/v1/orgs/{org_id}",
    tag = "operator",
    summary = "One organisation: plan state, seat usage, owner, whether it was claimed, and this month's model usage by purpose and model",
    params(("org_id" = String, Path, description = "The organisation id")),
    responses((status = 200, body = OrgSummary), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("operator" = [])),
)]
async fn get_org(_: OperatorAuth, State(state): State<AppState>, Path(org_id): Path<String>) -> AppResult<Json<OrgSummary>> {
    Ok(Json(operator::get(&state, &org_id).await?))
}

#[utoipa::path(
    operation_id = "operatorUpdateOrg",
    patch,
    path = "/api/operator/v1/orgs/{org_id}",
    tag = "operator",
    summary = "Change an organisation's plan, seat limit, trial end, name or monthly AI allowance",
    description = "Only the fields you send change. `seat_limit` and `trial_ends_at` take `null` to clear them. Lowering the seat limit below the seats in use (active members plus pending invitations) is refused with `operator.seats_in_use`, so the billing service can tell the customer to remove people first.",
    params(("org_id" = String, Path, description = "The organisation id")),
    request_body = UpdateOrg,
    responses((status = 200, body = OrgSummary), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("operator" = [])),
)]
async fn update_org(_: OperatorAuth, State(state): State<AppState>, Path(org_id): Path<String>, Json(body): Json<UpdateOrg>) -> AppResult<Json<OrgSummary>> {
    Ok(Json(operator::update(&state, &org_id, body).await?))
}

#[utoipa::path(
    operation_id = "operatorSuspendOrg",
    post,
    path = "/api/operator/v1/orgs/{org_id}/suspend",
    tag = "operator",
    summary = "Suspend an organisation (for example when payment fails)",
    description = "Within a few seconds every member's requests that read or write workspace content are refused (`org.suspended`) and its background work pauses. Members can still sign in and out, read the plan status, change their own password, export or erase their own data, and an owner or admin can read the People list. Suspending again keeps the first time. Nothing is deleted.",
    params(("org_id" = String, Path, description = "The organisation id")),
    request_body = SuspendOrg,
    responses((status = 200, body = OrgSummary), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("operator" = [])),
)]
async fn suspend_org(_: OperatorAuth, State(state): State<AppState>, Path(org_id): Path<String>, Json(body): Json<SuspendOrg>) -> AppResult<Json<OrgSummary>> {
    Ok(Json(operator::suspend(&state, &org_id, body).await?))
}

#[utoipa::path(
    operation_id = "operatorResumeOrg",
    post,
    path = "/api/operator/v1/orgs/{org_id}/resume",
    tag = "operator",
    summary = "Resume a suspended organisation",
    description = "Members' requests work again within a few seconds and paused background work picks up where it left off.",
    params(("org_id" = String, Path, description = "The organisation id")),
    responses((status = 200, body = OrgSummary), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("operator" = [])),
)]
async fn resume_org(_: OperatorAuth, State(state): State<AppState>, Path(org_id): Path<String>) -> AppResult<Json<OrgSummary>> {
    Ok(Json(operator::resume(&state, &org_id).await?))
}

#[utoipa::path(
    operation_id = "operatorResendOwnerClaim",
    post,
    path = "/api/operator/v1/orgs/{org_id}/owner-claim/resend",
    tag = "operator",
    summary = "Make a new owner claim link while the organisation is unclaimed",
    description = "The old link stops working at once. Refused once the owner has claimed the organisation (`operator.owner_claimed`) or when the owner's email has since got an account (`operator.owner_in_other_org`). Emailed too when this server sends email.",
    params(("org_id" = String, Path, description = "The organisation id")),
    responses((status = 200, body = ClaimResent), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("operator" = [])),
)]
async fn resend_claim(_: OperatorAuth, State(state): State<AppState>, Path(org_id): Path<String>) -> AppResult<Json<ClaimResent>> {
    Ok(Json(operator::resend_claim(&state, &org_id).await?))
}

#[utoipa::path(
    operation_id = "operatorGetCapacity",
    get,
    path = "/api/operator/v1/capacity",
    tag = "operator",
    summary = "How many organisations exist and the limit (`EUNOMIA_MAX_ORGS`), and this month's model spend across them",
    responses((status = 200, body = Capacity), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("operator" = [])),
)]
async fn get_capacity(_: OperatorAuth, State(state): State<AppState>) -> AppResult<Json<Capacity>> {
    Ok(Json(operator::capacity(&state).await?))
}

#[utoipa::path(
    operation_id = "operatorGetAiPrices",
    get,
    path = "/api/operator/v1/ai/prices",
    tag = "operator",
    summary = "The model price table in force: the defaults and the operator's rows",
    description = "Prices are US dollars per million input and output tokens. A model in neither the operator's rows nor the defaults is counted at `fallback`, which is deliberately high.",
    responses((status = 200, body = PriceTableOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("operator" = [])),
)]
async fn get_prices(_: OperatorAuth, State(state): State<AppState>) -> AppResult<Json<PriceTableOut>> {
    Ok(Json(operator::get_prices(&state).await?))
}

#[utoipa::path(
    operation_id = "operatorSetAiPrices",
    put,
    path = "/api/operator/v1/ai/prices",
    tag = "operator",
    summary = "Set or remove model prices, with no deploy",
    description = "Only the models you send change; a model with `null` goes back to its default. Applies to every replica within 30 seconds. Costs already metered keep the price they were counted at.",
    request_body = PricesUpdate,
    responses((status = 200, body = PriceTableOut), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("operator" = [])),
)]
async fn put_prices(_: OperatorAuth, State(state): State<AppState>, Json(body): Json<PricesUpdate>) -> AppResult<Json<PriceTableOut>> {
    Ok(Json(operator::set_prices(&state, body).await?))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(create_org, list_orgs, get_org, update_org, suspend_org, resume_org, resend_claim, get_capacity, get_prices, put_prices))]
pub struct Doc;
