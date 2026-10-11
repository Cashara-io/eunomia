//! The list of sources, and the operations that run across them: resolving a
//! source's connector credentials, and running one sync through the ingest
//! pipeline (`cache::ingest` -- upsert, links, embeddings).

use std::collections::HashSet;
use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;
use surrealdb::types::{RecordId, SurrealValue};

use crate::cache::ingest::{self, IngestReport};
use crate::cache::search::Envelope;
use crate::connectors::service;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::pool::OrgDb;
use crate::state::OrgState;
use crate::store;
use crate::sources::base::{Conn, Source, SourceCtx};
use crate::sources::routing;
use crate::sources::demo::DemoSource;
use crate::sources::discord::DiscordSource;
use crate::sources::github::GitHubSource;
use crate::sources::gmail::GmailSource;
use crate::sources::google_calendar::GoogleCalendarSource;
use crate::sources::heypocket::HeyPocketSource;
use crate::sources::linear::LinearSource;
use crate::sources::notion::NotionSource;
use crate::sources::slack::SlackSource;
use crate::sources::spotify::SpotifySource;
use crate::sources::stripe::StripeSource;
use crate::sources::todoist::TodoistSource;
use crate::sources::up_bank::UpBankSource;

/// Every source. Each has a connector kind in `connectors::service::CONNECTOR_KINDS`.
pub fn all() -> Vec<Arc<dyn Source>> {
    vec![
        Arc::new(UpBankSource) as Arc<dyn Source>,
        Arc::new(HeyPocketSource),
        Arc::new(GitHubSource),
        Arc::new(SlackSource),
        Arc::new(NotionSource),
        Arc::new(LinearSource),
        Arc::new(GmailSource),
        Arc::new(GoogleCalendarSource),
        Arc::new(DiscordSource),
        Arc::new(SpotifySource),
        Arc::new(TodoistSource),
        Arc::new(StripeSource),
    ]
}

/// A source by key -- any of [`all`], plus the synthetic `demo` source, which
/// can be synced on demand to seed test data but is never listed or scheduled.
pub fn get(key: &str) -> Option<Arc<dyn Source>> {
    if key == "demo" {
        return Some(Arc::new(DemoSource));
    }
    all().into_iter().find(|s| s.key() == key)
}

/// The source reading a connector kind's credentials (`pocketai` -> heypocket).
pub fn for_provider(kind: &str) -> Option<Arc<dyn Source>> {
    all().into_iter().find(|s| s.provider_key() == kind)
}

/// The decrypted credentials + config of this source's connector, scoped to `owner`.
pub async fn conn_for(db: &OrgDb, encryption_key: &str, owner: &RecordId, src: &dyn Source) -> AppResult<Conn> {
    let config = service::get_connector(db, owner, src.provider_key()).await?.map(|c| c.config).unwrap_or(Value::Null);
    let credentials = service::credentials_for(db, encryption_key, owner, src.provider_key()).await?;
    Ok(Conn { credentials, config })
}

#[derive(Debug, Deserialize, SurrealValue)]
struct ConnectorKindRow {
    kind: String,
    #[serde(default)]
    #[surreal(default)]
    config: Value,
}

/// Every source whose connector is enabled for `owner` (excluding connectors
/// left in the old demo mode).
pub async fn enabled(db: &OrgDb, owner: &RecordId) -> AppResult<Vec<Arc<dyn Source>>> {
    let mut res = store::app::CONNECTOR_ENABLED.on(db).bind(("owner", owner.clone())).await?;
    let rows: Vec<ConnectorKindRow> = res.take(0)?;
    let on: HashSet<String> = rows
        .into_iter()
        .filter(|r| !r.config.get("demo").and_then(|v| v.as_bool()).unwrap_or(false))
        .map(|r| r.kind)
        .collect();
    Ok(all().into_iter().filter(|s| on.contains(s.provider_key())).collect())
}

