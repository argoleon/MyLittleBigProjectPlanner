//! Sign-in through the tenant's OIDC providers and the tenant's
//! authentication policy (specification §3, §14).
//!
//! - Tenant admins configure the policy (`/tenant/auth-policy`) and the
//!   providers (`/tenant/oidc-providers`). Client secrets are written, never
//!   read back.
//! - The sign-in page lists the providers (`/auth/providers`); a button
//!   leads the browser to `/auth/oidc/{id}/start`, which sends it to the
//!   provider, which sends it back to `/auth/oidc/callback`. There, the ID
//!   token's verified email designates an active internal member of the
//!   tenant, and a session opens. Both answer redirects, never JSON: they
//!   are browser navigations. Errors land on the sign-in page with
//!   `?error=<code>`.

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{AppendHeaders, IntoResponse, Redirect, Response};
use chrono::{DateTime, Utc};
use domain::oidc::{EmailRefusal, ProviderInput, matching_email, validate_provider};
use domain::validation::FieldError;
use infra::accounts::{AccountType, TenantRole};
use infra::auth_policy::{AuthPolicy, PolicySettings};
use infra::oidc::{ClientCredentials, PendingLogin};
use infra::oidc_providers::{self as repo, Conflict, EncryptedSecret, Provider, ProviderFields};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::auth::{Auth, isolated_return_url};
use crate::etag::{etag, if_none_match, require_if_match};
use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::tenant::{AppState, PublicTenantTx, TenantTx};

/// OpenAPI tag of the tenant administration of authentication.
pub const TAG: &str = "authentication-settings";

/// Path of the OIDC callback, under the tenant's public address.
pub const CALLBACK_PATH: &str = "/api/v1/auth/oidc/callback";

const PROVIDERS_PATH: &str = "/api/v1/tenant/oidc-providers";

fn require_tenant_admin(tx: &TenantTx) -> Result<(), Problem> {
    if tx.member().tenant_role == TenantRole::TenantAdmin {
        Ok(())
    } else {
        Err(Problem::new(ErrorCode::Forbidden))
    }
}

// --- Policy ---------------------------------------------------------------------

/// Authentication policy of the tenant.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = AuthPolicy,
    example = json!({
        "localAccountsRequireMfa": true,
        "internalAccountsRequireSso": true,
        "portalPasswordEnabled": true,
        "portalMagicLinkEnabled": true
    })
)]
pub struct PolicyBody {
    /// Members signing in with a password must use two-step verification;
    /// those without it enroll at their next sign-in.
    local_accounts_require_mfa: bool,
    /// Internal members sign in through an enabled OIDC provider of the
    /// tenant, except the members marked as local accounts. Without an
    /// enabled provider, nobody is held to it.
    internal_accounts_require_sso: bool,
    /// Request portal: sign-in with a password.
    portal_password_enabled: bool,
    /// Request portal: sign-in with a link sent by email.
    portal_magic_link_enabled: bool,
}

impl From<PolicySettings> for PolicyBody {
    fn from(s: PolicySettings) -> Self {
        Self {
            local_accounts_require_mfa: s.local_accounts_require_mfa,
            internal_accounts_require_sso: s.internal_accounts_require_sso,
            portal_password_enabled: s.portal_password_enabled,
            portal_magic_link_enabled: s.portal_magic_link_enabled,
        }
    }
}

impl From<PolicyBody> for PolicySettings {
    fn from(b: PolicyBody) -> Self {
        Self {
            local_accounts_require_mfa: b.local_accounts_require_mfa,
            internal_accounts_require_sso: b.internal_accounts_require_sso,
            portal_password_enabled: b.portal_password_enabled,
            portal_magic_link_enabled: b.portal_magic_link_enabled,
        }
    }
}

fn policy_response(policy: AuthPolicy) -> Response {
    let mut response = ApiJson(PolicyBody::from(policy.settings)).into_response();
    response.headers_mut().insert(header::ETAG, etag(policy.version));
    response
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
    let policy = infra::auth_policy::get(&mut tx).await?;
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
    description = "Tenant admins only. Every field is required.",
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
    let policy = infra::auth_policy::set(&mut tx, &body.into(), if_match.versions())
        .await?
        .ok_or(Problem::new(ErrorCode::PreconditionFailed))?;
    tx.commit().await?;
    tracing::info!(?policy, "authentication policy changed");
    Ok(policy_response(policy))
}

