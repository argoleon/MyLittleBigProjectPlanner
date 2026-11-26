//! The workload heatmap (specification §8, L08-US06): the active resources
//! by day, week or month, each cell the load against the capacity, and the
//! tickets behind a cell. Internal members read it; load on tickets they do
//! not see (a confidential project, or one not granted to them) counts in
//! the cells but shows as busy time, without name.
//!
//! Weeks and months come from the precomputed sums (`load_aggregate`);
//! those missing, and days, are computed on the fly.

use std::collections::{HashMap, HashSet};

use axum::extract::State;
use domain::assignments::Mode;
use domain::workload::{Period, Scale, periods, sum_by_period};
use infra::capacity::{local_date, sql_date};
use infra::heatmap::{self as repo, Filter, HeatmapResource};
use infra::load_aggregates::{self as aggregates, Sums};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};
use utoipa::{IntoParams, ToSchema};

use crate::assignments::{ModeView, ProjectRefView};
use crate::extract::{ApiJson, ApiQuery};
use crate::nodes::{Sight, visible_node};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::resources::{KindView, TAG};
use crate::tenant::{AppState, TenantTx};
use domain::validation::FieldError;

/// The span of a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = HeatmapScale)]
pub enum ScaleView {
    /// A day: up to 92 columns.
    Day,
    /// A week, from Monday: up to 53 columns.
    #[default]
    Week,
    /// A month: up to 24 columns.
    Month,
}

impl From<ScaleView> for Scale {
    fn from(scale: ScaleView) -> Self {
        match scale {
            ScaleView::Day => Self::Day,
            ScaleView::Week => Self::Week,
            ScaleView::Month => Self::Month,
        }
    }
}

/// `?scale=&from=&to=&teamId=&nodeId=&includeProvisional=` of the heatmap.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct HeatmapParams {
    /// `week` when left out.
    #[serde(default)]
    scale: ScaleView,
    /// First day: its period is the first column.
    #[param(value_type = String, format = Date)]
    from: Date,
    /// Last day: its period is the last column.
    #[param(value_type = String, format = Date)]
    to: Date,
    /// Only the members of this team.
    team_id: Option<Uuid>,
    /// Only those with load in the range on tickets of this project, or of
    /// the projects below this program or portfolio. Their whole load shows.
    node_id: Option<Uuid>,
    /// Provisional assignments count in the load (`true` when left out).
    include_provisional: Option<bool>,
}

/// A column.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = HeatmapPeriod)]
pub struct PeriodView {
    #[schema(value_type = String, format = Date, example = "2026-11-16")]
    start: Date,
    /// Last day, included.
    #[schema(value_type = String, format = Date, example = "2026-11-22")]
    end: Date,
}

/// A resource of the heatmap.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = HeatmapResource)]
pub struct ResourceRefView {
    id: Uuid,
    kind: KindView,
    #[schema(example = "Camille Martin")]
    display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    job_title: Option<String>,
    /// Linked to a member's account.
    linked: bool,
}

/// A row: one resource, one value per column of each series, in minutes.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = HeatmapRow)]
pub struct RowView {
    resource: ResourceRefView,
    /// Load planned, provisional included unless left out.
    load: Vec<u32>,
    /// Its provisional part (0 when provisional assignments are left out).
    provisional: Vec<u32>,
    /// Its part on tickets the member does not see: busy time.
    hidden: Vec<u32>,
    /// Capacity, absences deducted.
    capacity: Vec<u32>,
    /// Capacity before absences: more than `capacity` when absences take
    /// some, 0 on a period with no working day.
    scheduled: Vec<u32>,
}

/// The heatmap: columns, then a row per resource, by name.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Heatmap)]
pub struct HeatmapView {
    scale: ScaleView,
    periods: Vec<PeriodView>,
    rows: Vec<RowView>,
    /// The tenant's overload tolerance: a cell overloads beyond the
    /// capacity plus this share.
    #[schema(example = 0)]
    tolerance_percent: u8,
    /// Provisional assignments count in `load`.
    include_provisional: bool,
}

