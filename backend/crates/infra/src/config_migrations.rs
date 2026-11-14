//! Migrations of tickets when a configuration change is published
//! (L04-US08): the recorded mapping, and the job that moves the tickets
//! then publishes the draft. Row-level security scopes every query to the
//! current tenant.
//!
//! Tickets come with L05: until then [`run`] has none to move and
//! publishes at once. L05 moves them here in batches
//! (`domain::migration::batches`), each ticket only while it is still in a
//! mapped status or type (`domain::migration::target_of`), with an audit
//! entry of origin `system`, so that a retried job resumes without moving
//! a ticket twice.

use chrono::{DateTime, Utc};
use domain::migration::State;
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgConnection;
use sqlx::types::Json;
use uuid::Uuid;

/// The kind of the job that runs a migration; its payload: [`MigrationJob`].
pub const MIGRATION_JOB: &str = "config.migration";

/// The payload of a migration job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationJob {
    pub migration_id: Uuid,
}

/// A removed status or type, and the one its tickets move to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mapping {
    pub from: Uuid,
    pub to: Uuid,
}

/// A migration and where it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    pub id: Uuid,
    pub config_id: Uuid,
    pub draft_id: Uuid,
    pub state: State,
    pub status_mappings: Vec<Mapping>,
    pub type_mappings: Vec<Mapping>,
    pub ticket_total: i64,
    pub tickets_migrated: i64,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

struct Row {
    id: Uuid,
    config_id: Uuid,
    draft_id: Uuid,
    state: String,
    status_mappings: Json<Vec<Mapping>>,
    type_mappings: Json<Vec<Mapping>>,
    ticket_total: i64,
    tickets_migrated: i64,
    error: Option<String>,
    created_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
}

impl From<Row> for Migration {
    fn from(row: Row) -> Self {
        Self {
            id: row.id,
            config_id: row.config_id,
            draft_id: row.draft_id,
            // The database only holds the states of the domain.
            state: State::parse(&row.state).unwrap_or(State::Failed),
            status_mappings: row.status_mappings.0,
            type_mappings: row.type_mappings.0,
            ticket_total: row.ticket_total,
            tickets_migrated: row.tickets_migrated,
            error: row.error,
            created_at: row.created_at,
            finished_at: row.finished_at,
        }
    }
}

/// A migration to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMigration<'a> {
    pub id: Uuid,
    pub config_id: Uuid,
    /// The draft it publishes.
    pub draft_id: Uuid,
    pub status_mappings: &'a [Mapping],
    pub type_mappings: &'a [Mapping],
    /// The tickets to move.
    pub ticket_total: i64,
    pub created_by: Uuid,
}

/// Records the migration, pending, and queues its job; false if a
/// migration of the configuration is already under way.
pub async fn start(conn: &mut PgConnection, new: &NewMigration<'_>) -> Result<bool, sqlx::Error> {
    let inserted = sqlx::query!(
        r#"
        INSERT INTO config_migration
            (id, tenant_id, config_id, draft_id, status_mappings, type_mappings, ticket_total, created_by)
        VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6, $7)
        "#,
        new.id,
        new.config_id,
        new.draft_id,
        Json(new.status_mappings) as _,
        Json(new.type_mappings) as _,
        new.ticket_total,
        new.created_by
    )
    .execute(&mut *conn)
    .await;
    match inserted {
        Ok(_) => {}
        Err(sqlx::Error::Database(err)) if err.constraint() == Some("config_migration_one_at_a_time") => {
            return Ok(false);
        }
        Err(err) => return Err(err),
    }
    let payload = serde_json::to_value(MigrationJob { migration_id: new.id }).unwrap_or_default();
    crate::jobs::enqueue(conn, MIGRATION_JOB, &payload).await?;
    Ok(true)
}

/// The migration `id` of `config`.
pub async fn find(conn: &mut PgConnection, config: Uuid, id: Uuid) -> Result<Option<Migration>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT id, config_id, draft_id, state, status_mappings AS "status_mappings: Json<Vec<Mapping>>",
               type_mappings AS "type_mappings: Json<Vec<Mapping>>", ticket_total, tickets_migrated, error,
               created_at, finished_at
        FROM config_migration WHERE config_id = $1 AND id = $2
        "#,
        config,
        id
    )
    .fetch_optional(conn)
    .await
    .map(|row| row.map(Migration::from))
}

/// What a run of the job did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Every ticket moved, the draft published.
    Published,
    /// Already over (done or failed): nothing to do again.
    AlreadyOver,
    /// The draft it was to publish is no longer the configuration's: the
    /// migration failed for good, nothing published.
    DraftChanged,
    /// No such migration (its configuration was deleted).
    Missing,
}

/// Runs the migration `id`, in the job's transaction: moves the tickets
/// (none before L05), then publishes the draft and marks it done. Running
/// it again once over does nothing.
pub async fn run(conn: &mut PgConnection, id: Uuid) -> Result<Outcome, sqlx::Error> {
    let found = sqlx::query!(
        "SELECT config_id, draft_id, state FROM config_migration WHERE id = $1 FOR UPDATE",
        id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(migration) = found else {
        return Ok(Outcome::Missing);
    };
    if State::parse(&migration.state).is_none_or(State::is_over) {
        return Ok(Outcome::AlreadyOver);
    }
    sqlx::query!("UPDATE config_migration SET state = 'running' WHERE id = $1", id)
        .execute(&mut *conn)
        .await?;
    let config = crate::configs::find(conn, migration.config_id).await?;
    if config.and_then(|c| c.draft).map(|d| d.id) != Some(migration.draft_id) {
        sqlx::query!(
            r#"
            UPDATE config_migration
            SET state = 'failed', error = 'the draft changed since the mapping', finished_at = now()
            WHERE id = $1
            "#,
            id
        )
        .execute(conn)
        .await?;
        return Ok(Outcome::DraftChanged);
    }
    // L05: the tickets move here, batch by batch, before publishing.
    let published = crate::configs::publish(conn, migration.config_id, None).await?;
    if published.is_err() {
        sqlx::query!(
            r#"
            UPDATE config_migration
            SET state = 'failed', error = 'the draft could not be published', finished_at = now()
            WHERE id = $1
            "#,
            id
        )
        .execute(conn)
        .await?;
        return Ok(Outcome::DraftChanged);
    }
    sqlx::query!(
        "UPDATE config_migration SET state = 'done', tickets_migrated = ticket_total, finished_at = now() WHERE id = $1",
        id
    )
    .execute(conn)
    .await?;
    Ok(Outcome::Published)
}
