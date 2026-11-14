//! Entity-memory tool functions -- the business logic behind the
//! `entities_*`/`memory_*`/`entity_*`/`code_*`/`consolidate_observations`
//! tools the registry exposes for MCP use.
//!
//! This module keeps only the logic: find-or-create/search/merge/
//! delete/consolidate. Registration and the error-to-`{"error": ...}` wrapper
//! live in `tools::registry`; every function here returns `AppResult<T>`.
//!

use surrealdb::types::RecordId;
use crate::rid::RecordIdExt;

use crate::config::Settings;
use crate::pool::{ControlDb, OrgDb};
use crate::state::OrgState;
use crate::error::AppResult;

use super::{consolidate, service};

/// Entities whose name or an alias contains `query` (case-insensitively), then the ones a fact
/// mentions it in, in the caller's personal vault plus every org vault they belong to (or `vault_id`).
/// One round trip; only the page leaves the database (see `store::entities::search_entities`).
pub async fn entities_search(
    db: &OrgDb,
    owner: &RecordId,
    query: &str,
    kind: Option<&str>,
    limit: usize,
    offset: usize,
    vault_ids: &[RecordId],
) -> AppResult<service::ListEntitiesOut> {
    let needle = query.trim();
    // No vault given: the personal vault plus every org vault the caller
    // belongs to, so org facts aren't invisible to a lookup without `vault_id`.
    let vaults = crate::vaults::service::read_scope(db, owner, vault_ids).await?.vaults;
    service::search_entities(db, &vaults, needle, kind, limit, offset).await
}

/// The built-in kinds and the custom kinds in the caller's personal vault plus every org vault they
/// belong to (or `vault_ids`), the same read scope as [`entities_search`].
pub async fn entity_types(db: &OrgDb, owner: &RecordId, vault_ids: &[RecordId]) -> AppResult<service::EntityTypesOut> {
    let scope = crate::vaults::service::read_scope(db, owner, vault_ids).await?;
    service::entity_types(db, &scope.vaults, &scope.names).await
}

pub async fn entities_get(db: &OrgDb, control: &ControlDb, owner: &RecordId, id: &RecordId) -> AppResult<Option<service::EntityDetail>> {
    service::get_entity(db, control, owner, id).await
}

pub async fn entities_graph(
    db: &OrgDb,
    control: &ControlDb,
    owner: &RecordId,
    kinds: Option<&[String]>,
    vault_id: Option<&RecordId>,
) -> AppResult<service::GraphOut> {
    service::graph(db, control, owner, kinds, vault_id).await
}

pub async fn code_entity_upsert(
    db: &OrgDb,
    owner: &RecordId,
    kind: &str,
    name: &str,
    parent_id: Option<&RecordId>,
    summary: Option<&str>,
    vault_id: Option<&RecordId>,
) -> AppResult<service::EntityOut> {
    service::upsert_code_entity(db, owner, kind, name, parent_id, summary, vault_id).await
}

/// Clearer-named alias for `add_relation`, for code-graph use -- same
/// implementation, just exposed directly as a tool (unlike `add_relation`,
/// which is otherwise only called internally).
pub async fn code_relate(
    db: &OrgDb,
    owner: &RecordId,
    from_id: &RecordId,
    to_id: &RecordId,
    label: &str,
    source_record_id: Option<&str>,
) -> AppResult<service::RelationOut> {
    service::add_relation(db, owner, from_id, to_id, label, source_record_id).await
}

/// `entity_relate`'s result: the relation, and the fact saved for its reason, when this call first wrote or restored it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EntityRelateOut {
    #[serde(flatten)]
    pub relation: service::RelationOut,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_memory_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub redacted: Vec<crate::redact::Redaction>,
}

