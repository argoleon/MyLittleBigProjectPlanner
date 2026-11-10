//! `Idempotency-Key` handling for creation endpoints (specification §15).
//!
//! A client that is unsure whether its `POST` went through (timeout, network
//! failure) sends it again with the same `Idempotency-Key`: the server
//! returns the first response instead of creating a duplicate. See
//! `reference_items::create` for the full sequence, and `infra::idempotency`
//! for storage.

use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use infra::idempotency::StoredResponse;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::extract::header_str;
use crate::problem::{ErrorCode, Problem};

pub const HEADER: &str = "idempotency-key";
/// Set on a replayed response, so clients can tell it from a fresh creation.
pub const REPLAYED_HEADER: &str = "idempotent-replayed";

/// The request's key, if it sent one. Keys are optional but recommended.
pub fn key(headers: &HeaderMap) -> Result<Option<String>, Problem> {
    if !headers.contains_key(HEADER) {
        return Ok(None);
    }
    match header_str(headers, HEADER) {
        Some(key) if domain::idempotency::is_valid_key(key) => Ok(Some(key.to_owned())),
        _ => Err(Problem::new(ErrorCode::IdempotencyKeyInvalid)
            .with_detail("use 1 to 255 visible ASCII characters, for example a UUID")),
    }
}

/// Fingerprint of a request: the same key must come with the same operation
/// and the same (parsed) body. Whitespace or key order in the JSON sent does
/// not matter, as the hash covers the parsed body re-serialized.
pub fn request_hash(operation: &str, body: &impl Serialize) -> Result<Vec<u8>, Problem> {
    let body = serde_json::to_vec(body).map_err(|e| Problem::internal(&e))?;
    let mut hasher = Sha256::new();
    hasher.update(operation.as_bytes());
    hasher.update([0]);
    hasher.update(&body);
    Ok(hasher.finalize().to_vec())
}

/// The stored response of the original request, marked as replayed.
pub fn replay(stored: StoredResponse) -> Response {
    let status = StatusCode::from_u16(stored.status).unwrap_or(StatusCode::OK);
    let mut response = (status, axum::Json(stored.body)).into_response();
    let headers = response.headers_mut();
    if let Some(location) = stored.location.and_then(|l| HeaderValue::from_str(&l).ok()) {
        headers.insert(header::LOCATION, location);
    }
    if let Some(etag) = stored.etag.and_then(|e| HeaderValue::from_str(&e).ok()) {
        headers.insert(header::ETAG, etag);
    }
    headers.insert(REPLAYED_HEADER, HeaderValue::from_static("true"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn absent_key_is_allowed() {
        assert_eq!(key(&HeaderMap::new()), Ok(None));
    }

    #[test]
    fn valid_and_invalid_keys() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HEADER,
            HeaderValue::from_static("8b4f1c1e-6a0c-4a0e-9f55-0d5c2b8f9e11"),
        );
        assert_eq!(
            key(&headers),
            Ok(Some("8b4f1c1e-6a0c-4a0e-9f55-0d5c2b8f9e11".to_owned()))
        );

        headers.insert(HEADER, HeaderValue::from_static(""));
        assert_eq!(key(&headers).unwrap_err().code, ErrorCode::IdempotencyKeyInvalid);
        headers.insert(HEADER, HeaderValue::from_static("has space"));
        assert_eq!(key(&headers).unwrap_err().code, ErrorCode::IdempotencyKeyInvalid);
    }

    #[test]
    fn hash_depends_on_operation_and_body_only() {
        let a = request_hash("POST /x", &json!({"a": 1})).unwrap();
        assert_eq!(a.len(), 32);
        assert_eq!(a, request_hash("POST /x", &json!({"a": 1})).unwrap());
        assert_ne!(a, request_hash("POST /y", &json!({"a": 1})).unwrap());
        assert_ne!(a, request_hash("POST /x", &json!({"a": 2})).unwrap());
    }

    #[test]
    fn replay_restores_status_headers_and_marks_it() {
        let response = replay(StoredResponse {
            status: 201,
            body: json!({"id": "x"}),
            location: Some("/api/v1/things/x".to_owned()),
            etag: Some("\"1\"".to_owned()),
        });
        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(response.headers()[header::LOCATION], "/api/v1/things/x");
        assert_eq!(response.headers()[header::ETAG], "\"1\"");
        assert_eq!(response.headers()[REPLAYED_HEADER], "true");
    }
}
