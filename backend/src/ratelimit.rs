//! In-process token buckets for the noisy-neighbour limits: per user, per
//! token, and a tighter one per client address for login and signup. Per
//! process, which is the right scope for a single-node self-host install; behind
//! several replicas each enforces its own share.
// ponytail: not tower_governor. Its per-key config and keyed extractors are more
// surface than three limits need; swap it in if limits ever become per-org.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct RateConfig {
    /// Requests per minute per signed-in user (0 turns the limit off).
    pub user_per_min: u32,
    /// Requests per minute per personal access token.
    pub token_per_min: u32,
    /// Login, signup and failed-credential attempts per minute per client address.
    pub auth_per_min: u32,
    /// `/oauth/token` refresh grants per minute per client and address (`RATE_LIMIT_REFRESH_PER_MIN`).
    /// Many users behind one address refresh through the same client; code exchange stays on `auth_per_min`.
    pub refresh_per_min: u32,
    /// Source webhook deliveries per minute per client address (`RATE_LIMIT_WEBHOOK_PER_MIN`).
    pub webhook_per_min: u32,
    /// Peers whose `X-Forwarded-For` is believed (`TRUSTED_PROXIES`).
    pub trusted_proxies: Vec<Cidr>,
    /// Hostnames in `TRUSTED_PROXIES` (the compose service `frontend`), resolved by the gate.
    pub trusted_hosts: Vec<String>,
}

/// An address range such as `172.16.0.0/12`; a bare address is a single host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    net: u128,
    mask: u128,
    v6: bool,
}

impl Cidr {
    pub fn parse(s: &str) -> Option<Cidr> {
        let (addr, bits) = match s.trim().split_once('/') {
            Some((a, b)) => (a, Some(b.parse::<u32>().ok()?)),
            None => (s.trim(), None),
        };
        let ip: IpAddr = addr.parse().ok()?;
        let (v, width) = bits_of(ip);
        let prefix = bits.unwrap_or(width);
        if prefix > width {
            return None;
        }
        let mask = if prefix == 0 { 0 } else { (u128::MAX << (width - prefix)) & (u128::MAX >> (128 - width)) };
        Some(Cidr { net: v & mask, mask, v6: width == 128 })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let (v, width) = bits_of(ip);
        (width == 128) == self.v6 && v & self.mask == self.net
    }
}

fn bits_of(ip: IpAddr) -> (u128, u32) {
    match ip {
        IpAddr::V4(a) => (u128::from(u32::from(a)), 32),
        IpAddr::V6(a) => (u128::from(a), 128),
    }
}

/// Loopback only: a LAN or bridge peer cannot vouch for `X-Forwarded-For` unless the operator
/// lists it. docker-compose.yml pins the frontend's address and passes it in as `TRUSTED_PROXIES`.
const DEFAULT_TRUSTED_PROXIES: &str = "127.0.0.0/8,::1/128";

fn parse_cidrs(list: &str) -> Vec<Cidr> {
    list.split(',').filter(|s| !s.trim().is_empty()).filter_map(Cidr::parse).collect()
}

/// `TRUSTED_PROXIES` entries: CIDRs (or bare addresses) and hostnames. `none` and anything that is
/// neither a CIDR nor a plausible hostname trusts nobody.
pub fn parse_trusted(list: &str) -> (Vec<Cidr>, Vec<String>) {
    let (mut cidrs, mut hosts) = (Vec::new(), Vec::new());
    for entry in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(c) = Cidr::parse(entry) {
            cidrs.push(c);
        } else if entry != "none" && entry.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_') {
            hosts.push(entry.to_string());
        }
    }
    (cidrs, hosts)
}

/// The configured ranges plus the addresses the hostnames last resolved to.
pub fn effective_trusted(cidrs: &[Cidr], resolved: &[IpAddr]) -> Vec<Cidr> {
    cidrs.iter().copied().chain(resolved.iter().filter_map(|ip| Cidr::parse(&ip.to_string()))).collect()
}

