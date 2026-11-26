//! SQL of assignments (L08-US01): one per resource and ticket, with its
//! segments. Row-level security scopes every query to the current tenant.
//!
//! Every write recomputes the assignment's daily load in the same
//! transaction (`crate::day_load`, L08-US02); deleting it takes its load
//! rows along. Every change is recorded in the outbox (`assignment.changed`,
//! with the days it touches).

use std::collections::HashMap;

use domain::assignments::Mode;
use domain::resources::Kind;
use serde_json::json;
use sqlx::postgres::PgConnection;
use time::Date;
use uuid::Uuid;

use crate::outbox::{self, Event};

/// Event of a change of an assignment: created, changed or deleted.
pub const ASSIGNMENT_CHANGED: &str = "assignment.changed";

const RESOURCE_UNIQUE: &str = "assignment_tenant_id_issue_id_resource_id_key";
const NO_OVERLAP: &str = "assignment_segment_no_overlap";

/// A stored segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSegment {
    pub start: Date,
    pub end: Date,
    pub mode: Mode,
    pub value: u32,
}

/// The resource of an assignment, as shown with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignedResource {
    pub id: Uuid,
    pub kind: Kind,
    /// The member's name when linked, else its own.
    pub display_name: String,
    pub job_title: Option<String>,
    pub active: bool,
    /// Linked to a member's account.
    pub linked: bool,
    /// Its site calendar; none: the tenant calendar.
    pub calendar_id: Option<Uuid>,
}

/// A stored assignment, its segments in the order given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub resource: AssignedResource,
    pub provisional: bool,
    pub segments: Vec<StoredSegment>,
    pub version: i64,
}

impl Assignment {
    /// Its first and last day.
    pub fn span(&self) -> Option<(Date, Date)> {
        let first = self.segments.iter().map(|s| s.start).min()?;
        let last = self.segments.iter().map(|s| s.end).max()?;
        Some((first, last))
    }
}

/// An assignment as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignmentInput {
    pub provisional: bool,
    pub segments: Vec<StoredSegment>,
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    NotFound,
    VersionMismatch,
    /// The resource already has an assignment on this ticket.
    ResourceTaken,
    /// Two segments take a same day (the domain checks it first).
    Overlap,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

struct Row {
    id: Uuid,
    issue_id: Uuid,
    resource_id: Uuid,
    kind: String,
    display_name: String,
    job_title: Option<String>,
    active: bool,
    linked: bool,
    calendar_id: Option<Uuid>,
    provisional: bool,
    version: i64,
}

async fn rows(
    conn: &mut PgConnection,
    issue_id: Option<Uuid>,
    ids: Option<&[Uuid]>,
) -> Result<Vec<Row>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT a.id, a.issue_id, a.resource_id, r.kind,
               coalesce(app_member_name(r.account_id), r.display_name) AS "display_name!",
               r.job_title, r.active, r.account_id IS NOT NULL AS "linked!", r.calendar_id,
               a.provisional, a.version
        FROM assignment a
        JOIN resource r ON r.id = a.resource_id
        WHERE ($1::uuid IS NULL OR a.issue_id = $1) AND ($2::uuid[] IS NULL OR a.id = ANY($2))
        ORDER BY lower(coalesce(app_member_name(r.account_id), r.display_name)), a.id
        "#,
        issue_id,
        ids
    )
    .fetch_all(conn)
    .await
}

