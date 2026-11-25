//! SQL of calendars (L07-US04): the tenant calendar, site calendars and
//! the dated exceptions of calendars and resources. Row-level security
//! scopes every query to the current tenant. Public holidays are computed
//! (`domain::holidays`), never stored.
//!
//! Every change is recorded in the outbox for the load engine (L08):
//! `calendar.changed` for a calendar (its resources, or every resource for
//! the tenant calendar), `resource.capacity_changed` for a resource's own
//! exception.

use std::collections::BTreeMap;

use domain::calendars::{Exception, ExceptionKind};
use domain::holidays::HolidaySet;
use serde_json::json;
use sqlx::postgres::PgConnection;
use sqlx::types::Json;
use time::Date;
use uuid::Uuid;

use crate::capacity::{RESOURCE_CHANGED, local_date};
use crate::outbox::{self, Event};

/// Event of a change of a calendar: its holiday set, name or exceptions.
pub const CALENDAR_CHANGED: &str = "calendar.changed";

const NAME_UNIQUE: [&str; 2] = ["calendar_tenant_name_fr_idx", "calendar_tenant_name_en_idx"];
const DAY_UNIQUE: [&str; 2] = [
    "calendar_exception_calendar_day_idx",
    "calendar_exception_resource_day_idx",
];

/// Kind of a calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalendarKind {
    /// The tenant's, one per tenant.
    Tenant,
    /// A site's ("Paris").
    Site,
}

impl CalendarKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tenant => "tenant",
            Self::Site => "site",
        }
    }
}

/// A calendar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Calendar {
    pub id: Uuid,
    pub kind: CalendarKind,
    pub name_translations: BTreeMap<String, String>,
    /// `None`: the tenant's (a site).
    pub holiday_set: Option<HolidaySet>,
    /// Resources of this site.
    pub resource_count: i64,
    pub version: i64,
}

/// A calendar as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarInput {
    /// Trimmed, empty ones dropped.
    pub name_translations: BTreeMap<String, String>,
    pub holiday_set: Option<HolidaySet>,
}

/// Whose an exception is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Calendar(Uuid),
    Resource(Uuid),
}

/// A stored exception.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredException {
    pub id: Uuid,
    pub day: Date,
    pub kind: ExceptionKind,
    pub label: Option<String>,
    pub version: i64,
}

impl StoredException {
    /// The domain's view of it.
    pub fn rule(&self) -> Option<Exception> {
        Some(Exception {
            date: local_date(self.day)?,
            kind: self.kind,
            label: self.label.clone(),
        })
    }
}

/// An exception as written: label trimmed, empty one absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExceptionInput<'a> {
    pub day: Date,
    pub kind: ExceptionKind,
    pub label: Option<&'a str>,
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    NotFound,
    VersionMismatch,
    /// Another calendar has this name in a language.
    NameTaken,
    /// The owner has another exception that day.
    DayTaken,
    /// The tenant calendar is never deleted, and always has a holiday set.
    TenantCalendar,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

struct Row {
    id: Uuid,
    kind: String,
    name: Json<BTreeMap<String, String>>,
    holiday_set: Option<String>,
    resource_count: i64,
    version: i64,
}

impl From<Row> for Calendar {
    fn from(row: Row) -> Self {
        Self {
            id: row.id,
            kind: if row.kind == "tenant" {
                CalendarKind::Tenant
            } else {
                CalendarKind::Site
            },
            name_translations: row.name.0,
            holiday_set: row.holiday_set.as_deref().and_then(HolidaySet::parse),
            resource_count: row.resource_count,
            version: row.version,
        }
    }
}

/// Every calendar of the tenant, the tenant's first, then by creation.
pub async fn list(conn: &mut PgConnection) -> Result<Vec<Calendar>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT c.id, c.kind, c.name AS "name: Json<BTreeMap<String, String>>", c.holiday_set, c.version,
               (SELECT count(*) FROM resource r WHERE r.calendar_id = c.id) AS "resource_count!"
        FROM calendar c
        ORDER BY c.kind = 'site', c.created_at, c.id
        "#
    )
    .fetch_all(conn)
    .await
    .map(|rows| rows.into_iter().map(Calendar::from).collect())
}

