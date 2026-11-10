//! Run the installer and its generated hooks through the real permission gate.
mod common;

use axum::{http::HeaderValue, middleware};
use eunomia_backend::models_user;
use serde_json::{Value, json};
use std::path::Path;
use tokio::process::Command;

fn bash() -> &'static str {
    if Path::new("/opt/homebrew/bin/bash").exists() { "/opt/homebrew/bin/bash" } else { "bash" }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installer_and_hooks_work_with_name_only_tokens_through_the_real_gate() {
    let state = common::bare_state().await;
    let fixture = uuid::Uuid::new_v4().to_string();
    let marker = HeaderValue::from_str(&fixture).unwrap();
    let router = eunomia_backend::app(state.clone()).layer(middleware::map_response(move |mut r: axum::response::Response| {
        let marker = marker.clone();
        async move { r.headers_mut().insert("x-installer-fixture", marker); r }
    }));
    let socket = (49152..=65535).find_map(|port| std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).ok())
        .expect("a free loopback fixture port");
    socket.set_nonblocking(true).unwrap();
    let listener = tokio::net::TcpListener::from_std(socket).unwrap();
    let addr = listener.local_addr().unwrap();
    assert!(addr.ip().is_loopback() && addr.port() >= 49152, "fixture must own a loopback ephemeral port");
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let base = format!("http://{addr}");
    // Prove the destination before the installer makes its first write.
    let health = reqwest::get(format!("{base}/healthz")).await.unwrap();
    assert!(health.status().is_success());
    assert_eq!(health.headers()["x-installer-fixture"], fixture);
    let home = tempfile::tempdir().unwrap();
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/connect-agents.sh");
    let result = Command::new(bash()).arg(script).args(["--url", &base, "--email", "installer@example.test", "--agents", "claude,codex,hermes"])
        .env_clear().env("PATH", std::env::var("PATH").unwrap()).env("HOME", home.path())
        .env("EUNOMIA_PASSWORD", common::PASSWORD).output().await.unwrap();
    assert!(result.status.success(), "installer failed");
    for agent in [".claude", ".codex", ".hermes"] {
        let skill = std::fs::read_to_string(home.path().join(agent).join("skills/eunomia-memory/SKILL.md")).expect("skill installed");
        assert!(skill.contains("name: eunomia-memory"));
        assert!(skill.contains(eunomia_backend::docs::skill_body(eunomia_backend::docs::DEFAULT_SKILL)));
    }
    let config: Value = serde_json::from_slice(&std::fs::read(home.path().join(".claude.json")).unwrap()).unwrap();
    let bearer = config["mcpServers"]["eunomia"]["headers"]["Authorization"].as_str().unwrap();
    let token = bearer.strip_prefix("Bearer ").unwrap();
    let models_user::TokenCheck::Valid(stored) = models_user::check_api_token(&state.control, token).await.unwrap() else { panic!("installer token must be valid") };
    assert_eq!(stored.scopes, vec!["memory:read", "memory:write"]);
    assert!(stored.vault.is_some());
    assert!(stored.expires_at.is_some());
    let client = reqwest::Client::new();
    assert_eq!(client.get(format!("{base}/api/settings")).header("Authorization", bearer).send().await.unwrap().status(), 403);
    let write = client.post(format!("{base}/api/tools/memory_write")).header("Authorization", bearer)
        .json(&json!({"subject_name": "Ada", "subject_kind": "person", "text": "Ada prefers async updates"})).send().await.unwrap();
    assert!(write.status().is_success());
    let hook = home.path().join(".config/eunomia/hook.sh");
    let session = Command::new(bash()).arg(&hook).arg("session").env_clear()
        .env("PATH", std::env::var("PATH").unwrap()).env("HOME", home.path()).output().await.unwrap();
    assert!(session.status.success());
    assert!(String::from_utf8(session.stdout).unwrap().contains("Eunomia"), "session injects skill");
    let mut prompt = Command::new(bash()).arg(&hook).arg("prompt").env_clear()
        .env("PATH", std::env::var("PATH").unwrap()).env("HOME", home.path())
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
    use tokio::io::AsyncWriteExt;
    prompt.stdin.take().unwrap().write_all(br#"{"prompt":"Ada"}"#).await.unwrap();
    let recalled = prompt.wait_with_output().await.unwrap();
    assert!(recalled.status.success());
    assert!(String::from_utf8(recalled.stdout).unwrap().contains("Ada prefers async updates"));
    server.abort();
}

#[test]
fn internal_token_issuance_requires_explicit_permissions_destination_and_expiry() {
    let source = include_str!("../src/models_user.rs");
    assert!(!source.contains("pub async fn create_api_token("), "remove the broad name-only convenience issuer");
    let replay = include_str!("../src/replay.rs");
    assert!(replay.contains("create_api_token_with("));
}

#[tokio::test]
async fn upgrade_fixture_explicitly_requests_only_its_required_permissions() {
    let app = common::TestApp::new().await;
    let script = include_str!("../../scripts/tests/update-path.test.sh");
    let mint = script.split("TOKEN_A=\"").nth(1).unwrap().lines().next().unwrap();
    let body = mint.split("-d '").nth(1).unwrap().split('\'').next().unwrap();
    let (status, issued) = app.http_session("POST", "/api/auth/tokens", Some(serde_json::from_str(body).unwrap())).await;
    assert_eq!(status, 200);
    assert_eq!(issued["scopes"], json!(["memory:read", "memory:write", "vaults:admin"]));
    assert!(issued["vault_id"].is_null());
    let token = issued["token"].as_str().unwrap();
    let ((status, vault), _) = common::http(&app.router, "POST", "/api/tools/vault_create", Some(json!({"name":"Upgrade team"})), Some(token), None).await;
    assert_eq!(status, 200);
    let ((status, _), _) = common::http(&app.router, "POST", "/api/tools/memory_write", Some(json!({
        "vault_id":vault["id"], "subject_name":"Upgrade fixture", "subject_kind":"person", "text":"Team memory"
    })), Some(token), None).await;
    assert_eq!(status, 200);
}
