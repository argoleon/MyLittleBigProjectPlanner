//! GDPR rights (specification §14, L02-US09): export of the signed-in
//! account's personal data, and erasure by anonymization.
//!
//! - The member exports their data: the worker collects it, and it can be
//!   downloaded for 15 minutes (`infra::exports`).
//! - The member erases their global account, after proving it is them again
//!   (password, or a sign-in through the organization's identity provider
//!   less than 10 minutes ago) and typing their email.
//! - A tenant admin erases a member of their tenant: the global account and
//!   the other tenants are untouched.
//! - A platform super admin erases a global account.
//!
//! Erased members appear as "Deleted user #N"; every erasure is logged with
//! the `audit` target.

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{AppendHeaders, IntoResponse, Response};
use chrono::{DateTime, Utc};
use domain::account::{MAX_FAILED_SIGN_INS_PER_EMAIL, SIGN_IN_WINDOW};
use domain::erasure::{Reauthentication, confirms, reauthentication};
use domain::permissions::PermissionChange;
use domain::validation::FieldError;
use infra::accounts;
use infra::credentials;
use infra::erasure::AccountErasure;
use infra::exports::{EXPORT_JOB, ExportJob, ExportStore};
use infra::members as members_repo;
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::etag::require_if_match;
use crate::extract::{ApiJson, ApiPath};
use crate::members::{check_an_admin_stays, conflict_problem, require_member_admin};
use crate::platform::PlatformAdmin;
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::tenant::{AppState, TenantTx};

/// Exports a member may open in [`SIGN_IN_WINDOW`].
const MAX_EXPORTS: u32 = 5;

/// Where an export stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportStatus {
    /// The worker is collecting the data.
    Pending,
    /// Ready to download until `expiresAt`.
    Ready,
}

/// An export of the signed-in account's personal data.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = PersonalDataExport)]
pub struct ExportView {
    /// To follow it and download it; works for this account only.
    id: String,
    status: ExportStatus,
    /// The export is gone after this time, downloaded or not.
    expires_at: DateTime<Utc>,
}

/// `POST /me/erasure` body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelfErasure {
    /// The account's password; not needed by an account without password
    /// that signed in through its organization less than 10 minutes ago.
    #[serde(default)]
    password: Option<String>,
    /// The account's email, typed to confirm.
    confirmation: String,
    /// Delete the account's comments too, rather than keep them under the
    /// pseudonym.
    #[serde(default)]
    purge_comments: bool,
}

/// Erasure of another account, by an administrator.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = Erasure)]
pub struct ErasureBody {
    /// Delete the member's comments too, rather than keep them under the
    /// pseudonym.
    #[serde(default)]
    purge_comments: bool,
}

fn export_view(store_id: String, export: &infra::exports::Export) -> ExportView {
    ExportView {
        id: store_id,
        status: if export.content.is_some() {
            ExportStatus::Ready
        } else {
            ExportStatus::Pending
        },
        expires_at: export.expires_at,
    }
}

fn exports(state: &AppState) -> &ExportStore {
    &state.auth.exports
}

/// The export `id` of the signed-in account; 404 otherwise.
async fn my_export(state: &AppState, tx: &TenantTx, id: &str) -> Result<infra::exports::Export, Problem> {
    exports(state)
        .get(id)
        .await?
        .filter(|export| export.account_id == tx.account_id())
        .ok_or(Problem::new(ErrorCode::NotFound))
}

