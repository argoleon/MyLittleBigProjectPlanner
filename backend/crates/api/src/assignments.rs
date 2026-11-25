//! Assignments of resources to tickets (specification §8, L08-US01).
//!
//! An assignment links one resource to one ticket, through successive
//! segments in percentage, hours per day or total effort
//! (`domain::assignments`). Members who see the ticket read its
//! assignments, with the load of each segment computed from the
//! resource's capacity; those with `assign_resources` on its project
//! (tenant admins always) write them, unless the project is archived.
//! Segments never overlap and stay within the ticket's dates, when it has
//! them. Every change writes `assignment.changed` to the event outbox.

use std::collections::HashMap;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use domain::assignments::{self as rules, Mode, Segment, SegmentError};
use domain::permissions::Permission;
use domain::validation::FieldError;
use infra::assignments::{self as repo, Assignment, AssignmentInput, StoredSegment, WriteConflict};
use infra::capacity::local_date;
use infra::issues::Issue;
use infra::nodes::Node;
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use time::Date;
use utoipa::ToSchema;

use crate::etag::{etag, require_if_match};
use crate::extract::{ApiJson, ApiPath};
use crate::idempotency::{self, Start};
use crate::issues::{TAG, visible_issue};
use crate::nodes::{ensure_changeable, is_tenant_admin};
use crate::permissions;
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::resources::KindView;
use crate::tenant::{AppState, TenantTx};

/// How a segment loads its days.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = AssignmentMode)]
pub enum ModeView {
    /// `value` is a percentage of each day's capacity, 1 to 100.
    Percentage,
    /// `value` is minutes per working day, 1 to 1,440.
    HoursPerDay,
    /// `value` is minutes spread evenly over the working days.
    TotalEffort,
}

impl From<ModeView> for Mode {
    fn from(mode: ModeView) -> Self {
        match mode {
            ModeView::Percentage => Self::Percentage,
            ModeView::HoursPerDay => Self::HoursPerDay,
            ModeView::TotalEffort => Self::TotalEffort,
        }
    }
}

impl From<Mode> for ModeView {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::Percentage => Self::Percentage,
            Mode::HoursPerDay => Self::HoursPerDay,
            Mode::TotalEffort => Self::TotalEffort,
        }
    }
}

/// A segment as written.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = AssignmentSegmentFields)]
pub struct SegmentBody {
    /// First day, a local date.
    #[schema(value_type = String, format = Date, example = "2026-11-16")]
    start: Date,
    /// Last day, included.
    #[schema(value_type = String, format = Date, example = "2026-11-20")]
    end: Date,
    mode: ModeView,
    /// A percentage, or minutes (see `mode`).
    #[schema(example = 240)]
    value: u32,
}

/// An assignment to create.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = AssignmentFields, example = json!({
    "resourceId": "0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b01",
    "segments": [
        {"start": "2026-11-16", "end": "2026-11-20", "mode": "hours_per_day", "value": 240},
        {"start": "2026-11-23", "end": "2026-11-27", "mode": "percentage", "value": 30}
    ]
}))]
pub struct AssignmentBody {
    /// An active resource of the tenant (never a team).
    resource_id: Uuid,
    /// `false` by default.
    #[serde(default)]
    provisional: bool,
    /// 1 to 50 segments that never take a same day.
    segments: Vec<SegmentBody>,
}

/// The new state of an assignment: its resource stays.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = AssignmentChange)]
pub struct AssignmentChangeBody {
    #[serde(default)]
    provisional: bool,
    segments: Vec<SegmentBody>,
}

/// The resource of an assignment.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AssignedResource)]
pub struct AssignedResourceView {
    id: Uuid,
    kind: KindView,
    #[schema(example = "Camille Martin")]
    display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    job_title: Option<String>,
    active: bool,
    /// Linked to a member's account.
    linked: bool,
}

/// A segment and its load.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AssignmentSegment)]
pub struct SegmentView {
    #[schema(value_type = String, format = Date)]
    start: Date,
    #[schema(value_type = String, format = Date)]
    end: Date,
    mode: ModeView,
    value: u32,
    /// Minutes it puts on the resource over its days.
    #[schema(example = 1200)]
    load_minutes: u32,
    /// Working days it covers (days the resource has capacity, absences aside).
    #[schema(example = 5)]
    working_days: u32,
}

