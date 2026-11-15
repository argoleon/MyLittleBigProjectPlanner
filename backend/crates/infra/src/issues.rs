//! SQL of tickets (L05-US01). Row-level security scopes every query to the
//! current tenant.
//!
//! A ticket takes the next key number of its project under the row lock of
//! the project's counter (`project_issue_counter`): two creations in one
//! project never share a key, whatever their order.

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::postgres::PgConnection;
use uuid::Uuid;

use crate::pagination::{Keyset, Page, page_from_rows};

/// A ticket as stored, with the names of its people.
#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub id: Uuid,
    pub project_id: Uuid,
    pub key: String,
    pub key_number: i32,
    /// The type and status of the configuration, by `item_id`.
    pub type_id: Uuid,
    pub status_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub summary: String,
    /// The rich text document (Tiptap JSON).
    pub description: Option<serde_json::Value>,
    pub priority: String,
    pub reporter_id: Option<Uuid>,
    pub reporter_name: Option<String>,
    pub assignee_id: Option<Uuid>,
    pub assignee_name: Option<String>,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    /// Custom field values, by field `item_id`.
    pub fields: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: i64,
}

impl Issue {
    pub fn keyset(&self) -> Keyset {
        Keyset {
            created_at: self.created_at,
            id: self.id,
        }
    }
}

/// A ticket to create; its key comes from its project.
#[derive(Debug, Clone, PartialEq)]
pub struct NewIssue<'a> {
    pub id: Uuid,
    pub project_id: Uuid,
    /// The key of the project (`PLAT`).
    pub project_key: &'a str,
    pub type_id: Uuid,
    /// The initial status of the type's workflow.
    pub status_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub summary: &'a str,
    pub description: Option<&'a serde_json::Value>,
    pub priority: &'a str,
    pub reporter_id: Uuid,
    pub assignee_id: Option<Uuid>,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub fields: &'a serde_json::Value,
}

/// The next key number of `project`, under the row lock of its counter
/// until the transaction ends.
async fn next_number(conn: &mut PgConnection, project: Uuid) -> Result<i32, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        INSERT INTO project_issue_counter (tenant_id, project_id, last_number)
        VALUES (app_current_tenant(), $1, 1)
        ON CONFLICT (tenant_id, project_id) DO UPDATE SET last_number = project_issue_counter.last_number + 1
        RETURNING last_number
        "#,
        project
    )
    .fetch_one(conn)
    .await
}

/// Creates the ticket with the next key of its project; it as stored.
pub async fn create(conn: &mut PgConnection, new: &NewIssue<'_>) -> Result<Issue, sqlx::Error> {
    let number = next_number(conn, new.project_id).await?;
    let key = domain::issues::key(new.project_key, u32::try_from(number).unwrap_or(u32::MAX));
    sqlx::query!(
        r#"
        INSERT INTO issue
            (id, tenant_id, project_id, key_number, key, type_id, status_id, parent_id, summary, description,
             priority, reporter_id, assignee_id, start_date, due_date, fields, created_by)
        VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $11)
        "#,
        new.id,
        new.project_id,
        number,
        key,
        new.type_id,
        new.status_id,
        new.parent_id,
        new.summary.trim(),
        new.description,
        new.priority,
        new.reporter_id,
        new.assignee_id,
        new.start_date,
        new.due_date,
        new.fields
    )
    .execute(&mut *conn)
    .await?;
    find(conn, new.id).await?.ok_or_else(|| sqlx::Error::RowNotFound)
}

/// Locks the ticket `id` until the transaction ends; its version, if it
/// exists.
pub async fn lock(conn: &mut PgConnection, id: Uuid) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar!("SELECT version FROM issue WHERE id = $1 FOR UPDATE", id)
        .fetch_optional(conn)
        .await
}

/// What a ticket becomes once edited (L05-US02): every editable value.
#[derive(Debug, Clone, PartialEq)]
pub struct Edited<'a> {
    pub summary: &'a str,
    pub description: Option<&'a serde_json::Value>,
    pub priority: &'a str,
    pub assignee_id: Option<Uuid>,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub fields: &'a serde_json::Value,
}

/// Writes `edited` to the ticket `id` and bumps its version; it as stored.
pub async fn update(conn: &mut PgConnection, id: Uuid, edited: &Edited<'_>) -> Result<Issue, sqlx::Error> {
    sqlx::query!(
        r#"
        UPDATE issue
        SET summary = $2, description = $3, priority = $4, assignee_id = $5, start_date = $6, due_date = $7,
            fields = $8, version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
        id,
        edited.summary.trim(),
        edited.description,
        edited.priority,
        edited.assignee_id,
        edited.start_date,
        edited.due_date,
        edited.fields
    )
    .execute(&mut *conn)
    .await?;
    find(conn, id).await?.ok_or(sqlx::Error::RowNotFound)
}