/// Persist -> map -> route -> ingest every raw record for `src`, scoped to `owner`. The connector's
/// routing is read once here; each raw record's envelopes all go where its first one (the record
/// itself, before any chunks) routes.
pub async fn ingest(state: &OrgState, owner: &RecordId, raw_records: &[Value], src: &dyn Source) -> AppResult<IngestReport> {
    let mut persist_errors = Vec::new();
    for raw in raw_records {
        if let Err(e) = src.persist(&state.db, owner, raw).await {
            persist_errors.push(format!("store {}: {}", raw.get("id").and_then(Value::as_str).unwrap_or("?"), e.message));
        }
    }
    let router = routing::Router::for_source(&state.db, owner, src).await?;
    let mut report = ingest::ingest(state, owner, src.key(), raw_records, |raw| {
        let mut envs: Vec<Envelope> =
            src.map_many(raw).into_iter().map(|env| serde_json::from_value::<Envelope>(env).map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
        if let Some(first) = envs.first() {
            let placement = router.placement(routing::route_keys(src.key(), raw, &first.title));
            for env in &mut envs {
                env.placement = Some(placement.clone());
            }
        }
        Ok(envs)
    })
    .await?;
    report.failed += persist_errors.len() as u64;
    report.errors.extend(persist_errors);
    Ok(report)
}

/// Deletes everything `owner` has from `src` ([`store::cache::DELETE_SOURCE_DATA`]): its cache records
/// (with their links), the facts and relations extracted from them, and anything the source stores
/// itself (heypocket's `pocket_recording`). Observations built from deleted facts go stale, or are
/// deleted if nothing else backs them. Entities, the connector's credentials and its sync cursor are
/// kept. Returns how many records and memories went.
pub async fn delete_data(db: &OrgDb, owner: &RecordId, src: &dyn Source) -> AppResult<Value> {
    let mut res = crate::tx::with_retry(|| async {
        store::cache::DELETE_SOURCE_DATA
            .on(db)
            .bind(("owner", owner.clone()))
            .bind(("source", src.key().to_string()))
            .await
    })
    .await?;
    Ok(res.take::<Option<Value>>(store::cache::DELETE_SOURCE_DATA.slot)?.unwrap_or(Value::Null))
}

/// Builds the context the cache-reading source helpers take.
pub fn ctx<'a>(db: &'a OrgDb, encryption_key: &'a str, owner: &'a RecordId) -> SourceCtx<'a> {
    SourceCtx { db, encryption_key, owner }
}

/// Fetch one source for `owner` and ingest what came back. Returns
/// `(report, next_cursor)`.
pub async fn run_sync(
    state: &OrgState,
    owner: &RecordId,
    key: &str,
    cursor: Option<String>,
) -> AppResult<(IngestReport, Option<String>)> {
    let src = get(key).ok_or_else(|| AppError::coded(ErrorCode::SourceNotFound, format!("no source {key:?}")))?;
    let conn = conn_for(&state.db, &state.settings.encryption_key, owner, src.as_ref()).await?;
    let result = src.fetch(&conn, cursor).await?;
    let report = ingest(state, owner, &result.records, src.as_ref()).await?;
    Ok((report, result.cursor))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_source_has_a_unique_key_and_a_real_connector_kind() {
        let keys: Vec<&str> = all().iter().map(|s| s.key()).collect();
        let unique: HashSet<&str> = keys.iter().copied().collect();
        assert_eq!(unique.len(), keys.len());
        for src in all() {
            assert!(service::CONNECTOR_KINDS.contains(&src.provider_key()), "{} has no connector kind", src.key());
        }
        // ...and every connector kind the UI offers is backed by a source.
        for kind in service::CONNECTOR_KINDS {
            assert!(for_provider(kind).is_some(), "connector {kind} has no source");
        }
    }

    #[test]
    fn get_and_for_provider_resolve_keys() {
        assert!(get("nonexistent").is_none());
        assert!(get("demo").is_some(), "seedable on demand");
        assert!(all().iter().all(|s| s.key() != "demo"), "but never listed or scheduled");
        assert_eq!(get("heypocket").unwrap().provider_key(), "pocketai");
        assert_eq!(for_provider("pocketai").unwrap().key(), "heypocket");
        assert_eq!(for_provider("up_bank").unwrap().key(), "up_bank");
    }
}
