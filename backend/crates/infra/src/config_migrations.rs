//! Migrations of tickets when a configuration change is published
//! (L04-US08): the recorded mapping, and the job that moves the tickets
//! then publishes the draft. Row-level security scopes every query to the
//! current tenant.
//!
//! Each run of the job moves one batch of tickets ([`BATCH_SIZE`]) in its
//! own transaction, with their history (origin `system`), and queues the
//! next run; the run that finds no more than a batch left moves it and
//! publishes the draft. A ticket moves only while it is still in a mapped
//! status or type (`domain::migration::moves`): a run killed midway rolls
//! back whole, and the retried job resumes without moving a ticket twice.

use std::collections::BTreeMap;

use domain::audit::{Action, Origin};
use domain::migration::{Move, State, Ticket, moves};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use sqlx::postgres::PgConnection;
use sqlx::types::Json;
use time::OffsetDateTime;
use uuid::Uuid;

/// The kind of the job that runs a migration; its payload: [`MigrationJob`].
pub const MIGRATION_JOB: &str = "config.migration";

/// The tickets moved by one run of the job.
pub const BATCH_SIZE: i64 = 500;

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
    pub created_at: OffsetDateTime,
    pub finished_at: Option<OffsetDateTime>,
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
    created_at: OffsetDateTime,
    finished_at: Option<OffsetDateTime>,
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
    queue(conn, new.id).await?;
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

/// Queues a run of the migration `id`.
async fn queue(conn: &mut PgConnection, id: Uuid) -> Result<(), sqlx::Error> {
    let payload = serde_json::to_value(MigrationJob { migration_id: id }).unwrap_or_default();
    crate::jobs::enqueue(conn, MIGRATION_JOB, &payload).await?;
    Ok(())
}

/// The tickets of the projects of the configuration `config` (archived ones
/// included), counted by status and type: `(status, type, count)`, by
/// `item_id`.
pub async fn ticket_counts(
    conn: &mut PgConnection,
    config: Uuid,
) -> Result<Vec<(Uuid, Uuid, i64)>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT i.status_id, i.type_id, count(*) AS "count!"
        FROM issue i
        WHERE i.project_id IN (SELECT n.id FROM node n WHERE n.config_id = $1)
        GROUP BY i.status_id, i.type_id
        "#,
        config
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.status_id, r.type_id, r.count))
        .collect())
}

/// What a run of the job did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A batch of tickets moved; the next run is queued.
    Continues,
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

