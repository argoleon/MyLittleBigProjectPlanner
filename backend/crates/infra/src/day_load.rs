//! Daily load of resources (specification §8, L08-US02): one row of
//! `resource_day_load` per assignment and day it loads, derived from the
//! segments by `domain::assignments::assignment_load` over the resource's
//! capacity, calendars and absences (`crate::day_capacity`).
//!
//! - Writing an assignment recomputes its rows in the same transaction
//!   (`crate::assignments`); deleting it takes them along.
//! - A change of capacity, calendar, absence or default capacity queues a
//!   [`RECOMPUTE_JOB`] (`crate::outbox::record`): the worker recomputes the
//!   assignments it may touch. Recomputing replaces an assignment's rows as
//!   a whole, under its row lock: a retry, or two jobs on the same
//!   assignment, end with the rows a single run gives.
//!
//! The table is partitioned by month of the day; the partitions a write
//! needs are created first (`app_ensure_load_partitions`).

use std::collections::HashMap;

use domain::assignments::assignment_load;
use domain::dates::Date as LocalDate;
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgConnection;
use time::Date;
use uuid::Uuid;

use crate::absences::ABSENCE_CHANGED;
use crate::assignments::{Assignment, StoredSegment};
use crate::calendars::CALENDAR_CHANGED;
use crate::capacity::{RESOURCE_CHANGED, TENANT_CHANGED, local_date, sql_date};

/// Job recomputing the daily load of the assignments a change touches.
pub const RECOMPUTE_JOB: &str = "workload.recompute";

/// Assignments recomputed together, in one set of queries.
const CHUNK: usize = 200;

/// Whose load a change may touch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scope {
    /// These resources.
    Resources { ids: Vec<Uuid> },
    /// The resources of this calendar: a site's, or every resource for the
    /// tenant calendar.
    Calendar { id: Uuid },
    /// Every resource of the tenant.
    All,
}

/// A recomputation: whose load, from which day until which day (excluded).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recompute {
    pub scope: Scope,
    /// First day concerned (`YYYY-MM-DD`); none: since ever.
    pub from: Option<String>,
    /// First day no longer concerned; none: no end.
    pub until: Option<String>,
}

fn text(payload: &serde_json::Value, name: &str) -> Option<String> {
    payload
        .get(name)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

impl Recompute {
    /// The recomputation an outbox event calls for, if any.
    pub fn of_event(kind: &str, payload: &serde_json::Value) -> Option<Self> {
        let id = |name: &str| text(payload, name).and_then(|v| Uuid::try_parse(&v).ok());
        match kind {
            RESOURCE_CHANGED | ABSENCE_CHANGED => Some(Self {
                scope: Scope::Resources {
                    ids: vec![id("resourceId")?],
                },
                from: text(payload, "from"),
                until: text(payload, "until"),
            }),
            CALENDAR_CHANGED => {
                // The days changed; none: any day (a holiday set, a deletion).
                let days: Option<Vec<LocalDate>> = payload
                    .get("days")
                    .and_then(serde_json::Value::as_array)
                    .map(|days| {
                        days.iter()
                            .filter_map(|d| d.as_str().and_then(LocalDate::parse))
                            .collect()
                    });
                let (from, until) = match days.as_deref() {
                    Some([first, .., last]) => (Some(*first), Some(last.add_days(1))),
                    Some([only]) => (Some(*only), Some(only.add_days(1))),
                    Some([]) | None => (None, None),
                };
                Some(Self {
                    scope: Scope::Calendar {
                        id: id("calendarId")?,
                    },
                    from: from.map(|d| d.to_string()),
                    until: until.map(|d| d.to_string()),
                })
            }
            TENANT_CHANGED => Some(Self {
                scope: Scope::All,
                from: None,
                until: None,
            }),
            _ => None,
        }
    }

    fn bound(value: Option<&str>) -> Option<Date> {
        value.and_then(LocalDate::parse).and_then(sql_date)
    }
}

/// Runs `job` in the current tenant: recomputes the assignments of
/// its scope with a segment between its days; the number of assignments.
pub async fn run(conn: &mut PgConnection, job: &Recompute) -> Result<usize, sqlx::Error> {
    let from = Recompute::bound(job.from.as_deref());
    let until = Recompute::bound(job.until.as_deref());
    let (resources, calendar): (Option<Vec<Uuid>>, Option<Uuid>) = match &job.scope {
        Scope::Resources { ids } => (Some(ids.clone()), None),
        Scope::All => (None, None),
        Scope::Calendar { id } => {
            let kind = sqlx::query_scalar!("SELECT kind FROM calendar WHERE id = $1", id)
                .fetch_optional(&mut *conn)
                .await?;
            match kind.as_deref() {
                // A site: its resources.
                Some("site") => (None, Some(*id)),
                // The tenant calendar, or a site deleted since (its
                // resources moved to the tenant calendar): everyone.
                _ => (None, None),
            }
        }
    };
    let ids = sqlx::query_scalar!(
        r#"
        SELECT DISTINCT a.id AS "id!"
        FROM assignment a
        JOIN resource r ON r.id = a.resource_id
        JOIN assignment_segment s ON s.assignment_id = a.id
        WHERE ($1::uuid[] IS NULL OR a.resource_id = ANY($1))
          AND ($2::uuid IS NULL OR r.calendar_id = $2)
          AND ($3::date IS NULL OR s.end_date >= $3)
          AND ($4::date IS NULL OR s.start_date < $4)
        ORDER BY 1
        "#,
        resources.as_deref(),
        calendar,
        from,
        until
    )
    .fetch_all(&mut *conn)
    .await?;
    for chunk in ids.chunks(CHUNK) {
        recompute(conn, chunk).await?;
    }
    Ok(ids.len())
}

/// A row of daily load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadRow {
    pub resource_id: Uuid,
    pub day: Date,
    pub assignment_id: Uuid,
    pub issue_id: Uuid,
    pub minutes: u32,
    pub provisional: bool,
}

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

