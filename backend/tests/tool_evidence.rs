//! Every registered tool must declare its echo contract. Canary arguments are never stored data.
mod common;
use common::TestApp;
use eunomia_backend::tools::registry::{self, evidence};
use serde_json::{json, Value};

fn has_canary(value: &Value) -> bool {
    // The marker must also be caught when a server sentence uses comma or scaled spelling.
    value.to_string().replace([',', '.'], "").contains("999777")
}

fn canary(schema: &Value) -> Value {
    if let Some(value) = schema["enum"].as_array().and_then(|a| a.first()) { return value.clone(); }
    match schema["type"].as_str() {
        Some("object") => Value::Object(schema["properties"].as_object().into_iter().flat_map(|p| p.iter()).filter(|(key, _)| {
            schema["required"].as_array().is_some_and(|a| a.iter().any(|v| v == key.as_str()))
        }).map(|(key, schema)| (key.clone(), canary(schema))).collect()),
        Some("array") => json!([canary(&schema["items"])]),
        Some("number" | "integer") => json!(999777),
        Some("boolean") => json!(false),
        _ => json!("sample999777"),
    }
}

#[tokio::test]
async fn every_registered_tool_keeps_argument_canaries_in_declared_echo_paths() {
    assert_eq!(common::test_settings().surreal_url, "mem://");
    let app = TestApp::new().await;
    let state = app.state.org(&app.user.org).await.unwrap();
    for (name, spec) in registry::all_tools() {
        let policy = evidence::for_tool(name).unwrap_or_else(|| panic!("{name}: declare its result echo paths"));
        // Wrong-shaped values exercise argument errors, including server sentences quoting input,
        // without creating records, calling external services or letting a canary match real data.
        let args = Value::Object(spec.schema["properties"].as_object().unwrap().keys().map(|key| {
            (key.clone(), json!({"sample999777":"request_canary999777"}))
        }).collect());
        let out = common::sys((spec.handler)(&state, &app.user.id, args)).await.unwrap_or_else(|e| e.to_tool_value());
        // Also exercise correctly typed read requests on a database containing no canary data.
        // Override each optional field in turn so queries, kinds, cursors and filters are covered.
        if spec.read_only {
            let base = canary(&spec.schema);
            let mut requests = vec![base.clone()];
            for (key, schema) in spec.schema["properties"].as_object().unwrap() {
                let mut request = base.clone();
                request[key] = canary(schema);
                requests.push(request);
            }
            for args in requests {
                let result = common::sys((spec.handler)(&state, &app.user.id, args)).await.unwrap_or_else(|e| e.to_tool_value());
                assert!(!has_canary(&policy.computed(&result)), "{name}: argument outside declared echo paths: {result}");
            }
        }
        let computed = policy.computed(&out);
        assert!(!has_canary(&computed), "{name}: argument outside declared echo paths: {out}");
        let mut future_echo = out.as_object().cloned().unwrap_or_default();
        future_echo.insert("_request".into(), json!({"query":"sample999777","top_n":999777}));
        assert!(!has_canary(&policy.computed(&Value::Object(future_echo))), "{name}: reserved echo key");
    }
}

#[test]
fn echo_paths_skip_nested_requests_but_keep_independent_computed_values() {
    let policy = evidence::for_tool("show_view").unwrap();
    let out = json!({"text":"No results for sample999777", "data":{"sources":{"items":{
        "parameters":{"top_n":5,"filters":[{"value":1000}]},
        "pagination":{"reach":{"kinds":["sample999777"]},"total":5},
        "metric":{"value":1000,"unit":"m²"},"rows":[{"name":"Store 3","value":1000}]
    }}}});
    let computed = policy.computed(&out);
    assert!(!has_canary(&computed));
    assert_eq!(computed["data"]["sources"]["items"]["metric"]["value"], 1000);
    assert_eq!(computed["data"]["sources"]["items"]["metric"]["unit"], "m²");
    assert_eq!(computed["data"]["sources"]["items"]["pagination"]["total"], 5);
    assert_eq!(out["text"], "No results for sample999777", "API result bytes stay unchanged");
}

