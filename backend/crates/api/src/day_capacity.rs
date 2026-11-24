//! Capacity of many resources at once (specification §8, L07-US06): the
//! minutes of each day, after public holidays, closures and absences, for
//! a page of resources. The heatmap and the availability search (L08) read
//! it. Any internal member reads it.

use domain::calendars::Status;
use domain::resources::trimmed;
use infra::capacity::{local_date, sql_date};
use infra::resources::{self as resources_repo, ResourceFilter};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use time::Date;
use utoipa::{IntoParams, ToSchema};

use crate::calendars::{DayPartView, RangeParams, status_view};
use crate::extract::{ApiJson, ApiQuery};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::resources::{TAG, decode_cursor, encode_cursor};
use crate::tenant::TenantTx;

/// Most resources per page, and in `resources`.
pub const MAX_RESOURCES: usize = 200;

/// `?resources=&team=&from=&to=&limit=&cursor=` of the capacity.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct CapacityParams {
    /// Ids of resources, separated by commas (200 at most); inactive ones
    /// included. Without it (nor `team`), every active resource.
    pub resources: Option<String>,
    /// Only the members of this team (active ones, unless `resources`
    /// names them).
    pub team: Option<Uuid>,
    /// First day, a local date.
    #[param(value_type = String, format = Date)]
    pub from: Option<Date>,
    /// Last day, included (366 days at most).
    #[param(value_type = String, format = Date)]
    pub to: Option<Date>,
    /// Page size, from 1 to 200 (default 200).
    #[param(minimum = 1, maximum = 200)]
    pub limit: Option<usize>,
    /// `nextCursor` of the previous page.
    pub cursor: Option<String>,
}

/// Why a day has no capacity, or less than its weekday.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = UnavailableReason)]
pub enum ReasonView {
    Holiday,
    NonWorking,
    Absence,
}

/// A day off, or partly off, of a resource.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = UnavailableDay)]
pub struct UnavailableDayView {
    #[schema(value_type = String, format = Date, example = "2026-12-25")]
    day: Date,
    reason: ReasonView,
    /// Key of the public holiday (`christmas`…).
    #[serde(skip_serializing_if = "Option::is_none")]
    holiday: Option<String>,
    /// Label of the exception.
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    /// The part an absence takes.
    #[serde(skip_serializing_if = "Option::is_none")]
    part: Option<DayPartView>,
}

/// The capacity of a resource over the range.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ResourceCapacity)]
pub struct ResourceCapacityView {
    resource_id: Uuid,
    #[schema(example = "Camille Martin")]
    display_name: String,
    active: bool,
    /// Minutes of each day, from `from` to `to`.
    #[schema(example = json!([420, 420, 0, 420, 210, 0, 0]))]
    minutes: Vec<u16>,
    /// Their sum.
    #[schema(example = 1470)]
    total_minutes: u32,
    /// The public holidays, closures and absences of the range, by day.
    unavailable: Vec<UnavailableDayView>,
}

/// A page of capacities.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CapacityPage {
    #[schema(value_type = String, format = Date)]
    from: Date,
    #[schema(value_type = String, format = Date)]
    to: Date,
    /// In the order of the resource list (name, then id).
    items: Vec<ResourceCapacityView>,
    /// Cursor of the next page, `null` on the last page.
    #[schema(required = true)]
    next_cursor: Option<String>,
}

/// The ids of `resources=`: distinct, 200 at most.
fn parse_ids(raw: &str) -> Result<Vec<Uuid>, Problem> {
    let mut ids = Vec::new();
    for part in raw.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let id = Uuid::try_parse(part).map_err(|_| {
            Problem::new(ErrorCode::InvalidQuery).with_detail("resources: ids separated by commas")
        })?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.len() > MAX_RESOURCES {
        return Err(Problem::new(ErrorCode::InvalidQuery).with_detail("resources: 200 ids at most"));
    }
    Ok(ids)
}

/// The range and page size of `params`.
fn checked(params: &CapacityParams) -> Result<(Date, Date, usize), Problem> {
    let (Some(from), Some(to)) = (params.from, params.to) else {
        return Err(Problem::new(ErrorCode::InvalidQuery).with_detail("from and to are required"));
    };
    RangeParams { from, to }.checked()?;
    let limit = params.limit.unwrap_or(MAX_RESOURCES);
    if !(1..=MAX_RESOURCES).contains(&limit) {
        return Err(Problem::new(ErrorCode::InvalidQuery).with_detail("limit: 1 to 200"));
    }
    Ok((from, to, limit))
}

fn unavailable(day: domain::calendars::Day) -> Option<UnavailableDayView> {
    let reason = match (&day.status, day.absence) {
        (Status::Holiday { .. }, _) => ReasonView::Holiday,
        (Status::NonWorking { .. }, _) => ReasonView::NonWorking,
        (Status::Working, Some(_)) => ReasonView::Absence,
        (Status::Working, None) => return None,
    };
    let date = sql_date(day.date)?;
    let part = (reason == ReasonView::Absence)
        .then_some(day.absence.map(|(_, part)| part.into()))
        .flatten();
    let (_, holiday, label) = status_view(day.status);
    Some(UnavailableDayView {
        day: date,
        reason,
        holiday,
        label,
        part,
    })
}

