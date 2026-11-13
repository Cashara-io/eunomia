//! Superseded facts: a raw fact a later fact shows is no longer true ("X has
//! no access to Y", then "X was granted access to Y") is marked
//! `status = "superseded"`. Recall leaves it out; `entities_get` still shows
//! it, marked. Checked by the chat model whenever new facts arrive -- in a
//! `supersede` job queued by `memory_write`, and in consolidation (which runs after every sync).
//! Best-effort like the rest of the enrichment: a failure marks nothing.
//!
//! Facts are placed by when we were last told them (`asserted_at`, see docs/concepts.md "A fact's two
//! dates"): an extracted fact by its record's date (never later than its write), a fact a person or
//! agent wrote by its write, an edited fact by its edit; a fact from before that field by its last
//! write. When a fact happened (`occurred_at`) never decides it: it is shown to the model, so a fact
//! about the past can be told apart from a correction. Only a fact told later can make one told
//! earlier untrue, whichever was written first: an old email imported today never replaces a newer
//! fact, while a correction made today wins whatever date it says it is about, and a fact from next
//! month's calendar event (told when it was read) does not outrank it. A newly written fact can
//! itself be found already out of date by a live fact told after it, so a history backfilled newest
//! first ends as one written oldest first. Two facts told at the same moment go to the later write.
//! A replaced fact records the fact that replaced it (`superseded_by`).
//!
//! Limit: a fact written directly about the past ("worked at Acme in 2019") is told today. Only the
//! model, which sees its `occurred_at`, keeps it from replacing a current fact.

use std::collections::HashSet;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use crate::config::Settings;
use crate::embeddings::{provider, service as embeddings};
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
    /// When we were last told it (`asserted_at`), else its last write.
    at: Datetime,
    /// Its last write: the tie-break between facts told at the same moment.
    #[serde(default)]
    #[surreal(default)]
    written: Option<Datetime>,
    /// When it happened, if known: shown to the model, never used for ordering.
    #[serde(default)]
    #[surreal(default)]
    occurred_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    sim: f64,
}

/// A fact's place in time: `(told, written)`, compared in that order.
type When = (Option<chrono::DateTime<chrono::Utc>>, Option<chrono::DateTime<chrono::Utc>>);

fn when(r: &FactRow) -> When {
    let c = crate::sources::base::datetime_to_chrono;
    (c(&r.at), r.written.as_ref().and_then(c))
}

/// `(when, fact, is_new)` oldest first: by when each was told, then when each was written, then an older
/// fact before a new one (the prompt says "oldest first").
fn oldest_first<T>(rows: &mut [(When, T, bool)]) {
    rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)));
}

/// `(date told, text, is_new)`, oldest first -> the prompt. A text may end with "(happened <date>)".
fn build_prompt(facts: &[(String, String, bool)]) -> String {
    let list = facts
        .iter()
        .enumerate()
        .map(|(i, (date, text, new))| format!("[{}] {date}{}: {text}", i + 1, if *new { " (new)" } else { "" }))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Below are facts recorded about one subject, in the order we were told them, oldest first, each with the date \
we were told it. Some also say when the fact happened: a fact about the past can be history (true then, still true as \
history) rather than a correction. Some are new. For each NEW fact, does it show that an OLDER fact is no longer true \
-- the same status, role, plan, number, date, location, access or relationship, now changed? And does a LATER fact show \
that the NEW fact itself is no longer true? A fact is superseded only when both cannot be true at once, and only by a \
fact listed after it. These are NOT superseded: a restatement or duplicate, a fact the other adds detail to, a fact \
about a different aspect of the subject, a record of what was true at an earlier time, a preference or principle that \
still holds, and anything merely old. When unsure, list nothing. Return strict JSON, no prose: {{\"superseded\": \
[{{\"fact\": <earlier fact number>, \"by\": <later fact number>}}]}}\n\nFacts:\n{list}\n"
    )
}

/// The model's `{fact, by}` pairs -> `(fact to mark, fact that replaced it)`. `facts` is oldest first. A
/// pair counts only when `fact` is listed (and told) before `by`, exactly one of the two is new, and
/// neither is out of range; anything else is ignored. So a fact told earlier never replaces one told
/// later, whatever order they were written in. (Two new facts are not compared with each other here.)
fn pick(pairs: &[Value], facts: &[(String, String, bool)], ids: &[RecordId]) -> Vec<(RecordId, RecordId)> {
    let mut seen = HashSet::new();
    pairs
        .iter()
        .filter_map(|p| {
            let fact = (p.get("fact")?.as_u64()? as usize).checked_sub(1)?;
            let by = (p.get("by")?.as_u64()? as usize).checked_sub(1)?;
            let (f, b) = (facts.get(fact)?, facts.get(by)?);
            (fact < by && b.2 != f.2 && f.0 <= b.0).then(|| Some((ids.get(fact)?.clone(), ids.get(by)?.clone()))).flatten()
        })
        .filter(|(id, _)| seen.insert(id.to_string()))
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
    // The check runs right after the write, beside that write's own background work (its embed_soon),
    // so a commit conflict is likely and says nothing about the facts: run it again. It is safe to
    // repeat (embedding and marking are idempotent).
    let mut attempt = 1;
    loop {
        match run(db, settings, owner, subject, new).await {
            Ok(marked) => return marked,
            Err(e) if e.code == crate::error::ErrorCode::DbConflict && attempt < CHECK_ATTEMPTS => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(50 * attempt as u64)).await;
            }
            Err(e) => {
                tracing::warn!("supersede check failed for {}: {}", subject.to_string(), e.message);
                return Vec::new();
            }
        }
    }
}

