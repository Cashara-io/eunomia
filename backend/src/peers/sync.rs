//! Keeping two copies of a vault in step (docs/peer-vaults.md, "Sync").
//!
//! Each side reads its own change log after a cursor and turns each changed item into an [`Item`]: the
//! item's current content and its hash, or a tombstone when it is gone. Rows the link itself wrote
//! (actor `peer:<link>`) are not sent back over that link, which stops echoes; rows other links wrote
//! are, which is what relays a change down a tree (A to B to C).
//!
//! The receiver checks each item's hash against its content (a mismatch is refused and counted), maps
//! the sender's id to a local item (`peer_item`), and writes through the normal service paths, so
//! the change log, counts and secret redaction stay right. An item whose content already matches is
//! a no-op that writes nothing (this also ends cycles). When both sides changed one item since they
//! last agreed, the later `updated_at` wins, ties broken by the larger hash; both sides decide alike.
//!
//! Carried: entities (built-in and custom kinds), raw memories, relations, skills. Not carried:
//! documents, and consolidated observations, which each side derives from its own facts.

use std::collections::{BTreeMap, HashSet};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use surrealdb::types::{Datetime, RecordId, SurrealValue};

use super::{crypto, link_secret, load_link, net, status, LinkRow, GUEST, HOST, ROLE_EDIT};
use crate::authz::{self, Actor, Caller};
use crate::changes;
use crate::entities::service as entities;
use crate::error::{AppError, AppResult};
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::state::OrgState;
use crate::store;
use crate::store::peers as q;

/// Change-log rows read per exchange page.
pub const PAGE: i64 = 500;
/// Serialized items per page, at most (the request body limit is 1 MiB, and base64 adds a third).
const BYTE_CAP: usize = 512 * 1024;
/// Pages per sync before the rest waits for the next one.
const MAX_ROUNDS: usize = 20;
/// The scheduler syncs a guest link whose last try is older than this.
pub const SYNC_EVERY: &str = "25s";

/// A reference to an entity of the sender (a memory's subject, a relation's ends, a merge winner).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Ref {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_id: Option<String>,
    #[serde(default)]
    pub origin: String,
}

/// One changed item on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    /// `entity`, `memory`, `relation` or `skill`.
    pub kind: String,
    /// The sender's record id.
    pub id: String,
    /// The receiver's id for it, when the sender knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_id: Option<String>,
    /// `<instance id>:<record id>` where the item was first written.
    #[serde(default)]
    pub origin: String,
    /// `upsert`, `delete`, or `merge` (an entity merged into `into`).
    pub op: String,
    /// The synced fields (what `hash` covers).
    #[serde(default)]
    pub data: Value,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub refs: BTreeMap<String, Ref>,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub into: Option<Ref>,
}

// --- content and hashes ------------------------------------------------------------------------

pub fn entity_content(kind: &str, name: &str, aliases: &[String], summary: &str) -> Value {
    let mut a = aliases.to_vec();
    a.sort();
    a.dedup();
    json!({ "kind": kind, "name": name, "aliases": a, "summary": summary })
}

pub fn memory_content(subject_kind: &str, subject_name: &str, text: &str, mem_type: &str) -> Value {
    json!({ "subject": { "kind": subject_kind, "name": subject_name }, "text": text, "type": mem_type })
}

pub fn relation_content(from: (&str, &str), to: (&str, &str), label: &str) -> Value {
    json!({ "from": { "kind": from.0, "name": from.1 }, "to": { "kind": to.0, "name": to.1 }, "label": label })
}

pub fn skill_content(name: &str, description: &str, body: &str) -> Value {
    json!({ "name": name, "description": description, "body": body })
}

/// sha256 over the canonical JSON of the item's kind and synced fields.
pub fn item_hash(kind: &str, data: &Value) -> String {
    crypto::content_hash(&json!({ "kind": kind, "data": data }))
}

fn kind_of_table(table: &str) -> Option<&'static str> {
    if entities::ENTITY_TABLES.contains(&table) {
        return Some(changes::ENTITY);
    }
    match table {
        "memory" => Some(changes::MEMORY),
        "relates_to" => Some(changes::RELATION),
        "skill" => Some(changes::SKILL),
        _ => None,
    }
}

fn static_kind(kind: &str) -> Option<&'static str> {
    [changes::ENTITY, changes::MEMORY, changes::RELATION, changes::SKILL].into_iter().find(|k| *k == kind)
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or_default()
}

fn ts_string(d: Option<Datetime>) -> Option<String> {
    d.map(|d| d.into_inner().to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
}

fn parse_ts(s: &Option<String>) -> Option<chrono::DateTime<chrono::Utc>> {
    s.as_deref().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok()).map(|d| d.with_timezone(&chrono::Utc))
}

// --- local rows --------------------------------------------------------------------------------

/// Any synced row, read with `SELECT *`: each kind fills its own fields.
#[derive(Debug, Clone, SurrealValue)]
struct AnyRow {
    id: RecordId,
    #[surreal(default)]
    vault: Option<RecordId>,
    #[surreal(default)]
    kind: Option<String>,
    #[surreal(default)]
    name: Option<String>,
    #[surreal(default)]
    aliases: Option<Vec<String>>,
    #[surreal(default)]
    summary: Option<String>,
    #[surreal(default)]
    subject: Option<RecordId>,
    #[surreal(default)]
    text: Option<String>,
    #[surreal(rename = "type", default)]
    mem_type: Option<String>,
    #[surreal(rename = "in", default)]
    src: Option<RecordId>,
    #[surreal(rename = "out", default)]
    dst: Option<RecordId>,
    #[surreal(default)]
    label: Option<String>,
    #[surreal(default)]
    description: Option<String>,
    #[surreal(default)]
    body: Option<String>,
    #[surreal(default)]
    created_at: Option<Datetime>,
    #[surreal(default)]
    updated_at: Option<Datetime>,
}

