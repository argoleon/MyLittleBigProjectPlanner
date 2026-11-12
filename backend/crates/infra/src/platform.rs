//! Platform administration: tenants managed by the platform super admin
//! (specification §3).
//!
//! Every call goes through an owner-defined database function that checks
//! by itself that `actor` is a platform super admin
//! (`migrations/20261111000001_tenant_settings_and_platform.sql`): the
//! runtime role cannot write the tenant table otherwise.

use chrono::{DateTime, Utc};
use domain::tenant::{OverloadMode, TenantSettingsInput};
use sqlx::postgres::PgConnection;
use uuid::Uuid;

use crate::pagination::{Keyset, Page, page_from_rows};

/// Kind of the job purging a deleted tenant's data.
pub const PURGE_TENANT_JOB: &str = "tenant.purge";

/// SQLSTATE raised by the platform functions for a non super admin.
const INSUFFICIENT_PRIVILEGE: &str = "42501";
/// Unique constraint of the tenant slug.
const SLUG_UNIQUE: &str = "tenant_slug_key";

/// Lifecycle of a tenant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantStatus {
    Active,
    /// Its host name no longer resolves; its data is being purged.
    Deleting,
}

/// A tenant with all its settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tenant {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub default_locale: String,
    pub time_zone: String,
    /// Minutes per weekday, Monday first.
    pub default_capacity_minutes: Vec<i32>,
    pub overload_tolerance_percent: i16,
    pub overload_mode: OverloadMode,
    pub storage_quota_bytes: Option<i64>,
    pub api_rate_limit_per_minute: Option<i32>,
    pub status: TenantStatus,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Tenant {
    pub fn keyset(&self) -> Keyset {
        Keyset {
            created_at: self.created_at,
            id: self.id,
        }
    }
}

struct Row {
    id: Uuid,
    slug: String,
    name: String,
    default_locale: String,
    time_zone: String,
    default_capacity_minutes: Vec<i32>,
    overload_tolerance_percent: i16,
    overload_mode: String,
    storage_quota_bytes: Option<i64>,
    api_rate_limit_per_minute: Option<i32>,
    status: String,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<Row> for Tenant {
    fn from(row: Row) -> Self {
        Self {
            id: row.id,
            slug: row.slug,
            name: row.name,
            default_locale: row.default_locale,
            time_zone: row.time_zone,
            default_capacity_minutes: row.default_capacity_minutes,
            overload_tolerance_percent: row.overload_tolerance_percent,
            // The database checks these values.
            overload_mode: OverloadMode::parse(&row.overload_mode).unwrap_or(OverloadMode::Blocking),
            storage_quota_bytes: row.storage_quota_bytes,
            api_rate_limit_per_minute: row.api_rate_limit_per_minute,
            status: if row.status == "deleting" {
                TenantStatus::Deleting
            } else {
                TenantStatus::Active
            },
            version: row.version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Settings in their stored types. Built from a validated input only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantSettings {
    pub name: String,
    pub default_locale: String,
    pub time_zone: String,
    pub default_capacity_minutes: Vec<i32>,
    pub overload_tolerance_percent: i16,
    pub overload_mode: OverloadMode,
    pub storage_quota_bytes: Option<i64>,
    pub api_rate_limit_per_minute: Option<i32>,
}

impl TenantSettings {
    /// The stored form of `input`, which `domain::tenant::validate_settings`
    /// accepted (`None` otherwise).
    pub fn from_valid(input: &TenantSettingsInput) -> Option<Self> {
        Some(Self {
            name: input.name.trim().to_owned(),
            default_locale: input.default_locale.clone(),
            time_zone: input.time_zone.clone(),
            default_capacity_minutes: input
                .default_capacity_minutes
                .iter()
                .map(|m| i32::try_from(*m).ok())
                .collect::<Option<Vec<_>>>()?,
            overload_tolerance_percent: i16::try_from(input.overload_tolerance_percent).ok()?,
            overload_mode: OverloadMode::parse(&input.overload_mode)?,
            storage_quota_bytes: input.storage_quota_bytes,
            api_rate_limit_per_minute: match input.api_rate_limit_per_minute {
                Some(rate) => Some(i32::try_from(rate).ok()?),
                None => None,
            },
        })
    }
}

/// Why a platform write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformConflict {
    /// The actor is not a platform super admin.
    Forbidden,
    /// No such tenant, or it is being deleted.
    NotFound,
    /// The tenant's version is not one of the expected ones (`If-Match`).
    VersionMismatch,
    /// Another tenant already has this slug.
    SlugTaken,
}

pub type PlatformResult<T> = Result<Result<T, PlatformConflict>, sqlx::Error>;

fn classify<T>(result: Result<T, sqlx::Error>) -> PlatformResult<T> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(sqlx::Error::Database(err)) if err.code().as_deref() == Some(INSUFFICIENT_PRIVILEGE) => {
            Ok(Err(PlatformConflict::Forbidden))
        }
        Err(sqlx::Error::Database(err)) if err.constraint() == Some(SLUG_UNIQUE) => {
            Ok(Err(PlatformConflict::SlugTaken))
        }
        Err(err) => Err(err),
    }
}

/// Whether `account_id` is a platform super admin.
pub async fn is_platform_admin(conn: &mut PgConnection, account_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar!(r#"SELECT app_is_platform_admin($1) AS "admin!""#, account_id)
        .fetch_one(conn)
        .await
}

/// Whether PostgreSQL knows the time zone `name` (IANA database).
pub async fn time_zone_exists(conn: &mut PgConnection, name: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_timezone_names WHERE name = $1) AS "exists!""#,
        name
    )
    .fetch_one(conn)
    .await
}

