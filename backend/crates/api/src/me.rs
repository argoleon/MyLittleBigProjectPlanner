//! The signed-in account and its tenants (specification §3, L02-US05).
//!
//! An account is global and may belong to several tenants; a session holds
//! one tenant. The tenant switcher lists the account's memberships
//! (`/me/memberships`) through `app_my_memberships()`, the only query path
//! across tenants, which answers for the caller only.
//!
//! A session's cookie belongs to its tenant's host, so switching takes two
//! steps: `POST /me/switch` issues a one-time token (60 seconds) and the
//! address of the target tenant's `GET /auth/switch?token=`, where the
//! browser goes. There the target tenant applies its own policy: SSO it
//! requires sends to its sign-in page; otherwise a new session opens, at
//! the two-step verification stage when the account needs it. Nothing of
//! the session left behind is carried over but what it proved.

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{AppendHeaders, IntoResponse, Redirect, Response};
use domain::auth_policy::{Proof, Switch, SwitchOutcome, after_switch};
use domain::tenancy::Tenancy;
use infra::accounts::AccountType;
use infra::memberships::MyMembership;
use infra::sessions::SessionStage;
use infra::tenant_switch::PendingSwitch;
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::auth::{Auth, SESSION_COOKIE, TenantRoleView, cookie};
use crate::extract::{ApiJson, ApiQuery};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::sso::sign_in_error;
use crate::tenant::{AppState, PublicTenantTx, TenantTx};

/// OpenAPI tag of the signed-in account.
pub const TAG: &str = "me";

/// Path of the switch's arrival, under the target tenant's public address.
pub const SWITCH_PATH: &str = "/api/v1/auth/switch";

/// Kind of account in a tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = AccountType)]
pub enum AccountTypeView {
    /// A member of the organization: uses the internal application.
    Internal,
    /// A customer or partner: uses the request portal only.
    External,
}

impl From<AccountTypeView> for AccountType {
    fn from(account_type: AccountTypeView) -> Self {
        match account_type {
            AccountTypeView::Internal => Self::Internal,
            AccountTypeView::External => Self::External,
        }
    }
}

impl From<AccountType> for AccountTypeView {
    fn from(account_type: AccountType) -> Self {
        match account_type {
            AccountType::Internal => Self::Internal,
            AccountType::External => Self::External,
        }
    }
}

/// The signed-in account, as the current tenant sees it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Me)]
pub struct MeView {
    id: Uuid,
    #[schema(example = "lucas.bernard@acme.example")]
    email: String,
    #[schema(example = "Lucas Bernard")]
    display_name: String,
    /// Preferred language; the tenant's default when absent.
    #[schema(example = "fr")]
    locale: Option<String>,
    tenant_id: Uuid,
    tenant_role: TenantRoleView,
    account_type: AccountTypeView,
}

/// A tenant the signed-in account belongs to.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Membership)]
pub struct MembershipView {
    tenant_id: Uuid,
    #[schema(example = "acme")]
    slug: String,
    #[schema(example = "Acme")]
    name: String,
    /// Address of the tenant's application.
    #[schema(example = "https://acme.planner.example")]
    url: String,
    account_type: AccountTypeView,
    tenant_role: TenantRoleView,
    /// The tenant of the current session.
    current: bool,
}

/// `POST /me/switch` body.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SwitchRequest {
    /// The tenant to switch to: one of `/me/memberships`.
    tenant_id: Uuid,
}

/// Where the browser goes to finish the switch.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = TenantSwitch)]
pub struct SwitchView {
    /// To navigate to (not to fetch): it sets the target tenant's cookies.
    /// Works once, within 60 seconds.
    url: String,
}

