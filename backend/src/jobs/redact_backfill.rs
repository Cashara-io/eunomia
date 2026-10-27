//! The one-off secret backfill. Writes are redacted ([`crate::redact`]), but rows written before that
//! shipped still hold plaintext secrets. This walks an org's memories (facts, observations and superseded facts
//! alike), skills and audit summaries in id order, a batch at a time, runs the same detector and writes
//! back only the rows it changed. Progress lives in `redact_backfill:v<DETECTOR_VERSION>`: a crashed run
//! resumes at its cursor, a finished one is never repeated, and a new detector version runs it again.
//!
//! Not covered (not redacted on write either): uploaded documents, synced connector records, chat
//! messages, entity names and summaries, extension webhook deliveries.

use serde::Deserialize;
use serde_json::Value;
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use crate::audit::{self, Event};
use crate::authz::Actor;
use crate::error::AppResult;
use crate::redact::{self, DETECTOR_VERSION};
use crate::rid::RecordIdExt;
use crate::state::OrgState;
use crate::store::redact as q;
use crate::tx::with_retry;

/// Rows read per batch. Each rewrite is its own statement: no long transaction, no table lock.
const BATCH: i64 = 200;

/// The tables walked, in order.
const STAGES: &[&str] = &["memory", "skill", "audit_log", "done"];

/// Where one org's backfill stands (the `redact_backfill` row).
#[derive(Debug, Clone, Deserialize, SurrealValue)]
pub struct Progress {
    pub stage: String,
    pub cursor: Option<RecordId>,
    pub changed_memory: i64,
    pub changed_skill: i64,
    pub changed_audit_log: i64,
    pub done_at: Option<Datetime>,
}

fn marker_id() -> RecordId {
    RecordId::from_table_key("redact_backfill", format!("v{DETECTOR_VERSION}"))
}

async fn load(state: &OrgState) -> AppResult<Option<Progress>> {
    Ok(q::MARKER.on(&state.db).bind(("id", marker_id())).await?.check()?.take(0)?)
}

/// Whether this org has been backfilled for the current detectors. Errors on a database still at the
/// schema before the marker table.
pub async fn done(state: &OrgState) -> AppResult<bool> {
    Ok(load(state).await?.is_some_and(|p| p.done_at.is_some()))
}

async fn save(state: &OrgState, p: &Progress) -> AppResult<()> {
    q::SAVE_MARKER
        .on(&state.db)
        .bind(("id", marker_id()))
        .bind(("stage", p.stage.clone()))
        .bind(("cursor", p.cursor.clone()))
        .bind(("changed_memory", p.changed_memory))
        .bind(("changed_skill", p.changed_skill))
        .bind(("changed_audit_log", p.changed_audit_log))
        .bind(("done_at", p.done_at))
        .await?
        .check()?;
    Ok(())
}

/// Runs the backfill from where it stopped, at most `max_batches` batches (`None`: to the end).
pub async fn run(state: &OrgState, max_batches: Option<usize>) -> AppResult<Progress> {
    let mut p = load(state).await?.unwrap_or(Progress {
        stage: STAGES[0].into(),
        cursor: None,
        changed_memory: 0,
        changed_skill: 0,
        changed_audit_log: 0,
        done_at: None,
    });
    let mut batches = 0;
    while p.done_at.is_none() && max_batches.is_none_or(|m| batches < m) {
        let (last, changed) = match p.stage.as_str() {
            "memory" => memory_batch(state, &p.cursor).await?,
            "skill" => skill_batch(state, &p.cursor).await?,
            "audit_log" => audit_batch(state, &p.cursor).await?,
            _ => (None, 0),
        };
        match p.stage.as_str() {
            "memory" => p.changed_memory += changed,
            "skill" => p.changed_skill += changed,
            _ => p.changed_audit_log += changed,
        }
        p.cursor = last;
        if p.cursor.is_none() {
            let next = STAGES.iter().position(|s| *s == p.stage).map_or(STAGES.len() - 1, |i| (i + 1).min(STAGES.len() - 1));
            p.stage = STAGES[next].into();
            if p.stage == "done" {
                p.done_at = Some(Datetime::from(chrono::Utc::now()));
            }
        }
        save(state, &p).await?;
        batches += 1;
        if p.done_at.is_some() {
            report(state, &p).await;
        }
    }
    Ok(p)
}

/// One log line and one `audit_event` per org when it finishes: counts only, never text.
async fn report(state: &OrgState, p: &Progress) {
    let org = state.db.org().key();
    tracing::info!(
        org = %org,
        version = DETECTOR_VERSION,
        memory = p.changed_memory,
        skill = p.changed_skill,
        audit_log = p.changed_audit_log,
        "secret backfill finished"
    );
    let detail = format!("version={DETECTOR_VERSION} memory={} skill={} audit_log={}", p.changed_memory, p.changed_skill, p.changed_audit_log);
    let actor = Actor { kind: "system", id: "system".into() };
    audit::record(&state.control, Event { user: None, actor: &actor, action: "redact.backfill", target: &org, outcome: "ok", detail: &detail }).await;
}

/// `true` when the compare-and-set wrote the row (it still held the text that was read).
fn wrote(mut res: surrealdb::IndexedResults) -> AppResult<bool> {
    Ok(!res.take::<Vec<RecordId>>(0)?.is_empty())
}

/// Best effort: a stale memo only costs a vector nobody can look up without the old text.
async fn forget_vectors(state: &OrgState, owner: &RecordId, texts: Vec<String>) {
    if let Err(e) = crate::embeddings::service::forget(&state.db, &state.settings, &texts, owner).await {
        tracing::warn!(error = %e.message, "secret backfill: could not drop cached vectors");
    }
}

