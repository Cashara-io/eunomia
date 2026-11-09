//! Slack source: messages in every channel the token's bot (or user) is a
//! member of, via `conversations.list` + `conversations.history`. Bot token
//! (`xoxb-`), bearer auth.
//!
//! Slack answers most failures with HTTP 200 and `{"ok": false, "error":
//! ...}`, so every call goes through [`call`], which turns that into an error.
//! Incremental: `oldest` = the start of the previous sync (minus a minute of
//! overlap); the first sync reads the last 30 days.

use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};

use crate::connectors::clients::{bearer, require, Api};
use crate::error::{AppError, AppResult};
use crate::sources::base::{envelope, from_unix, items, s, Conn, Source, SyncResult, MAX_PAGES};

const BASE_URL: &str = "https://slack.com/api";

fn api(conn: &Conn) -> AppResult<Api> {
    Ok(Api::new(&conn.config, BASE_URL, bearer(&require(&conn.credentials, "bot_token")?)))
}

async fn call(api: &Api, method: &str, query: &[(&str, String)]) -> AppResult<Value> {
    let v = api.get(&format!("/{method}"), query).await?;
    if v.get("ok").and_then(|ok| ok.as_bool()) == Some(true) {
        Ok(v)
    } else {
        Err(AppError::bad_request(format!("slack {method} failed: {}", s(&v, "/error"))))
    }
}

/// Every page of a cursor-paginated Slack method, concatenating `key`.
async fn paged(api: &Api, method: &str, key: &str, query: Vec<(&str, String)>) -> AppResult<Vec<Value>> {
    let mut out = Vec::new();
    let mut cursor = String::new();
    for _ in 0..MAX_PAGES {
        let mut q = query.clone();
        if !cursor.is_empty() {
            q.push(("cursor", cursor.clone()));
        }
        let page = call(api, method, &q).await?;
        out.extend(items(&page, &format!("/{key}")));
        cursor = s(&page, "/response_metadata/next_cursor").to_string();
        if cursor.is_empty() {
            break;
        }
    }
    Ok(out)
}

pub struct SlackSource;

