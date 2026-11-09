//! The source plug-in contract, plus the small helpers every source's mapper
//! shares.
//!
//! A source is one provider's fetch + map pair: [`Source::fetch`] pulls raw
//! provider JSON (through `connectors::clients::Api`) using the connector's
//! decrypted credentials and config ([`Conn`]); [`Source::map`] turns one raw
//! value into a cache envelope. `sources::registry` lists the sources and runs
//! them through the ingest pipeline; neither half touches the database, so
//! both are tested against a local mock of the provider's API
//! (`sources::mock`). A source that keeps its own tables besides the cache
//! (heypocket) does so in [`Source::persist`].

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::header::HeaderMap;
use serde_json::{json, Value};
use surrealdb::types::RecordId;

use crate::error::AppResult;
use crate::pool::OrgDb;
use crate::rid::RecordIdExt;

/// Raw records fetched from an origin, plus the opaque cursor to feed back
/// into the next sync.
#[derive(Debug, Clone, Default)]
pub struct SyncResult {
    pub records: Vec<Value>,
    pub cursor: Option<String>,
    pub progress: SyncProgress,
    /// Permanent record failures must be visible even when the rest of the batch succeeds.
    pub warnings: Vec<String>,
}

/// A partial fetch is ingestible progress, including zero records. It is not a
/// completed poll or a failure. Save its cursor only after all records ingest.
#[derive(Debug, Clone, Default)]
pub enum SyncProgress {
    #[default]
    Complete,
    Partial { reason: SyncPause, retry_after_secs: u64 },
}

#[derive(Debug, Clone)]
pub enum SyncPause {
    RateLimited,
    Transient,
    MoreRemaining,
}

impl SyncProgress {
    pub fn interrupted(error: &crate::error::AppError) -> Self {
        Self::Partial {
            reason: if error.reason == Some("connector_rate_limited") { SyncPause::RateLimited } else { SyncPause::Transient },
            retry_after_secs: error.retry_after_secs.unwrap_or(60).max(60),
        }
    }

    pub fn remaining() -> Self {
        Self::Partial { reason: SyncPause::MoreRemaining, retry_after_secs: 60 }
    }

    pub fn message(&self, imported: u64) -> Option<String> {
        match self {
            Self::Complete => None,
            Self::Partial { reason, .. } => {
                let cause = match reason {
                    SyncPause::RateLimited => "rate limited",
                    SyncPause::Transient => "provider temporarily unavailable",
                    SyncPause::MoreRemaining => "import in progress",
                };
                Some(format!("{cause}, {imported} imported, more remaining"))
            }
        }
    }
}

/// One connector row's decrypted credentials and plain config -- everything
/// a source needs to reach its provider.
#[derive(Clone, Default)]
pub struct Conn {
    pub credentials: Value,
    pub config: Value,
}

impl Drop for Conn {
    fn drop(&mut self) {
        crate::secrets::broker::zeroize_strings(&mut self.credentials);
    }
}

impl std::fmt::Debug for Conn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Conn").field("credentials", &"[secret]").field("config", &self.config).finish()
    }
}

/// Database handle + owner for the cache-reading helpers some sources expose
/// (`up_bank::finance_summary`, `heypocket::summary`, ...).
pub struct SourceCtx<'a> {
    pub db: &'a OrgDb,
    pub encryption_key: &'a str,
    pub owner: &'a RecordId,
}

#[async_trait]
pub trait Source: Send + Sync {
    /// Stable identifier, e.g. "up_bank" or "heypocket".
    fn key(&self) -> &'static str;

    /// The connector kind holding this source's credentials, when it differs
    /// from `key()` (heypocket's is "pocketai").
    fn provider(&self) -> &'static str {
        ""
    }

    /// Human label for the UI.
    fn label(&self) -> &'static str;

    /// Envelope `type` values this source emits, e.g. `["up.transaction"]`.
    fn record_types(&self) -> &'static [&'static str];

    fn provider_key(&self) -> &'static str {
        let p = self.provider();
        if p.is_empty() {
            self.key()
        } else {
            p
        }
    }

    /// One cheap authenticated call proving the credentials work -- what the
    /// Connectors page's "Test connection" runs.
    async fn check(&self, conn: &Conn) -> AppResult<()>;

    /// Raw records changed since `cursor` (`None` on the first sync), plus
    /// the cursor for the next sync. Hard provider errors surface in
    /// `sync_status.last_error`. A source may return a safe partial prefix
    /// on a temporary failure, with explicit `SyncProgress::Partial`, including
    /// zero-record pauses. The scheduler persists the reason and retry delay.
    async fn fetch(&self, conn: &Conn, cursor: Option<String>) -> AppResult<SyncResult>;

    /// One raw origin value -> a cache envelope, or `None` to skip.
    fn map(&self, raw: &Value) -> Option<Value>;

    /// Every envelope for one raw value: [`Source::map`]'s, unless a source splits a record
    /// (heypocket chunks long transcripts).
    fn map_many(&self, raw: &Value) -> Vec<Value> {
        self.map(raw).into_iter().collect()
    }

    /// Stores one raw value outside the cache, before it is ingested (heypocket's own tables). A
    /// failure counts as a failed record, so the sync cursor holds and the record is retried. `conn` is
    /// the connection it came through (credentials and config), when there is one.
    async fn persist(&self, _db: &OrgDb, _owner: &RecordId, _raw: &Value, _conn: Option<&Conn>) -> AppResult<()> {
        Ok(())
    }

    /// Whether [`Source::webhook`] does anything. The route answers "ignored" without opening the
    /// owner's database for a source that has none.
    fn has_webhook(&self) -> bool {
        false
    }

    /// Verify + translate a provider push into raw records (or `None`).
    /// Default: no webhook support.
    async fn webhook(&self, _conn: &Conn, _headers: &HeaderMap, _body: &[u8]) -> AppResult<Option<Vec<Value>>> {
        Ok(None)
    }
}

