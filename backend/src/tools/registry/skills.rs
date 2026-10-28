//! Agent skills: save, find, get, delete (`store/skills.rs`, issue #93). A skill is what an agent
//! learned, in SKILL.md style (name, description, markdown body), kept in a vault and embedded so
//! another agent finds it by meaning. Not the built-in memory skill (`docs.rs` DEFAULT_SKILL).
//! Part of the tool registry (see `registry/mod.rs`); one `register` call per tool.

use std::collections::HashMap;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Value};
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use super::{bad_args, register, to_tool_value, ToolSpec};
use crate::authz::{self, Action};
use crate::cache::recall::MIN_SIMILARITY;
use crate::embeddings::service as embeddings;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::state::OrgState;
use crate::store;
use crate::vaults::service::{accessible_vault_ids, default_vault_id};

const MAX_DESCRIPTION: usize = 1024;
const MAX_BODY: usize = 64 * 1024;
const DEFAULT_LIMIT: usize = 10;
const MAX_LIMIT: usize = 50;
/// Skills whose name and description the keyword match reads at most.
// ponytail: the keyword arm scans this many recent skills in Rust; a full-text index if a vault outgrows it.
const KEYWORD_SCAN: i64 = 1000;

#[derive(Deserialize, SurrealValue)]
struct Skill {
    id: RecordId,
    vault: RecordId,
    name: String,
    description: String,
    #[serde(default)]
    #[surreal(default)]
    body: String,
    updated_at: Datetime,
    /// Only the nearest-neighbour query selects it.
    #[serde(default)]
    #[surreal(default)]
    sim: f64,
}

impl Skill {
    /// The row without its body (what `skill_find` lists).
    fn summary(&self) -> Value {
        json!({
            "id": self.id.to_string(),
            "name": self.name,
            "description": self.description,
            "vault": self.vault.to_string(),
            "updated_at": self.updated_at,
        })
    }
}

/// SKILL.md style: lowercase letters, digits and hyphens, at most 64 characters, not starting with a hyphen.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 64 && chars.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn validate(name: &str, description: &str, body: &str) -> AppResult<()> {
    if !valid_name(name) {
        return Err(AppError::bad_request("name must be 1 to 64 characters: lowercase letters, digits and hyphens, starting with a letter or digit"));
    }
    if description.trim().is_empty() || description.chars().count() > MAX_DESCRIPTION {
        return Err(AppError::bad_request(format!("description must be 1 to {MAX_DESCRIPTION} characters")));
    }
    if body.trim().is_empty() || body.len() > MAX_BODY {
        return Err(AppError::bad_request(format!("body must be non-empty and at most {} KiB", MAX_BODY / 1024)));
    }
    Ok(())
}

fn parse_vault(v: &str) -> AppResult<RecordId> {
    crate::rid::parse(v).map_err(|_| AppError::bad_request(format!("invalid vault_id: {v:?}")))
}

/// The vaults a read covers: the named one (membership-checked), or every vault the caller can read.
async fn read_vaults(db: &OrgDb, owner: &RecordId, vault_id: Option<&str>) -> AppResult<Vec<RecordId>> {
    match vault_id {
        Some(v) => {
            let v = parse_vault(v)?;
            authz::authorize(db, owner, Action::ReadMemories, &v).await?;
            Ok(vec![v])
        }
        None => accessible_vault_ids(db, owner).await,
    }
}

/// Not a member of the vault, or a token restricted to another one: the skill reads as absent.
async fn accessible(db: &OrgDb, owner: &RecordId, vault: &RecordId, action: Action) -> AppResult<bool> {
    match authz::authorize(db, owner, action, vault).await {
        Ok(_) => Ok(true),
        Err(e) if matches!(e.code, ErrorCode::VaultForbidden | ErrorCode::AuthScope) => Ok(false),
        Err(e) => Err(e),
    }
}

fn skill_id(id: &str) -> AppResult<RecordId> {
    match crate::rid::parse(id) {
        Ok(rid) if rid.table() == "skill" => Ok(rid),
        _ => Err(AppError::bad_request(format!("{id:?} is not a skill id"))),
    }
}

