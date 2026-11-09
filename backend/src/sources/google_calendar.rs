//! Google Calendar source: events on the primary calendar, via
//! `events.list` (page-token paginated, recurring events expanded). Same
//! bring-your-own-OAuth-client credentials as Gmail (scope
//! `calendar.readonly`), refreshed each sync.
//!
//! First sync: 30 days back to 180 days ahead. After that: every event
//! modified since the previous sync started (`updatedMin`), including
//! cancellations, which mark the cached event deleted.

use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::connectors::clients::retryable;

use crate::error::AppResult;
use crate::sources::base::{envelope, items, rfc3339, s, Conn, Source, SyncProgress, SyncResult, MAX_PAGES};
use crate::sources::gmail::google_api;

const BASE_URL: &str = "https://www.googleapis.com/calendar/v3";

/// `start`/`end` carry `dateTime` for timed events, `date` for all-day ones.
fn when(v: &Value) -> Option<String> {
    let dt = s(v, "/dateTime");
    if !dt.is_empty() {
        return rfc3339(dt);
    }
    let d = s(v, "/date");
    (!d.is_empty()).then(|| rfc3339(&format!("{d}T00:00:00Z"))).flatten()
}

/// Page tokens must be replayed with the exact same query, including the first-sync window.
#[derive(Serialize, Deserialize)]
struct PageCursor {
    query: Vec<(String, String)>,
    page_token: String,
    started: String,
}

pub struct GoogleCalendarSource;

