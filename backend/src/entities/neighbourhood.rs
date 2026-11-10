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

#[cfg(feature = "test-support")]
tokio::task_local! {
    /// Actual edge rows materialised by this task, before filtering or deduplication.
    pub static FETCHED_EDGE_ROWS: std::cell::Cell<usize>;
    /// Largest diagram query result, including its lookahead row.
    pub static LARGEST_DIAGRAM_QUERY: std::cell::Cell<usize>;
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
    walk_bounded(db, vault, starts, depth, WalkLimits { nodes: MAX_NODES, edges: MAX_EDGES, fetched_rows: MAX_EDGES * 4, per_entity: MAX_EDGES_PER_ENTITY }, &[]).await
}

pub(crate) struct WalkLimits {
    pub nodes: usize,
    pub edges: usize,
    pub per_entity: usize,
    /// Includes duplicate, cross-vault and label-filtered rows from both query directions.
    /// One extra lookahead row may be read to prove that work was cut.
    pub fetched_rows: usize,
}

/// A bounded selection with separate caps for fetched rows and unique output edges.
pub(crate) async fn walk_bounded(
    db: &OrgDb, vault: &RecordId, starts: &[RecordId], depth: u8,
    limits: WalkLimits, labels: &[String],
) -> AppResult<Walk> {
    let WalkLimits { nodes: max_nodes, edges: max_edges, per_entity: edge_limit, fetched_rows: max_fetch } = limits;
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
    let mut fetched = 0;
    'walk: for hop in 1..=depth {
        if frontier.is_empty() { break; }
        let mut next = Vec::new();
        // Allocate against actual rows fetched, not an equal share of a batch. An empty
        // direction spends nothing, leaving its whole allowance for the next direction.
        for id in &frontier {
            for outgoing in [true, false] {
                let remaining = max_fetch.saturating_sub(fetched);
                // One lookahead row distinguishes an exactly full budget from omitted work.
                // When the budget is spent, empty directions still prove completeness.
                let limit = remaining.saturating_add(1).min(edge_limit.saturating_add(1));
                let mut res = q::EDGES_IN_DIRECTION.on(db).bind(("id", id.clone()))
                    .bind(("outgoing", outgoing)).bind(("limit", limit)).await?;
                let mut list: Vec<EdgeRow> = res.take(0)?;
                let count = list.len();
                fetched += count;
                #[cfg(feature = "test-support")]
                let _ = FETCHED_EDGE_ROWS.try_with(|n| n.set(n.get() + count));
                let budget_cut = count > remaining;
                truncated |= count > edge_limit || budget_cut;
                list.truncate(edge_limit.min(remaining));
                for e in list.into_iter().filter(|e| e.from_vault.as_ref() == Some(vault) && e.to_vault.as_ref() == Some(vault)) {
                    if (!labels.is_empty() && !labels.contains(&e.label)) || edge_ids.contains(&e.id.to_string()) { continue; }
                    if edges.len() >= max_edges { truncated = true; break 'walk; }
                    let new_end = [&e.from, &e.to].into_iter().find(|end| !seen.contains_key(&end.to_string()));
                    if let Some(end) = new_end {
                        if seen.len() >= max_nodes { truncated = true; continue; }
                        seen.insert(end.to_string(), hop);
                        via.insert(end.to_string(), e.clone());
                        order.push(end.clone());
                        next.push(end.clone());
                    }
                    edge_ids.insert(e.id.to_string());
                    edges.push(e);
                }
                if budget_cut { break 'walk; }
            }
        }
        frontier = next;
    }
    Ok(Walk { order, hops: seen, via, edges, truncated })
}

/// Exercise the real traversal with a small row budget in embedded integration tests.
#[cfg(feature = "test-support")]
pub async fn walk_fetch_budget_for_test(db: &OrgDb, vault: &RecordId, start: RecordId, budget: usize) -> AppResult<(usize, bool)> {
    let walk = walk_bounded(db, vault, &[start], 1, WalkLimits { nodes: 150, edges: 600, per_entity: 500, fetched_rows: budget }, &[]).await?;
    Ok((walk.edges.len(), walk.truncated))
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
/// Diagram selection only needs names and stored links, so skip facts, properties and ontology
/// hydration. Each indexed edge read returns at most 32 rows plus one lookahead; each direct-ID
/// node read returns at most 32 rows. Only one query is in flight. If the waiter times out, at
/// most that one bounded query can continue, and its server-side TIMEOUT is one second.
/// Hub pages have at most 500 skipped index entries, matching the existing per-entity cap.
pub(crate) async fn diagram_search(
    db: &OrgDb, vault: &RecordId, start: Node, depth: u8, deadline: tokio::time::Instant,
) -> AppResult<(Neighbourhood, bool)> {
    const BATCH: usize = 32;
    const FETCH_BUDGET: usize = MAX_EDGES * 4;
    let start_id = crate::rid::parse(&start.id).map_err(|_| not_found(vault))?;
    let mut order = vec![start_id];
    let mut seen = HashSet::from([start.id.clone()]);
    let mut edge_ids = HashSet::new();
    let mut edges = Vec::new();
    let mut frontier = order.clone();
    let mut fetched = 0;
    let mut truncated = false;
    let mut expired = false;
    'walk: for _ in 0..depth {
        let mut next = Vec::new();
        for id in &frontier {
            for outgoing in [true, false] {
              let mut offset = 0;
              loop {
                if tokio::time::Instant::now() >= deadline { expired = true; break 'walk; }
                let remaining = FETCH_BUDGET - fetched;
                if remaining == 0 || edges.len() >= MAX_EDGES { truncated = true; break 'walk; }
                let page = BATCH.min(remaining).min(MAX_EDGES_PER_ENTITY - offset);
                let limit = page + 1;
                let query = q::DIAGRAM_EDGES.on(db).until(deadline).bind(("id", id.clone()))
                    .bind(("outgoing", outgoing)).bind(("limit", limit)).bind(("start", offset));
                let mut response = match tokio::time::timeout_at(deadline, query).await {
                    Ok(Err(_)) if tokio::time::Instant::now() >= deadline => { expired = true; break 'walk; }
                    Ok(result) => result?,
                    Err(_) => { expired = true; break 'walk; }
                };
                let mut rows: Vec<EdgeRow> = response.take(0)?;
                let count = rows.len();
                #[cfg(feature = "test-support")]
                let _ = LARGEST_DIAGRAM_QUERY.try_with(|n| n.set(n.get().max(count)));
                #[cfg(feature = "test-support")]
                let _ = FETCHED_EDGE_ROWS.try_with(|n| n.set(n.get() + count));
                let more = count > page;
                fetched += count.min(remaining);
                rows.truncate(page);
                for e in rows {
                    if e.from_vault.as_ref() != Some(vault) || e.to_vault.as_ref() != Some(vault)
                        || edge_ids.contains(&e.id.to_string()) { continue; }
                    if edges.len() == MAX_EDGES { truncated = true; break 'walk; }
                    let new_end = [&e.from, &e.to].into_iter().find(|end| !seen.contains(&end.to_string()));
                    if let Some(end) = new_end {
                        if seen.len() == MAX_NODES { truncated = true; continue; }
                        seen.insert(end.to_string());
                        order.push(end.clone());
                        next.push(end.clone());
                    }
                    edge_ids.insert(e.id.to_string());
                    edges.push(e);
                }
                offset += page;
                if !more { break; }
                if offset == MAX_EDGES_PER_ENTITY { truncated = true; break; }
              }
            }
        }
        frontier = next;
        if frontier.is_empty() { break; }
    }
    let mut nodes = vec![start];
    for ids in order[1..].chunks(BATCH) {
        if expired || tokio::time::Instant::now() >= deadline { expired = true; break; }
        let query = q::DIAGRAM_NODES.on(db).until(deadline).bind(("ids", ids.to_vec())).bind(("limit", ids.len()));
        let mut response = match tokio::time::timeout_at(deadline, query).await {
            Ok(Err(_)) if tokio::time::Instant::now() >= deadline => { expired = true; break; }
            Ok(result) => result?,
            Err(_) => { expired = true; break; }
        };
        let rows: Vec<NodeRow> = response.take(0)?;
        #[cfg(feature = "test-support")]
        let _ = LARGEST_DIAGRAM_QUERY.try_with(|n| n.set(n.get().max(rows.len())));
        nodes.extend(rows.into_iter().filter(|n| &n.vault == vault).map(|n| Node {
            id: n.id.to_string(), name: n.name.clone(), kind: n.kind(),
            summary: None, facts: vec![], properties: vec![],
        }));
    }
    let ids: HashSet<_> = nodes.iter().map(|n| n.id.as_str()).collect();
    let edges = edges.into_iter().filter(|e| ids.contains(e.from.to_string().as_str()) && ids.contains(e.to.to_string().as_str()))
        .map(|e| Edge { id: e.id.to_string(), from: e.from.to_string(), to: e.to.to_string(), label: e.label }).collect();
    Ok((Neighbourhood { vault: vault.to_string(), depth, nodes, edges, link_types: vec![], truncated: truncated || expired }, expired))
}
