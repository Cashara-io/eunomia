//! Connector routing (docs/connectors.md): which vault a sync files each record in.
//!
//! Each connector has a default destination (the owner's personal vault unless one is set) and an
//! ordered list of rules, each `{field, value, vault}` over a field the source knows ([`fields`]: a
//! Pocket tag, a GitHub repo, a Slack channel, a Gmail label, ...; `title` everywhere). The first rule
//! that matches a record wins; a record no rule matches goes to the default. No AI classification.
//!
//! [`Router::for_source`] reads the rules and the owner's memberships once per sync, then each record
//! costs O(rules). A rule or default naming a vault the connector's owner is not an active member of
//! falls back to the default vault (the personal vault for a default), and that is logged: routing never
//! writes into a vault its owner cannot. Routing places a record when it is first stored (see
//! `cache::search::Placement`): a changed destination applies to future records, and [`move_existing`]
//! ("Move existing data") re-files what is already stored, with the facts and relations drawn from it.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use surrealdb::types::{RecordId, SurrealValue};

use crate::cache::search::Placement;
use crate::error::{AppError, AppResult};
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::sources::base::{items, s, Source};
use crate::state::OrgState;
use crate::store;
use crate::vaults::service as vaults_service;

/// Most rules one connector may have.
pub const MAX_RULES: usize = 50;
const MAX_VALUE_CHARS: usize = 200;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Rule {
    /// One of the connector's `routing_fields`.
    pub field: String,
    /// What the field must be (case-insensitive); for `title`, a word or phrase it contains.
    pub value: String,
    /// The vault a matching record goes to (`vault:...`).
    pub vault: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Routing {
    /// Where records no rule matches go (`vault:...`); absent: your personal vault.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_vault: Option<String>,
    /// Checked in order; the first match wins.
    #[serde(default)]
    pub rules: Vec<Rule>,
}

impl Routing {
    /// A connector row's stored routing (`{}` or absent: none).
    pub fn of(stored: &Value) -> Routing {
        serde_json::from_value(stored.clone()).unwrap_or_default()
    }
}

/// The fields a source's rules can match on.
pub fn fields(source: &str) -> &'static [&'static str] {
    match source {
        "heypocket" => &["tag", "folder", "title"],
        "github" => &["repo", "title"],
        "slack" => &["channel", "title"],
        "gmail" => &["label", "title"],
        "google_calendar" => &["calendar", "title"],
        "discord" => &["channel", "title"],
        "notion" => &["database", "page", "title"],
        "linear" => &["team", "title"],
        "todoist" => &["project", "title"],
        _ => &["title"],
    }
}

/// A field's value as rules compare it: trimmed and lowercase; a channel without its `#`, a Notion id
/// without dashes (the API writes them, a page URL does not).
fn normalize(field: &str, value: &str) -> String {
    let v = value.trim().to_lowercase();
    match field {
        "channel" => v.trim_start_matches('#').to_string(),
        "database" | "page" => v.replace('-', ""),
        _ => v,
    }
}

/// What rules can match on in one raw record of `source`, as `field:value` keys, plus its `title`.
/// Kept on the stored record (`cache_record.route_keys`) for [`move_existing`].
pub fn route_keys(source: &str, raw: &Value, title: &str) -> Vec<String> {
    let mut found: Vec<(&str, String)> = Vec::new();
    let mut add = |field: &'static str, value: &str| {
        if !value.trim().is_empty() {
            found.push((field, value.to_string()));
        }
    };
    match source {
        "heypocket" => {
            for t in items(raw, "/tags") {
                add("tag", t.get("name").and_then(Value::as_str).or(t.as_str()).unwrap_or(""));
            }
            // a folder object, a folder name, or `folder_name`, whichever the API sends
            add("folder", raw.pointer("/folder/name").and_then(Value::as_str).or(raw.get("folder").and_then(Value::as_str)).unwrap_or(""));
            add("folder", s(raw, "/folder_name"));
        }
        "github" => add("repo", s(raw, "/repository/full_name")),
        "slack" => {
            add("channel", s(raw, "/_channel_id"));
            add("channel", s(raw, "/_channel_name"));
        }
        "gmail" => {
            for label in items(raw, "/labelIds") {
                add("label", label.as_str().unwrap_or(""));
            }
        }
        "google_calendar" => {
            // only the primary calendar is synced; its id is also the owner's address
            add("calendar", "primary");
            if raw.pointer("/organizer/self").and_then(Value::as_bool) == Some(true) {
                add("calendar", s(raw, "/organizer/email"));
            }
        }
        "discord" => {
            add("channel", s(raw, "/channel_id"));
            add("channel", s(raw, "/_channel_name"));
        }
        "notion" => {
            add("database", s(raw, "/parent/database_id"));
            add("database", s(raw, "/parent/data_source_id"));
            add("page", s(raw, "/parent/page_id"));
        }
        "linear" => add("team", s(raw, "/team/name")),
        "todoist" => {
            add("project", s(raw, "/project_id"));
            add("project", s(raw, "/_project_name"));
        }
        _ => {}
    }
    add("title", title);
    found.into_iter().map(|(field, value)| format!("{field}:{}", normalize(field, &value))).collect()
}