/// `POST /api/v1/me/exports`.
#[utoipa::path(
    post,
    path = "/me/exports",
    tag = crate::me::TAG,
    operation_id = "exportMyData",
    summary = "Export the personal data of the signed-in account",
    description = "Right of access: the account, its preferences and memberships in every tenant, \
                   and the content it authored, as JSON. Collected in the background: follow the \
                   export until it is `ready`, then download it, within 15 minutes.",
    responses(
        (status = 202, description = "Being collected", body = ExportView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 429, description = "`too_many_requests`, with `Retry-After`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create_export(State(state): State<AppState>, mut tx: TenantTx) -> Result<Response, Problem> {
    let account = tx.account_id();
    let limit_key = format!("export:{account}");
    if state.auth.limits.hit(&limit_key, SIGN_IN_WINDOW).await? > MAX_EXPORTS {
        return Err(state.auth.too_many(&limit_key).await);
    }
    let (id, export) = exports(&state).open(account).await?;
    let job = ExportJob {
        export_id: id.clone(),
        account_id: account,
    };
    let payload = serde_json::to_value(&job).map_err(|e| Problem::internal(&e))?;
    infra::jobs::enqueue(&mut tx, EXPORT_JOB, &payload).await?;
    tx.commit().await?;
    tracing::info!(target: "audit", account = %account, "personal data export requested");
    Ok((StatusCode::ACCEPTED, ApiJson(export_view(id, &export))).into_response())
}

/// `GET /api/v1/me/exports/{id}`.
#[utoipa::path(
    get,
    path = "/me/exports/{id}",
    tag = crate::me::TAG,
    operation_id = "getMyDataExport",
    summary = "Follow an export of the signed-in account's personal data",
    params(("id" = String, Path, description = "`id` of the export")),
    responses(
        (status = 200, description = "The export", body = ExportView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such export of this account, or expired", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn get_export(
    State(state): State<AppState>,
    tx: TenantTx,
    ApiPath(id): ApiPath<String>,
) -> Result<ApiJson<ExportView>, Problem> {
    let export = my_export(&state, &tx, &id).await?;
    tx.commit().await?;
    Ok(ApiJson(export_view(id, &export)))
}

/// `GET /api/v1/me/exports/{id}/file`.
#[utoipa::path(
    get,
    path = "/me/exports/{id}/file",
    tag = crate::me::TAG,
    operation_id = "downloadMyDataExport",
    summary = "Download an export of the signed-in account's personal data",
    description = "A JSON file (`format`: `mlbpp.personal-data.v1`).",
    params(("id" = String, Path, description = "`id` of the export")),
    responses(
        (status = 200, description = "The file", content_type = "application/json", headers(
            ("Content-Disposition" = String, description = "`attachment` with a file name"),
        )),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such export of this account, or expired", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`export.pending`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn download_export(
    State(state): State<AppState>,
    tx: TenantTx,
    ApiPath(id): ApiPath<String>,
) -> Result<Response, Problem> {
    let export = my_export(&state, &tx, &id).await?;
    tx.commit().await?;
    let content = export.content.ok_or(Problem::new(ErrorCode::ExportPending))?;
    let body = serde_json::to_vec_pretty(&content).map_err(|e| Problem::internal(&e))?;
    let name = format!(
        "attachment; filename=\"personal-data-{}.json\"",
        Utc::now().format("%Y-%m-%d")
    );
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("application/json")),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&name).map_err(|e| Problem::internal(&e))?,
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        body,
    )
        .into_response())
}

