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
use domain::comments::Visibility;
use domain::fields::{Step, Value};
use domain::issues::{Current, IssueChanges, missing_required, validate_changes};
use domain::rich_text::{Block, Doc, Inline};
use domain::transitions::{Actor, Change, Condition, Refusal, Rules, Ticket, allowed, apply, evaluate};
use domain::validation::FieldError;
use infra::comments::Source;
use infra::issues::{self as repo, Issue};
use infra::uuid::Uuid;
use infra::workflows::Transition;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::etag::require_if_match;
use crate::extract::{AcceptLanguage, ApiJson, ApiPath};
use crate::issue_changes::{locked, require_edit, ticket_response};
use crate::issues::{
    Content, FormFieldView, IssueView, StatusView, TAG, field_defs, fields_shown, ids_in, issue_view,
    read_fields, type_ids_of, unknown_references, visible_issue,
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
async fn actor(state: &AppState, tx: &mut TenantTx, project: Uuid) -> Result<(Actor, bool), Problem> {
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

/// The rules of `transition` as the actor meets them: a tenant admin meets
/// every condition.
fn rules_for(transition: &Transition, admin: bool) -> Rules {
    if admin {
        Rules {
            condition: Condition::default(),
            ..transition.rules.clone()
        }
    } else {
        transition.rules.clone()
    }
}

/// The transitions of the workflow of `issue`'s type that leave its status
/// (or any status).
fn leaving<'a>(content: &'a Content, issue: &Issue) -> Vec<&'a Transition> {
    let Some(workflow) = content
        .types
        .iter()
        .find(|t| t.item_id == issue.type_id)
        .and_then(|t| t.workflow_id)
    else {
        return Vec::new();
    };
    content
        .workflows
        .transitions_of(workflow)
        .filter(|t| t.from_status_id.is_none_or(|from| from == issue.status_id))
        .collect()
}

