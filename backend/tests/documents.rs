//! Uploaded documents end to end against a real in-memory SurrealDB and its built-in file bucket
//! (`memory` backend): upload -> index job -> search, recall and get find the passage with its
//! document reference -> download is byte-exact -> export -> delete removes it from retrieval and
//! storage. Plus validation, isolation, revisions and failures.

mod common;

use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{header, HeaderMap, Request, StatusCode};
use common::TestApp;
use eunomia_backend::config::Settings;
use eunomia_backend::jobs::{handlers, worker, WorkerConfig};
use eunomia_backend::models_user;
use eunomia_backend::tools::registry;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use surrealdb::types::RecordId;
use tower::ServiceExt;

const PDF: &[u8] = include_bytes!("fixtures/notes.pdf");

/// A test app with offline (stub) embeddings and a worker running the job queue.
async fn app_with(settings: Settings) -> (TestApp, tokio::sync::watch::Sender<bool>) {
    let app = TestApp::with_settings(settings).await;
    let (tx, rx) = tokio::sync::watch::channel(false);
    let cfg = WorkerConfig { id: "docs".into(), poll: Duration::from_millis(20), retry_base: Duration::from_millis(10), owner_cap: 1000, ..WorkerConfig::default() };
    tokio::spawn(worker::run(app.state.clone(), handlers::registry(), cfg, rx));
    (app, tx)
}

async fn app() -> (TestApp, tokio::sync::watch::Sender<bool>) {
    app_with(Settings { embeddings_backend: "stub".into(), ..common::test_settings() }).await
}

/// A raw request (any body) as `token`; returns status, headers and body bytes.
async fn raw(app: &TestApp, method: &str, path: &str, body: Vec<u8>, content_type: Option<&str>, token: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut req = Request::builder().method(method).uri(path).header(header::AUTHORIZATION, format!("Bearer {token}"));
    if let Some(ct) = content_type {
        req = req.header(header::CONTENT_TYPE, ct);
    }
    let resp = app.router.clone().oneshot(req.body(Body::from(body)).unwrap()).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    (status, headers, resp.into_body().collect().await.unwrap().to_bytes().to_vec())
}

