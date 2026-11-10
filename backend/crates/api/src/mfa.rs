//! Two-step verification of the signed-in member (`/api/v1/account/mfa`)
//! and the tenant's authentication policy (`/api/v1/tenant/auth-policy`),
//! specification §3 and §14.
//!
//! Enrollment works from a complete session, and from a session at the
//! `mfa_enrollment` stage (the tenant requires two-step verification and
//! the member has none): confirming it then completes the session.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{AppendHeaders, IntoResponse, Response};
use domain::mfa::{SessionStage, display_backup_code};
use infra::accounts::TenantRole;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::{Accept, CodeBody, SessionView, check_code, complete_session};
use crate::etag::{etag, require_if_match};
use crate::extract::ApiJson;
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::tenant::{AppState, StagedTenantTx, TenantTx};

/// OpenAPI tag of these endpoints.
pub const TAG: &str = "account";

/// A session allowed to manage its member's two-step verification: complete,
/// or required to enroll.
fn enrolling_or_complete(tx: &StagedTenantTx) -> Result<(), Problem> {
    match tx.stage() {
        SessionStage::Complete | SessionStage::MfaEnrollment => Ok(()),
        SessionStage::MfaChallenge => Err(Problem::new(ErrorCode::Unauthenticated)),
    }
}

/// Two-step verification of the signed-in member.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = MfaStatus)]
pub struct StatusView {
    enabled: bool,
    /// Unused backup codes.
    backup_codes_left: i64,
    /// The tenant requires it: it cannot be disabled.
    required_by_policy: bool,
}

/// What an authenticator app needs.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = MfaSetup)]
pub struct SetupView {
    /// `otpauth://totp/…`, as encoded in the QR code.
    otpauth_uri: String,
    /// The secret in base32, to type when the QR code cannot be scanned (its
    /// text alternative).
    #[schema(example = "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP")]
    secret: String,
    /// The QR code, an SVG image drawn by the server.
    qr_code_svg: String,
}

/// Two-step verification just enabled.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = MfaEnabled)]
pub struct EnabledView {
    /// Shown once: each one replaces a code from the app once.
    #[schema(example = json!(["abcde-fghjk", "mnpqr-stuvw"]))]
    backup_codes: Vec<String>,
    /// The session, complete now if it was waiting for this enrollment.
    session: SessionView,
}