/// The calendar `id`.
pub async fn find(conn: &mut PgConnection, id: Uuid) -> Result<Option<Calendar>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT c.id, c.kind, c.name AS "name: Json<BTreeMap<String, String>>", c.holiday_set, c.version,
               (SELECT count(*) FROM resource r WHERE r.calendar_id = c.id) AS "resource_count!"
        FROM calendar c
        WHERE c.id = $1
        "#,
        id
    )
    .fetch_optional(conn)
    .await
    .map(|row| row.map(Calendar::from))
}

/// The tenant calendar.
pub async fn tenant_calendar(conn: &mut PgConnection) -> Result<Option<Calendar>, sqlx::Error> {
    let id = sqlx::query_scalar!("SELECT id FROM calendar WHERE kind = 'tenant'")
        .fetch_optional(&mut *conn)
        .await?;
    match id {
        Some(id) => find(conn, id).await,
        None => Ok(None),
    }
}

/// Whether `id` is a site calendar of the tenant.
pub async fn is_site(conn: &mut PgConnection, id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM calendar WHERE id = $1 AND kind = 'site') AS "exists!""#,
        id
    )
    .fetch_one(conn)
    .await
}

/// Creates the site calendar `id`.
pub async fn insert_site(
    conn: &mut PgConnection,
    id: Uuid,
    input: &CalendarInput,
    actor: Option<Uuid>,
) -> WriteResult<Calendar> {
    let inserted = sqlx::query!(
        "INSERT INTO calendar (id, tenant_id, kind, name, holiday_set) VALUES ($1, app_current_tenant(), 'site', $2, $3)",
        id,
        Json(&input.name_translations) as _,
        input.holiday_set.map(HolidaySet::as_str)
    )
    .execute(&mut *conn)
    .await;
    if let Err(conflict) = map_unique(inserted)? {
        return Ok(Err(conflict));
    }
    calendar_changed(conn, id, None, actor).await?;
    Ok(find(conn, id).await?.ok_or(WriteConflict::NotFound))
}

/// Replaces the name and holiday set of the calendar `id`, if its version
/// is one of `expected_versions` (`None`: any).
pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    input: &CalendarInput,
    actor: Option<Uuid>,
) -> WriteResult<Calendar> {
    let Some(before) = find(conn, id).await? else {
        return Ok(Err(WriteConflict::NotFound));
    };
    if before.kind == CalendarKind::Tenant && input.holiday_set.is_none() {
        return Ok(Err(WriteConflict::TenantCalendar));
    }
    let updated = sqlx::query_scalar!(
        r#"
        UPDATE calendar SET name = $3, holiday_set = $4, version = version + 1
        WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))
        RETURNING id
        "#,
        id,
        expected_versions,
        Json(&input.name_translations) as _,
        input.holiday_set.map(HolidaySet::as_str)
    )
    .fetch_optional(&mut *conn)
    .await;
    match map_unique(updated)? {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(Err(WriteConflict::VersionMismatch)),
        Err(conflict) => return Ok(Err(conflict)),
    }
    if before.holiday_set != input.holiday_set {
        calendar_changed(conn, id, None, actor).await?;
    }
    Ok(find(conn, id).await?.ok_or(WriteConflict::NotFound))
}

/// Deletes the site calendar `id`: its resources fall back to the tenant
/// calendar.
pub async fn delete_site(
    conn: &mut PgConnection,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    actor: Option<Uuid>,
) -> WriteResult<()> {
    let Some(before) = find(conn, id).await? else {
        return Ok(Err(WriteConflict::NotFound));
    };
    if before.kind == CalendarKind::Tenant {
        return Ok(Err(WriteConflict::TenantCalendar));
    }
    // Recorded before: the resources of the site still point to it.
    calendar_changed(conn, id, None, actor).await?;
    let deleted = sqlx::query!(
        "DELETE FROM calendar WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))",
        id,
        expected_versions
    )
    .execute(&mut *conn)
    .await?;
    if deleted.rows_affected() == 1 {
        Ok(Ok(()))
    } else {
        Ok(Err(WriteConflict::VersionMismatch))
    }
}