/// Resolve `hosts` now; `None` if none resolved (so the caller keeps its last good answer).
pub async fn resolve_hosts(hosts: &[String]) -> Option<Vec<IpAddr>> {
    let mut out = Vec::new();
    for h in hosts {
        match tokio::net::lookup_host((h.as_str(), 0)).await {
            Ok(addrs) => out.extend(addrs.map(|a| a.ip())),
            Err(e) => tracing::warn!(host = %h, error = %e, "TRUSTED_PROXIES: could not resolve, keeping the last answer"),
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The client behind `peer`: with an untrusted (or unknown) peer, the peer
/// itself and `X-Forwarded-For` is ignored; with a trusted one, the right-most
/// `X-Forwarded-For` entry that is not itself a trusted proxy (a header that is not a list of
/// addresses is ignored whole).
pub fn client_addr(peer: Option<IpAddr>, forwarded_for: Option<&str>, trusted: &[Cidr]) -> String {
    let is_trusted = |ip: IpAddr| trusted.iter().any(|c| c.contains(ip));
    let Some(peer) = peer else { return "unknown".into() };
    if !is_trusted(peer) {
        return peer.to_string();
    }
    // a header that is not a clean list of addresses is not from a well-behaved proxy: ignore all of it
    let Some(chain) = forwarded_for.map(|v| v.split(',').map(|s| s.trim().parse::<IpAddr>().ok()).collect::<Option<Vec<_>>>()) else {
        return peer.to_string();
    };
    chain.unwrap_or_default().into_iter().rev().find(|ip| !is_trusted(*ip)).unwrap_or(peer).to_string()
}

fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => a.is_private() || a.is_loopback() || a.is_link_local(),
        IpAddr::V6(a) => a.is_loopback() || (a.segments()[0] & 0xfe00) == 0xfc00 || (a.segments()[0] & 0xffc0) == 0xfe80,
    }
}

/// True (and records it) when `peer` has not been warned about within the last hour.
fn warn_due(seen: &mut HashMap<IpAddr, Instant>, peer: IpAddr, now: Instant) -> bool {
    if seen.get(&peer).is_some_and(|t| now.duration_since(*t) < Duration::from_secs(3600)) {
        return false;
    }
    if seen.len() >= 1024 {
        seen.clear(); // bounded: a hostile network can only make the warning repeat early
    }
    seen.insert(peer, now);
    true
}

/// A private-range peer that is not trusted sent `X-Forwarded-For`: most likely the operator's own
/// reverse proxy, whose header is being ignored (so every user shares its address). Warns once per
/// peer per hour.
pub fn note_untrusted_forwarder(peer: Option<IpAddr>, forwarded_for: Option<&str>, trusted: &[Cidr]) {
    static SEEN: std::sync::OnceLock<Mutex<HashMap<IpAddr, Instant>>> = std::sync::OnceLock::new();
    let Some(peer) = peer.filter(|p| forwarded_for.is_some() && is_private(*p) && !trusted.iter().any(|c| c.contains(*p))) else { return };
    if warn_due(&mut SEEN.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner()), peer, Instant::now()) {
        tracing::warn!(%peer, "X-Forwarded-For from a peer that is not in TRUSTED_PROXIES is ignored, so every client behind it shares {peer}'s rate limit; add {peer} (or its hostname) to TRUSTED_PROXIES");
    }
}

/// True when the request reached a trusted proxy over https: the peer is trusted and the
/// right-most `X-Forwarded-Proto` is `https`. An untrusted peer cannot claim it.
pub fn forwarded_https(peer: Option<IpAddr>, forwarded_proto: Option<&str>, trusted: &[Cidr]) -> bool {
    peer.is_some_and(|p| trusted.iter().any(|c| c.contains(p)))
        && forwarded_proto.and_then(|v| v.rsplit(',').next()).is_some_and(|p| p.trim().eq_ignore_ascii_case("https"))
}

