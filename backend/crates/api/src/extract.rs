//! Request extractors whose errors are Problem Details.
//!
//! Use these instead of axum's `Json`, `Path` and `Query`, which answer with
//! plain text. They behave the same otherwise.

use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts};
use axum::http::HeaderMap;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};

use crate::problem::{ErrorCode, Problem};

/// JSON body; also a JSON response.
#[derive(Debug, FromRequest)]
#[from_request(via(axum::Json), rejection(Problem))]
pub struct ApiJson<T>(pub T);

impl<T: serde::Serialize> IntoResponse for ApiJson<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// Path parameters. A malformed id (not a UUID) is a 404: no such resource.
#[derive(Debug, FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(Problem))]
pub struct ApiPath<T>(pub T);

/// Query string parameters.
#[derive(Debug, FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(Problem))]
pub struct ApiQuery<T>(pub T);

impl From<JsonRejection> for Problem {
    fn from(rejection: JsonRejection) -> Self {
        match rejection {
            JsonRejection::MissingJsonContentType(_) => Problem::new(ErrorCode::UnsupportedMediaType),
            JsonRejection::JsonDataError(err) => {
                // serde's message names the field and the expected type; it
                // holds nothing internal.
                Problem::new(ErrorCode::InvalidJson).with_detail(err.body_text())
            }
            JsonRejection::JsonSyntaxError(_) => {
                Problem::new(ErrorCode::InvalidJson).with_detail("the body is not well-formed JSON")
            }
            JsonRejection::BytesRejection(err) => {
                Problem::new(ErrorCode::for_status(err.status()).unwrap_or(ErrorCode::BadRequest))
            }
            _ => Problem::new(ErrorCode::BadRequest),
        }
    }
}

impl From<PathRejection> for Problem {
    fn from(_: PathRejection) -> Self {
        Problem::new(ErrorCode::NotFound)
    }
}

impl From<QueryRejection> for Problem {
    fn from(rejection: QueryRejection) -> Self {
        Problem::new(ErrorCode::InvalidQuery).with_detail(rejection.body_text())
    }
}

/// Value of the `Accept-Language` header, if any.
#[derive(Debug, Clone, Default)]
pub struct AcceptLanguage(pub Option<String>);

impl<S: Send + Sync> FromRequestParts<S> for AcceptLanguage {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(
            header_str(&parts.headers, "accept-language").map(str::to_owned),
        ))
    }
}

/// A header value as text, if present and visible ASCII.
pub fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}