/// An assignment of a resource to a ticket.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Assignment)]
pub struct AssignmentView {
    id: Uuid,
    issue_id: Uuid,
    resource: AssignedResourceView,
    provisional: bool,
    segments: Vec<SegmentView>,
    /// The load of its segments.
    #[schema(example = 1830)]
    load_minutes: u32,
    /// Some segments fall outside the ticket's dates, changed since.
    outside_issue_dates: bool,
    #[schema(example = 1)]
    version: i64,
}

/// The assignments of a ticket.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AssignmentList {
    /// By resource name.
    items: Vec<AssignmentView>,
    /// The ticket's load: the sum of its assignments.
    load_minutes: u32,
    /// The signed-in member assigns resources to this ticket.
    can_assign: bool,
}

/// The segments of a body, checked against the ticket's dates.
fn checked(
    segments: &[SegmentBody],
    issue_dates: (Option<Date>, Option<Date>),
) -> Result<Vec<StoredSegment>, Problem> {
    let mut rules_segments = Vec::with_capacity(segments.len());
    for (i, s) in segments.iter().enumerate() {
        let (Some(start), Some(end)) = (local_date(s.start), local_date(s.end)) else {
            return Err(Problem::validation(vec![FieldError::new(
                format!("segments.{i}.start"),
                "out_of_range",
            )]));
        };
        rules_segments.push(Segment {
            start,
            end,
            mode: s.mode.into(),
            value: s.value,
        });
    }
    let issue_start = issue_dates.0.and_then(local_date);
    let issue_due = issue_dates.1.and_then(local_date);
    rules::validate(&rules_segments, issue_start, issue_due).map_err(|error| match error {
        SegmentError::Invalid(errors) => Problem::validation(errors),
        SegmentError::Overlap(errors) => Problem::new(ErrorCode::AssignmentSegmentsOverlap)
            .with_detail("two segments take a same day")
            .with_errors(errors),
        SegmentError::OutsideIssueDates(errors) => Problem::new(ErrorCode::AssignmentOutsideIssueDates)
            .with_detail("segments must stay within the ticket's start and due dates")
            .with_errors(errors),
    })?;
    Ok(segments
        .iter()
        .map(|s| StoredSegment {
            start: s.start,
            end: s.end,
            mode: s.mode.into(),
            value: s.value,
        })
        .collect())
}

fn domain_segments(segments: &[StoredSegment]) -> Vec<Segment> {
    segments
        .iter()
        .filter_map(|s| {
            Some(Segment {
                start: local_date(s.start)?,
                end: local_date(s.end)?,
                mode: s.mode,
                value: s.value,
            })
        })
        .collect()
}

/// The load (minutes, working days) of each segment of `assignments`, from
/// their resources' capacity: one set of queries for them all.
async fn loads(
    tx: &mut TenantTx,
    assignments: &[&Assignment],
) -> Result<HashMap<Uuid, Vec<(u32, u32)>>, Problem> {
    let spans: Vec<(Date, Date)> = assignments.iter().filter_map(|a| a.span()).collect();
    let (Some(from), Some(to)) = (
        spans.iter().map(|(first, _)| *first).min(),
        spans.iter().map(|(_, last)| *last).max(),
    ) else {
        return Ok(HashMap::new());
    };
    let mut resources: Vec<(Uuid, Option<Uuid>)> = assignments
        .iter()
        .map(|a| (a.resource.id, a.resource.calendar_id))
        .collect();
    resources.sort_unstable();
    resources.dedup();
    let inputs = infra::day_capacity::load(tx, &resources, from, to).await?;
    Ok(assignments
        .iter()
        .map(|assignment| {
            let levels = inputs.levels(assignment.resource.id);
            let per_segment = inputs
                .calendar(assignment.resource.id, &levels)
                .map(|calendar| {
                    domain_segments(&assignment.segments)
                        .iter()
                        .map(|segment| {
                            let days = calendar.days(segment.start, segment.end);
                            let load = rules::segment_load(segment, &days);
                            let working = days.iter().filter(|d| d.scheduled > 0).count();
                            (load.iter().sum(), u32::try_from(working).unwrap_or(u32::MAX))
                        })
                        .collect()
                })
                .unwrap_or_default();
            (assignment.id, per_segment)
        })
        .collect())
}

