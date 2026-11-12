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
//!
//! The OpenAPI contract exports the catalog as the `ErrorCode` schema, and the
//! body as the `Problem` schema ([`ProblemBody`]), so clients can rely on both.

use std::borrow::Cow;
use std::fmt::Write as _;

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use domain::validation::FieldError;
use serde::{Serialize, Serializer};
use utoipa::ToSchema;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{ObjectBuilder, Schema, Type};

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
    TooManyRequests,
    Internal,
    // Authentication and access.
    Unauthenticated,
    Forbidden,
    CsrfInvalid,
    AuthInvalidCredentials,
    AuthResetTokenInvalid,
    AuthInvalidCode,
    MfaAlreadyEnabled,
    MfaNotStarted,
    MfaRequiredByPolicy,
    AuthSsoRequired,
    AuthOidcFailed,
    AuthOidcNoMember,
    AuthOidcEmailUnverified,
    AuthSwitchFailed,
    OidcProviderDuplicate,
    OidcProviderUnreachable,
    // Idempotency-Key header.
    IdempotencyKeyInvalid,
    IdempotencyKeyReused,
    IdempotencyKeyInFlight,
    // Resources.
    TenantNotFound,
    TenantSlugTaken,
    TenantConfirmationMismatch,
    TenantDeleteCurrent,
    ReferenceItemCodeTaken,
    InvitationInvalid,
    InvitationPending,
    InvitationAlreadyMember,
    InvitationClosed,
    MemberLastAdmin,
    GroupNameTaken,
    PermissionDenied,
    RoleLabelTaken,
    RoleInUse,
    AccountNoPassword,
    SessionCurrent,
    AccountReauthenticationRequired,
    AccountPlatformAdmin,
    ExportPending,
    NodeKeyTaken,
    NodeCycle,
}

