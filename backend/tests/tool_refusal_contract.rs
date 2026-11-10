//! Transport contracts for foreign-vault refusals, documented in docs/errors.md.
mod common;

use axum::{body::Body, http::{Request, StatusCode}};
use common::TestApp;
use eunomia_backend::{models_user, rid, scopes};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

fn keys(value: &Value, expected: &[&str]) {
    let mut actual: Vec<_> = value.as_object().expect("object").keys().map(String::as_str).collect();
    let mut expected = expected.to_vec();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected, "{value}");
}

async fn invoke(app: &TestApp, token: &str, tool: &str, args: Value, mcp: bool) -> (StatusCode, String, Value) {
    let (path, body) = if mcp {
        ("/mcp".to_string(), json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":tool, "arguments":args}}))
    } else {
        (format!("/api/tools/{tool}"), args)
    };
    let req = Request::builder().method("POST").uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string())).unwrap();
    let res = Box::pin(app.router.clone().oneshot(req)).await.unwrap();
    let status = res.status();
    let content_type = res.headers()["content-type"].to_str().unwrap().to_string();
    let trace = res.headers()["x-trace-id"].to_str().unwrap().to_string();
    let body: Value = serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(content_type, if status == StatusCode::OK { "application/json" } else { "application/problem+json" });
    (status, trace, body)
}

#[tokio::test]
async fn foreign_vault_refusals_keep_their_rest_and_mcp_contracts() {
    Box::pin(async {
        let app = TestApp::new().await;
        let bound = app.tool("vault_create", json!({"name":"Bound team", "kind":"org"})).await["id"].as_str().unwrap().to_string();
        let foreign = app.tool("memory_write", json!({"subject_name":"Foreign", "subject_kind":"person", "text":"Private fact"})).await;
        let vault = foreign["entity"]["vault"].as_str().unwrap();
        let entity = foreign["entity"]["id"].as_str().unwrap();
        let granted = scopes::ALL.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let token = models_user::create_api_token_with(app.control(), &app.user.id, "contract", &granted, Some(&rid::parse(&bound).unwrap()), None).await.unwrap().token;
        let scope_message = format!("this token is restricted to vault {bound}");
        let cases = [
            ("skill_save", json!({"name":"foreign", "description":"d", "body":"b", "vault_id":vault}), "auth.scope", scope_message.clone(), false),
            ("memory_write", json!({"subject_name":"Foreign", "subject_kind":"person", "text":"Refused", "vault_id":vault}), "auth.scope", scope_message.clone(), false),
            ("vault_invite", json!({"email":"other@example.test", "vault_id":vault}), "auth.scope", format!("vault_invite: {scope_message}"), false),
            ("entities_neighbourhood", json!({"entity_id":entity, "vault_id":vault}), "auth.scope", scope_message.clone(), false),
            ("search", json!({"query":"private", "vault_id":vault}), "auth.scope", scope_message.clone(), true),
            ("recall", json!({"query":"private", "vault_id":vault}), "auth.scope", scope_message.clone(), true),
            ("reflect", json!({"query":"private", "vault_id":vault}), "auth.scope", scope_message.clone(), true),
            ("secret_request", json!({"name":"contract", "description":"d", "vault_id":vault}), "auth.scope", scope_message.clone(), false),
            ("memory_write_batch", json!({"vault_id":vault, "items":[
                {"subject_name":"Foreign", "subject_kind":"person", "text":"Refused"},
                {"subject_name":"Allowed", "subject_kind":"person", "text":"Allowed override", "vault_id":bound}
            ]}), "auth.scope", scope_message, false),
        ];
        for mcp in [false, true] {
            for (tool, args, code, message, http_error) in &cases {
                let (status, trace, body) = invoke(&app, &token, tool, args.clone(), mcp).await;
                assert_eq!(status, if *http_error && !mcp { StatusCode::FORBIDDEN } else { StatusCode::OK }, "{tool}, mcp={mcp}: {body}");
                let value = if mcp {
                    keys(&body, &["jsonrpc", "id", "result"]);
                    assert_eq!(body["jsonrpc"], "2.0");
                    assert_eq!(body["id"], 1);
                    keys(&body["result"], &["content", "isError"]);
                    assert_eq!(body["result"]["isError"], *tool != "memory_write_batch", "{tool}: {body}");
                    assert_eq!(body["result"]["content"].as_array().unwrap().len(), 1);
                    keys(&body["result"]["content"][0], &["type", "text"]);
                    assert_eq!(body["result"]["content"][0]["type"], "text");
                    serde_json::from_str(body["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
                } else { body };
                if *tool == "memory_write_batch" {
                    keys(&value, &["written", "failed", "results"]);
                    assert_eq!(value["written"], 1);
                    assert_eq!(value["failed"], 1);
                    let refused = &value["results"][0];
                    keys(refused, &["index", "ok", "error", "code"]);
                    assert_eq!(refused, &json!({"index":0, "ok":false, "error":message, "code":code}));
                    assert_eq!(value["results"][1]["ok"], true);
                } else {
                    if *http_error && !mcp {
                        keys(&value, &["type", "title", "status", "detail", "code", "trace_id"]);
                        assert_eq!(value["type"], "about:blank");
                        assert_eq!(value["title"], "Forbidden");
                        assert_eq!(value["status"], 403);
                        assert_eq!(value["detail"], *message);
                    } else {
                        keys(&value, &["error", "code", "trace_id"]);
                        assert_eq!(value["error"], *message, "{tool}, mcp={mcp}");
                    }
                    assert_eq!(value["code"], *code, "{tool}, mcp={mcp}");
                    assert_eq!(value["trace_id"], trace);
                    assert_eq!(trace.len(), 32);
                    assert!(trace.bytes().all(|b| b.is_ascii_hexdigit()));
                }
                eprintln!("CONTRACT {tool} mcp={mcp}: status={status}, code={code}");
            }
        }
    }).await;
}
