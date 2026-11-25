//! SQL of absences (L07-US05). Row-level security scopes every query to the
//! current tenant. Writes lock the resource row first, so that two
//! declarations at the same time cannot overlap; each records
//! `resource.absence_changed` in the outbox with the days concerned, for
//! the load engine (L08).

use domain::absences::{Absence, AbsenceKind};
use serde_json::json;
use sqlx::postgres::PgConnection;
use time::Date;
use uuid::Uuid;

use crate::capacity::local_date;
use crate::outbox::{self, Event};

/// Event of a change of a resource's absences.
pub const ABSENCE_CHANGED: &str = "resource.absence_changed";

/// An absence as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAbsence {
    pub id: Uuid,
    pub resource_id: Uuid,
    pub kind: AbsenceKind,
    pub start: Date,
    pub starts_at_noon: bool,
    pub end: Date,
    pub ends_at_noon: bool,
    pub comment: Option<String>,
    /// Who declared it; `None`: the account no longer exists.
    pub declared_by: Option<Uuid>,
    /// Their name, or pseudonym once erased.
    pub declared_by_name: Option<String>,
    pub version: i64,
}

impl StoredAbsence {
    /// The domain's view of it.
    pub fn rule(&self) -> Option<Absence> {
        Some(Absence {
            kind: self.kind,
            start: local_date(self.start)?,
            starts_at_noon: self.starts_at_noon,
            end: local_date(self.end)?,
            ends_at_noon: self.ends_at_noon,
        })
    }
}

/// An absence as written: comment trimmed, empty one absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbsenceInput<'a> {
    pub kind: AbsenceKind,
    pub start: Date,
    pub starts_at_noon: bool,
    pub end: Date,
    pub ends_at_noon: bool,
    pub comment: Option<&'a str>,
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    NotFound,
    VersionMismatch,
    /// Another absence of the resource takes a same half day.
    Overlap,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

struct Row {
    id: Uuid,
    resource_id: Uuid,
    kind: String,
    start_date: Date,
    starts_at_noon: bool,
    end_date: Date,
    ends_at_noon: bool,
    comment: Option<String>,
    declared_by: Option<Uuid>,
    declared_by_name: Option<String>,
    version: i64,
}

impl From<Row> for StoredAbsence {
    fn from(row: Row) -> Self {
        Self {
            id: row.id,
            resource_id: row.resource_id,
            // The database checks the kind.
            kind: AbsenceKind::parse(&row.kind).unwrap_or(AbsenceKind::Other),
            start: row.start_date,
            starts_at_noon: row.starts_at_noon,
            end: row.end_date,
            ends_at_noon: row.ends_at_noon,
            comment: row.comment,
            declared_by: row.declared_by,
            declared_by_name: row.declared_by_name,
            version: row.version,
        }
    }
}

/// The absences of `resource_id` meeting `from`–`to` (both included), by
/// start date.
pub async fn list(
    conn: &mut PgConnection,
    resource_id: Uuid,
    from: Date,
    to: Date,
) -> Result<Vec<StoredAbsence>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT id, resource_id, kind, start_date, starts_at_noon, end_date, ends_at_noon, comment, declared_by,
               app_member_name(declared_by) AS declared_by_name, version
        FROM absence
        WHERE resource_id = $1 AND start_date <= $3 AND end_date >= $2
        ORDER BY start_date, id
        "#,
        resource_id,
        from,
        to
    )
    .fetch_all(conn)
    .await
    .map(|rows| rows.into_iter().map(StoredAbsence::from).collect())
}

async fn find(
    conn: &mut PgConnection,
    resource_id: Uuid,
    id: Uuid,
) -> Result<Option<StoredAbsence>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT id, resource_id, kind, start_date, starts_at_noon, end_date, ends_at_noon, comment, declared_by,
               app_member_name(declared_by) AS declared_by_name, version
        FROM absence
        WHERE id = $1 AND resource_id = $2
        "#,
        id,
        resource_id
    )
    .fetch_optional(conn)
    .await
    .map(|row| row.map(StoredAbsence::from))
}

/// The account of the person `resource_id` is, if linked.
pub async fn person_of(
    conn: &mut PgConnection,
    resource_id: Uuid,
) -> Result<Option<Option<Uuid>>, sqlx::Error> {
    sqlx::query_scalar!("SELECT account_id FROM resource WHERE id = $1", resource_id)
        .fetch_optional(conn)
        .await
}

/// Locks `resource_id` until the end of the transaction; whether it exists.
async fn lock_resource(conn: &mut PgConnection, resource_id: Uuid) -> Result<bool, sqlx::Error> {
    Ok(
        sqlx::query_scalar!("SELECT id FROM resource WHERE id = $1 FOR UPDATE", resource_id)
            .fetch_optional(conn)
            .await?
            .is_some(),
    )
}