/// The sub-tasks (direct children) of the ticket `id`, oldest first.
pub async fn children(conn: &mut PgConnection, id: Uuid) -> Result<Vec<Issue>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT i.id, i.project_id, i.key, i.key_number, i.type_id, i.status_id, i.parent_id, i.summary,
               i.description, i.priority, i.reporter_id, r.display_name AS "reporter_name?", i.assignee_id,
               a.display_name AS "assignee_name?", i.start_date, i.due_date, i.fields, i.created_at, i.updated_at,
               i.version
        FROM issue i
        LEFT JOIN account r ON r.id = i.reporter_id
        LEFT JOIN account a ON a.id = i.assignee_id
        WHERE i.parent_id = $1
        ORDER BY i.created_at, i.id
        "#,
        id
    )
    .fetch_all(conn)
    .await
    .map(|rows| rows.into_iter().map(Issue::from).collect())
}

/// Puts the ticket `id` under `parent` (`None`: at the top) and bumps its
/// version (L05-US03); it as stored.
pub async fn set_parent(
    conn: &mut PgConnection,
    id: Uuid,
    parent: Option<Uuid>,
) -> Result<Issue, sqlx::Error> {
    sqlx::query!(
        "UPDATE issue SET parent_id = $2, version = version + 1, updated_at = now() WHERE id = $1",
        id,
        parent
    )
    .execute(&mut *conn)
    .await?;
    find(conn, id).await?.ok_or(sqlx::Error::RowNotFound)
}

/// Whether `candidate` sits, at any depth, under the ticket `ancestor`.
pub async fn is_below(conn: &mut PgConnection, ancestor: Uuid, candidate: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        WITH RECURSIVE up (id, parent_id, depth) AS (
            SELECT id, parent_id, 0 FROM issue WHERE id = $2
            UNION ALL
            SELECT i.id, i.parent_id, up.depth + 1 FROM issue i JOIN up ON i.id = up.parent_id
            WHERE up.depth < 50
        )
        SELECT EXISTS (SELECT 1 FROM up WHERE parent_id = $1) AS "below!"
        "#,
        ancestor,
        candidate
    )
    .fetch_one(conn)
    .await
}

/// The tickets `ids` that exist, in no given order.
pub async fn find_many(conn: &mut PgConnection, ids: &[Uuid]) -> Result<Vec<Issue>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as!(
        Row,
        r#"
        SELECT i.id, i.project_id, i.key, i.key_number, i.type_id, i.status_id, i.parent_id, i.summary,
               i.description, i.priority, i.reporter_id, r.display_name AS "reporter_name?", i.assignee_id,
               a.display_name AS "assignee_name?", i.start_date, i.due_date, i.fields, i.created_at, i.updated_at,
               i.version
        FROM issue i
        LEFT JOIN account r ON r.id = i.reporter_id
        LEFT JOIN account a ON a.id = i.assignee_id
        WHERE i.id = ANY($1)
        "#,
        ids
    )
    .fetch_all(conn)
    .await
    .map(|rows| rows.into_iter().map(Issue::from).collect())
}

/// Tickets matching `text` (L05-US03, until the query language of L06):
/// the one whose key or former key it is first, then those whose summary
/// holds it, most recently updated first; in `project` if given. At most
/// `limit`, before the caller keeps those whose project the member sees.
pub async fn search(
    conn: &mut PgConnection,
    text: &str,
    project: Option<Uuid>,
    limit: usize,
) -> Result<Vec<Issue>, sqlx::Error> {
    let key = text.trim().to_uppercase();
    let pattern = crate::invitations::like_pattern(text);
    sqlx::query_as!(
        Row,
        r#"
        SELECT i.id, i.project_id, i.key, i.key_number, i.type_id, i.status_id, i.parent_id, i.summary,
               i.description, i.priority, i.reporter_id, r.display_name AS "reporter_name?", i.assignee_id,
               a.display_name AS "assignee_name?", i.start_date, i.due_date, i.fields, i.created_at, i.updated_at,
               i.version
        FROM issue i
        LEFT JOIN account r ON r.id = i.reporter_id
        LEFT JOIN account a ON a.id = i.assignee_id
        WHERE ($3::uuid IS NULL OR i.project_id = $3)
          AND (i.key = $1
               OR EXISTS (SELECT 1 FROM issue_key_alias k WHERE k.issue_id = i.id AND k.key = $1)
               OR lower(i.summary) LIKE $2)
        ORDER BY (i.key = $1 OR EXISTS (SELECT 1 FROM issue_key_alias k WHERE k.issue_id = i.id AND k.key = $1)) DESC,
                 i.updated_at DESC, i.id
        LIMIT $4
        "#,
        key,
        pattern,
        project,
        i64::try_from(limit).unwrap_or(i64::MAX)
    )
    .fetch_all(conn)
    .await
    .map(|rows| rows.into_iter().map(Issue::from).collect())
}

/// Where a ticket moves (L05-US02).
#[derive(Debug, Clone, PartialEq)]
pub struct Move<'a> {
    pub id: Uuid,
    pub project_id: Uuid,
    /// The key of the target project (`CRM`).
    pub project_key: &'a str,
    pub type_id: Uuid,
    pub status_id: Uuid,
    pub parent_id: Option<Uuid>,
    /// The values it keeps there.
    pub fields: &'a serde_json::Value,
}

