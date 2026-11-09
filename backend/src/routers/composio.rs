//! Consumer OAuth routes. The browser session owns pending state and every app choice.
use crate::{
    connectors::{
        composio::{
            account, storage,
            tools::{self, ReadClient, Selection},
        },
        service,
    },
    error::{AppError, AppResult},
    models_user::User,
    state::AppState,
};
use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/connectors/composio/status", get(status))
        .route("/connectors/composio/begin", post(begin))
        .route("/connectors/composio/callback", post(callback))
        .route("/connectors/composio/manage", post(manage))
        .route("/connectors/composio/selection", post(selection))
        .route("/connectors/composio/disconnect", post(disconnect))
}
fn binding(app: &AppState, headers: &HeaderMap) -> AppResult<String> {
    crate::auth::session_sid(&app.settings.jwt_secret, headers)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::bad_request("Use your browser session to connect Composio."))
}
#[utoipa::path(operation_id = "composioBegin", post, path = "/api/connectors/composio/begin", tag = "connectors",
    responses((status = 200, body = Object), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])))]
async fn begin(
    State(app): State<AppState>,
    user: User,
    headers: HeaderMap,
) -> AppResult<Json<Value>> {
    let binding = binding(&app, &headers)?;
    let state = app.org(&user.org).await?;
    let redirect = format!(
        "{}/connectors/composio/callback",
        app.settings.public_url.trim_end_matches('/')
    );
    let url = account::begin(&state, &user.id, &redirect, &binding).await?;
    Ok(Json(json!({"url":url})))
}
#[derive(Deserialize, utoipa::ToSchema)]
struct Callback {
    state: String,
    code: String,
}
#[utoipa::path(operation_id = "composioCallback", post, path = "/api/connectors/composio/callback", tag = "connectors",
    request_body = Callback,
    responses((status = 200, body = Object), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])))]
async fn callback(
    State(app): State<AppState>,
    user: User,
    headers: HeaderMap,
    Json(body): Json<Callback>,
) -> AppResult<Json<Value>> {
    let binding = binding(&app, &headers)?;
    let state = app.org(&user.org).await?;
    account::finish(&state, &user.id, &binding, &body.state, &body.code).await?;
    Ok(Json(json!({"ok":true})))
}
#[utoipa::path(operation_id = "composioStatus", get, path = "/api/connectors/composio/status", tag = "connectors",
    responses((status = 200, body = Object), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])))]
async fn status(State(app): State<AppState>, user: User) -> AppResult<Json<Value>> {
    let state = app.org(&user.org).await?;
    let row = storage::load(&state.db, &user.id).await?;
    let credentials = crate::sources::base::Conn {
        credentials: service::credentials_of(&state.settings.encryption_key, &row)?,
        config: json!({}),
    };
    let connected = credentials.credentials["access_token"]
        .as_str()
        .is_some_and(|v| !v.is_empty());
    let mut accounts = Vec::new();
    let mut health = None;
    if connected {
        match account::access_token(&state, &user.id).await {
            Ok(token) => match ReadClient::new(&token).await {
                Ok(mut client) => {
                    for toolkit in tools::TOOLKITS {
                        match client.accounts(toolkit).await {
                            Ok(data) => collect_accounts(&data, toolkit, &mut accounts),
                            Err(e) => {
                                health = Some(e.message);
                                break;
                            }
                        }
                    }
                }
                Err(e) => health = Some(e.message),
            },
            Err(e) => health = Some(e.message),
        }
    }
    Ok(Json(
        json!({"connected":connected,"account":credentials.credentials["account"].as_str().unwrap_or("Your Composio account"),"accounts":accounts,"apps":row.config.get("apps").cloned().unwrap_or(json!([])),"vault":row.routing["default_vault"],"enabled":row.enabled,"health":health}),
    ))
}
fn collect_accounts(v: &Value, toolkit: &str, out: &mut Vec<Value>) {
    match v {
        Value::Object(o) => {
            if let (Some(id), Some(status)) = (
                o.get("id").or(o.get("account_id")).and_then(Value::as_str),
                o.get("status").and_then(Value::as_str),
            ) {
                if !out.iter().any(|a| a["toolkit"] == toolkit && a["id"] == id) {
                    out.push(json!({"toolkit":toolkit,"id":id,"label":o.get("alias").and_then(Value::as_str).unwrap_or("Connected account"),"status":status}));
                }
            } else {
                for value in o.values() {
                    collect_accounts(value, toolkit, out);
                }
            }
        }
        Value::Array(a) => {
            for v in a {
                collect_accounts(v, toolkit, out)
            }
        }
        _ => {}
    }
}
#[derive(Deserialize, utoipa::ToSchema)]
struct Manage {
    toolkit: String,
}
#[utoipa::path(operation_id = "composioManage", post, path = "/api/connectors/composio/manage", tag = "connectors",
    request_body = Manage,
    responses((status = 200, body = Object), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])))]
