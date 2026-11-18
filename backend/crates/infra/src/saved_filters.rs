//! SQL of saved filters (L06-US04). Row-level security scopes every query to
//! the current tenant; what a member sees of it is decided here: their own
//! filters, those shared with the whole tenant, and those shared with one of
//! their groups.

use std::collections::HashMap;

use sqlx::postgres::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::groups::GroupRef;
use crate::id::new_id;

/// Name of the unique index on `(tenant_id, owner_id, lower(name))`.
const NAME_UNIQUE: &str = "saved_filter_owner_name_idx";

/// A filter, as a member sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedFilter {
    pub id: Uuid,
    pub name: String,
    pub icon: String,
    pub query: String,
    /// `None`: the account no longer exists.
    pub owner_id: Option<Uuid>,
    /// Their name, or their pseudonym once erased.
    pub owner_name: Option<String>,
    /// Whether the owner is still an active member of the tenant.
    pub owner_is_member: bool,
    pub shared_with_all: bool,
    /// The groups it is shared with, by name.
    pub groups: Vec<GroupRef>,
    pub version: i64,
    pub created_at: OffsetDateTime,
}

impl SavedFilter {
    /// Shared with someone besides its owner.
    pub fn shared(&self) -> bool {
        self.shared_with_all || !self.groups.is_empty()
    }
}

/// A filter as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterInput<'a> {
    pub name: &'a str,
    pub icon: &'a str,
    pub query: &'a str,
    pub shared_with_all: bool,
    pub groups: &'a [Uuid],
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    NotFound,
    VersionMismatch,
    /// The owner has another filter of this name (whatever its case).
    NameTaken,
    /// A group to share with is not one of the tenant's.
    UnknownGroup,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

struct Row {
    id: Uuid,
    name: String,
    icon: String,
    query: String,
    owner_id: Option<Uuid>,
    owner_name: Option<String>,
    owner_is_member: bool,
    shared_with_all: bool,
    version: i64,
    created_at: OffsetDateTime,
}

/// The filters `reader` sees, by name; `only` restricts them to one id.
async fn visible(
    conn: &mut PgConnection,
    reader: Uuid,
    only: Option<Uuid>,
) -> Result<Vec<SavedFilter>, sqlx::Error> {
    let rows = sqlx::query_as!(
        Row,
        r#"
        SELECT f.id, f.name, f.icon, f.query, f.owner_id, f.shared_with_all, f.version, f.created_at,
               app_member_name(f.owner_id) AS owner_name,
               EXISTS (SELECT 1 FROM membership m WHERE m.account_id = f.owner_id AND m.status = 'active')
                   AS "owner_is_member!"
        FROM saved_filter f
        WHERE ($2::uuid IS NULL OR f.id = $2)
          AND (f.owner_id = $1 OR f.shared_with_all OR EXISTS (
                  SELECT 1 FROM saved_filter_group sg
                  JOIN group_member gm ON gm.group_id = sg.group_id AND gm.account_id = $1
                  WHERE sg.filter_id = f.id))
        ORDER BY lower(f.name), f.id
        "#,
        reader,
        only
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let mut groups: HashMap<Uuid, Vec<GroupRef>> = HashMap::new();
    for share in sqlx::query!(
        r#"
        SELECT sg.filter_id, g.id, g.name
        FROM saved_filter_group sg JOIN "group" g ON g.id = sg.group_id
        WHERE sg.filter_id = ANY($1)
        ORDER BY lower(g.name), g.id
        "#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?
    {
        groups.entry(share.filter_id).or_default().push(GroupRef {
            id: share.id,
            name: share.name,
        });
    }
    Ok(rows
        .into_iter()
        .map(|row| SavedFilter {
            groups: groups.remove(&row.id).unwrap_or_default(),
            id: row.id,
            name: row.name,
            icon: row.icon,
            query: row.query,
            owner_id: row.owner_id,
            owner_name: row.owner_name,
            owner_is_member: row.owner_is_member,
            shared_with_all: row.shared_with_all,
            version: row.version,
            created_at: row.created_at,
        })
        .collect())
}

/// The filters `reader` sees, by name.
pub async fn list(conn: &mut PgConnection, reader: Uuid) -> Result<Vec<SavedFilter>, sqlx::Error> {
    visible(conn, reader, None).await
}

/// The filter `id`, if `reader` sees it.
pub async fn find(
    conn: &mut PgConnection,
    reader: Uuid,
    id: Uuid,
) -> Result<Option<SavedFilter>, sqlx::Error> {
    Ok(visible(conn, reader, Some(id)).await?.pop())
}

/// Saves a filter of `owner`.
pub async fn insert(
    conn: &mut PgConnection,
    id: Uuid,
    owner: Uuid,
    input: &FilterInput<'_>,
) -> WriteResult<SavedFilter> {
    if !groups_exist(conn, input.groups).await? {
        return Ok(Err(WriteConflict::UnknownGroup));
    }
    let inserted = sqlx::query!(
        r#"
        INSERT INTO saved_filter (id, tenant_id, owner_id, name, icon, query, shared_with_all)
        VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6)
        "#,
        id,
        owner,
        input.name.trim(),
        input.icon,
        input.query.trim(),
        input.shared_with_all
    )
    .execute(&mut *conn)
    .await;
    if let Err(conflict) = map_unique_violation(inserted)? {
        return Ok(Err(conflict));
    }
    share(conn, id, input.groups).await?;
    Ok(find(conn, owner, id).await?.ok_or(WriteConflict::NotFound))
}

/// Replaces the filter `id` of `owner`, if its version is one of
/// `expected_versions` (`None`: any).
pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    owner: Uuid,
    expected_versions: Option<&[i64]>,
    input: &FilterInput<'_>,
) -> WriteResult<SavedFilter> {
    if !groups_exist(conn, input.groups).await? {
        return Ok(Err(WriteConflict::UnknownGroup));
    }
    let updated = sqlx::query_scalar!(
        r#"
        UPDATE saved_filter
        SET name = $4, icon = $5, query = $6, shared_with_all = $7, version = version + 1
        WHERE id = $1 AND owner_id = $2 AND ($3::bigint[] IS NULL OR version = ANY($3))
        RETURNING id
        "#,
        id,
        owner,
        expected_versions,
        input.name.trim(),
        input.icon,
        input.query.trim(),
        input.shared_with_all
    )
    .fetch_optional(&mut *conn)
    .await;
    match map_unique_violation(updated)? {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(Err(missing_or_stale(conn, id).await?)),
        Err(conflict) => return Ok(Err(conflict)),
    }
    sqlx::query!("DELETE FROM saved_filter_group WHERE filter_id = $1", id)
        .execute(&mut *conn)
        .await?;
    share(conn, id, input.groups).await?;
    Ok(find(conn, owner, id).await?.ok_or(WriteConflict::NotFound))
}