/// The keys of a record stored before routing kept any: only its title can be matched.
fn legacy_keys(title: &str) -> Vec<String> {
    vec![format!("title:{}", normalize("title", title))]
}

/// Whether a rule over `field` with (normalized) `value` matches a record's `keys`: `title` by
/// containment, every other field by equality.
fn matches(field: &str, value: &str, keys: &[String]) -> bool {
    keys.iter().filter_map(|k| k.split_once(':')).any(|(f, v)| f == field && if field == "title" { v.contains(value) } else { v == value })
}

/// The index of the first of `rules` (`(field, normalized value)`) matching `keys`.
fn first_match(rules: &[(String, String)], keys: &[String]) -> Option<usize> {
    rules.iter().position(|(field, value)| matches(field, value, keys))
}

/// A connector's routing, resolved against its owner's memberships: what one sync (or one move) uses.
pub struct Router {
    /// `None` only for a user without one (none is ever made without one): nothing routes "personal".
    personal: Option<RecordId>,
    /// `None`: the personal vault.
    default: Option<RecordId>,
    rules: Vec<(String, String)>,
    targets: Vec<Option<RecordId>>,
    writable: Arc<Vec<RecordId>>,
}

#[derive(serde::Deserialize, SurrealValue)]
struct MemberRow {
    vault: RecordId,
}

impl Router {
    /// `routing` for `owner`'s connector of `source`.
    pub async fn load(db: &OrgDb, owner: &RecordId, source: &str, routing: &Routing) -> AppResult<Router> {
        let personal = vaults_service::personal_vault_id(db, owner).await.ok();
        // the owner's memberships, not the credential's: a sync acts for the connector's owner
        let rows: Vec<MemberRow> = store::vaults::ACCESSIBLE_IDS.on(db).bind(("user", owner.clone())).await?.take(0)?;
        let writable: Vec<RecordId> = rows.into_iter().map(|r| r.vault).collect();
        // `Some(None)`: the personal vault; `None`: not a vault the owner may write (logged)
        let resolve = |given: &str| -> Option<Option<RecordId>> {
            let vault = crate::rid::parse(given).ok().filter(|v| v.table() == "vault");
            match vault {
                Some(v) if Some(&v) == personal.as_ref() => Some(None),
                Some(v) if writable.contains(&v) => Some(Some(v)),
                _ => {
                    tracing::warn!(
                        owner = %owner.to_string(),
                        source,
                        vault = given,
                        "routing: the connector's owner is not a member of this vault; its records go to the default vault"
                    );
                    None
                }
            }
        };
        let default = routing.default_vault.as_deref().and_then(resolve).flatten();
        let mut rules = Vec::with_capacity(routing.rules.len());
        let mut targets = Vec::with_capacity(routing.rules.len());
        for rule in &routing.rules {
            rules.push((rule.field.clone(), normalize(&rule.field, &rule.value)));
            targets.push(resolve(&rule.vault).unwrap_or_else(|| default.clone()));
        }
        Ok(Router { personal, default, rules, targets, writable: Arc::new(writable) })
    }

    /// The routing of `owner`'s connector for `src`, read once.
    pub async fn for_source(db: &OrgDb, owner: &RecordId, src: &dyn Source) -> AppResult<Router> {
        let stored = crate::connectors::service::get_connector(db, owner, src.provider_key()).await?.map(|c| c.routing).unwrap_or(Value::Null);
        Router::load(db, owner, src.key(), &Routing::of(&stored)).await
    }

    /// The vault a record with `keys` goes to (`None`: the personal vault).
    pub fn route(&self, keys: &[String]) -> Option<RecordId> {
        match first_match(&self.rules, keys) {
            Some(i) => self.targets[i].clone(),
            None => self.default.clone(),
        }
    }

