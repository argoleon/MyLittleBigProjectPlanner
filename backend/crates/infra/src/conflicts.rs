//! SQL of absence conflicts (`workload_conflict`, specification §8,
//! L08-US04): per assignment and day, the hours an absence of the resource
//! leaves no room for (`domain::conflicts`).
//!
//! [`sync`] keeps them up to date wherever the daily load is recomputed
//! (`crate::day_load::recompute`): when an assignment is written, and when a
//! capacity, a calendar or an absence changes. Conflicts that appear are
//! opened (or reopened), those that no longer do are resolved. Days newly
//! in conflict, from today on, are recorded in the outbox
//! ([`CONFLICTS_OPENED`], on the ticket): the notification rules of its
//! project say whom to tell (`absence_conflict`).

use std::collections::{BTreeMap, HashMap, HashSet};

use domain::conflicts::absence_conflicts;
use serde_json::json;
use sqlx::postgres::PgConnection;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::assignments::{Assignment, StoredSegment};
use crate::capacity::{local_date, sql_date};
use crate::outbox::{self, Event};
use crate::pagination::{Keyset, Page, page_from_rows};

/// Event of days of a ticket newly in conflict with an absence.
pub const CONFLICTS_OPENED: &str = "workload.conflicts_opened";

fn rules(segments: &[StoredSegment]) -> Vec<domain::assignments::Segment> {
    segments
        .iter()
        .filter_map(|s| {
            Some(domain::assignments::Segment {
                start: local_date(s.start)?,
                end: local_date(s.end)?,
                mode: s.mode,
                value: s.value,
            })
        })
        .collect()
}

/// The conflicts of `assignments`, from their resources' capacity: each
/// assignment with its days and minutes.
async fn computed(
    conn: &mut PgConnection,
    assignments: &[Assignment],
) -> Result<HashMap<Uuid, Vec<(Date, u32)>>, sqlx::Error> {
    let spans: Vec<(Date, Date)> = assignments.iter().filter_map(Assignment::span).collect();
    let (Some(from), Some(to)) = (
        spans.iter().map(|(first, _)| *first).min(),
        spans.iter().map(|(_, last)| *last).max(),
    ) else {
        return Ok(HashMap::new());
    };
    let mut resources: Vec<(Uuid, Option<Uuid>)> = assignments
        .iter()
        .map(|a| (a.resource.id, a.resource.calendar_id))
        .collect();
    resources.sort_unstable();
    resources.dedup();
    let inputs = crate::day_capacity::load(conn, &resources, from, to).await?;
    let mut found = HashMap::new();
    for assignment in assignments {
        let levels = inputs.levels(assignment.resource.id);
        let Some(calendar) = inputs.calendar(assignment.resource.id, &levels) else {
            continue;
        };
        let days: Vec<(Date, u32)> = absence_conflicts(&rules(&assignment.segments), &calendar)
            .into_iter()
            .filter_map(|(day, minutes)| Some((sql_date(day)?, minutes)))
            .collect();
        found.insert(assignment.id, days);
    }
    Ok(found)
}

