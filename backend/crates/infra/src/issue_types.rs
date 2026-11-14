//! SQL of the issue types of a configuration version (L04-US02). Row-level
//! security scopes every query to the current tenant.
//!
//! A type is addressed by its `item_id`, which it keeps from one version to
//! the next; its row `id` changes with each copy.

use std::collections::BTreeMap;

use domain::issue_types::TypeNode;
use sqlx::postgres::PgConnection;
use sqlx::types::Json;
use uuid::Uuid;

/// A type as stored in a version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueType {
    /// Its identity across versions.
    pub item_id: Uuid,
    pub key: String,
    pub label_translations: BTreeMap<String, String>,
    pub icon: String,
    pub color: String,
    pub is_subtask: bool,
    /// Its allowed parents, by `item_id`.
    pub parent_ids: Vec<Uuid>,
    /// The workflow its tickets follow (L04-US04), by `item_id`.
    pub workflow_id: Option<Uuid>,
    pub position: i32,
    pub version: i64,
}

impl IssueType {
    /// Its place in the hierarchy.
    pub fn node(&self) -> TypeNode {
        TypeNode {
            id: self.item_id.as_u128(),
            is_subtask: self.is_subtask,
            parents: self.parent_ids.iter().map(Uuid::as_u128).collect(),
        }
    }
}

struct Row {
    item_id: Uuid,
    key: String,
    label: Json<BTreeMap<String, String>>,
    icon: String,
    color: String,
    is_subtask: bool,
    parent_ids: Vec<Uuid>,
    workflow_id: Option<Uuid>,
    position: i32,
    version: i64,
}

impl From<Row> for IssueType {
    fn from(row: Row) -> Self {
        Self {
            item_id: row.item_id,
            key: row.key,
            label_translations: row.label.0,
            icon: row.icon,
            color: row.color,
            is_subtask: row.is_subtask,
            parent_ids: row.parent_ids,
            workflow_id: row.workflow_id,
            position: row.position,
            version: row.version,
        }
    }
}

/// What a type is made of; its key is set once, when it is added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeFields<'a> {
    pub label_translations: &'a BTreeMap<String, String>,
    pub icon: &'a str,
    pub color: &'a str,
    pub is_subtask: bool,
    pub parent_ids: &'a [Uuid],
    /// The workflow its tickets follow, by `item_id`.
    pub workflow_id: Uuid,
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    NotFound,
    VersionMismatch,
    /// Another type of the version has this key.
    KeyTaken,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

/// Labels trimmed, empty ones dropped.
fn trimmed(labels: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    labels
        .iter()
        .map(|(locale, label)| (locale.clone(), label.trim().to_owned()))
        .filter(|(_, label)| !label.is_empty())
        .collect()
}

/// The types of `version`, in their order.
pub async fn list(conn: &mut PgConnection, version: Uuid) -> Result<Vec<IssueType>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT item_id, key, label AS "label: Json<BTreeMap<String, String>>", icon, color, is_subtask,
               parent_ids, workflow_id, position, version
        FROM issue_type WHERE version_id = $1
        ORDER BY position, key
        "#,
        version
    )
    .fetch_all(conn)
    .await
    .map(|rows| rows.into_iter().map(IssueType::from).collect())
}

/// The type `item` of `version`.
pub async fn find(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
) -> Result<Option<IssueType>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT item_id, key, label AS "label: Json<BTreeMap<String, String>>", icon, color, is_subtask,
               parent_ids, workflow_id, position, version
        FROM issue_type WHERE version_id = $1 AND item_id = $2
        "#,
        version,
        item
    )
    .fetch_optional(conn)
    .await
    .map(|row| row.map(IssueType::from))
}

/// Adds the type `item` to `version`, last.
pub async fn insert(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
    key: &str,
    fields: &TypeFields<'_>,
) -> WriteResult<IssueType> {
    let inserted = sqlx::query!(
        r#"
        INSERT INTO issue_type
            (id, tenant_id, version_id, item_id, key, label, icon, color, is_subtask, position, parent_ids, workflow_id)
        SELECT $1, app_current_tenant(), $2, $3, $4, $5, $6, $7, $8,
               coalesce((SELECT max(position) + 1 FROM issue_type WHERE version_id = $2), 0), $9, $10
        "#,
        crate::id::new_id(),
        version,
        item,
        key,
        Json(trimmed(fields.label_translations)) as _,
        fields.icon,
        fields.color,
        fields.is_subtask,
        fields.parent_ids,
        fields.workflow_id
    )
    .execute(&mut *conn)
    .await;
    match inserted {
        Ok(_) => Ok(find(conn, version, item).await?.ok_or(WriteConflict::NotFound)),
        Err(sqlx::Error::Database(err)) if err.constraint() == Some("issue_type_key_unique") => {
            Ok(Err(WriteConflict::KeyTaken))
        }
        Err(err) => Err(err),
    }
}