/// A labelled relation between two business entities (a truck `carries` an order, a product
/// `supplied_by` an organisation), with the same checks as `code_relate`, and an optional reason saved
/// as a fact on `from_id`.
pub async fn entity_relate(
    db: &OrgDb,
    owner: &RecordId,
    from_id: &RecordId,
    to_id: &RecordId,
    label: &str,
    reason: Option<&str>,
) -> AppResult<EntityRelateOut> {
    let (relation, memory) = service::relate_with_reason(db, owner, from_id, to_id, label, reason).await?;
    let (reason_memory_id, redacted) = match memory {
        Some(m) => (Some(m.id), m.redacted),
        None => (None, Vec::new()),
    };
    Ok(EntityRelateOut { relation, reason_memory_id, redacted })
}

/// Writes the fact. With a chat model, a `supersede` job then marks the earlier facts about the
/// subject it makes no longer true (see `supersede`); the write does not wait for the model.
#[allow(clippy::too_many_arguments)]
pub async fn memory_write(
    state: &OrgState,
    owner: &RecordId,
    subject_name: &str,
    subject_kind: &str,
    text: &str,
    source_record_id: Option<&str>,
    mem_type: &str,
    vault_id: Option<&RecordId>,
    occurred_at: Option<&str>,
) -> AppResult<service::WriteMemoryOut> {
    let occurred_at = occurred_at.map(|s| crate::cache::search::parse_datetime("occurred_at", s).map(surrealdb::types::Datetime::from)).transpose()?;
    // the date goes in with the fact, in one statement: no reader ever sees it undated
    let out = service::write_memory(&state.db, owner, subject_name, subject_kind, text, source_record_id, mem_type, vault_id, occurred_at).await?;
    embed_soon(state, owner);
    if out.memory.mem_type != "observation" && super::supersede::enabled(&state.db, &state.settings, owner).await {
        let subject = crate::rid::parse(&out.entity.id).map_err(|_| crate::error::AppError::internal("entity id did not round-trip"))?;
        let memory = crate::rid::parse(&out.memory.id).map_err(|_| crate::error::AppError::internal("memory id did not round-trip"))?;
        crate::jobs::enqueue_lossy(&state.control, crate::jobs::handlers::supersede_job(state.db.org(), owner, &subject, &memory)).await;
    }
    Ok(out)
}

/// Embed the owner's new facts now, off the request path, so recall finds them by meaning within
/// seconds rather than at the next embed job. Best effort: the job picks up whatever this misses.
fn embed_soon(state: &OrgState, owner: &RecordId) {
    let (db, settings, owner) = (state.db.clone(), state.settings.clone(), owner.clone());
    tokio::spawn(crate::authz::as_system(async move {
        if !crate::embeddings::service::available(&db, &settings, &owner).await {
            return;
        }
        if let Err(e) = crate::embeddings::service::embed_memory_backlog(&db, &settings, &owner, EMBED_SOON_BATCH).await {
            tracing::debug!(error = %e.message, "embedding a new memory failed; the embed job will retry");
        }
    }));
}

/// Memories embedded right after a write (the new one and any backlog alongside it).
const EMBED_SOON_BATCH: usize = 16;

/// Edits a fact. Changing the text of a current raw fact is being told it again (its `asserted_at` moves
/// to now), so it is checked again, like a new write, against the subject's other facts. A type-only
/// edit, or an edit of a replaced fact (which stays replaced), tells nothing and queues nothing.
pub async fn memory_update(
    state: &OrgState,
    owner: &RecordId,
    memory_id: &RecordId,
    text: Option<&str>,
    new_type: Option<&str>,
) -> AppResult<Option<service::MemoryOut>> {
    let out = service::update_memory_told(&state.db, owner, memory_id, text, new_type).await?;
    if let Some((m, true)) = &out
        && let (Ok(subject), Ok(id)) = (crate::rid::parse(&m.subject), crate::rid::parse(&m.id))
    {
        recheck(state, owner, &subject, &id, Some(m.version)).await;
    }
    Ok(out.map(|(m, _)| m))
}

