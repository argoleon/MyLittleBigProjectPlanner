//! SQL of the hierarchy's nodes (L03-US01, US02): portfolios, programs and
//! projects, their owners and their weighted parent links. Row-level security scopes
//! every query to the current tenant; the database enforces the attachment
//! matrix and refuses cycles (`node_edge` triggers). Whatever changes the
//! aggregates of a node marks them stale (`crate::aggregates`).

use chrono::{DateTime, NaiveDate, Utc};
use domain::hierarchy::{Health, NodeType};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgConnection;
use uuid::Uuid;

use crate::id::new_id;
use crate::invitations::like_pattern;
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

/// A node linked to another one (a parent or a child), with the weighting of
/// the link and the linked node's own parents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub id: Uuid,
    pub node_type: NodeType,
    pub name: String,
    pub key: Option<String>,
    pub weight_percent: i16,
    /// Blocks inheritance: those above it without a role on it see it veiled.
    pub confidential: bool,
    pub parents: Vec<NodeRef>,
}

/// A node that might become a parent, with what the database knows of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: Uuid,
    pub node_type: NodeType,
    pub name: String,
    pub key: Option<String>,
    pub parents: Vec<NodeRef>,
    /// It is a descendant of the child: the link would make a cycle.
    pub descends_from_child: bool,
    /// It already is a parent of the child.
    pub already_parent: bool,
}

/// What [`link`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkChange {
    Created,
    /// The link existed with another weighting.
    Updated,
    /// The link existed with this weighting.
    Unchanged,
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
    // The node gets its aggregate, and its parent's counts one more.
    let stale: Vec<Uuid> = std::iter::once(new.id).chain(new.parent).collect();
    crate::aggregates::mark_stale(conn, &stale).await?;
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

/// Which nodes a list holds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NodeFilter<'a> {
    /// Only nodes of this type.
    pub node_type: Option<NodeType>,
    /// Only the ancestors of this node (not the node itself).
    pub ancestor_of: Option<Uuid>,
    /// Only the descendants of this node (not the node itself).
    pub descendant_of: Option<Uuid>,
    /// Only these nodes: those a member sees or sees veiled
    /// (`crate::grants::sight`; `None`: all, for tenant admins).
    pub within: Option<&'a [Uuid]>,
    /// Archived nodes only; otherwise, active ones only.
    pub archived: bool,
}

/// One page of the nodes of `filter`, in creation order.
pub async fn list(
    conn: &mut PgConnection,
    filter: &NodeFilter<'_>,
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
          AND ($5::uuid IS NULL OR EXISTS (
              SELECT 1 FROM node_closure c
              WHERE c.ancestor_id = node.id AND c.descendant_id = $5 AND c.ancestor_id <> $5))
          AND ($6::uuid IS NULL OR EXISTS (
              SELECT 1 FROM node_closure c
              WHERE c.ancestor_id = $6 AND c.descendant_id = node.id AND c.descendant_id <> $6))
          AND ($7::uuid[] IS NULL OR node.id = ANY($7))
          AND (archived_at IS NOT NULL) = $8
        ORDER BY created_at, id
        LIMIT $4
        "#,
        filter.node_type.map(NodeType::as_str),
        after.map(|k| k.created_at),
        after.map(|k| k.id),
        fetch,
        filter.ancestor_of,
        filter.descendant_of,
        filter.within,
        filter.archived
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

/// Makes the node confidential or not, if its version is one of
/// `expected_versions` (`None`: any), then reads it.
pub async fn set_confidential(
    conn: &mut PgConnection,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    confidential: bool,
) -> WriteResult<Node> {
    let updated = sqlx::query_scalar!(
        r#"
        UPDATE node SET confidential = $3, version = version + 1
        WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))
        RETURNING id
        "#,
        id,
        expected_versions,
        confidential
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
    // Its ancestors tell their confidential projects apart.
    crate::aggregates::mark_stale(conn, &[id]).await?;
    find(conn, id)
        .await
        .map(|node| node.ok_or(WriteConflict::NotFound))
}

/// Archives the node or restores it, if its version is one of
/// `expected_versions` (`None`: any), then reads it. Archiving an archived
/// node keeps the date it was archived at.
pub async fn set_archived(
    conn: &mut PgConnection,
    id: Uuid,
    expected_versions: Option<&[i64]>,
    archived: bool,
) -> WriteResult<Node> {
    let updated = sqlx::query_scalar!(
        r#"
        UPDATE node
        SET archived_at = CASE WHEN $3 THEN coalesce(archived_at, now()) END, version = version + 1
        WHERE id = $1 AND ($2::bigint[] IS NULL OR version = ANY($2))
        RETURNING id
        "#,
        id,
        expected_versions,
        archived
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
    // It leaves the aggregates of its ancestors, or comes back to them.
    crate::aggregates::mark_stale(conn, &[id]).await?;
    find(conn, id)
        .await
        .map(|node| node.ok_or(WriteConflict::NotFound))
}

/// What the node `id` stands between, for its archiving: whether one of its
/// children is active, and whether one of its parents is archived.
pub async fn surroundings(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<domain::hierarchy::Surroundings, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        SELECT
            EXISTS (
                SELECT 1 FROM node_edge e JOIN node c ON c.id = e.child_id
                WHERE e.parent_id = $1 AND c.archived_at IS NULL
            ) AS "has_active_children!",
            EXISTS (
                SELECT 1 FROM node_edge e JOIN node p ON p.id = e.parent_id
                WHERE e.child_id = $1 AND p.archived_at IS NOT NULL
            ) AS "has_archived_parent!"
        "#,
        id
    )
    .fetch_one(conn)
    .await?;
    Ok(domain::hierarchy::Surroundings {
        has_active_children: row.has_active_children,
        has_archived_parent: row.has_archived_parent,
    })
}