impl Default for RateConfig {
    fn default() -> Self {
        RateConfig { user_per_min: 1200, token_per_min: 600, auth_per_min: 20, refresh_per_min: 300, webhook_per_min: 120, trusted_proxies: parse_cidrs(DEFAULT_TRUSTED_PROXIES), trusted_hosts: Vec::new() }
    }
}

impl RateConfig {
    /// `RATE_LIMIT_USER_PER_MIN`, `RATE_LIMIT_TOKEN_PER_MIN`, `RATE_LIMIT_AUTH_PER_MIN`, `RATE_LIMIT_WEBHOOK_PER_MIN`, `TRUSTED_PROXIES`.
    pub fn from_env() -> Self {
        let d = RateConfig::default();
        let get = |key: &str, default: u32| std::env::var(key).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default);
        RateConfig {
            user_per_min: get("RATE_LIMIT_USER_PER_MIN", d.user_per_min),
            token_per_min: get("RATE_LIMIT_TOKEN_PER_MIN", d.token_per_min),
            auth_per_min: get("RATE_LIMIT_AUTH_PER_MIN", d.auth_per_min),
            refresh_per_min: get("RATE_LIMIT_REFRESH_PER_MIN", d.refresh_per_min),
            webhook_per_min: get("RATE_LIMIT_WEBHOOK_PER_MIN", d.webhook_per_min),
            // unset or blank: the defaults; any value with no valid range (say `none`) trusts nobody
            trusted_proxies: match std::env::var("TRUSTED_PROXIES") {
                Ok(v) if !v.trim().is_empty() => parse_trusted(&v).0,
                _ => d.trusted_proxies,
            },
            trusted_hosts: match std::env::var("TRUSTED_PROXIES") {
                Ok(v) if !v.trim().is_empty() => parse_trusted(&v).1,
                _ => d.trusted_hosts,
            },
        }
    }
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

#[derive(Default)]
pub struct RateLimiter {
    buckets: Mutex<Buckets>,
}

#[derive(Default)]
struct Buckets {
    map: HashMap<String, Bucket>,
    /// Size at which idle buckets are next dropped: at least `PRUNE_AT`, and twice what a prune
    /// left, so a map full of busy keys is not rescanned on every check.
    prune_at: usize,
}

const PRUNE_AT: usize = 20_000;

impl RateLimiter {
    /// Takes one token from `key`'s bucket (capacity and refill: `per_min` per minute).
    /// `Err(seconds)` is how long until a token is available.
    pub fn check(&self, key: &str, per_min: u32) -> Result<(), u64> {
        self.check_n(key, per_min, 1)
    }

    /// Whether `key`'s bucket holds `n` tokens now, taking none.
    pub fn peek(&self, key: &str, per_min: u32, n: u32) -> Result<(), u64> {
        if per_min == 0 {
            return Ok(());
        }
        let guard = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        let Some(b) = guard.map.get(key) else { return Ok(()) };
        let (cap, rate) = (f64::from(per_min), f64::from(per_min) / 60.0);
        let tokens = (b.tokens + Instant::now().duration_since(b.last).as_secs_f64() * rate).min(cap);
        let need = f64::from(n.max(1)).min(cap);
        if tokens >= need { Ok(()) } else { Err(((need - tokens) / rate).ceil().max(1.0) as u64) }
    }

