//! Replacing a generic resource with a person (specification §8,
//! L08-US08), in one transaction.
//!
//! - Both resources are locked (`crate::overload::lock_resource`), in the
//!   order of their ids.
//! - Each assignment of the generic resource with days from `today` on is
//!   cut at `today` (`domain::assignments::split_at`): the days before stay
//!   on it, the others move to the person — onto the person's assignment
//!   of the same ticket if there is one (its segments merged), else a new
//!   one. An assignment left without days is deleted.
//! - Tickets of archived projects are read-only: their assignments stay on
//!   the generic resource (`Replacement::archived`).
//! - Each ticket's history records the change (`resource_replaced`).
//! - The person's load is checked against their capacity on the days the
//!   replacement adds to (`domain::overload`): the caller refuses, or
//!   forces, and commits or rolls back.

use std::collections::HashMap;

use domain::assignments::{Segment, SegmentError, split_at, validate};
use domain::audit::Action;
use domain::dates::Date as LocalDate;
use domain::overload::{DayLoad, Overload, Policy, overloads};
use serde_json::json;
use sqlx::postgres::PgConnection;
use time::Date;
use uuid::Uuid;

use crate::assignments::{self, Assignment, AssignmentInput, StoredSegment, WriteConflict};
use crate::audit::{Author, Entry};
use crate::capacity::{local_date, sql_date};

/// A resource as the replacement needs it.
#[derive(Debug, Clone, Copy)]
pub struct Side<'a> {
    pub id: Uuid,
    /// Its site calendar; none: the tenant calendar.
    pub calendar_id: Option<Uuid>,
    pub name: &'a str,
}

/// A ticket of the replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticket {
    pub issue_id: Uuid,
    pub key: String,
    pub project_id: Uuid,
}

/// An assignment the replacement moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    pub ticket: Ticket,
    /// The person's assignment of the ticket, as written.
    pub assignment_id: Uuid,
    /// Merged into the person's assignment of the ticket.
    pub merged: bool,
    /// The first and last day moved.
    pub first: Date,
    pub last: Date,
}

/// What a replacement did, or would do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Replacement {
    /// The assignments moved, in the order of their ids.
    pub moved: Vec<Moved>,
    /// Tickets where the person's segments and the moved ones would take a
    /// same day: nothing was written for them; the replacement is refused.
    pub overlaps: Vec<Ticket>,
    /// Tickets of archived projects, left on the generic resource.
    pub archived: Vec<Ticket>,
    /// The person's days over capacity plus tolerance the replacement adds
    /// load to.
    pub overloads: Vec<Overload>,
}

fn stored(segment: &Segment) -> Option<StoredSegment> {
    Some(StoredSegment {
        start: sql_date(segment.start)?,
        end: sql_date(segment.end)?,
        mode: segment.mode,
        value: segment.value,
    })
}

fn rule(segment: &StoredSegment) -> Option<Segment> {
    Some(Segment {
        start: local_date(segment.start)?,
        end: local_date(segment.end)?,
        mode: segment.mode,
        value: segment.value,
    })
}

/// The minutes `resource_id` has planned on each day from `from` to `to`,
/// counted by `policy` (provisional assignments).
async fn totals(
    conn: &mut PgConnection,
    resource_id: Uuid,
    from: Date,
    to: Date,
    policy: &Policy,
) -> Result<HashMap<LocalDate, u32>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT day, sum(minutes)::integer AS "minutes!" FROM resource_day_load
        WHERE resource_id = $1 AND day BETWEEN $2 AND $3 AND ($4 OR NOT provisional)
        GROUP BY day
        "#,
        resource_id,
        from,
        to,
        policy.count_provisional
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| Some((local_date(r.day)?, u32::try_from(r.minutes).unwrap_or(0))))
        .collect())
}

/// Both resources are locked and the segments checked first: a refusal
/// here is a bug, reported as such.
fn conflict_error(conflict: WriteConflict) -> sqlx::Error {
    sqlx::Error::Protocol(format!("assignment write refused while replacing: {conflict:?}"))
}

