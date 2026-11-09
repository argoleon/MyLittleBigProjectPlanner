//! Errors as RFC 9457 Problem Details (`application/problem+json`).
//!
//! Every error carries a stable `code` that clients translate; the `title`
//! is an English fallback for logs and API users. L01-US03 completes the
//! catalog and the mapping of validation and database errors.

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// An error returned by a handler or an extractor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub status: StatusCode,
    /// Stable, dotted identifier, e.g. `tenant.not_found`. Never changes once published.
    pub code: &'static str,
    pub title: &'static str,
}

impl Problem {
    pub const fn new(status: StatusCode, code: &'static str, title: &'static str) -> Self {
        Self { status, code, title }
    }

    /// No tenant matches the request's host name.
    pub const TENANT_NOT_FOUND: Self =
        Self::new(StatusCode::NOT_FOUND, "tenant.not_found", "Tenant not found");

    /// Unexpected failure; details are logged, never returned.
    pub const INTERNAL: Self = Self::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "Internal server error",
    );

    /// Logs `err` and returns [`Problem::INTERNAL`].
    pub fn internal(err: &dyn std::error::Error) -> Self {
        tracing::error!(error = %err, "internal error");
        Self::INTERNAL
    }
}

#[derive(Serialize)]
struct Body<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    title: &'a str,
    status: u16,
    code: &'a str,
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let body = Body {
            kind: "about:blank",
            title: self.title,
            status: self.status.as_u16(),
            code: self.code,
        };
        let mut response = (self.status, axum::Json(body)).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        response
    }
}

impl From<infra::sqlx::Error> for Problem {
    fn from(err: infra::sqlx::Error) -> Self {
        Self::internal(&err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    #[tokio::test]
    async fn renders_problem_json_with_stable_code() {
        let res = Problem::TENANT_NOT_FOUND.into_response();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        assert_eq!(res.headers()[header::CONTENT_TYPE], "application/problem+json");
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "type": "about:blank",
                "title": "Tenant not found",
                "status": 404,
                "code": "tenant.not_found"
            })
        );
    }

    #[test]
    fn database_errors_become_internal_problems() {
        let problem = Problem::from(infra::sqlx::Error::PoolTimedOut);
        assert_eq!(problem, Problem::INTERNAL);
    }
}
