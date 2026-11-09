//! Shared request layer applied to every route.
//!
//! Order, from the outside in: unusable caller correlation IDs dropped, then
//! request ID (generated if absent), tracing span carrying it (one INFO log
//! line per request; trace continued from the caller's `traceparent`),
//! request ID echoed on the response, request duration metric, Problem
//! Details for framework errors, panic catcher, timeout, body size limit.
//! The tenant transaction is opened per handler by the `TenantTx` extractor.

use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use infra::config::ApiConfig;
use opentelemetry::metrics::Histogram;
use opentelemetry::{KeyValue, global};
use tower::ServiceBuilder;
use tower_http::LatencyUnit;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::{Level, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::correlation::{HeaderExtractor, MakeCorrelationId, client_ip, sanitize_request_id};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem};

/// Header carrying the correlation ID of a request.
pub const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static(crate::correlation::REQUEST_ID_HEADER);

/// Bucket boundaries of the request duration histogram, in seconds
/// (OpenTelemetry HTTP semantic conventions).
const DURATION_BUCKETS: [f64; 14] = [
    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];

/// HTTP limits of the request layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpSettings {
    pub request_timeout: Duration,
    pub body_limit_bytes: usize,
}

impl From<&ApiConfig> for HttpSettings {
    fn from(config: &ApiConfig) -> Self {
        Self {
            request_timeout: config.request_timeout,
            body_limit_bytes: config.body_limit_bytes,
        }
    }
}

/// Wraps `router` in the shared request layer.
pub fn with_middleware(router: Router, settings: &HttpSettings) -> Router {
    // Closest to the handlers. Their responses carry their own body types,
    // so they are applied first and converted back to axum responses.
    let inner = ServiceBuilder::new()
        .layer(CatchPanicLayer::custom(on_panic))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            settings.request_timeout,
        ))
        .layer(RequestBodyLimitLayer::new(settings.body_limit_bytes));

    let outer = ServiceBuilder::new()
        .layer(axum::middleware::map_request(|mut request: Request| async move {
            sanitize_request_id(request.headers_mut());
            request
        }))
        .layer(SetRequestIdLayer::new(REQUEST_ID_HEADER, MakeCorrelationId))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(request_span)
                .on_response(
                    DefaultOnResponse::new()
                        .level(Level::INFO)
                        .latency_unit(LatencyUnit::Millis),
                ),
        )
        .layer(PropagateRequestIdLayer::new(REQUEST_ID_HEADER))
        .layer(axum::middleware::from_fn_with_state(
            HttpMetrics::new(),
            record_duration,
        ));

    router
        .layer(inner)
        .layer(axum::middleware::map_response(problem_for_framework_errors))
        .layer(outer)
}

/// The span of a request: its log lines carry these fields, and it becomes
/// the server span of the request's trace.
fn request_span(request: &Request<Body>) -> Span {
    let headers = request.headers();
    let request_id = headers
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("", MatchedPath::as_str);
    // `tenant` is filled in by the TenantTx extractor once resolved.
    let span = tracing::info_span!(
        "request",
        method = %request.method(),
        path = %request.uri().path(),
        route,
        request_id,
        client_ip = tracing::field::Empty,
        tenant = tracing::field::Empty,
        otel.name = %format!("{} {route}", request.method()),
        otel.kind = "server",
    );
    if let Some(ip) = client_ip(headers) {
        span.record("client_ip", tracing::field::display(ip));
    }
    let parent = global::get_text_map_propagator(|propagator| propagator.extract(&HeaderExtractor(headers)));
    let _ = span.set_parent(parent);
    span
}

/// The `http.server.request.duration` histogram (OpenTelemetry semantic
/// conventions). Without a meter provider (no export), recording is a no-op.
#[derive(Clone)]
struct HttpMetrics {
    duration: Histogram<f64>,
}

impl HttpMetrics {
    fn new() -> Self {
        let duration = global::meter("api")
            .f64_histogram("http.server.request.duration")
            .with_description("Duration of HTTP requests served by the API")
            .with_unit("s")
            .with_boundaries(DURATION_BUCKETS.to_vec())
            .build();
        Self { duration }
    }
}

