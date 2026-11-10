//! `entities_neighbourhood`: the entities within a few relations of one entity, in both directions,
//! with their freshest facts and the vault's review of each link type between them. Plain data: the
//! model reading it decides what a change to the entity affects, from the link labels, their
//! descriptions and the facts. Nothing here knows which way an effect travels.
//!
//! Also the row loaders `views` resolves a `show_view` call with.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use surrealdb::types::{RecordId, SurrealValue};

use super::service::{accessible, require_entity_id};
use crate::authz::Action;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::store;
use crate::store::entities as q;

pub const DEFAULT_DEPTH: u8 = 2;
pub const MAX_DEPTH: u8 = 3;
/// Most entities one neighbourhood returns, the start included.
pub const MAX_NODES: usize = 150;
/// Most relations one neighbourhood returns.
pub const MAX_EDGES: usize = 600;
/// Most relations read from one entity each way, so a hub with thousands of relations costs no more.
pub const MAX_EDGES_PER_ENTITY: usize = 500;

/// An entity as a neighbourhood or a view shows it.
#[derive(Debug, Clone, Deserialize, SurrealValue)]
pub(crate) struct NodeRow {
    pub id: RecordId,
    pub vault: RecordId,
    /// Set on a custom-kind row only; a built-in kind is its table.
    #[serde(default)]
    #[surreal(default)]
    kind: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    pub name: String,
    #[serde(default)]
    #[surreal(default)]
    pub summary: String,
}

impl NodeRow {
    pub fn kind(&self) -> String {
        self.kind.clone().unwrap_or_else(|| self.id.table().to_string())
    }
}

/// One `relates_to` row, in its stored direction.
#[derive(Debug, Clone, Deserialize, SurrealValue)]
pub(crate) struct EdgeRow {
    pub id: RecordId,
    #[serde(rename = "in")]
    #[surreal(rename = "in")]
    pub from: RecordId,
    #[serde(rename = "out")]
    #[surreal(rename = "out")]
    pub to: RecordId,
    #[serde(default)]
    #[surreal(default)]
    pub label: String,
    /// The ends' vaults, when the statement selected them.
    #[serde(default)]
    #[surreal(default)]
    from_vault: Option<RecordId>,
    #[serde(default)]
    #[surreal(default)]
    to_vault: Option<RecordId>,
}

/// The entities among `ids` that exist, by id (any vault: the caller checks access).
pub(crate) async fn load_nodes(db: &OrgDb, ids: Vec<RecordId>) -> AppResult<HashMap<String, NodeRow>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<NodeRow> = q::NODES_BY_ID.on(db).bind(("ids", ids)).await?.take(0)?;
    Ok(rows.into_iter().map(|r| (r.id.to_string(), r)).collect())
}

/// The relations among `ids` that exist, by id (any vault: the caller checks both ends).
pub(crate) async fn load_edges(db: &OrgDb, ids: Vec<RecordId>) -> AppResult<HashMap<String, EdgeRow>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<EdgeRow> = q::RELATIONS_BY_ID.on(db).bind(("ids", ids)).await?.take(0)?;
    Ok(rows.into_iter().map(|r| (r.id.to_string(), r)).collect())
}

#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub id: String,
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Up to three of its most recent live facts.
    pub facts: Vec<String>,
    /// The current value of each approved property it has.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<crate::ontology::property::PropertyValue>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Edge {
    pub id: String,
    pub from: String,
    pub label: String,
    pub to: String,
}