#[test]
fn echo_field_names_are_allowed_in_computed_rows_and_stored_payloads() {
    let row = json!({"top_n":1000,"filters":1000,"query":1000,"reach":1000,"warnings":1000});
    let result = json!({"parameters":{"top_n":2},"rows":[row]});
    let shown = json!({"data":{"sources":{"sample":result}}});
    let computed = evidence::for_tool("show_view").unwrap().computed(&shown);
    assert_eq!(computed["data"]["sources"]["sample"]["rows"][0], row);
    assert!(computed["data"]["sources"]["sample"].get("parameters").is_none());
    let stored = json!({"payload":row,"query":"sample999777"});
    let computed = evidence::for_tool("get").unwrap().computed(&stored);
    assert_eq!(computed["payload"], row);
    assert!(computed.get("query").is_none());
}

// These need a second instance, a filled secret, or a model. The malformed-argument walk above
// still covers their failure shapes. Keep exclusions named so adding a tool cannot silently skip it.
const SUCCESS_EXCLUSIONS: &[(&str, &str)] = &[
    ("vault_join_shared", "requires a second listening Eunomia instance and a signed share code"),
    ("vault_peer_sync", "requires an established peer link to a second instance"),
    ("secret_request", "requires an interactive tool invocation context and user consent"),
    ("secret_request_status", "requires an interactive secret request"),
    ("secret_transform", "requires user-filled held secrets and invocation grants"),
    ("http_request", "requires an outbound grant and a permitted external host"),
];

fn success(value: Value) -> Value {
    assert!(value.get("error").is_none(), "success fixture failed: {value}");
    value
}

async fn receipt(app: &TestApp, name: &str, mut args: Value, pointer: &str, expected: Value) -> Value {
    // ID-only and no-argument tools have no free-form marker slot. Also send the reserved
    // extension on those calls, while the malformed walk probes every actual schema field.
    if !matches!(name, "metric_eval" | "scenario_run" | "show_view") {
        args["_request"] = json!({"marker":"sample999777","limit":999777});
    }
    let out = app.tool(name, args).await;
    assert!(out.get("error").is_none(), "{name}: success fixture failed: {out}");
    let computed = evidence::for_tool(name).unwrap().computed(&out);
    assert!(!has_canary(&computed), "{name}: success echoed marker: {out}");
    if expected == "<echo-only>" {
        // These membership receipts contain only the accepted input. Successful execution is
        // checked above, and an empty projection is the explicit contract, not a blanket skip.
        assert_eq!(computed, json!({}), "{name}: echo-only receipt supplied evidence");
    } else if expected == "<server-id>" {
        assert!(computed.pointer(pointer).and_then(Value::as_str).is_some_and(|s| s.contains(':')), "{name}: missing generated id: {computed}");
    } else if expected == "<server-time>" {
        assert!(computed.pointer(pointer).and_then(Value::as_str).is_some_and(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok()), "{name}: missing generated time: {computed}");
    } else {
        assert_eq!(computed.pointer(pointer), Some(&expected), "{name}: computed receipt missing: {computed}");
    }
    out
}

