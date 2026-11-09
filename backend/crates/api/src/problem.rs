//! Errors as RFC 9457 Problem Details (`application/problem+json`).
//!
//! Every error response of the API has this shape:
//!
//! ```json
//! {
//!   "type": "urn:mlbpp:problem:validation_failed",
//!   "title": "The request contains invalid fields",
//!   "status": 400,
//!   "code": "validation_failed",
//!   "detail": "optional, human-readable, English",
//!   "errors": [{ "field": "code", "code": "format" }]
//! }
//! ```
//!
//! `code` is stable: clients translate it, and it never changes once
//! published. All codes live in [`ErrorCode`]; add new ones there. Generic
//! codes are plain (`not_found`); codes specific to a resource are prefixed
//! with it (`tenant.not_found`).

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use domain::validation::FieldError;
use serde::Serialize;

/// Catalog of every error code of the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    // Generic request errors.
    BadRequest,
    ValidationFailed,
    InvalidJson,
    InvalidQuery,
    NotFound,
    MethodNotAllowed,
    RequestTimeout,
    Conflict,
    PreconditionFailed,
    PayloadTooLarge,
    UnsupportedMediaType,
    PreconditionRequired,
    Internal,
    // Idempotency-Key header.
    IdempotencyKeyInvalid,
    IdempotencyKeyReused,
    IdempotencyKeyInFlight,
    // Resources.
    TenantNotFound,
    ReferenceItemCodeTaken,
}

impl ErrorCode {
    /// Every code, for documentation and the OpenAPI contract.
    pub const ALL: [Self; 18] = [
        Self::BadRequest,
        Self::ValidationFailed,
        Self::InvalidJson,
        Self::InvalidQuery,
        Self::NotFound,
        Self::MethodNotAllowed,
        Self::RequestTimeout,
        Self::Conflict,
        Self::PreconditionFailed,
        Self::PayloadTooLarge,
        Self::UnsupportedMediaType,
        Self::PreconditionRequired,
        Self::Internal,
        Self::IdempotencyKeyInvalid,
        Self::IdempotencyKeyReused,
        Self::IdempotencyKeyInFlight,
        Self::TenantNotFound,
        Self::ReferenceItemCodeTaken,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BadRequest => "bad_request",
            Self::ValidationFailed => "validation_failed",
            Self::InvalidJson => "invalid_json",
            Self::InvalidQuery => "invalid_query",
            Self::NotFound => "not_found",
            Self::MethodNotAllowed => "method_not_allowed",
            Self::RequestTimeout => "request_timeout",
            Self::Conflict => "conflict",
            Self::PreconditionFailed => "precondition_failed",
            Self::PayloadTooLarge => "payload_too_large",
            Self::UnsupportedMediaType => "unsupported_media_type",
            Self::PreconditionRequired => "precondition_required",
            Self::Internal => "internal",
            Self::IdempotencyKeyInvalid => "idempotency_key.invalid",
            Self::IdempotencyKeyReused => "idempotency_key.reused",
            Self::IdempotencyKeyInFlight => "idempotency_key.in_flight",
            Self::TenantNotFound => "tenant.not_found",
            Self::ReferenceItemCodeTaken => "reference_item.code_taken",
        }
    }

    pub const fn status(self) -> StatusCode {
        match self {
            Self::BadRequest
            | Self::ValidationFailed
            | Self::InvalidJson
            | Self::InvalidQuery
            | Self::IdempotencyKeyInvalid => StatusCode::BAD_REQUEST,
            Self::NotFound | Self::TenantNotFound => StatusCode::NOT_FOUND,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Self::RequestTimeout => StatusCode::REQUEST_TIMEOUT,
            Self::Conflict | Self::IdempotencyKeyInFlight | Self::ReferenceItemCodeTaken => {
                StatusCode::CONFLICT
            }
            Self::PreconditionFailed => StatusCode::PRECONDITION_FAILED,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Self::IdempotencyKeyReused => StatusCode::UNPROCESSABLE_ENTITY,
            Self::PreconditionRequired => StatusCode::PRECONDITION_REQUIRED,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// English fallback, for logs and API users; the interface translates `code`.
    pub const fn title(self) -> &'static str {
        match self {
            Self::BadRequest => "The request is malformed",
            Self::ValidationFailed => "The request contains invalid fields",
            Self::InvalidJson => "The request body is not valid JSON for this operation",
            Self::InvalidQuery => "The query string is invalid",
            Self::NotFound => "Resource not found",
            Self::MethodNotAllowed => "Method not allowed on this resource",
            Self::RequestTimeout => "The request took too long",
            Self::Conflict => "The request conflicts with the current state",
            Self::PreconditionFailed => "The resource was modified in the meantime",
            Self::PayloadTooLarge => "The request body is too large",
            Self::UnsupportedMediaType => "Unsupported content type; send application/json",
            Self::PreconditionRequired => "This request needs an If-Match header",
            Self::Internal => "Internal server error",
            Self::IdempotencyKeyInvalid => "The Idempotency-Key header is invalid",
            Self::IdempotencyKeyReused => "This Idempotency-Key was used for a different request",
            Self::IdempotencyKeyInFlight => "A request with this Idempotency-Key is still in progress",
            Self::TenantNotFound => "Tenant not found",
            Self::ReferenceItemCodeTaken => "Another reference item already has this code",
        }
    }

    /// Code of a framework-generated error response with this status, if any.
    pub fn for_status(status: StatusCode) -> Option<Self> {
        match status {
            StatusCode::BAD_REQUEST => Some(Self::BadRequest),
            StatusCode::NOT_FOUND => Some(Self::NotFound),
            StatusCode::METHOD_NOT_ALLOWED => Some(Self::MethodNotAllowed),
            StatusCode::REQUEST_TIMEOUT => Some(Self::RequestTimeout),
            StatusCode::PAYLOAD_TOO_LARGE => Some(Self::PayloadTooLarge),
            StatusCode::UNSUPPORTED_MEDIA_TYPE => Some(Self::UnsupportedMediaType),
            s if s.is_server_error() => Some(Self::Internal),
            _ => None,
        }
    }
}

