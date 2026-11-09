//! The HTTP plumbing every connector shares: [`Api`] (one provider's base URL
//! plus auth headers, with provider failures turned into readable errors),
//! credential readers, and the OAuth refresh-token exchange used by the Google
//! and Spotify connectors. Per-provider fetching and mapping lives in
//! `sources::<provider>`; only the Up Bank and PocketAI clients live here too,
//! because the `/connectors/*` summary endpoints call them directly.
//!
//! Every request goes through `llm_net::client`, the same guarded client model calls use: no
//! redirects, link-local and database addresses refused at connect time (a provider's own "next page"
//! link included), private ranges refused when `ALLOW_PRIVATE_LLM_URL=0`.
//!
//! A connector's API base URL (and the OAuth token endpoint) can be overridden with
//! `config.base_url` / `config.token_url`, but only when the operator sets
//! `EUNOMIA_ALLOW_CONNECTOR_BASE_URL=1`: the hook the mock-provider tests use, and how a self-hosted
//! provider such as GitHub Enterprise or PocketAI points elsewhere.

use std::sync::Arc;
use std::time::Duration;

use rand::Rng;
use tokio::sync::Mutex;

use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, RETRY_AFTER, USER_AGENT};
use axum::http::{Method, StatusCode};
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};

const TIMEOUT: Duration = Duration::from_secs(30);
/// A 429 asking us to wait at most this long is retried once in place;
/// anything longer fails the sync and the scheduler's backoff takes over.
const MAX_RETRY_AFTER_SECS: u64 = 30;

pub fn str_field(v: &Value, key: &str) -> String {
    v.get(key).and_then(|v| v.as_str()).unwrap_or("").trim().to_string()
}

/// A credential the connector can't work without, or an error telling the
/// user exactly what to enter.
pub fn require(credentials: &Value, key: &str) -> AppResult<String> {
    let v = str_field(credentials, key);
    if v.is_empty() {
        Err(AppError::bad_request(format!("not configured: missing {key} -- add it on the Connectors page")))
    } else {
        Ok(v)
    }
}

/// `Authorization: <value>` verbatim (Linear wants a bare key, Discord `Bot <token>`).
pub fn auth_header(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(value) {
        headers.insert(AUTHORIZATION, v);
    }
    headers
}

pub fn bearer(token: &str) -> HeaderMap {
    auth_header(&format!("Bearer {token}"))
}

/// The guarded client for one request (see the module docs). Its error already says why a URL is refused.
async fn guarded(url: &str, secrets: Option<&crate::secrets::connector::Context>) -> AppResult<reqwest::Client> {
    if let Some(secrets) = secrets { return secrets.client(url).await; }
    crate::llm_net::client(url).await.map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, format!("{} is not allowed: {}", host(url), e.message)))
}

fn host(url: &str) -> &str {
    url.split("://").nth(1).and_then(|rest| rest.split('/').next()).unwrap_or(url)
}

/// Google uses 403 for quota failures as well as permissions errors.
fn rate_limited(status: StatusCode, body: &str) -> bool {
    if status == StatusCode::TOO_MANY_REQUESTS { return true; }
    if status != StatusCode::FORBIDDEN { return false; }
    let Ok(value) = serde_json::from_str::<Value>(body) else { return false };
    value.pointer("/error/errors").and_then(Value::as_array).is_some_and(|errors| {
        errors.iter().any(|error| matches!(error["reason"].as_str(),
            Some("rateLimitExceeded" | "userRateLimitExceeded" | "quotaExceeded")))
    })
}

/// Shared fetch policy: quota, transport, response and server failures can be retried.
/// `connector_removed` is HTTP 404/410; `connector_auth` is non-quota 401/403.
/// `AppError::retry_after_secs` carries the final provider delay to the scheduler.
pub(crate) fn retryable(error: &AppError) -> bool {
    matches!(error.reason, Some("connector_rate_limited" | "connector_transient" | "connector_server_error" | "connector_response_error"))
}

pub(crate) fn provider_error(url: &str, status: StatusCode, body: &str) -> AppError {
    let limited = rate_limited(status, body);
    let hint = if limited { " (rate limited -- retried on the next sync)" } else { match status.as_u16() {
        401 | 403 => " (check the token and its scopes)",
        429 => " (rate limited -- retried on the next sync)",
        500..=599 => " (provider-side error -- retried on the next sync)",
        _ => "",
    }};
    let snippet: String = reason(body).unwrap_or_else(|| body.chars().take(200).collect());
    let error = AppError::new(StatusCode::BAD_GATEWAY, format!("{} returned HTTP {}{hint}: {}", host(url), status.as_u16(), snippet.trim()));
    if limited { error.with_reason("connector_rate_limited") }
    else if matches!(status.as_u16(), 401 | 403) { error.with_reason("connector_auth") }
    else if matches!(status.as_u16(), 404 | 410) { error.with_reason("connector_removed") }
    else if status.is_server_error() { error.with_reason("connector_server_error") }
    else if status == StatusCode::BAD_REQUEST { error.with_reason("connector_message_error") }
    else { error }
}

