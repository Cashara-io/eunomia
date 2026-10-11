//! Superseded facts: a raw fact a later fact shows is no longer true ("X has
//! no access to Y", then "X was granted access to Y") is marked
//! `status = "superseded"`. Recall leaves it out; `entities_get` still shows
//! it, marked. Checked by the chat model whenever new facts arrive -- in a
//! `supersede` job queued by `memory_write`, and in consolidation (which runs after every sync).
//! Best-effort like the rest of the enrichment: a failure marks nothing.

use std::collections::HashSet;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use crate::config::Settings;
use crate::embeddings::provider;
use crate::error::{AppError, AppResult};
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::store::entities as q;

/// How many of the subject's latest facts the new ones are compared with.
const MAX_FACTS: usize = 60;

#[derive(Debug, Deserialize, SurrealValue)]
struct FactRow {
    id: RecordId,
    text: String,
    at: Datetime,
}

/// `(date, text, is_new)`, oldest first -> the prompt.
fn build_prompt(facts: &[(String, &str, bool)]) -> String {
    let list = facts
        .iter()
        .enumerate()
        .map(|(i, (date, text, new))| format!("[{}] {date}{}: {text}", i + 1, if *new { " (new)" } else { "" }))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Below are facts recorded about one subject, oldest first, with the date each was recorded. Some are new. \
Which facts does a LATER fact show are no longer true -- a status, role, plan, location, access or relationship that \
changed? Only consider facts contradicted or replaced by a later one where at least one of the two is new. Never list a \
fact merely for being old, and never list one that is still compatible with the later facts. Return strict JSON, no \
prose: {{\"superseded\": [number]}}\n\nFacts:\n{list}\n"
    )
}

/// The facts (by 1-based number) the model said are superseded -> their ids.
fn pick(numbers: &[Value], ids: &[RecordId]) -> Vec<RecordId> {
    let mut seen = HashSet::new();
    numbers
        .iter()
        .filter_map(|n| n.as_u64())
        .filter_map(|n| ids.get((n as usize).checked_sub(1)?).cloned())
        .filter(|id| seen.insert(id.to_string()))
        .collect()
}

async fn call_llm(db: &OrgDb, settings: &Settings, owner: &RecordId, prompt: String) -> AppResult<Vec<Value>> {
    let p = provider::resolve(db, settings, owner).await?;
    let body = json!({
        "model": provider::chat_model(&p).await,
        "response_format": {"type": "json_object"},
        "messages": [{"role": "user", "content": prompt}],
    });
    let v: Value = p
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
        .map_err(|e| AppError::internal(e.to_string()))?
        .json()
        .await
        .map_err(|e| AppError::internal(e.to_string()))?;
    let content = v.pointer("/choices/0/message/content").and_then(|c| c.as_str()).unwrap_or("{}");
    let parsed: Value = serde_json::from_str(content).unwrap_or_default();
    Ok(parsed.get("superseded").and_then(|s| s.as_array()).cloned().unwrap_or_default())
}

/// Compares `subject`'s `new` facts with its other live facts, marks the ones
/// a later fact supersedes, and (if any) makes the subject's observation
/// stale. Returns the ids marked. No-op without a chat model.
pub async fn check(db: &OrgDb, settings: &Settings, owner: &RecordId, subject: &RecordId, new: &[RecordId]) -> Vec<RecordId> {
    if new.is_empty() || !enabled(db, settings, owner).await {
        return Vec::new();
    }
    match run(db, settings, owner, subject, new).await {
        Ok(marked) => marked,
        Err(e) => {
            tracing::warn!("supersede check failed for {}: {}", subject.to_string(), e.message);
            Vec::new()
        }
    }
}

/// Whether there is a chat model to run the check with (else it is a no-op, and not worth a job).
pub async fn enabled(db: &OrgDb, settings: &Settings, owner: &RecordId) -> bool {
    settings.embeddings_backend != "stub" && crate::embeddings::service::chat_available(db, settings, owner).await
}

async fn run(db: &OrgDb, settings: &Settings, owner: &RecordId, subject: &RecordId, new: &[RecordId]) -> AppResult<Vec<RecordId>> {
    let mut res = q::LIVE_FACTS
        .on(db)
        .bind(("subject", subject.clone()))
        .bind(("limit", MAX_FACTS as i64))
        .await?;
    let mut rows: Vec<FactRow> = res.take(0)?;
    if rows.len() < 2 {
        return Ok(Vec::new());
    }
    rows.reverse();
    let new: HashSet<String> = new.iter().map(|r| r.to_string()).collect();
    let facts: Vec<(String, &str, bool)> = rows
        .iter()
        .map(|r| {
            let date = crate::sources::base::datetime_to_chrono(&r.at).map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
            (date, r.text.as_str(), new.contains(&r.id.to_string()))
        })
        .collect();
    if !facts.iter().any(|f| f.2) {
        return Ok(Vec::new());
    }
    let ids: Vec<RecordId> = rows.iter().map(|r| r.id.clone()).collect();
    let marked = pick(&call_llm(db, settings, owner, build_prompt(&facts)).await?, &ids);
    if !marked.is_empty() {
        q::MARK_SUPERSEDED
            .on(db)
            .bind(("ids", marked.clone()))
        .bind(("subject", subject.clone()))
        .await?
        .check()?;
    }
    Ok(marked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_numbers_and_marks_new_facts() {
        let p = build_prompt(&[("2026-09-01 10:00".into(), "X has no access", false), ("2026-10-05 09:00".into(), "X was granted access", true)]);
        assert!(p.contains("[1] 2026-09-01 10:00: X has no access"));
        assert!(p.contains("[2] 2026-10-05 09:00 (new): X was granted access"));
        assert!(p.contains("\"superseded\""));
    }

    #[test]
    fn pick_maps_numbers_and_ignores_junk() {
        let ids: Vec<RecordId> = ["memory:a", "memory:b"].iter().map(|s| crate::rid::parse(s).unwrap()).collect();
        let got = pick(&[json!(1), json!(1), json!(0), json!(9), json!("2"), json!(-1)], &ids);
        assert_eq!(got, vec![ids[0].clone()]);
    }
}