/// An error returned by a handler or an extractor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub code: ErrorCode,
    /// Explanation specific to this occurrence. Never put internal details here.
    pub detail: Option<String>,
    /// Field-level errors, for `validation_failed` and similar.
    pub errors: Vec<FieldError>,
}

impl Problem {
    pub fn new(code: ErrorCode) -> Self {
        Self {
            code,
            detail: None,
            errors: Vec::new(),
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_errors(mut self, errors: Vec<FieldError>) -> Self {
        self.errors = errors;
        self
    }

    /// `validation_failed` with the given field errors.
    pub fn validation(errors: Vec<FieldError>) -> Self {
        Self::new(ErrorCode::ValidationFailed).with_errors(errors)
    }

    /// Logs `err` with its details and returns a generic 500: the client
    /// never sees internal messages.
    pub fn internal(err: &dyn std::error::Error) -> Self {
        tracing::error!(error = %err, "internal error");
        Self::new(ErrorCode::Internal)
    }
}

impl From<ErrorCode> for Problem {
    fn from(code: ErrorCode) -> Self {
        Self::new(code)
    }
}

impl From<infra::sqlx::Error> for Problem {
    fn from(err: infra::sqlx::Error) -> Self {
        Self::internal(&err)
    }
}

#[derive(Serialize)]
struct Body<'a> {
    #[serde(rename = "type")]
    kind: String,
    title: &'a str,
    status: u16,
    code: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<FieldErrorBody<'a>>,
}

#[derive(Serialize)]
struct FieldErrorBody<'a> {
    field: &'a str,
    code: &'a str,
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let code = self.code;
        let body = Body {
            kind: format!("urn:mlbpp:problem:{}", code.as_str()),
            title: code.title(),
            status: code.status().as_u16(),
            code: code.as_str(),
            detail: self.detail.as_deref(),
            errors: self
                .errors
                .iter()
                .map(|e| FieldErrorBody {
                    field: &e.field,
                    code: e.code,
                })
                .collect(),
        };
        let mut response = (code.status(), axum::Json(body)).into_response();
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(PROBLEM_JSON));
        response
    }
}

