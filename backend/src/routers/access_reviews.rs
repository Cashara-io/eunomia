//! Access reviews (admin only, Settings > Access reviews). An admin starts a review, which
//! snapshots who has access to what: every person in the workspace, their role, admin rights and
//! suspension, their space memberships, and their API tokens, connected apps and live sessions by
//! name and count (never a secret). Admins record a decision per person (keep, remove, suspend)
//! and complete the review. A completed review cannot change (the database refuses it, tenant
//! migration 0044), its start, decisions and completion are in the hash-chained security log, and
//! it can be downloaded with the hashes that tie it to that log. See docs/audit-chain.md.
//!
//! Decisions are records, not actions: suspending or removing someone is still done from People.
//! An admin cannot decide about their own access; when nobody else does, the completed review
//! lists it as not reviewed.

use std::collections::{BTreeMap, HashMap};

use axum::{
    extract::{Path, State},
    http::header,
    response::IntoResponse,
    routing::{get, post, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use crate::audit::chain::{self, sha256_hex, ts};
use crate::authz;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::User;
use crate::rid::RecordIdExt;
use crate::state::AppState;
use crate::store;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/access-reviews", get(list).post(start))
        .route("/admin/access-reviews/{review_id}", get(get_review))
        .route("/admin/access-reviews/{review_id}/decisions/{user_id}", put(decide))
        .route("/admin/access-reviews/{review_id}/complete", post(complete))
        .route("/admin/access-reviews/{review_id}/export", get(export))
}

/// JSON with object keys sorted at every level and no whitespace: the form the review hashes use.
pub fn canonical_json(v: &Value) -> String {
    fn sort(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let sorted: BTreeMap<&String, Value> = m.iter().map(|(k, v)| (k, sort(v))).collect();
                Value::Object(sorted.into_iter().map(|(k, v)| (k.clone(), v)).collect())
            }
            Value::Array(a) => Value::Array(a.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    sort(v).to_string()
}

fn time(d: &Option<Datetime>) -> Value {
    d.as_ref().and_then(crate::sources::base::datetime_to_chrono).map(|t| Value::String(ts(&t))).unwrap_or(Value::Null)
}

fn not_found() -> AppError {
    AppError::coded(ErrorCode::AccessReviewNotFound, "That access review doesn't exist.")
}

fn completed() -> AppError {
    AppError::coded(ErrorCode::AccessReviewCompleted, "This access review is completed and can't be changed.")
}

/// The database's own refusal of a change to a completed review (the schema's events).
fn frozen(e: surrealdb::Error) -> AppError {
    if e.to_string().contains("access_review.completed") { completed() } else { e.into() }
}

// ---------------------------------------------------------------- snapshot

#[derive(Debug, Deserialize, SurrealValue)]
struct TokenRow {
    id: RecordId,
    owner: RecordId,
    #[serde(default)]
    #[surreal(default)]
    name: String,
    #[serde(default)]
    #[surreal(default)]
    scopes: Vec<String>,
    #[serde(default)]
    #[surreal(default)]
    vault: Option<RecordId>,
    created_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    last_used_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    expires_at: Option<Datetime>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct AppRow {
    id: RecordId,
    owner: RecordId,
    #[serde(default)]
    #[surreal(default)]
    client_id: String,
    #[serde(default)]
    #[surreal(default)]
    client_name: String,
    #[serde(default)]
    #[surreal(default)]
    scope: Vec<String>,
    created_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    last_used_at: Option<Datetime>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct SessionCount {
    owner: RecordId,
    #[serde(default)]
    #[surreal(default)]
    n: i64,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct SpaceRow {
    user: RecordId,
    #[serde(default)]
    #[surreal(default)]
    role: String,
    #[serde(default)]
    #[surreal(default)]
    status: String,
    vault_id: RecordId,
    #[serde(default)]
    #[surreal(default)]
    vault_name: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    vault_kind: Option<String>,
}

/// One person's spaces, API tokens, apps and live session count.
type PersonAccess = (Vec<Value>, Vec<Value>, Vec<Value>, i64);

/// Who has access to what in the caller's workspace, right now. Names and dates only.
async fn snapshot(state: &AppState, user: &User) -> AppResult<Value> {
    let org = state.org(&user.org).await?;
    let people = super::admin::people_rows(state, user).await?;
    let creator = crate::capsules::first_user(&state.control).await?;
    let ids: Vec<RecordId> = people.iter().map(|p| p.id.clone()).collect();
    let tokens: Vec<TokenRow> = store::control::ACCESS_TOKENS.on(&state.control).bind(("ids", ids.clone())).await?.take(0)?;
    let apps: Vec<AppRow> = store::control::ACCESS_APPS.on(&state.control).bind(("ids", ids.clone())).await?.take(0)?;
    let sessions: Vec<SessionCount> = store::control::ACCESS_SESSIONS.on(&state.control).bind(("ids", ids.clone())).await?.take(0)?;
    let spaces: Vec<SpaceRow> = store::access_reviews::VAULT_ACCESS.on(&org.db).bind(("ids", ids)).await?.take(0)?;

    let mut by_user: HashMap<String, PersonAccess> = HashMap::new();
    for t in tokens {
        by_user.entry(chain::rid(&t.owner)).or_default().1.push(json!({
            "id": chain::rid(&t.id),
            "name": t.name,
            "scopes": t.scopes,
            "space": t.vault.as_ref().map(chain::rid),
            "created_at": time(&t.created_at),
            "last_used_at": time(&t.last_used_at),
            "expires_at": time(&t.expires_at),
        }));
    }
    for a in apps {
        by_user.entry(chain::rid(&a.owner)).or_default().2.push(json!({
            "id": chain::rid(&a.id),
            "name": a.client_name,
            "client_id": a.client_id,
            "scopes": a.scope,
            "created_at": time(&a.created_at),
            "last_used_at": time(&a.last_used_at),
        }));
    }
    for s in sessions {
        by_user.entry(chain::rid(&s.owner)).or_default().3 = s.n;
    }
    for s in spaces {
        by_user.entry(chain::rid(&s.user)).or_default().0.push(json!({
            "id": chain::rid(&s.vault_id),
            "name": s.vault_name.unwrap_or_default(),
            "kind": s.vault_kind.unwrap_or_default(),
            "role": s.role,
            "status": s.status,
        }));
    }
    let people: Vec<Value> = people
        .iter()
        .map(|p| {
            let id = chain::rid(&p.id);
            let (spaces, tokens, apps, sessions) = by_user.remove(&id).unwrap_or_default();
            let source = super::admin::admin_source(p, creator.as_ref());
            json!({
                "user_id": id,
                "email": p.email,
                "role": if source.is_empty() { "member" } else { "admin" },
                "admin_source": source,
                "suspended_at": time(&p.suspended_at),
                "created_at": time(&p.created_at),
                "spaces": spaces,
                "tokens": tokens,
                "apps": apps,
                "sessions": sessions,
            })
        })
        .collect();
    Ok(json!({
        "workspace": chain::org_ledger(&user.org),
        "taken_at": ts(&chrono::Utc::now()),
        "people": people,
    }))
}

// ---------------------------------------------------------------- reading

#[derive(Debug, Deserialize, SurrealValue)]
struct ReviewRow {
    id: RecordId,
    status: String,
    started_by: RecordId,
    #[serde(default)]
    #[surreal(default)]
    started_by_email: String,
    started_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    completed_by: Option<RecordId>,
    #[serde(default)]
    #[surreal(default)]
    completed_by_email: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    completed_at: Option<Datetime>,
    snapshot: Value,
    #[serde(default)]
    #[surreal(default)]
    snapshot_hash: String,
    #[serde(default)]
    #[surreal(default)]
    result_hash: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    summary: Option<Value>,
    #[serde(default)]
    #[surreal(default)]
    rev: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, SurrealValue)]
struct DecisionRow {
    subject: RecordId,
    #[serde(default)]
    #[surreal(default)]
    subject_email: String,
    decision: String,
    #[serde(default)]
    #[surreal(default)]
    note: String,
    reviewer: RecordId,
    #[serde(default)]
    #[surreal(default)]
    reviewer_email: String,
    decided_at: Option<Datetime>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct ListRow {
    id: RecordId,
    status: String,
    #[serde(default)]
    #[surreal(default)]
    started_by_email: String,
    started_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    completed_by_email: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    completed_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    result_hash: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    summary: Option<Value>,
    #[serde(default)]
    #[surreal(default)]
    people: i64,
}

/// One review in the list.
#[derive(Serialize, utoipa::ToSchema)]
struct AccessReviewSummary {
    id: String,
    /// `open` or `completed`.
    status: String,
    started_by: String,
    started_at: Option<String>,
    completed_by: Option<String>,
    completed_at: Option<String>,
    people: i64,
    /// Decisions per outcome once completed: `keep`, `remove`, `suspend`, `not_reviewed`.
    #[schema(value_type = Option<Object>)]
    summary: Option<Value>,
    result_hash: Option<String>,
}

fn opt_time(d: &Option<Datetime>) -> Option<String> {
    time(d).as_str().map(str::to_string)
}

/// A person's decision in a review.
#[derive(Serialize, utoipa::ToSchema)]
struct AccessReviewDecision {
    user_id: String,
    email: String,
    /// `keep`, `remove` or `suspend`.
    decision: String,
    note: String,
    reviewer_id: String,
    reviewer: String,
    decided_at: Option<String>,
}

/// A review with its snapshot and decisions.
#[derive(Serialize, utoipa::ToSchema)]
struct AccessReview {
    #[serde(flatten)]
    head: AccessReviewSummary,
    /// Who had access to what when the review started: `people`, each with `spaces`, `tokens`,
    /// `apps` and `sessions`.
    #[schema(value_type = Object)]
    snapshot: Value,
    snapshot_hash: String,
    decisions: Vec<AccessReviewDecision>,
    /// People nobody has decided about yet.
    undecided: Vec<String>,
    /// The caller: they cannot decide about themselves.
    you: String,
}

async fn load(db: &crate::pool::OrgDb, id: &str) -> AppResult<(ReviewRow, Vec<DecisionRow>)> {
    let org = db.org().key();
    let rid = crate::rid::parse(id).ok().filter(|r| r.table() == "access_review").ok_or_else(not_found)?;
    let rows: Vec<ReviewRow> = store::access_reviews::REVIEW_GET.on(db).bind(("id", rid.clone())).bind(("org", org)).await?.take(0)?;
    let row = rows.into_iter().next().ok_or_else(not_found)?;
    let decisions: Vec<DecisionRow> = store::access_reviews::REVIEW_DECISIONS.on(db).bind(("review", rid)).await?.take(0)?;
    Ok((row, decisions))
}

fn subjects(snapshot: &Value) -> Vec<(String, String)> {
    snapshot["people"]
        .as_array()
        .map(|a| a.iter().map(|p| (p["user_id"].as_str().unwrap_or_default().to_string(), p["email"].as_str().unwrap_or_default().to_string())).collect())
        .unwrap_or_default()
}

fn undecided(snapshot: &Value, decisions: &[DecisionRow]) -> Vec<String> {
    subjects(snapshot).into_iter().filter(|(id, _)| !decisions.iter().any(|d| chain::rid(&d.subject) == *id)).map(|(id, _)| id).collect()
}

fn summary_of(r: &ReviewRow) -> AccessReviewSummary {
    AccessReviewSummary {
        id: r.id.to_string(),
        status: r.status.clone(),
        started_by: r.started_by_email.clone(),
        started_at: opt_time(&r.started_at),
        completed_by: r.completed_by_email.clone(),
        completed_at: opt_time(&r.completed_at),
        people: subjects(&r.snapshot).len() as i64,
        summary: r.summary.clone(),
        result_hash: r.result_hash.clone(),
    }
}

fn view(r: ReviewRow, decisions: Vec<DecisionRow>, you: &User) -> AccessReview {
    let undecided = undecided(&r.snapshot, &decisions);
    AccessReview {
        head: summary_of(&r),
        snapshot_hash: r.snapshot_hash.clone(),
        decisions: decisions
            .into_iter()
            .map(|d| AccessReviewDecision {
                user_id: chain::rid(&d.subject),
                email: d.subject_email,
                decision: d.decision,
                note: d.note,
                reviewer_id: chain::rid(&d.reviewer),
                reviewer: d.reviewer_email,
                decided_at: opt_time(&d.decided_at),
            })
            .collect(),
        snapshot: r.snapshot,
        undecided,
        you: chain::rid(&you.id),
    }
}

/// What a completed review's `result_hash` is taken over (canonical JSON): the review, the hash
/// of its snapshot, every decision and who was not reviewed.
fn result_doc(r: &ReviewRow, decisions: &[DecisionRow], completed_by: &RecordId, completed_at: &str) -> Value {
    let mut ds: Vec<Value> = decisions
        .iter()
        .map(|d| {
            json!({
                "user_id": chain::rid(&d.subject),
                "decision": d.decision,
                "note": d.note,
                "reviewer_id": chain::rid(&d.reviewer),
                "decided_at": time(&d.decided_at),
            })
        })
        .collect();
    ds.sort_by(|a, b| a["user_id"].as_str().cmp(&b["user_id"].as_str()));
    json!({
        "format": "eunomia-access-review",
        "version": 1,
        "review_id": chain::rid(&r.id),
        "workspace": r.snapshot["workspace"],
        "snapshot_hash": r.snapshot_hash,
        "started_by": chain::rid(&r.started_by),
        "started_at": time(&r.started_at),
        "completed_by": chain::rid(completed_by),
        "completed_at": completed_at,
        "decisions": ds,
        "not_reviewed": undecided(&r.snapshot, decisions),
    })
}

// ---------------------------------------------------------------- routes

#[utoipa::path(
    operation_id = "listAccessReviews",
    get,
    path = "/api/admin/access-reviews",
    tag = "admin",
    summary = "Access reviews, newest first (admin only)",
    responses((status = 200, body = Vec<AccessReviewSummary>), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn list(State(state): State<AppState>, user: User) -> AppResult<Json<Vec<AccessReviewSummary>>> {
    super::admin::require_admin(&state, &user).await?;
    let org = state.org(&user.org).await?;
    let rows: Vec<ListRow> = store::access_reviews::REVIEW_LIST.on(&org.db).bind(("org", org.db.org().key())).bind(("limit", 50)).await?.take(0)?;
    Ok(Json(
        rows.into_iter()
            .map(|r| AccessReviewSummary {
                id: r.id.to_string(),
                status: r.status,
                started_by: r.started_by_email,
                started_at: opt_time(&r.started_at),
                completed_by: r.completed_by_email,
                completed_at: opt_time(&r.completed_at),
                people: r.people,
                summary: r.summary,
                result_hash: r.result_hash,
            })
            .collect(),
    ))
}

#[utoipa::path(
    operation_id = "startAccessReview",
    post,
    path = "/api/admin/access-reviews",
    tag = "admin",
    summary = "Start an access review: snapshot who has access to what (admin only, browser session)",
    description = "One review can be open at a time. The snapshot lists every person, their role, admin rights and suspension, their spaces, and their API tokens, connected apps and live sessions by name and count. It never holds a secret.",
    responses((status = 200, body = AccessReview), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn start(State(state): State<AppState>, user: User) -> AppResult<Json<AccessReview>> {
    // the review and its audit event are written together, whatever the client does meanwhile
    crate::audit::detached(start_now(state, user)).await
}

async fn start_now(state: AppState, user: User) -> AppResult<Json<AccessReview>> {
    authz::require_session()?;
    super::admin::require_admin(&state, &user).await?;
    let org = state.org(&user.org).await?;
    let snap = snapshot(&state, &user).await?;
    let snapshot_hash = sha256_hex(canonical_json(&snap));
    let people = snap["people"].as_array().map(Vec::len).unwrap_or(0);
    #[derive(Deserialize, SurrealValue)]
    struct Id {
        id: RecordId,
    }
    let created = store::access_reviews::REVIEW_CREATE
        .on(&org.db)
        .bind(("org", org.db.org().key()))
        .bind(("by", user.id.clone()))
        .bind(("email", user.email.clone()))
        .bind(("snapshot", snap))
        .bind(("snapshot_hash", snapshot_hash.clone()))
        .await;
    let id: Vec<Id> = match created {
        Ok(mut r) => r.take(0)?,
        Err(e) if crate::tx::is_duplicate(&e) => {
            return Err(AppError::coded(ErrorCode::AccessReviewOpen, "An access review is already open. Complete it before starting another."));
        }
        Err(e) => return Err(e.into()),
    };
    let id = id.into_iter().next().ok_or_else(|| AppError::internal("access review was not created"))?.id;
    crate::audit::record_as_caller_detail(&state.control, &user.id, "access_review.start", &id.to_string(), "ok", &format!("people={people} snapshot={snapshot_hash}")).await;
    let (row, decisions) = load(&org.db, &id.to_string()).await?;
    Ok(Json(view(row, decisions, &user)))
}

#[utoipa::path(
    operation_id = "getAccessReview",
    get,
    path = "/api/admin/access-reviews/{review_id}",
    tag = "admin",
    summary = "One access review with its snapshot and decisions (admin only)",
    params(("review_id" = String, Path)),
    responses((status = 200, body = AccessReview), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn get_review(State(state): State<AppState>, user: User, Path(review_id): Path<String>) -> AppResult<Json<AccessReview>> {
    super::admin::require_admin(&state, &user).await?;
    let org = state.org(&user.org).await?;
    let (row, decisions) = load(&org.db, &review_id).await?;
    Ok(Json(view(row, decisions, &user)))
}

#[derive(Deserialize, utoipa::ToSchema)]
struct AccessReviewDecisionBody {
    /// `keep`, `remove` or `suspend`.
    decision: String,
    /// Why, in a sentence (optional, at most 500 characters).
    #[serde(default)]
    note: String,
}

#[utoipa::path(
    operation_id = "decideAccessReview",
    put,
    path = "/api/admin/access-reviews/{review_id}/decisions/{user_id}",
    tag = "admin",
    summary = "Record a decision about one person in an open access review (admin only, browser session)",
    description = "Deciding again replaces the earlier decision while the review is open. You cannot decide about your own access. A decision is a record: suspending or removing the person is done from People.",
    params(("review_id" = String, Path), ("user_id" = String, Path)),
    request_body = AccessReviewDecisionBody,
    responses((status = 200, body = AccessReview), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn decide(
    State(state): State<AppState>,
    user: User,
    Path((review_id, user_id)): Path<(String, String)>,
    Json(body): Json<AccessReviewDecisionBody>,
) -> AppResult<Json<AccessReview>> {
    crate::audit::detached(decide_now(state, user, review_id, user_id, body)).await
}

async fn decide_now(state: AppState, user: User, review_id: String, user_id: String, body: AccessReviewDecisionBody) -> AppResult<Json<AccessReview>> {
    authz::require_session()?;
    super::admin::require_admin(&state, &user).await?;
    if !matches!(body.decision.as_str(), "keep" | "remove" | "suspend") {
        return Err(AppError::bad_request("`decision` must be keep, remove or suspend."));
    }
    let note: String = body.note.trim().chars().take(500).collect();
    let org = state.org(&user.org).await?;
    let (row, _) = load(&org.db, &review_id).await?;
    if row.status != "open" {
        return Err(completed());
    }
    let subject = subjects(&row.snapshot)
        .into_iter()
        .find(|(id, _)| *id == user_id || crate::rid::parse(&user_id).map(|r| chain::rid(&r) == *id).unwrap_or(false))
        .ok_or_else(|| AppError::coded(ErrorCode::AuthNotFound, "That person isn't in this review."))?;
    let subject_id = crate::rid::parse(&subject.0).map_err(|_| AppError::coded(ErrorCode::AuthNotFound, "That person isn't in this review."))?;
    if subject_id == user.id {
        return Err(AppError::coded(ErrorCode::AccessReviewSelf, "You can't review your own access. Ask another admin to decide about it."));
    }
    let key = crate::tx::stable_key('d', &format!("{}|{}", chain::rid(&row.id), subject.0));
    store::access_reviews::REVIEW_DECIDE
        .on(&org.db)
        .bind(("org", org.db.org().key()))
        .bind(("id", RecordId::new("access_review_decision", key.as_str())))
        .bind(("review", row.id.clone()))
        .bind(("subject", subject_id.clone()))
        .bind(("subject_email", subject.1.clone()))
        .bind(("decision", body.decision.clone()))
        .bind(("note", note))
        .bind(("reviewer", user.id.clone()))
        .bind(("reviewer_email", user.email.clone()))
        .await
        .map_err(frozen)?;
    crate::audit::record_as_caller_detail(
        &state.control,
        &user.id,
        "access_review.decide",
        &row.id.to_string(),
        "ok",
        &format!("subject={} email={} decision={}", subject.0, subject.1, body.decision),
    )
    .await;
    let (row, decisions) = load(&org.db, &review_id).await?;
    Ok(Json(view(row, decisions, &user)))
}

#[utoipa::path(
    operation_id = "completeAccessReview",
    post,
    path = "/api/admin/access-reviews/{review_id}/complete",
    tag = "admin",
    summary = "Complete an access review; it can't be changed afterwards (admin only, browser session)",
    description = "Everyone must have a decision, except you: your own access is recorded as not reviewed when no other admin decided it. The completion, with the hash of the review, goes into the security log.",
    params(("review_id" = String, Path)),
    responses((status = 200, body = AccessReview), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = [])),
)]
async fn complete(State(state): State<AppState>, user: User, Path(review_id): Path<String>) -> AppResult<Json<AccessReview>> {
    crate::audit::detached(complete_now(state, user, review_id)).await
}

async fn complete_now(state: AppState, user: User, review_id: String) -> AppResult<Json<AccessReview>> {
    authz::require_session()?;
    super::admin::require_admin(&state, &user).await?;
    let org = state.org(&user.org).await?;
    let (row, decisions) = load(&org.db, &review_id).await?;
    if row.status != "open" {
        return Err(completed());
    }
    let me = chain::rid(&user.id);
    let missing: Vec<String> = undecided(&row.snapshot, &decisions).into_iter().filter(|id| *id != me).collect();
    if !missing.is_empty() {
        let n = missing.len();
        return Err(AppError::coded(
            ErrorCode::AccessReviewIncomplete,
            format!("{n} {} still {} a decision.", if n == 1 { "person" } else { "people" }, if n == 1 { "needs" } else { "need" }),
        ));
    }
    let completed_at = chrono::DateTime::from_timestamp_micros(chrono::Utc::now().timestamp_micros()).unwrap_or_else(chrono::Utc::now);
    let doc = result_doc(&row, &decisions, &user.id, &ts(&completed_at));
    let result_hash = sha256_hex(canonical_json(&doc));
    let count = |d: &str| decisions.iter().filter(|x| x.decision == d).count();
    let not_reviewed = doc["not_reviewed"].as_array().map(Vec::len).unwrap_or(0);
    let summary = json!({ "keep": count("keep"), "remove": count("remove"), "suspend": count("suspend"), "not_reviewed": not_reviewed });
    #[derive(Deserialize, SurrealValue)]
    struct Id {
        #[allow(dead_code)]
        id: RecordId,
    }
    let done: Vec<Id> = store::access_reviews::REVIEW_COMPLETE
        .on(&org.db)
        .bind(("org", org.db.org().key()))
        .bind(("id", row.id.clone()))
        .bind(("by", user.id.clone()))
        .bind(("email", user.email.clone()))
        .bind(("at", Datetime::from(completed_at)))
        .bind(("result_hash", result_hash.clone()))
        .bind(("summary", summary.clone()))
        .bind(("rev", row.rev.unwrap_or(0)))
        .await
        .map_err(frozen)?
        .take(0)?;
    if done.is_empty() {
        return Err(AppError::coded(ErrorCode::DbConflict, "The review changed while it was being completed. Try again."));
    }
    crate::audit::record_as_caller_detail(
        &state.control,
        &user.id,
        "access_review.complete",
        &row.id.to_string(),
        "ok",
        &format!(
            "result={result_hash} keep={} remove={} suspend={} not_reviewed={not_reviewed}",
            count("keep"),
            count("remove"),
            count("suspend")
        ),
    )
    .await;
    let (row, decisions) = load(&org.db, &review_id).await?;
    Ok(Json(view(row, decisions, &user)))
}

/// A completed review as a file: the snapshot, the decisions, both hashes and the security log
/// event that recorded the completion.
#[derive(Serialize, utoipa::ToSchema)]
struct AccessReviewExport {
    /// The document `result_hash` is the SHA-256 of (canonical JSON: keys sorted, no spaces).
    #[schema(value_type = Object)]
    result: Value,
    result_hash: String,
    /// True when `result` still hashes to the `result_hash` recorded at completion.
    result_hash_matches: bool,
    /// Who had access to what at the start; `snapshot_hash` is its SHA-256 (canonical JSON).
    #[schema(value_type = Object)]
    snapshot: Value,
    snapshot_hash: String,
    snapshot_hash_matches: bool,
    /// The completion's place in the hash-chained security log, once sealed.
    audit_event: Option<AccessReviewAuditLink>,
    /// True when the chained completion event commits to this review as it is now: its detail
    /// names this `result` document's hash and the chain holds that detail's plain SHA-256. False
    /// means the review was changed after it was completed (even if `result_hash` was recomputed).
    /// The offline verifier checks the same: `verify-audit-export.py LOG.json --review THIS.json`.
    chain_matches: bool,
    decisions: Vec<AccessReviewDecision>,
}

#[derive(Serialize, utoipa::ToSchema)]
struct AccessReviewAuditLink {
    ledger: String,
    seq: Option<i64>,
    hash: Option<String>,
    created_at: Option<String>,
    /// The event's detail as written (`result=<hash> keep=.. remove=.. suspend=.. not_reviewed=..`).
    /// Its SHA-256 is the event's `detail_digest` in the chain.
    detail: String,
}

#[utoipa::path(
    operation_id = "exportAccessReview",
    get,
    path = "/api/admin/access-reviews/{review_id}/export",
    tag = "admin",
    summary = "Download a completed access review with the hashes that tie it to the security log (admin only)",
    params(("review_id" = String, Path)),
    responses((status = 200, body = AccessReviewExport), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])),
)]
async fn export(State(state): State<AppState>, user: User, Path(review_id): Path<String>) -> AppResult<impl IntoResponse> {
    super::admin::require_admin(&state, &user).await?;
    let org = state.org(&user.org).await?;
    let (row, decisions) = load(&org.db, &review_id).await?;
    let (Some(by), Some(at), Some(result_hash)) = (row.completed_by.clone(), row.completed_at, row.result_hash.clone()) else {
        return Err(AppError::coded(ErrorCode::AccessReviewIncomplete, "Only a completed access review can be downloaded."));
    };
    let result = result_doc(&row, &decisions, &by, time(&Some(at)).as_str().unwrap_or_default());
    let ledger = chain::org_ledger(&user.org);
    // the completion event is sealed in the background: make sure it has its place in the chain
    if let Err(e) = chain::seal(&state.control, &ledger).await {
        tracing::warn!(error = %e.message, "audit seal before review export failed");
    }
    #[derive(Deserialize, SurrealValue)]
    struct Found {
        #[serde(default)]
        #[surreal(default)]
        seq: Option<i64>,
        #[serde(default)]
        #[surreal(default)]
        hash: Option<String>,
        created_at: Option<Datetime>,
        #[serde(default)]
        #[surreal(default)]
        detail: String,
        #[serde(default)]
        #[surreal(default)]
        detail_digest: Option<String>,
        #[serde(default)]
        #[surreal(default)]
        digest_key: Option<String>,
    }
    let found: Vec<Found> = store::control::AUDIT_EVENT_FIND
        .on(&state.control)
        .bind(("action", "access_review.complete"))
        .bind(("target", row.id.to_string()))
        .bind(("ledger", ledger.clone()))
        .await?
        .take(0)?;
    let current_hash = sha256_hex(canonical_json(&result));
    let chain_matches = found.first().is_some_and(|f| {
        f.seq.is_some()
            && f.digest_key.as_deref() == Some(chain::PLAIN)
            && f.detail_digest.as_deref() == Some(sha256_hex(&f.detail).as_str())
            && f.detail.split_whitespace().any(|w| w == format!("result={current_hash}"))
    });
    let audit_event = found
        .into_iter()
        .next()
        .map(|f| AccessReviewAuditLink { ledger, seq: f.seq, hash: f.hash, created_at: opt_time(&f.created_at), detail: f.detail });
    let snapshot = row.snapshot.clone();
    let snapshot_hash = row.snapshot_hash.clone();
    let key = crate::rid::key_string(row.id.key()).unwrap_or_default();
    let out = AccessReviewExport {
        result_hash_matches: sha256_hex(canonical_json(&result)) == result_hash,
        result,
        result_hash,
        snapshot_hash_matches: sha256_hex(canonical_json(&snapshot)) == snapshot_hash,
        snapshot,
        snapshot_hash,
        audit_event,
        chain_matches,
        decisions: view(row, decisions, &user).decisions,
    };
    let name = format!("eunomia-access-review-{key}.json");
    Ok(([(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\""))], Json(out)))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(list, start, get_review, decide, complete, export))]
pub struct Doc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_sorts_keys_at_every_level() {
        let v = json!({ "b": 1, "a": { "d": [ { "z": 1, "y": 2 } ], "c": null } });
        assert_eq!(canonical_json(&v), r#"{"a":{"c":null,"d":[{"y":2,"z":1}]},"b":1}"#);
    }
}
