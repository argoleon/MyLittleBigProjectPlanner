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
//!
//! Every write is checked for overload (L08-US03, `domain::overload`): under
//! its resource's lock, the resulting load of each day is compared with the
//! capacity plus the tenant's tolerance. In blocking mode an overload is
//! refused (`workload.overload`, listing the days) unless the author, with
//! `force_overload`, forces it: the write is then traced in the ticket's
//! history and in the forced overloads. In warning mode it is written and
//! the assignment flags its days (`overloads`), as it does on every read.
//! `POST /assignments/preview` computes the same without writing.

use std::collections::HashMap;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use domain::assignments::{self as rules, Mode, Segment, SegmentError};
use domain::overload::{self as overload_rules, Outcome, Overload, Policy};
use domain::permissions::Permission;
use domain::tenant::OverloadMode;
use domain::validation::FieldError;
use infra::assignments::{self as repo, Assignment, AssignmentInput, StoredSegment, WriteConflict};
use infra::capacity::{local_date, sql_date};
use infra::issues::Issue;
use infra::nodes::Node;
use infra::overload::{self as checks, Candidate, Forced};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use time::Date;
use utoipa::{IntoParams, ToSchema};

use crate::etag::{etag, require_if_match};
use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::idempotency::{self, Start};
use crate::issues::{TAG, visible_issue};
use crate::nodes::{Sight, ensure_changeable, is_tenant_admin};
use crate::pagination::{PageParams, encode_cursor};
use crate::permissions;
use crate::problem::{ErrorCode, OverloadDay, OverloadProblem, PROBLEM_JSON, Problem, ProblemBody};
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
    /// Writes it even beyond the resource's capacity plus tolerance, in
    /// blocking mode: members with `force_overload` on its project (tenant
    /// admins always). Traced.
    #[serde(default)]
    force: bool,
}

/// The new state of an assignment: its resource stays.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = AssignmentChange)]
pub struct AssignmentChangeBody {
    #[serde(default)]
    provisional: bool,
    segments: Vec<SegmentBody>,
    /// As for a creation.
    #[serde(default)]
    force: bool,
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
    /// The days it loads while its resource is over capacity plus
    /// tolerance (provisional assignments counted by the tenant's
    /// setting), by date: empty when none.
    overloads: Vec<OverloadDay>,
    /// The days an absence of the resource leaves no room for its hours
    /// (hours per day or total effort), by date: to reschedule (L08-US04).
    conflicts: Vec<ConflictDayView>,
    #[schema(example = 1)]
    version: i64,
}

/// Hours of an assignment an absence leaves no room for, on a day.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AssignmentConflict)]
pub struct ConflictDayView {
    #[schema(value_type = String, format = Date, example = "2026-11-18")]
    date: Date,
    /// Minutes to reschedule.
    #[schema(example = 420)]
    minutes: u32,
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

/// An overloaded day as sent.
fn overload_day(overload: &Overload) -> Option<OverloadDay> {
    Some(OverloadDay {
        date: sql_date(overload.date)?,
        planned_minutes: overload.planned,
        available_minutes: overload.available,
        limit_minutes: overload.limit,
    })
}

fn overload_days(overloads: &[Overload]) -> Vec<OverloadDay> {
    overloads.iter().filter_map(overload_day).collect()
}

fn view(
    assignment: Assignment,
    loads: &HashMap<Uuid, Vec<(u32, u32)>>,
    flagged: &HashMap<Uuid, Vec<Overload>>,
    conflicts: &HashMap<Uuid, Vec<infra::conflicts::ConflictDay>>,
    issue: &Issue,
) -> AssignmentView {
    let conflicts = conflicts
        .get(&assignment.id)
        .map(|days| {
            days.iter()
                .map(|d| ConflictDayView {
                    date: d.day,
                    minutes: d.minutes,
                })
                .collect()
        })
        .unwrap_or_default();
    let per_segment = loads.get(&assignment.id).cloned().unwrap_or_default();
    let overloads = flagged
        .get(&assignment.id)
        .map(|o| overload_days(o))
        .unwrap_or_default();
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
        overloads,
        conflicts,
        version: assignment.version,
    }
}

