//! A vault's project ontology: the entity kinds and link types it uses, as observed in its entities
//! and relations, merged with what its owners decided about them (migration 0033).
//!
//! * Discover: [`get`] profiles the vault; [`suggest::suggest`] asks the server's model for proposals,
//!   [`propose`] takes an agent's. Proposals never overwrite a decision, so rerunning converges.
//! * Review: [`decide_kind`] and [`decide_link`], for a vault owner in a browser session only, so an
//!   agent cannot approve its own proposal (there is no MCP tool for them).
//! * Ground: [`ground`] sends a write under a rejected kind to its merge target, or refuses it.
//! * Expose: the `ontology_*` tools (`tools/registry/ontology.rs`).

pub mod suggest;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use surrealdb::types::{RecordId, SurrealValue};

use crate::authz::{self, Action};
use crate::entities::service::{kind_table, KINDS};
use crate::error::{AppError, AppResult};
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::store::ontology as q;
use crate::tx::with_retry_dup;

/// What a person decided about a kind or link, as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Proposed,
    Approved,
    Rejected,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Proposed => "proposed",
            Status::Approved => "approved",
            Status::Rejected => "rejected",
        }
    }

    fn parse(s: &str) -> AppResult<Self> {
        match s {
            "proposed" => Ok(Status::Proposed),
            "approved" => Ok(Status::Approved),
            "rejected" => Ok(Status::Rejected),
            other => Err(AppError::internal(format!("ontology row with status {other:?}"))),
        }
    }
}

/// A kind or link as the ontology view shows it. `Observed`: used in the vault, never reviewed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ViewStatus {
    Observed,
    Proposed,
    Approved,
    Rejected,
}

impl From<Status> for ViewStatus {
    fn from(s: Status) -> Self {
        match s {
            Status::Proposed => ViewStatus::Proposed,
            Status::Approved => ViewStatus::Approved,
            Status::Rejected => ViewStatus::Rejected,
        }
    }
}

/// Who proposed a row: the server's model, an agent over MCP, or a person in the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Suggested,
    Agent,
    Human,
}

impl Origin {
    fn as_str(self) -> &'static str {
        match self {
            Origin::Suggested => "suggested",
            Origin::Agent => "agent",
            Origin::Human => "human",
        }
    }

    fn parse(s: &str) -> AppResult<Self> {
        match s {
            "suggested" => Ok(Origin::Suggested),
            "agent" => Ok(Origin::Agent),
            "human" => Ok(Origin::Human),
            other => Err(AppError::internal(format!("ontology row with origin {other:?}"))),
        }
    }
}

/// A kind name `memory_write` would accept: a built-in kind or a valid custom one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Kind(String);

impl Kind {
    pub fn parse(s: &str) -> AppResult<Self> {
        let s = s.trim();
        kind_table(s)?;
        Ok(Kind(s.to_string()))
    }

