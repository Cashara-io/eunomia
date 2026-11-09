//! Bounded Streamable HTTP MCP client. Only the consumer service receives the bearer token.

use serde_json::{json, Value};
use zeroize::Zeroizing;
use crate::redact::scrub_exact_value;

const MAX_BYTES: usize = 16 * 1024 * 1024;
const VERSION: &str = "2025-03-26";

#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    Expired,
    RateLimited(u64),
    Upstream,
    Protocol,
}

impl Failure {
    pub fn message(&self) -> &'static str {
        match self {
            Self::Expired => "Composio sign-in expired. Reconnect Composio.",
            Self::RateLimited(_) => "Composio reached an app's rate limit. Saved progress will resume on the next sync.",
            Self::Upstream => "Composio could not read this app. Try again later.",
            Self::Protocol => "Composio returned an unsupported response. No sync progress was advanced.",
        }
    }
}

pub struct Client {
    http: reqwest::Client,
    endpoint: String,
    token: Zeroizing<String>,
    session: Option<Zeroizing<String>>,
    next_id: u64,
}

impl Client {
    pub async fn consumer(token: &str) -> Result<Self, Failure> {
        Self::connect(super::oauth::RESOURCE, token).await
    }

    async fn connect(endpoint: &str, token: &str) -> Result<Self, Failure> {
        let mut client = Self {
            http: reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(90)).build().map_err(|_| Failure::Upstream)?,
            endpoint: endpoint.into(), token: Zeroizing::new(token.to_string()), session: None, next_id: 0,
        };
        let result = client.request("initialize", json!({
            "protocolVersion": VERSION, "capabilities": {}, "clientInfo": {"name":"Eunomia", "version":env!("CARGO_PKG_VERSION")}
        }), false).await?;
        if result["protocolVersion"] != VERSION { return Err(Failure::Protocol); }
        client.request("notifications/initialized", json!({}), true).await?;
        Ok(client)
    }

    async fn request(&mut self, method: &str, params: Value, notification: bool) -> Result<Value, Failure> {
        self.next_id += 1;
        let id = self.next_id;
        let mut body = json!({"jsonrpc":"2.0", "method":method, "params":params});
        if !notification { body["id"] = json!(id); }
        let mut request = self.http.post(&self.endpoint)
            .bearer_auth(self.token.as_str())
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", VERSION).json(&body);
        if let Some(session) = &self.session { request = request.header("Mcp-Session-Id", session.as_str()); }
        let mut response = request.send().await.map_err(|_| Failure::Upstream)?;
        match response.status().as_u16() {
            401 => return Err(Failure::Expired),
            429 => return Err(Failure::RateLimited(response.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(60))),
            200..=299 => {},
            _ => return Err(Failure::Upstream),
        }
        if notification { return Ok(Value::Null); }
        if let Some(session) = response.headers().get("Mcp-Session-Id") {
            self.session = Some(Zeroizing::new(session.to_str().map_err(|_| Failure::Protocol)?.to_string()));
        }
        if response.content_length().is_some_and(|n| n > MAX_BYTES as u64) { return Err(Failure::Protocol); }
        let sse = response.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("text/event-stream"));
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| Failure::Upstream)? {
            if bytes.len() + chunk.len() > MAX_BYTES { return Err(Failure::Protocol); }
            bytes.extend_from_slice(&chunk);
            if sse && let Some(result) = sse_message(&bytes, id)? { return Ok(result); }
        }
        if sse { return Err(Failure::Protocol); }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Failure::Protocol)?;
        rpc_result(value, id)
    }

    pub async fn tools(&mut self) -> Result<Value, Failure> {
        self.request("tools/list", json!({}), false).await
    }

    pub async fn call(&mut self, name: &str, arguments: Value) -> Result<Value, Failure> {
        if !matches!(name, "COMPOSIO_SEARCH_TOOLS" | "COMPOSIO_GET_TOOL_SCHEMAS" | "COMPOSIO_MANAGE_CONNECTIONS" | "COMPOSIO_MULTI_EXECUTE_TOOL") {
            return Err(Failure::Protocol);
        }
        let result = self.request("tools/call", json!({"name":name, "arguments":arguments}), false).await?;
        if result["isError"] == true { return Err(Failure::Upstream); }
        let mut value = match result.get("structuredContent") {
            Some(value) => value.clone(),
            None => {
                let text = result["content"].as_array().and_then(|parts| parts.iter().find(|p| p["type"] == "text"))
                    .and_then(|p| p["text"].as_str()).ok_or(Failure::Protocol)?;
                serde_json::from_str(text).map_err(|_| Failure::Protocol)?
            },
        };
        let mut secrets = vec![Zeroizing::new(self.token.to_string())];
        if let Some(session) = &self.session { secrets.push(Zeroizing::new(session.to_string())); }
        scrub_exact_value(&mut value, &secrets);
        Ok(value)
    }
}

