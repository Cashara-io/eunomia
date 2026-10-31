//! Resolution at tool-call time (docs/architecture/specs/broker.md, "Resolution"). [`prepare`] runs in
//! `registry::call` after the scope checks and before the tool: it finds the `{"$secret": ...}` objects,
//! checks each against its registered [`CredentialRef`], resolves the lot through the sidecar and hands
//! back a copy of the arguments with the values in. The caller keeps the original (references only) for
//! the audit row, the capsule and the span. [`Prepared::scrub_result`] then takes every exact value out of
//! what the tool returned, and [`crate::redact::with_exact`] keeps them out of any memory or skill the tool
//! writes on the way. Every attempt is one `secret.resolve` row in control `audit_event`; never the value.

use std::sync::Arc;

use serde_json::{json, Value};
use surrealdb::types::RecordId;
use zeroize::{Zeroize, Zeroizing};

use super::client::{Client, Unreachable};
use super::reference::{self, MAX_REFS};
use super::{refs_named, CredentialRef};
use crate::audit::{self, Event};
use crate::authz::{self, Actor};
use crate::error::{AppError, ErrorCode};
use crate::models_user::User;
use crate::rid::RecordIdExt;
use crate::state::OrgState;

/// Built-in tools that take a resolved secret as an argument. None yet: today's tools write their
/// arguments into memories, entities, skills, documents and vaults, and #111's redaction covers only some
/// of those fields, so letting them resolve a secret would let an agent store it. A tool that makes an
/// outbound call with a credential (the first connectors' tools) is added here.
pub const SECRET_TOOLS: &[&str] = &[];

/// A value shorter than this cannot be scrubbed safely (it would match ordinary text), so it is refused.
pub const MIN_VALUE_LEN: usize = 4;

/// The audit outcomes (the spec's list).
pub mod outcome {
    pub const OK: &str = "ok";
    pub const DENIED: &str = "denied";
    pub const NOT_FOUND: &str = "not_found";
    pub const PROVIDER_ERROR: &str = "provider_error";
}

/// A call whose references all resolved.
pub struct Prepared {
    /// The arguments the tool runs with (values in). Zeroised when dropped.
    pub args: Value,
    pub values: Arc<Vec<Zeroizing<String>>>,
}

impl Drop for Prepared {
    fn drop(&mut self) {
        zeroize_strings(&mut self.args);
    }
}

fn zeroize_strings(v: &mut Value) {
    match v {
        Value::String(s) => s.zeroize(),
        Value::Array(items) => items.iter_mut().for_each(zeroize_strings),
        Value::Object(map) => map.values_mut().for_each(zeroize_strings),
        _ => {}
    }
}

impl Prepared {
    /// Every exact value out of a tool's result (strings at any depth, and an error's message), before it
    /// goes back to the agent or into a capsule.
    pub fn scrub_result(&self, result: &mut crate::error::AppResult<Value>) {
        match result {
            Ok(v) => {
                crate::redact::scrub_exact_value(v, &self.values);
            }
            Err(e) => {
                e.message = crate::redact::scrub_exact(&e.message, &self.values).0;
                if let Some(s) = &mut e.source {
                    *s = crate::redact::scrub_exact(s, &self.values).0;
                }
            }
        }
    }

    pub fn scrub_text(&self, text: &str) -> String {
        crate::redact::scrub_exact(text, &self.values).0
    }
}

pub enum Prep {
    /// No `$secret` in the arguments: the call goes on untouched.
    None,
    Ready(Prepared),
    /// `secret.unavailable`: the tool does not run.
    Failed(AppError),
}

/// Test builds can register extra tools that take secrets (`registry::register_test_tool`).
pub fn tool_takes_secrets(tool: &str) -> bool {
    SECRET_TOOLS.contains(&tool) || crate::tools::registry::test_tool_takes_secrets(tool)
}

/// One reference of the call, as it goes through the steps.
struct Item {
    pointer: String,
    /// The text for the audit row (as normalised when it parsed, else as given, cut short).
    text: String,
    row: Option<CredentialRef>,
    outcome: &'static str,
    reason: String,
    value: Option<Zeroizing<String>>,
}