/// Upper bound on pages followed in one sync, so a runaway cursor can't
/// loop forever; the next sync resumes from the saved cursor.
pub const MAX_PAGES: usize = 50;

/// The cache envelope every mapper builds. `id` is the cache literal
/// `"{source}:{type}:{external_id}"`; callers set `links`/`deleted` on the
/// result when they have them.
#[allow(clippy::too_many_arguments)]
pub fn envelope(
    source: &str,
    type_: &str,
    external_id: &str,
    title: &str,
    body_text: &str,
    occurred_at: Option<String>,
    url: &str,
    payload: Value,
) -> Value {
    json!({
        "id": format!("{source}:{type_}:{external_id}"),
        "source": source,
        "type": type_,
        "external_id": external_id,
        "title": title,
        "body_text": body_text,
        "occurred_at": occurred_at,
        "url": url,
        "payload": payload,
        "links": [],
        "deleted": false,
    })
}

/// The string at JSON `pointer` (e.g. `"/subject/title"`), or "".
pub fn s<'a>(v: &'a Value, pointer: &str) -> &'a str {
    v.pointer(pointer).and_then(|v| v.as_str()).unwrap_or("")
}

/// The array at `pointer`, or empty.
pub fn items(v: &Value, pointer: &str) -> Vec<Value> {
    v.pointer(pointer).and_then(|v| v.as_array()).cloned().unwrap_or_default()
}

/// Any RFC 3339 timestamp normalized to UTC (`cache_record.occurred_at` is a
/// datetime field; anything unparseable becomes `None`, not a failed write).
pub fn rfc3339(value: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(value).ok().map(|dt| dt.with_timezone(&Utc).to_rfc3339())
}

pub fn from_unix(secs: i64) -> Option<String> {
    DateTime::<Utc>::from_timestamp(secs, 0).map(|d| d.to_rfc3339())
}

/// The caller-facing half of a user record id, e.g. `"abc123"` out of
/// `user:abc123`.
pub fn owner_key_str(owner: &RecordId) -> String {
    let s = owner.to_string();
    match s.split_once(':') {
        Some((_, rest)) => rest.to_string(),
        None => s,
    }
}

/// Converts a `surrealdb::types::Datetime` to a `chrono::DateTime<Utc>` by
/// round-tripping through its `Serialize` impl (an RFC3339 string).
pub fn datetime_to_chrono(d: &surrealdb::types::Datetime) -> Option<DateTime<Utc>> {
    let value = serde_json::to_value(d).ok()?;
    let s = value.as_str()?;
    DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_key_str_strips_table_prefix() {
        let owner: RecordId = crate::rid::parse("user:abc123").unwrap();
        assert_eq!(owner_key_str(&owner), "abc123");
    }

    #[test]
    fn datetime_to_chrono_round_trips() {
        let now = Utc::now();
        let now = DateTime::parse_from_rfc3339(&now.to_rfc3339()).unwrap().with_timezone(&Utc);
        let sd: surrealdb::types::Datetime = now.into();
        assert_eq!(datetime_to_chrono(&sd), Some(now));
    }

    #[test]
    fn rfc3339_normalizes_offsets_and_rejects_garbage() {
        assert_eq!(rfc3339("2024-01-02T10:00:00+10:00").unwrap(), "2024-01-02T00:00:00+00:00");
        assert!(rfc3339("Mon, 1 Jan 2024").is_none());
        assert_eq!(from_unix(0).unwrap(), "1970-01-01T00:00:00+00:00");
    }

    #[test]
    fn envelope_builds_the_cache_literal_id() {
        let env = envelope("github", "github.issue", "7", "T", "B", None, "", json!({}));
        assert_eq!(env["id"], "github:github.issue:7");
        assert_eq!(env["deleted"], false);
    }
}
