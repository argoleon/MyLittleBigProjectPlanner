//! What ties a request's logs, traces and metrics together.
//!
//! - **Correlation ID**: `x-request-id`, taken from the caller when it sends
//!   a usable one, generated otherwise (UUID v4), returned in the response
//!   and recorded on every log line of the request.
//! - **Trace context**: a W3C `traceparent` header makes the request's trace
//!   continue the caller's (the MCP server calling the API, L16).
//! - **Client IP**: the first `X-Forwarded-For` address, set by Traefik.
//!   Technical logs holding it follow the log retention (deploy/README.md).

use axum::http::{HeaderMap, HeaderValue, Request};
use opentelemetry::propagation::Extractor;
use tower_http::request_id::{MakeRequestId, RequestId};
use uuid::Uuid;

/// Header carrying the correlation ID of a request.
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Longest correlation ID accepted from a caller.
const MAX_REQUEST_ID_LEN: usize = 128;

/// Whether a caller-provided correlation ID can be kept: 1 to 128 visible
/// ASCII characters (so it is safe in logs and headers).
pub fn is_valid_request_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_REQUEST_ID_LEN && value.bytes().all(|b| b.is_ascii_graphic())
}

/// Generates a correlation ID (UUID v4) for requests that bring none.
#[derive(Debug, Clone, Copy, Default)]
pub struct MakeCorrelationId;

impl MakeRequestId for MakeCorrelationId {
    fn make_request_id<B>(&mut self, _request: &Request<B>) -> Option<RequestId> {
        HeaderValue::from_str(&Uuid::new_v4().to_string())
            .ok()
            .map(RequestId::new)
    }
}

/// Drops a caller's correlation ID that is not usable, so that one is
/// generated instead. Runs before `SetRequestIdLayer`.
pub fn sanitize_request_id(headers: &mut HeaderMap) {
    let usable = headers
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .is_some_and(is_valid_request_id);
    if !usable {
        headers.remove(REQUEST_ID_HEADER);
    }
}

/// The client's IP address as Traefik reports it: the first address of
/// `X-Forwarded-For`, if it is one.
pub fn client_ip(headers: &HeaderMap) -> Option<std::net::IpAddr> {
    headers
        .get("x-forwarded-for")?
        .to_str()
        .ok()?
        .split(',')
        .next()?
        .trim()
        .parse()
        .ok()
}

/// Reads W3C trace context headers for the OpenTelemetry propagator.
#[derive(Debug)]
pub struct HeaderExtractor<'a>(pub &'a HeaderMap);

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(axum::http::HeaderName::as_str).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_request_ids_are_kept_when_usable() {
        assert!(is_valid_request_id("abc-123"));
        assert!(is_valid_request_id(&"x".repeat(128)));
        assert!(!is_valid_request_id(""));
        assert!(!is_valid_request_id(&"x".repeat(129)));
        assert!(!is_valid_request_id("two words"));
        assert!(!is_valid_request_id("line\nbreak"));
        assert!(!is_valid_request_id("é"));
    }

    #[test]
    fn unusable_request_ids_are_dropped() {
        let mut headers = HeaderMap::new();
        headers.insert(REQUEST_ID_HEADER, HeaderValue::from_static("has space"));
        sanitize_request_id(&mut headers);
        assert!(!headers.contains_key(REQUEST_ID_HEADER));

        headers.insert(REQUEST_ID_HEADER, HeaderValue::from_static("abc-123"));
        sanitize_request_id(&mut headers);
        assert_eq!(headers[REQUEST_ID_HEADER], "abc-123");
    }

    #[test]
    fn generated_request_ids_are_uuids() {
        let request = Request::new(());
        let id = MakeCorrelationId.make_request_id(&request).unwrap();
        let text = id.header_value().to_str().unwrap();
        assert!(Uuid::parse_str(text).is_ok(), "{text}");
        let other = MakeCorrelationId.make_request_id(&request).unwrap();
        assert_ne!(id.header_value(), other.header_value());
    }

    #[test]
    fn client_ip_comes_from_the_first_forwarded_address() {
        let mut headers = HeaderMap::new();
        assert_eq!(client_ip(&headers), None);
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("203.0.113.7, 10.0.0.2"),
        );
        assert_eq!(client_ip(&headers), Some("203.0.113.7".parse().unwrap()));
        headers.insert("x-forwarded-for", HeaderValue::from_static("2001:db8::1"));
        assert_eq!(client_ip(&headers), Some("2001:db8::1".parse().unwrap()));
        headers.insert("x-forwarded-for", HeaderValue::from_static("not-an-ip"));
        assert_eq!(client_ip(&headers), None);
    }

    #[test]
    fn trace_context_headers_are_readable() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "traceparent",
            HeaderValue::from_static("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
        );
        let extractor = HeaderExtractor(&headers);
        assert_eq!(
            extractor.get("traceparent"),
            Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
        );
        assert_eq!(extractor.keys(), vec!["traceparent"]);
    }
}
