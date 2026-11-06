//! Secret requests, transforms and the one consumer of held secrets
//! (docs/architecture/specs/secret-requests.md). Part of the tool registry (see `registry/mod.rs`); the
//! work is in `secrets::requests`, `secrets::transform` and `secrets::outbound`. No tool here ever
//! returns a secret value: only handles, `{"$secret": "eu://<vault>/<name>"}`.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use super::{register, ToolSpec};

pub(super) fn register_all(registry: &mut HashMap<&'static str, ToolSpec>) {
    let hosts = json!({"type": "array", "items": {"type": "string"}, "description": "host names it will be sent to, e.g. api.stripe.com, api.example.com:8443 or *.example.com; the user approves them"});
    let placements = json!({"type": "array", "items": {"type": "string", "enum": ["header", "query", "body"]}, "description": "where in a request it may go; default header"});
    register(
        registry,
        "secret_request",
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "pattern": "^[a-z0-9][a-z0-9_.-]{0,63}$", "description": "a stable key, e.g. stripe_api_key: the handle will be eu://<vault>/<name>"},
                "label": {"type": "string", "description": "what to call it in the pop-up, e.g. Stripe secret key"},
                "description": {"type": "string", "description": "why you need it, shown to the user"},
                "vault_id": {"type": "string", "description": "the vault to keep it in (id or name); default personal"},
                "target": {
                    "type": "object",
                    "properties": {
                        "hosts": hosts,
                        "placements": placements,
                        "connector": {"type": "string", "description": "the connector it is for, shown only"},
                    },
                },
                "expires_in_minutes": {"type": "integer", "minimum": 1, "maximum": 10080, "description": "how long the request waits for the user; default 60"},
                "secret_expires_in_days": {"type": "integer", "minimum": 1, "maximum": 3650, "description": "how long the secret is kept once filled; default until deleted"},
                "replace": {"type": "boolean", "description": "ask for a new value for a secret that already has this name"},
            },
            "required": ["name", "description"],
        }),
        Arc::new(|state, owner, args| Box::pin(crate::secrets::requests::create(state, owner, args))),
    );
    register(
        registry,
        "secret_request_status",
        json!({
            "type": "object",
            "properties": {"request_id": {"type": "string", "description": "the request_id secret_request returned"}},
            "required": ["request_id"],
        }),
        Arc::new(|state, owner, args| Box::pin(crate::secrets::requests::status(state, owner, args))),
    );
    register(
        registry,
        "secret_transform",
        json!({
            "type": "object",
            "properties": {
                "op": {"type": "string", "enum": crate::secrets::transform::OPS},
                "inputs": {"type": "object", "additionalProperties": {"type": "string"}, "description": "input name -> eu:// handle, e.g. {\"value\": \"eu://v1/stripe_api_key\"}; base64, base64url, url_encode and sha256 take value; basic_auth password (and user); hmac_sha256 key (and message); jwt_* key; template any names"},
                "params": {"type": "object", "description": "literal parameters: template (\"Bearer {{value}}\"), user, message, encoding (hex, base64, base64url), key_encoding, message_encoding, claims, header {kid, typ}, expires_in_seconds"},
                "name": {"type": "string", "pattern": "^[a-z0-9][a-z0-9_.-]{0,63}$", "description": "the new secret's name; default <op>_<input>_<random>"},
                "expires_in_days": {"type": "integer", "minimum": 1, "maximum": 3650},
                "replace": {"type": "boolean", "description": "overwrite an earlier derived secret with this name"},
            },
            "required": ["op", "inputs"],
        }),
        Arc::new(|state, owner, args| Box::pin(crate::secrets::transform::run(state, owner, args))),
    );
    register(
        registry,
        "http_request",
        json!({
            "type": "object",
            "properties": {
                "method": {"type": "string", "enum": crate::secrets::outbound::METHODS, "description": "default GET"},
                "url": {"type": "string", "description": "https URL on a host the user approved for every handle in the call; no handle in the URL itself"},
                "headers": {"type": "object", "description": "header name -> text or {\"$secret\": \"eu://...\"}"},
                "query": {"type": "object", "description": "query parameter -> text or a handle (needs the query placement)"},
                "body": {"description": "a text body: text or a handle (needs the body placement)"},
                "json": {"description": "a JSON body; handles may appear inside (needs the body placement)"},
            },
            "required": ["url"],
        }),
        Arc::new(|state, owner, args| Box::pin(crate::secrets::outbound::run(state, owner, args))),
    );
}