impl AnyRow {
    fn entity_kind(&self) -> String {
        self.kind.clone().unwrap_or_else(|| self.id.table().to_string())
    }
}

/// A local item as sync sees it.
struct Local {
    row: AnyRow,
    data: Value,
    hash: String,
    at: Option<Datetime>,
    /// subject / from / to: the local entity ids
    refs: BTreeMap<String, RecordId>,
}

enum Found {
    Missing,
    /// Exists but is not synced (a consolidated observation).
    Skip,
    Here(Box<Local>),
}

async fn entity_row(db: &OrgDb, vault: &RecordId, rid: &RecordId) -> AppResult<Option<AnyRow>> {
    if kind_of_table(rid.table()) != Some(changes::ENTITY) {
        return Ok(None);
    }
    let row: Option<AnyRow> = store::get(db, rid).await?;
    Ok(row.filter(|r| r.vault.as_ref() == Some(vault)))
}

/// The local `kind` item `rid` in `vault`, with its synced content. Anything in another vault, or of
/// another kind, is missing: a peer can never reach outside the linked vault.
async fn load_local(db: &OrgDb, vault: &RecordId, kind: &str, rid: &RecordId) -> AppResult<Found> {
    if kind_of_table(rid.table()) != Some(kind).and_then(static_kind) {
        return Ok(Found::Missing);
    }
    let Some(row) = store::get::<AnyRow>(db, rid).await? else { return Ok(Found::Missing) };
    let mut refs = BTreeMap::new();
    let data = match kind {
        changes::ENTITY => {
            if row.vault.as_ref() != Some(vault) {
                return Ok(Found::Missing);
            }
            entity_content(&row.entity_kind(), row.name.as_deref().unwrap_or_default(), row.aliases.as_deref().unwrap_or_default(), row.summary.as_deref().unwrap_or_default())
        }
        changes::MEMORY => {
            if row.vault.as_ref() != Some(vault) {
                return Ok(Found::Missing);
            }
            let mem_type = row.mem_type.clone().unwrap_or_else(|| "world".into());
            if mem_type == "observation" {
                return Ok(Found::Skip);
            }
            let Some(subject) = row.subject.clone() else { return Ok(Found::Missing) };
            let Some(sub) = entity_row(db, vault, &subject).await? else { return Ok(Found::Missing) };
            refs.insert("subject".to_string(), subject);
            memory_content(&sub.entity_kind(), sub.name.as_deref().unwrap_or_default(), row.text.as_deref().unwrap_or_default(), &mem_type)
        }
        changes::RELATION => {
            let (Some(src), Some(dst)) = (row.src.clone(), row.dst.clone()) else { return Ok(Found::Missing) };
            let (Some(a), Some(b)) = (entity_row(db, vault, &src).await?, entity_row(db, vault, &dst).await?) else { return Ok(Found::Missing) };
            refs.insert("from".to_string(), src);
            refs.insert("to".to_string(), dst);
            relation_content(
                (&a.entity_kind(), a.name.as_deref().unwrap_or_default()),
                (&b.entity_kind(), b.name.as_deref().unwrap_or_default()),
                row.label.as_deref().unwrap_or_default(),
            )
        }
        changes::SKILL => {
            if row.vault.as_ref() != Some(vault) {
                return Ok(Found::Missing);
            }
            skill_content(row.name.as_deref().unwrap_or_default(), row.description.as_deref().unwrap_or_default(), row.body.as_deref().unwrap_or_default())
        }
        _ => return Ok(Found::Missing),
    };
    let hash = item_hash(kind, &data);
    let at = row.updated_at.or(row.created_at);
    Ok(Found::Here(Box::new(Local { row, data, hash, at, refs })))
}

// --- the item map ------------------------------------------------------------------------------

#[derive(Debug, Clone, SurrealValue)]
struct MapRow {
    link: String,
    local: String,
    #[surreal(default)]
    remote: Option<String>,
    #[surreal(default)]
    origin: Option<String>,
    #[surreal(default)]
    hash: String,
}

fn map_rid(link: &str, local: &str) -> RecordId {
    RecordId::from_table_key("peer_item", format!("{link}|{local}"))
}

async fn maps_of(db: &OrgDb, vault: &RecordId, local: &str) -> AppResult<Vec<MapRow>> {
    let mut res = q::ITEMS_BY_LOCAL.on(db).bind(("vault", vault.clone())).bind(("local", local.to_string())).await?;
    Ok(res.take(0)?)
}

/// What this link knows about each item: one link's view of the shared state.
pub struct Ctx<'a> {
    pub state: &'a OrgState,
    pub link: &'a LinkRow,
    pub key: String,
    /// This instance's id.
    pub me: String,
}

impl Ctx<'_> {
    fn db(&self) -> &OrgDb {
        &self.state.db
    }

    fn vault(&self) -> &RecordId {
        &self.link.vault
    }

    fn owner(&self) -> &RecordId {
        &self.link.user
    }

    /// The change-log actor of what this link writes.
    pub fn actor(&self) -> String {
        format!("peer:{}", self.key)
    }

    fn origin_of(&self, local: &str, maps: &[MapRow]) -> String {
        maps.iter().find_map(|m| m.origin.clone()).unwrap_or_else(|| format!("{}:{local}", self.me))
    }

    async fn save_map(&self, kind: &str, local: &RecordId, remote: Option<String>, origin: Option<String>, hash: &str) -> AppResult<()> {
        let local = local.to_string();
        q::UPSERT_ITEM
            .on(self.db())
            .bind(("id", map_rid(&self.key, &local)))
            .bind(("link", self.key.clone()))
            .bind(("vault", self.vault().clone()))
            .bind(("kind", kind.to_string()))
            .bind(("local", local))
            .bind(("remote", remote))
            .bind(("origin", origin))
            .bind(("hash", hash.to_string()))
            .await?
            .check()?;
        Ok(())
    }

    async fn drop_map(&self, local: &RecordId) -> AppResult<()> {
        q::DELETE_ITEM.on(self.db()).bind(("id", map_rid(&self.key, &local.to_string()))).await?.check()?;
        Ok(())
    }

    async fn ref_for(&self, entity: &RecordId) -> AppResult<Ref> {
        let id = entity.to_string();
        let maps = maps_of(self.db(), self.vault(), &id).await?;
        let peer_id = maps.iter().find(|m| m.link == self.key).and_then(|m| m.remote.clone());
        Ok(Ref { origin: self.origin_of(&id, &maps), id, peer_id })
    }
}

