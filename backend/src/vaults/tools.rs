//! Vault-management agent-tool wrappers: thin reshapes over `vaults::service` for the MCP
//! agent-tool surface (registration and schemas live in `tools::registry`).

use serde_json::{json, Value};
use surrealdb::types::RecordId;

use crate::pool::{ControlDb, OrgDb};
use crate::vaults::service;

/// A tool must never raise on bad agent
/// input, it returns `{"error": "<tool_name>: <message>"}` instead.
fn err(tool_name: &str, message: &str) -> Value {
    json!({ "error": format!("{tool_name}: {message}") })
}

fn err_app(tool_name: &str, e: &crate::error::AppError) -> Value {
    let mut v = e.to_tool_value();
    v["error"] = format!("{tool_name}: {}", e.detail()).into();
    v
}

pub async fn vault_create(db: &OrgDb, owner: &RecordId, name: &str, kind: &str) -> Value {
    match service::create_vault(db, owner, name, kind).await {
        Ok(v) => serde_json::to_value(v).unwrap_or_else(|e| err("vault_create", &e.to_string())),
        Err(e) => err_app("vault_create", &e),
    }
}

pub async fn vault_list(db: &OrgDb, owner: &RecordId) -> Value {
    match service::list_my_vaults(db, owner).await {
        Ok(v) => json!({ "results": v }),
        Err(e) => err_app("vault_list", &e),
    }
}

pub async fn vault_get(db: &OrgDb, owner: &RecordId, vault_id: &RecordId) -> Value {
    match service::get_vault(db, owner, vault_id).await {
        Ok(v) => serde_json::to_value(v).unwrap_or_else(|e| err("vault_get", &e.to_string())),
        Err(e) => err_app("vault_get", &e),
    }
}

pub async fn vault_clone(db: &OrgDb, owner: &RecordId, vault_id: &RecordId, name: Option<&str>, kind: &str) -> Value {
    match service::clone_vault(db, owner, vault_id, name, kind).await {
        Ok(v) => serde_json::to_value(v).unwrap_or_else(|e| err("vault_clone", &e.to_string())),
        Err(e) => err_app("vault_clone", &e),
    }
}

pub async fn vault_merge(db: &OrgDb, owner: &RecordId, a: &RecordId, b: &RecordId, name: Option<&str>, kind: &str) -> Value {
    match service::merge_vaults(db, owner, a, b, name, kind).await {
        Ok(v) => serde_json::to_value(v).unwrap_or_else(|e| err("vault_merge", &e.to_string())),
        Err(e) => err_app("vault_merge", &e),
    }
}

pub async fn vault_invite(db: &OrgDb, control: &ControlDb, owner: &RecordId, vault_id: &RecordId, email: &str, role: &str) -> Value {
    match service::invite_member(db, control, owner, vault_id, email, role).await {
        Ok(v) => serde_json::to_value(v).unwrap_or_else(|e| err("vault_invite", &e.to_string())),
        Err(e) => err_app("vault_invite", &e),
    }
}

pub async fn vault_members(db: &OrgDb, control: &ControlDb, owner: &RecordId, vault_id: &RecordId) -> Value {
    match service::list_members(db, control, owner, vault_id).await {
        Ok(v) => json!({ "results": v }),
        Err(e) => err_app("vault_members", &e),
    }
}

pub async fn vault_remove_member(db: &OrgDb, control: &ControlDb, owner: &RecordId, vault_id: &RecordId, email: &str) -> Value {
    match service::remove_member(db, control, owner, vault_id, email).await {
        Ok(()) => json!({ "removed": true }),
        Err(e) => err_app("vault_remove_member", &e),
    }
}

pub async fn vault_set_role(db: &OrgDb, control: &ControlDb, owner: &RecordId, vault_id: &RecordId, email: &str, role: &str) -> Value {
    match service::set_member_role(db, control, owner, vault_id, email, role).await {
        Ok(v) => serde_json::to_value(v).unwrap_or_else(|e| err("vault_set_role", &e.to_string())),
        Err(e) => err_app("vault_set_role", &e),
    }
}

pub async fn vault_transfer_ownership(db: &OrgDb, control: &ControlDb, owner: &RecordId, vault_id: &RecordId, email: &str) -> Value {
    match service::transfer_ownership(db, control, owner, vault_id, email).await {
        Ok(v) => serde_json::to_value(v).unwrap_or_else(|e| err("vault_transfer_ownership", &e.to_string())),
        Err(e) => err_app("vault_transfer_ownership", &e),
    }
}

pub async fn vault_leave(db: &OrgDb, owner: &RecordId, vault_id: &RecordId) -> Value {
    match service::leave_vault(db, owner, vault_id).await {
        Ok(()) => json!({ "left": true }),
        Err(e) => err_app("vault_leave", &e),
    }
}

pub async fn vault_invitations(db: &OrgDb, owner: &RecordId) -> Value {
    match service::list_my_invitations(db, owner).await {
        Ok(v) => json!({ "results": v }),
        Err(e) => err_app("vault_invitations", &e),
    }
}

pub async fn vault_accept_invitation(db: &OrgDb, owner: &RecordId, vault_id: &RecordId) -> Value {
    match service::accept_invitation(db, owner, vault_id).await {
        Ok(v) => serde_json::to_value(v).unwrap_or_else(|e| err("vault_accept_invitation", &e.to_string())),
        Err(e) => err_app("vault_accept_invitation", &e),
    }
}

pub async fn vault_decline_invitation(db: &OrgDb, owner: &RecordId, vault_id: &RecordId) -> Value {
    match service::decline_invitation(db, owner, vault_id).await {
        Ok(()) => json!({ "declined": true }),
        Err(e) => err_app("vault_decline_invitation", &e),
    }
}

pub async fn vault_rename(db: &OrgDb, owner: &RecordId, vault_id: &RecordId, name: &str) -> Value {
    match service::rename_vault(db, owner, vault_id, name).await {
        Ok(v) => serde_json::to_value(v).unwrap_or_else(|e| err("vault_rename", &e.to_string())),
        Err(e) => err_app("vault_rename", &e),
    }
}

pub async fn vault_delete(db: &OrgDb, owner: &RecordId, vault_id: &RecordId) -> Value {
    match service::delete_vault(db, owner, vault_id).await {
        Ok(()) => json!({ "deleted": true }),
        Err(e) => err_app("vault_delete", &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn err_formats_as_tool_name_and_message() {
        let v = err("vault_create", "boom");
        assert_eq!(v, json!({ "error": "vault_create: boom" }));
    }

    #[test]
    fn removed_and_deleted_and_left_shapes_are_plain_bools() {
        assert_eq!(json!({ "removed": true }), json!({ "removed": true }));
        assert_eq!(json!({ "deleted": true })["deleted"], json!(true));
        assert_eq!(json!({ "left": true })["left"], json!(true));
    }
}
