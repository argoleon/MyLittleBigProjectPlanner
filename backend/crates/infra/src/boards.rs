//! SQL of Kanban boards (L06-US05). Row-level security scopes every query to
//! the current tenant; which nodes a member sees is decided by the caller
//! (`grants::sight`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sqlx::postgres::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::id::new_id;

/// Name of the unique index on `(tenant_id, node_id, lower(name))`.
const NAME_UNIQUE: &str = "board_node_name_idx";

/// A column of a board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub id: Uuid,
    pub name: String,
    /// Keys of the statuses it shows.
    pub statuses: Vec<String>,
    pub wip_limit: Option<u32>,
}

/// A quick filter: a clause of the language toggled on the board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickFilter {
    pub label: String,
    pub query: String,
}

/// A board, its node and columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    pub id: Uuid,
    pub node_id: Uuid,
    pub node_key: String,
    pub node_name: String,
    pub node_type: String,
    pub node_archived: bool,
    pub name: String,
    pub kind: String,
    pub filter: String,
    pub estimation: String,
    pub swimlanes: String,
    pub quick_filters: Vec<QuickFilter>,
    pub columns: Vec<Column>,
    pub version: i64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// A column as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInput<'a> {
    pub name: &'a str,
    pub statuses: &'a [String],
    pub wip_limit: Option<u32>,
}

/// A board as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardInput<'a> {
    pub name: &'a str,
    pub filter: &'a str,
    pub estimation: &'a str,
    pub swimlanes: &'a str,
    pub quick_filters: &'a [QuickFilter],
    pub columns: &'a [ColumnInput<'a>],
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    NotFound,
    VersionMismatch,
    /// The node has another board of this name (whatever its case).
    NameTaken,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

