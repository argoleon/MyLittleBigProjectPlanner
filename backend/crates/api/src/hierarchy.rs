//! Browsing the hierarchy with its aggregates (specification §4, L03-US05).
//!
//! - `GET /hierarchy` gives one level at a time: the nodes at the top of
//!   what the member sees, or the children of a node (`?under=`). Each row
//!   carries the aggregate of its node, so that a tree is loaded level by
//!   level.
//! - `GET /nodes/{id}/aggregate` gives the aggregate of one node.
//!
//! Aggregates are precomputed by the worker (`infra::aggregates`) over the
//! distinct descendant projects of each node, confidential ones included: a
//! member who sees a node sees its totals. A confidential project they have
//! no access to shows veiled in the rows, with its planned load only.

use axum::extract::State;
use domain::aggregates::Aggregate;
use domain::grants::{Projection, projection};
use infra::nodes::{self as repo, BrowseRow};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::nodes::{HealthView, NodeTypeView, OwnerView, ParentView, Sight, TAG, visible_node};
use crate::problem::{PROBLEM_JSON, Problem, ProblemBody};
use crate::tenant::{AppState, TenantTx};

/// What a node aggregates over its distinct descendant projects (itself
/// included, for a project). Archived projects are left out.
#[derive(Debug, Default, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = NodeAggregate)]
pub struct AggregateView {
    /// Distinct projects counted: one reached through several paths counts
    /// once.
    #[schema(example = 6)]
    projects: u32,
    /// Those of them that are confidential.
    #[schema(example = 1)]
    confidential_projects: u32,
    /// Planned load, in minutes. The weighting of a link applies to the
    /// parent of that link only.
    #[schema(example = 212_400)]
    planned_minutes: u64,
    /// Capacity of the teams assigned, in minutes, weighted like the load.
    #[schema(example = 246_900)]
    capacity_minutes: u64,
    /// Planned load over capacity, in percent; `null` without capacity.
    #[schema(example = 86)]
    capacity_used_percent: Option<u32>,
    /// Tickets not done yet.
    #[schema(example = 276)]
    open_tickets: u64,
    /// Open tickets past their due date.
    #[schema(example = 17)]
    late_tickets: u64,
    /// Share of the tickets done, in percent rounded down; `null` without
    /// tickets.
    #[schema(example = 46)]
    progress_percent: Option<u8>,
    /// Open tickets of the confidential projects.
    #[schema(example = 38)]
    confidential_open_tickets: u64,
}

impl From<Aggregate> for AggregateView {
    fn from(aggregate: Aggregate) -> Self {
        let figures = aggregate.figures;
        Self {
            projects: aggregate.projects,
            confidential_projects: aggregate.confidential_projects,
            planned_minutes: figures.planned_minutes,
            capacity_minutes: figures.capacity_minutes,
            capacity_used_percent: figures.capacity_used_percent(),
            open_tickets: figures.open(),
            late_tickets: figures.late,
            progress_percent: figures.progress_percent(),
            confidential_open_tickets: aggregate.confidential_open,
        }
    }
}

/// A node of the hierarchy as a row of its tree, with its aggregate.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = HierarchyNode)]
pub struct RowView {
    id: Uuid,
    #[serde(rename = "type")]
    node_type: NodeTypeView,
    #[schema(example = "Identity platform")]
    name: String,
    /// Projects only.
    #[schema(example = "PLAT")]
    key: Option<String>,
    health: HealthView,
    owners: Vec<OwnerView>,
    /// Weighting of the link to the node browsed; `null` at the top.
    #[schema(example = 100)]
    weight_percent: Option<i16>,
    /// Its parents the member sees, other than the node browsed.
    other_parents: Vec<ParentView>,
    /// It has children: browse it (`?under=`) for the next level.
    has_children: bool,
    aggregate: AggregateView,
}

/// A confidential project the member has no access to, as a row of the
/// tree: its planned load, without name or detail.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = VeiledHierarchyNode)]
pub struct VeiledRowView {
    /// Always `true`: tells a veiled row from a node's.
    #[schema(example = true)]
    veiled: bool,
    /// An opaque identifier: stable, but not the node's id, and of no use
    /// to read it.
    id: Uuid,
    #[serde(rename = "type")]
    node_type: NodeTypeView,
    /// Weighting of the link to the node browsed.
    #[schema(example = 100)]
    weight_percent: Option<i16>,
    /// Its planned load, in minutes.
    #[schema(example = 36_000)]
    planned_minutes: u64,
}

/// A row of the hierarchy: a node the member sees, or a veiled one.
#[derive(Debug, Serialize, ToSchema)]
#[serde(untagged)]
#[schema(as = HierarchyRow)]
pub enum RowItemView {
    Node(Box<RowView>),
    Veiled(VeiledRowView),
}

