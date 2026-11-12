//! SQL of a tenant's node roles (L02-US07). Row-level security scopes every
//! query to the current tenant.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use domain::permissions::{Permission, PermissionSet, RoleInput};
use sqlx::postgres::PgConnection;
use sqlx::types::Json;
use uuid::Uuid;

use crate::pagination::{Keyset, Page, page_from_rows};

/// Names of the unique indexes of labels, one per language.
const LABEL_UNIQUE: [&str; 2] = ["role_tenant_label_fr_idx", "role_tenant_label_en_idx"];

/// A role as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Role {
    pub id: Uuid,
    pub label_translations: BTreeMap<String, String>,
    pub permissions: PermissionSet,
    pub version: i64,
    pub created_at: DateTime<Utc>,
}

impl Role {
    pub fn keyset(&self) -> Keyset {
        Keyset {
            created_at: self.created_at,
            id: self.id,
        }
    }
}

struct Row {
    id: Uuid,
    label: Json<BTreeMap<String, String>>,
    permissions: Vec<String>,
    version: i64,
    created_at: DateTime<Utc>,
}

impl From<Row> for Role {
    fn from(row: Row) -> Self {
        Self {
            id: row.id,
            label_translations: row.label.0,
            // The database only holds catalog identifiers.
            permissions: set_of(&row.permissions),
            version: row.version,
            created_at: row.created_at,
        }
    }
}

fn set_of(ids: &[String]) -> PermissionSet {
    ids.iter().filter_map(|id| Permission::parse(id)).collect()
}

fn ids_of(set: PermissionSet) -> Vec<String> {
    set.iter().map(|p| p.as_str().to_owned()).collect()
}

/// Labels trimmed, empty ones dropped.
fn labels(input: &RoleInput) -> BTreeMap<String, String> {
    input
        .label_translations
        .iter()
        .map(|(locale, label)| (locale.clone(), label.trim().to_owned()))
        .filter(|(_, label)| !label.is_empty())
        .collect()
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    NotFound,
    VersionMismatch,
    /// Another role of the tenant has this label in a language.
    LabelTaken,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

pub async fn insert(conn: &mut PgConnection, id: Uuid, input: &RoleInput) -> WriteResult<Role> {
    let result = sqlx::query_as!(
        Row,
        r#"
        INSERT INTO role (id, tenant_id, label, permissions)
        VALUES ($1, app_current_tenant(), $2, $3)
        RETURNING id, label AS "label: Json<BTreeMap<String, String>>", permissions, version, created_at
        "#,
        id,
        Json(labels(input)) as _,
        &ids_of(input.permissions)
    )
    .fetch_one(conn)
    .await
    .map(Role::from);
    map_unique_violation(result)
}

pub async fn find(conn: &mut PgConnection, id: Uuid) -> Result<Option<Role>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT id, label AS "label: Json<BTreeMap<String, String>>", permissions, version, created_at
        FROM role WHERE id = $1
        "#,
        id
    )
    .fetch_optional(conn)
    .await
    .map(|row| row.map(Role::from))
}

/// One page of roles, in creation order.
pub async fn list(
    conn: &mut PgConnection,
    after: Option<Keyset>,
    limit: usize,
) -> Result<Page<Role>, sqlx::Error> {
    let fetch = i64::try_from(limit).unwrap_or(i64::MAX).saturating_add(1);
    let rows = sqlx::query_as!(
        Row,
        r#"
        SELECT id, label AS "label: Json<BTreeMap<String, String>>", permissions, version, created_at
        FROM role
        WHERE ($1::timestamptz IS NULL OR (created_at, id) > ($1, $2::uuid))
        ORDER BY created_at, id
        LIMIT $3
        "#,
        after.map(|k| k.created_at),
        after.map(|k| k.id),
        fetch
    )
    .fetch_all(conn)
    .await?;
    let items = rows.into_iter().map(Role::from).collect();
    Ok(page_from_rows(items, limit, Role::keyset))
}

/// Replaces the role's labels and permissions, if its version is one of
/// `expected_versions` (`None`: any).
pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    input: &RoleInput,
) -> WriteResult<Role> {
    let result = sqlx::query_as!(
        Row,
        r#"
        UPDATE role SET label = $3, permissions = $4, version = version + 1
        WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))
        RETURNING id, label AS "label: Json<BTreeMap<String, String>>", permissions, version, created_at
        "#,
        id,
        expected_versions,
        Json(labels(input)) as _,
        &ids_of(input.permissions)
    )
    .fetch_optional(&mut *conn)
    .await
    .map(|row| row.map(Role::from));
    match map_unique_violation(result)? {
        Ok(Some(role)) => Ok(Ok(role)),
        Ok(None) => Ok(Err(missing_or_stale(conn, id).await?)),
        Err(conflict) => Ok(Err(conflict)),
    }
}

/// Deletes the role if its version is expected. The caller checks first
/// that no grant uses it.
pub async fn delete(conn: &mut PgConnection, id: Uuid, expected_versions: Option<&[i64]>) -> WriteResult<()> {
    let deleted = sqlx::query!(
        "DELETE FROM role WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))",
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

/// The union of the permissions of `role_ids` (unknown ids count for nothing).
pub async fn permissions_of(
    conn: &mut PgConnection,
    role_ids: &[Uuid],
) -> Result<PermissionSet, sqlx::Error> {
    if role_ids.is_empty() {
        return Ok(PermissionSet::EMPTY);
    }
    let lists = sqlx::query_scalar!("SELECT permissions FROM role WHERE id = ANY($1)", role_ids)
        .fetch_all(conn)
        .await?;
    Ok(domain::permissions::effective(
        lists.iter().map(|ids| set_of(ids)),
    ))
}

async fn missing_or_stale(conn: &mut PgConnection, id: Uuid) -> Result<WriteConflict, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM role WHERE id = $1) AS "exists!""#,
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
        Err(sqlx::Error::Database(err)) if err.constraint().is_some_and(|c| LABEL_UNIQUE.contains(&c)) => {
            Ok(Err(WriteConflict::LabelTaken))
        }
        Err(err) => Err(err),
    }
}
