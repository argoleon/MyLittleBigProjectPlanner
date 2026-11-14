//! SQL of the screens of a configuration version (L04-US06). Row-level
//! security scopes every query to the current tenant.
//!
//! A screen is addressed by its `item_id`, which it keeps from one version
//! to the next; so are the type and the fields it names.

use std::collections::BTreeMap;

use domain::screens::Kind;
use sqlx::postgres::PgConnection;
use sqlx::types::Json;
use uuid::Uuid;

type Labels = BTreeMap<String, String>;

/// A screen as stored in a version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub item_id: Uuid,
    pub kind: Kind,
    /// The type it is for; none: general (creation and view), or a
    /// transition screen.
    pub issue_type_id: Option<Uuid>,
    pub name_translations: Labels,
    /// Its fields, in their order.
    pub field_ids: Vec<Uuid>,
    pub position: i32,
    pub version: i64,
}

impl Screen {
    pub fn domain(&self) -> domain::screens::Screen {
        domain::screens::Screen {
            id: self.item_id.as_u128(),
            kind: self.kind,
            issue_type: self.issue_type_id.map(|t| t.as_u128()),
            fields: self.field_ids.iter().map(Uuid::as_u128).collect(),
        }
    }
}

struct Row {
    item_id: Uuid,
    kind: String,
    issue_type_id: Option<Uuid>,
    name: Json<Labels>,
    field_ids: Vec<Uuid>,
    position: i32,
    version: i64,
}

impl From<Row> for Screen {
    fn from(row: Row) -> Self {
        Self {
            item_id: row.item_id,
            // The database only holds the kinds of the domain.
            kind: Kind::parse(&row.kind).unwrap_or(Kind::Transition),
            issue_type_id: row.issue_type_id,
            name_translations: row.name.0,
            field_ids: row.field_ids,
            position: row.position,
            version: row.version,
        }
    }
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    NotFound,
    VersionMismatch,
    /// The type (or the configuration) already has a screen of this kind.
    Exists,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

/// Labels trimmed, empty ones dropped.
fn trimmed(labels: &Labels) -> Labels {
    labels
        .iter()
        .map(|(locale, label)| (locale.clone(), label.trim().to_owned()))
        .filter(|(_, label)| !label.is_empty())
        .collect()
}

/// The screens of `version`: creation, view, then transition screens, each
/// in their order.
pub async fn list(conn: &mut PgConnection, version: Uuid) -> Result<Vec<Screen>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT s.item_id, s.kind, s.issue_type_id, s.name AS "name: Json<Labels>", s.field_ids, s.position, s.version
        FROM screen s
        LEFT JOIN issue_type t ON t.version_id = s.version_id AND t.item_id = s.issue_type_id
        WHERE s.version_id = $1
        ORDER BY array_position(ARRAY['creation', 'view', 'transition'], s.kind),
                 s.issue_type_id IS NOT NULL, t.position, s.position, s.item_id
        "#,
        version
    )
    .fetch_all(conn)
    .await
    .map(|rows| rows.into_iter().map(Screen::from).collect())
}

/// The screen `item` of `version`.
pub async fn find(conn: &mut PgConnection, version: Uuid, item: Uuid) -> Result<Option<Screen>, sqlx::Error> {
    Ok(list(conn, version).await?.into_iter().find(|s| s.item_id == item))
}

/// Adds the screen `item` to `version`, last.
pub async fn insert(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
    kind: Kind,
    issue_type_id: Option<Uuid>,
    name: &Labels,
    field_ids: &[Uuid],
) -> WriteResult<()> {
    let inserted = sqlx::query!(
        r#"
        INSERT INTO screen (id, tenant_id, version_id, item_id, kind, issue_type_id, name, field_ids, position)
        SELECT $1, app_current_tenant(), $2, $3, $4, $5, $6, $7,
               coalesce((SELECT max(position) + 1 FROM screen WHERE version_id = $2), 0)
        "#,
        crate::id::new_id(),
        version,
        item,
        kind.as_str(),
        issue_type_id,
        Json(trimmed(name)) as _,
        field_ids
    )
    .execute(conn)
    .await;
    match inserted {
        Ok(_) => Ok(Ok(())),
        Err(sqlx::Error::Database(err)) if err.constraint() == Some("screen_kind_type_unique") => {
            Ok(Err(WriteConflict::Exists))
        }
        Err(err) => Err(err),
    }
}

