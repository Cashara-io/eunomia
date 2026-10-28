//! Ed Discussion source: one record per thread (question, post or announcement) in the courses the
//! token's owner is enrolled in, with its answers and comments flattened into the body. Uses Ed's
//! unofficial API (`https://{region}.edstem.org/api`), bearer token from
//! edstem.org/us/settings/api-tokens. `config.region` (`us`, `au` or `eu`) picks the host and
//! `config.course_id` (comma-separated) limits the courses; unset, every active enrolment is synced.
//!
//! Incremental: the cursor is the newest thread `updated_at` seen in the course listings (UTC). Each
//! sync lists every course newest first, keeps threads updated at or after the cursor, and reads the
//! oldest 100 of them in full. `>=` re-reads the threads sharing the cursor's instant, which is safe
//! because the upsert is idempotent and means a tie cut by the 100 cap is never skipped. Only
//! `MAX_PAGES` pages of 100 are scanned per course, so threads deeper than that are never seen.

use std::collections::HashSet;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Map, Value};

use crate::connectors::clients::{bearer, require, str_field, Api};
use crate::error::{AppError, AppResult};
use crate::sources::base::{envelope, items, rfc3339, s, Conn, Source, SyncResult, MAX_PAGES};

/// The hosts Ed runs. The region goes into a hostname, so anything else is refused.
const REGIONS: &[&str] = &["us", "au", "eu"];
const PAGE_SIZE: usize = 100;
const LOOKBACK_DAYS: i64 = 30;
/// Each thread costs one extra request; bound one sync. When more are waiting, the oldest are read
/// first and the cursor resumes at the last of them, so a backlog drains over several syncs.
const MAX_THREADS_PER_SYNC: usize = 100;
const MAX_BODY_CHARS: usize = 20_000;

fn region(conn: &Conn) -> AppResult<String> {
    let region = str_field(&conn.config, "region").to_lowercase();
    if region.is_empty() {
        return Ok("us".to_string());
    }
    if REGIONS.contains(&region.as_str()) {
        Ok(region)
    } else {
        Err(AppError::bad_request(format!("region must be one of {}", REGIONS.join(", "))))
    }
}

fn api(conn: &Conn) -> AppResult<(Api, String)> {
    let region = region(conn)?;
    let base = format!("https://{region}.edstem.org/api");
    let token = require(&conn.credentials, "api_token")?;
    Ok((Api::new(&conn.config, &base, bearer(&token)), region))
}

/// `config.course_id`: one id, or several comma-separated. Empty: every active course.
fn course_ids(conn: &Conn) -> HashSet<String> {
    str_field(&conn.config, "course_id").split(',').map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect()
}

fn int(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(Value::as_i64)
}

/// A thread's `updated_at` as a UTC instant (compared as instants: Ed's offset varies).
fn updated_at(thread: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s(thread, "/updated_at")).ok().map(|d| d.with_timezone(&Utc))
}

/// Who wrote `node` (a thread, answer or comment), from the `_users` name map stashed on the record.
fn author(users: &Value, node: &Value) -> String {
    if node.get("is_anonymous").and_then(Value::as_bool) == Some(true) {
        return "Anonymous".to_string();
    }
    let name = int(node, "user_id").map(|id| s(users, &format!("/{id}"))).unwrap_or("");
    (if name.is_empty() { "Unknown" } else { name }).to_string()
}

fn is_deleted(node: &Value) -> bool {
    !s(node, "/deleted_at").is_empty()
}

/// `Author: text` for each live node and, recursively, its replies.
fn flatten(users: &Value, nodes: &[Value], out: &mut Vec<String>) {
    for node in nodes {
        let text = s(node, "/document").trim();
        if !is_deleted(node) && !text.is_empty() {
            out.push(format!("{}: {text}", author(users, node)));
        }
        flatten(users, &items(node, "/comments"), out);
    }
}

/// Live comments at any depth under `nodes`.
fn count_comments(nodes: &[Value]) -> usize {
    nodes.iter().map(|n| usize::from(!is_deleted(n)) + count_comments(&items(n, "/comments"))).sum()
}

pub struct EdSource;

