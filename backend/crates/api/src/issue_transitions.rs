//! Moving a ticket through its workflow (specification §5, L05-US09).
//!
//! The transitions offered are those of the workflow of the ticket's type
//! that leave its status (or any status), whose condition the member meets
//! (roles or groups on the project; tenant admins always). Running one
//! takes `edit` on the project and the version read (`If-Match`): the
//! values of its transition screen are checked against the rules of the
//! target status, its validators must find their fields filled, its
//! post-functions apply in order (`domain::transitions::evaluate`), and the
//! fields required at the target status must then hold a value. The ticket
//! takes the target status and writes `issue.transitioned` to the outbox.
//!
//! Its "add a comment" post-functions write internal comments (L05-US04),
//! by the member who ran it, in the same transaction.

use std::collections::{BTreeMap, BTreeSet};

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use domain::fields::Step;
use domain::transitions::{Actor, allowed};
use domain::validation::FieldError;
use infra::issues::Issue;
use infra::nodes::Node;
use infra::tickets::stored_values;
use infra::tickets::transition::Refused;
use infra::uuid::Uuid;
use infra::workflows::Transition;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::etag::require_if_match;
use crate::extract::{AcceptLanguage, ApiJson, ApiPath};
use crate::issue_changes::{locked, require_edit, ticket_response};
use crate::issues::{
    Content, FormFieldView, IssueView, StatusView, TAG, fields_shown, ids_in, issue_view, visible_issue,
};
use crate::nodes::{ensure_changeable, is_tenant_admin};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::tenant::{AppState, TenantTx};

/// A transition the member may run on a ticket.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueTransition)]
pub struct TransitionView {
    id: Uuid,
    #[schema(example = "Send to review")]
    name: Option<String>,
    /// The status it leads to.
    to_status: StatusView,
    /// The fields it takes, shown before it runs: those of its transition
    /// screen, its validators (required), and those required at the
    /// target status; the rules of the target status applied. Empty: it
    /// runs at once.
    fields: Vec<FormFieldView>,
}

/// How `transition` shows to the member about to run it on `issue`.
pub(crate) fn transition_view(
    language: &Language,
    content: &Content,
    transition: &Transition,
    issue: &Issue,
) -> TransitionView {
    TransitionView {
        id: transition.item_id,
        name: language.label(&transition.name_translations),
        to_status: language.status_view(content, transition.to_status_id),
        fields: screen_fields_of(language, content, transition, issue),
    }
}

impl TransitionView {
    /// It runs at once: no field to fill before.
    pub(crate) fn immediate(&self) -> bool {
        self.fields.is_empty()
    }
}

/// A transition to run.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = IssueTransitionRun, example = json!({"transitionId": "01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9a", "fields": {}}))]
pub struct RunBody {
    transition_id: Uuid,
    /// Values of the fields of its transition screen, by field id; `null`
    /// clears one.
    #[serde(default)]
    fields: BTreeMap<Uuid, serde_json::Value>,
}

/// Who runs transitions: their account, roles on the project, groups;
/// tenant admins meet every condition.
pub(crate) async fn actor(
    state: &AppState,
    tx: &mut TenantTx,
    project: Uuid,
) -> Result<(Actor, bool), Problem> {
    let account = tx.account_id();
    let roles = state.grants.roles_of(tx, account, Some(project)).await?;
    let groups = infra::groups::of_members(tx, &[account]).await?;
    Ok((
        Actor {
            account: account.as_u128(),
            roles: roles.iter().map(Uuid::as_u128).collect(),
            groups: groups.iter().map(|(_, g)| g.id.as_u128()).collect(),
        },
        is_tenant_admin(tx),
    ))
}

/// The fields `transition` takes for `issue`: those of its transition
/// screen, its validators (required), then those required at the target
/// status, the rules of the target status applied.
pub(crate) fn screen_fields_of(
    language: &Language,
    content: &Content,
    transition: &Transition,
    issue: &Issue,
) -> Vec<FormFieldView> {
    let wanted = infra::tickets::transition::wanted(content, transition);
    let held: BTreeSet<u128> = stored_values(&issue.fields)
        .values()
        .flat_map(ids_in)
        .map(|id| id.as_u128())
        .collect();
    let mut fields = fields_shown(
        language,
        content,
        &wanted,
        issue.type_id.as_u128(),
        Step::Status(transition.to_status_id.as_u128()),
        &held,
    );
    for field in &mut fields {
        if transition.rules.validators.contains(&field.field_id().as_u128()) {
            field.require();
        }
    }
    fields
}