/// An identifier standing for the node `id` where it shows veiled: stable,
/// and of no use to find the node (a SHA-256 of its id, as a UUID).
pub fn opaque_id(id: Uuid) -> Uuid {
    let digest = Sha256::new()
        .chain_update(b"mlbpp:veiled-node:")
        .chain_update(id.as_bytes())
        .finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_custom_bytes(bytes).into_uuid()
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

/// Links `child` under `parent` with `weight_percent`, or sets the weighting
/// of their existing link. The database checks the matrix and cycles.
pub async fn link(
    conn: &mut PgConnection,
    parent: Uuid,
    child: Uuid,
    weight_percent: i16,
) -> WriteResult<LinkChange> {
    if !exists(conn, child).await? {
        return Ok(Err(WriteConflict::NotFound));
    }
    if !exists(conn, parent).await? {
        return Ok(Err(WriteConflict::ParentNotFound));
    }
    let created = conflicts(
        sqlx::query_scalar!(
            r#"
            INSERT INTO node_edge (id, tenant_id, parent_id, child_id, weight_percent)
            VALUES ($1, app_current_tenant(), $2, $3, $4)
            ON CONFLICT (tenant_id, parent_id, child_id) DO UPDATE
                SET weight_percent = EXCLUDED.weight_percent, version = node_edge.version + 1
                WHERE node_edge.weight_percent <> EXCLUDED.weight_percent
            RETURNING (xmax = 0) AS "created!"
            "#,
            new_id(),
            parent,
            child,
            weight_percent
        )
        .fetch_optional(&mut *conn),
    )
    .await?;
    let change = created.map(|created| match created {
        Some(true) => LinkChange::Created,
        Some(false) => LinkChange::Updated,
        None => LinkChange::Unchanged,
    });
    if matches!(change, Ok(LinkChange::Created | LinkChange::Updated)) {
        // The parent and those above it count the child, or weigh it anew.
        crate::aggregates::mark_stale(conn, &[parent]).await?;
    }
    Ok(change)
}

/// Removes the link of `child` under `parent`; false if there was none.
pub async fn unlink(conn: &mut PgConnection, parent: Uuid, child: Uuid) -> Result<bool, sqlx::Error> {
    let removed = sqlx::query_scalar!(
        "DELETE FROM node_edge WHERE parent_id = $1 AND child_id = $2 RETURNING id",
        parent,
        child
    )
    .fetch_optional(&mut *conn)
    .await?;
    if removed.is_some() {
        // The parent and those above it no longer count the child.
        crate::aggregates::mark_stale(conn, &[parent]).await?;
    }
    Ok(removed.is_some())
}

/// A node as the hierarchy shows it when browsed: at the top, or under a
/// parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseRow {
    pub id: Uuid,
    pub node_type: NodeType,
    pub name: String,
    pub key: Option<String>,
    pub health: Health,
    pub confidential: bool,
    /// Weighting of its link under the browsed parent; `None` at the top.
    pub weight_percent: Option<i16>,
    /// It has active children of its own.
    pub has_children: bool,
    pub owners: Vec<Owner>,
    pub parents: Vec<NodeRef>,
}

