//! Changing tickets (specification §5, L05-US02): editing their fields with
//! the version read (`If-Match`), and moving them to another project.
//!
//! An edit follows the rules of the ticket's type at its status
//! (`domain::issues::validate_changes`) and writes `issue.updated` to the
//! outbox with each changed field, old and new value. A move takes the
//! ticket's sub-tasks along: each gets the next key of the target project,
//! its former key kept as an alias; types and statuses the target
//! configuration lacks are mapped first (`domain::issues::plan_move`), and
//! values it cannot hold are dropped. Each moved ticket writes
//! `issue.moved`.

use std::collections::{BTreeMap, BTreeSet};

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use domain::fields::Step;
use domain::issues::Priority;
use domain::permissions::Permission;
use domain::screens::Kind as ScreenKind;
use domain::validation::FieldError;
use infra::issues::{self as repo, Issue};
use infra::nodes::Node;
use infra::uuid::Uuid;
use serde::{Deserialize, Deserializer, Serialize};
use time::Date;
use utoipa::ToSchema;

use crate::etag::{etag, require_if_match};
use crate::extract::{AcceptLanguage, ApiJson, ApiPath};
use crate::issues::{
    Content, FormFieldView, IssueRefView, IssueView, PriorityView, ProjectView, TAG, ids_in, issue_ref,
    issue_view, project_of_key, project_view, require_create, screen_fields, visible_issue,
};
use crate::nodes::{ensure_changeable, is_tenant_admin};
use crate::permissions;
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::tenant::{AppState, TenantTx};
use infra::tickets::moves::{PrepareError, Prepared};
use infra::tickets::stored_values;

/// Reads a field that may be absent (`None`), `null` (`Some(None)`) or a
/// value (`Some(Some(…))`).
pub(crate) fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

/// Changes to a ticket: only what is given changes.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = IssueChanges,
    example = json!({"summary": "Migrate to OIDC", "priority": "highest", "assigneeId": null, "fields": {}})
)]
pub struct ChangesBody {
    /// 1 to 255 characters.
    #[serde(default)]
    summary: Option<String>,
    /// A rich text document (Tiptap JSON); `null` removes the description.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<Object>)]
    description: Option<Option<serde_json::Value>>,
    #[serde(default)]
    priority: Option<PriorityView>,
    /// An active member; `null`: nobody.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<Uuid>)]
    assignee_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, format = Date)]
    start_date: Option<Option<Date>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, format = Date)]
    due_date: Option<Option<Date>>,
    /// Custom field values to change, by field id; `null` clears one.
    #[serde(default)]
    fields: BTreeMap<Uuid, serde_json::Value>,
}

/// What editing a ticket offers.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueEditForm)]
pub struct EditFormView {
    /// The fields of its type's view screen at its status, the rules
    /// applied: hidden ones left out, required ones marked.
    fields: Vec<FormFieldView>,
}

/// A type or status of the ticket's configuration mapped to one of the
/// target's.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = IssueMoveMapping)]
pub struct MappingBody {
    pub(crate) from_id: Uuid,
    pub(crate) to_id: Uuid,
}

/// Where a ticket moves.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = IssueMove, example = json!({"projectKey": "CRM", "typeMappings": [], "statusMappings": []}))]
pub struct MoveBody {
    /// The key of the target project.
    project_key: String,
    /// For each type the target configuration lacks.
    #[serde(default)]
    type_mappings: Vec<MappingBody>,
    /// For each status that is not in the workflow of its ticket's new type.
    #[serde(default)]
    status_mappings: Vec<MappingBody>,
}

/// A type of the target configuration.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueMoveTypeTarget)]
pub struct TypeTargetView {
    pub(crate) id: Uuid,
    pub(crate) label: Option<String>,
    pub(crate) is_subtask: bool,
    /// The types it may sit under.
    pub(crate) parent_ids: Vec<Uuid>,
    /// The statuses of its workflow.
    pub(crate) status_ids: Vec<Uuid>,
}

/// A type of moving tickets the target configuration lacks.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueMoveTypeNeed)]
pub struct TypeNeedView {
    pub(crate) id: Uuid,
    pub(crate) key: String,
    pub(crate) label: Option<String>,
    pub(crate) is_subtask: bool,
    #[schema(required = true)]
    pub(crate) suggested_id: Option<Uuid>,
}

