//! OpenAPI drift gate. `UPDATE_OPENAPI=1 cargo test --test openapi` rewrites
//! `backend/openapi.json`; otherwise the committed file must match the code.

use std::collections::BTreeSet;
use std::path::PathBuf;

use eunomia_backend::openapi;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn committed_spec_matches_generated() {
    let generated = openapi::spec_json();
    let path = root().join("openapi.json");
    if std::env::var("UPDATE_OPENAPI").is_ok() {
        std::fs::write(&path, &generated).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == generated,
        "backend/openapi.json is stale. Run `UPDATE_OPENAPI=1 cargo test --test openapi` then `cd frontend && bun run gen:api`."
    );
}

/// Every `.route("path", get(..).post(..))` in `src/routers/*.rs` (except the
/// JSON-RPC `/mcp`) must have a matching path and method in the spec.
#[test]
fn every_registered_route_is_in_the_spec() {
    let method = regex::Regex::new(r"\b(get|post|put|patch|delete)\(").unwrap();
    let mut routes = BTreeSet::new();
    for entry in std::fs::read_dir(root().join("src/routers")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") || path.file_name().unwrap() == "mcp.rs" {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        for seg in src.split(".route(\"").skip(1) {
            let (route, rest) = seg.split_once('"').unwrap();
            // The registration ends at the next `.route(` or the end of its statement.
            let rest = rest.split(".route(").next().unwrap().split(';').next().unwrap();
            for m in method.captures_iter(rest) {
                // the extension inboxes live outside /api (`hook_router`), next to the OAuth and MCP paths
                let full = if route.starts_with("/hooks/") { route.to_string() } else { format!("/api{route}") };
                routes.insert((m[1].to_string(), full));
            }
        }
    }
    routes.insert(("get".into(), "/api/openapi.json".into()));
    routes.insert(("get".into(), "/readyz".into())); // registered in lib.rs, next to /healthz (which is not documented)
    assert!(routes.len() > 50, "route scan found only {} routes", routes.len());

    let doc = serde_json::to_value(openapi::spec()).unwrap();
    let mut documented = BTreeSet::new();
    for (path, item) in doc["paths"].as_object().unwrap() {
        for m in ["get", "post", "put", "patch", "delete"] {
            if item.get(m).is_some() {
                documented.insert((m.to_string(), path.replace(['{', '}'], "|")));
            }
        }
    }
    let norm = |s: &BTreeSet<(String, String)>| -> BTreeSet<(String, String)> {
        s.iter().map(|(m, p)| (m.clone(), p.replace(['{', '}'], "|"))).collect()
    };
    let routes = norm(&routes);
    let missing: Vec<_> = routes.difference(&documented).collect();
    let extra: Vec<_> = documented.difference(&routes).collect();
    assert!(missing.is_empty(), "routes without an OpenAPI path: {missing:?}");
    assert!(extra.is_empty(), "OpenAPI paths without a route: {extra:?}");
}