/// The provider's own words from a JSON error body (`message`, `error.message`, `error`, `detail`, `errors[0].message`), if it has any.
fn reason(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    let text = |v: &Value| v.as_str().filter(|s| !s.trim().is_empty()).map(str::to_string);
    let found = text(&v["message"])
        .or_else(|| text(&v["error"]["message"]))
        .or_else(|| text(&v["error"]))
        .or_else(|| text(&v["detail"]))
        .or_else(|| text(&v["errors"][0]["message"]))?;
    Some(found.chars().take(200).collect())
}

fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let raw = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = raw.parse::<f64>() {
        return Duration::try_from_secs_f64(seconds).ok();
    }
    let date = chrono::DateTime::parse_from_rfc2822(raw).ok()?;
    Some((date.with_timezone(&chrono::Utc) - chrono::Utc::now()).to_std().unwrap_or_default())
}

/// Shared by all requests of one Google sync, never by unrelated connections.
struct RetryBudget {
    remaining: Duration,
    retries: u32,
}

impl RetryBudget {
    fn reserve(&mut self, wait: Duration) -> bool {
        if self.retries == 0 || wait > self.remaining { return false; }
        self.retries -= 1;
        self.remaining = self.remaining.saturating_sub(wait);
        true
    }
}

fn jitter(attempt: u32) -> Duration {
    Duration::from_millis(rand::thread_rng().gen_range(0..=1000 * (1u64 << attempt.min(3))))
}

/// A per-user `config.base_url` would let any user of a shared server point
/// the backend at hosts of their choosing (the guard still refuses the database and cloud
/// metadata, but not every internal service) and read the replies through sync errors. So it's off
/// unless the operator opts in with `EUNOMIA_ALLOW_CONNECTOR_BASE_URL=1` (tests, mock providers, or a
/// self-hosted provider such as GitHub Enterprise).
pub fn base_url_override_allowed() -> bool {
    cfg!(test) || std::env::var("EUNOMIA_ALLOW_CONNECTOR_BASE_URL").is_ok_and(|v| v == "1")
}

/// One provider's REST API. Every request goes through [`Api::send`], which
/// retries Google quota/transient failures within a per-sync budget, or a short
/// 429 once for other providers, and turns any non-2xx into an error naming the
/// host and status -- what lands in `sync_status.last_error`.
#[derive(Clone)]
pub struct Api {
    base: String,
    headers: HeaderMap,
    retry_budget: Option<Arc<Mutex<RetryBudget>>>,
    pace: Option<Arc<Mutex<tokio::time::Instant>>>,
    spacing: Duration,
    secrets: Option<crate::secrets::connector::Context>,
}