/// The fields `transition` takes for `issue`: those of its transition
/// screen, its validators (required), then those required at the target
/// status, the rules of the target status applied.
fn screen_fields_of(
    language: &Language,
    content: &Content,
    transition: &Transition,
    issue: &Issue,
) -> Vec<FormFieldView> {
    let mut wanted: Vec<u128> = transition
        .screen_id
        .and_then(|screen| content.screens.iter().find(|s| s.item_id == screen))
        .map(|s| s.field_ids.iter().map(Uuid::as_u128).collect())
        .unwrap_or_default();
    for validator in &transition.rules.validators {
        if !wanted.contains(validator) {
            wanted.push(*validator);
        }
    }
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

fn stored_values(fields: &serde_json::Value) -> BTreeMap<u128, Value> {
    fields
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(id, json)| {
            Some((
                Uuid::parse_str(id).ok()?.as_u128(),
                infra::fields::value_of(json)?,
            ))
        })
        .collect()
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
        .map(|t| TransitionView {
            id: t.item_id,
            name: language.label(&t.name_translations),
            to_status: language.status_view(&content, t.to_status_id),
            fields: screen_fields_of(&language, &content, t, &issue),
        })
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
    let rules = rules_for(transition, admin);
    if !allowed(&rules.condition, &actor) {
        return Err(Problem::new(ErrorCode::PermissionDenied).with_detail("the transition is restricted"));
    }
    let language = Language::of(&tx, &accept);
    let type_id = issue.type_id.as_u128();
    let to = transition.to_status_id;

    // The values it takes: the fields it lists only, at the target status.
    let mut errors = Vec::new();
    let given = read_fields(&body.fields).unwrap_or_else(|e| {
        errors.extend(e);
        BTreeMap::new()
    });
    let on_screen: BTreeSet<u128> = screen_fields_of(&language, &content, transition, &issue)
        .iter()
        .map(|f| f.field_id().as_u128())
        .collect();
    for id in body.fields.keys() {
        if !on_screen.contains(&id.as_u128()) {
            errors.push(FieldError::new(format!("fields.{id}"), "not_applicable"));
        }
    }
    let current_values = stored_values(&issue.fields);
    let type_ids = type_ids_of(&content.fields);
    let defs = field_defs(&content.fields, &type_ids);
    let changes = IssueChanges {
        summary: None,
        dates_in_order: true,
        fields: &given,
    };
    let target = Current {
        type_id,
        status_id: to.as_u128(),
        fields: &current_values,
    };
    if errors.is_empty()
        && let Err(e) = validate_changes(&changes, &target, &defs)
    {
        errors.extend(e);
    }
    let unknown = unknown_references(&mut tx, &content.fields, &given, &errors).await?;
    errors.extend(unknown);
    if !errors.is_empty() {
        return Err(Problem::validation(errors));
    }

    // The validators and post-functions, on the ticket with its screen values.
    let mut with_screen = current_values.clone();
    for (id, value) in &given {
        if value.is_empty() {
            with_screen.remove(id);
        } else {
            with_screen.insert(*id, value.clone());
        }
    }
    let ticket = Ticket {
        reporter: issue.reporter_id.map(|id| id.as_u128()),
        assignee: issue.assignee_id.map(|id| id.as_u128()),
        fields: with_screen,
    };
    let post = evaluate(&rules, &ticket, &actor).map_err(|refusals| {
        if refusals.contains(&Refusal::NotAllowed) {
            return Problem::new(ErrorCode::PermissionDenied).with_detail("the transition is restricted");
        }
        Problem::validation(
            refusals
                .iter()
                .filter_map(|r| match r {
                    Refusal::FieldRequired(field) => Some(FieldError::new(
                        format!("fields.{}", Uuid::from_u128(*field)),
                        "required",
                    )),
                    Refusal::NotAllowed => None,
                })
                .collect(),
        )
    })?;
    let after = apply(&ticket, &post);
    let missing = missing_required(&defs, type_id, Step::Status(to.as_u128()), &after.fields);
    if !missing.is_empty() {
        return Err(Problem::validation(
            missing
                .iter()
                .map(|field| FieldError::new(format!("fields.{}", Uuid::from_u128(*field)), "required"))
                .collect(),
        ));
    }

    // What is stored: the screen values as given, then the post-functions.
    let mut stored = issue.fields.as_object().cloned().unwrap_or_default();
    for (id, json) in &body.fields {
        if given.get(&id.as_u128()).is_some_and(Value::is_empty) {
            stored.remove(&id.to_string());
        } else {
            stored.insert(id.to_string(), json.clone());
        }
    }
    let mut comments = Vec::new();
    for change in &post {
        match change {
            Change::SetField { field, value } => {
                stored.insert(Uuid::from_u128(*field).to_string(), infra::fields::json_of(value));
            }
            Change::ClearField { field } => {
                stored.remove(&Uuid::from_u128(*field).to_string());
            }
            Change::AddComment { text } => comments.push(text.clone()),
            Change::Assign(_) => {}
        }
    }
    let assignee = after.assignee.map(Uuid::from_u128);
    let mut diff = Vec::new();
    if assignee != issue.assignee_id {
        diff.push(serde_json::json!({"field": "assigneeId", "old": issue.assignee_id, "new": assignee}));
    }
    let ids: BTreeSet<&String> = stored
        .keys()
        .chain(issue.fields.as_object().into_iter().flatten().map(|(k, _)| k))
        .collect();
    for id in ids {
        let old = issue.fields.get(id).cloned().unwrap_or_default();
        let new = stored.get(id).cloned().unwrap_or_default();
        if old != new {
            diff.push(serde_json::json!({"field": format!("fields.{id}"), "old": old, "new": new}));
        }
    }
    let stored = serde_json::Value::Object(stored);
    let from = issue.status_id;
    let moved = repo::transition(&mut tx, issue.id, to, assignee, &stored).await?;
    for text in &comments {
        let doc = Doc(vec![Block::Paragraph(vec![Inline::Text {
            text: text.clone(),
            marks: Vec::new(),
        }])]);
        crate::issue_comments::write(&mut tx, &moved, Source::Transition, Visibility::Internal, &doc).await?;
    }
    let actor_id = tx.account_id();
    infra::outbox::record(
        &mut tx,
        &infra::outbox::Event {
            kind: "issue.transitioned",
            aggregate_type: "issue",
            aggregate_id: moved.id,
            payload: serde_json::json!({
                "key": moved.key,
                "projectId": project.id,
                "transitionId": transition.item_id,
                "fromStatusId": from,
                "toStatusId": to,
                "changes": diff,
                "comments": comments,
            }),
            actor_id: Some(actor_id),
        },
    )
    .await?;
    tracing::info!(target: "audit", actor = %actor_id, issue = %moved.id, key = %moved.key, transition = %transition.item_id, "ticket transitioned");
    let view = issue_view(&state, &mut tx, &language, moved, &project, true).await?;
    tx.commit().await?;
    Ok(ticket_response(view, &language))
}
