//! Weekly and monthly load of resources (`load_aggregate`, specification
//! §8, L08-US06), precomputed for the heatmap over the horizon of
//! `domain::workload::horizon`.
//!
//! - A change of load (an assignment recomputed, `crate::day_load`) or of
//!   capacity (a period, an absence, a calendar, the default capacity)
//!   calls [`mark_stale`]: the weeks and months it meets are marked, and a
//!   [`AGGREGATES_JOB`] is queued shortly after unless one is coming.
//! - The worker runs [`recompute`]: it takes every mark of the tenant and
//!   stores the sums of those periods, from the capacity
//!   (`crate::day_capacity`) and the daily load.
//! - [`compute`] gives the same sums for any periods without storing them:
//!   the heatmap uses it for days, and for periods it finds missing.

use std::collections::{BTreeMap, HashMap};

use domain::aggregates::recompute_in;
use domain::workload::{Period, Scale, horizon, periods, sum_by_period};
use sqlx::postgres::PgConnection;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::capacity::{local_date, sql_date};

/// Kind of the job that recomputes the stale aggregates of its tenant.
pub const AGGREGATES_JOB: &str = "workload.aggregates";

/// Resources recomputed together, in one set of queries.
const CHUNK: usize = 100;

/// A resource (id and site) and its periods marked stale, as (kind, start).
type Marked = BTreeMap<(Uuid, Option<Uuid>), Vec<(String, Date)>>;

/// The sums of a resource over a period, in minutes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sums {
    /// All the load, provisional included.
    pub load: u32,
    /// Its provisional part.
    pub provisional: u32,
    /// The capacity, absences deducted.
    pub capacity: u32,
    /// The capacity before absences.
    pub scheduled: u32,
}

/// A stored kind of period.
pub fn kind_of(scale: Scale) -> Option<&'static str> {
    match scale {
        Scale::Week => Some("week"),
        Scale::Month => Some("month"),
        Scale::Day => None,
    }
}

/// The weeks and months meeting `first`–`last` (each bound optional)
/// within the horizon, as (kind, start).
fn stored_periods(first: Option<Date>, last: Option<Date>) -> Vec<(&'static str, Date)> {
    let Some(today) = local_date(OffsetDateTime::now_utc().date()) else {
        return Vec::new();
    };
    let kept = horizon(today);
    let first = first
        .and_then(local_date)
        .map_or(kept.start, |d| d.max(kept.start));
    let last = last.and_then(local_date).map_or(kept.end, |d| d.min(kept.end));
    let mut found = Vec::new();
    for scale in [Scale::Week, Scale::Month] {
        let Some(kind) = kind_of(scale) else { continue };
        for period in periods(scale, first, last) {
            if let Some(start) = sql_date(period.start) {
                found.push((kind, start));
            }
        }
    }
    found
}

/// Marks stale the weeks and months of `resources` meeting `first`–`last`
/// (each bound optional: the horizon's) within the horizon, and makes sure
/// a job comes for them.
pub async fn mark_stale(
    conn: &mut PgConnection,
    resources: &[Uuid],
    first: Option<Date>,
    last: Option<Date>,
) -> Result<(), sqlx::Error> {
    if resources.is_empty() {
        return Ok(());
    }
    let marked = stored_periods(first, last);
    if marked.is_empty() {
        return Ok(());
    }
    let kinds: Vec<String> = marked.iter().map(|(kind, _)| (*kind).to_owned()).collect();
    let starts: Vec<Date> = marked.iter().map(|(_, start)| *start).collect();
    sqlx::query!(
        r#"
        INSERT INTO load_aggregate_stale (tenant_id, resource_id, period_kind, period_start)
        SELECT app_current_tenant(), r.id, p.kind, p.start
        FROM resource r
        CROSS JOIN unnest($2::text[], $3::date[]) AS p (kind, start)
        WHERE r.id = ANY($1)
        ON CONFLICT DO NOTHING
        "#,
        resources,
        &kinds,
        &starts
    )
    .execute(&mut *conn)
    .await?;
    let pending = crate::jobs::next_queued_in(conn, AGGREGATES_JOB).await?;
    if let Some(delay) = recompute_in(pending) {
        crate::jobs::enqueue_in(conn, AGGREGATES_JOB, &serde_json::json!({}), delay).await?;
    }
    Ok(())
}

