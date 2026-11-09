//! Gmail source: messages from the last 30 days, then everything newer than
//! a saved mailbox history ID. Bootstrap uses `users.messages.list`, then
//! `users.history.list` discovers additions regardless of message date.
//! `users.messages.get?format=full` reads each message for headers and
//! the plain-text body.
//!
//! Google has no personal access tokens, so the user brings their own OAuth
//! client: `client_id` + `client_secret` + a `refresh_token` (scope
//! `gmail.readonly`, e.g. minted in the OAuth Playground -- see
//! docs/connectors.md). Each sync exchanges the refresh token for a fresh
//! access token ([`google_api`], shared with Google Calendar).

use async_trait::async_trait;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::time::Duration;

use crate::connectors::clients::{bearer, refresh_access_token, retryable, Api};
use crate::error::{AppError, AppResult};
use crate::sources::base::{envelope, from_unix, items, s, Conn, Source, SyncProgress, SyncResult, MAX_PAGES};

const BASE_URL: &str = "https://gmail.googleapis.com/gmail/v1";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// Each message costs one extra request; bound one sync. When more are
/// waiting, the oldest are read first and the cursor resumes after them, so
/// a backlog drains over several syncs instead of starving.
const MAX_MESSAGES_PER_SYNC: usize = 200;
const MAX_BODY_CHARS: usize = 20_000;

/// A Google API authenticated with a freshly refreshed access token.
pub(crate) async fn google_api(conn: &Conn, base: &str) -> AppResult<Api> {
    let token = refresh_access_token(&conn.config, GOOGLE_TOKEN_URL, &conn.credentials).await?;
    Ok(Api::new(&conn.config, base, bearer(&token)).with_google_retries())
}

fn header(raw: &Value, name: &str) -> String {
    items(raw, "/payload/headers")
        .iter()
        .find(|h| s(h, "/name").eq_ignore_ascii_case(name))
        .map(|h| s(h, "/value").to_string())
        .unwrap_or_default()
}

/// The first `text/plain` part anywhere in the MIME tree, decoded.
fn plain_text(part: &Value) -> Option<String> {
    if s(part, "/mimeType") == "text/plain" {
        let data = s(part, "/body/data").trim_end_matches('=');
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(data).ok()?;
        return Some(String::from_utf8_lossy(&bytes).to_string());
    }
    items(part, "/parts").iter().find_map(plain_text)
}

/// `occurred_at` from the RFC 2822 `Date` header (normalized to UTC), else
/// Gmail's `internalDate` (epoch milliseconds).
fn sent_at(raw: &Value) -> Option<String> {
    let date = header(raw, "Date");
    // Drop a trailing comment like "(UTC)" or "(PDT)", which chrono rejects.
    let date = date.split(" (").next().unwrap_or("").trim();
    chrono::DateTime::parse_from_rfc2822(date)
        .ok()
        .map(|d| d.with_timezone(&chrono::Utc).to_rfc3339())
        .or_else(|| s(raw, "/internalDate").parse::<i64>().ok().and_then(|ms| from_unix(ms / 1000)))
}

/// Numeric 2.2.0 cursors and the earlier JSON snapshot both upgrade here.
/// History is captured before a relist and consumed before any message reads,
/// closing the race between listing a mailbox and backdated imports arriving.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct MessageCursor {
    after: Option<String>,
    pending: Vec<String>,
    seen: Vec<String>,
    history: Option<String>,
    message_failures: std::collections::BTreeMap<String, u8>,
    skipped: u64,
}

const MAX_MESSAGE_ERROR_SYNCS: u8 = 3;
const MAX_CURSOR_BYTES: usize = 1_048_576;
const MAX_PENDING: usize = 25_000;

impl MessageCursor {
    fn read(cursor: Option<String>) -> AppResult<Self> {
        match cursor {
            Some(c) if c.starts_with('{') => serde_json::from_str(&c).map_err(|_| AppError::internal("invalid Gmail cursor")),
            after => Ok(Self { after, ..Self::default() }),
        }
    }

    fn warnings(&self) -> Vec<String> {
        if self.skipped == 0 { Vec::new() } else { vec![format!("{} messages skipped after repeated errors", self.skipped)] }
    }

    fn encode(&self) -> AppResult<String> {
        let encoded = serde_json::to_string(self).map_err(|_| AppError::internal("invalid Gmail cursor"))?;
        if self.pending.len() > MAX_PENDING || encoded.len() > MAX_CURSOR_BYTES {
            // Failing leaves the persisted checkpoint intact. Never truncate unread IDs.
            return Err(AppError::internal("Gmail cursor exceeded its safe size; keeping the previous checkpoint for replay"));
        }
        Ok(encoded)
    }

    fn checkpoint(&mut self, records: &[Value]) -> AppResult<()> {
        let dates: Vec<i64> = records.iter().map(|r| s(r, "/internalDate").parse::<i64>()
            .map_err(|_| AppError::internal("Gmail message has no valid internalDate; keeping the previous cursor"))).collect::<AppResult<_>>()?;
        if let Some(newest) = dates.iter().max().map(|ms| ms / 1000) {
            // Backdated history additions must never move the date checkpoint backwards.
            let newest = self.after.as_deref().and_then(|a| a.parse::<i64>().ok()).map_or(newest, |old| newest.max(old.saturating_add(1)));
            self.after = Some((newest - 1).to_string());
        }
        // History replaces timestamp-boundary deduplication. Removed IDs cannot accumulate.
        self.seen.clear();
        Ok(())
    }

