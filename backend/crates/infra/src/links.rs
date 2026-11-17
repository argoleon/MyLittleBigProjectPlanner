//! SQL of links between tickets (L05-US03). Row-level security scopes every
//! query to the current tenant.

use domain::links::{Link, LinkType};
use sqlx::postgres::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

/// A link as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredLink {
    pub id: Uuid,
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub link_type: LinkType,
    pub created_at: OffsetDateTime,
}

impl StoredLink {
    pub fn domain(&self) -> Link {
        Link {
            source: self.source_id.as_u128(),
            target: self.target_id.as_u128(),
            link_type: self.link_type,
        }
    }

    /// The other ticket, seen from `issue`.
    pub fn other(&self, issue: Uuid) -> Uuid {
        if self.source_id == issue {
            self.target_id
        } else {
            self.source_id
        }
    }
}

struct Row {
    id: Uuid,
    source_id: Uuid,
    target_id: Uuid,
    link_type: String,
    created_at: OffsetDateTime,
}

impl Row {
    fn stored(self) -> Option<StoredLink> {
        Some(StoredLink {
            id: self.id,
            source_id: self.source_id,
            target_id: self.target_id,
            link_type: LinkType::parse(&self.link_type)?,
            created_at: self.created_at,
        })
    }
}

/// The links of the ticket `issue`, from either side, oldest first.
pub async fn of_issue(conn: &mut PgConnection, issue: Uuid) -> Result<Vec<StoredLink>, sqlx::Error> {
    let rows = sqlx::query_as!(
        Row,
        r#"
        SELECT id, source_id, target_id, link_type, created_at FROM issue_link
        WHERE source_id = $1 OR target_id = $1
        ORDER BY created_at, id
        "#,
        issue
    )
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().filter_map(Row::stored).collect())
}

/// The link `id`.
pub async fn find(conn: &mut PgConnection, id: Uuid) -> Result<Option<StoredLink>, sqlx::Error> {
    let row = sqlx::query_as!(
        Row,
        "SELECT id, source_id, target_id, link_type, created_at FROM issue_link WHERE id = $1",
        id
    )
    .fetch_optional(conn)
    .await?;
    Ok(row.and_then(Row::stored))
}

/// The link already joins the two tickets this way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exists;

/// Stores `link`; [`Exists`] if the two tickets already have it (a
/// concurrent request may have stored it first).
pub async fn insert(
    conn: &mut PgConnection,
    id: Uuid,
    link: &Link,
    created_by: Uuid,
) -> Result<Result<StoredLink, Exists>, sqlx::Error> {
    let result = sqlx::query_as!(
        Row,
        r#"
        INSERT INTO issue_link (id, tenant_id, source_id, target_id, link_type, created_by)
        VALUES ($1, app_current_tenant(), $2, $3, $4, $5)
        RETURNING id, source_id, target_id, link_type, created_at
        "#,
        id,
        Uuid::from_u128(link.source),
        Uuid::from_u128(link.target),
        link.link_type.as_str(),
        created_by
    )
    .fetch_one(conn)
    .await;
    match result {
        Ok(row) => Ok(row.stored().ok_or(Exists)),
        Err(sqlx::Error::Database(err))
            if err
                .constraint()
                .is_some_and(|c| ["issue_link_unique", "issue_link_relates_once"].contains(&c)) =>
        {
            Ok(Err(Exists))
        }
        Err(err) => Err(err),
    }
}

/// Deletes the link `id`; whether it existed.
pub async fn delete(conn: &mut PgConnection, id: Uuid) -> Result<bool, sqlx::Error> {
    let deleted = sqlx::query!("DELETE FROM issue_link WHERE id = $1", id)
        .execute(conn)
        .await?;
    Ok(deleted.rows_affected() == 1)
}
