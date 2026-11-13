//! Background job queue (specification §17), in PostgreSQL.
//!
//! - [`enqueue`] adds a job in the current tenant transaction: it exists only
//!   if the transaction commits.
//! - The worker [`claim`]s due jobs of every tenant (`FOR UPDATE SKIP LOCKED`:
//!   two workers never get the same job) with a lease, runs each one in its
//!   tenant's context, then [`complete`]s or [`fail`]s it. A job whose worker
//!   died is claimed again when its lease expires.

use std::time::Duration;

use chrono::{DateTime, Utc};
use domain::jobs::DEFAULT_MAX_ATTEMPTS;
use sqlx::postgres::{PgConnection, PgPool};
use uuid::Uuid;

use crate::id::new_id;

/// A job claimed by a worker.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimedJob {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub kind: String,
    pub payload: serde_json::Value,
    /// Number of this attempt, from 1.
    pub attempt: u32,
    pub max_attempts: u32,
}

/// Adds a job of `kind` to the current tenant's queue, due now.
pub async fn enqueue(
    conn: &mut PgConnection,
    kind: &str,
    payload: &serde_json::Value,
) -> Result<Uuid, sqlx::Error> {
    enqueue_in(conn, kind, payload, Duration::ZERO).await
}

/// Adds a job of `kind` to the current tenant's queue, due in `delay`.
pub async fn enqueue_in(
    conn: &mut PgConnection,
    kind: &str,
    payload: &serde_json::Value,
    delay: Duration,
) -> Result<Uuid, sqlx::Error> {
    let id = new_id();
    let max_attempts = i32::try_from(DEFAULT_MAX_ATTEMPTS).unwrap_or(5);
    sqlx::query!(
        r#"
        INSERT INTO job (id, tenant_id, kind, payload, max_attempts, run_at)
        VALUES ($1, app_current_tenant(), $2, $3, $4, now() + make_interval(secs => $5))
        "#,
        id,
        kind,
        payload,
        max_attempts,
        delay.as_secs_f64()
    )
    .execute(conn)
    .await?;
    Ok(id)
}

/// In how long the next queued job of `kind` of the current tenant is due
/// (zero if it already is), if there is one. The job is locked until the
/// end of the transaction: no worker claims it before what this transaction
/// wrote is there for it to read.
pub async fn next_queued_in(conn: &mut PgConnection, kind: &str) -> Result<Option<Duration>, sqlx::Error> {
    let seconds = sqlx::query_scalar!(
        r#"
        SELECT GREATEST(EXTRACT(EPOCH FROM (run_at - now())), 0)::float8 AS "seconds!"
        FROM job
        WHERE kind = $1 AND status = 'queued'
        ORDER BY run_at
        LIMIT 1
        FOR SHARE
        "#,
        kind
    )
    .fetch_optional(conn)
    .await?;
    Ok(seconds.map(Duration::from_secs_f64))
}

/// Claims up to `limit` due jobs of any tenant for `worker`, for `lease`.
pub async fn claim(
    pool: &PgPool,
    worker: &str,
    limit: u32,
    lease: Duration,
) -> Result<Vec<ClaimedJob>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT id AS "id!", tenant_id AS "tenant_id!", kind AS "kind!", payload AS "payload!",
               attempts AS "attempts!", max_attempts AS "max_attempts!"
        FROM app_claim_jobs($1, $2, $3)
        "#,
        worker,
        i32::try_from(limit).unwrap_or(i32::MAX),
        i32::try_from(lease.as_secs()).unwrap_or(i32::MAX)
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ClaimedJob {
            id: row.id,
            tenant_id: row.tenant_id,
            kind: row.kind,
            payload: row.payload,
            attempt: u32::try_from(row.attempts).unwrap_or(1),
            max_attempts: u32::try_from(row.max_attempts).unwrap_or(1),
        })
        .collect())
}

/// The job is done: removed from the queue. False if `worker` lost its lease.
pub async fn complete(pool: &PgPool, id: Uuid, worker: &str) -> Result<bool, sqlx::Error> {
    let done = sqlx::query_scalar!(r#"SELECT app_complete_job($1, $2) AS "done!""#, id, worker)
        .fetch_one(pool)
        .await?;
    Ok(done)
}

/// The job failed: queued again at `retry_at`, or failed for good when
/// `None`. False if `worker` lost its lease.
pub async fn fail(
    pool: &PgPool,
    id: Uuid,
    worker: &str,
    error: &str,
    retry_at: Option<DateTime<Utc>>,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query_scalar!(
        r#"SELECT app_fail_job($1, $2, $3, $4) AS "done!""#,
        id,
        worker,
        error,
        retry_at
    )
    .fetch_one(pool)
    .await?;
    Ok(done)
}
