//! Absence conflicts (specification §8, L08-US04): hours planned in hours
//! per day or total effort that an absence of the resource leaves no room
//! for, per assignment and day, to be rescheduled by the project manager.
//! Kept up to date with the daily load (`infra::conflicts`); members list
//! those of the tickets they see.

use infra::conflicts::{Conflict, Filter};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};
use utoipa::{IntoParams, ToSchema};

use crate::extract::{ApiJson, ApiQuery};
use crate::nodes::Sight;
use crate::pagination::{PageParams, encode_cursor};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::resources::TAG;
use crate::tenant::TenantTx;

/// Whether a conflict still stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = WorkloadConflictStatus)]
pub enum StatusView {
    /// Hours still fall on the absence.
    Open,
    /// The assignment or the absence changed since: nothing to reschedule.
    Resolved,
}

impl StatusView {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Resolved => "resolved",
        }
    }
}

/// `?resourceId=&projectId=&status=&limit=&cursor=` of the list.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct ListParams {
    /// Only this resource's.
    resource_id: Option<Uuid>,
    /// Only those of this project's tickets.
    project_id: Option<Uuid>,
    /// Only those in this state; both when left out.
    status: Option<StatusView>,
    /// Page size, from 1 to 200 (default 50).
    #[param(minimum = 1, maximum = 200)]
    limit: Option<usize>,
    /// `nextCursor` of the previous page.
    cursor: Option<String>,
}

/// The ticket of a conflict.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = WorkloadConflictIssue)]
pub struct IssueRefView {
    id: Uuid,
    #[schema(example = "PROJ-42")]
    key: String,
    summary: String,
    project_id: Uuid,
}

/// The resource of a conflict.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = WorkloadConflictResource)]
pub struct ResourceRefView {
    id: Uuid,
    #[schema(example = "Nathan Roux")]
    display_name: String,
}

/// Hours of an assignment an absence leaves no room for, on a day.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = WorkloadConflict)]
pub struct ConflictView {
    id: Uuid,
    assignment_id: Uuid,
    issue: IssueRefView,
    resource: ResourceRefView,
    /// A local date of the resource.
    #[schema(value_type = String, format = Date, example = "2026-11-18")]
    day: Date,
    /// Minutes to reschedule; the last value once resolved.
    #[schema(example = 420)]
    minutes: u32,
    status: StatusView,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    opened_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schema(value_type = Option<String>, format = DateTime, required = true)]
    resolved_at: Option<OffsetDateTime>,
}

impl From<Conflict> for ConflictView {
    fn from(conflict: Conflict) -> Self {
        Self {
            id: conflict.id,
            assignment_id: conflict.assignment_id,
            issue: IssueRefView {
                id: conflict.issue_id,
                key: conflict.issue_key,
                summary: conflict.issue_summary,
                project_id: conflict.project_id,
            },
            resource: ResourceRefView {
                id: conflict.resource_id,
                display_name: conflict.resource_name,
            },
            day: conflict.day,
            minutes: conflict.minutes,
            status: if conflict.status == "resolved" {
                StatusView::Resolved
            } else {
                StatusView::Open
            },
            opened_at: conflict.opened_at,
            resolved_at: conflict.resolved_at,
        }
    }
}

/// A page of conflicts.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = WorkloadConflictPage)]
pub struct ConflictPage {
    items: Vec<ConflictView>,
    /// Cursor of the next (older) page, `null` on the last page.
    #[schema(required = true)]
    next_cursor: Option<String>,
}

/// `GET /api/v1/workload/conflicts`.
#[utoipa::path(
    get,
    path = "/workload/conflicts",
    tag = TAG,
    operation_id = "listWorkloadConflicts",
    summary = "List the absence conflicts",
    description = "The conflicts of the tickets the member sees, newest first: per assignment and \
                   day, the hours planned in hours per day or total effort that an absence of the \
                   resource leaves no room for. Opened when an absence or a segment puts them \
                   there, resolved when no longer.",
    params(ListParams),
    responses(
        (status = 200, description = "A page of conflicts", body = ConflictPage),
        (status = 400, description = "Invalid `limit`, `cursor` or `status`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "`projectId`: no such project visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    mut tx: TenantTx,
    ApiQuery(params): ApiQuery<ListParams>,
) -> Result<ApiJson<ConflictPage>, Problem> {
    let (limit, before) = PageParams {
        limit: params.limit,
        cursor: params.cursor,
    }
    .resolve()?;
    let sight = Sight::of(&mut tx).await?;
    let visible = sight.visible();
    if params.project_id.is_some_and(|project| !sight.sees(project)) {
        return Err(Problem::new(ErrorCode::NotFound).with_detail("no such project"));
    }
    let page = infra::conflicts::list(
        &mut tx,
        &Filter {
            resource_id: params.resource_id,
            project_id: params.project_id,
            status: params.status.map(StatusView::as_str),
            visible_projects: visible.as_deref(),
        },
        limit,
        before,
    )
    .await?;
    tx.commit().await?;
    Ok(ApiJson(ConflictPage {
        items: page.items.into_iter().map(ConflictView::from).collect(),
        next_cursor: page.next.map(encode_cursor),
    }))
}