/// A status of the target configuration.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueMoveStatusTarget)]
pub struct StatusTargetView {
    pub(crate) id: Uuid,
    pub(crate) label: Option<String>,
    pub(crate) category: crate::workflows::CategoryView,
}

/// A status of moving tickets that is not in the workflow of their new
/// type, with the suggested types.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueMoveStatusNeed)]
pub struct StatusNeedView {
    pub(crate) id: Uuid,
    pub(crate) key: String,
    pub(crate) label: Option<String>,
    pub(crate) category: crate::workflows::CategoryView,
    /// The types (of the ticket's configuration) of the tickets in it: the
    /// status lands in the workflow of their new type.
    pub(crate) type_ids: Vec<Uuid>,
    #[schema(required = true)]
    pub(crate) suggested_id: Option<Uuid>,
}

/// A custom field whose values are dropped by the move.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueMoveDroppedField)]
pub struct DroppedFieldView {
    pub(crate) field_id: Uuid,
    pub(crate) label: Option<String>,
}

/// A mapping problem, as the move would report it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueMoveProblem)]
pub struct ProblemView {
    /// `typeMappings.{id}` or `statusMappings.{id}`.
    pub(crate) field: String,
    /// `required`, `not_found`, `not_allowed`, `not_in_workflow`.
    pub(crate) code: &'static str,
}

/// What moving a ticket to a project takes.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueMovePlan)]
pub struct MovePlanView {
    project: ProjectView,
    /// Both projects use the same configuration: nothing to map.
    same_configuration: bool,
    /// The tickets that move: the one asked for, then its sub-tasks.
    tickets: Vec<IssueRefView>,
    /// The ticket it sits under, which stays: the link is removed.
    #[schema(required = true)]
    parent_cleared: Option<IssueRefView>,
    /// The types of the target configuration.
    targets: Vec<TypeTargetView>,
    /// The statuses of the target configuration.
    status_targets: Vec<StatusTargetView>,
    /// The types to map, with a suggestion.
    types: Vec<TypeNeedView>,
    /// The statuses to map, with a suggestion.
    statuses: Vec<StatusNeedView>,
    /// With the mappings given (else the suggestions): the fields whose
    /// values are dropped.
    dropped_fields: Vec<DroppedFieldView>,
    /// With the mappings given (else the suggestions): what the move would
    /// refuse; empty when it can be made.
    problems: Vec<ProblemView>,
}

/// 403 `permission.denied` unless the member may edit tickets of `project`.
pub(crate) async fn require_edit(state: &AppState, tx: &mut TenantTx, project: Uuid) -> Result<(), Problem> {
    if is_tenant_admin(tx) {
        return Ok(());
    }
    let permissions = permissions::effective(state, tx, Some(project)).await?;
    permissions::check(permissions, Permission::Edit)
}

/// The ticket `id`, locked until the transaction ends, if its version is
/// one of `versions` (`None`: any); else 412.
pub(crate) async fn locked(tx: &mut TenantTx, id: Uuid, versions: Option<&[i64]>) -> Result<Issue, Problem> {
    let not_found = || Problem::new(ErrorCode::NotFound).with_detail("no such ticket");
    let version = repo::lock(tx, id).await?.ok_or_else(not_found)?;
    if versions.is_some_and(|v| !v.contains(&version)) {
        return Err(
            Problem::new(ErrorCode::PreconditionFailed).with_detail("the ticket changed since this version")
        );
    }
    repo::find(tx, id).await?.ok_or_else(not_found)
}

/// The response of a ticket: its view, `ETag`, language.
pub(crate) fn ticket_response(view: IssueView, language: &Language) -> Response {
    let version = view.version();
    let mut response = ApiJson(view).into_response();
    response.headers_mut().insert(header::ETAG, etag(version));
    language.headers(&mut response);
    response
}

