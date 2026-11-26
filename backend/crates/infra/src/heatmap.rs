//! SQL of the workload heatmap (specification §8, L08-US06): the resources
//! it shows, the load on tickets the reader does not see, and what is
//! behind a cell. The sums themselves come from `crate::load_aggregates`.

use std::collections::HashMap;

use domain::assignments::Mode;
use domain::dates::Date as LocalDate;
use domain::resources::Kind;
use sqlx::postgres::PgConnection;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::capacity::local_date;

/// A resource of the heatmap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeatmapResource {
    pub id: Uuid,
    pub kind: Kind,
    /// The member's name when linked, else its own.
    pub display_name: String,
    pub job_title: Option<String>,
    pub linked: bool,
    /// Its site calendar; none: the tenant calendar.
    pub calendar_id: Option<Uuid>,
}

/// Which resources the heatmap shows.
#[derive(Debug, Clone, Copy)]
pub struct Filter {
    /// The members of this team.
    pub team_id: Option<Uuid>,
    /// Those with load from `from` to `to` on tickets of this node's
    /// projects (the node itself, or those below it).
    pub node_id: Option<Uuid>,
    pub from: Date,
    pub to: Date,
}

/// The active resources of `filter`, by name.
pub async fn resources(
    conn: &mut PgConnection,
    filter: &Filter,
) -> Result<Vec<HeatmapResource>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT r.id, r.kind, coalesce(app_member_name(r.account_id), r.display_name) AS "display_name!",
               r.job_title, r.account_id IS NOT NULL AS "linked!", r.calendar_id
        FROM resource r
        WHERE r.active
          AND ($1::uuid IS NULL OR EXISTS (
                  SELECT 1 FROM team_member tm WHERE tm.team_id = $1 AND tm.resource_id = r.id))
          AND ($2::uuid IS NULL OR EXISTS (
                  SELECT 1 FROM resource_day_load l
                  JOIN issue i ON i.id = l.issue_id
                  JOIN node_closure c ON c.descendant_id = i.project_id AND c.ancestor_id = $2
                  WHERE l.resource_id = r.id AND l.day BETWEEN $3 AND $4))
        ORDER BY lower(coalesce(app_member_name(r.account_id), r.display_name)), r.id
        "#,
        filter.team_id,
        filter.node_id,
        filter.from,
        filter.to
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| HeatmapResource {
            id: r.id,
            kind: Kind::parse(&r.kind).unwrap_or(Kind::Person),
            display_name: r.display_name,
            job_title: r.job_title,
            linked: r.linked,
            calendar_id: r.calendar_id,
        })
        .collect())
}

/// The minutes of `resources` on tickets of projects outside `visible`, by
/// resource and day, from `from` to `to`.
pub async fn hidden_by_day(
    conn: &mut PgConnection,
    resources: &[Uuid],
    from: Date,
    to: Date,
    visible: &[Uuid],
) -> Result<HashMap<Uuid, Vec<(LocalDate, u32)>>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT l.resource_id, l.day, sum(l.minutes)::integer AS "minutes!"
        FROM resource_day_load l
        JOIN issue i ON i.id = l.issue_id
        WHERE l.resource_id = ANY($1) AND l.day BETWEEN $2 AND $3 AND NOT (i.project_id = ANY($4))
        GROUP BY l.resource_id, l.day
        "#,
        resources,
        from,
        to,
        visible
    )
    .fetch_all(conn)
    .await?;
    let mut hidden: HashMap<Uuid, Vec<(LocalDate, u32)>> = HashMap::new();
    for row in rows {
        if let Some(day) = local_date(row.day) {
            hidden
                .entry(row.resource_id)
                .or_default()
                .push((day, u32::try_from(row.minutes).unwrap_or(0)));
        }
    }
    Ok(hidden)
}

/// A ticket behind a cell: its load on the resource over the period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellItem {
    pub assignment_id: Uuid,
    pub issue_id: Uuid,
    pub key: String,
    pub summary: String,
    pub project_id: Uuid,
    pub project_key: String,
    pub project_name: String,
    pub minutes: u32,
    pub provisional: bool,
    /// The modes and values of its segments meeting the period.
    pub segments: Vec<(Mode, u32)>,
}

/// The tickets behind the load of `resource_id` from `from` to `to`, the
/// most loaded first.
pub async fn cell_items(
    conn: &mut PgConnection,
    resource_id: Uuid,
    from: Date,
    to: Date,
) -> Result<Vec<CellItem>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT l.assignment_id, l.issue_id, i.key, i.summary, i.project_id, n.key AS "project_key!",
               n.name AS project_name, sum(l.minutes)::integer AS "minutes!", bool_or(l.provisional) AS "provisional!",
               coalesce((
                   SELECT array_agg(s.mode ORDER BY s.position) FROM assignment_segment s
                   WHERE s.assignment_id = l.assignment_id AND s.end_date >= $2 AND s.start_date <= $3
               ), '{}') AS "modes!",
               coalesce((
                   SELECT array_agg(s.value ORDER BY s.position) FROM assignment_segment s
                   WHERE s.assignment_id = l.assignment_id AND s.end_date >= $2 AND s.start_date <= $3
               ), '{}') AS "values!"
        FROM resource_day_load l
        JOIN issue i ON i.id = l.issue_id
        JOIN node n ON n.id = i.project_id
        WHERE l.resource_id = $1 AND l.day BETWEEN $2 AND $3
        GROUP BY l.assignment_id, l.issue_id, i.key, i.summary, i.project_id, n.key, n.name
        ORDER BY sum(l.minutes) DESC, i.key
        "#,
        resource_id,
        from,
        to
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| CellItem {
            assignment_id: r.assignment_id,
            issue_id: r.issue_id,
            key: r.key,
            summary: r.summary,
            project_id: r.project_id,
            project_key: r.project_key,
            project_name: r.project_name,
            minutes: u32::try_from(r.minutes).unwrap_or(0),
            provisional: r.provisional,
            segments: r
                .modes
                .iter()
                .zip(&r.values)
                .filter_map(|(mode, value)| Some((Mode::parse(mode)?, u32::try_from(*value).ok()?)))
                .collect(),
        })
        .collect())
}

/// The projects of `node`: itself if it is one, or those below it.
pub async fn projects_under(conn: &mut PgConnection, node: Uuid) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        r#"
        SELECT c.descendant_id AS "id!" FROM node_closure c
        JOIN node n ON n.id = c.descendant_id
        WHERE c.ancestor_id = $1 AND n.node_type = 'project'
        "#,
        node
    )
    .fetch_all(conn)
    .await
}

/// An overload forced on the resource, as a cell tells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForcedNote {
    pub issue_id: Uuid,
    /// None: no longer a member.
    pub actor_name: Option<String>,
    pub created_at: OffsetDateTime,
}

/// The overloads forced on `resource_id` with a day from `from` to `to`,
/// newest first.
pub async fn forced_in(
    conn: &mut PgConnection,
    resource_id: Uuid,
    from: Date,
    to: Date,
) -> Result<Vec<ForcedNote>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT f.issue_id, app_member_name(f.actor_id) AS "actor_name?", f.created_at
        FROM forced_overload f
        WHERE f.resource_id = $1
          AND EXISTS (
              SELECT 1 FROM jsonb_array_elements(f.days) d
              WHERE (d ->> 'date')::date BETWEEN $2 AND $3
          )
        ORDER BY f.created_at DESC
        "#,
        resource_id,
        from,
        to
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| ForcedNote {
            issue_id: r.issue_id,
            actor_name: r.actor_name,
            created_at: r.created_at,
        })
        .collect())
}
