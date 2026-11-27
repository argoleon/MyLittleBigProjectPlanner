//! Replacing a generic resource with a person (specification §8,
//! L08-US08): tenant admins and resource managers move the assignments of
//! a generic resource, from today on, to a person (`infra::replacement`).
//!
//! - Days before today stay on the generic resource; a segment across today
//!   is cut, a total effort shared in proportion to its working days.
//! - When the person already has an assignment on a ticket, the segments
//!   join it; two segments taking a same day refuse the whole replacement
//!   (`assignment.segments_overlap`, naming the tickets).
//! - The person's load is checked as for an assignment: in blocking mode an
//!   overload is refused (`workload.overload`) unless forced, which needs
//!   `force_overload` on every project concerned (tenant admins always);
//!   forced, each ticket's overloaded days are traced.
//! - The generic resource is then deactivated, unless asked otherwise.
//!
//! The preview runs the same and writes nothing.

use axum::extract::State;
use domain::overload::{self as overload_rules, Outcome, Overload};
use domain::permissions::Permission;
use domain::resources::Kind;
use domain::validation::FieldError;
use infra::capacity::local_date;
use infra::overload::{self as checks, Forced};
use infra::replacement::{self, Moved, Replacement, Side, Ticket};
use infra::resources::{Resource, ResourceInput};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};
use utoipa::ToSchema;

use crate::assignments::{OverloadModeView, overload_days, overload_problem};
use crate::extract::{ApiJson, ApiPath};
use crate::nodes::{Sight, is_tenant_admin};
use crate::permissions;
use crate::problem::{ErrorCode, OverloadDay, PROBLEM_JSON, Problem, ProblemBody};
use crate::resources::{TAG, require_manager};
use crate::tenant::{AppState, TenantTx};

/// A replacement to preview.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = ResourceReplacementPreviewFields)]
pub struct PreviewBody {
    /// The person taking over: an active person resource.
    person_id: Uuid,
}

const fn yes() -> bool {
    true
}

/// A replacement to make.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = ResourceReplacementFields, example = json!({
    "personId": "0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b02",
    "deactivate": true
}))]
pub struct ReplaceBody {
    /// The person taking over: an active person resource.
    person_id: Uuid,
    /// Deactivate the generic resource afterwards; `true` when left out.
    #[serde(default = "yes")]
    deactivate: bool,
    /// Write it despite overloads in blocking mode: members with
    /// `force_overload` on every project concerned (tenant admins always).
    #[serde(default)]
    force: bool,
}

/// A ticket of a replacement. A ticket the member does not see shows
/// without its id and key.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ResourceReplacementTicket)]
pub struct TicketView {
    #[schema(required = true)]
    issue_id: Option<Uuid>,
    #[schema(required = true, example = "PLAT-148")]
    issue_key: Option<String>,
}

/// An assignment moved to the person.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ResourceReplacementMove)]
pub struct MovedView {
    #[serde(flatten)]
    ticket: TicketView,
    /// Joined to the person's assignment of the ticket.
    merged: bool,
    /// The first and last day moved.
    #[schema(value_type = String, format = Date, example = "2026-11-27")]
    start: Date,
    #[schema(value_type = String, format = Date, example = "2026-12-18")]
    end: Date,
}

/// What a replacement does, or would do.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ResourceReplacement)]
pub struct ReplacementView {
    /// The first day moved: today.
    #[schema(value_type = String, format = Date, example = "2026-11-27")]
    from: Date,
    /// The assignments moved.
    moved: Vec<MovedView>,
    /// Tickets where the person's segments and the moved ones would take a
    /// same day: the replacement is refused while there are any.
    overlaps: Vec<TicketView>,
    /// Tickets of archived projects, left on the generic resource.
    archived: Vec<TicketView>,
    /// The person's days the replacement would overload, by date.
    overloads: Vec<OverloadDay>,
    mode: OverloadModeView,
    /// The replacement would be refused: overlaps, or overloads in blocking
    /// mode without forcing.
    blocked: bool,
    /// The member may force the overloads.
    forceable: bool,
    /// The generic resource was deactivated (always `false` in a preview).
    deactivated: bool,
}