/// `PATCH /api/v1/issues/{idOrKey}`: requires `If-Match`.
#[utoipa::path(
    patch,
    path = "/issues/{idOrKey}",
    tag = TAG,
    operation_id = "updateIssue",
    summary = "Edit a ticket",
    description = "Members with `edit` on its project (tenant admins always). Only what is given \
                   changes; custom fields follow the rules of the ticket's type at its status, and \
                   only those changed are checked. Writes `issue.updated` to the event outbox with \
                   each changed field; nothing changed, nothing written (same version).",
    params(
        ("idOrKey" = String, Path, description = "Id, key or former key of the ticket"),
        ("If-Match" = String, Header, description = "ETag of the version edited, or `*`"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    request_body = ChangesBody,
    responses(
        (status = 200, description = "The ticket", body = IssueView, headers(
            ("ETag" = String, description = "Version of the ticket"),
        )),
        (status = 400, description = "`validation_failed`: `summary`, `description`, `assigneeId` `unknown_member`, `dueDate` `before_start`, `fields.{id}` (`required`, `not_applicable`, `not_found`, a value code, `unknown_member`, `unknown_group`, `unknown_issue`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "`precondition_failed`: the ticket changed since this version", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`precondition_required`: `If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath(id_or_key): ApiPath<String>,
    ApiJson(body): ApiJson<ChangesBody>,
) -> Result<Response, Problem> {
    let if_match = require_if_match(&headers)?;
    let (issue, project) = visible_issue(&state, &mut tx, &id_or_key).await?;
    require_edit(&state, &mut tx, project.id).await?;
    ensure_changeable(&project)?;
    let issue = locked(&mut tx, issue.id, if_match.versions()).await?;
    let content = Content::of(&mut tx, project.id).await?;
    let edit = infra::tickets::edit::Edit {
        summary: body.summary.as_deref(),
        description: body.description.clone(),
        priority: body.priority.map(Priority::from),
        assignee: body.assignee_id,
        start_date: body.start_date,
        due_date: body.due_date,
        fields: body.fields.clone(),
    };
    let author = tx.author();
    let account = tx.account_id();
    let edited = infra::tickets::edit::apply(&mut tx, author, account, project.id, issue, &content, &edit)
        .await?
        .map_err(Problem::validation)?;
    let language = Language::of(&tx, &accept);
    let view = issue_view(&state, &mut tx, &language, edited.issue, &project, false).await?;
    tx.commit().await?;
    Ok(ticket_response(view, &language))
}

/// `GET /api/v1/issues/{idOrKey}/edit-form`.
#[utoipa::path(
    get,
    path = "/issues/{idOrKey}/edit-form",
    tag = TAG,
    operation_id = "getIssueEditForm",
    summary = "What editing a ticket offers",
    description = "Members with `edit` on its project. The fields of its type's view screen at its \
                   status, the rules applied; list fields also offer the deactivated values the \
                   ticket holds.",
    params(
        ("idOrKey" = String, Path, description = "Id, key or former key of the ticket"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    responses(
        (status = 200, description = "The form", body = EditFormView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn edit_form(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id_or_key): ApiPath<String>,
) -> Result<Response, Problem> {
    let (issue, project) = visible_issue(&state, &mut tx, &id_or_key).await?;
    require_edit(&state, &mut tx, project.id).await?;
    let content = Content::of(&mut tx, project.id).await?;
    let language = Language::of(&tx, &accept);
    tx.commit().await?;
    let held: BTreeSet<u128> = stored_values(&issue.fields)
        .values()
        .flat_map(ids_in)
        .map(|id| id.as_u128())
        .collect();
    let fields = screen_fields(
        &language,
        &content,
        ScreenKind::View,
        issue.type_id.as_u128(),
        Step::Status(issue.status_id.as_u128()),
        &held,
    );
    let mut response = ApiJson(EditFormView { fields }).into_response();
    language.headers(&mut response);
    Ok(response)
}

/// Checks a move of `issue` (in `source`) to the project `project_key`:
/// `edit` on the source, `create_ticket` on the target, both active, not a
/// sub-task alone.
async fn prepare(
    state: &AppState,
    tx: &mut TenantTx,
    issue: Issue,
    source: &Node,
    project_key: &str,
) -> Result<Prepared, Problem> {
    require_edit(state, tx, source.id).await?;
    ensure_changeable(source)?;
    let target = match project_of_key(state, tx, project_key).await {
        Ok(target) => target,
        Err(problem) if problem.code == ErrorCode::NotFound => {
            return Err(Problem::validation(vec![FieldError::new(
                "projectKey",
                "not_found",
            )]));
        }
        Err(problem) => return Err(problem),
    };
    if target.id == source.id {
        return Err(Problem::validation(vec![FieldError::new(
            "projectKey",
            "same_project",
        )]));
    }
    require_create(state, tx, target.id).await?;
    ensure_changeable(&target)?;
    match infra::tickets::moves::prepare(tx, issue, source.id, target).await {
        Ok(prepared) => Ok(prepared),
        Err(PrepareError::Subtask) => Err(Problem::new(ErrorCode::IssueSubtaskMoves)),
        Err(PrepareError::Content(e)) => Err(e.into()),
    }
}

pub(crate) fn mappings(body: &[MappingBody]) -> BTreeMap<u128, u128> {
    body.iter()
        .map(|m| (m.from_id.as_u128(), m.to_id.as_u128()))
        .collect()
}

/// `POST /api/v1/issues/{idOrKey}/move-plan`: changes nothing.
#[utoipa::path(
    post,
    path = "/issues/{idOrKey}/move-plan",
    tag = TAG,
    operation_id = "planIssueMove",
    summary = "What moving a ticket to a project takes",
    description = "Members with `edit` on its project and `create_ticket` on the target. The \
                   tickets that move (its sub-tasks follow it), the types and statuses to map with \
                   a suggestion each, and, with the mappings given (else the suggestions), the \
                   fields whose values are dropped and what the move would refuse. Changes nothing.",
    params(
        ("idOrKey" = String, Path, description = "Id, key or former key of the ticket"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    request_body = MoveBody,
    responses(
        (status = 200, description = "The plan", body = MovePlanView),
        (status = 400, description = "`validation_failed`: `projectKey` `not_found` or `same_project`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`node.archived`, or `issue.subtask_moves_with_parent`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn move_plan(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id_or_key): ApiPath<String>,
    ApiJson(body): ApiJson<MoveBody>,
) -> Result<Response, Problem> {
    let (issue, source) = visible_issue(&state, &mut tx, &id_or_key).await?;
    let parent = match issue.parent_id {
        Some(parent) => repo::find(&mut tx, parent).await?,
        None => None,
    };
    let prepared = prepare(&state, &mut tx, issue, &source, &body.project_key).await?;
    let language = Language::of(&tx, &accept);
    tx.commit().await?;

    let mut types = prepared.suggested_types();
    types.extend(mappings(&body.type_mappings));
    let type_needs: Vec<TypeNeedView> = prepared
        .source_types_needed()
        .into_iter()
        .map(|t| TypeNeedView {
            id: t.item_id,
            key: t.key.clone(),
            label: language.label(&t.label_translations),
            is_subtask: t.is_subtask,
            suggested_id: prepared
                .suggested_types()
                .get(&t.item_id.as_u128())
                .map(|id| Uuid::from_u128(*id)),
        })
        .collect();
    let needed_statuses = prepared.statuses_needed(&types);
    let mut statuses: BTreeMap<u128, u128> = needed_statuses
        .iter()
        .filter_map(|(s, _)| Some((s.item_id.as_u128(), prepared.suggested_status(s, &types)?)))
        .collect();
    statuses.extend(mappings(&body.status_mappings));
    let status_needs: Vec<StatusNeedView> = needed_statuses
        .iter()
        .map(|(s, type_ids)| StatusNeedView {
            id: s.item_id,
            key: s.key.clone(),
            label: language.label(&s.label_translations),
            category: crate::workflows::CategoryView::of(&s.category),
            type_ids: type_ids.clone(),
            suggested_id: prepared.suggested_status(s, &types).map(Uuid::from_u128),
        })
        .collect();
    let (dropped, problems) = match prepared.landings(&types, &statuses) {
        Ok(landings) => (prepared.kept(&landings).1, Vec::new()),
        Err(errors) => (Vec::new(), errors),
    };
    let field_label = |id: &Uuid| {
        prepared
            .source_content
            .fields
            .iter()
            .find(|f| f.item_id == *id)
            .and_then(|f| language.label(&f.label_translations))
    };
    let target_content = &prepared.target_content;
    let view = MovePlanView {
        project: project_view(&prepared.target),
        same_configuration: prepared.same_configuration,
        tickets: prepared.moving.iter().map(issue_ref).collect(),
        parent_cleared: parent.as_ref().map(issue_ref),
        targets: target_content
            .types
            .iter()
            .map(|t| TypeTargetView {
                id: t.item_id,
                label: language.label(&t.label_translations),
                is_subtask: t.is_subtask,
                parent_ids: t.parent_ids.clone(),
                status_ids: t
                    .workflow_id
                    .map(|w| {
                        target_content
                            .workflows
                            .statuses_of(w)
                            .map(|s| s.item_id)
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect(),
        status_targets: target_content
            .workflows
            .statuses
            .iter()
            .map(|s| StatusTargetView {
                id: s.item_id,
                label: language.label(&s.label_translations),
                category: crate::workflows::CategoryView::of(&s.category),
            })
            .collect(),
        types: type_needs,
        statuses: status_needs,
        dropped_fields: dropped
            .iter()
            .map(|id| DroppedFieldView {
                field_id: *id,
                label: field_label(id),
            })
            .collect(),
        problems: problems
            .into_iter()
            .map(|e| ProblemView {
                field: e.field,
                code: e.code,
            })
            .collect(),
    };
    let mut response = ApiJson(view).into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `POST /api/v1/issues/{idOrKey}/move`: requires `If-Match`.
#[utoipa::path(
    post,
    path = "/issues/{idOrKey}/move",
    tag = TAG,
    operation_id = "moveIssue",
    summary = "Move a ticket to another project",
    description = "Members with `edit` on its project and `create_ticket` on the target. The ticket \
                   and its sub-tasks get the next keys of the target project, their former keys \
                   kept as aliases; the ticket leaves the one it sat under. Types and statuses the \
                   target lacks must be mapped (see `move-plan`); values the target cannot hold \
                   are dropped. Writes `issue.moved` to the event outbox for each ticket.",
    params(
        ("idOrKey" = String, Path, description = "Id, key or former key of the ticket"),
        ("If-Match" = String, Header, description = "ETag of the version moved, or `*`"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    request_body = MoveBody,
    responses(
        (status = 200, description = "The ticket, with its new key", body = IssueView, headers(
            ("ETag" = String, description = "Version of the ticket"),
            ("Location" = String, description = "URL of the ticket, by its new key"),
        )),
        (status = 400, description = "`validation_failed`: `projectKey` `not_found` or `same_project`; `typeMappings.{id}` or `statusMappings.{id}` (`required`, `not_found`, `not_allowed`, `not_in_workflow`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`node.archived`, or `issue.subtask_moves_with_parent`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "`precondition_failed`: the ticket changed since this version", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`precondition_required`: `If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn move_issue(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath(id_or_key): ApiPath<String>,
    ApiJson(body): ApiJson<MoveBody>,
) -> Result<Response, Problem> {
    let if_match = require_if_match(&headers)?;
    let (issue, source) = visible_issue(&state, &mut tx, &id_or_key).await?;
    let issue = locked(&mut tx, issue.id, if_match.versions()).await?;
    let prepared = prepare(&state, &mut tx, issue, &source, &body.project_key).await?;
    let landings = prepared
        .landings(&mappings(&body.type_mappings), &mappings(&body.status_mappings))
        .map_err(Problem::validation)?;
    let author = tx.author();
    let account = tx.account_id();
    let moved_all =
        infra::tickets::moves::execute(&mut tx, author, account, source.id, &prepared, &landings).await?;
    let target = &prepared.target;
    let moved_root = moved_all.into_iter().next().map(|m| m.issue);
    let moved = moved_root.ok_or_else(|| Problem::new(ErrorCode::NotFound))?;
    let language = Language::of(&tx, &accept);
    let location = format!("/api/v1/issues/{}", moved.key);
    let view = issue_view(&state, &mut tx, &language, moved, target, false).await?;
    tx.commit().await?;
    let mut response = ticket_response(view, &language);
    if let Ok(location) = HeaderValue::from_str(&location) {
        response.headers_mut().insert(header::LOCATION, location);
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_tells_absent_from_null_from_a_value() {
        let body: ChangesBody = serde_json::from_value(serde_json::json!({
            "assigneeId": null,
            "startDate": "2026-12-01",
        }))
        .unwrap();
        assert_eq!(body.assignee_id, Some(None));
        assert_eq!(body.start_date, Some(Some(time::macros::date!(2026 - 12 - 01))));
        assert_eq!(body.due_date, None);
        assert_eq!(body.description, None);
        assert!(serde_json::from_value::<ChangesBody>(serde_json::json!({"type": "x"})).is_err());
    }
}
