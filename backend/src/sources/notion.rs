//! Notion source: pages and databases shared with an internal integration,
//! via `POST /v1/search` (newest-edited first, cursor-paginated), plus each
//! changed page's text from `GET /v1/blocks/{id}/children`. Integration
//! token, bearer auth + `Notion-Version`.
//!
//! Incremental: stop paging once results are older than the cursor (the
//! newest `last_edited_time` seen last sync). Notion allows about 3 requests a second, and every page
//! costs one block request, so the block reads are paced and capped per sync: the oldest changed
//! records go first and the cursor resumes after the last one read, so a big workspace (the first
//! sync) drains over several syncs instead of hitting a 429 that fails every attempt from scratch.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::connectors::clients::{bearer, require, Api};
use crate::error::AppResult;
use crate::sources::base::{envelope, items, rfc3339, s, Conn, Source, SyncResult, MAX_PAGES};

const BASE_URL: &str = "https://api.notion.com/v1";
/// Pause before each block read: Notion's average limit is 3 requests a second.
const BLOCK_PACING: Duration = if cfg!(test) { Duration::ZERO } else { Duration::from_millis(400) };
/// Pages whose text is read in one sync (databases carry no text and are free).
const MAX_PAGES_PER_SYNC: usize = 100;

fn api(conn: &Conn) -> AppResult<Api> {
    Ok(Api::new(&conn.config, BASE_URL, bearer(&require(&conn.credentials, "integration_token")?))
        .with_header("notion-version", "2022-06-28"))
}

/// Concatenated `plain_text` of a rich-text array.
fn plain(rich: &Value) -> String {
    rich.as_array()
        .map(|a| a.iter().filter_map(|p| p.get("plain_text").and_then(|v| v.as_str())).collect())
        .unwrap_or_default()
}

/// A page's title lives in whichever property has type "title" (its name
/// varies per database); a database's in its top-level `title`.
fn extract_title(raw: &Value) -> String {
    if let Some(t) = raw.get("title") {
        return plain(t);
    }
    raw.get("properties")
        .and_then(|v| v.as_object())
        .and_then(|props| props.values().find(|p| p.get("type").and_then(|v| v.as_str()) == Some("title")))
        .map(|p| plain(&p["title"]))
        .unwrap_or_default()
}