pub(crate) fn checked_range(from: Date, to: Date, scale: Scale) -> Result<Vec<Period>, Problem> {
    if to < from {
        return Err(Problem::validation(vec![FieldError::new("to", "before_start")]));
    }
    let (Some(first), Some(last)) = (local_date(from), local_date(to)) else {
        return Err(Problem::validation(vec![FieldError::new("from", "out_of_range")]));
    };
    let columns = periods(scale, first, last);
    if columns.len() > scale.max_periods() {
        return Err(Problem::validation(vec![FieldError::new("to", "out_of_range")]));
    }
    Ok(columns)
}

fn resource_view(resource: &HeatmapResource) -> ResourceRefView {
    ResourceRefView {
        id: resource.id,
        kind: resource.kind.into(),
        display_name: resource.display_name.clone(),
        job_title: resource.job_title.clone(),
        linked: resource.linked,
    }
}

/// The sums of `resources` over `columns`: stored ones for weeks and
/// months, the others computed (and those of the horizon marked, for the
/// worker to store).
pub(crate) async fn sums_of(
    tx: &mut TenantTx,
    resources: &[HeatmapResource],
    scale: Scale,
    columns: &[Period],
) -> Result<HashMap<Uuid, Vec<Sums>>, Problem> {
    let keys: Vec<(Uuid, Option<Uuid>)> = resources.iter().map(|r| (r.id, r.calendar_id)).collect();
    let Some(kind) = aggregates::kind_of(scale) else {
        return Ok(aggregates::compute(tx, &keys, columns).await?);
    };
    let starts: Vec<Date> = columns.iter().filter_map(|p| sql_date(p.start)).collect();
    let ids: Vec<Uuid> = resources.iter().map(|r| r.id).collect();
    let stored = aggregates::stored(tx, &ids, kind, &starts).await?;
    let missing: Vec<(Uuid, Option<Uuid>)> = keys
        .iter()
        .filter(|(id, _)| starts.iter().any(|start| !stored.contains_key(&(*id, *start))))
        .copied()
        .collect();
    let computed = aggregates::compute(tx, &missing, columns).await?;
    if !missing.is_empty() {
        let missing_ids: Vec<Uuid> = missing.iter().map(|(id, _)| *id).collect();
        aggregates::mark_stale(
            tx,
            &missing_ids,
            starts.first().copied(),
            columns.last().and_then(|p| sql_date(p.end)),
        )
        .await?;
    }
    Ok(ids
        .iter()
        .map(|id| {
            let row = starts
                .iter()
                .enumerate()
                .map(|(i, start)| {
                    stored
                        .get(&(*id, *start))
                        .copied()
                        .or_else(|| computed.get(id).and_then(|v| v.get(i)).copied())
                        .unwrap_or_default()
                })
                .collect();
            (*id, row)
        })
        .collect())
}

