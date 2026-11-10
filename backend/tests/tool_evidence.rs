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