fn view(assignment: Assignment, loads: &HashMap<Uuid, Vec<(u32, u32)>>, issue: &Issue) -> AssignmentView {
    let per_segment = loads.get(&assignment.id).cloned().unwrap_or_default();
    let outside = rules::outside_issue_dates(
        &domain_segments(&assignment.segments),
        issue.start_date.and_then(local_date),
        issue.due_date.and_then(local_date),
    );
    let segments: Vec<SegmentView> = assignment
        .segments
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let (load_minutes, working_days) = per_segment.get(i).copied().unwrap_or((0, 0));
            SegmentView {
                start: s.start,
                end: s.end,
                mode: s.mode.into(),
                value: s.value,
                load_minutes,
                working_days,
            }
        })
        .collect();
    let resource = assignment.resource;
    AssignmentView {
        id: assignment.id,
        issue_id: assignment.issue_id,
        resource: AssignedResourceView {
            id: resource.id,
            kind: resource.kind.into(),
            display_name: resource.display_name,
            job_title: resource.job_title,
            active: resource.active,
            linked: resource.linked,
        },
        provisional: assignment.provisional,
        load_minutes: segments.iter().map(|s| s.load_minutes).sum(),
        segments,
        outside_issue_dates: outside,
        version: assignment.version,
    }
}

async fn one_view(
    tx: &mut TenantTx,
    assignment: Assignment,
    issue: &Issue,
) -> Result<AssignmentView, Problem> {
    let loads = loads(tx, &[&assignment]).await?;
    Ok(view(assignment, &loads, issue))
}

/// Whether the member acting assigns resources on `project`.
async fn assigns(state: &AppState, tx: &mut TenantTx, project: &Node) -> Result<bool, Problem> {
    if is_tenant_admin(tx) {
        return Ok(true);
    }
    Ok(permissions::effective(state, tx, Some(project.id))
        .await?
        .contains(Permission::AssignResources))
}

async fn require_assign(state: &AppState, tx: &mut TenantTx, project: &Node) -> Result<(), Problem> {
    if !assigns(state, tx, project).await? {
        return Err(
            Problem::new(ErrorCode::PermissionDenied).with_detail("missing permission: assign_resources")
        );
    }
    ensure_changeable(project)
}

/// The assignment `id`, its ticket and project, if the member sees them.
async fn visible_assignment(
    state: &AppState,
    tx: &mut TenantTx,
    id: Uuid,
) -> Result<(Assignment, Issue, Node), Problem> {
    let not_found = || Problem::new(ErrorCode::NotFound).with_detail("no such assignment");
    let assignment = repo::find(tx, id).await?.ok_or_else(not_found)?;
    let (issue, project) = visible_issue(state, tx, &assignment.issue_id.to_string())
        .await
        .map_err(|_| not_found())?;
    Ok((assignment, issue, project))
}

fn conflict_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound).with_detail("no such assignment"),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed)
            .with_detail("the assignment changed since this version"),
        WriteConflict::ResourceTaken => Problem::new(ErrorCode::AssignmentResourceTaken)
            .with_detail("the resource is already assigned to this ticket: add a segment to its assignment")
            .with_errors(vec![FieldError::new("resourceId", "taken")]),
        WriteConflict::Overlap => {
            Problem::new(ErrorCode::AssignmentSegmentsOverlap).with_detail("two segments take a same day")
        }
    }
}

/// Refuses total-effort segments with no working day of the resource.
async fn require_placeable(
    tx: &mut TenantTx,
    resource: (Uuid, Option<Uuid>),
    segments: &[StoredSegment],
) -> Result<(), Problem> {
    let domain = domain_segments(segments);
    let (Some(from), Some(to)) = (
        segments.iter().map(|s| s.start).min(),
        segments.iter().map(|s| s.end).max(),
    ) else {
        return Ok(());
    };
    let inputs = infra::day_capacity::load(tx, &[resource], from, to).await?;
    let levels = inputs.levels(resource.0);
    let Some(calendar) = inputs.calendar(resource.0, &levels) else {
        return Ok(());
    };
    let errors = rules::unplaceable(&domain, &calendar);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(Problem::validation(errors))
    }
}

fn with_etag(mut response: Response, version: i64) -> Response {
    response.headers_mut().insert(header::ETAG, etag(version));
    response
}

/// `GET /api/v1/issues/{idOrKey}/assignments`.
#[utoipa::path(
    get,
    path = "/issues/{idOrKey}/assignments",
    tag = TAG,
    operation_id = "listIssueAssignments",
    summary = "List the assignments of a ticket",
    description = "Members who see the ticket: each assignment with its segments and their load, \
                   computed from the resource's capacity, public holidays and absences; the ticket's \
                   load is their sum.",
    params(("idOrKey" = String, Path, description = "Id, key or former key of the ticket")),
    responses(
        (status = 200, description = "The assignments", body = AssignmentList),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id_or_key): ApiPath<String>,
) -> Result<ApiJson<AssignmentList>, Problem> {
    let (issue, project) = visible_issue(&state, &mut tx, &id_or_key).await?;
    let can_assign = assigns(&state, &mut tx, &project).await? && !project.archived;
    let assignments = repo::of_issue(&mut tx, issue.id).await?;
    let loads = loads(&mut tx, &assignments.iter().collect::<Vec<_>>()).await?;
    tx.commit().await?;
    let items: Vec<AssignmentView> = assignments.into_iter().map(|a| view(a, &loads, &issue)).collect();
    Ok(ApiJson(AssignmentList {
        load_minutes: items.iter().map(|a| a.load_minutes).sum(),
        items,
        can_assign,
    }))
}

