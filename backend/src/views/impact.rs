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
    /// Zero-based. Re-evaluated on each request, so later pages reflect current saved links.
    pub page: usize,
    pub page_size: usize,
    pub total: usize,
    pub has_more: bool,
    /// A partial page, never an incomplete total. Incomplete traversals fail explicitly.
    pub truncated: bool,
}

pub async fn resolve(db: &OrgDb, owner: &RecordId, start_id: &str, headline: &str, reach: ImpactReach, page: usize) -> AppResult<View> {
    if !(1..=neighbourhood::MAX_DEPTH).contains(&reach.depth) || reach.kinds.len() > 50 || reach.labels.len() > 50 {
        return Err(AppError::bad_request("reach needs depth 1 to 3, at most 50 kinds and 50 labels"));
    }
    let graph = Resolved::load(db, owner, std::iter::once(start_id), std::iter::empty()).await?;
    let start = graph.entity(start_id)?;
    let vault = crate::rid::parse(&start.vault).map_err(|_| AppError::bad_request("invalid vault"))?;
    let id = crate::rid::parse(start_id).map_err(|_| AppError::bad_request("invalid start"))?;
    let walk = neighbourhood::walk_bounded(db, &vault, &[id], reach.depth, neighbourhood::WalkLimits { nodes: 10_001, edges: 40_000, per_entity: 40_000 }, &reach.labels).await?;
    if walk.truncated {
        return Err(AppError::bad_request("impact selection exceeds traversal limits; narrow depth or labels. No complete total is available"));
    }
    let nodes = neighbourhood::load_nodes(db, walk.order.clone()).await?;
    let mut selected: Vec<_> = nodes.values().filter(|n| n.id.to_string() != start_id && (reach.kinds.is_empty() || reach.kinds.contains(&n.kind()))).collect();
    selected.sort_by(|a, b| (&a.name, a.id.to_string()).cmp(&(&b.name, b.id.to_string())));
    let total = selected.len();
    let offset = page.checked_mul(PAGE_SIZE).ok_or_else(|| AppError::bad_request("page is too large"))?;
    if offset >= total && page != 0 { return Err(AppError::bad_request("page is past the end of this selection")); }
    let mut affected = Vec::new();
    for node in selected.into_iter().skip(offset).take(PAGE_SIZE) {
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
    let has_more = offset + affected.len() < total;
    let pagination = ImpactPage { reach, page, page_size: PAGE_SIZE, total, has_more, truncated: offset > 0 || has_more };
    // Use the same checked tree as chat. Only server data can authorise headline numbers.
    let mut prose = super::ui::prose::ChatProse::default();
    prose.result(&serde_json::json!({"start":start,"affected":affected,"total":total}));
    let headline = crate::chat::tree::plain_text(&prose.safe_tree(&clean("headline", headline, MAX_HEADLINE)?));
    Ok(View::Impact { start, headline, affected, linked, pagination: Some(pagination) })
}
fn entity(n: &neighbourhood::NodeRow) -> EntityRef {
    EntityRef { id: n.id.to_string(), name: n.name.clone(), kind: n.kind(), vault: n.vault.to_string() }
}

pub fn schema() -> serde_json::Value {
    serde_json::json!({"type":"object","additionalProperties":false,"properties":{
        "depth":{"type":"integer","minimum":1,"maximum":3,"default":2},
        "kinds":{"type":"array","maxItems":50,"items":{"type":"string"},"description":"Endpoint kinds to include; empty means all kinds"},
        "labels":{"type":"array","maxItems":50,"items":{"type":"string"},"description":"Relation labels to follow in either direction; empty means all labels"}
    },"description":"Server-resolved linked items, not a prediction of causality. Use instead of affected for complete counts and paging. Returns 16 items per page. Narrow kinds and labels using the ontology."})
}
