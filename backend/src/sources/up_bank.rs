//! Up Bank source (https://developer.up.com.au). Personal access token,
//! bearer auth.
//!
//! Sync: `filter[since]` delta walk over `/transactions` (following JSON:API
//! `links.next` pagination) plus a full `/accounts` and `/categories` pull.
//! Transactions link to their account and category records.
//!
//! Webhook: verify `X-Up-Authenticity-Signature` (HMAC-SHA256 of the raw body
//! keyed by the webhook secret), then re-fetch the referenced transaction.
//!
//! The tool helpers (`finance_summary`, `list_transactions`, `list_accounts`)
//! read `cache_record` directly.

use surrealdb::types::SurrealValue;
use async_trait::async_trait;
use chrono::{Duration, Utc};
use hmac::{Hmac, Mac};
use reqwest::header::HeaderMap;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;

use crate::connectors::clients::UpBankClient;
use crate::error::AppResult;
use crate::store;
use crate::sources::base::{datetime_to_chrono, envelope, items, rfc3339, s, Conn, Source, SourceCtx, SyncResult, MAX_PAGES};

type HmacSha256 = Hmac<Sha256>;

pub struct UpBankSource;

#[async_trait]
impl Source for UpBankSource {
    fn key(&self) -> &'static str {
        "up_bank"
    }

    fn label(&self) -> &'static str {
        "Up Bank"
    }

    fn record_types(&self) -> &'static [&'static str] {
        &["up.transaction", "up.account", "up.category"]
    }

    async fn check(&self, conn: &Conn) -> AppResult<()> {
        UpBankClient::new(&conn.credentials, &conn.config)?.ping().await
    }

    async fn fetch(&self, conn: &Conn, cursor: Option<String>) -> AppResult<SyncResult> {
        let client = UpBankClient::new(&conn.credentials, &conn.config)?;
        // `filter[since]` keys on createdAt, so a HELD -> SETTLED change can
        // land behind the high-water mark: re-read a week of overlap (the
        // ingest skips unchanged rows).
        let since = cursor
            .as_deref()
            .and_then(|c| chrono::DateTime::parse_from_rfc3339(c).ok())
            .map(|c| (c.with_timezone(&Utc) - Duration::days(7)).to_rfc3339())
            .unwrap_or_else(|| (Utc::now() - Duration::days(30)).to_rfc3339());
        let mut records: Vec<Value> = Vec::new();

        let mut page = client.transactions(&[("filter[since]", since.clone()), ("page[size]", "100".to_string())]).await?;
        records.extend(items(&page, "/data"));
        for _ in 1..MAX_PAGES {
            let Some(next) = page.pointer("/links/next").and_then(|v| v.as_str()).map(String::from) else { break };
            page = client.api.get(&next, &[]).await?;
            records.extend(items(&page, "/data"));
        }
        records.extend(items(&client.accounts().await?, "/data"));
        records.extend(items(&client.categories().await?, "/data"));

        let newest = records
            .iter()
            .filter(|r| r.get("type").and_then(|t| t.as_str()) == Some("transactions"))
            .filter_map(|r| r.pointer("/attributes/createdAt").and_then(|v| v.as_str()))
            .max()
            .map(String::from)
            .or(cursor)
            .unwrap_or(since);

        Ok(SyncResult { records, cursor: Some(newest), ..SyncResult::default() })
    }

    fn map(&self, raw: &Value) -> Option<Value> {
        match raw.get("type").and_then(|v| v.as_str())? {
            "transactions" => Some(map_txn(raw)),
            "accounts" => Some(map_account(raw)),
            "categories" => Some(map_category(raw)),
            _ => None,
        }
    }

    fn has_webhook(&self) -> bool {
        true
    }

    async fn webhook(&self, conn: &Conn, headers: &HeaderMap, body: &[u8]) -> AppResult<Option<Vec<Value>>> {
        let secret = conn.credentials.get("webhook_secret_key").and_then(|v| v.as_str()).unwrap_or("");
        let sig = headers
            .get("X-Up-Authenticity-Signature")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if secret.is_empty() || !verify_signature(secret, body, sig) {
            return Ok(None);
        }

        let Ok(payload) = serde_json::from_slice::<Value>(body) else { return Ok(None) };
        let event = payload.get("data").cloned().unwrap_or(json!({}));
        let etype = event.pointer("/attributes/eventType").and_then(|v| v.as_str()).unwrap_or("");
        let txn = event.pointer("/relationships/transaction").cloned().unwrap_or(json!({}));
        let txn_id = txn.pointer("/data/id").and_then(|v| v.as_str());

        if etype == "TRANSACTION_DELETED" {
            if let Some(txn_id) = txn_id {
                let now = Utc::now().to_rfc3339();
                return Ok(Some(vec![json!({
                    "type": "transactions",
                    "id": txn_id,
                    "relationships": {},
                    "attributes": {
                        "description": "",
                        "createdAt": now,
                        "amount": {"value": "0", "valueInBaseUnits": 0, "currencyCode": "AUD"},
                    },
                    "_deleted": true,
                })]));
            }
            return Ok(None);
        }

        let Some(rel) = txn.pointer("/links/related").and_then(|v| v.as_str()) else { return Ok(None) };
        let client = UpBankClient::new(&conn.credentials, &conn.config)?;
        let resp = client.api.get(rel, &[]).await?;
        match resp.get("data").cloned() {
            Some(data) if !data.is_null() => Ok(Some(vec![data])),
            _ => Ok(None),
        }
    }
}

