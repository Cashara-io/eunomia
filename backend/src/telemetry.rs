//! Logging, request tracing and optional OpenTelemetry export.
//!
//! Every request gets a W3C trace id: the incoming `traceparent`'s if valid,
//! else the OpenTelemetry-generated one (OTLP on) or a random one (OTLP off).
//! It is echoed in the `x-trace-id` header, in problem+json bodies and in tool
//! errors, and is the trace id of the exported spans, so a user-visible id
//! resolves to a trace.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use axum::{
    extract::{MatchedPath, Request},
    http::HeaderValue,
    middleware::Next,
    response::Response,
};
use opentelemetry::trace::{SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState, TracerProvider};
use opentelemetry_sdk::{trace::SdkTracerProvider, Resource};
use tracing::{field::Empty, Instrument};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

tokio::task_local! {
    static TRACE_ID: String;
}

static OTEL_ON: AtomicBool = AtomicBool::new(false);

fn new_trace_id() -> String {
    // a v4 uuid has fixed version bits, so it is never the invalid all-zero id
    format!("{:032x}", uuid::Uuid::new_v4().as_u128())
}

/// The current request's trace id (32 lowercase hex). Outside a request
/// (unit tests, background jobs) this is a fresh valid id.
pub fn current_trace_id() -> String {
    TRACE_ID.try_with(String::clone).unwrap_or_else(|_| new_trace_id())
}

/// The current trace as a W3C `traceparent`, stored on a job row so every
/// attempt links back to the request that enqueued it.
pub fn current_traceparent() -> String {
    let (mut trace, mut span_id, mut flags) = (current_trace_id(), format!("{:016x}", rand::random::<u64>() | 1), 1u8);
    if OTEL_ON.load(Ordering::Relaxed) {
        let sc = tracing::Span::current().context().span().span_context().clone();
        if sc.is_valid() {
            (trace, span_id, flags) = (sc.trace_id().to_string(), sc.span_id().to_string(), sc.trace_flags().to_u8());
        }
    }
    format!("00-{trace}-{span_id}-{flags:02x}")
}

/// The trace id inside a `traceparent`, if it is valid.
pub fn trace_id_of(traceparent: &str) -> Option<String> {
    parse_traceparent(traceparent).map(|t| t.trace_id)
}

/// Adds an OpenTelemetry span link from `span` to the span in `traceparent` (no-op when OTLP is off).
pub fn link_span(span: &tracing::Span, traceparent: &str) {
    if !OTEL_ON.load(Ordering::Relaxed) {
        return;
    }
    if let Some(i) = parse_traceparent(traceparent)
        && let (Ok(t), Ok(p)) = (TraceId::from_hex(&i.trace_id), SpanId::from_hex(&i.parent_id))
    {
        span.add_link(SpanContext::new(t, p, TraceFlags::new(i.flags), true, TraceState::default()));
    }
}

/// Runs `f` with `trace_id` as the current trace id, so store queries and error bodies carry it.
pub async fn with_trace_id<F: std::future::Future>(trace_id: String, f: F) -> F::Output {
    TRACE_ID.scope(trace_id, f).await
}

/// Records the authenticated user on the request span and its closing log line.
pub fn record_user(user_id: &str) {
    tracing::Span::current().record("user_id", user_id);
}

struct Traceparent {
    trace_id: String,
    parent_id: String,
    flags: u8,
}

/// `00-<32 hex>-<16 hex>-<2 hex>`, lowercase, no all-zero ids (W3C Trace Context).
fn parse_traceparent(value: &str) -> Option<Traceparent> {
    let mut parts = value.trim().split('-');
    let (version, trace_id, parent_id, flags) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
    let hex = |s: &str, len: usize| s.len() == len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !hex(version, 2) || version == "ff" || !hex(trace_id, 32) || !hex(parent_id, 16) || !hex(flags, 2) {
        return None;
    }
    if version == "00" && parts.next().is_some() {
        return None;
    }
    if trace_id.bytes().all(|b| b == b'0') || parent_id.bytes().all(|b| b == b'0') {
        return None;
    }
    Some(Traceparent {
        trace_id: trace_id.to_string(),
        parent_id: parent_id.to_string(),
        flags: u8::from_str_radix(flags, 16).ok()?,
    })
}

