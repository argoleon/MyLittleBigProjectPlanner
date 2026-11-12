//! SQL of the hierarchy's nodes (L03-US01): portfolios, programs and
//! projects, their owners and their parent links. Row-level security scopes
//! every query to the current tenant; the database enforces the attachment
//! matrix and refuses cycles (`node_edge` triggers).

use chrono::{DateTime, NaiveDate, Utc};
use domain::hierarchy::{Health, NodeType};
use sqlx::postgres::PgConnection;
use uuid::Uuid;

use crate::id::new_id;
use crate::pagination::{Keyset, Page, page_from_rows};

/// A member owning a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owner {
    pub account_id: Uuid,
    pub display_name: String,
}

/// A node seen from another one (a parent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRef {
    pub id: Uuid,
    pub node_type: NodeType,
    pub name: String,
}

/// A node as stored, with its owners and parents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub id: Uuid,
    pub node_type: NodeType,
    pub name: String,
    pub key: Option<String>,
    pub description: String,
    pub objectives: String,
    pub start_date: Option<NaiveDate>,
    pub end_date: Option<NaiveDate>,
    pub health: Health,
    pub archived: bool,
    pub confidential: bool,
    pub owners: Vec<Owner>,
    pub parents: Vec<NodeRef>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
}

impl Node {
    pub fn keyset(&self) -> Keyset {
        Keyset {
            created_at: self.created_at,
            id: self.id,
        }
    }
}

/// What can be changed on a node (its type and key cannot).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeFields<'a> {
    pub name: &'a str,
    pub description: &'a str,
    pub objectives: &'a str,
    pub start_date: Option<NaiveDate>,
    pub end_date: Option<NaiveDate>,
    pub health: Health,
    pub owners: &'a [Uuid],
}

/// A node to create, at the root or under `parent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNode<'a> {
    pub id: Uuid,
    pub node_type: NodeType,
    pub key: Option<&'a str>,
    pub fields: NodeFields<'a>,
    pub parent: Option<Uuid>,
    pub created_by: Uuid,
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    NotFound,
    VersionMismatch,
    /// Another project of the tenant has this key.
    KeyTaken,
    /// No such parent in the tenant.
    ParentNotFound,
    /// The attachment matrix does not allow this type under that parent.
    ParentNotAllowed,
    /// The link would make the node its own ancestor.
    Cycle,
    /// An owner is no member of the tenant.
    UnknownOwner,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

/// The conflict a database error stands for, if any.
fn conflict_of(err: &sqlx::Error) -> Option<WriteConflict> {
    let sqlx::Error::Database(db) = err else {
        return None;
    };
    match (db.message(), db.constraint()) {
        ("node.parent_not_allowed", _) => Some(WriteConflict::ParentNotAllowed),
        ("node.cycle", _) => Some(WriteConflict::Cycle),
        (_, Some("node_tenant_key_idx")) => Some(WriteConflict::KeyTaken),
        (_, Some("node_owner_tenant_id_account_id_fkey")) => Some(WriteConflict::UnknownOwner),
        _ => None,
    }
}

/// Runs `write`, turning the database errors that stand for a conflict into
/// that conflict.
async fn conflicts<T>(write: impl Future<Output = Result<T, sqlx::Error>>) -> WriteResult<T> {
    match write.await {
        Ok(value) => Ok(Ok(value)),
        Err(err) => conflict_of(&err).map_or(Err(err), |conflict| Ok(Err(conflict))),
    }
}

