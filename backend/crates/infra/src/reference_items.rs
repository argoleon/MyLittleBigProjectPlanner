//! SQL of the reference resource, the template for new repositories.
//!
//! Every function takes the tenant transaction's connection: row-level
//! security scopes every query to the current tenant, so no query filters on
//! `tenant_id`, and inserts take it from `app_current_tenant()`.
//! Queries use `sqlx::query!` macros, checked against the schema at compile
//! time (`just sqlx-prepare` refreshes the offline data after a change).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use domain::reference_item::ReferenceItemInput;
use sqlx::postgres::PgConnection;
use sqlx::types::Json;
use uuid::Uuid;

use crate::pagination::{Keyset, Page, page_from_rows};

/// Name of the unique constraint on `(tenant_id, code)`.
const CODE_UNIQUE: &str = "reference_item_tenant_code_uq";

/// A reference item as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceItem {
    pub id: Uuid,
    pub code: String,
    pub label_translations: BTreeMap<String, String>,
    pub quantity: i32,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Row as returned by SQL; JSON columns are wrapped in `Json`. Converted
/// into [`ReferenceItem`] so callers never see SQL types.
struct Row {
    id: Uuid,
    code: String,
    label: Json<BTreeMap<String, String>>,
    quantity: i32,
    version: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<Row> for ReferenceItem {
    fn from(row: Row) -> Self {
        Self {
            id: row.id,
            code: row.code,
            label_translations: row.label.0,
            quantity: row.quantity,
            version: row.version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

impl ReferenceItem {
    pub fn keyset(&self) -> Keyset {
        Keyset {
            created_at: self.created_at,
            id: self.id,
        }
    }
}

/// Why a write did not happen. Database failures stay in `sqlx::Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    /// No item with this id in the current tenant.
    NotFound,
    /// The item's version is not one of the expected versions (`If-Match`).
    VersionMismatch,
    /// Another item of the tenant already has this code.
    CodeTaken,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

pub async fn insert(
    conn: &mut PgConnection,
    id: Uuid,
    input: &ReferenceItemInput,
) -> WriteResult<ReferenceItem> {
    let result = sqlx::query_as!(
        Row,
        r#"
        INSERT INTO reference_item (id, tenant_id, code, label, quantity)
        VALUES ($1, app_current_tenant(), $2, $3, $4)
        RETURNING id, code, label AS "label: Json<BTreeMap<String, String>>",
                  quantity, version, created_at, updated_at
        "#,
        id,
        input.code,
        Json(&input.label_translations) as _,
        input.quantity
    )
    .fetch_one(conn)
    .await
    .map(ReferenceItem::from);
    map_unique_violation(result)
}

pub async fn find(conn: &mut PgConnection, id: Uuid) -> Result<Option<ReferenceItem>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT id, code, label AS "label: Json<BTreeMap<String, String>>",
               quantity, version, created_at, updated_at
        FROM reference_item
        WHERE id = $1
        "#,
        id
    )
    .fetch_optional(conn)
    .await
    .map(|row| row.map(ReferenceItem::from))
}

/// One page of items, in creation order, after `after` (first page if `None`).
pub async fn list(
    conn: &mut PgConnection,
    after: Option<Keyset>,
    limit: usize,
) -> Result<Page<ReferenceItem>, sqlx::Error> {
    let fetch = i64::try_from(limit).unwrap_or(i64::MAX).saturating_add(1);
    let rows = sqlx::query_as!(
        Row,
        r#"
        SELECT id, code, label AS "label: Json<BTreeMap<String, String>>",
               quantity, version, created_at, updated_at
        FROM reference_item
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
    let items = rows.into_iter().map(ReferenceItem::from).collect();
    Ok(page_from_rows(items, limit, ReferenceItem::keyset))
}

/// Replaces the item's fields if its version is one of `expected_versions`
/// (`None`: any version, `If-Match: *`), and increments the version.
pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    input: &ReferenceItemInput,
) -> WriteResult<ReferenceItem> {
    let result = sqlx::query_as!(
        Row,
        r#"
        UPDATE reference_item
        SET code = $3, label = $4, quantity = $5, version = version + 1
        WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))
        RETURNING id, code, label AS "label: Json<BTreeMap<String, String>>",
                  quantity, version, created_at, updated_at
        "#,
        id,
        expected_versions,
        input.code,
        Json(&input.label_translations) as _,
        input.quantity
    )
    .fetch_optional(&mut *conn)
    .await
    .map(|row| row.map(ReferenceItem::from));
    match map_unique_violation(result)? {
        Ok(Some(item)) => Ok(Ok(item)),
        Ok(None) => Ok(Err(missing_or_stale(conn, id).await?)),
        Err(conflict) => Ok(Err(conflict)),
    }
}

/// Deletes the item if its version is one of `expected_versions` (`None`: any).
pub async fn delete(conn: &mut PgConnection, id: Uuid, expected_versions: Option<&[i64]>) -> WriteResult<()> {
    let deleted = sqlx::query!(
        "DELETE FROM reference_item WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))",
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

/// After a conditional write matched nothing: was the item missing, or stale?
async fn missing_or_stale(conn: &mut PgConnection, id: Uuid) -> Result<WriteConflict, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM reference_item WHERE id = $1) AS "exists!""#,
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
        Err(sqlx::Error::Database(err)) if err.constraint() == Some(CODE_UNIQUE) => {
            Ok(Err(WriteConflict::CodeTaken))
        }
        Err(err) => Err(err),
    }
}
