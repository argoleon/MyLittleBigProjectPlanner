//! Tenant-scoped transactions, the only way handlers reach the database.
//!
//! A handler that takes a [`TenantTx`] argument gets a transaction in which
//! the tenant of the request is already set for row-level security, on
//! behalf of a signed-in member of that tenant: the extractor answers 401
//! without a valid session, and 403 when a state-changing request lacks the
//! session's CSRF token (see [`crate::auth`]). [`PublicTenantTx`] is the same
//! without a session, for the few endpoints used before signing in. There is
//! no other database handle in [`AppState`]'s public surface used by
//! handlers, so neither the tenant context nor authentication can be
//! forgotten.
//!
//! ```ignore
//! async fn handler(mut tx: TenantTx) -> Result<Json<Thing>, Problem> {
//!     let thing = sqlx::query_as!(Thing, "SELECT … FROM thing").fetch_one(&mut *tx).await?;
//!     tx.commit().await?;
//!     Ok(Json(thing))
//! }
//! ```
//! Dropping the transaction without `commit` rolls back.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::header::HOST;
use axum::http::request::Parts;
use domain::tenancy::{Tenancy, tenant_slug_from_host};
use infra::accounts::{AccountType, Member};
use infra::health::Dependency;
use infra::sessions::{Session, SessionStage};
use infra::sqlx::postgres::{PgConnection, PgPool};
use infra::sqlx::{Postgres, Transaction};
use infra::uuid::Uuid;

use crate::auth::Auth;
use crate::problem::{ErrorCode, Problem};

/// Shared state of the API.
#[derive(Debug, Clone)]
pub struct AppState {
    pub(crate) pool: PgPool,
    pub(crate) tenancy: Arc<Tenancy>,
    /// Checked by `/health/ready`: PostgreSQL, plus the shared services.
    pub(crate) dependencies: Arc<[Dependency]>,
    pub(crate) auth: Arc<Auth>,
}

impl AppState {
    pub fn new(pool: PgPool, tenancy: Tenancy, auth: Auth) -> Self {
        let dependencies = Arc::from([Dependency::Postgres(pool.clone())]);
        Self {
            pool,
            tenancy: Arc::new(tenancy),
            dependencies,
            auth: Arc::new(auth),
        }
    }

    /// Adds dependencies to the readiness checks (Valkey, S3…).
    #[must_use]
    pub fn with_dependencies(mut self, more: impl IntoIterator<Item = Dependency>) -> Self {
        let mut all = self.dependencies.to_vec();
        all.extend(more);
        self.dependencies = all.into();
        self
    }
}

/// A database transaction bound to the request's tenant, without
/// authentication: only for the endpoints used before signing in.
#[derive(Debug)]
pub struct PublicTenantTx {
    tenant_id: Uuid,
    slug: String,
    default_locale: String,
    tx: Transaction<'static, Postgres>,
}

impl PublicTenantTx {
    pub fn tenant_id(&self) -> Uuid {
        self.tenant_id
    }

    /// The tenant's identifier in host names and links.
    pub fn slug(&self) -> &str {
        &self.slug
    }

    /// The tenant's default language, fallback of translated labels.
    pub fn default_locale(&self) -> &str {
        &self.default_locale
    }

    pub async fn commit(self) -> Result<(), Problem> {
        self.tx.commit().await.map_err(Problem::from)
    }
}

impl Deref for PublicTenantTx {
    type Target = PgConnection;
    fn deref(&self) -> &PgConnection {
        &self.tx
    }
}

impl DerefMut for PublicTenantTx {
    fn deref_mut(&mut self) -> &mut PgConnection {
        &mut self.tx
    }
}

impl FromRequestParts<AppState> for PublicTenantTx {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Problem> {
        let host = parts
            .headers
            .get(HOST)
            .and_then(|v| v.to_str().ok())
            .or_else(|| parts.uri.authority().map(axum::http::uri::Authority::as_str));
        let slug = tenant_slug_from_host(&state.tenancy, host)
            .map_err(|_| Problem::new(ErrorCode::TenantNotFound))?;

