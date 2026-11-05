//! How a guest reaches a host: plain HTTP(S) requests with a short timeout. Tests register an
//! in-process router for a base URL instead (`test_route`), so two instances talk without a socket.

use std::time::Duration;

/// How long one address may take to answer `hello` before the next is tried.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);
/// How long an exchange may take.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Reply {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&self.body).ok()
    }
}

fn client() -> &'static reqwest::Client {
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(concat!("eunomia-peer/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("http client")
    })
}

#[cfg(feature = "test-support")]
static TEST_ROUTES: std::sync::Mutex<Vec<(String, axum::Router)>> = std::sync::Mutex::new(Vec::new());

/// Test-only: requests to `base` go to `router` in-process.
#[cfg(feature = "test-support")]
pub fn test_route(base: &str, router: axum::Router) {
    let mut routes = TEST_ROUTES.lock().unwrap();
    routes.retain(|(b, _)| b != base);
    routes.push((base.trim_end_matches('/').to_string(), router));
}

#[cfg(feature = "test-support")]
async fn in_process(method: &str, url: &str, body: Option<String>) -> Option<Result<Reply, String>> {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let (base, router) = TEST_ROUTES.lock().unwrap().iter().find(|(b, _)| url.starts_with(b.as_str())).cloned()?;
    let path = &url[base.len()..];
    let mut req = axum::http::Request::builder().method(method).uri(path);
    // the gate reads the client address from the connection; give it one
    req = req.extension(axum::extract::ConnectInfo(std::net::SocketAddr::from(([192, 168, 7, 7], 40000))));
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(axum::body::Body::from(b)),
        None => req.body(axum::body::Body::empty()),
    };
    let req = match req {
        Ok(r) => r,
        Err(e) => return Some(Err(e.to_string())),
    };
    let resp = match router.oneshot(req).await {
        Ok(r) => r,
        Err(e) => return Some(Err(e.to_string())),
    };
    let status = resp.status().as_u16();
    let body = resp.into_body().collect().await.map(|b| b.to_bytes().to_vec()).unwrap_or_default();
    Some(Ok(Reply { status, body }))
}

pub async fn get(url: &str, timeout: Duration) -> Result<Reply, String> {
    #[cfg(feature = "test-support")]
    if let Some(r) = in_process("GET", url, None).await {
        return r;
    }
    let resp = client().get(url).timeout(timeout).send().await.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    // a hello is tiny; anything big is not a peer
    let body = resp.bytes().await.map_err(|e| e.to_string())?.to_vec();
    Ok(Reply { status, body })
}

pub async fn post(url: &str, body: String, timeout: Duration) -> Result<Reply, String> {
    #[cfg(feature = "test-support")]
    if let Some(r) = in_process("POST", url, Some(body.clone())).await {
        return r;
    }
    let resp = client().post(url).header("content-type", "application/json").body(body).timeout(timeout).send().await.map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let body = resp.bytes().await.map_err(|e| e.to_string())?.to_vec();
    Ok(Reply { status, body })
}