#[tokio::test]
async fn seeded_success_contract_covers_every_registered_tool() {
    use std::collections::HashSet;
    let app = TestApp::new().await;
    let mut covered = HashSet::new();
    let first = success(app.tool("memory_write", json!({"subject_name":"Fixture","subject_kind":"person","text":"Fixture has 42 points"})).await);
    let second = success(app.tool("memory_write", json!({"subject_name":"Other","subject_kind":"person","text":"Other has 17 points"})).await);
    let entity = first["entity"]["id"].clone();
    let other = second["entity"]["id"].clone();
    let memory = first["memory"]["id"].clone();
    success(app.tool("entity_relate", json!({"from_id":entity,"to_id":other,"label":"knows"})).await);
    let vault = success(app.tool("ontology_get", json!({})).await)["vault"].clone();
    success(app.tool("ontology_propose", json!({
        "properties":[{"kind":"person","name":"score","datatype":"number"}],
        "metrics":[{"kind":"person","name":"total_score","expr":{"agg":"sum","property":"score"}}]
    })).await);
    let vault_id = vault.as_str().unwrap();
    let (status, out) = app.http_session("POST", &format!("/api/vaults/{vault_id}/ontology/kinds/decide"), Some(json!({"kind":"person","decision":{"action":"approve"}}))).await;
    assert_eq!(status, 200, "{out}");
    for path in ["properties", "metrics"] {
        let name = if path == "properties" { "score" } else { "total_score" };
        let mut body = json!({"kind":"person","decision":{"action":"approve"}});
        body["name"] = json!(name);
        let (status, out) = app.http_session("POST", &format!("/api/vaults/{vault_id}/ontology/{path}/decide"), Some(body)).await;
        assert_eq!(status, 200, "{out}");
    }
    success(app.tool("entity_set_property", json!({"entity_id":entity,"property":"score","value":42})).await);
    let doc = success(app.tool("document_upload", json!({"filename":"fixture.csv","text":"name,score,unused\nThird,13,17\nFourth,29,19\n"})).await);
    let document = doc["id"].clone();
    let skill = success(app.tool("skill_save", json!({"name":"fixture","description":"Fixture skill","body":"Use 42 points"})).await);
    let skill_id = skill["id"].clone();
    let space = success(app.tool("vault_create", json!({"name":"Fixture space","kind":"org"})).await)["id"].clone();
    let spare = success(app.tool("vault_create", json!({"name":"Spare space","kind":"org"})).await)["id"].clone();
    let env = eunomia_backend::cache::search::Envelope {
        id:"fixture:item:1".into(), source:"fixture".into(), type_:"fixture.item".into(),
        external_id:"1".into(), title:"Stored fixture".into(), body_text:"A stored count of 42".into(),
        payload:json!({"count":42}), ..Default::default()
    };
    common::sys(eunomia_backend::cache::search::upsert(&app.db().await, &app.user.id, &env)).await.unwrap();
    // Each expected leaf is an actual success result, never a synthetic field added to the output.
    let cases = vec![
        ("docs", json!({}), "/docs/0/topic", json!("quickstart")),
        ("get", json!({"id":"fixture:item:1"}), "/payload/count", json!(42)),
        ("links", json!({"id":"fixture:item:1"}), "/links", json!([])),
        ("list", json!({"limit":999777}), "/total", json!(1)),
        ("search", json!({"query":"Fixture","mode":"keyword","sources":["fixture","sample999777"]}), "/results/0/title", json!("Stored fixture")),
        ("recall", json!({"query":"Fixture sample999777"}), "/results/0/text", json!("Fixture has 42 points")),
        ("reflect", json!({"query":"Fixture sample999777"}), "/mode", json!("recall_only")),
        ("entities_search", json!({"query":"","limit":999777}), "/total", json!(2)),
        ("entities_get", json!({"id":entity}), "/properties/0/value", json!(42.0)),
        ("entities_graph", json!({"kinds":["person","sample999777"]}), "/nodes/0/kind", json!("person")),
        ("entities_neighbourhood", json!({"entity_id":entity}), "/nodes/0/kind", json!("person")),
        ("entity_types", json!({}), "/built_in/0", json!("person")),
        ("ontology_get", json!({}), "/kinds/0/entities", json!(2)),
        ("ontology_readiness", json!({}), "/vault", vault.clone()),
        ("ontology_coverage", json!({"question":"sample999777 total score"}), "/vault", vault.clone()),
        ("metric_eval", json!({"metric":"total_score","filters":[{"property":"score","op":"lt","value":999777}]}), "/value", json!(42.0)),
        ("scenario_run", json!({"changes":[{"set":{"entity_id":entity,"property":"score","value":50}},{"scale":{"kind":"person","property":"score","factor":1,"filters":[{"property":"score","op":"lt","value":999777}]}}],"metrics":["total_score"]}), "/baseline/0/value", json!(42.0)),
        ("show_view", json!({"view":"impact","start_id":entity,"headline":"Fixture impact","reach":{"depth":1,"kinds":["person","sample999777"]}}), "/pagination/total", json!(1)),
        ("show_view", json!({"view":"ui","spec":{"root":"a","elements":{"a":{"type":"Stat","props":{"title":"Score","summary":"Fixture score","data":{"$state":"/sources/m/metric/value"}}}}},"sources":{"m":{"type":"metric","metric":"total_score","filters":[{"property":"score","op":"lt","value":999777}]}}}), "/data/sources/m/metric/value", json!(42.0)),
        ("document_list", json!({"limit":999777}), "/total", json!(1)),
        ("document_get", json!({"id":document,"max_chars":999777}), "/document/size_bytes", doc["size_bytes"].clone()),
        ("document_download", json!({"id":document}), "/size_bytes", doc["size_bytes"].clone()),
        ("document_export", json!({"id":document}), "/document/size_bytes", doc["size_bytes"].clone()),
        ("skill_find", json!({"query":"Fixture","limit":999777}), "/skills/0/name", json!("fixture")),
        ("skill_list", json!({"limit":999777}), "/skills/0/name", json!("fixture")),
        ("skill_get", json!({"id":skill_id}), "/body", json!("Use 42 points")),
        ("vault_list", json!({}), "/results/0/role", json!("owner")),
        ("vault_members", json!({"vault_id":space}), "/results/0/role", json!("owner")),
        ("vault_get", json!({"vault_id":space}), "/member_count", json!(1)),
        ("vault_check", json!({"vault_id":space}), "/ok", json!(true)),
        ("vault_repair", json!({"vault_id":space}), "/ok", json!(true)),
        ("vault_peers", json!({"vault_id":space}), "/links", json!([])),
        ("changes_since", json!({"limit":999777}), "/changes/0/seq", json!(1)),
        ("ontology_suggest", json!({}), "/profile/kinds/0/entities", json!(2)),
        ("ontology_automap", json!({"document_id":document,"record_type":"sample999777"}), "/profiles/0/records", json!(2)),
        ("document_upload", json!({"filename":"sample999777.txt","text":"Some text"}), "/size_bytes", json!(9)),
        ("document_import_rows", json!({"document_id":document,"kind":"person","name_column":"name","column_map":{"score":"score","unused":"sample999777"}}), "/created", json!(2)),
        ("ontology_propose", json!({"kinds":[{"kind":"sample999777"}]}), "/kinds/0/revision", json!(1)),
        ("memory_write", json!({"subject_name":"sample999777","subject_kind":"person","text":"sample999777"}), "/memory/version", json!(1)),
        ("memory_write_batch", json!({"items":[{"subject_name":"sample999777","subject_kind":"person","text":"sample999777"}]}), "/written", json!(1)),
        ("memory_update", json!({"memory_id":memory,"text":"sample999777"}), "/version", json!(2)),
        ("code_entity_upsert", json!({"kind":"repository","name":"Fixture repository (999777)","summary":"sample999777"}), "/id", json!("<server-id>")),
        ("code_relate", json!({"from_id":entity,"to_id":other,"label":"sample999777"}), "/direction", Value::Null),
        ("entity_relate", json!({"from_id":entity,"to_id":other,"label":"sample999777","reason":"sample999777"}), "/direction", Value::Null),
        ("entity_merge", json!({"winner_id":entity,"loser_id":other}), "/kind", json!("person")),
        ("entity_update", json!({"entity_id":entity,"name":"updated999777","summary":"sample999777","aliases":["sample999777"]}), "/kind", json!("person")),
        ("entity_set_property", json!({"entity_id":entity,"property":"score","value":999777}), "/value/datatype", json!("number")),
        ("consolidate_observations", json!({"subject_id":entity}), "/consolidated", json!([])),
        ("skill_save", json!({"name":"sample999777","description":"sample999777","body":"sample999777"}), "/embedded", json!(false)),
        ("skill_delete", json!({"id":skill_id}), "/deleted", skill_id.clone()),
        ("document_delete", json!({"id":document}), "/deleted", json!(true)),
        ("memory_delete", json!({"memory_id":memory}), "/deleted", json!(true)),
        ("entity_delete", json!({"entity_id":entity}), "/deleted", json!(true)),
        ("vault_create", json!({"name":"sample999777","kind":"org"}), "/created_at", json!("<server-time>")),
        ("vault_clone", json!({"vault_id":spare,"name":"clone999777","kind":"org"}), "/entities_copied", json!(0)),
        ("vault_merge", json!({"vault_id_a":space,"vault_id_b":spare,"name":"merged999777","kind":"org"}), "/entities", json!(0)),
        ("vault_rename", json!({"vault_id":spare,"name":"renamed999777"}), "/created_at", json!("<server-time>")),
        ("vault_share_code", json!({"vault_id":space}), "/expires_at", json!("<server-time>")),
        ("vault_delete", json!({"vault_id":spare}), "/deleted", json!(true)),
    ];
    for (name, args, pointer, expected) in cases {
        if name == "code_entity_upsert" {
            success(app.tool(name, json!({"kind":"repository","name":"Fixture repository"})).await);
        }
        receipt(&app, name, args, pointer, expected).await;
        covered.insert(name);
    }
    // Membership tools require an actual invited user. They stay local to this fresh control DB.
    let guest = common::register(&app.state, "guest@example.com").await;
    receipt(&app, "vault_invite", json!({"vault_id":space,"email":guest.email,"role":"member"}), "/role", json!("<echo-only>")).await;
    covered.insert("vault_invite");
    let guest_app = TestApp { user:guest.clone(), state:app.state.clone(), token:app.token.clone(), router:app.router.clone() };
    receipt(&guest_app, "vault_invitations", json!({}), "/results/0/role", json!("member")).await;
    covered.insert("vault_invitations");
    receipt(&guest_app, "vault_accept_invitation", json!({"vault_id":space}), "/role", json!("member")).await;
    covered.insert("vault_accept_invitation");
    receipt(&app, "vault_set_role", json!({"vault_id":space,"email":guest.email,"role":"member"}), "/role", json!("<echo-only>")).await;
    covered.insert("vault_set_role");
    receipt(&app, "vault_transfer_ownership", json!({"vault_id":space,"email":guest.email}), "/role", json!("owner")).await;
    covered.insert("vault_transfer_ownership");
    receipt(&app, "vault_leave", json!({"vault_id":space}), "/left", json!(true)).await;
    covered.insert("vault_leave");
    success(guest_app.tool("vault_invite", json!({"vault_id":space,"email":app.user.email,"role":"member"})).await);
    receipt(&app, "vault_decline_invitation", json!({"vault_id":space}), "/declined", json!(true)).await;
    covered.insert("vault_decline_invitation");
    success(guest_app.tool("vault_invite", json!({"vault_id":space,"email":app.user.email,"role":"member"})).await);
    receipt(&guest_app, "vault_remove_member", json!({"vault_id":space,"email":app.user.email}), "/removed", json!(true)).await;
    covered.insert("vault_remove_member");
    eprintln!("seeded successes: {}; explicit exclusions: {}", covered.len(), SUCCESS_EXCLUSIONS.len());
    for &name in registry::all_tools().keys() {
        let exclusion = SUCCESS_EXCLUSIONS.iter().find(|(excluded, _)| *excluded == name);
        assert!(covered.contains(name) || exclusion.is_some(), "{name}: add a real success fixture or an explicit reason");
        if let Some((_, reason)) = exclusion { assert!(!reason.is_empty()); eprintln!("success exclusion: {name}: {reason}"); }
    }
}