impl Api {
    /// `default_base`, or `config.base_url` when overrides are allowed (see
    /// [`base_url_override_allowed`]).
    pub fn new(config: &Value, default_base: &str, mut headers: HeaderMap) -> Self {
        let base = config
            .get("base_url")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty() && base_url_override_allowed())
            .unwrap_or(default_base);
        // GitHub rejects requests without a User-Agent; harmless elsewhere.
        headers.insert(USER_AGENT, HeaderValue::from_static("eunomia"));
        Self { base: base.trim_end_matches('/').to_string(), headers, secrets: crate::secrets::connector::current(), retry_budget: None, pace: None, spacing: Duration::ZERO }
    }

    pub(crate) fn with_google_retries(mut self) -> Self {
        self.retry_budget = Some(Arc::new(Mutex::new(RetryBudget { remaining: Duration::from_secs(10), retries: 4 })));
        self
    }

    pub(crate) fn with_pacing(mut self, spacing: Duration) -> Self {
        self.pace = Some(Arc::new(Mutex::new(tokio::time::Instant::now())));
        self.spacing = spacing;
        self
    }

    async fn pace(&self) {
        if let Some(pace) = &self.pace {
            let mut next = pace.lock().await;
            tokio::time::sleep_until(*next).await;
            *next = tokio::time::Instant::now() + self.spacing;
        }
    }

    pub fn with_header(mut self, name: &'static str, value: &'static str) -> Self {
        self.headers.insert(name, HeaderValue::from_static(value));
        self
    }

    /// `path` relative to the base URL, or an absolute URL (a provider's own
    /// "next page" link) used as-is.
    fn url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else {
            format!("{}{path}", self.base)
        }
    }

    pub async fn get(&self, path: &str, query: &[(&str, String)]) -> AppResult<Value> {
        Ok(self.send(Method::GET, path, query, None).await?.0)
    }

    /// Like [`Api::get`], plus the response headers (GitHub paginates via `Link`).
    pub async fn get_with_headers(&self, path: &str, query: &[(&str, String)]) -> AppResult<(Value, HeaderMap)> {
        self.send(Method::GET, path, query, None).await
    }

    pub async fn post(&self, path: &str, body: &Value) -> AppResult<Value> {
        Ok(self.send(Method::POST, path, &[], Some(body)).await?.0)
    }

    async fn send(&self, method: Method, path: &str, query: &[(&str, String)], body: Option<&Value>) -> AppResult<(Value, HeaderMap)> {
        let url = self.url(path);
        let mut retried = false;
        let mut attempt = 0;
        loop {
            let started = tokio::time::Instant::now();
            self.pace().await;
            let http = guarded(&url, self.secrets.as_ref()).await?;
            let full = if query.is_empty() { reqwest::Url::parse(&url) } else { reqwest::Url::parse_with_params(&url, query) }
                .map_err(|e| AppError::internal(format!("invalid connector url: {e}")))?;
            let mut req = http.request(method.clone(), full).headers(self.headers.clone()).timeout(TIMEOUT);
            if let Some(body) = body {
                req = req.json(body);
            }
            if attempt > 0 && let Some(budget) = &self.retry_budget {
                req = req.timeout(budget.lock().await.remaining.saturating_sub(started.elapsed()).max(Duration::from_millis(1)).min(TIMEOUT));
            }
            let response = req.send().await;
            let response = match response {
                Ok(resp) => Ok(resp),
                Err(e) => {
                    let message = format!("could not reach {}: {e}", host(&url));
                    let message = self.secrets.as_ref().map_or_else(|| message.clone(), |s| s.scrub(&message));
                    let error = AppError::new(StatusCode::BAD_GATEWAY, message);
                    Err(error.with_reason("connector_transient"))
                }
            };
            let response_status_received = response.is_ok();
            let (status, mut headers, text) = match response {
                Ok(resp) => {
                    let status = resp.status();
                    let headers = resp.headers().clone();
                    let text = resp.text().await.map_err(|_| AppError::new(StatusCode::BAD_GATEWAY,
                        format!("could not read response from {}", host(&url))).with_reason("connector_response_error"));
                    (status, headers, text)
                }
                Err(error) => (StatusCode::BAD_GATEWAY, HeaderMap::new(), Err(error)),
            };
            if attempt > 0 && let Some(budget) = &self.retry_budget {
                let mut budget = budget.lock().await;
                budget.remaining = budget.remaining.saturating_sub(started.elapsed());
            }
            // Scrub before shortening a provider message, so a secret crossing the snippet boundary stays hidden.
            let text = text.map(|text| self.secrets.as_ref().map_or_else(|| text.clone(), |secrets| secrets.scrub(&text)));
            let error = match &text {
                body if !status.is_success() && response_status_received => Some(provider_error(&url, status, body.as_deref().unwrap_or("response body unreadable"))),
                Err(error) => {
                    let mut copy = AppError::new(error.status, error.message.clone());
                    copy.reason = error.reason;
                    Some(copy)
                }
                _ => None,
            };
            if let Some(error) = error {
                let wait = retry_after(&headers);
                let retry = if let Some(budget) = &self.retry_budget {
                    let delay = jitter(attempt).max(wait.unwrap_or_default());
                    if method == Method::GET && retryable(&error) && budget.lock().await.reserve(delay) {
                        Some(delay)
                    } else { None }
                } else if status == StatusCode::TOO_MANY_REQUESTS && !retried {
                    wait.filter(|w| *w <= Duration::from_secs(MAX_RETRY_AFTER_SECS))
                } else { None };
                if let Some(delay) = retry {
                    retried = true;
                    attempt += 1;
                    tokio::time::sleep(delay).await;
                    continue;
                }
                let mut error = error;
                error.retry_after_secs = wait.map(|delay| delay.as_secs().saturating_add(u64::from(delay.subsec_nanos() > 0)));
                if let Some(secrets) = &self.secrets { error.message = secrets.scrub(&error.message); }
                return Err(error);
            }
            let text = text?;
            let text = if let Some(secrets) = &self.secrets {
                for value in headers.values_mut() {
                    if let Ok(raw) = value.to_str() {
                        *value = HeaderValue::from_str(&secrets.scrub(raw)).unwrap_or_else(|_| HeaderValue::from_static("[redacted]"));
                    }
                }
                secrets.scrub(&text)
            } else { text };
            if text.trim().is_empty() {
                return Ok((Value::Null, headers));
            }
            let value = serde_json::from_str(&text)
                .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, format!("{} returned invalid JSON: {e}", host(&url))))?;
            return Ok((value, headers));
        }
    }

    /// A binary download (Pocket audio): the body and its media type, or `None` on a 404 (not there
    /// yet). Like [`Api::send`] (guarded client, one short 429 retry, readable provider errors), with a
    /// longer timeout, and refused once the body passes `max_bytes` without buffering the rest.
    pub async fn get_bytes(&self, path: &str, max_bytes: usize) -> AppResult<Option<(Vec<u8>, String)>> {
        download(&self.url(path), &self.headers, max_bytes).await
    }
}

/// How long one binary download may take (an hour of audio is tens of MiB).
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// Marks the error [`download`] returns for a body over its cap.
pub const TOO_LARGE: &str = "is larger than the size limit";

/// GET `url` with `headers` as bytes; see [`Api::get_bytes`]. Also fetches a provider's signed URL
/// (with no auth headers of ours), which stays behind the same guard: redirects are never followed.
pub async fn download(url: &str, headers: &HeaderMap, max_bytes: usize) -> AppResult<Option<(Vec<u8>, String)>> {
    let mut retried = false;
    // a held credential only goes to the hosts it was approved for; a provider's signed URL carries
    // no headers of ours, so it needs no such check
    let secrets = if headers.is_empty() { None } else { crate::secrets::connector::current() };
    loop {
        let http = guarded(url, secrets.as_ref()).await?;
        let mut resp = http
            .get(url)
            .headers(headers.clone())
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await
            .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, format!("could not reach {}: {e}", host(url))))?;
        let status = resp.status();
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if status == StatusCode::TOO_MANY_REQUESTS
            && !retried
            && let Some(wait) = retry_after(resp.headers()).filter(|w| *w <= Duration::from_secs(MAX_RETRY_AFTER_SECS))
        {
            retried = true;
            tokio::time::sleep(wait).await;
            continue;
        }
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(provider_error(url, status, &text));
        }
        let too_large = || AppError::new(StatusCode::BAD_GATEWAY, format!("the download from {} {TOO_LARGE} ({max_bytes} bytes)", host(url)));
        if resp.content_length().is_some_and(|n| n > max_bytes as u64) {
            return Err(too_large());
        }
        let media_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(|v| v.trim().to_ascii_lowercase())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "application/octet-stream".to_string());
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, format!("download from {} failed: {e}", host(url))))? {
            if body.len() + chunk.len() > max_bytes {
                return Err(too_large());
            }
            body.extend_from_slice(&chunk);
        }
        return Ok(Some((body, media_type)));
    }
}