async fn manage(
    State(app): State<AppState>,
    user: User,
    Json(body): Json<Manage>,
) -> AppResult<Json<Value>> {
    if !tools::TOOLKITS.contains(&body.toolkit.as_str()) {
        return Err(AppError::bad_request("Choose a supported app."));
    }
    let state = app.org(&user.org).await?;
    let token = account::access_token(&state, &user.id).await?;
    let mut client = ReadClient::new(&token).await?;
    let _=client.mcp.call("COMPOSIO_SEARCH_TOOLS",json!({"queries":[{"use_case":format!("Read {}",body.toolkit)}],"session":{"generate_id":true}})).await.map_err(tools::failure)?;
    let value = tools::data(
        client
            .mcp
            .call(
                "COMPOSIO_MANAGE_CONNECTIONS",
                json!({"toolkits":[{"name":body.toolkit,"action":"add"}]}),
            )
            .await
            .map_err(tools::failure)?,
    )?;
    fn link(v: &Value) -> Option<&str> {
        match v {
            Value::Object(o) => o
                .get("redirect_url")
                .and_then(Value::as_str)
                .or_else(|| o.values().find_map(link)),
            Value::Array(a) => a.iter().find_map(link),
            _ => None,
        }
    }
    let url = link(&value).ok_or_else(|| {
        AppError::bad_request("Composio did not return a connection link. Try again.")
    })?;
    let parsed = url::Url::parse(url)
        .map_err(|_| AppError::bad_request("Invalid Composio connection link."))?;
    let mock = crate::connectors::composio::test_origin()
        .is_some_and(|o| parsed.origin().ascii_serialization() == o);
    if !mock
        && (parsed.scheme() != "https"
            || !parsed.host_str().is_some_and(|h| {
                h == "composio.dev"
                    || h.ends_with(".composio.dev")
                    || h == "composio.ai"
                    || h.ends_with(".composio.ai")
            }))
    {
        return Err(AppError::bad_request("Invalid Composio connection link."));
    }
    Ok(Json(json!({"url":url})))
}
#[derive(Deserialize, utoipa::ToSchema)]
struct Choices {
    apps: Vec<Selection>,
    vault: Option<String>,
}
#[utoipa::path(operation_id = "composioSelection", post, path = "/api/connectors/composio/selection", tag = "connectors",
    request_body = Choices,
    responses((status = 200, body = Object), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])))]
async fn selection(
    State(app): State<AppState>,
    user: User,
    Json(body): Json<Choices>,
) -> AppResult<Json<Value>> {
    if body.apps.len() > 24 {
        return Err(AppError::bad_request("Choose at most 24 app accounts."));
    }
    let state = app.org(&user.org).await?;
    let token = account::access_token(&state, &user.id).await?;
    let connected = storage::load(&state.db, &user.id).await?;
    let current = service::credentials_of(&state.settings.encryption_key, &connected)?;
    let current = crate::sources::base::Conn {
        credentials: current,
        config: json!({}),
    };
    if current.credentials["access_token"].as_str() != Some(token.as_str()) {
        return Err(AppError::bad_request(
            "Composio changed. Refresh the app list and try again.",
        ));
    }
    let mut client = ReadClient::new(&token).await?;
    let mut seen = std::collections::HashSet::new();
    for choice in &body.apps {
        choice.validate()?;
        if !seen.insert(choice.key()) {
            return Err(AppError::bad_request(
                "This app account is already selected.",
            ));
        }
        let accounts = client.accounts(&choice.toolkit).await?;
        if !tools::has_account(&accounts, &choice.account) {
            return Err(AppError::bad_request(
                "Choose one of your active connected accounts.",
            ));
        }
    }
    let routing = crate::sources::routing::validate(
        &state.db,
        &user.id,
        "composio",
        crate::sources::routing::Routing {
            default_vault: body.vault,
            rules: vec![],
        },
    )
    .await?;
    if !storage::select(
        &state.db,
        &user.id,
        &connected,
        json!({"apps":body.apps}),
        serde_json::to_value(&routing)
            .map_err(|_| AppError::internal("Could not save Composio choices."))?,
        !body.apps.is_empty(),
    )
    .await?
    {
        return Err(AppError::bad_request(
            "Composio changed. Refresh the app list and try again.",
        ));
    }
    Ok(Json(json!({"ok":true})))
}
#[utoipa::path(operation_id = "composioDisconnect", post, path = "/api/connectors/composio/disconnect", tag = "connectors",
    responses((status = 200, body = Object), (status = "default", description = "Error", body = crate::openapi::Problem, content_type = "application/problem+json")),
    security(("cookie" = []), ("bearer" = [])))]
async fn disconnect(State(app): State<AppState>, user: User) -> AppResult<Json<Value>> {
    let state = app.org(&user.org).await?;
    let revoked = account::disconnect(&state, &user.id).await.unwrap_or(false);
    // A provider failure must not turn local deletion into a false success.
    let row = storage::load(&state.db, &user.id).await?;
    if !row.credentials_encrypted.is_empty() {
        return Err(AppError::internal(
            "Composio tokens could not be deleted. Try disconnecting again.",
        ));
    }
    Ok(Json(
        json!({"revoked":revoked,"message":if revoked{"Disconnected. Composio tokens were revoked and deleted."}else{"Disconnected. Local tokens were deleted. Remove Eunomia's session in Composio to revoke remote access."}}),
    ))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(begin, callback, status, manage, selection, disconnect))]
pub struct Doc;