/// The person's assignment of `ticket` once `moved` joins it: its id, its
/// segments, and whether they overlap.
async fn merged_into(
    conn: &mut PgConnection,
    ticket: &Ticket,
    person: Uuid,
    moved: &[StoredSegment],
) -> Result<Option<(Assignment, Vec<StoredSegment>, bool)>, sqlx::Error> {
    let existing = sqlx::query_scalar!(
        "SELECT id FROM assignment WHERE issue_id = $1 AND resource_id = $2",
        ticket.issue_id,
        person
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(id) = existing else { return Ok(None) };
    let Some(current) = assignments::find(conn, id).await? else {
        return Ok(None);
    };
    let mut segments = current.segments.clone();
    segments.extend(moved.iter().cloned());
    segments.sort_by_key(|s| s.start);
    let rules: Vec<Segment> = segments.iter().filter_map(rule).collect();
    let overlap = matches!(validate(&rules, None, None), Err(SegmentError::Overlap(_)));
    Ok(Some((current, segments, overlap)))
}

/// Moves the assignments of `generic` from `today` on to `person`, by
/// `author`, under `policy`; what it did. Writes nothing for a ticket
/// where segments would overlap (`Replacement::overlaps`): the caller then
/// rolls back.
pub async fn replace(
    conn: &mut PgConnection,
    generic: Side<'_>,
    person: Side<'_>,
    today: Date,
    policy: &Policy,
    author: Author,
) -> Result<Replacement, sqlx::Error> {
    let (first_lock, second_lock) = if generic.id < person.id {
        (generic.id, person.id)
    } else {
        (person.id, generic.id)
    };
    crate::overload::lock_resource(conn, first_lock).await?;
    crate::overload::lock_resource(conn, second_lock).await?;
    let ids = sqlx::query_scalar!(
        r#"
        SELECT DISTINCT a.id AS "id!" FROM assignment a
        JOIN assignment_segment s ON s.assignment_id = a.id
        WHERE a.resource_id = $1 AND s.end_date >= $2
        ORDER BY 1
        "#,
        generic.id,
        today
    )
    .fetch_all(&mut *conn)
    .await?;
    let found: Vec<Assignment> = assignments::find_many(conn, &ids).await?;
    let mut replacement = Replacement::default();
    let spans: Vec<(Date, Date)> = found.iter().filter_map(Assignment::span).collect();
    let (Some(span_first), Some(span_last)) = (
        spans.iter().map(|(first, _)| *first).min(),
        spans.iter().map(|(_, last)| *last).max(),
    ) else {
        return Ok(replacement);
    };
    let (Some(local_today), Some(local_last)) = (local_date(today), local_date(span_last)) else {
        return Ok(replacement);
    };
    let inputs =
        crate::day_capacity::load(conn, &[(generic.id, generic.calendar_id)], span_first, span_last).await?;
    let levels = inputs.levels(generic.id);
    let Some(calendar) = inputs.calendar(generic.id, &levels) else {
        return Ok(replacement);
    };
    let before = totals(conn, person.id, today, span_last, policy).await?;
    let issue_ids: Vec<Uuid> = found.iter().map(|a| a.issue_id).collect();
    let refs = assignments::issue_refs(conn, &issue_ids).await?;

    for assignment in &found {
        let (mut past, mut future) = (Vec::new(), Vec::new());
        for segment in assignment.segments.iter().filter_map(rule) {
            let (kept, moved) = split_at(&segment, local_today, &calendar.days(segment.start, segment.end));
            past.extend(kept.as_ref().and_then(stored));
            future.extend(moved.as_ref().and_then(stored));
        }
        let (Some(first), Some(last)) = (
            future.iter().map(|s| s.start).min(),
            future.iter().map(|s| s.end).max(),
        ) else {
            continue;
        };
        let Some(issue) = refs.get(&assignment.issue_id) else {
            continue;
        };
        let ticket = Ticket {
            issue_id: issue.issue_id,
            key: issue.key.clone(),
            project_id: issue.project_id,
        };
        if issue.project_archived {
            replacement.archived.push(ticket);
            continue;
        }
        let merged = merged_into(conn, &ticket, person.id, &future).await?;
        if merged.as_ref().is_some_and(|(_, _, overlap)| *overlap) {
            replacement.overlaps.push(ticket);
            continue;
        }
        let was_merged = merged.is_some();
        let written = match merged {
            Some((current, segments, _)) => {
                let input = AssignmentInput {
                    provisional: current.provisional,
                    segments,
                };
                assignments::update(conn, current.id, None, &input, author.actor_id)
                    .await?
                    .map_err(conflict_error)?
                    .id
            }
            None => {
                let input = AssignmentInput {
                    provisional: assignment.provisional,
                    segments: future,
                };
                assignments::insert(
                    conn,
                    crate::id::new_id(),
                    assignment.issue_id,
                    person.id,
                    &input,
                    author.actor_id,
                )
                .await?
                .map_err(conflict_error)?
                .id
            }
        };
        if past.is_empty() {
            assignments::delete(conn, assignment.id, None, author.actor_id)
                .await?
                .map_err(conflict_error)?;
        } else {
            let input = AssignmentInput {
                provisional: assignment.provisional,
                segments: past,
            };
            assignments::update(conn, assignment.id, None, &input, author.actor_id)
                .await?
                .map_err(conflict_error)?;
        }
        crate::audit::record(
            conn,
            assignment.issue_id,
            author,
            &[Entry {
                old: Some(json!({"resourceId": generic.id, "name": generic.name})),
                new: Some(json!({"resourceId": person.id, "name": person.name, "from": today.to_string()})),
                ..Entry::action(Action::ResourceReplaced)
            }],
        )
        .await?;
        replacement.moved.push(Moved {
            ticket,
            assignment_id: written,
            merged: was_merged,
            first,
            last,
        });
    }

    // The person's days the replacement adds to, against their capacity.
    let after = totals(conn, person.id, today, span_last, policy).await?;
    let person_inputs =
        crate::day_capacity::load(conn, &[(person.id, person.calendar_id)], today, span_last).await?;
    let person_levels = person_inputs.levels(person.id);
    if let Some(person_calendar) = person_inputs.calendar(person.id, &person_levels) {
        let days: Vec<DayLoad> = person_calendar
            .days(local_today, local_last)
            .iter()
            .map(|day| DayLoad {
                date: day.date,
                capacity: u32::from(day.minutes),
                existing: 0,
                before: before.get(&day.date).copied().unwrap_or(0),
                after: after.get(&day.date).copied().unwrap_or(0),
            })
            .collect();
        replacement.overloads = overloads(&days, policy);
    }
    Ok(replacement)
}
