//! SQL of teams (L07-US02). Row-level security scopes every query to the
//! current tenant. The members of a team are listed with the resources
//! (`crate::resources::ResourceFilter::team`).

use std::collections::BTreeMap;

use domain::teams::TeamInput;
use sqlx::postgres::PgConnection;
use sqlx::types::Json;
use uuid::Uuid;

use crate::id::new_id;

/// Names of the unique indexes of names, one per language.
const NAME_UNIQUE: [&str; 2] = ["team_tenant_name_fr_idx", "team_tenant_name_en_idx"];

/// A team.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Team {
    pub id: Uuid,
    pub name_translations: BTreeMap<String, String>,
    pub description: Option<String>,
    /// Resources in it, active or not.
    pub member_count: i64,
    pub version: i64,
}

/// A team a resource belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamRef {
    pub id: Uuid,
    pub name_translations: BTreeMap<String, String>,
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    NotFound,
    VersionMismatch,
    /// Another team of the tenant has this name in a language.
    NameTaken,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

/// What came of adding or removing a member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberChange {
    Done,
    /// Already in, or already out.
    Unchanged,
    NoTeam,
    NoResource,
}

struct Row {
    id: Uuid,
    name: Json<BTreeMap<String, String>>,
    description: Option<String>,
    member_count: i64,
    version: i64,
}

impl From<Row> for Team {
    fn from(row: Row) -> Self {
        Self {
            id: row.id,
            name_translations: row.name.0,
            description: row.description,
            member_count: row.member_count,
            version: row.version,
        }
    }
}

/// Every team of the tenant, in creation order (a tenant has a few dozen).
pub async fn list(conn: &mut PgConnection) -> Result<Vec<Team>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT t.id, t.name AS "name: Json<BTreeMap<String, String>>", t.description, t.version,
               (SELECT count(*) FROM team_member tm WHERE tm.team_id = t.id) AS "member_count!"
        FROM team t
        ORDER BY t.created_at, t.id
        "#
    )
    .fetch_all(conn)
    .await
    .map(|rows| rows.into_iter().map(Team::from).collect())
}

/// The team `id`.
pub async fn find(conn: &mut PgConnection, id: Uuid) -> Result<Option<Team>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT t.id, t.name AS "name: Json<BTreeMap<String, String>>", t.description, t.version,
               (SELECT count(*) FROM team_member tm WHERE tm.team_id = t.id) AS "member_count!"
        FROM team t
        WHERE t.id = $1
        "#,
        id
    )
    .fetch_optional(conn)
    .await
    .map(|row| row.map(Team::from))
}

/// Creates the team `id`.
pub async fn insert(conn: &mut PgConnection, id: Uuid, input: &TeamInput) -> WriteResult<Team> {
    let inserted = sqlx::query!(
        "INSERT INTO team (id, tenant_id, name, description) VALUES ($1, app_current_tenant(), $2, $3)",
        id,
        Json(input.names()) as _,
        input.description()
    )
    .execute(&mut *conn)
    .await;
    if let Err(conflict) = map_unique_violation(inserted)? {
        return Ok(Err(conflict));
    }
    Ok(find(conn, id).await?.ok_or(WriteConflict::NotFound))
}

/// Replaces the team `id`, if its version is one of `expected_versions`
/// (`None`: any).
pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    input: &TeamInput,
) -> WriteResult<Team> {
    let updated = sqlx::query_scalar!(
        r#"
        UPDATE team SET name = $3, description = $4, version = version + 1
        WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))
        RETURNING id
        "#,
        id,
        expected_versions,
        Json(input.names()) as _,
        input.description()
    )
    .fetch_optional(&mut *conn)
    .await;
    match map_unique_violation(updated)? {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(Err(missing_or_stale(conn, id).await?)),
        Err(conflict) => return Ok(Err(conflict)),
    }
    Ok(find(conn, id).await?.ok_or(WriteConflict::NotFound))
}

