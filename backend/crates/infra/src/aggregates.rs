//! Precomputed aggregates of the hierarchy (specification §4, L03-US05).
//!
//! - A change that may alter aggregates (a link, its weighting, a project's
//!   figures, its confidentiality) calls [`mark_stale`] in its own
//!   transaction: the nodes are marked and a [`AGGREGATE_JOB`] is queued
//!   [`RECOMPUTE_DELAY`](domain::aggregates::RECOMPUTE_DELAY) later, unless
//!   one is already coming. Changes made in a burst share one recompute.
//! - The worker runs [`recompute`]: it takes every mark of the tenant,
//!   computes the aggregates of the marked nodes and of all their ancestors
//!   (`domain::aggregates`) and stores them in `node_aggregate`.
//! - [`find`] reads them; a node never computed aggregates nothing.

use std::collections::HashMap;

use domain::aggregates::{Aggregate, Figures, Hierarchy, Link, Project, recompute_in};
use sqlx::postgres::PgConnection;
use uuid::Uuid;

/// Kind of the job that recomputes the stale aggregates of its tenant.
pub const AGGREGATE_JOB: &str = "node.aggregates";

/// A count or an amount as PostgreSQL stores it.
fn stored(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// A stored count or amount: never negative (`CHECK`).
fn read(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

/// Marks the aggregates of `nodes`, and of their ancestors, to recompute,
/// and makes sure a job comes for them. Nodes that do not exist are skipped.
pub async fn mark_stale(conn: &mut PgConnection, nodes: &[Uuid]) -> Result<(), sqlx::Error> {
    if nodes.is_empty() {
        return Ok(());
    }
    sqlx::query!(
        r#"
        INSERT INTO node_aggregate_stale (tenant_id, node_id)
        SELECT app_current_tenant(), id FROM node WHERE id = ANY($1)
        ON CONFLICT (tenant_id, node_id) DO NOTHING
        "#,
        nodes
    )
    .execute(&mut *conn)
    .await?;
    let pending = crate::jobs::next_queued_in(conn, AGGREGATE_JOB).await?;
    if let Some(delay) = recompute_in(pending) {
        crate::jobs::enqueue_in(conn, AGGREGATE_JOB, &serde_json::json!({}), delay).await?;
    }
    Ok(())
}

/// Marks every node of the tenant: a full recompute.
pub async fn mark_all_stale(conn: &mut PgConnection) -> Result<(), sqlx::Error> {
    let nodes = sqlx::query_scalar!("SELECT id FROM node")
        .fetch_all(&mut *conn)
        .await?;
    mark_stale(conn, &nodes).await
}

/// Sets what is measured on `project` by itself, and marks its aggregates
/// stale. False if it is no project of the tenant.
pub async fn set_figures(
    conn: &mut PgConnection,
    project: Uuid,
    figures: &Figures,
) -> Result<bool, sqlx::Error> {
    let written = sqlx::query!(
        r#"
        INSERT INTO project_figure (tenant_id, node_id, planned_minutes, capacity_minutes,
                                    tickets_todo, tickets_in_progress, tickets_done, tickets_late)
        SELECT app_current_tenant(), id, $2, $3, $4, $5, $6, $7
        FROM node WHERE id = $1 AND node_type = 'project'
        ON CONFLICT (tenant_id, node_id) DO UPDATE
            SET planned_minutes = EXCLUDED.planned_minutes,
                capacity_minutes = EXCLUDED.capacity_minutes,
                tickets_todo = EXCLUDED.tickets_todo,
                tickets_in_progress = EXCLUDED.tickets_in_progress,
                tickets_done = EXCLUDED.tickets_done,
                tickets_late = EXCLUDED.tickets_late,
                version = project_figure.version + 1
        "#,
        project,
        stored(figures.planned_minutes),
        stored(figures.capacity_minutes),
        stored(figures.todo),
        stored(figures.in_progress),
        stored(figures.done),
        stored(figures.late)
    )
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if written == 0 {
        return Ok(false);
    }
    mark_stale(conn, &[project]).await?;
    Ok(true)
}

/// The links and projects of the tenant.
async fn hierarchy(conn: &mut PgConnection) -> Result<Hierarchy, sqlx::Error> {
    let links: Vec<Link> = sqlx::query!("SELECT parent_id, child_id, weight_percent FROM node_edge")
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|row| Link {
            parent: row.parent_id.as_u128(),
            child: row.child_id.as_u128(),
            // 0 to 100 (`CHECK`).
            weight_percent: u8::try_from(row.weight_percent).unwrap_or(100),
        })
        .collect();
    let projects = sqlx::query!(
        r#"
        SELECT n.id, n.archived_at IS NOT NULL AS "archived!", n.confidential,
               coalesce(f.planned_minutes, 0) AS "planned_minutes!",
               coalesce(f.capacity_minutes, 0) AS "capacity_minutes!",
               coalesce(f.tickets_todo, 0) AS "todo!",
               coalesce(f.tickets_in_progress, 0) AS "in_progress!",
               coalesce(f.tickets_done, 0) AS "done!",
               coalesce(f.tickets_late, 0) AS "late!"
        FROM node n
        LEFT JOIN project_figure f ON f.node_id = n.id
        WHERE n.node_type = 'project'
        "#
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|row| {
        let project = Project {
            figures: Figures {
                planned_minutes: read(row.planned_minutes),
                capacity_minutes: read(row.capacity_minutes),
                todo: read(row.todo),
                in_progress: read(row.in_progress),
                done: read(row.done),
                late: read(row.late),
            },
            archived: row.archived,
            confidential: row.confidential,
        };
        (row.id.as_u128(), project)
    });
    Ok(Hierarchy::new(projects, &links))
}

/// Recomputes the aggregates of the tenant's stale nodes and of all their
/// ancestors; returns how many nodes were computed. One recompute at a time
/// per tenant: a second one waits, then finds what is left.
pub async fn recompute(conn: &mut PgConnection) -> Result<usize, sqlx::Error> {
    sqlx::query!(
        "SELECT pg_advisory_xact_lock(hashtextextended('node_aggregate:' || app_current_tenant()::text, 0))"
    )
    .fetch_one(&mut *conn)
    .await?;
    // Marks made from now on are for the next job: whoever makes them finds
    // this one running and queues another.
    let stale: Vec<u128> = sqlx::query_scalar!("DELETE FROM node_aggregate_stale RETURNING node_id")
        .fetch_all(&mut *conn)
        .await?
        .iter()
        .map(Uuid::as_u128)
        .collect();
    if stale.is_empty() {
        return Ok(0);
    }
    let hierarchy = hierarchy(conn).await?;
    let nodes: Vec<Uuid> = hierarchy
        .affected(&stale)
        .into_iter()
        .map(Uuid::from_u128)
        .collect();
    let aggregates: Vec<Aggregate> = nodes.iter().map(|id| hierarchy.aggregate(id.as_u128())).collect();
    let column =
        |pick: fn(&Aggregate) -> u64| -> Vec<i64> { aggregates.iter().map(|a| stored(pick(a))).collect() };
    // Only what changed is written; a node deleted meanwhile is skipped.
    sqlx::query!(
        r#"
        INSERT INTO node_aggregate (tenant_id, node_id, projects, confidential_projects,
                                    planned_minutes, capacity_minutes, tickets_todo, tickets_in_progress,
                                    tickets_done, tickets_late, confidential_open_tickets)
        SELECT app_current_tenant(), n.id, a.projects::integer, a.confidential_projects::integer,
               a.planned_minutes, a.capacity_minutes, a.todo, a.in_progress, a.done, a.late,
               a.confidential_open
        FROM unnest($1::uuid[], $2::bigint[], $3::bigint[], $4::bigint[], $5::bigint[], $6::bigint[],
                    $7::bigint[], $8::bigint[], $9::bigint[], $10::bigint[])
            AS a (node_id, projects, confidential_projects, planned_minutes, capacity_minutes, todo,
                  in_progress, done, late, confidential_open)
        JOIN node n ON n.id = a.node_id
        ON CONFLICT (tenant_id, node_id) DO UPDATE
            SET projects = EXCLUDED.projects,
                confidential_projects = EXCLUDED.confidential_projects,
                planned_minutes = EXCLUDED.planned_minutes,
                capacity_minutes = EXCLUDED.capacity_minutes,
                tickets_todo = EXCLUDED.tickets_todo,
                tickets_in_progress = EXCLUDED.tickets_in_progress,
                tickets_done = EXCLUDED.tickets_done,
                tickets_late = EXCLUDED.tickets_late,
                confidential_open_tickets = EXCLUDED.confidential_open_tickets,
                version = node_aggregate.version + 1
            WHERE (node_aggregate.projects, node_aggregate.confidential_projects,
                   node_aggregate.planned_minutes, node_aggregate.capacity_minutes,
                   node_aggregate.tickets_todo, node_aggregate.tickets_in_progress,
                   node_aggregate.tickets_done, node_aggregate.tickets_late,
                   node_aggregate.confidential_open_tickets)
                IS DISTINCT FROM
                  (EXCLUDED.projects, EXCLUDED.confidential_projects,
                   EXCLUDED.planned_minutes, EXCLUDED.capacity_minutes,
                   EXCLUDED.tickets_todo, EXCLUDED.tickets_in_progress,
                   EXCLUDED.tickets_done, EXCLUDED.tickets_late,
                   EXCLUDED.confidential_open_tickets)
        "#,
        &nodes,
        &column(|a| u64::from(a.projects)),
        &column(|a| u64::from(a.confidential_projects)),
        &column(|a| a.figures.planned_minutes),
        &column(|a| a.figures.capacity_minutes),
        &column(|a| a.figures.todo),
        &column(|a| a.figures.in_progress),
        &column(|a| a.figures.done),
        &column(|a| a.figures.late),
        &column(|a| a.confidential_open)
    )
    .execute(&mut *conn)
    .await?;
    Ok(nodes.len())
}

/// The stored aggregate of each of `nodes`; those never computed are
/// missing.
pub async fn find(conn: &mut PgConnection, nodes: &[Uuid]) -> Result<HashMap<Uuid, Aggregate>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT node_id, projects, confidential_projects, planned_minutes, capacity_minutes,
               tickets_todo, tickets_in_progress, tickets_done, tickets_late, confidential_open_tickets
        FROM node_aggregate WHERE node_id = ANY($1)
        "#,
        nodes
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let aggregate = Aggregate {
                projects: u32::try_from(row.projects).unwrap_or(0),
                confidential_projects: u32::try_from(row.confidential_projects).unwrap_or(0),
                figures: Figures {
                    planned_minutes: read(row.planned_minutes),
                    capacity_minutes: read(row.capacity_minutes),
                    todo: read(row.tickets_todo),
                    in_progress: read(row.tickets_in_progress),
                    done: read(row.tickets_done),
                    late: read(row.tickets_late),
                },
                confidential_open: read(row.confidential_open_tickets),
            };
            (row.node_id, aggregate)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_fit_what_postgresql_stores() {
        assert_eq!(stored(42), 42);
        assert_eq!(stored(u64::MAX), i64::MAX);
        assert_eq!(read(42), 42);
        assert_eq!(read(-1), 0);
    }
}