/// `GET /api/v1/account/mfa`.
#[utoipa::path(
    get,
    path = "/account/mfa",
    tag = TAG,
    operation_id = "getMfa",
    summary = "My two-step verification",
    responses(
        (status = 200, description = "Its state", body = StatusView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn status(mut tx: StagedTenantTx) -> Result<ApiJson<StatusView>, Problem> {
    enrolling_or_complete(&tx)?;
    let account = tx.account_id();
    let view = StatusView {
        enabled: infra::mfa::is_enabled(&mut tx, account).await?,
        backup_codes_left: infra::mfa::backup_codes_left(&mut tx, account).await?,
        required_by_policy: infra::mfa::auth_policy(&mut tx).await?.local_accounts_require_mfa,
    };
    tx.0.commit().await?;
    Ok(ApiJson(view))
}

/// `POST /api/v1/account/mfa/setup`: a new secret to scan.
#[utoipa::path(
    post,
    path = "/account/mfa/setup",
    tag = TAG,
    operation_id = "setUpMfa",
    summary = "Start enabling two-step verification",
    description = "Draws a new secret, shown as a QR code and as text. It is only enabled once \
                   `POST /account/mfa/confirm` receives a code from the app. Starting again \
                   replaces the secret.",
    responses(
        (status = 200, description = "The secret to add to the authenticator app", body = SetupView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`mfa.already_enabled`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn setup(
    State(state): State<AppState>,
    mut tx: StagedTenantTx,
) -> Result<ApiJson<SetupView>, Problem> {
    enrolling_or_complete(&tx)?;
    let account = tx.account_id();
    let email = tx.member().email.clone();
    let secret = infra::mfa::generate_secret();
    let (ciphertext, nonce) = state
        .auth
        .mfa
        .encrypt(account, &secret)
        .map_err(|e| Problem::internal(&e))?;
    if !infra::mfa::start_enrollment(&mut tx, account, &ciphertext, &nonce).await? {
        return Err(Problem::new(ErrorCode::MfaAlreadyEnabled));
    }
    let enrollment = infra::mfa::enrollment(&secret, &email).map_err(|e| Problem::internal(&e))?;
    tx.0.commit().await?;
    Ok(ApiJson(SetupView {
        otpauth_uri: enrollment.otpauth_uri,
        secret: enrollment.secret_base32,
        qr_code_svg: enrollment.qr_code_svg,
    }))
}

/// `POST /api/v1/account/mfa/confirm`: enable with a first code.
#[utoipa::path(
    post,
    path = "/account/mfa/confirm",
    tag = TAG,
    operation_id = "confirmMfa",
    summary = "Enable two-step verification with a first code",
    description = "A code from the app proves it holds the secret. Answers 8 backup codes, shown \
                   this once. A session waiting for this enrollment becomes complete (new cookies).",
    request_body = CodeBody,
    responses(
        (status = 200, description = "Enabled", body = EnabledView),
        (status = 401, description = "`auth.invalid_code`, or `unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`mfa.not_started`, or `mfa.already_enabled`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 429, description = "`too_many_requests`, with `Retry-After`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn confirm(
    State(state): State<AppState>,
    mut tx: StagedTenantTx,
    ApiJson(body): ApiJson<CodeBody>,
) -> Result<Response, Problem> {
    enrolling_or_complete(&tx)?;
    let account = tx.account_id();
    match infra::mfa::find(&mut tx, account).await? {
        None => return Err(Problem::new(ErrorCode::MfaNotStarted)),
        Some(stored) if stored.enabled => return Err(Problem::new(ErrorCode::MfaAlreadyEnabled)),
        Some(_) => {}
    }
    let auth = &state.auth;
    let step = check_code(auth, &mut tx, account, &body.code, Accept::Enrolling)
        .await?
        .unwrap_or_default();
    let codes = infra::mfa::generate_backup_codes();
    if !infra::mfa::enable(&mut tx, account, step, &codes).await? {
        return Err(Problem::new(ErrorCode::MfaAlreadyEnabled));
    }
    let member = tx.member().clone();
    let backup_codes = codes.iter().map(|c| display_backup_code(c)).collect();
    tracing::info!(account = %account, "two-step verification enabled");
    if tx.stage() == SessionStage::MfaEnrollment {
        let (token, session) = complete_session(auth, &tx).await?;
        tx.0.commit().await?;
        let body = EnabledView {
            backup_codes,
            session: SessionView::new(&member, &session),
        };
        return Ok((
            AppendHeaders(auth.session_cookies(&token, &session)?),
            ApiJson(body),
        )
            .into_response());
    }
    let session = SessionView::new(&member, tx.session());
    tx.0.commit().await?;
    Ok(ApiJson(EnabledView {
        backup_codes,
        session,
    })
    .into_response())
}

/// `POST /api/v1/account/mfa/disable`.
#[utoipa::path(
    post,
    path = "/account/mfa/disable",
    tag = TAG,
    operation_id = "disableMfa",
    summary = "Disable two-step verification",
    description = "Confirmed by a code from the app or a backup code. Refused when the \
                   organization requires two-step verification.",
    request_body = CodeBody,
    responses(
        (status = 204, description = "Disabled: secret and backup codes deleted"),
        (status = 401, description = "`auth.invalid_code`, or `unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`mfa.required_by_policy`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 429, description = "`too_many_requests`, with `Retry-After`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn disable(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiJson(body): ApiJson<CodeBody>,
) -> Result<StatusCode, Problem> {
    if infra::mfa::auth_policy(&mut tx).await?.local_accounts_require_mfa {
        return Err(Problem::new(ErrorCode::MfaRequiredByPolicy));
    }
    let account = tx.account_id();
    check_code(&state.auth, &mut tx, account, &body.code, Accept::Enabled).await?;
    infra::mfa::disable(&mut tx, account).await?;
    tx.commit().await?;
    tracing::info!(account = %account, "two-step verification disabled");
    Ok(StatusCode::NO_CONTENT)
}

// --- Authentication policy ------------------------------------------------------

/// Authentication policy of the tenant.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = AuthPolicy, example = json!({"localAccountsRequireMfa": true}))]
pub struct PolicyBody {
    /// Members signing in with a password must use two-step verification;
    /// those without it enroll at their next sign-in.
    local_accounts_require_mfa: bool,
}

fn policy_response(policy: infra::mfa::AuthPolicy) -> Response {
    let mut response = ApiJson(PolicyBody {
        local_accounts_require_mfa: policy.local_accounts_require_mfa,
    })
    .into_response();
    response.headers_mut().insert(header::ETAG, etag(policy.version));
    response
}

fn require_tenant_admin(tx: &TenantTx) -> Result<(), Problem> {
    if tx.member().tenant_role == TenantRole::TenantAdmin {
        Ok(())
    } else {
        Err(Problem::new(ErrorCode::Forbidden))
    }
}

/// `GET /api/v1/tenant/auth-policy`.
#[utoipa::path(
    get,
    path = "/tenant/auth-policy",
    tag = TAG,
    operation_id = "getAuthPolicy",
    summary = "Authentication policy of the tenant",
    description = "Tenant admins only.",
    responses(
        (status = 200, description = "The policy", body = PolicyBody, headers(
            ("ETag" = String, description = "Version of the policy"),
        )),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn get_policy(mut tx: TenantTx) -> Result<Response, Problem> {
    require_tenant_admin(&tx)?;
    let policy = infra::mfa::auth_policy(&mut tx).await?;
    tx.commit().await?;
    Ok(policy_response(policy))
}

/// `PUT /api/v1/tenant/auth-policy`: requires `If-Match`.
#[utoipa::path(
    put,
    path = "/tenant/auth-policy",
    tag = TAG,
    operation_id = "updateAuthPolicy",
    summary = "Change the authentication policy of the tenant",
    description = "Tenant admins only.",
    params(("If-Match" = String, Header, description = "ETag of the version being replaced, or `*`")),
    request_body = PolicyBody,
    responses(
        (status = 200, description = "The new policy", body = PolicyBody, headers(
            ("ETag" = String, description = "New version of the policy"),
        )),
        (status = 400, description = "Invalid JSON", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The policy changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update_policy(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiJson(body): ApiJson<PolicyBody>,
) -> Result<Response, Problem> {
    require_tenant_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let policy = infra::mfa::set_auth_policy(&mut tx, body.local_accounts_require_mfa, if_match.versions())
        .await?
        .ok_or(Problem::new(ErrorCode::PreconditionFailed))?;
    tx.commit().await?;
    tracing::info!(
        require_mfa = policy.local_accounts_require_mfa,
        "authentication policy changed"
    );
    Ok(policy_response(policy))
}