/// Creates the node, its owners and its link to `parent`, then reads it.
/// The caller runs it in a transaction: a conflict leaves the transaction
/// to roll back.
pub async fn insert(conn: &mut PgConnection, new: &NewNode<'_>) -> WriteResult<Node> {
    if let Some(parent) = new.parent
        && !exists(conn, parent).await?
    {
        return Ok(Err(WriteConflict::ParentNotFound));
    }
    let fields = &new.fields;
    let inserted = conflicts(
        sqlx::query!(
            r#"
            INSERT INTO node (id, tenant_id, node_type, name, key, description, objectives,
                              start_date, end_date, health, created_by)
            VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
            new.id,
            new.node_type.as_str(),
            fields.name.trim(),
            new.key,
            fields.description,
            fields.objectives,
            fields.start_date,
            fields.end_date,
            fields.health.as_str(),
            new.created_by
        )
        .execute(&mut *conn),
    )
    .await?;
    if let Err(conflict) = inserted {
        return Ok(Err(conflict));
    }
    if let Err(conflict) = set_owners(conn, new.id, fields.owners).await? {
        return Ok(Err(conflict));
    }
    if let Some(parent) = new.parent {
        let linked = conflicts(
            sqlx::query!(
                r#"
                INSERT INTO node_edge (id, tenant_id, parent_id, child_id)
                VALUES ($1, app_current_tenant(), $2, $3)
                "#,
                new_id(),
                parent,
                new.id
            )
            .execute(&mut *conn),
        )
        .await?;
        if let Err(conflict) = linked {
            return Ok(Err(conflict));
        }
    }
    find(conn, new.id)
        .await
        .map(|node| node.ok_or(WriteConflict::NotFound))
}

/// Whether the node `id` exists in the tenant.
pub async fn exists(conn: &mut PgConnection, id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM node WHERE id = $1) AS "exists!""#,
        id
    )
    .fetch_one(conn)
    .await
}

struct Row {
    id: Uuid,
    node_type: String,
    name: String,
    key: Option<String>,
    description: String,
    objectives: String,
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
    health: String,
    archived: bool,
    confidential: bool,
    version: i64,
    created_at: DateTime<Utc>,
}

impl Row {
    fn into_node(self, owners: Vec<Owner>, parents: Vec<NodeRef>) -> Node {
        Node {
            id: self.id,
            // The database only holds known identifiers.
            node_type: NodeType::parse(&self.node_type).unwrap_or(NodeType::Project),
            name: self.name,
            key: self.key,
            description: self.description,
            objectives: self.objectives,
            start_date: self.start_date,
            end_date: self.end_date,
            health: Health::parse(&self.health).unwrap_or_default(),
            archived: self.archived,
            confidential: self.confidential,
            owners,
            parents,
            version: self.version,
            created_at: self.created_at,
        }
    }
}

/// The node `id`, with its owners and parents.
pub async fn find(conn: &mut PgConnection, id: Uuid) -> Result<Option<Node>, sqlx::Error> {
    let row = sqlx::query_as!(
        Row,
        r#"
        SELECT id, node_type, name, key, description, objectives, start_date, end_date, health,
               archived_at IS NOT NULL AS "archived!", confidential, version, created_at
        FROM node WHERE id = $1
        "#,
        id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let mut owners = owners_of(conn, &[id]).await?;
    let mut parents = parents_of(conn, &[id]).await?;
    Ok(Some(row.into_node(
        owners.remove(&id).unwrap_or_default(),
        parents.remove(&id).unwrap_or_default(),
    )))
}

/// One page of nodes, of `node_type` if given, in creation order.
pub async fn list(
    conn: &mut PgConnection,
    node_type: Option<NodeType>,
    after: Option<Keyset>,
    limit: usize,
) -> Result<Page<Node>, sqlx::Error> {
    let fetch = i64::try_from(limit).unwrap_or(i64::MAX).saturating_add(1);
    let rows = sqlx::query_as!(
        Row,
        r#"
        SELECT id, node_type, name, key, description, objectives, start_date, end_date, health,
               archived_at IS NOT NULL AS "archived!", confidential, version, created_at
        FROM node
        WHERE ($1::text IS NULL OR node_type = $1)
          AND ($2::timestamptz IS NULL OR (created_at, id) > ($2, $3::uuid))
        ORDER BY created_at, id
        LIMIT $4
        "#,
        node_type.map(NodeType::as_str),
        after.map(|k| k.created_at),
        after.map(|k| k.id),
        fetch
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let mut owners = owners_of(conn, &ids).await?;
    let mut parents = parents_of(conn, &ids).await?;
    let items = rows
        .into_iter()
        .map(|row| {
            let id = row.id;
            row.into_node(
                owners.remove(&id).unwrap_or_default(),
                parents.remove(&id).unwrap_or_default(),
            )
        })
        .collect();
    Ok(page_from_rows(items, limit, Node::keyset))
}

/// Changes the node's fields and owners, if its version is one of
/// `expected_versions` (`None`: any), then reads it.
pub async fn update(
    conn: &mut PgConnection,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    fields: &NodeFields<'_>,
) -> WriteResult<Node> {
    let updated = sqlx::query_scalar!(
        r#"
        UPDATE node
        SET name = $3, description = $4, objectives = $5, start_date = $6, end_date = $7,
            health = $8, version = version + 1
        WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))
        RETURNING id
        "#,
        id,
        expected_versions,
        fields.name.trim(),
        fields.description,
        fields.objectives,
        fields.start_date,
        fields.end_date,
        fields.health.as_str()
    )
    .fetch_optional(&mut *conn)
    .await?;
    if updated.is_none() {
        return Ok(Err(if exists(conn, id).await? {
            WriteConflict::VersionMismatch
        } else {
            WriteConflict::NotFound
        }));
    }
    if let Err(conflict) = set_owners(conn, id, fields.owners).await? {
        return Ok(Err(conflict));
    }
    find(conn, id)
        .await
        .map(|node| node.ok_or(WriteConflict::NotFound))
}

