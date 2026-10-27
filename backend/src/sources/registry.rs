//! The list of sources, and the operations that run across them: resolving a
//! source's connector credentials, and running one sync through the ingest
//! pipeline (`cache::ingest` -- upsert, links, embeddings).

use std::sync::Arc;

use serde_json::Value;
use surrealdb::types::RecordId;

use crate::cache::ingest::{self, IngestReport};
use crate::cache::search::Envelope;
use crate::connectors::service::{self, Connector};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;
use crate::state::OrgState;
use crate::store;
use crate::sources::base::{owner_key_str, Conn, Source, SourceCtx};
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

/// The key one connection syncs under (its `sync_status` row, its sync jobs, its `/api/sources` row): the
/// source key for a kind's first connection (as before 2.0.4), `{source}~{instance}` for each added one.
pub fn sync_key(src: &dyn Source, conn: &Connector) -> String {
    if conn.is_first() {
        src.key().to_string()
    } else {
        format!("{}~{}", src.key(), conn.instance)
    }
}

/// A [`sync_key`] split into the source key and the connection's instance ("" for the first one).
pub fn split_key(key: &str) -> (&str, &str) {
    key.split_once('~').unwrap_or((key, ""))
}

/// The source and connection a [`sync_key`] names, if `owner` has it. A kind's first connection may
/// not exist yet (`None` connection), an added one must.
pub async fn connection(db: &OrgDb, owner: &RecordId, key: &str) -> AppResult<Option<(Arc<dyn Source>, Option<Connector>)>> {
    let (source, instance) = split_key(key);
    let Some(src) = get(source) else { return Ok(None) };
    let conn = service::get_connector(db, owner, src.provider_key(), instance).await?;
    if conn.is_none() && !instance.is_empty() {
        return Ok(None);
    }
    Ok(Some((src, conn)))
}

/// The decrypted credentials + config of one connection (empty when there is none).
pub fn conn_for(encryption_key: &str, row: Option<&Connector>) -> AppResult<Conn> {
    let Some(row) = row else { return Ok(Conn::default()) };
    Ok(Conn { credentials: service::credentials_of(encryption_key, row)?, config: row.config.clone() })
}

/// Every enabled connection of `owner` with its source (excluding connectors left in the old demo mode).
pub async fn enabled(db: &OrgDb, owner: &RecordId) -> AppResult<Vec<(Arc<dyn Source>, Connector)>> {
    let mut res = store::app::CONNECTOR_ENABLED.on(db).bind(("owner", owner.clone())).await?;
    let rows: Vec<Connector> = res.take(0)?;
    Ok(rows
        .into_iter()
        .filter(|r| !r.config.get("demo").and_then(|v| v.as_bool()).unwrap_or(false))
        .filter_map(|r| for_provider(&r.kind).map(|src| (src, r)))
        .collect())
}

/// [`ingest_via`] the first connection of `src`'s kind (what a caller without a connection means).
pub async fn ingest(state: &OrgState, owner: &RecordId, raw_records: &[Value], src: &dyn Source) -> AppResult<IngestReport> {
    let conn = service::get_connector(&state.db, owner, src.provider_key(), "").await?;
    ingest_via(state, owner, raw_records, src, conn.as_ref()).await
}

