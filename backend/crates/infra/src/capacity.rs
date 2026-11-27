//! SQL of capacity patterns (L07-US03) and of the tenant's workload
//! settings. Row-level security scopes every query to the current tenant.
//!
//! Every change records `resource.capacity_changed` (a resource's periods)
//! or `tenant.capacity_changed` (the default capacity) in the outbox, with
//! the days it may change: the load engine (L08) recomputes them.

use domain::capacity::{self as rules, WeekMinutes};
use domain::dates::Date as LocalDate;
use serde_json::json;
use sqlx::postgres::PgConnection;
use time::{Date, Month};
use uuid::Uuid;

use crate::outbox::{self, Event};

/// Event of a change of a resource's capacity periods.
pub const RESOURCE_CHANGED: &str = "resource.capacity_changed";

/// Event of a change of the tenant's default capacity.
pub const TENANT_CHANGED: &str = "tenant.capacity_changed";

/// Name of the unique constraint on `(tenant_id, resource_id, effective_from)`.
const DATE_UNIQUE: &str = "capacity_pattern_resource_date_key";

/// A capacity period of a resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapacityPattern {
    pub id: Uuid,
    pub resource_id: Uuid,
    pub effective_from: Date,
    pub week: WeekMinutes,
    pub label: Option<String>,
    pub version: i64,
}

/// A period as written: label trimmed, empty one absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternInput<'a> {
    pub effective_from: Date,
    pub week: WeekMinutes,
    pub label: Option<&'a str>,
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    NotFound,
    VersionMismatch,
    /// The resource has another period starting that day.
    DateTaken,
    /// No such resource in the tenant.
    NoResource,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

/// The workload settings of the tenant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkloadSettings {
    pub default_capacity: WeekMinutes,
    pub overload_tolerance_percent: i16,
    pub overload_mode: String,
    /// Provisional assignments count in the overload check.
    pub overload_counts_provisional: bool,
    /// Version of the tenant.
    pub version: i64,
}

struct Row {
    id: Uuid,
    resource_id: Uuid,
    effective_from: Date,
    minutes: Vec<i32>,
    label: Option<String>,
    version: i64,
}

impl From<Row> for CapacityPattern {
    fn from(row: Row) -> Self {
        Self {
            id: row.id,
            resource_id: row.resource_id,
            effective_from: row.effective_from,
            // The database checks the minutes.
            week: WeekMinutes::from_stored(&row.minutes).unwrap_or(WeekMinutes([0; 7])),
            label: row.label,
            version: row.version,
        }
    }
}

/// The domain's date of `date`; `None` before year 1.
pub fn local_date(date: Date) -> Option<LocalDate> {
    LocalDate::from_ymd(
        date.year(),
        u32::from(u8::from(date.month())),
        u32::from(date.day()),
    )
}

/// The database's date of `date`.
pub fn sql_date(date: LocalDate) -> Option<Date> {
    let (year, month, day) = date.ymd();
    let month = Month::try_from(u8::try_from(month).ok()?).ok()?;
    Date::from_calendar_date(year, month, u8::try_from(day).ok()?).ok()
}

/// The periods of `resource_id`, by effective date.
pub async fn list(conn: &mut PgConnection, resource_id: Uuid) -> Result<Vec<CapacityPattern>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT id, resource_id, effective_from, minutes, label, version
        FROM capacity_pattern
        WHERE resource_id = $1
        ORDER BY effective_from
        "#,
        resource_id
    )
    .fetch_all(conn)
    .await
    .map(|rows| rows.into_iter().map(CapacityPattern::from).collect())
}

async fn find(
    conn: &mut PgConnection,
    resource_id: Uuid,
    id: Uuid,
) -> Result<Option<CapacityPattern>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT id, resource_id, effective_from, minutes, label, version
        FROM capacity_pattern
        WHERE id = $1 AND resource_id = $2
        "#,
        id,
        resource_id
    )
    .fetch_optional(conn)
    .await
    .map(|row| row.map(CapacityPattern::from))
}

