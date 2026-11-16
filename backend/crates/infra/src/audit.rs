//! The history of tickets (specification §12, §18, L05-US06): `audit_log`,
//! written in the transaction of each ticket write, one row per changed
//! field or other action ([`domain::audit`]). The table is partitioned by
//! month (`app_ensure_audit_partitions`).

use chrono::{DateTime, Utc};
use domain::audit::{Action, Origin};
use sqlx::postgres::PgConnection;
use uuid::Uuid;

use crate::id::new_id;
use crate::pagination::{Keyset, Page, page_from_rows};

/// Months of partitions kept ready ahead of the current one.
pub const MONTHS_AHEAD: i32 = 3;

/// One entry to record.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub action: Action,
    /// The field of an [`Action::Updated`] entry: `status`, `fields.{id}`…
    pub field: Option<String>,
    /// None (or JSON null): nothing.
    pub old: Option<serde_json::Value>,
    pub new: Option<serde_json::Value>,
}

impl Entry {
    /// An action without values.
    pub fn action(action: Action) -> Self {
        Self {
            action,
            field: None,
            old: None,
            new: None,
        }
    }

    /// A changed field.
    pub fn field(field: impl Into<String>, old: serde_json::Value, new: serde_json::Value) -> Self {
        Self {
            action: Action::Updated,
            field: Some(field.into()),
            old: Some(old),
            new: Some(new),
        }
    }
}

/// Who wrote, and from where.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Author {
    /// None: the system.
    pub actor_id: Option<Uuid>,
    pub origin: Origin,
}

/// JSON null stored as SQL null: both mean nothing.
fn stored(value: Option<&serde_json::Value>) -> Option<&serde_json::Value> {
    value.filter(|v| !v.is_null())
}

/// Records `entries` about the ticket `issue` in the current transaction and
/// tenant, as one change; its id (none recorded when `entries` is empty).
///
/// Rows of one change share their time; their ids decrease in the order
/// given, so that the history, newest first, lists them in that order.
pub async fn record(
    conn: &mut PgConnection,
    issue: Uuid,
    author: Author,
    entries: &[Entry],
) -> Result<Option<Uuid>, sqlx::Error> {
    if entries.is_empty() {
        return Ok(None);
    }
    let change = new_id();
    let mut ids: Vec<Uuid> = entries.iter().map(|_| new_id()).collect();
    ids.reverse();
    for (id, entry) in ids.into_iter().zip(entries) {
        sqlx::query!(
            r#"
            INSERT INTO audit_log (id, tenant_id, entity_type, entity_id, change_id, action, field, old_value,
                                   new_value, actor_id, origin)
            VALUES ($1, app_current_tenant(), 'issue', $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
            id,
            issue,
            change,
            entry.action.as_str(),
            entry.field,
            stored(entry.old.as_ref()),
            stored(entry.new.as_ref()),
            author.actor_id,
            author.origin.as_str()
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(Some(change))
}

/// An entry as recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct Recorded {
    pub id: Uuid,
    pub change_id: Uuid,
    pub action: Action,
    pub field: Option<String>,
    pub old: Option<serde_json::Value>,
    pub new: Option<serde_json::Value>,
    pub actor_id: Option<Uuid>,
    /// The name the actor shows under: "Deleted user #N" once erased, none
    /// when no longer a member.
    pub actor_name: Option<String>,
    pub origin: Origin,
    pub created_at: DateTime<Utc>,
}

struct Row {
    id: Uuid,
    change_id: Uuid,
    action: String,
    field: Option<String>,
    old_value: Option<serde_json::Value>,
    new_value: Option<serde_json::Value>,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    origin: String,
    created_at: DateTime<Utc>,
}

impl Row {
    fn into_recorded(self) -> Option<Recorded> {
        Some(Recorded {
            id: self.id,
            change_id: self.change_id,
            action: Action::parse(&self.action)?,
            field: self.field,
            old: self.old_value,
            new: self.new_value,
            actor_id: self.actor_id,
            actor_name: self.actor_name,
            origin: Origin::parse(&self.origin)?,
            created_at: self.created_at,
        })
    }
}

/// The history of the ticket `issue`, newest first: `limit` entries before
/// `before` (none: the newest).
pub async fn of_issue(
    conn: &mut PgConnection,
    issue: Uuid,
    limit: usize,
    before: Option<Keyset>,
) -> Result<Page<Recorded>, sqlx::Error> {
    let fetch = i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX);
    let rows = sqlx::query_as!(
        Row,
        r#"
        SELECT id, change_id, action, field, old_value, new_value, actor_id,
               app_member_name(actor_id) AS "actor_name?", origin, created_at
        FROM audit_log
        WHERE entity_type = 'issue' AND entity_id = $1
          AND ($2::timestamptz IS NULL OR (created_at, id) < ($2, $3))
        ORDER BY created_at DESC, id DESC
        LIMIT $4
        "#,
        issue,
        before.map(|k| k.created_at),
        before.map(|k| k.id),
        fetch
    )
    .fetch_all(conn)
    .await?;
    let entries: Vec<Recorded> = rows.into_iter().filter_map(Row::into_recorded).collect();
    Ok(page_from_rows(entries, limit, |entry| Keyset {
        created_at: entry.created_at,
        id: entry.id,
    }))
}

/// Creates the partitions of the current month and of the next
/// [`MONTHS_AHEAD`] if missing; the number created. The worker calls it
/// every day.
pub async fn ensure_partitions(conn: &mut PgConnection) -> Result<i32, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT app_ensure_audit_partitions($1) AS "created!""#,
        MONTHS_AHEAD
    )
    .fetch_one(conn)
    .await
}

/// Removes the content of the comments of `author` from the history (what
/// an erasure purging comments asks); the number of entries changed.
/// Entries keep their action, comment, time and actor.
pub async fn purge_comments_of(conn: &mut PgConnection, author: Uuid) -> Result<u64, sqlx::Error> {
    let done = sqlx::query!(
        r#"
        UPDATE audit_log l SET old_value = l.old_value - 'content', new_value = l.new_value - 'content'
        FROM comment c
        WHERE l.action IN ('comment_created', 'comment_updated', 'comment_deleted')
          AND c.id = coalesce(l.new_value ->> 'commentId', l.old_value ->> 'commentId')::uuid
          AND c.author_id = $1
          AND (l.old_value ? 'content' OR l.new_value ? 'content')
        "#,
        author
    )
    .execute(conn)
    .await?;
    Ok(done.rows_affected())
}

/// The names the accounts `ids` show under in the current tenant: members
/// by name, erased ones as "Deleted user #N"; others (no longer members)
/// are left out.
pub async fn names(
    conn: &mut PgConnection,
    ids: &[Uuid],
) -> Result<std::collections::BTreeMap<Uuid, String>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT id AS "id!", app_member_name(id) AS "name?" FROM unnest($1::uuid[]) AS ids (id)"#,
        ids
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| Some((row.id, row.name?)))
        .collect())
}
