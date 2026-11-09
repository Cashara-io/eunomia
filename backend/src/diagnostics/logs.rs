//! Fixed templates and event-specific bounded integers only. Never retain formatted values.
use std::{collections::VecDeque, sync::{Arc, LazyLock, Mutex, atomic::{AtomicU64, Ordering}, mpsc::{sync_channel, SyncSender}}};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tracing::{field::{Field, Visit}, Event, Subscriber};
use tracing_subscriber::{layer::Context, Layer};

const MAX_LINES: usize = 5000;
const MAX_BYTES: usize = 5 * 1024 * 1024;
#[derive(Deserialize)]
struct EventSpec { target: String, message: String, fields: std::collections::HashMap<String, [u64; 2]> }
static EVENTS: LazyLock<std::collections::HashMap<String, EventSpec>> = LazyLock::new(|| serde_json::from_str(include_str!("events.json")).expect("static diagnostics event schema"));

#[derive(Default)]
pub struct Ring { lines: VecDeque<Arc<str>>, bytes: usize }
impl Ring {
    fn push(&mut self, line: Arc<str>) {
        if line.len() > MAX_BYTES { return; }
        self.bytes += line.len();
        self.lines.push_back(line);
        while self.lines.len() > MAX_LINES || self.bytes > MAX_BYTES {
            if let Some(old) = self.lines.pop_front() { self.bytes -= old.len(); }
        }
    }
}
#[derive(Default)]
pub struct LogBuffer { ring: Mutex<Ring>, dropped: AtomicU64 }
impl LogBuffer {
    fn push(&self, line: Arc<str>) {
        // Construction and privacy filtering happen before this short, nonblocking section.
        if let Ok(mut ring) = self.ring.try_lock() { ring.push(line); }
        else { self.dropped.fetch_add(1, Ordering::Relaxed); }
    }
    pub fn snapshot(&self) -> String {
        // Clone references only. Formatting and copying bytes happen after releasing the lock.
        let lines = self.ring.try_lock().ok().map(|ring| ring.lines.iter().cloned().collect::<Vec<_>>());
        let mut text = format!("{}\n", json!({"dropped_events": self.dropped.load(Ordering::Relaxed), "snapshot_unavailable": lines.is_none(), "stdout_dropped_events": STDOUT_DROPPED.load(Ordering::Relaxed)}));
        if let Some(lines) = lines { for line in lines { text.push_str(&line); } }
        text
    }
}
pub static LOGS: LazyLock<Arc<LogBuffer>> = LazyLock::new(|| Arc::new(LogBuffer::default()));
static STDOUT_DROPPED: AtomicU64 = AtomicU64::new(0);
// A blocked pipe can block only this writer thread. Application threads never write to stdout.
static STDOUT: LazyLock<SyncSender<Arc<str>>> = LazyLock::new(|| {
    let (tx, rx) = sync_channel::<Arc<str>>(1024);
    std::thread::spawn(move || {
        use std::io::Write;
        for line in rx { let _ = std::io::stdout().lock().write_all(line.as_bytes()); }
    });
    tx
});

struct Measurements<'a> { spec: Option<&'a EventSpec>, values: Map<String, Value> }
impl Visit for Measurements<'_> {
    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
    fn record_str(&mut self, _: &Field, _: &str) {}
    fn record_u64(&mut self, field: &Field, value: u64) {
        if let Some([low, high]) = self.spec.and_then(|s| s.fields.get(field.name()))
            && (*low..=*high).contains(&value) { self.values.insert(field.name().into(), value.into()); }
    }
    fn record_i64(&mut self, field: &Field, value: i64) { if let Ok(value) = u64::try_from(value) { self.record_u64(field, value); } }
    fn record_bool(&mut self, _: &Field, _: bool) {}
    fn record_f64(&mut self, _: &Field, _: f64) {}
}

pub struct SafeLogs { pub ring: Arc<LogBuffer>, pub stdout: bool }
impl<S: Subscriber> Layer<S> for SafeLogs {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let meta = event.metadata();
        if !meta.target().starts_with("eunomia_backend::") { return; }
        let spec = EVENTS.get(meta.name()).filter(|s| s.target == meta.target());
        let mut measurements = Measurements { spec, values: Map::new() };
        event.record(&mut measurements);
        let value = json!({"timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true), "level": meta.level().as_str(),
            "name": spec.map(|_| meta.name()), "target": spec.map_or("<redacted>", |s| s.target.as_str()),
            "message": spec.map_or("<redacted>", |s| s.message.as_str()), "fields": measurements.values});
        let line: Arc<str> = format!("{value}\n").into();
        self.ring.push(line.clone());
        if self.stdout && STDOUT.try_send(line).is_err() { STDOUT_DROPPED.fetch_add(1, Ordering::Relaxed); }
    }
}

