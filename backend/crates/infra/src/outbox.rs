//! The event outbox (specification §12, §17): every change writes its event
//! in its own transaction, so that real time, notifications and later
//! webhooks read one stream that never misses nor invents a change. The
//! table is partitioned by month (`app_ensure_outbox_partitions`).

use sqlx::postgres::PgConnection;
use uuid::Uuid;

/// Months of partitions kept ready ahead of the current one.
pub const MONTHS_AHEAD: i32 = 3;

/// An event to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event<'a> {
    /// `issue.created`…
    pub kind: &'a str,
    /// `issue`…
    pub aggregate_type: &'a str,
    pub aggregate_id: Uuid,
    pub payload: serde_json::Value,
    /// Who made the change; none for the system.
    pub actor_id: Option<Uuid>,
}

/// Records `event` in the current transaction and tenant, and queues its
/// dispatch (`crate::notifications::dispatch`); its id.
pub async fn record(conn: &mut PgConnection, event: &Event<'_>) -> Result<Uuid, sqlx::Error> {
    let id = crate::id::new_id();
    sqlx::query!(
        r#"
        INSERT INTO outbox_event (id, tenant_id, kind, aggregate_type, aggregate_id, payload, actor_id)
        VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6)
        "#,
        id,
        event.kind,
        event.aggregate_type,
        event.aggregate_id,
        event.payload,
        event.actor_id
    )
    .execute(&mut *conn)
    .await?;
    let job = crate::notifications::DispatchJob { event_id: id };
    let payload = serde_json::to_value(job).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    crate::jobs::enqueue(conn, crate::notifications::DISPATCH_JOB, &payload).await?;
    Ok(id)
}

/// Creates the partitions of the current month and of the next
/// [`MONTHS_AHEAD`] if missing; the number created. The worker calls it
/// every day.
pub async fn ensure_partitions(conn: &mut PgConnection) -> Result<i32, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT app_ensure_outbox_partitions($1) AS "created!""#,
        MONTHS_AHEAD
    )
    .fetch_one(conn)
    .await
}