fn ticket_view(ticket: &Ticket, sight: &Sight) -> TicketView {
    if sight.sees(ticket.project_id) {
        TicketView {
            issue_id: Some(ticket.issue_id),
            issue_key: Some(ticket.key.clone()),
        }
    } else {
        TicketView {
            issue_id: None,
            issue_key: None,
        }
    }
}

fn moved_view(moved: &Moved, sight: &Sight) -> MovedView {
    MovedView {
        ticket: ticket_view(&moved.ticket, sight),
        merged: moved.merged,
        start: moved.first,
        end: moved.last,
    }
}

/// The generic resource `id` and the person `person_id`, checked.
async fn sides(tx: &mut TenantTx, id: Uuid, person_id: Uuid) -> Result<(Resource, Resource), Problem> {
    let generic = infra::resources::find(tx, id)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such resource"))?;
    if generic.kind != Kind::Generic {
        return Err(Problem::validation(vec![FieldError::new("id", "not_generic")]));
    }
    let error = |code: &'static str| Problem::validation(vec![FieldError::new("personId", code)]);
    let person = infra::resources::find(tx, person_id)
        .await?
        .ok_or_else(|| error("not_found"))?;
    if person.kind != Kind::Person {
        return Err(error("not_a_person"));
    }
    if !person.active {
        return Err(error("inactive"));
    }
    Ok((generic, person))
}