/// The skill `id` if it exists and the caller may use its vault; absent otherwise, alike for both.
async fn load(state: &OrgState, owner: &RecordId, id: &str, action: Action) -> AppResult<Skill> {
    let rid = skill_id(id)?;
    let row: Option<Skill> = store::get(&state.db, &rid).await?;
    match row {
        Some(s) if accessible(&state.db, owner, &s.vault, action).await? => Ok(s),
        _ => Err(AppError::not_found(format!("no skill {id}"))),
    }
}

/// Embeds name + description under the owner's provider and stores the vector. `false`, with a warning,
/// when there is no provider or it fails: the skill is saved either way, found by keyword.
// A skill left without a vector (no key at save time, a failed call) is picked up by `embed_backlog`.
async fn embed_skill(state: &OrgState, owner: &RecordId, skill: &Skill) -> bool {
    if !embeddings::available(&state.db, &state.settings, owner).await {
        return false;
    }
    let run = async {
        let spec = embeddings::spec_for(&state.db, &state.settings, owner).await?.tag();
        let text = format!("{}\n{}", skill.name, skill.description);
        let vector = embeddings::embed(&state.db, &state.settings, &[text], Some(owner)).await?.into_iter().next().unwrap_or_default();
        if vector.len() != embeddings::dim() {
            return Err(AppError::internal(format!("embedding has dimension {}", vector.len())));
        }
        store::skills::SET_EMBEDDING
            .on(&state.db)
            .bind(("id", skill.id.clone()))
            .bind(("embedding", vector))
            .bind(("spec", spec))
            .bind(("vault", skill.vault.clone()))
            .bind(("name", skill.name.clone()))
            .bind(("description", skill.description.clone()))
            .await?
            .check()?;
        AppResult::Ok(())
    };
    match run.await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("skill_save: saved without a vector: {}", e.message);
            false
        }
    }
}

/// Embeds up to `limit` of `owner`'s skills that have no vector from the current model. Returns how many
/// it found; one that fails stays in the backlog for the next embed job.
pub(crate) async fn embed_backlog(state: &OrgState, owner: &RecordId, limit: usize) -> AppResult<usize> {
    let spec = embeddings::spec_for(&state.db, &state.settings, owner).await?.tag();
    let mut res = store::skills::EMBED_BACKLOG.on(&state.db).bind(("owner", owner.clone())).bind(("spec", spec)).bind(("limit", limit as i64)).await?;
    let ids: Vec<RecordId> = res.take(0)?;
    for id in &ids {
        let row: Option<Skill> = store::get(&state.db, id).await?;
        if let Some(skill) = row {
            embed_skill(state, owner, &skill).await;
        }
    }
    Ok(ids.len())
}

async fn save(state: &OrgState, owner: &RecordId, mut a: SaveArgs) -> AppResult<Value> {
    // the name is a lowercase slug, too narrow to hold a secret of a known shape
    let mut redacted = Vec::new();
    a.description = crate::redact::redact(&a.description, &mut redacted);
    a.body = crate::redact::redact(&a.body, &mut redacted);
    validate(&a.name, &a.description, &a.body)?;
    let vault = match a.vault_id.as_deref() {
        None => default_vault_id(&state.db, owner).await?,
        Some(v) => authz::authorize(&state.db, owner, Action::WriteMemories, &parse_vault(v)?).await?.vault().clone(),
    };
    let rows: Vec<Skill> = store::skills::UPSERT
        .on(&state.db)
        .bind(("owner", owner.clone()))
        .bind(("vault", vault))
        .bind(("name", a.name))
        .bind(("description", a.description))
        .bind(("body", a.body))
        .await?
        .take(0)?;
    let skill = rows.into_iter().next().ok_or_else(|| AppError::internal("skill upsert returned no row"))?;
    let embedded = embed_skill(state, owner, &skill).await;
    let mut out = json!({ "id": skill.id.to_string(), "name": skill.name, "vault": skill.vault.to_string(), "embedded": embedded });
    if !redacted.is_empty() {
        out["redacted"] = json!(redacted);
    }
    Ok(out)
}

/// Lowercase words of at least two characters.
fn terms(query: &str) -> Vec<String> {
    query.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 2).map(String::from).collect()
}

