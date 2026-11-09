//! Demo source -- pushes synthetic, clearly `demo`-sourced Up Bank +
//! heypocket data through the ingest pipeline so the record tools can be
//! exercised with no live accounts (the e2e suite seeds with
//! `POST /api/sources/demo/sync`). It is not a connector: it is never listed
//! in the UI or scheduled (see `registry::get`).
//!
//! Synthetic records are generated inline. The fixed seed (42) drives
//! `StdRng`, so the generated rows are deterministic within this backend.

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::{json, Value};

use crate::error::AppResult;
use crate::sources::base::{Conn, Source, SyncResult};

const ACCOUNTS: [&str; 2] = ["Spending", "Saver"];

const CATEGORIES: [(&str, &[&str]); 6] = [
    ("Groceries", &["Woolworths", "Coles", "Aldi", "IGA"]),
    ("Transport", &["Uber", "Opal", "Shell", "BP"]),
    ("Dining out", &["Guzman y Gomez", "Corner Cafe", "Deliveroo", "Menulog"]),
    ("Subscriptions", &["Netflix", "Spotify", "iCloud", "GitHub"]),
    ("Shopping", &["Amazon", "Kmart", "Bunnings", "Officeworks"]),
    ("Entertainment", &["Ticketek", "Steam", "Event Cinemas"]),
];

const POCKET_TITLES: [&str; 10] = [
    "Weekly standup",
    "1:1 with manager",
    "Client call · Acme Corp",
    "Sprint planning",
    "Design review",
    "Onboarding call",
    "Product sync",
    "Retro",
    "All-hands",
    "Customer interview",
];
const POCKET_TAGS: [&str; 4] = ["work", "client", "internal", "personal"];

fn gen_transactions(rng: &mut StdRng, now: DateTime<Utc>) -> Vec<Value> {
    let mut rows = Vec::new();
    for i in 0..55 {
        let (category, merchants) = CATEGORIES[rng.gen_range(0..CATEGORIES.len())];
        let merchant = merchants[rng.gen_range(0..merchants.len())];
        // weights 85/15 for Spending/Saver
        let account = if rng.gen_range(0..100) < 85 { ACCOUNTS[0] } else { ACCOUNTS[1] };
        let created_at = now - Duration::days(rng.gen_range(0..=45)) - Duration::hours(rng.gen_range(0..24));
        rows.push(json!({
            "_kind": "txn",
            "id": format!("txn{i}"),
            "account": account,
            "description": merchant,
            "category": category,
            "amount_cents": -rng.gen_range(500..=12000),
            "created_at": created_at.to_rfc3339(),
        }));
    }
    for (i, day) in [3i64, 17, 31].into_iter().enumerate() {
        rows.push(json!({
            "_kind": "txn",
            "id": format!("salary{i}"),
            "account": "Spending",
            "description": "Salary",
            "category": "Income",
            "amount_cents": rng.gen_range(250000..=400000),
            "created_at": (now - Duration::days(day)).to_rfc3339(),
        }));
    }
    rows.push(json!({
        "_kind": "txn",
        "id": "transfer0",
        "account": "Saver",
        "description": "Transfer from Spending",
        "category": "Transfer",
        "amount_cents": rng.gen_range(100000..=300000),
        "created_at": (now - Duration::days(rng.gen_range(30..=44))).to_rfc3339(),
    }));
    rows
}

fn gen_recordings(rng: &mut StdRng, now: DateTime<Utc>) -> Vec<Value> {
    (0..16)
        .map(|i| {
            let title = POCKET_TITLES[rng.gen_range(0..POCKET_TITLES.len())];
            let n_tags = rng.gen_range(1..=2);
            let mut tags: Vec<&str> = POCKET_TAGS.to_vec();
            // Fisher-Yates partial shuffle to sample `n_tags` without
            // replacement.
            for i in 0..n_tags {
                let j = rng.gen_range(i..tags.len());
                tags.swap(i, j);
            }
            let recorded_at = now - Duration::days(rng.gen_range(0..=30)) - Duration::hours(rng.gen_range(0..24));
            json!({
                "_kind": "rec",
                "id": format!("rec{i}"),
                "title": title,
                "duration_seconds": rng.gen_range(600..=3600),
                "tags": tags[..n_tags],
                "recorded_at": recorded_at.to_rfc3339(),
            })
        })
        .collect()
}

#[allow(clippy::too_many_arguments)] // flat constructor for demo fixtures
fn env(
    id: String,
    source: &str,
    type_: &str,
    external_id: String,
    title: String,
    body: String,
    occurred: Option<String>,
    payload: Value,
) -> Value {
    json!({
        "id": id,
        "source": source,
        "type": type_,
        "external_id": external_id,
        "title": title,
        "body_text": body,
        "occurred_at": occurred,
        "url": "",
        "payload": payload,
        "links": [],
        "deleted": false,
    })
}

pub struct DemoSource;

