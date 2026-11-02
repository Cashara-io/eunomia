//! "Is this machine online?", asked before a scheduled sync so an offline laptop skips its syncs
//! quietly instead of failing every source, recording errors and growing the backoff.
//!
//! The probe is a TCP connect (no DNS, no TLS, no request) to a few well-known anycast addresses, all
//! raced at once with a short timeout: any one that answers means online. It needs no new dependency
//! and sends no data. The result is cached for a short while, and concurrent callers share one probe,
//! so a tick that finds a dozen sources due costs one probe, not a dozen.
//!
//! `EUNOMIA_CONNECTIVITY_PROBE` (`Settings::connectivity_probe`) overrides the targets with a comma
//! separated `host:port` list (for a network that blocks these addresses), or turns probing off with
//! `off` (the machine is then always treated as online). Blank means the built-in targets. The probe
//! does not use an HTTP(S) proxy, so a host that reaches the internet only through one reads as offline:
//! the first "offline" answer (and every flip to it) is logged as a warning that says so.

use std::collections::HashMap;
use std::future::Future;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use tokio::task::JoinSet;

/// Cloudflare, Google and Quad9 resolvers on 443: reachable from nearly any network that is up at all.
pub const DEFAULT_TARGETS: &[&str] = &["1.1.1.1:443", "8.8.8.8:443", "9.9.9.9:443"];
/// How long one probe waits for a connection.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// An "online" answer is reused this long; "offline" for less, so a returning connection is noticed quickly.
const ONLINE_TTL: Duration = Duration::from_secs(30);
const OFFLINE_TTL: Duration = Duration::from_secs(10);

/// What `EUNOMIA_CONNECTIVITY_PROBE` asks for.
#[derive(Debug, PartialEq, Eq)]
enum Probe {
    /// Never probe: always online.
    Off,
    Targets(Vec<String>),
}

fn parse(spec: &str) -> Probe {
    let spec = spec.trim();
    if matches!(spec.to_ascii_lowercase().as_str(), "off" | "false" | "0" | "none" | "disabled") {
        return Probe::Off;
    }
    let targets: Vec<String> = spec.split(',').map(str::trim).filter(|t| !t.is_empty()).map(String::from).collect();
    if targets.is_empty() {
        Probe::Targets(DEFAULT_TARGETS.iter().map(|t| t.to_string()).collect())
    } else {
        Probe::Targets(targets)
    }
}

/// True if any target accepts a TCP connection within [`PROBE_TIMEOUT`].
async fn tcp_probe(targets: Vec<String>) -> bool {
    let mut set = JoinSet::new();
    for target in targets {
        set.spawn(async move { tokio::time::timeout(PROBE_TIMEOUT, TcpStream::connect(target)).await.is_ok_and(|r| r.is_ok()) });
    }
    while let Some(done) = set.join_next().await {
        if matches!(done, Ok(true)) {
            return true; // dropping the set aborts the probes still waiting
        }
    }
    false
}

/// Last answer per probe spec. The lock is held across a probe, so callers arriving meanwhile wait
/// for it and then read its answer instead of probing again.
#[derive(Default)]
struct Cache {
    seen: tokio::sync::Mutex<HashMap<String, (Instant, bool)>>,
}

impl Cache {
    async fn get<F, Fut>(&self, key: &str, fresh: bool, probe: F) -> bool
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = bool>,
    {
        let mut seen = self.seen.lock().await;
        let before = seen.get(key).copied();
        if !fresh && let Some((at, online)) = before && at.elapsed() < if online { ONLINE_TTL } else { OFFLINE_TTL } {
            return online;
        }
        let online = probe().await;
        seen.insert(key.to_string(), (Instant::now(), online));
        match (before.map(|(_, was)| was), online) {
            (Some(true) | None, false) => tracing::warn!(
                "no internet connection found: scheduled syncs are paused until it returns. If this machine is online, it likely \
                 reaches the internet only through an HTTP(S) proxy or blocks direct connections to the probe addresses: set \
                 EUNOMIA_CONNECTIVITY_PROBE to a host:port it can reach (or `off`), or turn off \"Skip syncs while offline\" in Settings"
            ),
            (Some(false), true) => tracing::info!("internet connection is back: scheduled syncs resume"),
            _ => {}
        }
        online
    }
}

static CACHE: LazyLock<Cache> = LazyLock::new(Cache::default);

/// Whether the machine can reach the internet, per the probe `spec` (`Settings::connectivity_probe`).
/// `fresh` ignores the cached answer (a manual "Sync now" wants the truth, not 30-second-old news).
pub async fn is_online(spec: &str, fresh: bool) -> bool {
    match parse(spec) {
        Probe::Off => true,
        Probe::Targets(targets) => CACHE.get(spec.trim(), fresh, || tcp_probe(targets)).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn parse_reads_off_targets_and_blank() {
        assert_eq!(parse("off"), Probe::Off);
        assert_eq!(parse(" OFF "), Probe::Off);
        assert_eq!(parse("a.example:443, b.example:80 ,"), Probe::Targets(vec!["a.example:443".into(), "b.example:80".into()]));
        let Probe::Targets(t) = parse("") else { panic!("blank means the built-in targets") };
        assert_eq!(t.len(), DEFAULT_TARGETS.len());
    }

    #[tokio::test]
    async fn the_cache_reuses_an_answer_and_a_fresh_ask_probes_again() {
        let cache = Cache::default();
        let calls = AtomicUsize::new(0);
        let probe = |answer: bool| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move { answer }
        };
        assert!(cache.get("k", false, || probe(true)).await);
        assert!(cache.get("k", false, || probe(false)).await, "still the cached answer");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!cache.get("k", true, || probe(false)).await, "fresh probes again");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(!cache.get("k", false, || probe(true)).await, "and the new answer is what is cached");
        assert!(cache.get("other", false, || probe(true)).await, "specs are cached apart");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn concurrent_callers_share_one_probe() {
        let cache = Cache::default();
        let calls = AtomicUsize::new(0);
        let ask = || {
            cache.get("k", false, || {
                calls.fetch_add(1, Ordering::SeqCst);
                async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    false
                }
            })
        };
        let (a, b, c) = tokio::join!(ask(), ask(), ask());
        assert!(!a && !b && !c);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn tcp_probe_is_true_when_one_target_answers_and_false_when_none_do() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let open = listener.local_addr().unwrap().to_string();
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().to_string() // released again: nothing listens there
        };
        assert!(tcp_probe(vec![closed.clone(), open]).await);
        assert!(!tcp_probe(vec![closed]).await);
        assert!(!tcp_probe(vec![]).await);
    }
}
