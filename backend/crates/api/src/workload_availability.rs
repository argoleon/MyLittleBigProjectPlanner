//! The availability search (specification §8, L08-US10): who has a number
//! of free hours or free days over a period, optionally in a team. Internal
//! members read it, like the heatmap; load on tickets they do not see
//! (confidential or not granted) reduces the free time all the same, only
//! as minutes. Also behind the MCP tool `find_availability` (L16).

use domain::availability::{Availability, Need, rank};
use domain::validation::FieldError;
use domain::workload::Scale;
use infra::heatmap::{self as heatmap_repo, Filter, HeatmapResource};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use time::Date;
use utoipa::{IntoParams, ToSchema};

use crate::extract::{ApiJson, ApiQuery};
use crate::problem::{PROBLEM_JSON, Problem, ProblemBody};
use crate::resources::TAG;
use crate::tenant::TenantTx;
use crate::workload_heatmap::{ResourceRefView, checked_range, resource_view};

/// Most free hours a search may ask for.
pub const MAX_HOURS: u32 = 2_000;

/// `?from=&to=&teamId=&minHours=&minDays=&includeProvisional=` of the search.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct AvailabilityParams {
    /// First day.
    #[param(value_type = String, format = Date)]
    from: Date,
    /// Last day, included: 92 days at most.
    #[param(value_type = String, format = Date)]
    to: Date,
    /// Only the members of this team.
    team_id: Option<Uuid>,
    /// At least these free hours over the period (1 to 2000).
    #[param(minimum = 1, maximum = 2000)]
    min_hours: Option<u32>,
    /// At least this many working days entirely free (1 to 92), not
    /// necessarily in a row. Not with `minHours`.
    #[param(minimum = 1, maximum = 92)]
    min_days: Option<u32>,
    /// Provisional assignments take free time (`true` when left out).
    include_provisional: Option<bool>,
}

/// A resource found, with its free capacity over the period.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AvailableResource)]
pub struct AvailableView {
    resource: ResourceRefView,
    /// Capacity left, day by day (capacity less load, at least 0).
    #[schema(example = 1260)]
    free_minutes: u32,
    /// Working days entirely free: nothing planned, no absence.
    #[schema(example = 2)]
    free_days: u32,
    /// Capacity, absences, public holidays and closures deducted.
    #[schema(example = 2100)]
    capacity_minutes: u32,
    /// Load planned, provisional included or not.
    #[schema(example = 840)]
    load_minutes: u32,
    /// Days with capacity.
    #[schema(example = 5)]
    working_days: u32,
}

/// The resources meeting a search, most free time first.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Availability)]
pub struct AvailabilityView {
    #[schema(value_type = String, format = Date)]
    from: Date,
    #[schema(value_type = String, format = Date)]
    to: Date,
    include_provisional: bool,
    items: Vec<AvailableView>,
}

/// What `params` ask for, or why not.
fn need_of(params: &AvailabilityParams) -> Result<Option<Need>, Problem> {
    match (params.min_hours, params.min_days) {
        (Some(_), Some(_)) => Err(Problem::validation(vec![FieldError::new(
            "minDays",
            "not_allowed",
        )])),
        (Some(hours), None) if (1..=MAX_HOURS).contains(&hours) => Ok(Some(Need::Minutes(hours * 60))),
        (Some(_), None) => Err(Problem::validation(vec![FieldError::new(
            "minHours",
            "out_of_range",
        )])),
        (None, Some(days)) if (1..=92).contains(&days) => Ok(Some(Need::Days(days))),
        (None, Some(_)) => Err(Problem::validation(vec![FieldError::new(
            "minDays",
            "out_of_range",
        )])),
        (None, None) => Ok(None),
    }
}

/// The resources of `found` meeting `need`, ranked.
fn ranked(found: Vec<(HeatmapResource, Availability)>, need: Option<Need>) -> Vec<AvailableView> {
    let mut kept: Vec<(String, Availability, HeatmapResource)> = found
        .into_iter()
        .filter(|(_, availability)| availability.meets(need))
        .map(|(resource, availability)| (resource.display_name.clone(), availability, resource))
        .collect();
    rank(&mut kept);
    kept.into_iter()
        .map(|(_, a, resource)| AvailableView {
            resource: resource_view(&resource),
            free_minutes: a.free_minutes,
            free_days: a.free_days,
            capacity_minutes: a.capacity_minutes,
            load_minutes: a.load_minutes,
            working_days: a.working_days,
        })
        .collect()
}

/// `GET /api/v1/workload/availability`.
#[utoipa::path(
    get,
    path = "/workload/availability",
    tag = TAG,
    operation_id = "findAvailability",
    summary = "Search for available resources",
    description = "Internal members: the active resources (of a team) with at least `minHours` \
                   free hours, or `minDays` working days entirely free, from `from` to `to`, most \
                   free time first. Free time is the capacity (absences, public holidays and \
                   closures deducted) less the load, day by day; the overload tolerance is left \
                   out. Load on tickets the member does not see counts, without naming them.",
    params(AvailabilityParams),
    responses(
        (status = 200, description = "The resources found", body = AvailabilityView),
        (status = 400, description = "`validation_failed` (`to`: `before_start`, `out_of_range` beyond 92 \
                                      days; `minHours`, `minDays`: `out_of_range`; `minDays`: `not_allowed` \
                                      with `minHours`), or `invalid_query`",
         body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn find(
    mut tx: TenantTx,
    ApiQuery(params): ApiQuery<AvailabilityParams>,
) -> Result<ApiJson<AvailabilityView>, Problem> {
    checked_range(params.from, params.to, Scale::Day)?;
    let need = need_of(&params)?;
    let include_provisional = params.include_provisional.unwrap_or(true);
    let resources = heatmap_repo::resources(
        &mut tx,
        &Filter {
            team_id: params.team_id,
            node_id: None,
            from: params.from,
            to: params.to,
        },
    )
    .await?;
    let keys: Vec<_> = resources.iter().map(|r| (r.id, r.calendar_id)).collect();
    let mut availability =
        infra::availability::of(&mut tx, &keys, params.from, params.to, include_provisional).await?;
    tx.commit().await?;
    let found = resources
        .into_iter()
        .filter_map(|resource| {
            let a = availability.remove(&resource.id)?;
            Some((resource, a))
        })
        .collect();
    Ok(ApiJson(AvailabilityView {
        from: params.from,
        to: params.to,
        include_provisional,
        items: ranked(found, need),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(min_hours: Option<u32>, min_days: Option<u32>) -> AvailabilityParams {
        AvailabilityParams {
            from: time::macros::date!(2026 - 11 - 16),
            to: time::macros::date!(2026 - 11 - 20),
            team_id: None,
            min_hours,
            min_days,
            include_provisional: None,
        }
    }

    #[test]
    fn a_search_asks_for_hours_or_days_within_bounds() {
        assert_eq!(need_of(&params(None, None)).ok(), Some(None));
        assert_eq!(
            need_of(&params(Some(10), None)).ok(),
            Some(Some(Need::Minutes(600)))
        );
        assert_eq!(need_of(&params(None, Some(3))).ok(), Some(Some(Need::Days(3))));
        for refused in [
            params(Some(10), Some(3)),
            params(Some(0), None),
            params(Some(2001), None),
            params(None, Some(0)),
            params(None, Some(93)),
        ] {
            assert!(need_of(&refused).is_err(), "{refused:?}");
        }
    }
}