// --- outgoing ----------------------------------------------------------------------------------

pub struct Outgoing {
    pub items: Vec<Item>,
    /// The last change-log seq this page covers (the cursor to ack).
    pub upto: i64,
    pub more: bool,
    /// Documents changed in the page (not carried).
    pub documents: usize,
}

async fn build_item(ctx: &Ctx<'_>, kind: &str, id: &str, into: Option<&str>) -> AppResult<Option<Item>> {
    let Ok(rid) = crate::rid::parse(id) else { return Ok(None) };
    let maps = maps_of(ctx.db(), ctx.vault(), id).await?;
    let origin = ctx.origin_of(id, &maps);
    let peer_id = maps.iter().find(|m| m.link == ctx.key).and_then(|m| m.remote.clone());
    let base = Item { kind: kind.to_string(), id: id.to_string(), peer_id, origin, op: "delete".into(), data: Value::Null, refs: BTreeMap::new(), hash: String::new(), updated_at: None, into: None };
    match Box::pin(load_local(ctx.db(), ctx.vault(), kind, &rid)).await? {
        Found::Skip => Ok(None),
        Found::Missing => {
            let mut winner = None;
            if let Some(Ok(w)) = into.map(crate::rid::parse)
                && matches!(Box::pin(load_local(ctx.db(), ctx.vault(), changes::ENTITY, &w)).await?, Found::Here(_))
            {
                winner = Some(ctx.ref_for(&w).await?);
            }
            let into = winner;
            Ok(Some(Item { op: if into.is_some() { "merge".into() } else { "delete".into() }, into, ..base }))
        }
        Found::Here(local) => {
            let mut refs = BTreeMap::new();
            for (name, entity) in &local.refs {
                refs.insert(name.clone(), ctx.ref_for(entity).await?);
            }
            Ok(Some(Item { op: "upsert".into(), data: local.data, refs, hash: local.hash, updated_at: ts_string(local.at), ..base }))
        }
    }
}

/// This side's items changed after `after`, except what this link wrote itself.
pub async fn outgoing(ctx: &Ctx<'_>, after: i64) -> AppResult<Outgoing> {
    let page = changes::after(ctx.db(), ctx.vault(), after, PAGE).await?;
    let mut more = page.changes.len() as i64 >= PAGE;
    let actor = ctx.actor();
    let (mut items, mut seen, mut bytes, mut upto, mut documents) = (Vec::new(), HashSet::new(), 0usize, after, 0usize);
    for (i, ch) in page.changes.iter().enumerate() {
        upto = ch.seq;
        if ch.actor == actor || !seen.insert(ch.id.clone()) {
            continue;
        }
        if ch.kind == changes::DOCUMENT {
            documents += 1;
            continue;
        }
        // a merge logs the loser `merged`, then the winner `updated`
        let into = (ch.op == changes::MERGED)
            .then(|| page.changes.get(i + 1))
            .flatten()
            .filter(|n| n.kind == changes::ENTITY && n.op == changes::UPDATED)
            .map(|n| n.id.as_str());
        let Some(item) = Box::pin(build_item(ctx, &ch.kind, &ch.id, into)).await? else { continue };
        bytes += serde_json::to_string(&item).map_or(0, |s| s.len());
        items.push(item);
        if bytes > BYTE_CAP {
            more = more || i + 1 < page.changes.len();
            break;
        }
    }
    Ok(Outgoing { items, upto, more, documents })
}

/// After the other side took `items`: their content is the agreed base from now on.
async fn mark_sent(ctx: &Ctx<'_>, items: &[Item]) {
    for item in items {
        let Ok(rid) = crate::rid::parse(&item.id) else { continue };
        let done = if item.op == "upsert" { ctx.save_map(&item.kind, &rid, None, None, &item.hash).await } else { ctx.drop_map(&rid).await };
        if let Err(e) = done {
            tracing::warn!(link = %ctx.key, error = %e.message, "peer sync: item map not updated");
        }
    }
}

// --- incoming ----------------------------------------------------------------------------------

/// Counts of one side applying the other's items.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct Report {
    /// Written here.
    #[serde(default)]
    pub applied: usize,
    /// Already the same here.
    #[serde(default)]
    pub unchanged: usize,
    /// Failed their integrity check (content did not match its hash): refused.
    #[serde(default)]
    pub rejected: usize,
    /// Could not be written (logged).
    #[serde(default)]
    pub failed: usize,
    /// Not taken: this side only shares to read.
    #[serde(default)]
    pub ignored: usize,
    /// Changed on both sides; this side's newer version was kept.
    #[serde(default)]
    pub kept_local: usize,
    /// Kinds this version does not carry.
    #[serde(default)]
    pub skipped: usize,
}

impl Report {
    fn add(&mut self, o: &Report) {
        self.applied += o.applied;
        self.unchanged += o.unchanged;
        self.rejected += o.rejected;
        self.failed += o.failed;
        self.ignored += o.ignored;
        self.kept_local += o.kept_local;
        self.skipped += o.skipped;
    }
}

enum Outcome {
    Applied,
    Unchanged,
    Rejected,
    KeptLocal,
    Skipped,
}

