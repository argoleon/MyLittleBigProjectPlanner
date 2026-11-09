//! Shared request layer applied to every route.
//!
//! Order, from the outside in: request ID (generated if absent), tracing span
//! carrying it, request ID echoed on the response, Problem Details for
//! framework errors, panic catcher, timeout, body size limit. The tenant
//! transaction is opened per handler by the `TenantTx` extractor.

use std::time::Duration;

use axum::Router;
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use infra::config::ApiConfig;
use tower::ServiceBuilder;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::problem::{ErrorCode, PROBLEM_JSON, Problem};

/// Header carrying the correlation ID of a request.
pub const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

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
        .layer(SetRequestIdLayer::new(REQUEST_ID_HEADER, MakeRequestUuid))
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &axum::http::Request<_>| {
                let request_id = request
                    .headers()
                    .get(REQUEST_ID_HEADER)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default();
                // `tenant` is filled in by the TenantTx extractor once resolved.
                tracing::info_span!(
                    "request",
                    method = %request.method(),
                    uri = %request.uri(),
                    request_id,
                    tenant = tracing::field::Empty
                )
            }),
        )
        .layer(PropagateRequestIdLayer::new(REQUEST_ID_HEADER));

    router
        .layer(inner)
        .layer(axum::middleware::map_response(problem_for_framework_errors))
        .layer(outer)
}

/// Turns error responses produced outside handlers (unknown route, wrong
/// method, timeout, body too large, panic) into Problem Details, keeping
/// their other headers (`Allow`, `x-request-id`).
async fn problem_for_framework_errors(response: Response) -> Response {
    let status = response.status();
    let is_problem = response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|v| v.as_bytes().starts_with(PROBLEM_JSON.as_bytes()));
    if !(status.is_client_error() || status.is_server_error()) || is_problem {
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