/// Axum middleware: continues or starts a trace, opens the request span, sets
/// `x-trace-id` and logs one line per request (method, route, status, latency, user).
pub async fn trace_request(req: Request, next: Next) -> Response {
    let incoming = req.headers().get("traceparent").and_then(|v| v.to_str().ok()).and_then(parse_traceparent);
    let method = req.method().to_string();
    let route = req.extensions().get::<MatchedPath>().map_or("unmatched", MatchedPath::as_str).to_string();
    let span = tracing::info_span!(
        "http.request",
        otel.name = %format!("{method} {route}"),
        http.request.method = %method,
        http.route = %route,
        trace_id = Empty,
        user_id = Empty,
        http.response.status_code = Empty,
    );

    let mut trace_id = incoming.as_ref().map(|i| i.trace_id.clone());
    if OTEL_ON.load(Ordering::Relaxed) {
        if let Some(i) = &incoming
            && let (Ok(t), Ok(p)) = (TraceId::from_hex(&i.trace_id), SpanId::from_hex(&i.parent_id))
        {
            let remote = SpanContext::new(t, p, TraceFlags::new(i.flags), true, TraceState::default());
            let _ = span.set_parent(opentelemetry::Context::new().with_remote_span_context(remote));
        }
        let sc = span.context().span().span_context().clone();
        if sc.is_valid() {
            trace_id = Some(sc.trace_id().to_string());
        }
    }
    let trace_id = trace_id.unwrap_or_else(new_trace_id);
    span.record("trace_id", trace_id.as_str());

    let started = Instant::now();
    let mut resp = TRACE_ID.scope(trace_id.clone(), next.run(req).instrument(span.clone())).await;

    let status = resp.status();
    span.record("http.response.status_code", status.as_u16());
    let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
    span.in_scope(|| {
        if route == "/healthz" {
            tracing::debug!(status = status.as_u16(), latency_ms, "request");
        } else {
            tracing::info!(status = status.as_u16(), latency_ms, "request");
        }
    });
    if let Ok(v) = HeaderValue::from_str(&trace_id) {
        resp.headers_mut().insert("x-trace-id", v);
    }
    resp
}

/// JSON logs honouring `RUST_LOG`, else `LOG_LEVEL`. OTLP export turns on only
/// when `OTEL_EXPORTER_OTLP_ENDPOINT` is set; keep the returned provider alive.
pub fn init(log_level: &str) -> Option<SdkTracerProvider> {
    std::sync::LazyLock::force(&crate::diagnostics::START);
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(log_level.to_lowercase()))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let mut otel_error = None;
    let provider = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .ok()
        .filter(|e| !e.trim().is_empty())
        .and_then(|_| build_provider().map_err(|e| otel_error = Some(e)).ok());
    let otel_layer = provider.as_ref().map(|p| tracing_opentelemetry::layer().with_tracer(p.tracer("eunomia-backend")));
    OTEL_ON.store(provider.is_some(), Ordering::Relaxed);

    tracing_subscriber::registry()
        .with(otel_layer)
        .with(filter)
        .with(crate::diagnostics::logs::SafeLogs { ring: crate::diagnostics::logs::LOGS.clone(), stdout: true })
        .init();
    if let Some(e) = otel_error {
        tracing::warn!("OTLP export disabled: {e}");
    }
    provider
}

fn build_provider() -> Result<SdkTracerProvider, String> {
    let exporter = opentelemetry_otlp::SpanExporter::builder().with_http().build().map_err(|e| e.to_string())?;
    let name = std::env::var("OTEL_SERVICE_NAME").ok().filter(|n| !n.is_empty()).unwrap_or_else(|| "eunomia-backend".into());
    Ok(SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(Resource::builder().with_service_name(name).build())
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traceparent_parsing() {
        let ok = parse_traceparent("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01").unwrap();
        assert_eq!(ok.trace_id, "0af7651916cd43dd8448eb211c80319c");
        assert_eq!(ok.flags, 1);
        for bad in [
            "",
            "garbage",
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331",
            "00-00000000000000000000000000000000-b7ad6b7169203331-01",
            "00-0AF7651916CD43DD8448EB211C80319C-b7ad6b7169203331-01",
            "ff-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01-extra",
        ] {
            assert!(parse_traceparent(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn fresh_trace_ids_are_32_hex() {
        let id = current_trace_id();
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    }
}