/// Brings the conflicts of `assignments` (as stored, under their row
/// locks) up to date, `actor` making the change (none: the system); the
/// number of days newly in conflict.
pub async fn sync(
    conn: &mut PgConnection,
    assignments: &[Assignment],
    actor: Option<Uuid>,
) -> Result<usize, sqlx::Error> {
    if assignments.is_empty() {
        return Ok(0);
    }
    let ids: Vec<Uuid> = assignments.iter().map(|a| a.id).collect();
    let found = computed(conn, assignments).await?;
    let open_before: HashSet<(Uuid, Date)> = sqlx::query!(
        "SELECT assignment_id, day FROM workload_conflict WHERE assignment_id = ANY($1) AND status = 'open'",
        &ids
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|row| (row.assignment_id, row.day))
    .collect();

    let (mut conflict_ids, mut assignment_ids, mut resource_ids, mut issue_ids, mut days, mut minutes) = (
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    for assignment in assignments {
        for (day, m) in found.get(&assignment.id).into_iter().flatten() {
            conflict_ids.push(crate::id::new_id());
            assignment_ids.push(assignment.id);
            resource_ids.push(assignment.resource.id);
            issue_ids.push(assignment.issue_id);
            days.push(*day);
            minutes.push(i32::try_from(*m).unwrap_or(i32::MAX));
        }
    }
    // Opened, changed, or reopened.
    sqlx::query!(
        r#"
        INSERT INTO workload_conflict (id, tenant_id, assignment_id, resource_id, issue_id, day, minutes)
        SELECT c.id, app_current_tenant(), c.assignment_id, c.resource_id, c.issue_id, c.day, c.minutes
        FROM unnest($1::uuid[], $2::uuid[], $3::uuid[], $4::uuid[], $5::date[], $6::integer[])
             AS c (id, assignment_id, resource_id, issue_id, day, minutes)
        ON CONFLICT (tenant_id, assignment_id, day) DO UPDATE
        SET minutes = EXCLUDED.minutes, status = 'open', resolved_at = NULL,
            opened_at = CASE WHEN workload_conflict.status = 'resolved' THEN now() ELSE workload_conflict.opened_at END
        "#,
        &conflict_ids,
        &assignment_ids,
        &resource_ids,
        &issue_ids,
        &days,
        &minutes
    )
    .execute(&mut *conn)
    .await?;
    // Those no longer there.
    sqlx::query!(
        r#"
        UPDATE workload_conflict w SET status = 'resolved', resolved_at = now()
        WHERE w.assignment_id = ANY($1) AND w.status = 'open'
          AND NOT EXISTS (
              SELECT 1 FROM unnest($2::uuid[], $3::date[]) AS c (assignment_id, day)
              WHERE c.assignment_id = w.assignment_id AND c.day = w.day
          )
        "#,
        &ids,
        &assignment_ids,
        &days
    )
    .execute(&mut *conn)
    .await?;

    // The days newly in conflict that can still be rescheduled.
    let today = OffsetDateTime::now_utc().date();
    let mut opened = 0;
    for assignment in assignments {
        let new: Vec<(Date, u32)> = found
            .get(&assignment.id)
            .into_iter()
            .flatten()
            .filter(|(day, _)| *day >= today && !open_before.contains(&(assignment.id, *day)))
            .copied()
            .collect();
        if new.is_empty() {
            continue;
        }
        opened += new.len();
        outbox::record(
            conn,
            &Event {
                kind: CONFLICTS_OPENED,
                aggregate_type: "issue",
                aggregate_id: assignment.issue_id,
                payload: json!({
                    "assignmentId": assignment.id,
                    "resourceId": assignment.resource.id,
                    "conflict": {
                        "resourceName": assignment.resource.display_name,
                        "minutes": new.iter().map(|(_, m)| *m).sum::<u32>(),
                        "days": new.iter().map(|(day, m)| json!({"date": day.to_string(), "minutes": m})).collect::<Vec<_>>(),
                    },
                }),
                actor_id: actor,
            },
        )
        .await?;
    }
    Ok(opened)
}

/// An open conflict day of an assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConflictDay {
    pub day: Date,
    pub minutes: u32,
}

/// The open conflicts of `assignment_ids`, by assignment, by day.
pub async fn open_of(
    conn: &mut PgConnection,
    assignment_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<ConflictDay>>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT assignment_id, day, minutes FROM workload_conflict
        WHERE assignment_id = ANY($1) AND status = 'open'
        ORDER BY day
        "#,
        assignment_ids
    )
    .fetch_all(conn)
    .await?;
    let mut by_assignment: HashMap<Uuid, Vec<ConflictDay>> = HashMap::new();
    for row in rows {
        by_assignment
            .entry(row.assignment_id)
            .or_default()
            .push(ConflictDay {
                day: row.day,
                minutes: u32::try_from(row.minutes).unwrap_or(0),
            });
    }
    Ok(by_assignment)
}