    async fn relist(&mut self, api: &dyn GmailReader, full_mailbox: bool) -> AppResult<()> {
        let history = api.get("/users/me/profile", &[]).await?;
        let history = s(&history, "/historyId");
        if history.is_empty() { return Err(AppError::internal("Gmail profile has no historyId")); }
        let q = if full_mailbox { String::new() } else {
            self.after.as_deref().map(|c| format!("after:{c}")).unwrap_or_else(|| "newer_than:30d".to_string())
        };
        let mut page_token = String::new();
        let mut listed = Vec::new();
        for _ in 0..MAX_PAGES {
            let mut query = vec![("maxResults", "500".to_string())];
            if !q.is_empty() { query.push(("q", q.clone())); }
            if !page_token.is_empty() { query.push(("pageToken", page_token.clone())); }
            let page = api.get("/users/me/messages", &query).await?;
            listed.extend(items(&page, "/messages").iter().map(|m| s(m, "/id").to_string()));
            page_token = s(&page, "/nextPageToken").to_string();
            if page_token.is_empty() { break; }
        }
        if !page_token.is_empty() {
            return Err(AppError::internal("Gmail listing exceeded 25,000 messages; keeping the cursor so older mail is not skipped"));
        }
        let live: HashSet<_> = listed.iter().cloned().collect();
        self.seen.retain(|id| live.contains(id));
        let mut known: HashSet<_> = self.pending.iter().chain(&self.seen).cloned().collect();
        // Keep existing pending order; newly discovered messages go behind it.
        listed.retain(|id| !id.is_empty() && known.insert(id.clone()));
        listed.append(&mut self.pending);
        self.pending = listed;
        self.history = Some(history.to_string());
        Ok(())
    }

    async fn additions(&mut self, api: &dyn GmailReader) -> AppResult<()> {
        let start = self.history.clone().ok_or_else(|| AppError::internal("missing Gmail history checkpoint"))?;
        let mut page_token = String::new();
        let mut added = Vec::new();
        let mut removed = HashSet::new();
        let mut next_history = String::new();
        for _ in 0..MAX_PAGES {
            let mut query = vec![("startHistoryId", start.clone()), ("maxResults", "500".to_string())];
            if !page_token.is_empty() { query.push(("pageToken", page_token.clone())); }
            let page = api.get("/users/me/history", &query).await?;
            for change in items(&page, "/history") {
                for entry in items(&change, "/messagesDeleted") {
                    let id = s(&entry, "/message/id").to_string();
                    removed.insert(id.clone());
                    added.retain(|a| a != &id);
                }
                for entry in items(&change, "/messagesAdded") {
                    let id = s(&entry, "/message/id").to_string();
                    removed.remove(&id);
                    added.push(id);
                }
            }
            next_history = s(&page, "/historyId").to_string();
            page_token = s(&page, "/nextPageToken").to_string();
            if page_token.is_empty() { break; }
        }
        if !page_token.is_empty() || next_history.is_empty() {
            return Err(AppError::internal("Gmail history listing incomplete; keeping the previous checkpoint"));
        }
        self.pending.retain(|id| !removed.contains(id));
        self.message_failures.retain(|id, _| !removed.contains(id));
        for id in &removed { tracing::info!(message_id = %id, "Gmail message removed"); }
        let mut known: HashSet<_> = self.pending.iter().chain(&self.seen).cloned().collect();
        added.retain(|id| !id.is_empty() && known.insert(id.clone()));
        // history.list is oldest first, whereas pending is popped from the end.
        added.reverse();
        added.append(&mut self.pending);
        self.pending = added;
        self.history = Some(next_history);
        self.seen.clear();
        Ok(())
    }
}