/// `GET /api/v1/issues/{idOrKey}/transitions`.
#[utoipa::path(
    get,
    path = "/issues/{idOrKey}/transitions",
    tag = TAG,
    operation_id = "listIssueTransitions",
    summary = "The transitions a member may run on a ticket",
    description = "Members with `edit` on its project. Those of the workflow of its type that \
                   leave its status (or any status) and whose role or group condition the member \
                   meets (tenant admins meet all), in the order of the workflow, each with the \
                   fields of its transition screen.",
    params(
        ("idOrKey" = String, Path, description = "Id, key or former key of the ticket"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    responses(
        (status = 200, description = "The transitions", body = Vec<TransitionView>),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id_or_key): ApiPath<String>,
) -> Result<Response, Problem> {
    let (issue, project) = visible_issue(&state, &mut tx, &id_or_key).await?;
    require_edit(&state, &mut tx, project.id).await?;
    let content = Content::of(&mut tx, project.id).await?;
    let (actor, admin) = actor(&state, &mut tx, project.id).await?;
    let language = Language::of(&tx, &accept);
    tx.commit().await?;
    let views: Vec<TransitionView> = leaving(&content, &issue)
        .into_iter()
        .filter(|t| admin || allowed(&t.rules.condition, &actor))
        .map(|t| transition_view(&language, &content, t, &issue))
        .collect();
    let mut response = ApiJson(views).into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `POST /api/v1/issues/{idOrKey}/transitions`: requires `If-Match`.
#[utoipa::path(
    post,
    path = "/issues/{idOrKey}/transitions",
    tag = TAG,
    operation_id = "runIssueTransition",
    summary = "Move a ticket through its workflow",
    description = "Members with `edit` on its project who meet the transition's condition (tenant \
                   admins always). The values of its transition screen follow the rules of the \
                   target status; its validators need their fields filled; its post-functions \
                   (assign, set or clear a field) apply in order; then the fields required at the \
                   target status must hold a value. Writes `issue.transitioned` to the event \
                   outbox, with the changes and the comments its post-functions add.",
    params(
        ("idOrKey" = String, Path, description = "Id, key or former key of the ticket"),
        ("If-Match" = String, Header, description = "ETag of the version moved, or `*`"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    request_body = RunBody,
    responses(
        (status = 200, description = "The ticket in its new status", body = IssueView, headers(
            ("ETag" = String, description = "Version of the ticket"),
        )),
        (status = 400, description = "`validation_failed`: `transitionId` `not_found` (not offered from its status), `fields.{id}` (`required`, `not_applicable`, a value code, `unknown_member`, `unknown_group`, `unknown_issue`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`: no `edit`, or the transition's condition is not met; or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "`precondition_failed`: the ticket changed since this version", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`precondition_required`: `If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn run(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath(id_or_key): ApiPath<String>,
    ApiJson(body): ApiJson<RunBody>,
) -> Result<Response, Problem> {
    let if_match = require_if_match(&headers)?;
    let (issue, project) = visible_issue(&state, &mut tx, &id_or_key).await?;
    require_edit(&state, &mut tx, project.id).await?;
    ensure_changeable(&project)?;
    let issue = locked(&mut tx, issue.id, if_match.versions()).await?;
    let content = Content::of(&mut tx, project.id).await?;
    let transition = leaving(&content, &issue)
        .into_iter()
        .find(|t| t.item_id == body.transition_id)
        .ok_or_else(|| Problem::validation(vec![FieldError::new("transitionId", "not_found")]))?;
    let (actor, admin) = actor(&state, &mut tx, project.id).await?;
    let language = Language::of(&tx, &accept);
    let ran = Run {
        project: project.id,
        content: &content,
        transition,
        actor: &actor,
        admin,
    };
    let view = perform(&state, &mut tx, &language, &ran, &project, issue, &body.fields).await?;
    tx.commit().await?;
    Ok(ticket_response(view, &language))
}

pub(crate) use infra::tickets::transition::{Run, leaving};

/// Runs `run.transition` on `issue` (locked, of `project`) as the member of
/// `tx` with the values `fields` of its transition screen
/// (`infra::tickets::transition::perform`): the ticket's view, or the
/// refusal as a problem.
pub(crate) async fn perform(
    state: &AppState,
    tx: &mut TenantTx,
    language: &Language,
    run: &Run<'_>,
    project: &Node,
    issue: Issue,
    fields: &BTreeMap<Uuid, serde_json::Value>,
) -> Result<IssueView, Problem> {
    let author = tx.author();
    let account = tx.account_id();
    match infra::tickets::transition::perform(tx, author, account, run, issue, fields).await? {
        Ok(moved) => issue_view(state, tx, language, moved, project, true).await,
        Err(Refused::Restricted) => {
            Err(Problem::new(ErrorCode::PermissionDenied).with_detail("the transition is restricted"))
        }
        Err(Refused::Invalid(errors)) => Err(Problem::validation(errors)),
    }
}
