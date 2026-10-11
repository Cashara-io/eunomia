//! LLM entity-extraction pass, intended to run as a later stage of cache
//! ingestion (this
//! module is self-contained and takes the ingested record's fields directly).
//!
//! Pulls person/organisation/location mentions, facts, and relations out of
//! a cache record's text via an OpenAI chat completion (JSON mode), then
//! upserts them through `entities::service`. Best-effort, enrichment only:
//! this must never fail the caller -- a failure here (bad LLM output,
//! network error, OpenAI outage) is logged and swallowed, same spirit as
//! "embedding failure is non-fatal" elsewhere in this codebase.
//!
//! Stub mode: there is no separate extraction-backend setting.
//! `settings.embeddings_backend == "stub"` is reused as the one signal for
//! "no real network calls in tests" -- when set, `extract_entities` is a
//! no-op (skips straight through, creates nothing).
//!
//! The OpenAI settings lookup (`resolve_openai` below) is inlined here rather than
//! shared with `connectors::service`.

use surrealdb::types::SurrealValue;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use surrealdb::types::RecordId;
use crate::rid::RecordIdExt;

use crate::config::Settings;
use crate::pool::OrgDb;
use crate::store;
use crate::embeddings::provider;
use crate::error::{AppError, AppResult};
use crate::store::entities as q;

use super::service;

/// Text shorter than this has nothing worth extracting -- skip the LLM call
/// entirely rather than spend tokens on "ok", "thanks", etc.
const MIN_BODY_LEN: usize = 40;