/// The same checkpoint algorithm serves direct Google and Composio reads.
#[async_trait]
pub(crate) trait GmailReader: Send + Sync {
    async fn get(&self, path: &str, query: &[(&str, String)]) -> AppResult<Value>;
}
#[async_trait]
impl GmailReader for Api {
    async fn get(&self, path: &str, query: &[(&str, String)]) -> AppResult<Value> { Api::get(self, path, query).await }
}
pub(crate) async fn fetch_with(api: &dyn GmailReader, reader: &dyn GmailReader, cursor: Option<String>) -> AppResult<SyncResult> {
        let mut state = MessageCursor::read(cursor)?;
        let discovery = async {
            if state.history.is_none() { state.relist(api, false).await?; }
            match state.additions(api).await {
                Err(error) if error.reason == Some("connector_removed") => {
                    // Expired history can hide imports older than the date checkpoint.
                    tracing::warn!("Gmail history expired; relisting the mailbox for replay");
                    state.relist(api, true).await?;
                    state.additions(api).await
                }
                result => result,
            }
        }.await;
        if let Err(error) = discovery {
            if retryable(&error) {
                return Ok(SyncResult { cursor: Some(state.encode()?), progress: SyncProgress::interrupted(&error), warnings: state.warnings(), ..SyncResult::default() });
            }
            return Err(error);
        }
        // Check the bound before spending quota or importing any records.
        state.encode()?;
        let mut records = Vec::new();
        let mut message_errors = Vec::new();
        let mut progress = SyncProgress::Complete;
        for _ in 0..MAX_MESSAGES_PER_SYNC {
            let Some(id) = state.pending.last().cloned() else { break };
            match reader.get(&format!("/users/me/messages/{id}"), &[("format", "full".to_string())]).await {
                Ok(record) => {
                    records.push(record);
                    state.pending.pop();
                    state.message_failures.remove(&id);
                }
                Err(error) if error.reason == Some("connector_removed") => {
                    tracing::info!(message_id = %id, "Gmail message removed");
                    state.pending.pop();
                    state.message_failures.remove(&id);
                    state.seen.retain(|seen| seen != &id);
                }
                Err(error) if matches!(error.reason, Some("connector_message_error" | "connector_server_error")) => {
                    // Try other IDs before deciding whether this is a message-specific failure.
                    state.pending.pop();
                    message_errors.push((id, error));
                }
                Err(error) if retryable(&error) => {
                    progress = SyncProgress::interrupted(&error);
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        for (id, error) in message_errors.into_iter().rev() {
            let per_message = error.reason == Some("connector_message_error") || !records.is_empty();
            if per_message {
                let message_failures = state.message_failures.entry(id.clone()).or_default();
                *message_failures = message_failures.saturating_add(1);
                if *message_failures >= MAX_MESSAGE_ERROR_SYNCS {
                    tracing::warn!(message_id = %id, "Gmail message skipped after repeated HTTP errors");
                    state.skipped = state.skipped.saturating_add(1);
                    state.message_failures.remove(&id);
                    continue;
                }
            }
            state.pending.push(id);
            if matches!(progress, SyncProgress::Complete) { progress = SyncProgress::interrupted(&error); }
        }
        state.checkpoint(&records)?;
        if !state.pending.is_empty() && matches!(progress, SyncProgress::Complete) { progress = SyncProgress::remaining(); }
        Ok(SyncResult { records, cursor: Some(state.encode()?), progress, warnings: state.warnings() })
}

pub struct GmailSource;

#[async_trait]
impl Source for GmailSource {
    fn key(&self) -> &'static str {
        "gmail"
    }

    fn label(&self) -> &'static str {
        "Gmail"
    }

    fn record_types(&self) -> &'static [&'static str] {
        &["gmail.message"]
    }

    async fn check(&self, conn: &Conn) -> AppResult<()> {
        google_api(conn, BASE_URL).await?.get("/users/me/profile", &[]).await.map(|_| ())
    }

    async fn fetch(&self, conn: &Conn, cursor: Option<String>) -> AppResult<SyncResult> {
        let api = google_api(conn, BASE_URL).await?;
        let spacing = if cfg!(test) { Duration::from_millis(1) } else { Duration::from_secs(1) };
        let reader = api.clone().with_pacing(spacing);
        fetch_with(&api, &reader, cursor).await
    }

    fn map(&self, raw: &Value) -> Option<Value> {
        let id = raw.get("id").and_then(|v| v.as_str())?;
        let subject = header(raw, "Subject");
        let from = header(raw, "From");
        let body: String = plain_text(&raw["payload"]).unwrap_or_else(|| s(raw, "/snippet").to_string()).chars().take(MAX_BODY_CHARS).collect();
        Some(envelope(
            "gmail",
            "gmail.message",
            id,
            if subject.is_empty() { "(no subject)" } else { &subject },
            &format!("From: {from}\n{}", body.trim()),
            sent_at(raw),
            &format!("https://mail.google.com/mail/u/0/#all/{}", s(raw, "/threadId")),
            json!({
                "from": from,
                "to": header(raw, "To"),
                "thread_id": raw.get("threadId"),
                "label_ids": raw.get("labelIds"),
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::mock::{assert_fetch_fails, envelopes, route, serve as serve_mock};
    use std::collections::HashSet;

    fn with_history(mut routes: Vec<crate::sources::mock::Route>) -> Vec<crate::sources::mock::Route> {
        routes.extend([
            route("GET", "/users/me/profile", json!({"historyId": "100"})),
            route("GET", "/users/me/history", json!({"historyId": "100"})),
        ]);
        routes
    }

    async fn serve(routes: Vec<crate::sources::mock::Route>) -> crate::sources::mock::Mock {
        serve_mock(with_history(routes)).await
    }

    // Review regressions also provide history routes for the incremental implementation.
    fn token_and_history(history: Value) -> Vec<crate::sources::mock::Route> {
        vec![
            route("POST", "/oauth/token", json!({"access_token": "t"})),
            route("GET", "/users/me/profile", json!({"historyId": "100"})),
            route("GET", "/users/me/history", history),
        ]
    }

    #[tokio::test]
    async fn review_deleted_pending_id_does_not_block_later_mail() {
        for code in [404, 410] {
            let mut routes = token_and_history(json!({"historyId": "101"}));
            routes.extend([
                route("GET", "/users/me/messages", json!({"messages": [{"id": "later"}]})),
                route("GET", "/users/me/messages/gone", json!({})).status(code),
                route("GET", "/users/me/messages/later", message("later", "1700000200000", "Later")),
            ]);
            let mock = serve(routes).await;
            let old = json!({"after": "1700000000", "pending": ["later", "gone"], "seen": []}).to_string();
            let res = GmailSource.fetch(&mock.conn(json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"})), Some(old)).await.unwrap();
            assert_eq!(res.records.iter().map(|r| s(r, "/id")).collect::<Vec<_>>(), ["later"]);
            assert!(MessageCursor::read(res.cursor).unwrap().pending.is_empty());
        }
    }

    #[tokio::test]
    async fn review_backdated_import_during_recovery_is_read_exactly_once() {
        let mut routes = token_and_history(json!({"historyId": "100"}));
        routes.extend([
            route("GET", "/users/me/messages", json!({"messages": [{"id": "newest"}, {"id": "oldest"}]})),
            route("GET", "/users/me/messages/oldest", message("oldest", "1700000100000", "Oldest")),
            route("GET", "/users/me/messages/newest", json!({})).status(429).header("retry-after", "60"),
        ]);
        let mock = serve(routes).await;
        let conn = mock.conn(json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"}));
        let first = GmailSource.fetch(&conn, Some("1700000000".into())).await.unwrap();
        let mut recovered = token_and_history(json!({"historyId": "101", "history": [{"messagesAdded": [{"message": {"id": "imported"}}]}]}));
        recovered.extend([
            route("GET", "/users/me/messages", json!({"messages": [{"id": "newest"}, {"id": "imported"}, {"id": "oldest"}]})),
            route("GET", "/users/me/messages/imported", message("imported", "1600000000000", "Backdated")),
            route("GET", "/users/me/messages/newest", message("newest", "1700000300000", "Newest")),
        ]);
        mock.set_routes(with_history(recovered));
        let second = GmailSource.fetch(&conn, first.cursor).await.unwrap();
        let mut quiet = token_and_history(json!({"historyId": "101"}));
        quiet.push(route("GET", "/users/me/messages", json!({})));
        mock.set_routes(quiet);
        let third = GmailSource.fetch(&conn, second.cursor).await.unwrap();
        let ids: Vec<_> = first.records.iter().chain(&second.records).chain(&third.records).map(|r| s(r, "/id")).collect();
        assert_eq!(ids.iter().filter(|id| **id == "imported").count(), 1, "{ids:?}");
        assert_eq!(ids.len(), 3, "each arrival is fetched once");
        assert_eq!(MessageCursor::read(third.cursor).unwrap().after.as_deref(), Some("1700000299"), "backdated arrivals cannot rewind the checkpoint");
    }

    #[tokio::test]
    async fn review_cursor_stays_bounded_through_boundary_remove_readd_cycles() {
        let mock = serve(vec![]).await;
        let conn = mock.conn(json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"}));
        let mut cursor = Some("1700000000".to_string());
        for i in 0..80 {
            let id = format!("message-{i:080}");
            let mut routes = token_and_history(json!({"historyId": format!("{}", 101 + i), "history": [{"messagesAdded": [{"message": {"id": id}}]}]}));
            routes.extend([
                route("GET", "/users/me/messages", json!({"messages": [{"id": id}]})),
                route("GET", &format!("/users/me/messages/{id}"), message(&id, "1700000100000", "Tie")),
            ]);
            mock.set_routes(with_history(routes));
            let result = GmailSource.fetch(&conn, cursor).await.unwrap();
            assert_eq!(result.records.len(), 1);
            cursor = result.cursor;
            assert!(cursor.as_ref().unwrap().len() < 2048, "removed boundary IDs accumulated at cycle {i}");
        }
    }

    #[tokio::test]
    async fn review_quota_pause_is_explicit_even_with_zero_progress() {
        let mut routes = token_and_history(json!({"historyId": "100"}));
        routes.extend([
            route("GET", "/users/me/messages", json!({"messages": [{"id": "limited"}]})),
            route("GET", "/users/me/messages/limited", json!({})).status(429).header("retry-after", "120"),
        ]);
        let mock = serve(routes).await;
        let res = GmailSource.fetch(&mock.conn(json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"})), None).await.unwrap();
        assert!(format!("{res:?}").contains("RateLimited"), "quota interruption must be explicit: {res:?}");
    }

    #[tokio::test]
    async fn persistent_message_http_errors_skip_after_three_syncs_with_durable_count() {
        for status in [400, 500] {
            let mock = serve(vec![]).await;
            let conn = mock.conn(json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"}));
            let mut cursor = Some(json!({"history": "100", "pending": ["broken"], "failures": {"broken": 2}}).to_string());
            for i in 0..3 {
                let id = format!("good-{i}");
                let mut routes = token_and_history(json!({"historyId": "100", "history": [{"messagesAdded": [{"message": {"id": id}}]}]}));
                routes.extend([
                    route("GET", "/users/me/messages/broken", json!({})).status(status).header("retry-after", "60"),
                    route("GET", &format!("/users/me/messages/{id}"), message(&id, "1700000200000", "Good")),
                ]);
                mock.set_routes(routes);
                let result = GmailSource.fetch(&conn, cursor).await.unwrap();
                assert_eq!(result.records[0]["id"], id);
                cursor = result.cursor;
                let state = MessageCursor::read(cursor.clone()).unwrap();
                if i < 2 {
                    assert_eq!(state.pending, ["broken"]);
                    assert_eq!(state.message_failures["broken"], i + 1);
                    assert!(result.warnings.is_empty());
                } else {
                    assert!(state.pending.is_empty());
                    assert!(state.message_failures.is_empty());
                    assert_eq!(result.warnings, ["1 messages skipped after repeated errors"]);
                }
            }
            mock.set_routes(token_and_history(json!({"historyId": "100"})));
            let next = GmailSource.fetch(&conn, cursor).await.unwrap();
            assert_eq!(next.warnings, ["1 messages skipped after repeated errors"]);
        }
    }

    #[tokio::test]
    async fn repeated_bad_request_is_message_specific_without_other_successes() {
        let mut routes = token_and_history(json!({"historyId": "100"}));
        routes.push(route("GET", "/users/me/messages/bad", json!({})).status(400));
        let mock = serve(routes).await;
        let conn = mock.conn(json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"}));
        let mut cursor = Some(json!({"history": "100", "pending": ["bad"]}).to_string());
        for i in 0..3 {
            let result = GmailSource.fetch(&conn, cursor).await.unwrap();
            cursor = result.cursor;
            let state = MessageCursor::read(cursor.clone()).unwrap();
            assert_eq!(state.pending.is_empty(), i == 2);
            assert_eq!(state.skipped, u64::from(i == 2));
        }
    }

    #[tokio::test]
    async fn truncated_quota_and_body_failures_never_spend_message_allowance() {
        for status in [429, 200] {
            let mut routes = token_and_history(json!({"historyId": "100"}));
            routes.push(route("GET", "/users/me/messages/unread", json!({})).status(status).header("retry-after", "60").truncated());
            let mock = serve(routes).await;
            let conn = mock.conn(json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"}));
            let mut cursor = Some(json!({"history": "100", "pending": ["later", "unread"], "failures": {"unread": 2}}).to_string());
            for _ in 0..10 {
                let result = GmailSource.fetch(&conn, cursor).await.unwrap();
                if status == 429 {
                    assert!(matches!(result.progress, SyncProgress::Partial { reason: crate::sources::base::SyncPause::RateLimited, .. }));
                }
                assert!(result.records.is_empty());
                assert!(result.warnings.is_empty());
                cursor = result.cursor;
                let state = MessageCursor::read(cursor.clone()).unwrap();
                assert_eq!(state.pending, ["later", "unread"]);
                assert!(state.message_failures.is_empty());
            }
        }
    }

    #[tokio::test]
    async fn truncated_auth_failures_always_fail_visibly() {
        for status in [401, 403] {
            let mut routes = token_and_history(json!({"historyId": "100"}));
            routes.push(route("GET", "/users/me/messages/unread", json!({})).status(status).truncated());
            let mock = serve(routes).await;
            let conn = mock.conn(json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"}));
            let cursor = Some(json!({"history": "100", "pending": ["unread"]}).to_string());
            for _ in 0..4 {
                let error = GmailSource.fetch(&conn, cursor.clone()).await.unwrap_err();
                assert_eq!(error.reason, Some("connector_auth"));
            }
        }
    }

    #[tokio::test]
    async fn systemic_server_errors_never_drop_unread_messages() {
        let mut routes = token_and_history(json!({"historyId": "100"}));
        for id in ["a", "b"] {
            routes.push(route("GET", &format!("/users/me/messages/{id}"), json!({})).status(500).header("retry-after", "60"));
        }
        let mock = serve(routes).await;
        let conn = mock.conn(json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"}));
        let mut cursor = Some(json!({"history": "100", "pending": ["a", "b"]}).to_string());
        for _ in 0..10 {
            let result = GmailSource.fetch(&conn, cursor).await.unwrap();
            assert!(result.warnings.is_empty());
            cursor = result.cursor;
            let state = MessageCursor::read(cursor.clone()).unwrap();
            assert_eq!(state.pending, ["a", "b"]);
            assert!(state.message_failures.is_empty());
        }
    }

    #[tokio::test]
    async fn network_message_failures_keep_the_pending_cursor_across_many_syncs() {
        let mock = serve(vec![route("POST", "/oauth/token", json!({"access_token": "t"}))]).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut conn = mock.conn(json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"}));
        conn.config["base_url"] = json!(format!("http://{}", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut request = vec![0; 8192];
                    let mut read = 0;
                    while !request[..read].windows(4).any(|w| w == b"\r\n\r\n") {
                        let n = socket.read(&mut request[read..]).await.unwrap();
                        if n == 0 { return; }
                        read += n;
                    }
                    if request[..read].starts_with(b"GET /users/me/history?") {
                        let body = r#"{"historyId":"100"}"#;
                        let reply = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                        socket.write_all(reply.as_bytes()).await.unwrap();
                    }
                    // Message reads close without any HTTP status or headers.
                });
            }
        });
        let mut cursor = Some(json!({"history": "100", "pending": ["unread"]}).to_string());
        for _ in 0..10 {
            let result = GmailSource.fetch(&conn, cursor).await.unwrap();
            assert!(matches!(result.progress, SyncProgress::Partial { reason: crate::sources::base::SyncPause::Transient, .. }));
            assert!(result.warnings.is_empty());
            cursor = result.cursor;
            let state = MessageCursor::read(cursor.clone()).unwrap();
            assert_eq!(state.pending, ["unread"]);
            assert!(state.message_failures.is_empty());
        }
        server.abort();
    }

    #[tokio::test]
    async fn expired_history_relists_without_a_date_filter_and_catches_older_imports() {
        let mut routes = token_and_history(json!({"historyId": "201"}));
        routes.insert(0, route("GET", "/users/me/history", json!({})).query_has("startHistoryId=expired").status(404));
        routes.extend([
            route("GET", "/users/me/messages", json!({"messages": [{"id": "ancient"}]})),
            route("GET", "/users/me/messages/ancient", message("ancient", "1600000000000", "Imported")),
        ]);
        let mock = serve(routes).await;
        let old = json!({"after": "1700000000", "history": "expired"}).to_string();
        let result = GmailSource.fetch(&mock.conn(json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"})), Some(old)).await.unwrap();
        assert_eq!(result.records[0]["id"], "ancient");
        let reqs = mock.requests();
        let list = reqs.iter().find(|r| r.path == "/users/me/messages").unwrap();
        assert!(!list.query.contains("q="), "expired history must include backdated imports: {}", list.query);
        assert_eq!(MessageCursor::read(result.cursor).unwrap().history.as_deref(), Some("201"));
    }

    #[test]
    fn cursor_hard_cap_refuses_to_truncate_unread_work() {
        let cursor = MessageCursor { pending: vec!["x".repeat(MAX_CURSOR_BYTES)], ..MessageCursor::default() };
        assert!(cursor.encode().unwrap_err().message.contains("keeping the previous checkpoint"));
        let cursor = MessageCursor { pending: vec!["id".into(); MAX_PENDING + 1], ..MessageCursor::default() };
        assert!(cursor.encode().is_err());
    }

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s)
    }

    fn message(id: &str, internal: &str, subject: &str) -> Value {
        json!({
            "id": id, "threadId": format!("t-{id}"), "labelIds": ["INBOX", "UNREAD"], "snippet": "snippet only",
            "internalDate": internal,
            "payload": {
                "mimeType": "multipart/alternative",
                "headers": [{"name": "Subject", "value": subject}, {"name": "From", "value": "Ada <ada@example.com>"},
                            {"name": "To", "value": "me@example.com"}],
                "parts": [
                    {"mimeType": "text/plain", "body": {"size": 30, "data": b64("Lunch on Thursday works for me.")}},
                    {"mimeType": "text/html", "body": {"size": 40, "data": b64("<p>Lunch on Thursday works for me.</p>")}},
                ],
            },
        })
    }

    #[tokio::test]
    async fn fetch_refreshes_the_token_then_lists_and_reads_messages() {
        let mock = serve(vec![
            route("POST", "/oauth/token", json!({"access_token": "ya29.fresh", "expires_in": 3599, "token_type": "Bearer"})),
            route("GET", "/users/me/messages", json!({"messages": [{"id": "m1", "threadId": "t-m1"}], "resultSizeEstimate": 1}))
                .query_has("pageToken=p2"),
            route("GET", "/users/me/messages", json!({"messages": [{"id": "m2", "threadId": "t-m2"}], "nextPageToken": "p2"}))
                .query_has("q=after:1700000000"),
            route("GET", "/users/me/messages/m1", json!(message("m1", "1700000100000", "Lunch?"))).query_has("format=full"),
            route("GET", "/users/me/messages/m2", json!(message("m2", "1700000200000", "Re: Lunch?"))),
        ])
        .await;

        let creds = json!({"client_id": "cid", "client_secret": "csecret", "refresh_token": "1//rt"});
        let res = GmailSource.fetch(&mock.conn(creds), Some("1700000000".into())).await.unwrap();
        assert_eq!(res.records.len(), 2);
        assert_eq!(MessageCursor::read(res.cursor).unwrap().after.as_deref(), Some("1700000199"));

        let reqs = mock.requests();
        let token = &reqs[0];
        assert_eq!(token.path, "/oauth/token");
        assert!(token.body.contains("grant_type=refresh_token") && token.body.contains("refresh_token=1%2F%2Frt"));
        assert!(token.header("authorization").starts_with("Basic "));
        assert!(reqs[1..].iter().all(|r| r.header("authorization") == "Bearer ya29.fresh"));

        let envs = envelopes(&GmailSource, &res.records);
        assert_eq!(envs[0].title, "Lunch?");
        assert_eq!(envs[0].body_text, "From: Ada <ada@example.com>\nLunch on Thursday works for me.");
        assert!(envs[0].occurred_at.is_some());
    }

    #[test]
    fn rfc2822_date_header_becomes_utc_occurred_at() {
        let mut m = message("m", "1700000000000", "s");
        m["payload"]["headers"] = json!([{"name": "Date", "value": "Tue, 5 Mar 2024 09:30:00 +1100 (AEDT)"}]);
        assert_eq!(sent_at(&m).unwrap(), "2024-03-04T22:30:00+00:00");
        m["payload"]["headers"] = json!([{"name": "Date", "value": "garbage"}]);
        assert_eq!(sent_at(&m).unwrap(), "2023-11-14T22:13:20+00:00", "falls back to internalDate");
    }

    /// 250 waiting messages, 200 per sync: the first run reads the oldest
    /// 200, the second resumes after them and reads the rest.
    #[tokio::test]
    async fn a_backlog_beyond_one_sync_drains_oldest_first_without_starvation() {
        let at = |i: usize| (1_700_000_000 + (250 - i) as i64 * 10) * 1000; // m0 newest
        let refs = |r: std::ops::Range<usize>| json!(r.map(|i| json!({"id": format!("m{i}")})).collect::<Vec<_>>());
        let mut routes = vec![
            route("POST", "/oauth/token", json!({"access_token": "t"})),
            // second run: after the newest message read by the first (m50, minus a second)
            route("GET", "/users/me/messages", json!({"messages": refs(0..51)})).query_has("q=after:1700001999"),
            route("GET", "/users/me/messages", json!({"messages": refs(200..250)})).query_has("pageToken=p3"),
            route("GET", "/users/me/messages", json!({"messages": refs(100..200), "nextPageToken": "p3"})).query_has("pageToken=p2"),
            route("GET", "/users/me/messages", json!({"messages": refs(0..100), "nextPageToken": "p2"})),
        ];
        for i in 0..250 {
            routes.push(route("GET", &format!("/users/me/messages/m{i}"), message(&format!("m{i}"), &at(i).to_string(), &format!("Message {i}"))));
        }
        let mock = serve(routes).await;
        let conn = mock.conn(json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"}));

        let first = GmailSource.fetch(&conn, None).await.unwrap();
        assert_eq!(first.records.len(), 200);
        assert!(first.records.iter().all(|r| s(r, "/id")[1..].parse::<usize>().unwrap() >= 50), "oldest 200 first");
        assert_eq!(MessageCursor::read(first.cursor.clone()).unwrap().after.as_deref(), Some("1700001999"));

        let second = GmailSource.fetch(&conn, first.cursor).await.unwrap();
        let seen: HashSet<String> = first.records.iter().chain(&second.records).map(|r| s(r, "/id").to_string()).collect();
        assert_eq!(seen.len(), 250, "every message imported");
    }

    #[tokio::test]
    async fn quota_failure_keeps_a_contiguous_prefix_and_the_next_sync_drains() {
        let routes = || vec![
            route("POST", "/oauth/token", json!({"access_token": "t"})),
            route("GET", "/users/me/messages", json!({"messages": [{"id": "m3"}, {"id": "m2"}, {"id": "m1"}]})),
            route("GET", "/users/me/messages/m1", message("m1", "1700000100000", "First")),
            route("GET", "/users/me/messages/m2", json!({"error": {"errors": [{"reason": "userRateLimitExceeded"}]}}))
                .status(403).header("retry-after", "60"),
            route("GET", "/users/me/messages/m3", message("m3", "1700000300000", "Third")),
        ];
        let mock = serve(routes()).await;
        let conn = mock.conn(json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"}));
        // The old implementation fails both attempts, discarding the successful gets each time.
        let attempts = [GmailSource.fetch(&conn, Some("1700000000".into())).await,
                        GmailSource.fetch(&conn, Some("1700000000".into())).await];
        assert!(attempts.iter().all(Result::is_ok), "both attempts must preserve progress: {attempts:?}");
        let first = attempts.into_iter().next().unwrap().unwrap();
        assert_eq!(first.records.iter().map(|r| s(r, "/id")).collect::<Vec<_>>(), ["m1"]);
        assert_eq!(MessageCursor::read(first.cursor.clone()).unwrap().after.as_deref(), Some("1700000099"));
        assert!(!mock.requests().iter().any(|r| r.path.ends_with("/m3")), "stop at the first gap");

        let mut recovered = routes();
        recovered[1] = route("GET", "/users/me/messages", json!({"messages": [{"id": "m3"}, {"id": "m2"}, {"id": "m1"}]}))
            .query_has("q=after:1700000099");
        recovered[3] = route("GET", "/users/me/messages/m2", message("m2", "1700000200000", "Second"));
        mock.set_routes(with_history(recovered));
        let second = GmailSource.fetch(&conn, first.cursor).await.unwrap();
        let seen: HashSet<_> = first.records.iter().chain(&second.records).map(|r| s(r, "/id")).collect();
        assert_eq!(seen, HashSet::from(["m1", "m2", "m3"]));
        assert_eq!(first.records.len() + second.records.len(), 3, "no duplicate reads");
        assert_eq!(MessageCursor::read(second.cursor).unwrap().after.as_deref(), Some("1700000299"));
    }

    #[tokio::test]
    async fn an_exhausted_retry_budget_returns_partial_progress() {
        for status in [429, 503] {
            let mock = serve(vec![
                route("POST", "/oauth/token", json!({"access_token": "t"})),
                route("GET", "/users/me/messages", json!({"messages": [{"id": "m3"}, {"id": "m2"}, {"id": "m1"}]})),
                route("GET", "/users/me/messages/m1", message("m1", "1700000100000", "First")),
                route("GET", "/users/me/messages/m2", json!({})).status(status),
                route("GET", "/users/me/messages/m3", message("m3", "1700000300000", "Third")),
            ]).await;
            let started = tokio::time::Instant::now();
            let result = GmailSource.fetch(&mock.conn(json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"})), None).await.unwrap();
            assert_eq!(result.records.len(), if status == 429 { 1 } else { 2 });
            let cursor = MessageCursor::read(result.cursor).unwrap();
            assert_eq!(cursor.after.as_deref(), Some(if status == 429 { "1700000099" } else { "1700000299" }));
            assert_eq!(cursor.pending, if status == 429 { vec!["m3", "m2"] } else { vec!["m2"] });
            let reads = mock.requests().iter().filter(|r| r.path.ends_with("/m2")).count();
            assert!((2..=5).contains(&reads), "retry in place, then stop: {reads}");
            assert!(started.elapsed() < Duration::from_secs(15), "bounded retry time");
        }
    }

    #[tokio::test]
    async fn a_large_same_second_group_drains_without_duplicates_or_relisting() {
        let mut routes = vec![
            route("POST", "/oauth/token", json!({"access_token": "t"})),
            route("GET", "/users/me/messages", json!({"messages": (0..250).rev().map(|i| json!({"id": format!("m{i}")})).collect::<Vec<_>>()})),
        ];
        for i in 0..250 {
            routes.push(route("GET", &format!("/users/me/messages/m{i}"), message(&format!("m{i}"), "1700000100000", "Tie")));
        }
        let mock = serve(routes.clone()).await;
        let conn = mock.conn(json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"}));
        let first = GmailSource.fetch(&conn, None).await.unwrap();
        let second = GmailSource.fetch(&conn, first.cursor).await.unwrap();
        assert_eq!((first.records.len(), second.records.len()), (200, 50));
        let ids: HashSet<_> = first.records.iter().chain(&second.records).map(|r| s(r, "/id")).collect();
        assert_eq!(ids.len(), 250);
        assert_eq!(mock.requests().iter().filter(|r| r.path == "/users/me/messages").count(), 1);
        assert!(mock.requests().iter().any(|r| r.query.contains("maxResults=500")));
        routes[1] = route("GET", "/users/me/messages", json!({"messages": [{"id": "new"}, {"id": "m249"}]}));
        routes.push(route("GET", "/users/me/history", json!({"historyId": "101", "history": [{"messagesAdded": [{"message": {"id": "new"}}]}]})));
        routes.push(route("GET", "/users/me/messages/new", message("new", "1700000200000", "New")));
        mock.set_routes(with_history(routes));
        let third = GmailSource.fetch(&conn, second.cursor).await.unwrap();
        assert_eq!(third.records.iter().map(|r| s(r, "/id")).collect::<Vec<_>>(), ["new"]);
    }

    #[tokio::test]
    async fn an_incomplete_listing_never_skips_older_messages() {
        let mock = serve(vec![
            route("POST", "/oauth/token", json!({"access_token": "t"})),
            route("GET", "/users/me/messages", json!({"messages": [{"id": "newest"}], "nextPageToken": "more"})),
        ]).await;
        let error = GmailSource.fetch(&mock.conn(json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"})), None).await.unwrap_err();
        assert!(error.message.contains("older mail is not skipped"));
        assert_eq!(mock.requests().len(), MAX_PAGES + 2);
        assert!(!mock.requests().iter().any(|r| r.path.ends_with("/newest")));
    }

    #[tokio::test]
    async fn a_failed_message_read_fails_the_sync_for_retry() {
        let mock = serve(vec![
            route("POST", "/oauth/token", json!({"access_token": "t"})),
            route("GET", "/users/me/messages", json!({"messages": [{"id": "m1"}]})),
            route("GET", "/users/me/messages/m1", json!({"error": {"code": 403, "errors": [{"reason": "insufficientPermissions"}]}})).status(403),
        ])
        .await;
        let err = GmailSource.fetch(&mock.conn(json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"})), Some("1".into())).await.unwrap_err();
        assert!(err.message.contains("HTTP 403"), "{}", err.message);
    }

    #[tokio::test]
    async fn a_rejected_refresh_token_is_a_visible_error() {
        let mock = serve(vec![route("POST", "/oauth/token", json!({"error": "invalid_grant", "error_description": "Token has been expired or revoked."})).status(400)]).await;
        let creds = json!({"client_id": "cid", "client_secret": "s", "refresh_token": "old"});
        let err = GmailSource.fetch(&mock.conn(creds), None).await.unwrap_err();
        assert!(err.message.contains("HTTP 400") && err.message.contains("invalid_grant"), "{}", err.message);
    }

    #[tokio::test]
    async fn fetch_errors_are_visible() {
        let creds = json!({"client_id": "cid", "client_secret": "s", "refresh_token": "rt"});
        assert_fetch_fails(&GmailSource, 404, "/users/me/profile", creds.clone(), "HTTP 404").await;
        assert_fetch_fails(&GmailSource, 401, "/users/me/profile", creds.clone(), "HTTP 401").await;
        assert_fetch_fails(&GmailSource, 200, "/x", json!({"client_id": "cid"}), "missing client_secret").await;
    }
}