    /// Takes `n` tokens from `key`'s bucket, or none: a refused request (a batch too big for what is
    /// left) costs nothing, so a smaller one right after can still go through. `Err(seconds)` until `n` tokens will be there.
    pub fn check_n(&self, key: &str, per_min: u32, n: u32) -> Result<(), u64> {
        if per_min == 0 {
            return Ok(());
        }
        let cap = f64::from(per_min);
        let rate = cap / 60.0;
        let need = f64::from(n.max(1)).min(cap);
        let now = Instant::now();
        let mut guard = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        let buckets = &mut *guard;
        if buckets.map.len() > buckets.prune_at.max(PRUNE_AT) {
            // an idle bucket has refilled; dropping it changes nothing
            buckets.map.retain(|_, b| now.duration_since(b.last) < Duration::from_secs(120));
            buckets.prune_at = buckets.map.len() * 2;
        }
        // the key is copied only for a new bucket, not on every check
        if !buckets.map.contains_key(key) {
            buckets.map.insert(key.to_string(), Bucket { tokens: cap, last: now });
        }
        let Some(b) = buckets.map.get_mut(key) else { return Ok(()) };
        b.tokens = (b.tokens + now.duration_since(b.last).as_secs_f64() * rate).min(cap);
        b.last = now;
        if b.tokens >= need {
            if n > 0 {
                b.tokens -= need;
            }
            Ok(())
        } else {
            Err(((need - b.tokens) / rate).ceil().max(1.0) as u64)
        }
    }
}

/// Failed sign-in attempts per key (account plus address, account alone as a looser ceiling, or OAuth
/// client plus address), counted in a fixed window. Per process, like [`RateLimiter`]. The map holds at
/// most `FAIL_MAX_KEYS` keys: past that, expired windows go first, then the oldest tenth.
// ponytail: per-process, so N replicas allow N times the budget.
#[derive(Default)]
pub struct FailThrottle {
    map: Mutex<HashMap<String, (u32, Instant)>>,
}

/// Failures allowed per key before attempts are refused.
pub const LOGIN_FAILS: u32 = 10;
pub const OAUTH_CLIENT_FAILS: u32 = 30;
/// Failures per account across all addresses (distributed guessing), looser than `LOGIN_FAILS`.
pub const LOGIN_ACCOUNT_FAILS: u32 = 100;
const FAIL_MAX_KEYS: usize = 50_000;
const FAIL_WINDOW: Duration = Duration::from_secs(15 * 60);

impl FailThrottle {
    fn key(key: &str) -> String {
        key.chars().take(320).collect()
    }

    /// `Err(seconds)` once `key` has used up `max` failures in the current window. Does not count.
    pub fn check(&self, key: &str, max: u32) -> Result<(), u64> {
        let now = Instant::now();
        let map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        match map.get(&Self::key(key)) {
            Some((n, start)) if *n >= max && now.duration_since(*start) < FAIL_WINDOW => Err((FAIL_WINDOW - now.duration_since(*start)).as_secs().max(1)),
            _ => Ok(()),
        }
    }

    pub fn fail(&self, key: &str) {
        self.fail_capped(key, FAIL_MAX_KEYS);
    }

    fn fail_capped(&self, key: &str, cap: usize) {
        let now = Instant::now();
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() >= cap {
            map.retain(|_, (_, start)| now.duration_since(*start) < FAIL_WINDOW);
        }
        if map.len() >= cap {
            let mut starts: Vec<Instant> = map.values().map(|(_, s)| *s).collect();
            starts.sort_unstable();
            let cut = starts[(cap / 10).max(1) - 1];
            map.retain(|_, (_, start)| *start > cut);
        }
        let e = map.entry(Self::key(key)).or_insert((0, now));
        if now.duration_since(e.1) >= FAIL_WINDOW {
            *e = (0, now);
        }
        e.0 += 1;
    }

    pub fn clear(&self, key: &str) {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).remove(&Self::key(key));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_multi_token_charge_takes_nothing() {
        let l = RateLimiter::default();
        assert!(l.check_n("k", 15, 10).is_ok());
        assert!(l.check_n("k", 15, 10).is_err(), "only 5 left");
        assert!(l.peek("k", 15, 5).is_ok(), "the refusal took nothing");
        assert!(l.check_n("k", 15, 5).is_ok());
        assert!(l.check("k", 15).is_err());
        assert!(l.check_n("free", 0, 1000).is_ok(), "0 turns the limit off");
    }

