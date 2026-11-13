//! SQL of the grants of roles on hierarchy nodes (L03-US03), and the grant
//! source of the permission engine. Row-level security scopes every query to
//! the current tenant.
//!
//! The rule of inheritance lives in `domain::grants`: this module loads a
//! node's ancestry (through the closure table) and the grants made on it,
//! and lets the domain decide which ones apply.

use std::collections::{BTreeMap, HashSet};

use domain::grants::{AncestryNode, Grant, Principal, Subject, granting_nodes};
use domain::hierarchy::NodeType;
use sqlx::postgres::PgConnection;
use sqlx::types::Json;
use uuid::Uuid;

use crate::nodes::NodeRef;
use crate::permissions::{GrantFuture, GrantSource};

/// Grants read from `node_role_grant`, inherited through the hierarchy: the
/// grant source of the application.
#[derive(Debug, Clone, Copy, Default)]
pub struct NodeGrants;

impl GrantSource for NodeGrants {
    fn roles_of<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        account_id: Uuid,
        node: Option<Uuid>,
    ) -> GrantFuture<'a, Vec<Uuid>> {
        Box::pin(async move {
            match node {
                Some(node) => roles_reaching(conn, account_id, node).await,
                // Roles are granted on nodes only.
                None => Ok(Vec::new()),
            }
        })
    }

    fn grants_of_role<'a>(&'a self, conn: &'a mut PgConnection, role_id: Uuid) -> GrantFuture<'a, u64> {
        Box::pin(async move {
            let count = sqlx::query_scalar!(
                r#"SELECT count(*) AS "count!" FROM node_role_grant WHERE role_id = $1"#,
                role_id
            )
            .fetch_one(conn)
            .await?;
            Ok(u64::try_from(count).unwrap_or(0))
        })
    }
}

/// The node `node` and its ancestors, each with its parents and whether it
/// blocks inheritance.
pub async fn ancestry(conn: &mut PgConnection, node: Uuid) -> Result<Vec<AncestryNode>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT n.id, n.confidential,
               COALESCE(array_agg(e.parent_id) FILTER (WHERE e.parent_id IS NOT NULL), '{}') AS "parents!"
        FROM node_closure c
        JOIN node n ON n.id = c.ancestor_id
        LEFT JOIN node_edge e ON e.child_id = n.id
        WHERE c.descendant_id = $1
        GROUP BY n.id, n.confidential
        "#,
        node
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| AncestryNode {
            id: row.id.as_u128(),
            confidential: row.confidential,
            parents: row.parents.iter().map(Uuid::as_u128).collect(),
        })
        .collect())
}

/// The roles that reach `account_id` on `node`: granted to them or to one of
/// their groups, there or above, as far as inheritance goes.
pub async fn roles_reaching(
    conn: &mut PgConnection,
    account_id: Uuid,
    node: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    // Their grants, and their groups', on the node and its ancestors.
    let rows = sqlx::query!(
        r#"
        SELECT g.node_id, g.role_id, g.account_id, g.group_id
        FROM node_role_grant g
        JOIN node_closure c ON c.ancestor_id = g.node_id AND c.descendant_id = $1
        WHERE g.account_id = $2
           OR g.group_id IN (SELECT gm.group_id FROM group_member gm WHERE gm.account_id = $2)
        "#,
        node,
        account_id
    )
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut groups = Vec::new();
    let grants: Vec<Grant> = rows
        .iter()
        .map(|row| Grant {
            node: row.node_id.as_u128(),
            role: row.role_id.as_u128(),
            principal: match row.group_id {
                Some(group) => {
                    groups.push(group.as_u128());
                    Principal::Group(group.as_u128())
                }
                None => Principal::Member(account_id.as_u128()),
            },
        })
        .collect();
    let ancestry = ancestry(conn, node).await?;
    let subject = Subject {
        member: account_id.as_u128(),
        groups: &groups,
    };
    Ok(
        domain::grants::roles_reaching(node.as_u128(), &ancestry, &grants, subject)
            .into_iter()
            .map(Uuid::from_u128)
            .collect(),
    )
}

/// The nodes a member finds in the lists of the hierarchy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sight {
    /// Those they see: a role with "view" reaches them there.
    pub visible: HashSet<Uuid>,
    /// Confidential nodes they have no access to, under a node they see:
    /// shown veiled (`domain::grants::Projection`).
    pub veiled: HashSet<Uuid>,
}

