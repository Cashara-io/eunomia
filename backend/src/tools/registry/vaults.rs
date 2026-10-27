//! Vault management: create, list, clone, merge, invite, members, remove, leave, rename, delete, and
//! `changes_since`, a vault's change log (`crate::changes`).
//! Part of the tool registry (see `registry/mod.rs`); one `register` call per tool.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use super::{register, bad_args, parse_rid, to_tool_value, ToolSpec};
use crate::authz::{self, Action};
use crate::vaults::service::default_vault_id;
use crate::vaults::tools as vaults_tools;

pub(super) fn register_all(registry: &mut HashMap<&'static str, ToolSpec>) {
    // -- vaults::tools (read-only + mutating) --------------------------------

    fn default_vault_kind() -> String {
        "org".to_string()
    }

    #[derive(serde::Deserialize)]
    struct VaultCreateArgs {
        name: String,
        #[serde(default = "default_vault_kind")]
        kind: String,
    }

    register(
        registry,
        "vault_create",
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "kind": {"type": "string", "enum": ["org", "personal"], "description": "default org"},
            },
            "required": ["name"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: VaultCreateArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                Ok(vaults_tools::vault_create(&state.db, owner, &a.name, &a.kind).await)
            })
        }),
    );

    register(
        registry,
        "vault_list",
        json!({"type": "object", "properties": {}}),
        Arc::new(|state, owner, _args| Box::pin(async move { Ok(vaults_tools::vault_list(&state.db, owner).await) })),
    );

    #[derive(serde::Deserialize)]
    struct VaultCloneArgs {
        vault_id: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default = "default_vault_kind")]
        kind: String,
    }

    register(
        registry,
        "vault_clone",
        json!({
            "type": "object",
            "properties": {
                "vault_id": {"type": "string", "description": "vault to deep-copy (you must be a member)"},
                "name": {"type": "string", "description": "default: \"<source name> (copy)\""},
                "kind": {"type": "string", "enum": ["org", "personal"], "description": "default org"},
            },
            "required": ["vault_id"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: VaultCloneArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let vault_id = match parse_rid("vault_id", &a.vault_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                Ok(vaults_tools::vault_clone(&state.db, owner, &vault_id, a.name.as_deref(), &a.kind).await)
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct VaultMergeArgs {
        vault_id_a: String,
        vault_id_b: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default = "default_vault_kind")]
        kind: String,
    }

    register(
        registry,
        "vault_merge",
        json!({
            "type": "object",
            "properties": {
                "vault_id_a": {"type": "string", "description": "first vault (you must be a member)"},
                "vault_id_b": {"type": "string", "description": "second vault (you must be a member); folded into the first on duplicates"},
                "name": {"type": "string", "description": "default: \"<A> + <B>\""},
                "kind": {"type": "string", "enum": ["org", "personal"], "description": "default org"},
            },
            "required": ["vault_id_a", "vault_id_b"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: VaultMergeArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let va = match parse_rid("vault_id_a", &a.vault_id_a) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                let vb = match parse_rid("vault_id_b", &a.vault_id_b) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                Ok(vaults_tools::vault_merge(&state.db, owner, &va, &vb, a.name.as_deref(), &a.kind).await)
            })
        }),
    );

    fn default_vault_role() -> String {
        "member".to_string()
    }

    #[derive(serde::Deserialize)]
    struct VaultInviteArgs {
        vault_id: String,
        email: String,
        #[serde(default = "default_vault_role")]
        role: String,
    }

    register(
        registry,
        "vault_invite",
        json!({
            "type": "object",
            "properties": {
                "vault_id": {"type": "string"},
                "email": {"type": "string"},
                "role": {"type": "string", "enum": ["owner", "member"], "description": "default member"},
            },
            "required": ["vault_id", "email"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: VaultInviteArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let vault_id = match parse_rid("vault_id", &a.vault_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                Ok(vaults_tools::vault_invite(&state.db, &state.control, owner, &vault_id, &a.email, &a.role).await)
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct VaultIdArgs {
        vault_id: String,
    }

    register(
        registry,
        "vault_members",
        json!({"type": "object", "properties": {"vault_id": {"type": "string"}}, "required": ["vault_id"]}),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: VaultIdArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let vault_id = match parse_rid("vault_id", &a.vault_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                Ok(vaults_tools::vault_members(&state.db, &state.control, owner, &vault_id).await)
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct VaultRemoveMemberArgs {
        vault_id: String,
        email: String,
    }

    register(
        registry,
        "vault_remove_member",
        json!({
            "type": "object",
            "properties": {"vault_id": {"type": "string"}, "email": {"type": "string"}},
            "required": ["vault_id", "email"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: VaultRemoveMemberArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let vault_id = match parse_rid("vault_id", &a.vault_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                Ok(vaults_tools::vault_remove_member(&state.db, &state.control, owner, &vault_id, &a.email).await)
            })
        }),
    );

    register(
        registry,
        "vault_leave",
        json!({"type": "object", "properties": {"vault_id": {"type": "string"}}, "required": ["vault_id"]}),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: VaultIdArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let vault_id = match parse_rid("vault_id", &a.vault_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                Ok(vaults_tools::vault_leave(&state.db, owner, &vault_id).await)
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct VaultRenameArgs {
        vault_id: String,
        name: String,
    }

    register(
        registry,
        "vault_rename",
        json!({
            "type": "object",
            "properties": {"vault_id": {"type": "string"}, "name": {"type": "string"}},
            "required": ["vault_id", "name"],
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: VaultRenameArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let vault_id = match parse_rid("vault_id", &a.vault_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                Ok(vaults_tools::vault_rename(&state.db, owner, &vault_id, &a.name).await)
            })
        }),
    );

    register(
        registry,
        "vault_delete",
        json!({"type": "object", "properties": {"vault_id": {"type": "string"}}, "required": ["vault_id"]}),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: VaultIdArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let vault_id = match parse_rid("vault_id", &a.vault_id) {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                Ok(vaults_tools::vault_delete(&state.db, owner, &vault_id).await)
            })
        }),
    );

    #[derive(serde::Deserialize)]
    struct ChangesSinceArgs {
        #[serde(default)]
        vault_id: Option<String>,
        #[serde(default)]
        after: i64,
        #[serde(default = "default_changes_limit")]
        limit: i64,
    }

    fn default_changes_limit() -> i64 {
        crate::changes::DEFAULT_LIMIT
    }

    register(
        registry,
        "changes_since",
        json!({
            "type": "object",
            "properties": {
                "vault_id": {"type": "string", "description": "the vault (an id or its name); omit for your personal vault"},
                "after": {"type": "integer", "description": "the `next` of the previous call; 0 (default) reads from the start"},
                "limit": {"type": "integer", "description": "at most 1000, default 100"},
            },
        }),
        Arc::new(|state, owner, args| {
            Box::pin(async move {
                let a: ChangesSinceArgs = match serde_json::from_value(args) {
                    Ok(a) => a,
                    Err(e) => return Ok(bad_args(e)),
                };
                let vault = match a.vault_id.as_deref().map(|v| parse_rid("vault_id", v)).transpose() {
                    Ok(v) => v,
                    Err(e) => return Ok(e),
                };
                let page = async {
                    let vault = match vault {
                        Some(v) => authz::authorize(&state.db, owner, Action::ReadMemories, &v).await?.vault().clone(),
                        None => default_vault_id(&state.db, owner).await?,
                    };
                    crate::changes::after(&state.db, &vault, a.after, a.limit).await
                };
                Ok(to_tool_value(page.await))
            })
        }),
    );
}
