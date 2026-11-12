//! SQL of a tenant's groups of members (L02-US06). Row-level security
//! scopes every query to the current tenant.

use chrono::{DateTime, Utc};
use domain::members::GroupInput;
use sqlx::postgres::PgConnection;
use uuid::Uuid;

use crate::id::new_id;
use crate::pagination::{Keyset, Page, page_from_rows};

/// Name of the unique index on `(tenant_id, lower(name))`.
const NAME_UNIQUE: &str = "group_tenant_name_idx";

/// A group as stored, with its number of members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub member_count: i64,
    pub version: i64,
    pub created_at: DateTime<Utc>,
}

impl Group {
    pub fn keyset(&self) -> Keyset {
        Keyset {
            created_at: self.created_at,
            id: self.id,
        }
    }
}

/// A member of a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupMember {
    pub account_id: Uuid,
    pub email: String,
    pub display_name: String,
}

/// A group a member belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRef {
    pub id: Uuid,
    pub name: String,
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    NotFound,
    VersionMismatch,
    /// Another group of the tenant has this name (whatever its case).
    NameTaken,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

fn description(input: &GroupInput) -> Option<String> {
    input
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_owned)
}

pub async fn insert(conn: &mut PgConnection, id: Uuid, input: &GroupInput) -> WriteResult<Group> {
    let result = sqlx::query_as!(
        Group,
        r#"
        INSERT INTO "group" (id, tenant_id, name, description)
        VALUES ($1, app_current_tenant(), $2, $3)
        RETURNING id, name, description, 0::bigint AS "member_count!", version, created_at
        "#,
        id,
        input.name.trim(),
        description(input)
    )
    .fetch_one(conn)
    .await;
    map_unique_violation(result)
}

pub async fn find(conn: &mut PgConnection, id: Uuid) -> Result<Option<Group>, sqlx::Error> {
    sqlx::query_as!(
        Group,
        r#"
        SELECT g.id, g.name, g.description, g.version, g.created_at,
               (SELECT count(*) FROM group_member gm WHERE gm.group_id = g.id) AS "member_count!"
        FROM "group" g
        WHERE g.id = $1
        "#,
        id
    )
    .fetch_optional(conn)
    .await
}

/// One page of groups, in creation order.
pub async fn list(
    conn: &mut PgConnection,
    after: Option<Keyset>,
    limit: usize,
) -> Result<Page<Group>, sqlx::Error> {
    let fetch = i64::try_from(limit).unwrap_or(i64::MAX).saturating_add(1);
    let rows = sqlx::query_as!(
        Group,
        r#"
        SELECT g.id, g.name, g.description, g.version, g.created_at,
               (SELECT count(*) FROM group_member gm WHERE gm.group_id = g.id) AS "member_count!"
        FROM "group" g
        WHERE ($1::timestamptz IS NULL OR (g.created_at, g.id) > ($1, $2::uuid))
        ORDER BY g.created_at, g.id
        LIMIT $3
        "#,
        after.map(|k| k.created_at),
        after.map(|k| k.id),
        fetch
    )
    .fetch_all(conn)
    .await?;
    Ok(page_from_rows(rows, limit, Group::keyset))
}

/// Renames the group and replaces its description, if its version is one
/// of `expected_versions` (`None`: any).
pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    input: &GroupInput,
) -> WriteResult<Group> {
    let result = sqlx::query_scalar!(
        r#"
        UPDATE "group" SET name = $3, description = $4, version = version + 1
        WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))
        RETURNING id
        "#,
        id,
        expected_versions,
        input.name.trim(),
        description(input)
    )
    .fetch_optional(&mut *conn)
    .await;
    match map_unique_violation(result)? {
        Ok(Some(_)) => Ok(find(conn, id).await?.ok_or(WriteConflict::NotFound)),
        Ok(None) => Ok(Err(missing_or_stale(conn, id).await?)),
        Err(conflict) => Ok(Err(conflict)),
    }
}