/// Deletes the team `id` and its memberships, if its version is expected.
pub async fn delete(conn: &mut PgConnection, id: Uuid, expected_versions: Option<&[i64]>) -> WriteResult<()> {
    let deleted = sqlx::query!(
        "DELETE FROM team WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))",
        id,
        expected_versions
    )
    .execute(&mut *conn)
    .await?;
    if deleted.rows_affected() == 1 {
        Ok(Ok(()))
    } else {
        Ok(Err(missing_or_stale(conn, id).await?))
    }
}

/// Puts `resource_id` in the team.
pub async fn add_member(
    conn: &mut PgConnection,
    team_id: Uuid,
    resource_id: Uuid,
) -> Result<MemberChange, sqlx::Error> {
    let exists = sqlx::query!(
        r#"
        SELECT EXISTS (SELECT 1 FROM team WHERE id = $1) AS "team!",
               EXISTS (SELECT 1 FROM resource WHERE id = $2) AS "resource!"
        "#,
        team_id,
        resource_id
    )
    .fetch_one(&mut *conn)
    .await?;
    if !exists.team {
        return Ok(MemberChange::NoTeam);
    }
    if !exists.resource {
        return Ok(MemberChange::NoResource);
    }
    let added = sqlx::query!(
        r#"
        INSERT INTO team_member (id, tenant_id, team_id, resource_id)
        VALUES ($1, app_current_tenant(), $2, $3)
        ON CONFLICT (tenant_id, team_id, resource_id) DO NOTHING
        "#,
        new_id(),
        team_id,
        resource_id
    )
    .execute(&mut *conn)
    .await?;
    if added.rows_affected() == 0 {
        return Ok(MemberChange::Unchanged);
    }
    bump(conn, team_id, resource_id).await?;
    Ok(MemberChange::Done)
}

/// Takes `resource_id` out of the team; its assignments are not touched.
pub async fn remove_member(
    conn: &mut PgConnection,
    team_id: Uuid,
    resource_id: Uuid,
) -> Result<MemberChange, sqlx::Error> {
    let removed = sqlx::query!(
        "DELETE FROM team_member WHERE team_id = $1 AND resource_id = $2",
        team_id,
        resource_id
    )
    .execute(&mut *conn)
    .await?;
    if removed.rows_affected() == 1 {
        bump(conn, team_id, resource_id).await?;
        return Ok(MemberChange::Done);
    }
    let team = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM team WHERE id = $1) AS "exists!""#,
        team_id
    )
    .fetch_one(conn)
    .await?;
    Ok(if team {
        MemberChange::Unchanged
    } else {
        MemberChange::NoTeam
    })
}

/// The teams of each of `resource_ids`, in creation order.
pub async fn of_resources(
    conn: &mut PgConnection,
    resource_ids: &[Uuid],
) -> Result<Vec<(Uuid, TeamRef)>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT tm.resource_id, t.id, t.name AS "name: Json<BTreeMap<String, String>>"
        FROM team_member tm
        JOIN team t ON t.id = tm.team_id
        WHERE tm.resource_id = ANY($1)
        ORDER BY t.created_at, t.id
        "#,
        resource_ids
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.resource_id,
                TeamRef {
                    id: row.id,
                    name_translations: row.name.0,
                },
            )
        })
        .collect())
}

/// A member change changes the versions of the team (its member count) and
/// of the resource (its teams): cached copies of either are stale.
async fn bump(conn: &mut PgConnection, team_id: Uuid, resource_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!("UPDATE team SET version = version + 1 WHERE id = $1", team_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query!(
        "UPDATE resource SET version = version + 1 WHERE id = $1",
        resource_id
    )
    .execute(conn)
    .await?;
    Ok(())
}

async fn missing_or_stale(conn: &mut PgConnection, id: Uuid) -> Result<WriteConflict, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM team WHERE id = $1) AS "exists!""#,
        id
    )
    .fetch_one(conn)
    .await?;
    Ok(if exists {
        WriteConflict::VersionMismatch
    } else {
        WriteConflict::NotFound
    })
}

fn map_unique_violation<T>(result: Result<T, sqlx::Error>) -> WriteResult<T> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(sqlx::Error::Database(err)) if err.constraint().is_some_and(|c| NAME_UNIQUE.contains(&c)) => {
            Ok(Err(WriteConflict::NameTaken))
        }
        Err(err) => Err(err),
    }
}
