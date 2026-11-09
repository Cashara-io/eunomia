//! Read-only app execution, discovered schemas and explicit connected-account selection.
use super::mcp::{Client, Failure};
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const TOOLKITS: &[&str] = &[
    "googledrive",
    "gmail",
    "googlecalendar",
    "timelinesai",
    "whatsapp",
    "github",
];

#[derive(Clone, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub toolkit: String,
    pub account: String,
    #[serde(default)]
    pub folder: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default = "default_ref")]
    pub reference: String,
    #[serde(default)]
    pub phone_number_id: String,
}
fn default_ref() -> String {
    "main".into()
}
impl Selection {
    pub fn key(&self) -> String {
        json!([
            self.toolkit,
            self.account,
            self.folder,
            self.repo,
            self.reference,
            self.phone_number_id
        ])
        .to_string()
    }
    pub fn validate(&self) -> AppResult<()> {
        if !TOOLKITS.contains(&self.toolkit.as_str())
            || self.account.is_empty()
            || self.key().len() > 1000
        {
            return Err(AppError::bad_request(
                "Choose a supported app and one of your connected accounts.",
            ));
        }
        if self.toolkit == "github"
            && (self.repo.split('/').count() != 2 || self.repo.split('/').any(str::is_empty))
        {
            return Err(AppError::bad_request(
                "Enter the Obsidian repository as owner/repository.",
            ));
        }
        if self.toolkit == "whatsapp" && self.phone_number_id.is_empty() {
            return Err(AppError::bad_request(
                "Choose a WhatsApp Business phone number ID.",
            ));
        }
        Ok(())
    }
}

pub fn failure(f: Failure) -> AppError {
    let mut error = AppError::bad_request(f.message());
    error.reason = Some(match f {
        Failure::RateLimited(_) => "connector_rate_limited",
        Failure::Expired => "connector_auth",
        Failure::Upstream => "connector_transient",
        Failure::Protocol => "connector_protocol",
    });
    if let Failure::RateLimited(delay) = f {
        error.retry_after_secs = Some(delay.clamp(60, 86400));
    }
    error
}
/// Meta-tool responses can carry the consumer wrapper before the actual payload.
pub fn data(mut value: Value) -> AppResult<Value> {
    for _ in 0..4 {
        if value.get("successful").and_then(Value::as_bool) == Some(false)
            || value.get("success").and_then(Value::as_bool) == Some(false)
        {
            return Err(execution_error(&value));
        }
        if value.get("data").is_some()
            && (value.get("successful").is_some() || value.get("success").is_some())
        {
            value = value["data"].take();
        } else {
            break;
        }
    }
    Ok(value)
}
fn execution_error(value: &Value) -> AppError {
    fn status(v: &Value) -> Option<u16> {
        if let Some(o) = v.as_object() {
            for key in [
                "status_code",
                "statusCode",
                "http_status_code",
                "status",
                "code",
            ] {
                if let Some(n) = o
                    .get(key)
                    .and_then(Value::as_u64)
                    .and_then(|n| u16::try_from(n).ok())
                {
                    if (100..600).contains(&n) {
                        return Some(n);
                    }
                }
            }
            for v in o.values() {
                if let Some(n) = status(v) {
                    return Some(n);
                }
            }
        }
        None
    }
    let code = status(value).unwrap_or(502);
    let status = reqwest::StatusCode::from_u16(code).unwrap_or(reqwest::StatusCode::BAD_GATEWAY);
    let mut e = crate::connectors::clients::provider_error(
        "https://connect.composio.dev",
        status,
        &value.to_string(),
    );
    e.message = if e.reason == Some("connector_rate_limited") {
        Failure::RateLimited(60).message()
    } else {
        Failure::Upstream.message()
    }
    .into();
    e.source = None;
    e
}