/// `GET /api/v1/workload/heatmap`.
#[utoipa::path(
    get,
    path = "/workload/heatmap",
    tag = TAG,
    operation_id = "getWorkloadHeatmap",
    summary = "The workload heatmap",
    description = "Internal members: the active resources (of a team, or with load on a node's \
                   tickets), by name, and for each day, week or month of the range their load, its \
                   provisional part, its part on tickets the member does not see, and their capacity \
                   before and after absences.",
    params(HeatmapParams),
    responses(
        (status = 200, description = "The heatmap", body = HeatmapView),
        (status = 400, description = "`validation_failed` (`to`: `before_start`, `out_of_range` beyond 92 \
                                      days, 53 weeks or 24 months), or `invalid_query`",
         body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "`nodeId`: no such node visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn heatmap(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiQuery(params): ApiQuery<HeatmapParams>,
) -> Result<ApiJson<HeatmapView>, Problem> {
    let scale: Scale = params.scale.into();
    let columns = checked_range(params.from, params.to, scale)?;
    if let Some(node) = params.node_id {
        visible_node(&state, &mut tx, node, "no such node").await?;
    }
    let (Some(first), Some(last)) = (
        columns.first().and_then(|p| sql_date(p.start)),
        columns.last().and_then(|p| sql_date(p.end)),
    ) else {
        return Err(Problem::validation(vec![FieldError::new("from", "out_of_range")]));
    };
    let include_provisional = params.include_provisional.unwrap_or(true);
    let resources = repo::resources(
        &mut tx,
        &Filter {
            team_id: params.team_id,
            node_id: params.node_id,
            from: first,
            to: last,
        },
    )
    .await?;
    let sums = sums_of(&mut tx, &resources, scale, &columns).await?;
    let sight = Sight::of(&mut tx).await?;
    let hidden = match sight.visible() {
        None => HashMap::new(),
        Some(visible) => {
            let ids: Vec<Uuid> = resources.iter().map(|r| r.id).collect();
            repo::hidden_by_day(&mut tx, &ids, first, last, &visible).await?
        }
    };
    let tolerance = infra::overload::policy(&mut tx).await?.tolerance_percent;
    tx.commit().await?;
    let rows = resources
        .iter()
        .map(|resource| {
            let row = sums.get(&resource.id).cloned().unwrap_or_default();
            let at = |i: usize| row.get(i).copied().unwrap_or_default();
            let n = columns.len();
            let hidden = hidden
                .get(&resource.id)
                .map_or_else(|| vec![0; n], |days| sum_by_period(days, &columns));
            RowView {
                resource: resource_view(resource),
                load: (0..n)
                    .map(|i| {
                        let s = at(i);
                        if include_provisional {
                            s.load
                        } else {
                            s.load - s.provisional
                        }
                    })
                    .collect(),
                provisional: (0..n)
                    .map(|i| if include_provisional { at(i).provisional } else { 0 })
                    .collect(),
                hidden,
                capacity: (0..n).map(|i| at(i).capacity).collect(),
                scheduled: (0..n).map(|i| at(i).scheduled).collect(),
            }
        })
        .collect();
    Ok(ApiJson(HeatmapView {
        scale: params.scale,
        periods: columns
            .iter()
            .filter_map(|p| {
                Some(PeriodView {
                    start: sql_date(p.start)?,
                    end: sql_date(p.end)?,
                })
            })
            .collect(),
        rows,
        tolerance_percent: tolerance,
        include_provisional,
    }))
}

/// `?resourceId=&from=&to=&nodeId=` of a cell.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct CellParams {
    resource_id: Uuid,
    /// First day of the cell.
    #[param(value_type = String, format = Date)]
    from: Date,
    /// Last day, included (a year at most).
    #[param(value_type = String, format = Date)]
    to: Date,
    /// The node the heatmap is filtered on: tickets of its projects are
    /// marked (`inNode`).
    node_id: Option<Uuid>,
}

/// The mode and value of a segment behind a cell.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = HeatmapCellSegment)]
pub struct CellSegmentView {
    mode: ModeView,
    value: u32,
}

/// The ticket of a cell item.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = HeatmapCellIssue)]
pub struct CellIssueView {
    id: Uuid,
    #[schema(example = "PLAT-142")]
    key: String,
    summary: String,
    project: ProjectRefView,
}

/// What a cell's load is made of: a ticket, or busy time on tickets the
/// member does not see.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = HeatmapCellItem)]
pub struct CellItemView {
    /// `null` for busy time.
    #[schema(required = true)]
    issue: Option<CellIssueView>,
    #[schema(example = 1050)]
    minutes: u32,
    provisional: bool,
    /// The segments meeting the period: how the ticket loads it.
    segments: Vec<CellSegmentView>,
    /// A ticket of the node the heatmap is filtered on.
    in_node: bool,
}

/// An overload forced on the resource in the period.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = HeatmapForcedOverload)]
pub struct ForcedView {
    #[schema(example = "PLAT-148")]
    issue_key: String,
    /// `null`: no longer a member.
    #[schema(required = true)]
    actor_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    created_at: OffsetDateTime,
}

/// A cell: a resource over a period, and what makes up its load.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = HeatmapCell)]
pub struct CellView {
    resource: ResourceRefView,
    #[schema(value_type = String, format = Date)]
    from: Date,
    #[schema(value_type = String, format = Date)]
    to: Date,
    capacity_minutes: u32,
    scheduled_minutes: u32,
    /// All of it, provisional included.
    load_minutes: u32,
    provisional_minutes: u32,
    /// The most loaded first; busy time last.
    items: Vec<CellItemView>,
    /// On tickets the member sees, newest first.
    forced: Vec<ForcedView>,
}

