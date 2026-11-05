//! Built-in job kinds. Each handler re-derives what to do from the database, so a duplicate or
//! late run is harmless. Without an LLM or embeddings key they log and finish cleanly.

use surrealdb::types::SurrealValue;
use serde_json::json;
use surrealdb::types::RecordId;
use crate::rid::RecordIdExt;

use super::worker::Registry;
use super::{kind, Job, JobError, NewJob};
use crate::cache::search;
use crate::embeddings::service as embeddings;
use crate::entities::{consolidate, extract};
use crate::error::{AppError, ErrorCode};
use crate::state::{AppState, OrgState};
use crate::store;

pub fn registry() -> Registry {
    Registry::new()
        .register(kind::SYNC, sync)
        .register(kind::EMBED, embed)
        .register(kind::EXTRACT, extract_record)
        .register(kind::CONSOLIDATE, consolidate_subject)
        .register(kind::PRUNE_CAPSULES, prune_capsules)
        .register(kind::PRUNE_AUTH, prune_auth)
        .register(kind::MIGRATE_TENANT, migrate_tenant)
        .register(kind::INDEX_DOCUMENT, crate::documents::index_job)
        .register(kind::SUPERSEDE, supersede)
        .register(kind::REDACT_BACKFILL, redact_backfill)
}

/// The org database a job works in. A missing org is permanent; a database that is not ready or is
/// behind on schema is retried (`AppError` 5xx), an unknown org is not (404).
async fn org_state(state: &AppState, job: &Job) -> Result<OrgState, JobError> {
    Ok(state.org(&job.org_id()?).await?)
}

fn payload_str<'a>(job: &'a Job, field: &str) -> Result<&'a str, JobError> {
    job.payload
        .get(field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| JobError::permanent(ErrorCode::ValidationInvalid, format!("job payload has no {field:?}")))
}

/// One source for one user. `sync_source` records health and backoff in `sync_status` and never
/// fails, so a failing source ends this job normally and the next window tries again. A run that finds
/// another sync of the same source in progress stands down.
async fn sync(state: AppState, job: Job) -> Result<(), JobError> {
    let source = payload_str(&job, "source")?;
    let state = org_state(&state, &job).await?;
    let report = crate::sources::scheduler::sync_source(&state, &job.owner, source, Some(&job.id)).await;
    match (report.get("error"), report.get("status")) {
        (Some(e), _) => tracing::warn!(source, error = %e, "source sync failed"),
        (None, Some(_)) => tracing::info!(source, "source sync skipped: already running"),
        (None, None) => tracing::info!(source, report = %report, "source synced"),
    }
    Ok(())
}

const EMBED_BATCH: i64 = 50;
/// Per run; the reconciler enqueues again while a backlog remains.
const EMBED_BATCHES: usize = 10;

/// Embed cache records whose `embedding` is NONE, then memories without a vector from the current model.
async fn embed(state: AppState, job: Job) -> Result<(), JobError> {
    let state = org_state(&state, &job).await?;
    #[derive(serde::Deserialize, SurrealValue)]
    struct Row {
        id: RecordId,
        title: String,
        body_text: String,
    }
    let db = &state.db;
    if !embeddings::available(db, &state.settings, &job.owner).await {
        tracing::info!("embed skipped: no embeddings key configured");
        return Ok(());
    }
    for _ in 0..EMBED_BATCHES {
        let mut res = store::jobs::EMBED_BACKLOG
            .on(db)
            .bind(("owner", job.owner.clone()))
            .bind(("limit", EMBED_BATCH))
            .await
            .map_err(AppError::from)?;
        let rows: Vec<Row> = res.take(0).map_err(AppError::from)?;
        if rows.is_empty() {
            break;
        }
        let texts: Vec<String> = rows.iter().map(|r| format!("{}\n{}", r.title, r.body_text).trim().to_string()).collect();
        let vectors = embeddings::embed(db, &state.settings, &texts, Some(&job.owner)).await?;
        for (row, vector) in rows.iter().zip(vectors) {
            if vector.len() != embeddings::dim() {
                return Err(JobError::permanent(ErrorCode::Internal, format!("embedding for {} has dimension {}", row.id.to_string(), vector.len())));
            }
            store::cache::SET_EMBEDDING
                .on(db)
                .bind(("id", row.id.clone()))
                .bind(("embedding", vector))
                .await
                .map_err(AppError::from)?;
        }
    }
    // remembered facts too, so recall's semantic arm finds them by meaning (issue #65)
    for _ in 0..EMBED_BATCHES {
        if embeddings::embed_memory_backlog(db, &state.settings, &job.owner, EMBED_BATCH as usize).await? < EMBED_BATCH as usize {
            break;
        }
    }
    // and skills (name + description) left without a vector
    crate::tools::registry::skills::embed_backlog(&state, &job.owner, EMBED_BATCH as usize).await?;
    Ok(())
}

