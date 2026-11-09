//! GitHub source: issues and pull requests you're involved in (created,
//! assigned, mentioned, subscribed) across every repo the token can see, via
//! `GET /issues?filter=all`. Personal access token, bearer auth.
//!
//! Incremental: `since` = the newest `updated_at` seen, ascending by update
//! time, following the `Link: rel="next"` header -- so a capped first sync
//! simply resumes where it stopped.

use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde_json::{json, Value};

use crate::connectors::clients::{bearer, require, Api};
use crate::error::AppResult;
use crate::sources::base::{envelope, rfc3339, s, Conn, Source, SyncResult, MAX_PAGES};

const BASE_URL: &str = "https://api.github.com";

fn api(conn: &Conn) -> AppResult<Api> {
    let token = require(&conn.credentials, "personal_access_token")?;
    Ok(Api::new(&conn.config, BASE_URL, bearer(&token))
        .with_header("accept", "application/vnd.github+json")
        .with_header("x-github-api-version", "2022-11-28"))
}

/// The `rel="next"` URL out of a `Link` header, if any.
fn next_link(headers: &reqwest::header::HeaderMap) -> Option<String> {
    let link = headers.get("link")?.to_str().ok()?;
    link.split(',').find(|part| part.contains("rel=\"next\"")).and_then(|part| {
        let start = part.find('<')? + 1;
        let end = part.find('>')?;
        Some(part[start..end].to_string())
    })
}

pub struct GitHubSource;