    #[test]
    fn fail_throttle_blocks_after_max_and_clears() {
        let t = FailThrottle::default();
        for _ in 0..3 {
            assert!(t.check("a", 3).is_ok());
            t.fail("a");
        }
        assert!((1..=900).contains(&t.check("a", 3).unwrap_err()));
        assert!(t.check("b", 3).is_ok());
        t.clear("a");
        assert!(t.check("a", 3).is_ok());
    }

    #[test]
    fn fail_throttle_is_capped_and_drops_the_oldest_first() {
        let t = FailThrottle::default();
        for i in 0..100 {
            t.fail_capped(&format!("k{i}"), 100);
        }
        t.fail_capped("new", 100);
        let len = t.map.lock().unwrap().len();
        assert!((85..=100).contains(&len), "{len}");
        assert!(t.map.lock().unwrap().contains_key("new"));
        assert!(!t.map.lock().unwrap().contains_key("k0"));
    }

    #[test]
    fn a_forwarded_for_that_is_not_an_address_list_is_ignored() {
        let trusted = parse_cidrs("172.19.0.4");
        let peer = Some("172.19.0.4".parse::<IpAddr>().unwrap());
        assert_eq!(client_addr(peer, Some("203.0.113.9"), &trusted), "203.0.113.9");
        for bad in ["not-an-ip", "6.6.6.6, junk", "", "203.0.113.9,"] {
            assert_eq!(client_addr(peer, Some(bad), &trusted), "172.19.0.4", "{bad:?}");
        }
    }

    #[test]
    fn allows_the_burst_then_limits_with_a_retry_hint() {
        let rl = RateLimiter::default();
        for _ in 0..3 {
            assert!(rl.check("k", 3).is_ok());
        }
        let wait = rl.check("k", 3).unwrap_err();
        assert!((1..=20).contains(&wait), "{wait}");
        // another key is unaffected, and 0 means unlimited
        assert!(rl.check("other", 3).is_ok());
        for _ in 0..50 {
            assert!(rl.check("free", 0).is_ok());
        }
    }

    #[test]
    fn busy_keys_past_the_prune_size_are_not_rescanned_every_check() {
        let rl = RateLimiter::default();
        for i in 0..=PRUNE_AT {
            rl.check(&format!("k{i}"), 10).unwrap();
        }
        rl.check("one more", 10).unwrap(); // prunes: every bucket is busy, so all stay
        let b = rl.buckets.lock().unwrap();
        assert_eq!(b.map.len(), PRUNE_AT + 2);
        assert_eq!(b.prune_at, 2 * (PRUNE_AT + 1), "the next prune waits for the map to double");
    }

    #[test]
    fn trusted_proxies_split_into_cidrs_and_hostnames() {
        let (cidrs, hosts) = parse_trusted("frontend, 10.0.0.0/8,203.0.113.5 ,proxy.internal,none,bad host!");
        assert_eq!(cidrs.len(), 2);
        assert_eq!(hosts, ["frontend", "proxy.internal"]);
        assert_eq!(parse_trusted("none"), (vec![], vec![]));
    }

    #[test]
    fn a_resolved_hostname_is_trusted_and_a_stale_one_is_not() {
        let ip = |s: &str| Some(s.parse::<IpAddr>().unwrap());
        let resolved: Vec<IpAddr> = vec!["172.19.0.4".parse().unwrap()];
        let trusted = effective_trusted(&parse_trusted("frontend").0, &resolved);
        assert_eq!(client_addr(ip("172.19.0.4"), Some("203.0.113.9"), &trusted), "203.0.113.9");
        // a neighbour on the same bridge is not the frontend
        assert_eq!(client_addr(ip("172.19.0.5"), Some("203.0.113.9"), &trusted), "172.19.0.5");
        // before the first resolution nothing is trusted
        assert_eq!(client_addr(ip("172.19.0.4"), Some("203.0.113.9"), &effective_trusted(&[], &[])), "172.19.0.4");
    }