/// Revalidate updater output too, including output from older installed collectors.
pub fn filter_container(text: &str) -> String {
    text.lines().take(300).map(|line| {
        let event: Value = serde_json::from_str(line).unwrap_or(Value::Null);
        let spec = event["name"].as_str().and_then(|name| EVENTS.get(name))
            .filter(|spec| event["target"].as_str() == Some(&spec.target));
        let mut fields = Map::new();
        if let Some(spec) = spec {
            for (key, [low, high]) in &spec.fields {
                if let Some(value) = event["fields"][key].as_u64().filter(|n| (*low..=*high).contains(n)) { fields.insert(key.clone(), value.into()); }
            }
        }
        let level = event["level"].as_str().filter(|level| ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"].contains(level)).unwrap_or("unknown");
        let timestamp = event["timestamp"].as_str().and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .filter(|t| { use chrono::Datelike; (2000..=2100).contains(&t.year()) })
            .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        format!("{}\n", json!({"timestamp": timestamp, "level": level,
            "name": spec.and_then(|_| event["name"].as_str()), "target": spec.map_or("<redacted>", |s| &s.target),
            "message": spec.map_or("<redacted>", |s| &s.message), "fields": fields}))
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::prelude::*;
    #[test]
    fn bounded_by_lines_and_bytes() {
        let mut ring = Ring::default();
        for _ in 0..6000 { ring.push("line\n".into()); }
        assert_eq!(ring.lines.len(), MAX_LINES);
        for _ in 0..6000 { ring.push("x ".repeat(1000).into()); }
        assert!(ring.bytes <= MAX_BYTES);
    }
    #[test]
    fn content_and_numeric_secrets_never_enter_ring_or_zip() {
        let ring = Arc::new(LogBuffer::default());
        let subscriber = tracing_subscriber::registry().with(SafeLogs { ring: ring.clone(), stdout: false });
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(count = 314159265358979323_u64, duration_ms = 4111111111111111_u64, status = 61412345678_u64, "provider error: {}", "{\"count\":314159265358979323846}");
            tracing::warn!(count = 8675309_u64, phone = "+61412345678", account = "1234567890123456789012345678901234567890", "numeric .env value");
            tracing::info!(name: "diagnostics.worker_started", target: "eunomia_backend::jobs::worker", concurrency = 4_u64, count = 1234_u64, "PRIVATE_CANARY");
            tracing::info!(name: "diagnostics.worker_stopped", target: "eunomia_backend::jobs::worker", released = 4111111111111111_u64, "PRIVATE_CANARY");
        });
        let text = ring.snapshot();
        assert!(text.contains("job worker started") && text.contains("\"concurrency\":4"));
        let (zip, _) = super::super::archive::build(vec![("backend.log".into(), text)], 8192).unwrap();
        let bytes = std::fs::read(zip.path()).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        for secret in ["314159265358979323", "4111111111111111", "61412345678", "8675309", "1234", "1234567890123456789012345678901234567890", "PRIVATE_CANARY"] { assert!(!text.contains(secret), "secret survived: {secret}"); }
    }
    #[test]
    fn logging_drops_instead_of_waiting_for_snapshot_lock() {
        let ring = Arc::new(LogBuffer::default());
        let held = ring.ring.lock().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_ring = ring.clone();
        let worker = std::thread::spawn(move || {
            let subscriber = tracing_subscriber::registry().with(SafeLogs { ring: worker_ring, stdout: false });
            tracing::subscriber::with_default(subscriber, || { for _ in 0..1000 { tracing::info!("event"); } });
            tx.send(()).unwrap();
        });
        let finished = rx.recv_timeout(std::time::Duration::from_secs(2));
        drop(held); // Also unblock a mutated implementation before asserting.
        worker.join().unwrap();
        assert!(finished.is_ok(), "application logger blocked on the ring mutex");
        assert_eq!(ring.dropped.load(Ordering::Relaxed), 1000);
        assert!(ring.snapshot().contains("dropped_events"));
    }
}