/// The rows of `assignments`, from their resources' capacity: one set of
/// queries for them all.
pub async fn rows_of(
    conn: &mut PgConnection,
    assignments: &[Assignment],
) -> Result<Vec<LoadRow>, sqlx::Error> {
    let spans: Vec<(Date, Date)> = assignments.iter().filter_map(Assignment::span).collect();
    let (Some(from), Some(to)) = (
        spans.iter().map(|(first, _)| *first).min(),
        spans.iter().map(|(_, last)| *last).max(),
    ) else {
        return Ok(Vec::new());
    };
    let mut resources: Vec<(Uuid, Option<Uuid>)> = assignments
        .iter()
        .map(|a| (a.resource.id, a.resource.calendar_id))
        .collect();
    resources.sort_unstable();
    resources.dedup();
    let inputs = crate::day_capacity::load(conn, &resources, from, to).await?;
    let mut rows = Vec::new();
    for assignment in assignments {
        let levels = inputs.levels(assignment.resource.id);
        let Some(calendar) = inputs.calendar(assignment.resource.id, &levels) else {
            continue;
        };
        for (day, minutes) in assignment_load(&rules(&assignment.segments), &calendar) {
            let Some(day) = sql_date(day) else { continue };
            rows.push(LoadRow {
                resource_id: assignment.resource.id,
                day,
                assignment_id: assignment.id,
                issue_id: assignment.issue_id,
                minutes,
                provisional: assignment.provisional,
            });
        }
    }
    Ok(rows)
}

/// Recomputes the daily load of the assignments `ids` that still exist,
/// under their row locks; the number of rows written.
pub async fn recompute(conn: &mut PgConnection, ids: &[Uuid]) -> Result<usize, sqlx::Error> {
    // In the order of their ids: two transactions never wait on each other.
    let existing = sqlx::query_scalar!(
        "SELECT id FROM assignment WHERE id = ANY($1) ORDER BY id FOR UPDATE",
        ids
    )
    .fetch_all(&mut *conn)
    .await?;
    let assignments = crate::assignments::find_many(conn, &existing).await?;
    let rows = rows_of(conn, &assignments).await?;
    sqlx::query!(
        "DELETE FROM resource_day_load WHERE assignment_id = ANY($1)",
        &existing
    )
    .execute(&mut *conn)
    .await?;
    if let (Some(first), Some(last)) = (rows.iter().map(|r| r.day).min(), rows.iter().map(|r| r.day).max()) {
        ensure_partitions(conn, first, last).await?;
    }
    let resource_ids: Vec<Uuid> = rows.iter().map(|r| r.resource_id).collect();
    let days: Vec<Date> = rows.iter().map(|r| r.day).collect();
    let assignment_ids: Vec<Uuid> = rows.iter().map(|r| r.assignment_id).collect();
    let issue_ids: Vec<Uuid> = rows.iter().map(|r| r.issue_id).collect();
    let minutes: Vec<i32> = rows
        .iter()
        .map(|r| i32::try_from(r.minutes).unwrap_or(i32::MAX))
        .collect();
    let provisional: Vec<bool> = rows.iter().map(|r| r.provisional).collect();
    sqlx::query!(
        r#"
        INSERT INTO resource_day_load (tenant_id, resource_id, day, assignment_id, issue_id, minutes, provisional)
        SELECT app_current_tenant(), l.resource_id, l.day, l.assignment_id, l.issue_id, l.minutes, l.provisional
        FROM unnest($1::uuid[], $2::date[], $3::uuid[], $4::uuid[], $5::integer[], $6::boolean[])
             AS l (resource_id, day, assignment_id, issue_id, minutes, provisional)
        "#,
        &resource_ids,
        &days,
        &assignment_ids,
        &issue_ids,
        &minutes,
        &provisional
    )
    .execute(&mut *conn)
    .await?;
    Ok(rows.len())
}