/// A page of every tenant, in creation order.
pub async fn list(
    conn: &mut PgConnection,
    actor: Uuid,
    after: Option<Keyset>,
    limit: usize,
) -> PlatformResult<Page<Tenant>> {
    let fetch = i32::try_from(limit).unwrap_or(i32::MAX).saturating_add(1);
    let rows = sqlx::query_as!(
        Row,
        r#"
        SELECT id AS "id!", slug AS "slug!", name AS "name!", default_locale AS "default_locale!",
               time_zone AS "time_zone!", default_capacity_minutes AS "default_capacity_minutes!",
               overload_tolerance_percent AS "overload_tolerance_percent!", overload_mode AS "overload_mode!",
               storage_quota_bytes AS "storage_quota_bytes?", api_rate_limit_per_minute AS "api_rate_limit_per_minute?",
               status AS "status!", version AS "version!", created_at AS "created_at!", updated_at AS "updated_at!"
        FROM app_platform_tenants($1, $2, $3, $4)
        "#,
        actor,
        after.map(|k| k.created_at),
        after.map(|k| k.id),
        fetch
    )
    .fetch_all(conn)
    .await;
    Ok(classify(rows)?.map(|rows| {
        let items = rows.into_iter().map(Tenant::from).collect();
        page_from_rows(items, limit, Tenant::keyset)
    }))
}

/// One tenant, active or being deleted.
pub async fn get(conn: &mut PgConnection, actor: Uuid, id: Uuid) -> PlatformResult<Tenant> {
    let row = sqlx::query_as!(
        Row,
        r#"
        SELECT id AS "id!", slug AS "slug!", name AS "name!", default_locale AS "default_locale!",
               time_zone AS "time_zone!", default_capacity_minutes AS "default_capacity_minutes!",
               overload_tolerance_percent AS "overload_tolerance_percent!", overload_mode AS "overload_mode!",
               storage_quota_bytes AS "storage_quota_bytes?", api_rate_limit_per_minute AS "api_rate_limit_per_minute?",
               status AS "status!", version AS "version!", created_at AS "created_at!", updated_at AS "updated_at!"
        FROM app_platform_tenant($1, $2)
        "#,
        actor,
        id
    )
    .fetch_optional(conn)
    .await;
    Ok(classify(row)?.and_then(|row| row.map(Tenant::from).ok_or(PlatformConflict::NotFound)))
}

