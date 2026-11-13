//! SQL of project configurations (L04-US01). Row-level security scopes
//! every query to the current tenant.
//!
//! What a configuration holds lives in versions: one published, used by its
//! projects, and at most one draft, edited by its admins. [`open_draft`]
//! copies the published version into a new draft, [`record_change`] counts
//! what the draft changed, and [`publish`] swaps them, in the caller's
//! transaction: a failure leaves the published version as it was.
//!
//! The content tables of L04-US02 onwards (types, fields, workflows,
//! screens, notifications) hang on `project_config_version`; they copy their
//! rows in [`copy_content`].

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use domain::configuration::Item;
use sqlx::postgres::PgConnection;
use sqlx::types::Json;
use uuid::Uuid;

use crate::pagination::{Keyset, Page, page_from_rows};

/// Names of the unique indexes of names, one per language.
const NAME_UNIQUE: [&str; 2] = [
    "project_config_tenant_name_fr_idx",
    "project_config_tenant_name_en_idx",
];

/// The published version of a configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub id: Uuid,
    pub number: i32,
    pub published_at: DateTime<Utc>,
}

/// The draft of a configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub id: Uuid,
    pub number: i32,
    /// Items it changed, each once.
    pub change_count: i64,
    pub created_at: DateTime<Utc>,
}

/// A configuration as stored, with its versions and the number of projects
/// using it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub id: Uuid,
    pub name_translations: BTreeMap<String, String>,
    pub is_default: bool,
    pub project_count: i64,
    pub published: Published,
    pub draft: Option<Draft>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
}

impl Config {
    pub fn keyset(&self) -> Keyset {
        Keyset {
            created_at: self.created_at,
            id: self.id,
        }
    }
}

struct Row {
    id: Uuid,
    name: Json<BTreeMap<String, String>>,
    is_default: bool,
    project_count: i64,
    published_id: Uuid,
    published_number: i32,
    published_at: DateTime<Utc>,
    draft_id: Option<Uuid>,
    draft_number: Option<i32>,
    draft_changes: i64,
    draft_created_at: Option<DateTime<Utc>>,
    version: i64,
    created_at: DateTime<Utc>,
}

impl From<Row> for Config {
    fn from(row: Row) -> Self {
        let draft = match (row.draft_id, row.draft_number, row.draft_created_at) {
            (Some(id), Some(number), Some(created_at)) => Some(Draft {
                id,
                number,
                change_count: row.draft_changes,
                created_at,
            }),
            _ => None,
        };
        Self {
            id: row.id,
            name_translations: row.name.0,
            is_default: row.is_default,
            project_count: row.project_count,
            published: Published {
                id: row.published_id,
                number: row.published_number,
                published_at: row.published_at,
            },
            draft,
            version: row.version,
            created_at: row.created_at,
        }
    }
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    NotFound,
    VersionMismatch,
    /// Another configuration of the tenant has this name in a language.
    NameTaken,
    /// The configuration has no draft to publish or discard.
    NoDraft,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

/// Names trimmed, empty ones dropped.
fn trimmed(names: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    names
        .iter()
        .map(|(locale, name)| (locale.clone(), name.trim().to_owned()))
        .filter(|(_, name)| !name.is_empty())
        .collect()
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

/// The configuration `id`.
pub async fn find(conn: &mut PgConnection, id: Uuid) -> Result<Option<Config>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT c.id, c.name AS "name: Json<BTreeMap<String, String>>", c.is_default,
               (SELECT count(*) FROM node n WHERE n.config_id = c.id) AS "project_count!",
               p.id AS published_id, p.number AS published_number,
               p.published_at AS "published_at!",
               d.id AS "draft_id?", d.number AS "draft_number?", d.created_at AS "draft_created_at?",
               (SELECT count(*) FROM project_config_change ch WHERE ch.version_id = d.id) AS "draft_changes!",
               c.version, c.created_at
        FROM project_config c
        JOIN project_config_version p ON p.config_id = c.id AND p.state = 'published'
        LEFT JOIN project_config_version d ON d.config_id = c.id AND d.state = 'draft'
        WHERE c.id = $1
        "#,
        id
    )
    .fetch_optional(conn)
    .await
    .map(|row| row.map(Config::from))
}

