//! Optional real Next integration proof. Uses a fresh in-memory backend, never a shared server.
mod common;

#[tokio::test]
#[ignore = "requires bun and installed frontend dependencies; run with --ignored"]
async fn next_streams_only_after_real_backend_authentication() {
    let app = common::TestApp::with_settings(eunomia_backend::config::Settings {
        surreal_url: "mem://".into(), documents_max_bytes: 25 << 20, ..common::test_settings()
    }).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let marker = uuid::Uuid::new_v4().to_string();
    let header = axum::http::HeaderValue::from_str(&marker).unwrap();
    let router = app.router.layer(axum::middleware::map_response(move |mut response: axum::response::Response| {
        let header = header.clone();
        async move {
            response.headers_mut().insert("x-eunomia-proxy-fixture", header);
            response
        }
    }));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let frontend = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../frontend");
    // Next config rewrites are baked into the production build. Build against this fixture too.
    if std::env::var_os("PROXY_TEST_PRODUCTION").is_some() {
        let build = tokio::process::Command::new("bun").args(["run", "build"])
            .current_dir(&frontend).env("BACKEND_INTERNAL_URL", format!("http://{address}"))
            .output().await.unwrap();
        assert!(build.status.success(), "{}\n{}", String::from_utf8_lossy(&build.stdout), String::from_utf8_lossy(&build.stderr));
    }
    let output = tokio::process::Command::new("bun")
        .args(["run", "test:proxy"])
        .current_dir(frontend)
        .env("PROXY_TEST_REAL_BACKEND_URL", format!("http://{address}"))
        .env("PROXY_TEST_EXPECTED_FIXTURE", marker)
        .output().await.unwrap();
    server.abort();
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}