        let mut tx = state.pool.begin().await?;
        let tenant = infra::db::resolve_tenant(&mut tx, &slug)
            .await?
            .ok_or(Problem::new(ErrorCode::TenantNotFound))?;
        infra::db::set_tenant(&mut tx, tenant.id).await?;
        tracing::Span::current().record("tenant", tracing::field::display(&slug));
        Ok(Self {
            tenant_id: tenant.id,
            slug,
            default_locale: tenant.default_locale,
            tx,
        })
    }
}

/// A database transaction bound to the request's tenant, on behalf of a
/// signed-in, active internal member of that tenant whose session is
/// complete (two-step verification done, when required).
#[derive(Debug)]
pub struct TenantTx {
    inner: PublicTenantTx,
    session_token: String,
    session: Session,
    member: Member,
}

impl TenantTx {
    pub fn tenant_id(&self) -> Uuid {
        self.inner.tenant_id
    }

    /// The tenant's default language, fallback of translated labels.
    pub fn default_locale(&self) -> &str {
        &self.inner.default_locale
    }

    /// The signed-in member.
    pub fn member(&self) -> &Member {
        &self.member
    }

    pub fn account_id(&self) -> Uuid {
        self.member.account_id
    }

    pub(crate) fn session(&self) -> &Session {
        &self.session
    }

    pub(crate) fn session_token(&self) -> &str {
        &self.session_token
    }

    /// Where the session stands: always `Complete` for a [`TenantTx`]
    /// itself, any stage for a [`StagedTenantTx`].
    pub fn stage(&self) -> SessionStage {
        self.session.stage
    }

    pub async fn commit(self) -> Result<(), Problem> {
        self.inner.commit().await
    }

    /// The tenant, the session of the request's cookie at any stage, and its
    /// active internal member.
    async fn load(parts: &mut Parts, state: &AppState) -> Result<Self, Problem> {
        // The tenant first: an unknown host is a 404 whoever asks.
        let mut inner = PublicTenantTx::from_request_parts(parts, state).await?;
        let (session_token, session) = state.auth.session_of(parts, inner.tenant_id).await?;
        let member = infra::accounts::find(&mut inner, session.account_id)
            .await?
            .filter(|member| member.active)
            // Suspended or removed since sign-in.
            .ok_or(Problem::new(ErrorCode::Unauthenticated))?;
        if member.account_type != AccountType::Internal {
            // External members use the request portal only.
            return Err(Problem::new(ErrorCode::Forbidden));
        }
        Ok(Self {
            inner,
            session_token,
            session,
            member,
        })
    }
}

impl Deref for TenantTx {
    type Target = PgConnection;
    fn deref(&self) -> &PgConnection {
        &self.inner
    }
}

impl DerefMut for TenantTx {
    fn deref_mut(&mut self) -> &mut PgConnection {
        &mut self.inner
    }
}

impl FromRequestParts<AppState> for TenantTx {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Problem> {
        let tx = Self::load(parts, state).await?;
        if tx.session.stage == SessionStage::Complete {
            Ok(tx)
        } else {
            // Password checked, second factor still expected: not signed in yet.
            Err(Problem::new(ErrorCode::Unauthenticated))
        }
    }
}

/// A [`TenantTx`] whose session may still wait for two-step verification:
/// only for the endpoints of that step (session, sign-out, code check,
/// enrollment). Each checks [`TenantTx::stage`].
#[derive(Debug)]
pub struct StagedTenantTx(pub TenantTx);

impl FromRequestParts<AppState> for StagedTenantTx {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Problem> {
        TenantTx::load(parts, state).await.map(Self)
    }
}

impl Deref for StagedTenantTx {
    type Target = TenantTx;
    fn deref(&self) -> &TenantTx {
        &self.0
    }
}

impl DerefMut for StagedTenantTx {
    fn deref_mut(&mut self) -> &mut TenantTx {
        &mut self.0
    }
}
