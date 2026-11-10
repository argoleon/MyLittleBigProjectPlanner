//! PostgreSQL access: connection pool, migrations, tenant-scoped transactions.
//!
//! Business code never opens a bare transaction: it gets one from
//! [`begin_tenant_tx`] (through the API request layer), so row-level security
//! always knows the tenant. See `backend/migrations/README.md`.

use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnection, PgPool, PgPoolOptions};
use sqlx::{Connection, Postgres, Transaction};
use uuid::Uuid;

use crate::config::{DatabaseConfig, Secret};

/// Every migration in `backend/migrations`, embedded at compile time.
pub static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");

/// Tables allowed to have no `tenant_id` column (they still need RLS,
/// except the SQLx bookkeeping table).
const TENANT_ID_EXEMPT: &[&str] = &["tenant", "account", "platform_admin"];

/// Pool for the runtime role. Connections open lazily, so the service starts
/// even if the database is momentarily unavailable; readiness reports it.
pub fn connect_lazy(config: &DatabaseConfig) -> Result<PgPool, sqlx::Error> {
    PgPoolOptions::new()
        .max_connections(config.max_connections)
        .acquire_timeout(config.acquire_timeout)
        .connect_lazy(config.url.expose())
}

/// Applies pending migrations with the owner role. Already applied
/// migrations are skipped; a modified applied migration is an error.
/// Uses a single connection, not a pool: an unreachable database fails at
/// once instead of after the pool's retries.
pub async fn migrate(migration_url: &Secret) -> Result<(), sqlx::Error> {
    let mut conn = PgConnection::connect(migration_url.expose()).await?;
    let result = MIGRATOR.run(&mut conn).await;
    let _ = conn.close().await;
    result.map_err(Into::into)
}

/// Starts a transaction bound to `tenant_id`: the setting read by every
/// row-level security policy is applied before any other statement, and
/// vanishes at commit or rollback.
pub async fn begin_tenant_tx(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Transaction<'static, Postgres>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, tenant_id).await?;
    Ok(tx)
}

/// Binds the current transaction to `tenant_id` (`SET LOCAL app.tenant_id`).
pub async fn set_tenant(conn: &mut PgConnection, tenant_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "SELECT set_config('app.tenant_id', $1, true) AS tenant",
        tenant_id.to_string()
    )
    .fetch_one(conn)
    .await?;
    Ok(())
}

/// A tenant found by its slug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTenant {
    pub id: Uuid,
    /// Fallback language of translatable labels (`fr` or `en`).
    pub default_locale: String,
    pub quotas: TenantQuotas,
}

/// Quotas of a tenant, for the services that enforce them; `None` means
/// unlimited.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TenantQuotas {
    /// Total size of the tenant's attachments (L05).
    pub storage_bytes: Option<i64>,
    /// API requests per minute (API tokens, L16).
    pub api_requests_per_minute: Option<i32>,
}

/// The active tenant for `slug`, or `None` if no such tenant exists or it is
/// being deleted. Works outside a tenant context (owner-defined function),
/// without exposing other tenants.
pub async fn resolve_tenant(
    conn: &mut PgConnection,
    slug: &str,
) -> Result<Option<ResolvedTenant>, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        SELECT id AS "id!", default_locale AS "default_locale!",
               storage_quota_bytes AS "storage_quota_bytes?",
               api_rate_limit_per_minute AS "api_rate_limit_per_minute?"
        FROM app_resolve_tenant($1)
        "#,
        slug
    )
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|r| ResolvedTenant {
        id: r.id,
        default_locale: r.default_locale,
        quotas: TenantQuotas {
            storage_bytes: r.storage_quota_bytes,
            api_requests_per_minute: r.api_rate_limit_per_minute,
        },
    }))
}

/// Tables of the `public` schema that break the tenant isolation rules:
/// row-level security disabled, no policy, or (except for exempt tables)
/// RLS not forced or no `tenant_id NOT NULL` column. Must be empty.
pub async fn tables_breaking_rls_rules(pool: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    let exempt: Vec<String> = TENANT_ID_EXEMPT.iter().map(|s| (*s).to_owned()).collect();
    let tables = sqlx::query_scalar!(
        r#"
        SELECT c.relname AS "name!"
        FROM pg_class c
        JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE n.nspname = 'public'
          AND c.relkind IN ('r', 'p')
          AND c.relname <> '_sqlx_migrations'
          AND (
                NOT c.relrowsecurity
             OR NOT EXISTS (SELECT 1 FROM pg_policy p WHERE p.polrelid = c.oid)
             OR (
                    c.relname <> ALL ($1::text[])
                AND (
                        NOT c.relforcerowsecurity
                     OR NOT EXISTS (
                            SELECT 1 FROM pg_attribute a
                            WHERE a.attrelid = c.oid
                              AND a.attname = 'tenant_id'
                              AND a.attnotnull
                              AND NOT a.attisdropped
                        )
                    )
                )
          )
        ORDER BY 1
        "#,
        &exempt
    )
    .fetch_all(pool)
    .await?;
    Ok(tables)
}