/// A link type present among the edges, as the vault's ontology describes it.
#[derive(Debug, Clone, Serialize)]
pub struct LinkType {
    pub from_kind: String,
    pub label: String,
    pub to_kind: String,
    pub status: crate::ontology::ViewStatus,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Neighbourhood {
    pub vault: String,
    pub depth: u8,
    /// The start entity first, then the others by how many relations away they are, then by name.
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub link_types: Vec<LinkType>,
    /// More entities or relations were within reach than [`MAX_NODES`], [`MAX_EDGES`] or
    /// [`MAX_EDGES_PER_ENTITY`] let through.
    pub truncated: bool,
}

fn not_found(id: &RecordId) -> AppError {
    AppError::coded(ErrorCode::EntityNotFound, format!("entity {} not found", id.to_string()))
}

/// The relations of `id` inside `vault`, both ways, at most [`MAX_EDGES_PER_ENTITY`] each way, and
/// whether more were cut.
pub(crate) async fn relations_of(db: &OrgDb, vault: &RecordId, id: &RecordId) -> AppResult<(Vec<EdgeRow>, bool)> {
    let mut res = q::EDGES_TOUCHING.on(db).bind(("ids", vec![id.clone()])).bind(("limit", MAX_EDGES_PER_ENTITY + 1)).await?;
    let lists: Vec<Vec<EdgeRow>> = res.take(q::EDGES_TOUCHING.slot)?;
    let mut cut = false;
    let mut out = Vec::new();
    for mut list in lists {
        cut |= list.len() > MAX_EDGES_PER_ENTITY;
        list.truncate(MAX_EDGES_PER_ENTITY);
        out.extend(list.into_iter().filter(|e| e.from_vault.as_ref() == Some(vault) && e.to_vault.as_ref() == Some(vault)));
    }
    Ok((out, cut))
}

/// What [`walk`] reached.
pub(crate) struct Walk {
    /// The starts, then each entity in the order it was reached.
    pub order: Vec<RecordId>,
    /// Hops from the nearest start, by id; 0 for a start.
    pub hops: HashMap<String, u8>,
    /// The relation that first reached each entity that is not a start, by id.
    pub via: HashMap<String, EdgeRow>,
    pub edges: Vec<EdgeRow>,
    pub truncated: bool,
}

/// Breadth-first from `starts` over their relations in both directions, `depth` hops, never leaving
/// `vault`. At most [`MAX_NODES`] entities, [`MAX_EDGES`] relations and [`MAX_EDGES_PER_ENTITY`]
/// relations each way from any one entity.
pub(crate) async fn walk(db: &OrgDb, vault: &RecordId, starts: &[RecordId], depth: u8) -> AppResult<Walk> {
    walk_bounded(db, vault, starts, depth, WalkLimits { nodes: MAX_NODES, edges: MAX_EDGES, per_entity: MAX_EDGES_PER_ENTITY }, &[]).await
}

pub(crate) struct WalkLimits {
    pub nodes: usize,
    pub edges: usize,
    pub per_entity: usize,
}

/// A complete selection for paged views. Callers refuse a truncated walk rather than invent a total.
pub(crate) async fn walk_bounded(
    db: &OrgDb, vault: &RecordId, starts: &[RecordId], depth: u8,
    limits: WalkLimits, labels: &[String],
) -> AppResult<Walk> {
    let WalkLimits { nodes: max_nodes, edges: max_edges, per_entity: edge_limit } = limits;
    let mut order: Vec<RecordId> = Vec::new();
    let mut seen: HashMap<String, u8> = HashMap::new();
    for s in starts {
        if seen.insert(s.to_string(), 0).is_none() {
            order.push(s.clone());
        }
    }
    let mut via: HashMap<String, EdgeRow> = HashMap::new();
    let mut edges: Vec<EdgeRow> = Vec::new();
    let mut edge_ids: HashSet<String> = HashSet::new();
    let mut truncated = false;
    let mut frontier = order.clone();
    for hop in 1..=depth {
        if frontier.is_empty() { break; }
        if edges.len() >= max_edges {
            truncated = true;
            break;
        }
        // one more than the cap, to tell a full list from a cut one
        let mut res = q::EDGES_TOUCHING.on(db).bind(("ids", frontier)).bind(("limit", edge_limit + 1)).await?;
        let lists: Vec<Vec<EdgeRow>> = res.take(q::EDGES_TOUCHING.slot)?;
        let mut touching = Vec::new();
        for mut list in lists {
            if list.len() > edge_limit {
                truncated = true;
                list.truncate(edge_limit);
            }
            touching.extend(list);
        }
        let mut next = Vec::new();
        // an edge from before relations were confined to one vault stays hidden
        for e in touching.into_iter().filter(|e| e.from_vault.as_ref() == Some(vault) && e.to_vault.as_ref() == Some(vault)) {
            if (!labels.is_empty() && !labels.contains(&e.label)) || edge_ids.contains(&e.id.to_string()) {
                continue;
            }
            if edges.len() >= max_edges {
                truncated = true;
                break;
            }
            // one end is on the frontier, so at most the other is new
            let new_end = [&e.from, &e.to].into_iter().find(|end| !seen.contains_key(&end.to_string()));
            if let Some(end) = new_end {
                if seen.len() >= max_nodes {
                    truncated = true;
                    continue;
                }
                seen.insert(end.to_string(), hop);
                via.insert(end.to_string(), e.clone());
                order.push(end.clone());
                next.push(end.clone());
            }
            edge_ids.insert(e.id.to_string());
            edges.push(e);
        }
        frontier = next;
    }
    Ok(Walk { order, hops: seen, via, edges, truncated })
}

/// Breadth-first from `entity_id` over its relations in both directions, `depth` hops (default
/// [`DEFAULT_DEPTH`], clamped to 1..=[`MAX_DEPTH`]), never leaving the entity's vault. Read access as
/// for `entities_get`: an entity the caller cannot read is not found. With `vault_id`, the entity must be
/// in that vault.
pub async fn neighbourhood(
    db: &OrgDb,
    owner: &RecordId,
    entity_id: &RecordId,
    depth: Option<i64>,
    vault_id: Option<&RecordId>,
) -> AppResult<Neighbourhood> {
    require_entity_id(entity_id)?;
    let start: NodeRow = store::get(db, entity_id).await?.ok_or_else(|| not_found(entity_id))?;
    if vault_id.is_some_and(|v| v != &start.vault) || !accessible(db, owner, &start.vault, Action::ReadMemories).await? {
        return Err(not_found(entity_id));
    }
    let vault = start.vault.clone();
    let depth = depth.unwrap_or(DEFAULT_DEPTH as i64).clamp(1, MAX_DEPTH as i64) as u8;
    let Walk { order, hops: seen, mut edges, truncated, .. } = walk(db, &vault, std::slice::from_ref(&start.id), depth).await?;

    let mut rows = load_nodes(db, order.clone()).await?;
    rows.retain(|_, r| r.vault == vault);
    let mut order: Vec<NodeRow> = order.iter().filter_map(|id| rows.remove(&id.to_string())).collect();
    order.sort_by(|a, b| (seen[&a.id.to_string()], &a.name, a.kind()).cmp(&(seen[&b.id.to_string()], &b.name, b.kind())));
    let ids: Vec<RecordId> = order.iter().map(|n| n.id.clone()).collect();
    let mut res = q::RECENT_FACTS.on(db).bind(("ids", ids)).bind(("vault", vault.clone())).await?;
    let facts: Vec<Vec<String>> = res.take(0)?;
    let mut values = crate::ontology::property::latest_values(db, &vault, &order.iter().map(|n| n.id.clone()).collect::<Vec<_>>()).await?;

    let kinds: HashMap<String, String> = order.iter().map(|n| (n.id.to_string(), n.kind())).collect();
    // an edge to an entity left out (the cap, a deleted one) would point at nothing
    edges.retain(|e| kinds.contains_key(&e.from.to_string()) && kinds.contains_key(&e.to.to_string()));
    let names: HashMap<String, &str> = order.iter().map(|n| (n.id.to_string(), n.name.as_str())).collect();
    let edge_key = |e: &EdgeRow| (names[&e.from.to_string()], e.label.clone(), names[&e.to.to_string()]);
    edges.sort_by_key(|e| edge_key(e));
    let present: BTreeSet<(String, String, String)> =
        edges.iter().map(|e| (kinds[&e.from.to_string()].clone(), e.label.clone(), kinds[&e.to.to_string()].clone())).collect();
    let link_types = crate::ontology::link_types(db, &vault, &present)
        .await?
        .into_iter()
        .map(|l| LinkType { from_kind: l.from_kind, label: l.label, to_kind: l.to_kind, status: l.status, description: l.description })
        .collect();

    let nodes = order
        .into_iter()
        .zip(facts.into_iter().chain(std::iter::repeat_with(Vec::new)))
        .map(|(n, facts)| Node {
            properties: values.remove(&n.id.to_string()).unwrap_or_default(),
            id: n.id.to_string(),
            kind: n.kind(),
            summary: Some(n.summary.clone()).filter(|s| !s.trim().is_empty()),
            name: n.name,
            facts,
        })
        .collect();
    let edges = edges
        .into_iter()
        .map(|e| Edge { id: e.id.to_string(), from: e.from.to_string(), label: e.label, to: e.to.to_string() })
        .collect();
    Ok(Neighbourhood { vault: vault.to_string(), depth, nodes, edges, link_types, truncated })
}
