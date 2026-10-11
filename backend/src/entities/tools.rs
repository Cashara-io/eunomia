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
use crate::error::AppResult;

use super::{consolidate, service};

/// Case-insensitive substring match against an entity's name or any alias --
/// split out as a pure function so the search filter is unit-testable.
fn matches_search(name: &str, aliases: &[String], needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let needle = needle.to_lowercase();
    name.to_lowercase().contains(&needle) || aliases.iter().any(|a| a.to_lowercase().contains(&needle))
}

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
    let vaults = crate::vaults::service::read_vaults(db, owner, vault_ids).await?;
    // Name/alias matches first, then entities a fact mentions it in.
    let mut hits = Vec::new();
    let mut mentioned = Vec::new();
    for vault in &vaults {
        let page = service::list_entities(db, owner, kind, Some(vault), None, 0).await?;
        let in_facts = if needle.is_empty() { Default::default() } else { service::subjects_mentioning(db, vault, needle).await? };
        for r in page.results {
            if matches_search(&r.name, &r.aliases, needle) {
                hits.push(r);
            } else if in_facts.contains(&r.id) {
                mentioned.push(r);
            }
        }
    }
    hits.extend(mentioned);
    let total = hits.len();
    let sliced: Vec<service::EntityOut> = hits.into_iter().skip(offset).take(limit).collect();
    let has_more = offset + sliced.len() < total;
    Ok(service::ListEntitiesOut { results: sliced, total, has_more })
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

/// Writes the fact, then has the chat model mark earlier facts about the
/// subject it makes no longer true (see `supersede`).
#[allow(clippy::too_many_arguments)]
pub async fn memory_write(
    db: &OrgDb,
    settings: &Settings,
    owner: &RecordId,
    subject_name: &str,
    subject_kind: &str,
    text: &str,
    source_record_id: Option<&str>,
    mem_type: &str,
    vault_id: Option<&RecordId>,
) -> AppResult<service::WriteMemoryOut> {
    let mut out = service::write_memory(db, owner, subject_name, subject_kind, text, source_record_id, mem_type, vault_id).await?;
    if out.memory.mem_type != "observation" {
        let subject = crate::rid::parse(&out.entity.id).map_err(|_| crate::error::AppError::internal("entity id did not round-trip"))?;
        let new = crate::rid::parse(&out.memory.id).map_err(|_| crate::error::AppError::internal("memory id did not round-trip"))?;
        out.superseded = super::supersede::check(db, settings, owner, &subject, &[new]).await.iter().map(|r| r.to_string()).collect();
    }
    Ok(out)
}

pub async fn memory_update(
    db: &OrgDb,
    owner: &RecordId,
    memory_id: &RecordId,
    text: Option<&str>,
    new_type: Option<&str>,
) -> AppResult<Option<service::MemoryOut>> {
    service::update_memory(db, owner, memory_id, text, new_type).await
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

pub async fn memory_delete(db: &OrgDb, owner: &RecordId, memory_id: &RecordId) -> AppResult<bool> {
    service::delete_memory(db, owner, memory_id).await
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
    fn matches_search_matches_name_case_insensitively() {
        assert!(matches_search("Alice Smith", &[], "alice"));
        assert!(matches_search("Alice Smith", &[], "SMITH"));
        assert!(!matches_search("Alice Smith", &[], "bob"));
    }

    #[test]
    fn matches_search_checks_aliases_too() {
        assert!(matches_search("Robert", &["Bob".to_string(), "Bobby".to_string()], "bob"));
        assert!(!matches_search("Robert", &["Bobby".to_string()], "xyz"));
    }

    #[test]
    fn matches_search_empty_needle_matches_everything() {
        assert!(matches_search("Anything", &[], ""));
    }

    #[test]
    fn consolidate_out_default_is_all_empty() {
        let out = ConsolidateOut::default();
        assert!(out.consolidated.is_empty());
        assert!(out.skipped.is_empty());
        assert!(out.errors.is_empty());
    }
}