/// The `limit` skills of `vaults` nearest `vector` among those embedded under `spec`, best first, each at
/// least [`MIN_SIMILARITY`]. HNSW first, then the exact scan when the shared index underfills, as for memories
/// (`cache::recall::nearest_memories`).
async fn nearest_skills(db: &OrgDb, vaults: &[RecordId], vector: &[f32], spec: &str, limit: usize) -> AppResult<Vec<Skill>> {
    let ef = (limit * 2).max(64);
    let cols = "SELECT id, vault, name, description, updated_at, vector::similarity::cosine(embedding, $vec) AS sim FROM skill";
    // dynamic: the KNN `<|K,EF|>` operator needs literal integers.
    let knn = format!("{cols} WHERE vault IN $vaults AND embedding_spec = $spec AND embedding <|{limit},{ef}|> $vec ORDER BY sim DESC");
    let bind = |sql: String, name: &'static str| {
        store::dynamic(db, name, sql).bind(("vaults", vaults.to_vec())).bind(("spec", spec.to_string())).bind(("vec", vector.to_vec()))
    };
    let mut rows: Vec<Skill> = bind(knn, "skills.nearest").await?.take(0)?;
    if rows.len() < limit {
        let exact = format!("{cols} WHERE vault IN $vaults AND embedding_spec = $spec ORDER BY sim DESC LIMIT $limit");
        rows = bind(exact, "skills.nearest_exact").bind(("limit", limit as i64)).await?.take(0)?;
    }
    Ok(rows.into_iter().filter(|r| r.sim >= MIN_SIMILARITY).collect())
}

/// The query's vector under the owner's provider; `None` (keyword-only) without one or when it fails.
async fn query_vector(state: &OrgState, owner: &RecordId, query: &str) -> Option<(Vec<f32>, String)> {
    if !embeddings::available(&state.db, &state.settings, owner).await {
        return None;
    }
    let run = async {
        let spec = embeddings::spec_for(&state.db, &state.settings, owner).await?.tag();
        let vector = embeddings::embed(&state.db, &state.settings, &[query.to_string()], Some(owner)).await?.into_iter().next().unwrap_or_default();
        AppResult::Ok((vector.len() == embeddings::dim()).then_some((vector, spec)))
    };
    match run.await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("skill_find: keyword only: {}", e.message);
            None
        }
    }
}

async fn find(state: &OrgState, owner: &RecordId, a: FindArgs) -> AppResult<Value> {
    if a.query.trim().is_empty() {
        return Err(AppError::bad_request("query must not be empty"));
    }
    let limit = a.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let vaults = read_vaults(&state.db, owner, a.vault_id.as_deref()).await?;
    if vaults.is_empty() {
        return Ok(json!({ "skills": [] }));
    }
    let mut hits: Vec<Skill> = match query_vector(state, owner, &a.query).await {
        Some((vector, spec)) => nearest_skills(&state.db, &vaults, &vector, &spec, limit).await?,
        None => Vec::new(),
    };
    // keyword arm: skills whose name or description holds a query word, the most words first
    let words = terms(&a.query);
    let recent: Vec<Skill> = store::skills::LIST_META.on(&state.db).bind(("vaults", vaults)).bind(("limit", KEYWORD_SCAN)).await?.take(0)?;
    let mut keyword: Vec<(usize, Skill)> = recent
        .into_iter()
        .filter(|s| !hits.iter().any(|h| h.id == s.id))
        .filter_map(|s| {
            let text = format!("{} {}", s.name, s.description).to_lowercase();
            let n = words.iter().filter(|w| text.contains(w.as_str())).count();
            (n > 0).then_some((n, s))
        })
        .collect();
    keyword.sort_by_key(|(n, _)| std::cmp::Reverse(*n)); // stable: newest first among equals
    hits.extend(keyword.into_iter().map(|(_, s)| s));
    hits.truncate(limit);
    Ok(json!({ "skills": hits.iter().map(Skill::summary).collect::<Vec<_>>() }))
}

