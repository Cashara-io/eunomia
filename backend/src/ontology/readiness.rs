//! Whether a vault's ontology is ready to answer number and what-if questions: its kinds, properties,
//! metrics and link types counted by status, and what still blocks it, in plain words. Read-only; the
//! owner fixes a blocker in the Eunomia app (Kinds and links).
//!
//! Ready means: nothing proposed is waiting for an owner, at least one metric is approved, and every
//! property an approved metric reads is approved and has values. A kind with no metric is not a
//! blocker: most kinds (people, organisations, vehicles) are never counted.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use surrealdb::types::RecordId;

use super::metric::MetricExpr;
use super::property::PropertyItem;
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
    /// No blockers: nothing proposed is waiting, a metric is approved, and every property an approved
    /// metric reads is approved and has values.
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
    let mut metric_blockers = Vec::new();
    for k in o.kinds.iter().filter(|k| !(k.built_in && k.entities == 0 && k.status == ViewStatus::Observed)) {
        kinds.add(k.status);
        let by_name: BTreeMap<&str, &PropertyItem> = k.properties.iter().map(|p| (p.name.as_str(), p)).collect();
        for p in &k.properties {
            properties.add(p.status.into());
        }
        for m in &k.metrics {
            metrics.add(m.status.into());
            // a proposed metric already waits for an owner, and a rejected one never runs
            if m.status != Status::Approved {
                continue;
            }
            for p in properties_used(&m.expr) {
                let why = match by_name.get(p) {
                    None => format!("metric {} on {} uses {p}, which is not a property of {}", m.name, k.kind, k.kind),
                    Some(i) if i.status == Status::Rejected => format!("metric {} on {} uses a rejected property, {p}", m.name, k.kind),
                    Some(i) if i.status == Status::Proposed => format!("metric {} on {} uses {p}, which is proposed, not approved", m.name, k.kind),
                    Some(i) if i.values == 0 => format!("metric {} on {} uses {p}, which has no values yet: import them or set them", m.name, k.kind),
                    Some(_) => continue,
                };
                metric_blockers.push(why);
            }
        }
    }
    for l in &o.links {
        links.add(l.status);
    }
    let mut blockers = Vec::new();
    blockers.extend(waiting(kinds.proposed, "kind", "kinds"));
    blockers.extend(waiting(properties.proposed, "property", "properties"));
    blockers.extend(waiting(metrics.proposed, "metric", "metrics"));
    blockers.extend(waiting(links.proposed, "link type", "link types"));
    if metrics.approved == 0 {
        blockers.push("no metric is approved yet, so there is no number to compute".to_string());
    }
    blockers.extend(metric_blockers);
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