/// The local item the sender's `remote` id (or `peer_id`, or `origin`) is, if it is in the vault.
async fn resolve(ctx: &Ctx<'_>, kind: &str, remote: &str, peer_id: Option<&str>, origin: &str) -> AppResult<Option<RecordId>> {
    let mut res = q::ITEM_BY_REMOTE.on(ctx.db()).bind(("link", ctx.key.clone())).bind(("remote", remote.to_string())).await?;
    let by_remote: Vec<MapRow> = res.take(0)?;
    let mut candidates: Vec<String> = by_remote.into_iter().map(|m| m.local).collect();
    candidates.extend(peer_id.map(str::to_string));
    if let Some(mine) = origin.strip_prefix(&format!("{}:", ctx.me)) {
        candidates.push(mine.to_string());
    } else if !origin.is_empty() {
        let mut res = q::ITEM_BY_ORIGIN.on(ctx.db()).bind(("vault", ctx.vault().clone())).bind(("origin", origin.to_string())).await?;
        let rows: Vec<MapRow> = res.take(0)?;
        candidates.extend(rows.into_iter().map(|m| m.local));
    }
    for c in candidates {
        let Ok(rid) = crate::rid::parse(&c) else { continue };
        if !matches!(Box::pin(load_local(ctx.db(), ctx.vault(), kind, &rid)).await?, Found::Missing) {
            return Ok(Some(rid));
        }
    }
    Ok(None)
}

/// The local entity a sender's reference names: mapped, or found or made by kind and name.
async fn entity_for(ctx: &Ctx<'_>, r: Option<&Ref>, who: &Value) -> AppResult<RecordId> {
    if let Some(r) = r
        && let Some(local) = Box::pin(resolve(ctx, changes::ENTITY, &r.id, r.peer_id.as_deref(), &r.origin)).await?
    {
        return Ok(local);
    }
    let (out, _) = Box::pin(entities::upsert_entity_created(ctx.db(), ctx.owner(), s(who, "kind"), s(who, "name"), None, Some(ctx.vault()))).await?;
    let local = crate::rid::parse(&out.id).map_err(|_| AppError::internal("entity id did not round-trip"))?;
    if let Some(r) = r
        && let Found::Here(l) = Box::pin(load_local(ctx.db(), ctx.vault(), changes::ENTITY, &local)).await?
    {
        ctx.save_map(changes::ENTITY, &local, Some(r.id.clone()), (!r.origin.is_empty()).then(|| r.origin.clone()), &l.hash).await?;
    }
    Ok(local)
}

fn json_id(v: &Value) -> AppResult<RecordId> {
    crate::rid::parse(s(v, "id")).map_err(|_| AppError::internal("write returned no id"))
}

/// Writes a new local item for `item`; returns its id, or `None` when the kind is not written here.
async fn create_local(ctx: &Ctx<'_>, item: &Item) -> AppResult<Option<RecordId>> {
    let (db, owner, vault, d) = (ctx.db(), ctx.owner(), ctx.vault(), &item.data);
    Ok(Some(match item.kind.as_str() {
        changes::ENTITY => {
            let aliases: Vec<String> = serde_json::from_value(d["aliases"].clone()).unwrap_or_default();
            let (out, _) = Box::pin(entities::upsert_entity_created(db, owner, s(d, "kind"), s(d, "name"), Some(aliases.clone()), Some(vault))).await?;
            let id = crate::rid::parse(&out.id).map_err(|_| AppError::internal("entity id did not round-trip"))?;
            // found an entity of that name already: keep its aliases too, take the summary unless empty
            let mut merged = out.aliases.clone();
            for a in aliases {
                if !merged.contains(&a) {
                    merged.push(a);
                }
            }
            let summary = if s(d, "summary").is_empty() { out.summary.clone() } else { s(d, "summary").to_string() };
            if merged != out.aliases || summary != out.summary {
                Box::pin(entities::update_entity(db, owner, &id, None, Some(merged), Some(&summary))).await?;
            }
            id
        }
        changes::MEMORY => {
            if s(d, "type") == "observation" {
                return Ok(None);
            }
            let subject = entity_for(ctx, item.refs.get("subject"), &d["subject"]).await?;
            let out = Box::pin(entities::add_memory(db, owner, &subject, s(d, "text"), None, s(d, "type"))).await?;
            crate::rid::parse(&out.id).map_err(|_| AppError::internal("memory id did not round-trip"))?
        }
        changes::RELATION => {
            let from = entity_for(ctx, item.refs.get("from"), &d["from"]).await?;
            let to = entity_for(ctx, item.refs.get("to"), &d["to"]).await?;
            let out = Box::pin(entities::add_relation(db, owner, &from, &to, s(d, "label"), None)).await?;
            crate::rid::parse(&out.id).map_err(|_| AppError::internal("relation id did not round-trip"))?
        }
        changes::SKILL => {
            let out = Box::pin(crate::tools::registry::skills::save_in(ctx.state, owner, vault, s(d, "name"), s(d, "description"), s(d, "body"))).await?;
            json_id(&out)?
        }
        _ => return Ok(None),
    }))
}