/// What `account_id` sees of the hierarchy. The rule of `domain::grants`,
/// downwards: a grant covers its node, then the children that do not block
/// inheritance, and so on.
pub async fn sight(conn: &mut PgConnection, account_id: Uuid) -> Result<Sight, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        WITH RECURSIVE visible (id) AS (
            SELECT g.node_id
            FROM node_role_grant g
            JOIN role r ON r.id = g.role_id
            WHERE 'view' = ANY (r.permissions)
              AND (g.account_id = $1
                   OR g.group_id IN (SELECT gm.group_id FROM group_member gm WHERE gm.account_id = $1))
            UNION
            SELECT e.child_id
            FROM visible v
            JOIN node_edge e ON e.parent_id = v.id
            JOIN node child ON child.id = e.child_id
            WHERE NOT child.confidential
        )
        SELECT v.id AS "id!", false AS "veiled!" FROM visible v
        UNION ALL
        SELECT n.id, true
        FROM node n
        WHERE n.confidential
          AND n.id NOT IN (SELECT id FROM visible)
          AND EXISTS (
              SELECT 1 FROM node_edge e
              WHERE e.child_id = n.id AND e.parent_id IN (SELECT id FROM visible))
        "#,
        account_id
    )
    .fetch_all(conn)
    .await?;
    let mut sight = Sight::default();
    for row in rows {
        if row.veiled {
            sight.veiled.insert(row.id);
        } else {
            sight.visible.insert(row.id);
        }
    }
    Ok(sight)
}

/// Who a role is granted to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grantee {
    Member(Uuid),
    Group(Uuid),
}

/// Who holds a grant, with their name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Holder {
    Member { account_id: Uuid, display_name: String },
    Group { id: Uuid, name: String },
}

/// A grant that applies on a node: who, which role, and the node it was
/// granted on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    pub grant_id: Uuid,
    pub holder: Holder,
    pub role_id: Uuid,
    pub role_label: BTreeMap<String, String>,
    /// The node the role was granted on.
    pub origin: NodeRef,
    /// Granted on the node itself, not inherited.
    pub direct: bool,
}

/// The grants that apply on `node`: the direct ones first, then the
/// inherited ones, by holder name.
pub async fn access(conn: &mut PgConnection, node: Uuid) -> Result<Vec<Access>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT g.id, g.role_id, r.label AS "label: Json<BTreeMap<String, String>>",
               g.node_id, o.node_type, o.name AS node_name,
               g.account_id, a.display_name AS "display_name?",
               g.group_id, gr.name AS "group_name?"
        FROM node_role_grant g
        JOIN node_closure c ON c.ancestor_id = g.node_id AND c.descendant_id = $1
        JOIN node o ON o.id = g.node_id
        JOIN role r ON r.id = g.role_id
        LEFT JOIN account a ON a.id = g.account_id
        LEFT JOIN "group" gr ON gr.id = g.group_id
        ORDER BY (g.node_id <> $1), lower(COALESCE(a.display_name, gr.name)), g.id
        "#,
        node
    )
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let ancestry = ancestry(conn, node).await?;
    let sources = granting_nodes(node.as_u128(), &ancestry);
    Ok(rows
        .into_iter()
        .filter(|row| sources.contains(&row.node_id.as_u128()))
        .filter_map(|row| {
            let holder = match (row.account_id, row.group_id) {
                (Some(account_id), _) => Holder::Member {
                    account_id,
                    display_name: row.display_name.unwrap_or_default(),
                },
                (None, Some(id)) => Holder::Group {
                    id,
                    name: row.group_name.unwrap_or_default(),
                },
                // The table's check excludes it.
                (None, None) => return None,
            };
            Some(Access {
                grant_id: row.id,
                holder,
                role_id: row.role_id,
                role_label: row.label.0,
                origin: NodeRef {
                    id: row.node_id,
                    node_type: NodeType::parse(&row.node_type).unwrap_or(NodeType::Project),
                    name: row.node_name,
                },
                direct: row.node_id == node,
            })
        })
        .collect())
}

/// A grant to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewGrant {
    pub id: Uuid,
    pub node: Uuid,
    pub role: Uuid,
    pub grantee: Grantee,
    pub created_by: Uuid,
}

