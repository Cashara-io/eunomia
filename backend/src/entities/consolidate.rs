//! Observation consolidation: periodically synthesizes an entity's raw
//! `memory` facts ("world"/"experience" rows) into one durable belief
//! statement, stored as that subject's `type="observation"` memory row.
//!
//! Modeled on vectorize.io's Hindsight "observations" concept, with
//! deliberate scope cuts: scope is always one entity (`subject`), no
//! NLP-based world/experience classification, "update history" is just the
//! `version` counter, and the mission text is a single configurable string
//! rather than a full strategy-matching system.
//!
//! Staleness: handled eagerly, not at read time -- `service::add_memory`,
//! `update_memory` and `delete_memory` set `status="stale"` on a subject's
//! observation the moment one of its raw facts is written, edited or deleted
//! (a deletion also drops the fact from the observation's lineage). A fresh
//! observation is extended with just the facts not yet in its lineage; a
//! stale one is rebuilt from scratch out of the subject's surviving facts --
//! the old belief is not carried forward, since it may rest on a fact that
//! has since been corrected or deleted. Recall leaves stale observations out.
//! This module only ever sets `status="fresh"` again, once consolidation has
//! caught up; without a model it changes nothing.
//! New facts are first checked against earlier ones (`supersede`); a fact
//! they make untrue is marked superseded and left out of the rebuild.
//!
//! Best-effort throughout, same safety pattern as `extract.rs`: never fails
//! the caller, no-ops in stub-backend mode.
//!

use surrealdb::types::SurrealValue;
use std::collections::HashSet;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use surrealdb::types::RecordId;
use crate::rid::RecordIdExt;

use crate::config::Settings;
use crate::pool::OrgDb;
use crate::store;
use crate::error::{AppError, AppResult};
use crate::store::entities as q;
use crate::tx::with_retry_dup;

use super::service::observation_rid;

use crate::embeddings::provider;

use super::extract::app_settings_row;

pub const DEFAULT_MISSION: &str = "Observations are stable facts about people and relationships: preferences, skills, roles, \
recurring patterns, and how they change over time. Ignore ephemeral or one-off details.";

#[derive(Debug, Deserialize, SurrealValue)]
struct SubjectRow {
    vault: RecordId,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct RawMemoryRow {
    id: RecordId,
    text: String,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct ObservationRow {
    id: RecordId,
    #[serde(default)]
    #[surreal(default)]
    text: String,
    #[serde(default)]
    #[surreal(default)]
    source_memories: Option<Vec<RecordId>>,
    #[serde(default)]
    #[surreal(default)]
    version: i64,
    #[serde(default)]
    #[surreal(default)]
    status: Option<String>,
}

/// The result of a successful consolidation -- just enough to let callers
/// (the `consolidate_observations` tool) report what happened, as an `Option<..>`.
#[derive(Debug, Clone)]
pub struct ConsolidatedObservation {
    pub id: String,
    pub text: String,
}

fn build_prompt(mission: &str, current_belief: Option<&str>, facts: &[String]) -> String {
    let facts_block = facts.iter().map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n");
    format!(
        "{mission}\n\n\
You maintain a single evolving belief statement about one subject, built from raw facts observed about them over time. \
Given the current belief (if any) and a batch of new raw facts, produce an updated belief statement that synthesizes \
all of it -- evolving the belief where facts have changed rather than just appending, and preserving genuinely \
still-true parts of the old belief. Return strict JSON, no prose, with this exact shape:\n\n\
{{\"belief\": str}}\n\n\
Current belief: {}\n\n\
New raw facts:\n{facts_block}\n",
        current_belief.unwrap_or("(none yet)")
    )
}

/// (facts to send, belief to extend, lineage to store)
type Plan<'a> = (Vec<&'a (String, String)>, Option<&'a str>, Vec<String>);

/// What to consolidate given the subject's current raw facts `raw` (id,
/// text) and its observation (`None` if there isn't one): `(facts to send,
/// current belief to extend, lineage to store)`, or `None` when nothing
/// changed. A stale observation is rebuilt from every surviving fact, without
/// its old belief; a fresh one is extended with just the facts not yet in
/// its lineage. Lineage only ever names facts that still exist.
fn plan<'a>(
    raw: &'a [(String, String)],
    existing: Option<(&'a str, &HashSet<String>, bool)>,
) -> Option<Plan<'a>> {
    let all_ids: Vec<String> = raw.iter().map(|(id, _)| id.clone()).collect();
    match existing {
        Some((belief, seen, false)) => {
            let fresh: Vec<_> = raw.iter().filter(|(id, _)| !seen.contains(id)).collect();
            (!fresh.is_empty()).then_some((fresh, Some(belief), all_ids))
        }
        _ if raw.is_empty() => None,
        _ => Some((raw.iter().collect(), None, all_ids)),
    }
}

async fn call_llm(
    db: &OrgDb,
    settings: &Settings,
    owner: &RecordId,
    mission: &str,
    current_belief: Option<&str>,
    facts: &[String],
) -> AppResult<String> {
    let p = provider::resolve(db, settings, owner).await?;

    let body = json!({
        "model": provider::chat_model(&p).await,
        "response_format": {"type": "json_object"},
        "messages": [{"role": "user", "content": build_prompt(mission, current_belief, facts)}],
    });

    let resp = p
        .client()
        .await?
        .post(p.url("chat/completions"))
        .bearer_auth(p.bearer())
        .json(&body)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| AppError::internal(e.to_string()))?
        .error_for_status()
        .map_err(|e| AppError::internal(e.to_string()))?;