#[async_trait]
impl Source for EdSource {
    fn key(&self) -> &'static str {
        "ed"
    }

    fn label(&self) -> &'static str {
        "Ed Discussion"
    }

    fn record_types(&self) -> &'static [&'static str] {
        &["ed.thread"]
    }

    async fn check(&self, conn: &Conn) -> AppResult<()> {
        api(conn)?.0.get("/user", &[]).await.map(|_| ())
    }

    async fn fetch(&self, conn: &Conn, cursor: Option<String>) -> AppResult<SyncResult> {
        let (api, region) = api(conn)?;
        let since = cursor
            .as_deref()
            .and_then(|c| DateTime::parse_from_rfc3339(c).ok())
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|| Utc::now() - Duration::days(LOOKBACK_DAYS));
        let wanted = course_ids(conn);

        // (updated_at, course, listed thread) of everything changed since the cursor, over all courses
        let mut candidates: Vec<(DateTime<Utc>, Value, Value)> = Vec::new();
        for enrolment in items(&api.get("/user", &[]).await?, "/courses") {
            let course = enrolment.get("course").cloned().unwrap_or(Value::Null);
            let Some(course_id) = int(&course, "id") else { continue };
            let wanted_here = if wanted.is_empty() { s(&course, "/status") == "active" } else { wanted.contains(&course_id.to_string()) };
            if !wanted_here {
                continue;
            }
            for page in 0..MAX_PAGES {
                let query = [("limit", PAGE_SIZE.to_string()), ("offset", (page * PAGE_SIZE).to_string()), ("sort", "new".to_string())];
                let threads = items(&api.get(&format!("/courses/{course_id}/threads"), &query).await?, "/threads");
                let full = threads.len() >= PAGE_SIZE;
                for thread in threads {
                    if let Some(at) = updated_at(&thread).filter(|at| *at >= since) {
                        candidates.push((at, course.clone(), thread));
                    }
                }
                if !full {
                    break;
                }
            }
        }

        // Oldest first, so the cursor never skips past a thread left unread by the cap. A failed read
        // fails the sync and keeps the cursor, so it is retried.
        candidates.sort_by_key(|(at, _, thread)| (*at, int(thread, "id").unwrap_or(0)));
        candidates.truncate(MAX_THREADS_PER_SYNC);
        let mut records = Vec::with_capacity(candidates.len());
        for (_, course, listed) in &candidates {
            let Some(id) = int(listed, "id") else { continue };
            let detail = api.get(&format!("/threads/{id}"), &[]).await?;
            let mut thread = detail.get("thread").filter(|t| t.is_object()).cloned().unwrap_or_else(|| listed.clone());
            let mut users: Map<String, Value> = items(&detail, "/users")
                .iter()
                .filter_map(|u| Some((int(u, "id")?.to_string(), json!(s(u, "/name")))))
                .collect();
            if let Some(uid) = listed.pointer("/user/id").and_then(Value::as_i64) {
                users.entry(uid.to_string()).or_insert_with(|| json!(s(listed, "/user/name")));
            }
            let users = Value::Object(users);
            thread["_author"] = json!(author(&users, &thread));
            thread["_users"] = users;
            thread["_region"] = json!(region);
            thread["_course_name"] = json!(s(course, "/name"));
            thread["_course_code"] = json!(s(course, "/code"));
            records.push(thread);
        }
        // normalized to UTC, so the next sync compares instants and not text
        let newest = candidates.last().map(|(at, _, _)| at.to_rfc3339());
        Ok(SyncResult { records, cursor: newest.or(cursor.map(|_| since.to_rfc3339())) })
    }

    fn map(&self, raw: &Value) -> Option<Value> {
        let id = int(raw, "id")?;
        let course_id = int(raw, "course_id").unwrap_or(0);
        let region = if s(raw, "/_region").is_empty() { "us" } else { s(raw, "/_region") };
        let users = raw.get("_users").unwrap_or(&Value::Null);
        let (answers, comments) = (items(raw, "/answers"), items(raw, "/comments"));

        let mut parts = Vec::new();
        if !s(raw, "/document").trim().is_empty() {
            parts.push(s(raw, "/document").trim().to_string());
        }
        flatten(users, &answers, &mut parts);
        flatten(users, &comments, &mut parts);
        let body: String = parts.join("\n\n").chars().take(MAX_BODY_CHARS).collect();

        let title = s(raw, "/title");
        let mut env = envelope(
            "ed",
            "ed.thread",
            &id.to_string(),
            if title.is_empty() { "(untitled)" } else { title },
            &body,
            rfc3339(s(raw, "/updated_at")),
            &format!("https://edstem.org/{region}/courses/{course_id}/discussion/{id}"),
            json!({
                "course": s(raw, "/_course_name"),
                "course_code": s(raw, "/_course_code"),
                "category": s(raw, "/category"),
                "subcategory": s(raw, "/subcategory"),
                "type": s(raw, "/type"),
                "author": s(raw, "/_author"),
                "is_answered": raw.get("is_answered"),
                "answers": answers.iter().filter(|a| !is_deleted(a)).count(),
                "comments": answers.iter().map(|a| count_comments(&items(a, "/comments"))).sum::<usize>() + count_comments(&comments),
            }),
        );
        if is_deleted(raw) {
            env["deleted"] = json!(true);
        }
        Some(env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::mock::{assert_fetch_fails, envelopes, route, serve, Route};

    fn me(courses: &[(i64, &str, &str)]) -> Value {
        let courses: Vec<Value> = courses
            .iter()
            .map(|(id, name, status)| json!({"course": {"id": id, "code": format!("C{id}"), "name": name, "status": status}, "role": {"role": "student"}}))
            .collect();
        json!({"user": {"id": 7, "name": "Sam"}, "courses": courses})
    }

    fn listed(id: i64, course: i64, updated: &str) -> Value {
        json!({"id": id, "number": id % 1000, "course_id": course, "user_id": 7, "title": format!("Thread {id}"), "category": "General",
               "type": "question", "is_answered": false, "created_at": "2024-01-01T00:00:00+00:00", "updated_at": updated,
               "user": {"id": 7, "name": "Ada"}})
    }

    fn detail(id: i64, course: i64, updated: &str) -> Route {
        route("GET", &format!("/threads/{id}"), json!({
            "thread": {"id": id, "number": id % 1000, "course_id": course, "user_id": 7, "title": format!("Thread {id}"), "category": "General",
                       "subcategory": "Week 1", "type": "question", "document": "How do I start?", "is_answered": false, "is_anonymous": false,
                       "updated_at": updated, "deleted_at": null, "answers": [], "comments": []},
            "users": [{"id": 7, "name": "Ada", "role": "student"}]}))
    }

    fn creds() -> Value {
        json!({"api_token": "tok"})
    }

    #[tokio::test]
    async fn fetch_pages_compares_instants_and_sends_the_bearer_token() {
        let mut page1: Vec<Value> = (1..=98).map(|i| listed(i, 10, "2024-01-01T00:00:00+00:00")).collect();
        page1.push(listed(99, 10, "2024-04-30T20:00:00-05:00")); // 2024-05-01T01:00Z: after the cursor
        page1.push(listed(100, 10, "2024-05-01T11:00:00+12:00")); // 2024-04-30T23:00Z: before it, though its text sorts after
        let mock = serve(vec![
            route("GET", "/user", me(&[(10, "Intro to Rust", "active")])),
            route("GET", "/courses/10/threads", json!({"threads": page1})).query_has("offset=0"),
            // the cursor's own instant, written in another offset: kept (`>=`)
            route("GET", "/courses/10/threads", json!({"threads": [listed(101, 10, "2024-05-01T10:00:00+10:00")]})).query_has("offset=100"),
            detail(99, 10, "2024-04-30T20:00:00-05:00"),
            detail(101, 10, "2024-05-01T10:00:00+10:00"),
        ])
        .await;

        let res = EdSource.fetch(&mock.conn(creds()), Some("2024-05-01T10:00:00+10:00".into())).await.unwrap();
        assert_eq!(res.records.len(), 2);
        assert_eq!(res.cursor.as_deref(), Some("2024-05-01T01:00:00+00:00"));
        let reqs = mock.requests();
        assert!(reqs.iter().all(|r| r.header("authorization") == "Bearer tok"));
        assert!(reqs.iter().any(|r| r.path == "/threads/99" && !r.query.contains("view")), "detail is read without view=1");
        assert!(reqs.iter().all(|r| r.path != "/threads/100"), "an older thread is not read");

        let envs = envelopes(&EdSource, &res.records);
        let first = envs.iter().find(|e| e.external_id == "99").unwrap();
        assert_eq!(first.id, "ed:ed.thread:99");
        assert_eq!(first.title, "Thread 99");
        assert_eq!(first.url, "https://edstem.org/us/courses/10/discussion/99");
        assert_eq!(first.payload["course"], "Intro to Rust");
        assert_eq!(first.payload["author"], "Ada");
        assert_eq!(first.payload["category"], "General");
    }

    #[tokio::test]
    async fn a_tie_cut_by_the_cap_is_read_on_the_next_sync() {
        // 102 threads: the 100th, 101st and 102nd share one instant, so the cap cuts the tie
        let when = |i: i64| if i < 99 { format!("2024-03-01T00:{:02}:{:02}Z", i / 60, i % 60) } else { "2024-03-01T01:00:00Z".to_string() };
        let all: Vec<Value> = (0..102).rev().map(|i| listed(1000 + i, 10, &when(i))).collect();
        let mut routes = vec![
            route("GET", "/user", me(&[(10, "Intro", "active")])),
            route("GET", "/courses/10/threads", json!({"threads": all[..100]})).query_has("offset=0"),
            route("GET", "/courses/10/threads", json!({"threads": all[100..]})).query_has("offset=100"),
        ];
        routes.extend((0..102).map(|i| detail(1000 + i, 10, &when(i))));
        let mock = serve(routes).await;
        let conn = mock.conn(creds());

        let first = EdSource.fetch(&conn, Some("2024-01-01T00:00:00Z".into())).await.unwrap();
        assert_eq!(first.records.len(), 100);
        assert_eq!(first.cursor.as_deref(), Some("2024-03-01T01:00:00+00:00"));
        assert_eq!(first.records.last().unwrap()["id"], 1099, "oldest first, ties by id");

        let second = EdSource.fetch(&conn, first.cursor).await.unwrap();
        let ids: Vec<i64> = second.records.iter().map(|r| r["id"].as_i64().unwrap()).collect();
        assert_eq!(ids, vec![1099, 1100, 1101], "the whole tie, including the cursor's own thread");
    }

    #[tokio::test]
    async fn only_active_courses_are_synced_unless_course_ids_are_listed() {
        let routes = || {
            vec![
                route("GET", "/user", me(&[(10, "Live", "active"), (11, "Old", "archived"), (12, "Also live", "active")])),
                route("GET", "/courses/10/threads", json!({"threads": []})),
                route("GET", "/courses/11/threads", json!({"threads": []})),
                route("GET", "/courses/12/threads", json!({"threads": []})),
            ]
        };
        let listed_courses = |mock: &crate::sources::mock::Mock| -> Vec<String> {
            mock.requests().into_iter().filter(|r| r.path.ends_with("/threads")).map(|r| r.path).collect()
        };

        let mock = serve(routes()).await;
        EdSource.fetch(&mock.conn(creds()), None).await.unwrap();
        assert_eq!(listed_courses(&mock), vec!["/courses/10/threads", "/courses/12/threads"]);

        let mock = serve(routes()).await;
        let mut conn = mock.conn(creds());
        conn.config["course_id"] = json!("12, 99");
        EdSource.fetch(&conn, None).await.unwrap();
        assert_eq!(listed_courses(&mock), vec!["/courses/12/threads"]);
    }

    #[tokio::test]
    async fn the_first_sync_looks_back_30_days() {
        let ago = |days: i64| (Utc::now() - Duration::days(days)).to_rfc3339();
        let (recent, old) = (ago(5), ago(60));
        let mock = serve(vec![
            route("GET", "/user", me(&[(10, "Intro", "active")])),
            route("GET", "/courses/10/threads", json!({"threads": [listed(1, 10, &recent), listed(2, 10, &old)]})),
            detail(1, 10, &recent),
            detail(2, 10, &old),
        ])
        .await;
        let res = EdSource.fetch(&mock.conn(creds()), None).await.unwrap();
        assert_eq!(res.records.len(), 1);
        assert_eq!(res.records[0]["id"], 1);
        assert_eq!(res.cursor, rfc3339(&recent));
    }

    #[tokio::test]
    async fn a_region_outside_the_allowlist_is_refused_before_any_request() {
        let mock = serve(vec![]).await;
        for bad in ["evil.com/x#", "us.evil", "usa", "../"] {
            let mut conn = mock.conn(creds());
            conn.config["region"] = json!(bad);
            let err = EdSource.fetch(&conn, None).await.unwrap_err();
            assert!(err.message.contains("region must be one of us, au, eu"), "{bad:?}: {}", err.message);
            assert!(EdSource.check(&conn).await.is_err(), "{bad:?}");
        }
        let mut conn = mock.conn(creds());
        conn.config["region"] = json!(" AU ");
        assert_eq!(region(&conn).unwrap(), "au");
        assert!(mock.requests().is_empty());
    }

    #[tokio::test]
    async fn a_missing_token_is_a_visible_error() {
        let mock = serve(vec![]).await;
        let err = EdSource.fetch(&mock.conn(json!({})), None).await.unwrap_err();
        assert!(err.message.contains("missing api_token"));
    }

    #[tokio::test]
    async fn check_calls_user_with_the_token() {
        let mock = serve(vec![route("GET", "/user", me(&[]))]).await;
        EdSource.check(&mock.conn(creds())).await.unwrap();
        assert_eq!(mock.requests()[0].header("authorization"), "Bearer tok");
    }

    #[tokio::test]
    async fn fetch_errors_are_visible() {
        for status in [401, 429, 500] {
            assert_fetch_fails(&EdSource, status, "/user", creds(), &format!("HTTP {status}")).await;
        }
        // a failing thread read fails the sync too, rather than skipping the thread
        let mock = serve(vec![
            route("GET", "/user", me(&[(10, "Intro", "active")])),
            route("GET", "/courses/10/threads", json!({"threads": [listed(1, 10, "2999-01-01T00:00:00Z")]})),
            route("GET", "/threads/1", json!({"message": "nope"})).status(500),
        ])
        .await;
        let err = EdSource.fetch(&mock.conn(creds()), None).await.unwrap_err();
        assert!(err.message.contains("HTTP 500"), "{}", err.message);
    }

    #[test]
    fn map_flattens_answers_and_comments_recursively() {
        let raw = json!({
            "id": 501, "course_id": 10, "title": "Q about lifetimes", "document": "How do borrows end?", "category": "Lectures",
            "subcategory": "Week 1", "type": "question", "is_answered": true, "updated_at": "2024-03-01T10:00:00+11:00", "deleted_at": null,
            "_region": "au", "_course_name": "Intro", "_course_code": "X101", "_author": "Ada", "_users": {"8": "Grace", "9": "Linus"},
            "answers": [{"user_id": 8, "document": "At last use.", "is_anonymous": false, "deleted_at": null,
                         "comments": [{"user_id": 9, "document": "Thanks!", "deleted_at": null,
                                       "comments": [{"user_id": 8, "is_anonymous": true, "document": "Anytime", "deleted_at": null, "comments": []}]}]}],
            "comments": [{"user_id": 9, "document": "removed text", "deleted_at": "2024-03-02T00:00:00Z", "comments": []},
                         {"user_id": 99, "document": "Me too", "deleted_at": null, "comments": []}]
        });
        let envs = envelopes(&EdSource, std::slice::from_ref(&raw));
        let env = &envs[0];
        assert_eq!(env.body_text, "How do borrows end?\n\nGrace: At last use.\n\nLinus: Thanks!\n\nAnonymous: Anytime\n\nUnknown: Me too");
        assert_eq!(env.url, "https://edstem.org/au/courses/10/discussion/501");
        assert_eq!(env.occurred_at.as_ref().map(|d| serde_json::to_value(d).unwrap()), Some(json!("2024-02-29T23:00:00Z")));
        assert!(!env.deleted);
        assert_eq!(env.payload["answers"], 1);
        assert_eq!(env.payload["comments"], 3, "the deleted comment is not counted");
        assert_eq!(env.payload["course_code"], "X101");
        assert_eq!(env.payload["subcategory"], "Week 1");
        assert_eq!(env.payload["is_answered"], true);
    }

    #[test]
    fn map_marks_a_deleted_thread_and_caps_the_body() {
        let raw = json!({"id": 7, "course_id": 3, "title": "", "document": "x".repeat(30_000), "updated_at": "2024-03-01T10:00:00Z",
                         "deleted_at": "2024-03-02T10:00:00Z"});
        let envs = envelopes(&EdSource, std::slice::from_ref(&raw));
        assert!(envs[0].deleted);
        assert_eq!(envs[0].title, "(untitled)");
        assert_eq!(envs[0].body_text.chars().count(), MAX_BODY_CHARS);
        assert_eq!(envs[0].url, "https://edstem.org/us/courses/3/discussion/7", "no stashed region: us");
        assert!(EdSource.map(&json!({"id": "7"})).is_none(), "ids are integers");
    }
}