struct Row {
    id: Uuid,
    node_id: Uuid,
    node_key: String,
    node_name: String,
    node_type: String,
    node_archived: bool,
    name: String,
    kind: String,
    filter: String,
    estimation: String,
    swimlanes: String,
    quick_filters: serde_json::Value,
    version: i64,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

/// The boards of the nodes `nodes` (`None`: every node), or of `node` only,
/// or the board `only`: by node name, then board name.
async fn select(
    conn: &mut PgConnection,
    nodes: Option<&[Uuid]>,
    node: Option<Uuid>,
    only: Option<Uuid>,
) -> Result<Vec<Board>, sqlx::Error> {
    let rows = sqlx::query_as!(
        Row,
        r#"
        SELECT b.id, b.node_id, n.key AS node_key, n.name AS node_name, n.node_type,
               (n.archived_at IS NOT NULL) AS "node_archived!",
               b.name, b.kind, b.filter, b.estimation, b.swimlanes, b.quick_filters,
               b.version, b.created_at, b.updated_at
        FROM board b JOIN node n ON n.id = b.node_id
        WHERE ($1::uuid[] IS NULL OR b.node_id = ANY($1))
          AND ($2::uuid IS NULL OR b.node_id = $2)
          AND ($3::uuid IS NULL OR b.id = $3)
        ORDER BY lower(n.name), n.id, lower(b.name), b.id
        "#,
        nodes,
        node,
        only
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let mut columns: HashMap<Uuid, Vec<Column>> = HashMap::new();
    for column in sqlx::query!(
        r#"
        SELECT board_id, id, name, statuses, wip_limit
        FROM board_column WHERE board_id = ANY($1)
        ORDER BY board_id, position
        "#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?
    {
        columns.entry(column.board_id).or_default().push(Column {
            id: column.id,
            name: column.name,
            statuses: column.statuses,
            wip_limit: column.wip_limit.and_then(|l| u32::try_from(l).ok()),
        });
    }
    Ok(rows
        .into_iter()
        .map(|row| Board {
            columns: columns.remove(&row.id).unwrap_or_default(),
            quick_filters: serde_json::from_value(row.quick_filters).unwrap_or_default(),
            id: row.id,
            node_id: row.node_id,
            node_key: row.node_key,
            node_name: row.node_name,
            node_type: row.node_type,
            node_archived: row.node_archived,
            name: row.name,
            kind: row.kind,
            filter: row.filter,
            estimation: row.estimation,
            swimlanes: row.swimlanes,
            version: row.version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
        .collect())
}

/// The boards of the nodes `nodes` (`None`: every node), of `node` only
/// when given: by node name, then board name.
pub async fn list(
    conn: &mut PgConnection,
    nodes: Option<&[Uuid]>,
    node: Option<Uuid>,
) -> Result<Vec<Board>, sqlx::Error> {
    select(conn, nodes, node, None).await
}

/// The board `id`.
pub async fn find(conn: &mut PgConnection, id: Uuid) -> Result<Option<Board>, sqlx::Error> {
    Ok(select(conn, None, None, Some(id)).await?.pop())
}

/// Creates a board on `node`.
pub async fn insert(
    conn: &mut PgConnection,
    id: Uuid,
    node: Uuid,
    created_by: Uuid,
    input: &BoardInput<'_>,
) -> WriteResult<Board> {
    let quick = serde_json::to_value(input.quick_filters).unwrap_or_default();
    let inserted = sqlx::query!(
        r#"
        INSERT INTO board (id, tenant_id, node_id, name, filter, estimation, swimlanes, quick_filters, created_by)
        VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6, $7, $8)
        "#,
        id,
        node,
        input.name.trim(),
        input.filter.trim(),
        input.estimation,
        input.swimlanes,
        quick,
        created_by
    )
    .execute(&mut *conn)
    .await;
    if let Err(conflict) = map_unique_violation(inserted)? {
        return Ok(Err(conflict));
    }
    write_columns(conn, id, input.columns).await?;
    Ok(find(conn, id).await?.ok_or(WriteConflict::NotFound))
}

/// Replaces the board `id`, its columns included, if its version is one of
/// `expected_versions` (`None`: any).
pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    input: &BoardInput<'_>,
) -> WriteResult<Board> {
    let quick = serde_json::to_value(input.quick_filters).unwrap_or_default();
    let updated = sqlx::query_scalar!(
        r#"
        UPDATE board
        SET name = $3, filter = $4, estimation = $5, swimlanes = $6, quick_filters = $7,
            version = version + 1
        WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))
        RETURNING id
        "#,
        id,
        expected_versions,
        input.name.trim(),
        input.filter.trim(),
        input.estimation,
        input.swimlanes,
        quick
    )
    .fetch_optional(&mut *conn)
    .await;
    match map_unique_violation(updated)? {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(Err(missing_or_stale(conn, id).await?)),
        Err(conflict) => return Ok(Err(conflict)),
    }
    sqlx::query!("DELETE FROM board_column WHERE board_id = $1", id)
        .execute(&mut *conn)
        .await?;
    write_columns(conn, id, input.columns).await?;
    Ok(find(conn, id).await?.ok_or(WriteConflict::NotFound))
}

/// Deletes the board `id`, if its version is expected.
pub async fn delete(conn: &mut PgConnection, id: Uuid, expected_versions: Option<&[i64]>) -> WriteResult<()> {
    let deleted = sqlx::query!(
        "DELETE FROM board WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))",
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

async fn write_columns(
    conn: &mut PgConnection,
    board: Uuid,
    columns: &[ColumnInput<'_>],
) -> Result<(), sqlx::Error> {
    for (position, column) in columns.iter().enumerate() {
        sqlx::query!(
            r#"
            INSERT INTO board_column (id, tenant_id, board_id, position, name, statuses, wip_limit)
            VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6)
            "#,
            new_id(),
            board,
            i32::try_from(position).unwrap_or(i32::MAX),
            column.name.trim(),
            column.statuses,
            column.wip_limit.and_then(|l| i32::try_from(l).ok())
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

async fn missing_or_stale(conn: &mut PgConnection, id: Uuid) -> Result<WriteConflict, sqlx::Error> {
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM board WHERE id = $1) AS "exists!""#,
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

/// A status of the published configurations, by key: what columns map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusOption {
    pub key: String,
    /// `{"fr": …, "en": …}`.
    pub label: serde_json::Value,
    /// `todo`, `in_progress` or `done`.
    pub category: String,
}

/// The statuses of the published configurations of `projects` (`None`: of
/// every project), one per key: the most used label and category of the
/// key, by workflow position.
pub async fn statuses(
    conn: &mut PgConnection,
    projects: Option<&[Uuid]>,
) -> Result<Vec<StatusOption>, sqlx::Error> {
    sqlx::query_as!(
        StatusOption,
        r#"
        SELECT DISTINCT ON (s.key) s.key, s.label, s.category
        FROM workflow_status s
        JOIN project_config_version v ON v.id = s.version_id AND v.state = 'published'
        WHERE $1::uuid[] IS NULL OR v.config_id IN (
            SELECT n.config_id FROM node n WHERE n.id = ANY($1) AND n.config_id IS NOT NULL)
        ORDER BY s.key, s.position, s.id
        "#,
        projects
    )
    .fetch_all(conn)
    .await
}

/// The epic of a card: the nearest ancestor whose type is an epic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpicOf {
    pub id: Uuid,
    pub key: String,
    pub key_number: i32,
    pub summary: String,
}

/// The epic of each of `cards` that has one, among the projects `visible`
/// (`None`: all): an epic the member may not see stays unknown.
pub async fn epics_of(
    conn: &mut PgConnection,
    cards: &[Uuid],
    visible: Option<&[Uuid]>,
) -> Result<HashMap<Uuid, EpicOf>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        WITH RECURSIVE up AS (
            SELECT i.id AS card_id, p.id, p.key, p.key_number, p.summary, p.type_id, p.project_id,
                   p.parent_id, 1 AS depth
            FROM issue i JOIN issue p ON p.id = i.parent_id
            WHERE i.id = ANY($1)
            UNION ALL
            SELECT up.card_id, p.id, p.key, p.key_number, p.summary, p.type_id, p.project_id,
                   p.parent_id, up.depth + 1
            FROM up JOIN issue p ON p.id = up.parent_id
            WHERE up.depth < 8
        )
        SELECT DISTINCT ON (up.card_id) up.card_id AS "card_id!", up.id AS "id!", up.key AS "key!",
               up.key_number AS "key_number!", up.summary AS "summary!"
        FROM up
        JOIN issue_type t ON t.item_id = up.type_id AND t.color = 'epic'
        JOIN project_config_version v ON v.id = t.version_id AND v.state = 'published'
        WHERE $2::uuid[] IS NULL OR up.project_id = ANY($2)
        ORDER BY up.card_id, up.depth
        "#,
        cards,
        visible
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.card_id,
                EpicOf {
                    id: row.id,
                    key: row.key,
                    key_number: row.key_number,
                    summary: row.summary,
                },
            )
        })
        .collect())
}