// --- Providers ------------------------------------------------------------------

/// A provider as the tenant admin writes it.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = OidcProviderInput,
    example = json!({
        "displayName": "Entra ID",
        "issuer": "https://login.microsoftonline.com/<tenant>/v2.0",
        "clientId": "6731de76-14a6-49ae-97bc-6eba6914391e",
        "clientSecret": "<secret>",
        "emailClaim": "email",
        "enabled": true
    })
)]
pub struct ProviderBody {
    /// Shown on the sign-in button: "Continue with …". 1 to 100 characters.
    display_name: String,
    /// Issuer URL (HTTPS); `/.well-known/openid-configuration` is read from it.
    issuer: String,
    client_id: String,
    /// Required on creation; on update, absent keeps the stored secret.
    /// Never returned.
    client_secret: Option<String>,
    /// Claim of the ID token holding the member's email. With `email`
    /// (default), the provider must also send `email_verified: true`; another
    /// claim (`upn`…) is trusted as the directory's.
    #[serde(default = "default_email_claim")]
    #[schema(default = "email")]
    email_claim: String,
    /// Default true.
    #[serde(default = "enabled_by_default")]
    #[schema(default = true)]
    enabled: bool,
}

fn default_email_claim() -> String {
    "email".to_owned()
}

fn enabled_by_default() -> bool {
    true
}

impl ProviderBody {
    fn input(&self) -> ProviderInput {
        ProviderInput {
            display_name: self.display_name.clone(),
            issuer: self.issuer.trim().to_owned(),
            client_id: self.client_id.trim().to_owned(),
            client_secret: self.client_secret.clone(),
            email_claim: self.email_claim.clone(),
            enabled: self.enabled,
        }
    }
}

/// A provider as configured; the client secret is never sent back.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = OidcProvider)]
pub struct ProviderView {
    id: Uuid,
    display_name: String,
    issuer: String,
    client_id: String,
    email_claim: String,
    enabled: bool,
    /// The redirect URI to register at the provider.
    #[schema(example = "https://acme.planner.example.org/api/v1/auth/oidc/callback")]
    redirect_uri: String,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl ProviderView {
    fn new(provider: Provider, redirect_uri: String) -> Self {
        Self {
            id: provider.id,
            display_name: provider.display_name,
            issuer: provider.issuer,
            client_id: provider.client_id,
            email_claim: provider.email_claim,
            enabled: provider.enabled,
            redirect_uri,
            version: provider.version,
            created_at: provider.created_at,
            updated_at: provider.updated_at,
        }
    }
}

/// What the provider announces, when it answers.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = OidcProviderCheck)]
pub struct CheckView {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
}

/// A provider of the sign-in page.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SignInProvider)]
pub struct ButtonView {
    id: Uuid,
    #[schema(example = "Entra ID")]
    display_name: String,
}

fn redirect_uri(auth: &Auth, slug: &str) -> String {
    format!("{}{CALLBACK_PATH}", auth.public_url().for_tenant(slug))
}

/// The tenant's slug, for its public address.
async fn tenant_slug(tx: &mut TenantTx) -> Result<String, Problem> {
    Ok(sqlx::query_scalar!("SELECT slug FROM tenant")
        .fetch_one(&mut **tx)
        .await?)
}

fn conflict_problem(conflict: Conflict) -> Problem {
    match conflict {
        Conflict::NotFound => Problem::new(ErrorCode::NotFound),
        Conflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
        Conflict::Duplicate => Problem::new(ErrorCode::OidcProviderDuplicate)
            .with_errors(vec![FieldError::new("issuer", "taken")]),
    }
}

fn fields(input: &ProviderInput) -> ProviderFields<'_> {
    ProviderFields {
        display_name: input.display_name.trim(),
        issuer: &input.issuer,
        client_id: &input.client_id,
        email_claim: &input.email_claim,
        enabled: input.enabled,
    }
}

