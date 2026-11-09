//! Only code locations and numeric measurements enter logs. No formatted messages or span values.
use std::{collections::VecDeque, sync::{Arc, LazyLock, Mutex}};
use serde_json::{json, Map, Value};
use tracing::{field::{Field, Visit}, Event, Subscriber};
use tracing_subscriber::{layer::Context, Layer};

const MAX_LINES: usize = 5000;
const MAX_BYTES: usize = 5 * 1024 * 1024;
#[derive(Default)]
pub struct Ring { lines: VecDeque<String>, bytes: usize }
impl Ring {
    pub fn push(&mut self, line: &str) {
        let line = super::privacy::scrub(line);
        if line.len() > MAX_BYTES { return; }
        self.bytes += line.len();
        self.lines.push_back(line);
        while self.lines.len() > MAX_LINES || self.bytes > MAX_BYTES {
            if let Some(old) = self.lines.pop_front() { self.bytes -= old.len(); }
        }
    }
    pub fn snapshot(&self) -> String { self.lines.iter().cloned().collect() }
}
pub static LOGS: LazyLock<Arc<Mutex<Ring>>> = LazyLock::new(|| Arc::new(Mutex::new(Ring::default())));

fn numeric(field: &Field) -> bool {
    ["status", "latency_ms", "duration_ms", "count", "attempts", "written", "failed", "skipped", "jobs", "released", "concurrency"].contains(&field.name())
}
#[derive(Default)]
struct Measurements(Map<String, Value>);
impl Visit for Measurements {
    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
    fn record_str(&mut self, _: &Field, _: &str) {}
    fn record_u64(&mut self, field: &Field, value: u64) { if numeric(field) { self.0.insert(field.name().into(), value.into()); } }
    fn record_i64(&mut self, field: &Field, value: i64) { if numeric(field) { self.0.insert(field.name().into(), value.into()); } }
    fn record_bool(&mut self, field: &Field, value: bool) { if numeric(field) { self.0.insert(field.name().into(), value.into()); } }
    fn record_f64(&mut self, field: &Field, value: f64) { if numeric(field) { self.0.insert(field.name().into(), json!(value)); } }
}

pub struct SafeLogs {
    pub ring: Arc<Mutex<Ring>>,
    pub stdout: bool,
}
impl<S: Subscriber> Layer<S> for SafeLogs {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        // Dependency log targets may be influenced by remote peers. Only our compile-time targets.
        let meta = event.metadata();
        if !meta.target().starts_with("eunomia_backend") { return; }
        let mut measurements = Measurements::default();
        event.record(&mut measurements);
        let mut value = json!({"timestamp": chrono::Utc::now().to_rfc3339(), "level": meta.level().as_str(),
            "target": meta.target(), "file": meta.file(), "line": meta.line(), "fields": measurements.0});
        super::privacy::scrub_value(&mut value);
        let line = format!("{value}\n");
        if let Ok(mut ring) = self.ring.lock() { ring.push(&line); }
        if self.stdout {
            use std::io::Write;
            let _ = std::io::stdout().lock().write_all(line.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::prelude::*;
    #[test]
    fn bounded_by_lines_and_bytes() {
        let mut ring = Ring::default();
        for _ in 0..6000 { ring.push("line\n"); }
        assert_eq!(ring.lines.len(), MAX_LINES);
        for _ in 0..6000 { ring.push(&"x ".repeat(1000)); }
        assert!(ring.bytes <= MAX_BYTES);
    }
    #[test]
    fn content_and_secrets_never_enter_ring() {
        let ring = Arc::new(Mutex::new(Ring::default()));
        let subscriber = tracing_subscriber::registry().with(SafeLogs { ring: ring.clone(), stdout: false });
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("request", text = "MEMORY_CANARY", document = "DOCUMENT_CANARY");
            let _entered = span.enter();
            tracing::warn!(body = "CHAT_CANARY", error = "password=SECRET_CANARY", count = 3, "provider error: {}", "MEMORY_CANARY");
        });
        let text = ring.lock().unwrap().snapshot();
        assert!(text.contains("WARN") && text.contains("count"));
        assert!(!text.contains("CANARY"));
    }
}