async fn one_view(
    tx: &mut TenantTx,
    assignment: Assignment,
    issue: &Issue,
) -> Result<AssignmentView, Problem> {
    let loads = loads(tx, &[&assignment]).await?;
    let policy = checks::policy(tx).await?;
    let flagged = checks::flagged(tx, &policy, &[&assignment]).await?;
    let conflicts = infra::conflicts::open_of(tx, &[assignment.id]).await?;
    Ok(view(assignment, &loads, &flagged, &conflicts, issue))
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

/// Whether the member acting forces overloads on `project`.
async fn forces(state: &AppState, tx: &mut TenantTx, project: &Node) -> Result<bool, Problem> {
    if is_tenant_admin(tx) {
        return Ok(true);
    }
    Ok(permissions::effective(state, tx, Some(project.id))
        .await?
        .contains(Permission::ForceOverload))
}

/// `workload.overload`, listing `days`.
fn overload_problem(days: &[Overload], forceable: bool) -> Problem {
    let first = days.first().map(|d| d.date.to_string()).unwrap_or_default();
    Problem::overload(
        OverloadProblem {
            days: overload_days(days),
            forceable,
        },
        format!(
            "{} day(s) over the resource's capacity plus tolerance, the first on {first}",
            days.len()
        ),
    )
}

/// Checks the overload of writing `candidate`, under its resource's lock
/// (held until the end of the transaction): refused in blocking mode
/// unless `force`, which needs `force_overload`.
async fn check_overload(
    state: &AppState,
    tx: &mut TenantTx,
    project: &Node,
    candidate: &Candidate<'_>,
    force: bool,
) -> Result<Outcome, Problem> {
    let forceable = forces(state, tx, project).await?;
    if force && !forceable {
        return Err(
            Problem::new(ErrorCode::PermissionDenied).with_detail("missing permission: force_overload")
        );
    }
    checks::lock_resource(tx, candidate.resource_id).await?;
    let policy = checks::policy(tx).await?;
    let days = checks::day_loads(tx, &policy, candidate).await?;
    let outcome = overload_rules::decide(&policy, overload_rules::overloads(&days, &policy), force);
    if let Outcome::Refused(days) = &outcome {
        return Err(overload_problem(days, forceable));
    }
    Ok(outcome)
}

/// Traces `outcome` when it was forced.
async fn trace_forced(tx: &mut TenantTx, outcome: &Outcome, assignment: &Assignment) -> Result<(), Problem> {
    let Outcome::Forced(days) = outcome else {
        return Ok(());
    };
    let author = tx.author();
    checks::record_forced(
        tx,
        author,
        &Forced {
            assignment_id: assignment.id,
            issue_id: assignment.issue_id,
            resource_id: assignment.resource.id,
            resource_name: &assignment.resource.display_name,
            overloads: days,
        },
    )
    .await?;
    tracing::info!(target: "audit", actor = ?author.actor_id, issue = %assignment.issue_id,
                   assignment = %assignment.id, days = days.len(), "overload forced");
    Ok(())
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
    let all: Vec<&Assignment> = assignments.iter().collect();
    let loads = loads(&mut tx, &all).await?;
    let policy = checks::policy(&mut tx).await?;
    let flagged = checks::flagged(&mut tx, &policy, &all).await?;
    let ids: Vec<Uuid> = assignments.iter().map(|a| a.id).collect();
    let conflicts = infra::conflicts::open_of(&mut tx, &ids).await?;
    tx.commit().await?;
    let items: Vec<AssignmentView> = assignments
        .into_iter()
        .map(|a| view(a, &loads, &flagged, &conflicts, &issue))
        .collect();
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
                   within the ticket's dates, when it has them. The resulting load of each day is \
                   checked against the resource's capacity plus the tenant's tolerance: in blocking \
                   mode an overload is refused unless forced (`force`, with `force_overload`); in \
                   warning mode it is written and flagged (`overloads`).",
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
        (status = 403, description = "`permission.denied` (`assign_resources`, or `force_overload` when \
                                      forcing), or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`assignment.resource_taken`, `assignment.segments_overlap`, \
                                      `assignment.outside_issue_dates`, `workload.overload` (with `overload`: \
                                      the days, and whether the member may force it), `node.archived`, or \
                                      `idempotency_key.in_flight`",
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
    let outcome = check_overload(
        &state,
        &mut tx,
        &project,
        &Candidate {
            assignment_id: None,
            issue_id: issue.id,
            resource_id: resource.id,
            calendar_id: resource.calendar_id,
            provisional: body.provisional,
            segments: &segments,
        },
        body.force,
    )
    .await?;
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
    trace_forced(&mut tx, &outcome, &assignment).await?;
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
                   provisional flag are replaced; its resource stays. Checked for overload as a \
                   creation is, on the days it adds load to.",
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
        (status = 409, description = "`assignment.segments_overlap`, `assignment.outside_issue_dates`, \
                                      `workload.overload`, or `node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
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
    let outcome = check_overload(
        &state,
        &mut tx,
        &project,
        &Candidate {
            assignment_id: Some(id),
            issue_id: issue.id,
            resource_id: assignment.resource.id,
            calendar_id: assignment.resource.calendar_id,
            provisional: body.provisional,
            segments: &segments,
        },
        body.force,
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
    trace_forced(&mut tx, &outcome, &updated).await?;
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

/// `?from=&to=&limit=&cursor=&expand=` of a resource's assignments.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct ResourceListParams {
    /// Only those with a segment ending on or after this local date.
    #[param(value_type = Option<String>, format = Date)]
    from: Option<Date>,
    /// Only those with a segment starting on or before this local date.
    #[param(value_type = Option<String>, format = Date)]
    to: Option<Date>,
    /// Page size, from 1 to 200 (default 50).
    #[param(minimum = 1, maximum = 200)]
    limit: Option<usize>,
    /// `nextCursor` of the previous page.
    cursor: Option<String>,
    /// `issue`: each visible assignment with its ticket and project.
    expand: Option<String>,
}

/// The project of a ticket, as listed with an assignment.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AssignedProject)]
pub struct ProjectRefView {
    id: Uuid,
    #[schema(example = "PLAT")]
    key: String,
    #[schema(example = "Identity platform")]
    name: String,
}

/// The ticket of an assignment (`expand=issue`).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AssignedIssue)]
pub struct IssueRefView {
    id: Uuid,
    #[schema(example = "PLAT-148")]
    key: String,
    summary: String,
    project: ProjectRefView,
}

/// An assignment of a resource, as its page lists it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ResourceAssignment)]
pub struct ResourceAssignmentView {
    id: Uuid,
    /// On a ticket the member does not see (a confidential project, or one
    /// not granted to them): only its periods and hours show, as busy time.
    busy: bool,
    /// `null` when busy.
    #[schema(required = true)]
    issue_id: Option<Uuid>,
    /// With `expand=issue`, when not busy.
    #[serde(skip_serializing_if = "Option::is_none")]
    issue: Option<IssueRefView>,
    provisional: bool,
    segments: Vec<SegmentView>,
    #[schema(example = 1830)]
    load_minutes: u32,
    /// As on the ticket; empty when busy.
    overloads: Vec<OverloadDay>,
    /// As on the ticket; empty when busy.
    conflicts: Vec<ConflictDayView>,
    /// The member changes and removes it (`assign_resources` on its
    /// project, not archived).
    can_edit: bool,
    #[schema(example = 1)]
    version: i64,
}

/// A page of a resource's assignments.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ResourceAssignmentPage)]
pub struct ResourceAssignmentPage {
    /// Newest first.
    items: Vec<ResourceAssignmentView>,
    /// Cursor of the next (older) page, `null` on the last page.
    #[schema(required = true)]
    next_cursor: Option<String>,
}

/// `GET /api/v1/resources/{id}/assignments`.
#[utoipa::path(
    get,
    path = "/resources/{id}/assignments",
    tag = crate::resources::TAG,
    operation_id = "listResourceAssignments",
    summary = "List the assignments of a resource",
    description = "Internal members: the resource's assignments with a segment meeting \
                   `from`–`to`, newest first, each with its segments and their load. An assignment \
                   on a ticket the member does not see shows as busy time: its periods and hours, \
                   no ticket nor project. `expand=issue` adds the ticket and its project.",
    params(("id" = Uuid, Path, description = "Id of the resource"), ResourceListParams),
    responses(
        (status = 200, description = "A page of assignments", body = ResourceAssignmentPage),
        (status = 400, description = "Invalid `limit`, `cursor`, dates or `expand`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such resource in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn resource_list(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(params): ApiQuery<ResourceListParams>,
) -> Result<ApiJson<ResourceAssignmentPage>, Problem> {
    let (limit, before) = PageParams {
        limit: params.limit,
        cursor: params.cursor,
    }
    .resolve()?;
    let expand_issue = match params.expand.as_deref() {
        None | Some("") => false,
        Some("issue") => true,
        Some(_) => {
            return Err(Problem::validation(vec![FieldError::new(
                "expand",
                "unknown_field",
            )]));
        }
    };
    if let (Some(from), Some(to)) = (params.from, params.to)
        && to < from
    {
        return Err(Problem::validation(vec![FieldError::new("to", "before_start")]));
    }
    if infra::resources::find(&mut tx, id).await?.is_none() {
        return Err(Problem::new(ErrorCode::NotFound).with_detail("no such resource"));
    }
    let page = repo::of_resource(&mut tx, id, params.from, params.to, limit, before).await?;
    let issue_ids: Vec<Uuid> = page.items.iter().map(|a| a.issue_id).collect();
    let refs = repo::issue_refs(&mut tx, &issue_ids).await?;
    let sight = Sight::of(&mut tx).await?;
    let all: Vec<&Assignment> = page.items.iter().collect();
    let loads = loads(&mut tx, &all).await?;
    let policy = checks::policy(&mut tx).await?;
    let flagged = checks::flagged(&mut tx, &policy, &all).await?;
    let ids: Vec<Uuid> = page.items.iter().map(|a| a.id).collect();
    let conflicts = infra::conflicts::open_of(&mut tx, &ids).await?;
    // Whether the member assigns on each project, asked once per project.
    let mut assigns_on: HashMap<Uuid, bool> = HashMap::new();
    let mut items = Vec::with_capacity(page.items.len());
    for assignment in page.items {
        let issue = refs.get(&assignment.issue_id);
        let seen = issue.filter(|r| sight.sees(r.project_id));
        let can_edit = match seen {
            Some(r) if !r.project_archived => match assigns_on.get(&r.project_id) {
                Some(known) => *known,
                None => {
                    let known = is_tenant_admin(&tx)
                        || permissions::effective(&state, &mut tx, Some(r.project_id))
                            .await?
                            .contains(Permission::AssignResources);
                    assigns_on.insert(r.project_id, known);
                    known
                }
            },
            _ => false,
        };
        let busy = seen.is_none();
        let issue_view = seen.filter(|_| expand_issue).map(|r| IssueRefView {
            id: r.issue_id,
            key: r.key.clone(),
            summary: r.summary.clone(),
            project: ProjectRefView {
                id: r.project_id,
                key: r.project_key.clone(),
                name: r.project_name.clone(),
            },
        });
        let (version, provisional, assignment_id, issue_id) = (
            assignment.version,
            assignment.provisional,
            assignment.id,
            assignment.issue_id,
        );
        let per_segment = loads.get(&assignment_id).cloned().unwrap_or_default();
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
        items.push(ResourceAssignmentView {
            id: assignment_id,
            busy,
            issue_id: (!busy).then_some(issue_id),
            issue: issue_view,
            provisional,
            load_minutes: segments.iter().map(|s| s.load_minutes).sum(),
            segments,
            overloads: if busy {
                Vec::new()
            } else {
                flagged
                    .get(&assignment_id)
                    .map(|o| overload_days(o))
                    .unwrap_or_default()
            },
            conflicts: if busy {
                Vec::new()
            } else {
                conflicts
                    .get(&assignment_id)
                    .map(|days| {
                        days.iter()
                            .map(|d| ConflictDayView {
                                date: d.day,
                                minutes: d.minutes,
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            },
            can_edit,
            version,
        });
    }
    tx.commit().await?;
    Ok(ApiJson(ResourceAssignmentPage {
        items,
        next_cursor: page.next.map(encode_cursor),
    }))
}

/// What a tenant does with an overload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = OverloadMode)]
pub enum OverloadModeView {
    /// Refused, unless forced.
    Blocking,
    /// Written and flagged.
    Warning,
}

impl From<OverloadMode> for OverloadModeView {
    fn from(mode: OverloadMode) -> Self {
        match mode {
            OverloadMode::Blocking => Self::Blocking,
            OverloadMode::Warning => Self::Warning,
        }
    }
}

/// An assignment to preview.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = AssignmentPreviewFields, example = json!({
    "issueId": "0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a01",
    "resourceId": "0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b01",
    "segments": [{"start": "2026-11-16", "end": "2026-11-20", "mode": "hours_per_day", "value": 240}]
}))]
pub struct PreviewBody {
    /// The ticket.
    issue_id: Uuid,
    /// The resource: an active one, or that of `assignmentId`.
    resource_id: Uuid,
    /// The assignment changed, of this ticket and resource; left out for a
    /// new one.
    #[serde(default)]
    assignment_id: Option<Uuid>,
    #[serde(default)]
    provisional: bool,
    /// As for a write.
    segments: Vec<SegmentBody>,
}

/// One day of a preview.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AssignmentPreviewDay)]
pub struct PreviewDayView {
    #[schema(value_type = String, format = Date, example = "2026-11-18")]
    date: Date,
    /// Minutes of capacity, absences deducted.
    #[schema(example = 420)]
    capacity_minutes: u32,
    /// Minutes allowed: the capacity plus the tolerance.
    #[schema(example = 420)]
    limit_minutes: u32,
    /// Load of the resource's other assignments.
    #[schema(example = 240)]
    existing_minutes: u32,
    /// Load of this assignment as previewed.
    #[schema(example = 240)]
    assignment_minutes: u32,
    /// Their sum.
    #[schema(example = 480)]
    total_minutes: u32,
    /// The write would overload it.
    overloaded: bool,
}

/// What writing an assignment would give, without writing it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = AssignmentPreview)]
pub struct PreviewView {
    /// The days of its segments that have capacity or load, by date. Loads
    /// are counted as the check counts them: provisional assignments by the
    /// tenant's setting.
    days: Vec<PreviewDayView>,
    /// The days the write would overload, by date.
    overloads: Vec<OverloadDay>,
    mode: OverloadModeView,
    #[schema(example = 0)]
    tolerance_percent: u8,
    /// Provisional assignments count in the check.
    counts_provisional: bool,
    /// The write would be refused (blocking mode, overloads, not forced).
    blocked: bool,
    /// The member may force it.
    forceable: bool,
}

fn preview_days(days: &[overload_rules::DayLoad], policy: &Policy) -> Vec<PreviewDayView> {
    days.iter()
        .filter(|d| d.capacity > 0 || d.existing > 0 || d.after > 0)
        .filter_map(|d| {
            Some(PreviewDayView {
                date: sql_date(d.date)?,
                capacity_minutes: d.capacity,
                limit_minutes: policy.limit(d.capacity),
                existing_minutes: d.existing,
                assignment_minutes: d.after,
                total_minutes: d.total(),
                overloaded: overload_rules::overloads_day(d, policy),
            })
        })
        .collect()
}

/// `POST /api/v1/assignments/preview`: writes nothing.
#[utoipa::path(
    post,
    path = "/assignments/preview",
    tag = TAG,
    operation_id = "previewAssignment",
    summary = "Preview the load of an assignment",
    description = "Members with `assign_resources` on the ticket's project: for each day of the \
                   segments, the load of the resource's other assignments, this assignment's and \
                   their sum against the capacity plus tolerance, and the days a write would \
                   overload. The same check as a write, without writing or locking.",
    request_body = PreviewBody,
    responses(
        (status = 200, description = "The preview", body = PreviewView),
        (status = 400, description = "`validation_failed`, as for a write (`assignmentId`: `not_found`)",
         body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`assignment.segments_overlap`, `assignment.outside_issue_dates`, or `node.archived`",
         body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn preview(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiJson(body): ApiJson<PreviewBody>,
) -> Result<ApiJson<PreviewView>, Problem> {
    let (issue, project) = visible_issue(&state, &mut tx, &body.issue_id.to_string()).await?;
    require_assign(&state, &mut tx, &project).await?;
    let resource = match body.assignment_id {
        Some(id) => {
            let assignment = repo::find(&mut tx, id)
                .await?
                .filter(|a| a.issue_id == issue.id && a.resource.id == body.resource_id)
                .ok_or_else(|| Problem::validation(vec![FieldError::new("assignmentId", "not_found")]))?;
            (assignment.resource.id, assignment.resource.calendar_id)
        }
        None => match infra::resources::find(&mut tx, body.resource_id).await? {
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
            Some(r) => (r.id, r.calendar_id),
        },
    };
    let segments = checked(&body.segments, (issue.start_date, issue.due_date))?;
    require_placeable(&mut tx, resource, &segments).await?;
    let forceable = forces(&state, &mut tx, &project).await?;
    let policy = checks::policy(&mut tx).await?;
    let days = checks::day_loads(
        &mut tx,
        &policy,
        &Candidate {
            assignment_id: body.assignment_id,
            issue_id: issue.id,
            resource_id: resource.0,
            calendar_id: resource.1,
            provisional: body.provisional,
            segments: &segments,
        },
    )
    .await?;
    tx.commit().await?;
    let overloads = overload_rules::overloads(&days, &policy);
    let blocked = matches!(
        overload_rules::decide(&policy, overloads.clone(), false),
        Outcome::Refused(_)
    );
    Ok(ApiJson(PreviewView {
        days: preview_days(&days, &policy),
        overloads: overload_days(&overloads),
        mode: policy.mode.into(),
        tolerance_percent: policy.tolerance_percent,
        counts_provisional: policy.count_provisional,
        blocked,
        forceable,
    }))
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

    fn local(text: &str) -> domain::dates::Date {
        domain::dates::Date::parse(text).unwrap_or_else(|| panic!("{text}"))
    }

    #[test]
    fn an_overload_maps_to_its_problem_with_every_day() {
        let days = [
            Overload {
                date: local("2026-11-18"),
                planned: 480,
                available: 420,
                limit: 420,
            },
            Overload {
                date: local("2026-11-19"),
                planned: 500,
                available: 420,
                limit: 462,
            },
        ];
        let problem = overload_problem(&days, false);
        assert_eq!(problem.code, ErrorCode::WorkloadOverload);
        assert_eq!(
            problem.detail.as_deref(),
            Some("2 day(s) over the resource's capacity plus tolerance, the first on 2026-11-18")
        );
        let overload = problem.overload.unwrap_or_else(|| panic!("no days"));
        assert!(!overload.forceable);
        assert_eq!(
            overload.days[1],
            OverloadDay {
                date: date!(2026 - 11 - 19),
                planned_minutes: 500,
                available_minutes: 420,
                limit_minutes: 462,
            }
        );
    }

    #[test]
    fn a_preview_keeps_the_days_with_capacity_or_load() {
        let policy = Policy {
            tolerance_percent: 10,
            ..Policy::default()
        };
        let day = |text: &str, capacity, existing, after| overload_rules::DayLoad {
            date: local(text),
            capacity,
            existing,
            before: 0,
            after,
        };
        let days = preview_days(
            &[
                day("2026-11-20", 420, 240, 240),
                // A weekend with nothing, then one with load.
                day("2026-11-21", 0, 0, 0),
                day("2026-11-22", 0, 60, 0),
                day("2026-11-23", 420, 0, 0),
            ],
            &policy,
        );
        let dates: Vec<Date> = days.iter().map(|d| d.date).collect();
        assert_eq!(
            dates,
            [
                date!(2026 - 11 - 20),
                date!(2026 - 11 - 22),
                date!(2026 - 11 - 23)
            ]
        );
        assert_eq!(
            (days[0].limit_minutes, days[0].total_minutes, days[0].overloaded),
            (462, 480, true)
        );
        assert!(!days[1].overloaded, "the assignment adds nothing to it");
    }

    #[test]
    fn modes_round_trip() {
        for mode in Mode::ALL {
            assert_eq!(Mode::from(ModeView::from(mode)), mode);
        }
    }
}
