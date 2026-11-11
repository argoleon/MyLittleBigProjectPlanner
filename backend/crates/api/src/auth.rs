//! Local authentication of the web front end (specification §14): sign-in
//! with email and password, sessions in Valkey, CSRF protection, password
//! reset by email.
//!
//! - The session token travels in the `mlbpp_session` cookie: `HttpOnly`,
//!   `SameSite=Lax`, `Secure` (unless `SESSION_COOKIE_SECURE=false` in
//!   development), without `Domain`, so the browser sends it to the tenant's
//!   own host only. A session also belongs to one tenant: its cookie is
//!   refused on any other.
//! - Every state-changing request also carries the session's CSRF token in
//!   the `X-CSRF-Token` header. The front end reads it from the `mlbpp_csrf`
//!   cookie (readable by scripts of the same origin only) or from
//!   `GET /auth/session`.
//! - Failed sign-ins are limited per email and tenant, and per client
//!   address. Answers never reveal whether an email has an account.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, FromRequestParts, State};
use axum::http::header::SET_COOKIE;
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{AppendHeaders, IntoResponse, Response};
use chrono::{DateTime, Utc};
use domain::account::{
    MAX_FAILED_SIGN_INS_PER_EMAIL, MAX_PASSWORD_RESETS_PER_EMAIL, MAX_SIGN_INS_PER_IP,
    PASSWORD_RESET_LIFETIME, SIGN_IN_WINDOW, normalize_email, validate_password,
};
use domain::mfa::{Code, MAX_FAILED_CODES, parse_code, stage_after_password, time_step};
use domain::validation::FieldError;
use infra::accounts::{self, AccountType, Member, TenantRole};
use infra::config::{PublicUrl, SessionSettings};
use infra::credentials;
use infra::crypto::SecretCipher;
use infra::email::{EMAIL_JOB, EmailJob, EmailMessage};
use infra::http_client::{HttpClient, HttpError};
use infra::oidc::LoginStore;
use infra::rate_limit::RateLimiter;
use infra::sessions::{Session, SessionLifetime, SessionStage, SessionStore};
use infra::tenant_switch::SwitchStore;
use infra::uuid::Uuid;
use infra::valkey::Valkey;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::extract::{ApiJson, header_str};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::tenant::{AppState, PublicTenantTx, StagedTenantTx};

/// OpenAPI tag of the authentication endpoints.
pub const TAG: &str = "auth";

/// Cookie holding the session token.
pub const SESSION_COOKIE: &str = "mlbpp_session";

/// Cookie holding the CSRF token, for the front end to copy into
/// [`CSRF_HEADER`].
pub const CSRF_COOKIE: &str = "mlbpp_csrf";

/// Header carrying the CSRF token on state-changing requests.
pub const CSRF_HEADER: &str = "x-csrf-token";

/// Sessions, rate limits and the settings of authentication.
#[derive(Debug, Clone)]
pub struct Auth {
    pub(crate) sessions: SessionStore,
    pub(crate) limits: RateLimiter,
    public_url: PublicUrl,
    cookie_secure: bool,
    /// Encrypts the secrets stored in the database (TOTP, OIDC clients).
    pub(crate) cipher: SecretCipher,
    /// Calls the OIDC providers.
    pub(crate) http: HttpClient,
    /// OIDC sign-ins waiting for the provider's answer.
    pub(crate) logins: LoginStore,
    /// Plain-HTTP issuers are accepted (development and tests).
    pub(crate) oidc_allow_http: bool,
    /// Switches to another tenant, waiting for the browser there.
    pub(crate) switches: SwitchStore,
}

impl Auth {
    /// `key_prefix` starts every Valkey key (`mlbpp:` in production; tests
    /// use a unique one). `oidc_allow_http`: accept plain-HTTP OIDC issuers
    /// (development and tests only).
    pub fn new(
        valkey: Valkey,
        key_prefix: &str,
        public_url: PublicUrl,
        settings: SessionSettings,
        cipher: SecretCipher,
        oidc_allow_http: bool,
    ) -> Result<Self, HttpError> {
        let lifetime = SessionLifetime {
            idle: settings.idle,
            max_age: settings.max_age,
        };
        Ok(Self {
            sessions: SessionStore::new(valkey.clone(), key_prefix, lifetime),
            limits: RateLimiter::new(valkey.clone(), key_prefix),
            logins: LoginStore::new(valkey.clone(), key_prefix),
            switches: SwitchStore::new(valkey, key_prefix),
            http: HttpClient::new(oidc_allow_http)?,
            public_url,
            cookie_secure: settings.cookie_secure,
            cipher,
            oidc_allow_http,
        })
    }