#[tokio::test]
async fn selector_refusals_never_turn_request_values_into_evidence() {
    use eunomia_backend::{authz::{self, Actor, Caller}, rid, scopes};
    let app = TestApp::new().await;
    let caller = Caller { actor: Actor { kind: "token", id: "api_token:evidence-test".into() },
        scopes: scopes::ALL.iter().map(|s| s.to_string()).collect(),
        vault: Some(rid::parse("vault:bound999777").unwrap()), expires_at: None };
    for (name, spec) in registry::all_tools() {
        if *name == "memory_write_batch" { continue; }
        for key in ["vault_id", "vault_id_a", "vault_id_b", "vault_ids"] {
            if spec.schema["properties"].get(key).is_none() { continue; }
            let mut args = canary(&spec.schema);
            for selector in ["vault_id", "vault_id_a", "vault_id_b", "vault_ids"] {
                if spec.schema["properties"].get(selector).is_some() {
                    args[selector] = if selector == "vault_ids" { json!(["vault:bound999777"]) } else { json!("vault:bound999777") };
                }
            }
            args[key] = if key == "vault_ids" { json!(["vault:request999777"]) } else { json!("vault:request999777") };
            let result = authz::with_caller(caller.clone(), registry::call(&app.state, &app.user, name, args)).await.unwrap_or_else(|e| e.to_tool_value());
            assert_eq!(result["code"], "auth.scope", "{name}/{key}");
            let computed = evidence::for_tool(name).unwrap().computed(&result);
            assert!(!has_canary(&computed), "{name}/{key}: refusal registered a request value as evidence");
        }
    }
}
