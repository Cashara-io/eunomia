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
    /// The record's source key (`gmail`, ...): Gmail messages get their own prompt.
    pub source: String,
    /// The record's payload (for Gmail: `from`, `to`, `label_ids`).
    pub payload: Value,
    /// The vault owner's display name, when known; the email prompt says "the vault owner" otherwise.
    pub owner_name: Option<String>,
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

/// The prompt for one Gmail message. Same JSON shape as [`build_prompt`], but far stricter about what
/// is worth remembering: most email yields nothing. `owner` is the vault owner's name, if known.
pub(crate) fn build_email_prompt(owner: Option<&str>, date: Option<&str>, labels: &[String], subject: &str, text: &str) -> String {
    let known_name = owner.map(str::trim).filter(|n| !n.is_empty());
    let known = known_name.is_some();
    let owner = known_name.unwrap_or("the mailbox owner");
    let unknown_owner = if known {
        ""
    } else {
        "If you don't know the owner's name, attach owner facts to the other person or organisation involved, or skip them; never create an entity for 'the mailbox owner'.\n"
    };
    let date_line = date.unwrap_or("unknown");
    let labels_line = if labels.is_empty() { "none".to_string() } else { labels.join(", ") };
    format!(
        "You extract durable facts from one email for {owner}'s personal memory. Return strict JSON, no prose, with this exact shape:\n\n\
{{\n  \"people\": [{{\"name\": str, \"aliases\": [str], \"facts\": [str]}}],\n  \"organisations\": [{{\"name\": str, \"aliases\": [str], \"facts\": [str]}}],\n  \"locations\": [{{\"name\": str, \"aliases\": [str], \"facts\": [str]}}],\n  \"relations\": [{{\"from\": str, \"from_kind\": \"person|organisation|location\", \"to\": str, \"to_kind\": \"person|organisation|location\", \"label\": str}}]\n}}\n\n\
Email date: {date_line}\nGmail labels: {labels_line}\nSubject: {subject}\n\n\
Facts attach to the entity they are about. Most facts are about {owner}{owner_entry}. \
{unknown_owner}Return NOTHING (empty arrays) for most emails; that is correct and expected.\n\n\
SAVE at most 3 facts per email, usually 0 or 1:\n\
- deadlines, due dates, closing times, exam or demo dates that apply to {owner}\n\
- decisions or approvals about {owner}\n\
- things {owner} was asked to do or agreed to do, and by whom\n\
- meetings, interviews or events {owner} is invited to, with the date\n\
- a person's role relative to {owner}, only when that person wrote to {owner} personally or {owner} wrote to them\n\
- an account or service {owner} signed up for (one fact)\n\
- \"action required\" or overdue notices addressed to {owner} or a role they hold, with what is due\n\
- receipts for applications or requests {owner} made (extension, reimbursement, job), with their status\n\
- the role of a staff member or teammate in a thread {owner} is part of (sender, recipient or cc), once per person\n\n\
NEVER SAVE:\n\
- marketing or product claims, newsletter content\n\
- organisation boilerplate or policies\n\
- who emailed or thanked whom\n\
- signature details, quoted earlier replies\n\
- survey or research blurbs\n\
- cohort-wide announcements with no date or action for {owner}\n\
- notifications that only point at a post elsewhere (Ed, GitHub, Moodle)\n\n\
Bulk or automated mail (Promotions or Updates labels, no-reply senders, mailing lists) gets at most 1 fact, usually none.\n\
Each fact is one self-contained sentence that names its subject. Time-sensitive facts start with \"On {date_line},\" or give the absolute date.\n\
Relations only with a label from: {labels}, and only when the email states it outright.\n\n\
Email:\n{text}\n",
        labels = super::email::EMAIL_RELATION_LABELS.join(", "),
        owner_entry = if known { format!(": put them under a person entry named \"{owner}\"") } else { String::new() },
    )
}

/// Whether a record is a Gmail message, and so gets the email prompt and the email caps.
fn is_email(record: &ExtractRecord) -> bool {
    record.source == "gmail"
}

