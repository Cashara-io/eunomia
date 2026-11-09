use std::{collections::VecDeque, sync::{LazyLock, Mutex}, time::Instant};
use serde_json::{json, Value};
static HISTORY: LazyLock<Mutex<VecDeque<(String, Value)>>> = LazyLock::new(|| Mutex::new(VecDeque::new()));
pub fn record(org: &str, owner: &str, source: &str, start: Instant, report: &Value) {
    let key = super::opaque(&format!("{org}/{owner}/{source}"));
    let error = report["error"].as_str().unwrap_or("");
    let value = json!({"at": chrono::Utc::now().to_rfc3339(), "duration_ms": start.elapsed().as_millis() as u64,
        "written": report["written"].as_u64(), "skipped": report["skipped"].as_u64(), "failed": report["failed"].as_u64(),
        "partial": report["failed"].as_u64().unwrap_or(0) > 0, "error": super::privacy::error_summary(error),
        "rate_limited": error.contains("429") || error.to_lowercase().contains("rate limit")});
    if let Ok(mut history) = HISTORY.lock() {
        history.push_back((key, value));
        while history.len() > 2000 { history.pop_front(); }
    }
}
pub fn recent(key: &str) -> Vec<Value> {
    HISTORY.lock().map(|h| h.iter().rev().filter(|(k, _)| k == key).take(20).map(|(_, v)| v.clone()).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sync_history_contains_counts_and_categories_without_content() {
        let org = "history-test-org";
        let owner = "user:history-test";
        let source = "gmail";
        record(org, owner, source, Instant::now(), &json!({"written": 3, "failed": 1, "error": "429 MEMORY_CANARY person@example.com password=SECRET_CANARY"}));
        let values = recent(&super::super::opaque(&format!("{org}/{owner}/{source}")));
        assert_eq!(values[0]["written"], 3);
        assert_eq!(values[0]["partial"], true);
        assert_eq!(values[0]["rate_limited"], true);
        assert!(values[0]["duration_ms"].is_number());
        assert!(!values[0].to_string().contains("CANARY"));
    }
}
