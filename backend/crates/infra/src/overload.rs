//! SQL of the overload check (specification §8, L08-US03): the tenant's
//! policy, the per-resource lock, the day loads an assignment written
//! would give (`domain::overload` decides), the overloads its ticket flags,
//! and the trace of forced overloads.
//!
//! An assignment write takes its resource's lock ([`lock_resource`])
//! before evaluating: two writers on the same resource run one after the
//! other, and the second one sees the load of the first (read committed).

use std::collections::HashMap;

use domain::calendars::Day;
use domain::dates::Date as LocalDate;
use domain::overload::{self, DayLoad, Overload, Policy};
use domain::resources::Kind;
use domain::tenant::OverloadMode;
use serde_json::json;
use sqlx::postgres::PgConnection;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::assignments::{AssignedResource, Assignment, StoredSegment};
use crate::audit::Author;
use crate::capacity::local_date;
use crate::pagination::{Keyset, Page, page_from_rows};

/// The tenant's overload policy.
pub async fn policy(conn: &mut PgConnection) -> Result<Policy, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        SELECT overload_tolerance_percent, overload_mode, overload_counts_provisional
        FROM tenant WHERE id = app_current_tenant()
        "#
    )
    .fetch_optional(conn)
    .await?;
    Ok(row.map_or_else(Policy::default, |row| Policy {
        // The database keeps it between 0 and 100.
        tolerance_percent: u8::try_from(row.overload_tolerance_percent).unwrap_or(0),
        mode: OverloadMode::parse(&row.overload_mode).unwrap_or(OverloadMode::Blocking),
        count_provisional: row.overload_counts_provisional,
    }))
}

/// Takes the lock of `resource_id`'s load until the end of the transaction:
/// every assignment write on the resource takes it first.
pub async fn lock_resource(conn: &mut PgConnection, resource_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "SELECT pg_advisory_xact_lock(hashtextextended('resource_day_load ' || $1::uuid::text, 0))",
        resource_id
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// An assignment as it would be written.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    /// The assignment changed; none for a new one.
    pub assignment_id: Option<Uuid>,
    pub issue_id: Uuid,
    pub resource_id: Uuid,
    /// The resource's site calendar; none: the tenant calendar.
    pub calendar_id: Option<Uuid>,
    pub provisional: bool,
    pub segments: &'a [StoredSegment],
}

impl Candidate<'_> {
    /// The assignment as `crate::day_load` computes it: only its ids,
    /// calendar, flag and segments matter.
    fn as_assignment(&self) -> Assignment {
        Assignment {
            id: self.assignment_id.unwrap_or_else(Uuid::nil),
            issue_id: self.issue_id,
            resource: AssignedResource {
                id: self.resource_id,
                kind: Kind::Person,
                display_name: String::new(),
                job_title: None,
                active: true,
                linked: false,
                calendar_id: self.calendar_id,
            },
            provisional: self.provisional,
            segments: self.segments.to_vec(),
            version: 0,
        }
    }
}

/// The days of `candidate`'s segments, each with its capacity, the load of
/// the resource's other assignments, and the assignment's load before and
/// after the write, counted by `policy`: what [`overload::overloads`]
/// checks. The same rows as the write computes (`crate::day_load`).
pub async fn day_loads(
    conn: &mut PgConnection,
    policy: &Policy,
    candidate: &Candidate<'_>,
) -> Result<Vec<DayLoad>, sqlx::Error> {
    let (Some(from), Some(to)) = (
        candidate.segments.iter().map(|s| s.start).min(),
        candidate.segments.iter().map(|s| s.end).max(),
    ) else {
        return Ok(Vec::new());
    };
    let after: HashMap<LocalDate, u32> = if policy.counts(candidate.provisional) {
        crate::day_load::rows_of(conn, &[candidate.as_assignment()])
            .await?
            .into_iter()
            .filter_map(|row| Some((local_date(row.day)?, row.minutes)))
            .collect()
    } else {
        HashMap::new()
    };
    let rows = sqlx::query!(
        r#"
        SELECT day, assignment_id, minutes
        FROM resource_day_load
        WHERE resource_id = $1 AND day BETWEEN $2 AND $3 AND ($4 OR NOT provisional)
        "#,
        candidate.resource_id,
        from,
        to,
        policy.count_provisional
    )
    .fetch_all(&mut *conn)
    .await?;
    let (mut existing, mut before) = (HashMap::new(), HashMap::new());
    for row in rows {
        let Some(day) = local_date(row.day) else { continue };
        let side = if Some(row.assignment_id) == candidate.assignment_id {
            &mut before
        } else {
            &mut existing
        };
        *side.entry(day).or_insert(0) += u32::try_from(row.minutes).unwrap_or(0);
    }
    let days = calendar_days(conn, (candidate.resource_id, candidate.calendar_id), from, to).await?;
    Ok(overload::day_loads(&days, &existing, &before, &after))
}