/// Brings local item `l` (whose current state is `cur`) to `item`'s content; returns its id after
/// (a memory that moved to another subject, a relation whose ends changed, a renamed skill: a new one).
async fn update_local(ctx: &Ctx<'_>, item: &Item, l: &RecordId, cur: &Local) -> AppResult<Option<RecordId>> {
    let (db, owner, d) = (ctx.db(), ctx.owner(), &item.data);
    match item.kind.as_str() {
        changes::ENTITY => {
            let aliases: Vec<String> = serde_json::from_value(d["aliases"].clone()).unwrap_or_default();
            let name = s(d, "name");
            let rename = (cur.row.name.as_deref() != Some(name)).then_some(name);
            Box::pin(entities::update_entity(db, owner, l, rename, Some(aliases), Some(s(d, "summary")))).await?;
            Ok(Some(l.clone()))
        }
        changes::MEMORY => {
            let subject = entity_for(ctx, item.refs.get("subject"), &d["subject"]).await?;
            if cur.refs.get("subject") != Some(&subject) {
                Box::pin(entities::delete_memory(db, owner, l)).await?;
                return Box::pin(create_local(ctx, item)).await;
            }
            let text = (cur.row.text.as_deref() != Some(s(d, "text"))).then(|| s(d, "text"));
            let raw = |t: &str| t == "world" || t == "experience";
            let cur_type = cur.row.mem_type.as_deref().unwrap_or("world");
            let new_type = (cur_type != s(d, "type") && raw(cur_type) && raw(s(d, "type"))).then(|| s(d, "type"));
            if text.is_some() || new_type.is_some() {
                Box::pin(entities::update_memory(db, owner, l, text, new_type)).await?;
            }
            Ok(Some(l.clone()))
        }
        changes::RELATION => {
            let from = entity_for(ctx, item.refs.get("from"), &d["from"]).await?;
            let to = entity_for(ctx, item.refs.get("to"), &d["to"]).await?;
            if cur.refs.get("from") == Some(&from) && cur.refs.get("to") == Some(&to) && cur.row.label.as_deref() == Some(s(d, "label")) {
                return Ok(Some(l.clone()));
            }
            delete_local(ctx, changes::RELATION, l).await?;
            Box::pin(create_local(ctx, item)).await
        }
        changes::SKILL => {
            if cur.row.name.as_deref() != Some(s(d, "name")) {
                delete_local(ctx, changes::SKILL, l).await?;
            }
            Box::pin(create_local(ctx, item)).await
        }
        _ => Ok(None),
    }
}

async fn delete_local(ctx: &Ctx<'_>, kind: &str, l: &RecordId) -> AppResult<()> {
    let (db, owner) = (ctx.db(), ctx.owner());
    match kind {
        changes::ENTITY => {
            Box::pin(entities::delete_entity(db, owner, l)).await?;
        }
        changes::MEMORY => {
            Box::pin(entities::delete_memory(db, owner, l)).await?;
        }
        changes::RELATION => {
            crate::tx::with_retry(|| async { q::DELETE_RELATION.on(db).bind(("id", l.clone())).await?.check() }).await?;
            changes::record(db, ctx.vault(), changes::DELETED, changes::RELATION, l).await;
        }
        changes::SKILL => {
            Box::pin(crate::tools::registry::skills::delete_by_id(ctx.state, owner, l)).await?;
        }
        _ => {}
    }
    ctx.drop_map(l).await
}

async fn apply_one(ctx: &Ctx<'_>, item: &Item) -> AppResult<Outcome> {
    let Some(kind) = static_kind(&item.kind) else { return Ok(Outcome::Skipped) };
    if item.id.len() > 200 || item.origin.len() > 300 {
        return Ok(Outcome::Rejected);
    }
    if item.op == "upsert" && item_hash(kind, &item.data) != item.hash {
        tracing::warn!(link = %ctx.key, kind, id = %item.id, "peer sync: an item failed its integrity check and was refused");
        return Ok(Outcome::Rejected);
    }
    let local = resolve(ctx, kind, &item.id, item.peer_id.as_deref(), &item.origin).await?;
    match item.op.as_str() {
        "delete" | "merge" => {
            let Some(l) = local else { return Ok(Outcome::Unchanged) };
            let winner = match (&item.into, item.op.as_str()) {
                (Some(w), "merge") if kind == changes::ENTITY => resolve(ctx, changes::ENTITY, &w.id, w.peer_id.as_deref(), &w.origin).await?,
                _ => None,
            };
            match winner {
                Some(w) if w != l && w.table() == l.table() => {
                    Box::pin(entities::merge_entities(ctx.db(), ctx.owner(), &w, &l)).await?;
                    ctx.drop_map(&l).await?;
                }
                _ => delete_local(ctx, kind, &l).await?,
            }
            Ok(Outcome::Applied)
        }
        "upsert" => {
            let after = match &local {
                Some(l) => match Box::pin(load_local(ctx.db(), ctx.vault(), kind, l)).await? {
                    Found::Here(cur) => {
                        if cur.hash == item.hash {
                            ctx.save_map(kind, l, Some(item.id.clone()), Some(item.origin.clone()), &cur.hash).await?;
                            return Ok(Outcome::Unchanged);
                        }
                        // changed here too since both last agreed: the later write wins, alike on both sides
                        let maps = maps_of(ctx.db(), ctx.vault(), &l.to_string()).await?;
                        let base = maps.iter().find(|m| m.link == ctx.key).map(|m| m.hash.clone());
                        if base.is_some_and(|b| b != cur.hash) {
                            let mine = (cur.at.map(|d| d.into_inner()), cur.hash.as_str());
                            let theirs = (parse_ts(&item.updated_at), item.hash.as_str());
                            if mine > theirs {
                                return Ok(Outcome::KeptLocal);
                            }
                        }
                        Box::pin(update_local(ctx, item, l, &cur)).await?
                    }
                    Found::Skip => return Ok(Outcome::Skipped),
                    Found::Missing => Box::pin(create_local(ctx, item)).await?,
                },
                None => Box::pin(create_local(ctx, item)).await?,
            };
            let Some(l) = after else { return Ok(Outcome::Skipped) };
            let hash = match Box::pin(load_local(ctx.db(), ctx.vault(), kind, &l)).await? {
                Found::Here(cur) => cur.hash,
                _ => item.hash.clone(),
            };
            ctx.save_map(kind, &l, Some(item.id.clone()), Some(item.origin.clone()), &hash).await?;
            if hash != item.hash {
                // the write came out different here (a name that matched another entity, a redacted
                // secret): log it as this side's own change, so it goes back and both copies agree
                authz::as_system(changes::record(ctx.db(), ctx.vault(), changes::UPDATED, kind, &l)).await;
            }
            Ok(Outcome::Applied)
        }
        _ => Ok(Outcome::Skipped),
    }
}

