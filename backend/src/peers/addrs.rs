//! The addresses a share code advertises, best first (docs/peer-vaults.md, "Addresses"):
//!
//! 1. LAN: `EUNOMIA_LAN_ADDRS` (the installer writes it: `host:port,...`), then, outside a container,
//!    the private IPv4 address this machine routes from (RFC 1918, link-local, or 100.64/10 for
//!    Tailscale), found with a UDP `connect` that sends nothing.
//! 2. The public name: `EUNOMIA_DOMAIN` (HTTPS through Caddy), then `PUBLIC_URL` unless it is a
//!    loopback address.
//! 3. `EUNOMIA_PEER_ADDRS`: anything else that reaches this instance (a tunnel hostname, a port forward).
//!
//! An address is a base URL (`http://192.168.1.20:3000`); a peer appends `/api/peer/v1/...`.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};

/// The frontend port a LAN address uses when none is given (`EUNOMIA_PEER_PORT`, default 3000).
fn lan_port() -> u16 {
    std::env::var("EUNOMIA_PEER_PORT").ok().and_then(|p| p.trim().parse().ok()).unwrap_or(3000)
}

/// A base URL from `entry`: kept as is with a scheme; `host:port` is http (https for port 443); a
/// bare host is https.
pub fn normalize(entry: &str) -> Option<String> {
    let e = entry.trim().trim_end_matches('/');
    if e.is_empty() {
        return None;
    }
    if e.starts_with("http://") || e.starts_with("https://") {
        return url::Url::parse(e).ok().map(|_| e.to_string());
    }
    let url = match e.rsplit_once(':').map(|(_, p)| p) {
        Some("443") => format!("https://{e}"),
        Some(p) if p.parse::<u16>().is_ok() => format!("http://{e}"),
        _ => format!("https://{e}"),
    };
    url::Url::parse(&url).ok().map(|_| url)
}

/// Private ranges a LAN peer can reach: RFC 1918, link-local, and CGNAT 100.64.0.0/10 (Tailscale).
pub fn is_lan(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_private() || ip.is_link_local() || (o[0] == 100 && (o[1] & 0b1100_0000) == 64)
}

fn in_container() -> bool {
    std::path::Path::new("/.dockerenv").exists() || std::path::Path::new("/run/.containerenv").exists()
}

/// The local address the default route leaves from. `connect` on UDP only picks a route: no packet
/// is sent.
fn routed_ipv4() -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("1.1.1.1:80").ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(ip) => Some(ip),
        IpAddr::V6(_) => None,
    }
}

fn split_list(var: &str) -> Vec<String> {
    std::env::var(var).unwrap_or_default().split(',').filter_map(normalize).collect()
}

fn is_loopback_url(u: &str) -> bool {
    url::Url::parse(u).ok().and_then(|u| u.host_str().map(str::to_string)).is_none_or(|h| h == "localhost" || h == "127.0.0.1" || h == "::1" || h == "[::1]")
}

/// This instance's addresses, best first, without duplicates.
pub fn advertised(public_url: &str) -> Vec<String> {
    advertised_with(public_url, None)
}

/// The address a browser on this network used to open the app (`Host` / `X-Forwarded-Host`, e.g.
/// `192.168.1.20:3000`), as a base URL, when it is a private IPv4 address: another Eunomia on the
/// same network can reach this one there too. `None` for a domain name, a public or loopback address.
pub fn browser_lan_addr(host: &str) -> Option<String> {
    let host = host.split(',').next()?.trim();
    let ip = host.rsplit_once(':').map_or(host, |(h, _)| h);
    let ip: Ipv4Addr = ip.parse().ok()?;
    is_lan(ip).then(|| format!("http://{host}"))
}

/// [`advertised`], plus `browser` (from [`browser_lan_addr`]) after the configured LAN addresses, so
/// sharing works on an install that never set `EUNOMIA_LAN_ADDRS`.
pub fn advertised_with(public_url: &str, browser: Option<String>) -> Vec<String> {
    let mut out: Vec<String> = split_list("EUNOMIA_LAN_ADDRS");
    out.extend(browser);
    // inside a container the routed address is the container's own: only the installer knows the host's
    if !in_container()
        && let Some(ip) = routed_ipv4().filter(|ip| is_lan(*ip))
    {
        out.push(format!("http://{ip}:{}", lan_port()));
    }
    if let Some(d) = std::env::var("EUNOMIA_DOMAIN").ok().and_then(|d| normalize(&d)) {
        out.push(d);
    }
    if let Some(p) = normalize(public_url).filter(|p| !is_loopback_url(p)) {
        out.push(p);
    }
    out.extend(split_list("EUNOMIA_PEER_ADDRS"));
    let mut seen = std::collections::HashSet::new();
    out.retain(|a| seen.insert(a.clone()));
    out
}

/// `addrs` as given in a share code: base URLs only, at most 16, each at most 300 characters.
pub fn sanitize(addrs: &[String]) -> Vec<String> {
    addrs.iter().filter(|a| a.len() <= 300).filter_map(|a| normalize(a)).take(16).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_entries() {
        assert_eq!(normalize("192.168.1.5:3000").as_deref(), Some("http://192.168.1.5:3000"));
        assert_eq!(normalize("eunomia.example.com").as_deref(), Some("https://eunomia.example.com"));
        assert_eq!(normalize("eunomia.example.com:443").as_deref(), Some("https://eunomia.example.com:443"));
        assert_eq!(normalize("https://x.example.com/").as_deref(), Some("https://x.example.com"));
        assert_eq!(normalize("  "), None);
    }

    #[test]
    fn lan_ranges() {
        for ip in ["10.0.0.1", "172.16.4.2", "192.168.1.9", "169.254.3.3", "100.101.102.103"] {
            assert!(is_lan(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "100.128.0.1", "1.1.1.1"] {
            assert!(!is_lan(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn browser_address_only_when_private_ipv4() {
        assert_eq!(browser_lan_addr("192.168.8.196:3000").as_deref(), Some("http://192.168.8.196:3000"));
        assert_eq!(browser_lan_addr("10.0.0.5").as_deref(), Some("http://10.0.0.5"));
        assert_eq!(browser_lan_addr("192.168.1.2:3000, 10.0.0.1").as_deref(), Some("http://192.168.1.2:3000"));
        assert_eq!(browser_lan_addr("localhost:3000"), None);
        assert_eq!(browser_lan_addr("127.0.0.1:3000"), None);
        assert_eq!(browser_lan_addr("8.8.8.8:3000"), None);
        assert_eq!(browser_lan_addr("eunomia.example.com"), None);
    }

    #[test]
    fn loopback_public_url_is_not_advertised() {
        assert!(is_loopback_url("http://localhost:8001"));
        assert!(!is_loopback_url("https://eunomia.example.com"));
    }
}