/// Renames the screen `item` and gives it `field_ids`, if its version is
/// one of `expected_versions` (`None`: any).
pub async fn update(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
    expected_versions: Option<&[i64]>,
    name: &Labels,
    field_ids: &[Uuid],
) -> WriteResult<()> {
    let updated = sqlx::query!(
        r#"
        UPDATE screen SET name = $4, field_ids = $5, version = version + 1
        WHERE version_id = $1 AND item_id = $2 AND ($3::bigint[] IS NULL OR version = ANY($3))
        "#,
        version,
        item,
        expected_versions,
        Json(trimmed(name)) as _,
        field_ids
    )
    .execute(&mut *conn)
    .await?;
    if updated.rows_affected() == 0 {
        return Ok(Err(missing_or_stale(conn, version, item).await?));
    }
    Ok(Ok(()))
}

/// Removes the screen `item`, if its version is expected. The caller checks
/// that no transition uses it.
pub async fn delete(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
    expected_versions: Option<&[i64]>,
) -> WriteResult<()> {
    let deleted = sqlx::query!(
        "DELETE FROM screen WHERE version_id = $1 AND item_id = $2 AND ($3::bigint[] IS NULL OR version = ANY($3))",
        version,
        item,
        expected_versions
    )
    .execute(&mut *conn)
    .await?;
    if deleted.rows_affected() == 0 {
        return Ok(Err(missing_or_stale(conn, version, item).await?));
    }
    Ok(Ok(()))
}

/// Takes the field `field`, removed from `version`, off its screens; the
/// screens that changed.
pub async fn forget_field(
    conn: &mut PgConnection,
    version: Uuid,
    field: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        UPDATE screen SET field_ids = array_remove(field_ids, $2), version = version + 1
        WHERE version_id = $1 AND $2 = ANY(field_ids)
        RETURNING item_id
        "#,
        version,
        field
    )
    .fetch_all(conn)
    .await
}

/// Puts the new field `field` at the end of the general creation and view
/// screens of `version`; the screens that changed.
pub async fn append_field(
    conn: &mut PgConnection,
    version: Uuid,
    field: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        UPDATE screen SET field_ids = array_append(field_ids, $2), version = version + 1
        WHERE version_id = $1 AND kind IN ('creation', 'view') AND issue_type_id IS NULL
          AND NOT $2 = ANY(field_ids)
        RETURNING item_id
        "#,
        version,
        field
    )
    .fetch_all(conn)
    .await
}

/// Copies the screens of the version `from` into `to`, keeping their
/// identities. The types and fields go first.
pub async fn copy(conn: &mut PgConnection, from: Uuid, to: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO screen (id, tenant_id, version_id, item_id, kind, issue_type_id, name, field_ids, position)
        SELECT uuidv7(), tenant_id, $2, item_id, kind, issue_type_id, name, field_ids, position
        FROM screen WHERE version_id = $1
        "#,
        from,
        to
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Gives `version` the general screens (`app_seed_default_screens`), after
/// its fields.
pub async fn seed_defaults(conn: &mut PgConnection, version: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!("SELECT app_seed_default_screens($1)", version)
        .execute(conn)
        .await?;
    Ok(())
}

async fn missing_or_stale(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
) -> Result<WriteConflict, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM screen WHERE version_id = $1 AND item_id = $2) AS "exists!""#,
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