/// Deletes the group and its memberships, if its version is expected.
pub async fn delete(conn: &mut PgConnection, id: Uuid, expected_versions: Option<&[i64]>) -> WriteResult<()> {
    let deleted = sqlx::query!(
        r#"DELETE FROM "group" WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))"#,
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

/// The members of the group, by name.
pub async fn members(conn: &mut PgConnection, group_id: Uuid) -> Result<Vec<GroupMember>, sqlx::Error> {
    sqlx::query_as!(
        GroupMember,
        r#"
        SELECT a.id AS account_id, a.email, a.display_name
        FROM group_member gm
        JOIN account a ON a.id = gm.account_id
        WHERE gm.group_id = $1
        ORDER BY lower(a.display_name), a.id
        "#,
        group_id
    )
    .fetch_all(conn)
    .await
}

/// What adding or removing a member did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberChange {
    /// Added, or removed.
    Done,
    /// Already in the group, or not in it: nothing changed.
    Unchanged,
    /// No such group in the tenant.
    NoGroup,
    /// No such member in the tenant.
    NoMember,
}

/// Puts `account_id`, a member of the tenant, in the group.
pub async fn add_member(
    conn: &mut PgConnection,
    group_id: Uuid,
    account_id: Uuid,
) -> Result<MemberChange, sqlx::Error> {
    let exists = sqlx::query!(
        r#"
        SELECT EXISTS (SELECT 1 FROM "group" WHERE id = $1) AS "group!",
               EXISTS (SELECT 1 FROM membership WHERE account_id = $2) AS "member!"
        "#,
        group_id,
        account_id
    )
    .fetch_one(&mut *conn)
    .await?;
    if !exists.group {
        return Ok(MemberChange::NoGroup);
    }
    if !exists.member {
        return Ok(MemberChange::NoMember);
    }
    let added = sqlx::query!(
        r#"
        INSERT INTO group_member (id, tenant_id, group_id, account_id)
        VALUES ($1, app_current_tenant(), $2, $3)
        ON CONFLICT (tenant_id, group_id, account_id) DO NOTHING
        "#,
        new_id(),
        group_id,
        account_id
    )
    .execute(&mut *conn)
    .await?;
    if added.rows_affected() == 0 {
        return Ok(MemberChange::Unchanged);
    }
    bump(conn, group_id).await?;
    Ok(MemberChange::Done)
}

/// Takes `account_id` out of the group.
pub async fn remove_member(
    conn: &mut PgConnection,
    group_id: Uuid,
    account_id: Uuid,
) -> Result<MemberChange, sqlx::Error> {
    let removed = sqlx::query!(
        "DELETE FROM group_member WHERE group_id = $1 AND account_id = $2",
        group_id,
        account_id
    )
    .execute(&mut *conn)
    .await?;
    if removed.rows_affected() == 1 {
        bump(conn, group_id).await?;
        return Ok(MemberChange::Done);
    }
    let group = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM "group" WHERE id = $1) AS "exists!""#,
        group_id
    )
    .fetch_one(conn)
    .await?;
    Ok(if group {
        MemberChange::Unchanged
    } else {
        MemberChange::NoGroup
    })
}

/// The groups of each of `account_ids`, by group name.
pub async fn of_members(
    conn: &mut PgConnection,
    account_ids: &[Uuid],
) -> Result<Vec<(Uuid, GroupRef)>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT gm.account_id, g.id, g.name
        FROM group_member gm
        JOIN "group" g ON g.id = gm.group_id
        WHERE gm.account_id = ANY($1)
        ORDER BY lower(g.name), g.id
        "#,
        account_ids
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.account_id,
                GroupRef {
                    id: row.id,
                    name: row.name,
                },
            )
        })
        .collect())
}

/// A new version: the members changed.
async fn bump(conn: &mut PgConnection, group_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"UPDATE "group" SET version = version + 1 WHERE id = $1"#,
        group_id
    )
    .execute(conn)
    .await?;
    Ok(())
}

async fn missing_or_stale(conn: &mut PgConnection, id: Uuid) -> Result<WriteConflict, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM "group" WHERE id = $1) AS "exists!""#,
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
        Err(sqlx::Error::Database(err)) if err.constraint() == Some(NAME_UNIQUE) => {
            Ok(Err(WriteConflict::NameTaken))
        }
        Err(err) => Err(err),
    }
}