    /// Public address of the front end (links of emails, tenant addresses).
    pub(crate) fn public_url(&self) -> &PublicUrl {
        &self.public_url
    }

    /// The session of the request's cookie, for `tenant_id`: 401 without a
    /// valid one, 403 when a state-changing request lacks its CSRF token.
    pub(crate) async fn session_of(
        &self,
        parts: &Parts,
        tenant_id: Uuid,
    ) -> Result<(String, Session), Problem> {
        let token = cookie(&parts.headers, SESSION_COOKIE).ok_or(Problem::new(ErrorCode::Unauthenticated))?;
        let session = self
            .sessions
            .get(token)
            .await?
            .filter(|session| session.tenant_id == tenant_id)
            .ok_or(Problem::new(ErrorCode::Unauthenticated))?;
        if !parts.method.is_safe() {
            let sent = header_str(&parts.headers, CSRF_HEADER).unwrap_or_default();
            if !credentials::constant_time_eq(sent, &session.csrf_token) {
                return Err(Problem::new(ErrorCode::CsrfInvalid));
            }
        }
        Ok((token.to_owned(), session))
    }

    fn cookie_attributes(&self, max_age_secs: u64) -> String {
        let secure = if self.cookie_secure { "; Secure" } else { "" };
        format!("Path=/; Max-Age={max_age_secs}; SameSite=Lax{secure}")
    }

    /// `Set-Cookie` headers opening a session.
    pub(crate) fn session_cookies(
        &self,
        token: &str,
        session: &Session,
    ) -> Result<[(axum::http::HeaderName, HeaderValue); 2], Problem> {
        // The session was just opened: it lasts the whole maximum age.
        let lifetime = self.sessions.lifetime_of(session.stage);
        let attributes = self.cookie_attributes(lifetime.max_age.as_secs());
        Ok([
            (
                SET_COOKIE,
                header_value(&format!("{SESSION_COOKIE}={token}; {attributes}; HttpOnly"))?,
            ),
            (
                SET_COOKIE,
                header_value(&format!("{CSRF_COOKIE}={}; {attributes}", session.csrf_token))?,
            ),
        ])
    }

    /// `Set-Cookie` headers removing the session's cookies.
    fn cleared_cookies(&self) -> Result<[(axum::http::HeaderName, HeaderValue); 2], Problem> {
        let attributes = self.cookie_attributes(0);
        Ok([
            (
                SET_COOKIE,
                header_value(&format!("{SESSION_COOKIE}=; {attributes}; HttpOnly"))?,
            ),
            (
                SET_COOKIE,
                header_value(&format!("{CSRF_COOKIE}=; {attributes}"))?,
            ),
        ])
    }

    /// 429 with the time left in the window of `key`.
    pub(crate) async fn too_many(&self, key: &str) -> Problem {
        let retry = self.limits.retry_after(key).await.unwrap_or(SIGN_IN_WINDOW);
        Problem::new(ErrorCode::TooManyRequests).with_retry_after(retry)
    }
}

fn header_value(value: &str) -> Result<HeaderValue, Problem> {
    HeaderValue::from_str(value).map_err(|e| Problem::internal(&e))
}

/// Value of the cookie `name` in the request's `Cookie` headers.
pub fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value)
        .filter(|value| !value.is_empty())
}

/// Whether `url` is a path of the application, safe to send the browser to
/// after sign-in (never another site: `//evil.example`, `/\\evil.example`).
pub fn isolated_return_url(url: &str) -> bool {
    url.starts_with('/') && !url.starts_with("//") && !url.starts_with("/\\") && !url.contains(['\r', '\n'])
}

/// Address of the client, for rate limits: the last `X-Forwarded-For` entry
/// (added by the reverse proxy in front of the API), else the peer address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientIp(pub String);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(client_ip(parts)))
    }
}

fn client_ip(parts: &Parts) -> String {
    let forwarded = parts
        .headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .rfind(|entry| !entry.is_empty());
    if let Some(ip) = forwarded {
        return ip.to_owned();
    }
    parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map_or_else(|| "unknown".to_owned(), |info| info.0.ip().to_string())
}

