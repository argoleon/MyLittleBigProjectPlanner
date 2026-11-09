//! Storage of `Idempotency-Key` requests (specification §15).
//!
//! The claim and the creation run in the same tenant transaction:
//! 1. [`claim`] inserts the key. A concurrent request with the same key waits
//!    on the unique index until the first transaction ends.
//! 2. The handler creates the resource, then [`complete`] stores its response.
//! 3. Commit. A replay finds the stored response instead of creating again.
//!
//! If the creation fails, the transaction rolls back and the key is free again.

use domain::idempotency::KEY_LIFETIME;
use sqlx::postgres::PgConnection;

use crate::id::new_id;

/// The response of the original request, replayed as is.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredResponse {
    pub status: u16,
    pub body: serde_json::Value,
    pub location: Option<String>,
    pub etag: Option<String>,
}

/// Outcome of claiming a key.
#[derive(Debug, Clone, PartialEq)]
pub enum Claim {
    /// First use (or the previous use expired): go ahead and create.
    New,
    /// Same key, same request, already completed: return this response.
    Replay(StoredResponse),
    /// Same key used for a different request: refuse.
    Mismatch,
    /// Same key, original request not completed (should not happen as the
    /// claim waits for it): ask the client to retry.
    InFlight,
}

/// Claims `key` for the request identified by `request_hash` in the current
/// tenant (the transaction must be tenant-scoped).
pub async fn claim(conn: &mut PgConnection, key: &str, request_hash: &[u8]) -> Result<Claim, sqlx::Error> {
    let lifetime_secs = KEY_LIFETIME.as_secs_f64();
    let claimed = sqlx::query_scalar!(
        r#"
        INSERT INTO idempotency_key (id, tenant_id, key, request_hash, expires_at)
        VALUES ($1, app_current_tenant(), $2, $3, now() + make_interval(secs => $4))
        ON CONFLICT (tenant_id, key) DO UPDATE
            SET request_hash = EXCLUDED.request_hash,
                response_status = NULL,
                response_body = NULL,
                response_location = NULL,
                response_etag = NULL,
                expires_at = EXCLUDED.expires_at,
                version = idempotency_key.version + 1
            WHERE idempotency_key.expires_at <= now()
        RETURNING id
        "#,
        new_id(),
        key,
        request_hash,
        lifetime_secs
    )
    .fetch_optional(&mut *conn)
    .await?;
    if claimed.is_some() {
        return Ok(Claim::New);
    }

    let existing = sqlx::query!(
        r#"
        SELECT request_hash, response_status, response_body, response_location, response_etag
        FROM idempotency_key
        WHERE key = $1
        "#,
        key
    )
    .fetch_one(&mut *conn)
    .await?;
    if existing.request_hash != request_hash {
        return Ok(Claim::Mismatch);
    }
    match (existing.response_status, existing.response_body) {
        (Some(status), Some(body)) => Ok(Claim::Replay(StoredResponse {
            status: u16::try_from(status).unwrap_or(500),
            body,
            location: existing.response_location,
            etag: existing.response_etag,
        })),
        _ => Ok(Claim::InFlight),
    }
}

/// Stores the response of the request that claimed `key`.
pub async fn complete(
    conn: &mut PgConnection,
    key: &str,
    response: &StoredResponse,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        UPDATE idempotency_key
        SET response_status = $2, response_body = $3, response_location = $4, response_etag = $5
        WHERE key = $1
        "#,
        key,
        i16::try_from(response.status).unwrap_or(500),
        response.body,
        response.location,
        response.etag
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Deletes the current tenant's expired keys; returns how many. Called per
/// tenant by the worker (L02); expired keys are also reused by [`claim`].
pub async fn purge_expired(conn: &mut PgConnection) -> Result<u64, sqlx::Error> {
    let done = sqlx::query!("DELETE FROM idempotency_key WHERE expires_at <= now()")
        .execute(conn)
        .await?;
    Ok(done.rows_affected())
}