/// Deletes a memory. The facts it had replaced are current again, and each gets a supersede check.
pub async fn memory_delete(state: &OrgState, owner: &RecordId, memory_id: &RecordId) -> AppResult<bool> {
    let Some(restored) = service::delete_memory_restoring(&state.db, owner, memory_id).await? else { return Ok(false) };
    for (subject, id) in &restored {
        recheck(state, owner, subject, id, None).await;
    }
    Ok(true)
}

/// Queues a supersede check for one raw fact: after an edit that told it again (keyed by its new
/// `version`, so it is not taken for the check its first write queued), or after it became current
/// again. Best effort, like the check after a write.
async fn recheck(state: &OrgState, owner: &RecordId, subject: &RecordId, id: &RecordId, version: Option<i64>) {
    if !super::supersede::enabled(&state.db, &state.settings, owner).await {
        return;
    }
    let job = match version {
        Some(v) => crate::jobs::handlers::supersede_edit_job(state.db.org(), owner, subject, id, v),
        None => crate::jobs::handlers::supersede_restored_job(state.db.org(), owner, subject, id),
    };
    crate::jobs::enqueue_lossy(&state.control, job).await;
}

pub async fn entity_update(
    db: &OrgDb,
    owner: &RecordId,
    entity_id: &RecordId,
    name: Option<&str>,
    aliases: Option<Vec<String>>,
    summary: Option<&str>,
) -> AppResult<Option<service::EntityOut>> {
    service::update_entity(db, owner, entity_id, name, aliases, summary).await
}

pub async fn entity_delete(db: &OrgDb, owner: &RecordId, entity_id: &RecordId) -> AppResult<bool> {
    service::delete_entity(db, owner, entity_id).await
}

pub async fn entity_merge(
    db: &OrgDb,
    owner: &RecordId,
    winner_id: &RecordId,
    loser_id: &RecordId,
) -> AppResult<service::EntityOut> {
    service::merge_entities(db, owner, winner_id, loser_id).await
}

#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct ConsolidateOut {
    pub consolidated: Vec<String>,
    pub skipped: Vec<String>,
    pub errors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

const NO_MODEL_NOTE: &str = "Eunomia has no model configured, so nothing was consolidated. Do it yourself: read the \
entity with `entities_get`, then record the updated belief with `memory_write` (type `observation`).";

/// Consolidate one entity's raw facts into its observation (`subject_id`
/// given), or every entity for `owner` that has new unconsolidated raw facts
/// (omitted).
pub async fn consolidate_observations(
    db: &OrgDb,
    settings: &Settings,
    owner: &RecordId,
    subject_id: Option<&RecordId>,
) -> AppResult<ConsolidateOut> {
    if !crate::embeddings::service::chat_available(db, settings, owner).await {
        return Ok(ConsolidateOut { note: Some(NO_MODEL_NOTE.to_string()), ..Default::default() });
    }
    let mission = consolidate::observations_mission(db, owner).await?;

    if let Some(sid) = subject_id {
        let result = consolidate::consolidate_subject(db, settings, owner, sid, Some(&mission)).await?;
        return Ok(match result {
            Some(_) => ConsolidateOut { consolidated: vec![sid.to_string()], ..Default::default() },
            None => ConsolidateOut { skipped: vec![sid.to_string()], ..Default::default() },
        });
    }

    let mut out = ConsolidateOut::default();
    let all = service::list_entities(db, owner, None, None, None, 0).await?;
    for entity in all.results {
        let sid: RecordId = match crate::rid::parse(&entity.id) {
            Ok(id) => id,
            Err(_) => {
                out.errors.push(format!("{}: invalid entity id", entity.id));
                continue;
            }
        };
        match consolidate::consolidate_subject(db, settings, owner, &sid, Some(&mission)).await {
            Ok(Some(_)) => out.consolidated.push(entity.id),
            Ok(None) => out.skipped.push(entity.id),
            Err(e) => out.errors.push(format!("{}: {}", entity.id, e.message)),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consolidate_out_default_is_all_empty() {
        let out = ConsolidateOut::default();
        assert!(out.consolidated.is_empty());
        assert!(out.skipped.is_empty());
        assert!(out.errors.is_empty());
    }
}