/// `GET /api/v1/me`.
#[utoipa::path(
    get,
    path = "/me",
    tag = TAG,
    operation_id = "getMe",
    summary = "The signed-in account",
    description = "The account and its role in the current tenant.",
    responses(
        (status = 200, description = "The account", body = MeView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: an external account (request portal only)", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn me(tx: TenantTx) -> ApiJson<MeView> {
    let member = tx.member();
    ApiJson(MeView {
        id: member.account_id,
        email: member.email.clone(),
        display_name: member.display_name.clone(),
        locale: member.locale.clone(),
        tenant_id: tx.tenant_id(),
        tenant_role: member.tenant_role.into(),
        account_type: member.account_type.into(),
    })
}

/// The account's memberships the switcher offers: every one in multi-tenant
/// mode, the current tenant only in single-tenant mode (one host, one tenant).
async fn listed(state: &AppState, tx: &mut TenantTx) -> Result<Vec<MyMembership>, Problem> {
    let all = infra::memberships::mine(tx).await?;
    let current = tx.tenant_id();
    Ok(match *state.tenancy {
        Tenancy::Multi { .. } => all,
        Tenancy::Single { .. } => all.into_iter().filter(|m| m.tenant_id == current).collect(),
    })
}

/// `GET /api/v1/me/memberships`.
#[utoipa::path(
    get,
    path = "/me/memberships",
    tag = TAG,
    operation_id = "listMyMemberships",
    summary = "The tenants of the signed-in account",
    description = "Active memberships of active tenants, by tenant name, with the account's role \
                   in each: the tenant switcher. In single-tenant mode, the current tenant only.",
    responses(
        (status = 200, description = "The memberships", body = Vec<MembershipView>),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: an external account", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn memberships(
    State(state): State<AppState>,
    mut tx: TenantTx,
) -> Result<ApiJson<Vec<MembershipView>>, Problem> {
    let memberships = listed(&state, &mut tx).await?;
    let current = tx.tenant_id();
    tracing::info!(account = %tx.account_id(), count = memberships.len(), "memberships read across tenants");
    tx.commit().await?;
    let public_url = state.auth.public_url();
    Ok(ApiJson(
        memberships
            .into_iter()
            .map(|m| MembershipView {
                url: public_url.for_tenant(&m.slug),
                current: m.tenant_id == current,
                tenant_id: m.tenant_id,
                slug: m.slug,
                name: m.name,
                account_type: m.account_type.into(),
                tenant_role: m.tenant_role.into(),
            })
            .collect(),
    ))
}

/// `POST /api/v1/me/switch`.
#[utoipa::path(
    post,
    path = "/me/switch",
    tag = TAG,
    operation_id = "switchTenant",
    summary = "Start switching to another tenant",
    description = "Issues a one-time link to the target tenant, valid for 60 seconds. The browser \
                   navigates to it: the target applies its own policy and opens a new session \
                   there (its SSO, two-step verification). The current session stays open.",
    request_body = SwitchRequest,
    responses(
        (status = 200, description = "Where the browser goes", body = SwitchView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: an external account here or in the target, or \
                                      `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "`not_found`: not a tenant of the account", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn switch(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiJson(body): ApiJson<SwitchRequest>,
) -> Result<ApiJson<SwitchView>, Problem> {
    let target = listed(&state, &mut tx)
        .await?
        .into_iter()
        .find(|m| m.tenant_id == body.tenant_id)
        .ok_or(Problem::new(ErrorCode::NotFound))?;
    if !target.account_type.reaches_internal_app() {
        // The request portal has its own sign-in.
        return Err(Problem::new(ErrorCode::Forbidden));
    }
    let account_id = tx.account_id();
    let proof = tx.session().proof();
    tx.commit().await?;

    let token = infra::credentials::random_token();
    let pending = PendingSwitch {
        account_id,
        target_tenant_id: target.tenant_id,
        second_factor: proof.second_factor,
        sso: proof.sso,
    };
    state.auth.switches.save(&token, &pending).await?;
    tracing::info!(account = %account_id, target = %target.tenant_id, "tenant switch issued");
    Ok(ApiJson(SwitchView {
        url: format!(
            "{}{SWITCH_PATH}?token={token}",
            state.auth.public_url().for_tenant(&target.slug)
        ),
    }))
}

/// `?token=` of the switch's arrival.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ArrivalParams {
    /// Issued by `POST /me/switch`.
    pub token: Option<String>,
}

/// `GET /api/v1/auth/switch`.
#[utoipa::path(
    get,
    path = "/auth/switch",
    tag = crate::auth::TAG,
    operation_id = "finishTenantSwitch",
    summary = "Arrive in another tenant",
    description = "A browser navigation, to the link of `POST /me/switch`. The token must be \
                   unused, less than 60 seconds old and issued for this tenant, to an active \
                   internal member of it. The tenant's policy applies: when it requires its \
                   identity provider, the browser goes to the sign-in page with \
                   `?error=auth.sso_required`; otherwise a session opens (cookies) and the \
                   browser goes to the application, which asks for a verification code when \
                   the account has two-step verification and the session left did not check \
                   it. Any other failure: the sign-in page with `?error=auth.switch_failed`.",
    security(()),
    params(ArrivalParams),
    responses(
        (status = 303, description = "Into the application, or to the sign-in page", headers(
            ("Location" = String, description = "Where the browser goes"),
            ("Set-Cookie" = String, description = "`mlbpp_session` and `mlbpp_csrf`, when a session opens"),
        )),
        (status = 404, description = "`tenant.not_found`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn arrive(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut tx: PublicTenantTx,
    ApiQuery(params): ApiQuery<ArrivalParams>,
) -> Result<Response, Problem> {
    let auth: &Auth = &state.auth;
    let slug = tx.slug().to_owned();
    let pending = match &params.token {
        Some(token) => auth.switches.take(token).await?,
        None => None,
    };
    let Some(pending) = pending.filter(|p| p.target_tenant_id == tx.tenant_id()) else {
        tracing::warn!("tenant switch with an unknown, used or foreign token");
        return Ok(sign_in_error(auth, &slug, ErrorCode::AuthSwitchFailed));
    };
    // Suspended, removed or external since the token was issued.
    let Some(member) = infra::accounts::find(&mut tx, pending.account_id)
        .await?
        .filter(|m| m.active && m.account_type.reaches_internal_app())
    else {
        tracing::info!(account = %pending.account_id, "tenant switch of an account that is no internal member");
        return Ok(sign_in_error(auth, &slug, ErrorCode::AuthSwitchFailed));
    };
    let policy = infra::auth_policy::get(&mut tx).await?;
    let outcome = after_switch(Switch {
        policy: policy.rules(),
        local_account: member.local_account,
        tenant_has_sso: infra::oidc_providers::has_enabled(&mut tx).await?,
        account_mfa_enabled: infra::mfa::is_enabled(&mut tx, member.account_id).await?,
        proof: Proof {
            second_factor: pending.second_factor,
            sso: pending.sso,
        },
    });
    let stage = match outcome {
        SwitchOutcome::SignInThroughProvider => {
            tx.commit().await?;
            return Ok(sign_in_error(auth, &slug, ErrorCode::AuthSsoRequired));
        }
        SwitchOutcome::Session(stage) => stage,
    };
    // A new token: a session planted on this host before is not reused.
    if let Some(previous) = cookie(&headers, SESSION_COOKIE) {
        auth.sessions.delete(previous).await?;
    }
    // This tenant's provider was not used; the second factor counts once checked.
    let proof = Proof {
        second_factor: pending.second_factor && stage == SessionStage::Complete,
        sso: false,
    };
    let opened = auth
        .sessions
        .create_proven(member.account_id, tx.tenant_id(), stage, proof)
        .await?;
    tx.commit().await?;
    tracing::info!(account = %member.account_id, stage = stage.as_str(), "arrived from a tenant switch");
    Ok((
        AppendHeaders(auth.session_cookies(&opened.token, &opened.session)?),
        Redirect::to(&format!("{}/", auth.public_url().for_tenant(&slug))),
    )
        .into_response())
}