/// Changes the type `item` of `version`, if its version is one of
/// `expected_versions` (`None`: any).
pub async fn update(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
    expected_versions: Option<&[i64]>,
    fields: &TypeFields<'_>,
) -> WriteResult<IssueType> {
    let updated = sqlx::query_scalar!(
        r#"
        UPDATE issue_type
        SET label = $4, icon = $5, color = $6, is_subtask = $7, parent_ids = $8, workflow_id = $9, version = version + 1
        WHERE version_id = $1 AND item_id = $2 AND ($3::bigint[] IS NULL OR version = ANY($3))
        RETURNING item_id
        "#,
        version,
        item,
        expected_versions,
        Json(trimmed(fields.label_translations)) as _,
        fields.icon,
        fields.color,
        fields.is_subtask,
        fields.parent_ids,
        fields.workflow_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    if updated.is_none() {
        return Ok(Err(missing_or_stale(conn, version, item).await?));
    }
    Ok(find(conn, version, item).await?.ok_or(WriteConflict::NotFound))
}

/// Removes the type `item` from `version`, if its version is expected, and
/// from the allowed parents of the others.
pub async fn delete(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
    expected_versions: Option<&[i64]>,
) -> WriteResult<()> {
    let deleted = sqlx::query!(
        "DELETE FROM issue_type WHERE version_id = $1 AND item_id = $2 AND ($3::bigint[] IS NULL OR version = ANY($3))",
        version,
        item,
        expected_versions
    )
    .execute(&mut *conn)
    .await?;
    if deleted.rows_affected() == 0 {
        return Ok(Err(missing_or_stale(conn, version, item).await?));
    }
    sqlx::query!(
        r#"
        UPDATE issue_type SET parent_ids = array_remove(parent_ids, $2), version = version + 1
        WHERE version_id = $1 AND $2 = ANY(parent_ids)
        "#,
        version,
        item
    )
    .execute(&mut *conn)
    .await?;
    Ok(Ok(()))
}

/// Puts the types of `version` in the order of `items` (every type once:
/// the caller checks it); the ones that moved.
pub async fn reorder(
    conn: &mut PgConnection,
    version: Uuid,
    items: &[Uuid],
) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        UPDATE issue_type t SET position = o.position - 1, version = t.version + 1
        FROM unnest($2::uuid[]) WITH ORDINALITY AS o (item_id, position)
        WHERE t.version_id = $1 AND t.item_id = o.item_id AND t.position <> o.position - 1
        RETURNING t.item_id
        "#,
        version,
        items
    )
    .fetch_all(conn)
    .await
}

/// Copies the types of the version `from` into `to`, keeping their
/// identity, order and hierarchy.
pub async fn copy(conn: &mut PgConnection, from: Uuid, to: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO issue_type
            (id, tenant_id, version_id, item_id, key, label, icon, color, is_subtask, position, parent_ids, workflow_id)
        SELECT uuidv7(), tenant_id, $2, item_id, key, label, icon, color, is_subtask, position, parent_ids, workflow_id
        FROM issue_type WHERE version_id = $1
        "#,
        from,
        to
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Gives `version` the default types (`app_seed_default_issue_types`).
pub async fn seed_defaults(conn: &mut PgConnection, version: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!("SELECT app_seed_default_issue_types($1)", version)
        .execute(conn)
        .await?;
    Ok(())
}

/// The types of `published` that `draft` no longer has.
pub async fn removed(
    conn: &mut PgConnection,
    published: Uuid,
    draft: Uuid,
) -> Result<Vec<IssueType>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT item_id, key, label AS "label: Json<BTreeMap<String, String>>", icon, color, is_subtask,
               parent_ids, workflow_id, position, version
        FROM issue_type p
        WHERE p.version_id = $1
          AND NOT EXISTS (SELECT 1 FROM issue_type d WHERE d.version_id = $2 AND d.item_id = p.item_id)
        ORDER BY position, key
        "#,
        published,
        draft
    )
    .fetch_all(conn)
    .await
    .map(|rows| rows.into_iter().map(IssueType::from).collect())
}

async fn missing_or_stale(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
) -> Result<WriteConflict, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM issue_type WHERE version_id = $1 AND item_id = $2) AS "exists!""#,
        version,
        item
    )
    .fetch_one(conn)
    .await?;
    Ok(if exists {
        WriteConflict::VersionMismatch
    } else {
        WriteConflict::NotFound
    })
}