async fn with_segments(conn: &mut PgConnection, rows: Vec<Row>) -> Result<Vec<Assignment>, sqlx::Error> {
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let segments = sqlx::query!(
        r#"
        SELECT assignment_id, start_date, end_date, mode, value
        FROM assignment_segment
        WHERE assignment_id = ANY($1)
        ORDER BY position
        "#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut by_assignment: HashMap<Uuid, Vec<StoredSegment>> = HashMap::new();
    for row in segments {
        by_assignment
            .entry(row.assignment_id)
            .or_default()
            .push(StoredSegment {
                start: row.start_date,
                end: row.end_date,
                // The database checks the mode and the value.
                mode: Mode::parse(&row.mode).unwrap_or(Mode::HoursPerDay),
                value: u32::try_from(row.value).unwrap_or(0),
            });
    }
    Ok(rows
        .into_iter()
        .map(|row| Assignment {
            id: row.id,
            issue_id: row.issue_id,
            resource: AssignedResource {
                id: row.resource_id,
                kind: Kind::parse(&row.kind).unwrap_or(Kind::Person),
                display_name: row.display_name,
                job_title: row.job_title,
                active: row.active,
                linked: row.linked,
                calendar_id: row.calendar_id,
            },
            provisional: row.provisional,
            segments: by_assignment.remove(&row.id).unwrap_or_default(),
            version: row.version,
        })
        .collect())
}

/// The assignments of `issue_id`, by resource name.
pub async fn of_issue(conn: &mut PgConnection, issue_id: Uuid) -> Result<Vec<Assignment>, sqlx::Error> {
    let rows = rows(conn, Some(issue_id), None).await?;
    with_segments(conn, rows).await
}

/// The assignment `id`.
pub async fn find(conn: &mut PgConnection, id: Uuid) -> Result<Option<Assignment>, sqlx::Error> {
    Ok(find_many(conn, &[id]).await?.pop())
}

/// The assignments `ids` that exist.
pub async fn find_many(conn: &mut PgConnection, ids: &[Uuid]) -> Result<Vec<Assignment>, sqlx::Error> {
    let rows = rows(conn, None, Some(ids)).await?;
    with_segments(conn, rows).await
}

/// Its version, locking the row until the end of the transaction.
pub async fn lock(conn: &mut PgConnection, id: Uuid) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar!("SELECT version FROM assignment WHERE id = $1 FOR UPDATE", id)
        .fetch_optional(conn)
        .await
}

/// Assigns `resource_id` to `issue_id` as the assignment `id`, by `actor`.
pub async fn insert(
    conn: &mut PgConnection,
    id: Uuid,
    issue_id: Uuid,
    resource_id: Uuid,
    input: &AssignmentInput,
    actor: Option<Uuid>,
) -> WriteResult<Assignment> {
    // Checked first so that the transaction stays usable; the unique
    // constraint still settles a race.
    let taken = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM assignment WHERE issue_id = $1 AND resource_id = $2) AS "taken!""#,
        issue_id,
        resource_id
    )
    .fetch_one(&mut *conn)
    .await?;
    if taken {
        return Ok(Err(WriteConflict::ResourceTaken));
    }
    let inserted = sqlx::query!(
        r#"
        INSERT INTO assignment (id, tenant_id, issue_id, resource_id, provisional, created_by)
        VALUES ($1, app_current_tenant(), $2, $3, $4, $5)
        "#,
        id,
        issue_id,
        resource_id,
        input.provisional,
        actor
    )
    .execute(&mut *conn)
    .await;
    if let Some(conflict) = conflict_of(&inserted) {
        return Ok(Err(conflict));
    }
    inserted?;
    if let Err(conflict) = write_segments(conn, id, &input.segments).await? {
        return Ok(Err(conflict));
    }
    crate::day_load::recompute(conn, &[id]).await?;
    changed(conn, id, issue_id, resource_id, &input.segments, &[], actor).await?;
    Ok(find(conn, id).await?.ok_or(WriteConflict::NotFound))
}

