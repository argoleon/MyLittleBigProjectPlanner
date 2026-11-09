//! Shared request layer applied to every route.
//!
//! Order, from the outside in: request ID (generated if absent), tracing span
//! carrying it, request ID echoed on the response, timeout, body size limit.
//! The tenant transaction will join this layer in L01-US02.

use std::time::Duration;

use axum::Router;
use axum::http::{HeaderName, StatusCode};
use infra::config::ApiConfig;
use tower::ServiceBuilder;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

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
    let layers = ServiceBuilder::new()
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
        .layer(PropagateRequestIdLayer::new(REQUEST_ID_HEADER))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            settings.request_timeout,
        ))
        .layer(RequestBodyLimitLayer::new(settings.body_limit_bytes));
    router.layer(layers)
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