impl ErrorCode {
    /// Every code, for documentation and the OpenAPI contract.
    pub const ALL: [Self; 54] = [
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
        Self::TooManyRequests,
        Self::Internal,
        Self::Unauthenticated,
        Self::Forbidden,
        Self::CsrfInvalid,
        Self::AuthInvalidCredentials,
        Self::AuthResetTokenInvalid,
        Self::AuthInvalidCode,
        Self::MfaAlreadyEnabled,
        Self::MfaNotStarted,
        Self::MfaRequiredByPolicy,
        Self::AuthSsoRequired,
        Self::AuthOidcFailed,
        Self::AuthOidcNoMember,
        Self::AuthOidcEmailUnverified,
        Self::AuthSwitchFailed,
        Self::OidcProviderDuplicate,
        Self::OidcProviderUnreachable,
        Self::IdempotencyKeyInvalid,
        Self::IdempotencyKeyReused,
        Self::IdempotencyKeyInFlight,
        Self::TenantNotFound,
        Self::TenantSlugTaken,
        Self::TenantConfirmationMismatch,
        Self::TenantDeleteCurrent,
        Self::ReferenceItemCodeTaken,
        Self::InvitationInvalid,
        Self::InvitationPending,
        Self::InvitationAlreadyMember,
        Self::InvitationClosed,
        Self::MemberLastAdmin,
        Self::GroupNameTaken,
        Self::PermissionDenied,
        Self::RoleLabelTaken,
        Self::RoleInUse,
        Self::AccountNoPassword,
        Self::SessionCurrent,
        Self::AccountReauthenticationRequired,
        Self::AccountPlatformAdmin,
        Self::ExportPending,
        Self::NodeKeyTaken,
        Self::NodeCycle,
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
            Self::TooManyRequests => "too_many_requests",
            Self::Internal => "internal",
            Self::Unauthenticated => "unauthenticated",
            Self::Forbidden => "forbidden",
            Self::CsrfInvalid => "csrf.invalid",
            Self::AuthInvalidCredentials => "auth.invalid_credentials",
            Self::AuthResetTokenInvalid => "auth.reset_token_invalid",
            Self::AuthInvalidCode => "auth.invalid_code",
            Self::MfaAlreadyEnabled => "mfa.already_enabled",
            Self::MfaNotStarted => "mfa.not_started",
            Self::MfaRequiredByPolicy => "mfa.required_by_policy",
            Self::AuthSsoRequired => "auth.sso_required",
            Self::AuthOidcFailed => "auth.oidc_failed",
            Self::AuthOidcNoMember => "auth.oidc_no_member",
            Self::AuthOidcEmailUnverified => "auth.oidc_email_unverified",
            Self::AuthSwitchFailed => "auth.switch_failed",
            Self::OidcProviderDuplicate => "oidc_provider.duplicate",
            Self::OidcProviderUnreachable => "oidc_provider.unreachable",
            Self::IdempotencyKeyInvalid => "idempotency_key.invalid",
            Self::IdempotencyKeyReused => "idempotency_key.reused",
            Self::IdempotencyKeyInFlight => "idempotency_key.in_flight",
            Self::TenantNotFound => "tenant.not_found",
            Self::TenantSlugTaken => "tenant.slug_taken",
            Self::TenantConfirmationMismatch => "tenant.confirmation_mismatch",
            Self::TenantDeleteCurrent => "tenant.delete_current",
            Self::ReferenceItemCodeTaken => "reference_item.code_taken",
            Self::InvitationInvalid => "invitation.invalid",
            Self::InvitationPending => "invitation.pending",
            Self::InvitationAlreadyMember => "invitation.already_member",
            Self::InvitationClosed => "invitation.closed",
            Self::MemberLastAdmin => "member.last_admin",
            Self::GroupNameTaken => "group.name_taken",
            Self::PermissionDenied => "permission.denied",
            Self::RoleLabelTaken => "role.label_taken",
            Self::RoleInUse => "role.in_use",
            Self::AccountNoPassword => "account.no_password",
            Self::SessionCurrent => "session.current",
            Self::AccountReauthenticationRequired => "account.reauthentication_required",
            Self::AccountPlatformAdmin => "account.platform_admin",
            Self::ExportPending => "export.pending",
            Self::NodeKeyTaken => "node.key_taken",
            Self::NodeCycle => "node.cycle",
        }
    }

    pub const fn status(self) -> StatusCode {
        match self {
            Self::BadRequest
            | Self::ValidationFailed
            | Self::InvalidJson
            | Self::InvalidQuery
            | Self::IdempotencyKeyInvalid
            | Self::AuthResetTokenInvalid
            | Self::TenantConfirmationMismatch
            | Self::AuthOidcFailed
            | Self::AuthSwitchFailed
            | Self::InvitationInvalid => StatusCode::BAD_REQUEST,
            Self::Unauthenticated | Self::AuthInvalidCredentials | Self::AuthInvalidCode => {
                StatusCode::UNAUTHORIZED
            }
            Self::Forbidden
            | Self::PermissionDenied
            | Self::AccountReauthenticationRequired
            | Self::CsrfInvalid
            | Self::AuthSsoRequired
            | Self::AuthOidcNoMember
            | Self::AuthOidcEmailUnverified => StatusCode::FORBIDDEN,
            Self::OidcProviderUnreachable => StatusCode::BAD_GATEWAY,
            Self::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            Self::NotFound | Self::TenantNotFound => StatusCode::NOT_FOUND,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Self::RequestTimeout => StatusCode::REQUEST_TIMEOUT,
            Self::Conflict
            | Self::IdempotencyKeyInFlight
            | Self::ReferenceItemCodeTaken
            | Self::TenantSlugTaken
            | Self::TenantDeleteCurrent
            | Self::MfaAlreadyEnabled
            | Self::MfaNotStarted
            | Self::MfaRequiredByPolicy
            | Self::OidcProviderDuplicate
            | Self::InvitationPending
            | Self::InvitationAlreadyMember
            | Self::InvitationClosed
            | Self::MemberLastAdmin
            | Self::GroupNameTaken
            | Self::RoleLabelTaken
            | Self::RoleInUse
            | Self::AccountNoPassword
            | Self::SessionCurrent
            | Self::AccountPlatformAdmin
            | Self::ExportPending
            | Self::NodeKeyTaken
            | Self::NodeCycle => StatusCode::CONFLICT,
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
            Self::TooManyRequests => "Too many attempts; retry after the delay in Retry-After",
            Self::Internal => "Internal server error",
            Self::Unauthenticated => "Sign-in required, or the session has ended",
            Self::Forbidden => "You are not allowed to do this",
            Self::CsrfInvalid => "Missing or invalid X-CSRF-Token header",
            Self::AuthInvalidCredentials => "Incorrect email or password",
            Self::AuthResetTokenInvalid => "This password reset link is invalid, expired or already used",
            Self::AuthInvalidCode => "Incorrect or already used verification code",
            Self::MfaAlreadyEnabled => "Two-step verification is already enabled",
            Self::MfaNotStarted => "Start two-step verification setup first",
            Self::MfaRequiredByPolicy => "The organization requires two-step verification",
            Self::AuthSsoRequired => "This organization signs its members in through its identity provider",
            Self::AuthOidcFailed => "Sign-in through the identity provider failed; try again",
            Self::AuthOidcNoMember => "No member of this organization has this account's email",
            Self::AuthOidcEmailUnverified => "The identity provider does not vouch for this account's email",
            Self::AuthSwitchFailed => "This switch link is invalid, expired or already used; sign in again",
            Self::OidcProviderDuplicate => "This identity provider is already configured",
            Self::OidcProviderUnreachable => "The identity provider cannot be reached or answered badly",
            Self::IdempotencyKeyInvalid => "The Idempotency-Key header is invalid",
            Self::IdempotencyKeyReused => "This Idempotency-Key was used for a different request",
            Self::IdempotencyKeyInFlight => "A request with this Idempotency-Key is still in progress",
            Self::TenantNotFound => "Tenant not found",
            Self::TenantSlugTaken => "Another tenant already has this slug",
            Self::TenantConfirmationMismatch => "Type the tenant's slug in `confirm` to delete it",
            Self::TenantDeleteCurrent => "The tenant you are signed in to cannot be deleted",
            Self::ReferenceItemCodeTaken => "Another reference item already has this code",
            Self::InvitationInvalid => "This invitation link is invalid, expired or already used",
            Self::InvitationPending => "This email already has an open invitation; send it again instead",
            Self::InvitationAlreadyMember => "This email already belongs to a member of the tenant",
            Self::InvitationClosed => "This invitation was already accepted or revoked",
            Self::MemberLastAdmin => "The tenant needs at least one active tenant admin",
            Self::GroupNameTaken => "Another group already has this name",
            Self::PermissionDenied => "You do not have the permission this needs",
            Self::RoleLabelTaken => "Another role already has this label",
            Self::RoleInUse => "Grants use this role: remove them first",
            Self::AccountNoPassword => "This account signs in through its organization: it has no password",
            Self::SessionCurrent => "This is the session of the request: sign out instead",
            Self::AccountReauthenticationRequired => {
                "Sign in again through your organization, then erase your account within 10 minutes"
            }
            Self::AccountPlatformAdmin => {
                "A platform super admin cannot be erased: another one removes the role first"
            }
            Self::ExportPending => "The export is not ready yet",
            Self::NodeKeyTaken => "Another project of the organization has this key",
            Self::NodeCycle => "This link would make the node its own ancestor",
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
            StatusCode::TOO_MANY_REQUESTS => Some(Self::TooManyRequests),
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
    /// Sent as `Retry-After` (whole seconds), for `too_many_requests`.
    pub retry_after: Option<std::time::Duration>,
}

impl Problem {
    pub fn new(code: ErrorCode) -> Self {
        Self {
            code,
            detail: None,
            errors: Vec::new(),
            retry_after: None,
        }
    }

    /// Adds a `Retry-After` header (at least one second).
    pub fn with_retry_after(mut self, delay: std::time::Duration) -> Self {
        self.retry_after = Some(delay);
        self
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

impl From<infra::valkey::ValkeyError> for Problem {
    fn from(err: infra::valkey::ValkeyError) -> Self {
        Self::internal(&err)
    }
}

impl From<tokio::task::JoinError> for Problem {
    fn from(err: tokio::task::JoinError) -> Self {
        Self::internal(&err)
    }
}

impl Serialize for ErrorCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// The `ErrorCode` schema of the contract: a string enum built from
/// [`ErrorCode::ALL`], with the status and title of each code.
impl utoipa::PartialSchema for ErrorCode {
    fn schema() -> RefOr<Schema> {
        let mut description = String::from(
            "Stable error code of a Problem Details response. Clients translate it; \
             it never changes once published.\n\n| Code | Status | Title |\n| --- | --- | --- |\n",
        );
        for code in Self::ALL {
            let _ = writeln!(
                description,
                "| `{}` | {} | {} |",
                code.as_str(),
                code.status().as_u16(),
                code.title()
            );
        }
        ObjectBuilder::new()
            .schema_type(Type::String)
            .enum_values(Some(Self::ALL.map(Self::as_str)))
            .description(Some(description))
            .into()
    }
}

impl ToSchema for ErrorCode {
    fn name() -> Cow<'static, str> {
        Cow::Borrowed("ErrorCode")
    }
}

/// Body of every error response (`application/problem+json`).
#[derive(Debug, Serialize, ToSchema)]
#[schema(
    as = Problem,
    example = json!({
        "type": "urn:mlbpp:problem:validation_failed",
        "title": "The request contains invalid fields",
        "status": 400,
        "code": "validation_failed",
        "errors": [{ "field": "code", "code": "format" }]
    })
)]
pub struct ProblemBody<'a> {
    /// `urn:mlbpp:problem:<code>`.
    #[serde(rename = "type")]
    #[schema(example = "urn:mlbpp:problem:not_found")]
    pub kind: String,
    /// English summary of the code; the interface translates `code` instead.
    pub title: &'static str,
    /// HTTP status, repeated from the response.
    #[schema(example = 404)]
    pub status: u16,
    pub code: ErrorCode,
    /// Explanation of this occurrence, in English.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<&'a str>,
    /// Invalid fields, for `validation_failed` and similar codes.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    #[schema(required = false)]
    pub errors: Vec<FieldErrorBody<'a>>,
}