/// Constant-time HMAC-SHA256 verification of a hex-encoded signature.
fn verify_signature(secret: &str, body: &[u8], sig_hex: &str) -> bool {
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else { return false };
    mac.update(body);
    let Ok(expected) = hex::decode(sig_hex) else { return false };
    mac.verify_slice(&expected).is_ok()
}

fn map_txn(raw: &Value) -> Value {
    let a = &raw["attributes"];
    let id = s(raw, "/id");
    let cat = raw.pointer("/relationships/category/data/id").and_then(|v| v.as_str());
    let account = raw.pointer("/relationships/account/data/id").and_then(|v| v.as_str());
    let parts: Vec<&str> = [a.get("description"), a.get("rawText"), a.get("message")]
        .into_iter()
        .filter_map(|v| v.and_then(|v| v.as_str()))
        .filter(|s| !s.is_empty())
        .collect();
    let value_base_units = a.pointer("/amount/valueInBaseUnits").and_then(|v| v.as_i64()).unwrap_or(0);
    let mut env = envelope(
        "up_bank",
        "up.transaction",
        id,
        s(a, "/description"),
        &parts.join(" "),
        rfc3339(s(a, "/createdAt")),
        "",
        json!({
            "amount": a.pointer("/amount/value"),
            "amount_cents": value_base_units,
            "currency": a.pointer("/amount/currencyCode"),
            "status": a.get("status"),
            "settled_at": a.get("settledAt"),
            "category": cat,
            "account": account,
            "is_income": value_base_units > 0,
        }),
    );
    let mut links = Vec::new();
    if let Some(account) = account {
        links.push(json!({"target": format!("up_bank:up.account:{account}"), "rel": "account"}));
    }
    if let Some(cat) = cat {
        links.push(json!({"target": format!("up_bank:up.category:{cat}"), "rel": "category"}));
    }
    env["links"] = json!(links);
    env["deleted"] = json!(raw.get("_deleted").and_then(|v| v.as_bool()).unwrap_or(false));
    env
}

fn map_account(raw: &Value) -> Value {
    let a = &raw["attributes"];
    let display_name = s(a, "/displayName");
    envelope(
        "up_bank",
        "up.account",
        s(raw, "/id"),
        display_name,
        &format!("{display_name} · {}", s(a, "/accountType")),
        rfc3339(s(a, "/createdAt")),
        "",
        json!({
            "balance": a.pointer("/balance/value"),
            "balance_cents": a.pointer("/balance/valueInBaseUnits"),
            "account_type": a.get("accountType"),
            "ownership_type": a.get("ownershipType"),
        }),
    )
}

fn map_category(raw: &Value) -> Value {
    let name = s(raw, "/attributes/name");
    let parent = raw.pointer("/relationships/parent/data/id").and_then(|v| v.as_str());
    envelope("up_bank", "up.category", s(raw, "/id"), name, name, None, "", json!({"parent": parent}))
}

// ---------------------------------------------------------------------------
// Tool helpers (read from the cache, not the live API)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, SurrealValue, Clone)]
pub(crate) struct CachedRecord {
    #[serde(default)]
    #[surreal(default)]
    title: String,
    #[serde(default)]
    #[surreal(default)]
    external_id: String,
    #[serde(default)]
    #[surreal(default)]
    occurred_at: Option<surrealdb::types::Datetime>,
    #[serde(default)]
    #[surreal(default)]
    payload: Value,
}

fn iso(dt: &Option<surrealdb::types::Datetime>) -> Option<String> {
    dt.as_ref().and_then(datetime_to_chrono).map(|d| d.to_rfc3339())
}