/// Creates the tenant `slug` with `settings`.
pub async fn create(
    conn: &mut PgConnection,
    actor: Uuid,
    id: Uuid,
    slug: &str,
    settings: &TenantSettings,
) -> PlatformResult<Tenant> {
    let row = sqlx::query_as!(
        Row,
        r#"
        SELECT id AS "id!", slug AS "slug!", name AS "name!", default_locale AS "default_locale!",
               time_zone AS "time_zone!", default_capacity_minutes AS "default_capacity_minutes!",
               overload_tolerance_percent AS "overload_tolerance_percent!", overload_mode AS "overload_mode!",
               storage_quota_bytes AS "storage_quota_bytes?", api_rate_limit_per_minute AS "api_rate_limit_per_minute?",
               status AS "status!", version AS "version!", created_at AS "created_at!", updated_at AS "updated_at!"
        FROM app_platform_create_tenant($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        "#,
        actor,
        id,
        slug,
        settings.name,
        settings.default_locale,
        settings.time_zone,
        &settings.default_capacity_minutes,
        settings.overload_tolerance_percent,
        settings.overload_mode.as_str(),
        settings.storage_quota_bytes,
        settings.api_rate_limit_per_minute
    )
    .fetch_one(conn)
    .await;
    Ok(classify(row)?.map(Tenant::from))
}

/// Replaces the settings of an active tenant whose version is one of
/// `expected_versions` (`None`: any).
pub async fn update(
    conn: &mut PgConnection,
    actor: Uuid,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    settings: &TenantSettings,
) -> PlatformResult<Tenant> {
    let row = sqlx::query_as!(
        Row,
        r#"
        SELECT id AS "id!", slug AS "slug!", name AS "name!", default_locale AS "default_locale!",
               time_zone AS "time_zone!", default_capacity_minutes AS "default_capacity_minutes!",
               overload_tolerance_percent AS "overload_tolerance_percent!", overload_mode AS "overload_mode!",
               storage_quota_bytes AS "storage_quota_bytes?", api_rate_limit_per_minute AS "api_rate_limit_per_minute?",
               status AS "status!", version AS "version!", created_at AS "created_at!", updated_at AS "updated_at!"
        FROM app_platform_update_tenant($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        "#,
        actor,
        id,
        expected_versions,
        settings.name,
        settings.default_locale,
        settings.time_zone,
        &settings.default_capacity_minutes,
        settings.overload_tolerance_percent,
        settings.overload_mode.as_str(),
        settings.storage_quota_bytes,
        settings.api_rate_limit_per_minute
    )
    .fetch_optional(&mut *conn)
    .await;
    match classify(row)? {
        Ok(Some(row)) => Ok(Ok(Tenant::from(row))),
        Ok(None) => Ok(Err(missing_or_stale(conn, actor, id).await?)),
        Err(conflict) => Ok(Err(conflict)),
    }
}

/// Starts deleting an active tenant whose version is one of
/// `expected_versions`: it stops resolving at once, and the purge job
/// `job_id` ([`PURGE_TENANT_JOB`]) removes its data in the background.
pub async fn delete(
    conn: &mut PgConnection,
    actor: Uuid,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    job_id: Uuid,
) -> PlatformResult<Tenant> {
    let row = sqlx::query_as!(
        Row,
        r#"
        SELECT id AS "id!", slug AS "slug!", name AS "name!", default_locale AS "default_locale!",
               time_zone AS "time_zone!", default_capacity_minutes AS "default_capacity_minutes!",
               overload_tolerance_percent AS "overload_tolerance_percent!", overload_mode AS "overload_mode!",
               storage_quota_bytes AS "storage_quota_bytes?", api_rate_limit_per_minute AS "api_rate_limit_per_minute?",
               status AS "status!", version AS "version!", created_at AS "created_at!", updated_at AS "updated_at!"
        FROM app_platform_delete_tenant($1, $2, $3, $4)
        "#,
        actor,
        id,
        expected_versions,
        job_id
    )
    .fetch_optional(&mut *conn)
    .await;
    match classify(row)? {
        Ok(Some(row)) => Ok(Ok(Tenant::from(row))),
        Ok(None) => Ok(Err(missing_or_stale(conn, actor, id).await?)),
        Err(conflict) => Ok(Err(conflict)),
    }
}

/// Why an update or deletion matched no row.
async fn missing_or_stale(
    conn: &mut PgConnection,
    actor: Uuid,
    id: Uuid,
) -> Result<PlatformConflict, sqlx::Error> {
    Ok(match get(conn, actor, id).await? {
        Ok(tenant) if tenant.status == TenantStatus::Active => PlatformConflict::VersionMismatch,
        Ok(_) => PlatformConflict::NotFound,
        Err(conflict) => conflict,
    })
}

/// Removes every row of a tenant being deleted, the tenant itself and the
/// accounts that belonged to it only. Returns the number of rows removed.
/// Fails unless the tenant is being deleted.
pub async fn purge(conn: &mut PgConnection, id: Uuid) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar!(r#"SELECT app_platform_purge_tenant($1) AS "removed!""#, id)
        .fetch_one(conn)
        .await
}