/// Adds the period `id` to `resource_id`, by `actor`.
pub async fn insert(
    conn: &mut PgConnection,
    id: Uuid,
    resource_id: Uuid,
    input: &PatternInput<'_>,
    actor: Option<Uuid>,
) -> WriteResult<CapacityPattern> {
    if !resource_exists(conn, resource_id).await? {
        return Ok(Err(WriteConflict::NoResource));
    }
    let inserted = sqlx::query!(
        r#"
        INSERT INTO capacity_pattern (id, tenant_id, resource_id, effective_from, minutes, label)
        VALUES ($1, app_current_tenant(), $2, $3, $4, $5)
        "#,
        id,
        resource_id,
        input.effective_from,
        &minutes_of(input.week),
        input.label
    )
    .execute(&mut *conn)
    .await;
    if let Err(conflict) = map_unique_violation(inserted)? {
        return Ok(Err(conflict));
    }
    changed(conn, resource_id, &[input.effective_from], actor).await?;
    Ok(find(conn, resource_id, id).await?.ok_or(WriteConflict::NotFound))
}

/// Replaces the period `id` of `resource_id`, if its version is one of
/// `expected_versions` (`None`: any).
pub async fn update(
    conn: &mut PgConnection,
    resource_id: Uuid,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    input: &PatternInput<'_>,
    actor: Option<Uuid>,
) -> WriteResult<CapacityPattern> {
    let Some(before) = find(conn, resource_id, id).await? else {
        return Ok(Err(WriteConflict::NotFound));
    };
    let updated = sqlx::query_scalar!(
        r#"
        UPDATE capacity_pattern
        SET effective_from = $4, minutes = $5, label = $6, version = version + 1
        WHERE id = $1 AND resource_id = $2 AND ($3::bigint[] IS NULL OR version = ANY($3))
        RETURNING id
        "#,
        id,
        resource_id,
        expected_versions,
        input.effective_from,
        &minutes_of(input.week),
        input.label
    )
    .fetch_optional(&mut *conn)
    .await;
    match map_unique_violation(updated)? {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(Err(WriteConflict::VersionMismatch)),
        Err(conflict) => return Ok(Err(conflict)),
    }
    changed(
        conn,
        resource_id,
        &[before.effective_from, input.effective_from],
        actor,
    )
    .await?;
    Ok(find(conn, resource_id, id).await?.ok_or(WriteConflict::NotFound))
}

/// Deletes the period `id` of `resource_id`, if its version is expected.
pub async fn delete(
    conn: &mut PgConnection,
    resource_id: Uuid,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    actor: Option<Uuid>,
) -> WriteResult<()> {
    let deleted = sqlx::query_scalar!(
        r#"
        DELETE FROM capacity_pattern
        WHERE id = $1 AND resource_id = $2 AND ($3::bigint[] IS NULL OR version = ANY($3))
        RETURNING effective_from
        "#,
        id,
        resource_id,
        expected_versions
    )
    .fetch_optional(&mut *conn)
    .await?;
    match deleted {
        Some(from) => {
            changed(conn, resource_id, &[from], actor).await?;
            Ok(Ok(()))
        }
        None if find(conn, resource_id, id).await?.is_some() => Ok(Err(WriteConflict::VersionMismatch)),
        None => Ok(Err(WriteConflict::NotFound)),
    }
}

/// Records which days of `resource_id` may have another capacity now that
/// periods starting on `dates` changed.
async fn changed(
    conn: &mut PgConnection,
    resource_id: Uuid,
    dates: &[Date],
    actor: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    let starts: Vec<LocalDate> = list(conn, resource_id)
        .await?
        .iter()
        .filter_map(|p| local_date(p.effective_from))
        .collect();
    let changed: Vec<LocalDate> = dates.iter().copied().filter_map(local_date).collect();
    let Some((from, until)) = rules::affected(&changed, &starts) else {
        return Ok(());
    };
    outbox::record(
        conn,
        &Event {
            kind: RESOURCE_CHANGED,
            aggregate_type: "resource",
            aggregate_id: resource_id,
            payload: json!({
                "resourceId": resource_id,
                "from": from.to_string(),
                "until": until.map(|d| d.to_string()),
            }),
            actor_id: actor,
        },
    )
    .await?;
    Ok(())
}

