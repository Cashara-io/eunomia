//! Whether a vault's ontology is ready to answer number and what-if questions: its kinds, properties,
//! metrics and link types counted by status, and what still blocks it, in plain words. Read-only; the
//! owner fixes a blocker in the Eunomia app (Kinds and links).

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use surrealdb::types::RecordId;

use super::metric::MetricExpr;
use super::{resolve, view, OntologyOut, Status, ViewStatus};
use crate::authz::Action;
use crate::error::AppResult;
use crate::pool::OrgDb;

/// Rows of one sort, by status.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct StatusCounts {
    pub approved: usize,
    pub proposed: usize,
    pub rejected: usize,
    /// In use but never reviewed (kinds and link types only).
    pub observed: usize,
}

impl StatusCounts {
    fn add(&mut self, s: ViewStatus) {
        match s {
            ViewStatus::Approved => self.approved += 1,
            ViewStatus::Proposed => self.proposed += 1,
            ViewStatus::Rejected => self.rejected += 1,
            ViewStatus::Observed => self.observed += 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ReadinessOut {
    pub vault: String,
    /// Whether the vault takes only approved kinds and links.
    pub strict: bool,
    pub kinds: StatusCounts,
    pub properties: StatusCounts,
    pub metrics: StatusCounts,
    pub links: StatusCounts,
    /// No blockers.
    pub ready: bool,
    /// What stops the vault answering numbers, each in one sentence, for the owner.
    pub blockers: Vec<String>,
}

/// The readiness of `vault_id` (the caller's personal vault when omitted).
pub async fn readiness(db: &OrgDb, owner: &RecordId, vault_id: Option<&RecordId>) -> AppResult<ReadinessOut> {
    let vault = resolve(db, owner, vault_id, Action::ReadMemories).await?;
    Ok(assess(&view(db, &vault).await?))
}

fn waiting(n: usize, one: &str, many: &str) -> Option<String> {
    match n {
        0 => None,
        1 => Some(format!("1 {one} is proposed and needs an owner")),
        n => Some(format!("{n} {many} are proposed and need an owner")),
    }
}

/// Every property a metric reads: the measured one, a ratio's sides, the filters and the time property.
fn properties_used(e: &MetricExpr) -> BTreeSet<&str> {
    let mut out: BTreeSet<&str> = BTreeSet::new();
    out.extend(e.property.as_deref());
    out.extend(e.time_property.as_deref());
    out.extend(e.filters.iter().map(|f| f.property.as_str()));
    for side in [&e.numerator, &e.denominator].into_iter().flatten() {
        out.extend(side.property.as_deref());
        out.extend(side.filters.iter().map(|f| f.property.as_str()));
    }
    out
}

pub(super) fn assess(o: &OntologyOut) -> ReadinessOut {
    let (mut kinds, mut properties, mut metrics, mut links) = (StatusCounts::default(), StatusCounts::default(), StatusCounts::default(), StatusCounts::default());
    let mut blockers = Vec::new();
    let mut metric_blockers = Vec::new();
    let mut bare_kinds = Vec::new();
    for k in o.kinds.iter().filter(|k| !(k.built_in && k.entities == 0 && k.status == ViewStatus::Observed)) {
        kinds.add(k.status);
        let status_of: BTreeMap<&str, Status> = k.properties.iter().map(|p| (p.name.as_str(), p.status)).collect();
        for p in &k.properties {
            properties.add(p.status.into());
        }
        for m in &k.metrics {
            metrics.add(m.status.into());
            if m.status == Status::Rejected {
                continue;
            }
            for p in properties_used(&m.expr) {
                let why = match status_of.get(p) {
                    None => format!("metric {} on {} uses {p}, which is not a property of {}", m.name, k.kind, k.kind),
                    Some(Status::Rejected) => format!("metric {} on {} uses a rejected property, {p}", m.name, k.kind),
                    Some(Status::Proposed) => format!("metric {} on {} uses {p}, which is proposed, not approved", m.name, k.kind),
                    Some(Status::Approved) => continue,
                };
                metric_blockers.push(why);
            }
        }
        if k.status == ViewStatus::Approved && !k.metrics.iter().any(|m| m.status == Status::Approved) {
            let has_property = k.properties.iter().any(|p| p.status == Status::Approved);
            bare_kinds.push(if has_property {
                format!("no approved metric on kind {}", k.kind)
            } else {
                format!("no approved property or metric on kind {}", k.kind)
            });
        }
    }
    for l in &o.links {
        links.add(l.status);
    }
    blockers.extend(waiting(kinds.proposed, "kind", "kinds"));
    blockers.extend(waiting(properties.proposed, "property", "properties"));
    blockers.extend(waiting(metrics.proposed, "metric", "metrics"));
    blockers.extend(waiting(links.proposed, "link type", "link types"));
    blockers.extend(metric_blockers);
    blockers.extend(bare_kinds);
    ReadinessOut { vault: o.vault.clone(), strict: o.strict, kinds, properties, metrics, links, ready: blockers.is_empty(), blockers }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plurals_read_as_words() {
        assert_eq!(waiting(1, "property", "properties").unwrap(), "1 property is proposed and needs an owner");
        assert_eq!(waiting(3, "property", "properties").unwrap(), "3 properties are proposed and need an owner");
        assert!(waiting(0, "kind", "kinds").is_none());
    }
}