#[async_trait]
impl Source for SlackSource {
    fn key(&self) -> &'static str {
        "slack"
    }

    fn label(&self) -> &'static str {
        "Slack"
    }

    fn record_types(&self) -> &'static [&'static str] {
        &["slack.message"]
    }

    async fn check(&self, conn: &Conn) -> AppResult<()> {
        call(&api(conn)?, "auth.test", &[]).await.map(|_| ())
    }

    async fn fetch(&self, conn: &Conn, cursor: Option<String>) -> AppResult<SyncResult> {
        let api = api(conn)?;
        let started = Utc::now().timestamp();
        let oldest = cursor.and_then(|c| c.parse::<i64>().ok()).map(|t| t - 60).unwrap_or(started - 30 * 86400);
        let workspace_url = s(&call(&api, "auth.test", &[]).await?, "/url").to_string();

        let channels = paged(
            &api,
            "conversations.list",
            "channels",
            vec![("types", "public_channel,private_channel".into()), ("exclude_archived", "true".into()), ("limit", "200".into())],
        )
        .await?;

        let mut records = Vec::new();
        for ch in channels.iter().filter(|c| c.get("is_member").and_then(|v| v.as_bool()).unwrap_or(false)) {
            let id = s(ch, "/id");
            let messages = paged(
                &api,
                "conversations.history",
                "messages",
                vec![("channel", id.to_string()), ("oldest", oldest.to_string()), ("limit", "200".into())],
            )
            .await?;
            for mut m in messages {
                m["_channel_id"] = json!(id);
                m["_channel_name"] = json!(s(ch, "/name"));
                m["_workspace_url"] = json!(workspace_url);
                records.push(m);
            }
        }
        Ok(SyncResult { records, cursor: Some(started.to_string()), ..SyncResult::default() })
    }

    fn map(&self, raw: &Value) -> Option<Value> {
        let ts = raw.get("ts").and_then(|v| v.as_str())?;
        // Joins, topic changes etc. carry a subtype and no real content.
        if raw.get("subtype").is_some_and(|st| st != "thread_broadcast" && st != "file_share") {
            return None;
        }
        let channel_id = s(raw, "/_channel_id");
        let channel = s(raw, "/_channel_name");
        let text = s(raw, "/text");
        let first_line: String = text.lines().next().unwrap_or("").chars().take(80).collect();
        let url = format!("{}archives/{channel_id}/p{}", s(raw, "/_workspace_url"), ts.replace('.', ""));
        let secs = ts.split('.').next().and_then(|t| t.parse::<i64>().ok()).unwrap_or(0);
        Some(envelope(
            "slack",
            "slack.message",
            &format!("{channel_id}:{ts}"),
            &format!("#{channel}: {first_line}"),
            text,
            from_unix(secs),
            &url,
            json!({
                "channel": channel,
                "channel_id": channel_id,
                "user": raw.get("user"),
                "thread_ts": raw.get("thread_ts"),
                "reply_count": raw.get("reply_count"),
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::mock::{assert_fetch_fails, envelopes, route, serve};

    #[tokio::test]
    async fn fetch_reads_history_of_joined_channels_across_pages() {
        let mock = serve(vec![
            route("GET", "/auth.test", json!({"ok": true, "url": "https://acme.slack.com/", "team": "Acme", "user_id": "U0BOT"})),
            route("GET", "/conversations.list", json!({"ok": true, "channels": [
                {"id": "C2", "name": "random", "is_member": false},
            ], "response_metadata": {"next_cursor": ""}}))
            .query_has("cursor=dGVhbTpDMQ=="),
            route("GET", "/conversations.list", json!({"ok": true, "channels": [
                {"id": "C1", "name": "general", "is_member": true},
            ], "response_metadata": {"next_cursor": "dGVhbTpDMQ=="}})),
            route("GET", "/conversations.history", json!({"ok": true, "messages": [
                {"type": "message", "user": "U1", "text": "Deploy is green, shipping v2 today", "ts": "1714000000.000200"},
                {"type": "message", "subtype": "channel_join", "user": "U2", "text": "<@U2> has joined the channel", "ts": "1714000001.000100"},
            ], "has_more": false, "response_metadata": {"next_cursor": ""}}))
            .query_has("channel=C1")
            .query_has("oldest=1713999940"),
        ])
        .await;

        let res = SlackSource.fetch(&mock.conn(json!({"bot_token": "xoxb-test"})), Some("1714000000".into())).await.unwrap();
        assert_eq!(res.records.len(), 2, "only the joined channel's history is read");
        assert!(mock.requests().iter().all(|r| r.header("authorization") == "Bearer xoxb-test"));
        assert!(!mock.requests().iter().any(|r| r.query.contains("C2")));

        let envs = envelopes(&SlackSource, &res.records);
        assert_eq!(envs.len(), 1, "the join notice is skipped");
        assert_eq!(envs[0].title, "#general: Deploy is green, shipping v2 today");
        assert_eq!(envs[0].url, "https://acme.slack.com/archives/C1/p1714000000000200");
        assert!(envs[0].occurred_at.is_some());
    }

    #[tokio::test]
    async fn ok_false_is_an_error_not_an_empty_sync() {
        let mock = serve(vec![route("GET", "/auth.test", json!({"ok": false, "error": "invalid_auth"}))]).await;
        let err = SlackSource.fetch(&mock.conn(json!({"bot_token": "xoxb-bad"})), None).await.unwrap_err();
        assert!(err.message.contains("invalid_auth"), "{}", err.message);
        assert!(SlackSource.check(&mock.conn(json!({"bot_token": "xoxb-bad"}))).await.is_err());
    }

    #[tokio::test]
    async fn fetch_errors_are_visible() {
        let creds = json!({"bot_token": "xoxb"});
        assert_fetch_fails(&SlackSource, 429, "/auth.test", creds.clone(), "HTTP 429").await;
        assert_fetch_fails(&SlackSource, 500, "/auth.test", creds, "HTTP 500").await;
    }
}