fn rpc_result(value: Value, id: u64) -> Result<Value, Failure> {
    if value["jsonrpc"] != "2.0" || value["id"] != id { return Err(Failure::Protocol); }
    if value.get("error").is_some() { return Err(Failure::Upstream); }
    value.get("result").cloned().ok_or(Failure::Protocol)
}

fn sse_message(bytes: &[u8], id: u64) -> Result<Option<Value>, Failure> {
    let text = String::from_utf8_lossy(bytes).replace("\r\n", "\n");
    for event in text.split("\n\n").take(text.matches("\n\n").count()) {
        let data = event.lines().filter_map(|line| line.strip_prefix("data:").map(str::trim_start)).collect::<Vec<_>>().join("\n");
        if data.is_empty() { continue; }
        let value: Value = serde_json::from_str(&data).map_err(|_| Failure::Protocol)?;
        if value["id"] == id { return rpc_result(value, id).map(Some); }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::mock::{route, serve};

    #[tokio::test]
    async fn initialises_a_session_and_scrubs_canaries_from_tool_results() {
        let mock = serve(vec![
            route("POST", "/mcp", json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":VERSION}})).body("initialize\"").header("Mcp-Session-Id", "canary-session"),
            route("POST", "/mcp", json!({})).body("notifications/initialized").status(202),
            route("POST", "/mcp", json!({"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"{\"value\":\"canary-token\",\"session\":\"canary-session\"}"}]}})).body("tools/call"),
        ]).await;
        let mut client = Client::connect(&format!("{}/mcp", mock.base), "canary-token").await.unwrap();
        let result = client.call("COMPOSIO_MANAGE_CONNECTIONS", json!({"toolkits":[{"name":"gmail","action":"list"}]})).await.unwrap();
        assert!(!result.to_string().contains("canary"));
        let requests = mock.requests();
        assert!(requests.iter().all(|r| r.header("authorization") == "Bearer canary-token"));
        assert_eq!(requests[2].header("mcp-session-id"), "canary-session");
        assert!(client.call("COMPOSIO_REMOTE_BASH_TOOL", json!({})).await.is_err());
    }

    #[test]
    fn sse_waits_for_complete_matching_event() {
        assert!(sse_message(b"data: {\"jsonrpc\":\"2.0\",\"id\":3", 3).unwrap().is_none());
        assert_eq!(sse_message(b": heartbeat\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"ok\":true}}\n\n", 3).unwrap().unwrap()["ok"], true);
        assert!(rpc_result(json!({"jsonrpc":"2.0","id":9,"result":{}}),3).is_err());
    }

    #[tokio::test]
    async fn classifies_rate_limits_and_never_echoes_provider_errors() {
        let mock = serve(vec![route("POST", "/mcp", json!({"secret":"canary"})).status(429).header("retry-after","12")]).await;
        let result = Client::connect(&format!("{}/mcp",mock.base),"canary").await;
        assert!(matches!(result, Err(Failure::RateLimited(12))));
        assert!(!Failure::Upstream.message().contains("canary"));
    }
}