// --- Bodies ---------------------------------------------------------------------

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SignInRequest {
    #[schema(example = "lucas.bernard@acme.example")]
    email: String,
    #[schema(format = Password)]
    password: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PasswordResetRequest {
    #[schema(example = "lucas.bernard@acme.example")]
    email: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PasswordResetConfirmation {
    /// The token of the emailed link.
    token: String,
    /// The new password: 12 to 128 characters, different from the email.
    #[schema(format = Password)]
    password: String,
}

/// Role of a member in its tenant.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = TenantRole)]
pub enum TenantRoleView {
    Member,
    ConfigurationAdmin,
    TenantAdmin,
}

impl From<TenantRole> for TenantRoleView {
    fn from(role: TenantRole) -> Self {
        match role {
            TenantRole::Member => Self::Member,
            TenantRole::ConfigurationAdmin => Self::ConfigurationAdmin,
            TenantRole::TenantAdmin => Self::TenantAdmin,
        }
    }
}

/// The signed-in member.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = SessionAccount)]
pub struct AccountView {
    id: Uuid,
    #[schema(example = "lucas.bernard@acme.example")]
    email: String,
    #[schema(example = "Lucas Bernard")]
    display_name: String,
    /// Preferred language; the tenant's default when absent.
    #[schema(example = "fr")]
    locale: Option<String>,
    tenant_role: TenantRoleView,
}

/// How far sign-in has gone.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = SessionStage)]
pub enum SessionStageView {
    /// Signed in.
    Complete,
    /// The password was right; send a code with `POST /auth/sign-in/verify`.
    MfaChallenge,
    /// The password was right; the tenant requires two-step verification:
    /// enable it with `POST /account/mfa/setup` then `/confirm`.
    MfaEnrollment,
}

impl From<SessionStage> for SessionStageView {
    fn from(stage: SessionStage) -> Self {
        match stage {
            SessionStage::Complete => Self::Complete,
            SessionStage::MfaChallenge => Self::MfaChallenge,
            SessionStage::MfaEnrollment => Self::MfaEnrollment,
        }
    }
}

/// The current session.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Session)]
pub struct SessionView {
    account: AccountView,
    /// Only a `complete` session reaches the application.
    stage: SessionStageView,
    /// To send in `X-CSRF-Token` on every state-changing request.
    csrf_token: String,
    /// The session ends at this time at the latest, or earlier when unused.
    expires_at: DateTime<Utc>,
}

impl SessionView {
    pub(crate) fn new(member: &Member, session: &Session) -> Self {
        Self {
            account: AccountView {
                id: member.account_id,
                email: member.email.clone(),
                display_name: member.display_name.clone(),
                locale: member.locale.clone(),
                tenant_role: member.tenant_role.into(),
            },
            stage: session.stage.into(),
            csrf_token: session.csrf_token.clone(),
            expires_at: session.expires_at,
        }
    }
}

fn required(fields: &[(&'static str, &str)]) -> Result<(), Problem> {
    let missing: Vec<FieldError> = fields
        .iter()
        .filter(|(_, value)| value.trim().is_empty())
        .map(|(name, _)| FieldError::new(*name, "required"))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(Problem::validation(missing))
    }
}

/// Whether `password` matches `hash`, off the async runtime. Without a hash,
/// spends the same time anyway: timing reveals nothing.
async fn check_password(password: String, hash: Option<String>) -> Result<bool, Problem> {
    Ok(tokio::task::spawn_blocking(move || match hash {
        Some(hash) => credentials::verify_password(&password, &hash),
        None => {
            credentials::verify_against_nothing(&password);
            false
        }
    })
    .await?)
}

// --- Second factor ---------------------------------------------------------------

/// A code typed by the member.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = MfaCode)]
pub struct CodeBody {
    /// 6 digits from the authenticator app, or a backup code (`abcde-fghjk`).
    #[schema(example = "482913")]
    pub(crate) code: String,
}

/// Which codes [`check_code`] accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Accept {
    /// TOTP or backup code, against the enabled secret.
    Enabled,
    /// TOTP only, against the secret being enrolled.
    Enrolling,
}

/// The time step of an accepted code (TOTP), or `None` for a backup code.
pub(crate) type AcceptedStep = Option<i64>;

/// Checks the second-factor `raw` code of `account_id`: 5 failures in 15
/// minutes block it (429), each failure is 401 `auth.invalid_code`, a code
/// or backup code works once.
pub(crate) async fn check_code(
    auth: &Auth,
    conn: &mut infra::sqlx::PgConnection,
    account_id: Uuid,
    raw: &str,
    accept: Accept,
) -> Result<AcceptedStep, Problem> {
    let key = format!("mfa:account:{account_id}");
    if auth.limits.count(&key).await? >= MAX_FAILED_CODES {
        return Err(auth.too_many(&key).await);
    }
    let accepted = match (parse_code(raw), accept) {
        (Some(Code::Totp(code)), _) => totp_step(auth, conn, account_id, &code, accept).await?,
        (Some(Code::Backup(code)), Accept::Enabled) => infra::mfa::use_backup_code(conn, account_id, &code)
            .await?
            .then_some(None),
        _ => None,
    };
    match accepted {
        Some(step) => {
            auth.limits.clear(&key).await?;
            Ok(step)
        }
        None => {
            auth.limits.hit(&key, SIGN_IN_WINDOW).await?;
            Err(Problem::new(ErrorCode::AuthInvalidCode))
        }
    }
}

/// `Some(Some(step))` when `code` is the TOTP code of a step in the window,
/// not used before.
async fn totp_step(
    auth: &Auth,
    conn: &mut infra::sqlx::PgConnection,
    account_id: Uuid,
    code: &str,
    accept: Accept,
) -> Result<Option<AcceptedStep>, Problem> {
    let Some(stored) = infra::mfa::find(conn, account_id).await? else {
        return Ok(None);
    };
    if stored.enabled != (accept == Accept::Enabled) {
        return Ok(None);
    }
    let secret = auth
        .cipher
        .decrypt(
            account_id.as_bytes(),
            &stored.secret_ciphertext,
            &stored.secret_nonce,
        )
        .map_err(|e| Problem::internal(&e))?;
    let now = u64::try_from(Utc::now().timestamp()).unwrap_or(0);
    let last_used = stored.last_used_step.and_then(|s| u64::try_from(s).ok());
    let candidates = domain::mfa::candidate_steps(time_step(now), last_used);
    let Some(step) =
        infra::mfa::matching_step(&secret, code, &candidates).map_err(|e| Problem::internal(&e))?
    else {
        return Ok(None);
    };
    let step = i64::try_from(step).unwrap_or(i64::MAX);
    // An enabled secret records the step now; an enrolling one when enabled.
    if accept == Accept::Enabled && !infra::mfa::record_step(conn, account_id, step).await? {
        return Ok(None);
    }
    Ok(Some(Some(step)))
}

/// Replaces the request's session with a complete one: a new token, so the
/// one that waited for the second factor is worthless.
pub(crate) async fn complete_session(auth: &Auth, tx: &StagedTenantTx) -> Result<(String, Session), Problem> {
    auth.sessions.delete(tx.session_token()).await?;
    // The code proved the second factor.
    let proof = infra::sessions::Proof {
        second_factor: true,
        sso: tx.session().sso,
    };
    let opened = auth
        .sessions
        .create_proven(tx.account_id(), tx.tenant_id(), SessionStage::Complete, proof)
        .await?;
    Ok((opened.token, opened.session))
}

// --- Endpoints ------------------------------------------------------------------

/// Signs in with email and password.
#[utoipa::path(
    post,
    path = "/auth/sign-in",
    tag = TAG,
    operation_id = "signIn",
    summary = "Sign in with email and password",
    description = "Opens a session in the tenant of the request's host: sets the `mlbpp_session` \
                   (HttpOnly) and `mlbpp_csrf` cookies. After 5 failures on one email, or many from \
                   one address, sign-in is refused for 15 minutes (429). The error never tells \
                   whether the email has an account.\n\n\
                   When two-step verification is on, or required by the tenant, the session's \
                   `stage` is `mfa_challenge` or `mfa_enrollment`: it reaches nothing but the next \
                   step, for 5 or 15 minutes.",
    security(()),
    request_body = SignInRequest,
    responses(
        (status = 200, description = "Signed in", body = SessionView, headers(
            ("Set-Cookie" = String, description = "`mlbpp_session` and `mlbpp_csrf`"),
        )),
        (status = 400, description = "Missing email or password", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`auth.invalid_credentials`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`auth.sso_required`: the organization signs this member in through its identity provider", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "`tenant.not_found`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 429, description = "`too_many_requests`, with `Retry-After`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn sign_in(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    mut tx: PublicTenantTx,
    ApiJson(body): ApiJson<SignInRequest>,
) -> Result<Response, Problem> {
    required(&[("email", &body.email), ("password", &body.password)])?;
    let auth = &state.auth;
    let ip_key = format!("sign-in:ip:{ip}");
    let email = normalize_email(&body.email);
    let email_key = format!(
        "sign-in:email:{}:{}",
        tx.tenant_id(),
        email.as_deref().unwrap_or(body.email.trim())
    );
    if auth.limits.count(&ip_key).await? >= MAX_SIGN_INS_PER_IP {
        return Err(auth.too_many(&ip_key).await);
    }
    if auth.limits.count(&email_key).await? >= MAX_FAILED_SIGN_INS_PER_EMAIL {
        return Err(auth.too_many(&email_key).await);
    }

    let candidate = match &email {
        Some(email) => accounts::find_by_email(&mut tx, email).await?,
        None => None,
    };
    let (member, hash) = match candidate {
        Some(found) => (Some(found.member), found.password_hash),
        None => (None, None),
    };
    let matches = check_password(body.password, hash).await?;
    // Suspended and external members get the same answer as a wrong password.
    let member = member.filter(|m| matches && m.active && m.account_type == AccountType::Internal);
    let Some(member) = member else {
        auth.limits.hit(&ip_key, SIGN_IN_WINDOW).await?;
        auth.limits.hit(&email_key, SIGN_IN_WINDOW).await?;
        return Err(Problem::new(ErrorCode::AuthInvalidCredentials));
    };

    auth.limits.clear(&email_key).await?;
    // A new token on every sign-in: a token planted before cannot be reused.
    if let Some(previous) = cookie(&headers, SESSION_COOKIE) {
        auth.sessions.delete(previous).await?;
    }
    // Checked after the password: only the member learns that SSO is required.
    let policy = infra::auth_policy::get(&mut tx).await?;
    let who = domain::auth_policy::PasswordSignIn {
        internal: member.account_type == AccountType::Internal,
        local_account: member.local_account,
        tenant_has_sso: infra::oidc_providers::has_enabled(&mut tx).await?,
    };
    if !domain::auth_policy::password_allowed(policy.rules(), who) {
        return Err(Problem::new(ErrorCode::AuthSsoRequired));
    }
    // Two-step verification: required by the account, or by the tenant's policy.
    let mfa_enabled = infra::mfa::is_enabled(&mut tx, member.account_id).await?;
    let stage = stage_after_password(mfa_enabled, policy.settings.local_accounts_require_mfa);
    let opened = auth
        .sessions
        .create_staged(member.account_id, tx.tenant_id(), stage)
        .await?;
    tx.commit().await?;
    tracing::info!(account = %member.account_id, stage = stage.as_str(), "password accepted");
    Ok((
        AppendHeaders(auth.session_cookies(&opened.token, &opened.session)?),
        ApiJson(SessionView::new(&member, &opened.session)),
    )
        .into_response())
}

/// Ends the current session.
#[utoipa::path(
    post,
    path = "/auth/sign-out",
    tag = TAG,
    operation_id = "signOut",
    summary = "Sign out",
    description = "Ends the session: its cookie no longer works, even if replayed.",
    responses(
        (status = 204, description = "Signed out; the cookies are cleared"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn sign_out(State(state): State<AppState>, tx: StagedTenantTx) -> Result<Response, Problem> {
    state.auth.sessions.delete(tx.session_token()).await?;
    tracing::info!(account = %tx.account_id(), "signed out");
    Ok((
        StatusCode::NO_CONTENT,
        AppendHeaders(state.auth.cleared_cookies()?),
    )
        .into_response())
}

/// The current session and member.
#[utoipa::path(
    get,
    path = "/auth/session",
    tag = TAG,
    operation_id = "getSession",
    summary = "The current session",
    description = "The signed-in member, the CSRF token and the session's `stage` (a session \
                   waiting for two-step verification included), or 401.",
    responses(
        (status = 200, description = "Signed in", body = SessionView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn current_session(tx: StagedTenantTx) -> ApiJson<SessionView> {
    ApiJson(SessionView::new(tx.member(), tx.session()))
}

/// Sends a password reset link.
#[utoipa::path(
    post,
    path = "/auth/password-reset",
    tag = TAG,
    operation_id = "requestPasswordReset",
    summary = "Email a password reset link",
    description = "Always 202, whether the email has an account or not. A member of the tenant \
                   receives a link valid for one hour.",
    security(()),
    request_body = PasswordResetRequest,
    responses(
        (status = 202, description = "Accepted: an email is sent if the account exists"),
        (status = 400, description = "Missing email", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "`tenant.not_found`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 429, description = "`too_many_requests`, with `Retry-After`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn request_password_reset(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    mut tx: PublicTenantTx,
    ApiJson(body): ApiJson<PasswordResetRequest>,
) -> Result<StatusCode, Problem> {
    required(&[("email", &body.email)])?;
    let auth = &state.auth;
    let ip_key = format!("password-reset:ip:{ip}");
    if auth.limits.hit(&ip_key, SIGN_IN_WINDOW).await? > MAX_SIGN_INS_PER_IP {
        return Err(auth.too_many(&ip_key).await);
    }
    let Some(email) = normalize_email(&body.email) else {
        return Ok(StatusCode::ACCEPTED);
    };
    let email_key = format!("password-reset:email:{}:{email}", tx.tenant_id());
    if auth.limits.hit(&email_key, SIGN_IN_WINDOW).await? > MAX_PASSWORD_RESETS_PER_EMAIL {
        // Enough emails already: accepted, nothing sent.
        return Ok(StatusCode::ACCEPTED);
    }
    let member = accounts::find_by_email(&mut tx, &email)
        .await?
        .map(|found| found.member)
        .filter(|m| m.active && m.account_type == AccountType::Internal);
    let Some(member) = member else {
        return Ok(StatusCode::ACCEPTED);
    };

    let token = credentials::random_token();
    let lifetime = chrono::Duration::from_std(PASSWORD_RESET_LIFETIME).unwrap_or(chrono::Duration::hours(1));
    accounts::create_password_reset(
        &mut tx,
        member.account_id,
        &credentials::token_hash(&token),
        Utc::now() + lifetime,
    )
    .await?;
    let tenant_name = sqlx::query_scalar!("SELECT name FROM tenant")
        .fetch_one(&mut *tx)
        .await?;
    let email_job = EmailJob {
        to: member.email.clone(),
        locale: member
            .locale
            .clone()
            .unwrap_or_else(|| tx.default_locale().to_owned()),
        message: EmailMessage::PasswordReset {
            display_name: member.display_name.clone(),
            tenant_name,
            link: format!(
                "{}/reset-password?token={token}",
                auth.public_url.for_tenant(tx.slug())
            ),
            valid_minutes: PASSWORD_RESET_LIFETIME.as_secs() / 60,
        },
    };
    let payload = serde_json::to_value(&email_job).map_err(|e| Problem::internal(&e))?;
    infra::jobs::enqueue(&mut tx, EMAIL_JOB, &payload).await?;
    tx.commit().await?;
    tracing::info!(account = %member.account_id, "password reset requested");
    Ok(StatusCode::ACCEPTED)
}

/// Chooses a new password with a reset link.
#[utoipa::path(
    post,
    path = "/auth/password-reset/confirm",
    tag = TAG,
    operation_id = "confirmPasswordReset",
    summary = "Choose a new password with a reset link",
    description = "Sets the new password and ends every session of the account. The link works once.",
    security(()),
    request_body = PasswordResetConfirmation,
    responses(
        (status = 204, description = "Password changed; sign in again"),
        (status = 400, description = "`auth.reset_token_invalid`, or the password is too short, too long \
                                      or equal to the email (`validation_failed`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "`tenant.not_found`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn confirm_password_reset(
    State(state): State<AppState>,
    mut tx: PublicTenantTx,
    ApiJson(body): ApiJson<PasswordResetConfirmation>,
) -> Result<StatusCode, Problem> {
    let invalid = || Problem::new(ErrorCode::AuthResetTokenInvalid);
    let account_id = accounts::consume_password_reset(&mut tx, &credentials::token_hash(&body.token))
        .await?
        .ok_or_else(invalid)?;
    let member = accounts::find(&mut tx, account_id)
        .await?
        .filter(|m| m.active)
        .ok_or_else(invalid)?;
    // On error the transaction rolls back: the link still works.
    validate_password("password", &body.password, &member.email).map_err(Problem::validation)?;

    let password = body.password;
    let hash = tokio::task::spawn_blocking(move || credentials::hash_password(&password))
        .await?
        .map_err(|e| Problem::internal(&e))?;
    accounts::set_password(&mut tx, account_id, &hash).await?;
    accounts::revoke_password_resets(&mut tx, account_id).await?;
    // Before the commit: if it fails, the member is merely signed out.
    let ended = state.auth.sessions.delete_all(account_id).await?;
    let failures_key = format!("sign-in:email:{}:{}", tx.tenant_id(), member.email);
    tx.commit().await?;
    // The new password works at once, even after failed sign-ins.
    state.auth.limits.clear(&failures_key).await?;
    tracing::info!(account = %account_id, sessions_ended = ended, "password reset");
    Ok(StatusCode::NO_CONTENT)
}

/// Completes sign-in with the second factor.
#[utoipa::path(
    post,
    path = "/auth/sign-in/verify",
    tag = TAG,
    operation_id = "verifySignIn",
    summary = "Complete sign-in with a two-step verification code",
    description = "For a session at the `mfa_challenge` stage: a code from the authenticator app, \
                   or a backup code, which then cannot be used again. Opens a complete session \
                   (new cookies). After 5 wrong codes, the account is blocked 15 minutes (429).",
    request_body = CodeBody,
    responses(
        (status = 200, description = "Signed in", body = SessionView, headers(
            ("Set-Cookie" = String, description = "`mlbpp_session` and `mlbpp_csrf`"),
        )),
        (status = 401, description = "`auth.invalid_code`, or `unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`conflict`: the session is not waiting for a code", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 429, description = "`too_many_requests`, with `Retry-After`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn verify_sign_in(
    State(state): State<AppState>,
    mut tx: StagedTenantTx,
    ApiJson(body): ApiJson<CodeBody>,
) -> Result<Response, Problem> {
    if tx.stage() != SessionStage::MfaChallenge {
        return Err(Problem::new(ErrorCode::Conflict));
    }
    let account = tx.account_id();
    let auth = &state.auth;
    check_code(auth, &mut tx, account, &body.code, Accept::Enabled).await?;
    let (token, session) = complete_session(auth, &tx).await?;
    let member = tx.member().clone();
    tx.0.commit().await?;
    tracing::info!(account = %account, "signed in with two-step verification");
    Ok((
        AppendHeaders(auth.session_cookies(&token, &session)?),
        ApiJson(SessionView::new(&member, &session)),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    fn parts(headers: &[(&str, &str)]) -> Parts {
        let mut request = Request::builder().uri("/");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.body(()).unwrap().into_parts().0
    }

    #[test]
    fn cookies_are_read_from_every_cookie_header() {
        let parts = parts(&[
            ("cookie", "theme=dark; mlbpp_session=abc"),
            ("cookie", "mlbpp_csrf=xyz"),
        ]);
        assert_eq!(cookie(&parts.headers, SESSION_COOKIE), Some("abc"));
        assert_eq!(cookie(&parts.headers, CSRF_COOKIE), Some("xyz"));
        assert_eq!(cookie(&parts.headers, "missing"), None);
        let empty = self::parts(&[("cookie", "mlbpp_session=")]);
        assert_eq!(cookie(&empty.headers, SESSION_COOKIE), None);
        let prefixed = self::parts(&[("cookie", "xmlbpp_session=abc")]);
        assert_eq!(cookie(&prefixed.headers, SESSION_COOKIE), None);
    }

    #[test]
    fn the_client_is_the_last_forwarded_address_or_the_peer() {
        let forwarded = parts(&[("x-forwarded-for", "203.0.113.9, 198.51.100.7")]);
        assert_eq!(client_ip(&forwarded), "198.51.100.7", "added by our proxy");
        let mut direct = parts(&[]);
        direct
            .extensions
            .insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 1], 41000))));
        assert_eq!(client_ip(&direct), "192.0.2.1");
        assert_eq!(client_ip(&parts(&[])), "unknown");
    }

    #[test]
    fn required_fields_must_not_be_blank() {
        assert!(required(&[("email", "a@b.io")]).is_ok());
        let err = required(&[("email", " "), ("password", "")]).unwrap_err();
        assert_eq!(err.code, ErrorCode::ValidationFailed);
        let fields: Vec<_> = err.errors.iter().map(|e| e.field.as_str()).collect();
        assert_eq!(fields, ["email", "password"]);
    }
}
