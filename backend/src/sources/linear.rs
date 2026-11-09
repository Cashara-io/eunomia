//! Linear source: issues assigned to the API key's owner, via the GraphQL
//! API (`viewer.assignedIssues`), cursor-paginated. Personal API key, sent
//! bare in `Authorization`.
//!
//! GraphQL reports failures as an `errors` array (often with HTTP 200), so
//! [`query`] turns that into an error. Incremental: `updatedAt > cursor`.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::connectors::clients::{auth_header, require, Api};
use crate::error::{AppError, AppResult};
use crate::sources::base::{envelope, items, rfc3339, s, Conn, Source, SyncResult, MAX_PAGES};

const BASE_URL: &str = "https://api.linear.app";

fn api(conn: &Conn) -> AppResult<Api> {
    Ok(Api::new(&conn.config, BASE_URL, auth_header(&require(&conn.credentials, "api_key")?)))
}

async fn query(api: &Api, query: &str, variables: Value) -> AppResult<Value> {
    let v = api.post("/graphql", &json!({"query": query, "variables": variables})).await?;
    if let Some(err) = v.pointer("/errors/0/message").and_then(|m| m.as_str()) {
        return Err(AppError::bad_request(format!("linear: {err}")));
    }
    Ok(v)
}

/// `since` is our own RFC 3339 cursor, safe to inline.
fn issues_query(since: &str) -> String {
    format!(
        "query($after: String) {{ viewer {{ assignedIssues(first: 50, after: $after, orderBy: updatedAt, \
         filter: {{ updatedAt: {{ gt: \"{since}\" }} }}) {{ \
         nodes {{ id identifier title description url priorityLabel createdAt updatedAt completedAt \
         state {{ name }} team {{ name }} project {{ name }} labels {{ nodes {{ name }} }} }} \
         pageInfo {{ hasNextPage endCursor }} }} }} }}"
    )
}

pub struct LinearSource;

#[async_trait]
impl Source for LinearSource {
    fn key(&self) -> &'static str {
        "linear"
    }

    fn label(&self) -> &'static str {
        "Linear"
    }

    fn record_types(&self) -> &'static [&'static str] {
        &["linear.issue"]
    }

    async fn check(&self, conn: &Conn) -> AppResult<()> {
        query(&api(conn)?, "{ viewer { id } }", json!({})).await.map(|_| ())
    }

    async fn fetch(&self, conn: &Conn, cursor: Option<String>) -> AppResult<SyncResult> {
        let api = api(conn)?;
        let q = issues_query(cursor.as_deref().unwrap_or("1970-01-01T00:00:00Z"));
        let mut records = Vec::new();
        let mut after = Value::Null;
        for _ in 0..MAX_PAGES {
            let page = query(&api, &q, json!({"after": after})).await?;
            let issues = page.pointer("/data/viewer/assignedIssues").cloned().unwrap_or(Value::Null);
            records.extend(items(&issues, "/nodes"));
            if issues.pointer("/pageInfo/hasNextPage").and_then(|v| v.as_bool()) != Some(true) {
                break;
            }
            after = issues.pointer("/pageInfo/endCursor").cloned().unwrap_or(Value::Null);
        }
        let newest = records.iter().map(|r| s(r, "/updatedAt").to_string()).max();
        Ok(SyncResult { records, cursor: newest.or(cursor), ..SyncResult::default() })
    }

    fn map(&self, raw: &Value) -> Option<Value> {
        let id = raw.get("id").and_then(|v| v.as_str())?;
        let identifier = s(raw, "/identifier");
        let labels: Vec<String> = items(raw, "/labels/nodes").iter().map(|l| s(l, "/name").to_string()).collect();
        Some(envelope(
            "linear",
            "linear.issue",
            id,
            &format!("{identifier}: {}", s(raw, "/title")),
            s(raw, "/description"),
            rfc3339(s(raw, "/updatedAt")),
            s(raw, "/url"),
            json!({
                "identifier": identifier,
                "state": raw.pointer("/state/name"),
                "team": raw.pointer("/team/name"),
                "project": raw.pointer("/project/name"),
                "priority": raw.get("priorityLabel"),
                "labels": labels,
                "created_at": raw.get("createdAt"),
                "completed_at": raw.get("completedAt"),
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::mock::{assert_fetch_fails, envelopes, route, serve};

    fn issue(id: &str, ident: &str, updated: &str) -> Value {
        json!({"id": id, "identifier": ident, "title": "Fix flaky test", "description": "It flakes on CI",
               "url": format!("https://linear.app/acme/issue/{ident}"), "priorityLabel": "High",
               "createdAt": "2024-01-01T00:00:00.000Z", "updatedAt": updated, "completedAt": null,
               "state": {"name": "In Progress"}, "team": {"name": "Engineering"}, "project": null,
               "labels": {"nodes": [{"name": "bug"}]}})
    }

    #[tokio::test]
    async fn fetch_paginates_with_end_cursor_and_sends_the_bare_key() {
        let mock = serve(vec![
            route("POST", "/graphql", json!({"data": {"viewer": {"assignedIssues": {
                "nodes": [issue("i2", "ENG-2", "2024-02-02T00:00:00.000Z")], "pageInfo": {"hasNextPage": false, "endCursor": "c2"}}}}}))
            .body("\"after\":\"c1\""),
            route("POST", "/graphql", json!({"data": {"viewer": {"assignedIssues": {
                "nodes": [issue("i1", "ENG-1", "2024-02-01T00:00:00.000Z")], "pageInfo": {"hasNextPage": true, "endCursor": "c1"}}}}})),
        ])
        .await;

        let res = LinearSource
            .fetch(&mock.conn(json!({"api_key": "lin_api_test"})), Some("2024-01-15T00:00:00Z".into()))
            .await
            .unwrap();
        assert_eq!(res.records.len(), 2);
        assert_eq!(res.cursor.as_deref(), Some("2024-02-02T00:00:00.000Z"));
        let reqs = mock.requests();
        assert!(reqs.iter().all(|r| r.header("authorization") == "lin_api_test"));
        assert!(reqs[0].body.contains("gt: \\\"2024-01-15T00:00:00Z\\\""), "{}", reqs[0].body);

        let envs = envelopes(&LinearSource, &res.records);
        assert_eq!(envs[0].title, "ENG-1: Fix flaky test");
        assert_eq!(envs[0].payload["state"], "In Progress");
        assert_eq!(envs[0].payload["labels"], json!(["bug"]));
    }

    #[tokio::test]
    async fn graphql_errors_fail_the_sync() {
        let mock = serve(vec![route("POST", "/graphql", json!({"errors": [{"message": "Authentication required, not authenticated"}]}))]).await;
        let err = LinearSource.fetch(&mock.conn(json!({"api_key": "x"})), None).await.unwrap_err();
        assert!(err.message.contains("Authentication required"), "{}", err.message);
    }

    #[tokio::test]
    async fn fetch_errors_are_visible() {
        let creds = json!({"api_key": "bad"});
        assert_fetch_fails(&LinearSource, 401, "/graphql", creds.clone(), "HTTP 401").await;
        assert_fetch_fails(&LinearSource, 429, "/graphql", creds.clone(), "HTTP 429").await;
        assert_fetch_fails(&LinearSource, 500, "/graphql", creds, "HTTP 500").await;
    }
}