/// Moves a ticket: the next key of its new project, its former key kept as
/// an alias, its type, status, parent and values as given, its version
/// bumped; it as stored.
pub async fn move_to(conn: &mut PgConnection, moved: &Move<'_>) -> Result<Issue, sqlx::Error> {
    let number = next_number(conn, moved.project_id).await?;
    let key = domain::issues::key(moved.project_key, u32::try_from(number).unwrap_or(u32::MAX));
    sqlx::query!(
        r#"
        INSERT INTO issue_key_alias (id, tenant_id, key, issue_id)
        SELECT $1, tenant_id, key, id FROM issue WHERE id = $2
        "#,
        crate::id::new_id(),
        moved.id
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        r#"
        UPDATE issue
        SET project_id = $2, key_number = $3, key = $4, type_id = $5, status_id = $6, parent_id = $7,
            fields = $8, version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
        moved.id,
        moved.project_id,
        number,
        key,
        moved.type_id,
        moved.status_id,
        moved.parent_id,
        moved.fields
    )
    .execute(&mut *conn)
    .await?;
    find(conn, moved.id).await?.ok_or(sqlx::Error::RowNotFound)
}

struct Row {
    id: Uuid,
    project_id: Uuid,
    key: String,
    key_number: i32,
    type_id: Uuid,
    status_id: Uuid,
    parent_id: Option<Uuid>,
    summary: String,
    description: Option<serde_json::Value>,
    priority: String,
    reporter_id: Option<Uuid>,
    reporter_name: Option<String>,
    assignee_id: Option<Uuid>,
    assignee_name: Option<String>,
    start_date: Option<NaiveDate>,
    due_date: Option<NaiveDate>,
    fields: serde_json::Value,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    version: i64,
}

impl From<Row> for Issue {
    fn from(r: Row) -> Self {
        Self {
            id: r.id,
            project_id: r.project_id,
            key: r.key,
            key_number: r.key_number,
            type_id: r.type_id,
            status_id: r.status_id,
            parent_id: r.parent_id,
            summary: r.summary,
            description: r.description,
            priority: r.priority,
            reporter_id: r.reporter_id,
            reporter_name: r.reporter_name,
            assignee_id: r.assignee_id,
            assignee_name: r.assignee_name,
            start_date: r.start_date,
            due_date: r.due_date,
            fields: r.fields,
            created_at: r.created_at,
            updated_at: r.updated_at,
            version: r.version,
        }
    }
}

/// The ticket `id`.
pub async fn find(conn: &mut PgConnection, id: Uuid) -> Result<Option<Issue>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT i.id, i.project_id, i.key, i.key_number, i.type_id, i.status_id, i.parent_id, i.summary,
               i.description, i.priority, i.reporter_id, r.display_name AS "reporter_name?", i.assignee_id,
               a.display_name AS "assignee_name?", i.start_date, i.due_date, i.fields, i.created_at, i.updated_at,
               i.version
        FROM issue i
        LEFT JOIN account r ON r.id = i.reporter_id
        LEFT JOIN account a ON a.id = i.assignee_id
        WHERE i.id = $1
        "#,
        id
    )
    .fetch_optional(conn)
    .await
    .map(|row| row.map(Issue::from))
}

/// The id of the ticket whose key, or former key, is `key`.
pub async fn id_of_key(conn: &mut PgConnection, key: &str) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        SELECT id AS "id!" FROM issue WHERE key = $1
        UNION ALL
        SELECT issue_id FROM issue_key_alias WHERE key = $1
        LIMIT 1
        "#,
        key
    )
    .fetch_optional(conn)
    .await
}

/// The tickets assigned to `account` or reported by them, newest first,
/// `limit` per page after `after`.
pub async fn of_account(
    conn: &mut PgConnection,
    account: Uuid,
    after: Option<Keyset>,
    limit: usize,
) -> Result<Page<Issue>, sqlx::Error> {
    let rows = sqlx::query_as!(
        Row,
        r#"
        SELECT i.id, i.project_id, i.key, i.key_number, i.type_id, i.status_id, i.parent_id, i.summary,
               i.description, i.priority, i.reporter_id, r.display_name AS "reporter_name?", i.assignee_id,
               a.display_name AS "assignee_name?", i.start_date, i.due_date, i.fields, i.created_at, i.updated_at,
               i.version
        FROM issue i
        LEFT JOIN account r ON r.id = i.reporter_id
        LEFT JOIN account a ON a.id = i.assignee_id
        WHERE (i.assignee_id = $1 OR i.reporter_id = $1)
          AND ($2::timestamptz IS NULL OR (i.created_at, i.id) < ($2, $3))
        ORDER BY i.created_at DESC, i.id DESC
        LIMIT $4
        "#,
        account,
        after.map(|k| k.created_at),
        after.map(|k| k.id),
        i64::try_from(limit + 1).unwrap_or(i64::MAX)
    )
    .fetch_all(conn)
    .await?;
    Ok(page_from_rows(
        rows.into_iter().map(Issue::from).collect(),
        limit,
        Issue::keyset,
    ))
}