fn provider_response(provider: Provider, redirect_uri: String, status: StatusCode) -> Response {
    let (id, version) = (provider.id, provider.version);
    let mut response = (status, ApiJson(ProviderView::new(provider, redirect_uri))).into_response();
    response.headers_mut().insert(header::ETAG, etag(version));
    if status == StatusCode::CREATED
        && let Ok(location) = HeaderValue::from_str(&format!("{PROVIDERS_PATH}/{id}"))
    {
        response.headers_mut().insert(header::LOCATION, location);
    }
    response
}

/// `GET /api/v1/tenant/oidc-providers`.
#[utoipa::path(
    get,
    path = "/tenant/oidc-providers",
    tag = TAG,
    operation_id = "listOidcProviders",
    summary = "OIDC providers of the tenant",
    description = "Tenant admins only. In creation order; a tenant has few.",
    responses(
        (status = 200, description = "The providers", body = Vec<ProviderView>),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list_providers(State(state): State<AppState>, mut tx: TenantTx) -> Result<Response, Problem> {
    require_tenant_admin(&tx)?;
    let slug = tenant_slug(&mut tx).await?;
    let providers = repo::list(&mut tx).await?;
    tx.commit().await?;
    let uri = redirect_uri(&state.auth, &slug);
    let views: Vec<_> = providers
        .into_iter()
        .map(|p| ProviderView::new(p, uri.clone()))
        .collect();
    Ok(ApiJson(views).into_response())
}

/// `POST /api/v1/tenant/oidc-providers`.
#[utoipa::path(
    post,
    path = "/tenant/oidc-providers",
    tag = TAG,
    operation_id = "createOidcProvider",
    summary = "Add an OIDC provider",
    description = "Tenant admins only. Register `redirectUri` (in the answer) at the provider.",
    request_body = ProviderBody,
    responses(
        (status = 201, description = "Created", body = ProviderView, headers(
            ("ETag" = String, description = "Version of the provider"),
            ("Location" = String, description = "URL of the provider"),
        )),
        (status = 400, description = "Invalid fields or JSON", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`oidc_provider.duplicate`: same issuer and client", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create_provider(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiJson(body): ApiJson<ProviderBody>,
) -> Result<Response, Problem> {
    require_tenant_admin(&tx)?;
    let input = body.input();
    validate_provider(&input, true, state.auth.oidc_allow_http).map_err(Problem::validation)?;
    let id = infra::id::new_id();
    let secret = input.client_secret.as_deref().unwrap_or_default();
    let (ciphertext, nonce) = state
        .auth
        .cipher
        .encrypt(id.as_bytes(), secret.as_bytes())
        .map_err(|e| Problem::internal(&e))?;
    let slug = tenant_slug(&mut tx).await?;
    let provider = repo::create(
        &mut tx,
        id,
        &fields(&input),
        EncryptedSecret {
            ciphertext: &ciphertext,
            nonce: &nonce,
        },
    )
    .await?
    .map_err(conflict_problem)?;
    tx.commit().await?;
    tracing::info!(provider = %provider.id, issuer = %provider.issuer, "OIDC provider added");
    Ok(provider_response(
        provider,
        redirect_uri(&state.auth, &slug),
        StatusCode::CREATED,
    ))
}

/// `GET /api/v1/tenant/oidc-providers/{id}`.
#[utoipa::path(
    get,
    path = "/tenant/oidc-providers/{id}",
    tag = TAG,
    operation_id = "getOidcProvider",
    summary = "An OIDC provider of the tenant",
    description = "Tenant admins only.",
    params(
        ("id" = Uuid, Path, description = "Id of the provider"),
        ("If-None-Match" = Option<String>, Header, description = "ETag already held: 304 if still current"),
    ),
    responses(
        (status = 200, description = "The provider", body = ProviderView, headers(
            ("ETag" = String, description = "Version of the provider"),
        )),
        (status = 304, description = "Not modified since the `If-None-Match` ETag"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such provider in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn read_provider(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    require_tenant_admin(&tx)?;
    let slug = tenant_slug(&mut tx).await?;
    let provider = repo::find(&mut tx, id)
        .await?
        .ok_or(Problem::new(ErrorCode::NotFound))?;
    tx.commit().await?;
    if if_none_match(&headers, provider.version) {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        response
            .headers_mut()
            .insert(header::ETAG, etag(provider.version));
        return Ok(response);
    }
    Ok(provider_response(
        provider,
        redirect_uri(&state.auth, &slug),
        StatusCode::OK,
    ))
}

/// `PUT /api/v1/tenant/oidc-providers/{id}`: requires `If-Match`.
#[utoipa::path(
    put,
    path = "/tenant/oidc-providers/{id}",
    tag = TAG,
    operation_id = "updateOidcProvider",
    summary = "Change an OIDC provider",
    description = "Tenant admins only. Without `clientSecret`, the stored secret is kept.",
    params(
        ("id" = Uuid, Path, description = "Id of the provider"),
        ("If-Match" = String, Header, description = "ETag of the version being replaced, or `*`"),
    ),
    request_body = ProviderBody,
    responses(
        (status = 200, description = "The updated provider", body = ProviderView, headers(
            ("ETag" = String, description = "New version of the provider"),
        )),
        (status = 400, description = "Invalid fields or JSON", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such provider in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`oidc_provider.duplicate`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The provider changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update_provider(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ProviderBody>,
) -> Result<Response, Problem> {
    require_tenant_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let input = body.input();
    validate_provider(&input, false, state.auth.oidc_allow_http).map_err(Problem::validation)?;
    let encrypted = match &input.client_secret {
        Some(secret) => Some(
            state
                .auth
                .cipher
                .encrypt(id.as_bytes(), secret.as_bytes())
                .map_err(|e| Problem::internal(&e))?,
        ),
        None => None,
    };
    let secret = encrypted
        .as_ref()
        .map(|(ciphertext, nonce)| EncryptedSecret { ciphertext, nonce });
    let slug = tenant_slug(&mut tx).await?;
    let provider = repo::update(&mut tx, id, if_match.versions(), &fields(&input), secret)
        .await?
        .map_err(conflict_problem)?;
    tx.commit().await?;
    tracing::info!(provider = %provider.id, "OIDC provider changed");
    Ok(provider_response(
        provider,
        redirect_uri(&state.auth, &slug),
        StatusCode::OK,
    ))
}

/// `DELETE /api/v1/tenant/oidc-providers/{id}`: requires `If-Match`.
#[utoipa::path(
    delete,
    path = "/tenant/oidc-providers/{id}",
    tag = TAG,
    operation_id = "deleteOidcProvider",
    summary = "Remove an OIDC provider",
    description = "Tenant admins only.",
    params(
        ("id" = Uuid, Path, description = "Id of the provider"),
        ("If-Match" = String, Header, description = "ETag of the version being deleted, or `*`"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such provider in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The provider changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete_provider(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, Problem> {
    require_tenant_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    repo::delete(&mut tx, id, if_match.versions())
        .await?
        .map_err(conflict_problem)?;
    tx.commit().await?;
    tracing::info!(provider = %id, "OIDC provider removed");
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/tenant/oidc-providers/{id}/test`.
#[utoipa::path(
    post,
    path = "/tenant/oidc-providers/{id}/test",
    tag = TAG,
    operation_id = "testOidcProvider",
    summary = "Check that an OIDC provider answers",
    description = "Tenant admins only. Reads the provider's metadata from its issuer.",
    params(("id" = Uuid, Path, description = "Id of the provider")),
    responses(
        (status = 200, description = "The provider answers", body = CheckView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such provider in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 502, description = "`oidc_provider.unreachable`, with the reason in `detail`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn test_provider(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<ApiJson<CheckView>, Problem> {
    require_tenant_admin(&tx)?;
    let provider = repo::find(&mut tx, id)
        .await?
        .ok_or(Problem::new(ErrorCode::NotFound))?;
    tx.commit().await?;
    let metadata = infra::oidc::discover(&state.auth.http, &provider.issuer)
        .await
        .map_err(|e| Problem::new(ErrorCode::OidcProviderUnreachable).with_detail(e.to_string()))?;
    Ok(ApiJson(CheckView {
        issuer: metadata.issuer,
        authorization_endpoint: metadata.authorization_endpoint,
        token_endpoint: metadata.token_endpoint,
    }))
}

// --- Sign-in --------------------------------------------------------------------

/// `GET /api/v1/auth/providers`.
#[utoipa::path(
    get,
    path = "/auth/providers",
    tag = crate::auth::TAG,
    operation_id = "listSignInProviders",
    summary = "Identity providers of the sign-in page",
    description = "The enabled OIDC providers of the tenant, for the sign-in buttons. A button \
                   leads the browser to `/api/v1/auth/oidc/{id}/start`.",
    security(()),
    responses(
        (status = 200, description = "The providers", body = Vec<ButtonView>),
        (status = 404, description = "`tenant.not_found`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn sign_in_providers(mut tx: PublicTenantTx) -> Result<ApiJson<Vec<ButtonView>>, Problem> {
    let buttons = repo::buttons(&mut tx).await?;
    tx.commit().await?;
    Ok(ApiJson(
        buttons
            .into_iter()
            .map(|b| ButtonView {
                id: b.id,
                display_name: b.display_name,
            })
            .collect(),
    ))
}

/// `?returnUrl=` of the start.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct StartParams {
    /// Page of the application to show once signed in (a path).
    pub return_url: Option<String>,
}

/// The sign-in page with an error, as the browser's next stop.
pub(crate) fn sign_in_error(auth: &Auth, slug: &str, code: ErrorCode) -> Response {
    Redirect::to(&format!(
        "{}/sign-in?error={}",
        auth.public_url().for_tenant(slug),
        code.as_str()
    ))
    .into_response()
}

/// `GET /api/v1/auth/oidc/{id}/start`.
#[utoipa::path(
    get,
    path = "/auth/oidc/{id}/start",
    tag = crate::auth::TAG,
    operation_id = "startOidcSignIn",
    summary = "Leave for the identity provider",
    description = "A browser navigation: redirects to the provider, with state, nonce and PKCE. \
                   On error, redirects to the sign-in page with `?error=<code>`.",
    security(()),
    params(("id" = Uuid, Path, description = "Id of the provider"), StartParams),
    responses(
        (status = 303, description = "To the provider, or back to the sign-in page", headers(
            ("Location" = String, description = "Where the browser goes"),
        )),
        (status = 404, description = "`tenant.not_found`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn start(
    State(state): State<AppState>,
    mut tx: PublicTenantTx,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(params): ApiQuery<StartParams>,
) -> Result<Response, Problem> {
    let auth = &state.auth;
    let slug = tx.slug().to_owned();
    let Some(provider) = repo::find(&mut tx, id).await?.filter(|p| p.enabled) else {
        return Ok(sign_in_error(auth, &slug, ErrorCode::AuthOidcFailed));
    };
    let tenant_id = tx.tenant_id();
    tx.commit().await?;
    let metadata = match infra::oidc::discover(&auth.http, &provider.issuer).await {
        Ok(metadata) => metadata,
        Err(err) => {
            tracing::warn!(provider = %id, error = %err, "OIDC provider unreachable");
            return Ok(sign_in_error(auth, &slug, ErrorCode::AuthOidcFailed));
        }
    };
    let started = infra::oidc::start(&metadata, &provider.client_id, &redirect_uri(auth, &slug))
        .map_err(|e| Problem::internal(&e))?;
    let pending = PendingLogin {
        tenant_id,
        provider_id: provider.id,
        nonce: started.nonce,
        code_verifier: started.code_verifier,
        return_url: params.return_url.filter(|url| isolated_return_url(url)),
    };
    auth.logins.save(&started.state, &pending).await?;
    Ok(Redirect::to(&started.url).into_response())
}

/// What the provider sends back.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CallbackParams {
    pub code: Option<String>,
    pub state: Option<String>,
    /// Set by the provider when the user cancelled or it failed.
    pub error: Option<String>,
}

/// `GET /api/v1/auth/oidc/callback`.
#[utoipa::path(
    get,
    path = "/auth/oidc/callback",
    tag = crate::auth::TAG,
    operation_id = "finishOidcSignIn",
    summary = "Come back from the identity provider",
    description = "A browser navigation, the redirect URI registered at the provider. The state \
                   must be one this API issued for this tenant, unused and less than 10 minutes \
                   old; the ID token is checked (signature, issuer, audience, expiry, nonce). Its \
                   verified email must be an active internal member's: a session opens (cookies) \
                   and the browser goes to the page it came from. Otherwise, to the sign-in page \
                   with `?error=auth.oidc_failed`, `auth.oidc_no_member` or \
                   `auth.oidc_email_unverified`.",
    security(()),
    params(CallbackParams),
    responses(
        (status = 303, description = "Into the application, or back to the sign-in page", headers(
            ("Location" = String, description = "Where the browser goes"),
            ("Set-Cookie" = String, description = "`mlbpp_session` and `mlbpp_csrf`, when signed in"),
        )),
        (status = 404, description = "`tenant.not_found`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn callback(
    State(state): State<AppState>,
    mut tx: PublicTenantTx,
    ApiQuery(params): ApiQuery<CallbackParams>,
) -> Result<Response, Problem> {
    let auth = &state.auth;
    let slug = tx.slug().to_owned();
    let fail = |code: ErrorCode| Ok(sign_in_error(auth, &slug, code));

    // The state is taken even when the provider reports an error: once only.
    let pending = match &params.state {
        Some(state) => auth.logins.take(state).await?,
        None => None,
    };
    let Some(pending) = pending.filter(|p| p.tenant_id == tx.tenant_id()) else {
        tracing::warn!("OIDC callback with an unknown, used or foreign state");
        return fail(ErrorCode::AuthOidcFailed);
    };
    let (Some(code), None) = (&params.code, &params.error) else {
        tracing::info!(error = ?params.error, "the identity provider returned no code");
        return fail(ErrorCode::AuthOidcFailed);
    };
    let Some(provider) = repo::find(&mut tx, pending.provider_id)
        .await?
        .filter(|p| p.enabled)
    else {
        return fail(ErrorCode::AuthOidcFailed);
    };
    let secret = auth
        .cipher
        .decrypt(
            provider.id.as_bytes(),
            &provider.client_secret_ciphertext,
            &provider.client_secret_nonce,
        )
        .map_err(|e| Problem::internal(&e))?;
    let secret = String::from_utf8(secret).map_err(|e| Problem::internal(&e))?;
    let claims = match async {
        let metadata = infra::oidc::discover(&auth.http, &provider.issuer).await?;
        infra::oidc::finish(
            &auth.http,
            &metadata,
            ClientCredentials {
                client_id: &provider.client_id,
                client_secret: &secret,
            },
            &redirect_uri(auth, &slug),
            code,
            &pending.code_verifier,
            &pending.nonce,
        )
        .await
    }
    .await
    {
        Ok(claims) => claims,
        Err(err) => {
            tracing::warn!(provider = %provider.id, error = %err, "OIDC sign-in refused");
            return fail(ErrorCode::AuthOidcFailed);
        }
    };

    let email_value = claims.get(&provider.email_claim).and_then(|v| v.as_str());
    let verified = claims.get("email_verified").and_then(serde_json::Value::as_bool);
    let email = match matching_email(&provider.email_claim, email_value, verified) {
        Ok(email) => email,
        Err(EmailRefusal::Unverified) => return fail(ErrorCode::AuthOidcEmailUnverified),
        Err(EmailRefusal::Missing) => return fail(ErrorCode::AuthOidcNoMember),
    };
    // Externals use the request portal; suspended members are out.
    let Some(member) = infra::accounts::find_by_email(&mut tx, &email)
        .await?
        .map(|found| found.member)
        .filter(|m| m.active && m.account_type == AccountType::Internal)
    else {
        tracing::info!(provider = %provider.id, "OIDC sign-in of an email that is no member");
        return fail(ErrorCode::AuthOidcNoMember);
    };
    // Two-step verification is the provider's business: the session is complete.
    let proof = infra::sessions::Proof {
        second_factor: false,
        sso: true,
    };
    let opened = auth
        .sessions
        .create_proven(
            member.account_id,
            tx.tenant_id(),
            infra::sessions::SessionStage::Complete,
            proof,
        )
        .await?;
    tx.commit().await?;
    tracing::info!(account = %member.account_id, provider = %provider.id, "signed in through OIDC");
    let target = format!(
        "{}{}",
        auth.public_url().for_tenant(&slug),
        pending.return_url.as_deref().unwrap_or("/")
    );
    Ok((
        AppendHeaders(auth.session_cookies(&opened.token, &opened.session)?),
        Redirect::to(&target),
    )
        .into_response())
}
