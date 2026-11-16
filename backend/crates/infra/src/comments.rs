//! SQL of comments (L05-US04). Row-level security scopes every query to the
//! current tenant.

use chrono::{DateTime, Utc};
use domain::comments::Visibility;
use sqlx::postgres::PgConnection;
use uuid::Uuid;

/// Who wrote a comment: a member, or a transition's "add a comment".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Member,
    Transition,
}

impl Source {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Member => "member",
            Self::Transition => "transition",
        }
    }
}

/// A comment as stored, with its author's name.
#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    pub id: Uuid,
    pub issue_id: Uuid,
    pub author_id: Option<Uuid>,
    pub author_name: Option<String>,
    pub visibility: Visibility,
    pub source: Source,
    /// The rich text document; none once deleted.
    pub content: Option<serde_json::Value>,
    pub content_html: Option<String>,
    pub mentions: Vec<Uuid>,
    pub edited_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub version: i64,
}

/// What a comment holds when written or edited.
#[derive(Debug, Clone, PartialEq)]
pub struct Written<'a> {
    pub visibility: Visibility,
    pub content: &'a serde_json::Value,
    pub content_html: &'a str,
    pub mentions: &'a [Uuid],
}

struct Row {
    id: Uuid,
    issue_id: Uuid,
    author_id: Option<Uuid>,
    author_name: Option<String>,
    visibility: String,
    source: String,
    content: Option<serde_json::Value>,
    content_html: Option<String>,
    mentions: Vec<Uuid>,
    edited_at: Option<DateTime<Utc>>,
    deleted_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    version: i64,
}

impl From<Row> for Comment {
    fn from(r: Row) -> Self {
        Self {
            id: r.id,
            issue_id: r.issue_id,
            author_id: r.author_id,
            author_name: r.author_name,
            visibility: Visibility::parse(&r.visibility).unwrap_or_default(),
            source: if r.source == "transition" {
                Source::Transition
            } else {
                Source::Member
            },
            content: r.content,
            content_html: r.content_html,
            mentions: r.mentions,
            edited_at: r.edited_at,
            deleted_at: r.deleted_at,
            created_at: r.created_at,
            version: r.version,
        }
    }
}

/// Writes a comment on the ticket `issue` by `author`; it as stored.
pub async fn insert(
    conn: &mut PgConnection,
    id: Uuid,
    issue: Uuid,
    author: Uuid,
    source: Source,
    written: &Written<'_>,
) -> Result<Comment, sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO comment (id, tenant_id, issue_id, author_id, visibility, source, content, content_html, mentions)
        VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6, $7, $8)
        "#,
        id,
        issue,
        author,
        written.visibility.as_str(),
        source.as_str(),
        written.content,
        written.content_html,
        written.mentions
    )
    .execute(&mut *conn)
    .await?;
    find(conn, id).await?.ok_or(sqlx::Error::RowNotFound)
}

/// The comment `id`.
pub async fn find(conn: &mut PgConnection, id: Uuid) -> Result<Option<Comment>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT c.id, c.issue_id, c.author_id, app_member_name(c.author_id) AS "author_name?", c.visibility, c.source, c.content,
               c.content_html, c.mentions, c.edited_at, c.deleted_at, c.created_at, c.version
        FROM comment c
        WHERE c.id = $1
        "#,
        id
    )
    .fetch_optional(conn)
    .await
    .map(|row| row.map(Comment::from))
}

/// The comments of the ticket `issue`, oldest first; public ones only if
/// `public_only` (what the portal may show).
pub async fn of_issue(
    conn: &mut PgConnection,
    issue: Uuid,
    public_only: bool,
) -> Result<Vec<Comment>, sqlx::Error> {
    sqlx::query_as!(
        Row,
        r#"
        SELECT c.id, c.issue_id, c.author_id, app_member_name(c.author_id) AS "author_name?", c.visibility, c.source, c.content,
               c.content_html, c.mentions, c.edited_at, c.deleted_at, c.created_at, c.version
        FROM comment c
        WHERE c.issue_id = $1 AND (NOT $2 OR c.visibility = 'public')
        ORDER BY c.created_at, c.id
        "#,
        issue,
        public_only
    )
    .fetch_all(conn)
    .await
    .map(|rows| rows.into_iter().map(Comment::from).collect())
}

/// Locks the comment `id` until the transaction ends; its version.
pub async fn lock(conn: &mut PgConnection, id: Uuid) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar!("SELECT version FROM comment WHERE id = $1 FOR UPDATE", id)
        .fetch_optional(conn)
        .await
}

/// Replaces what the comment `id` holds, marks it edited and bumps its
/// version; it as stored.
pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    written: &Written<'_>,
) -> Result<Comment, sqlx::Error> {
    sqlx::query!(
        r#"
        UPDATE comment
        SET visibility = $2, content = $3, content_html = $4, mentions = $5, edited_at = now(),
            version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
        id,
        written.visibility.as_str(),
        written.content,
        written.content_html,
        written.mentions
    )
    .execute(&mut *conn)
    .await?;
    find(conn, id).await?.ok_or(sqlx::Error::RowNotFound)
}

/// Deletes the comment `id` by `by`: its content goes, the placeholder
/// stays; its version is bumped.
pub async fn delete(conn: &mut PgConnection, id: Uuid, by: Uuid) -> Result<Comment, sqlx::Error> {
    sqlx::query!(
        r#"
        UPDATE comment
        SET content = NULL, content_html = NULL, mentions = '{}', deleted_at = now(), deleted_by = $2,
            version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
        id,
        by
    )
    .execute(&mut *conn)
    .await?;
    find(conn, id).await?.ok_or(sqlx::Error::RowNotFound)
}