/// Exchanges a long-lived refresh token (`credentials.refresh_token`, issued
/// to the user's own OAuth client `client_id`/`client_secret`) for a fresh
/// access token. Google and Spotify both accept the client credentials as
/// HTTP Basic auth, so one function serves both. Access tokens last about an
/// hour, so every sync simply refreshes first.
pub async fn refresh_access_token(config: &Value, default_token_url: &str, credentials: &Value) -> AppResult<String> {
    let client_id = require(credentials, "client_id")?;
    let client_secret = require(credentials, "client_secret")?;
    let refresh_token = require(credentials, "refresh_token")?;
    let url = config
        .get("token_url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty() && base_url_override_allowed())
        .unwrap_or(default_token_url);
    let secrets = crate::secrets::connector::current();
    let resp = guarded(url, secrets.as_ref())
        .await?
        .post(url)
        .basic_auth(client_id, Some(client_secret))
        .form(&[("grant_type", "refresh_token"), ("refresh_token", refresh_token.as_str())])
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, format!("could not reach {}: {e}", host(url))))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let text = secrets.as_ref().map_or_else(|| text.clone(), |s| s.scrub(&text));
        return Err(provider_error(url, status, &text));
    }
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v.get("access_token").and_then(|t| t.as_str()).map(String::from))
        .ok_or_else(|| AppError::new(StatusCode::BAD_GATEWAY, format!("{} returned no access_token", host(url))))
}

fn round_to(x: f64, places: i32) -> f64 {
    let factor = 10f64.powi(places);
    (x * factor).round() / factor
}

// ---------------------------------------------------------------------------
// Up Bank
// ---------------------------------------------------------------------------

pub struct UpBankClient {
    pub api: Api,
}

impl UpBankClient {
    pub const BASE_URL: &'static str = "https://api.up.com.au/api/v1";

    pub fn new(credentials: &Value, config: &Value) -> AppResult<Self> {
        let token = require(credentials, "personal_access_token")?;
        Ok(Self { api: Api::new(config, Self::BASE_URL, bearer(&token)) })
    }

    pub async fn ping(&self) -> AppResult<()> {
        self.api.get("/util/ping", &[]).await.map(|_| ())
    }

    pub async fn accounts(&self) -> AppResult<Value> {
        self.api.get("/accounts", &[]).await
    }

    pub async fn transactions(&self, params: &[(&str, String)]) -> AppResult<Value> {
        self.api.get("/transactions", params).await
    }

    pub async fn categories(&self) -> AppResult<Value> {
        self.api.get("/categories", &[]).await
    }

    /// Balance across accounts + settled spend broken down by category since
    /// `since_iso`. Every field here comes straight off the
    /// transaction/account/category resources -- no invented metrics.
    pub async fn finance_summary(&self, since_iso: &str) -> AppResult<Value> {
        let accounts = self.accounts().await?;
        let categories = self.categories().await?;
        let transactions = self
            .transactions(&[("filter[since]", since_iso.to_string()), ("page[size]", "100".to_string())])
            .await?;
        Ok(compute_finance_summary(&accounts, &categories, &transactions))
    }

    /// Settled transaction count + total spend (negative amounts) since
    /// `since_iso`.
    pub async fn week_summary(&self, since_iso: &str) -> AppResult<Value> {
        let transactions = self
            .transactions(&[("filter[since]", since_iso.to_string()), ("page[size]", "100".to_string())])
            .await?;
        Ok(compute_week_summary(&transactions))
    }
}

