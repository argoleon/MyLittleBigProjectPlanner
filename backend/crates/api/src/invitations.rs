//! Invitations of new members (specification §3, L02-US06).
//!
//! A tenant admin invites an email with an account type and a tenant role
//! (`/tenant/invitations`); the worker emails a link valid for 7 days. Its
//! owner opens the invitation page, which shows the invitation
//! (`POST /auth/invitation`) and accepts it (`POST /auth/invitation/accept`):
//! a new account is created with the name and password chosen there, or the
//! existing account of this email joins the tenant (its password stays).
//! An internal member is then signed in under the tenant's policy. The token
//! travels in request bodies, never in URLs the API logs.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{AppendHeaders, IntoResponse, Response};
use chrono::{DateTime, Utc};
use domain::account::{MAX_SIGN_INS_PER_IP, SIGN_IN_WINDOW, validate_display_name, validate_password};
use domain::auth_policy::{PasswordSignIn, password_allowed};
use domain::members::{Access, INVITATION_LIFETIME, InvitationInput, InvitationStatus, validate_invitation};
use domain::mfa::stage_after_password;
use infra::accounts::{self, NewMember};
use infra::email::{EMAIL_JOB, EmailJob, EmailMessage};
use infra::invitations::{self as repo, Invitation, InvitationFilter, NewInvitation, WriteConflict};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::auth::{ClientIp, SESSION_COOKIE, SessionView, TenantRoleView, cookie};
use crate::etag::{etag, require_if_match};
use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::idempotency::{self, Start};
use crate::me::AccountTypeView;
use crate::members::{TAG, require_member_admin};
use crate::pagination::{PageParams, encode_cursor};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::tenant::{AppState, PublicTenantTx, TenantTx};

const PATH: &str = "/api/v1/tenant/invitations";

/// Where an invitation stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = InvitationStatus)]
pub enum InvitationStatusView {
    /// Sent, waiting for the invitee.
    Pending,
    /// Not accepted in time: send it again.
    Expired,
    Accepted,
    Revoked,
}

impl From<InvitationStatus> for InvitationStatusView {
    fn from(status: InvitationStatus) -> Self {
        match status {
            InvitationStatus::Pending => Self::Pending,
            InvitationStatus::Expired => Self::Expired,
            InvitationStatus::Accepted => Self::Accepted,
            InvitationStatus::Revoked => Self::Revoked,
        }
    }
}

/// An invitation to join the tenant.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Invitation)]
pub struct InvitationView {
    id: Uuid,
    #[schema(example = "p.durand@client.example")]
    email: String,
    account_type: AccountTypeView,
    tenant_role: TenantRoleView,
    status: InvitationStatusView,
    /// The link works until then.
    expires_at: DateTime<Utc>,
    /// Name of the admin who sent it, if their account still exists.
    #[schema(example = "Lucas Bernard")]
    invited_by: Option<String>,
    created_at: DateTime<Utc>,
    #[schema(example = 1)]
    version: i64,
}

impl InvitationView {
    fn new(invitation: Invitation) -> Self {
        Self {
            status: invitation.status(Utc::now()).into(),
            id: invitation.id,
            email: invitation.email,
            account_type: invitation.account_type.into(),
            tenant_role: invitation.tenant_role.into(),
            expires_at: invitation.expires_at,
            invited_by: invitation.invited_by_name,
            created_at: invitation.created_at,
            version: invitation.version,
        }
    }
}

/// A page of invitations.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvitationPage {
    items: Vec<InvitationView>,
    /// Cursor of the next page, `null` on the last page.
    #[schema(required = true)]
    next_cursor: Option<String>,
}

/// `?search=&accountType=&limit=&cursor=` of the invitations list.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct InvitationListParams {
    /// Part of the email, any case.
    pub search: Option<String>,
    /// Only internal, or only external invitations.
    pub account_type: Option<AccountTypeView>,
    /// Page size, from 1 to 200 (default 50).
    #[param(minimum = 1, maximum = 200)]
    pub limit: Option<usize>,
    /// `nextCursor` of the previous page.
    pub cursor: Option<String>,
}