struct ExceptionRow {
    id: Uuid,
    day: Date,
    kind: String,
    label: Option<String>,
    version: i64,
}

impl From<ExceptionRow> for StoredException {
    fn from(row: ExceptionRow) -> Self {
        Self {
            id: row.id,
            day: row.day,
            // The database checks the kind.
            kind: ExceptionKind::parse(&row.kind).unwrap_or(ExceptionKind::NonWorking),
            label: row.label,
            version: row.version,
        }
    }
}

/// The exceptions of `owner` from `from` to `to` included, by date.
pub async fn exceptions(
    conn: &mut PgConnection,
    owner: Owner,
    from: Date,
    to: Date,
) -> Result<Vec<StoredException>, sqlx::Error> {
    let (calendar, resource) = owner_ids(owner);
    sqlx::query_as!(
        ExceptionRow,
        r#"
        SELECT id, day, kind, label, version
        FROM calendar_exception
        WHERE ($1::uuid IS NULL OR calendar_id = $1) AND ($2::uuid IS NULL OR resource_id = $2)
          AND day BETWEEN $3 AND $4
        ORDER BY day
        "#,
        calendar,
        resource,
        from,
        to
    )
    .fetch_all(conn)
    .await
    .map(|rows| rows.into_iter().map(StoredException::from).collect())
}

async fn find_exception(
    conn: &mut PgConnection,
    owner: Owner,
    id: Uuid,
) -> Result<Option<StoredException>, sqlx::Error> {
    let (calendar, resource) = owner_ids(owner);
    sqlx::query_as!(
        ExceptionRow,
        r#"
        SELECT id, day, kind, label, version
        FROM calendar_exception
        WHERE id = $1 AND ($2::uuid IS NULL OR calendar_id = $2) AND ($3::uuid IS NULL OR resource_id = $3)
        "#,
        id,
        calendar,
        resource
    )
    .fetch_optional(conn)
    .await
    .map(|row| row.map(StoredException::from))
}

/// Adds the exception `id` to `owner`.
pub async fn insert_exception(
    conn: &mut PgConnection,
    id: Uuid,
    owner: Owner,
    input: &ExceptionInput<'_>,
    actor: Option<Uuid>,
) -> WriteResult<StoredException> {
    if !owner_exists(conn, owner).await? {
        return Ok(Err(WriteConflict::NotFound));
    }
    let (calendar, resource) = owner_ids(owner);
    let inserted = sqlx::query!(
        r#"
        INSERT INTO calendar_exception (id, tenant_id, calendar_id, resource_id, day, kind, label)
        VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6)
        "#,
        id,
        calendar,
        resource,
        input.day,
        input.kind.as_str(),
        input.label
    )
    .execute(&mut *conn)
    .await;
    if let Err(conflict) = map_unique(inserted)? {
        return Ok(Err(conflict));
    }
    exception_changed(conn, owner, &[input.day], actor).await?;
    Ok(find_exception(conn, owner, id)
        .await?
        .ok_or(WriteConflict::NotFound))
}

/// Replaces the exception `id` of `owner`, if its version is expected.
pub async fn update_exception(
    conn: &mut PgConnection,
    owner: Owner,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    input: &ExceptionInput<'_>,
    actor: Option<Uuid>,
) -> WriteResult<StoredException> {
    let Some(before) = find_exception(conn, owner, id).await? else {
        return Ok(Err(WriteConflict::NotFound));
    };
    let updated = sqlx::query_scalar!(
        r#"
        UPDATE calendar_exception SET day = $3, kind = $4, label = $5, version = version + 1
        WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))
        RETURNING id
        "#,
        id,
        expected_versions,
        input.day,
        input.kind.as_str(),
        input.label
    )
    .fetch_optional(&mut *conn)
    .await;
    match map_unique(updated)? {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(Err(WriteConflict::VersionMismatch)),
        Err(conflict) => return Ok(Err(conflict)),
    }
    exception_changed(conn, owner, &[before.day, input.day], actor).await?;
    Ok(find_exception(conn, owner, id)
        .await?
        .ok_or(WriteConflict::NotFound))
}

