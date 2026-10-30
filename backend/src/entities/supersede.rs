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

/// Most facts one check sends to the model (the new ones plus their nearest live facts).
const MAX_FACTS: usize = 40;
/// Live facts compared with each new fact, nearest first.
const NEIGHBOURS: usize = 8;
/// Cosine similarity an older fact needs with a new one to be a candidate at all. Below it the two
/// are about different things, and one can't make the other untrue. (On Sam's vault, every
/// genuinely superseded fact sat at 0.70+ with the fact that replaced it.)
const MIN_SIMILARITY: f64 = 0.6;

#[derive(Debug, Deserialize, SurrealValue)]
struct FactRow {
    id: RecordId,
    text: String,
    at: Datetime,
    #[serde(default)]
    #[surreal(default)]
    sim: f64,
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
For each NEW fact, does it show that an OLDER fact is no longer true -- the same status, role, plan, number, date, \
location, access or relationship, now changed? A fact is superseded only when both cannot be true at once. These are \
NOT superseded: a restatement or duplicate, a fact the new one adds detail to, a fact about a different aspect of the \
subject, a preference or principle that still holds, and anything merely old. When unsure, list nothing. Return strict \
JSON, no prose: {{\"superseded\": [{{\"fact\": <older fact number>, \"by\": <new fact number>}}]}}\n\nFacts:\n{list}\n"
    )
}

/// The model's `{fact, by}` pairs -> ids of facts to mark. A pair counts only when `by` is a new fact,
/// `fact` is an older, not-new one, and neither is out of range; anything else is ignored.
fn pick(pairs: &[Value], facts: &[(String, &str, bool)], ids: &[RecordId]) -> Vec<RecordId> {
    let mut seen = HashSet::new();
    pairs
        .iter()
        .filter_map(|p| {
            let fact = (p.get("fact")?.as_u64()? as usize).checked_sub(1)?;
            let by = (p.get("by")?.as_u64()? as usize).checked_sub(1)?;
            let (f, b) = (facts.get(fact)?, facts.get(by)?);
            (fact != by && b.2 && !f.2 && f.0 <= b.0).then(|| ids.get(fact).cloned()).flatten()
        })
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
    let mut res = q::FACTS_BY_ID.on(db).bind(("ids", new.to_vec())).await?;
    let new_rows: Vec<FactRow> = res.take(0)?;
    if new_rows.is_empty() {
        return Ok(Vec::new());
    }
    let new_ids: HashSet<String> = new_rows.iter().map(|r| r.id.to_string()).collect();
    // Only live facts close in meaning to a new fact are candidates: one fact can't make an unrelated one untrue.
    let mut older: Vec<FactRow> = Vec::new();
    for n in &new_rows {
        let mut res = q::SIMILAR_LIVE_FACTS
            .on(db)
            .bind(("new", n.id.clone()))
            .bind(("subject", subject.clone()))
            .bind(("limit", NEIGHBOURS as i64))
            .await?;
        let near: Vec<FactRow> = res.take(q::SIMILAR_LIVE_FACTS.slot)?;
        for r in near {
            if r.sim >= MIN_SIMILARITY && !new_ids.contains(&r.id.to_string()) && !older.iter().any(|o| o.id == r.id) {
                older.push(r);
            }
        }
    }
    if older.is_empty() {
        return Ok(Vec::new());
    }
    older.sort_by(|a, b| b.sim.total_cmp(&a.sim));
    older.truncate(MAX_FACTS.saturating_sub(new_rows.len()));
    let mut rows: Vec<(String, FactRow, bool)> = new_rows
        .into_iter()
        .map(|r| (false, r))
        .chain(older.into_iter().map(|r| (true, r)))
        .map(|(old, r)| {
            let date = crate::sources::base::datetime_to_chrono(&r.at).map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
            (date, r, !old)
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let facts: Vec<(String, &str, bool)> = rows.iter().map(|(d, r, n)| (d.clone(), r.text.as_str(), *n)).collect();
    let ids: Vec<RecordId> = rows.iter().map(|(_, r, _)| r.id.clone()).collect();
    let marked = pick(&call_llm(db, settings, owner, build_prompt(&facts)).await?, &facts, &ids);
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

    fn facts() -> Vec<(String, &'static str, bool)> {
        vec![
            ("2026-09-01 10:00".into(), "X has no access", false),
            ("2026-09-02 10:00".into(), "X prefers tea", false),
            ("2026-10-05 09:00".into(), "X was granted access", true),
        ]
    }

    #[test]
    fn prompt_numbers_and_marks_new_facts() {
        let f = facts();
        let p = build_prompt(&f);
        assert!(p.contains("[1] 2026-09-01 10:00: X has no access"));
        assert!(p.contains("[3] 2026-10-05 09:00 (new): X was granted access"));
        assert!(p.contains("\"by\""));
    }

    #[test]
    fn pick_only_older_facts_replaced_by_a_new_one() {
        let ids: Vec<RecordId> = ["memory:a", "memory:b", "memory:c"].iter().map(|s| crate::rid::parse(s).unwrap()).collect();
        let f = facts();
        let got = pick(
            &[
                json!({"fact": 1, "by": 3}), // valid
                json!({"fact": 1, "by": 3}), // repeat
                json!({"fact": 3, "by": 1}), // a new fact can't be superseded, nor an old one supersede
                json!({"fact": 2, "by": 1}), // `by` is not new
                json!({"fact": 9, "by": 3}),
                json!({"fact": 0, "by": 3}),
                json!(1),                    // old flat-number format is ignored
                json!({"fact": "2", "by": 3}),
            ],
            &f,
            &ids,
        );
        assert_eq!(got, vec![ids[0].clone()]);
    }

    #[test]
    fn pick_rejects_a_replacement_dated_before_the_fact() {
        let ids: Vec<RecordId> = ["memory:a", "memory:b"].iter().map(|s| crate::rid::parse(s).unwrap()).collect();
        let f = vec![("2026-10-09 10:00".to_string(), "later old fact", false), ("2026-10-01 10:00".to_string(), "earlier new fact", true)];
        assert!(pick(&[json!({"fact": 1, "by": 2})], &f, &ids).is_empty());
    }
}