/// One level of the hierarchy, portfolios first, then programs and
/// projects, each by name; archived nodes are left out.
///
/// - Under `under`: its children.
/// - Without `under`: the nodes at the top of `within` (those a member
///   sees; `None`: all), that is without a parent, or without a parent in
///   `within`.
pub async fn browse(
    conn: &mut PgConnection,
    under: Option<Uuid>,
    within: Option<&[Uuid]>,
) -> Result<Vec<BrowseRow>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT n.id, n.node_type, n.name, n.key, n.health, n.confidential,
               e.weight_percent AS "weight_percent?",
               EXISTS (
                   SELECT 1 FROM node_edge ce JOIN node c ON c.id = ce.child_id
                   WHERE ce.parent_id = n.id AND c.archived_at IS NULL
               ) AS "has_children!"
        FROM node n
        LEFT JOIN node_edge e ON e.child_id = n.id AND e.parent_id = $1
        WHERE n.archived_at IS NULL
          AND CASE WHEN $1::uuid IS NOT NULL THEN e.id IS NOT NULL
              ELSE ($2::uuid[] IS NULL OR n.id = ANY($2))
                   AND NOT EXISTS (
                       SELECT 1 FROM node_edge pe
                       WHERE pe.child_id = n.id AND ($2::uuid[] IS NULL OR pe.parent_id = ANY($2)))
              END
        ORDER BY array_position(ARRAY['portfolio', 'program', 'project'], n.node_type), lower(n.name), n.id
        "#,
        under,
        within
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let mut owners = owners_of(conn, &ids).await?;
    let mut parents = parents_of(conn, &ids).await?;
    Ok(rows
        .into_iter()
        .map(|row| BrowseRow {
            id: row.id,
            node_type: NodeType::parse(&row.node_type).unwrap_or(NodeType::Project),
            name: row.name,
            key: row.key,
            health: Health::parse(&row.health).unwrap_or_default(),
            confidential: row.confidential,
            weight_percent: row.weight_percent,
            has_children: row.has_children,
            owners: owners.remove(&row.id).unwrap_or_default(),
            parents: parents.remove(&row.id).unwrap_or_default(),
        })
        .collect())
}

struct LinkRow {
    id: Uuid,
    node_type: String,
    name: String,
    key: Option<String>,
    weight_percent: i16,
    confidential: bool,
}

/// Adds their own parents to `rows`.
async fn links_with_parents(conn: &mut PgConnection, rows: Vec<LinkRow>) -> Result<Vec<Link>, sqlx::Error> {
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let mut parents = parents_of(conn, &ids).await?;
    Ok(rows
        .into_iter()
        .map(|row| Link {
            id: row.id,
            node_type: NodeType::parse(&row.node_type).unwrap_or(NodeType::Project),
            name: row.name,
            key: row.key,
            weight_percent: row.weight_percent,
            confidential: row.confidential,
            parents: parents.remove(&row.id).unwrap_or_default(),
        })
        .collect())
}

/// The parents of `id`, by name, with the weighting of each link.
pub async fn parent_links(conn: &mut PgConnection, id: Uuid) -> Result<Vec<Link>, sqlx::Error> {
    let rows = sqlx::query_as!(
        LinkRow,
        r#"
        SELECT p.id, p.node_type, p.name, p.key, e.weight_percent, p.confidential
        FROM node_edge e JOIN node p ON p.id = e.parent_id
        WHERE e.child_id = $1
        ORDER BY p.name, p.id
        "#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    links_with_parents(conn, rows).await
}

/// The active children of `id`, by name, with the weighting of each link.
/// An archived project keeps showing its sub-projects, archived with it.
pub async fn child_links(conn: &mut PgConnection, id: Uuid) -> Result<Vec<Link>, sqlx::Error> {
    let rows = sqlx::query_as!(
        LinkRow,
        r#"
        SELECT c.id, c.node_type, c.name, c.key, e.weight_percent, c.confidential
        FROM node_edge e JOIN node c ON c.id = e.child_id
        WHERE e.parent_id = $1
          AND (c.archived_at IS NULL
               OR EXISTS (SELECT 1 FROM node p WHERE p.id = $1 AND p.archived_at IS NOT NULL))
        ORDER BY c.name, c.id
        "#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    links_with_parents(conn, rows).await
}

/// Up to `limit` active nodes, other than `child`, whose name or key holds
/// `search` (all if `None`), by name: the possible parents of `child`, each
/// with whether it descends from `child` or already is its parent.
pub async fn parent_candidates(
    conn: &mut PgConnection,
    child: Uuid,
    search: Option<&str>,
    limit: usize,
) -> Result<Vec<Candidate>, sqlx::Error> {
    let pattern = search.map(like_pattern);
    let rows = sqlx::query!(
        r#"
        SELECT n.id, n.node_type, n.name, n.key,
               EXISTS (SELECT 1 FROM node_closure c WHERE c.ancestor_id = $1 AND c.descendant_id = n.id)
                   AS "descends!",
               EXISTS (SELECT 1 FROM node_edge e WHERE e.parent_id = n.id AND e.child_id = $1)
                   AS "already!"
        FROM node n
        WHERE n.id <> $1 AND n.archived_at IS NULL
          AND ($2::text IS NULL OR lower(n.name) LIKE $2 OR lower(n.key) LIKE $2)
        ORDER BY lower(n.name), n.id
        LIMIT $3
        "#,
        child,
        pattern,
        i64::try_from(limit).unwrap_or(i64::MAX)
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let mut parents = parents_of(conn, &ids).await?;
    Ok(rows
        .into_iter()
        .map(|row| Candidate {
            id: row.id,
            node_type: NodeType::parse(&row.node_type).unwrap_or(NodeType::Project),
            name: row.name,
            key: row.key,
            parents: parents.remove(&row.id).unwrap_or_default(),
            descends_from_child: row.descends,
            already_parent: row.already,
        })
        .collect())
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