async fn get(state: &OrgState, owner: &RecordId, a: GetArgs) -> AppResult<Value> {
    let skill = match (a.id.as_deref(), a.name.as_deref()) {
        (Some(id), _) => load(state, owner, id, Action::ReadMemories).await?,
        (None, Some(name)) => {
            let vaults = read_vaults(&state.db, owner, a.vault_id.as_deref()).await?;
            let mut rows: Vec<Skill> = store::skills::BY_NAME.on(&state.db).bind(("vaults", vaults)).bind(("name", name.to_string())).await?.take(0)?;
            match rows.len() {
                0 => return Err(AppError::not_found(format!("no skill named {name:?}"))),
                1 => rows.remove(0),
                _ => {
                    let ids: Vec<String> = rows.iter().map(|s| s.id.to_string()).collect();
                    return Err(AppError::bad_request(format!("several skills are named {name:?}; pass one of these ids: {}", ids.join(", "))));
                }
            }
        }
        (None, None) => return Err(AppError::bad_request("pass an id, or a name")),
    };
    let mut out = skill.summary();
    out["body"] = json!(skill.body);
    Ok(out)
}

async fn delete(state: &OrgState, owner: &RecordId, id: &str) -> AppResult<Value> {
    let skill = load(state, owner, id, Action::WriteMemories).await?;
    store::skills::DELETE.on(&state.db).bind(("id", skill.id.clone())).await?.check()?;
    Ok(json!({ "deleted": skill.id.to_string() }))
}

#[derive(Deserialize)]
struct SaveArgs {
    name: String,
    description: String,
    body: String,
    #[serde(default)]
    vault_id: Option<String>,
}

#[derive(Deserialize)]
struct FindArgs {
    query: String,
    #[serde(default)]
    vault_id: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct GetArgs {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    vault_id: Option<String>,
}

#[derive(Deserialize)]
struct DeleteArgs {
    id: String,
}

pub(super) fn register_all(registry: &mut HashMap<&'static str, ToolSpec>) {
    register(
        registry,
        "skill_save",
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "pattern": "^[a-z0-9][a-z0-9-]{0,63}$", "description": "lowercase letters, digits and hyphens, e.g. deploy-rollback; saving the same name in a vault replaces the skill"},
                "description": {"type": "string", "description": "when to use the skill, in one or two sentences (at most 1024 characters); this and the name are what `skill_find` searches"},
                "body": {"type": "string", "description": "the skill itself, in Markdown (at most 64 KiB)"},
                "vault_id": {"type": "string", "description": "keep it in this vault (a vault id or its name, e.g. \"Acme\"); omit for your personal vault"},
            },
            "required": ["name", "description", "body"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                match serde_json::from_value::<SaveArgs>(args) {
                    Ok(a) => Ok(to_tool_value(save(state, owner, a).await)),
                    Err(e) => Ok(bad_args(e)),
                }
            })
        }),
    );

    register(
        registry,
        "skill_find",
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "what you need to do, in plain words"},
                "vault_id": {"type": "string", "description": "search only this vault (a vault id or its name); omit to search every vault you can read"},
                "limit": {"type": "integer", "description": "at most 50, default 10"},
            },
            "required": ["query"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                match serde_json::from_value::<FindArgs>(args) {
                    Ok(a) => Ok(to_tool_value(find(state, owner, a).await)),
                    Err(e) => Ok(bad_args(e)),
                }
            })
        }),
    );

    register(
        registry,
        "skill_get",
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "a skill id (skill:...) from `skill_find`"},
                "name": {"type": "string", "description": "the skill's name, instead of an id"},
                "vault_id": {"type": "string", "description": "with `name`: look only in this vault (a vault id or its name)"},
            },
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                match serde_json::from_value::<GetArgs>(args) {
                    Ok(a) => Ok(to_tool_value(get(state, owner, a).await)),
                    Err(e) => Ok(bad_args(e)),
                }
            })
        }),
    );

    register(
        registry,
        "skill_delete",
        json!({"type": "object", "properties": {"id": {"type": "string", "description": "a skill id (skill:...)"}}, "required": ["id"]}),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                match serde_json::from_value::<DeleteArgs>(args) {
                    Ok(a) => Ok(to_tool_value(delete(state, owner, &a.id).await)),
                    Err(e) => Ok(bad_args(e)),
                }
            })
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_skill_md_style() {
        for ok in ["a", "0", "deploy-rollback", "a-", &"a".repeat(64)] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in ["", "-a", "A", "has space", "under_score", "ünï", &"a".repeat(65)] {
            assert!(!valid_name(bad), "{bad}");
        }
    }
}