/// Persist -> map -> route -> ingest every raw record for `src`, scoped to `owner`, each record marked
/// as synced through `conn`. The connection's routing is read once here; each raw record's envelopes
/// all go where its first one (the record itself, before any chunks) routes.
pub async fn ingest_via(
    state: &OrgState,
    owner: &RecordId,
    raw_records: &[Value],
    src: &dyn Source,
    conn: Option<&Connector>,
) -> AppResult<IngestReport> {
    let mut persist_errors = Vec::new();
    for raw in raw_records {
        if let Err(e) = src.persist(&state.db, owner, raw).await {
            persist_errors.push(format!("store {}: {}", raw.get("id").and_then(Value::as_str).unwrap_or("?"), e.message));
        }
    }
    let router = routing::Router::for_connection(&state.db, owner, src, conn).await?;
    let connector = conn.map(|c| c.id.clone());
    let mut report = ingest::ingest(state, owner, src.key(), raw_records, |raw| {
        let mut envs: Vec<Envelope> =
            src.map_many(raw).into_iter().map(|env| serde_json::from_value::<Envelope>(env).map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
        if let Some(first) = envs.first() {
            let placement = router.placement(routing::route_keys(src.key(), raw, &first.title));
            for env in &mut envs {
                env.placement = Some(placement.clone());
            }
        }
        for env in &mut envs {
            env.connector = connector.clone();
        }
        Ok(envs)
    })
    .await?;
    report.failed += persist_errors.len() as u64;
    report.errors.extend(persist_errors);
    Ok(report)
}

/// Deletes everything `owner` synced through `conn` ([`store::cache::DELETE_SOURCE_DATA`]): its cache
/// records (with their links), the facts and relations extracted from them, and anything the source
/// stores itself (heypocket's `pocket_recording`). Observations built from deleted facts go stale, or
/// are deleted if nothing else backs them. Entities, the connector's credentials and its sync cursor are
/// kept. Returns how many records and memories went.
pub async fn delete_data(db: &OrgDb, owner: &RecordId, conn: &Connector) -> AppResult<Value> {
    let src = for_provider(&conn.kind).ok_or_else(|| AppError::coded(ErrorCode::ConnectorNotFound, "unknown connector"))?;
    let mut res = crate::tx::with_retry(|| async {
        store::cache::DELETE_SOURCE_DATA
            .on(db)
            .bind(("owner", owner.clone()))
            .bind(("source", src.key().to_string()))
            .bind(("connector", conn.id.clone()))
            .bind(("first", conn.is_first()))
            .await
    })
    .await?;
    let mut report = res.take::<Option<Value>>(store::cache::DELETE_SOURCE_DATA.slot)?.unwrap_or(Value::Null);
    crate::changes::record_deleted_memories(db, &mut report).await;
    Ok(report)
}

/// Removes one connection: its data ([`delete_data`]), its sync status and the connection itself. A
/// kind's first connection comes back empty the next time the connectors are listed.
pub async fn delete_connection(db: &OrgDb, owner: &RecordId, conn: &Connector) -> AppResult<Value> {
    let out = delete_data(db, owner, conn).await?;
    let src = for_provider(&conn.kind).ok_or_else(|| AppError::coded(ErrorCode::ConnectorNotFound, "unknown connector"))?;
    let status = RecordId::from_table_key("sync_status", format!("{}:{}", owner_key_str(owner), sync_key(src.as_ref(), conn)));
    crate::tx::with_retry(|| async {
        store::app::CONNECTOR_DELETE.on(db).bind(("status", status.clone())).bind(("id", conn.id.clone())).await?.check()
    })
    .await?;
    Ok(out)
}

/// Builds the context the cache-reading source helpers take.
pub fn ctx<'a>(db: &'a OrgDb, encryption_key: &'a str, owner: &'a RecordId) -> SourceCtx<'a> {
    SourceCtx { db, encryption_key, owner }
}

/// Fetch one connection ([`sync_key`]) for `owner` and ingest what came back. Returns
/// `(report, next_cursor)`.
pub async fn run_sync(
    state: &OrgState,
    owner: &RecordId,
    key: &str,
    cursor: Option<String>,
) -> AppResult<(IngestReport, Option<String>)> {
    let (src, row) = connection(&state.db, owner, key)
        .await?
        .ok_or_else(|| AppError::coded(ErrorCode::SourceNotFound, format!("no source {key:?}")))?;
    let conn = conn_for(&state.settings.encryption_key, row.as_ref())?;
    let result = src.fetch(&conn, cursor).await?;
    let report = ingest_via(state, owner, &result.records, src.as_ref(), row.as_ref()).await?;
    Ok((report, result.cursor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

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

    #[test]
    fn sync_keys_name_the_first_connection_by_its_source_and_others_by_instance() {
        let mut conn = Connector {
            id: crate::rid::parse("connector:abc").unwrap(),
            owner: crate::rid::parse("user:u").unwrap(),
            kind: "pocketai".into(),
            instance: String::new(),
            label: String::new(),
            enabled: true,
            config: Value::Null,
            credentials_encrypted: String::new(),
            updated_at: chrono::Utc::now().into(),
            routing: Value::Null,
        };
        assert_eq!(sync_key(&HeyPocketSource, &conn), "heypocket");
        conn.instance = "abc".into();
        assert_eq!(sync_key(&HeyPocketSource, &conn), "heypocket~abc");
        assert_eq!(split_key("heypocket~abc"), ("heypocket", "abc"));
        assert_eq!(split_key("github"), ("github", ""));
    }
}
