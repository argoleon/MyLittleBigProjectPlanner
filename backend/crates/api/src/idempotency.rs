//! `Idempotency-Key` handling for creation endpoints (specification §15).
//!
//! A client that is unsure whether its `POST` went through (timeout, network
//! failure) sends it again with the same `Idempotency-Key`: the server
//! returns the first response instead of creating a duplicate. See
//! `reference_items::create` for the full sequence, and `infra::idempotency`
//! for storage.

use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use infra::idempotency::{Claim, StoredResponse};
use infra::sqlx::postgres::PgConnection;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::etag::etag;
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

/// How an idempotent creation goes on.
#[derive(Debug)]
pub enum Start {
    /// Create, then call [`created`] with this key (if the client sent one).
    Proceed(Option<String>),
    /// Already done: answer this replay.
    Replay(Response),
}

/// Starts the creation `operation` (`POST /path`) of `body`, in the tenant
/// transaction `conn`.
pub async fn start(
    conn: &mut PgConnection,
    headers: &HeaderMap,
    operation: &str,
    body: &impl Serialize,
) -> Result<Start, Problem> {
    let Some(key) = key(headers)? else {
        return Ok(Start::Proceed(None));
    };
    let hash = request_hash(operation, body)?;
    match infra::idempotency::claim(conn, &key, &hash).await? {
        Claim::New => Ok(Start::Proceed(Some(key))),
        Claim::Replay(stored) => Ok(Start::Replay(replay(stored))),
        Claim::Mismatch => Err(Problem::new(ErrorCode::IdempotencyKeyReused)),
        Claim::InFlight => Err(Problem::new(ErrorCode::IdempotencyKeyInFlight)),
    }
}

/// The `201 Created` response of a creation (`Location`, `ETag`), stored
/// for replays under `key` in the same transaction. Commit afterwards.
pub async fn created(
    conn: &mut PgConnection,
    key: Option<&str>,
    body: &impl Serialize,
    location: String,
    version: i64,
) -> Result<Response, Problem> {
    let json = serde_json::to_value(body).map_err(|e| Problem::internal(&e))?;
    if let Some(key) = key {
        let stored = StoredResponse {
            status: StatusCode::CREATED.as_u16(),
            body: json.clone(),
            location: Some(location.clone()),
            etag: Some(format!("\"{version}\"")),
        };
        infra::idempotency::complete(conn, key, &stored).await?;
    }
    let mut response = (StatusCode::CREATED, axum::Json(json)).into_response();
    let headers = response.headers_mut();
    headers.insert(header::ETAG, etag(version));
    if let Ok(value) = HeaderValue::from_str(&location) {
        headers.insert(header::LOCATION, value);
    }
    Ok(response)
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