/// The load rows of `resource_id` from `from` to `to` included, by day.
pub async fn of_resource(
    conn: &mut PgConnection,
    resource_id: Uuid,
    from: Date,
    to: Date,
) -> Result<Vec<LoadRow>, sqlx::Error> {
    sqlx::query!(
        r#"
        SELECT resource_id, day, assignment_id, issue_id, minutes, provisional
        FROM resource_day_load
        WHERE resource_id = $1 AND day BETWEEN $2 AND $3
        ORDER BY day, assignment_id
        "#,
        resource_id,
        from,
        to
    )
    .fetch_all(conn)
    .await
    .map(|rows| {
        rows.into_iter()
            .map(|r| LoadRow {
                resource_id: r.resource_id,
                day: r.day,
                assignment_id: r.assignment_id,
                issue_id: r.issue_id,
                minutes: u32::try_from(r.minutes).unwrap_or(0),
                provisional: r.provisional,
            })
            .collect()
    })
}

/// Creates the monthly partitions from the month of `first` to that of
/// `last` if missing; the number created.
pub async fn ensure_partitions(conn: &mut PgConnection, first: Date, last: Date) -> Result<i32, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT app_ensure_load_partitions($1, $2) AS "created!""#,
        first,
        last
    )
    .fetch_one(conn)
    .await
}

/// Creates the partitions of the year before and the two years after the
/// current month if missing; the number created. The worker calls it
/// every day.
pub async fn ensure_partitions_ahead(conn: &mut PgConnection) -> Result<i32, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        SELECT app_ensure_load_partitions(
            (date_trunc('month', now() AT TIME ZONE 'UTC') - interval '12 months')::date,
            (date_trunc('month', now() AT TIME ZONE 'UTC') + interval '24 months')::date
        ) AS "created!"
        "#
    )
    .fetch_one(conn)
    .await
}

/// The minutes of each day of `rows`, summed over their assignments.
pub fn by_day(rows: &[LoadRow]) -> HashMap<Date, u32> {
    let mut days = HashMap::new();
    for row in rows {
        *days.entry(row.day).or_insert(0) += row.minutes;
    }
    days
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn capacity_and_absence_changes_touch_their_resource_over_their_days() {
        let id = Uuid::now_v7();
        for kind in [RESOURCE_CHANGED, ABSENCE_CHANGED] {
            let recompute = Recompute::of_event(
                kind,
                &json!({"resourceId": id, "from": "2026-12-01", "until": null}),
            );
            assert_eq!(
                recompute,
                Some(Recompute {
                    scope: Scope::Resources { ids: vec![id] },
                    from: Some("2026-12-01".to_owned()),
                    until: None,
                })
            );
        }
        assert_eq!(
            Recompute::of_event(RESOURCE_CHANGED, &json!({"from": "2026-12-01"})),
            None
        );
    }

    #[test]
    fn a_calendar_change_touches_its_days_and_a_default_capacity_change_everyone() {
        let id = Uuid::now_v7();
        let days = |value: serde_json::Value| {
            Recompute::of_event(CALENDAR_CHANGED, &json!({"calendarId": id, "days": value}))
        };
        let expected = |from: Option<&str>, until: Option<&str>| {
            Some(Recompute {
                scope: Scope::Calendar { id },
                from: from.map(str::to_owned),
                until: until.map(str::to_owned),
            })
        };
        assert_eq!(
            days(json!(["2026-12-24", "2026-12-31"])),
            expected(Some("2026-12-24"), Some("2027-01-01"))
        );
        assert_eq!(
            days(json!(["2026-12-25"])),
            expected(Some("2026-12-25"), Some("2026-12-26"))
        );
        assert_eq!(days(json!(null)), expected(None, None), "any day");
        assert_eq!(
            Recompute::of_event(
                TENANT_CHANGED,
                &json!({"defaultCapacityMinutes": [420, 420, 420, 420, 420, 0, 0]})
            ),
            Some(Recompute {
                scope: Scope::All,
                from: None,
                until: None
            })
        );
        assert_eq!(Recompute::of_event("issue.created", &json!({})), None);
    }

    #[test]
    fn a_recomputation_reads_back_as_written() {
        let recompute = Recompute {
            scope: Scope::Calendar { id: Uuid::now_v7() },
            from: Some("2026-12-24".to_owned()),
            until: None,
        };
        let value = serde_json::to_value(&recompute).unwrap_or_default();
        assert_eq!(value["scope"]["kind"], "calendar");
        assert_eq!(serde_json::from_value::<Recompute>(value).ok(), Some(recompute));
        assert_eq!(Recompute::bound(Some("2026-02-30")), None);
    }

    #[test]
    fn rows_add_up_by_day() {
        let row = |day: Date, minutes| LoadRow {
            resource_id: Uuid::nil(),
            day,
            assignment_id: Uuid::now_v7(),
            issue_id: Uuid::nil(),
            minutes,
            provisional: false,
        };
        let monday = time::macros::date!(2026 - 11 - 23);
        let days = by_day(&[
            row(monday, 120),
            row(monday, 60),
            row(monday.next_day().unwrap_or(monday), 30),
        ]);
        assert_eq!(days[&monday], 180);
        assert_eq!(days.len(), 2);
    }
}
