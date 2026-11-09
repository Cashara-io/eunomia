//! The ingest pipeline: raw source record -> cache row -> enrichment.
//!
//! One entrypoint, [`ingest`], shared by scheduled syncs, manual syncs and webhooks (via
//! `sources::registry::ingest`). Per record (one or more envelopes): map -> upsert (idempotent; reconciles `links`) ->
//! embed -> queue entity extraction.
//!
//! Partial failure is isolated: a record that can't be mapped or stored is counted in `failed`
//! (the caller then keeps its sync cursor so the record is retried) and the batch continues.
//! Enrichment is best-effort and never counts as a failed record:
//! - Embedding runs when an embedding backend is available. A record left without an embedding
//!   (provider outage) is re-embedded the next time the same unchanged record is replayed, and the
//!   `embed` job reconciler finds it too.
//! - Entity extraction and consolidation run as background jobs (`jobs::handlers`): this pipeline
//!   only enqueues an `extract` job per new or changed record; the handler does nothing when the
//!   owner has no chat model, so with no model raw records are stored and nothing calls out.

use serde::Serialize;
use serde_json::Value;
use surrealdb::types::RecordId;

use crate::cache::search::{self, Envelope};
use crate::config::Settings;
use crate::pool::OrgDb;
use crate::error::AppResult;
use crate::store;

#[derive(Debug, Clone, Serialize)]
pub struct IngestReport {
    pub source: String,
    pub written: u64,
    pub skipped: u64,
    pub failed: u64,
    pub errors: Vec<String>,
}

impl IngestReport {
    fn new(source: &str) -> Self {
        IngestReport { source: source.to_string(), written: 0, skipped: 0, failed: 0, errors: Vec::new() }
    }

    /// Truncates `errors`
    /// to the first 20 for the response payload.
    pub fn as_dict(&self) -> Value {
        serde_json::json!({
            "source": self.source,
            "written": self.written,
            "skipped": self.skipped,
            "failed": self.failed,
            "errors": self.errors.iter().take(20).collect::<Vec<_>>(),
        })
    }
}

async fn embed_record(db: &OrgDb, settings: &Settings, owner: &RecordId, rec: &search::CacheRecord) -> AppResult<()> {
    let text = format!("{}\n{}", rec.title, rec.body_text).trim().to_string();
    if text.is_empty() {
        return Ok(());
    }
    let vec = crate::embeddings::service::embed(db, settings, &[text], Some(owner))
        .await?
        .into_iter()
        .next()
        .unwrap_or_default();
    search::set_embedding(db, owner, &rec.id, vec).await
}

async fn has_embedding(db: &OrgDb, owner: &RecordId, record_id: &str) -> AppResult<bool> {
    let mut res = store::cache::HAS_EMBEDDING.on(db).bind(("id", search::rid(owner, record_id))).await?;
    Ok(res.take::<Option<bool>>(0)?.unwrap_or(false))
}

/// `raw_records` are untyped JSON values; `map_fn` maps one raw record to its envelopes (none to skip
/// it), or `Err(message)` on a mapping failure.
pub async fn ingest(
    state: &crate::state::OrgState,
    owner: &RecordId,
    source_key: &str,
    raw_records: &[Value],
    map_fn: impl Fn(&Value) -> Result<Vec<Envelope>, String>,
) -> AppResult<IngestReport> {
    let (db, settings) = (&state.db, &state.settings);
    let mut report = IngestReport::new(source_key);
    // Composio holds a local commit gate here. Its missing vectors are filled by the
    // existing embed reconciler, so a provider call cannot delay disconnect.
    let can_embed = source_key != "composio" && crate::embeddings::service::available(db, settings, owner).await;

    for raw in raw_records {
        let envs = match map_fn(raw) {
            Ok(envs) if envs.is_empty() => {
                report.skipped += 1;
                continue;
            }
            Ok(envs) => envs,
            Err(e) => {
                report.failed += 1;
                report.errors.push(format!("{}: {e}", raw_id(raw)));
                continue;
            }
        };
        for mut env in envs {
            if env.source.is_empty() {
                env.source = source_key.to_string();
            }
            let (rec, changed) = match search::upsert(db, owner, &env).await {
                Ok(r) => r,
                Err(e) => {
                    report.failed += 1;
                    report.errors.push(format!("{}: {}", env.id, e.message));
                    continue;
                }
            };
            if changed {
                report.written += 1;
            } else {
                report.skipped += 1;
            }
            if rec.deleted {
                continue;
            }

            // An unchanged record is re-embedded only if a previous attempt failed.
            if can_embed
                && (changed || !has_embedding(db, owner, &rec.id).await.unwrap_or(true))
                && let Err(e) = embed_record(db, settings, owner, &rec).await
            {
                report.errors.push(format!("embed {}: {}", rec.id, e.message));
            }
            if changed {
                crate::jobs::handlers::enqueue_extract(state, owner, &rec.id, &rec.content_hash).await;
            }
        }
    }

    Ok(report)
}

fn raw_id(raw: &Value) -> String {
    raw.get("id").and_then(|v| v.as_str()).map(str::to_string).unwrap_or_else(|| "?".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_dict_truncates_errors_to_20() {
        let mut report = IngestReport::new("demo");
        for i in 0..30 {
            report.errors.push(format!("err {i}"));
        }
        let d = report.as_dict();
        assert_eq!(d["errors"].as_array().unwrap().len(), 20);
    }

    #[test]
    fn as_dict_reports_counters() {
        let mut report = IngestReport::new("demo");
        report.written = 3;
        report.skipped = 1;
        report.failed = 2;
        let d = report.as_dict();
        assert_eq!(d["source"], "demo");
        assert_eq!(d["written"], 3);
        assert_eq!(d["skipped"], 1);
        assert_eq!(d["failed"], 2);
    }

    #[test]
    fn raw_id_falls_back_to_question_mark() {
        assert_eq!(raw_id(&serde_json::json!({"no_id": true})), "?");
        assert_eq!(raw_id(&serde_json::json!({"id": "abc"})), "abc");
    }
}