pub fn compute_finance_summary(accounts: &Value, categories: &Value, transactions: &Value) -> Value {
    let accounts_data: Vec<Value> = accounts.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default();

    let balance_cents: i64 = accounts_data
        .iter()
        .filter_map(|a| a.pointer("/attributes/balance/valueInBaseUnits").and_then(|v| v.as_i64()))
        .sum();

    let cat_names: std::collections::HashMap<String, String> = categories
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    let id = c.get("id")?.as_str()?.to_string();
                    let name = c.pointer("/attributes/name")?.as_str()?.to_string();
                    Some((id, name))
                })
                .collect()
        })
        .unwrap_or_default();

    let data: Vec<Value> = transactions.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default();
    let settled: Vec<&Value> = data
        .iter()
        .filter(|t| t.pointer("/attributes/status").and_then(|s| s.as_str()) == Some("SETTLED"))
        .collect();

    let mut spend_by_category: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let mut spend_by_day: std::collections::HashMap<String, i64> = std::collections::HashMap::new();

    for t in &settled {
        let cents = t.pointer("/attributes/amount/valueInBaseUnits").and_then(|v| v.as_i64()).unwrap_or(0);
        if cents >= 0 {
            continue;
        }
        let cat_id = t.pointer("/relationships/category/data/id").and_then(|v| v.as_str());
        let name = match cat_id {
            Some(id) => cat_names.get(id).cloned().unwrap_or_else(|| "Uncategorised".to_string()),
            None => "Uncategorised".to_string(),
        };
        *spend_by_category.entry(name).or_insert(0) -= cents;

        let created_at = t.pointer("/attributes/createdAt").and_then(|v| v.as_str()).unwrap_or("");
        let day: String = created_at.chars().take(10).collect();
        *spend_by_day.entry(day).or_insert(0) -= cents;
    }

    let mut spend_by_category_vec: Vec<Value> = spend_by_category
        .into_iter()
        .map(|(k, v)| json!({"category": k, "amount": round_to(v as f64 / 100.0, 2)}))
        .collect();
    spend_by_category_vec.sort_by(|a, b| {
        let av = a["amount"].as_f64().unwrap_or(0.0);
        let bv = b["amount"].as_f64().unwrap_or(0.0);
        bv.partial_cmp(&av).unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut spend_by_day_vec: Vec<Value> = spend_by_day
        .into_iter()
        .map(|(k, v)| json!({"day": k, "amount": round_to(v as f64 / 100.0, 2)}))
        .collect();
    spend_by_day_vec.sort_by(|a, b| a["day"].as_str().unwrap_or("").cmp(b["day"].as_str().unwrap_or("")));

    let mut sorted_data = data.clone();
    sorted_data.sort_by(|a, b| {
        let a_c = a.pointer("/attributes/createdAt").and_then(|v| v.as_str()).unwrap_or("");
        let b_c = b.pointer("/attributes/createdAt").and_then(|v| v.as_str()).unwrap_or("");
        b_c.cmp(a_c)
    });
    let recent_transactions: Vec<Value> = sorted_data
        .into_iter()
        .take(20)
        .map(|t| {
            json!({
                "description": t.pointer("/attributes/description").and_then(|v| v.as_str()).unwrap_or(""),
                "amount": t.pointer("/attributes/amount/value").and_then(|v| v.as_str()).unwrap_or(""),
                "created_at": t.pointer("/attributes/createdAt").and_then(|v| v.as_str()).unwrap_or(""),
            })
        })
        .collect();

    json!({
        "balance": round_to(balance_cents as f64 / 100.0, 2),
        "accounts": accounts_data.iter().map(|a| json!({
            "name": a.pointer("/attributes/displayName").and_then(|v| v.as_str()).unwrap_or(""),
            "balance": a.pointer("/attributes/balance/value").and_then(|v| v.as_str()).unwrap_or(""),
        })).collect::<Vec<_>>(),
        "spend_by_category": spend_by_category_vec,
        "spend_by_day": spend_by_day_vec,
        "recent_transactions": recent_transactions,
    })
}

pub fn compute_week_summary(transactions: &Value) -> Value {
    let data: Vec<Value> = transactions.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default();
    let rows: Vec<&Value> = data
        .iter()
        .filter(|t| t.pointer("/attributes/status").and_then(|s| s.as_str()) == Some("SETTLED"))
        .collect();
    let spend_cents: i64 = rows
        .iter()
        .filter_map(|t| {
            let v = t.pointer("/attributes/amount/valueInBaseUnits").and_then(|v| v.as_i64())?;
            if v < 0 {
                Some(-v)
            } else {
                None
            }
        })
        .sum();
    json!({
        "transaction_count": rows.len(),
        "spent": round_to(spend_cents as f64 / 100.0, 2),
    })
}

// ---------------------------------------------------------------------------
// PocketAI (heypocket)
// ---------------------------------------------------------------------------

pub struct PocketAIClient {
    pub api: Api,
}

impl PocketAIClient {
    pub const DEFAULT_BASE_URL: &'static str = "https://public.heypocketai.com/api/v1";

    pub fn new(credentials: &Value, config: &Value) -> AppResult<Self> {
        let key = require(credentials, "api_key")?;
        Ok(Self { api: Api::new(config, Self::DEFAULT_BASE_URL, bearer(&key)) })
    }

    pub async fn ping(&self) -> AppResult<()> {
        self.api.get("/public/recordings", &[("limit", "1".to_string())]).await.map(|_| ())
    }

    pub async fn recordings(&self, params: &[(&str, String)]) -> AppResult<Value> {
        self.api.get("/public/recordings", params).await
    }

    pub async fn search(&self, query: &str) -> AppResult<Value> {
        self.api.post("/public/search", &json!({"query": query})).await
    }

    /// Full detail for a single recording -- transcript + summarizations.
    pub async fn recording(&self, recording_id: &str) -> AppResult<Value> {
        self.api.get(&format!("/public/recordings/{recording_id}"), &[]).await
    }

    /// A recording's audio, at most `max_bytes`: `None` while Pocket has none for it (a 404: the
    /// device has not uploaded it yet). The endpoint is not in Pocket's public docs we could check, so
    /// both shapes are handled: the audio bytes themselves, or JSON naming a signed URL, which is then
    /// fetched (behind the same guard, without our key). See docs/connectors.md, "Pocket audio".
    pub async fn audio(&self, recording_id: &str, max_bytes: usize) -> AppResult<Option<(Vec<u8>, String)>> {
        let Some((body, media_type)) = self.api.get_bytes(&format!("/public/recordings/{recording_id}/audio"), max_bytes).await? else {
            return Ok(None);
        };
        if !media_type.contains("json") {
            return Ok(Some((body, media_type)));
        }
        let v: Value = serde_json::from_slice(&body)
            .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, format!("the Pocket audio answer is not valid JSON: {e}")))?;
        match signed_audio_url(&v) {
            // a signed URL carries its own authorization; ours must not travel to another host
            Some(url) => {
                let mut headers = HeaderMap::new();
                headers.insert(USER_AGENT, HeaderValue::from_static("eunomia"));
                download(&url, &headers, max_bytes).await
            }
            // JSON without a link: nothing to download yet
            None => Ok(None),
        }
    }

    /// Recording count/duration/tags since a given date. Every field comes
    /// straight off the recording resource (`duration`, `tags`).
    pub async fn summary(&self, since_iso_date: &str) -> AppResult<Value> {
        let recordings = self
            .recordings(&[("start_date", since_iso_date.to_string()), ("limit", "100".to_string())])
            .await?;
        Ok(compute_pocketai_summary(&recordings))
    }
}