    pub fn placement(&self, keys: Vec<String>) -> Placement {
        Placement { vault: self.route(&keys), route_keys: keys, writable: self.writable.clone() }
    }

    /// The vault a record filed in `vault` is in (`None` is the personal vault).
    fn actual(&self, vault: Option<&RecordId>) -> AppResult<RecordId> {
        vault.or(self.personal.as_ref()).cloned().ok_or_else(|| AppError::internal("this user has no personal vault"))
    }

    /// Whether the owner may still move data out of (or into) `vault`.
    fn can_write(&self, vault: &RecordId) -> bool {
        Some(vault) == self.personal.as_ref() || self.writable.contains(vault)
    }
}

/// Checks and tidies routing a user wants to save for `source`: known fields, non-empty values, at
/// most [`MAX_RULES`] rules, and only vaults the user is an active member of.
pub async fn validate(db: &OrgDb, owner: &RecordId, source: &str, routing: Routing) -> AppResult<Routing> {
    if routing.rules.len() > MAX_RULES {
        return Err(AppError::bad_request(format!("at most {MAX_RULES} routing rules")));
    }
    let known = fields(source);
    let vaults = vaults_service::accessible_vault_ids(db, owner).await?;
    let check_vault = |given: &str| -> AppResult<String> {
        let id = crate::rid::parse(given.trim()).ok().filter(|v| v.table() == "vault" && vaults.contains(v));
        id.map(|v| v.to_string()).ok_or_else(|| AppError::bad_request(format!("{given:?} is not a vault you are a member of")))
    };
    let default_vault = routing.default_vault.as_deref().filter(|v| !v.trim().is_empty()).map(check_vault).transpose()?;
    let mut rules = Vec::with_capacity(routing.rules.len());
    for rule in routing.rules {
        if !known.contains(&rule.field.as_str()) {
            return Err(AppError::bad_request(format!("unknown routing field {:?}; this connector routes on {known:?}", rule.field)));
        }
        let value = rule.value.trim().to_string();
        if value.is_empty() || value.chars().count() > MAX_VALUE_CHARS {
            return Err(AppError::bad_request(format!("a routing rule's value must be 1 to {MAX_VALUE_CHARS} characters")));
        }
        rules.push(Rule { field: rule.field, value, vault: check_vault(&rule.vault)? });
    }
    Ok(Routing { default_vault, rules })
}

/// What "Move existing data" did.
#[derive(Debug, Default, Serialize, utoipa::ToSchema)]
pub struct MoveReport {
    /// Records re-filed to the vault the current rules route them to.
    pub records: u64,
    /// Facts drawn from them that moved too (one the destination already had is not duplicated).
    pub memories: u64,
    /// Relations drawn from them that moved too.
    pub relations: u64,
    /// Facts and relations left in a vault the owner is no longer a member of.
    pub left: u64,
}

#[derive(serde::Deserialize, SurrealValue)]
struct StoredRecord {
    id: RecordId,
    #[serde(default)]
    #[surreal(default)]
    vault: Option<RecordId>,
    #[serde(default)]
    #[surreal(default)]
    title: String,
    #[serde(default)]
    #[surreal(default)]
    route_keys: Option<Vec<String>>,
}

#[derive(serde::Deserialize, SurrealValue)]
struct Fact {
    id: RecordId,
    subject: RecordId,
    vault: RecordId,
    #[serde(default)]
    #[surreal(default)]
    text: String,
    #[serde(rename = "type", default)]
    #[surreal(rename = "type", default)]
    mem_type: String,
    source: RecordId,
}

#[derive(serde::Deserialize, SurrealValue)]
struct Edge {
    id: RecordId,
    #[serde(rename = "in")]
    #[surreal(rename = "in")]
    in_: RecordId,
    out: RecordId,
    #[serde(default)]
    #[surreal(default)]
    label: String,
    source: RecordId,
}

#[derive(serde::Deserialize, SurrealValue)]
struct EntityRow {
    vault: RecordId,
    #[serde(default)]
    #[surreal(default)]
    name: String,
    #[serde(default)]
    #[surreal(default)]
    aliases: Vec<String>,
}

const MOVE_PAGE: usize = 200;

