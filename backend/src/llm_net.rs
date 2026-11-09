//! Where the backend may send a user's OpenAI-compatible requests. The base URL is checked when it is
//! saved (`routers/settings.rs`) and again on every call: the client built here resolves hostnames through
//! a resolver that applies the same address rules, so a name that later resolves somewhere forbidden (DNS
//! rebinding) is refused at connect time, and it never follows redirects (a redirect could name an IP
//! literal the checks never see).
//!
//! Rules: link-local and unspecified addresses (cloud metadata) and the address of the compose service
//! `surrealdb` are always refused; private and loopback addresses are refused when `ALLOW_PRIVATE_LLM_URL=0`.
//! Remaining limit: the `surrealdb` address is resolved once per process, so a database that moves to a
//! new address needs a restart; other internal services on a private network are only blocked by
//! `ALLOW_PRIVATE_LLM_URL=0`.

use std::net::{IpAddr, SocketAddr};

use crate::error::{AppError, AppResult};

/// Hosts that name Eunomia's own stack or a cloud metadata service: never a model server.
const BLOCKED_LLM_HOSTS: &[&str] = &["surrealdb", "backup", "metadata", "metadata.google.internal"];

/// `ALLOW_PRIVATE_LLM_URL`: a model server on this machine or its network (Ollama on localhost) is the
/// normal self-host setup, so private and loopback targets are allowed unless this is `0` or `false`.
pub fn allow_private_llm_url() -> bool {
    !std::env::var("ALLOW_PRIVATE_LLM_URL").is_ok_and(|v| matches!(v.trim(), "0" | "false"))
}

/// Names that resolve to fixed addresses instead of DNS (tests only).
#[cfg(feature = "test-support")]
pub static TEST_HOSTS: std::sync::Mutex<Vec<(String, Vec<IpAddr>)>> = std::sync::Mutex::new(Vec::new());

async fn lookup(host: &str, port: u16) -> std::io::Result<Vec<IpAddr>> {
    #[cfg(feature = "test-support")]
    if let Some((_, ips)) = TEST_HOSTS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(h, _)| h == host) {
        return Ok(ips.clone());
    }
    let found = tokio::time::timeout(std::time::Duration::from_secs(5), tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "lookup timed out"))??;
    Ok(found.map(|a| a.ip()).collect())
}

/// The address of the compose service `surrealdb`, resolved once (None outside compose).
async fn surrealdb_ip() -> Option<IpAddr> {
    static IP: tokio::sync::OnceCell<Option<IpAddr>> = tokio::sync::OnceCell::const_new();
    *IP.get_or_init(|| async { lookup("surrealdb", 8000).await.ok().and_then(|v| v.into_iter().next()) }).await
}

fn unmap(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

async fn ip_verdict(ip: IpAddr, allow_private: bool) -> Result<(), &'static str> {
    let ip = unmap(ip);
    let link_local = match ip {
        IpAddr::V4(v4) => v4.is_link_local() || v4.is_unspecified(),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80 || v6.is_unspecified() || v6.segments()[..6] == [0xfd00, 0x0ec2, 0, 0, 0, 0],
    };
    if link_local {
        Err("That address is reserved (link-local or cloud metadata).")
    } else if surrealdb_ip().await.is_some_and(|s| unmap(s) == ip) {
        Err("That address is this stack's database, not a model server.")
    } else if !allow_private && crate::oauth::cimd::ip_blocked(ip) {
        Err("That address is on a private network, which this server does not allow.")
    } else {
        Ok(())
    }
}

/// Validates `raw` and returns its host and port: scheme, credentials, blocked host names, and an IP
/// literal's address. Resolution of a hostname is the caller's (save time) or the client's (call time).
fn parse(raw: &str) -> Result<(url::Url, String), String> {
    let url = url::Url::parse(raw.trim()).map_err(|_| "not a valid URL.".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
        return Err("use http or https, without credentials.".into());
    }
    let host = url.host_str().ok_or("no host.")?.trim_end_matches('.').to_ascii_lowercase();
    if BLOCKED_LLM_HOSTS.contains(&host.as_str()) {
        return Err("that host is part of this stack, not a model server.".into());
    }
    Ok((url, host))
}

/// Save-time check: also resolves a hostname now and checks every address.
pub async fn check_base_url(raw: &str, allow_private: bool) -> AppResult<()> {
    if raw.trim().is_empty() {
        return Ok(());
    }
    let bad = |m: &str| AppError::bad_request(format!("Invalid base URL: {m}"));
    let (url, host) = parse(raw).map_err(|m| bad(&m))?;
    let ips: Vec<IpAddr> = match url.host() {
        Some(url::Host::Ipv4(ip)) => vec![ip.into()],
        Some(url::Host::Ipv6(ip)) => vec![ip.into()],
        _ => lookup(&host, url.port_or_known_default().unwrap_or(443)).await.map_err(|_| bad("the host did not resolve."))?,
    };
    for ip in &ips {
        ip_verdict(*ip, allow_private).await.map_err(bad)?;
    }
    if allow_private && ips.iter().any(|ip| crate::oauth::cimd::ip_blocked(*ip)) {
        tracing::warn!(%host, "openai_base_url points at a private or loopback address (ALLOW_PRIVATE_LLM_URL=0 forbids it)");
    }
    Ok(())
}

struct GuardedResolver { public_only: bool }

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let public_only = self.public_only;
        Box::pin(async move {
            let ips = lookup(name.as_str(), 0).await?;
            let allow_private = !public_only && allow_private_llm_url();
            for ip in &ips {
                ip_verdict(*ip, allow_private).await.map_err(Box::<dyn std::error::Error + Send + Sync>::from)?;
            }
            Ok(Box::new(ips.into_iter().map(|ip| SocketAddr::new(ip, 0))) as reqwest::dns::Addrs)
        })
    }
}

/// The HTTP client for a model call to `base_url`: refuses blocked hosts and IP literals now, hostnames
/// when they are resolved, and does not follow redirects.
pub async fn client(base_url: &str) -> AppResult<reqwest::Client> {
    guarded_client(base_url, false).await
}

/// Signed provider downloads must stay public even when local model servers are allowed.
pub async fn public_client(base_url: &str) -> AppResult<reqwest::Client> {
    guarded_client(base_url, true).await
}

async fn guarded_client(base_url: &str, public_only: bool) -> AppResult<reqwest::Client> {
    let (url, _) = parse(base_url).map_err(|m| AppError::internal(format!("the model base URL is not allowed: {m}")))?;
    let literal = match url.host() {
        Some(url::Host::Ipv4(ip)) => Some(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => Some(IpAddr::V6(ip)),
        _ => None,
    };
    if let Some(ip) = literal {
        ip_verdict(ip, !public_only && allow_private_llm_url()).await.map_err(|m| AppError::internal(format!("the model base URL is not allowed: {m}")))?;
    }
    reqwest::Client::builder()
        .dns_resolver(std::sync::Arc::new(GuardedResolver { public_only }))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| AppError::internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn signed_download_clients_refuse_private_and_metadata_literals() {
        for url in ["http://127.0.0.1/file", "http://[::1]/file", "http://169.254.169.254/file", "http://0.0.0.0/file"] {
            assert!(super::public_client(url).await.is_err(), "signed download accepted {url}");
        }
    }
}
