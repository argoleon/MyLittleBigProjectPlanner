//! Bootstrap of a new platform: the first tenant and its super admin, from
//! the environment ([`BootstrapConfig`]), so that someone can sign in and
//! create the other tenants.
//!
//! Runs with the owner role, right after the migrations, and only while no
//! platform super admin exists: later starts change nothing, even if the
//! variables are still set or changed.

use sqlx::{Connection, PgConnection};
use uuid::Uuid;

use crate::accounts::{AccountType, NewMember, TenantRole};
use crate::config::{BootstrapConfig, Secret};
use crate::id::new_id;

/// Serializes concurrent bootstraps (several api instances starting at once).
const BOOTSTRAP_LOCK_KEY: i64 = 0x6d6c_6270_6273;

/// What [`run`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapOutcome {
    /// A platform super admin already exists: nothing changed.
    AlreadyDone,
    Done {
        tenant_id: Uuid,
        account_id: Uuid,
        /// The tenant did not exist yet.
        tenant_created: bool,
        /// The account did not exist yet (an existing one keeps its password).
        account_created: bool,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("cannot hash the password: {0}")]
    Hash(String),
}

/// Creates the tenant and its super admin of `config` unless a platform super
/// admin exists. `migration_url` is the owner role's URL.
pub async fn run(
    migration_url: &Secret,
    config: &BootstrapConfig,
) -> Result<BootstrapOutcome, BootstrapError> {
    let mut conn = PgConnection::connect(migration_url.expose()).await?;
    let result = run_on(&mut conn, config).await;
    let _ = conn.close().await;
    result
}

async fn run_on(
    conn: &mut PgConnection,
    config: &BootstrapConfig,
) -> Result<BootstrapOutcome, BootstrapError> {
    let mut tx = conn.begin().await?;
    sqlx::query!("SELECT pg_advisory_xact_lock($1) AS locked", BOOTSTRAP_LOCK_KEY)
        .fetch_one(&mut *tx)
        .await?;
    let done = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM platform_admin) AS "done!""#)
        .fetch_one(&mut *tx)
        .await?;
    if done {
        return Ok(BootstrapOutcome::AlreadyDone);
    }

    let existing = sqlx::query_scalar!("SELECT id FROM tenant WHERE slug = $1", config.tenant_slug)
        .fetch_optional(&mut *tx)
        .await?;
    let (tenant_id, tenant_created) = match existing {
        Some(id) => (id, false),
        None => {
            let id = new_id();
            sqlx::query!(
                "INSERT INTO tenant (id, slug, name) VALUES ($1, $2, $3)",
                id,
                config.tenant_slug,
                config.tenant_name
            )
            .execute(&mut *tx)
            .await?;
            (id, true)
        }
    };
    crate::db::set_tenant(&mut tx, tenant_id).await?;

    let password = config.admin_password.expose().to_owned();
    let hash = tokio::task::spawn_blocking(move || crate::credentials::hash_password(&password))
        .await
        .map_err(|e| BootstrapError::Hash(e.to_string()))?
        .map_err(|e| BootstrapError::Hash(e.to_string()))?;
    let added = crate::accounts::add_member(
        &mut tx,
        &NewMember {
            email: &config.admin_email,
            display_name: &config.admin_name,
            password_hash: Some(&hash),
            account_type: AccountType::Internal,
            tenant_role: TenantRole::TenantAdmin,
        },
    )
    .await?;
    sqlx::query!(
        "INSERT INTO platform_admin (account_id) VALUES ($1) ON CONFLICT DO NOTHING",
        added.account_id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(BootstrapOutcome::Done {
        tenant_id,
        account_id: added.account_id,
        tenant_created,
        account_created: added.new_account,
    })
}
