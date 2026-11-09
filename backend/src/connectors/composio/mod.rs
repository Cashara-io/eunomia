//! Client for Composio's consumer MCP service. No Eunomia platform key or managed entity.
pub mod oauth;

pub mod mcp;

pub mod storage;

pub mod account;

pub mod tools;

/// Operator-only mock endpoint. Per-user config can never redirect OAuth or bearer tokens.
pub fn test_origin() -> Option<String> {
    if !crate::connectors::clients::base_url_override_allowed() {
        return None;
    }
    let raw = std::env::var("EUNOMIA_COMPOSIO_TEST_ORIGIN").ok()?;
    let url = url::Url::parse(&raw).ok()?;
    if url.scheme() != "http" || !matches!(url.host_str(), Some("127.0.0.1" | "localhost")) {
        return None;
    }
    Some(url.origin().ascii_serialization())
}
pub fn endpoint() -> String {
    test_origin()
        .map(|o| format!("{o}/mcp"))
        .unwrap_or_else(|| oauth::RESOURCE.into())
}