/// (last id read, rows changed); no last id once the table is exhausted.
type Batch = (Option<RecordId>, i64);

async fn memory_batch(state: &OrgState, cursor: &Option<RecordId>) -> AppResult<Batch> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        id: RecordId,
        owner: RecordId,
        text: String,
    }
    let rows: Vec<Row> = q::MEMORY_BATCH.on(&state.db).bind(("cursor", cursor.clone())).bind(("limit", BATCH)).await?.check()?.take(0)?;
    let mut changed = 0;
    for row in &rows {
        let text = redact::redact(&row.text, &mut Vec::new());
        if text == row.text {
            continue;
        }
        let set = || async { q::SET_MEMORY_TEXT.on(&state.db).bind(("id", row.id.clone())).bind(("text", text.clone())).bind(("old", row.text.clone())).await?.check() };
        let res = with_retry(set).await?;
        if wrote(res)? {
            changed += 1;
            forget_vectors(state, &row.owner, vec![row.text.clone()]).await;
        }
    }
    Ok((rows.last().map(|r| r.id.clone()), changed))
}

async fn skill_batch(state: &OrgState, cursor: &Option<RecordId>) -> AppResult<Batch> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        id: RecordId,
        owner: RecordId,
        name: String,
        description: String,
        body: String,
    }
    let rows: Vec<Row> = q::SKILL_BATCH.on(&state.db).bind(("cursor", cursor.clone())).bind(("limit", BATCH)).await?.check()?.take(0)?;
    let mut changed = 0;
    for row in &rows {
        let description = redact::redact(&row.description, &mut Vec::new());
        let body = redact::redact(&row.body, &mut Vec::new());
        if description == row.description && body == row.body {
            continue;
        }
        let new_description = description != row.description;
        let res = with_retry(|| async {
            q::SET_SKILL_TEXT
                .on(&state.db)
                .bind(("id", row.id.clone()))
                .bind(("description", description.clone()))
                .bind(("body", body.clone()))
                .bind(("old_description", row.description.clone()))
                .bind(("old_body", row.body.clone()))
                .await?
                .check()
        })
        .await?;
        if !wrote(res)? {
            continue;
        }
        changed += 1;
        // only name + description are embedded: a body-only change keeps its vector
        if new_description {
            forget_vectors(state, &row.owner, vec![format!("{}\n{}", row.name, row.description)]).await;
            crate::tools::registry::skills::reembed(state, &row.owner, &row.id).await?;
        }
    }
    Ok((rows.last().map(|r| r.id.clone()), changed))
}

async fn audit_batch(state: &OrgState, cursor: &Option<RecordId>) -> AppResult<Batch> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        id: RecordId,
        args_summary: String,
    }
    let rows: Vec<Row> = q::AUDIT_BATCH.on(&state.db).bind(("cursor", cursor.clone())).bind(("limit", BATCH)).await?.check()?.take(0)?;
    let mut changed = 0;
    for row in &rows {
        let summary = redact_summary(&row.args_summary);
        if summary == row.args_summary {
            continue;
        }
        let set = || async { q::SET_AUDIT_SUMMARY.on(&state.db).bind(("id", row.id.clone())).bind(("summary", summary.clone())).bind(("old", row.args_summary.clone())).await?.check() };
        let res = with_retry(set).await?;
        if wrote(res)? {
            changed += 1;
        }
    }
    Ok((rows.last().map(|r| r.id.clone()), changed))
}

/// An audit summary is serialized JSON, where a newline is the two letters `\n` and hides the start of a
/// secret from the detector's word boundary. Parsed, each string is redacted as on write. A summary cut at
/// its length cap no longer parses: its `\n`, `\t` and `\r` escapes are undone for the detector and put
/// back after (both ways are exact, since serialized JSON holds no raw control characters).
fn redact_summary(summary: &str) -> String {
    if let Ok(mut v) = serde_json::from_str::<Value>(summary) {
        let before = v.clone();
        redact::redact_value(&mut v);
        return if v == before { summary.to_string() } else { v.to_string() };
    }
    let plain = summary.replace("\\n", "\n").replace("\\t", "\t").replace("\\r", "\r");
    let mut found = Vec::new();
    let out = redact::redact(&plain, &mut found);
    if found.is_empty() {
        return summary.to_string();
    }
    out.replace('\n', "\\n").replace('\t', "\\t").replace('\r', "\\r")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_are_redacted_whole_or_cut() {
        let token = format!("ghp_{}", "a1B2c3D4e5".repeat(4));
        let whole = serde_json::json!({ "text": format!("line1\n{token} and then more words"), "subject_name": "Alice" }).to_string();
        let out = redact_summary(&whole);
        assert!(!out.contains(&token) && out.contains("[redacted: github token]"), "{out}");
        assert!(out.contains("Alice"));

        // cut mid-string, as summarize_args does past 500 characters
        let cut = format!("{}\u{2026}", &whole[..whole.len() - 12]);
        let out = redact_summary(&cut);
        assert!(!out.contains(&token) && out.contains("[redacted: github token]"), "{out}");
        assert!(out.starts_with(r#"{"text":"line1\n[redacted: github token] and"#), "escapes put back: {out}");

        let plain = r#"{"text":"no secrets\nhere \\n either"}"#;
        assert_eq!(redact_summary(plain), plain);
        assert_eq!(redact_summary(&plain[..20]), &plain[..20]);
    }
}