/// One page of configurations, in creation order: the default one first.
pub async fn list(
    conn: &mut PgConnection,
    after: Option<Keyset>,
    limit: usize,
) -> Result<Page<Config>, sqlx::Error> {
    let fetch = i64::try_from(limit).unwrap_or(i64::MAX).saturating_add(1);
    let rows = sqlx::query_as!(
        Row,
        r#"
        SELECT c.id, c.name AS "name: Json<BTreeMap<String, String>>", c.is_default,
               (SELECT count(*) FROM node n WHERE n.config_id = c.id) AS "project_count!",
               p.id AS published_id, p.number AS published_number,
               p.published_at AS "published_at!",
               d.id AS "draft_id?", d.number AS "draft_number?", d.created_at AS "draft_created_at?",
               (SELECT count(*) FROM project_config_change ch WHERE ch.version_id = d.id) AS "draft_changes!",
               c.version, c.created_at
        FROM project_config c
        JOIN project_config_version p ON p.config_id = c.id AND p.state = 'published'
        LEFT JOIN project_config_version d ON d.config_id = c.id AND d.state = 'draft'
        WHERE ($1::timestamptz IS NULL OR (c.created_at, c.id) > ($1, $2::uuid))
        ORDER BY c.created_at, c.id
        LIMIT $3
        "#,
        after.map(|k| k.created_at),
        after.map(|k| k.id),
        fetch
    )
    .fetch_all(conn)
    .await?;
    let items = rows.into_iter().map(Config::from).collect();
    Ok(page_from_rows(items, limit, Config::keyset))
}

/// The default configuration of the tenant, which new projects get.
pub async fn default_id(conn: &mut PgConnection) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar!("SELECT id FROM project_config WHERE is_default")
        .fetch_optional(conn)
        .await
}

/// Creates a configuration named `name`, with a published version 1 that
/// holds the default content (L04-US02 onwards: the default types).
pub async fn insert(
    conn: &mut PgConnection,
    id: Uuid,
    name: &BTreeMap<String, String>,
    created_by: Option<Uuid>,
) -> WriteResult<Config> {
    let config = match insert_empty(conn, id, name, created_by).await? {
        Ok(config) => config,
        Err(conflict) => return Ok(Err(conflict)),
    };
    crate::issue_types::seed_defaults(conn, config.published.id).await?;
    found(conn, id).await
}

/// A configuration named `name`, with an empty published version 1.
async fn insert_empty(
    conn: &mut PgConnection,
    id: Uuid,
    name: &BTreeMap<String, String>,
    created_by: Option<Uuid>,
) -> WriteResult<Config> {
    let inserted = map_unique_violation(
        sqlx::query!(
            "INSERT INTO project_config (id, tenant_id, name) VALUES ($1, app_current_tenant(), $2)",
            id,
            Json(trimmed(name)) as _
        )
        .execute(&mut *conn)
        .await,
    )?;
    if let Err(conflict) = inserted {
        return Ok(Err(conflict));
    }
    sqlx::query!(
        r#"
        INSERT INTO project_config_version (id, tenant_id, config_id, number, state, created_by, published_at)
        VALUES ($1, app_current_tenant(), $2, 1, 'published', $3, now())
        "#,
        crate::id::new_id(),
        id,
        created_by
    )
    .execute(&mut *conn)
    .await?;
    found(conn, id).await
}

/// Renames the configuration, if its version is one of `expected_versions`
/// (`None`: any).
pub async fn rename(
    conn: &mut PgConnection,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    name: &BTreeMap<String, String>,
) -> WriteResult<Config> {
    let updated = map_unique_violation(
        sqlx::query_scalar!(
            r#"
            UPDATE project_config SET name = $3, version = version + 1
            WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))
            RETURNING id
            "#,
            id,
            expected_versions,
            Json(trimmed(name)) as _
        )
        .fetch_optional(&mut *conn)
        .await,
    )?;
    match updated {
        Err(conflict) => Ok(Err(conflict)),
        Ok(None) => Ok(Err(missing_or_stale(conn, id).await?)),
        Ok(Some(_)) => found(conn, id).await,
    }
}