/// `row`, found under `under` (`None`: at the top), as it shows to the
/// member: whole, veiled, or not at all.
fn row_item(row: BrowseRow, aggregate: Aggregate, under: Option<Uuid>, sight: &Sight) -> Option<RowItemView> {
    // Under a node: the member sees that parent. At the top: only nodes
    // they see are listed.
    match projection(sight.sees(row.id), row.confidential, under.is_some()) {
        Projection::Full => Some(RowItemView::Node(Box::new(RowView {
            id: row.id,
            node_type: row.node_type.into(),
            name: row.name,
            key: row.key,
            health: row.health.into(),
            owners: row.owners.into_iter().map(OwnerView::from).collect(),
            weight_percent: row.weight_percent,
            other_parents: sight.refs(
                row.parents
                    .into_iter()
                    .filter(|parent| Some(parent.id) != under)
                    .collect(),
            ),
            has_children: row.has_children,
            aggregate: aggregate.into(),
        }))),
        Projection::Veiled => Some(RowItemView::Veiled(VeiledRowView {
            veiled: true,
            id: repo::opaque_id(row.id),
            node_type: row.node_type.into(),
            weight_percent: row.weight_percent,
            planned_minutes: aggregate.figures.planned_minutes,
        })),
        Projection::Hidden => None,
    }
}

/// `?under=` of the hierarchy browse.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub struct BrowseParams {
    /// The node whose children are listed; without it, the top of the
    /// hierarchy.
    pub under: Option<Uuid>,
}

