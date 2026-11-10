//! Intent-only diagrams. All displayed data and layout are resolved inside one authorised vault.
use super::{EntityRef, Resolved};
use crate::rid::RecordIdExt;
use crate::{
    entities::neighbourhood,
    error::{AppError, AppResult},
    ontology,
    pool::OrgDb,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use surrealdb::types::RecordId;

pub const MAX_NODES: usize = 12;
pub const MAX_EDGES: usize = 24;
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hops {
    One,
    #[default]
    Two,
    Three,
}
impl Hops {
    fn depth(self) -> u8 {
        match self {
            Self::One => 1,
            Self::Two => 2,
            Self::Three => 3,
        }
    }
}
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Both,
    In,
    Out,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Annotation {
    pub entity_id: String,
    pub metric: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Selection {
    Neighbourhood {
        focus_id: String,
        #[serde(default)]
        direction: Direction,
        #[serde(default)]
        hops: Hops,
    },
    Path {
        from_id: String,
        to_id: String,
        #[serde(default)]
        direction: Direction,
        #[serde(default)]
        hops: Hops,
    },
    // Same selection and total as the existing impact view, in both stored directions.
    Impact {
        focus_id: String,
        #[serde(default)]
        hops: Hops,
    },
    Kinds {
        vault_id: String,
        #[serde(default)]
        kind: Option<String>,
    },
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub selection: Selection,
    #[serde(default)]
    pub emphasis: Vec<String>,
    #[serde(default)]
    pub annotations: Vec<Annotation>,
}
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Node {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub focus: bool,
    pub emphasis: bool,
    pub x: usize,
    pub y: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
    pub metrics: Vec<serde_json::Value>,
}
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Edge {
    pub id: String,
    pub from: String,
    pub to: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
}
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Group {
    pub kind: String,
    pub count: usize,
}
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Diagram {
    pub diagram_type: String,
    pub vault: String,
    pub title: String,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub groups: Vec<Group>,
    pub total_nodes: usize,
    pub total_edges: usize,
    pub omitted_edges: usize,
    pub partial: bool,
    pub total_partial: bool,
    pub height: usize,
    pub notice: Option<String>,
}
fn bad(message: impl Into<String>) -> AppError {
    AppError::bad_request(message)
}
fn node(e: EntityRef, focus: bool) -> Node {
    Node {
        id: e.id,
        name: e.name,
        kind: e.kind,
        focus,
        emphasis: false,
        x: 0,
        y: 0,
        count: None,
        metrics: vec![],
    }
}

pub async fn resolve(db: &OrgDb, owner: &RecordId, request: Request) -> AppResult<Diagram> {
    if request.emphasis.len() > MAX_NODES || request.annotations.len() > 4 {
        return Err(bad(
            "diagram supports at most twelve emphasis references and four metric references",
        ));
    }
    let mut out = match request.selection {
        Selection::Kinds { vault_id, kind } => {
            let vault = crate::rid::parse(&vault_id).map_err(|_| bad("invalid vault reference"))?;
            let o = ontology::get(db, owner, Some(&vault), None).await?;
            if kind
                .as_ref()
                .is_some_and(|k| !o.kinds.iter().any(|n| &n.kind == k))
            {
                return Err(bad("unknown ontology kind"));
            }
            let links: Vec<_> = o
                .links
                .into_iter()
                .filter(|e| {
                    kind.as_ref()
                        .is_none_or(|k| &e.from_kind == k || &e.to_kind == k)
                })
                .collect();
            let mut kinds: BTreeSet<_> = links
                .iter()
                .flat_map(|e| [e.from_kind.clone(), e.to_kind.clone()])
                .collect();
            if let Some(k) = &kind {
                kinds.insert(k.clone());
            }
            let nodes = o
                .kinds
                .into_iter()
                .filter(|n| kind.is_none() || kinds.contains(&n.kind))
                .map(|n| Node {
                    id: n.kind.clone(),
                    name: n.kind.replace('_', " "),
                    focus: kind.as_ref() == Some(&n.kind),
                    kind: n.kind,
                    count: Some(n.entities),
                    emphasis: false,
                    x: 0,
                    y: 0,
                    metrics: vec![],
                })
                .collect();
            let edges = links
                .into_iter()
                .map(|e| Edge {
                    id: serde_json::to_string(&(&e.from_kind, &e.label, &e.to_kind))
                        .unwrap_or_default(),
                    from: e.from_kind,
                    to: e.to_kind,
                    label: e.label,
                    count: Some(e.count),
                })
                .collect();
            Diagram::new(
                "kinds",
                o.vault,
                "Business kinds".into(),
                nodes,
                edges,
                false,
            )
        }
        Selection::Impact { focus_id, hops } => {
            let (start, _, walk, rows) = super::impact::selection(
                db,
                owner,
                &focus_id,
                &super::ImpactReach {
                    depth: hops.depth(),
                    kinds: vec![],
                    labels: vec![],
                },
            )
            .await?;
            let nodes = rows
                .values()
                .filter(|n| n.vault.to_string() == start.vault)
                .map(|n| {
                    node(
                        EntityRef {
                            id: n.id.to_string(),
                            name: n.name.clone(),
                            kind: n.kind(),
                            vault: start.vault.clone(),
                        },
                        n.id.to_string() == start.id,
                    )
                })
                .collect();
            let edges = walk
                .edges
                .into_iter()
                .map(|e| Edge {
                    id: e.id.to_string(),
                    from: e.from.to_string(),
                    to: e.to.to_string(),
                    label: e.label,
                    count: None,
                })
                .collect();
            let mut out = Diagram::new(
                "impact",
                start.vault,
                format!("Links reached from {}", start.name),
                nodes,
                edges,
                walk.truncated,
            );
            out.notice = Some(format!(
                "{}{} linked items. Reachability follows links in both directions; it does not establish causality.",
                if walk.truncated { "At least " } else { "" },
                out.total_nodes.saturating_sub(1)
            ));
            out
        }
        selection => {
            let (focus, target, direction, hops) = match selection {
                Selection::Neighbourhood {
                    focus_id,
                    direction,
                    hops,
                } => (focus_id, None, direction, hops),
                Selection::Path {
                    from_id,
                    to_id,
                    direction,
                    hops,
                } => (from_id, Some(to_id), direction, hops),
                _ => unreachable!(),
            };
            let refs = Resolved::load(
                db,
                owner,
                std::iter::once(focus.as_str()).chain(target.as_deref()),
                std::iter::empty(),
            )
            .await?;
            let start = refs.entity(&focus)?;
            let target = target.map(|t| refs.entity(&t)).transpose()?;
            if target.as_ref().is_some_and(|e| e.vault != start.vault) {
                return Err(bad("diagram references must share one vault"));
            }
            let id = crate::rid::parse(&start.id).map_err(|_| bad("invalid focus reference"))?;
            let g = neighbourhood::neighbourhood(db, owner, &id, Some(hops.depth().into()), None)
                .await?;
            let (dist, predecessors) = shortest(&start.id, &g.edges, direction, hops.depth());
            let mut ids: HashSet<_> = dist.keys().cloned().collect();
            let mut path_edges = HashSet::new();
            let mut notice = None;
            if let Some(t) = &target {
                ids = HashSet::from([start.id.clone(), t.id.clone()]);
                let mut queue = VecDeque::from([t.id.clone()]);
                while let Some(at) = queue.pop_front() {
                    for (prev, edge) in predecessors.get(&at).into_iter().flatten() {
                        path_edges.insert(edge.clone());
                        if ids.insert(prev.clone()) {
                            queue.push_back(prev.clone());
                        }
                    }
                }
                if !dist.contains_key(&t.id) {
                    notice = Some(format!(
                        "No path found within {} {}. This does not mean no relationship exists.{}",
                        hops.depth(),
                        if hops.depth() == 1 { "hop" } else { "hops" },
                        if g.truncated {
                            " The search reached its resource limit."
                        } else {
                            ""
                        }
                    ));
                }
            }
            let mut nodes: Vec<_> = g
                .nodes
                .into_iter()
                .filter(|n| ids.contains(&n.id))
                .map(|n| {
                    node(
                        EntityRef {
                            id: n.id,
                            name: n.name,
                            kind: n.kind,
                            vault: g.vault.clone(),
                        },
                        false,
                    )
                })
                .collect();
            // An authorised but unreached destination is still labelled from the server.
            if let Some(t) = &target
                && !nodes.iter().any(|n| n.id == t.id)
            {
                nodes.push(node(t.clone(), false));
            }
            for n in &mut nodes {
                n.focus = n.id == start.id || target.as_ref().is_some_and(|t| n.id == t.id);
            }
            let edges = g
                .edges
                .into_iter()
                .filter(|e| {
                    ids.contains(&e.from)
                        && ids.contains(&e.to)
                        && (target.is_none() || path_edges.contains(&e.id))
                })
                .map(|e| Edge {
                    id: e.id,
                    from: e.from,
                    to: e.to,
                    label: e.label,
                    count: None,
                })
                .collect();
            let mut out = Diagram::new(
                if target.is_some() {
                    "path"
                } else {
                    "neighbourhood"
                },
                g.vault,
                if let Some(t) = &target {
                    format!("{} to {}", start.name, t.name)
                } else {
                    format!("Around {}", start.name)
                },
                nodes,
                edges,
                g.truncated,
            );
            out.notice = notice;
            out
        }
    };
    for id in &request.emphasis {
        let Some(n) = out.nodes.iter_mut().find(|n| &n.id == id) else {
            return Err(bad("emphasis reference is outside the diagram selection"));
        };
        n.emphasis = true;
    }
    // Resolve annotation references before capping, so invalid references never silently disappear.
    for a in request.annotations {
        if out.diagram_type == "kinds" {
            return Err(bad("metric annotations require an entity diagram"));
        }
        let Some(n) = out.nodes.iter_mut().find(|n| n.id == a.entity_id) else {
            return Err(bad("annotation reference is outside the diagram selection"));
        };
        let vault = crate::rid::parse(&out.vault).map_err(|_| bad("invalid vault"))?;
        let id = crate::rid::parse(&n.id).map_err(|_| bad("invalid annotation reference"))?;
        let args = ontology::metric::EvalArgs {
            metric: a.metric,
            kind: Some(n.kind.clone()),
            vault_id: Some(out.vault.clone()),
            ..Default::default()
        };
        let m = ontology::metric::eval_object(db, &vault, &args, &id).await?;
        if m["status"] != "approved" {
            return Err(bad("diagram metric must be approved"));
        }
        n.emphasis = true;
        n.metrics.push(serde_json::json!({"metric":m["metric"],"value":m["value"],"unit":m["unit"],"n_entities":m["n_entities"]}));
    }
    out.finish();
    Ok(out)
}

type Predecessors = HashMap<String, Vec<(String, String)>>;
fn shortest(
    start: &str,
    edges: &[neighbourhood::Edge],
    direction: Direction,
    depth: u8,
) -> (HashMap<String, u8>, Predecessors) {
    let mut dist = HashMap::from([(start.to_string(), 0)]);
    let mut prev: Predecessors = HashMap::new();
    let mut queue = VecDeque::from([start.to_string()]);
    while let Some(at) = queue.pop_front() {
        let hop = dist[&at] + 1;
        if hop > depth {
            continue;
        }
        for e in edges {
            let next = if e.from == at && direction != Direction::In {
                Some(&e.to)
            } else if e.to == at && direction != Direction::Out {
                Some(&e.from)
            } else {
                None
            };
            let Some(next) = next else {
                continue;
            };
            if !dist.contains_key(next) {
                dist.insert(next.clone(), hop);
                queue.push_back(next.clone());
            }
            if dist[next] == hop {
                prev.entry(next.clone())
                    .or_default()
                    .push((at.clone(), e.id.clone()));
            }
        }
    }
    (dist, prev)
}
impl Diagram {
    fn new(
        t: &str,
        vault: String,
        title: String,
        nodes: Vec<Node>,
        edges: Vec<Edge>,
        partial: bool,
    ) -> Self {
        Self {
            diagram_type: t.into(),
            vault,
            title,
            total_nodes: nodes.len(),
            total_edges: edges.len(),
            nodes,
            edges,
            groups: vec![],
            omitted_edges: 0,
            partial,
            total_partial: partial,
            height: 0,
            notice: None,
        }
    }
    fn finish(&mut self) {
        self.nodes.sort_by(|a, b| {
            (!a.focus, !a.emphasis, &a.kind, &a.name, &a.id).cmp(&(
                !b.focus,
                !b.emphasis,
                &b.kind,
                &b.name,
                &b.id,
            ))
        });
        self.nodes.dedup_by(|a, b| a.id == b.id);
        let mut groups = BTreeMap::new();
        for n in self.nodes.iter().skip(MAX_NODES) {
            *groups.entry(n.kind.clone()).or_insert(0) += 1;
        }
        self.groups = groups
            .into_iter()
            .map(|(kind, count)| Group { kind, count })
            .collect();
        self.nodes.truncate(MAX_NODES);
        let ids: HashSet<_> = self.nodes.iter().map(|n| n.id.as_str()).collect();
        self.edges.sort_by(|a, b| {
            (&a.from, &a.to, &a.label, &a.id).cmp(&(&b.from, &b.to, &b.label, &b.id))
        });
        self.edges
            .retain(|e| ids.contains(e.from.as_str()) && ids.contains(e.to.as_str()));
        self.edges.truncate(MAX_EDGES);
        self.omitted_edges = self.total_edges.saturating_sub(self.edges.len());
        self.partial |= !self.groups.is_empty() || self.omitted_edges > 0;
        let row_height = 150
            + 18 * self
                .nodes
                .iter()
                .map(|n| n.metrics.len())
                .max()
                .unwrap_or(0);
        for (i, n) in self.nodes.iter_mut().enumerate() {
            n.x = if i % 2 == 0 { 80 } else { 240 };
            n.y = 70 + (i / 2) * row_height;
        }
        self.height = row_height * self.nodes.len().div_ceil(2).max(1);
    }
    pub fn text(&self) -> String {
        let mut text = format!(
            "{}\n{}{} objects or kinds; {} links in the resolved selection.{}\n",
            self.title,
            if self.total_partial { "At least " } else { "" },
            self.total_nodes,
            self.total_edges,
            if self.partial {
                " Partial diagram."
            } else {
                ""
            }
        );
        if let Some(n) = &self.notice {
            text.push_str(n);
            text.push('\n');
        }
        for n in &self.nodes {
            text.push_str(&format!("- {} ({})", n.name, n.kind.replace('_', " ")));
            if let Some(c) = n.count {
                text.push_str(&format!(": {c} objects"));
            }
            for m in &n.metrics {
                text.push_str(&format!(
                    "; {}: {}",
                    m["metric"].as_str().unwrap_or(""),
                    m["value"]
                        .as_f64()
                        .map(|v| super::number(v, m["unit"].as_str()))
                        .unwrap_or_else(|| "No value".into())
                ));
            }
            text.push('\n');
        }
        let names: HashMap<_, _> = self.nodes.iter().map(|n| (&n.id, &n.name)).collect();
        for e in &self.edges {
            text.push_str(&format!(
                "- {} {} {}{}\n",
                names[&e.from],
                e.label.replace('_', " "),
                names[&e.to],
                e.count.map(|c| format!(" ({c} links)")).unwrap_or_default()
            ));
        }
        for g in &self.groups {
            text.push_str(&format!(
                "{} more {} objects or kinds in the resolved selection.\n",
                g.count,
                g.kind.replace('_', " ")
            ));
        }
        if self.omitted_edges > 0 {
            text.push_str(&format!(
                "{} more resolved links are omitted.\n",
                self.omitted_edges
            ));
        }
        text
    }
}
pub fn schema() -> serde_json::Value {
    use serde_json::json;
    let hops = json!({"type":"string","enum":["one","two","three"]});
    let direction = json!({"type":"string","enum":["both","in","out"]});
    let id = json!({"type":"string"});
    json!({"type":"object","additionalProperties":false,"required":["selection"],"properties":{
        "selection":{"oneOf":[
            {"type":"object","additionalProperties":false,"required":["type","focus_id"],"properties":{"type":{"const":"neighbourhood"},"focus_id":id,"hops":hops,"direction":direction}},
            {"type":"object","additionalProperties":false,"required":["type","from_id","to_id"],"properties":{"type":{"const":"path"},"from_id":id,"to_id":id,"hops":hops,"direction":direction}},
            {"type":"object","additionalProperties":false,"required":["type","focus_id"],"properties":{"type":{"const":"impact"},"focus_id":id,"hops":hops}},
            {"type":"object","additionalProperties":false,"required":["type","vault_id"],"properties":{"type":{"const":"kinds"},"vault_id":id,"kind":id}}
        ]},
        "emphasis":{"type":"array","maxItems":MAX_NODES,"items":id},
        "annotations":{"type":"array","maxItems":4,"items":{"type":"object","additionalProperties":false,"required":["entity_id","metric"],"properties":{"entity_id":id,"metric":id}}}
    }})
}