/// "Move existing data": re-files `owner`'s records from `src` to where the connector's current rules
/// route them, with the facts and relations drawn from each (into the same-named entities in the
/// destination, created if missing). Idempotent: a record already where the rules route it is left
/// alone, and a fact the destination already has is not copied twice. Facts move before their record,
/// so a run cut short leaves the record in its old vault and the next run finishes it. A fact in a
/// vault the owner has left stays there (`left`); the record itself still moves, it is the owner's.
pub async fn move_existing(state: &OrgState, owner: &RecordId, src: &dyn Source) -> AppResult<MoveReport> {
    let db = &state.db;
    let router = Router::for_source(db, owner, src).await?;
    let mut report = MoveReport::default();
    // (entity, destination vault) -> the same entity there
    #[allow(clippy::mutable_key_type)] // RecordId hashes by value; the interior mutability is never touched
    let mut twins: HashMap<(RecordId, RecordId), RecordId> = HashMap::new();
    let mut touched: Vec<RecordId> = Vec::new();
    let mut start = 0usize;
    loop {
        let page: Vec<StoredRecord> = store::routing::RECORDS_OF_SOURCE
            .on(db)
            .bind(("owner", owner.clone()))
            .bind(("source", src.key().to_string()))
            .bind(("limit", MOVE_PAGE as i64))
            .bind(("start", start as i64))
            .await?
            .take(0)?;
        if page.is_empty() {
            break;
        }
        start += page.len();

        // record -> (where it goes, as stored; the vault that is)
        #[allow(clippy::mutable_key_type)]
        let mut moving: HashMap<RecordId, (Option<RecordId>, RecordId)> = HashMap::new();
        for r in page {
            let keys = r.route_keys.unwrap_or_else(|| legacy_keys(&r.title));
            let to = router.route(&keys);
            if to != r.vault {
                let actual = router.actual(to.as_ref())?;
                moving.insert(r.id, (to, actual));
            }
        }
        if moving.is_empty() {
            continue;
        }
        let ids: Vec<RecordId> = moving.keys().cloned().collect();

        let facts: Vec<Fact> = store::routing::FACTS_OF.on(db).bind(("ids", ids.clone())).await?.take(0)?;
        for f in facts {
            let to = &moving[&f.source].1;
            if f.vault == *to {
                continue;
            }
            if !router.can_write(&f.vault) {
                report.left += 1;
                continue;
            }
            let subject = twin(db, owner, &mut twins, &f.subject, to).await?;
            crate::tx::with_retry(|| async {
                store::routing::MOVE_FACT
                    .on(db)
                    .bind(("id", f.id.clone()))
                    .bind(("old", f.subject.clone()))
                    .bind(("subject", subject.clone()))
                    .bind(("vault", to.clone()))
                    .bind(("type", f.mem_type.clone()))
                    .bind(("text", f.text.clone()))
                    .await?
                    .check()
            })
            .await?;
            report.memories += 1;
            if !touched.contains(&subject) {
                touched.push(subject);
            }
        }

        let edges: Vec<Edge> = store::routing::EDGES_OF.on(db).bind(("ids", ids.clone())).await?.take(0)?;
        for e in edges {
            let to = &moving[&e.source].1;
            let Some(from) = store::get::<EntityRow>(db, &e.in_).await? else { continue };
            if from.vault == *to {
                continue;
            }
            if !router.can_write(&from.vault) {
                report.left += 1;
                continue;
            }
            let (a, b) = (twin(db, owner, &mut twins, &e.in_, to).await?, twin(db, owner, &mut twins, &e.out, to).await?);
            crate::tx::with_retry(|| async {
                store::routing::MOVE_EDGE
                    .on(db)
                    .bind(("edge", e.id.clone()))
                    .bind(("in", a.clone()))
                    .bind(("out", b.clone()))
                    .bind(("label", e.label.clone()))
                    .bind(("owner", owner.clone()))
                    .bind(("source", e.source.clone()))
                    .await?
                    .check()
            })
            .await?;
            report.relations += 1;
        }

        // the records last, one statement per destination
        #[allow(clippy::mutable_key_type)]
        let mut by_dest: HashMap<Option<RecordId>, Vec<RecordId>> = HashMap::new();
        for (id, (to, _)) in moving {
            by_dest.entry(to).or_default().push(id);
        }
        for (to, ids) in by_dest {
            let n = ids.len() as u64;
            crate::tx::with_retry(|| async { store::routing::MOVE_RECORDS.on(db).bind(("ids", ids.clone())).bind(("vault", to.clone())).await?.check() }).await?;
            report.records += n;
        }
    }
    // the destinations' observations went stale: rebuild them like extraction does
    for subject in &touched {
        crate::jobs::enqueue_lossy(&state.control, crate::jobs::leader::consolidate_job(db.org(), owner, subject)).await;
    }
    tracing::info!(owner = %owner.to_string(), source = src.key(), records = report.records, memories = report.memories, relations = report.relations, left = report.left, "routing: moved existing data");
    Ok(report)
}

