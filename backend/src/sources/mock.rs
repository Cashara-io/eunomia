//! Test-only stand-in for a provider's HTTP API: serves canned JSON (copied
//! from each provider's documented response shapes) on 127.0.0.1, records
//! every request so tests can assert auth headers and pagination params, and
//! hands back its base URL to put in a connector's `config.base_url`.
//!
//! `{base}` anywhere in a canned body or header is replaced with the mock's
//! own base URL, for providers whose "next page" links are absolute.

use std::sync::{Arc, Mutex};

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    Router,
};
use serde_json::Value;

use crate::cache::search::Envelope;
use crate::sources::base::{Conn, Source};

#[derive(Clone)]
pub struct Route {
    method: Method,
    path: String,
    query: Vec<&'static str>,
    body_has: Vec<&'static str>,
    delay_ms: u64,
    status: u16,
    body: Value,
    headers: Vec<(&'static str, String)>,
    /// Answer with this text verbatim (and `content-type` from [`Route::header`]) instead of `body`.
    raw: Option<String>,
    remaining: Option<usize>,
    truncated: bool,
}

pub fn route(method: &str, path: &str, body: Value) -> Route {
    Route { method: method.parse().unwrap(), path: path.to_string(), query: vec![], body_has: vec![], delay_ms: 0, status: 200, body, headers: vec![], raw: None, remaining: None, truncated: false }
}

impl Route {
    /// Send headers and a body prefix, then break the response stream.
    pub fn truncated(mut self) -> Self {
        self.truncated = true;
        self
    }

    /// Answer only this many matching requests, then try the next matching route.
    pub fn times(mut self, count: usize) -> Self {
        self.remaining = Some(count);
        self
    }

    /// Only match requests whose raw query string contains `fragment`.
    pub fn query_has(mut self, fragment: &'static str) -> Self {
        self.query.push(fragment);
        self
    }

    /// Only match requests whose body contains `fragment`.
    pub fn body(mut self, fragment: &'static str) -> Self {
        self.body_has.push(fragment);
        self
    }

    /// Answer after `ms` milliseconds (a slow provider).
    pub fn delay(mut self, ms: u64) -> Self {
        self.delay_ms = ms;
        self
    }

    pub fn status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    /// Answer with `text` as is (a binary download), not the JSON body.
    pub fn raw(mut self, text: &str) -> Self {
        self.raw = Some(text.to_string());
        self
    }

    pub fn header(mut self, name: &'static str, value: &str) -> Self {
        self.headers.push((name, value.to_string()));
        self
    }
}

#[derive(Debug, Clone)]
pub struct Req {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: HeaderMap,
    pub body: String,
}

impl Req {
    pub fn header(&self, name: &str) -> &str {
        self.headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or("")
    }
}

#[derive(Clone)]
struct MockState {
    base: String,
    routes: Arc<Mutex<Vec<Route>>>,
    requests: Arc<Mutex<Vec<Req>>>,
}

pub struct Mock {
    pub base: String,
    routes: Arc<Mutex<Vec<Route>>>,
    requests: Arc<Mutex<Vec<Req>>>,
}

impl Mock {
    /// Answers with `routes` from now on: for a reply that names something made after the mock started.
    pub fn set_routes(&self, routes: Vec<Route>) {
        *self.routes.lock().unwrap() = routes;
    }

    pub fn requests(&self) -> Vec<Req> {
        self.requests.lock().unwrap().clone()
    }

    /// A connector pointed at this mock.
    pub fn conn(&self, credentials: Value) -> Conn {
        Conn { credentials, config: serde_json::json!({"base_url": self.base, "token_url": format!("{}/oauth/token", self.base)}) }
    }
}

async fn handle(State(st): State<MockState>, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    let query = uri.to_string().split_once('?').map_or(String::new(), |(_, q)| q.to_string());
    let body = String::from_utf8_lossy(&body).to_string();
    st.requests.lock().unwrap().push(Req {
        method: method.to_string(),
        path: uri.path().to_string(),
        query: query.clone(),
        headers,
        body: body.clone(),
    });
    let decoded = decode_query(&query);
    let found = st
        .routes
        .lock()
        .unwrap()
        .iter_mut()
        .find(|r| {
            r.remaining != Some(0) &&
            r.method == method
                && r.path == uri.path()
                && r.query.iter().all(|q| decoded.contains(q))
                && r.body_has.iter().all(|b| body.contains(b))
        })
        .map(|r| {
            if let Some(remaining) = &mut r.remaining { *remaining -= 1; }
            r.clone()
        });
    let Some(r) = found else {
        return (StatusCode::NOT_FOUND, format!("mock: no route for {method} {uri}")).into_response();
    };
    tokio::time::sleep(std::time::Duration::from_millis(r.delay_ms)).await;
    let out = r.raw.clone().unwrap_or_else(|| r.body.to_string().replace("{base}", &st.base));
    let mut resp = (StatusCode::from_u16(r.status).unwrap(), [("content-type", "application/json")], out).into_response();
    if r.truncated {
        *resp.body_mut() = axum::body::Body::from_stream(futures::stream::unfold(0, |step| async move {
            match step {
                0 => Some((Ok(Bytes::from_static(b"{")), 1)),
                1 => {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    Some((Err(std::io::Error::other("truncated response")), 2))
                }
                _ => None,
            }
        }));
        resp.headers_mut().remove("content-length");
    }
    for (k, v) in &r.headers {
        resp.headers_mut().insert(*k, v.replace("{base}", &st.base).parse().unwrap());
    }
    resp
}

/// Query strings arrive percent-encoded (`filter%5Bsince%5D=...`); routes are
/// written decoded, so match against the decoded form.
fn decode_query(q: &str) -> String {
    serde_urlencoded::from_str::<Vec<(String, String)>>(q)
        .map(|pairs| pairs.into_iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&"))
        .unwrap_or_else(|_| q.to_string())
}

pub async fn serve(routes: Vec<Route>) -> Mock {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let routes = Arc::new(Mutex::new(routes));
    let state = MockState { base: base.clone(), routes: routes.clone(), requests: requests.clone() };
    let app = Router::new().fallback(handle).with_state(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Mock { base, routes, requests }
}

/// Maps raw records exactly as the ingest pipeline does (`Source::map`, then
/// deserializing into the cache `Envelope`), so a mapper emitting something
/// ingest would reject -- a non-RFC3339 `occurred_at`, a missing id -- fails
/// the test instead of the user's sync.
pub fn envelopes(src: &dyn Source, records: &[Value]) -> Vec<Envelope> {
    records
        .iter()
        .filter_map(|r| src.map(r))
        .map(|v| serde_json::from_value::<Envelope>(v.clone()).unwrap_or_else(|e| panic!("bad envelope {v}: {e}")))
        .collect()
}

/// Asserts a failed fetch produced a readable error mentioning `needle`
/// (e.g. "HTTP 401") rather than panicking or silently succeeding.
pub async fn assert_fetch_fails(src: &dyn Source, status: u16, path: &str, credentials: Value, needle: &str) {
    let mock = serve(vec![
        route("GET", path, serde_json::json!({"message": "nope"})).status(status),
        route("POST", path, serde_json::json!({"message": "nope"})).status(status),
        route("POST", "/oauth/token", serde_json::json!({"access_token": "fresh"})),
    ])
    .await;
    let err = src.fetch(&mock.conn(credentials), None).await.expect_err("fetch should fail");
    assert!(err.message.contains(needle), "error {:?} should mention {needle:?}", err.message);
}