/// Applies the other side's items, as the link (`peer:<link>` in the change log, so they are not
/// echoed back over it).
pub async fn apply(ctx: &Ctx<'_>, items: Vec<Item>) -> Report {
    let caller = Caller {
        actor: Actor { kind: "system", id: ctx.actor() },
        scopes: crate::scopes::ALL.iter().map(|s| s.to_string()).collect(),
        vault: Some(ctx.vault().clone()),
        expires_at: None,
    };
    authz::with_caller(caller, async {
        let mut r = Report::default();
        for item in &items {
            match Box::pin(apply_one(ctx, item)).await {
                Ok(Outcome::Applied) => r.applied += 1,
                Ok(Outcome::Unchanged) => r.unchanged += 1,
                Ok(Outcome::Rejected) => r.rejected += 1,
                Ok(Outcome::KeptLocal) => r.kept_local += 1,
                Ok(Outcome::Skipped) => r.skipped += 1,
                Err(e) => {
                    tracing::warn!(link = %ctx.key, kind = %item.kind, id = %item.id, error = %e.message, "peer sync: an item could not be written");
                    r.failed += 1;
                }
            }
        }
        r
    })
    .await
}

// --- digest ------------------------------------------------------------------------------------

/// sha256 over the sorted hashes of everything sync carries in `vault`, and how many items that is.
/// Two copies holding the same items have the same digest.
pub async fn digest(db: &OrgDb, vault: &RecordId) -> AppResult<(String, usize)> {
    #[derive(SurrealValue)]
    struct E {
        #[surreal(default)]
        kind: Option<String>,
        #[surreal(default)]
        name: String,
        #[surreal(default)]
        aliases: Vec<String>,
        #[surreal(default)]
        summary: String,
    }
    #[derive(SurrealValue)]
    struct M {
        #[surreal(default)]
        text: String,
        #[surreal(rename = "type", default)]
        mem_type: String,
        subject: RecordId,
        #[surreal(default)]
        subject_kind: Option<String>,
        #[surreal(default)]
        subject_name: String,
    }
    #[derive(SurrealValue)]
    struct R {
        #[surreal(default)]
        label: String,
        src: RecordId,
        dst: RecordId,
        #[surreal(default)]
        src_kind: Option<String>,
        #[surreal(default)]
        src_name: String,
        #[surreal(default)]
        dst_kind: Option<String>,
        #[surreal(default)]
        dst_name: String,
    }
    #[derive(SurrealValue)]
    struct S {
        #[surreal(default)]
        name: String,
        #[surreal(default)]
        description: String,
        #[surreal(default)]
        body: String,
    }
    let mut res = q::VAULT_CONTENT.on(db).bind(("vault", vault.clone())).await?;
    let mut hashes = Vec::new();
    for (slot, table) in entities::ENTITY_TABLES.iter().enumerate() {
        let rows: Vec<E> = res.take(slot)?;
        for e in rows {
            let kind = e.kind.unwrap_or_else(|| table.to_string());
            hashes.push(item_hash(changes::ENTITY, &entity_content(&kind, &e.name, &e.aliases, &e.summary)));
        }
    }
    let base = entities::ENTITY_TABLES.len();
    let mems: Vec<M> = res.take(base)?;
    for m in mems {
        let kind = m.subject_kind.unwrap_or_else(|| m.subject.table().to_string());
        hashes.push(item_hash(changes::MEMORY, &memory_content(&kind, &m.subject_name, &m.text, &m.mem_type)));
    }
    let rels: Vec<R> = res.take(base + 1)?;
    for r in rels {
        let a = r.src_kind.unwrap_or_else(|| r.src.table().to_string());
        let b = r.dst_kind.unwrap_or_else(|| r.dst.table().to_string());
        hashes.push(item_hash(changes::RELATION, &relation_content((&a, &r.src_name), (&b, &r.dst_name), &r.label)));
    }
    let skills: Vec<S> = res.take(base + 2)?;
    for k in skills {
        hashes.push(item_hash(changes::SKILL, &skill_content(&k.name, &k.description, &k.body)));
    }
    hashes.sort();
    let n = hashes.len();
    Ok((crypto::sha256_hex(hashes.join("\n").as_bytes()), n))
}

// --- the exchange ------------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ExchangeRequest {
    #[serde(default)]
    pub changes: Vec<Item>,
    /// The guest's last seq these changes cover.
    #[serde(default)]
    pub upto: i64,
    /// The host's seq the guest has applied.
    #[serde(default)]
    pub after: i64,
    /// The guest's digest after its last sync.
    #[serde(default)]
    pub digest: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ExchangeResponse {
    #[serde(default)]
    pub changes: Vec<Item>,
    #[serde(default)]
    pub next: i64,
    #[serde(default)]
    pub more: bool,
    /// The guest seq the host has taken.
    #[serde(default)]
    pub ack: i64,
    /// The host's digest, when this page caught the guest up.
    #[serde(default)]
    pub digest: Option<String>,
    /// How the host applied the guest's changes.
    #[serde(default)]
    pub report: Report,
}

pub enum CallError {
    /// No address answered (or it answered with an error that may pass).
    Unreachable(String),
    /// The host says the link is over.
    Revoked,
}

impl CallError {
    pub fn into_app(self) -> AppError {
        match self {
            CallError::Unreachable(m) => AppError::new(axum::http::StatusCode::BAD_GATEWAY, format!("Can't reach the other Eunomia: {m}")),
            CallError::Revoked => AppError::bad_request("The other Eunomia stopped sharing this space."),
        }
    }
}

