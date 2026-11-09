//! Durable, transactionally appended ontology activity and review preconditions.
use crate::{
    authz::{self, Action},
    error::{AppError, AppResult, ErrorCode},
    pool::OrgDb,
    store::ontology_activity as q,
};
use serde::{Deserialize, Serialize};
use std::future::Future;
use surrealdb::types::{Datetime, RecordId, SurrealValue};

pub const SCHEMA_VERSION: i64 = 42;

#[derive(Clone)]
pub struct DecisionContext {
    pub revision: i64,
    pub reason: Option<String>,
}
tokio::task_local! { static DECISION: DecisionContext; pub static ACTOR_NAME: String; }
pub fn actor_name() -> String {
    ACTOR_NAME
        .try_with(Clone::clone)
        .unwrap_or_else(|_| "System".into())
}
pub fn decision_context() -> Option<DecisionContext> {
    DECISION.try_with(Clone::clone).ok()
}
pub fn require_ready(db: &OrgDb) -> AppResult<()> {
    if db.schema_version() < SCHEMA_VERSION {
        return Err(AppError::coded(
            ErrorCode::TenantSchemaBehind,
            "This space is upgrading. Try again shortly.",
        ));
    }
    Ok(())
}

pub fn check_seen(revision: i64) -> AppResult<()> {
    if decision_context().is_some_and(|seen| seen.revision != revision) {
        return Err(AppError::coded(
            ErrorCode::OntologyStaleRevision,
            "This definition changed since you reviewed it. Review the latest version before deciding.",
        ));
    }
    Ok(())
}

pub async fn verify_seen(db: &OrgDb, id: &RecordId) -> AppResult<i64> {
    let rows: Vec<Option<i64>> = q::REVISION.on(db).bind(("id", id.clone())).await?.take(0)?;
    let revision = rows.into_iter().flatten().next().unwrap_or(0);
    check_seen(revision)?;
    Ok(revision)
}

pub async fn reviewing<T>(
    revision: i64,
    reason: Option<String>,
    work: impl Future<Output = AppResult<T>>,
) -> AppResult<T> {
    if revision < 0 {
        return Err(AppError::bad_request("revision must not be negative"));
    }
    let reason = super::clean_text("decision reason", reason.as_deref())?;
    DECISION
        .scope(DecisionContext { revision, reason }, work)
        .await
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue, utoipa::ToSchema)]
pub struct ActivityEvent {
    pub seq: i64,
    #[schema(value_type = String)]
    pub at: Datetime,
    pub actor: String,
    pub actor_type: String,
    pub actor_name: String,
    pub action: String,
    pub definition_type: String,
    pub kind: String,
    pub name: String,
    pub target: String,
    pub revision: i64,
    pub reason: Option<String>,
    pub before: Option<serde_json::Value>,
    pub after: Option<serde_json::Value>,
}
#[derive(Default, Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ActivityQuery {
    #[serde(default)]
    pub after: i64,
    #[serde(default)]
    pub newest: bool,
    #[serde(default)]
    pub review_only: bool,
    pub before: Option<i64>,
    pub actor: Option<String>,
    pub actor_type: Option<String>,
    pub action: Option<String>,
    pub definition_type: Option<String>,
    pub kind: Option<String>,
    pub target: Option<String>,
    pub name: Option<String>,
    pub limit: Option<i64>,
}
#[derive(Serialize, utoipa::ToSchema)]
pub struct ActivityPage {
    pub events: Vec<ActivityEvent>,
    pub next: i64,
    pub more: bool,
}

/// Static optional predicates keep all filter values parameterised.
pub async fn page(db: &OrgDb, vault: &RecordId, args: &ActivityQuery) -> AppResult<ActivityPage> {
    if db.schema_version() < SCHEMA_VERSION {
        return Ok(ActivityPage {
            events: Vec::new(),
            next: args.after,
            more: false,
        });
    }
    let limit = args.limit.unwrap_or(30);
    if args.after < 0 || args.before.is_some_and(|n| n < 0) || !(1..=200).contains(&limit) {
        return Err(AppError::bad_request("invalid activity cursor or limit"));
    }
    let mut events: Vec<ActivityEvent> = (if args.newest { &q::NEWEST } else { &q::PAGE })
        .on(db)
        .bind(("vault", vault.clone()))
        .bind(("after", args.after))
        .bind(("before", args.before))
        .bind(("review_only", args.review_only))
        .bind(("actor", args.actor.clone()))
        .bind(("actor_type", args.actor_type.clone()))
        .bind(("action", args.action.clone()))
        .bind(("definition_type", args.definition_type.clone()))
        .bind(("name", args.name.clone()))
        .bind(("kind", args.kind.clone()))
        .bind(("target", args.target.clone()))
        .bind(("limit", limit + 1))
        .await?
        .take(0)?;
    for event in &mut events {
        if !matches!(event.action.as_str(), "approved" | "rejected" | "merged" | "described") { event.reason = None; }
        if event.definition_type == "import" { event.name = "CSV import".into(); }
        for fields in [&mut event.before, &mut event.after] {
            if let Some(serde_json::Value::Object(object)) = fields {
                object.retain(|key, value| ["revision", "values", "entities", "links"].contains(&key.as_str()) && value.is_u64());
            } else {
                *fields = None;
            }
        }
    }
    let more = events.len() > limit as usize;
    events.truncate(limit as usize);
    let next = events.last().map_or(args.after, |e| e.seq);
    Ok(ActivityPage { events, next, more })
}
pub async fn get(
    db: &OrgDb,
    user: &RecordId,
    vault: &RecordId,
    args: &ActivityQuery,
) -> AppResult<ActivityPage> {
    authz::authorize(db, user, Action::ReadMemories, vault).await?;
    page(db, vault, args).await
}
pub async fn head(db: &OrgDb, vault: &RecordId) -> AppResult<i64> {
    let rows: Vec<Option<i64>> = q::HEAD
        .on(db)
        .bind(("vault", vault.clone()))
        .await?
        .take(0)?;
    Ok(rows.into_iter().flatten().next().unwrap_or(0))
}

/// Missing proposal revisions allow creation and identical retries; changes need the current revision.
pub fn proposal_revision(revision: i64) -> AppResult<i64> {
    if revision < 0 { return Err(AppError::bad_request("revision must not be negative")); }
    Ok(revision)
}