    pub fn is_built_in(&self) -> bool {
        KINDS.contains(&self.0.as_str())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

const MAX_LABEL: usize = 64;
const MAX_TEXT: usize = 500;

/// A relation label (`works_at`, `calls`): 1 to 64 characters, trimmed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Label(String);

impl Label {
    pub fn parse(s: &str) -> AppResult<Self> {
        let s = s.trim();
        if s.is_empty() || s.chars().count() > MAX_LABEL {
            return Err(AppError::bad_request(format!("a link label is 1 to {MAX_LABEL} characters, got {s:?}")));
        }
        Ok(Label(s.to_string()))
    }
}

/// One link type: entities of `from_kind` relate to entities of `to_kind` under `label`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LinkKey {
    pub from_kind: Kind,
    pub label: Label,
    pub to_kind: Kind,
}

impl LinkKey {
    pub fn parse(from_kind: &str, label: &str, to_kind: &str) -> AppResult<Self> {
        Ok(LinkKey { from_kind: Kind::parse(from_kind)?, label: Label::parse(label)?, to_kind: Kind::parse(to_kind)? })
    }
}

/// Free text from an agent, a model or a person: trimmed, at most [`MAX_TEXT`] characters, secrets
/// dropped as in a memory. Blank is none.
fn clean_text(field: &str, s: Option<&str>) -> AppResult<Option<String>> {
    let Some(s) = s.map(str::trim).filter(|s| !s.is_empty()) else { return Ok(None) };
    if s.chars().count() > MAX_TEXT {
        return Err(AppError::bad_request(format!("{field} is at most {MAX_TEXT} characters")));
    }
    Ok(Some(crate::redact::redact(s, &mut Vec::new())))
}

// Ids derive from the key, so every writer of one kind or link meets on one row.
fn kind_rid(vault: &RecordId, kind: &Kind) -> RecordId {
    RecordId::from_table_key("ontology_kind", crate::tx::stable_key('k', &format!("{}|{}", vault.to_string(), kind.0)))
}

fn link_rid(vault: &RecordId, key: &LinkKey) -> RecordId {
    let parts = format!("{}|{}|{}|{}", vault.to_string(), key.from_kind.0, key.label.0, key.to_kind.0);
    RecordId::from_table_key("ontology_link", crate::tx::stable_key('l', &parts))
}

/// Whether `e` is a read of an ontology table on an org database still at schema 32 (before
/// migration 0033), which is served until its migration job runs.
// ponytail: drop with the next tenant migration, once 0033 is the oldest schema served.
fn no_ontology_table(e: &surrealdb::Error) -> bool {
    let s = e.to_string();
    s.contains("The table 'ontology_kind' does not exist") || s.contains("The table 'ontology_link' does not exist")
}

// ---------------------------------------------------------------------------
// Rows and the view
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, SurrealValue)]
struct KindRow {
    kind: String,
    #[serde(default)]
    #[surreal(default)]
    description: Option<String>,
    status: String,
    #[serde(default)]
    #[surreal(default)]
    merge_into: Option<String>,
    origin: String,
    #[serde(default)]
    #[surreal(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct LinkRow {
    from_kind: String,
    label: String,
    to_kind: String,
    #[serde(default)]
    #[surreal(default)]
    description: Option<String>,
    status: String,
    origin: String,
    #[serde(default)]
    #[surreal(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct KindCount {
    kind: String,
    #[serde(default)]
    #[surreal(default)]
    count: i64,
    #[serde(default)]
    #[surreal(default)]
    samples: Vec<String>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct SampleRow {
    #[serde(default)]
    #[surreal(default)]
    from_name: Option<String>,
    #[serde(default)]
    #[surreal(default)]
    to_name: Option<String>,
}

#[derive(Debug, Deserialize, SurrealValue)]
struct LinkCount {
    from_kind: String,
    label: String,
    to_kind: String,
    #[serde(default)]
    #[surreal(default)]
    count: i64,
    #[serde(default)]
    #[surreal(default)]
    samples: Vec<SampleRow>,
}

/// One entity kind of the vault.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct KindItem {
    pub kind: String,
    pub built_in: bool,
    pub status: ViewStatus,
    pub description: Option<String>,
    /// Rejected: where writes under this kind go instead. Proposed: the merge the proposer suggests.
    pub merge_into: Option<String>,
    /// Who proposed it; none for a kind that was never reviewed.
    pub origin: Option<Origin>,
    pub reason: Option<String>,
    /// Entities of this kind in the vault.
    pub entities: i64,
    /// A few of their names.
    pub samples: Vec<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct LinkSample {
    pub from: String,
    pub to: String,
}

/// One link type of the vault: relations labelled `label` from a `from_kind` entity to a `to_kind` one.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct LinkItem {
    pub from_kind: String,
    pub label: String,
    pub to_kind: String,
    pub status: ViewStatus,
    pub description: Option<String>,
    pub origin: Option<Origin>,
    pub reason: Option<String>,
    /// Relations of this type in the vault.
    pub count: i64,
    pub samples: Vec<LinkSample>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct OntologyOut {
    pub vault: String,
    /// The built-in kinds first, then the others by name.
    pub kinds: Vec<KindItem>,
    pub links: Vec<LinkItem>,
}

/// `vault_id`, authorized for `action`, or the caller's default vault.
async fn resolve(db: &OrgDb, owner: &RecordId, vault_id: Option<&RecordId>, action: Action) -> AppResult<RecordId> {
    let vault = match vault_id {
        Some(v) => v.clone(),
        None => crate::vaults::service::default_vault_id(db, owner).await?,
    };
    Ok(authz::authorize(db, owner, action, &vault).await?.vault().clone())
}

/// The ontology of `vault_id` (the caller's personal vault when omitted).
pub async fn get(db: &OrgDb, owner: &RecordId, vault_id: Option<&RecordId>) -> AppResult<OntologyOut> {
    let vault = resolve(db, owner, vault_id, Action::ReadMemories).await?;
    view(db, &vault).await
}

/// What `vault` uses merged with its registry rows. The caller checked access.
pub(crate) async fn view(db: &OrgDb, vault: &RecordId) -> AppResult<OntologyOut> {
    let mut profile = q::PROFILE.on(db).bind(("vault", vault.clone())).await?;
    let mut registry = q::REGISTRY.on(db).bind(("vault", vault.clone())).await?;
    let counts: Vec<KindCount> = profile.take(q::PROFILE_KINDS_SLOT)?;
    let link_counts: Vec<LinkCount> = profile.take(q::PROFILE_LINKS_SLOT)?;
    let kind_rows: Vec<KindRow> = registry.take(0)?;
    let link_rows: Vec<LinkRow> = registry.take(1)?;
    Ok(OntologyOut { vault: vault.to_string(), kinds: merge_kinds(counts, kind_rows)?, links: merge_links(link_counts, link_rows)? })
}

fn merge_kinds(counts: Vec<KindCount>, rows: Vec<KindRow>) -> AppResult<Vec<KindItem>> {
    let mut by_kind: BTreeMap<String, KindItem> = BTreeMap::new();
    let blank = |kind: &str| KindItem {
        kind: kind.to_string(),
        built_in: KINDS.contains(&kind),
        status: ViewStatus::Observed,
        description: None,
        merge_into: None,
        origin: None,
        reason: None,
        entities: 0,
        samples: Vec::new(),
    };
    for c in counts {
        by_kind.insert(c.kind.clone(), KindItem { entities: c.count, samples: c.samples, ..blank(&c.kind) });
    }
    for r in rows {
        let item = by_kind.entry(r.kind.clone()).or_insert_with(|| blank(&r.kind));
        item.status = Status::parse(&r.status)?.into();
        item.description = r.description;
        item.merge_into = r.merge_into;
        item.origin = Some(Origin::parse(&r.origin)?);
        item.reason = r.reason;
    }
    let mut kinds: Vec<KindItem> = by_kind.into_values().collect();
    kinds.sort_by_key(|k| (KINDS.iter().position(|b| *b == k.kind).unwrap_or(KINDS.len()), k.kind.clone()));
    Ok(kinds)
}

fn merge_links(counts: Vec<LinkCount>, rows: Vec<LinkRow>) -> AppResult<Vec<LinkItem>> {
    let mut by_key: BTreeMap<(String, String, String), LinkItem> = BTreeMap::new();
    let blank = |from_kind: &str, label: &str, to_kind: &str| LinkItem {
        from_kind: from_kind.to_string(),
        label: label.to_string(),
        to_kind: to_kind.to_string(),
        status: ViewStatus::Observed,
        description: None,
        origin: None,
        reason: None,
        count: 0,
        samples: Vec::new(),
    };
    for c in counts {
        let samples = c
            .samples
            .into_iter()
            .take(3)
            .filter_map(|s| Some(LinkSample { from: s.from_name?, to: s.to_name? }))
            .collect();
        let item = LinkItem { count: c.count, samples, ..blank(&c.from_kind, &c.label, &c.to_kind) };
        by_key.insert((c.from_kind, c.label, c.to_kind), item);
    }
    for r in rows {
        let key = (r.from_kind.clone(), r.label.clone(), r.to_kind.clone());
        let item = by_key.entry(key).or_insert_with(|| blank(&r.from_kind, &r.label, &r.to_kind));
        item.status = Status::parse(&r.status)?.into();
        item.description = r.description;
        item.origin = Some(Origin::parse(&r.origin)?);
        item.reason = r.reason;
    }
    Ok(by_key.into_values().collect())
}

// ---------------------------------------------------------------------------
// Proposals
// ---------------------------------------------------------------------------

/// A proposed kind, as an agent or the model sends it.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct KindProposal {
    pub kind: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Proposes folding this kind into another one (rejecting it with that merge target).
    #[serde(default)]
    pub merge_into: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

/// A proposed link type.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LinkProposal {
    pub from_kind: String,
    pub label: String,
    pub to_kind: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

struct ValidKind {
    kind: Kind,
    description: Option<String>,
    merge_into: Option<Kind>,
    reason: Option<String>,
}

struct ValidLink {
    key: LinkKey,
    description: Option<String>,
    reason: Option<String>,
}

impl KindProposal {
    fn validate(&self) -> AppResult<ValidKind> {
        let kind = Kind::parse(&self.kind)?;
        let merge_into = self.merge_into.as_deref().map(str::trim).filter(|m| !m.is_empty()).map(Kind::parse).transpose()?;
        if let Some(target) = &merge_into {
            if target == &kind {
                return Err(AppError::bad_request(format!("{:?} cannot merge into itself", kind.0)));
            }
            if kind.is_built_in() {
                return Err(AppError::bad_request(format!("{:?} is a built-in kind: it cannot be merged into another", kind.0)));
            }
        }
        Ok(ValidKind {
            kind,
            description: clean_text("description", self.description.as_deref())?,
            merge_into,
            reason: clean_text("reason", self.reason.as_deref())?,
        })
    }
}

impl LinkProposal {
    fn validate(&self) -> AppResult<ValidLink> {
        Ok(ValidLink {
            key: LinkKey::parse(&self.from_kind, &self.label, &self.to_kind)?,
            description: clean_text("description", self.description.as_deref())?,
            reason: clean_text("reason", self.reason.as_deref())?,
        })
    }
}

/// Most kinds and links one proposal call takes.
pub const MAX_PROPOSED_KINDS: usize = 50;
pub const MAX_PROPOSED_LINKS: usize = 100;

/// A proposed kind after the call: `status` is the row's, so a kind a person already decided shows
/// that decision (the proposal did not change it).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ProposedKind {
    pub kind: String,
    pub status: Status,
    pub merge_into: Option<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ProposedLink {
    pub from_kind: String,
    pub label: String,
    pub to_kind: String,
    pub status: Status,
}

#[derive(Debug, Clone, Default, Serialize, utoipa::ToSchema)]
pub struct ProposeOut {
    pub kinds: Vec<ProposedKind>,
    pub links: Vec<ProposedLink>,
}

/// An agent's proposals for `vault_id` (the caller's personal vault when omitted). Every item is
/// checked before any is written: one bad name fails the call.
pub async fn propose(
    db: &OrgDb,
    owner: &RecordId,
    vault_id: Option<&RecordId>,
    kinds: &[KindProposal],
    links: &[LinkProposal],
) -> AppResult<ProposeOut> {
    if kinds.is_empty() && links.is_empty() {
        return Err(AppError::bad_request("propose at least one kind or link"));
    }
    if kinds.len() > MAX_PROPOSED_KINDS || links.len() > MAX_PROPOSED_LINKS {
        return Err(AppError::bad_request(format!("at most {MAX_PROPOSED_KINDS} kinds and {MAX_PROPOSED_LINKS} links per call")));
    }
    let vault = resolve(db, owner, vault_id, Action::WriteMemories).await?;
    let kinds = kinds.iter().map(KindProposal::validate).collect::<AppResult<Vec<_>>>()?;
    let links = links.iter().map(LinkProposal::validate).collect::<AppResult<Vec<_>>>()?;
    apply(db, &vault, kinds, links, Origin::Agent).await
}

async fn apply(db: &OrgDb, vault: &RecordId, kinds: Vec<ValidKind>, links: Vec<ValidLink>, origin: Origin) -> AppResult<ProposeOut> {
    let mut out = ProposeOut::default();
    for k in kinds {
        let id = kind_rid(vault, &k.kind);
        let row: KindRow = with_retry_dup(|| async {
            let mut res = q::PROPOSE_KIND
                .on(db)
                .bind(("id", id.clone()))
                .bind(("vault", vault.clone()))
                .bind(("kind", k.kind.0.clone()))
                .bind(("description", k.description.clone()))
                .bind(("merge_into", k.merge_into.as_ref().map(|m| m.0.clone())))
                .bind(("origin", origin.as_str().to_string()))
                .bind(("reason", k.reason.clone()))
                .await?;
            let rows: Vec<KindRow> = res.take(q::PROPOSE_KIND.slot)?;
            rows.into_iter().next().ok_or_else(|| surrealdb::Error::query("ontology kind upsert returned no row".into(), None))
        })
        .await?;
        out.kinds.push(ProposedKind { kind: row.kind, status: Status::parse(&row.status)?, merge_into: row.merge_into });
    }
    for l in links {
        let id = link_rid(vault, &l.key);
        let row: LinkRow = with_retry_dup(|| async {
            let mut res = q::PROPOSE_LINK
                .on(db)
                .bind(("id", id.clone()))
                .bind(("vault", vault.clone()))
                .bind(("from_kind", l.key.from_kind.0.clone()))
                .bind(("label", l.key.label.0.clone()))
                .bind(("to_kind", l.key.to_kind.0.clone()))
                .bind(("description", l.description.clone()))
                .bind(("origin", origin.as_str().to_string()))
                .bind(("reason", l.reason.clone()))
                .await?;
            let rows: Vec<LinkRow> = res.take(q::PROPOSE_LINK.slot)?;
            rows.into_iter().next().ok_or_else(|| surrealdb::Error::query("ontology link upsert returned no row".into(), None))
        })
        .await?;
        out.links.push(ProposedLink { from_kind: row.from_kind, label: row.label, to_kind: row.to_kind, status: Status::parse(&row.status)? });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Review: a person decides
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum KindDecision {
    Approve,
    /// Writes under the kind go to `merge_into` from now on; without it they are refused.
    Reject {
        #[serde(default)]
        merge_into: Option<String>,
    },
    /// Set or (blank) clear the description; the status stays as it is.
    Describe { description: String },
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum LinkDecision {
    Approve,
    Reject,
    Describe { description: String },
}

/// Who may decide: an owner of the vault, acting in the browser (a token, which an agent may hold,
/// cannot approve what an agent proposed).
async fn decider(db: &OrgDb, user: &RecordId, vault: &RecordId) -> AppResult<()> {
    authz::require_session()?;
    authz::authorize(db, user, Action::DecideOntology, vault).await?;
    Ok(())
}

pub async fn decide_kind(db: &OrgDb, user: &RecordId, vault: &RecordId, kind: &str, decision: KindDecision) -> AppResult<OntologyOut> {
    decider(db, user, vault).await?;
    let kind = Kind::parse(kind)?;
    let id = kind_rid(vault, &kind);
    let (status, merge_into) = match decision {
        KindDecision::Describe { description } => {
            let description = clean_text("description", Some(&description))?;
            with_retry_dup(|| async {
                q::DESCRIBE_KIND
                    .on(db)
                    .bind(("id", id.clone()))
                    .bind(("vault", vault.clone()))
                    .bind(("kind", kind.0.clone()))
                    .bind(("description", description.clone()))
                    .await
            })
            .await?;
            return view(db, vault).await;
        }
        KindDecision::Approve => (Status::Approved, None),
        KindDecision::Reject { merge_into } => {
            if kind.is_built_in() {
                return Err(AppError::bad_request(format!("{:?} is a built-in kind: it can be described and approved, not rejected", kind.0)));
            }
            let target = merge_into.as_deref().map(str::trim).filter(|m| !m.is_empty()).map(Kind::parse).transpose()?;
            if let Some(t) = &target {
                check_merge_target(db, vault, &kind, t).await?;
            }
            (Status::Rejected, target)
        }
    };
    with_retry_dup(|| async {
        q::DECIDE_KIND
            .on(db)
            .bind(("id", id.clone()))
            .bind(("vault", vault.clone()))
            .bind(("kind", kind.0.clone()))
            .bind(("status", status.as_str().to_string()))
            .bind(("merge_into", merge_into.as_ref().map(|m| m.0.clone())))
            .bind(("user", user.clone()))
            .await
    })
    .await?;
    view(db, vault).await
}

/// A rejected kind's writes may go to a built-in kind, a kind the vault already uses, or one an owner
/// approved; never to itself or to another rejected kind.
async fn check_merge_target(db: &OrgDb, vault: &RecordId, kind: &Kind, target: &Kind) -> AppResult<()> {
    if target == kind {
        return Err(AppError::bad_request(format!("{:?} cannot merge into itself", kind.0)));
    }
    if target.is_built_in() {
        return Ok(());
    }
    let mut res = q::TARGET_STATE
        .on(db)
        .bind(("id", kind_rid(vault, target)))
        .bind(("vault", vault.clone()))
        .bind(("kind", target.0.clone()))
        .await?;
    let status: Vec<String> = res.take(0)?;
    let used: Vec<RecordId> = res.take(1)?;
    match status.first().map(|s| Status::parse(s)).transpose()? {
        Some(Status::Rejected) => Err(AppError::bad_request(format!("{:?} is rejected too: merge into a kind that is kept", target.0))),
        Some(Status::Approved) => Ok(()),
        _ if !used.is_empty() => Ok(()),
        _ => Err(AppError::bad_request(format!(
            "{:?} is not a kind this vault uses or approved: merge into one of those, or approve it first",
            target.0
        ))),
    }
}

pub async fn decide_link(
    db: &OrgDb,
    user: &RecordId,
    vault: &RecordId,
    from_kind: &str,
    label: &str,
    to_kind: &str,
    decision: LinkDecision,
) -> AppResult<OntologyOut> {
    decider(db, user, vault).await?;
    let key = LinkKey::parse(from_kind, label, to_kind)?;
    let id = link_rid(vault, &key);
    let on = |stmt: &crate::store::Stmt| {
        stmt.on(db)
            .bind(("id", id.clone()))
            .bind(("vault", vault.clone()))
            .bind(("from_kind", key.from_kind.0.clone()))
            .bind(("label", key.label.0.clone()))
            .bind(("to_kind", key.to_kind.0.clone()))
    };
    match decision {
        LinkDecision::Describe { description } => {
            let description = clean_text("description", Some(&description))?;
            with_retry_dup(|| async { on(&q::DESCRIBE_LINK).bind(("description", description.clone())).await }).await?;
        }
        LinkDecision::Approve | LinkDecision::Reject => {
            let status = if matches!(decision, LinkDecision::Approve) { Status::Approved } else { Status::Rejected };
            with_retry_dup(|| async { on(&q::DECIDE_LINK).bind(("status", status.as_str().to_string())).bind(("user", user.clone())).await }).await?;
        }
    }
    view(db, vault).await
}

// ---------------------------------------------------------------------------
// Grounding: decisions steer later writes
// ---------------------------------------------------------------------------

/// A write under a rejected kind that went to its merge target instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KindMapped {
    pub from: String,
    pub to: String,
}

/// Where a write under custom kind `kind` in `vault` goes: `None` to `kind` itself (approved, proposed
/// or never reviewed), `Some` to the merge target of a rejected kind. A kind rejected with no target
/// is refused, naming the approved kinds. One point read.
pub async fn ground(db: &OrgDb, vault: &RecordId, kind: &str) -> AppResult<Option<KindMapped>> {
    #[derive(Deserialize, SurrealValue)]
    struct Row {
        status: String,
        #[serde(default)]
        #[surreal(default)]
        merge_into: Option<String>,
    }
    let kind = Kind::parse(kind)?;
    let rows: Vec<Row> = match q::GROUND.on(db).bind(("id", kind_rid(vault, &kind))).await {
        Err(e) if no_ontology_table(&e) => return Ok(None),
        res => res?.take(0)?,
    };
    let Some(row) = rows.into_iter().next() else { return Ok(None) };
    if Status::parse(&row.status)? != Status::Rejected {
        return Ok(None);
    }
    match row.merge_into {
        Some(to) => Ok(Some(KindMapped { from: kind.0, to })),
        None => {
            let approved: Vec<String> = q::APPROVED_KINDS.on(db).bind(("vault", vault.clone())).await?.take(0)?;
            let custom = if approved.is_empty() { String::new() } else { format!(" or an approved kind ({})", approved.join(", ")) };
            Err(AppError::bad_request(format!(
                "the kind {:?} was rejected for this vault: write under a built-in kind ({}){custom}",
                kind.0,
                KINDS.join(", ")
            )))
        }
    }
}

/// A reviewed kind in one of the vaults `entity_types` reads.
#[derive(Debug, Deserialize, SurrealValue)]
pub struct ReviewedKind {
    pub vault: RecordId,
    pub kind: String,
    #[serde(default)]
    #[surreal(default)]
    pub description: Option<String>,
    status: String,
    #[serde(default)]
    #[surreal(default)]
    pub merge_into: Option<String>,
}

impl ReviewedKind {
    pub fn status(&self) -> AppResult<Status> {
        Status::parse(&self.status)
    }
}

/// The reviewed kinds of `vaults` (already checked by the caller); none on a database before 0033.
pub async fn reviewed_kinds(db: &OrgDb, vaults: &[RecordId]) -> AppResult<Vec<ReviewedKind>> {
    match q::KINDS_IN.on(db).bind(("vaults", vaults.to_vec())).await {
        Err(e) if no_ontology_table(&e) => Ok(Vec::new()),
        res => Ok(res?.take(0)?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(kind: &str, n: i64) -> KindCount {
        KindCount { kind: kind.into(), count: n, samples: vec![] }
    }

    fn row(kind: &str, status: &str) -> KindRow {
        KindRow { kind: kind.into(), description: None, status: status.into(), merge_into: None, origin: "agent".into(), reason: None }
    }

    #[test]
    fn built_in_kinds_come_first_then_the_rest_by_name() {
        let kinds = merge_kinds(vec![count("zebra", 2), count("person", 1), count("db", 3), count("file", 0)], vec![row("alpha", "proposed")]).unwrap();
        let names: Vec<&str> = kinds.iter().map(|k| k.kind.as_str()).collect();
        assert_eq!(names, ["person", "file", "alpha", "db", "zebra"]);
        assert_eq!(kinds[2].status, ViewStatus::Proposed);
        assert_eq!(kinds[2].entities, 0);
        assert_eq!(kinds[3].status, ViewStatus::Observed);
        assert!(kinds[0].built_in && !kinds[3].built_in);
    }

    #[test]
    fn a_registry_row_sets_the_status_of_an_observed_kind() {
        let kinds = merge_kinds(vec![count("db", 3)], vec![row("db", "rejected")]).unwrap();
        assert_eq!((kinds[0].status, kinds[0].entities, kinds[0].origin), (ViewStatus::Rejected, 3, Some(Origin::Agent)));
    }

    #[test]
    fn a_proposal_cannot_merge_a_kind_into_itself_or_a_built_in_away() {
        let p = |kind: &str, into: &str| KindProposal { kind: kind.into(), description: None, merge_into: Some(into.into()), reason: None };
        assert!(p("client", "client").validate().is_err());
        assert!(p("person", "customer").validate().is_err());
        assert!(p("Bad Kind", "customer").validate().is_err());
        assert_eq!(p("client", "customer").validate().unwrap().merge_into, Some(Kind("customer".into())));
    }

    #[test]
    fn labels_and_texts_are_bounded() {
        assert!(Label::parse("  ").is_err());
        assert!(Label::parse(&"x".repeat(65)).is_err());
        assert_eq!(Label::parse(" works_at ").unwrap(), Label("works_at".into()));
        assert_eq!(clean_text("description", Some("   ")).unwrap(), None);
        assert!(clean_text("description", Some(&"x".repeat(501))).is_err());
    }
}