/// Whether `input` takes a half day of another absence of the resource.
async fn overlaps(
    conn: &mut PgConnection,
    resource_id: Uuid,
    input: &AbsenceInput<'_>,
    except: Option<Uuid>,
) -> Result<bool, sqlx::Error> {
    let Some(wanted) = rule_of(input) else {
        return Ok(false);
    };
    let others = list(conn, resource_id, input.start, input.end).await?;
    Ok(others
        .iter()
        .filter(|a| Some(a.id) != except)
        .filter_map(StoredAbsence::rule)
        .any(|a| a.overlaps(&wanted)))
}

fn rule_of(input: &AbsenceInput<'_>) -> Option<Absence> {
    Some(Absence {
        kind: input.kind,
        start: local_date(input.start)?,
        starts_at_noon: input.starts_at_noon,
        end: local_date(input.end)?,
        ends_at_noon: input.ends_at_noon,
    })
}

/// Declares the absence `id` of `resource_id`, by `actor`.
pub async fn insert(
    conn: &mut PgConnection,
    id: Uuid,
    resource_id: Uuid,
    input: &AbsenceInput<'_>,
    actor: Option<Uuid>,
) -> WriteResult<StoredAbsence> {
    if !lock_resource(conn, resource_id).await? {
        return Ok(Err(WriteConflict::NotFound));
    }
    if overlaps(conn, resource_id, input, None).await? {
        return Ok(Err(WriteConflict::Overlap));
    }
    sqlx::query!(
        r#"
        INSERT INTO absence (id, tenant_id, resource_id, kind, start_date, starts_at_noon, end_date, ends_at_noon, comment, declared_by)
        VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
        id,
        resource_id,
        input.kind.as_str(),
        input.start,
        input.starts_at_noon,
        input.end,
        input.ends_at_noon,
        input.comment,
        actor
    )
    .execute(&mut *conn)
    .await?;
    changed(conn, resource_id, id, &[(input.start, input.end)], actor).await?;
    Ok(find(conn, resource_id, id).await?.ok_or(WriteConflict::NotFound))
}

/// Replaces the absence `id` of `resource_id`, if its version is one of
/// `expected_versions` (`None`: any). Its declarer becomes `actor`.
pub async fn update(
    conn: &mut PgConnection,
    resource_id: Uuid,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    input: &AbsenceInput<'_>,
    actor: Option<Uuid>,
) -> WriteResult<StoredAbsence> {
    if !lock_resource(conn, resource_id).await? {
        return Ok(Err(WriteConflict::NotFound));
    }
    let Some(before) = find(conn, resource_id, id).await? else {
        return Ok(Err(WriteConflict::NotFound));
    };
    if overlaps(conn, resource_id, input, Some(id)).await? {
        return Ok(Err(WriteConflict::Overlap));
    }
    let updated = sqlx::query_scalar!(
        r#"
        UPDATE absence
        SET kind = $3, start_date = $4, starts_at_noon = $5, end_date = $6, ends_at_noon = $7, comment = $8,
            declared_by = $9, version = version + 1
        WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))
        RETURNING id
        "#,
        id,
        expected_versions,
        input.kind.as_str(),
        input.start,
        input.starts_at_noon,
        input.end,
        input.ends_at_noon,
        input.comment,
        actor
    )
    .fetch_optional(&mut *conn)
    .await?;
    if updated.is_none() {
        return Ok(Err(WriteConflict::VersionMismatch));
    }
    changed(
        conn,
        resource_id,
        id,
        &[(before.start, before.end), (input.start, input.end)],
        actor,
    )
    .await?;
    Ok(find(conn, resource_id, id).await?.ok_or(WriteConflict::NotFound))
}

/// Deletes the absence `id` of `resource_id`, if its version is expected.
pub async fn delete(
    conn: &mut PgConnection,
    resource_id: Uuid,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    actor: Option<Uuid>,
) -> WriteResult<()> {
    let Some(before) = find(conn, resource_id, id).await? else {
        return Ok(Err(WriteConflict::NotFound));
    };
    let deleted = sqlx::query!(
        "DELETE FROM absence WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))",
        id,
        expected_versions
    )
    .execute(&mut *conn)
    .await?;
    if deleted.rows_affected() == 0 {
        return Ok(Err(WriteConflict::VersionMismatch));
    }
    changed(conn, resource_id, id, &[(before.start, before.end)], actor).await?;
    Ok(Ok(()))
}

/// Records that the days of `spans` (first, last) of `resource_id` changed.
async fn changed(
    conn: &mut PgConnection,
    resource_id: Uuid,
    id: Uuid,
    spans: &[(Date, Date)],
    actor: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    let from = spans.iter().map(|(first, _)| *first).min();
    let until = spans.iter().map(|(_, last)| *last).max().and_then(Date::next_day);
    outbox::record(
        conn,
        &Event {
            kind: ABSENCE_CHANGED,
            aggregate_type: "resource",
            aggregate_id: resource_id,
            payload: json!({
                "resourceId": resource_id,
                "absenceId": id,
                "from": from.map(|d| d.to_string()),
                "until": until.map(|d| d.to_string()),
            }),
            actor_id: actor,
        },
    )
    .await?;
    Ok(())
}