/// Whether the member forces overloads on every project of `moved`.
async fn forces(state: &AppState, tx: &mut TenantTx, moved: &[Moved]) -> Result<bool, Problem> {
    if is_tenant_admin(tx) {
        return Ok(true);
    }
    let mut projects: Vec<Uuid> = moved.iter().map(|m| m.ticket.project_id).collect();
    projects.sort_unstable();
    projects.dedup();
    for project in projects {
        if !permissions::effective(state, tx, Some(project))
            .await?
            .contains(Permission::ForceOverload)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Moves the assignments of `generic` to `person` from today on, in `tx`.
async fn run(
    tx: &mut TenantTx,
    generic: &Resource,
    person: &Resource,
    today: Date,
) -> Result<(Replacement, domain::overload::Policy), Problem> {
    let policy = checks::policy(tx).await?;
    let author = tx.author();
    let replacement = replacement::replace(
        tx,
        Side {
            id: generic.id,
            calendar_id: generic.calendar_id,
            name: &generic.display_name,
        },
        Side {
            id: person.id,
            calendar_id: person.calendar_id,
            name: &person.display_name,
        },
        today,
        &policy,
        author,
    )
    .await?;
    Ok((replacement, policy))
}

/// `assignment.segments_overlap`, naming the tickets the member sees.
fn overlap_problem(overlaps: &[Ticket], sight: &Sight) -> Problem {
    let keys: Vec<&str> = overlaps
        .iter()
        .filter(|t| sight.sees(t.project_id))
        .map(|t| t.key.as_str())
        .collect();
    let hidden = overlaps.len() - keys.len();
    let mut detail = format!(
        "the person's segments and the moved ones take a same day on {} ticket(s)",
        overlaps.len()
    );
    if !keys.is_empty() {
        detail.push_str(&format!(": {}", keys.join(", ")));
    }
    if hidden > 0 {
        detail.push_str(&format!(" ({hidden} not visible to you)"));
    }
    Problem::new(ErrorCode::AssignmentSegmentsOverlap).with_detail(detail)
}

/// The overloads within the days `moved` moved.
fn overloads_of(moved: &Moved, overloads: &[Overload]) -> Vec<Overload> {
    let (Some(first), Some(last)) = (local_date(moved.first), local_date(moved.last)) else {
        return Vec::new();
    };
    overloads
        .iter()
        .filter(|o| o.date >= first && o.date <= last)
        .copied()
        .collect()
}

/// `POST /api/v1/resources/{id}/replacement/preview`: writes nothing.
#[utoipa::path(
    post,
    path = "/resources/{id}/replacement/preview",
    tag = TAG,
    operation_id = "previewResourceReplacement",
    summary = "Preview replacing a generic resource with a person",
    description = "Tenant admins and resource managers: the assignments that would move from today \
                   on, the tickets where segments would overlap, those of archived projects left \
                   behind, and the person's days the replacement would overload. The same as a \
                   replacement, without writing.",
    params(("id" = Uuid, Path, description = "The generic resource")),
    request_body = PreviewBody,
    responses(
        (status = 200, description = "The preview", body = ReplacementView),
        (status = 400, description = "`validation_failed` (`id`: `not_generic`; `personId`: `not_found`, \
                                      `not_a_person`, `inactive`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: neither tenant admin nor resource manager; or `csrf.invalid`",
         body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such resource in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn preview(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<PreviewBody>,
) -> Result<ApiJson<ReplacementView>, Problem> {
    require_manager(&tx)?;
    let (generic, person) = sides(&mut tx, id, body.person_id).await?;
    let today = OffsetDateTime::now_utc().date();
    let (replacement, policy) = run(&mut tx, &generic, &person, today).await?;
    let forceable = forces(&state, &mut tx, &replacement.moved).await?;
    let sight = Sight::of(&mut tx).await?;
    // Dropped without committing: nothing written stays.
    drop(tx);
    let refused = matches!(
        overload_rules::decide(&policy, replacement.overloads.clone(), false),
        Outcome::Refused(_)
    );
    Ok(ApiJson(ReplacementView {
        from: today,
        moved: replacement.moved.iter().map(|m| moved_view(m, &sight)).collect(),
        overlaps: replacement
            .overlaps
            .iter()
            .map(|t| ticket_view(t, &sight))
            .collect(),
        archived: replacement
            .archived
            .iter()
            .map(|t| ticket_view(t, &sight))
            .collect(),
        overloads: overload_days(&replacement.overloads),
        mode: policy.mode.into(),
        blocked: refused || !replacement.overlaps.is_empty(),
        forceable,
        deactivated: false,
    }))
}

/// `POST /api/v1/resources/{id}/replacement`.
#[utoipa::path(
    post,
    path = "/resources/{id}/replacement",
    tag = TAG,
    operation_id = "replaceResource",
    summary = "Replace a generic resource with a person",
    description = "Tenant admins and resource managers. The assignments of the generic resource move \
                   to the person from today on: days before stay, a segment across today is cut (a \
                   total effort shared in proportion to its working days), and segments join the \
                   person's assignment of the same ticket when there is one. Tickets of archived \
                   projects stay. Each ticket's history records it. The person's load is checked as \
                   for an assignment: in blocking mode an overload is refused unless forced \
                   (`force`, with `force_overload` on every project concerned), and a forced one is \
                   traced. The generic resource is then deactivated unless `deactivate` is `false`.",
    params(("id" = Uuid, Path, description = "The generic resource")),
    request_body = ReplaceBody,
    responses(
        (status = 200, description = "Replaced", body = ReplacementView),
        (status = 400, description = "`validation_failed` (`id`: `not_generic`; `personId`: `not_found`, \
                                      `not_a_person`, `inactive`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: neither tenant admin nor resource manager; \
                                      `permission.denied` (`force_overload` when forcing); or `csrf.invalid`",
         body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such resource in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`assignment.segments_overlap` (naming the tickets), or \
                                      `workload.overload` (with `overload`: the days, and whether the member \
                                      may force it); nothing is written", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn replace(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ReplaceBody>,
) -> Result<ApiJson<ReplacementView>, Problem> {
    require_manager(&tx)?;
    let (generic, person) = sides(&mut tx, id, body.person_id).await?;
    let today = OffsetDateTime::now_utc().date();
    let (replacement, policy) = run(&mut tx, &generic, &person, today).await?;
    let sight = Sight::of(&mut tx).await?;
    // Every refusal below drops the transaction: nothing is written.
    if !replacement.overlaps.is_empty() {
        return Err(overlap_problem(&replacement.overlaps, &sight));
    }
    let forceable = forces(&state, &mut tx, &replacement.moved).await?;
    if body.force && !forceable {
        return Err(
            Problem::new(ErrorCode::PermissionDenied).with_detail("missing permission: force_overload")
        );
    }
    let outcome = overload_rules::decide(&policy, replacement.overloads.clone(), body.force);
    if let Outcome::Refused(days) = &outcome {
        return Err(overload_problem(days, forceable));
    }
    let author = tx.author();
    if let Outcome::Forced(days) = &outcome {
        for moved in &replacement.moved {
            let overloads = overloads_of(moved, days);
            if overloads.is_empty() {
                continue;
            }
            checks::record_forced(
                &mut tx,
                author,
                &Forced {
                    assignment_id: moved.assignment_id,
                    issue_id: moved.ticket.issue_id,
                    resource_id: person.id,
                    resource_name: &person.display_name,
                    overloads: &overloads,
                },
            )
            .await?;
        }
    }
    let deactivated = body.deactivate && generic.active;
    if deactivated {
        let input = ResourceInput {
            kind: generic.kind,
            display_name: Some(&generic.display_name),
            job_title: generic.job_title.as_deref(),
            account_id: generic.account_id,
            active: false,
            calendar_id: generic.calendar_id,
        };
        infra::resources::update(&mut tx, generic.id, None, &input)
            .await?
            .map_err(|conflict| {
                Problem::new(ErrorCode::Internal).with_detail(format!("deactivating: {conflict:?}"))
            })?;
    }
    tracing::info!(target: "audit", actor = ?author.actor_id, generic = %generic.id, person = %person.id,
                   moved = replacement.moved.len(), forced = matches!(outcome, Outcome::Forced(_)),
                   deactivated, "generic resource replaced");
    tx.commit().await?;
    Ok(ApiJson(ReplacementView {
        from: today,
        moved: replacement.moved.iter().map(|m| moved_view(m, &sight)).collect(),
        overlaps: Vec::new(),
        archived: replacement
            .archived
            .iter()
            .map(|t| ticket_view(t, &sight))
            .collect(),
        overloads: overload_days(&replacement.overloads),
        mode: policy.mode.into(),
        blocked: false,
        forceable,
        deactivated,
    }))
}

#[cfg(test)]
mod tests {
    use domain::dates::Date as LocalDate;
    use time::macros::date;

    use super::*;

    fn ticket(key: &str, project: u128) -> Ticket {
        Ticket {
            issue_id: Uuid::from_u128(project * 100),
            key: key.to_owned(),
            project_id: Uuid::from_u128(project),
        }
    }

    #[test]
    fn overlaps_name_only_the_tickets_the_member_sees() {
        let sight = Sight::Limited(infra::grants::Sight {
            visible: [Uuid::from_u128(1)].into_iter().collect(),
            ..Default::default()
        });
        let problem = overlap_problem(&[ticket("PLAT-1", 1), ticket("SECRET-2", 2)], &sight);
        assert_eq!(problem.code, ErrorCode::AssignmentSegmentsOverlap);
        let detail = problem.detail.unwrap_or_default();
        assert!(
            detail.contains("2 ticket(s): PLAT-1 (1 not visible to you)"),
            "{detail}"
        );
        assert!(!detail.contains("SECRET"), "{detail}");
        assert_eq!(ticket_view(&ticket("SECRET-2", 2), &sight).issue_key, None);
        assert_eq!(
            ticket_view(&ticket("PLAT-1", 1), &Sight::All)
                .issue_key
                .as_deref(),
            Some("PLAT-1")
        );
    }

    #[test]
    fn a_forced_trace_keeps_the_days_its_ticket_moved() {
        let day = |d: u8| Overload {
            date: LocalDate::from_ymd(2026, 12, u32::from(d)).unwrap_or_else(|| unreachable!()),
            planned: 600,
            available: 420,
            limit: 420,
        };
        let moved = Moved {
            ticket: ticket("PLAT-1", 1),
            assignment_id: Uuid::from_u128(9),
            merged: false,
            first: date!(2026 - 12 - 02),
            last: date!(2026 - 12 - 03),
        };
        let kept = overloads_of(&moved, &[day(1), day(2), day(3), day(4)]);
        assert_eq!(kept, vec![day(2), day(3)]);
    }
}