impl Item {
    fn fail(&mut self, outcome: &'static str, reason: impl Into<String>) {
        if self.outcome == outcome::OK {
            self.outcome = outcome;
            self.reason = reason.into();
        }
    }
}

/// Who is calling: the request's credential, or the user outside a request.
fn caller(user: &User) -> authz::Caller {
    authz::current().unwrap_or_else(|| authz::Caller::session(&user.id))
}

/// Why `row` may not be used by this call, or `None` when it may.
async fn refusal(state: &OrgState, user: &User, who: &authz::Caller, tool: &str, row: &CredentialRef) -> Option<String> {
    if authz::ensure_member(&state.db, &user.id, &row.vault).await.is_err() {
        return Some("you are not a member of the vault the reference is bound to".into());
    }
    if who.vault.as_ref().is_some_and(|v| *v != row.vault) {
        return Some("this credential is restricted to another vault".into());
    }
    if !row.allowed_tools.iter().any(|t| t == "*" || t == tool) {
        return Some(format!("the reference is not allowed for the tool {tool}"));
    }
    if !row.allowed_actors.is_empty() && !row.allowed_actors.iter().any(|a| *a == who.actor.id || *a == user.id.to_string()) {
        return Some("the reference is not allowed for this credential".into());
    }
    None
}