/// `POST /api/v1/issues/{idOrKey}/assignments`: idempotent with `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/issues/{idOrKey}/assignments",
    tag = TAG,
    operation_id = "createIssueAssignment",
    summary = "Assign a resource to a ticket",
    description = "Members with `assign_resources` on its project (tenant admins always). One \
                   assignment per resource and ticket; its segments never take a same day and stay \
                   within the ticket's dates, when it has them.",
    params(
        ("idOrKey" = String, Path, description = "Id, key or former key of the ticket"),
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation"),
    ),
    request_body = AssignmentBody,
    responses(
        (status = 201, description = "Assigned", body = AssignmentView, headers(
            ("ETag" = String, description = "Version of the assignment"),
            ("Location" = String, description = "URL of the assignment"),
        )),
        (status = 400, description = "`validation_failed` (`resourceId`: `not_found`, `inactive`; `segments`: \
                                      `required`, `too_many`; `segments.{i}.end`: `before_start`, `out_of_range`; \
                                      `segments.{i}.value`: `out_of_range`; `segments.{i}.start`: `no_working_day`)",
         body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`assignment.resource_taken`, `assignment.segments_overlap`, \
                                      `assignment.outside_issue_dates`, `node.archived`, or `idempotency_key.in_flight`",
         body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`idempotency_key.reused`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id_or_key): ApiPath<String>,
    ApiJson(body): ApiJson<AssignmentBody>,
) -> Result<Response, Problem> {
    let key = match idempotency::start(
        &mut tx,
        &headers,
        &format!("POST /api/v1/issues/{id_or_key}/assignments"),
        &body,
    )
    .await?
    {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    let (issue, project) = visible_issue(&state, &mut tx, &id_or_key).await?;
    require_assign(&state, &mut tx, &project).await?;
    let resource = match infra::resources::find(&mut tx, body.resource_id).await? {
        None => {
            return Err(Problem::validation(vec![FieldError::new(
                "resourceId",
                "not_found",
            )]));
        }
        Some(r) if !r.active => {
            return Err(Problem::validation(vec![FieldError::new(
                "resourceId",
                "inactive",
            )]));
        }
        Some(r) => r,
    };
    let segments = checked(&body.segments, (issue.start_date, issue.due_date))?;
    require_placeable(&mut tx, (resource.id, resource.calendar_id), &segments).await?;
    let actor = tx.account_id();
    let input = AssignmentInput {
        provisional: body.provisional,
        segments,
    };
    let assignment = repo::insert(
        &mut tx,
        infra::id::new_id(),
        issue.id,
        resource.id,
        &input,
        Some(actor),
    )
    .await?
    .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %actor, issue = %issue.id, resource = %resource.id, assignment = %assignment.id, "resource assigned");
    let (id, version) = (assignment.id, assignment.version);
    let view = one_view(&mut tx, assignment, &issue).await?;
    let response = idempotency::created(
        &mut tx,
        key.as_deref(),
        &view,
        format!("/api/v1/assignments/{id}"),
        version,
    )
    .await?;
    tx.commit().await?;
    Ok(response)
}

