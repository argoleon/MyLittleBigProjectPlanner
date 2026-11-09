//! Tenant-scoped transaction, the only way handlers reach the database.
//!
//! A handler that takes a [`TenantTx`] argument gets a transaction in which
//! the tenant of the request is already set for row-level security. There is
//! no other database handle in [`AppState`]'s public surface used by
//! handlers, so the tenant context cannot be forgotten.
//!
//! ```ignore
//! async fn handler(mut tx: TenantTx) -> Result<Json<Thing>, Problem> {
//!     let thing = sqlx::query_as!(Thing, "SELECT … FROM thing").fetch_one(&mut *tx).await?;
//!     tx.commit().await?;
//!     Ok(Json(thing))
//! }
//! ```
//! Dropping the `TenantTx` without `commit` rolls back.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::header::HOST;
use axum::http::request::Parts;
use domain::tenancy::{Tenancy, tenant_slug_from_host};
use infra::health::Dependency;
use infra::sqlx::postgres::{PgConnection, PgPool};
use infra::sqlx::{Postgres, Transaction};
use infra::uuid::Uuid;

use crate::problem::{ErrorCode, Problem};

/// Shared state of the API.
#[derive(Debug, Clone)]
pub struct AppState {
    pub(crate) pool: PgPool,
    pub(crate) tenancy: Arc<Tenancy>,
    /// Checked by `/health/ready`: PostgreSQL, plus the shared services.
    pub(crate) dependencies: Arc<[Dependency]>,
}

impl AppState {
    pub fn new(pool: PgPool, tenancy: Tenancy) -> Self {
        let dependencies = Arc::from([Dependency::Postgres(pool.clone())]);
        Self {
            pool,
            tenancy: Arc::new(tenancy),
            dependencies,
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

/// A database transaction bound to the request's tenant.
#[derive(Debug)]
pub struct TenantTx {
    tenant_id: Uuid,
    default_locale: String,
    tx: Transaction<'static, Postgres>,
}

impl TenantTx {
    pub fn tenant_id(&self) -> Uuid {
        self.tenant_id
    }

    /// The tenant's default language, fallback of translated labels.
    pub fn default_locale(&self) -> &str {
        &self.default_locale
    }

    pub async fn commit(self) -> Result<(), Problem> {
        self.tx.commit().await.map_err(Problem::from)
    }
}

impl Deref for TenantTx {
    type Target = PgConnection;
    fn deref(&self) -> &PgConnection {
        &self.tx
    }
}

impl DerefMut for TenantTx {
    fn deref_mut(&mut self) -> &mut PgConnection {
        &mut self.tx
    }
}

impl FromRequestParts<AppState> for TenantTx {
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
            default_locale: tenant.default_locale,
            tx,
        })
    }
}