    let v: Value = resp.json().await.map_err(|e| AppError::internal(e.to_string()))?;
    let content = v
        .pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .ok_or_else(|| AppError::internal("OpenAI response missing choices[0].message.content"))?;
    let parsed: Value =
        serde_json::from_str(content).map_err(|e| AppError::internal(format!("invalid consolidation JSON: {e}")))?;
    parsed
        .get("belief")
        .and_then(|b| b.as_str())
        .map(str::to_string)
        .ok_or_else(|| AppError::internal("consolidation response missing 'belief'"))
}

/// The configured observations mission for `owner`, or `DEFAULT_MISSION` if
/// unset, read from
/// `app_settings.observations_mission`.
pub async fn observations_mission(db: &OrgDb, owner: &RecordId) -> AppResult<String> {
    let row = app_settings_row(db, owner).await?;
    Ok(if row.observations_mission.is_empty() { DEFAULT_MISSION.to_string() } else { row.observations_mission })
}

/// Consolidate one entity's raw facts into its observation row.
///
/// Returns the updated/created observation, or `None` if there was nothing
/// to do (no new raw facts beyond what's already consolidated) or the LLM
/// call failed / isn't available (stub backend).
#[allow(clippy::result_large_err)] // surrealdb::Error is large; boxing it would change the error type
pub async fn consolidate_subject(
    db: &OrgDb,
    settings: &Settings,
    owner: &RecordId,
    subject_id: &RecordId,
    mission: Option<&str>,
) -> AppResult<Option<ConsolidatedObservation>> {
    if super::service::require_entity_id(subject_id).is_err() {
        return Ok(None);
    }
    let subject_row: Option<SubjectRow> = store::get(db, subject_id).await?;
    let Some(subject_row) = subject_row else { return Ok(None) };
    // Not a member: same as not found. The facts must never reach the caller's model, nor the
    // observation land in someone else's vault.
    match crate::authz::authorize(db, owner, crate::authz::Action::WriteMemories, &subject_row.vault).await {
        Ok(_) => {}
        Err(e) if e.code == crate::error::ErrorCode::VaultForbidden => return Ok(None),
        Err(e) => return Err(e),
    }

    let mut raw_res = q::RAW_MEMORIES
        .on(db)
        .bind(("id", subject_id.clone()))
        .await?;
    let raw_rows: Vec<RawMemoryRow> = raw_res.take(0)?;

    let mut existing_res = q::OBSERVATION_OF
        .on(db)
        .bind(("id", subject_id.clone()))
        .await?;
    let existing_rows: Vec<ObservationRow> = existing_res.take(0)?;
    let existing = existing_rows.into_iter().next();

    let seen: HashSet<String> = existing
        .as_ref()
        .and_then(|e| e.source_memories.as_ref())
        .map(|ids| ids.iter().map(|r| r.to_string()).collect())
        .unwrap_or_default();
    // Facts new since the last consolidation may make earlier ones untrue:
    // those drop out, and the belief is rebuilt without them.
    let new_ids: Vec<RecordId> = raw_rows.iter().filter(|r| !seen.contains(&r.id.to_string())).map(|r| r.id.clone()).collect();
    let superseded: HashSet<String> =
        super::supersede::check(db, settings, owner, subject_id, &new_ids).await.iter().map(|r| r.to_string()).collect();
    let raw: Vec<(String, String)> = raw_rows
        .iter()
        .filter(|r| !superseded.contains(&r.id.to_string()))
        .map(|r| (r.id.to_string(), r.text.clone()))
        .collect();
    let existing_plan =
        existing.as_ref().map(|e| (e.text.as_str(), &seen, e.status.as_deref() == Some("stale") || !superseded.is_empty()));
    let Some((facts, current_belief, lineage)) = plan(&raw, existing_plan) else { return Ok(None) };

    if settings.embeddings_backend == "stub" {
        return Ok(None);
    }

    let mission = mission.map(str::to_string).unwrap_or(DEFAULT_MISSION.to_string());
    let fact_texts: Vec<String> = facts.iter().map(|(_, text)| text.clone()).collect();

    let belief = match call_llm(db, settings, owner, &mission, current_belief, &fact_texts).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("consolidation LLM call failed for {}: {}", subject_id.to_string(), e.message);
            return Ok(None);
        }
    };

    let source_memories: Vec<RecordId> = lineage
        .iter()
        .map(|s| crate::rid::parse(s))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| AppError::internal("source memory id did not round-trip"))?;
    let proof_count = lineage.len() as i64;

    // The LLM call above is too slow to hold a transaction open, so the write
    // is optimistic: it only lands if the observation is still the one we read
    // (same version, or still absent). Otherwise someone else revised it
    // meanwhile; skip, and the next consolidation recomputes from the new state.
    let stmt = if existing.is_none() { &q::CONSOLIDATE_CREATE } else { &q::CONSOLIDATE_UPDATE };
    let row = with_retry_dup(|| async {
        let mut res = stmt
            .on(db)
            .bind(("owner", owner.clone()))
            .bind(("vault", subject_row.vault.clone()))
            .bind(("subject", subject_id.clone()))
            .bind(("obs_id", observation_rid(subject_id)))
            .bind(("id", existing.as_ref().map(|e| e.id.clone())))
            .bind(("expected_version", existing.as_ref().map(|e| e.version)))
            .bind(("text", belief.clone()))
            .bind(("proof_count", proof_count))
            .bind(("source_memories", source_memories.clone()))
            .await?
            .check()?;
        let rows: Vec<ObservationRow> = res.take(stmt.slot)?;
        Ok(rows.into_iter().next())
    })
    .await?;
    let Some(row) = row else { return Ok(None) };
    let op = if existing.is_none() { crate::changes::CREATED } else { crate::changes::UPDATED };
    crate::changes::record(db, &subject_row.vault, op, crate::changes::MEMORY, &row.id).await;

    Ok(Some(ConsolidatedObservation { id: row.id.to_string(), text: row.text }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(ids: &[&str]) -> Vec<(String, String)> {
        ids.iter().map(|i| (format!("memory:{i}"), format!("fact {i}"))).collect()
    }

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|i| format!("memory:{i}")).collect()
    }

    #[test]
    fn plan_without_an_observation_builds_from_every_fact() {
        let r = raw(&["a", "b"]);
        let (facts, belief, lineage) = plan(&r, None).unwrap();
        assert_eq!(facts.len(), 2);
        assert_eq!(belief, None);
        assert_eq!(lineage, vec!["memory:a", "memory:b"]);
        assert!(plan(&[], None).is_none());
    }

    #[test]
    fn plan_extends_a_fresh_observation_with_only_new_facts() {
        let seen = set(&["a"]);
        let r = raw(&["a", "b"]);
        let (facts, belief, lineage) = plan(&r, Some(("old", &seen, false))).unwrap();
        assert_eq!(facts.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(), vec!["memory:b"]);
        assert_eq!(belief, Some("old"));
        assert_eq!(lineage, vec!["memory:a", "memory:b"]);
        assert!(plan(&raw(&["a"]), Some(("old", &seen, false))).is_none());
    }

    #[test]
    fn plan_rebuilds_a_stale_observation_from_surviving_facts_without_the_old_belief() {
        // "a" was edited (same id, so nothing looks new) and "x" was deleted
        let seen = set(&["a", "x"]);
        let r = raw(&["a"]);
        let (facts, belief, lineage) = plan(&r, Some(("lives in Paris", &seen, true))).unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(belief, None);
        assert_eq!(lineage, vec!["memory:a"]);
        assert!(plan(&[], Some(("old", &seen, true))).is_none());
    }

    #[test]
    fn build_prompt_includes_mission_belief_and_facts() {
        let facts = vec!["likes tea".to_string(), "works remote".to_string()];
        let prompt = build_prompt(DEFAULT_MISSION, Some("Alice drinks coffee."), &facts);
        assert!(prompt.contains(DEFAULT_MISSION));
        assert!(prompt.contains("Alice drinks coffee."));
        assert!(prompt.contains("- likes tea"));
        assert!(prompt.contains("- works remote"));
        assert!(prompt.contains("\"belief\""));
    }

    #[test]
    fn build_prompt_defaults_missing_belief_to_none_yet() {
        let prompt = build_prompt(DEFAULT_MISSION, None, &[]);
        assert!(prompt.contains("(none yet)"));
    }

    #[test]
    fn default_mission_text() {
        assert!(DEFAULT_MISSION.contains("stable facts about people and relationships"));
    }
}