/// Finds, checks, resolves. See the module docs.
pub async fn prepare(state: &OrgState, user: &User, tool: &str, args: &Value) -> Prep {
    if !reference::contains(args) {
        return Prep::None;
    }
    let found = reference::find(args);
    if found.is_empty() {
        return Prep::None;
    }
    let who = caller(user);
    let call_vault: Option<RecordId> = args.get("vault_id").and_then(Value::as_str).and_then(|v| crate::rid::parse(v).ok());
    let mut items: Vec<Item> = found
        .into_iter()
        .map(|f| {
            let (text, outcome, reason) = match f.reference {
                Ok(r) => (r.text(), outcome::OK, String::new()),
                Err(m) => (f.raw.chars().take(200).collect(), outcome::DENIED, format!("malformed reference: {}", m.0)),
            };
            Item { pointer: f.pointer, text, row: None, outcome, reason, value: None }
        })
        .collect();

    if items.len() > MAX_REFS {
        items.iter_mut().for_each(|i| i.fail(outcome::DENIED, format!("more than {MAX_REFS} references in one call")));
    }
    if !tool_takes_secrets(tool) {
        items.iter_mut().for_each(|i| i.fail(outcome::DENIED, format!("the tool {tool} does not take secrets")));
    }

    // the registered row each reference may use
    for item in items.iter_mut().filter(|i| i.outcome == outcome::OK) {
        let rows = match refs_named(&state.db, &user.id, &item.text).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(error = %e.message, "secret broker: reading references failed");
                item.fail(outcome::PROVIDER_ERROR, "the references could not be read");
                continue;
            }
        };
        if rows.is_empty() {
            item.fail(outcome::DENIED, "the reference is not registered for any vault you belong to");
            continue;
        }
        // the row bound to the call's vault first, then the others in creation order
        let mut ordered: Vec<&CredentialRef> = rows.iter().filter(|r| Some(&r.vault) == call_vault.as_ref()).collect();
        ordered.extend(rows.iter().filter(|r| Some(&r.vault) != call_vault.as_ref()));
        let mut first_reason = None;
        for row in ordered {
            match refusal(state, user, &who, tool, row).await {
                None => {
                    item.row = Some(row.clone());
                    break;
                }
                Some(r) => {
                    first_reason.get_or_insert(r);
                }
            }
        }
        if item.row.is_none() {
            item.fail(outcome::DENIED, first_reason.unwrap_or_default());
        }
    }

    // resolve, one sidecar call per provider kind, only when nothing so far was refused
    if items.iter().all(|i| i.outcome == outcome::OK) {
        let client = Client::from_settings(&state.settings);
        let mut kinds: Vec<String> = items.iter().filter_map(|i| i.row.as_ref().map(|r| r.provider.clone())).collect();
        kinds.sort();
        kinds.dedup();
        for kind in kinds {
            let idx: Vec<usize> = (0..items.len()).filter(|n| items[*n].row.as_ref().is_some_and(|r| r.provider == kind)).collect();
            let config = match super::provider_config(&state.db, &state.settings.encryption_key, &kind).await {
                Ok(Some(c)) => c,
                Ok(None) => {
                    idx.iter().for_each(|n| items[*n].fail(outcome::PROVIDER_ERROR, format!("no {kind} provider is configured")));
                    continue;
                }
                Err(e) => {
                    tracing::warn!(code = e.code.as_str(), "secret broker: the provider token does not decrypt");
                    idx.iter().for_each(|n| items[*n].fail(outcome::PROVIDER_ERROR, "the stored provider token does not decrypt (ENCRYPTION_KEY changed?)"));
                    continue;
                }
            };
            let refs: Vec<String> = idx.iter().map(|n| items[*n].text.clone()).collect();
            match client.resolve(&config, &refs).await {
                Err(Unreachable(m)) => idx.iter().for_each(|n| items[*n].fail(outcome::PROVIDER_ERROR, m.clone())),
                Ok(results) => {
                    for (n, r) in idx.iter().zip(results) {
                        let item = &mut items[*n];
                        match (r.ok, r.value) {
                            (true, Some(v)) if v.chars().count() >= MIN_VALUE_LEN => item.value = Some(v),
                            (true, _) => item.fail(outcome::PROVIDER_ERROR, format!("the value is shorter than {MIN_VALUE_LEN} characters and cannot be kept out of logs safely")),
                            (false, _) => {
                                let message: String = r.message.unwrap_or_default().chars().take(160).collect();
                                match r.error.as_deref() {
                                    Some("not_found") => item.fail(outcome::NOT_FOUND, message),
                                    Some("denied") | Some("invalid_ref") => item.fail(outcome::DENIED, message),
                                    _ => item.fail(outcome::PROVIDER_ERROR, message),
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let failed = items.iter().any(|i| i.outcome != outcome::OK);
    if failed {
        for item in items.iter_mut().filter(|i| i.outcome == outcome::OK) {
            item.value = None; // resolved, but not handed out: another reference of the call failed
            item.fail(outcome::DENIED, "not used: another reference in this call could not be resolved");
        }
    }
    let actor: Actor = who.actor.clone();
    let org = state.db.org().key();
    let ctx = crate::usage::current_context();
    for item in &items {
        let detail = json!({
            "ref": item.text.chars().take(200).collect::<String>(),
            "ref_id": item.row.as_ref().map(|r| r.id.to_string()).unwrap_or_default(),
            "tool": tool,
            "channel": ctx.channel.as_str(),
            "session": ctx.session,
            "vault": item.row.as_ref().map(|r| r.vault.to_string()).or_else(|| call_vault.as_ref().map(|v| v.to_string())).unwrap_or_default(),
            "reason": item.reason.chars().take(120).collect::<String>(),
        })
        .to_string();
        audit::record(&state.control, Event { user: Some(&user.id), actor: &actor, action: "secret.resolve", target: &org, outcome: item.outcome, detail: &detail }).await;
    }
    if failed {
        let why: Vec<String> = items.iter().filter(|i| !i.reason.starts_with("not used:")).map(|i| format!("{} ({}: {})", i.text, i.outcome, i.reason)).collect();
        return Prep::Failed(AppError::coded(ErrorCode::SecretUnavailable, format!("A secret reference could not be used, so the tool did not run: {}", why.join("; "))));
    }

    let pairs: Vec<(String, &str)> = items.iter().map(|i| (i.pointer.clone(), i.value.as_ref().map(|v| v.as_str()).unwrap_or_default())).collect();
    let resolved = reference::substitute(args, &pairs);
    let values: Vec<Zeroizing<String>> = items.into_iter().filter_map(|i| i.value).collect();
    Prep::Ready(Prepared { args: resolved, values: Arc::new(values) })
}