/// One invalid field of a request.
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = FieldError)]
pub struct FieldErrorBody<'a> {
    /// Path of the field as sent, e.g. `code` or `labelTranslations.fr`.
    #[schema(example = "code")]
    pub field: &'a str,
    /// Stable code the client translates, e.g. `required`, `format`,
    /// `length`, `too_long`, `out_of_range`, `taken`, `unsupported_locale`,
    /// `unknown_field`.
    #[schema(example = "format")]
    pub code: &'static str,
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let code = self.code;
        let body = ProblemBody {
            kind: format!("urn:mlbpp:problem:{}", code.as_str()),
            title: code.title(),
            status: code.status().as_u16(),
            code,
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
        if let Some(delay) = self.retry_after {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(delay.as_secs().max(1)));
        }
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
        let expected: [(ErrorCode, u16); 54] = [
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
            (ErrorCode::TooManyRequests, 429),
            (ErrorCode::Internal, 500),
            (ErrorCode::Unauthenticated, 401),
            (ErrorCode::Forbidden, 403),
            (ErrorCode::CsrfInvalid, 403),
            (ErrorCode::AuthInvalidCredentials, 401),
            (ErrorCode::AuthResetTokenInvalid, 400),
            (ErrorCode::AuthInvalidCode, 401),
            (ErrorCode::MfaAlreadyEnabled, 409),
            (ErrorCode::MfaNotStarted, 409),
            (ErrorCode::MfaRequiredByPolicy, 409),
            (ErrorCode::AuthSsoRequired, 403),
            (ErrorCode::AuthOidcFailed, 400),
            (ErrorCode::AuthOidcNoMember, 403),
            (ErrorCode::AuthOidcEmailUnverified, 403),
            (ErrorCode::AuthSwitchFailed, 400),
            (ErrorCode::OidcProviderDuplicate, 409),
            (ErrorCode::OidcProviderUnreachable, 502),
            (ErrorCode::IdempotencyKeyInvalid, 400),
            (ErrorCode::IdempotencyKeyReused, 422),
            (ErrorCode::IdempotencyKeyInFlight, 409),
            (ErrorCode::TenantNotFound, 404),
            (ErrorCode::TenantSlugTaken, 409),
            (ErrorCode::TenantConfirmationMismatch, 400),
            (ErrorCode::TenantDeleteCurrent, 409),
            (ErrorCode::ReferenceItemCodeTaken, 409),
            (ErrorCode::InvitationInvalid, 400),
            (ErrorCode::InvitationPending, 409),
            (ErrorCode::InvitationAlreadyMember, 409),
            (ErrorCode::InvitationClosed, 409),
            (ErrorCode::MemberLastAdmin, 409),
            (ErrorCode::GroupNameTaken, 409),
            (ErrorCode::PermissionDenied, 403),
            (ErrorCode::RoleLabelTaken, 409),
            (ErrorCode::RoleInUse, 409),
            (ErrorCode::AccountNoPassword, 409),
            (ErrorCode::SessionCurrent, 409),
            (ErrorCode::AccountReauthenticationRequired, 403),
            (ErrorCode::AccountPlatformAdmin, 409),
            (ErrorCode::ExportPending, 409),
            (ErrorCode::NodeKeyTaken, 409),
            (ErrorCode::NodeCycle, 409),
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

    #[tokio::test]
    async fn too_many_requests_says_when_to_retry() {
        let res = Problem::new(ErrorCode::TooManyRequests)
            .with_retry_after(std::time::Duration::from_millis(90_500))
            .into_response();
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(res.headers()[header::RETRY_AFTER], "90");
        let res = Problem::new(ErrorCode::TooManyRequests)
            .with_retry_after(std::time::Duration::ZERO)
            .into_response();
        assert_eq!(res.headers()[header::RETRY_AFTER], "1", "never 0");
        assert!(
            !Problem::new(ErrorCode::NotFound)
                .into_response()
                .headers()
                .contains_key(header::RETRY_AFTER)
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