/// Whether there is a chat model to run the check with (else it is a no-op, and not worth a job).
pub async fn enabled(db: &OrgDb, settings: &Settings, owner: &RecordId) -> bool {
    settings.embeddings_backend != "stub" && crate::embeddings::service::chat_available(db, settings, owner).await
}

/// Runs of one check when a commit conflict interrupts it.
const CHECK_ATTEMPTS: u32 = 3;

/// Most older facts embedded on the spot for one check.
const EMBED_MISSING: i64 = 30;

/// Embeds the subject's live facts that have no vector yet. Their embed job runs after the write, so a fact
/// that a new one contradicts can still be unembedded, and would otherwise never be a candidate.
async fn embed_missing(db: &OrgDb, settings: &Settings, owner: &RecordId, subject: &RecordId, new: &[RecordId], spec: &str) -> AppResult<()> {
    let mut res = q::UNEMBEDDED_LIVE_FACTS
        .on(db)
        .bind(("subject", subject.clone()))
        .bind(("new", new.to_vec()))
        .bind(("spec", spec.to_string()))
        .bind(("limit", EMBED_MISSING))
        .await?;
    let missing: Vec<FactRow> = res.take(q::UNEMBEDDED_LIVE_FACTS.slot)?;
    if missing.is_empty() {
        return Ok(());
    }
    let texts: Vec<String> = missing.iter().map(|r| r.text.clone()).collect();
    let vectors = embeddings::embed(db, settings, &texts, Some(owner)).await?;
    for (r, vector) in missing.iter().zip(vectors) {
        if vector.len() != embeddings::dim() {
            continue;
        }
        embeddings::set_memory_embedding(db, &r.id, vector, spec, &r.text).await?;
    }
    Ok(())
}

