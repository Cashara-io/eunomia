//! Todoist source: active tasks, via the unified API v1 (`GET /tasks`,
//! cursor-paginated), with project names from `GET /projects`. API token,
//! bearer auth. Every sync re-reads the active task list; the ingest skips
//! unchanged tasks.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::connectors::clients::{bearer, require, Api};
use crate::error::AppResult;
use crate::sources::base::{envelope, items, rfc3339, s, Conn, Source, SyncResult, MAX_PAGES};

const BASE_URL: &str = "https://api.todoist.com/api/v1";

fn api(conn: &Conn) -> AppResult<Api> {
    Ok(Api::new(&conn.config, BASE_URL, bearer(&require(&conn.credentials, "api_token")?)))
}

/// Every page of a `{results, next_cursor}` list endpoint.
async fn all_pages(api: &Api, path: &str) -> AppResult<Vec<Value>> {
    let mut out = Vec::new();
    let mut cursor = String::new();
    for _ in 0..MAX_PAGES {
        let mut query = vec![("limit", "200".to_string())];
        if !cursor.is_empty() {
            query.push(("cursor", cursor.clone()));
        }
        let page = api.get(path, &query).await?;
        out.extend(items(&page, "/results"));
        cursor = s(&page, "/next_cursor").to_string();
        if cursor.is_empty() {
            break;
        }
    }
    Ok(out)
}

pub struct TodoistSource;

#[async_trait]
impl Source for TodoistSource {
    fn key(&self) -> &'static str {
        "todoist"
    }

    fn label(&self) -> &'static str {
        "Todoist"
    }

    fn record_types(&self) -> &'static [&'static str] {
        &["todoist.task"]
    }

    async fn check(&self, conn: &Conn) -> AppResult<()> {
        api(conn)?.get("/projects", &[("limit", "1".to_string())]).await.map(|_| ())
    }

    async fn fetch(&self, conn: &Conn, _cursor: Option<String>) -> AppResult<SyncResult> {
        let api = api(conn)?;
        let projects: HashMap<String, String> =
            all_pages(&api, "/projects").await?.iter().map(|p| (s(p, "/id").to_string(), s(p, "/name").to_string())).collect();
        let mut records = all_pages(&api, "/tasks").await?;
        for t in records.iter_mut() {
            t["_project_name"] = json!(projects.get(s(t, "/project_id")).cloned().unwrap_or_default());
        }
        Ok(SyncResult { records, cursor: None, ..SyncResult::default() })
    }

    fn map(&self, raw: &Value) -> Option<Value> {
        let id = raw.get("id").and_then(|v| v.as_str())?;
        let content = s(raw, "/content");
        let description = s(raw, "/description");
        let due = s(raw, "/due/date");
        let body = [content, description, if due.is_empty() { "" } else { due }]
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let updated = s(raw, "/updated_at");
        Some(envelope(
            "todoist",
            "todoist.task",
            id,
            content,
            &body,
            rfc3339(if updated.is_empty() { s(raw, "/added_at") } else { updated }),
            &format!("https://app.todoist.com/app/task/{id}"),
            json!({
                "project": s(raw, "/_project_name"),
                "due": raw.get("due"),
                "priority": raw.get("priority"),
                "labels": raw.get("labels"),
                "added_at": raw.get("added_at"),
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::mock::{assert_fetch_fails, envelopes, route, serve};

    #[tokio::test]
    async fn fetch_follows_next_cursor_and_names_projects() {
        let task = |id: &str, content: &str| {
            json!({"id": id, "project_id": "p1", "content": content, "description": "Bring the receipts",
                   "due": {"date": "2024-07-01", "is_recurring": false, "string": "Jul 1"}, "priority": 4,
                   "labels": ["admin"], "checked": false, "added_at": "2024-06-01T09:00:00.000000Z",
                   "updated_at": "2024-06-02T09:00:00.000000Z"})
        };
        let mock = serve(vec![
            route("GET", "/projects", json!({"results": [{"id": "p1", "name": "Home"}], "next_cursor": null})),
            route("GET", "/tasks", json!({"results": [task("t2", "Book dentist")], "next_cursor": null})).query_has("cursor=abc"),
            route("GET", "/tasks", json!({"results": [task("t1", "File taxes")], "next_cursor": "abc"})),
        ])
        .await;

        let res = TodoistSource.fetch(&mock.conn(json!({"api_token": "todo-test"})), None).await.unwrap();
        assert_eq!(res.records.len(), 2);
        assert!(mock.requests().iter().all(|r| r.header("authorization") == "Bearer todo-test"));

        let envs = envelopes(&TodoistSource, &res.records);
        assert_eq!(envs[0].title, "File taxes");
        assert_eq!(envs[0].body_text, "File taxes\nBring the receipts\n2024-07-01");
        assert_eq!(envs[0].payload["project"], "Home");
        assert_eq!(envs[0].url, "https://app.todoist.com/app/task/t1");
    }

    #[tokio::test]
    async fn fetch_errors_are_visible() {
        let creds = json!({"api_token": "bad"});
        assert_fetch_fails(&TodoistSource, 401, "/projects", creds.clone(), "HTTP 401").await;
        assert_fetch_fails(&TodoistSource, 429, "/projects", creds.clone(), "HTTP 429").await;
        assert_fetch_fails(&TodoistSource, 500, "/projects", creds, "HTTP 500").await;
    }
}
