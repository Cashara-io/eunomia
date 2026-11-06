//! Peer vaults (`crate::peers`, docs/peer-vaults.md): share a vault with another Eunomia, join one,
//! list who a vault is shared with, sync now. The same service calls as the REST routes in
//! `routers/peers.rs`, so an agent can do what the UI does.
//! Part of the tool registry (see `registry/mod.rs`); one `register` call per tool.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use super::{bad_args, parse_rid, register, to_tool_value, ToolSpec};
use crate::peers;

pub(super) fn register_all(registry: &mut HashMap<&'static str, ToolSpec>) {
    fn default_role() -> String {
        peers::ROLE_EDIT.to_string()
    }

    #[derive(serde::Deserialize)]
    struct ShareArgs {
        vault_id: String,
        #[serde(default = "default_role")]
        role: String,
        #[serde(default)]
        expires_in_hours: Option<i64>,
    }

    register(
        registry,
        "vault_share_code",
        json!({
            "type": "object",
            "properties": {
                "vault_id": {"type": "string", "description": "the vault to share (an id or its name); you must own it"},
                "role": {"type": "string", "enum": ["edit", "read"], "description": "edit (default): the other Eunomia can add and change; read: it only receives"},
                "expires_in_hours": {"type": "integer", "description": "how long the code works, default 168 (7 days), at most 720"},
            },
            "required": ["vault_id"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: ShareArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let vault = match parse_rid("vault_id", &a.vault_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                let addrs = peers::addrs::advertised(&state.settings.public_url);
                Ok(to_tool_value(peers::create_share(state, owner, &vault, &a.role, a.expires_in_hours, addrs).await))
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct JoinArgs {
        share_code: String,
        #[serde(default)]
        name: Option<String>,
    }

    register(
        registry,
        "vault_join_shared",
        json!({
            "type": "object",
            "properties": {
                "share_code": {"type": "string", "description": "the code the other Eunomia's owner sent, starting with eunomia1:"},
                "name": {"type": "string", "description": "how the other Eunomia will name this one"},
            },
            "required": ["share_code"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: JoinArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let addrs = peers::addrs::advertised(&state.settings.public_url);
                let name = a.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
                    let n = peers::instance_name();
                    if n.is_empty() { "Another Eunomia".to_string() } else { n }
                });
                Ok(to_tool_value(peers::join(state, owner, &a.share_code, addrs, &name).await))
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct PeersArgs {
        vault_id: String,
    }

    register(
        registry,
        "vault_peers",
        json!({
            "type": "object",
            "properties": {"vault_id": {"type": "string", "description": "the vault (an id or its name)"}},
            "required": ["vault_id"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: PeersArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let vault = match parse_rid("vault_id", &a.vault_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                let addrs = peers::addrs::advertised(&state.settings.public_url);
                Ok(to_tool_value(peers::list(state, owner, &vault, addrs).await))
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct SyncArgs {
        link_id: String,
    }

    register(
        registry,
        "vault_peer_sync",
        json!({
            "type": "object",
            "properties": {"link_id": {"type": "string", "description": "a link id from `vault_peers`"}},
            "required": ["link_id"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: SyncArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                Ok(to_tool_value(peers::sync_now(state, owner, &a.link_id).await))
            })
        }),
    );
}