async fn run(db: &OrgDb, settings: &Settings, owner: &RecordId, subject: &RecordId, new: &[RecordId]) -> AppResult<Vec<RecordId>> {
    let mut res = q::FACTS_BY_ID.on(db).bind(("ids", new.to_vec())).await?;
    let new_rows: Vec<FactRow> = res.take(0)?;
    if new_rows.is_empty() {
        return Ok(Vec::new());
    }
    let new_ids: HashSet<String> = new_rows.iter().map(|r| r.id.to_string()).collect();
    // New facts usually have no vector yet (the embed job runs later), so embed them here. The vector
    // is saved too, so recall finds the fact by meaning straight away.
    let spec = embeddings::spec_for(db, settings, owner).await?.tag();
    embed_missing(db, settings, owner, subject, new, &spec).await?;
    let texts: Vec<String> = new_rows.iter().map(|r| r.text.clone()).collect();
    let vectors = embeddings::embed(db, settings, &texts, Some(owner)).await?;
    // Only live facts close in meaning to a new fact are candidates: one fact can't make an unrelated one untrue.
    let mut older: Vec<FactRow> = Vec::new();
    for (n, vector) in new_rows.iter().zip(vectors) {
        if vector.len() != embeddings::dim() {
            return Err(AppError::internal(format!("embedding for {} has dimension {}", n.id.to_string(), vector.len())));
        }
        embeddings::set_memory_embedding(db, &n.id, vector.clone(), &spec, &n.text).await?;
        let mut res = q::SIMILAR_LIVE_FACTS
            .on(db)
            .bind(("new", n.id.clone()))
            .bind(("vector", vector))
            .bind(("spec", spec.clone()))
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
    let mut rows: Vec<(When, FactRow, bool)> = new_rows
        .into_iter()
        .map(|r| (false, r))
        .chain(older.into_iter().map(|r| (true, r)))
        .map(|(old, r)| (when(&r), r, !old))
        .collect();
    oldest_first(&mut rows);
    let date = |w: &When| w.0.map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
    let text = |r: &FactRow| match r.occurred_at.as_ref().and_then(crate::sources::base::datetime_to_chrono) {
        Some(d) => format!("{} (happened {})", r.text, d.format("%Y-%m-%d")),
        None => r.text.clone(),
    };
    let facts: Vec<(String, String, bool)> = rows.iter().map(|(w, r, n)| (date(w), text(r), *n)).collect();
    let ids: Vec<RecordId> = rows.iter().map(|(_, r, _)| r.id.clone()).collect();
    let marked = pick(&call_llm(db, settings, owner, build_prompt(&facts)).await?, &facts, &ids);
    if !marked.is_empty() {
        // the subject's other writers (consolidation, a new fact's embed) can touch these rows too
        let pairs: Vec<Vec<RecordId>> = marked.iter().map(|(f, b)| vec![f.clone(), b.clone()]).collect();
        let mark = || async { q::MARK_SUPERSEDED.on(db).bind(("pairs", pairs.clone())).bind(("subject", subject.clone())).await?.check() };
        crate::tx::with_retry(mark).await?;
    }
    Ok(marked.into_iter().map(|(f, _)| f).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(rows: &[(&str, &str, bool)]) -> Vec<(String, String, bool)> {
        rows.iter().map(|(d, t, n)| (d.to_string(), t.to_string(), *n)).collect()
    }

    fn facts() -> Vec<(String, String, bool)> {
        list(&[
            ("2026-09-01 10:00", "X has no access", false),
            ("2026-09-02 10:00", "X prefers tea", false),
            ("2026-10-05 09:00", "X was granted access (happened 2026-10-01)", true),
        ])
    }

    fn at(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
        Some(s.parse().unwrap())
    }

    #[test]
    fn facts_are_ordered_by_when_they_were_told_then_when_they_were_written() {
        let mut rows = vec![
            ((at("2026-10-05T09:00:00Z"), at("2026-10-05T09:00:30Z")), "granted", true),
            ((at("2026-10-05T09:00:00Z"), at("2026-10-05T09:00:10Z")), "no access", false),
            ((at("2026-10-05T08:59:00Z"), at("2026-10-05T08:59:00Z")), "earlier", false),
            // written last, but told in 2021 (an old email read today): it goes first
            ((at("2021-05-01T00:00:00Z"), at("2026-11-13T00:00:00Z")), "imported", true),
        ];
        oldest_first(&mut rows);
        assert_eq!(rows.iter().map(|r| r.1).collect::<Vec<_>>(), ["imported", "earlier", "no access", "granted"]);
        // the same moment and the same write time: the older fact first
        let mut tie = vec![((at("2026-10-05T09:00:00Z"), None), "new", true), ((at("2026-10-05T09:00:00Z"), None), "old", false)];
        oldest_first(&mut tie);
        assert_eq!(tie.iter().map(|r| r.1).collect::<Vec<_>>(), ["old", "new"]);
    }

    #[test]
    fn prompt_numbers_and_marks_new_facts() {
        let f = facts();
        let p = build_prompt(&f);
        assert!(p.contains("[1] 2026-09-01 10:00: X has no access"));
        assert!(p.contains("[3] 2026-10-05 09:00 (new): X was granted access (happened 2026-10-01)"));
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
        assert_eq!(got, vec![(ids[0].clone(), ids[2].clone())], "the replaced fact and the fact that replaced it");
    }

    #[test]
    fn pick_lets_a_later_told_live_fact_supersede_an_earlier_told_new_one() {
        let ids: Vec<RecordId> = ["memory:a", "memory:b", "memory:c"].iter().map(|s| crate::rid::parse(s).unwrap()).collect();
        // a 2023 fact arrives after a 2025 one (a backfill read newest first)
        let f = list(&[
            ("2023-04-01 10:00", "Ada is a junior engineer", true),
            ("2024-01-01 10:00", "Ada mentors two interns", true),
            ("2025-06-01 10:00", "Ada is a senior engineer", false),
        ]);
        assert_eq!(pick(&[json!({"fact": 1, "by": 3})], &f, &ids), vec![(ids[0].clone(), ids[2].clone())]);
        // the reverse pair would let the older-dated fact replace the newer one: refused
        assert!(pick(&[json!({"fact": 3, "by": 1})], &f, &ids).is_empty());
        // two new facts are not compared here
        assert!(pick(&[json!({"fact": 1, "by": 2})], &f, &ids).is_empty());
    }

    #[test]
    fn pick_rejects_a_replacement_dated_before_the_fact() {
        let ids: Vec<RecordId> = ["memory:a", "memory:b"].iter().map(|s| crate::rid::parse(s).unwrap()).collect();
        let f = list(&[("2026-10-09 10:00", "later old fact", false), ("2026-10-01 10:00", "earlier new fact", true)]);
        assert!(pick(&[json!({"fact": 1, "by": 2})], &f, &ids).is_empty());
    }
}