fn json_of(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

async fn upload_http(app: &TestApp, filename: &str, content_type: Option<&str>, bytes: &[u8]) -> (StatusCode, Value) {
    let path = format!("/api/documents?filename={}", filename.replace(' ', "%20"));
    let (s, _, b) = raw(app, "POST", &path, bytes.to_vec(), content_type, &app.token).await;
    (s, json_of(&b))
}

/// Polls `document_get` until the document leaves `indexing`.
async fn settled(app: &TestApp, id: &str) -> Value {
    let start = Instant::now();
    loop {
        let d = app.tool("document_get", json!({"id": id})).await;
        if d["document"]["status"] != "indexing" {
            return d;
        }
        assert!(start.elapsed() < Duration::from_secs(60), "document {id} still indexing: {d}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

async fn object_exists(app: &TestApp, path: &str) -> bool {
    let db = app.db().await;
    let mut res = db.test_raw().query("file::exists(type::file('documents', $p))").bind(("p", path.to_string())).await.unwrap();
    res.take::<Option<bool>>(0).unwrap().unwrap_or(false)
}

async fn object_path(app: &TestApp, id: &str) -> String {
    let db = app.db().await;
    let rid = eunomia_backend::rid::parse(id).unwrap();
    let mut res = db.test_raw().query("SELECT VALUE object_path FROM ONLY $id").bind(("id", rid)).await.unwrap();
    res.take::<Option<String>>(0).unwrap().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upload_index_retrieve_download_export_delete() {
    let (app, _stop) = app().await;
    let text = "# Project notes\n\nThe quarterly zebra migration moves the herd to the northern pasture.\nOwner: Dana Okafor.\n";
    let (status, doc) = upload_http(&app, "project-notes.md", Some("text/markdown"), text.as_bytes()).await;
    assert_eq!(status, StatusCode::OK, "{doc}");
    assert_eq!(doc["status"], "indexing");
    assert!(doc.get("error").is_none() && doc.get("code").is_none(), "a success must not read as a failed tool call: {doc}");
    assert_eq!(doc["sha256"], sha(text.as_bytes()));
    assert_eq!(doc["size_bytes"], text.len());
    let id = doc["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("document:"));

    let got = settled(&app, &id).await;
    assert_eq!(got["document"]["status"], "ready", "{got}");
    assert_eq!(got["document"]["chunk_count"], 1);
    assert_eq!(got["text"], text);
    let chunk_id = got["chunks"][0]["chunk_id"].as_str().unwrap().to_string();
    assert_eq!(chunk_id, format!("documents:document.chunk:{}:1:0", id.trim_start_matches("document:")));
    assert_eq!(got["chunks"][0]["char_end"], text.chars().count());
    assert_eq!(got["chunks"][0]["has_embedding"], true);

    // keyword and semantic search find the passage and name its document
    for mode in ["keyword", "semantic", "hybrid"] {
        let hits = app.tool("search", json!({"query": "zebra migration", "mode": mode})).await;
        let hit = hits["results"].as_array().unwrap().iter().find(|h| h["id"] == chunk_id).unwrap_or_else(|| panic!("{mode}: {hits}"));
        assert_eq!(hit["document"]["document_id"], id);
        assert_eq!(hit["document"]["chunk_id"], chunk_id);
        assert_eq!(hit["document"]["revision"], 1);
        assert_eq!(hit["document"]["download_url"], format!("/api/documents/{id}/download"));
    }
    // recall too
    let recalled = app.tool("recall", json!({"query": "zebra migration pasture"})).await;
    let hit = recalled["results"].as_array().unwrap().iter().find(|h| h["id"] == chunk_id).unwrap_or_else(|| panic!("{recalled}"));
    assert_eq!(hit["document"]["document_id"], id);
    assert_eq!(hit["document"]["char_start"], 0);
    // and get resolves the chunk to its document
    let rec = app.tool("get", json!({"id": chunk_id})).await;
    assert_eq!(rec["document"]["document_id"], id, "{rec}");
    assert_eq!(rec["document"]["status"], "ready");
    assert_eq!(rec["document"]["sha256"], sha(text.as_bytes()));

    // the original bytes, exactly, as an attachment; audited without content
    let (s, h, bytes) = raw(&app, "GET", &format!("/api/documents/{id}/download"), vec![], None, &app.token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(bytes, text.as_bytes());
    assert_eq!(sha(&bytes), doc["sha256"].as_str().unwrap());
    assert_eq!(h[header::CONTENT_TYPE], "text/markdown; charset=utf-8");
    assert!(h[header::CONTENT_DISPOSITION].to_str().unwrap().starts_with("attachment; filename=\"project-notes.md\""));
    let dl = app.tool("document_download", json!({"id": id})).await;
    assert_eq!(dl["download"]["path"], format!("/api/documents/{id}/download"));
    assert_eq!(dl["sha256"], doc["sha256"]);

    // export: tool without vectors, HTTP with them
    let m = app.tool("document_export", json!({"id": id})).await;
    assert_eq!(m["format"], "eunomia.document-export");
    assert_eq!(m["embedding"]["dimension"], 1536);
    assert_eq!(m["embedding"]["model"], "stub");
    assert_eq!(m["chunks"][0]["embedding_state"], "present");
    assert!(m["chunks"][0].get("embedding").is_none());
    assert_eq!(m["vectors_url"]["path"], format!("/api/documents/{id}/export"));
    let (s, h, body) = raw(&app, "GET", &format!("/api/documents/{id}/export"), vec![], None, &app.token).await;
    assert_eq!(s, StatusCode::OK);
    assert!(h[header::CONTENT_DISPOSITION].to_str().unwrap().contains("project-notes.md.export.json"));
    let full = json_of(&body);
    assert_eq!(full["chunks"][0]["embedding"].as_array().unwrap().len(), 1536);
    assert_eq!(full["chunks"][0]["text"], text);

    let audit = app.http("GET", "/api/audit?limit=50", None, true).await.1;
    let rows = audit.to_string();
    for action in ["document_upload", "document_download", "document_export"] {
        assert!(rows.contains(action), "{action} not audited: {rows}");
    }
    assert!(!rows.contains("zebra"), "document content reached the audit log: {rows}");
    let events = app.control().test_raw().query("SELECT action, target, detail FROM audit_event").await.unwrap().take::<Vec<Value>>(0).unwrap();
    assert!(!serde_json::to_string(&events).unwrap().contains("zebra"));

    // delete: gone from retrieval, storage and the list at once
    let path = object_path(&app, &id).await;
    assert!(object_exists(&app, &path).await);
    let (s, _, body) = raw(&app, "DELETE", &format!("/api/documents/{id}"), vec![], None, &app.token).await;
    assert_eq!(s, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    assert_eq!(json_of(&body)["chunks"], 1);
    assert!(!object_exists(&app, &path).await);
    let hits = app.tool("search", json!({"query": "zebra migration", "mode": "keyword"})).await;
    assert_eq!(hits["results"], json!([]), "{hits}");
    let recalled = app.tool("recall", json!({"query": "zebra migration pasture"})).await;
    assert!(!recalled.to_string().contains(&chunk_id));
    assert_eq!(app.tool("get", json!({"id": chunk_id})).await["error"], "not found");
    let (s, _, body) = raw(&app, "GET", &format!("/api/documents/{id}/download"), vec![], None, &app.token).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(json_of(&body)["code"], "document.not_found");
    assert_eq!(app.tool("document_list", json!({})).await["total"], 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uploads_are_validated() {
    let (app, _stop) = app().await;
    // (filename, content type, body, status, code)
    type Case = (&'static str, Option<&'static str>, Vec<u8>, StatusCode, &'static str);
    let cases: Vec<Case> = vec![
        ("big.txt", Some("text/plain"), vec![b'a'; 300 * 1024], StatusCode::PAYLOAD_TOO_LARGE, "document.too_large"),
        ("tool.exe", Some("application/x-msdownload"), b"MZ".to_vec(), StatusCode::UNSUPPORTED_MEDIA_TYPE, "document.unsupported_type"),
        ("sheet.docx", None, b"PK".to_vec(), StatusCode::UNSUPPORTED_MEDIA_TYPE, "document.unsupported_type"),
        ("bin.txt", Some("text/plain"), vec![0xff, 0xfe, 0x00], StatusCode::BAD_REQUEST, "validation.invalid"),
        ("bad.json", Some("application/json"), b"{not json".to_vec(), StatusCode::BAD_REQUEST, "validation.invalid"),
        ("fake.pdf", Some("application/pdf"), b"hello".to_vec(), StatusCode::BAD_REQUEST, "validation.invalid"),
        ("empty.txt", Some("text/plain"), vec![], StatusCode::BAD_REQUEST, "validation.invalid"),
    ];
    for (name, ct, body, want, code) in cases {
        let (s, out) = upload_http(&app, name, ct, &body).await;
        assert_eq!((s, out["code"].as_str().unwrap_or_default()), (want, code), "{name}: {out}");
    }
    // a body past even the route's ceiling is refused before it is read
    let (s, _) = upload_http(&app, "huge.txt", Some("text/plain"), &vec![b'a'; 2 << 20]).await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
    let (s, _, out) = raw(&app, "POST", "/api/documents", b"x".to_vec(), Some("text/plain"), &app.token).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "no filename: {}", String::from_utf8_lossy(&out));

    // the tool: bad base64, both inputs, neither
    let bad = app.tool("document_upload", json!({"filename": "a.pdf", "content_base64": "%%%"})).await;
    assert!(bad["error"].as_str().unwrap().contains("base64"), "{bad}");
    let both = app.tool("document_upload", json!({"filename": "a.txt", "text": "x", "content_base64": "eA=="})).await;
    assert!(both["error"].as_str().unwrap().contains("exactly one"), "{both}");
    let tool_big = app.tool("document_upload", json!({"filename": "a.txt", "text": "a".repeat(300 * 1024)})).await;
    assert_eq!(tool_big["code"], "document.too_large");
    // nothing was stored
    assert_eq!(app.tool("document_list", json!({})).await["total"], 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_user_or_org_cannot_reach_a_document() {
    let (app, _stop) = app().await;
    let (_, doc) = upload_http(&app, "secret.txt", Some("text/plain"), b"the vault combination is 4-8-15").await;
    let id = doc["id"].as_str().unwrap().to_string();
    settled(&app, &id).await;
    let chunk = format!("documents:document.chunk:{}:1:0", id.trim_start_matches("document:"));

    // a second user in the same org, and a user in an org of their own
    let colleague = common::register(&app.state, "colleague@example.com").await;
    let stranger = common::register_personal(&app.state, "stranger@example.com").await;
    for other in [&colleague, &stranger] {
        let token = models_user::create_api_token(&app.state.control, &other.id, "t").await.unwrap().token;
        for (method, path) in [
            ("GET", format!("/api/documents/{id}")),
            ("GET", format!("/api/documents/{id}/download")),
            ("GET", format!("/api/documents/{id}/export")),
            ("POST", format!("/api/documents/{id}/reindex")),
            ("DELETE", format!("/api/documents/{id}")),
        ] {
            let (s, _, body) = raw(&app, method, &path, vec![], None, &token).await;
            assert_eq!(s, StatusCode::NOT_FOUND, "{} {method} {path}: {}", other.email, String::from_utf8_lossy(&body));
            assert!(!String::from_utf8_lossy(&body).contains("combination"));
        }
        let listed = raw(&app, "GET", "/api/documents", vec![], None, &token).await;
        assert_eq!(json_of(&listed.2)["total"], 0);
        for tool in ["document_get", "document_download", "document_export", "document_delete"] {
            let out = common::sys(registry::call(&app.state, other, tool, json!({"id": id}))).await.unwrap();
            assert_eq!(out["code"], "document.not_found", "{tool}: {out}");
        }
        let replaced = common::sys(registry::call(&app.state, other, "document_upload", json!({"filename": "x.txt", "text": "mine now", "document_id": id}))).await.unwrap();
        assert_eq!(replaced["code"], "document.not_found", "{replaced}");
        let hits = common::sys(registry::call(&app.state, other, "search", json!({"query": "vault combination"}))).await.unwrap();
        assert!(!hits.to_string().contains("combination"), "{hits}");
        let got = common::sys(registry::call(&app.state, other, "get", json!({"id": chunk}))).await.unwrap();
        assert_eq!(got["error"], "not found");
    }
    // guessed and malformed ids
    for guess in ["document:aaaaaaaaaaaaaaaaaaaa", "memory:abc", "document:../x", "x"] {
        assert_eq!(app.tool("document_get", json!({"id": guess})).await["code"], "document.not_found", "{guess}");
    }
    // a token restricted to an org vault cannot reach personal-vault documents
    let vault = app.tool("vault_create", json!({"name": "Team", "kind": "org"})).await;
    let vid = eunomia_backend::rid::parse(vault["id"].as_str().unwrap()).unwrap();
    let scopes: Vec<String> = ["memory:read", "memory:write"].iter().map(|s| s.to_string()).collect();
    let restricted = models_user::create_api_token_with(&app.state.control, &app.user.id, "r", &scopes, Some(&vid), None).await.unwrap().token;
    let (s, _, _) = raw(&app, "GET", &format!("/api/documents/{id}/download"), vec![], None, &restricted).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // a read-only token cannot upload or delete
    let read_only = models_user::create_api_token_with(&app.state.control, &app.user.id, "ro", &["memory:read".to_string()], None, None).await.unwrap().token;
    let (s, _, _) = raw(&app, "DELETE", &format!("/api/documents/{id}"), vec![], None, &read_only).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = raw(&app, "GET", &format!("/api/documents/{id}/download"), vec![], None, &read_only).await;
    assert_eq!(s, StatusCode::OK);
    // the owner still has it, unchanged
    assert_eq!(settled(&app, &id).await["document"]["status"], "ready");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_revision_retires_the_old_chunks() {
    let (app, _stop) = app().await;
    let v1 = app.tool("document_upload", json!({"filename": "plan.md", "text": "Version one mentions the zebra crossing."})).await;
    let id = v1["id"].as_str().unwrap().to_string();
    settled(&app, &id).await;
    let old_path = object_path(&app, &id).await;

    let v2 = app.tool("document_upload", json!({"filename": "plan.md", "text": "Version two mentions the yak pasture instead.", "document_id": id})).await;
    assert_eq!(v2["id"], id, "{v2}");
    assert!(v2.get("code").is_none(), "{v2}");
    assert_eq!(v2["revision"], 2);
    let got = settled(&app, &id).await;
    assert_eq!(got["document"]["status"], "ready");
    assert!(got["chunks"][0]["chunk_id"].as_str().unwrap().ends_with(":2:0"));
    assert_eq!(app.tool("search", json!({"query": "zebra crossing", "mode": "keyword"})).await["results"], json!([]));
    let hits = app.tool("search", json!({"query": "yak pasture", "mode": "keyword"})).await;
    assert_eq!(hits["results"][0]["document"]["revision"], 2, "{hits}");
    assert!(!object_exists(&app, &old_path).await, "the replaced file is removed");
    let (_, _, bytes) = raw(&app, "GET", &format!("/api/documents/{id}/download"), vec![], None, &app.token).await;
    assert_eq!(bytes, b"Version two mentions the yak pasture instead.");

    // re-index: a new revision of the same bytes, the same text found under it, nothing stale left
    let (s, _, body) = raw(&app, "POST", &format!("/api/documents/{id}/reindex"), vec![], None, &app.token).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(json_of(&body)["revision"], 3);
    let got = settled(&app, &id).await;
    assert_eq!(got["document"]["revision"], 3);
    let hits = app.tool("search", json!({"query": "yak pasture", "mode": "keyword"})).await;
    assert_eq!(hits["results"].as_array().unwrap().len(), 1, "{hits}");
    assert_eq!(hits["results"][0]["document"]["revision"], 3);
    let db = app.db().await;
    let n: Option<i64> = db.test_raw().query("SELECT count() FROM cache_record WHERE source = 'documents' GROUP ALL").await.unwrap().take("count").unwrap();
    assert_eq!(n, Some(1), "stale chunks left");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pdfs_long_texts_and_failures() {
    use base64::Engine;
    let (app, _stop) = app().await;
    // a text-based PDF through the tool
    let b64 = base64::engine::general_purpose::STANDARD.encode(PDF);
    let pdf = app.tool("document_upload", json!({"filename": "notes.pdf", "content_base64": b64})).await;
    assert_eq!(pdf["media_type"], "application/pdf", "{pdf}");
    let got = settled(&app, pdf["id"].as_str().unwrap()).await;
    assert_eq!(got["document"]["status"], "ready", "{got}");
    assert!(got["text"].as_str().unwrap().contains("quarterly zebra migration plan"), "{got}");
    let hits = app.tool("search", json!({"query": "zebra migration plan", "mode": "keyword"})).await;
    assert_eq!(hits["results"][0]["document"]["filename"], "notes.pdf", "{hits}");
    let (_, _, bytes) = raw(&app, "GET", &format!("/api/documents/{}/download", pdf["id"].as_str().unwrap()), vec![], None, &app.token).await;
    assert_eq!(bytes, PDF);

    // a long text: bounded chunks that rebuild the text exactly
    let long: String = (0..2000).map(|i| format!("Line {i}: notes about the harbour project.\n")).collect();
    let doc = app.tool("document_upload", json!({"filename": "long.txt", "text": long})).await;
    let got = app.tool("document_get", json!({"id": doc["id"], "max_chars": 1_000_000})).await;
    let got = if got["document"]["status"] == "indexing" { settled(&app, doc["id"].as_str().unwrap()).await } else { got };
    let got = app.tool("document_get", json!({"id": got["document"]["id"], "max_chars": 1_000_000})).await;
    let n = got["document"]["chunk_count"].as_i64().unwrap();
    assert!(n > 10, "{n} chunks");
    assert_eq!(got["text"].as_str().unwrap(), long);
    let chunks = got["chunks"].as_array().unwrap();
    for (i, c) in chunks.iter().enumerate() {
        assert_eq!(c["part"], i);
        assert!(c["char_end"].as_i64().unwrap() - c["char_start"].as_i64().unwrap() <= 6000);
    }
    let short = app.tool("document_get", json!({"id": doc["id"], "max_chars": 100})).await;
    assert_eq!((short["text"].as_str().unwrap().chars().count(), short["text_truncated"].clone()), (100, json!(true)));

    // a PDF that cannot be parsed: failed with a reason, the original still downloadable
    let broken = b"%PDF-1.4\nthis is not really a pdf\n%%EOF\n".to_vec();
    let (s, doc) = upload_http(&app, "broken.pdf", Some("application/pdf"), &broken).await;
    assert_eq!(s, StatusCode::OK, "{doc}");
    let got = settled(&app, doc["id"].as_str().unwrap()).await;
    assert_eq!(got["document"]["status"], "failed", "{got}");
    assert!(got["document"]["error"].as_str().unwrap().contains("PDF"), "{got}");
    let (s, _, bytes) = raw(&app, "GET", &format!("/api/documents/{}/download", doc["id"].as_str().unwrap()), vec![], None, &app.token).await;
    assert_eq!((s, bytes), (StatusCode::OK, broken));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_embeddings_keyword_search_works_and_vectors_are_explicitly_missing() {
    // the default test settings: an OpenAI backend with no key, so nothing is embedded
    let (app, _stop) = app_with(common::test_settings()).await;
    let doc = app.tool("document_upload", json!({"filename": "n.txt", "text": "The lighthouse keeper logs every storm."})).await;
    let id = doc["id"].as_str().unwrap().to_string();
    assert_eq!(settled(&app, &id).await["chunks"][0]["has_embedding"], false);
    let hits = app.tool("search", json!({"query": "lighthouse storm", "mode": "keyword"})).await;
    assert_eq!(hits["results"][0]["document"]["document_id"], id, "{hits}");
    let (_, _, body) = raw(&app, "GET", &format!("/api/documents/{id}/export"), vec![], None, &app.token).await;
    let m = json_of(&body);
    assert_eq!(m["chunks"][0]["embedding_state"], "missing");
    assert_eq!(m["chunks"][0]["embedding"], Value::Null);
    assert_eq!(m["embedding"]["chunks_missing"], 1);
}

#[tokio::test]
async fn storage_off_is_a_clear_error_and_the_rest_works() {
    let app = TestApp::with_settings(Settings { documents_backend: String::new(), ..common::test_settings() }).await;
    let (s, out) = upload_http(&app, "a.txt", Some("text/plain"), b"hello").await;
    assert_eq!((s, out["code"].as_str().unwrap()), (StatusCode::SERVICE_UNAVAILABLE, "document.storage_unavailable"), "{out}");
    assert!(out["detail"].as_str().unwrap().contains("EUNOMIA_DOCUMENTS_BACKEND"));
    assert_eq!(app.tool("document_list", json!({})).await["total"], 0);
    // and the browser session works the same way as tokens
    let (s, body) = app.http_session("GET", "/api/documents", None).await;
    assert_eq!((s, body["total"].clone()), (StatusCode::OK, json!(0)));
}

async fn vault_doc_ids(app: &TestApp, vault: &str) -> Vec<String> {
    let listed = app.tool("document_list", json!({"vault_id": vault})).await;
    listed["results"].as_array().unwrap_or_else(|| panic!("{listed}")).iter().map(|d| d["id"].as_str().unwrap().to_string()).collect()
}

async fn download(app: &TestApp, id: &str, token: &str) -> (StatusCode, Vec<u8>) {
    let (s, _, b) = raw(app, "GET", &format!("/api/documents/{id}/download"), vec![], None, token).await;
    (s, b)
}

fn prefix(vault: &str) -> String {
    format!("/{}/", vault.trim_start_matches("vault:"))
}

async fn ids(db: &eunomia_backend::pool::OrgDb, sql: &str, bind: (&'static str, String)) -> Vec<RecordId> {
    let mut res = db.test_raw().query(sql.to_string()).bind(bind).await.unwrap();
    res.take(0).unwrap()
}

/// Issue #98: each vault's documents in their own storage area, carried by a clone or merge, removed
/// with the vault; a document stored before 2.0.4 (no vault prefix) still downloads and is carried too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn documents_live_in_their_vault_and_travel_with_it() {
    let (app, _stop) = app().await;
    let db = app.db().await;
    let rid = |s: &str| eunomia_backend::rid::parse(s).unwrap();
    let a = app.tool("vault_create", json!({"name": "Alpha", "kind": "org"})).await["id"].as_str().unwrap().to_string();
    let b = app.tool("vault_create", json!({"name": "Beta", "kind": "org"})).await["id"].as_str().unwrap().to_string();
    let vaults = app.tool("vault_list", json!({})).await;
    let personal = vaults["results"].as_array().unwrap_or_else(|| panic!("{vaults}")).iter().find(|v| v["kind"] == "personal").unwrap()["id"].as_str().unwrap().to_string();

    // upload into Alpha (by name, like any vault argument) and Beta (over HTTP): each listed and stored in its own vault
    let text = "Alpha plan: the heron survey starts in May.";
    let up = app.tool("document_upload", json!({"filename": "plan.md", "text": text, "vault_id": "Alpha"})).await;
    let doc_a = up["id"].as_str().unwrap_or_else(|| panic!("{up}")).to_string();
    assert_eq!(settled(&app, &doc_a).await["document"]["status"], "ready");
    let (s, _, body) = raw(&app, "POST", &format!("/api/documents?filename=b.txt&vault_id={b}"), b"beta notes".to_vec(), Some("text/plain"), &app.token).await;
    let doc_b = json_of(&body)["id"].as_str().unwrap_or_else(|| panic!("{s}: {}", String::from_utf8_lossy(&body))).to_string();
    settled(&app, &doc_b).await;
    assert_eq!(vault_doc_ids(&app, &a).await, vec![doc_a.clone()]);
    assert_eq!(vault_doc_ids(&app, &b).await, vec![doc_b.clone()]);
    assert_eq!(app.tool("document_list", json!({})).await["total"], 0, "the personal vault has none");
    let (s, _, listed) = raw(&app, "GET", &format!("/api/documents?vault_id={a}"), vec![], None, &app.token).await;
    assert_eq!((s, json_of(&listed)["total"].clone()), (StatusCode::OK, json!(1)));
    let path_a = object_path(&app, &doc_a).await;
    assert!(path_a.starts_with(&prefix(&a)), "{path_a}");
    assert!(object_path(&app, &doc_b).await.starts_with(&prefix(&b)));
    let mut res = db.test_raw().query("SELECT VALUE id FROM cache_record WHERE payload.document_id = $d AND vault = $v").bind(("d", doc_a.clone())).bind(("v", rid(&a))).await.unwrap();
    let chunk_a: Vec<RecordId> = res.take(0).unwrap();
    assert_eq!(chunk_a.len(), 1, "the chunk is filed in Alpha");

    // a token for Beta, and a colleague in no shared vault, cannot reach Alpha's document
    let scopes: Vec<String> = ["memory:read", "memory:write"].iter().map(|s| s.to_string()).collect();
    let beta_token = models_user::create_api_token_with(&app.state.control, &app.user.id, "b", &scopes, Some(&rid(&b)), None).await.unwrap().token;
    assert_eq!(download(&app, &doc_a, &beta_token).await.0, StatusCode::FORBIDDEN);
    assert_eq!(download(&app, &doc_b, &beta_token).await, (StatusCode::OK, b"beta notes".to_vec()));
    let (_, _, beta_list) = raw(&app, "GET", "/api/documents", vec![], None, &beta_token).await;
    assert_eq!(json_of(&beta_list)["total"], 1, "a restricted token lists its own vault");
    let colleague = common::register(&app.state, "colleague2@example.com").await;
    let colleague_token = models_user::create_api_token(&app.state.control, &colleague.id, "t").await.unwrap().token;
    assert_eq!(download(&app, &doc_a, &colleague_token).await.0, StatusCode::NOT_FOUND);
    let (s, _, _) = raw(&app, "GET", &format!("/api/documents?vault_id={a}"), vec![], None, &colleague_token).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // a document from before 2.0.4: org-level path, no vault prefix
    let legacy = b"stored before vault buckets";
    db.test_raw()
        .query("file::put(type::file('documents', '/legacykey0000000000/1/old.txt'), $b); CREATE document:legacykey0000000000 SET owner = $owner, vault = $vault, filename = 'old.txt', media_type = 'text/plain', size_bytes = $size, sha256 = $sha, object_path = '/legacykey0000000000/1/old.txt', revision = 1, status = 'failed', chunk_count = 0")
        .bind(("b", surrealdb::types::Bytes::from(legacy.to_vec())))
        .bind(("owner", app.user.id.clone()))
        .bind(("vault", rid(&personal)))
        .bind(("size", legacy.len() as i64))
        .bind(("sha", sha(legacy)))
        .await
        .unwrap()
        .check()
        .unwrap();
    assert_eq!(download(&app, "document:legacykey0000000000", &app.token).await, (StatusCode::OK, legacy.to_vec()));

    // a memory drawn from Alpha's chunk
    let wrote = app.tool("memory_write", json!({"subject_name": "Heron Survey", "subject_kind": "organisation", "text": "starts in May", "vault_id": a})).await;
    assert!(wrote.get("error").is_none(), "{wrote}");
    db.test_raw().query("UPDATE memory SET source = $c WHERE vault = $v").bind(("c", chunk_a[0].clone())).bind(("v", rid(&a))).await.unwrap().check().unwrap();

    // a failed clone leaves no document or chunk behind
    let (all_docs, all_chunks, none) = ("SELECT VALUE id FROM document", "SELECT VALUE id FROM cache_record", ("x", String::new()));
    let (docs_before, chunks_before) = (ids(&db, all_docs, none.clone()).await.len(), ids(&db, all_chunks, none.clone()).await.len());
    eunomia_backend::vaults::service::FAIL_COPY_AT.store(1, std::sync::atomic::Ordering::SeqCst);
    let failed = app.tool("vault_clone", json!({"vault_id": a, "name": "Doomed"})).await;
    assert!(failed.get("error").is_some(), "{failed}");
    assert_eq!(ids(&db, all_docs, none.clone()).await.len(), docs_before);
    assert_eq!(ids(&db, all_chunks, none).await.len(), chunks_before);

    // clone: the copy has its own id, its file under the new vault, its chunk with the vector, and the memory's source
    let clone = app.tool("vault_clone", json!({"vault_id": a, "name": "Alpha Copy"})).await;
    let c = clone["id"].as_str().unwrap_or_else(|| panic!("{clone}")).to_string();
    let copied = vault_doc_ids(&app, &c).await;
    assert_eq!(copied.len(), 1);
    assert_ne!(copied[0], doc_a);
    assert_eq!(download(&app, &copied[0], &app.token).await, (StatusCode::OK, text.as_bytes().to_vec()));
    let copy_path = object_path(&app, &copied[0]).await;
    assert!(copy_path.starts_with(&prefix(&c)), "{copy_path}");
    let got = app.tool("document_get", json!({"id": copied[0]})).await;
    assert_eq!((got["document"]["status"].clone(), got["text"].clone()), (json!("ready"), json!(text)), "{got}");
    assert_eq!(got["chunks"][0]["has_embedding"], true);
    let copy_chunk = got["chunks"][0]["chunk_id"].as_str().unwrap().to_string();
    let hits = app.tool("search", json!({"query": "heron survey", "mode": "keyword", "vault_id": c})).await;
    assert!(hits.to_string().contains(&copy_chunk), "{hits}");
    let copy_chunk_rid = ids(&db, "SELECT VALUE id FROM cache_record WHERE payload.document_id = $d", ("d", copied[0].clone())).await;
    let mut res = db.test_raw().query("SELECT VALUE source FROM memory WHERE vault = $v").bind(("v", rid(&c))).await.unwrap();
    let sources: Vec<Option<RecordId>> = res.take(0).unwrap();
    assert_eq!(sources, vec![Some(copy_chunk_rid[0].clone())], "the copied memory points at the copied chunk");
    assert_eq!(vault_doc_ids(&app, &a).await, vec![doc_a.clone()], "the source is unchanged");

    // merge Alpha and the personal vault: both documents, the legacy one now under the new prefix
    let merged = app.tool("vault_merge", json!({"vault_id_a": a, "vault_id_b": personal, "name": "Merged"})).await;
    let m = merged["id"].as_str().unwrap_or_else(|| panic!("{merged}")).to_string();
    let mut bodies: Vec<Vec<u8>> = Vec::new();
    for id in vault_doc_ids(&app, &m).await {
        let (s, body) = download(&app, &id, &app.token).await;
        assert_eq!(s, StatusCode::OK);
        assert!(object_path(&app, &id).await.starts_with(&prefix(&m)));
        bodies.push(body);
    }
    bodies.sort();
    let mut want = vec![text.as_bytes().to_vec(), legacy.to_vec()];
    want.sort();
    assert_eq!(bodies, want);

    // delete the clone: its file and rows go, the source's stay
    let deleted = app.tool("vault_delete", json!({"vault_id": c})).await;
    assert!(deleted.get("error").is_none(), "{deleted}");
    assert!(!object_exists(&app, &copy_path).await);
    assert!(ids(&db, "SELECT VALUE id FROM document WHERE object_path = $p", ("p", copy_path.clone())).await.is_empty());
    assert!(ids(&db, "SELECT VALUE id FROM cache_record WHERE payload.document_id = $d", ("d", copied[0].clone())).await.is_empty());
    assert!(object_exists(&app, &path_a).await);
    assert_eq!(download(&app, &doc_a, &app.token).await.0, StatusCode::OK);
}