/// The sums of `resources` (id and site of each) over each of `periods`,
/// in order and never overlapping, from their capacity and daily load;
/// a resource not found has none.
pub async fn compute(
    conn: &mut PgConnection,
    resources: &[(Uuid, Option<Uuid>)],
    periods: &[Period],
) -> Result<HashMap<Uuid, Vec<Sums>>, sqlx::Error> {
    let (Some(first), Some(last)) = (periods.first(), periods.last()) else {
        return Ok(HashMap::new());
    };
    let (Some(from), Some(to)) = (sql_date(first.start), sql_date(last.end)) else {
        return Ok(HashMap::new());
    };
    if resources.is_empty() {
        return Ok(HashMap::new());
    }
    let ids: Vec<Uuid> = resources.iter().map(|(id, _)| *id).collect();
    let rows = sqlx::query!(
        r#"
        SELECT resource_id, day, sum(minutes)::integer AS "load!",
               coalesce(sum(minutes) FILTER (WHERE provisional), 0)::integer AS "provisional!"
        FROM resource_day_load
        WHERE resource_id = ANY($1) AND day BETWEEN $2 AND $3
        GROUP BY resource_id, day
        "#,
        &ids,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut load: HashMap<Uuid, (Vec<_>, Vec<_>)> = HashMap::new();
    for row in rows {
        let Some(day) = local_date(row.day) else { continue };
        let entry = load.entry(row.resource_id).or_default();
        entry.0.push((day, u32::try_from(row.load).unwrap_or(0)));
        entry.1.push((day, u32::try_from(row.provisional).unwrap_or(0)));
    }
    let inputs = crate::day_capacity::load(conn, resources, from, to).await?;
    let mut sums = HashMap::new();
    for (id, _) in resources {
        let levels = inputs.levels(*id);
        let Some(calendar) = inputs.calendar(*id, &levels) else {
            continue;
        };
        let days = calendar.days(first.start, last.end);
        let capacity: Vec<_> = days.iter().map(|d| (d.date, u32::from(d.minutes))).collect();
        let scheduled: Vec<_> = days.iter().map(|d| (d.date, u32::from(d.scheduled))).collect();
        let (loaded, provisional) = load.remove(id).unwrap_or_default();
        let (loaded, provisional, capacity, scheduled) = (
            sum_by_period(&loaded, periods),
            sum_by_period(&provisional, periods),
            sum_by_period(&capacity, periods),
            sum_by_period(&scheduled, periods),
        );
        sums.insert(
            *id,
            (0..periods.len())
                .map(|i| Sums {
                    load: loaded[i],
                    provisional: provisional[i],
                    capacity: capacity[i],
                    scheduled: scheduled[i],
                })
                .collect(),
        );
    }
    Ok(sums)
}

/// Recomputes every stale aggregate of the tenant; the number stored.
pub async fn recompute(conn: &mut PgConnection) -> Result<usize, sqlx::Error> {
    let marks = sqlx::query!(
        r#"
        DELETE FROM load_aggregate_stale s
        USING resource r
        WHERE r.id = s.resource_id
        RETURNING s.resource_id, r.calendar_id, s.period_kind, s.period_start
        "#
    )
    .fetch_all(&mut *conn)
    .await?;
    // By resource, each with its marked periods.
    let mut by_resource: Marked = BTreeMap::new();
    for mark in marks {
        by_resource
            .entry((mark.resource_id, mark.calendar_id))
            .or_default()
            .push((mark.period_kind, mark.period_start));
    }
    let resources: Vec<(Uuid, Option<Uuid>)> = by_resource.keys().copied().collect();
    let mut stored = 0;
    for chunk in resources.chunks(CHUNK) {
        for scale in [Scale::Week, Scale::Month] {
            let Some(kind) = kind_of(scale) else { continue };
            let wanted: Vec<(Uuid, Date)> = chunk
                .iter()
                .flat_map(|key| {
                    by_resource
                        .get(key)
                        .into_iter()
                        .flatten()
                        .filter(|(k, _)| k == kind)
                        .map(|(_, start)| (key.0, *start))
                })
                .collect();
            let (Some(first), Some(last)) = (
                wanted.iter().map(|(_, s)| *s).min(),
                wanted.iter().map(|(_, s)| *s).max(),
            ) else {
                continue;
            };
            let (Some(first), Some(last)) = (local_date(first), local_date(last)) else {
                continue;
            };
            let spans = periods(scale, first, last);
            let sums = compute(conn, chunk, &spans).await?;
            let (mut ids, mut starts, mut loads, mut provisionals, mut capacities, mut scheduled) = (
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            );
            for (resource, start) in wanted {
                let Some(local) = local_date(start) else { continue };
                let Some(index) = spans.iter().position(|p| p.start == local) else {
                    continue;
                };
                let s = sums
                    .get(&resource)
                    .and_then(|v| v.get(index))
                    .copied()
                    .unwrap_or_default();
                let int = |v: u32| i32::try_from(v).unwrap_or(i32::MAX);
                ids.push(resource);
                starts.push(start);
                loads.push(int(s.load));
                provisionals.push(int(s.provisional));
                capacities.push(int(s.capacity));
                scheduled.push(int(s.scheduled));
            }
            stored += ids.len();
            sqlx::query!(
                r#"
                INSERT INTO load_aggregate (tenant_id, resource_id, period_kind, period_start, load_minutes,
                                            provisional_minutes, capacity_minutes, scheduled_minutes)
                SELECT app_current_tenant(), a.resource_id, $1, a.start, a.load, a.provisional, a.capacity, a.scheduled
                FROM unnest($2::uuid[], $3::date[], $4::integer[], $5::integer[], $6::integer[], $7::integer[])
                     AS a (resource_id, start, load, provisional, capacity, scheduled)
                ON CONFLICT (tenant_id, resource_id, period_kind, period_start) DO UPDATE
                SET load_minutes = EXCLUDED.load_minutes, provisional_minutes = EXCLUDED.provisional_minutes,
                    capacity_minutes = EXCLUDED.capacity_minutes, scheduled_minutes = EXCLUDED.scheduled_minutes
                "#,
                kind,
                &ids,
                &starts,
                &loads,
                &provisionals,
                &capacities,
                &scheduled
            )
            .execute(&mut *conn)
            .await?;
        }
    }
    Ok(stored)
}

/// The stored aggregates of `resources` for the periods of `kind` starting
/// on `starts`, by resource and start.
pub async fn stored(
    conn: &mut PgConnection,
    resources: &[Uuid],
    kind: &str,
    starts: &[Date],
) -> Result<HashMap<(Uuid, Date), Sums>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT resource_id, period_start, load_minutes, provisional_minutes, capacity_minutes, scheduled_minutes
        FROM load_aggregate
        WHERE resource_id = ANY($1) AND period_kind = $2 AND period_start = ANY($3)
        "#,
        resources,
        kind,
        starts
    )
    .fetch_all(conn)
    .await?;
    let read = |v: i32| u32::try_from(v).unwrap_or(0);
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                (r.resource_id, r.period_start),
                Sums {
                    load: read(r.load_minutes),
                    provisional: read(r.provisional_minutes),
                    capacity: read(r.capacity_minutes),
                    scheduled: read(r.scheduled_minutes),
                },
            )
        })
        .collect())
}
