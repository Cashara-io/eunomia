//! Held credentials for connectors. The saved credential stays a handle. Each use checks the
//! connector grant, caller, vault and expiry; every provider request checks hosts and placements.

use std::sync::Arc;

use serde_json::{Value, json};
use surrealdb::types::RecordId;
use zeroize::Zeroizing;

use super::{broker, outbound, reference};
use crate::connectors::service::{self, Connector};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::models_user::User;
use crate::sources::base::Conn;
use crate::state::OrgState;

#[derive(Clone)]
pub struct Context {
    grants: Arc<Vec<outbound::Grant>>,
    values: Arc<Vec<Zeroizing<String>>>,
}

tokio::task_local! { static ACTIVE: Context; }

pub fn current() -> Option<Context> {
    ACTIVE.try_with(Clone::clone).ok()
}

impl Context {
    pub async fn client(&self, raw: &str) -> AppResult<reqwest::Client> {
        let (_, host, port) = outbound::check_url(raw).map_err(|m| AppError::coded(ErrorCode::SecretOutboundBlocked, m))?;
        outbound::check_grants(&self.grants, &host, port).map_err(|m| AppError::coded(ErrorCode::SecretHostDenied, m))?;
        outbound::client()
    }

    pub fn scrub(&self, text: &str) -> String {
        crate::redact::scrub_exact(text, &self.values).0
    }
}

/// Browser actions keep their caller. Background sync acts as the connection owner, so handles
/// restricted to an agent's credential remain unavailable unless the person approved any agent.
pub async fn with_connection<T, F, Fut>(state: &OrgState, owner: &RecordId, row: Option<&Connector>, run: F) -> AppResult<T>
where
    F: FnOnce(Conn) -> Fut,
    Fut: std::future::Future<Output = AppResult<T>>,
{
    let Some(row) = row else {
        return run(Conn::default()).await;
    };
    if row.kind == "composio" {
        let token = crate::connectors::composio::account::access_token(state, owner).await?;
        return run(Conn { credentials: json!({"access_token":token.as_str()}), config: row.config.clone() }).await;
    }
    let mut credentials = service::credentials_of(&state.settings.encryption_key, row)?;
    let mut args = json!({"headers": {}, "json": {}});
    let mut fields = Vec::new();
    if let Some(obj) = credentials.as_object() {
        for (field, value) in obj {
            let handle = value
                .as_str()
                .filter(|s| s.starts_with("eu://"))
                .or_else(|| value.get(reference::MARKER).and_then(Value::as_str));
            if let Some(handle) = handle {
                if !handle.starts_with("eu://") {
                    return Err(AppError::coded(ErrorCode::SecretUnavailable, "Connector references must be eu:// handles."));
                }
                // OAuth refresh tokens go in the form body. The other supported credentials
                // go in Authorization, including client_id/client_secret as HTTP Basic auth.
                let place = if field == "refresh_token" { "json" } else { "headers" };
                args[place][field] = json!({reference::MARKER: handle});
                fields.push((field.clone(), place));
            }
        }
    }
    if fields.is_empty() {
        return run(Conn {
            credentials,
            config: row.config.clone(),
        })
        .await;
    }
    let user = User {
        id: owner.clone(),
        email: String::new(),
        org: state.db.org(),
    };
    let tool = format!("connector:{}", row.kind);
    let prepared = match Box::pin(broker::prepare(state, &user, &tool, &args)).await {
        broker::Prep::Ready(p) => p,
        broker::Prep::Failed(e) => return Err(e),
        broker::Prep::None => {
            return Err(AppError::coded(ErrorCode::SecretUnavailable, "The connector handles could not be resolved."));
        }
    };
    for (field, place) in fields {
        credentials[&field] = prepared.args[place][&field].clone();
    }
    let context = Context {
        grants: Arc::clone(&prepared.grants),
        values: Arc::clone(&prepared.values),
    };
    let result = ACTIVE
        .scope(
            context,
            crate::redact::with_exact(
                Arc::clone(&prepared.values),
                run(Conn {
                    credentials,
                    config: row.config.clone(),
                }),
            ),
        )
        .await;
    result.map_err(|mut e| {
        e.message = prepared.scrub_text(&e.message);
        e.source = e.source.map(|s| prepared.scrub_text(&s));
        e
    })
}