/// The audio link in a JSON answer from Pocket's audio endpoint: `url`, `audio_url`, `signed_url` or
/// `download_url`, at the top level or under `data`. Only http(s).
pub fn signed_audio_url(v: &Value) -> Option<String> {
    const KEYS: [&str; 4] = ["url", "audio_url", "signed_url", "download_url"];
    [v, &v["data"]]
        .into_iter()
        .flat_map(|o| KEYS.iter().map(move |k| &o[*k]))
        .filter_map(Value::as_str)
        .map(str::trim)
        .find(|u| u.starts_with("https://") || u.starts_with("http://"))
        .map(str::to_string)
}

pub fn compute_pocketai_summary(recordings: &Value) -> Value {
    let data: Vec<Value> = recordings.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default();

    let mut tag_counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for r in &data {
        if let Some(tags) = r.get("tags").and_then(|t| t.as_array()) {
            for tag in tags {
                let name = tag.get("name").and_then(|v| v.as_str()).unwrap_or("untagged").to_string();
                *tag_counts.entry(name).or_insert(0) += 1;
            }
        }
    }
    let mut tag_breakdown: Vec<Value> =
        tag_counts.into_iter().map(|(k, v)| json!({"tag": k, "count": v})).collect();
    tag_breakdown.sort_by(|a, b| b["count"].as_i64().unwrap_or(0).cmp(&a["count"].as_i64().unwrap_or(0)));

    let total_duration_secs: f64 = data.iter().map(|r| r.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0)).sum();

    let mut sorted_data = data.clone();
    sorted_data.sort_by(|a, b| {
        let a_at = a.get("recording_at").and_then(|v| v.as_str()).unwrap_or("");
        let b_at = b.get("recording_at").and_then(|v| v.as_str()).unwrap_or("");
        b_at.cmp(a_at)
    });
    let recent_recordings: Vec<Value> = sorted_data
        .into_iter()
        .take(10)
        .map(|r| {
            let duration = r.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0);
            json!({
                "title": r.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                "duration_minutes": round_to(duration / 60.0, 1),
                "recorded_at": r.get("recording_at").and_then(|v| v.as_str())
                    .or_else(|| r.get("created_at").and_then(|v| v.as_str())),
                "tags": r.get("tags").and_then(|t| t.as_array()).map(|arr| {
                    arr.iter().filter_map(|tag| tag.get("name").and_then(|v| v.as_str()).map(String::from)).collect::<Vec<_>>()
                }).unwrap_or_default(),
            })
        })
        .collect();

    json!({
        "recordings_count": data.len(),
        "total_duration_minutes": round_to(total_duration_secs / 60.0, 1),
        "tag_breakdown": tag_breakdown,
        "recent_recordings": recent_recordings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_audio_url_reads_the_usual_shapes_and_only_http() {
        assert_eq!(signed_audio_url(&json!({"url": "https://s3.example/a.m4a?sig=1"})).as_deref(), Some("https://s3.example/a.m4a?sig=1"));
        assert_eq!(signed_audio_url(&json!({"data": {"audio_url": " https://cdn.example/b.mp3 "}})).as_deref(), Some("https://cdn.example/b.mp3"));
        assert_eq!(signed_audio_url(&json!({"signed_url": "file:///etc/passwd"})), None);
        assert_eq!(signed_audio_url(&json!({"status": "processing"})), None);
    }

    #[test]
    fn bearer_sets_authorization() {
        assert_eq!(bearer("abc123").get(AUTHORIZATION).unwrap(), "Bearer abc123");
    }

    #[test]
    fn require_names_the_missing_credential() {
        let err = require(&json!({"api_key": "  "}), "api_key").unwrap_err();
        assert!(err.message.contains("missing api_key"), "{}", err.message);
        assert_eq!(require(&json!({"api_key": " k "}), "api_key").unwrap(), "k");
    }

    #[test]
    fn api_base_url_defaults_and_overrides() {
        let api = Api::new(&json!({}), "https://api.example.com/v1/", HeaderMap::new());
        assert_eq!(api.url("/x"), "https://api.example.com/v1/x");
        let api = Api::new(&json!({"base_url": "http://127.0.0.1:9/"}), "https://api.example.com", HeaderMap::new());
        assert_eq!(api.url("/x"), "http://127.0.0.1:9/x");
        assert_eq!(api.url("https://other/next?page=2"), "https://other/next?page=2");
    }

    #[test]
    fn up_bank_client_requires_a_token() {
        assert!(UpBankClient::new(&json!({}), &json!({})).is_err());
        assert!(UpBankClient::new(&json!({"personal_access_token": "up:yeah:abc"}), &json!({})).is_ok());
    }

    #[tokio::test]
    async fn a_short_429_is_retried_once_then_reported() {
        use crate::sources::mock::{route, serve};
        let mock = serve(vec![route("GET", "/x", json!({"message": "slow down"})).status(429).header("retry-after", "0")]).await;
        let api = Api::new(&json!({"base_url": mock.base}), "", HeaderMap::new());
        let err = api.get("/x", &[]).await.unwrap_err();
        assert!(err.message.contains("HTTP 429 (rate limited"), "{}", err.message);
        assert_eq!(mock.requests().len(), 2);
    }

    #[tokio::test]
    async fn http_status_takes_precedence_over_a_truncated_body() {
        use crate::sources::mock::{route, serve};
        for (status, class) in [(429, "connector_rate_limited"), (401, "connector_auth"), (403, "connector_auth"),
            (404, "connector_removed"), (410, "connector_removed"), (400, "connector_message_error"),
            (500, "connector_server_error"), (200, "connector_response_error")] {
            let mock = serve(vec![route("GET", "/x", json!({})).status(status).header("retry-after", "60").truncated()]).await;
            let api = Api::new(&json!({"base_url": mock.base}), "", HeaderMap::new()).with_google_retries();
            let error = api.get("/x", &[]).await.unwrap_err();
            assert_eq!(error.reason, Some(class), "HTTP {status}: {error:?}");
            assert_eq!(error.retry_after_secs, Some(60));
            assert_eq!(mock.requests().len(), 1);
            if status != 200 { assert!(error.message.contains(&format!("HTTP {status}"))); }
        }
    }

    #[tokio::test]
    async fn no_http_response_is_a_transient_network_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let api = Api::new(&json!({"base_url": base}), "", HeaderMap::new());
        assert_eq!(api.get("/x", &[]).await.unwrap_err().reason, Some("connector_transient"));
    }

    #[test]
    fn google_quota_reasons_are_distinct_from_permissions() {
        for reason in ["rateLimitExceeded", "userRateLimitExceeded", "quotaExceeded"] {
            let body = json!({"error": {"errors": [{"reason": reason}]}}).to_string();
            assert!(retryable(&provider_error("https://google.test", StatusCode::FORBIDDEN, &body)), "{reason}");
        }
        for body in [r#"{"error":{"errors":[{"reason":"insufficientPermissions"}]}}"#, "not JSON"] {
            assert!(!retryable(&provider_error("https://google.test", StatusCode::FORBIDDEN, body)));
        }
        assert!(rate_limited(StatusCode::TOO_MANY_REQUESTS, "Too many concurrent requests for user"));
        assert!(!rate_limited(StatusCode::UNAUTHORIZED, r#"{"error":{"errors":[{"reason":"userRateLimitExceeded"}]}}"#));
    }

    #[tokio::test]
    async fn google_retries_403_and_429_with_and_without_retry_after() {
        use crate::sources::mock::{route, serve};
        for status in [403, 429, 503] {
            for retry_after in [None, Some("0.01")] {
                let mut limited = route("GET", "/x", json!({"error": {"errors": [{"reason": "userRateLimitExceeded"}]}})).status(status).times(1);
                if let Some(value) = retry_after { limited = limited.header("retry-after", value); }
                let mock = serve(vec![limited, route("GET", "/x", json!({"ok": true}))]).await;
                let api = Api::new(&json!({"base_url": mock.base}), "", HeaderMap::new()).with_google_retries();
                assert_eq!(api.get("/x", &[]).await.unwrap()["ok"], true);
                assert_eq!(mock.requests().len(), 2);
            }
        }
    }

    #[tokio::test]
    async fn retry_budget_is_shared_across_requests_and_honours_long_retry_after() {
        use crate::sources::mock::{route, serve};
        let mock = serve(vec![
            route("GET", "/long", json!({})).status(429).header("retry-after", "60"),
            route("GET", "/x", json!({})).status(429),
        ]).await;
        let api = Api::new(&json!({"base_url": mock.base}), "", HeaderMap::new()).with_google_retries();
        let started = tokio::time::Instant::now();
        assert!(retryable(&api.get("/long", &[]).await.unwrap_err()));
        assert!(started.elapsed() < Duration::from_secs(1), "do not wait less than Retry-After or exceed the budget");
        api.retry_budget.as_ref().unwrap().lock().await.retries = 1;
        assert!(retryable(&api.get("/x", &[]).await.unwrap_err()));
        assert!(retryable(&api.clone().get("/x", &[]).await.unwrap_err()));
        assert_eq!(mock.requests().len(), 4, "one long, two for first x, one for second x");
    }

    #[test]
    fn full_jitter_is_capped_and_wait_budget_is_finite_even_with_zero_delays() {
        for attempt in 0..12 {
            assert!(jitter(attempt) <= Duration::from_secs(1 << attempt.min(3)));
        }
        let mut budget = RetryBudget { remaining: Duration::from_secs(3), retries: 4 };
        assert!(!budget.reserve(Duration::from_secs(4)));
        assert!(budget.reserve(Duration::from_secs(3)));
        for _ in 0..3 { assert!(budget.reserve(Duration::ZERO)); }
        assert!(!budget.reserve(Duration::ZERO));
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("Wed, 01 Jan 2098 00:00:00 GMT"));
        assert!(retry_after(&headers).unwrap() > Duration::from_secs(10));
    }

    #[tokio::test(start_paused = true)]
    async fn pacing_is_local_to_the_api_and_shared_by_clones() {
        let api = Api::new(&json!({}), "https://example.com", HeaderMap::new()).with_pacing(Duration::from_secs(1));
        let start = tokio::time::Instant::now();
        api.pace().await;
        api.clone().pace().await;
        assert_eq!(start.elapsed(), Duration::from_secs(1));
        let other = Api::new(&json!({}), "https://example.com", HeaderMap::new()).with_pacing(Duration::from_secs(1));
        other.pace().await;
        assert_eq!(start.elapsed(), Duration::from_secs(1));
    }

    #[test]
    fn provider_error_names_host_status_and_hint() {
        let e = provider_error("https://api.github.com/issues", StatusCode::UNAUTHORIZED, "{\"message\":\"Bad credentials\"}");
        assert!(e.message.starts_with("api.github.com returned HTTP 401 (check the token"), "{}", e.message);
        assert!(e.message.contains("Bad credentials"));
        // a JSON body is reduced to the provider's message, not echoed raw
        assert!(!e.message.contains('{'), "{}", e.message);
        let plain = provider_error("https://x.test/a", StatusCode::BAD_GATEWAY, "upstream down");
        assert!(plain.message.ends_with(": upstream down"), "{}", plain.message);
        let nested = provider_error("https://x.test/a", StatusCode::BAD_REQUEST, "{\"error\":{\"message\":\"bad model\"}}");
        assert!(nested.message.ends_with(": bad model"), "{}", nested.message);
    }

    #[test]
    fn compute_week_summary_counts_settled_spend_only() {
        let transactions = json!({
            "data": [
                {"attributes": {"status": "SETTLED", "amount": {"valueInBaseUnits": -500}}},
                {"attributes": {"status": "SETTLED", "amount": {"valueInBaseUnits": 1000}}},
                {"attributes": {"status": "PENDING", "amount": {"valueInBaseUnits": -999}}},
            ]
        });
        let summary = compute_week_summary(&transactions);
        assert_eq!(summary["transaction_count"], 2);
        assert_eq!(summary["spent"], 5.0);
    }

    #[test]
    fn compute_finance_summary_breaks_down_spend_by_category_and_day() {
        let accounts = json!({
            "data": [
                {"attributes": {"displayName": "Spending", "balance": {"value": "100.00", "valueInBaseUnits": 10000}}},
            ]
        });
        let categories = json!({"data": [{"id": "groceries", "attributes": {"name": "Groceries"}}]});
        let transactions = json!({
            "data": [
                {
                    "attributes": {
                        "status": "SETTLED",
                        "description": "Woolworths",
                        "amount": {"value": "-50.00", "valueInBaseUnits": -5000},
                        "createdAt": "2024-01-02T10:00:00Z"
                    },
                    "relationships": {"category": {"data": {"id": "groceries"}}}
                },
                {
                    "attributes": {
                        "status": "SETTLED",
                        "description": "Mystery shop",
                        "amount": {"value": "-10.00", "valueInBaseUnits": -1000},
                        "createdAt": "2024-01-03T10:00:00Z"
                    },
                    "relationships": {}
                },
            ]
        });

        let summary = compute_finance_summary(&accounts, &categories, &transactions);
        assert_eq!(summary["balance"], 100.0);
        assert_eq!(summary["spend_by_category"][0]["category"], "Groceries");
        assert_eq!(summary["spend_by_category"][0]["amount"], 50.0);
        assert_eq!(summary["spend_by_category"][1]["category"], "Uncategorised");
        assert_eq!(summary["spend_by_day"].as_array().unwrap().len(), 2);
        assert_eq!(summary["recent_transactions"].as_array().unwrap().len(), 2);
        // Most recent first.
        assert_eq!(summary["recent_transactions"][0]["description"], "Mystery shop");
    }

    #[test]
    fn compute_pocketai_summary_tallies_tags_and_duration() {
        let recordings = json!({
            "data": [
                {"title": "Standup", "duration": 600.0, "recording_at": "2024-01-02T00:00:00Z", "tags": [{"name": "work"}]},
                {"title": "1:1", "duration": 1800.0, "recording_at": "2024-01-03T00:00:00Z", "tags": [{"name": "work"}, {"name": "1:1"}]},
            ]
        });
        let summary = compute_pocketai_summary(&recordings);
        assert_eq!(summary["recordings_count"], 2);
        assert_eq!(summary["total_duration_minutes"], 40.0);
        assert_eq!(summary["tag_breakdown"][0]["tag"], "work");
        assert_eq!(summary["tag_breakdown"][0]["count"], 2);
        assert_eq!(summary["recent_recordings"][0]["title"], "1:1");
    }
}