/// Pull entities out of one cache record, then queue consolidation for each entity that got a fact.
async fn extract_record(state: AppState, job: Job) -> Result<(), JobError> {
    let state = org_state(&state, &job).await?;
    let db = &state.db;
    let record_id = payload_str(&job, "record")?;
    if !embeddings::chat_available(db, &state.settings, &job.owner).await {
        tracing::info!(record = record_id, "extract skipped: no LLM key configured");
        return Ok(());
    }
    let Some(rec) = search::get_own(db, &job.owner, record_id).await? else { return Ok(()) };
    if rec.deleted {
        return Ok(());
    }
    let occurred_at = rec.occurred_at.as_ref().and_then(crate::sources::base::datetime_to_chrono).map(|d| d.to_rfc3339());
    // facts go to the vault the record is filed in (routing), the owner's personal vault when none
    let record = extract::ExtractRecord { id: rec.id, title: rec.title, body_text: rec.body_text, occurred_at, vault: rec.vault };
    for subject in extract::extract_entities(db, &state.settings, &job.owner, &record).await {
        if let Ok(subject) = crate::rid::parse(&subject) {
            super::enqueue_lossy(&state.control, super::leader::consolidate_job(state.db.org(), &job.owner, &subject)).await;
        }
    }
    Ok(())
}

/// Rewrite one entity's observation from its raw facts.
async fn consolidate_subject(state: AppState, job: Job) -> Result<(), JobError> {
    let state = org_state(&state, &job).await?;
    let db = &state.db;
    let subject = payload_str(&job, "subject")?;
    if !embeddings::chat_available(db, &state.settings, &job.owner).await {
        tracing::info!(subject, "consolidate skipped: no LLM key configured");
        return Ok(());
    }
    let subject: RecordId = crate::rid::parse(subject).map_err(|_| JobError::permanent(ErrorCode::ValidationInvalid, "bad subject id"))?;
    let mission = consolidate::observations_mission(db, &job.owner).await?;
    consolidate::consolidate_subject(db, &state.settings, &job.owner, &subject, Some(&mission)).await?;
    Ok(())
}

/// The check `memory_write` used to wait for: earlier facts about `subject` the new `memory` makes
/// untrue are marked superseded (and the subject's observation stale, so consolidation rebuilds it).
/// Best effort like the rest of the enrichment: a failed model call marks nothing.
async fn supersede(state: AppState, job: Job) -> Result<(), JobError> {
    let state = org_state(&state, &job).await?;
    let parse = |field| {
        payload_str(&job, field).and_then(|v| crate::rid::parse(v).map_err(|_| JobError::permanent(ErrorCode::ValidationInvalid, format!("bad {field} id"))))
    };
    let (subject, memory) = (parse("subject")?, parse("memory")?);
    crate::entities::supersede::check(&state.db, &state.settings, &job.owner, &subject, &[memory]).await;
    Ok(())
}

/// The org's secret backfill, from its saved cursor to the end. A crash or a lost lease resumes it there.
async fn redact_backfill(state: AppState, job: Job) -> Result<(), JobError> {
    let state = org_state(&state, &job).await?;
    super::redact_backfill::run(&state, None).await?;
    Ok(())
}

/// One supersede check per written fact.
pub fn supersede_job(org: crate::pool::OrgId, owner: &RecordId, subject: &RecordId, memory: &RecordId) -> NewJob {
    let key = format!("{}:{}:{}", kind::SUPERSEDE, org.key(), memory.to_string());
    NewJob::new(kind::SUPERSEDE, owner.clone(), key)
        .in_org(org)
        .payload(json!({ "subject": subject.to_string(), "memory": memory.to_string() }))
}

/// Queue entity extraction for a freshly written cache record. `hash` is its content hash, so an
/// unchanged record never extracts twice. Best effort: a lost enqueue only delays enrichment.
pub async fn enqueue_extract(state: &OrgState, owner: &RecordId, record_id: &str, hash: &str) {
    let key = format!("{}:{}:{record_id}:{hash}", kind::EXTRACT, crate::sources::base::owner_key_str(owner));
    let job = NewJob::new(kind::EXTRACT, owner.clone(), key).in_org(state.db.org()).payload(json!({ "record": record_id }));
    super::enqueue_lossy(&state.control, job).await;
}

/// Failure capsules older than 7 days, and all but the newest 1000.
async fn prune_capsules(state: AppState, _job: Job) -> Result<(), JobError> {
    Ok(crate::capsules::prune_default(&state.control).await?)
}

/// Tool-usage hours past `TOOL_USAGE_RETENTION_DAYS`, audit events past `AUDIT_RETENTION_DAYS` and sessions expired for `SESSION_RETENTION_DAYS`.
async fn prune_auth(state: AppState, _job: Job) -> Result<(), JobError> {
    crate::usage::prune_default(&state.control).await?;
    Ok(crate::audit::prune_default(&state.control).await?)
}

/// Bring one org's database to the latest tenant schema. Idempotent: migrating a current database is a no-op.
async fn migrate_tenant(state: AppState, job: Job) -> Result<(), JobError> {
    let org = job.org_id()?;
    let Some(p) = &state.provisioner else {
        return Err(JobError::permanent(ErrorCode::TenantProvisioningDisabled, "this build cannot migrate org databases"));
    };
    p.migrate_org(&state.control, &org).await?;
    Ok(())
}