/// Deletes the exception `id` of `owner`, if its version is expected.
pub async fn delete_exception(
    conn: &mut PgConnection,
    owner: Owner,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    actor: Option<Uuid>,
) -> WriteResult<()> {
    let Some(before) = find_exception(conn, owner, id).await? else {
        return Ok(Err(WriteConflict::NotFound));
    };
    let deleted = sqlx::query!(
        "DELETE FROM calendar_exception WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))",
        id,
        expected_versions
    )
    .execute(&mut *conn)
    .await?;
    if deleted.rows_affected() == 0 {
        return Ok(Err(WriteConflict::VersionMismatch));
    }
    exception_changed(conn, owner, &[before.day], actor).await?;
    Ok(Ok(()))
}

fn owner_ids(owner: Owner) -> (Option<Uuid>, Option<Uuid>) {
    match owner {
        Owner::Calendar(id) => (Some(id), None),
        Owner::Resource(id) => (None, Some(id)),
    }
}

async fn owner_exists(conn: &mut PgConnection, owner: Owner) -> Result<bool, sqlx::Error> {
    match owner {
        Owner::Calendar(id) => {
            sqlx::query_scalar!(
                r#"SELECT EXISTS (SELECT 1 FROM calendar WHERE id = $1) AS "e!""#,
                id
            )
            .fetch_one(conn)
            .await
        }
        Owner::Resource(id) => {
            sqlx::query_scalar!(
                r#"SELECT EXISTS (SELECT 1 FROM resource WHERE id = $1) AS "e!""#,
                id
            )
            .fetch_one(conn)
            .await
        }
    }
}

async fn exception_changed(
    conn: &mut PgConnection,
    owner: Owner,
    days: &[Date],
    actor: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    match owner {
        Owner::Calendar(id) => calendar_changed(conn, id, Some(days), actor).await,
        Owner::Resource(id) => {
            let from = days.iter().min().copied();
            let until = days.iter().max().and_then(|d| d.next_day());
            outbox::record(
                conn,
                &Event {
                    kind: RESOURCE_CHANGED,
                    aggregate_type: "resource",
                    aggregate_id: id,
                    payload: json!({
                        "resourceId": id,
                        "from": from.map(|d| d.to_string()),
                        "until": until.map(|d| d.to_string()),
                    }),
                    actor_id: actor,
                },
            )
            .await?;
            Ok(())
        }
    }
}

/// Records that the calendar `id` changed on `days` (`None`: any day).
async fn calendar_changed(
    conn: &mut PgConnection,
    id: Uuid,
    days: Option<&[Date]>,
    actor: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    let mut days: Option<Vec<String>> = days.map(|d| d.iter().map(ToString::to_string).collect());
    if let Some(list) = days.as_mut() {
        list.sort();
        list.dedup();
    }
    outbox::record(
        conn,
        &Event {
            kind: CALENDAR_CHANGED,
            aggregate_type: "calendar",
            aggregate_id: id,
            payload: json!({ "calendarId": id, "days": days }),
            actor_id: actor,
        },
    )
    .await?;
    Ok(())
}

fn map_unique<T>(result: Result<T, sqlx::Error>) -> WriteResult<T> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(sqlx::Error::Database(err)) => match err.constraint() {
            Some(c) if NAME_UNIQUE.contains(&c) => Ok(Err(WriteConflict::NameTaken)),
            Some(c) if DAY_UNIQUE.contains(&c) => Ok(Err(WriteConflict::DayTaken)),
            _ => Err(sqlx::Error::Database(err)),
        },
        Err(err) => Err(err),
    }
}