#[async_trait]
impl Source for GoogleCalendarSource {
    fn key(&self) -> &'static str {
        "google_calendar"
    }

    fn label(&self) -> &'static str {
        "Google Calendar"
    }

    fn record_types(&self) -> &'static [&'static str] {
        &["gcal.event"]
    }

    async fn check(&self, conn: &Conn) -> AppResult<()> {
        google_api(conn, BASE_URL).await?.get("/calendars/primary", &[]).await.map(|_| ())
    }

    async fn fetch(&self, conn: &Conn, cursor: Option<String>) -> AppResult<SyncResult> {
        let api = google_api(conn, BASE_URL).await?;
        let started = Utc::now();
        let mut base = vec![("singleEvents", "true".to_string()), ("maxResults", "250".to_string())];
        match &cursor {
            Some(c) => {
                base.push(("updatedMin", c.clone()));
                base.push(("showDeleted", "true".to_string()));
            }
            None => {
                base.push(("timeMin", (started - Duration::days(30)).to_rfc3339()));
                base.push(("timeMax", (started + Duration::days(180)).to_rfc3339()));
            }
        }

        let mut resume = if let Some(c) = cursor.as_deref().filter(|c| c.starts_with('{')) {
            serde_json::from_str::<PageCursor>(c).map_err(|_| crate::error::AppError::internal("invalid Calendar cursor"))?
        } else {
            PageCursor { query: base.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
                page_token: String::new(), started: started.to_rfc3339() }
        };
        let mut records = Vec::new();
        for _ in 0..MAX_PAGES {
            let mut query: Vec<(&str, String)> = resume.query.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
            if !resume.page_token.is_empty() {
                query.push(("pageToken", resume.page_token.clone()));
            }
            let page = match api.get("/calendars/primary/events", &query).await {
                Ok(page) => page,
                Err(error) if retryable(&error) => {
                    tracing::warn!(fetched = records.len(), rate_limited = error.reason == Some("connector_rate_limited"),
                        "Google Calendar paused fetching; keeping fetched events for import, will continue on the next sync");
                    return Ok(SyncResult { records, cursor: Some(serde_json::to_string(&resume).unwrap()), progress: SyncProgress::interrupted(&error), ..SyncResult::default() });
                }
                Err(error) => return Err(error),
            };
            records.extend(items(&page, "/items"));
            resume.page_token = s(&page, "/nextPageToken").to_string();
            if resume.page_token.is_empty() {
                return Ok(SyncResult { records, cursor: Some(resume.started), ..SyncResult::default() });
            }
        }
        Ok(SyncResult { records, cursor: Some(serde_json::to_string(&resume).unwrap()), progress: SyncProgress::remaining(), ..SyncResult::default() })
    }

    fn map(&self, raw: &Value) -> Option<Value> {
        let id = raw.get("id").and_then(|v| v.as_str())?;
        let title = s(raw, "/summary");
        let attendees: Vec<String> = items(raw, "/attendees").iter().map(|a| s(a, "/email").to_string()).collect();
        let body = [title, s(raw, "/description"), s(raw, "/location")]
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let mut env = envelope(
            "google_calendar",
            "gcal.event",
            id,
            if title.is_empty() { "(no title)" } else { title },
            &body,
            when(&raw["start"]),
            s(raw, "/htmlLink"),
            json!({
                "start": when(&raw["start"]),
                "end": when(&raw["end"]),
                "all_day": raw.pointer("/start/date").is_some(),
                "location": raw.get("location"),
                "organizer": raw.pointer("/organizer/email"),
                "attendees": attendees,
                "status": raw.get("status"),
            }),
        );
        env["deleted"] = json!(s(raw, "/status") == "cancelled");
        Some(env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::mock::{assert_fetch_fails, envelopes, route, serve};

    #[tokio::test]
    async fn review_calendar_quota_pause_is_explicit() {
        let mock = serve(vec![
            route("POST", "/oauth/token", json!({"access_token": "t"})),
            route("GET", "/calendars/primary/events", json!({})).query_has("pageToken=p2").status(429).header("retry-after", "120"),
            route("GET", "/calendars/primary/events", json!({"items": [{"id": "e1"}], "nextPageToken": "p2"})),
        ]).await;
        let res = GoogleCalendarSource.fetch(&mock.conn(json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"})), None).await.unwrap();
        assert_eq!(res.records.len(), 1);
        assert!(format!("{res:?}").contains("RateLimited"), "quota interruption must be explicit: {res:?}");
    }

    #[tokio::test]
    async fn quota_failure_saves_pages_and_resumes_the_exact_query() {
        let mock = serve(vec![
            route("POST", "/oauth/token", json!({"access_token": "t"})),
            route("GET", "/calendars/primary/events", json!({"error": {"errors": [{"reason": "userRateLimitExceeded"}]}}))
                .query_has("pageToken=p2").status(403).header("retry-after", "60"),
            route("GET", "/calendars/primary/events", json!({"items": [{"id": "e1"}], "nextPageToken": "p2"})),
        ]).await;
        let conn = mock.conn(json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"}));
        let first = GoogleCalendarSource.fetch(&conn, None).await.unwrap();
        assert_eq!(first.records[0]["id"], "e1");
        let checkpoint: PageCursor = serde_json::from_str(first.cursor.as_deref().unwrap()).unwrap();
        assert_eq!(checkpoint.page_token, "p2");
        mock.set_routes(vec![
            route("POST", "/oauth/token", json!({"access_token": "t"})),
            route("GET", "/calendars/primary/events", json!({"items": [{"id": "e2"}]})).query_has("pageToken=p2"),
        ]);
        let second = GoogleCalendarSource.fetch(&conn, first.cursor).await.unwrap();
        assert_eq!(second.records[0]["id"], "e2");
        assert_eq!(second.cursor.as_deref(), Some(checkpoint.started.as_str()));
        let requests = mock.requests();
        let decode = |q: &str| serde_urlencoded::from_str::<Vec<(String, String)>>(q).unwrap();
        let initial = decode(&requests[1].query);
        let resumed = decode(&requests.last().unwrap().query);
        for pair in initial { assert!(resumed.contains(&pair), "reuse original time bounds"); }
    }

    #[tokio::test]
    async fn a_hard_error_on_a_later_page_still_fails() {
        let mock = serve(vec![
            route("POST", "/oauth/token", json!({"access_token": "t"})),
            route("GET", "/calendars/primary/events", json!({"error": {"errors": [{"reason": "insufficientPermissions"}]}}))
                .query_has("pageToken=p2").status(403),
            route("GET", "/calendars/primary/events", json!({"items": [{"id": "e1"}], "nextPageToken": "p2"})),
        ]).await;
        let error = GoogleCalendarSource.fetch(&mock.conn(json!({"client_id": "c", "client_secret": "s", "refresh_token": "r"})), None).await.unwrap_err();
        assert!(error.message.contains("HTTP 403"));
        assert_eq!(mock.requests().len(), 3);
    }

    #[tokio::test]
    async fn fetch_uses_updated_min_after_the_first_sync_and_follows_page_tokens() {
        let mock = serve(vec![
            route("POST", "/oauth/token", json!({"access_token": "ya29.cal"})),
            route("GET", "/calendars/primary/events", json!({"kind": "calendar#events", "items": [
                {"id": "e2", "status": "cancelled", "summary": "Old sync", "start": {"date": "2024-03-05"}, "end": {"date": "2024-03-06"}},
            ]}))
            .query_has("pageToken=n2"),
            route("GET", "/calendars/primary/events", json!({"kind": "calendar#events", "nextPageToken": "n2", "items": [
                {"id": "e1", "status": "confirmed", "summary": "Design review", "description": "Walk through the sync UI",
                 "location": "Room 4", "htmlLink": "https://www.google.com/calendar/event?eid=e1",
                 "start": {"dateTime": "2024-03-04T10:00:00+11:00"}, "end": {"dateTime": "2024-03-04T11:00:00+11:00"},
                 "organizer": {"email": "ada@example.com"}, "attendees": [{"email": "ada@example.com"}, {"email": "me@example.com"}]},
            ]}))
            .query_has("updatedMin=2024-03-01T00:00:00Z")
            .query_has("showDeleted=true"),
        ])
        .await;

        let creds = json!({"client_id": "cid", "client_secret": "s", "refresh_token": "rt"});
        let res = GoogleCalendarSource.fetch(&mock.conn(creds), Some("2024-03-01T00:00:00Z".into())).await.unwrap();
        assert_eq!(res.records.len(), 2);
        assert!(mock.requests()[1..].iter().all(|r| r.header("authorization") == "Bearer ya29.cal"));

        let envs = envelopes(&GoogleCalendarSource, &res.records);
        assert_eq!(envs[0].title, "Design review");
        assert_eq!(envs[0].body_text, "Design review\nWalk through the sync UI\nRoom 4");
        assert_eq!(envs[0].payload["attendees"], json!(["ada@example.com", "me@example.com"]));
        assert!(envs[1].deleted, "cancelled events are deletions");
        assert!(envs[1].occurred_at.is_some(), "all-day dates become midnight UTC");
    }

    #[tokio::test]
    async fn fetch_errors_are_visible() {
        let creds = json!({"client_id": "cid", "client_secret": "s", "refresh_token": "rt"});
        assert_fetch_fails(&GoogleCalendarSource, 403, "/calendars/primary/events", creds.clone(), "HTTP 403").await;
    }
}