/// Records the duration of every request but the health probes, by method,
/// route template (`/api/v1/reference-items/{id}`, never the raw path) and
/// status code.
async fn record_duration(State(metrics): State<HttpMetrics>, request: Request, next: Next) -> Response {
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "unmatched".to_owned(), |path| path.as_str().to_owned());
    if route.starts_with("/health/") {
        return next.run(request).await;
    }
    let method = request.method().to_string();
    let started = Instant::now();
    let response = next.run(request).await;
    metrics.duration.record(
        started.elapsed().as_secs_f64(),
        &[
            KeyValue::new("http.request.method", method),
            KeyValue::new("http.route", route),
            KeyValue::new("http.response.status_code", i64::from(response.status().as_u16())),
        ],
    );
    response
}

/// Turns error responses produced outside handlers (unknown route, wrong
/// method, timeout, body too large, panic) into Problem Details, keeping
/// their other headers (`Allow`, `x-request-id`). JSON responses of handlers
/// are left untouched.
async fn problem_for_framework_errors(response: Response) -> Response {
    let status = response.status();
    // A JSON body comes from a handler (a Problem, or the readiness report):
    // left as is. Framework errors come with an empty or plain-text body.
    let is_json = response.headers().get(header::CONTENT_TYPE).is_some_and(|v| {
        let v = v.as_bytes();
        v.starts_with(PROBLEM_JSON.as_bytes()) || v.starts_with(b"application/json")
    });
    if !(status.is_client_error() || status.is_server_error()) || is_json {
        return response;
    }
    let Some(code) = ErrorCode::for_status(status) else {
        return response;
    };
    let (mut parts, _) = response.into_parts();
    let problem = Problem::new(code).into_response();
    parts.status = problem.status();
    parts.headers.remove(header::CONTENT_LENGTH);
    parts
        .headers
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(PROBLEM_JSON));
    Response::from_parts(parts, problem.into_body())
}

/// A panic in a handler is a bug: log it, answer a generic 500.
// The signature is imposed by `CatchPanicLayer::custom`.
#[allow(clippy::needless_pass_by_value)]
fn on_panic(panic: Box<dyn std::any::Any + Send + 'static>) -> Response {
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap_or("unknown panic");
    tracing::error!(panic = message, "handler panicked");
    Problem::new(ErrorCode::Internal).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, Bytes};
    use axum::http::Request;
    use axum::routing::{get, post};
    use tower::ServiceExt;

    fn settings() -> HttpSettings {
        HttpSettings {
            request_timeout: Duration::from_millis(100),
            body_limit_bytes: 16,
        }
    }

    fn test_router() -> Router {
        let routes = Router::new()
            .route("/ok", get(|| async { "ok" }))
            .route("/echo", post(|body: Bytes| async move { body }))
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    "late"
                }),
            );
        with_middleware(routes, &settings())
    }

    #[tokio::test]
    async fn generates_a_request_id_when_absent() {
        let res = test_router()
            .oneshot(Request::get("/ok").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let id = res.headers().get(REQUEST_ID_HEADER).expect("request id header");
        assert_eq!(id.to_str().unwrap().len(), 36, "a UUID is generated");
    }

    #[tokio::test]
    async fn propagates_the_caller_request_id() {
        let req = Request::get("/ok")
            .header(REQUEST_ID_HEADER, "abc-123")
            .body(Body::empty())
            .unwrap();
        let res = test_router().oneshot(req).await.unwrap();
        assert_eq!(res.headers().get(REQUEST_ID_HEADER).unwrap(), "abc-123");
    }

    #[tokio::test]
    async fn rejects_bodies_over_the_limit() {
        let req = Request::post("/echo").body(Body::from(vec![b'x'; 17])).unwrap();
        let res = test_router().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(res.headers().contains_key(REQUEST_ID_HEADER));
    }

    #[tokio::test]
    async fn accepts_bodies_within_the_limit() {
        let req = Request::post("/echo").body(Body::from(vec![b'x'; 16])).unwrap();
        let res = test_router().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn times_out_slow_requests() {
        let res = test_router()
            .oneshot(Request::get("/slow").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::REQUEST_TIMEOUT);
    }
}