    #[test]
    fn untrusted_private_forwarders_are_warned_about_once_an_hour() {
        let mut seen = HashMap::new();
        let (peer, other): (IpAddr, IpAddr) = ("172.18.0.5".parse().unwrap(), "172.18.0.6".parse().unwrap());
        let t0 = Instant::now();
        assert!(warn_due(&mut seen, peer, t0));
        assert!(!warn_due(&mut seen, peer, t0 + Duration::from_secs(3599)));
        assert!(warn_due(&mut seen, other, t0 + Duration::from_secs(1)), "per peer");
        assert!(warn_due(&mut seen, peer, t0 + Duration::from_secs(3601)));
        assert!(is_private(peer) && is_private("::1".parse().unwrap()) && is_private("fd00::1".parse().unwrap()));
        assert!(!is_private("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn a_realistic_chain_through_caddy_and_the_frontend_resolves_to_the_client() {
        // Next's rewrite proxy adds no X-Forwarded-For of its own (httpxy `xfwd` is off): the header
        // is what Caddy sent. The peer the backend sees is the frontend container.
        let ip = |s: &str| Some(s.parse::<IpAddr>().unwrap());
        let frontend_only = parse_cidrs("172.19.0.4");
        assert_eq!(client_addr(ip("172.19.0.4"), Some("203.0.113.9"), &frontend_only), "203.0.113.9");
        // if the chain does carry Caddy's address, list Caddy as trusted too, or it is taken for the client
        let both = parse_cidrs("172.19.0.4,172.19.0.2");
        assert_eq!(client_addr(ip("172.19.0.4"), Some("203.0.113.9, 172.19.0.2"), &both), "203.0.113.9");
        assert_eq!(client_addr(ip("172.19.0.4"), Some("6.6.6.6, 203.0.113.9, 172.19.0.2"), &both), "203.0.113.9");
        assert_eq!(client_addr(ip("172.19.0.4"), Some("203.0.113.9, 172.19.0.2"), &frontend_only), "172.19.0.2");
    }

    #[test]
    fn default_trusts_loopback_only() {
        let trusted = RateConfig::default().trusted_proxies;
        let ip = |s: &str| Some(s.parse::<IpAddr>().unwrap());
        // a LAN or bridge peer cannot spoof X-Forwarded-For
        assert_eq!(client_addr(ip("192.168.1.50"), Some("1.2.3.4"), &trusted), "192.168.1.50");
        assert_eq!(client_addr(ip("172.18.0.5"), Some("1.2.3.4"), &trusted), "172.18.0.5");
        assert_eq!(client_addr(ip("127.0.0.1"), Some("1.2.3.4"), &trusted), "1.2.3.4");
    }

    #[test]
    fn forwarded_for_is_honoured_only_from_trusted_peers_right_most_untrusted() {
        let trusted = parse_cidrs("172.16.0.0/12,10.0.0.0/8");
        let ip = |s: &str| Some(s.parse::<IpAddr>().unwrap());
        // a public peer cannot spoof
        assert_eq!(client_addr(ip("8.8.8.8"), Some("1.2.3.4"), &trusted), "8.8.8.8");
        // the frontend proxy (private) vouches for the header
        assert_eq!(client_addr(ip("172.18.0.5"), Some("203.0.113.9"), &trusted), "203.0.113.9");
        // a client-supplied left entry is skipped: right-most untrusted wins
        assert_eq!(client_addr(ip("172.18.0.5"), Some("6.6.6.6, 203.0.113.9, 10.0.0.2"), &trusted), "203.0.113.9");
        assert_eq!(client_addr(ip("172.18.0.5"), None, &trusted), "172.18.0.5");
        assert_eq!(client_addr(None, Some("1.2.3.4"), &trusted), "unknown");
        assert!(Cidr::parse("fc00::/7").unwrap().contains("fd12::1".parse().unwrap()));
        assert!(!Cidr::parse("10.0.0.0/8").unwrap().contains("::1".parse().unwrap()));
    }
}
