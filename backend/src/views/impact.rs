//! Server-resolved, paged graph selections. No business-specific meaning is assigned to a link.
use serde::{Deserialize, Serialize};
use surrealdb::types::RecordId;
use crate::{entities::neighbourhood, error::{AppError, AppResult}, pool::OrgDb};
use crate::rid::RecordIdExt;
use super::{Affected, EntityRef, Resolved, View, clean, has_relation, MAX_HEADLINE};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ImpactReach {
    #[serde(default = "default_depth")]
    pub depth: u8,
    /// Select only these endpoint kinds. Intermediate entities remain in paths.
    #[serde(default)]
    pub kinds: Vec<String>,
    /// Follow only these relation labels, in either stored direction. Empty means all labels.
    #[serde(default)]
    pub labels: Vec<String>,
}
fn default_depth() -> u8 { 2 }

pub const PAGE_SIZE: usize = 16;
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ImpactPage {
    pub reach: ImpactReach,
    /// Zero-based page ordinal carried by the cursor.
    pub page: usize,
    pub page_size: usize,
    pub total: usize,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    /// True when the total is a lower bound because traversal reached a resource limit.
    pub total_partial: bool,
    /// This page does not contain the whole selection.
    pub truncated: bool,
}

pub async fn resolve(db: &OrgDb, owner: &RecordId, start_id: &str, headline: &str, reach: ImpactReach, page: usize, cursor: Option<&str>) -> AppResult<View> {
    let (start, id) = selection_start(db, owner, start_id, &reach).await?;
    let scope = super::cursor::scope(&(start.id.as_str(), &reach));
    let boundary = super::cursor::Cursor::read(cursor, &scope)?;
    if (boundary.is_none() && page != 0) || boundary.as_ref().is_some_and(|c| page != 0 && page != c.page) {
        return Err(AppError::bad_request("later pages require the returned next_cursor as cursor; page alone is not a boundary"));
    }
    let page = boundary.as_ref().map_or(0, |c| c.page);
    let (walk, nodes) = selection_walk(db, &start, &id, &reach).await?;
    let mut selected: Vec<_> = nodes.values().filter(|n| n.id != id && (reach.kinds.is_empty() || reach.kinds.contains(&n.kind()))).collect();
    // IDs do not change when a name or a shortest path changes between requests.
    selected.sort_by_key(|n| n.id.to_string());
    let total = selected.len();
    let remaining: Vec<_> = selected.into_iter().filter(|n| boundary.as_ref().is_none_or(|c| n.id.to_string() > c.key)).collect();
    let has_more = remaining.len() > PAGE_SIZE;
    let next_cursor = if has_more { remaining.get(PAGE_SIZE - 1).map(|n| super::cursor::Cursor::next(n.id.to_string(), page + 1, scope)) } else { None };
    let mut affected = Vec::new();
    for node in remaining.into_iter().take(PAGE_SIZE) {
        let mut at = node.id.to_string();
        let mut path = Vec::new();
        while let Some(edge) = walk.via.get(&at) {
            let from = edge.from.to_string();
            let to = edge.to.to_string();
            let from_node = nodes.get(&from).ok_or_else(|| AppError::bad_request("links changed while reading; fetch the selection again"))?;
            let to_node = nodes.get(&to).ok_or_else(|| AppError::bad_request("links changed while reading; fetch the selection again"))?;
            path.push(super::Step { id: edge.id.to_string(), from: entity(from_node), label: edge.label.clone(), to: entity(to_node) });
            at = if from == at { to } else { from };
        }
        path.reverse();
        affected.push(Affected { entity: entity(node), path, why: String::new() });
    }
    let linked = !affected.is_empty() || has_relation(db, &start).await?;
    let pagination = ImpactPage { reach, page, page_size: PAGE_SIZE, total, has_more, next_cursor, total_partial: walk.truncated, truncated: page > 0 || has_more || walk.truncated };
    // Use the same checked tree as chat. Only server data can authorise headline numbers.
    let mut prose = super::ui::prose::ChatProse::default();
    prose.result(&serde_json::json!({"start":start,"affected":affected,"total":total}));
    let headline = crate::chat::tree::plain_text(&prose.safe_tree(&clean("headline", headline, MAX_HEADLINE)?));
    Ok(View::Impact { start, headline, affected, linked, pagination: Some(pagination) })
}
pub(crate) async fn selection(db: &OrgDb, owner: &RecordId, start_id: &str, reach: &ImpactReach) -> AppResult<(EntityRef, RecordId, neighbourhood::Walk, std::collections::HashMap<String, neighbourhood::NodeRow>)> {
    let (start, id) = selection_start(db, owner, start_id, reach).await?;
    let (walk, nodes) = selection_walk(db, &start, &id, reach).await?;
    Ok((start, id, walk, nodes))
}
async fn selection_start(db: &OrgDb, owner: &RecordId, start_id: &str, reach: &ImpactReach) -> AppResult<(EntityRef, RecordId)> {
    if !(1..=neighbourhood::MAX_DEPTH).contains(&reach.depth) || reach.kinds.len() > 50 || reach.labels.len() > 50 {
        return Err(AppError::bad_request("reach needs depth 1 to 3, at most 50 kinds and 50 labels"));
    }
    let graph = Resolved::load(db, owner, std::iter::once(start_id), std::iter::empty()).await?;
    let start = graph.entity(start_id)?;
    let id = crate::rid::parse(start_id).map_err(|_| AppError::bad_request("invalid start"))?;
    Ok((start, id))
}
async fn selection_walk(db: &OrgDb, start: &EntityRef, id: &RecordId, reach: &ImpactReach) -> AppResult<(neighbourhood::Walk, std::collections::HashMap<String, neighbourhood::NodeRow>)> {
    let vault = crate::rid::parse(&start.vault).map_err(|_| AppError::bad_request("invalid vault"))?;
    let walk = neighbourhood::walk_bounded(db, &vault, std::slice::from_ref(id), reach.depth, neighbourhood::WalkLimits { nodes: 10_001, edges: 40_000, fetched_rows: 40_000, per_entity: 40_000 }, &reach.labels).await?;
    let nodes = neighbourhood::load_nodes(db, walk.order.clone()).await?;
    Ok((walk, nodes))
}
fn entity(n: &neighbourhood::NodeRow) -> EntityRef {
    EntityRef { id: n.id.to_string(), name: n.name.clone(), kind: n.kind(), vault: n.vault.to_string() }
}

pub fn schema() -> serde_json::Value {
    serde_json::json!({"type":"object","additionalProperties":false,"properties":{
        "depth":{"type":"integer","minimum":1,"maximum":3,"default":2},
        "kinds":{"type":"array","maxItems":50,"items":{"type":"string"},"description":"Endpoint kinds to include; empty means all kinds"},
        "labels":{"type":"array","maxItems":50,"items":{"type":"string"},"description":"Relation labels to follow in either direction; empty means all labels"}
    },"description":"Server-resolved linked items, not a prediction of causality. Use instead of affected for counts and cursor paging in immutable record-ID order. Returns 16 items and next_cursor. Repeat the same selection with cursor set to next_cursor. Totals with total_partial are lower bounds: say at least N. Items added after the first page may not appear. Narrow kinds and labels using the ontology."})
}
