//! OIDC providers of the current tenant (specification §3), configured by its
//! tenant admin. Client secrets are stored encrypted ([`crate::crypto`],
//! the provider id as associated data); the callers encrypt and decrypt.
//! A tenant has few providers: lists are not paginated.

use chrono::{DateTime, Utc};
use sqlx::postgres::PgConnection;
use uuid::Uuid;

/// Unique constraint of (tenant, issuer, client id).
const UNIQUE: &str = "oidc_provider_tenant_id_issuer_client_id_key";

/// A provider, with its encrypted client secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    pub id: Uuid,
    pub display_name: String,
    pub issuer: String,
    pub client_id: String,
    pub client_secret_ciphertext: Vec<u8>,
    pub client_secret_nonce: Vec<u8>,
    pub email_claim: String,
    pub enabled: bool,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A provider as shown on the sign-in page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    pub id: Uuid,
    pub display_name: String,
}

/// What the tenant admin sets, in stored form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderFields<'a> {
    pub display_name: &'a str,
    pub issuer: &'a str,
    pub client_id: &'a str,
    pub email_claim: &'a str,
    pub enabled: bool,
}

/// An encrypted client secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncryptedSecret<'a> {
    pub ciphertext: &'a [u8],
    pub nonce: &'a [u8],
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    NotFound,
    VersionMismatch,
    /// The tenant already has this issuer and client id.
    Duplicate,
}

pub type WriteResult<T> = Result<Result<T, Conflict>, sqlx::Error>;

fn duplicate<T>(result: Result<T, sqlx::Error>) -> WriteResult<T> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(sqlx::Error::Database(err)) if err.constraint() == Some(UNIQUE) => Ok(Err(Conflict::Duplicate)),
        Err(err) => Err(err),
    }
}

/// Every provider of the tenant, in creation order.
pub async fn list(conn: &mut PgConnection) -> Result<Vec<Provider>, sqlx::Error> {
    sqlx::query_as!(
        Provider,
        r#"
        SELECT id, display_name, issuer, client_id, client_secret_ciphertext, client_secret_nonce,
               email_claim, enabled, version, created_at, updated_at
        FROM oidc_provider ORDER BY created_at, id
        "#
    )
    .fetch_all(conn)
    .await
}

/// The enabled providers, for the sign-in page.
pub async fn buttons(conn: &mut PgConnection) -> Result<Vec<Button>, sqlx::Error> {
    sqlx::query_as!(
        Button,
        "SELECT id, display_name FROM oidc_provider WHERE enabled ORDER BY created_at, id"
    )
    .fetch_all(conn)
    .await
}

/// Whether the tenant has an enabled provider to sign in with.
pub async fn has_enabled(conn: &mut PgConnection) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM oidc_provider WHERE enabled) AS "any!""#)
        .fetch_one(conn)
        .await
}

pub async fn find(conn: &mut PgConnection, id: Uuid) -> Result<Option<Provider>, sqlx::Error> {
    sqlx::query_as!(
        Provider,
        r#"
        SELECT id, display_name, issuer, client_id, client_secret_ciphertext, client_secret_nonce,
               email_claim, enabled, version, created_at, updated_at
        FROM oidc_provider WHERE id = $1
        "#,
        id
    )
    .fetch_optional(conn)
    .await
}

pub async fn create(
    conn: &mut PgConnection,
    id: Uuid,
    fields: &ProviderFields<'_>,
    secret: EncryptedSecret<'_>,
) -> WriteResult<Provider> {
    let result = sqlx::query_as!(
        Provider,
        r#"
        INSERT INTO oidc_provider (id, tenant_id, display_name, issuer, client_id,
                                   client_secret_ciphertext, client_secret_nonce, email_claim, enabled)
        VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6, $7, $8)
        RETURNING id, display_name, issuer, client_id, client_secret_ciphertext, client_secret_nonce,
                  email_claim, enabled, version, created_at, updated_at
        "#,
        id,
        fields.display_name,
        fields.issuer,
        fields.client_id,
        secret.ciphertext,
        secret.nonce,
        fields.email_claim,
        fields.enabled
    )
    .fetch_one(conn)
    .await;
    duplicate(result)
}

/// Replaces the provider's fields if its version is one of
/// `expected_versions` (`None`: any); `secret`: `None` keeps the stored one.
pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    fields: &ProviderFields<'_>,
    secret: Option<EncryptedSecret<'_>>,
) -> WriteResult<Provider> {
    let result = sqlx::query_as!(
        Provider,
        r#"
        UPDATE oidc_provider
        SET display_name = $3, issuer = $4, client_id = $5, email_claim = $6, enabled = $7,
            client_secret_ciphertext = coalesce($8, client_secret_ciphertext),
            client_secret_nonce = coalesce($9, client_secret_nonce),
            version = version + 1
        WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))
        RETURNING id, display_name, issuer, client_id, client_secret_ciphertext, client_secret_nonce,
                  email_claim, enabled, version, created_at, updated_at
        "#,
        id,
        expected_versions,
        fields.display_name,
        fields.issuer,
        fields.client_id,
        fields.email_claim,
        fields.enabled,
        secret.map(|s| s.ciphertext),
        secret.map(|s| s.nonce)
    )
    .fetch_optional(&mut *conn)
    .await;
    match duplicate(result)? {
        Ok(Some(provider)) => Ok(Ok(provider)),
        Ok(None) => Ok(Err(missing_or_stale(conn, id).await?)),
        Err(conflict) => Ok(Err(conflict)),
    }
}

pub async fn delete(conn: &mut PgConnection, id: Uuid, expected_versions: Option<&[i64]>) -> WriteResult<()> {
    let done = sqlx::query!(
        "DELETE FROM oidc_provider WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))",
        id,
        expected_versions
    )
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 1 {
        Ok(Ok(()))
    } else {
        Ok(Err(missing_or_stale(conn, id).await?))
    }
}

async fn missing_or_stale(conn: &mut PgConnection, id: Uuid) -> Result<Conflict, sqlx::Error> {
    Ok(if find(conn, id).await?.is_some() {
        Conflict::VersionMismatch
    } else {
        Conflict::NotFound
    })
}