#[async_trait]
impl Source for DemoSource {
    fn key(&self) -> &'static str {
        "demo"
    }

    fn provider(&self) -> &'static str {
        "demo"
    }

    fn label(&self) -> &'static str {
        "Demo data"
    }

    fn record_types(&self) -> &'static [&'static str] {
        &["up.transaction", "up.account", "heypocket.recording"]
    }

    async fn check(&self, _conn: &Conn) -> AppResult<()> {
        Ok(())
    }

    async fn fetch(&self, _conn: &Conn, _cursor: Option<String>) -> AppResult<SyncResult> {
        let mut rng = StdRng::seed_from_u64(42);
        let now = Utc::now();

        let mut records = gen_transactions(&mut rng, now);

        let mut accounts: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
        for t in &records {
            let account = t["account"].as_str().unwrap().to_string();
            let cents = t["amount_cents"].as_i64().unwrap();
            *accounts.entry(account).or_insert(0) += cents;
        }
        for (name, cents) in accounts {
            records.push(json!({"_kind": "acct", "name": name, "cents": cents}));
        }
        records.extend(gen_recordings(&mut rng, now));

        Ok(SyncResult { records, cursor: Some("demo".to_string()), ..SyncResult::default() })
    }

    fn map(&self, raw: &Value) -> Option<Value> {
        match raw.get("_kind").and_then(|v| v.as_str())? {
            "txn" => {
                let description = raw["description"].as_str().unwrap_or("").to_string();
                let account = raw["account"].as_str().unwrap_or("").to_string();
                let amount_cents = raw["amount_cents"].as_i64().unwrap_or(0);
                Some(env(
                    format!("demo:up.transaction:{}", raw["id"].as_str().unwrap_or("")),
                    "demo",
                    "up.transaction",
                    raw["id"].as_str().unwrap_or("").to_string(),
                    description.clone(),
                    format!("{description} at {account}"),
                    raw.get("created_at").and_then(|v| v.as_str()).map(String::from),
                    json!({
                        "amount_cents": amount_cents,
                        "amount": format!("{:.2}", amount_cents as f64 / 100.0),
                        "currency": "AUD",
                        "status": "SETTLED",
                        "category": raw["category"],
                        "is_income": amount_cents > 0,
                    }),
                ))
            }
            "acct" => {
                let name = raw["name"].as_str().unwrap_or("").to_string();
                let cents = raw["cents"].as_i64().unwrap_or(0);
                Some(env(
                    format!("demo:up.account:{name}"),
                    "demo",
                    "up.account",
                    name.clone(),
                    name.clone(),
                    format!("{name} account"),
                    None,
                    json!({
                        "balance_cents": cents,
                        "balance": format!("{:.2}", cents as f64 / 100.0),
                    }),
                ))
            }
            "rec" => {
                let title = raw["title"].as_str().unwrap_or("").to_string();
                Some(env(
                    format!("demo:heypocket.recording:{}", raw["id"].as_str().unwrap_or("")),
                    "demo",
                    "heypocket.recording",
                    raw["id"].as_str().unwrap_or("").to_string(),
                    title.clone(),
                    title,
                    raw.get("recorded_at").and_then(|v| v.as_str()).map(String::from),
                    json!({
                        "duration_seconds": raw["duration_seconds"],
                        "tags": raw["tags"],
                    }),
                ))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_returns_none_for_unknown_kind() {
        let src = DemoSource;
        assert!(src.map(&json!({"_kind": "bogus"})).is_none());
    }

    #[test]
    fn map_txn_sets_is_income_from_amount_sign() {
        let src = DemoSource;
        let raw = json!({
            "_kind": "txn", "id": "txn1", "account": "Spending", "description": "Coles",
            "category": "Groceries", "amount_cents": -2500, "created_at": "2024-01-01T00:00:00+00:00",
        });
        let env = src.map(&raw).unwrap();
        assert_eq!(env["id"], "demo:up.transaction:txn1");
        assert_eq!(env["payload"]["is_income"], false);
        assert_eq!(env["payload"]["amount"], "-25.00");

        let income = src
            .map(&json!({
                "_kind": "txn", "id": "salary0", "account": "Spending", "description": "Salary",
                "category": "Income", "amount_cents": 300000, "created_at": "2024-01-01T00:00:00+00:00",
            }))
            .unwrap();
        assert_eq!(income["payload"]["is_income"], true);
    }

    #[test]
    fn map_acct_has_no_occurred_at() {
        let src = DemoSource;
        let env = src.map(&json!({"_kind": "acct", "name": "Saver", "cents": 150000})).unwrap();
        assert_eq!(env["id"], "demo:up.account:Saver");
        assert!(env["occurred_at"].is_null());
        assert_eq!(env["payload"]["balance"], "1500.00");
    }

    #[test]
    fn sync_is_deterministic_for_a_fixed_seed() {
        let mut rng_a = StdRng::seed_from_u64(42);
        let mut rng_b = StdRng::seed_from_u64(42);
        let now = Utc::now();
        let a = gen_transactions(&mut rng_a, now);
        let b = gen_transactions(&mut rng_b, now);
        assert_eq!(a, b);
    }

    #[test]
    fn gen_recordings_samples_one_or_two_unique_tags() {
        let mut rng = StdRng::seed_from_u64(1);
        let now = Utc::now();
        for rec in gen_recordings(&mut rng, now) {
            let tags = rec["tags"].as_array().unwrap();
            assert!(tags.len() == 1 || tags.len() == 2);
            let unique: std::collections::HashSet<_> = tags.iter().collect();
            assert_eq!(unique.len(), tags.len());
        }
    }
}