/// An invitation to send.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = NewInvitation,
    example = json!({"email": "p.durand@client.example", "accountType": "external", "tenantRole": "member"})
)]
pub struct NewInvitationBody {
    email: String,
    account_type: AccountTypeView,
    /// An external member is always `member`.
    tenant_role: TenantRoleView,
}

fn conflict_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
        WriteConflict::AlreadyInvited => Problem::new(ErrorCode::InvitationPending),
        WriteConflict::Closed => Problem::new(ErrorCode::InvitationClosed),
    }
}

fn expiry() -> DateTime<Utc> {
    Utc::now() + chrono::Duration::from_std(INVITATION_LIFETIME).unwrap_or(chrono::Duration::days(7))
}

/// Queues the invitation email of `invitation`, with its new `token`.
async fn send(
    state: &AppState,
    tx: &mut TenantTx,
    invitation: &Invitation,
    token: &str,
) -> Result<(), Problem> {
    let tenant_name = sqlx::query_scalar!("SELECT name FROM tenant")
        .fetch_one(&mut **tx)
        .await?;
    let email_job = EmailJob {
        to: invitation.email.clone(),
        // Nobody knows the invitee's language yet.
        locale: tx.default_locale().to_owned(),
        message: EmailMessage::Invitation {
            inviter_name: tx.member().display_name.clone(),
            tenant_name,
            link: format!(
                "{}/accept-invitation?token={token}",
                state.auth.public_url().for_tenant(tx.slug())
            ),
            valid_days: INVITATION_LIFETIME.as_secs() / 86_400,
            external: !invitation.account_type.reaches_internal_app(),
        },
    };
    let payload = serde_json::to_value(&email_job).map_err(|e| Problem::internal(&e))?;
    infra::jobs::enqueue(tx, EMAIL_JOB, &payload).await?;
    Ok(())
}