/// Marks the migration `id` failed for good with `error`.
async fn fail(conn: &mut PgConnection, id: Uuid, error: &str) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE config_migration SET state = 'failed', error = $2, finished_at = now() WHERE id = $1",
        id,
        error
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Runs the migration `id` once, in the job's transaction: moves the next
/// batch of tickets and queues the next run; or, when no more than a batch
/// is left, moves it, publishes the draft and marks the migration done.
/// Running it again once over does nothing.
pub async fn run(conn: &mut PgConnection, id: Uuid) -> Result<Outcome, sqlx::Error> {
    let found = sqlx::query!(
        r#"
        SELECT config_id, draft_id, state, status_mappings AS "status_mappings: Json<Vec<Mapping>>",
               type_mappings AS "type_mappings: Json<Vec<Mapping>>"
        FROM config_migration WHERE id = $1 FOR UPDATE
        "#,
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
    let published = match config {
        Some(c) if c.draft.as_ref().map(|d| d.id) == Some(migration.draft_id) => c.published.id,
        _ => {
            fail(conn, id, "the draft changed since the mapping").await?;
            return Ok(Outcome::DraftChanged);
        }
    };
    let mapping = |list: &[Mapping]| -> BTreeMap<u128, u128> {
        list.iter().map(|m| (m.from.as_u128(), m.to.as_u128())).collect()
    };
    let versions = Versions {
        config: migration.config_id,
        published,
        draft: migration.draft_id,
    };
    let moved = move_batch(
        conn,
        &versions,
        &mapping(&migration.status_mappings.0),
        &mapping(&migration.type_mappings.0),
    )
    .await?;
    if moved >= BATCH_SIZE {
        // Maybe more: the next run reads what is left. Tickets that came
        // into a removed status meanwhile count too.
        sqlx::query!(
            r#"
            UPDATE config_migration
            SET tickets_migrated = tickets_migrated + $2,
                ticket_total = greatest(ticket_total, tickets_migrated + $2)
            WHERE id = $1
            "#,
            id,
            moved
        )
        .execute(&mut *conn)
        .await?;
        queue(conn, id).await?;
        return Ok(Outcome::Continues);
    }
    if crate::configs::publish(conn, migration.config_id, None)
        .await?
        .is_err()
    {
        fail(conn, id, "the draft could not be published").await?;
        return Ok(Outcome::DraftChanged);
    }
    // The total becomes what actually moved: tickets may have left a
    // removed status by themselves meanwhile.
    sqlx::query!(
        r#"
        UPDATE config_migration
        SET state = 'done', tickets_migrated = tickets_migrated + $2, ticket_total = tickets_migrated + $2,
            finished_at = now()
        WHERE id = $1
        "#,
        id,
        moved
    )
    .execute(conn)
    .await?;
    Ok(Outcome::Published)
}

/// The configuration a migration publishes, and its two versions.
struct Versions {
    config: Uuid,
    published: Uuid,
    draft: Uuid,
}

/// A type or a status as the history stores it: its id and its labels at
/// the time (as `api::issue_history` does).
fn labelled(id: u128, labels: &BTreeMap<Uuid, BTreeMap<String, String>>) -> JsonValue {
    let id = Uuid::from_u128(id);
    json!({"id": id, "label": labels.get(&id).cloned().unwrap_or_default()})
}

/// The labels of the statuses and types of the version `version`, by `item_id`.
async fn labels_of(
    conn: &mut PgConnection,
    version: Uuid,
) -> Result<
    (
        BTreeMap<Uuid, BTreeMap<String, String>>,
        BTreeMap<Uuid, BTreeMap<String, String>>,
    ),
    sqlx::Error,
> {
    let statuses = crate::workflows::load(conn, version)
        .await?
        .statuses
        .into_iter()
        .map(|s| (s.item_id, s.label_translations))
        .collect();
    let types = crate::issue_types::list(conn, version)
        .await?
        .into_iter()
        .map(|t| (t.item_id, t.label_translations))
        .collect();
    Ok((statuses, types))
}

/// Moves up to [`BATCH_SIZE`] tickets still in a mapped status or type,
/// their rows locked, each with one change in its history (origin
/// `system`, nobody's behalf); the number moved.
async fn move_batch(
    conn: &mut PgConnection,
    versions: &Versions,
    statuses: &BTreeMap<u128, u128>,
    types: &BTreeMap<u128, u128>,
) -> Result<i64, sqlx::Error> {
    let from =
        |mapping: &BTreeMap<u128, u128>| mapping.keys().map(|k| Uuid::from_u128(*k)).collect::<Vec<_>>();
    let rows = sqlx::query!(
        r#"
        SELECT i.id, i.status_id, i.type_id
        FROM issue i
        WHERE i.project_id IN (SELECT n.id FROM node n WHERE n.config_id = $1)
          AND (i.status_id = ANY($2) OR i.type_id = ANY($3))
        ORDER BY i.id
        LIMIT $4
        FOR UPDATE OF i
        "#,
        versions.config,
        &from(statuses),
        &from(types),
        BATCH_SIZE
    )
    .fetch_all(&mut *conn)
    .await?;
    let tickets: Vec<Ticket> = rows
        .iter()
        .map(|r| Ticket {
            id: r.id.as_u128(),
            status: r.status_id.as_u128(),
            issue_type: r.type_id.as_u128(),
        })
        .collect();
    let planned = moves(&tickets, statuses, types);
    if planned.is_empty() {
        return Ok(0);
    }
    let (old_statuses, old_types) = labels_of(conn, versions.published).await?;
    let (new_statuses, new_types) = labels_of(conn, versions.draft).await?;
    let after = |m: &Move| {
        let ticket = tickets.iter().find(|t| t.id == m.id);
        (
            m.status
                .map_or_else(|| ticket.map_or(0, |t| t.status), |(_, to)| to),
            m.issue_type
                .map_or_else(|| ticket.map_or(0, |t| t.issue_type), |(_, to)| to),
        )
    };
    let (mut ids, mut status_ids, mut type_ids) = (Vec::new(), Vec::new(), Vec::new());
    // The history: one change per ticket.
    let mut entries = HistoryRows::default();
    for m in &planned {
        let (status, issue_type) = after(m);
        ids.push(Uuid::from_u128(m.id));
        status_ids.push(Uuid::from_u128(status));
        type_ids.push(Uuid::from_u128(issue_type));
        let change = crate::id::new_id();
        // Ids grow in this order: the history, newest first, lists the
        // status, then the type (as `audit::record` orders a change).
        let mut changed = Vec::new();
        if let Some((old, new)) = m.issue_type {
            changed.push(("type", labelled(old, &old_types), labelled(new, &new_types)));
        }
        if let Some((old, new)) = m.status {
            changed.push((
                "status",
                labelled(old, &old_statuses),
                labelled(new, &new_statuses),
            ));
        }
        for (field, old, new) in changed {
            entries.push(Uuid::from_u128(m.id), change, field, old, new);
        }
    }
    sqlx::query!(
        r#"
        UPDATE issue
        SET status_id = m.status_id, type_id = m.type_id, version = issue.version + 1, updated_at = now()
        FROM unnest($1::uuid[], $2::uuid[], $3::uuid[]) AS m(id, status_id, type_id)
        WHERE issue.id = m.id
        "#,
        &ids,
        &status_ids,
        &type_ids
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        r#"
        INSERT INTO audit_log (id, tenant_id, entity_type, entity_id, change_id, action, field, old_value,
                               new_value, actor_id, origin)
        SELECT e.id, app_current_tenant(), 'issue', e.entity_id, e.change_id, $7, e.field, e.old_value,
               e.new_value, NULL, $8
        FROM unnest($1::uuid[], $2::uuid[], $3::uuid[], $4::text[], $5::jsonb[], $6::jsonb[])
             AS e(id, entity_id, change_id, field, old_value, new_value)
        "#,
        &entries.ids,
        &entries.issues,
        &entries.changes,
        &entries.fields,
        &entries.old,
        &entries.new,
        Action::Updated.as_str(),
        Origin::System.as_str()
    )
    .execute(&mut *conn)
    .await?;
    Ok(i64::try_from(planned.len()).unwrap_or(i64::MAX))
}

/// History rows to insert at once, column by column.
#[derive(Default)]
struct HistoryRows {
    ids: Vec<Uuid>,
    issues: Vec<Uuid>,
    changes: Vec<Uuid>,
    fields: Vec<String>,
    old: Vec<JsonValue>,
    new: Vec<JsonValue>,
}

impl HistoryRows {
    fn push(&mut self, issue: Uuid, change: Uuid, field: &str, old: JsonValue, new: JsonValue) {
        self.ids.push(crate::id::new_id());
        self.issues.push(issue);
        self.changes.push(change);
        self.fields.push(field.to_owned());
        self.old.push(old);
        self.new.push(new);
    }
}