/// `GET /api/v1/assignments/{id}`.
#[utoipa::path(
    get,
    path = "/assignments/{id}",
    tag = TAG,
    operation_id = "getAssignment",
    summary = "Read an assignment",
    description = "Members who see its ticket.",
    params(("id" = Uuid, Path, description = "Id of the assignment")),
    responses(
        (status = 200, description = "The assignment", body = AssignmentView, headers(("ETag" = String, description = "Version"))),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such assignment visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn read(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    let (assignment, issue, _) = visible_assignment(&state, &mut tx, id).await?;
    let version = assignment.version;
    let view = one_view(&mut tx, assignment, &issue).await?;
    tx.commit().await?;
    Ok(with_etag(ApiJson(view).into_response(), version))
}

/// `PUT /api/v1/assignments/{id}`: requires `If-Match`.
#[utoipa::path(
    put,
    path = "/assignments/{id}",
    tag = TAG,
    operation_id = "updateAssignment",
    summary = "Change an assignment",
    description = "Members with `assign_resources` on the ticket's project: its segments and \
                   provisional flag are replaced; its resource stays.",
    params(
        ("id" = Uuid, Path, description = "Id of the assignment"),
        ("If-Match" = String, Header, description = "ETag of the version being replaced, or `*`"),
    ),
    request_body = AssignmentChangeBody,
    responses(
        (status = 200, description = "The assignment", body = AssignmentView, headers(("ETag" = String, description = "New version"))),
        (status = 400, description = "`validation_failed`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such assignment visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`assignment.segments_overlap`, `assignment.outside_issue_dates`, or `node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The assignment changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<AssignmentChangeBody>,
) -> Result<Response, Problem> {
    let if_match = require_if_match(&headers)?;
    let (assignment, issue, project) = visible_assignment(&state, &mut tx, id).await?;
    require_assign(&state, &mut tx, &project).await?;
    let segments = checked(&body.segments, (issue.start_date, issue.due_date))?;
    require_placeable(
        &mut tx,
        (assignment.resource.id, assignment.resource.calendar_id),
        &segments,
    )
    .await?;
    let actor = tx.account_id();
    let input = AssignmentInput {
        provisional: body.provisional,
        segments,
    };
    let updated = repo::update(&mut tx, id, if_match.versions(), &input, Some(actor))
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %actor, issue = %issue.id, assignment = %id, "assignment changed");
    let version = updated.version;
    let view = one_view(&mut tx, updated, &issue).await?;
    tx.commit().await?;
    Ok(with_etag(ApiJson(view).into_response(), version))
}

/// `DELETE /api/v1/assignments/{id}`: requires `If-Match`.
#[utoipa::path(
    delete,
    path = "/assignments/{id}",
    tag = TAG,
    operation_id = "deleteAssignment",
    summary = "Remove an assignment",
    description = "Members with `assign_resources` on the ticket's project.",
    params(
        ("id" = Uuid, Path, description = "Id of the assignment"),
        ("If-Match" = String, Header, description = "ETag of the assignment, or `*`"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such assignment visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The assignment changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, Problem> {
    let if_match = require_if_match(&headers)?;
    let (_, issue, project) = visible_assignment(&state, &mut tx, id).await?;
    require_assign(&state, &mut tx, &project).await?;
    let actor = tx.account_id();
    repo::delete(&mut tx, id, if_match.versions(), Some(actor))
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %actor, issue = %issue.id, assignment = %id, "assignment removed");
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use time::macros::date;

    use super::*;

    fn segment(start: Date, end: Date, mode: ModeView, value: u32) -> SegmentBody {
        SegmentBody {
            start,
            end,
            mode,
            value,
        }
    }

    #[test]
    fn segments_map_to_their_problem() {
        let ticket = (Some(date!(2026 - 11 - 16)), Some(date!(2026 - 11 - 27)));
        let week = segment(
            date!(2026 - 11 - 16),
            date!(2026 - 11 - 20),
            ModeView::HoursPerDay,
            240,
        );
        let next = segment(
            date!(2026 - 11 - 23),
            date!(2026 - 11 - 27),
            ModeView::Percentage,
            30,
        );
        let stored = checked(&[week.clone(), next], ticket).unwrap_or_default();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[1].mode, Mode::Percentage);

        let code = |segments: &[SegmentBody], ticket| checked(segments, ticket).err().map(|p| p.code);
        assert_eq!(code(&[], ticket), Some(ErrorCode::ValidationFailed));
        let overlapping = segment(
            date!(2026 - 11 - 20),
            date!(2026 - 11 - 23),
            ModeView::TotalEffort,
            600,
        );
        assert_eq!(
            code(&[week.clone(), overlapping], ticket),
            Some(ErrorCode::AssignmentSegmentsOverlap)
        );
        let beyond = segment(
            date!(2026 - 11 - 23),
            date!(2026 - 11 - 30),
            ModeView::Percentage,
            30,
        );
        assert_eq!(
            code(&[week.clone(), beyond.clone()], ticket),
            Some(ErrorCode::AssignmentOutsideIssueDates)
        );
        // An undated ticket takes any segment.
        assert_eq!(code(&[week, beyond], (None, None)), None);
    }

    #[test]
    fn modes_round_trip() {
        for mode in Mode::ALL {
            assert_eq!(Mode::from(ModeView::from(mode)), mode);
        }
    }
}