/// The keys of the unfinished tickets blocking each of `cards` (a `blocks`
/// link whose source is not in a Done status), among the projects `visible`
/// (`None`: all), by key.
pub async fn blockers_of(
    conn: &mut PgConnection,
    cards: &[Uuid],
    visible: Option<&[Uuid]>,
) -> Result<HashMap<Uuid, Vec<String>>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT l.target_id, s.key
        FROM issue_link l JOIN issue s ON s.id = l.source_id
        WHERE l.link_type = 'blocks' AND l.target_id = ANY($1)
          AND ($2::uuid[] IS NULL OR s.project_id = ANY($2))
          AND NOT EXISTS (
              SELECT 1 FROM workflow_status ws
              JOIN project_config_version v ON v.id = ws.version_id AND v.state = 'published'
              WHERE ws.item_id = s.status_id AND ws.category = 'done')
        ORDER BY l.target_id, s.key, s.id
        "#,
        cards,
        visible
    )
    .fetch_all(conn)
    .await?;
    let mut blockers: HashMap<Uuid, Vec<String>> = HashMap::new();
    for row in rows {
        blockers.entry(row.target_id).or_default().push(row.key);
    }
    Ok(blockers)
}

/// The projects of `node` and below: itself if a project.
pub async fn projects_below(conn: &mut PgConnection, node: Uuid) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        SELECT n.id FROM node n
        WHERE n.node_type = 'project'
          AND (n.id = $1 OR EXISTS (
              SELECT 1 FROM node_closure c WHERE c.ancestor_id = $1 AND c.descendant_id = n.id))
        ORDER BY n.id
        "#,
        node
    )
    .fetch_all(conn)
    .await
}