/// Deletes the filter `id`, if its version is expected.
pub async fn delete(conn: &mut PgConnection, id: Uuid, expected_versions: Option<&[i64]>) -> WriteResult<()> {
    let deleted = sqlx::query!(
        "DELETE FROM saved_filter WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))",
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

/// Deletes the filters of `account` shared with nobody: they leave the
/// tenant, or are erased. Returns how many.
pub async fn delete_private_of(conn: &mut PgConnection, account: Uuid) -> Result<u64, sqlx::Error> {
    let deleted = sqlx::query!(
        r#"
        DELETE FROM saved_filter f
        WHERE f.owner_id = $1 AND NOT f.shared_with_all
          AND NOT EXISTS (SELECT 1 FROM saved_filter_group sg WHERE sg.filter_id = f.id)
        "#,
        account
    )
    .execute(conn)
    .await?;
    Ok(deleted.rows_affected())
}

/// The groups of the tenant whose name holds `search`, by name: what a
/// filter can be shared with.
pub async fn share_groups(
    conn: &mut PgConnection,
    search: Option<&str>,
    limit: usize,
) -> Result<Vec<GroupRef>, sqlx::Error> {
    let pattern = search.map(crate::invitations::like_pattern);
    sqlx::query_as!(
        GroupRef,
        r#"
        SELECT id, name FROM "group"
        WHERE $1::text IS NULL OR lower(name) LIKE $1
        ORDER BY lower(name), id
        LIMIT $2
        "#,
        pattern,
        i64::try_from(limit).unwrap_or(i64::MAX)
    )
    .fetch_all(conn)
    .await
}

async fn groups_exist(conn: &mut PgConnection, groups: &[Uuid]) -> Result<bool, sqlx::Error> {
    if groups.is_empty() {
        return Ok(true);
    }
    let mut distinct = groups.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    let found = sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!" FROM "group" WHERE id = ANY($1)"#,
        &distinct
    )
    .fetch_one(conn)
    .await?;
    Ok(usize::try_from(found).is_ok_and(|found| found == distinct.len()))
}

async fn share(conn: &mut PgConnection, filter: Uuid, groups: &[Uuid]) -> Result<(), sqlx::Error> {
    let mut distinct = groups.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    let ids: Vec<Uuid> = distinct.iter().map(|_| new_id()).collect();
    sqlx::query!(
        r#"
        INSERT INTO saved_filter_group (id, tenant_id, filter_id, group_id)
        SELECT s.id, app_current_tenant(), $1, s.group_id FROM unnest($2::uuid[], $3::uuid[]) AS s(id, group_id)
        "#,
        filter,
        &ids,
        &distinct
    )
    .execute(conn)
    .await?;
    Ok(())
}

async fn missing_or_stale(conn: &mut PgConnection, id: Uuid) -> Result<WriteConflict, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM saved_filter WHERE id = $1) AS "exists!""#,
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
