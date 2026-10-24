//! The OpenAPI response schemas must describe what the routes really return.
//! Each case calls the real route through the harness and validates the JSON
//! body against that operation's 200 schema in the generated spec.

mod common;

use axum::http::StatusCode;
use common::TestApp;
use eunomia_backend::openapi;
use serde_json::{json, Value};

struct Checker {
    spec: Value,
    app: TestApp,
    covered: Vec<String>,
}

impl Checker {
    async fn new() -> Self {
        Checker { spec: serde_json::to_value(openapi::spec()).unwrap(), app: TestApp::new().await, covered: vec![] }
    }

    /// (path template, method) of an operation id.
    fn locate(&self, op: &str) -> (String, String) {
        for (path, item) in self.spec["paths"].as_object().unwrap() {
            for (method, o) in item.as_object().unwrap() {
                if o["operationId"] == op {
                    return (path.clone(), method.clone());
                }
            }
        }
        panic!("no operation {op}");
    }

    /// Call `op` at the concrete `path` and validate the 200 body. Returns the body.
    async fn call(&mut self, op: &str, path: &str, body: Option<Value>) -> Value {
        let (template, method) = self.locate(op);
        let want: Vec<&str> = template.split('/').collect();
        let path_only = path.split('?').next().unwrap();
        let got: Vec<&str> = path_only.split('/').collect();
        assert_eq!(want.len(), got.len(), "{op}: {path} does not match {template}");
        for (w, g) in want.iter().zip(&got) {
            assert!(w.starts_with('{') || w == g, "{op}: {path} does not match {template}");
        }

        let (status, json) = self.app.http(&method.to_uppercase(), path, body, true).await;
        assert_eq!(status, StatusCode::OK, "{op}: {json}");

        let schema = &self.spec["paths"][&template][&method]["responses"]["200"]["content"]["application/json"]["schema"];
        assert!(!schema.is_null(), "{op} has no JSON 200 schema");
        // Same document root as the spec, so `#/components/schemas/..` refs resolve.
        let mut root = schema.clone();
        root["components"] = self.spec["components"].clone();
        let validator = jsonschema::validator_for(&root).expect("schema compiles");
        let errors: Vec<String> = validator.iter_errors(&json).map(|e| format!("{} at {}", e, e.instance_path())).collect();
        assert!(errors.is_empty(), "{op} response does not match its schema:\n{}\nbody: {json}", errors.join("\n"));
        self.covered.push(op.to_string());
        json
    }
}

#[tokio::test]
async fn vault_responses_match_their_schemas() {
    let mut c = Checker::new().await;
    c.call("listVaults", "/api/vaults", None).await;
    let created = c.call("createVault", "/api/vaults", Some(json!({"name": "Team"}))).await;
    let id = created["id"].as_str().unwrap().to_string();
    c.call("renameVault", &format!("/api/vaults/{id}"), Some(json!({"name": "Crew"}))).await;
    c.call("listMembers", &format!("/api/vaults/{id}/members"), None).await;
    c.call("listInvitations", "/api/vaults/invitations", None).await;
    c.call("cloneVault", &format!("/api/vaults/{id}/clone"), Some(json!({"name": "Copy", "kind": "org"}))).await;
    let other = c.call("createVault", "/api/vaults", Some(json!({"name": "Other"}))).await;
    c.call("mergeVaults", "/api/vaults/merge", Some(json!({"vault_ids": [id, other["id"]], "name": "Both"}))).await;
}

#[tokio::test]
async fn entity_responses_match_their_schemas() {
    let mut c = Checker::new().await;
    let ada = c.call("createEntity", "/api/entities", Some(json!({"kind": "person", "name": "Ada", "aliases": ["A"]}))).await;
    let bob = c.call("createEntity", "/api/entities", Some(json!({"kind": "person", "name": "Bob"}))).await;
    let ada_id = ada["id"].as_str().unwrap().to_string();
    let bob_id = bob["id"].as_str().unwrap().to_string();
    let mem = c.call("addMemory", &format!("/api/entities/{ada_id}/memory"), Some(json!({"text": "likes tea"}))).await;
    c.call(
        "updateMemory",
        &format!("/api/entities/memory/{}", mem["id"].as_str().unwrap()),
        Some(json!({"text": "likes green tea", "type": "experience"})),
    )
    .await;
    c.call("addRelation", &format!("/api/entities/{ada_id}/relations"), Some(json!({"to_id": bob_id, "label": "knows"}))).await;
    c.call("listEntities", "/api/entities", None).await;
    c.call("getEntity", &format!("/api/entities/{ada_id}"), None).await;
    c.call("updateEntity", &format!("/api/entities/{ada_id}"), Some(json!({"summary": "a mathematician"}))).await;
    c.call("getEntityGraph", "/api/entities/graph", None).await;
    c.call("getVectorCloud", "/api/entities/cloud", None).await;
    c.call("getCloudSimilarity", "/api/entities/cloud/similarity", Some(json!({"ids": []}))).await;
    let dup = c.call("createEntity", "/api/entities", Some(json!({"kind": "person", "name": "Robert"}))).await;
    c.call("mergeEntities", &format!("/api/entities/{bob_id}/merge"), Some(json!({"loser_id": dup["id"]}))).await;
}

#[tokio::test]
async fn settings_audit_update_and_tool_responses_match_their_schemas() {
    let mut c = Checker::new().await;
    c.call("getSettings", "/api/settings", None).await;
    c.call("updateSettings", "/api/settings", Some(json!({"embedding_model": "text-embedding-ada-002", "theme": {"mode": "dark"}}))).await;
    c.call("listAudit", "/api/audit?limit=5", None).await;
    c.call("getUpdateStatus", "/api/update/status", None).await;
    c.call("requestUpdate", "/api/update/request", Some(json!({}))).await;
    c.call("checkForUpdate", "/api/update/check", Some(json!({}))).await;
    c.call("listTools", "/api/tools", None).await;
    c.call("invokeTool", "/api/tools/docs", Some(json!({}))).await;
}

#[tokio::test]
async fn connector_and_source_responses_match_their_schemas() {
    let mut c = Checker::new().await;
    c.call("listConnectors", "/api/connectors", None).await;
    c.call("getConnector", "/api/connectors/github", None).await;
    c.call("updateConnector", "/api/connectors/github", Some(json!({"enabled": true, "config": {"org": "x"}}))).await;
    c.call("getSnapshot", "/api/snapshot", None).await;
    c.call("listSources", "/api/sources", None).await;
    c.call("getSourcesStatus", "/api/sources/status", None).await;
}

/// Guards the checker itself: a body that disagrees with the schema must fail.
#[tokio::test]
async fn the_validator_rejects_a_wrong_body() {
    let spec = serde_json::to_value(openapi::spec()).unwrap();
    let mut root = json!({"$ref": "#/components/schemas/VaultWithRole"});
    root["components"] = spec["components"].clone();
    let validator = jsonschema::validator_for(&root).unwrap();
    assert!(!validator.is_valid(&json!({"id": "vault:x", "name": 7})));
}

/// Operation ids are unique camelCase names.
#[tokio::test]
async fn operation_ids_are_unique_and_camel_case() {
    let spec = serde_json::to_value(openapi::spec()).unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for item in spec["paths"].as_object().unwrap().values() {
        for op in item.as_object().unwrap().values() {
            let id = op["operationId"].as_str().expect("operationId");
            assert!(id.chars().next().unwrap().is_ascii_lowercase() && !id.contains('_'), "{id} is not camelCase");
            assert!(seen.insert(id.to_string()), "duplicate operationId {id}");
        }
    }
}