/// One ingested record's fields relevant to extraction -- mirrors the `id`/
/// `title`/`body_text` subset of a `cache_record` row.
#[derive(Debug, Clone)]
pub struct ExtractRecord {
    pub id: String,
    pub title: String,
    pub body_text: String,
    /// When the record happened (RFC 3339), so time-sensitive facts carry it.
    pub occurred_at: Option<String>,
    /// The vault the record is filed in (routing); `None`: the owner's personal vault. Its entities,
    /// facts and relations are written there.
    pub vault: Option<RecordId>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct EntityExtract {
    name: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    facts: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RelationExtract {
    from: Option<String>,
    from_kind: Option<String>,
    to: Option<String>,
    to_kind: Option<String>,
    label: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ExtractionData {
    #[serde(default)]
    people: Vec<EntityExtract>,
    #[serde(default)]
    organisations: Vec<EntityExtract>,
    #[serde(default)]
    locations: Vec<EntityExtract>,
    #[serde(default)]
    relations: Vec<RelationExtract>,
}

/// `(section key, memory/entity kind)` pairs.
const KIND_MAP: [(&str, &str); 3] =
    [("people", "person"), ("organisations", "organisation"), ("locations", "location")];

fn build_prompt(text: &str, date: Option<&str>) -> String {
    let dated = match date {
        Some(d) => format!("The text is dated {d}. Write time-sensitive facts (plans, statuses, who was where) with that date, e.g. \"On {d}, ...\".\n"),
        None => String::new(),
    };
    format!(
        "Extract entities: the people, organisations and locations the text below talks about -- in a conversation or meeting \
transcript, that is the speakers and everyone and everything they mention -- and how they relate. Return strict JSON, \
no prose, with this exact shape:\n\n\
{{\n  \"people\": [{{\"name\": str, \"aliases\": [str], \"facts\": [str]}}],\n  \"organisations\": [{{\"name\": str, \"aliases\": [str], \"facts\": [str]}}],\n  \"locations\": [{{\"name\": str, \"aliases\": [str], \"facts\": [str]}}],\n  \"relations\": [{{\"from\": str, \"from_kind\": \"person|organisation|location\", \"to\": str, \"to_kind\": \"person|organisation|location\", \"label\": str}}]\n}}\n\n\
Only include entities actually mentioned in the text. Do not create an entity for the document, message, meeting or \
recording itself, and skip placeholder names such as \"Unknown\" or \"Speaker 2\". Each fact is a self-contained \
sentence that names its subject. Omit a section/field entirely if nothing was found for it, rather than inventing filler.\n\
{dated}\n\
Text:\n{text}\n"
    )
}

/// A speaker label rather than a name ("Unknown", "Speaker 2", "SPEAKER_01").
fn is_placeholder(name: &str) -> bool {
    let n = name.trim().to_lowercase().replace('_', " ");
    let rest = n.strip_prefix("speaker").map(str::trim);
    matches!(n.as_str(), "unknown" | "unknown speaker" | "speaker" | "narrator" | "me" | "you")
        || rest.is_some_and(|r| r.len() <= 3 && r.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Whether extraction should even attempt an LLM call -- short bodies and
/// the stub backend both skip straight through. Split out as a pure
/// function so the gate is unit-testable without a live network call.
fn should_extract(body: &str, embeddings_backend: &str) -> bool {
    body.trim().len() >= MIN_BODY_LEN && embeddings_backend != "stub"
}

#[derive(Debug, Deserialize, SurrealValue, Default)]
pub(super) struct AppSettingsRow {
    #[serde(default)]
    #[surreal(default)]
    pub observations_mission: String,
}

/// The `app_settings:<owner_id>` row, creating it with (schema-)defaults if
/// missing. Used by `consolidate.rs`'s mission lookup.
pub(super) async fn app_settings_row(db: &OrgDb, owner: &RecordId) -> AppResult<AppSettingsRow> {
    let rid = RecordId::from_table_key("app_settings", owner.key().clone());
    let row: Option<AppSettingsRow> = store::get(db, &rid).await?;
    match row {
        Some(r) => Ok(r),
        None => {
            // Relies on the `app_settings` table's own field DEFAULTs (see
            // `db.rs`'s SCHEMA_STATEMENTS) for everything but `owner`.
            let mut res =
                q::UPSERT_APP_SETTINGS.on(db).bind(("id", rid)).bind(("owner", owner.clone())).await?;
            let rows: Vec<AppSettingsRow> = res.take(0)?;
            Ok(rows.into_iter().next().unwrap_or_default())
        }
    }
}

async fn call_llm(db: &OrgDb, settings: &Settings, owner: &RecordId, text: &str, date: Option<&str>) -> AppResult<ExtractionData> {
    let p = provider::resolve(db, settings, owner).await?;

    let body = json!({
        "model": provider::chat_model(&p).await,
        "response_format": {"type": "json_object"},
        "messages": [{"role": "user", "content": build_prompt(text, date)}],
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
    serde_json::from_str(content).map_err(|e| AppError::internal(format!("invalid extraction JSON: {e}")))
}

/// Applies the parsed extraction and returns the set of subject ids (as
/// strings) that got a new raw `memory` row -- used by the ingest pipeline
/// to know which entities need re-consolidation.
async fn apply_extraction(
    db: &OrgDb,
    owner: &RecordId,
    source_record_id: &str,
    vault: Option<&RecordId>,
    data: &ExtractionData,
) -> AppResult<HashSet<String>> {
    let mut entity_ids: HashMap<String, (RecordId, String)> = HashMap::new();
    let mut touched: HashSet<String> = HashSet::new();

    #[allow(clippy::too_many_arguments)]
    async fn ensure(
        db: &OrgDb,
        owner: &RecordId,
        vault: Option<&RecordId>,
        entity_ids: &mut HashMap<String, (RecordId, String)>,
        name: &str,
        kind: &str,
        aliases: Option<Vec<String>>,
    ) -> AppResult<(RecordId, String)> {
        let key = name.to_lowercase();
        if let Some(entry) = entity_ids.get(&key) {
            return Ok(entry.clone());
        }
        let entity = service::upsert_entity(db, owner, kind, name, aliases, vault).await?;
        let rid: RecordId = crate::rid::parse(&entity.id).map_err(|_| AppError::internal("entity id did not round-trip"))?;
        let entry = (rid, kind.to_string());
        entity_ids.insert(key, entry.clone());
        Ok(entry)
    }

    let sections: [(&str, &[EntityExtract]); 3] =
        [("people", &data.people), ("organisations", &data.organisations), ("locations", &data.locations)];
    for (section, items) in sections {
        let kind = KIND_MAP.iter().find(|(s, _)| *s == section).map(|(_, k)| *k).unwrap();
        for item in items {
            let Some(name) = item.name.as_deref().map(str::trim).filter(|n| !n.is_empty() && !is_placeholder(n)) else { continue };
            let (entry_id, _) = ensure(db, owner, vault, &mut entity_ids, name, kind, Some(item.aliases.clone())).await?;
            for fact in &item.facts {
                if fact.is_empty() {
                    continue;
                }
                service::add_memory(db, owner, &entry_id, fact, Some(source_record_id), "world").await?;
                touched.insert(entry_id.to_string());
            }
        }
    }

    for rel in &data.relations {
        let (Some(from_name), Some(to_name)) =
            (rel.from.as_deref().map(str::trim).filter(|n| !n.is_empty() && !is_placeholder(n)), rel.to.as_deref().map(str::trim).filter(|n| !n.is_empty() && !is_placeholder(n)))
        else {
            continue;
        };
        let (Some(from_kind), Some(to_kind)) = (rel.from_kind.as_deref(), rel.to_kind.as_deref()) else { continue };
        let valid_kinds = ["person", "organisation", "location"];
        if !valid_kinds.contains(&from_kind) || !valid_kinds.contains(&to_kind) {
            continue;
        }
        let (from_id, _) = ensure(db, owner, vault, &mut entity_ids, from_name, from_kind, None).await?;
        let (to_id, _) = ensure(db, owner, vault, &mut entity_ids, to_name, to_kind, None).await?;
        service::add_relation(db, owner, &from_id, &to_id, rel.label.as_deref().unwrap_or(""), Some(source_record_id))
            .await?;
    }

    Ok(touched)
}

/// Best-effort entity extraction + upsert for one ingested record. Never
/// fails the caller. Returns the set of subject ids (as strings) that got a
/// new raw memory (empty on no-op/failure) -- a future ingest pipeline would
/// use this to batch-trigger `consolidate.rs` per unique subject touched.
pub async fn extract_entities(db: &OrgDb, settings: &Settings, owner: &RecordId, record: &ExtractRecord) -> HashSet<String> {
    let body = record.body_text.trim();
    if !should_extract(body, &settings.embeddings_backend) {
        return HashSet::new();
    }

    let title = record.title.trim();
    let text = if title.is_empty() { body.to_string() } else { format!("{title}\n{body}") };

    let date = record.occurred_at.as_deref().map(|d| d.get(..10).unwrap_or(d));
    let data = match call_llm(db, settings, owner, &text, date).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("entity extraction LLM call failed for {}: {}", record.id, e.message);
            return HashSet::new();
        }
    };

    match apply_extraction(db, owner, &record.id, record.vault.as_ref(), &data).await {
        Ok(touched) => touched,
        Err(e) => {
            tracing::warn!("entity extraction upsert failed for {}: {}", record.id, e.message);
            HashSet::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_extract_skips_short_bodies() {
        assert!(!should_extract("hi", "openai"));
        assert!(!should_extract("", "openai"));
    }

    #[test]
    fn should_extract_skips_stub_backend_regardless_of_length() {
        let long_body = "x".repeat(100);
        assert!(!should_extract(&long_body, "stub"));
    }

    #[test]
    fn should_extract_true_for_long_body_and_real_backend() {
        let long_body = "x".repeat(100);
        assert!(should_extract(&long_body, "openai"));
    }

    #[test]
    fn should_extract_boundary_is_inclusive() {
        let exact = "x".repeat(MIN_BODY_LEN);
        assert!(should_extract(&exact, "openai"));
        let under = "x".repeat(MIN_BODY_LEN - 1);
        assert!(!should_extract(&under, "openai"));
    }

    #[test]
    fn build_prompt_embeds_text_and_shape() {
        let prompt = build_prompt("Alice works at Acme.", Some("2026-10-02"));
        assert!(prompt.contains("Alice works at Acme."));
        assert!(prompt.contains("On 2026-10-02, ..."));
        assert!(prompt.contains("Do not create an entity for the document, message, meeting"));
        assert!(!build_prompt("x", None).contains("dated"));
        assert!(prompt.contains("\"people\""));
        assert!(prompt.contains("\"relations\""));
    }

    #[test]
    fn extraction_data_parses_full_shape() {
        let json = r#"{
            "people": [{"name": "Alice", "aliases": ["Al"], "facts": ["likes tea"]}],
            "organisations": [{"name": "Acme"}],
            "relations": [{"from": "Alice", "from_kind": "person", "to": "Acme", "to_kind": "organisation", "label": "works_at"}]
        }"#;
        let data: ExtractionData = serde_json::from_str(json).unwrap();
        assert_eq!(data.people.len(), 1);
        assert_eq!(data.people[0].name.as_deref(), Some("Alice"));
        assert_eq!(data.people[0].aliases, vec!["Al".to_string()]);
        assert_eq!(data.organisations.len(), 1);
        assert_eq!(data.locations.len(), 0);
        assert_eq!(data.relations.len(), 1);
        assert_eq!(data.relations[0].label.as_deref(), Some("works_at"));
    }

    #[test]
    fn extraction_data_defaults_missing_sections_to_empty() {
        let data: ExtractionData = serde_json::from_str("{}").unwrap();
        assert!(data.people.is_empty());
        assert!(data.organisations.is_empty());
        assert!(data.locations.is_empty());
        assert!(data.relations.is_empty());
    }

    #[test]
    fn speaker_labels_are_not_people() {
        for p in ["Unknown", "Speaker 2", "SPEAKER_01", "speaker A", "unknown speaker"] {
            assert!(is_placeholder(p), "{p}");
        }
        for n in ["Ada Lovelace", "Speakerman Industries", "Unknown Pleasures Ltd"] {
            assert!(!is_placeholder(n), "{n}");
        }
    }

    #[test]
    fn kind_map_covers_all_three_sections() {
        assert_eq!(KIND_MAP.len(), 3);
        assert!(KIND_MAP.contains(&("people", "person")));
        assert!(KIND_MAP.contains(&("organisations", "organisation")));
        assert!(KIND_MAP.contains(&("locations", "location")));
    }
}