/// `GET /api/v1/tenant/invitations`: open invitations, in creation order.
#[utoipa::path(
    get,
    path = "/tenant/invitations",
    tag = TAG,
    operation_id = "listInvitations",
    summary = "List the open invitations",
    description = "Tenant admins only. Invitations not accepted or revoked yet, expired ones \
                   included (`status`), in creation order.",
    params(InvitationListParams),
    responses(
        (status = 200, description = "A page of invitations", body = InvitationPage),
        (status = 400, description = "Invalid `limit`, `cursor` or `accountType`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    mut tx: TenantTx,
    ApiQuery(params): ApiQuery<InvitationListParams>,
) -> Result<ApiJson<InvitationPage>, Problem> {
    require_member_admin(&tx)?;
    let (limit, after) = PageParams {
        limit: params.limit,
        cursor: params.cursor,
    }
    .resolve()?;
    let filter = InvitationFilter {
        search: params.search.as_deref().map(str::trim).filter(|s| !s.is_empty()),
        account_type: params.account_type.map(Into::into),
    };
    let page = repo::list_open(&mut tx, &filter, after, limit).await?;
    tx.commit().await?;
    Ok(ApiJson(InvitationPage {
        items: page.items.into_iter().map(InvitationView::new).collect(),
        next_cursor: page.next.map(encode_cursor),
    }))
}

/// `POST /api/v1/tenant/invitations`: idempotent with `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/tenant/invitations",
    tag = TAG,
    operation_id = "createInvitation",
    summary = "Invite someone to join the tenant",
    description = "Tenant admins only. Emails a link valid for 7 days. Replaying the same request \
                   with the same `Idempotency-Key` within 24 hours returns the first response.",
    params(
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters"),
    ),
    request_body = NewInvitationBody,
    responses(
        (status = 201, description = "Sent", body = InvitationView, headers(
            ("ETag" = String, description = "Version of the invitation"),
            ("Location" = String, description = "URL of the invitation"),
        )),
        (status = 400, description = "`validation_failed`: invalid email, or an external admin", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`invitation.already_member`, `invitation.pending` (send it again \
                                      instead), or `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiJson(body): ApiJson<NewInvitationBody>,
) -> Result<Response, Problem> {
    require_member_admin(&tx)?;
    let key = match idempotency::start(&mut tx, &headers, &format!("POST {PATH}"), &body).await? {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    let input = InvitationInput {
        email: body.email,
        access: Access {
            account_type: body.account_type.into(),
            tenant_role: body.tenant_role.into(),
        },
    };
    let email = validate_invitation(&input).map_err(Problem::validation)?;
    if accounts::find_by_email(&mut tx, &email).await?.is_some() {
        return Err(Problem::new(ErrorCode::InvitationAlreadyMember));
    }
    let token = infra::credentials::random_token();
    let invited_by = tx.account_id();
    let invitation = repo::insert(
        &mut tx,
        &NewInvitation {
            id: infra::id::new_id(),
            email: &email,
            access: input.access,
            token_hash: &infra::credentials::token_hash(&token),
            expires_at: expiry(),
            invited_by,
        },
    )
    .await?
    .map_err(conflict_problem)?;
    send(&state, &mut tx, &invitation, &token).await?;
    tracing::info!(
        target: "audit",
        actor = %tx.account_id(),
        invitation = %invitation.id,
        account_type = invitation.account_type.as_str(),
        tenant_role = invitation.tenant_role.as_str(),
        "member invited"
    );
    let (id, version) = (invitation.id, invitation.version);
    let response = idempotency::created(
        &mut tx,
        key.as_deref(),
        &InvitationView::new(invitation),
        format!("{PATH}/{id}"),
        version,
    )
    .await?;
    tx.commit().await?;
    Ok(response)
}

/// `POST /api/v1/tenant/invitations/{id}/resend`.
#[utoipa::path(
    post,
    path = "/tenant/invitations/{id}/resend",
    tag = TAG,
    operation_id = "resendInvitation",
    summary = "Send an invitation again",
    description = "Tenant admins only. Emails a new link, valid for 7 days from now; the previous \
                   link stops working. For pending and expired invitations.",
    params(("id" = Uuid, Path, description = "Id of the invitation")),
    responses(
        (status = 200, description = "Sent again", body = InvitationView, headers(
            ("ETag" = String, description = "New version of the invitation"),
        )),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such invitation in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`invitation.closed`: accepted or revoked", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn resend(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    require_member_admin(&tx)?;
    let token = infra::credentials::random_token();
    let invitation = repo::renew(
        &mut tx,
        id,
        None,
        &infra::credentials::token_hash(&token),
        expiry(),
    )
    .await?
    .map_err(conflict_problem)?;
    send(&state, &mut tx, &invitation, &token).await?;
    tracing::info!(target: "audit", actor = %tx.account_id(), invitation = %id, "invitation sent again");
    tx.commit().await?;
    let version = invitation.version;
    let mut response = ApiJson(InvitationView::new(invitation)).into_response();
    response
        .headers_mut()
        .insert(axum::http::header::ETAG, etag(version));
    Ok(response)
}

/// `DELETE /api/v1/tenant/invitations/{id}`: requires `If-Match`.
#[utoipa::path(
    delete,
    path = "/tenant/invitations/{id}",
    tag = TAG,
    operation_id = "revokeInvitation",
    summary = "Revoke an invitation",
    description = "Tenant admins only. Its link stops working.",
    params(
        ("id" = Uuid, Path, description = "Id of the invitation"),
        ("If-Match" = String, Header, description = "ETag of the invitation, or `*`"),
    ),
    responses(
        (status = 204, description = "Revoked"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such invitation in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`invitation.closed`: already accepted or revoked", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The invitation changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn revoke(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, Problem> {
    require_member_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    repo::revoke(&mut tx, id, if_match.versions())
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %tx.account_id(), invitation = %id, "invitation revoked");
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- Invitee ----------------------------------------------------------------------

/// The token of an invitation link.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvitationTokenBody {
    /// The `token` parameter of the link.
    token: String,
}

/// An invitation, as its invitee sees it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvitationPreview {
    #[schema(example = "p.durand@client.example")]
    email: String,
    #[schema(example = "Acme Industrie")]
    tenant_name: String,
    #[schema(example = "Lucas Bernard")]
    invited_by: Option<String>,
    account_type: AccountTypeView,
    /// No account has this email yet: accepting creates it, with the name and
    /// password chosen then. Otherwise the existing account joins, unchanged.
    new_account: bool,
    expires_at: DateTime<Utc>,
}

/// Accepting an invitation.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(example = json!({"token": "…", "displayName": "Paul Durand", "password": "a long passphrase"}))]
pub struct AcceptInvitationBody {
    token: String,
    /// For a new account only: required then.
    display_name: Option<String>,
    /// For a new account only: required then, 12 to 128 characters.
    #[schema(format = Password)]
    password: Option<String>,
}

/// The invitation is accepted.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvitationAccepted {
    account_type: AccountTypeView,
    /// The session opened for an internal member (cookies set), maybe at a
    /// two-step verification stage. `null` for an external member (request
    /// portal), or when the tenant signs its members in through its identity
    /// provider (`ssoRequired`).
    session: Option<SessionView>,
    /// Sign in with the tenant's identity provider.
    sso_required: bool,
}

/// 429 after too many invitation requests from `ip`.
async fn limit(state: &AppState, ip: &str) -> Result<(), Problem> {
    let key = format!("invitation:ip:{ip}");
    if state.auth.limits.hit(&key, SIGN_IN_WINDOW).await? > MAX_SIGN_INS_PER_IP {
        return Err(state.auth.too_many(&key).await);
    }
    Ok(())
}

async fn open_invitation(tx: &mut PublicTenantTx, token: &str) -> Result<Invitation, Problem> {
    repo::find_by_token(tx, &infra::credentials::token_hash(token))
        .await?
        .ok_or(Problem::new(ErrorCode::InvitationInvalid))
}

/// `POST /api/v1/auth/invitation`.
#[utoipa::path(
    post,
    path = "/auth/invitation",
    tag = crate::auth::TAG,
    operation_id = "getInvitation",
    summary = "Show an invitation to its invitee",
    description = "With the token of the invitation link (in the body, so that it is never \
                   logged). For the invitation page.",
    security(()),
    request_body = InvitationTokenBody,
    responses(
        (status = 200, description = "The invitation", body = InvitationPreview),
        (status = 400, description = "`invitation.invalid`: unknown, expired, accepted or revoked", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "`tenant.not_found`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 429, description = "`too_many_requests`, with `Retry-After`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn preview(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    mut tx: PublicTenantTx,
    ApiJson(body): ApiJson<InvitationTokenBody>,
) -> Result<ApiJson<InvitationPreview>, Problem> {
    limit(&state, &ip).await?;
    let invitation = open_invitation(&mut tx, &body.token).await?;
    let new_account = accounts::id_by_email(&mut tx, &invitation.email).await?.is_none();
    let tenant_name = sqlx::query_scalar!("SELECT name FROM tenant")
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(ApiJson(InvitationPreview {
        email: invitation.email,
        tenant_name,
        invited_by: invitation.invited_by_name,
        account_type: invitation.account_type.into(),
        new_account,
        expires_at: invitation.expires_at,
    }))
}

/// `POST /api/v1/auth/invitation/accept`.
#[utoipa::path(
    post,
    path = "/auth/invitation/accept",
    tag = crate::auth::TAG,
    operation_id = "acceptInvitation",
    summary = "Accept an invitation",
    description = "Joins the tenant with the account of the invitation's email, created with \
                   `displayName` and `password` when there is none. An internal member is then \
                   signed in under the tenant's policy (two-step verification, SSO). The link \
                   works once.",
    security(()),
    request_body = AcceptInvitationBody,
    responses(
        (status = 200, description = "Joined", body = InvitationAccepted, headers(
            ("Set-Cookie" = String, description = "`mlbpp_session` and `mlbpp_csrf`, when a session opens"),
        )),
        (status = 400, description = "`invitation.invalid`, or `validation_failed` (name, password)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "`tenant.not_found`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 429, description = "`too_many_requests`, with `Retry-After`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn accept(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    mut tx: PublicTenantTx,
    ApiJson(body): ApiJson<AcceptInvitationBody>,
) -> Result<Response, Problem> {
    limit(&state, &ip).await?;
    let invitation = open_invitation(&mut tx, &body.token).await?;
    let existing = accounts::id_by_email(&mut tx, &invitation.email).await?;

    // A new account needs a name and a password.
    let mut password_hash = None;
    let mut display_name = String::new();
    if existing.is_none() {
        display_name = body.display_name.unwrap_or_default().trim().to_owned();
        let password = body.password.unwrap_or_default();
        let mut errors = validate_display_name("displayName", &display_name)
            .err()
            .unwrap_or_default();
        errors.extend(
            validate_password("password", &password, &invitation.email)
                .err()
                .unwrap_or_default(),
        );
        if !errors.is_empty() {
            return Err(Problem::validation(errors));
        }
        let hash = tokio::task::spawn_blocking(move || infra::credentials::hash_password(&password))
            .await?
            .map_err(|e| Problem::internal(&e))?;
        password_hash = Some(hash);
    }

    if !repo::mark_accepted(&mut tx, invitation.id).await? {
        return Err(Problem::new(ErrorCode::InvitationInvalid));
    }
    // Already a member (added since the invitation): their access stays.
    let account_id = match accounts::find_by_email(&mut tx, &invitation.email).await? {
        Some(found) => found.member.account_id,
        None => {
            accounts::add_member(
                &mut tx,
                &NewMember {
                    email: &invitation.email,
                    display_name: &display_name,
                    password_hash: password_hash.as_deref(),
                    account_type: invitation.account_type,
                    tenant_role: invitation.tenant_role,
                    local_account: false,
                },
            )
            .await?
            .account_id
        }
    };
    tracing::info!(
        target: "audit",
        account = %account_id,
        invitation = %invitation.id,
        new_account = existing.is_none(),
        "invitation accepted"
    );
    let member = accounts::find(&mut tx, account_id)
        .await?
        .ok_or(Problem::new(ErrorCode::InvitationInvalid))?;

    let account_type = member.account_type.into();
    if !(member.active && member.account_type.reaches_internal_app()) {
        tx.commit().await?;
        return Ok(ApiJson(InvitationAccepted {
            account_type,
            session: None,
            sso_required: false,
        })
        .into_response());
    }
    let policy = infra::auth_policy::get(&mut tx).await?;
    let who = PasswordSignIn {
        internal: true,
        local_account: member.local_account,
        tenant_has_sso: infra::oidc_providers::has_enabled(&mut tx).await?,
    };
    if !password_allowed(policy.rules(), who) {
        tx.commit().await?;
        return Ok(ApiJson(InvitationAccepted {
            account_type,
            session: None,
            sso_required: true,
        })
        .into_response());
    }
    let mfa_enabled = infra::mfa::is_enabled(&mut tx, account_id).await?;
    let stage = stage_after_password(mfa_enabled, policy.settings.local_accounts_require_mfa);
    let auth = &state.auth;
    if let Some(previous) = cookie(&headers, SESSION_COOKIE) {
        auth.sessions.delete(previous).await?;
    }
    let opened = auth
        .sessions
        .create_staged(account_id, tx.tenant_id(), stage)
        .await?;
    tx.commit().await?;
    Ok((
        AppendHeaders(auth.session_cookies(&opened.token, &opened.session)?),
        ApiJson(InvitationAccepted {
            account_type,
            session: Some(SessionView::new(&member, &opened.session)),
            sso_required: false,
        }),
    )
        .into_response())
}