/// The open conflict minutes of `resource_id` on each day from `from` to
/// `to`, all tickets together.
pub async fn minutes_by_day(
    conn: &mut PgConnection,
    resource_id: Uuid,
    from: Date,
    to: Date,
) -> Result<BTreeMap<Date, u32>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT day, sum(minutes)::integer AS "minutes!" FROM workload_conflict
        WHERE resource_id = $1 AND status = 'open' AND day BETWEEN $2 AND $3
        GROUP BY day
        "#,
        resource_id,
        from,
        to
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| (row.day, u32::try_from(row.minutes).unwrap_or(0)))
        .collect())
}

/// Which conflicts to list.
#[derive(Debug, Clone, Default)]
pub struct Filter<'a> {
    pub resource_id: Option<Uuid>,
    pub project_id: Option<Uuid>,
    /// `open` or `resolved`; none: both.
    pub status: Option<&'a str>,
    /// The projects the member sees; none: all.
    pub visible_projects: Option<&'a [Uuid]>,
}

/// A conflict, as listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub id: Uuid,
    pub assignment_id: Uuid,
    pub issue_id: Uuid,
    pub issue_key: String,
    pub issue_summary: String,
    pub project_id: Uuid,
    pub resource_id: Uuid,
    pub resource_name: String,
    pub day: Date,
    pub minutes: u32,
    pub status: String,
    pub opened_at: OffsetDateTime,
    pub resolved_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
}

/// The conflicts of `filter`, newest first: `limit` of them before
/// `before` (none: the newest).
pub async fn list(
    conn: &mut PgConnection,
    filter: &Filter<'_>,
    limit: usize,
    before: Option<Keyset>,
) -> Result<Page<Conflict>, sqlx::Error> {
    let fetch = i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX);
    let rows = sqlx::query!(
        r#"
        SELECT w.id, w.assignment_id, w.issue_id, i.key AS issue_key, i.summary AS issue_summary,
               i.project_id, w.resource_id,
               coalesce(app_member_name(r.account_id), r.display_name) AS "resource_name!",
               w.day, w.minutes, w.status, w.opened_at, w.resolved_at, w.created_at
        FROM workload_conflict w
        JOIN issue i ON i.id = w.issue_id
        JOIN resource r ON r.id = w.resource_id
        WHERE ($1::uuid IS NULL OR w.resource_id = $1)
          AND ($2::uuid IS NULL OR i.project_id = $2)
          AND ($3::text IS NULL OR w.status = $3)
          AND ($4::uuid[] IS NULL OR i.project_id = ANY($4))
          AND ($5::timestamptz IS NULL OR (w.created_at, w.id) < ($5, $6))
        ORDER BY w.created_at DESC, w.id DESC
        LIMIT $7
        "#,
        filter.resource_id,
        filter.project_id,
        filter.status,
        filter.visible_projects,
        before.map(|k| k.created_at),
        before.map(|k| k.id),
        fetch
    )
    .fetch_all(conn)
    .await?;
    let items = rows
        .into_iter()
        .map(|row| Conflict {
            id: row.id,
            assignment_id: row.assignment_id,
            issue_id: row.issue_id,
            issue_key: row.issue_key,
            issue_summary: row.issue_summary,
            project_id: row.project_id,
            resource_id: row.resource_id,
            resource_name: row.resource_name,
            day: row.day,
            minutes: u32::try_from(row.minutes).unwrap_or(0),
            status: row.status,
            opened_at: row.opened_at,
            resolved_at: row.resolved_at,
            created_at: row.created_at,
        })
        .collect();
    Ok(page_from_rows(items, limit, |c| Keyset {
        created_at: c.created_at,
        id: c.id,
    }))
}
