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
use infra::sqlx::postgres::{PgConnection, PgPool};
use infra::sqlx::{Postgres, Transaction};
use infra::uuid::Uuid;

use crate::problem::Problem;

/// Shared state of the API.
#[derive(Debug, Clone)]
pub struct AppState {
    pub(crate) pool: PgPool,
    pub(crate) tenancy: Arc<Tenancy>,
}

impl AppState {
    pub fn new(pool: PgPool, tenancy: Tenancy) -> Self {
        Self {
            pool,
            tenancy: Arc::new(tenancy),
        }
    }
}

/// A database transaction bound to the request's tenant.
#[derive(Debug)]
pub struct TenantTx {
    tenant_id: Uuid,
    tx: Transaction<'static, Postgres>,
}

impl TenantTx {
    pub fn tenant_id(&self) -> Uuid {
        self.tenant_id
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
        let slug = tenant_slug_from_host(&state.tenancy, host).map_err(|_| Problem::TENANT_NOT_FOUND)?;

        let mut tx = state.pool.begin().await?;
        let tenant_id = infra::db::resolve_tenant(&mut tx, &slug)
            .await?
            .ok_or(Problem::TENANT_NOT_FOUND)?;
        infra::db::set_tenant(&mut tx, tenant_id).await?;
        tracing::Span::current().record("tenant", tracing::field::display(&slug));
        Ok(Self { tenant_id, tx })
    }
}