/// `entity`'s counterpart in `vault`: the entity of the same kind and name there, created (with its
/// aliases) when there is none.
#[allow(clippy::mutable_key_type)]
async fn twin(db: &OrgDb, owner: &RecordId, twins: &mut HashMap<(RecordId, RecordId), RecordId>, entity: &RecordId, vault: &RecordId) -> AppResult<RecordId> {
    let key = (entity.clone(), vault.clone());
    if let Some(found) = twins.get(&key) {
        return Ok(found.clone());
    }
    let row: EntityRow = store::get(db, entity).await?.ok_or_else(|| AppError::internal(format!("entity {} is gone", entity.to_string())))?;
    let id = if row.vault == *vault {
        entity.clone()
    } else {
        let out = crate::entities::service::upsert_entity(db, owner, entity.table(), &row.name, Some(row.aliases), Some(vault)).await?;
        crate::rid::parse(&out.id).map_err(|_| AppError::internal("entity id did not round-trip"))?
    };
    twins.insert(key, id.clone());
    Ok(id)
}

/// After a vault merge: every connector of `owner` whose default or rules name one of `from` now names
/// `to` (a vault `owner` is an active member of). Returns how many connectors changed.
pub async fn repoint(db: &OrgDb, owner: &RecordId, from: &[RecordId], to: &RecordId) -> AppResult<usize> {
    crate::authz::authorize(db, owner, crate::authz::Action::WriteMemories, to).await?;
    #[derive(serde::Deserialize, SurrealValue)]
    struct Row {
        id: RecordId,
        #[serde(default)]
        #[surreal(default)]
        routing: Value,
    }
    let from: Vec<String> = from.iter().map(|v| v.to_string()).collect();
    let rows: Vec<Row> = store::routing::ROUTED_CONNECTORS.on(db).bind(("owner", owner.clone())).await?.take(0)?;
    let mut changed = 0;
    for row in rows {
        let mut routing = Routing::of(&row.routing);
        let before = routing.clone();
        let swap = |v: &mut String| {
            if from.contains(v) {
                *v = to.to_string();
            }
        };
        if let Some(v) = routing.default_vault.as_mut() {
            swap(v);
        }
        routing.rules.iter_mut().for_each(|r| swap(&mut r.vault));
        if routing != before {
            set(db, &row.id, &routing).await?;
            changed += 1;
        }
    }
    Ok(changed)
}