/// `GET /api/v1/workload/cell`.
#[utoipa::path(
    get,
    path = "/workload/cell",
    tag = TAG,
    operation_id = "getWorkloadCell",
    summary = "What is behind a cell of the heatmap",
    description = "Internal members: a resource's capacity and load over a period, the tickets \
                   behind it with their hours (those the member does not see as one busy entry, \
                   without name), and the overloads forced there.",
    params(CellParams),
    responses(
        (status = 200, description = "The cell", body = CellView),
        (status = 400, description = "`validation_failed` (`to`: `before_start`, `out_of_range`)",
         body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such resource, or `nodeId` not visible", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn cell(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiQuery(params): ApiQuery<CellParams>,
) -> Result<ApiJson<CellView>, Problem> {
    if params.to < params.from {
        return Err(Problem::validation(vec![FieldError::new("to", "before_start")]));
    }
    if (params.to - params.from).whole_days() > 366 {
        return Err(Problem::validation(vec![FieldError::new("to", "out_of_range")]));
    }
    let (Some(first), Some(last)) = (local_date(params.from), local_date(params.to)) else {
        return Err(Problem::validation(vec![FieldError::new("from", "out_of_range")]));
    };
    let resource = infra::resources::find(&mut tx, params.resource_id)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such resource"))?;
    let in_node: HashSet<Uuid> = match params.node_id {
        Some(node) => {
            visible_node(&state, &mut tx, node, "no such node").await?;
            repo::projects_under(&mut tx, node).await?.into_iter().collect()
        }
        None => HashSet::new(),
    };
    let period = Period {
        start: first,
        end: last,
    };
    let sums = aggregates::compute(&mut tx, &[(resource.id, resource.calendar_id)], &[period])
        .await?
        .remove(&resource.id)
        .and_then(|v| v.first().copied())
        .unwrap_or_default();
    let sight = Sight::of(&mut tx).await?;
    let found = repo::cell_items(&mut tx, resource.id, params.from, params.to).await?;
    let notes = repo::forced_in(&mut tx, resource.id, params.from, params.to).await?;
    tx.commit().await?;

    let mut items = Vec::new();
    let mut busy: Option<CellItemView> = None;
    let mut keys: HashMap<Uuid, String> = HashMap::new();
    for item in found {
        if !sight.sees(item.project_id) {
            let entry = busy.get_or_insert(CellItemView {
                issue: None,
                minutes: 0,
                provisional: false,
                segments: Vec::new(),
                in_node: false,
            });
            entry.minutes += item.minutes;
            continue;
        }
        keys.insert(item.issue_id, item.key.clone());
        items.push(CellItemView {
            in_node: in_node.contains(&item.project_id),
            issue: Some(CellIssueView {
                id: item.issue_id,
                key: item.key,
                summary: item.summary,
                project: ProjectRefView {
                    id: item.project_id,
                    key: item.project_key,
                    name: item.project_name,
                },
            }),
            minutes: item.minutes,
            provisional: item.provisional,
            segments: item
                .segments
                .into_iter()
                .map(|(mode, value): (Mode, u32)| CellSegmentView {
                    mode: mode.into(),
                    value,
                })
                .collect(),
        });
    }
    items.extend(busy);
    Ok(ApiJson(CellView {
        resource: ResourceRefView {
            id: resource.id,
            kind: resource.kind.into(),
            display_name: resource.display_name.clone(),
            job_title: resource.job_title.clone(),
            linked: resource.account_id.is_some(),
        },
        from: params.from,
        to: params.to,
        capacity_minutes: sums.capacity,
        scheduled_minutes: sums.scheduled,
        load_minutes: sums.load,
        provisional_minutes: sums.provisional,
        items,
        forced: notes
            .into_iter()
            .filter_map(|note| {
                Some(ForcedView {
                    issue_key: keys.get(&note.issue_id)?.clone(),
                    actor_name: note.actor_name,
                    created_at: note.created_at,
                })
            })
            .collect(),
    }))
}