/// Why a grant was not made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantConflict {
    NodeNotFound,
    RoleNotFound,
    /// The grantee is no member of the tenant.
    UnknownMember,
    GroupNotFound,
    /// The grantee already has this role on this node.
    Exists,
}

/// Grants the role; the database checks that the node, the role and the
/// grantee are of the tenant, and that the grant is new.
pub async fn insert(
    conn: &mut PgConnection,
    new: &NewGrant,
) -> Result<Result<(), GrantConflict>, sqlx::Error> {
    let (account_id, group_id) = match new.grantee {
        Grantee::Member(id) => (Some(id), None),
        Grantee::Group(id) => (None, Some(id)),
    };
    let inserted = sqlx::query!(
        r#"
        INSERT INTO node_role_grant (id, tenant_id, node_id, role_id, account_id, group_id, created_by)
        VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6)
        "#,
        new.id,
        new.node,
        new.role,
        account_id,
        group_id,
        new.created_by
    )
    .execute(conn)
    .await;
    match inserted {
        Ok(_) => Ok(Ok(())),
        Err(sqlx::Error::Database(err)) => match err.constraint() {
            Some("node_role_grant_node_fkey") => Ok(Err(GrantConflict::NodeNotFound)),
            Some("node_role_grant_role_fkey") => Ok(Err(GrantConflict::RoleNotFound)),
            Some("node_role_grant_member_fkey") => Ok(Err(GrantConflict::UnknownMember)),
            Some("node_role_grant_group_fkey") => Ok(Err(GrantConflict::GroupNotFound)),
            Some("node_role_grant_member_idx" | "node_role_grant_group_idx") => {
                Ok(Err(GrantConflict::Exists))
            }
            _ => Err(sqlx::Error::Database(err)),
        },
        Err(err) => Err(err),
    }
}

/// The role of the grant `grant_id` made on `node`, if there is one.
pub async fn role_of(
    conn: &mut PgConnection,
    node: Uuid,
    grant_id: Uuid,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        "SELECT role_id FROM node_role_grant WHERE id = $1 AND node_id = $2",
        grant_id,
        node
    )
    .fetch_optional(conn)
    .await
}

/// Removes the grant `grant_id` made on `node`; false if there was none.
pub async fn delete(conn: &mut PgConnection, node: Uuid, grant_id: Uuid) -> Result<bool, sqlx::Error> {
    let removed = sqlx::query_scalar!(
        "DELETE FROM node_role_grant WHERE id = $1 AND node_id = $2 RETURNING id",
        grant_id,
        node
    )
    .fetch_optional(conn)
    .await?;
    Ok(removed.is_some())
}

/// A member or a group a role can be granted to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Candidate {
    Member {
        account_id: Uuid,
        display_name: String,
        email: String,
    },
    Group {
        id: Uuid,
        name: String,
    },
}

/// Up to `limit` groups, then up to `limit` active members, whose name (or
/// email) holds `search` (all if `None`), each by name.
pub async fn candidates(
    conn: &mut PgConnection,
    search: Option<&str>,
    limit: usize,
) -> Result<Vec<Candidate>, sqlx::Error> {
    let pattern = search.map(crate::invitations::like_pattern);
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let groups = sqlx::query!(
        r#"
        SELECT id, name FROM "group"
        WHERE $1::text IS NULL OR lower(name) LIKE $1
        ORDER BY lower(name), id
        LIMIT $2
        "#,
        pattern,
        limit
    )
    .fetch_all(&mut *conn)
    .await?;
    let members = sqlx::query!(
        r#"
        SELECT a.id, a.display_name, a.email
        FROM membership m
        JOIN account a ON a.id = m.account_id
        WHERE m.status = 'active'
          AND ($1::text IS NULL OR lower(a.display_name) LIKE $1 OR a.email LIKE $1)
        ORDER BY lower(a.display_name), a.id
        LIMIT $2
        "#,
        pattern,
        limit
    )
    .fetch_all(&mut *conn)
    .await?;
    let groups = groups.into_iter().map(|row| Candidate::Group {
        id: row.id,
        name: row.name,
    });
    let members = members.into_iter().map(|row| Candidate::Member {
        account_id: row.id,
        display_name: row.display_name,
        email: row.email,
    });
    Ok(groups.chain(members).collect())
}