/// Replaces the segments and the provisional flag of the assignment `id`,
/// if its version is one of `expected_versions` (`None`: any).
pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    input: &AssignmentInput,
    actor: Option<Uuid>,
) -> WriteResult<Assignment> {
    let Some(version) = lock(conn, id).await? else {
        return Ok(Err(WriteConflict::NotFound));
    };
    if expected_versions.is_some_and(|versions| !versions.contains(&version)) {
        return Ok(Err(WriteConflict::VersionMismatch));
    }
    let Some(before) = find(conn, id).await? else {
        return Ok(Err(WriteConflict::NotFound));
    };
    sqlx::query!(
        "UPDATE assignment SET provisional = $2, version = version + 1 WHERE id = $1",
        id,
        input.provisional
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!("DELETE FROM assignment_segment WHERE assignment_id = $1", id)
        .execute(&mut *conn)
        .await?;
    if let Err(conflict) = write_segments(conn, id, &input.segments).await? {
        return Ok(Err(conflict));
    }
    crate::day_load::recompute(conn, &[id]).await?;
    changed(
        conn,
        id,
        before.issue_id,
        before.resource.id,
        &input.segments,
        &before.segments,
        actor,
    )
    .await?;
    Ok(find(conn, id).await?.ok_or(WriteConflict::NotFound))
}

/// Deletes the assignment `id`, if its version is expected.
pub async fn delete(
    conn: &mut PgConnection,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    actor: Option<Uuid>,
) -> WriteResult<()> {
    let Some(version) = lock(conn, id).await? else {
        return Ok(Err(WriteConflict::NotFound));
    };
    if expected_versions.is_some_and(|versions| !versions.contains(&version)) {
        return Ok(Err(WriteConflict::VersionMismatch));
    }
    let Some(before) = find(conn, id).await? else {
        return Ok(Err(WriteConflict::NotFound));
    };
    sqlx::query!("DELETE FROM assignment WHERE id = $1", id)
        .execute(&mut *conn)
        .await?;
    changed(
        conn,
        id,
        before.issue_id,
        before.resource.id,
        &[],
        &before.segments,
        actor,
    )
    .await?;
    Ok(Ok(()))
}

async fn write_segments(conn: &mut PgConnection, id: Uuid, segments: &[StoredSegment]) -> WriteResult<()> {
    let ids: Vec<Uuid> = segments.iter().map(|_| crate::id::new_id()).collect();
    let positions: Vec<i16> = (0..).take(segments.len()).collect();
    let starts: Vec<Date> = segments.iter().map(|s| s.start).collect();
    let ends: Vec<Date> = segments.iter().map(|s| s.end).collect();
    let modes: Vec<String> = segments.iter().map(|s| s.mode.as_str().to_owned()).collect();
    let values: Vec<i32> = segments
        .iter()
        .map(|s| i32::try_from(s.value).unwrap_or(i32::MAX))
        .collect();
    let inserted = sqlx::query!(
        r#"
        INSERT INTO assignment_segment (id, tenant_id, assignment_id, position, start_date, end_date, mode, value)
        SELECT s.id, app_current_tenant(), $1, s.position, s.start_date, s.end_date, s.mode, s.value
        FROM unnest($2::uuid[], $3::smallint[], $4::date[], $5::date[], $6::text[], $7::integer[])
             AS s (id, position, start_date, end_date, mode, value)
        "#,
        id,
        &ids,
        &positions,
        &starts,
        &ends,
        &modes,
        &values
    )
    .execute(&mut *conn)
    .await;
    if let Some(conflict) = conflict_of(&inserted) {
        return Ok(Err(conflict));
    }
    inserted?;
    Ok(Ok(()))
}

fn conflict_of<T>(result: &Result<T, sqlx::Error>) -> Option<WriteConflict> {
    let Err(sqlx::Error::Database(error)) = result else {
        return None;
    };
    match error.constraint() {
        Some(RESOURCE_UNIQUE) => Some(WriteConflict::ResourceTaken),
        Some(NO_OVERLAP) => Some(WriteConflict::Overlap),
        _ => None,
    }
}

/// Records that the assignment `id` changed, with the days its segments
/// took before and take now.
async fn changed(
    conn: &mut PgConnection,
    id: Uuid,
    issue_id: Uuid,
    resource_id: Uuid,
    now: &[StoredSegment],
    before: &[StoredSegment],
    actor: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    let all = now.iter().chain(before);
    let from = all.clone().map(|s| s.start).min();
    let until = all.map(|s| s.end).max().and_then(Date::next_day);
    outbox::record(
        conn,
        &Event {
            kind: ASSIGNMENT_CHANGED,
            aggregate_type: "assignment",
            aggregate_id: id,
            payload: json!({
                "assignmentId": id,
                "issueId": issue_id,
                "resourceId": resource_id,
                "from": from.map(|d| d.to_string()),
                "until": until.map(|d| d.to_string()),
            }),
            actor_id: actor,
        },
    )
    .await?;
    Ok(())
}