/// Replaces the owners of the node `id` with `owners`.
async fn set_owners(conn: &mut PgConnection, id: Uuid, owners: &[Uuid]) -> WriteResult<()> {
    sqlx::query!(
        "DELETE FROM node_owner WHERE node_id = $1 AND NOT (account_id = ANY($2))",
        id,
        owners
    )
    .execute(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = owners.iter().map(|_| new_id()).collect();
    conflicts(
        sqlx::query!(
            r#"
            INSERT INTO node_owner (id, tenant_id, node_id, account_id)
            SELECT new_id, app_current_tenant(), $1, account_id
            FROM unnest($2::uuid[], $3::uuid[]) AS o (new_id, account_id)
            ON CONFLICT (tenant_id, node_id, account_id) DO NOTHING
            "#,
            id,
            &ids,
            owners
        )
        .execute(&mut *conn),
    )
    .await
    .map(|result| result.map(|_| ()))
}

/// The owners of each of `ids`, by name.
async fn owners_of(
    conn: &mut PgConnection,
    ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, Vec<Owner>>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT o.node_id, a.id AS account_id, a.display_name
        FROM node_owner o
        JOIN account a ON a.id = o.account_id
        WHERE o.node_id = ANY($1)
        ORDER BY a.display_name, a.id
        "#,
        ids
    )
    .fetch_all(conn)
    .await?;
    let mut owners: std::collections::HashMap<Uuid, Vec<Owner>> = std::collections::HashMap::new();
    for row in rows {
        owners.entry(row.node_id).or_default().push(Owner {
            account_id: row.account_id,
            display_name: row.display_name,
        });
    }
    Ok(owners)
}

/// The parents of each of `ids`, by name.
async fn parents_of(
    conn: &mut PgConnection,
    ids: &[Uuid],
) -> Result<std::collections::HashMap<Uuid, Vec<NodeRef>>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT e.child_id, p.id, p.node_type, p.name
        FROM node_edge e
        JOIN node p ON p.id = e.parent_id
        WHERE e.child_id = ANY($1)
        ORDER BY p.name, p.id
        "#,
        ids
    )
    .fetch_all(conn)
    .await?;
    let mut parents: std::collections::HashMap<Uuid, Vec<NodeRef>> = std::collections::HashMap::new();
    for row in rows {
        parents.entry(row.child_id).or_default().push(NodeRef {
            id: row.id,
            node_type: NodeType::parse(&row.node_type).unwrap_or(NodeType::Project),
            name: row.name,
        });
    }
    Ok(parents)
}