/// The text of a page's top-level blocks: every block type keeps its
/// content in `block[block.type].rich_text`.
fn blocks_text(blocks: &[Value]) -> String {
    blocks
        .iter()
        .filter_map(|b| {
            let t = b.get("type")?.as_str()?;
            let text = plain(b.get(t)?.get("rich_text")?);
            (!text.is_empty()).then_some(text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub struct NotionSource;

#[async_trait]
impl Source for NotionSource {
    fn key(&self) -> &'static str {
        "notion"
    }

    fn label(&self) -> &'static str {
        "Notion"
    }

    fn record_types(&self) -> &'static [&'static str] {
        &["notion.page"]
    }

    async fn check(&self, conn: &Conn) -> AppResult<()> {
        api(conn)?.get("/users/me", &[]).await.map(|_| ())
    }

    async fn fetch(&self, conn: &Conn, cursor: Option<String>) -> AppResult<SyncResult> {
        let api = api(conn)?;
        let since = cursor.clone().unwrap_or_default();
        let mut changed = Vec::new();
        let mut start: Option<String> = None;
        'pages: for _ in 0..MAX_PAGES {
            let mut body = json!({"sort": {"direction": "descending", "timestamp": "last_edited_time"}, "page_size": 100});
            if let Some(c) = &start {
                body["start_cursor"] = json!(c);
            }
            let page = api.post("/search", &body).await?;
            for r in items(&page, "/results") {
                // RFC 3339 UTC strings from one API compare correctly as text.
                if s(&r, "/last_edited_time") < since.as_str() {
                    break 'pages;
                }
                changed.push(r);
            }
            if page.get("has_more").and_then(|v| v.as_bool()) != Some(true) {
                break;
            }
            start = page.get("next_cursor").and_then(|v| v.as_str()).map(String::from);
        }

        // Oldest first, so the cursor never skips past a record left unread by the cap. Equal
        // timestamps stay (`<` above), so a tie cut by the cap is read on the next sync.
        changed.sort_by(|a, b| s(a, "/last_edited_time").cmp(s(b, "/last_edited_time")));
        let mut pages = 0;
        if let Some(cut) = changed.iter().position(|r| {
            pages += usize::from(s(r, "/object") == "page");
            pages > MAX_PAGES_PER_SYNC
        }) {
            changed.truncate(cut);
        }

        for r in changed.iter_mut().filter(|r| s(r, "/object") == "page") {
            tokio::time::sleep(BLOCK_PACING).await;
            let blocks = api.get(&format!("/blocks/{}/children", s(r, "/id")), &[("page_size", "100".into())]).await?;
            r["_text"] = json!(blocks_text(&items(&blocks, "/results")));
        }
        let newest = changed.iter().map(|r| s(r, "/last_edited_time").to_string()).max();
        Ok(SyncResult { records: changed, cursor: newest.or(cursor) })
    }

    fn map(&self, raw: &Value) -> Option<Value> {
        let id = raw.get("id").and_then(|v| v.as_str())?;
        let title = extract_title(raw);
        let text = s(raw, "/_text");
        let mut env = envelope(
            "notion",
            "notion.page",
            id,
            &title,
            if text.is_empty() { &title } else { text },
            rfc3339(s(raw, "/last_edited_time")),
            s(raw, "/url"),
            json!({"object": s(raw, "/object"), "created_time": raw.get("created_time")}),
        );
        env["deleted"] = json!(raw.get("archived").and_then(|v| v.as_bool()).unwrap_or(false) || raw.get("in_trash").and_then(|v| v.as_bool()).unwrap_or(false));
        Some(env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::mock::{assert_fetch_fails, envelopes, route, serve};

    fn page(id: &str, edited: &str, title: &str) -> Value {
        json!({
            "object": "page", "id": id, "created_time": "2024-01-01T00:00:00.000Z", "last_edited_time": edited,
            "archived": false, "url": format!("https://www.notion.so/{id}"),
            "properties": {"Tags": {"type": "multi_select", "multi_select": []},
                           "Name": {"id": "title", "type": "title", "title": [{"type": "text", "plain_text": title}]}},
        })
    }

    #[test]
    fn extract_title_handles_pages_and_databases() {
        assert_eq!(extract_title(&page("p", "", "Roadmap Q1")), "Roadmap Q1");
        assert_eq!(extract_title(&json!({"object": "database", "title": [{"plain_text": "Tasks"}]})), "Tasks");
    }

    #[tokio::test]
    async fn fetch_pages_until_older_than_cursor_and_reads_block_text() {
        let mock = serve(vec![
            route("POST", "/search", json!({"object": "list", "results": [
                {"object": "database", "id": "db1", "last_edited_time": "2024-03-01T12:00:00.000Z", "title": [{"plain_text": "Tasks"}], "url": "https://www.notion.so/db1"},
            ], "has_more": false, "next_cursor": null}))
            .body("\"start_cursor\":\"c2\""),
            route("POST", "/search", json!({"object": "list", "results": [page("p1", "2024-03-02T10:00:00.000Z", "Roadmap")],
                "has_more": true, "next_cursor": "c2"})),
            route("GET", "/blocks/p1/children", json!({"object": "list", "results": [
                {"type": "heading_2", "heading_2": {"rich_text": [{"plain_text": "Q2 goals"}]}},
                {"type": "paragraph", "paragraph": {"rich_text": [{"plain_text": "Ship the "}, {"plain_text": "sync engine."}]}},
                {"type": "divider", "divider": {}},
            ], "has_more": false})),
        ])
        .await;

        let res = NotionSource
            .fetch(&mock.conn(json!({"integration_token": "ntn_test"})), Some("2024-03-01T00:00:00.000Z".into()))
            .await
            .unwrap();
        assert_eq!(res.records.len(), 2, "both search pages");
        assert_eq!(res.cursor.as_deref(), Some("2024-03-02T10:00:00.000Z"));
        let reqs = mock.requests();
        assert!(reqs.iter().all(|r| r.header("authorization") == "Bearer ntn_test" && r.header("notion-version") == "2022-06-28"));

        let envs = envelopes(&NotionSource, &res.records);
        assert_eq!(envs[0].title, "Tasks", "oldest first");
        assert_eq!(envs[0].payload["object"], "database");
        assert_eq!(envs[1].title, "Roadmap");
        assert_eq!(envs[1].body_text, "Q2 goals\nShip the sync engine.");
    }

    #[tokio::test]
    async fn fetch_stops_at_records_older_than_the_cursor() {
        let mock = serve(vec![
            route("POST", "/search", json!({"results": [page("new", "2024-03-02T00:00:00.000Z", "New"), page("old", "2024-01-01T00:00:00.000Z", "Old")],
                "has_more": true, "next_cursor": "c2"})),
            route("GET", "/blocks/new/children", json!({"results": []})),
        ])
        .await;
        let res = NotionSource
            .fetch(&mock.conn(json!({"integration_token": "t"})), Some("2024-02-01T00:00:00.000Z".into()))
            .await
            .unwrap();
        assert_eq!(res.records.len(), 1);
        assert_eq!(mock.requests().iter().filter(|r| r.method == "POST").count(), 1, "no second search page");
    }

    #[tokio::test]
    async fn a_big_backlog_is_read_oldest_first_and_the_cursor_resumes_after_it() {
        let results: Vec<Value> =
            (0..MAX_PAGES_PER_SYNC + 5).map(|i| page(&format!("p{i}"), &format!("2024-03-01T00:{:02}:{:02}.000Z", i / 60, i % 60), "T")).collect();
        let mut routes = vec![route("POST", "/search", json!({"results": results.iter().rev().cloned().collect::<Vec<_>>(), "has_more": false}))];
        routes.extend((0..MAX_PAGES_PER_SYNC + 5).map(|i| route("GET", &format!("/blocks/p{i}/children"), json!({"results": []}))));
        let mock = serve(routes).await;
        let res = NotionSource.fetch(&mock.conn(json!({"integration_token": "t"})), None).await.unwrap();
        assert_eq!(res.records.len(), MAX_PAGES_PER_SYNC);
        assert_eq!(s(&res.records[0], "/id"), "p0", "oldest first");
        let last = MAX_PAGES_PER_SYNC - 1;
        assert_eq!(res.cursor.as_deref(), Some(format!("2024-03-01T00:{:02}:{:02}.000Z", last / 60, last % 60).as_str()));
    }

    #[tokio::test]
    async fn fetch_errors_are_visible() {
        let creds = json!({"integration_token": "bad"});
        assert_fetch_fails(&NotionSource, 401, "/search", creds.clone(), "HTTP 401").await;
        assert_fetch_fails(&NotionSource, 429, "/search", creds.clone(), "HTTP 429").await;
        assert_fetch_fails(&NotionSource, 500, "/search", creds, "HTTP 500").await;
    }
}