/// Applies the email limits in code: at most [`email::MAX_EMAIL_FACTS`] facts across all entities (in
/// order) and only the allowed relation labels.
fn cap_email(data: &mut ExtractionData) {
    let mut left = super::email::MAX_EMAIL_FACTS;
    for items in [&mut data.people, &mut data.organisations, &mut data.locations] {
        for item in items.iter_mut() {
            item.facts.retain(|f| !f.trim().is_empty());
            item.facts.truncate(left);
            left -= item.facts.len();
        }
    }
    data.relations.retain(|r| {
        r.label.as_deref().is_some_and(|l| super::email::EMAIL_RELATION_LABELS.contains(&l.trim().to_lowercase().as_str()))
    });
    // an entity with no facts is only worth keeping when a kept relation names it
    let named: HashSet<String> = data
        .relations
        .iter()
        .flat_map(|r| [r.from.as_deref(), r.to.as_deref()])
        .flatten()
        .map(|n| n.trim().to_lowercase())
        .collect();
    for items in [&mut data.people, &mut data.organisations, &mut data.locations] {
        items.retain(|e| !e.facts.is_empty() || e.name.as_deref().is_some_and(|n| named.contains(&n.trim().to_lowercase())));
    }
}

/// A speaker label rather than a name ("Unknown", "Speaker 2", "SPEAKER_01").
fn is_placeholder(name: &str) -> bool {
    let n = name.trim().to_lowercase().replace('_', " ");
    let rest = n.strip_prefix("speaker").map(str::trim);
    matches!(n.as_str(), "the mailbox owner" | "the vault owner" | "mailbox owner" | "vault owner")
        || matches!(n.as_str(), "unknown" | "unknown speaker" | "speaker" | "narrator" | "me" | "you")
        || rest.is_some_and(|r| r.len() <= 3 && r.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// Whether extraction should even attempt an LLM call -- short bodies and
/// the stub backend both skip straight through. Split out as a pure
/// function so the gate is unit-testable without a live network call.
fn should_extract(body: &str, embeddings_backend: &str) -> bool {
    body.trim().len() >= MIN_BODY_LEN && embeddings_backend != "stub"
}

#[derive(Debug, Deserialize, SurrealValue, Default)]
pub(crate) struct AppSettingsRow {
    #[serde(default)]
    #[surreal(default)]
    pub observations_mission: String,
    #[serde(default)]
    #[surreal(default)]
    pub owner_name: String,
}

/// The `app_settings:<owner_id>` row, creating it with (schema-)defaults if
/// missing. Used by `consolidate.rs`'s mission lookup.
pub(crate) async fn app_settings_row(db: &OrgDb, owner: &RecordId) -> AppResult<AppSettingsRow> {
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

async fn call_llm(db: &OrgDb, settings: &Settings, owner: &RecordId, prompt: String) -> AppResult<ExtractionData> {
    let p = provider::resolve(db, settings, owner).await?;

    let body = json!({
        "model": provider::chat_model(&p).await,
        "response_format": {"type": "json_object"},
        "messages": [{"role": "user", "content": prompt}],
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
///
/// Failures are logged. For email they are also returned, so the job retries (a re-extract has already
/// deleted the old facts); for other sources a failed model call just yields nothing.
pub async fn extract_entities(
    db: &OrgDb,
    settings: &Settings,
    owner: &RecordId,
    record: &ExtractRecord,
) -> Result<HashSet<String>, AppError> {
    let email = is_email(record);
    let cleaned;
    let body = if email {
        cleaned = super::email::strip_quoted(record.body_text.trim());
        cleaned.trim()
    } else {
        record.body_text.trim()
    };
    if !should_extract(body, &settings.embeddings_backend) {
        return Ok(HashSet::new());
    }

    let title = record.title.trim();
    let text = if title.is_empty() { body.to_string() } else { format!("{title}\n{body}") };

    let date = record.occurred_at.as_deref().map(|d| d.get(..10).unwrap_or(d));
    let prompt = if email {
        build_email_prompt(record.owner_name.as_deref(), date, &super::email::labels_of(&record.payload), title, body)
    } else {
        build_prompt(&text, date)
    };
    let mut data = match call_llm(db, settings, owner, prompt).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("entity extraction LLM call failed for {}: {}", record.id, e.message);
            return if email { Err(e) } else { Ok(HashSet::new()) };
        }
    };
    if email {
        cap_email(&mut data);
    }

    match apply_extraction(db, owner, &record.id, record.vault.as_ref(), &data).await {
        Ok(touched) => Ok(touched),
        Err(e) => {
            tracing::warn!("entity extraction upsert failed for {}: {}", record.id, e.message);
            if email { Err(e) } else { Ok(HashSet::new()) }
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

    fn email_record(source: &str) -> ExtractRecord {
        ExtractRecord {
            id: "gmail:gmail.message:1".into(),
            title: "s".into(),
            body_text: "b".into(),
            occurred_at: None,
            vault: None,
            source: source.into(),
            payload: Value::Null,
            owner_name: None,
        }
    }

    #[test]
    fn only_gmail_records_use_the_email_prompt() {
        assert!(is_email(&email_record("gmail")));
        assert!(!is_email(&email_record("github")));
        assert!(!is_email(&email_record("")));
    }

    #[test]
    fn email_prompt_has_header_rules_and_shape() {
        let labels = vec!["CATEGORY_UPDATES".to_string(), "SENT".to_string()];
        let p = build_email_prompt(Some("Sam Roberts"), Some("2026-03-04"), &labels, "Exam on Friday", "From: a@x.org\nbody");
        assert!(p.contains("Email date: 2026-03-04"));
        assert!(p.contains("Gmail labels: CATEGORY_UPDATES, SENT"));
        assert!(p.contains("Subject: Exam on Friday"));
        assert!(p.contains("SAVE at most 3 facts") && p.contains("NEVER SAVE"));
        assert!(p.contains("Sam Roberts") && !p.contains("mailbox owner") && !p.contains("don't know the owner"));
        assert!(p.contains("works_at, studies_at, teaches, manages, member_of, located_in"));
        assert!(p.contains("\"people\"") && p.contains("\"relations\""));
        assert!(p.ends_with("From: a@x.org\nbody\n"));
        let anon = build_email_prompt(None, None, &[], "s", "t");
        assert!(anon.contains("the mailbox owner") && anon.contains("never create an entity for 'the mailbox owner'") && anon.contains("Gmail labels: none"));
    }

    #[test]
    fn email_caps_facts_and_relation_labels() {
        let ent = |n: &str, facts: &[&str]| EntityExtract { name: Some(n.into()), facts: facts.iter().map(|f| f.to_string()).collect(), ..Default::default() };
        let rel = |l: &str| RelationExtract {
            from: Some("A".into()),
            from_kind: Some("person".into()),
            to: Some("B".into()),
            to_kind: Some("organisation".into()),
            label: Some(l.into()),
        };
        let mut data = ExtractionData {
            people: vec![ent("Sam", &["a", "", "b"]), ent("Ada", &["c", "d"])],
            organisations: vec![ent("Acme", &["e"])],
            locations: vec![],
            relations: vec![rel("works_at"), rel("Studies_At"), rel("thanked"), RelationExtract::default()],
        };
        cap_email(&mut data);
        let total: usize = data.people.iter().chain(&data.organisations).map(|e| e.facts.len()).sum();
        assert_eq!(total, 3);
        assert_eq!(data.people[0].facts, vec!["a", "b"]);
        assert_eq!(data.people[1].facts, vec!["c"]);
        assert!(data.organisations.is_empty(), "Acme has no facts and its relation was dropped");
        let labels: Vec<_> = data.relations.iter().filter_map(|r| r.label.as_deref()).collect();
        assert_eq!(labels, vec!["works_at", "Studies_At"]);
    }

    #[test]
    fn cap_email_drops_factless_entities_unless_a_kept_relation_names_them() {
        let ent = |n: &str, f: &[&str]| EntityExtract {
            name: Some(n.into()),
            aliases: vec![],
            facts: f.iter().map(|s| s.to_string()).collect(),
        };
        let mut data = ExtractionData {
            people: vec![ent("Sam", &["owes a form"]), ent("Noise", &[]), ent("Ada", &[])],
            organisations: vec![ent("Acme", &[]), ent("Other", &[])],
            locations: vec![],
            relations: vec![RelationExtract {
                from: Some(" ada ".into()),
                from_kind: Some("person".into()),
                to: Some("Acme".into()),
                to_kind: Some("organisation".into()),
                label: Some("works_at".into()),
            }],
        };
        cap_email(&mut data);
        let names = |v: &[EntityExtract]| v.iter().filter_map(|e| e.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&data.people), vec!["Sam", "Ada"]);
        assert_eq!(names(&data.organisations), vec!["Acme"]);
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
        for p in ["the mailbox owner", "The Vault Owner", "mailbox owner", "vault owner"] {
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