/// `GET /api/v1/hierarchy`.
#[utoipa::path(
    get,
    path = "/hierarchy",
    tag = TAG,
    operation_id = "browseHierarchy",
    summary = "Browse one level of the hierarchy, with aggregates",
    description = "Without `under`: the nodes at the top of what the member sees, that is without \
                   a parent, or without a parent they see (all root nodes for tenant admins). With \
                   `under`: the children of that node. Portfolios first, then programs and \
                   projects, each by name; archived nodes are left out. Every row carries the \
                   aggregate of its node over its distinct descendant projects. A confidential \
                   child the member has no access to comes veiled (`veiled: true`): its planned \
                   load, without name or detail.",
    params(BrowseParams),
    responses(
        (status = 200, description = "The rows of the level", body = Vec<RowItemView>),
        (status = 400, description = "Invalid `under`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such node in this tenant, or not visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn browse(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiQuery(params): ApiQuery<BrowseParams>,
) -> Result<ApiJson<Vec<RowItemView>>, Problem> {
    let sight = Sight::of(&mut tx).await?;
    let rows = match params.under {
        Some(under) => {
            visible_node(&state, &mut tx, under, "no such node").await?;
            repo::browse(&mut tx, Some(under), None).await?
        }
        None => {
            let visible = sight.visible();
            repo::browse(&mut tx, None, visible.as_deref()).await?
        }
    };
    let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
    let mut aggregates = infra::aggregates::find(&mut tx, &ids).await?;
    tx.commit().await?;
    Ok(ApiJson(
        rows.into_iter()
            .filter_map(|row| {
                // Not computed yet: nothing to show.
                let aggregate = aggregates.remove(&row.id).unwrap_or_default();
                row_item(row, aggregate, params.under, &sight)
            })
            .collect(),
    ))
}

/// `GET /api/v1/nodes/{id}/aggregate`.
#[utoipa::path(
    get,
    path = "/nodes/{id}/aggregate",
    tag = TAG,
    operation_id = "getNodeAggregate",
    summary = "Read the aggregate of a node",
    description = "Planned load, consumed capacity, progress, open and late tickets over the \
                   distinct descendant projects of the node, confidential ones included and \
                   archived ones left out. Precomputed: it follows a change within seconds.",
    params(("id" = Uuid, Path, description = "Id of the node")),
    responses(
        (status = 200, description = "The aggregate", body = AggregateView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such node in this tenant, or not visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn aggregate(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<ApiJson<AggregateView>, Problem> {
    visible_node(&state, &mut tx, id, "no such node").await?;
    let found = infra::aggregates::find(&mut tx, &[id]).await?.remove(&id);
    tx.commit().await?;
    Ok(ApiJson(found.unwrap_or_default().into()))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use domain::aggregates::Figures;
    use domain::hierarchy::{Health, NodeType};
    use infra::nodes::{NodeRef, Owner};
    use serde_json::json;

    use super::*;

    const PORTFOLIO: Uuid = Uuid::from_u128(1);
    const PROGRAM: Uuid = Uuid::from_u128(2);
    const OTHER_PROGRAM: Uuid = Uuid::from_u128(3);
    const PLAT: Uuid = Uuid::from_u128(4);

    fn parent(id: Uuid, name: &str) -> NodeRef {
        NodeRef {
            id,
            node_type: NodeType::Program,
            name: name.to_owned(),
        }
    }

    fn platform(confidential: bool) -> BrowseRow {
        BrowseRow {
            id: PLAT,
            node_type: NodeType::Project,
            name: "Identity platform".to_owned(),
            key: Some("PLAT".to_owned()),
            health: Health::AtRisk,
            confidential,
            weight_percent: Some(60),
            has_children: true,
            owners: vec![Owner {
                account_id: Uuid::from_u128(9),
                display_name: "Camille Martin".to_owned(),
            }],
            parents: vec![
                parent(PROGRAM, "Modernization"),
                parent(OTHER_PROGRAM, "Compliance"),
            ],
        }
    }

    fn aggregate_of_platform() -> Aggregate {
        Aggregate {
            projects: 2,
            confidential_projects: 1,
            figures: Figures {
                planned_minutes: 37_200,
                capacity_minutes: 43_200,
                todo: 30,
                in_progress: 12,
                done: 58,
                late: 1,
            },
            confidential_open: 9,
        }
    }

    fn limited(visible: &[Uuid]) -> Sight {
        Sight::Limited(infra::grants::Sight {
            visible: visible.iter().copied().collect(),
            veiled: HashSet::new(),
        })
    }

    fn as_json(item: Option<RowItemView>) -> serde_json::Value {
        serde_json::to_value(item).unwrap_or_default()
    }

    #[test]
    fn an_aggregate_carries_its_percentages() {
        let view = AggregateView::from(aggregate_of_platform());
        assert_eq!(
            serde_json::to_value(&view).unwrap_or_default(),
            json!({
                "projects": 2,
                "confidentialProjects": 1,
                "plannedMinutes": 37_200,
                "capacityMinutes": 43_200,
                "capacityUsedPercent": 86,
                "openTickets": 42,
                "lateTickets": 1,
                "progressPercent": 58,
                "confidentialOpenTickets": 9,
            })
        );
        // Never computed, or nothing to measure: zeros, and no percentage.
        let empty = serde_json::to_value(AggregateView::from(Aggregate::default())).unwrap_or_default();
        assert_eq!(empty["projects"], 0);
        assert_eq!(empty["capacityUsedPercent"], json!(null));
        assert_eq!(empty["progressPercent"], json!(null));
    }

    #[test]
    fn a_row_carries_the_node_its_aggregate_and_its_other_parents() {
        let row = as_json(row_item(
            platform(false),
            aggregate_of_platform(),
            Some(PROGRAM),
            &Sight::All,
        ));
        assert_eq!(row["id"], json!(PLAT));
        assert_eq!(
            (&row["type"], &row["name"], &row["key"]),
            (&json!("project"), &json!("Identity platform"), &json!("PLAT"))
        );
        assert_eq!(row["health"], "at_risk");
        assert_eq!(row["owners"][0]["displayName"], "Camille Martin");
        assert_eq!(row["weightPercent"], 60);
        assert_eq!(row["hasChildren"], true);
        assert_eq!(row["aggregate"]["plannedMinutes"], 37_200);
        // The parent browsed is not repeated.
        assert_eq!(
            row["otherParents"],
            json!([{"id": OTHER_PROGRAM, "type": "program", "name": "Compliance"}])
        );
        assert_eq!(row.get("veiled"), None);

        // At the top, every parent the member sees is another parent.
        let top = as_json(row_item(
            BrowseRow {
                weight_percent: None,
                ..platform(false)
            },
            Aggregate::default(),
            None,
            &limited(&[PLAT, PROGRAM]),
        ));
        assert_eq!(top["weightPercent"], json!(null));
        assert_eq!(
            top["otherParents"],
            json!([{"id": PROGRAM, "type": "program", "name": "Modernization"}]),
            "no name of a node they do not see"
        );
        assert_eq!(top["aggregate"]["projects"], 0);
    }

    #[test]
    fn a_confidential_child_out_of_reach_shows_its_load_only() {
        let sight = limited(&[PORTFOLIO, PROGRAM]);
        let row = as_json(row_item(
            platform(true),
            aggregate_of_platform(),
            Some(PROGRAM),
            &sight,
        ));
        assert_eq!(
            row,
            json!({
                "veiled": true,
                "id": repo::opaque_id(PLAT),
                "type": "project",
                "weightPercent": 60,
                "plannedMinutes": 37_200,
            }),
            "neither name, key, owners, health nor tickets"
        );
        assert_ne!(row["id"], json!(PLAT));

        // With a role on the project itself, it shows whole.
        let whole = as_json(row_item(
            platform(true),
            aggregate_of_platform(),
            Some(PROGRAM),
            &limited(&[PROGRAM, PLAT]),
        ));
        assert_eq!(whole["name"], "Identity platform");
        // Tenant admins see everything.
        let whole = as_json(row_item(
            platform(true),
            aggregate_of_platform(),
            Some(PROGRAM),
            &Sight::All,
        ));
        assert_eq!(whole["name"], "Identity platform");
    }

    #[test]
    fn nodes_out_of_sight_are_not_rows() {
        // At the top, a node the member does not see is no row, veiled or not.
        assert!(row_item(platform(true), Aggregate::default(), None, &limited(&[PROGRAM])).is_none());
        assert!(row_item(platform(false), Aggregate::default(), None, &limited(&[PROGRAM])).is_none());
        // Nor is, under a node, one that is neither seen nor confidential.
        assert!(
            row_item(
                platform(false),
                Aggregate::default(),
                Some(PROGRAM),
                &limited(&[PROGRAM])
            )
            .is_none()
        );
    }
}