/// The tenant's workload settings.
pub async fn workload_settings(conn: &mut PgConnection) -> Result<Option<WorkloadSettings>, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        SELECT default_capacity_minutes, overload_tolerance_percent, overload_mode,
               overload_counts_provisional, version
        FROM tenant WHERE id = app_current_tenant()
        "#
    )
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|row| WorkloadSettings {
        default_capacity: WeekMinutes::from_stored(&row.default_capacity_minutes)
            .unwrap_or(WeekMinutes([420, 420, 420, 420, 420, 0, 0])),
        overload_tolerance_percent: row.overload_tolerance_percent,
        overload_mode: row.overload_mode,
        overload_counts_provisional: row.overload_counts_provisional,
        version: row.version,
    }))
}

/// Changes the tenant's workload settings for `actor`, a tenant admin, if
/// the tenant's version is expected. A new default capacity is recorded as
/// `tenant.capacity_changed`. `Ok(None)`: the version did not match.
pub async fn update_workload_settings(
    conn: &mut PgConnection,
    actor: Uuid,
    expected_versions: Option<&[i64]>,
    settings: &WorkloadSettings,
) -> Result<Option<WorkloadSettings>, sqlx::Error> {
    let before = workload_settings(conn).await?;
    let updated = sqlx::query!(
        r#"
        SELECT default_capacity_minutes AS "default_capacity_minutes!", overload_tolerance_percent AS "overload_tolerance_percent!",
               overload_mode AS "overload_mode!",
               overload_counts_provisional AS "overload_counts_provisional!", version AS "version!"
        FROM app_update_workload_settings($1, $2, $3, $4, $5, $6)
        "#,
        actor,
        expected_versions,
        &minutes_of(settings.default_capacity),
        settings.overload_tolerance_percent,
        settings.overload_mode,
        settings.overload_counts_provisional
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = updated else {
        return Ok(None);
    };
    let after = WorkloadSettings {
        default_capacity: WeekMinutes::from_stored(&row.default_capacity_minutes)
            .unwrap_or(settings.default_capacity),
        overload_tolerance_percent: row.overload_tolerance_percent,
        overload_mode: row.overload_mode,
        overload_counts_provisional: row.overload_counts_provisional,
        version: row.version,
    };
    if before.is_some_and(|b| b.default_capacity != after.default_capacity) {
        let tenant = sqlx::query_scalar!(r#"SELECT app_current_tenant() AS "id!""#)
            .fetch_one(&mut *conn)
            .await?;
        outbox::record(
            conn,
            &Event {
                kind: TENANT_CHANGED,
                aggregate_type: "tenant",
                aggregate_id: tenant,
                payload: json!({ "defaultCapacityMinutes": after.default_capacity.0 }),
                actor_id: Some(actor),
            },
        )
        .await?;
    }
    Ok(Some(after))
}

fn minutes_of(week: WeekMinutes) -> Vec<i32> {
    week.0.iter().map(|m| i32::from(*m)).collect()
}

async fn resource_exists(conn: &mut PgConnection, resource_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM resource WHERE id = $1) AS "exists!""#,
        resource_id
    )
    .fetch_one(conn)
    .await
}

fn map_unique_violation<T>(result: Result<T, sqlx::Error>) -> WriteResult<T> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(sqlx::Error::Database(err)) if err.constraint() == Some(DATE_UNIQUE) => {
            Ok(Err(WriteConflict::DateTaken))
        }
        Err(err) => Err(err),
    }
}
