//! `entities_neighbourhood`: the entities within a few relations of one entity, in both directions,
//! with their freshest facts and the vault's review of each link type between them. Plain data: the
//! model reading it decides what a change to the entity affects, from the link labels, their
//! descriptions and the facts. Nothing here knows which way an effect travels.

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

#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub id: String,
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Up to three of its most recent live facts.
    pub facts: Vec<String>,
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
    /// More entities were within reach than [`MAX_NODES`].
    pub truncated: bool,
}

fn not_found(id: &RecordId) -> AppError {
    AppError::coded(ErrorCode::EntityNotFound, format!("entity {} not found", id.to_string()))
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

    let mut order: Vec<RecordId> = vec![start.id.clone()];
    // hops from the start, by id
    let mut seen: HashMap<String, u8> = HashMap::from([(start.id.to_string(), 0)]);
    let mut edges: Vec<EdgeRow> = Vec::new();
    let mut edge_ids: HashSet<String> = HashSet::new();
    let mut truncated = false;
    let mut frontier = vec![start.id.clone()];
    for hop in 1..=depth {
        if frontier.is_empty() {
            break;
        }
        let mut res = q::EDGES_TOUCHING.on(db).bind(("ids", frontier)).await?;
        let touching: Vec<EdgeRow> = res.take(q::EDGES_TOUCHING.slot)?;
        let mut next = Vec::new();
        // an edge from before relations were confined to one vault stays hidden
        for e in touching.into_iter().filter(|e| e.from_vault.as_ref() == Some(&vault) && e.to_vault.as_ref() == Some(&vault)) {
            for end in [&e.from, &e.to] {
                if seen.contains_key(&end.to_string()) {
                    continue;
                }
                if seen.len() >= MAX_NODES {
                    truncated = true;
                    continue;
                }
                seen.insert(end.to_string(), hop);
                order.push(end.clone());
                next.push(end.clone());
            }
            if edge_ids.insert(e.id.to_string()) {
                edges.push(e);
            }
        }
        frontier = next;
    }

    let mut rows = load_nodes(db, order.clone()).await?;
    rows.retain(|_, r| r.vault == vault);
    let mut order: Vec<NodeRow> = order.iter().filter_map(|id| rows.remove(&id.to_string())).collect();
    order.sort_by(|a, b| (seen[&a.id.to_string()], &a.name, a.kind()).cmp(&(seen[&b.id.to_string()], &b.name, b.kind())));
    let ids: Vec<RecordId> = order.iter().map(|n| n.id.clone()).collect();
    let mut res = q::RECENT_FACTS.on(db).bind(("ids", ids)).bind(("vault", vault.clone())).await?;
    let facts: Vec<Vec<String>> = res.take(0)?;

    let kinds: HashMap<String, String> = order.iter().map(|n| (n.id.to_string(), n.kind())).collect();
    // an edge to an entity left out (the cap, a deleted one) would point at nothing
    edges.retain(|e| kinds.contains_key(&e.from.to_string()) && kinds.contains_key(&e.to.to_string()));
    let names: HashMap<String, &str> = order.iter().map(|n| (n.id.to_string(), n.name.as_str())).collect();
    let edge_key = |e: &EdgeRow| (names[&e.from.to_string()], e.label.clone(), names[&e.to.to_string()]);
    edges.sort_by_key(|e| edge_key(e));
    let present: BTreeSet<(String, String, String)> =
        edges.iter().map(|e| (kinds[&e.from.to_string()].clone(), e.label.clone(), kinds[&e.to.to_string()].clone())).collect();
    let link_types = crate::ontology::view(db, &vault)
        .await?
        .links
        .into_iter()
        .filter(|l| present.contains(&(l.from_kind.clone(), l.label.clone(), l.to_kind.clone())))
        .map(|l| LinkType { from_kind: l.from_kind, label: l.label, to_kind: l.to_kind, status: l.status, description: l.description })
        .collect();

    let nodes = order
        .into_iter()
        .zip(facts.into_iter().chain(std::iter::repeat_with(Vec::new)))
        .map(|(n, facts)| Node {
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