/// One authenticated, encrypted request to the host of a guest link, trying the address that
/// answered last first. Returns the decrypted answer and the address that gave it.
pub async fn call(state: &OrgState, row: &LinkRow, payload: Value) -> Result<(Value, String), CallError> {
    let key = row.key();
    let secret = link_secret(&state.settings, row).map_err(|e| CallError::Unreachable(e.message))?;
    let path = format!("/api/peer/v1/{}/links/{key}", row.peer_org);
    let plain = serde_json::to_vec(&payload).map_err(|e| CallError::Unreachable(e.to_string()))?;
    let mut addrs: Vec<String> = row.addr.iter().cloned().collect();
    addrs.extend(row.peer_addrs.iter().filter(|a| Some(*a) != row.addr.as_ref()).cloned());
    let mut last = "no address to try".to_string();
    for addr in addrs {
        let (ts, nonce) = (chrono::Utc::now().timestamp(), crypto::random_id());
        let data = crypto::seal(&secret, &crypto::request_aad("POST", &path, &key, ts, &nonce), &plain);
        let body = json!({ "link": key, "ts": ts, "nonce": nonce, "data": data }).to_string();
        let reply = match net::post(&format!("{addr}{path}"), body, net::CALL_TIMEOUT).await {
            Ok(r) => r,
            Err(e) => {
                last = e;
                continue;
            }
        };
        if reply.status != 200 {
            last = format!("HTTP {}", reply.status);
            continue;
        }
        let Some(env) = reply.json() else {
            last = "an answer that isn't JSON".into();
            continue;
        };
        let (rts, rnonce, rdata) = (env["ts"].as_i64().unwrap_or_default(), s(&env, "nonce").to_string(), s(&env, "data").to_string());
        let Some(opened) = crypto::open(&secret, &crypto::response_aad(&key, &nonce, rts, &rnonce), &rdata) else {
            // not from the host (or tampered with): try the next address
            last = "an answer that failed its authenticity check".into();
            continue;
        };
        if crypto::replay().check(&format!("r:{key}"), &rnonce, rts, chrono::Utc::now().timestamp()).is_err() {
            last = "a replayed or badly timed answer (check both clocks)".into();
            continue;
        }
        let v: Value = serde_json::from_slice(&opened).map_err(|e| CallError::Unreachable(e.to_string()))?;
        if v["revoked"] == json!(true) {
            return Err(CallError::Revoked);
        }
        return Ok((v, addr));
    }
    Err(CallError::Unreachable(last))
}

static RUNNING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// A guest link's sync is running in this process.
pub fn in_progress(key: &str) -> bool {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|s| s.contains(key))
}

struct Running(String);

