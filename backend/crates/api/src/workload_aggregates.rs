//! Load against capacity by hierarchy node and by team (specification §4,
//! §8, L08-US07), per week or month, from the precomputed sums.
//!
//! - A node sums its distinct descendant projects (itself if it is one),
//!   each with the weighting of its link (`domain::aggregates`), archived
//!   ones left out. A project's capacity is the share of its resources'
//!   capacity in proportion to their load there (`load_aggregate_project`).
//!   Confidential projects count without detail: only totals are given.
//! - A team sums its active members' load and capacity.

use axum::extract::State;
use domain::workload::{Period, Scale};
use infra::capacity::sql_date;
use infra::heatmap::Filter;
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use time::Date;
use utoipa::{IntoParams, ToSchema};

use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::nodes::visible_node;
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::resources::TAG;
use crate::tenant::{AppState, TenantTx};
use crate::workload_heatmap::{checked_range, sums_of};
use domain::validation::FieldError;

/// The span of a period of an aggregate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = WorkloadAggregateScale)]
pub enum ScaleView {
    /// A week, from Monday: up to 53.
    Week,
    /// A month: up to 24.
    #[default]
    Month,
}

impl From<ScaleView> for Scale {
    fn from(scale: ScaleView) -> Self {
        match scale {
            ScaleView::Week => Self::Week,
            ScaleView::Month => Self::Month,
        }
    }
}

/// `?scale=&from=&to=` of an aggregate.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct AggregateParams {
    /// `month` when left out.
    #[serde(default)]
    scale: ScaleView,
    /// First day: its period comes first.
    #[param(value_type = String, format = Date)]
    from: Date,
    /// Last day: its period comes last.
    #[param(value_type = String, format = Date)]
    to: Date,
}

/// A period of an aggregate.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = WorkloadAggregatePeriod)]
pub struct PeriodView {
    #[schema(value_type = String, format = Date, example = "2026-11-01")]
    start: Date,
    #[schema(value_type = String, format = Date, example = "2026-11-30")]
    end: Date,
    /// Load planned, provisional included, in minutes.
    #[schema(example = 18_720)]
    load_minutes: u64,
    /// Its provisional part.
    provisional_minutes: u64,
    /// Capacity, in minutes: of the members for a team; for a node, the
    /// share of its resources' capacity their load there takes.
    #[schema(example = 20_400)]
    capacity_minutes: u64,
    /// Load over capacity, in percent rounded to the nearest; `null`
    /// without capacity.
    #[schema(example = 92, required = true)]
    used_percent: Option<u32>,
}

/// Load against capacity per period.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = WorkloadAggregate)]
pub struct AggregateView {
    scale: ScaleView,
    periods: Vec<PeriodView>,
    /// For a node: its distinct projects counted; for a team: its active
    /// members.
    #[schema(example = 6)]
    count: u32,
    /// For a node: those of its projects that are confidential, counted
    /// without detail; 0 for a team.
    #[schema(example = 1)]
    confidential_projects: u32,
}

fn used(load: u64, capacity: u64) -> Option<u32> {
    (capacity > 0).then(|| u32::try_from((load * 100 + capacity / 2) / capacity).unwrap_or(u32::MAX))
}

fn period_view(period: &Period, load: u64, provisional: u64, capacity: u64) -> Option<PeriodView> {
    Some(PeriodView {
        start: sql_date(period.start)?,
        end: sql_date(period.end)?,
        load_minutes: load,
        provisional_minutes: provisional,
        capacity_minutes: capacity,
        used_percent: used(load, capacity),
    })
}

/// Weeks and months only: days have no precomputed sums.
fn kind(scale: Scale) -> &'static str {
    match scale {
        Scale::Week => "week",
        Scale::Day | Scale::Month => "month",
    }
}

