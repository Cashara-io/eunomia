//! Entity-memory graph: person/organisation/location/repository/file/symbol +
//! memory + relates_to. Vault-scoped CRUD/query layer, same pattern as
//! `vaults::service` -- except the scope is a *vault* (`vaults::service`),
//! not a single owner, so entities and memories can be shared by every
//! member of a vault (a personal vault, an org vault, or a vault shared
//! person-to-person -- see `db.rs`'s schema comment).
//!
//! Every write still records `owner` (who wrote it, for audit/display) but
//! access control is entirely vault membership. Callers that don't pass a
//! `vault_id` get the caller's own personal vault.
//!
//! Dedup for people/orgs/locations is name-or-alias, case-insensitive, per
//! vault. `repository`/`file`/`symbol` are a second, separate entity map
//! (code entities an external agent maps while working in a codebase) that
//! lives in the same graph as person/organisation/location and can be
//! cross-linked to them via `relates_to`.
//!
//! Every function here deals in `RecordId` -- callers (routers, tools) parse path/user-supplied
//! ids first, same convention as `vaults::service`.

use surrealdb::types::SurrealValue;
use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use surrealdb::types::{Datetime, RecordId};
use crate::rid::RecordIdExt;

use crate::pool::{ControlDb, OrgDb};
use crate::store;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::store::entities as q;
use crate::tx::{lock, with_retry, with_retry_dup};
use crate::authz::{self, Action};
use crate::changes;
use crate::vaults::service as vaults_service;

/// Entity kinds this module manages -- the same set as
/// `vaults::service::ENTITY_KINDS`.
pub const KINDS: &[&str] = &["person", "organisation", "location", "repository", "file", "symbol"];

/// Code-graph subset of `KINDS`.
pub const CODE_KINDS: &[&str] = &["repository", "file", "symbol"];

/// The one table every agent-defined kind's entities live in, told apart by their `kind` field: a kind
/// name comes from an agent, so it never becomes a table name.
pub const CUSTOM_TABLE: &str = "entity";

/// Every entity table: the built-in kinds' own tables, then [`CUSTOM_TABLE`].
pub const ENTITY_TABLES: &[&str] = &["person", "organisation", "location", "repository", "file", "symbol", CUSTOM_TABLE];

/// Whether `e` is a read of [`CUSTOM_TABLE`] on a database from before migration 0016, which has no such
/// table. Such a database is still served (schema N-1) until its migration job runs, so a read across
/// every entity table treats the custom-kind table as empty there; writing a custom kind fails until then.
// ponytail: drop with the next tenant migration, once 0016 is the oldest schema served.
pub fn no_custom_table(e: &surrealdb::Error) -> bool {
    e.to_string().contains("The table 'entity' does not exist")
}

/// How many custom kinds one vault may hold, so an agent cannot mint thousands.
pub const MAX_CUSTOM_KINDS: usize = 50;

/// A custom kind name: a lowercase slug (`db`, `table`, `design_decision`) of at most 32 characters,
/// starting with a letter, and not a built-in kind.
pub fn is_custom_kind(kind: &str) -> bool {
    let mut chars = kind.chars();
    kind.len() <= 32
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !KINDS.contains(&kind)
}

/// The table `kind`'s entities live in: a built-in kind's own table, or [`CUSTOM_TABLE`] for a valid
/// custom kind. Used everywhere a kind picks the table of a raw SurrealDB query (table names can't be
/// bound parameters), so nothing but a fixed table name ever reaches string-formatted SQL.
pub fn kind_table(kind: &str) -> AppResult<&'static str> {
    if let Some(k) = KINDS.iter().find(|k| **k == kind) {
        return Ok(k);
    }
    if is_custom_kind(kind) {
        return Ok(CUSTOM_TABLE);
    }
    Err(AppError::bad_request(format!(
        "invalid kind {kind:?}: use a built-in kind ({}) or your own lowercase name, a letter then letters, digits or _ (at most 32), e.g. \"db\" or \"decision\"",
        KINDS.join(", ")
    )))
}

fn owner_key_string(owner: &RecordId) -> String {
    crate::rid::key_string(owner.key()).unwrap_or_else(|| owner.to_string())
}

/// The internal `cache_record` RecordId for a record's caller-facing id --
/// (the owner-id prefix is internal).
fn cache_record_rid(owner: &RecordId, record_id: &str) -> RecordId {
    RecordId::from_table_key("cache_record", format!("{}:{}", owner_key_string(owner), record_id))
}

// ---------------------------------------------------------------------------
// Row / output shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, SurrealValue)]
struct EntityRow {
    id: RecordId,
    #[serde(default)]
    #[surreal(default)]
    owner: Option<RecordId>,
    vault: RecordId,
    /// Set on a [`CUSTOM_TABLE`] row only; a built-in kind is its table.
    #[serde(default)]
    #[surreal(default)]
    kind: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    name: String,
    #[serde(default)]
    #[surreal(default)]
    aliases: Vec<String>,
    #[serde(default)]
    #[surreal(default)]
    summary: String,
}