/// The days of `resource` from `from` to `to`, with their capacity.
async fn calendar_days(
    conn: &mut PgConnection,
    resource: (Uuid, Option<Uuid>),
    from: Date,
    to: Date,
) -> Result<Vec<Day>, sqlx::Error> {
    let (Some(first), Some(last)) = (local_date(from), local_date(to)) else {
        return Ok(Vec::new());
    };
    let inputs = crate::day_capacity::load(conn, &[resource], from, to).await?;
    let levels = inputs.levels(resource.0);
    Ok(inputs
        .calendar(resource.0, &levels)
        .map(|calendar| calendar.days(first, last))
        .unwrap_or_default())
}

/// The overloads each of `assignments` (stored) takes part in, counted by
/// `policy` ([`overload::flagged`]): what its ticket shows. Assignments
/// without any are left out.
pub async fn flagged(
    conn: &mut PgConnection,
    policy: &Policy,
    assignments: &[&Assignment],
) -> Result<HashMap<Uuid, Vec<Overload>>, sqlx::Error> {
    let counted: Vec<&Assignment> = assignments
        .iter()
        .copied()
        .filter(|a| policy.counts(a.provisional))
        .collect();
    let spans: Vec<(Date, Date)> = counted.iter().filter_map(|a| a.span()).collect();
    let (Some(from), Some(to)) = (
        spans.iter().map(|(first, _)| *first).min(),
        spans.iter().map(|(_, last)| *last).max(),
    ) else {
        return Ok(HashMap::new());
    };
    let ids: Vec<Uuid> = counted.iter().map(|a| a.id).collect();
    // Each day an assignment loads, with the load of the resource's others.
    let rows = sqlx::query!(
        r#"
        SELECT l.assignment_id, l.day, l.minutes,
               coalesce((
                   SELECT sum(o.minutes) FROM resource_day_load o
                   WHERE o.resource_id = l.resource_id AND o.day = l.day
                     AND o.assignment_id <> l.assignment_id AND ($2 OR NOT o.provisional)
               ), 0)::integer AS "others!"
        FROM resource_day_load l
        WHERE l.assignment_id = ANY($1)
        ORDER BY l.day
        "#,
        &ids,
        policy.count_provisional
    )
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Ok(HashMap::new());
    }
    let mut resources: Vec<(Uuid, Option<Uuid>)> = counted
        .iter()
        .map(|a| (a.resource.id, a.resource.calendar_id))
        .collect();
    resources.sort_unstable();
    resources.dedup();
    let inputs = crate::day_capacity::load(conn, &resources, from, to).await?;
    let resource_of: HashMap<Uuid, Uuid> = counted.iter().map(|a| (a.id, a.resource.id)).collect();
    let mut flagged: HashMap<Uuid, Vec<Overload>> = HashMap::new();
    for row in rows {
        let (Some(day), Some(resource_id)) = (local_date(row.day), resource_of.get(&row.assignment_id))
        else {
            continue;
        };
        let levels = inputs.levels(*resource_id);
        let Some(calendar) = inputs.calendar(*resource_id, &levels) else {
            continue;
        };
        let load = DayLoad {
            date: day,
            capacity: u32::from(calendar.minutes_on(day)),
            existing: u32::try_from(row.others).unwrap_or(0),
            before: 0,
            after: u32::try_from(row.minutes).unwrap_or(0),
        };
        if let Some(found) = overload::flagged(&[load], policy).pop() {
            flagged.entry(row.assignment_id).or_default().push(found);
        }
    }
    Ok(flagged)
}

/// The days of `overloads` as stored and sent.
pub fn days_json(overloads: &[Overload]) -> serde_json::Value {
    overloads
        .iter()
        .map(|o| {
            json!({
                "date": o.date.to_string(),
                "plannedMinutes": o.planned,
                "availableMinutes": o.available,
                "limitMinutes": o.limit,
            })
        })
        .collect()
}

/// A forced overload to trace.
#[derive(Debug, Clone)]
pub struct Forced<'a> {
    pub assignment_id: Uuid,
    pub issue_id: Uuid,
    pub resource_id: Uuid,
    /// The resource's name, kept in the ticket's history.
    pub resource_name: &'a str,
    pub overloads: &'a [Overload],
}