/// Deletes the configuration with its versions, if its version is
/// expected. The caller checks first that it is neither the default nor in
/// use (the foreign key of `node` refuses it anyway).
pub async fn delete(conn: &mut PgConnection, id: Uuid, expected_versions: Option<&[i64]>) -> WriteResult<()> {
    let deleted = sqlx::query!(
        "DELETE FROM project_config WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))",
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

/// Opens a draft of the configuration, a copy of its published version, if
/// it has none; then reads it. Its admins share the one draft.
pub async fn open_draft(conn: &mut PgConnection, id: Uuid, created_by: Option<Uuid>) -> WriteResult<Config> {
    let Some(config) = lock(conn, id, None).await? else {
        return Ok(Err(missing_or_stale(conn, id).await?));
    };
    if config.draft.is_some() {
        return Ok(Ok(config));
    }
    let draft = crate::id::new_id();
    sqlx::query!(
        r#"
        INSERT INTO project_config_version (id, tenant_id, config_id, number, state, created_by)
        SELECT $1, app_current_tenant(), $2, max(number) + 1, 'draft', $3
        FROM project_config_version WHERE config_id = $2
        "#,
        draft,
        id,
        created_by
    )
    .execute(&mut *conn)
    .await?;
    copy_content(conn, config.published.id, draft).await?;
    bump(conn, id).await?;
    found(conn, id).await
}

/// Records that the draft `draft` changed `item`, once however many times;
/// false if it had already. Every edit of a draft goes through it.
pub async fn record_change(conn: &mut PgConnection, draft: Uuid, item: Item) -> Result<bool, sqlx::Error> {
    // The configuration of the draft, if the change was new.
    let recorded = sqlx::query_scalar!(
        r#"
        WITH draft AS (
            SELECT id, config_id FROM project_config_version WHERE id = $2 AND state = 'draft'
        ), inserted AS (
            INSERT INTO project_config_change (id, tenant_id, version_id, item_kind, item_id)
            SELECT $1, app_current_tenant(), draft.id, $3, $4 FROM draft
            ON CONFLICT (version_id, item_kind, item_id) DO NOTHING
            RETURNING version_id
        )
        SELECT draft.config_id FROM draft JOIN inserted ON inserted.version_id = draft.id
        "#,
        crate::id::new_id(),
        draft,
        item.kind.as_str(),
        Uuid::from_u128(item.id)
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(config) = recorded else {
        return Ok(false);
    };
    bump(conn, config).await?;
    Ok(true)
}

/// Discards the draft of the configuration, if its version is expected.
pub async fn discard_draft(
    conn: &mut PgConnection,
    id: Uuid,
    expected_versions: Option<&[i64]>,
) -> WriteResult<Config> {
    let Some(config) = lock(conn, id, expected_versions).await? else {
        return Ok(Err(missing_or_stale(conn, id).await?));
    };
    let Some(draft) = config.draft else {
        return Ok(Err(WriteConflict::NoDraft));
    };
    sqlx::query!("DELETE FROM project_config_version WHERE id = $1", draft.id)
        .execute(&mut *conn)
        .await?;
    bump(conn, id).await?;
    found(conn, id).await
}

/// Publishes the draft of the configuration, if its version is expected:
/// the draft becomes the published version, the former one is retired.
/// Its projects use it from the end of the caller's transaction.
pub async fn publish(
    conn: &mut PgConnection,
    id: Uuid,
    expected_versions: Option<&[i64]>,
) -> WriteResult<Config> {
    let Some(config) = lock(conn, id, expected_versions).await? else {
        return Ok(Err(missing_or_stale(conn, id).await?));
    };
    let Some(draft) = config.draft else {
        return Ok(Err(WriteConflict::NoDraft));
    };
    // Retired first: a configuration has one published version at a time.
    sqlx::query!(
        "UPDATE project_config_version SET state = 'retired' WHERE id = $1",
        config.published.id
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        "UPDATE project_config_version SET state = 'published', published_at = now() WHERE id = $1",
        draft.id
    )
    .execute(&mut *conn)
    .await?;
    bump(conn, id).await?;
    found(conn, id).await
}

/// Creates the configuration `id` named `name`, a copy of the published
/// version of `source`, then moves `project` to it if given (detaching it
/// from `source`). The draft of `source`, if any, stays with it.
pub async fn duplicate(
    conn: &mut PgConnection,
    source: Uuid,
    id: Uuid,
    name: &BTreeMap<String, String>,
    project: Option<Uuid>,
    created_by: Option<Uuid>,
) -> WriteResult<Config> {
    let Some(original) = find(conn, source).await? else {
        return Ok(Err(WriteConflict::NotFound));
    };
    let copy = match insert_empty(conn, id, name, created_by).await? {
        Ok(copy) => copy,
        Err(conflict) => return Ok(Err(conflict)),
    };
    copy_content(conn, original.published.id, copy.published.id).await?;
    if let Some(project) = project {
        assign(conn, project, id).await?;
    }
    found(conn, id).await
}

/// The configuration of `project`; `None` if it is no project of the tenant.
pub async fn of_project(conn: &mut PgConnection, project: Uuid) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT config_id AS "config_id!" FROM node WHERE id = $1 AND node_type = 'project'"#,
        project
    )
    .fetch_optional(conn)
    .await
}

/// Moves `project` to the configuration `config`; false if it is no
/// project of the tenant. Its tickets keep their types and statuses until
/// the migration wizard (L04-US08) maps them.
pub async fn assign(conn: &mut PgConnection, project: Uuid, config: Uuid) -> Result<bool, sqlx::Error> {
    let updated = sqlx::query!(
        "UPDATE node SET config_id = $2 WHERE id = $1 AND node_type = 'project'",
        project,
        config
    )
    .execute(conn)
    .await?;
    Ok(updated.rows_affected() == 1)
}

/// A project using a configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigProject {
    pub id: Uuid,
    pub name: String,
    pub key: String,
    pub archived: bool,
}

