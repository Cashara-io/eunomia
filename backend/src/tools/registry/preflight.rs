//! Explicit vault selectors are checked before a handler can look up any object.
//! Refusals keep the service's security event, without retaining the other arguments.

use serde_json::{Value, json};
use surrealdb::types::RecordId;

use super::{ToolSpec, VAULT_ARGS, VAULT_LIST_ARGS};
use crate::{audit, authz};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::rid::RecordIdExt;
use crate::state::OrgState;

pub(super) struct Refusal {
    pub error: AppError,
    pub selector: &'static str,
    pub requested: RecordId,
}

/// Keep the handler's error contract even when its vault check runs before dispatch.
/// Search, recall and reflect propagate HTTP errors; the other handlers return tool values.
pub(super) fn result(tool: &str, error: AppError) -> AppResult<Value> {
    match tool {
        "search" | "recall" | "reflect" => Err(error),
        "vault_get" | "vault_clone" | "vault_merge" | "vault_invite" | "vault_members"
        | "vault_remove_member" | "vault_set_role" | "vault_transfer_ownership" | "vault_leave"
        | "vault_check" | "vault_repair" | "vault_accept_invitation" | "vault_decline_invitation"
        | "vault_rename" | "vault_delete" => Ok(crate::vaults::tools::err_app(tool, &error)),
        _ => Ok(error.to_tool_value()),
    }
}

pub(super) fn check(name: &str, spec: &ToolSpec, args: &Value) -> Result<(), Box<Refusal>> {
    // Batches retain their per-item checks: an item's selector overrides the batch's.
    if name == "memory_write_batch" {
        return Ok(());
    }
    for key in VAULT_ARGS {
        if spec.schema["properties"].get(*key).is_some()
            && let Some(given) = args.get(*key).and_then(Value::as_str)
            && let Ok(vault) = crate::rid::parse(given)
        {
            authz::check_vault(&vault).map_err(|error| Box::new(Refusal { error, selector: key, requested: vault }))?;
        }
    }
    for key in VAULT_LIST_ARGS {
        if spec.schema["properties"].get(*key).is_some()
            && let Some(vaults) = args.get(*key).and_then(Value::as_array)
        {
            for given in vaults.iter().filter_map(Value::as_str) {
                if let Ok(vault) = crate::rid::parse(given) {
                    authz::check_vault(&vault).map_err(|error| Box::new(Refusal { error, selector: key, requested: vault }))?;
                }
            }
        }
    }
    Ok(())
}

/// Mirrors the refusal actions in `vaults::service`, including invitation decisions.
/// Removing a member emits `vault.revoke` only on a successful withdrawal, never a refusal.
fn refusal_action(tool: &str) -> &'static str {
    match tool {
        "vault_invite" => "vault.invite",
        "vault_remove_member" => "vault.member_remove",
        "vault_set_role" => "vault.role",
        "vault_transfer_ownership" => "vault.transfer",
        "vault_delete" => "vault.delete",
        "vault_rename" => "vault.rename",
        "vault_clone" => "vault.clone",
        "vault_merge" => "vault.merge",
        "vault_accept_invitation" => "vault.join",
        "vault_decline_invitation" => "vault.decline",
        // Peer tool services, integrity, documents, entities and ontology do not
        // audit refusals. Secret requests audit success only; transforms and HTTP
        // consumers have no vault selector in their schemas.
        _ => "auth.scope_refused",
    }
}

/// Existing domain refusals are never budgeted. Only new generic refusal rows
/// use the per-credential budget, whose suppressed counts are flushed by a timer.
/// Keep the hidden target so ledger rendering never looks up a foreign vault.
pub(super) async fn record(state: &OrgState, owner: &RecordId, tool: &str, refusal: &Refusal, requested: &str) {
    if refusal.error.code != ErrorCode::AuthScope {
        return;
    }
    let Some(caller) = authz::current() else { return };
    let Some(bound) = caller.vault else { return };
    let action = refusal_action(tool);
    if action == "auth.scope_refused" && !state.scope_refusal_audit.allow(owner, &caller.actor, tool).await {
        return;
    }
    let requested: String = requested.chars().take(120).collect();
    let bound: String = bound.to_string().chars().take(120).collect();
    let detail = json!({
        "tool": tool,
        "bound_vault": bound,
        "requested_vault": requested,
        "selector": refusal.selector,
        "reason": "vault_binding_mismatch",
    }).to_string();
    audit::record_structured(&state.control, audit::Event {
        user: Some(owner), actor: &caller.actor, action,
        target: crate::vaults::service::HIDDEN_SPACE,
        outcome: refusal.error.code.as_str(), detail: &detail,
    }).await;
}