/// `GET /api/v1/capacity`.
#[utoipa::path(
    get,
    path = "/capacity",
    tag = TAG,
    operation_id = "listCapacity",
    summary = "Read the capacity of resources, day by day",
    description = "Any internal member: for each resource of the page, the minutes of each day from \
                   `from` to `to` (366 days at most) — its capacity periods, less the public holidays \
                   and closures of its calendars, less its absences —, and the days off. Without \
                   `resources` nor `team`, every active resource, 200 per page.",
    params(CapacityParams),
    responses(
        (status = 200, description = "A page of capacities", body = CapacityPage),
        (status = 400, description = "`invalid_query`: bad range, `resources`, `limit` or `cursor`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: an external member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    mut tx: TenantTx,
    ApiQuery(params): ApiQuery<CapacityParams>,
) -> Result<ApiJson<CapacityPage>, Problem> {
    let (from, to, limit) = checked(&params)?;
    let ids = trimmed(params.resources.as_deref()).map(parse_ids).transpose()?;
    let after = match params.cursor.as_deref() {
        None => None,
        Some(cursor) => Some(decode_cursor(cursor).ok_or_else(|| {
            Problem::new(ErrorCode::InvalidQuery).with_detail("cursor is invalid or expired")
        })?),
    };
    let filter = ResourceFilter {
        // Named ones whatever their state; else the active ones.
        active: if ids.is_some() { None } else { Some(true) },
        team: params.team,
        ids: ids.as_deref(),
        ..ResourceFilter::default()
    };
    let page = resources_repo::list(&mut tx, &filter, after.as_ref(), limit).await?;
    let selected: Vec<(Uuid, Option<Uuid>)> = page.items.iter().map(|r| (r.id, r.calendar_id)).collect();
    let inputs = infra::day_capacity::load(&mut tx, &selected, from, to).await?;
    tx.commit().await?;

    let (Some(first), Some(last)) = (local_date(from), local_date(to)) else {
        return Err(Problem::new(ErrorCode::InvalidQuery));
    };
    let items = page
        .items
        .into_iter()
        .filter_map(|resource| {
            let levels = inputs.levels(resource.id);
            let calendar = inputs.calendar(resource.id, &levels)?;
            let days = calendar.days(first, last);
            let minutes: Vec<u16> = days.iter().map(|d| d.minutes).collect();
            Some(ResourceCapacityView {
                resource_id: resource.id,
                display_name: resource.display_name,
                active: resource.active,
                total_minutes: minutes.iter().map(|m| u32::from(*m)).sum(),
                minutes,
                unavailable: days.into_iter().filter_map(unavailable).collect(),
            })
        })
        .collect();
    Ok(ApiJson(CapacityPage {
        from,
        to,
        items,
        next_cursor: page.next.as_ref().map(encode_cursor),
    }))
}

#[cfg(test)]
mod tests {
    use time::macros::date;

    use super::*;

    fn params(from: Option<Date>, to: Option<Date>, limit: Option<usize>) -> CapacityParams {
        CapacityParams {
            from,
            to,
            limit,
            ..CapacityParams::default()
        }
    }

    #[test]
    fn the_range_is_required_ordered_and_366_days_at_most() {
        let jan = date!(2027 - 01 - 01);
        assert!(checked(&params(Some(jan), Some(date!(2027 - 12 - 31)), None)).is_ok());
        assert_eq!(
            checked(&params(Some(jan), Some(jan), None))
                .unwrap_or((jan, jan, 0))
                .2,
            MAX_RESOURCES,
            "one day, 200 resources by default"
        );
        for bad in [
            params(None, Some(jan), None),
            params(Some(jan), None, None),
            params(Some(jan), Some(date!(2026 - 12 - 31)), None),
            params(Some(jan), Some(date!(2028 - 01 - 02)), None),
            params(Some(jan), Some(jan), Some(0)),
            params(Some(jan), Some(jan), Some(201)),
        ] {
            let problem = checked(&bad).err();
            assert_eq!(problem.map(|p| p.code), Some(ErrorCode::InvalidQuery), "{bad:?}");
        }
    }

    #[test]
    fn resource_ids_are_distinct_uuids_200_at_most() {
        let a = Uuid::now_v7();
        let b = Uuid::now_v7();
        assert_eq!(parse_ids(&format!("{a}, {b},{a},")).ok(), Some(vec![a, b]));
        assert!(parse_ids("not-a-uuid").is_err());
        let many: Vec<String> = (0..=MAX_RESOURCES).map(|_| Uuid::now_v7().to_string()).collect();
        assert!(parse_ids(&many.join(",")).is_err());
        assert!(parse_ids(&many[..MAX_RESOURCES].join(",")).is_ok());
    }
}