#[async_trait]
impl Source for GitHubSource {
    fn key(&self) -> &'static str {
        "github"
    }

    fn label(&self) -> &'static str {
        "GitHub"
    }

    fn record_types(&self) -> &'static [&'static str] {
        &["github.issue"]
    }

    async fn check(&self, conn: &Conn) -> AppResult<()> {
        api(conn)?.get("/user", &[]).await.map(|_| ())
    }

    async fn fetch(&self, conn: &Conn, cursor: Option<String>) -> AppResult<SyncResult> {
        let api = api(conn)?;
        let since = cursor.clone().unwrap_or_else(|| (Utc::now() - Duration::days(90)).to_rfc3339());
        let query = [
            ("filter", "all".to_string()),
            ("state", "all".to_string()),
            ("sort", "updated".to_string()),
            ("direction", "asc".to_string()),
            ("per_page", "100".to_string()),
            ("since", since),
        ];
        let mut records = Vec::new();
        let (mut page, mut headers) = api.get_with_headers("/issues", &query).await?;
        for i in 0..MAX_PAGES {
            records.extend(page.as_array().cloned().unwrap_or_default());
            let Some(next) = next_link(&headers).filter(|_| i + 1 < MAX_PAGES) else { break };
            (page, headers) = api.get_with_headers(&next, &[]).await?;
        }
        let newest = records.iter().filter_map(|r| r.get("updated_at").and_then(|v| v.as_str())).max().map(String::from);
        Ok(SyncResult { cursor: newest.or(cursor), records, ..SyncResult::default() })
    }

    fn map(&self, raw: &Value) -> Option<Value> {
        let number = raw.get("number")?.as_i64()?;
        let repo = s(raw, "/repository/full_name");
        let is_pr = raw.get("pull_request").is_some();
        let title = s(raw, "/title");
        let labels: Vec<&str> = raw
            .get("labels")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|l| l.get("name").and_then(|n| n.as_str())).collect())
            .unwrap_or_default();
        let kind = if is_pr { "Pull request" } else { "Issue" };
        let body = format!("{kind} {repo}#{number} by {}: {title}\n\n{}", s(raw, "/user/login"), s(raw, "/body"));
        Some(envelope(
            "github",
            "github.issue",
            &format!("{repo}#{number}"),
            title,
            body.trim(),
            rfc3339(s(raw, "/updated_at")),
            s(raw, "/html_url"),
            json!({
                "repository": repo,
                "number": number,
                "state": raw.get("state"),
                "is_pull_request": is_pr,
                "author": s(raw, "/user/login"),
                "assignees": raw.get("assignees").and_then(|v| v.as_array()).map(|a| a.iter().map(|u| s(u, "/login").to_string()).collect::<Vec<_>>()),
                "labels": labels,
                "comments": raw.get("comments"),
                "created_at": raw.get("created_at"),
                "closed_at": raw.get("closed_at"),
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::mock::{assert_fetch_fails, envelopes, route, serve};

    fn issue(number: i64, updated: &str, pr: bool) -> Value {
        let mut v = json!({
            "id": 1000 + number, "number": number, "title": format!("Issue {number}"), "state": "open",
            "body": "Steps to reproduce: run the importer twice.", "user": {"login": "octocat"},
            "labels": [{"name": "bug"}], "assignees": [{"login": "octocat"}], "comments": 2,
            "html_url": format!("https://github.com/acme/widget/issues/{number}"),
            "created_at": "2024-01-01T00:00:00Z", "updated_at": updated, "closed_at": null,
            "repository": {"full_name": "acme/widget"},
        });
        if pr {
            v["pull_request"] = json!({"url": "https://api.github.com/repos/acme/widget/pulls/2"});
        }
        v
    }

    #[tokio::test]
    async fn fetch_follows_the_link_header_and_sends_auth() {
        let mock = serve(vec![
            route("GET", "/issues", json!([issue(2, "2024-02-02T00:00:00Z", true)])).query_has("page=2"),
            route("GET", "/issues", json!([issue(1, "2024-02-01T00:00:00Z", false)]))
                .query_has("since=2024-01-15T00:00:00Z")
                .query_has("filter=all")
                .header("link", "<{base}/issues?page=2>; rel=\"next\", <{base}/issues?page=2>; rel=\"last\""),
        ])
        .await;

        let res = GitHubSource
            .fetch(&mock.conn(json!({"personal_access_token": "ghp_test"})), Some("2024-01-15T00:00:00Z".into()))
            .await
            .unwrap();
        assert_eq!(res.records.len(), 2);
        assert_eq!(res.cursor.as_deref(), Some("2024-02-02T00:00:00Z"));
        let reqs = mock.requests();
        assert!(reqs.iter().all(|r| r.header("authorization") == "Bearer ghp_test" && r.header("user-agent") == "eunomia"));

        let envs = envelopes(&GitHubSource, &res.records);
        assert_eq!(envs[0].id, "github:github.issue:acme/widget#1");
        assert_eq!(envs[0].url, "https://github.com/acme/widget/issues/1");
        assert!(envs[0].body_text.contains("run the importer twice"));
        assert_eq!(envs[1].payload["is_pull_request"], true);
    }

    /// 120 changed items over three Link-paginated pages all land in one
    /// sync, and the next sync resumes from the newest one seen.
    #[tokio::test]
    async fn fetch_imports_every_page_of_a_120_item_feed_and_resumes() {
        let page = |r: std::ops::Range<i64>| {
            json!(r.map(|n| issue(n, &format!("2024-03-01T00:{:02}:{:02}Z", n / 60, n % 60), false)).collect::<Vec<_>>())
        };
        let mock = serve(vec![
            route("GET", "/issues", json!([])).query_has("since=2024-03-01T00:01:59Z"),
            route("GET", "/issues", page(80..120)).query_has("page=3"),
            route("GET", "/issues", page(40..80)).query_has("page=2").header("link", "<{base}/issues?page=3>; rel=\"next\""),
            route("GET", "/issues", page(0..40)).header("link", "<{base}/issues?page=2>; rel=\"next\""),
        ])
        .await;
        let conn = mock.conn(json!({"personal_access_token": "t"}));
        let res = GitHubSource.fetch(&conn, None).await.unwrap();
        let numbers: std::collections::HashSet<i64> = res.records.iter().map(|r| r["number"].as_i64().unwrap()).collect();
        assert_eq!(numbers.len(), 120);
        assert_eq!(res.cursor.as_deref(), Some("2024-03-01T00:01:59Z"));

        let next = GitHubSource.fetch(&conn, res.cursor).await.unwrap();
        assert!(next.records.is_empty());
        assert_eq!(next.cursor.as_deref(), Some("2024-03-01T00:01:59Z"), "no change keeps the watermark");
    }

    #[tokio::test]
    async fn fetch_errors_are_visible() {
        let creds = json!({"personal_access_token": "bad"});
        assert_fetch_fails(&GitHubSource, 401, "/issues", creds.clone(), "HTTP 401").await;
        assert_fetch_fails(&GitHubSource, 429, "/issues", creds.clone(), "HTTP 429").await;
        assert_fetch_fails(&GitHubSource, 502, "/issues", creds, "HTTP 502").await;
    }
}