impl Running {
    fn start(key: &str) -> Option<Running> {
        let mut g = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        g.get_or_insert_with(HashSet::new).insert(key.to_string()).then(|| Running(key.to_string()))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(s) = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            s.remove(&self.0);
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn save_progress(
    db: &OrgDb,
    row: &LinkRow,
    cursors: (i64, i64),
    addr: Option<String>,
    status: &str,
    last_error: Option<String>,
    integrity: usize,
    digests: (Option<String>, Option<String>),
    report: Value,
) -> AppResult<()> {
    let matches = match &digests {
        (Some(a), Some(b)) => Some(a == b),
        _ => None,
    };
    q::SAVE_PROGRESS
        .on(db)
        .bind(("id", row.id.clone()))
        .bind(("out_cursor", cursors.0))
        .bind(("in_cursor", cursors.1))
        .bind(("addr", addr))
        .bind(("status", status.to_string()))
        .bind(("last_error", last_error))
        .bind(("ok", status != status::UNREACHABLE && status != status::REVOKED))
        .bind(("integrity", integrity as i64))
        .bind(("local_digest", digests.0))
        .bind(("remote_digest", digests.1))
        .bind(("digests_match", matches))
        .bind(("report", Some(report)))
        .await?
        .check()?;
    Ok(())
}

/// One sync of a guest link: pages of changes both ways until both are caught up (or
/// [`MAX_ROUNDS`]), then the digests. Records status words for the UI either way.
pub async fn sync_link(state: &OrgState, key: &str) -> AppResult<Report> {
    let Some(_running) = Running::start(key) else { return Ok(Report::default()) };
    let row = load_link(&state.db, key).await?.ok_or_else(|| AppError::not_found("No such shared space link."))?;
    if row.direction != GUEST || row.status == status::REVOKED {
        return Ok(Report::default());
    }
    let ctx = Ctx { state, link: &row, key: key.to_string(), me: super::instance_id(&state.db).await? };
    let (mut out_cursor, mut in_cursor) = (row.out_cursor, row.in_cursor);
    let (mut mine, mut theirs, mut documents) = (Report::default(), Report::default(), 0usize);
    let (mut addr, mut remote_digest, mut caught_up, mut failure) = (row.addr.clone(), None, false, None);
    for _ in 0..MAX_ROUNDS {
        let out = outgoing(&ctx, out_cursor).await?;
        documents += out.documents;
        let req = ExchangeRequest { changes: out.items.clone(), upto: out.upto, after: in_cursor, digest: row.local_digest.clone() };
        let payload = json!({ "op": "exchange", "changes": req.changes, "upto": req.upto, "after": req.after, "digest": req.digest });
        let (v, used) = match call(state, &row, payload).await {
            Ok(r) => r,
            Err(e) => {
                failure = Some(e);
                break;
            }
        };
        addr = Some(used);
        let Ok(resp) = serde_json::from_value::<ExchangeResponse>(v) else {
            failure = Some(CallError::Unreachable("an answer this Eunomia can't read (is the other one a different version?)".into()));
            break;
        };
        if resp.ack >= out.upto {
            mark_sent(&ctx, &out.items).await;
            out_cursor = out_cursor.max(resp.ack);
        }
        theirs.add(&resp.report);
        let host_more = resp.more;
        mine.add(&apply(&ctx, resp.changes).await);
        in_cursor = in_cursor.max(resp.next);
        if !out.more && !host_more {
            remote_digest = resp.digest;
            caught_up = true;
            break;
        }
    }
    let report = json!({ "received": mine, "sent": theirs, "documents_not_shared": documents });
    match failure {
        Some(CallError::Revoked) => {
            let msg = "The other Eunomia stopped sharing this space. Your copy stays here as a normal space.".to_string();
            save_progress(&state.db, &row, (out_cursor, in_cursor), addr, status::REVOKED, Some(msg), mine.rejected, (None, None), report).await?;
        }
        Some(CallError::Unreachable(why)) => {
            tracing::info!(link = %key, why, "peer sync: host unreachable, will retry");
            save_progress(&state.db, &row, (out_cursor, in_cursor), addr, status::UNREACHABLE, Some(why), mine.rejected, (None, None), report).await?;
        }
        None => {
            let local = if caught_up { Some(digest(&state.db, &row.vault).await?.0) } else { None };
            let same = matches!((&local, &remote_digest), (Some(a), Some(b)) if a == b);
            let status = if mine.rejected > 0 || theirs.rejected > 0 {
                status::INTEGRITY_PROBLEM
            } else if same {
                status::UP_TO_DATE
            } else {
                status::SYNCING
            };
            save_progress(&state.db, &row, (out_cursor, in_cursor), addr, status, None, mine.rejected, (local, remote_digest), report).await?;
        }
    }
    Ok(mine)
}

/// Each scheduler tick (`jobs::leader`): sync every guest link not tried for [`SYNC_EVERY`], each in a
/// task of its own so a slow host does not hold up the tick.
pub async fn reconcile(state: &OrgState) {
    let ids: Vec<RecordId> = match q::GUEST_LINKS.on(&state.db).bind(("age", SYNC_EVERY)).await.and_then(|mut r| r.take(0)) {
        Ok(ids) => ids,
        // an org still on the schema before peer links: nothing to do yet
        Err(_) => return,
    };
    for id in ids {
        let Some(key) = crate::rid::key_string(id.key()) else { continue };
        if in_progress(&key) {
            continue;
        }
        let st = state.clone();
        tokio::spawn(authz::as_system(async move {
            if let Err(e) = sync_link(&st, &key).await {
                tracing::warn!(link = %key, error = %e.message, "peer sync failed");
            }
        }));
    }
}

// --- the host side -----------------------------------------------------------------------------

/// Why a peer request was refused before it was understood (status, short reason). Says nothing
/// about the link to a caller that cannot prove it holds the key.
pub type Refusal = (u16, &'static str);

/// `POST /api/peer/v1/{org}/links/{link}`: one sealed request from a guest, one sealed answer.
pub async fn host_rpc(state: &OrgState, org: &str, key: &str, env: &Value) -> Result<Value, Refusal> {
    const NOPE: Refusal = (401, "not authenticated");
    let row = load_link(&state.db, key).await.map_err(|_| (503, "try again"))?.filter(|r| r.direction == HOST).ok_or(NOPE)?;
    let secret = link_secret(&state.settings, &row).map_err(|_| (500, "link key unreadable"))?;
    let (ts, nonce, data) = (env["ts"].as_i64().unwrap_or_default(), s(env, "nonce"), s(env, "data"));
    if s(env, "link") != key || nonce.is_empty() || nonce.len() > 64 {
        return Err(NOPE);
    }
    let now = chrono::Utc::now().timestamp();
    if (now - ts).abs() > crypto::MAX_SKEW_SECS {
        return Err((401, "clock skew: check both computers' clocks"));
    }
    let path = format!("/api/peer/v1/{org}/links/{key}");
    let plain = crypto::open(&secret, &crypto::request_aad("POST", &path, key, ts, nonce), data).ok_or(NOPE)?;
    // only an authentic message reaches the replay set, so nobody else can fill it
    crypto::replay().check(key, nonce, ts, now).map_err(|_| (401, "replayed"))?;
    let req: Value = serde_json::from_slice(&plain).map_err(|_| (400, "bad request"))?;
    let vault_there = super::vault_name(&state.db, &row.vault).await.ok().flatten().is_some();
    let answer = if row.status == status::REVOKED || !vault_there {
        // told once, authentically; then forgotten
        let _ = super::forget(&state.db, &row).await;
        json!({ "revoked": true })
    } else {
        match s(&req, "op") {
            "exchange" => {
                let me = super::instance_id(&state.db).await.map_err(|_| (503, "try again"))?;
                let ctx = Ctx { state, link: &row, key: key.to_string(), me };
                let req: ExchangeRequest = serde_json::from_value(req).map_err(|_| (400, "bad request"))?;
                host_exchange(&ctx, req).await.map_err(|e| {
                    tracing::warn!(link = %key, error = %e.message, "peer exchange failed");
                    (503, "try again")
                })?
            }
            "verify" => {
                let (d, n) = digest(&state.db, &row.vault).await.map_err(|_| (503, "try again"))?;
                json!({ "digest": d, "items": n })
            }
            "leave" => {
                let _ = super::forget(&state.db, &row).await;
                json!({ "left": true })
            }
            _ => return Err((400, "unknown op")),
        }
    };
    let (rts, rnonce) = (chrono::Utc::now().timestamp(), crypto::random_id());
    let sealed = crypto::seal(&secret, &crypto::response_aad(key, nonce, rts, &rnonce), &serde_json::to_vec(&answer).unwrap_or_default());
    Ok(json!({ "ts": rts, "nonce": rnonce, "data": sealed }))
}

/// The host takes the guest's changes (when it may edit), then answers with its own after the guest's
/// cursor, and its digest when that catches the guest up.
pub async fn host_exchange(ctx: &Ctx<'_>, req: ExchangeRequest) -> AppResult<Value> {
    let row = ctx.link;
    let report = if row.role == ROLE_EDIT { apply(ctx, req.changes).await } else { Report { ignored: req.changes.len(), ..Default::default() } };
    let out = outgoing(ctx, req.after).await?;
    mark_sent(ctx, &out.items).await;
    let digest = if out.more { None } else { Some(digest(ctx.db(), ctx.vault()).await?.0) };
    let status = if report.rejected > 0 { status::INTEGRITY_PROBLEM } else { status::UP_TO_DATE };
    let summary = json!({ "received": report, "documents_not_shared": out.documents });
    save_progress(ctx.db(), row, (out.upto, row.in_cursor.max(req.upto)), row.addr.clone(), status, None, report.rejected, (digest.clone(), req.digest.clone()), summary).await?;
    let resp = ExchangeResponse { changes: out.items, next: out.upto, more: out.more, ack: req.upto, digest, report };
    Ok(serde_json::to_value(resp).unwrap_or_default())
}