async fn cached_by_type(ctx: &SourceCtx<'_>, type_: &str, limit: i64) -> AppResult<Vec<CachedRecord>> {
    let mut res = store::app::SOURCES_UP_BY_TYPE
        .on(ctx.db)
        .bind(("owner", ctx.owner.clone()))
        .bind(("type", type_.to_string()))
        .bind(("limit", limit))
        .await?;
    Ok(res.take(0)?)
}

async fn category_names(ctx: &SourceCtx<'_>) -> AppResult<std::collections::HashMap<String, String>> {
    Ok(cached_by_type(ctx, "up.category", 500)
        .await?
        .into_iter()
        .map(|r| (r.external_id, r.title))
        .collect())
}

/// Balance + spend-by-category/day + recent transactions, computed purely
/// from already-fetched cache rows -- split out from its DB-querying caller
/// so it's unit-testable without SurrealDB.
pub(crate) fn compute_finance_summary(
    since: &str,
    accounts: &[CachedRecord],
    txns: &[CachedRecord],
    category_names: &std::collections::HashMap<String, String>,
) -> Value {
    let balance_cents: i64 = accounts.iter().filter_map(|a| a.payload.get("balance_cents").and_then(|v| v.as_i64())).sum();

    let mut txns: Vec<&CachedRecord> =
        txns.iter().filter(|t| iso(&t.occurred_at).map(|o| o.as_str() >= since).unwrap_or(false)).collect();
    txns.sort_by_key(|t| std::cmp::Reverse(iso(&t.occurred_at).unwrap_or_default()));

    let mut by_cat: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let mut by_day: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for t in &txns {
        let cents = t.payload.get("amount_cents").and_then(|v| v.as_i64()).unwrap_or(0);
        if cents >= 0 {
            continue;
        }
        let cat_id = t.payload.get("category").and_then(|v| v.as_str());
        let name = cat_id
            .map(|id| category_names.get(id).cloned().unwrap_or_else(|| id.to_string()))
            .unwrap_or_else(|| "uncategorised".to_string());
        *by_cat.entry(name).or_insert(0) -= cents;
        let day = iso(&t.occurred_at).unwrap_or_default().chars().take(10).collect::<String>();
        *by_day.entry(day).or_insert(0) -= cents;
    }

    let mut spend_by_category: Vec<Value> =
        by_cat.into_iter().map(|(k, v)| json!({"category": k, "amount": round2(v as f64 / 100.0)})).collect();
    spend_by_category.sort_by(|a, b| b["amount"].as_f64().unwrap_or(0.0).partial_cmp(&a["amount"].as_f64().unwrap_or(0.0)).unwrap());

    let mut spend_by_day: Vec<Value> =
        by_day.into_iter().map(|(k, v)| json!({"day": k, "amount": round2(v as f64 / 100.0)})).collect();
    spend_by_day.sort_by(|a, b| a["day"].as_str().unwrap_or("").cmp(b["day"].as_str().unwrap_or("")));

    json!({
        "since": since,
        "balance": round2(balance_cents as f64 / 100.0),
        "accounts": accounts.iter().map(|a| json!({"name": a.title, "balance": a.payload.get("balance")})).collect::<Vec<_>>(),
        "spend_by_category": spend_by_category,
        "spend_by_day": spend_by_day,
        "recent_transactions": txns.iter().take(20).map(|t| json!({
            "description": t.title,
            "amount": t.payload.get("amount"),
            "status": t.payload.get("status"),
            "occurred_at": iso(&t.occurred_at),
        })).collect::<Vec<_>>(),
    })
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

pub async fn finance_summary(ctx: &SourceCtx<'_>, since: Option<&str>) -> AppResult<Value> {
    let since = since.map(String::from).unwrap_or_else(|| (Utc::now() - Duration::days(30)).to_rfc3339());
    let cat_names = category_names(ctx).await?;
    let accounts = cached_by_type(ctx, "up.account", 200).await?;
    let txns = cached_by_type(ctx, "up.transaction", 2000).await?;
    Ok(compute_finance_summary(&since, &accounts, &txns, &cat_names))
}

pub async fn list_transactions(ctx: &SourceCtx<'_>, days: i64, category: Option<&str>, limit: i64) -> AppResult<Value> {
    let since = (Utc::now() - Duration::days(days)).to_rfc3339();
    let cat_names = category_names(ctx).await?;
    let mut txns = cached_by_type(ctx, "up.transaction", 2000).await?;
    txns.retain(|t| iso(&t.occurred_at).map(|o| o >= since).unwrap_or(false));
    if let Some(category) = category {
        txns.retain(|t| t.payload.get("category").and_then(|v| v.as_str()) == Some(category));
    }
    txns.sort_by_key(|t| std::cmp::Reverse(iso(&t.occurred_at).unwrap_or_default()));
    let limit = limit.clamp(0, 200) as usize;
    let out: Vec<Value> = txns
        .iter()
        .take(limit)
        .map(|t| {
            let cat_id = t.payload.get("category").and_then(|v| v.as_str());
            json!({
                "description": t.title,
                "amount": t.payload.get("amount"),
                "status": t.payload.get("status"),
                "category": cat_id.map(|id| cat_names.get(id).cloned().unwrap_or_else(|| id.to_string())),
                "created_at": iso(&t.occurred_at),
            })
        })
        .collect();
    Ok(json!(out))
}

pub async fn list_accounts(ctx: &SourceCtx<'_>) -> AppResult<Value> {
    let accounts = cached_by_type(ctx, "up.account", 200).await?;
    Ok(json!(accounts
        .iter()
        .map(|a| json!({
            "name": a.title,
            "balance": a.payload.get("balance"),
            "account_type": a.payload.get("account_type"),
            "ownership_type": a.payload.get("ownership_type"),
        }))
        .collect::<Vec<_>>()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_txn_sets_is_income_and_joins_body_text() {
        let raw = json!({
            "type": "transactions", "id": "txn1",
            "attributes": {
                "description": "Woolworths", "rawText": "WOOLWORTHS 123", "createdAt": "2024-01-02T10:00:00Z",
                "amount": {"value": "-50.00", "valueInBaseUnits": -5000, "currencyCode": "AUD"}, "status": "SETTLED",
            },
            "relationships": {"category": {"data": {"id": "groceries"}}},
        });
        let env = map_txn(&raw);
        assert_eq!(env["id"], "up_bank:up.transaction:txn1");
        assert_eq!(env["body_text"], "Woolworths WOOLWORTHS 123");
        assert_eq!(env["payload"]["is_income"], false);
        assert_eq!(env["payload"]["category"], "groceries");
    }

    #[test]
    fn map_account_formats_body_text() {
        let raw = json!({
            "type": "accounts", "id": "acc1",
            "attributes": {"displayName": "Spending", "accountType": "TRANSACTIONAL", "createdAt": "2024-01-01T00:00:00Z",
                "balance": {"value": "100.00", "valueInBaseUnits": 10000}},
        });
        let env = map_account(&raw);
        assert_eq!(env["body_text"], "Spending · TRANSACTIONAL");
        assert_eq!(env["deleted"], false);
    }

    #[test]
    fn map_category_reads_parent_relationship() {
        let raw = json!({
            "type": "categories", "id": "restaurants",
            "attributes": {"name": "Restaurants"},
            "relationships": {"parent": {"data": {"id": "good-life"}}},
        });
        let env = map_category(&raw);
        assert_eq!(env["payload"]["parent"], "good-life");
        assert!(env["occurred_at"].is_null());
    }

    #[test]
    fn verify_signature_accepts_correct_hmac_and_rejects_tampered() {
        let secret = "shh";
        let body = b"{\"data\":{}}";
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let sig = hex::encode(mac.finalize().into_bytes());
        assert!(verify_signature(secret, body, &sig));
        assert!(!verify_signature(secret, body, "deadbeef"));
        assert!(!verify_signature("wrong-secret", body, &sig));
    }

    fn cached(title: &str, occurred_at: &str, payload: Value) -> CachedRecord {
        CachedRecord {
            title: title.to_string(),
            external_id: String::new(),
            occurred_at: Some(chrono::DateTime::parse_from_rfc3339(occurred_at).unwrap().with_timezone(&Utc).into()),
            payload,
        }
    }

    #[test]
    fn compute_finance_summary_breaks_down_spend_and_sorts_recent_first() {
        let accounts = vec![cached("Spending", "2024-01-01T00:00:00Z", json!({"balance_cents": 10000, "balance": "100.00"}))];
        let txns = vec![
            cached(
                "Woolworths",
                "2024-01-02T10:00:00Z",
                json!({"amount_cents": -5000, "amount": "-50.00", "status": "SETTLED", "category": "groceries"}),
            ),
            cached(
                "Mystery shop",
                "2024-01-03T10:00:00Z",
                json!({"amount_cents": -1000, "amount": "-10.00", "status": "SETTLED"}),
            ),
        ];
        let mut cat_names = std::collections::HashMap::new();
        cat_names.insert("groceries".to_string(), "Groceries".to_string());

        let summary = compute_finance_summary("2024-01-01T00:00:00Z", &accounts, &txns, &cat_names);
        assert_eq!(summary["balance"], 100.0);
        assert_eq!(summary["spend_by_category"][0]["category"], "Groceries");
        assert_eq!(summary["spend_by_category"][0]["amount"], 50.0);
        assert_eq!(summary["spend_by_category"][1]["category"], "uncategorised");
        assert_eq!(summary["recent_transactions"][0]["description"], "Mystery shop");
    }

    #[test]
    fn compute_finance_summary_excludes_income_from_spend_breakdown() {
        let txns = vec![cached("Salary", "2024-01-03T00:00:00Z", json!({"amount_cents": 300000, "status": "SETTLED"}))];
        let summary = compute_finance_summary("2024-01-01T00:00:00Z", &[], &txns, &std::collections::HashMap::new());
        assert_eq!(summary["spend_by_category"].as_array().unwrap().len(), 0);
    }

    use crate::sources::mock::{assert_fetch_fails, envelopes, route, serve};

    fn txn(id: &str, created: &str, description: &str, cents: i64) -> Value {
        json!({
            "type": "transactions", "id": id,
            "attributes": {
                "status": "SETTLED", "rawText": null, "description": description, "message": "pizza night",
                "amount": {"currencyCode": "AUD", "value": format!("{:.2}", cents as f64 / 100.0), "valueInBaseUnits": cents},
                "settledAt": created, "createdAt": created,
            },
            "relationships": {
                "account": {"data": {"type": "accounts", "id": "acc-1"}},
                "category": {"data": if id == "t1" { json!({"type": "categories", "id": "takeaway"}) } else { Value::Null }},
            },
        })
    }

    #[tokio::test]
    async fn fetch_follows_links_next_and_pulls_accounts_and_categories() {
        let mock = serve(vec![
            route("GET", "/transactions", json!({"data": [txn("t2", "2024-03-02T09:00:00+11:00", "Coles", -1250)], "links": {"prev": null, "next": null}}))
                .query_has("page[after]=t1"),
            route(
                "GET",
                "/transactions",
                json!({"data": [txn("t1", "2024-03-01T05:08:57+11:00", "Pizza Hut", -5998)], "links": {"prev": null, "next": "{base}/transactions?page[after]=t1&page[size]=100"}}),
            )
            .query_has("filter[since]=2024-02-01T00:00:00"),
            route("GET", "/accounts", json!({"data": [{"type": "accounts", "id": "acc-1", "attributes": {
                "displayName": "Spending", "accountType": "TRANSACTIONAL", "ownershipType": "INDIVIDUAL",
                "balance": {"currencyCode": "AUD", "value": "1.00", "valueInBaseUnits": 100}, "createdAt": "2020-01-01T00:00:00+11:00"}}],
                "links": {"prev": null, "next": null}})),
            route("GET", "/categories", json!({"data": [{"type": "categories", "id": "takeaway", "attributes": {"name": "Takeaway"},
                "relationships": {"parent": {"data": {"type": "categories", "id": "good-life"}}}}]})),
        ])
        .await;

        let conn = mock.conn(json!({"personal_access_token": "up:yeah:test"}));
        let res = UpBankSource.fetch(&conn, Some("2024-02-08T00:00:00Z".into())).await.unwrap();
        assert_eq!(res.records.len(), 4, "2 transaction pages + accounts + categories");
        assert_eq!(res.cursor.as_deref(), Some("2024-03-02T09:00:00+11:00"));
        assert!(mock.requests().iter().all(|r| r.header("authorization") == "Bearer up:yeah:test"));

        let envs = envelopes(&UpBankSource, &res.records);
        let pizza = envs.iter().find(|e| e.external_id == "t1").unwrap();
        assert_eq!(pizza.title, "Pizza Hut");
        assert_eq!(pizza.body_text, "Pizza Hut pizza night");
        assert!(pizza.occurred_at.is_some());
        assert_eq!(pizza.links.len(), 2);
        assert_eq!(pizza.links[1].target, "up_bank:up.category:takeaway");
        assert!(envs.iter().any(|e| e.type_ == "up.account" && e.title == "Spending"));
    }

    #[tokio::test]
    async fn fetch_errors_are_visible() {
        let creds = json!({"personal_access_token": "bad"});
        assert_fetch_fails(&UpBankSource, 401, "/transactions", creds.clone(), "HTTP 401").await;
        assert_fetch_fails(&UpBankSource, 429, "/transactions", creds.clone(), "HTTP 429").await;
        assert_fetch_fails(&UpBankSource, 500, "/transactions", creds, "HTTP 500").await;
        assert_fetch_fails(&UpBankSource, 200, "/x", json!({}), "missing personal_access_token").await;
    }
}