/// `POST /api/v1/me/erasure`.
#[utoipa::path(
    post,
    path = "/me/erasure",
    tag = crate::me::TAG,
    operation_id = "eraseMyAccount",
    summary = "Erase the signed-in account",
    description = "Right to erasure: the account's name, email, password and preferences are \
                   removed, it leaves every tenant, and it appears as \"Deleted user #N\" \
                   everywhere, history included. Its content stays, unless `purgeComments`. \
                   Needs the password, or for an account without one, a sign-in through the \
                   organization's identity provider less than 10 minutes ago; and the email, \
                   typed. Ends every session.",
    request_body = SelfErasure,
    responses(
        (status = 204, description = "Erased; the session's cookies are cleared"),
        (status = 400, description = "`validation_failed`: `confirmation` `mismatch`, `password` \
                                      `required` or `incorrect`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`account.reauthentication_required`, `forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`member.last_admin`, or `account.platform_admin`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 429, description = "`too_many_requests`: too many incorrect passwords", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn erase_me(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiJson(body): ApiJson<SelfErasure>,
) -> Result<Response, Problem> {
    let auth = &state.auth;
    let account = tx.account_id();
    let mut errors = Vec::new();
    if !confirms(&body.confirmation, &tx.member().email) {
        errors.push(FieldError::new("confirmation", "mismatch"));
    }
    let hash = accounts::password_hash(&mut tx, account).await?;
    let session_age = (Utc::now() - tx.session().created_at)
        .to_std()
        .unwrap_or_default();
    // Same counter as the password change: both guess the password.
    let failures_key = format!("password-change:{account}");
    match (
        reauthentication(hash.is_some(), tx.session().sso, session_age),
        hash,
    ) {
        (Reauthentication::Password, Some(hash)) => {
            if auth.limits.count(&failures_key).await? >= MAX_FAILED_SIGN_INS_PER_EMAIL {
                return Err(auth.too_many(&failures_key).await);
            }
            let password = body.password.unwrap_or_default();
            if password.is_empty() {
                errors.push(FieldError::new("password", "required"));
            } else if !tokio::task::spawn_blocking(move || credentials::verify_password(&password, &hash))
                .await?
            {
                auth.limits.hit(&failures_key, SIGN_IN_WINDOW).await?;
                errors.push(FieldError::new("password", "incorrect"));
            }
        }
        (Reauthentication::RecentSso, _) => {}
        _ => return Err(Problem::new(ErrorCode::AccountReauthenticationRequired)),
    }
    if !errors.is_empty() {
        return Err(Problem::validation(errors));
    }
    let number =
        erased(infra::erasure::erase_account(&mut tx, account, account, body.purge_comments).await?)?;
    // Before the commit: if it fails, the account is merely signed out.
    let ended = auth.sessions.delete_all(account).await?;
    tx.commit().await?;
    auth.limits.clear(&failures_key).await?;
    tracing::info!(
        target: "audit",
        actor = %account,
        account = %account,
        pseudonym = number,
        scope = "account",
        purge_comments = body.purge_comments,
        sessions_ended = ended,
        "account erased by itself"
    );
    Ok((StatusCode::NO_CONTENT, AppendHeaders(auth.cleared_cookies()?)).into_response())
}

/// The pseudonym number of an account erasure, or its refusal.
fn erased(erasure: AccountErasure) -> Result<i64, Problem> {
    match erasure {
        AccountErasure::Erased(number) => Ok(number),
        AccountErasure::NotFound => Err(Problem::new(ErrorCode::NotFound)),
        AccountErasure::LastAdmin => Err(Problem::new(ErrorCode::MemberLastAdmin)),
        AccountErasure::PlatformAdmin => Err(Problem::new(ErrorCode::AccountPlatformAdmin)),
    }
}

/// `POST /api/v1/tenant/members/{accountId}/erasure`.
#[utoipa::path(
    post,
    path = "/tenant/members/{accountId}/erasure",
    tag = crate::members::TAG,
    operation_id = "eraseMember",
    summary = "Remove a member and erase them from the tenant",
    description = "Tenant admins only. The member leaves the tenant like a removal, and the \
                   tenant shows them as \"Deleted user #N\" from then on, their content \
                   anonymized (or their comments deleted with `purgeComments`). Their global \
                   account and their other tenants are untouched. The last active tenant admin \
                   cannot be erased.",
    params(
        ("accountId" = Uuid, Path, description = "Account of the member"),
        ("If-Match" = String, Header, description = "ETag of the membership, or `*`"),
    ),
    request_body = ErasureBody,
    responses(
        (status = 204, description = "Erased from the tenant"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such member in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`member.last_admin`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The membership changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn erase_member(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(account_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ErasureBody>,
) -> Result<StatusCode, Problem> {
    require_member_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let current = members_repo::find(&mut tx, account_id)
        .await?
        .ok_or(Problem::new(ErrorCode::NotFound))?;
    check_an_admin_stays(&mut tx, &current, false).await?;
    let actor = tx.account_id();
    let number =
        infra::erasure::erase_in_tenant(&mut tx, actor, account_id, &current.email, body.purge_comments)
            .await?;
    members_repo::remove(&mut tx, account_id, if_match.versions())
        .await?
        .map_err(conflict_problem)?;
    let tenant = tx.tenant_id();
    // Before the commit: if it fails, the member is merely signed out.
    let ended = state.auth.sessions.delete_in_tenant(account_id, tenant).await?;
    tx.commit().await?;
    crate::permissions::changed(&state, tenant, PermissionChange::MemberRemoved).await;
    tracing::info!(
        target: "audit",
        actor = %actor,
        account = %account_id,
        tenant = %tenant,
        pseudonym = number,
        scope = "tenant",
        purge_comments = body.purge_comments,
        sessions_ended = ended,
        "member erased from the tenant"
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/platform/accounts/{accountId}/erasure`.
#[utoipa::path(
    post,
    path = "/platform/accounts/{accountId}/erasure",
    tag = crate::platform::TAG,
    operation_id = "eraseAccount",
    summary = "Erase a global account",
    description = "Platform super admins only. The account leaves every tenant and appears as \
                   \"Deleted user #N\" everywhere, its personal data removed; its content stays, \
                   unless `purgeComments`. Every session of the account ends.",
    params(("accountId" = Uuid, Path, description = "The account")),
    request_body = ErasureBody,
    responses(
        (status = 204, description = "Erased"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a platform super admin, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such account, or erased already", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`member.last_admin`, or `account.platform_admin`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn erase_account(
    State(state): State<AppState>,
    mut tx: PlatformAdmin,
    ApiPath(account_id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ErasureBody>,
) -> Result<StatusCode, Problem> {
    let actor = tx.actor();
    let number =
        erased(infra::erasure::erase_account(&mut tx, actor, account_id, body.purge_comments).await?)?;
    let ended = state.auth.sessions.delete_all(account_id).await?;
    tx.commit().await?;
    tracing::info!(
        target: "audit",
        actor = %actor,
        account = %account_id,
        pseudonym = number,
        scope = "account",
        purge_comments = body.purge_comments,
        sessions_ended = ended,
        "account erased by a platform super admin"
    );
    Ok(StatusCode::NO_CONTENT)
}