/// The projects using the configuration `id`, by name, archived ones
/// included.
pub async fn projects(conn: &mut PgConnection, id: Uuid) -> Result<Vec<ConfigProject>, sqlx::Error> {
    sqlx::query_as!(
        ConfigProject,
        r#"
        SELECT id, name, key AS "key!", archived_at IS NOT NULL AS "archived!"
        FROM node WHERE config_id = $1
        ORDER BY name, id
        "#,
        id
    )
    .fetch_all(conn)
    .await
}

/// Copies what the version `from` holds into the version `to`: each
/// content table of L04-US02 onwards adds its copy here, keeping the
/// identifiers its items have across versions.
async fn copy_content(conn: &mut PgConnection, from: Uuid, to: Uuid) -> Result<(), sqlx::Error> {
    crate::issue_types::copy(conn, from, to).await
}

/// The configuration, its row locked until the end of the transaction, if
/// its version is one of `expected_versions` (`None`: any).
async fn lock(
    conn: &mut PgConnection,
    id: Uuid,
    expected_versions: Option<&[i64]>,
) -> Result<Option<Config>, sqlx::Error> {
    let locked = sqlx::query_scalar!(
        r#"
        SELECT id FROM project_config
        WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))
        FOR UPDATE
        "#,
        id,
        expected_versions
    )
    .fetch_optional(&mut *conn)
    .await?;
    match locked {
        Some(_) => find(conn, id).await,
        None => Ok(None),
    }
}

/// A new version of the configuration `id` (its ETag).
async fn bump(conn: &mut PgConnection, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE project_config SET version = version + 1 WHERE id = $1",
        id
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// The configuration `id`, just written in this transaction.
async fn found(conn: &mut PgConnection, id: Uuid) -> WriteResult<Config> {
    Ok(find(conn, id).await?.ok_or(WriteConflict::NotFound))
}

async fn missing_or_stale(conn: &mut PgConnection, id: Uuid) -> Result<WriteConflict, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM project_config WHERE id = $1) AS "exists!""#,
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