impl EntityRow {
    fn kind(&self) -> String {
        self.kind.clone().unwrap_or_else(|| self.id.table().to_string())
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct EntityOut {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub summary: String,
    pub vault: String,
}

fn entity_out(row: &EntityRow) -> EntityOut {
    EntityOut {
        id: row.id.to_string(),
        kind: row.kind(),
        name: row.name.clone(),
        aliases: row.aliases.clone(),
        summary: row.summary.clone(),
        vault: row.vault.to_string(),
    }
}

#[derive(Debug, Clone, Deserialize, SurrealValue)]
struct MemoryRow {
    id: RecordId,
    #[serde(default)]
    #[surreal(default)]
    owner: Option<RecordId>,
    vault: RecordId,
    subject: RecordId,
    #[serde(default)]
    #[surreal(default)]
    text: String,
    #[serde(default)]
    #[surreal(default)]
    source: Option<RecordId>,
    created_at: Datetime,
    #[serde(rename = "type", default = "default_memory_type")]
    #[surreal(rename = "type", default = "default_memory_type")]
    mem_type: String,
    #[serde(default)]
    #[surreal(default)]
    proof_count: i64,
    #[serde(default)]
    #[surreal(default)]
    status: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    source_memories: Option<Vec<RecordId>>,
    #[serde(default)]
    #[surreal(default)]
    updated_at: Option<Datetime>,
    #[serde(default)]
    #[surreal(default)]
    version: i64,
}

fn default_memory_type() -> String {
    "world".to_string()
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MemoryOut {
    pub id: String,
    pub owner: Option<String>,
    pub vault: String,
    pub subject: String,
    pub text: String,
    pub source: Option<String>,
    #[schema(value_type = String)]
    pub created_at: Datetime,
    #[serde(rename = "type")]
    pub mem_type: String,
    pub proof_count: i64,
    pub status: Option<String>,
    pub source_memories: Option<Vec<String>>,
    #[schema(value_type = Option<String>)]
    pub updated_at: Option<Datetime>,
    pub version: i64,
    pub owner_email: Option<String>,
    /// Secrets this write replaced with `[redacted: <kind>]` markers (kind and count only). Present
    /// only on a write or edit that redacted something.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub redacted: Vec<crate::redact::Redaction>,
}

fn memory_out(row: &MemoryRow, owner_email: Option<String>) -> MemoryOut {
    MemoryOut {
        id: row.id.to_string(),
        owner: row.owner.as_ref().map(|o| o.to_string()),
        vault: row.vault.to_string(),
        subject: row.subject.to_string(),
        text: row.text.clone(),
        source: row.source.as_ref().map(|s| s.to_string()),
        created_at: row.created_at,
        mem_type: row.mem_type.clone(),
        proof_count: row.proof_count,
        status: row.status.clone(),
        source_memories: row.source_memories.as_ref().map(|v| v.iter().map(|r| r.to_string()).collect()),
        updated_at: row.updated_at,
        version: row.version,
        owner_email,
        redacted: Vec::new(),
    }
}

#[derive(Debug, Clone, Deserialize, SurrealValue)]
struct RelationRow {
    id: RecordId,
    #[serde(rename = "in")]
    #[surreal(rename = "in")]
    in_: RecordId,
    #[serde(rename = "out")]
    #[surreal(rename = "out")]
    out_: RecordId,
    #[serde(default)]
    #[surreal(default)]
    label: String,
    #[serde(default)]
    #[surreal(default)]
    source: Option<RecordId>,
    #[serde(default)]
    #[surreal(default)]
    owner: Option<RecordId>,
    #[serde(default)]
    #[surreal(default)]
    created_at: Option<Datetime>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct RelationOut {
    pub id: String,
    #[serde(rename = "in")]
    pub in_: String,
    #[serde(rename = "out")]
    pub out_: String,
    pub label: String,
    pub source: Option<String>,
    pub owner: Option<String>,
    #[schema(value_type = Option<String>)]
    pub created_at: Option<Datetime>,
    pub direction: Option<String>,
    pub owner_email: Option<String>,
}

fn relation_out(row: &RelationRow, direction: Option<&str>, owner_email: Option<String>) -> RelationOut {
    RelationOut {
        id: row.id.to_string(),
        in_: row.in_.to_string(),
        out_: row.out_.to_string(),
        label: row.label.clone(),
        source: row.source.as_ref().map(|s| s.to_string()),
        owner: row.owner.as_ref().map(|o| o.to_string()),
        created_at: row.created_at,
        direction: direction.map(str::to_string),
        owner_email,
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct EntityDetail {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub aliases: Vec<String>,
    pub summary: String,
    pub owner_email: Option<String>,
    pub memory: Vec<MemoryOut>,
    pub relations: Vec<RelationOut>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ListEntitiesOut {
    pub results: Vec<EntityOut>,
    pub total: usize,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct GraphNode {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub owner_email: Option<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub label: String,
    pub owner_email: Option<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct GraphOut {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WriteMemoryOut {
    pub entity: EntityOut,
    pub memory: MemoryOut,
    /// When the write created a new entity: existing ones of the same kind
    /// whose name shares a word with it -- candidates for `entity_merge`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub possible_duplicates: Vec<EntityOut>,
}

// ---------------------------------------------------------------------------
// Scope / access helpers
// ---------------------------------------------------------------------------

/// `vault_id`, membership-checked, or `owner`'s personal vault when omitted --
/// the one place every public function in this module resolves its scope.
async fn resolve_vault(db: &OrgDb, owner: &RecordId, vault_id: Option<&RecordId>, action: Action) -> AppResult<RecordId> {
    match vault_id {
        None => vaults_service::default_vault_id(db, owner).await,
        Some(v) => Ok(authz::authorize(db, owner, action, v).await?.vault().clone()),
    }
}

/// Whether `owner` may read/write a row in `vault` -- member of its vault.
/// Not-a-member is treated the same as not-found everywhere in this module.
async fn accessible(db: &OrgDb, owner: &RecordId, vault: &RecordId, action: Action) -> AppResult<bool> {
    match authz::authorize(db, owner, action, vault).await {
        Ok(_) => Ok(true),
        Err(e) if e.code == ErrorCode::VaultForbidden => Ok(false),
        Err(e) => Err(e),
    }
}

/// Batch-resolve `user` RecordIds to emails, for attributing who wrote what
/// in a shared vault -- one query per call site rather than N+1 lookups.
async fn emails_for(db: &ControlDb, user_ids: Vec<Option<RecordId>>) -> AppResult<HashMap<String, String>> {
    let ids: Vec<RecordId> = {
        #[allow(clippy::mutable_key_type)] // RecordId hashes by value; the interior mutability is never touched
        let mut seen = HashSet::new();
        user_ids
            .into_iter()
            .flatten()
            .filter(|id| seen.insert(id.clone()))
            .collect()
    };
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        id: RecordId,
        email: String,
    }
    let mut res = q::EMAILS_FOR.on(db).bind(("ids", ids)).await?;
    let rows: Vec<Row> = res.take(0)?;
    Ok(rows.into_iter().map(|r| (r.id.to_string(), r.email)).collect())
}

/// `rid` must name an entity table. `rid::parse` takes any table, so without this a tool or route
/// handed `memory:..` or `vault_member:..` would read it as an entity.
pub fn require_entity_id(rid: &RecordId) -> AppResult<()> {
    if ENTITY_TABLES.contains(&rid.table()) {
        return Ok(());
    }
    Err(AppError::bad_request(format!("{} is not an entity id", rid.to_string())))
}

/// `rid` must be a `memory` row.
pub fn require_memory_id(rid: &RecordId) -> AppResult<()> {
    if rid.table() == "memory" {
        return Ok(());
    }
    Err(AppError::bad_request(format!("{} is not a memory id", rid.to_string())))
}

/// Relations and merges stay inside one vault: an edge from a shared vault's entity into someone's
/// personal one would show its label and endpoint to every member, and merging would rewrite the other.
fn same_vault(a: &EntityRow, b: &EntityRow) -> AppResult<()> {
    if a.vault != b.vault {
        return Err(AppError::bad_request("both entities must be in the same vault"));
    }
    Ok(())
}

async fn select_entity(db: &OrgDb, rid: &RecordId) -> AppResult<Option<EntityRow>> {
    require_entity_id(rid)?;
    let row: Option<EntityRow> = store::get(db, rid).await?;
    Ok(row)
}

// ---------------------------------------------------------------------------
// Core CRUD
// ---------------------------------------------------------------------------

/// Find-or-create a `kind` entity in the resolved vault, matched
/// case-insensitively against existing `name`/`aliases`. New aliases are
/// merged onto a match rather than creating a duplicate row.
pub async fn upsert_entity(
    db: &OrgDb,
    owner: &RecordId,
    kind: &str,
    name: &str,
    aliases: Option<Vec<String>>,
    vault_id: Option<&RecordId>,
) -> AppResult<EntityOut> {
    Ok(upsert_entity_created(db, owner, kind, name, aliases, vault_id).await?.0)
}

/// `"Jane Doe (Acme/JD)"` -> `("Jane Doe", ["Acme", "JD"])`: a name with a
/// parenthesised suffix of aliases/affiliations.
fn split_decorated(name: &str) -> Option<(&str, Vec<String>)> {
    let (base, extra) = name.strip_suffix(')')?.rsplit_once('(')?;
    let base = base.trim();
    let extra: Vec<String> = extra.split(['/', ',', ';']).map(str::trim).filter(|a| !a.is_empty()).map(String::from).collect();
    (!base.is_empty() && !extra.is_empty()).then_some((base, extra))
}

/// [`upsert_entity`], plus whether the entity was created just now. A
/// decorated name (see [`split_decorated`]) whose base name already exists
/// resolves to that entity, the suffix parts becoming aliases.
pub async fn upsert_entity_created(
    db: &OrgDb,
    owner: &RecordId,
    kind: &str,
    name: &str,
    aliases: Option<Vec<String>>,
    vault_id: Option<&RecordId>,
) -> AppResult<(EntityOut, bool)> {
    let table = kind_table(kind)?;
    let vault = resolve_vault(db, owner, vault_id, Action::WriteMemories).await?;
    let aliases = aliases.unwrap_or_default();
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::bad_request("entity name can't be empty"));
    }
    let needle = name.to_lowercase();
    if table == CUSTOM_TABLE {
        check_custom_kind_room(db, &vault, kind).await?;
    }

    // A decorated name ("Jane Doe (Acme/JD)") whose base name already exists resolves to it, the
    // suffix parts becoming aliases.
    let decorated = split_decorated(name);
    // Read-then-write: a concurrent upsert of the same name (in any case) loses on the
    // `(vault, name_key)` unique index, and the retry's re-read finds the winner. Both lookups are
    // index reads (`name_key`, `alias_keys`), however large the vault.
    let (row, changed) = with_retry_dup(|| async {
        let mut found = None;
        let mut aliases = aliases.clone();
        let needles = std::iter::once((needle.clone(), None)).chain(decorated.as_ref().map(|(base, extra)| (base.to_lowercase(), Some(extra))));
        for (key, extra) in needles {
            let mut res =
                q::find_by_key(db, table).bind(("vault", vault.clone())).bind(("kind", kind.to_string())).bind(("needle", key)).await?.check()?;
            let by_name: Vec<EntityRow> = res.take(0)?;
            let by_alias: Vec<EntityRow> = res.take(1)?;
            if let Some(row) = by_name.into_iter().chain(by_alias).next() {
                aliases.extend(extra.into_iter().flatten().cloned());
                found = Some(row);
                break;
            }
        }

        if let Some(row) = found {
            if aliases.iter().all(|a| row.aliases.contains(a)) {
                return Ok((row, None));
            }
            // array::union is atomic, so two alias merges cannot lose each other's update.
            let mut updated = q::MERGE_ALIASES
                .on(db)
                .bind(("id", row.id.clone()))
                .bind(("aliases", aliases.clone()))
                .await?
                .check()?;
            let rows: Vec<EntityRow> = updated.take(0)?;
            return Ok((rows.into_iter().next().unwrap_or(row), Some(changes::UPDATED)));
        }

        let mut created = q::create_entity(db, table)
            .bind(("owner", owner.clone()))
            .bind(("vault", vault.clone()))
            .bind(("kind", kind.to_string()))
            .bind(("name", name.to_string()))
            .bind(("aliases", aliases.clone()))
            .await?
            .check()?;
        let rows: Vec<EntityRow> = created.take(0)?;
        let row = rows.into_iter().next().ok_or_else(|| surrealdb::Error::query("entity insert returned no row".into(), None))?;
        Ok((row, Some(changes::CREATED)))
    })
    .await?;
    if let Some(op) = changed {
        changes::record(db, &vault, op, changes::ENTITY, &row.id).await;
    }
    Ok((entity_out(&row), changed == Some(changes::CREATED)))
}

/// A new custom kind may join `vault` only while it holds fewer than [`MAX_CUSTOM_KINDS`]; a kind it
/// already has always passes.
// ponytail: read then write, so two writers adding different new kinds at once can each pass at the
// cap and leave it one over; a per-vault kind registry with a unique index would make it exact.
async fn check_custom_kind_room(db: &OrgDb, vault: &RecordId, kind: &str) -> AppResult<()> {
    let mut res = q::CUSTOM_KIND_ROOM.on(db).bind(("vault", vault.clone())).bind(("kind", kind.to_string())).await?.check()?;
    let known: Vec<RecordId> = res.take(0)?;
    if !known.is_empty() {
        return Ok(());
    }
    let kinds: Vec<CustomKindRow> = res.take(1)?;
    if kinds.len() >= MAX_CUSTOM_KINDS {
        return Err(AppError::bad_request(format!(
            "this vault already has {MAX_CUSTOM_KINDS} custom entity kinds, the most it can hold; reuse one (`entity_types` lists them)"
        )));
    }
    Ok(())
}

#[derive(Deserialize, SurrealValue)]
struct CustomKindRow {
    kind: String,
    #[serde(default)]
    #[surreal(default)]
    count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct CustomKind {
    pub kind: String,
    /// entities of this kind in the listed vaults
    pub entities: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntityTypesOut {
    pub built_in: Vec<&'static str>,
    pub custom: Vec<CustomKind>,
}

/// The built-in kinds, then every custom kind used in `vaults` (already resolved and checked by the
/// caller) with its entity count.
pub async fn entity_types(db: &OrgDb, vaults: &[RecordId]) -> AppResult<EntityTypesOut> {
    let rows: Vec<CustomKindRow> = match q::CUSTOM_KINDS_IN.on(db).bind(("vaults", vaults.to_vec())).await {
        Err(e) if no_custom_table(&e) => Vec::new(),
        res => res?.take(0)?,
    };
    Ok(EntityTypesOut {
        built_in: KINDS.to_vec(),
        custom: rows.into_iter().map(|r| CustomKind { kind: r.kind, entities: r.count }).collect(),
    })
}

/// Other `kind` entities in the entity's vault whose name or an alias shares a word (3+ letters) with
/// its name -- likely the same thing under another name ("Dave" / "David Smith" share nothing;
/// "David" / "David Smith" do).
pub async fn possible_duplicates(db: &OrgDb, entity: &EntityOut) -> AppResult<Vec<EntityOut>> {
    let table = kind_table(&entity.kind)?;
    let words: Vec<String> =
        entity.name.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 3).map(String::from).collect();
    if words.is_empty() {
        return Ok(Vec::new());
    }
    let mut res = q::sharing_a_word(db, table)
        .bind(("kind", entity.kind.clone()))
        .bind(("vault", crate::rid::parse(&entity.vault).map_err(|_| AppError::internal("vault id did not round-trip"))?))
        .bind(("id", crate::rid::parse(&entity.id).map_err(|_| AppError::internal("entity id did not round-trip"))?))
        .bind(("words", words))
        .await?
        .check()?;
    let rows: Vec<EntityRow> = res.take(0)?;
    Ok(rows.iter().map(entity_out).collect())
}

/// `source_record_id` is optional -- an automatic extraction path always ties
/// a memory back to the cache_record it was pulled from, but a programmatic
/// write (`write_memory`) may be a fact an agent knows directly, with no
/// backing cache_record.
///
/// `mem_type` defaults to "world"; callers may pass "experience" or
/// "observation" (the latter is written directly by `consolidate.rs` rather
/// than through this function in practice).
///
/// The memory's vault is always `subject_id`'s vault (not a separate
/// choice) -- returns a 403 if `owner` isn't a member of it.
///
/// Staleness: writing a new raw fact ("world"/"experience") for a subject
/// invalidates that subject's existing observation, if any -- marked here
/// rather than only in `write_memory` so the auto-extraction path also
/// triggers it.
pub async fn add_memory(
    db: &OrgDb,
    owner: &RecordId,
    subject_id: &RecordId,
    text: &str,
    source_record_id: Option<&str>,
    mem_type: &str,
) -> AppResult<MemoryOut> {
    let subject_row = select_entity(db, subject_id)
        .await?
        .ok_or_else(|| AppError::bad_request(format!("subject entity not found: {}", subject_id.to_string())))?;
    if !accessible(db, owner, &subject_row.vault, Action::WriteMemories).await? {
        return Err(AppError::coded(
            crate::error::ErrorCode::VaultForbidden,
            format!("not a member of {}'s vault", subject_row.vault.to_string()),
        ));
    }
    insert_memory(db, owner, subject_id, &subject_row.vault, text, source_record_id, mem_type).await
}

/// [`add_memory`] once the subject and its vault are known and the caller may write there.
async fn insert_memory(
    db: &OrgDb,
    owner: &RecordId,
    subject_id: &RecordId,
    vault: &RecordId,
    text: &str,
    source_record_id: Option<&str>,
    mem_type: &str,
) -> AppResult<MemoryOut> {
    // every memory write (tools, REST, extraction) lands here, so this is the one place secrets are dropped
    let mut redacted = Vec::new();
    let text = crate::redact::redact(text, &mut redacted);
    let source = source_record_id.map(|r| cache_record_rid(owner, r));
    let is_obs = mem_type == "observation";
    let obs_id = observation_rid(subject_id);

    // One round trip, all-or-nothing. An observation is revised in place (one
    // per subject; a new one gets a deterministic id so two racing creators
    // collide on the key instead of making duplicates). A raw fact creates the
    // memory and marks the subject's observation stale, skipping the write when
    // it is already stale so concurrent fact writers do not conflict on it.
    // The hot path only point-reads the deterministic id: a `WHERE subject`
    // range read would conflict with every concurrent insert for the subject.
    // The range read remains as a fallback for observations that predate
    // deterministic ids.
    let stmt = if is_obs { &q::WRITE_OBSERVATION } else { &q::WRITE_FACT };
    let _guard = lock(&subject_id.to_string()).await;
    let memory = with_retry_dup(|| async {
        let mut res = stmt
            .on(db)
            .bind(("owner", owner.clone()))
            .bind(("vault", vault.clone()))
            .bind(("subject", subject_id.clone()))
            .bind(("text", text.to_string()))
            .bind(("type", mem_type.to_string()))
            .bind(("source", source.clone()))
            .bind(("obs_id", obs_id.clone()))
            .await?
            .check()?;
        let rows: Vec<MemoryRow> = res.take(stmt.slot)?;
        rows.into_iter().next().ok_or_else(|| surrealdb::Error::query("memory write returned no row".into(), None))
    })
    .await?;
    // an observation is revised in place: past its first version it is an update
    let op = if memory.version > 1 { changes::UPDATED } else { changes::CREATED };
    changes::record(db, vault, op, changes::MEMORY, &memory.id).await;

    Ok(MemoryOut { redacted, ..memory_out(&memory, None) })
}

/// Deterministic id of a subject's observation when it is first created, so
/// concurrent creators collide (and retry into an update) instead of leaving
/// two observations. Older observations keep their random ids; lookups go by
/// `subject` + `type`, never by this id.
pub fn observation_rid(subject: &RecordId) -> RecordId {
    RecordId::from_table_key("memory", crate::tx::stable_key('o', &subject.to_string()))
}

/// Programmatic memory write -- a direct path for an agent to record a fact
/// about a person/organisation/location via a tool call. Finds-or-creates
/// the subject entity by reusing `upsert_entity`'s dedupe logic, then
#[allow(clippy::too_many_arguments)] // public signature, grouping args would change callers
/// records the memory against it.
pub async fn write_memory(
    db: &OrgDb,
    owner: &RecordId,
    subject_name: &str,
    subject_kind: &str,
    text: &str,
    source_record_id: Option<&str>,
    mem_type: &str,
    vault_id: Option<&RecordId>,
) -> AppResult<WriteMemoryOut> {
    authz::require_scope(Action::WriteMemories.scope())?;
    // upsert_entity_created resolved and checked the vault for a write, and the entity lives in it:
    // no need to read it back and check again (as `add_memory` does for a bare subject id)
    let (entity, created) = upsert_entity_created(db, owner, subject_kind, subject_name, None, vault_id).await?;
    let entity_rid: RecordId =
        crate::rid::parse(&entity.id).map_err(|_| AppError::internal("entity id did not round-trip"))?;
    let vault: RecordId = crate::rid::parse(&entity.vault).map_err(|_| AppError::internal("vault id did not round-trip"))?;
    let memory = insert_memory(db, owner, &entity_rid, &vault, text, source_record_id, mem_type).await?;
    let possible_duplicates = if created { possible_duplicates(db, &entity).await? } else { Vec::new() };
    Ok(WriteMemoryOut { entity, memory, possible_duplicates })
}

/// RELATE two entities, idempotent on the (in, out, label) unique index -- a
/// duplicate relation is a no-op that returns the existing edge. Both
/// endpoints must be in the same vault, one `owner` belongs to.
pub async fn add_relation(
    db: &OrgDb,
    owner: &RecordId,
    from_id: &RecordId,
    to_id: &RecordId,
    label: &str,
    source_record_id: Option<&str>,
) -> AppResult<RelationOut> {
    let in_row = select_entity(db, from_id).await?;
    let out_row = select_entity(db, to_id).await?;
    let in_ok = match &in_row {
        Some(r) => accessible(db, owner, &r.vault, Action::WriteMemories).await?,
        None => false,
    };
    let out_ok = match &out_row {
        Some(r) => accessible(db, owner, &r.vault, Action::WriteMemories).await?,
        None => false,
    };
    let (Some(in_row), Some(out_row)) = (in_row, out_row) else {
        return Err(AppError::coded(crate::error::ErrorCode::VaultForbidden, "not a member of both entities' vaults"));
    };
    if !in_ok || !out_ok {
        return Err(AppError::coded(crate::error::ErrorCode::VaultForbidden, "not a member of both entities' vaults"));
    }
    same_vault(&in_row, &out_row)?;

    if let Some(existing) = find_relation(db, from_id, to_id, label).await? {
        return Ok(relation_out(&existing, None, None));
    }

    let source = source_record_id.map(|r| cache_record_rid(owner, r));
    let res = q::RELATE_RETURNING
        .on(db)
        .bind(("in", from_id.clone()))
        .bind(("out", to_id.clone()))
        .bind(("label", label.to_string()))
        .bind(("owner", owner.clone()))
        .bind(("source", source))
        .await;

    match res {
        Ok(mut res) => {
            let rows: Vec<RelationRow> = res.take(0)?;
            match rows.into_iter().next() {
                Some(row) => {
                    changes::record(db, &in_row.vault, changes::CREATED, changes::RELATION, &row.id).await;
                    Ok(relation_out(&row, None, None))
                }
                None => find_relation(db, from_id, to_id, label)
                    .await?
                    .map(|r| relation_out(&r, None, None))
                    .ok_or_else(|| AppError::internal("RELATE returned no row")),
            }
        }
        // Could be a lost race against a concurrent RELATE for the same edge
        // (recoverable -- fetch what won) or a genuine failure. Only swallow
        // the former; re-raise if the fallback SELECT also finds nothing.
        Err(e) => match find_relation(db, from_id, to_id, label).await? {
            Some(row) => Ok(relation_out(&row, None, None)),
            None => Err(AppError::from(e)),
        },
    }
}

async fn find_relation(db: &OrgDb, from_id: &RecordId, to_id: &RecordId, label: &str) -> AppResult<Option<RelationRow>> {
    let mut res = q::FIND_RELATION
        .on(db)
        .bind(("in", from_id.clone()))
        .bind(("out", to_id.clone()))
        .bind(("label", label.to_string()))
        .await?;
    let rows: Vec<RelationRow> = res.take(0)?;
    Ok(rows.into_iter().next())
}

/// Delete one `memory` row, vault-scoped. Returns `false` (no-op) if it
/// doesn't exist or `owner` isn't a member of its vault, rather than
/// erroring -- mirrors `get_entity`'s not-found-is-None convention.
pub async fn delete_memory(db: &OrgDb, owner: &RecordId, memory_id: &RecordId) -> AppResult<bool> {
    require_memory_id(memory_id)?;
    let row: Option<MemoryRow> = store::get(db, memory_id).await?;
    let Some(row) = row else { return Ok(false) };
    if !accessible(db, owner, &row.vault, Action::WriteMemories).await? {
        return Ok(false);
    }
    // A deleted raw fact leaves its subject's observation's lineage and makes it stale; an
    // observation built from nothing but this fact is deleted with it (one transaction).
    with_retry(|| async {
        q::DELETE_MEMORY
            .on(db)
            .bind(("id", memory_id.clone()))
            .bind(("subject", row.subject.clone()))
            .bind(("raw", row.mem_type != "observation"))
            .await?
            .check()
    })
    .await?;
    // ponytail: an observation deleted along with its only fact gets no row of its own; recall finds it gone
    changes::record(db, &row.vault, changes::DELETED, changes::MEMORY, memory_id).await;
    Ok(true)
}

/// Memory types a raw fact can be switched between by [`update_memory`];
/// `observation` is the consolidated belief and keeps its type.
const RAW_MEMORY_TYPES: &[&str] = &["world", "experience"];

/// Validates an edit request: something to change, and a legal type change
/// for a memory that is currently `current_type`.
fn check_memory_edit(current_type: &str, text: Option<&str>, new_type: Option<&str>) -> AppResult<()> {
    if text.is_none() && new_type.is_none() {
        return Err(AppError::bad_request("nothing to update: pass `text` and/or `type`"));
    }
    if text.is_some_and(|t| t.trim().is_empty()) {
        return Err(AppError::bad_request("memory text can't be empty"));
    }
    if let Some(t) = new_type
        && (current_type == "observation" || !RAW_MEMORY_TYPES.contains(&t)) {
            return Err(AppError::bad_request(
                "`type` can only switch a fact between world and experience; observations keep theirs",
            ));
        }
    Ok(())
}

/// Edit one memory's text and/or type (world <-> experience), vault-scoped,
/// not-found-is-None like `get_entity`. Bumps `version`. Editing a raw fact
/// marks its subject's observation stale, same as writing a new one.
pub async fn update_memory(
    db: &OrgDb,
    owner: &RecordId,
    memory_id: &RecordId,
    text: Option<&str>,
    new_type: Option<&str>,
) -> AppResult<Option<MemoryOut>> {
    require_memory_id(memory_id)?;
    let row: Option<MemoryRow> = store::get(db, memory_id).await?;
    let Some(row) = row else { return Ok(None) };
    if !accessible(db, owner, &row.vault, Action::WriteMemories).await? {
        return Ok(None);
    }
    check_memory_edit(&row.mem_type, text, new_type)?;
    let mut redacted = Vec::new();
    let text = text.map(|t| crate::redact::redact(t, &mut redacted));
    let text = text.as_deref();

    let stmt = match (text, new_type) {
        (Some(_), Some(_)) => &q::UPDATE_MEMORY_TEXT_TYPE,
        (Some(_), None) => &q::UPDATE_MEMORY_TEXT,
        _ => &q::UPDATE_MEMORY_TYPE,
    };
    let mut res = with_retry(|| async {
        let mut qry = stmt
            .on(db)
            .bind(("id", memory_id.clone()))
            .bind(("subject", row.subject.clone()))
            .bind(("raw", row.mem_type != "observation"));
        if let Some(t) = text {
            qry = qry.bind(("text", t.to_string()));
        }
        if let Some(t) = new_type {
            qry = qry.bind(("type", t.to_string()));
        }
        qry.await?.check()
    })
    .await?;
    let rows: Vec<MemoryRow> = res.take(stmt.slot)?;
    let updated = rows.into_iter().next().ok_or_else(|| AppError::internal("memory update returned no row"))?;
    changes::record(db, &row.vault, changes::UPDATED, changes::MEMORY, memory_id).await;
    Ok(Some(MemoryOut { redacted, ..memory_out(&updated, None) }))
}

/// Edit an entity's own fields -- name/aliases/summary -- vault-scoped, same
/// not-found-is-None convention as `get_entity`. Only the fields passed
/// (`Some`) are updated.
pub async fn update_entity(
    db: &OrgDb,
    owner: &RecordId,
    entity_id: &RecordId,
    name: Option<&str>,
    aliases: Option<Vec<String>>,
    summary: Option<&str>,
) -> AppResult<Option<EntityOut>> {
    let name = name.map(str::trim);
    if name == Some("") {
        return Err(AppError::bad_request("entity name can't be empty"));
    }
    let Some(mut row) = select_entity(db, entity_id).await? else { return Ok(None) };
    if !accessible(db, owner, &row.vault, Action::WriteMemories).await? {
        return Ok(None);
    }

    let mut set_clauses: Vec<&str> = Vec::new();
    if name.is_some() {
        set_clauses.push("name = $name");
    }
    if aliases.is_some() {
        set_clauses.push("aliases = $aliases");
    }
    if summary.is_some() {
        set_clauses.push("summary = $summary");
    }

    if !set_clauses.is_empty() {
        set_clauses.push("updated_at = time::now()");
        let mut qry = q::update_entity(db, &set_clauses.join(", ")).bind(("id", entity_id.clone()));
        if let Some(n) = name {
            qry = qry.bind(("name", n.to_string()));
        }
        if let Some(a) = aliases {
            qry = qry.bind(("aliases", a));
        }
        if let Some(s) = summary {
            qry = qry.bind(("summary", s.to_string()));
        }
        let mut res = qry.await?;
        let rows: Vec<EntityRow> = res.take(0)?;
        row = rows.into_iter().next().ok_or_else(|| AppError::internal("entity update returned no row"))?;
        changes::record(db, &row.vault, changes::UPDATED, changes::ENTITY, entity_id).await;
    }

    Ok(Some(entity_out(&row)))
}

/// Delete an entity and everything hanging off it: its `memory` rows and its
/// `relates_to` edges in both directions. Vault-scoped, same
/// not-found-is-false convention as `delete_memory`.
pub async fn delete_entity(db: &OrgDb, owner: &RecordId, entity_id: &RecordId) -> AppResult<bool> {
    let Some(row) = select_entity(db, entity_id).await? else { return Ok(false) };
    if !accessible(db, owner, &row.vault, Action::WriteMemories).await? {
        return Ok(false);
    }
    with_retry(|| async {
        q::DELETE_ENTITY
        .on(db)
        .bind(("id", entity_id.clone()))
        .await?
        .check()
    })
    .await?;
    // its memories and edges go with it: one row for the entity, not one per memory
    changes::record(db, &row.vault, changes::DELETED, changes::ENTITY, entity_id).await;
    Ok(true)
}

/// Merge `loser_id` into `winner_id` -- for two entities of the same `kind`
/// that turned out to be duplicates. Reassigns the loser's `memory` rows and
/// `relates_to` edges (both directions) to the winner, adds the loser's name
/// and aliases as aliases of the winner, then deletes the loser, all in one
/// transaction ([`merge_rows`]). If both had an observation they become one
/// stale observation, rebuilt on the next consolidation. Returns the winner's
/// row after the merge.
///
/// Errors (400) if the two ids are the same, of different kinds or vaults, or not
/// found / `owner` isn't a member of either one's vault.
pub async fn merge_entities(
    db: &OrgDb,
    owner: &RecordId,
    winner_id: &RecordId,
    loser_id: &RecordId,
) -> AppResult<EntityOut> {
    if winner_id == loser_id {
        return Err(AppError::bad_request("winner_id and loser_id must refer to different entities"));
    }
    if winner_id.table() != loser_id.table() {
        return Err(AppError::bad_request(format!(
            "cannot merge entities of different kinds: {:?} vs {:?}",
            winner_id.table(),
            loser_id.table()
        )));
    }

    let winner = select_entity(db, winner_id)
        .await?
        .ok_or_else(|| AppError::bad_request(format!("winner entity not found: {}", winner_id.to_string())))?;
    if !accessible(db, owner, &winner.vault, Action::WriteMemories).await? {
        return Err(AppError::bad_request(format!("winner entity not found: {}", winner_id.to_string())));
    }
    let loser = select_entity(db, loser_id)
        .await?
        .ok_or_else(|| AppError::bad_request(format!("loser entity not found: {}", loser_id.to_string())))?;
    if !accessible(db, owner, &loser.vault, Action::WriteMemories).await? {
        return Err(AppError::bad_request(format!("loser entity not found: {}", loser_id.to_string())));
    }
    same_vault(&winner, &loser)?;
    if winner.kind() != loser.kind() {
        return Err(AppError::bad_request(format!("cannot merge entities of different kinds: {:?} vs {:?}", winner.kind(), loser.kind())));
    }

    merge_rows(db, winner_id, loser_id, loser_names(&loser)).await?;
    changes::record(db, &winner.vault, changes::MERGED, changes::ENTITY, loser_id).await;
    changes::record(db, &winner.vault, changes::UPDATED, changes::ENTITY, winner_id).await;
    let winner = select_entity(db, winner_id).await?.ok_or_else(|| AppError::internal("winner vanished during merge"))?;
    Ok(entity_out(&winner))
}

/// What a merged-away entity leaves on the winner: its name and aliases.
fn loser_names(loser: &EntityRow) -> Vec<String> {
    std::iter::once(&loser.name).chain(&loser.aliases).filter(|n| !n.is_empty()).cloned().collect()
}

/// Moves everything of `loser` onto `winner` (same kind and vault, already checked) and deletes
/// `loser`, in ONE transaction (see [`q::MERGE_ENTITIES`]): any failing statement rolls all of it
/// back, so a merge never half-happens. Retried on a commit conflict with a concurrent write.
async fn merge_rows(db: &OrgDb, winner: &RecordId, loser: &RecordId, add_aliases: Vec<String>) -> AppResult<()> {
    with_retry(|| async {
        q::MERGE_ENTITIES
            .on(db)
            .bind(("winner", winner.clone()))
            .bind(("loser", loser.clone()))
            .bind(("aliases", add_aliases.clone()))
            .await?
            .check()
    })
    .await?;
    Ok(())
}

/// An entity's row plus its `memory` entries and `relates_to` edges in both
/// directions, or `None` if it doesn't exist / `owner` isn't a member of its
/// vault. Every row carries `owner_email` -- who wrote it -- since in a
/// shared vault that's no longer implied by who's asking.
pub async fn get_entity(db: &OrgDb, control: &ControlDb, owner: &RecordId, entity_id: &RecordId) -> AppResult<Option<EntityDetail>> {
    let Some(row) = select_entity(db, entity_id).await? else { return Ok(None) };
    if !accessible(db, owner, &row.vault, Action::ReadMemories).await? {
        return Ok(None);
    }

    // only rows in the entity's own vault: a cross-vault edge or memory left over from before
    // relations and merges were confined to one vault stays hidden
    let mut mem_res = q::MEMORIES_OF
        .on(db)
        .bind(("id", entity_id.clone()))
        .bind(("vault", row.vault.clone()))
        .await?;
    let memories: Vec<MemoryRow> = mem_res.take(0)?;

    let mut out_res = q::EDGES_OUT_IN_VAULT.on(db).bind(("id", entity_id.clone())).bind(("vault", row.vault.clone())).await?;
    let outgoing: Vec<RelationRow> = out_res.take(0)?;
    let mut in_res = q::EDGES_IN_IN_VAULT.on(db).bind(("id", entity_id.clone())).bind(("vault", row.vault.clone())).await?;
    let incoming: Vec<RelationRow> = in_res.take(0)?;

    let mut owner_ids: Vec<Option<RecordId>> = vec![row.owner.clone()];
    owner_ids.extend(memories.iter().map(|m| m.owner.clone()));
    owner_ids.extend(outgoing.iter().map(|r| r.owner.clone()));
    owner_ids.extend(incoming.iter().map(|r| r.owner.clone()));
    let emails = emails_for(control, owner_ids).await?;

    let memory_out_rows: Vec<MemoryOut> = memories
        .iter()
        .map(|m| memory_out(m, m.owner.as_ref().and_then(|o| emails.get(&o.to_string()).cloned())))
        .collect();
    let mut relations: Vec<RelationOut> = outgoing
        .iter()
        .map(|r| relation_out(r, Some("out"), r.owner.as_ref().and_then(|o| emails.get(&o.to_string()).cloned())))
        .collect();
    relations.extend(
        incoming
            .iter()
            .map(|r| relation_out(r, Some("in"), r.owner.as_ref().and_then(|o| emails.get(&o.to_string()).cloned()))),
    );

    Ok(Some(EntityDetail {
        id: entity_id.to_string(),
        kind: row.kind(),
        name: row.name.clone(),
        aliases: row.aliases.clone(),
        summary: row.summary.clone(),
        owner_email: row.owner.as_ref().and_then(|o| emails.get(&o.to_string()).cloned()),
        memory: memory_out_rows,
        relations,
    }))
}

/// Where each of `kinds` lives: its table, plus the `kind` to filter [`CUSTOM_TABLE`] by. `None` is
/// every entity table, unfiltered (all kinds, built-in and custom).
fn kind_scopes(kinds: Option<Vec<&str>>) -> AppResult<Vec<(&'static str, Option<String>)>> {
    match kinds {
        None => Ok(ENTITY_TABLES.iter().map(|t| (*t, None)).collect()),
        Some(ks) => ks
            .into_iter()
            .map(|k| kind_table(k).map(|t| (t, (t == CUSTOM_TABLE).then(|| k.to_string()))))
            .collect(),
    }
}

/// Every entity of `scopes` in `vault`, scope by scope (each ordered by name when `ordered`).
async fn select_in_vault(db: &OrgDb, vault: &RecordId, scopes: &[(&'static str, Option<String>)], ordered: bool) -> AppResult<Vec<EntityRow>> {
    let mut out = Vec::new();
    for (table, kind) in scopes {
        let res = q::select_by_vault(db, table, ordered, kind.is_some())
            .bind(("vault", vault.clone()))
            .bind(("kind", kind.clone().unwrap_or_default()))
            .await;
        match res {
            Err(e) if no_custom_table(&e) => {}
            res => out.extend(res?.take::<Vec<EntityRow>>(0)?),
        }
    }
    Ok(out)
}

/// Entities across `kind` (or all kinds, built-in then custom) in the resolved vault, ordered by
/// name within each kind. Offset-paginated across the combined (all-kinds)
/// result -- `limit=None` returns everything from `offset` onward. Returns
/// `{results, total, has_more}`.
pub async fn list_entities(
    db: &OrgDb,
    owner: &RecordId,
    kind: Option<&str>,
    vault_id: Option<&RecordId>,
    limit: Option<usize>,
    offset: usize,
) -> AppResult<ListEntitiesOut> {
    let vault = resolve_vault(db, owner, vault_id, Action::ReadMemories).await?;
    let rows = select_in_vault(db, &vault, &kind_scopes(kind.map(|k| vec![k]))?, true).await?;

    let total = rows.len();
    let page: Vec<EntityRow> = match limit {
        None => rows.into_iter().skip(offset).collect(),
        Some(l) => rows.into_iter().skip(offset).take(l).collect(),
    };
    let results: Vec<EntityOut> = page.iter().map(entity_out).collect();
    let has_more = offset + results.len() < total;

    Ok(ListEntitiesOut { results, total, has_more })
}

/// [`list_entities`] filtered to `needle` (see `entities::tools::entities_search`) over `vaults`, which
/// the caller has already resolved and checked.
pub async fn search_entities(
    db: &OrgDb,
    vaults: &[RecordId],
    needle: &str,
    kind: Option<&str>,
    limit: usize,
    offset: usize,
) -> AppResult<ListEntitiesOut> {
    let scopes = kind_scopes(kind.map(|k| vec![k]))?;
    let tables: Vec<&str> = scopes.iter().map(|(t, _)| *t).collect();
    let by_kind = kind.filter(|_| tables == [CUSTOM_TABLE]);
    #[derive(Deserialize, SurrealValue)]
    struct Page {
        total: i64,
        results: Vec<EntityRow>,
    }
    let end = offset.saturating_add(limit);
    let run = |tables: &[&str]| {
        q::search_entities(db, tables, by_kind.is_some(), !needle.is_empty())
            .bind(("kind", by_kind.unwrap_or_default().to_string()))
            .bind(("vaults", vaults.to_vec()))
            .bind(("needle", needle.to_lowercase()))
            .bind(("q", needle.to_string()))
            .bind(("start", offset as i64))
            .bind(("end", i64::try_from(end).unwrap_or(i64::MAX)))
    };
    let mut res = match run(&tables).await {
        // schema N-1: no custom-kind table yet, so search the built-in kinds alone
        Err(e) if no_custom_table(&e) && by_kind.is_none() => run(KINDS).await?,
        res => res?,
    };
    let page: Option<Page> = res.take(3)?; // three LETs, then the RETURN
    let Page { total, results } = page.ok_or_else(|| AppError::internal("entity search returned nothing"))?;
    let total = usize::try_from(total).unwrap_or_default();
    let results: Vec<EntityOut> = results.iter().map(entity_out).collect();
    let has_more = offset + results.len() < total;
    Ok(ListEntitiesOut { results, total, has_more })
}

/// `{nodes, edges}` -- the full entity graph for the dashboard's
/// force-directed view (resolved vault, or `owner`'s personal vault), or a
/// subgraph restricted to `kinds`.
///
/// v1 limitation, accepted as-is: an edge is
/// only included when BOTH endpoints are in the requested kind set.
pub async fn graph(
    db: &OrgDb,
    control: &ControlDb,
    owner: &RecordId,
    kinds: Option<&[String]>,
    vault_id: Option<&RecordId>,
) -> AppResult<GraphOut> {
    let vault = resolve_vault(db, owner, vault_id, Action::ReadMemories).await?;
    let scopes = kind_scopes(kinds.map(|ks| ks.iter().map(String::as_str).collect()))?;
    let rows_by_kind = select_in_vault(db, &vault, &scopes, false).await?;

    let emails = emails_for(control, rows_by_kind.iter().map(|r| r.owner.clone()).collect()).await?;
    let nodes: Vec<GraphNode> = rows_by_kind
        .iter()
        .map(|r| GraphNode {
            id: r.id.to_string(),
            kind: r.kind(),
            name: r.name.clone(),
            owner_email: r.owner.as_ref().and_then(|o| emails.get(&o.to_string()).cloned()),
        })
        .collect();
    let ids: Vec<RecordId> = rows_by_kind.iter().map(|r| r.id.clone()).collect();

    let mut edges: Vec<GraphEdge> = Vec::new();
    if !ids.is_empty() {
        // `in IN $ids` doesn't match against a union-typed `record<a|b|c>`
        // field reliably -- `$ids CONTAINS field` does.
        let mut res = q::EDGES_AMONG
            .on(db)
            .bind(("ids", ids))
            .await?;
        let rows: Vec<RelationRow> = res.take(0)?;
        let edge_emails = emails_for(control, rows.iter().map(|r| r.owner.clone()).collect()).await?;
        edges = rows
            .iter()
            .map(|r| GraphEdge {
                source: r.in_.to_string(),
                target: r.out_.to_string(),
                label: r.label.clone(),
                owner_email: r.owner.as_ref().and_then(|o| edge_emails.get(&o.to_string()).cloned()),
            })
            .collect();
    }

    Ok(GraphOut { nodes, edges })
}

/// Expected `relates_to` label for a code-entity's parent pairing --
/// file->repository = "part_of", symbol->file = "defined_in".
fn parent_label(kind: &str, parent_kind: &str) -> &'static str {
    match (kind, parent_kind) {
        ("file", "repository") => "part_of",
        ("symbol", "file") => "defined_in",
        _ => "part_of",
    }
}

/// Find-or-create a `repository`/`file`/`symbol` entity -- the write path an
/// external agent calls as it maps a codebase, parallel to `write_memory`'s
/// pattern for person/organisation/location.
///
/// `parent_id`, if given, auto-creates a `relates_to` edge from this entity
/// to the parent, labelled by the expected kind pairing. This is an
/// agent-facing API, so a mismatched pairing isn't an error -- it just falls
/// back to the generic "part_of" label rather than hard-failing.
pub async fn upsert_code_entity(
    db: &OrgDb,
    owner: &RecordId,
    kind: &str,
    name: &str,
    parent_id: Option<&RecordId>,
    summary: Option<&str>,
    vault_id: Option<&RecordId>,
) -> AppResult<EntityOut> {
    if !CODE_KINDS.contains(&kind) {
        return Err(AppError::bad_request(format!("unknown code kind {kind:?}")));
    }
    let mut entity = upsert_entity(db, owner, kind, name, None, vault_id).await?;
    let entity_rid: RecordId = crate::rid::parse(&entity.id).map_err(|_| AppError::internal("entity id did not round-trip"))?;

    if let Some(summary) = summary {
        let mut updated = q::SET_SUMMARY
            .on(db)
            .bind(("id", entity_rid.clone()))
            .bind(("summary", summary.to_string()))
            .await?;
        let rows: Vec<EntityRow> = updated.take(0)?;
        let row = rows.into_iter().next().ok_or_else(|| AppError::internal("entity update returned no row"))?;
        changes::record(db, &row.vault, changes::UPDATED, changes::ENTITY, &row.id).await;
        entity = entity_out(&row);
    }

    if let Some(parent_id) = parent_id {
        let label = parent_label(kind, parent_id.table());
        add_relation(db, owner, &entity_rid, parent_id, label, None).await?;
    }

    Ok(entity)
}

#[cfg(test)]
mod tests {
    use super::check_memory_edit;

    #[test]
    fn memory_edit_needs_something_to_change() {
        assert!(check_memory_edit("world", None, None).is_err());
        assert!(check_memory_edit("world", Some("new"), None).is_ok());
        assert!(check_memory_edit("world", None, Some("experience")).is_ok());
    }

    #[test]
    fn memory_edit_rejects_blank_text_and_observation_type_changes() {
        assert!(check_memory_edit("world", Some("  "), None).is_err());
        assert!(check_memory_edit("world", None, Some("observation")).is_err());
        assert!(check_memory_edit("world", None, Some("bogus")).is_err());
        assert!(check_memory_edit("observation", None, Some("world")).is_err());
        assert!(check_memory_edit("observation", Some("revised belief"), None).is_ok());
    }

    use super::*;

    #[test]
    fn decorated_names_split_into_base_and_aliases() {
        assert_eq!(split_decorated("Jane Doe (Acme/JD)"), Some(("Jane Doe", vec!["Acme".into(), "JD".into()])));
        assert_eq!(split_decorated("Jane Doe(Acme, JD; jd2)").unwrap().1, vec!["Acme", "JD", "jd2"]);
        assert_eq!(split_decorated("Jane Doe"), None);
        assert_eq!(split_decorated("(Acme)"), None);
        assert_eq!(split_decorated("Jane Doe ()"), None);
    }

    #[test]
    fn kinds_match_schema_union() {
        assert_eq!(KINDS, ["person", "organisation", "location", "repository", "file", "symbol"]);
    }

    #[test]
    fn code_kinds_is_subset_of_kinds() {
        for k in CODE_KINDS {
            assert!(KINDS.contains(k));
        }
    }

    #[test]
    fn kind_table_accepts_known_kinds() {
        assert_eq!(kind_table("person").unwrap(), "person");
        assert_eq!(kind_table("symbol").unwrap(), "symbol");
    }

    #[test]
    fn kind_table_rejects_unknown_or_injected_kind() {
        // any other valid name is a custom kind, kept in the custom-kind table: never a table of its own
        assert_eq!(kind_table("user").unwrap(), CUSTOM_TABLE);
        assert_eq!(kind_table("person").unwrap(), "person");
        assert!(kind_table("person; DROP TABLE user").is_err());
        assert!(kind_table("").is_err());
    }

    #[test]
    fn custom_kind_names_are_lowercase_slugs() {
        for ok in ["db", "table", "design_decision", "k2", &"a".repeat(32)] {
            assert!(is_custom_kind(ok), "{ok}");
        }
        for bad in ["", "DB", "Db", "2db", "_db", "db-table", "db table", "dé", &"a".repeat(33), "person", "symbol"] {
            assert!(!is_custom_kind(bad), "{bad}");
        }
    }

    #[test]
    fn parent_label_maps_file_to_repository() {
        assert_eq!(parent_label("file", "repository"), "part_of");
    }

    #[test]
    fn parent_label_maps_symbol_to_file() {
        assert_eq!(parent_label("symbol", "file"), "defined_in");
    }

    #[test]
    fn parent_label_falls_back_to_part_of_for_mismatched_pairing() {
        assert_eq!(parent_label("symbol", "repository"), "part_of");
        assert_eq!(parent_label("file", "symbol"), "part_of");
    }

    #[test]
    fn default_memory_type_is_world() {
        assert_eq!(default_memory_type(), "world");
    }
}