/// `GET /api/v1/nodes/{id}/workload`.
#[utoipa::path(
    get,
    path = "/nodes/{id}/workload",
    tag = TAG,
    operation_id = "getNodeWorkload",
    summary = "Load against capacity of a node",
    description = "Members who see the node: per week or month, the load planned on its distinct \
                   projects (itself if it is one, or those below it, each with the weighting of its \
                   link; a project reached twice counts once), and the share of their resources' \
                   capacity that load takes. Confidential projects count without detail. Sums kept \
                   over a year back and two years ahead.",
    params(("id" = Uuid, Path, description = "Id of the node"), AggregateParams),
    responses(
        (status = 200, description = "The aggregate", body = AggregateView),
        (status = 400, description = "`validation_failed` (`to`: `before_start`, `out_of_range`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such node visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn node(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(params): ApiQuery<AggregateParams>,
) -> Result<ApiJson<AggregateView>, Problem> {
    let scale: Scale = params.scale.into();
    let columns = checked_range(params.from, params.to, scale)?;
    visible_node(&state, &mut tx, id, "no such node").await?;
    let hierarchy = infra::aggregates::hierarchy(&mut tx).await?;
    let counted = hierarchy.counted(id.as_u128());
    let aggregate = hierarchy.aggregate(id.as_u128());
    let projects: Vec<Uuid> = counted
        .iter()
        .map(|(project, _)| Uuid::from_u128(*project))
        .collect();
    let starts: Vec<Date> = columns.iter().filter_map(|p| sql_date(p.start)).collect();
    let shares = infra::load_aggregates::project_shares(&mut tx, &projects, kind(scale), &starts).await?;
    tx.commit().await?;
    let periods = columns
        .iter()
        .filter_map(|period| {
            let start = sql_date(period.start)?;
            // Weighted in hundredths, rounded once.
            let (mut load, mut provisional, mut capacity) = (0_u64, 0_u64, 0_u64);
            for (project, share) in &counted {
                let Some((l, p, c)) = shares.get(&(Uuid::from_u128(*project), start)) else {
                    continue;
                };
                let share = u64::from(*share);
                load += u64::from(*l) * share;
                provisional += u64::from(*p) * share;
                capacity += u64::from(*c) * share;
            }
            let minutes = |hundredths: u64| (hundredths + 50) / 100;
            period_view(period, minutes(load), minutes(provisional), minutes(capacity))
        })
        .collect();
    Ok(ApiJson(AggregateView {
        scale: params.scale,
        periods,
        count: aggregate.projects,
        confidential_projects: aggregate.confidential_projects,
    }))
}

/// `GET /api/v1/teams/{id}/workload`.
#[utoipa::path(
    get,
    path = "/teams/{id}/workload",
    tag = TAG,
    operation_id = "getTeamWorkload",
    summary = "Load against capacity of a team",
    description = "Internal members: per week or month, the load and capacity of the team's active \
                   members, summed.",
    params(("id" = Uuid, Path, description = "Id of the team"), AggregateParams),
    responses(
        (status = 200, description = "The aggregate", body = AggregateView),
        (status = 400, description = "`validation_failed` (`to`: `before_start`, `out_of_range`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such team in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn team(
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(params): ApiQuery<AggregateParams>,
) -> Result<ApiJson<AggregateView>, Problem> {
    let scale: Scale = params.scale.into();
    let columns = checked_range(params.from, params.to, scale)?;
    if infra::teams::find(&mut tx, id).await?.is_none() {
        return Err(Problem::new(ErrorCode::NotFound).with_detail("no such team"));
    }
    let (Some(first), Some(last)) = (
        columns.first().and_then(|p| sql_date(p.start)),
        columns.last().and_then(|p| sql_date(p.end)),
    ) else {
        return Err(Problem::validation(vec![FieldError::new("from", "out_of_range")]));
    };
    let members = infra::heatmap::resources(
        &mut tx,
        &Filter {
            team_id: Some(id),
            node_id: None,
            from: first,
            to: last,
        },
    )
    .await?;
    let sums = sums_of(&mut tx, &members, scale, &columns).await?;
    tx.commit().await?;
    let periods = columns
        .iter()
        .enumerate()
        .filter_map(|(i, period)| {
            let (mut load, mut provisional, mut capacity) = (0_u64, 0_u64, 0_u64);
            for row in sums.values() {
                if let Some(s) = row.get(i) {
                    load += u64::from(s.load);
                    provisional += u64::from(s.provisional);
                    capacity += u64::from(s.capacity);
                }
            }
            period_view(period, load, provisional, capacity)
        })
        .collect();
    Ok(ApiJson(AggregateView {
        scale: params.scale,
        periods,
        count: u32::try_from(members.len()).unwrap_or(u32::MAX),
        confidential_projects: 0,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_share_used_is_rounded_and_absent_without_capacity() {
        assert_eq!(used(18_720, 20_400), Some(92));
        assert_eq!(used(2352, 2100), Some(112));
        assert_eq!(used(60, 0), None);
        assert_eq!(kind(Scale::Week), "week");
        assert_eq!(kind(Scale::Month), "month");
    }
}
