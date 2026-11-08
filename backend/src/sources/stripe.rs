//! Stripe source: charges, via `GET /v1/charges` (`has_more` +
//! `starting_after` pagination). A restricted key with Charges: Read is
//! enough; bearer auth.
//!
//! Incremental: `created[gt]=` the newest charge's `created` seen last sync.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::connectors::clients::{bearer, require, Api};
use crate::error::AppResult;
use crate::sources::base::{envelope, from_unix, items, s, Conn, Source, SyncResult, MAX_PAGES};

const BASE_URL: &str = "https://api.stripe.com/v1";

fn api(conn: &Conn) -> AppResult<Api> {
    Ok(Api::new(&conn.config, BASE_URL, bearer(&require(&conn.credentials, "secret_key")?)))
}

pub struct StripeSource;

#[async_trait]
impl Source for StripeSource {
    fn key(&self) -> &'static str {
        "stripe"
    }

    fn label(&self) -> &'static str {
        "Stripe"
    }

    fn record_types(&self) -> &'static [&'static str] {
        &["stripe.charge"]
    }

    async fn check(&self, conn: &Conn) -> AppResult<()> {
        api(conn)?.get("/charges", &[("limit", "1".to_string())]).await.map(|_| ())
    }

    async fn fetch(&self, conn: &Conn, cursor: Option<String>) -> AppResult<SyncResult> {
        let api = api(conn)?;
        let mut records: Vec<Value> = Vec::new();
        for _ in 0..MAX_PAGES {
            let mut query = vec![("limit", "100".to_string())];
            if let Some(c) = &cursor {
                query.push(("created[gt]", c.clone()));
            }
            if let Some(last) = records.last() {
                query.push(("starting_after", s(last, "/id").to_string()));
            }
            let page = api.get("/charges", &query).await?;
            records.extend(items(&page, "/data"));
            if page.get("has_more").and_then(|v| v.as_bool()) != Some(true) {
                break;
            }
        }
        let newest = records.iter().filter_map(|r| r.get("created").and_then(|v| v.as_i64())).max().map(|c| c.to_string());
        Ok(SyncResult { records, cursor: newest.or(cursor) })
    }

    fn map(&self, raw: &Value) -> Option<Value> {
        let id = raw.get("id").and_then(|v| v.as_str())?;
        let amount = raw.get("amount").and_then(|v| v.as_i64()).unwrap_or(0) as f64 / 100.0;
        let currency = s(raw, "/currency").to_uppercase();
        let description = s(raw, "/description");
        let customer = s(raw, "/billing_details/email");
        let amount = crate::views::money(amount);
        let title = format!("{amount} {currency} · {}", if description.is_empty() { s(raw, "/status") } else { description });
        let body = format!("Charge of {amount} {currency} ({}) {description} {customer}", s(raw, "/status"));
        let url_id = raw.get("payment_intent").and_then(|v| v.as_str()).unwrap_or(id);
        Some(envelope(
            "stripe",
            "stripe.charge",
            id,
            &title,
            body.trim(),
            from_unix(raw.get("created").and_then(|v| v.as_i64()).unwrap_or(0)),
            &format!("https://dashboard.stripe.com/payments/{url_id}"),
            json!({
                "amount": amount,
                "currency": currency,
                "status": raw.get("status"),
                "paid": raw.get("paid"),
                "refunded": raw.get("refunded"),
                "customer": raw.get("customer"),
                "customer_email": customer,
                "livemode": raw.get("livemode"),
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::mock::{assert_fetch_fails, envelopes, route, serve};

    fn charge(id: &str, created: i64) -> Value {
        json!({"id": id, "object": "charge", "amount": 2500, "currency": "aud", "created": created, "status": "succeeded",
               "paid": true, "refunded": false, "description": "Pro plan", "customer": "cus_1", "livemode": false,
               "payment_intent": format!("pi_{id}"), "billing_details": {"email": "buyer@example.com"}})
    }

    #[tokio::test]
    async fn fetch_pages_with_starting_after_since_the_cursor() {
        let mock = serve(vec![
            route("GET", "/charges", json!({"object": "list", "data": [charge("ch_2", 1700000200)], "has_more": false, "url": "/v1/charges"}))
                .query_has("starting_after=ch_1"),
            route("GET", "/charges", json!({"object": "list", "data": [charge("ch_1", 1700000100)], "has_more": true, "url": "/v1/charges"}))
                .query_has("created[gt]=1700000000"),
        ])
        .await;

        let res = StripeSource.fetch(&mock.conn(json!({"secret_key": "rk_test_x"})), Some("1700000000".into())).await.unwrap();
        assert_eq!(res.records.len(), 2);
        assert_eq!(res.cursor.as_deref(), Some("1700000200"));
        assert!(mock.requests().iter().all(|r| r.header("authorization") == "Bearer rk_test_x"));

        let envs = envelopes(&StripeSource, &res.records);
        assert_eq!(envs[0].title, "25.00 AUD · Pro plan");
        assert_eq!(envs[0].url, "https://dashboard.stripe.com/payments/pi_ch_1");
        assert!(envs[0].occurred_at.is_some());
    }

    #[tokio::test]
    async fn fetch_errors_are_visible() {
        let creds = json!({"secret_key": "bad"});
        assert_fetch_fails(&StripeSource, 401, "/charges", creds.clone(), "HTTP 401").await;
        assert_fetch_fails(&StripeSource, 429, "/charges", creds.clone(), "HTTP 429").await;
        assert_fetch_fails(&StripeSource, 500, "/charges", creds, "HTTP 500").await;
    }
}
