//! Discord source: messages in the channels listed in `config.channel_id`
//! (one id, or several comma-separated), via `GET /channels/{id}/messages`.
//! Bot token, `Authorization: Bot <token>`. The bot needs View Channel +
//! Read Message History in those channels, and the Message Content intent
//! enabled in the Developer Portal (without it Discord returns empty text).
//!
//! Incremental: the cursor is a JSON map of channel id -> newest message id
//! seen; each sync pages forward with `after=`.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::connectors::clients::{auth_header, require, str_field, Api};
use crate::error::{AppError, AppResult};
use crate::sources::base::{envelope, rfc3339, s, Conn, Source, SyncResult, MAX_PAGES};

const BASE_URL: &str = "https://discord.com/api/v10";

fn api(conn: &Conn) -> AppResult<Api> {
    Ok(Api::new(&conn.config, BASE_URL, auth_header(&format!("Bot {}", require(&conn.credentials, "bot_token")?))))
}

fn channel_ids(conn: &Conn) -> Vec<String> {
    str_field(&conn.config, "channel_id").split(',').map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect()
}

/// Snowflakes are u64s; compare numerically, not as text.
fn snowflake(v: &Value) -> u64 {
    s(v, "/id").parse().unwrap_or(0)
}

pub struct DiscordSource;

#[async_trait]
impl Source for DiscordSource {
    fn key(&self) -> &'static str {
        "discord"
    }

    fn label(&self) -> &'static str {
        "Discord"
    }

    fn record_types(&self) -> &'static [&'static str] {
        &["discord.message"]
    }

    async fn check(&self, conn: &Conn) -> AppResult<()> {
        let api = api(conn)?;
        api.get("/users/@me", &[]).await?;
        for id in channel_ids(conn) {
            api.get(&format!("/channels/{id}"), &[]).await?;
        }
        Ok(())
    }

    async fn fetch(&self, conn: &Conn, cursor: Option<String>) -> AppResult<SyncResult> {
        let api = api(conn)?;
        let channels = channel_ids(conn);
        if channels.is_empty() {
            return Err(AppError::bad_request("not configured: missing channel_id -- add it on the Connectors page"));
        }
        let mut newest: HashMap<String, u64> = cursor.and_then(|c| serde_json::from_str(&c).ok()).unwrap_or_default();
        let mut records = Vec::new();

        for id in channels {
            let channel = api.get(&format!("/channels/{id}"), &[]).await?;
            for _ in 0..MAX_PAGES {
                let mut query = vec![("limit", "100".to_string())];
                if let Some(after) = newest.get(&id) {
                    query.push(("after", after.to_string()));
                }
                let page = api.get(&format!("/channels/{id}/messages"), &query).await?.as_array().cloned().unwrap_or_default();
                let Some(max) = page.iter().map(snowflake).max() else { break };
                newest.insert(id.clone(), max.max(newest.get(&id).copied().unwrap_or(0)));
                let full = page.len() == 100;
                for mut m in page {
                    m["_channel_name"] = json!(s(&channel, "/name"));
                    m["_guild_id"] = json!(s(&channel, "/guild_id"));
                    records.push(m);
                }
                if !full {
                    break;
                }
            }
        }
        Ok(SyncResult { records, cursor: Some(json!(newest).to_string()), ..SyncResult::default() })
    }

    fn map(&self, raw: &Value) -> Option<Value> {
        let id = raw.get("id").and_then(|v| v.as_str())?;
        let content = s(raw, "/content");
        let attachments: Vec<&str> = raw
            .get("attachments")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().map(|x| s(x, "/filename")).collect())
            .unwrap_or_default();
        if content.is_empty() && attachments.is_empty() {
            return None;
        }
        let author = s(raw, "/author/global_name");
        let author = if author.is_empty() { s(raw, "/author/username") } else { author };
        let channel_id = s(raw, "/channel_id");
        let first_line: String = content.lines().next().unwrap_or("").chars().take(80).collect();
        Some(envelope(
            "discord",
            "discord.message",
            id,
            &format!("#{} {author}: {first_line}", s(raw, "/_channel_name")),
            &format!("{author}: {content}"),
            rfc3339(s(raw, "/timestamp")),
            &format!("https://discord.com/channels/{}/{channel_id}/{id}", s(raw, "/_guild_id")),
            json!({
                "channel": s(raw, "/_channel_name"),
                "channel_id": channel_id,
                "author": author,
                "attachments": attachments,
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::mock::{envelopes, route, serve};

    fn msg(id: &str, content: &str) -> Value {
        json!({"id": id, "channel_id": "C1", "content": content, "timestamp": "2024-04-01T12:00:00.000000+00:00",
               "author": {"id": "u1", "username": "ada", "global_name": "Ada"}, "attachments": []})
    }

    #[tokio::test]
    async fn fetch_pages_forward_from_the_saved_snowflake() {
        let page1: Vec<Value> = (0..100).map(|i| msg(&(1000 + i).to_string(), "hello")).collect();
        let mock = serve(vec![
            route("GET", "/channels/C1", json!({"id": "C1", "name": "general", "guild_id": "G1", "type": 0})),
            route("GET", "/channels/C1/messages", json!([msg("1100", "Release notes are up")])).query_has("after=1099"),
            route("GET", "/channels/C1/messages", json!(page1)).query_has("after=999"),
        ])
        .await;

        let mut conn = mock.conn(json!({"bot_token": "bot-test"}));
        conn.config["channel_id"] = json!("C1");
        let res = DiscordSource.fetch(&conn, Some(r#"{"C1": 999}"#.into())).await.unwrap();
        assert_eq!(res.records.len(), 101);
        assert_eq!(res.cursor.as_deref(), Some(r#"{"C1":1100}"#));
        assert!(mock.requests().iter().all(|r| r.header("authorization") == "Bot bot-test"));

        let envs = envelopes(&DiscordSource, &res.records);
        let last = envs.iter().find(|e| e.external_id == "1100").unwrap();
        assert_eq!(last.title, "#general Ada: Release notes are up");
        assert_eq!(last.url, "https://discord.com/channels/G1/C1/1100");
    }

    #[tokio::test]
    async fn a_missing_channel_id_is_a_visible_error() {
        let mock = serve(vec![]).await;
        let err = DiscordSource.fetch(&mock.conn(json!({"bot_token": "b"})), None).await.unwrap_err();
        assert!(err.message.contains("missing channel_id"));
    }

    #[tokio::test]
    async fn fetch_errors_are_visible() {
        for (status, needle) in [(401, "HTTP 401"), (429, "HTTP 429"), (500, "HTTP 500")] {
            let mock = serve(vec![route("GET", "/channels/C1", json!({"message": "nope"})).status(status)]).await;
            let mut conn = mock.conn(json!({"bot_token": "b"}));
            conn.config["channel_id"] = json!("C1");
            let err = DiscordSource.fetch(&conn, None).await.unwrap_err();
            assert!(err.message.contains(needle), "{}", err.message);
        }
    }
}