/// Traces `forced` by `author`: in `forced_overload`, and in the ticket's
/// history (`overload_forced`).
pub async fn record_forced(
    conn: &mut PgConnection,
    author: Author,
    forced: &Forced<'_>,
) -> Result<(), sqlx::Error> {
    let Some(actor) = author.actor_id else {
        return Ok(());
    };
    let days = days_json(forced.overloads);
    sqlx::query!(
        r#"
        INSERT INTO forced_overload (id, tenant_id, assignment_id, issue_id, resource_id, actor_id, origin, days)
        VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6, $7)
        "#,
        crate::id::new_id(),
        forced.assignment_id,
        forced.issue_id,
        forced.resource_id,
        actor,
        author.origin.as_str(),
        days
    )
    .execute(&mut *conn)
    .await?;
    crate::audit::record(
        conn,
        forced.issue_id,
        author,
        &[crate::audit::Entry {
            new: Some(json!({
                "assignmentId": forced.assignment_id,
                "resourceId": forced.resource_id,
                "resourceName": forced.resource_name,
                "days": days,
            })),
            ..crate::audit::Entry::action(domain::audit::Action::OverloadForced)
        }],
    )
    .await?;
    Ok(())
}

/// A forced overload, as tenant admins read it.
#[derive(Debug, Clone, PartialEq)]
pub struct ForcedOverload {
    pub id: Uuid,
    pub assignment_id: Uuid,
    pub issue_id: Uuid,
    /// None: the ticket is gone.
    pub issue_key: Option<String>,
    pub issue_summary: Option<String>,
    pub resource_id: Uuid,
    /// None: the resource is gone.
    pub resource_name: Option<String>,
    pub actor_id: Uuid,
    /// None: no longer a member.
    pub actor_name: Option<String>,
    pub origin: String,
    pub days: Vec<Overload>,
    pub created_at: OffsetDateTime,
}

fn overloads_of(days: &serde_json::Value) -> Vec<Overload> {
    let number = |day: &serde_json::Value, name: &str| {
        day.get(name)
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0)
    };
    days.as_array()
        .map(|days| {
            days.iter()
                .filter_map(|day| {
                    Some(Overload {
                        date: day.get("date")?.as_str().and_then(LocalDate::parse)?,
                        planned: number(day, "plannedMinutes"),
                        available: number(day, "availableMinutes"),
                        limit: number(day, "limitMinutes"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The forced overloads of the tenant, newest first: `limit` of them
/// before `before` (none: the newest).
pub async fn list_forced(
    conn: &mut PgConnection,
    limit: usize,
    before: Option<Keyset>,
) -> Result<Page<ForcedOverload>, sqlx::Error> {
    let fetch = i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX);
    let rows = sqlx::query!(
        r#"
        SELECT f.id, f.assignment_id, f.issue_id, i.key AS "issue_key?", i.summary AS "issue_summary?",
               f.resource_id, coalesce(app_member_name(r.account_id), r.display_name) AS "resource_name?",
               f.actor_id, app_member_name(f.actor_id) AS "actor_name?", f.origin, f.days, f.created_at
        FROM forced_overload f
        LEFT JOIN issue i ON i.id = f.issue_id
        LEFT JOIN resource r ON r.id = f.resource_id
        WHERE ($1::timestamptz IS NULL OR (f.created_at, f.id) < ($1, $2))
        ORDER BY f.created_at DESC, f.id DESC
        LIMIT $3
        "#,
        before.map(|k| k.created_at),
        before.map(|k| k.id),
        fetch
    )
    .fetch_all(conn)
    .await?;
    let items = rows
        .into_iter()
        .map(|row| ForcedOverload {
            id: row.id,
            assignment_id: row.assignment_id,
            issue_id: row.issue_id,
            issue_key: row.issue_key,
            issue_summary: row.issue_summary,
            resource_id: row.resource_id,
            resource_name: row.resource_name,
            actor_id: row.actor_id,
            actor_name: row.actor_name,
            origin: row.origin,
            days: overloads_of(&row.days),
            created_at: row.created_at,
        })
        .collect();
    Ok(page_from_rows(items, limit, |f| Keyset {
        created_at: f.created_at,
        id: f.id,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_read_back_as_stored() {
        let days = vec![Overload {
            date: LocalDate::parse("2026-11-18").unwrap_or_else(|| panic!("date")),
            planned: 480,
            available: 420,
            limit: 420,
        }];
        let json = days_json(&days);
        assert_eq!(json[0]["date"], "2026-11-18");
        assert_eq!(json[0]["plannedMinutes"], 480);
        assert_eq!(overloads_of(&json), days);
        assert_eq!(overloads_of(&json!([{"date": "nope"}])), Vec::new());
        assert_eq!(overloads_of(&json!(null)), Vec::new());
    }
}