pub struct ReadClient {
    pub mcp: Client,
    session: Option<String>,
    prepared: std::collections::HashSet<String>,
}
impl ReadClient {
    pub async fn new(token: &str) -> AppResult<Self> {
        Ok(Self {
            mcp: Client::consumer(token).await.map_err(failure)?,
            session: None,
            prepared: Default::default(),
        })
    }
    pub async fn accounts(&mut self, toolkit: &str) -> AppResult<Value> {
        data(
            self.mcp
                .call(
                    "COMPOSIO_MANAGE_CONNECTIONS",
                    json!({"toolkits":[{"name":toolkit,"action":"list"}]}),
                )
                .await
                .map_err(failure)?,
        )
    }
    pub async fn execute(
        &mut self,
        selection: &Selection,
        slug: &str,
        args: Value,
    ) -> AppResult<Value> {
        if !allowed(slug) {
            return Err(failure(Failure::Protocol));
        }
        if !self.prepared.contains(slug) {
            let search = data(self.mcp.call("COMPOSIO_SEARCH_TOOLS", json!({"queries":[{"use_case":format!("Read {} using {slug}", selection.toolkit)}],"session":{"generate_id":true}})).await.map_err(failure)?)?;
            if !contains_string(&search, slug) {
                return Err(failure(Failure::Protocol));
            }
            self.session = search
                .pointer("/session/id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or(self.session.take());
            let schemas = data(
                self.mcp
                    .call(
                        "COMPOSIO_GET_TOOL_SCHEMAS",
                        json!({"tool_slugs":[slug],"session_id":self.session}),
                    )
                    .await
                    .map_err(failure)?,
            )?;
            if !contains_string(&schemas, slug) {
                return Err(failure(Failure::Protocol));
            }
            let accounts = self.accounts(&selection.toolkit).await?;
            if !has_account(&accounts, &selection.account) {
                return Err(AppError::bad_request(
                    "The selected Composio app account is no longer active. Reconnect it.",
                ));
            }
            self.prepared.insert(slug.into());
        }
        let result = data(self.mcp.call("COMPOSIO_MULTI_EXECUTE_TOOL", json!({"tools":[{"tool_slug":slug,"account":selection.account,"arguments":args}],"session_id":self.session,"sync_response_to_workbench":false})).await.map_err(failure)?)?;
        let first = result
            .pointer("/results/0")
            .ok_or_else(|| failure(Failure::Protocol))?;
        if first.get("error").is_some_and(|e| !e.is_null() && e != "") {
            return Err(execution_error(first));
        }
        data(
            first
                .get("response")
                .cloned()
                .ok_or_else(|| failure(Failure::Protocol))?,
        )
    }
}
fn allowed(slug: &str) -> bool {
    matches!(
        slug,
        "GOOGLEDRIVE_FIND_FILE"
            | "GOOGLEDRIVE_DOWNLOAD_FILE"
            | "GOOGLEDRIVE_EXPORT_GOOGLE_WORKSPACE_FILE"
            | "GMAIL_GET_PROFILE"
            | "GMAIL_LIST_HISTORY"
            | "GMAIL_FETCH_EMAILS"
            | "GMAIL_FETCH_MESSAGE_BY_MESSAGE_ID"
            | "GOOGLECALENDAR_EVENTS_LIST"
            | "TIMELINESAI_GET_CHATS"
            | "TIMELINESAI_GET_CHAT_MESSAGES"
            | "WHATSAPP_GET_MESSAGE_HISTORY"
            | "GITHUB_GET_A_TREE"
            | "GITHUB_GET_REPOSITORY_CONTENT"
    )
}
fn contains_string(v: &Value, needle: &str) -> bool {
    match v {
        Value::String(s) => s == needle,
        Value::Array(a) => a.iter().any(|v| contains_string(v, needle)),
        Value::Object(o) => {
            o.contains_key(needle) || o.values().any(|v| contains_string(v, needle))
        }
        _ => false,
    }
}
pub fn has_account(v: &Value, account: &str) -> bool {
    match v {
        Value::Object(o) => {
            ((o.get("id").or(o.get("account_id")).and_then(Value::as_str) == Some(account))
                && o.get("status")
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.eq_ignore_ascii_case("ACTIVE")))
                || o.values().any(|v| has_account(v, account))
        }
        Value::Array(a) => a.iter().any(|v| has_account(v, account)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_writes_and_foreign_or_inactive_accounts() {
        assert!(!allowed("GMAIL_SEND_EMAIL"));
        assert!(!allowed("COMPOSIO_REMOTE_BASH_TOOL"));
        let accounts = json!({"results":[{"id":"mine","status":"ACTIVE"},{"id":"expired","status":"EXPIRED"}]});
        assert!(has_account(&accounts, "mine"));
        assert!(!has_account(&accounts, "other"));
        assert!(!has_account(&accounts, "expired"));
    }
    #[test]
    fn classifies_tool_quota_errors_without_echoing_provider_secrets() {
        let e =
            data(json!({"successful":false,"error":{"status_code":429,"message":"canary-token"}}))
                .unwrap_err();
        assert_eq!(e.reason, Some("connector_rate_limited"));
        assert!(!e.message.contains("canary"));
        assert!(e.source.is_none());
        let e=data(json!({"successful":false,"error":{"status_code":403,"errors":[{"reason":"rateLimitExceeded"}]}})).unwrap_err();
        assert_eq!(e.reason, Some("connector_rate_limited"));
        let e=data(json!({"successful":false,"error":{"status_code":403,"errors":[{"reason":"insufficientPermissions"}]}})).unwrap_err();
        assert_eq!(e.reason, Some("connector_auth"));
    }
}