pub const PROBLEM_JSON: &str = "application/problem+json";

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use std::collections::HashSet;

    async fn json(problem: Problem) -> (StatusCode, String, serde_json::Value) {
        let res = problem.into_response();
        let status = res.status();
        let content_type = res.headers()[header::CONTENT_TYPE].to_str().unwrap().to_owned();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (status, content_type, serde_json::from_slice(&bytes).unwrap())
    }

    #[test]
    fn every_code_is_unique_and_has_a_status_and_title() {
        let mut seen = HashSet::new();
        for code in ErrorCode::ALL {
            assert!(seen.insert(code.as_str()), "duplicate code {}", code.as_str());
            assert!(code.status().is_client_error() || code.status().is_server_error());
            assert!(!code.title().is_empty());
            assert!(
                code.as_str()
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_' || c == '.'),
                "{}",
                code.as_str()
            );
        }
    }

    #[test]
    fn every_code_maps_to_the_expected_status() {
        let expected: [(ErrorCode, u16); 18] = [
            (ErrorCode::BadRequest, 400),
            (ErrorCode::ValidationFailed, 400),
            (ErrorCode::InvalidJson, 400),
            (ErrorCode::InvalidQuery, 400),
            (ErrorCode::NotFound, 404),
            (ErrorCode::MethodNotAllowed, 405),
            (ErrorCode::RequestTimeout, 408),
            (ErrorCode::Conflict, 409),
            (ErrorCode::PreconditionFailed, 412),
            (ErrorCode::PayloadTooLarge, 413),
            (ErrorCode::UnsupportedMediaType, 415),
            (ErrorCode::PreconditionRequired, 428),
            (ErrorCode::Internal, 500),
            (ErrorCode::IdempotencyKeyInvalid, 400),
            (ErrorCode::IdempotencyKeyReused, 422),
            (ErrorCode::IdempotencyKeyInFlight, 409),
            (ErrorCode::TenantNotFound, 404),
            (ErrorCode::ReferenceItemCodeTaken, 409),
        ];
        for (code, status) in expected {
            assert_eq!(code.status().as_u16(), status, "{}", code.as_str());
        }
    }

    #[tokio::test]
    async fn renders_minimal_problem_json() {
        let (status, content_type, body) = json(Problem::new(ErrorCode::TenantNotFound)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(content_type, PROBLEM_JSON);
        assert_eq!(
            body,
            serde_json::json!({
                "type": "urn:mlbpp:problem:tenant.not_found",
                "title": "Tenant not found",
                "status": 404,
                "code": "tenant.not_found"
            })
        );
    }

    #[tokio::test]
    async fn renders_detail_and_field_errors() {
        let problem = Problem::validation(vec![FieldError::new("code", "format")]).with_detail("see errors");
        let (status, _, body) = json(problem).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["detail"], "see errors");
        assert_eq!(
            body["errors"],
            serde_json::json!([{ "field": "code", "code": "format" }])
        );
    }

    #[test]
    fn database_errors_become_generic_internal_problems() {
        let problem = Problem::from(infra::sqlx::Error::PoolTimedOut);
        assert_eq!(problem, Problem::new(ErrorCode::Internal));
    }

    #[test]
    fn framework_statuses_have_codes() {
        assert_eq!(
            ErrorCode::for_status(StatusCode::PAYLOAD_TOO_LARGE),
            Some(ErrorCode::PayloadTooLarge)
        );
        assert_eq!(
            ErrorCode::for_status(StatusCode::BAD_GATEWAY),
            Some(ErrorCode::Internal)
        );
        assert_eq!(ErrorCode::for_status(StatusCode::IM_A_TEAPOT), None);
    }
}