/// Stores `routing` on connector `id`.
pub async fn set(db: &OrgDb, id: &RecordId, routing: &Routing) -> AppResult<()> {
    let value = serde_json::to_value(routing).map_err(|e| AppError::internal(e.to_string()))?;
    store::routing::SET_ROUTING.on(db).bind(("id", id.clone())).bind(("routing", value)).await?.check()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn routed(rules: &[(&str, &str)], keys: &[String]) -> Option<usize> {
        let rules: Vec<(String, String)> = rules.iter().map(|(f, v)| (f.to_string(), normalize(f, v))).collect();
        first_match(&rules, keys)
    }

    #[test]
    fn pocket_routes_on_tags_folder_and_title() {
        let raw = json!({"id": "r1", "title": "Build Paperless standup", "tags": [{"name": "Work"}, {"name": "weekly"}], "folder": {"name": "Clients"}});
        let keys = route_keys("heypocket", &raw, "Build Paperless standup");
        assert_eq!(routed(&[("tag", "work")], &keys), Some(0));
        assert_eq!(routed(&[("tag", "personal"), ("tag", "WEEKLY")], &keys), Some(1));
        assert_eq!(routed(&[("folder", "clients")], &keys), Some(0));
        assert_eq!(routed(&[("title", "paperless")], &keys), Some(0));
        assert_eq!(routed(&[("tag", "wor")], &keys), None, "a tag matches whole, not by prefix");
        let personal = json!({"id": "r2", "title": "Call with mum", "tags": []});
        assert_eq!(routed(&[("tag", "work"), ("title", "paperless")], &route_keys("heypocket", &personal, "Call with mum")), None);
        // a folder sent as a plain name, or as folder_name
        assert_eq!(routed(&[("folder", "home")], &route_keys("heypocket", &json!({"folder": "Home"}), "")), Some(0));
        assert_eq!(routed(&[("folder", "home")], &route_keys("heypocket", &json!({"folder_name": "Home"}), "")), Some(0));
    }

    #[test]
    fn first_matching_rule_wins() {
        let keys = route_keys("heypocket", &json!({"tags": [{"name": "work"}]}), "Paperless sync");
        assert_eq!(routed(&[("title", "paperless"), ("tag", "work")], &keys), Some(0));
        assert_eq!(routed(&[("tag", "work"), ("title", "paperless")], &keys), Some(0));
        assert_eq!(routed(&[("tag", "other"), ("title", "paperless")], &keys), Some(1));
    }

    #[test]
    fn github_routes_on_the_repo() {
        let keys = route_keys("github", &json!({"repository": {"full_name": "Acme/Paperless"}}), "Fix login");
        assert_eq!(routed(&[("repo", "acme/paperless")], &keys), Some(0));
        assert_eq!(routed(&[("repo", "acme/other")], &keys), None);
    }

    #[test]
    fn slack_and_discord_route_on_channel_id_or_name() {
        let keys = route_keys("slack", &json!({"_channel_id": "C123", "_channel_name": "build-paperless"}), "");
        assert_eq!(routed(&[("channel", "#build-paperless")], &keys), Some(0));
        assert_eq!(routed(&[("channel", "c123")], &keys), Some(0));
        let keys = route_keys("discord", &json!({"channel_id": "998", "_channel_name": "general"}), "");
        assert_eq!(routed(&[("channel", "998")], &keys), Some(0));
        assert_eq!(routed(&[("channel", "General")], &keys), Some(0));
    }

    #[test]
    fn gmail_routes_on_label_ids() {
        let keys = route_keys("gmail", &json!({"labelIds": ["INBOX", "Label_42"]}), "Invoice");
        assert_eq!(routed(&[("label", "label_42")], &keys), Some(0));
        assert_eq!(routed(&[("label", "spam")], &keys), None);
    }

    #[test]
    fn calendar_notion_linear_todoist_fields() {
        let keys = route_keys("google_calendar", &json!({"organizer": {"email": "me@x.com", "self": true}}), "1:1");
        assert_eq!(routed(&[("calendar", "primary")], &keys), Some(0));
        assert_eq!(routed(&[("calendar", "me@x.com")], &keys), Some(0));
        let keys = route_keys("notion", &json!({"parent": {"database_id": "1a2b-3c4d"}}), "Spec");
        assert_eq!(routed(&[("database", "1a2b3c4d")], &keys), Some(0), "dashes are ignored");
        assert_eq!(routed(&[("page", "1a2b3c4d")], &keys), None);
        let keys = route_keys("linear", &json!({"team": {"name": "Engineering"}}), "ENG-1: x");
        assert_eq!(routed(&[("team", "engineering")], &keys), Some(0));
        let keys = route_keys("todoist", &json!({"project_id": "p1", "_project_name": "Home"}), "Buy milk");
        assert_eq!(routed(&[("project", "home")], &keys), Some(0));
        assert_eq!(routed(&[("project", "p1")], &keys), Some(0));
    }

    #[test]
    fn other_sources_route_on_title_only() {
        for source in ["up_bank", "spotify", "stripe"] {
            assert_eq!(fields(source), &["title"]);
            let keys = route_keys(source, &json!({"description": "x"}), "Woolworths Paperless");
            assert_eq!(routed(&[("title", "woolworths")], &keys), Some(0), "{source}");
        }
    }

    #[test]
    fn a_record_stored_before_routing_matches_on_its_title() {
        assert_eq!(routed(&[("tag", "work"), ("title", "standup")], &legacy_keys("Daily Standup")), Some(1));
    }

    #[test]
    fn every_source_has_title_and_its_own_fields() {
        for src in crate::sources::registry::all() {
            assert!(fields(src.key()).contains(&"title"), "{}", src.key());
        }
        assert_eq!(fields("heypocket"), &["tag", "folder", "title"]);
    }

    #[test]
    fn stored_routing_round_trips_and_tolerates_absence() {
        assert_eq!(Routing::of(&Value::Null), Routing::default());
        assert_eq!(Routing::of(&json!({})), Routing::default());
        let r = Routing { default_vault: Some("vault:w".into()), rules: vec![Rule { field: "tag".into(), value: "work".into(), vault: "vault:w".into() }] };
        assert_eq!(Routing::of(&serde_json::to_value(&r).unwrap()), r);
    }
}
