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
use chrono::NaiveDate;
use domain::fields::{Step, Value};
use domain::issues::{
    Current, IssueChanges, Landing, Moving, Priority, TargetType, kept_values, plan_move, validate_changes,
};
use domain::migration::{StatusOption, TypeOption, suggest_status, suggest_type};
use domain::permissions::Permission;
use domain::screens::Kind as ScreenKind;
use domain::validation::FieldError;
use domain::workflows::Category;
use infra::issues::{self as repo, Issue};
use infra::nodes::Node;
use infra::uuid::Uuid;
use serde::{Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;

use crate::etag::{etag, require_if_match};
use crate::extract::{AcceptLanguage, ApiJson, ApiPath};
use crate::issue_history;
use crate::issues::{
    Content, FormFieldView, IssueRefView, IssueView, PriorityView, ProjectView, TAG, field_defs, ids_in,
    issue_ref, issue_view, project_of_key, project_view, read_fields, require_create, screen_fields,
    type_ids_of, unknown_references, visible_issue,
};
use crate::nodes::{ensure_changeable, is_tenant_admin};
use crate::permissions;
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::tenant::{AppState, TenantTx};

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
    start_date: Option<Option<NaiveDate>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, format = Date)]
    due_date: Option<Option<NaiveDate>>,
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
    from_id: Uuid,
    to_id: Uuid,
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
    id: Uuid,
    label: Option<String>,
    is_subtask: bool,
    /// The types it may sit under.
    parent_ids: Vec<Uuid>,
    /// The statuses of its workflow.
    status_ids: Vec<Uuid>,
}

/// A type of moving tickets the target configuration lacks.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueMoveTypeNeed)]
pub struct TypeNeedView {
    id: Uuid,
    key: String,
    label: Option<String>,
    is_subtask: bool,
    #[schema(required = true)]
    suggested_id: Option<Uuid>,
}

/// A status of the target configuration.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueMoveStatusTarget)]
pub struct StatusTargetView {
    id: Uuid,
    label: Option<String>,
    category: crate::workflows::CategoryView,
}

/// A status of moving tickets that is not in the workflow of their new
/// type, with the suggested types.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueMoveStatusNeed)]
pub struct StatusNeedView {
    id: Uuid,
    key: String,
    label: Option<String>,
    category: crate::workflows::CategoryView,
    /// The types (of the ticket's configuration) of the tickets in it: the
    /// status lands in the workflow of their new type.
    type_ids: Vec<Uuid>,
    #[schema(required = true)]
    suggested_id: Option<Uuid>,
}

/// A custom field whose values are dropped by the move.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueMoveDroppedField)]
pub struct DroppedFieldView {
    field_id: Uuid,
    label: Option<String>,
}

/// A mapping problem, as the move would report it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueMoveProblem)]
pub struct ProblemView {
    /// `typeMappings.{id}` or `statusMappings.{id}`.
    field: String,
    /// `required`, `not_found`, `not_allowed`, `not_in_workflow`.
    code: &'static str,
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

/// The custom field values stored on a ticket, by field.
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

/// A rich text document as stored: read through the allow-list and
/// checked; `None` when empty.
fn read_description(json: &serde_json::Value) -> Result<Option<serde_json::Value>, FieldError> {
    let doc = infra::rich_text::parse(json).map_err(|_| FieldError::new("description", "invalid"))?;
    doc.validate()
        .map_err(|refusal| FieldError::new("description", refusal.code()))?;
    Ok((!doc.is_empty()).then(|| infra::rich_text::to_json(&doc)))
}

/// The response of a ticket: its view, `ETag`, language.
pub(crate) fn ticket_response(view: IssueView, language: &Language) -> Response {
    let version = view.version();
    let mut response = ApiJson(view).into_response();
    response.headers_mut().insert(header::ETAG, etag(version));
    language.headers(&mut response);
    response
}

/// One changed value, for the outbox.
fn change(field: &str, old: serde_json::Value, new: serde_json::Value) -> Option<serde_json::Value> {
    if old == new {
        return None;
    }
    let mut change = serde_json::Map::new();
    change.insert("field".to_owned(), field.into());
    change.insert("old".to_owned(), old);
    change.insert("new".to_owned(), new);
    Some(serde_json::Value::Object(change))
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

    let mut errors = Vec::new();
    let changed = read_fields(&body.fields).unwrap_or_else(|e| {
        errors.extend(e);
        BTreeMap::new()
    });
    let description = match &body.description {
        None => issue.description.clone(),
        Some(None) => None,
        Some(Some(json)) => read_description(json).unwrap_or_else(|e| {
            errors.push(e);
            None
        }),
    };
    let assignee = body.assignee_id.unwrap_or(issue.assignee_id);
    if let Some(Some(member)) = body.assignee_id
        && infra::rule_references::members(&mut tx, &[member])
            .await?
            .is_empty()
    {
        errors.push(FieldError::new("assigneeId", "unknown_member"));
    }
    let start_date = body.start_date.unwrap_or(issue.start_date);
    let due_date = body.due_date.unwrap_or(issue.due_date);
    let dates_in_order = match (start_date, due_date) {
        (Some(start), Some(due)) => start <= due,
        _ => true,
    };
    let current_values = stored_values(&issue.fields);
    let type_ids = type_ids_of(&content.fields);
    let defs = field_defs(&content.fields, &type_ids);
    let changes = IssueChanges {
        summary: body.summary.as_deref(),
        dates_in_order,
        fields: &changed,
    };
    let current = Current {
        type_id: issue.type_id.as_u128(),
        status_id: issue.status_id.as_u128(),
        fields: &current_values,
    };
    if let Err(e) = validate_changes(&changes, &current, &defs) {
        errors.extend(e);
    }
    let unknown = unknown_references(&mut tx, &content.fields, &changed, &errors).await?;
    errors.extend(unknown);
    if !errors.is_empty() {
        return Err(Problem::validation(errors));
    }

    // The values once changed, each a field given replacing or clearing it.
    let mut stored = issue.fields.as_object().cloned().unwrap_or_default();
    for (id, json) in &body.fields {
        if changed.get(&id.as_u128()).is_some_and(Value::is_empty) {
            stored.remove(&id.to_string());
        } else {
            stored.insert(id.to_string(), json.clone());
        }
    }
    let summary = body.summary.as_deref().map_or(issue.summary.as_str(), str::trim);
    let priority = body
        .priority
        .map_or(issue.priority.clone(), |p| Priority::from(p).as_str().to_owned());
    let mut diff: Vec<serde_json::Value> = [
        change("summary", issue.summary.clone().into(), summary.into()),
        change(
            "description",
            issue.description.clone().unwrap_or_default(),
            description.clone().unwrap_or_default(),
        ),
        change("priority", issue.priority.clone().into(), priority.clone().into()),
        change(
            "assigneeId",
            serde_json::json!(issue.assignee_id),
            serde_json::json!(assignee),
        ),
        change(
            "startDate",
            serde_json::json!(issue.start_date),
            serde_json::json!(start_date),
        ),
        change(
            "dueDate",
            serde_json::json!(issue.due_date),
            serde_json::json!(due_date),
        ),
    ]
    .into_iter()
    .flatten()
    .collect();
    for id in body.fields.keys() {
        let field = id.to_string();
        let old = issue.fields.get(&field).cloned().unwrap_or_default();
        let new = stored.get(&field).cloned().unwrap_or_default();
        diff.extend(change(&format!("fields.{id}"), old, new));
    }

    let language = Language::of(&tx, &accept);
    if diff.is_empty() {
        let view = issue_view(&state, &mut tx, &language, issue, &project, false).await?;
        tx.commit().await?;
        return Ok(ticket_response(view, &language));
    }
    let stored = serde_json::Value::Object(stored);
    let updated = repo::update(
        &mut tx,
        issue.id,
        &repo::Edited {
            summary,
            description: description.as_ref(),
            priority: &priority,
            assignee_id: assignee,
            start_date,
            due_date,
            fields: &stored,
        },
    )
    .await?;
    let entries = issue_history::changes(
        &issue_history::snapshot(&issue, &content),
        &issue_history::snapshot(&updated, &content),
    );
    issue_history::record(&mut tx, updated.id, &entries).await?;
    // A new assignee follows the ticket.
    if let Some(assignee) = updated.assignee_id.filter(|a| Some(*a) != issue.assignee_id) {
        infra::watchers::watch(&mut tx, updated.id, assignee).await?;
    }
    let actor = tx.account_id();
    infra::outbox::record(
        &mut tx,
        &infra::outbox::Event {
            kind: "issue.updated",
            aggregate_type: "issue",
            aggregate_id: updated.id,
            payload: serde_json::json!({"key": updated.key, "projectId": project.id, "changes": diff}),
            actor_id: Some(actor),
        },
    )
    .await?;
    tracing::info!(target: "audit", actor = %actor, issue = %updated.id, key = %updated.key, changes = diff.len(), "ticket edited");
    let view = issue_view(&state, &mut tx, &language, updated, &project, false).await?;
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

/// A move, checked: the tickets that move and where to.
struct Prepared {
    target: Node,
    source_content: Content,
    target_content: Content,
    same_configuration: bool,
    /// The ticket asked for, then its sub-tasks.
    moving: Vec<Issue>,
}

impl Prepared {
    fn target_types(&self) -> Vec<TargetType> {
        self.target_content
            .types
            .iter()
            .map(|t| TargetType {
                node: t.node(),
                statuses: t
                    .workflow_id
                    .map(|w| {
                        self.target_content
                            .workflows
                            .statuses_of(w)
                            .map(|s| s.item_id.as_u128())
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect()
    }

    /// The type each type the target lacks is suggested to become.
    fn suggested_types(&self) -> BTreeMap<u128, u128> {
        let options: Vec<TypeOption<'_>> = self
            .target_content
            .types
            .iter()
            .map(|t| TypeOption {
                id: t.item_id.as_u128(),
                key: &t.key,
                is_subtask: t.is_subtask,
            })
            .collect();
        self.source_types_needed()
            .into_iter()
            .filter_map(|t| Some((t.item_id.as_u128(), suggest_type(&t.key, t.is_subtask, &options)?)))
            .collect()
    }

    /// The types of moving tickets the target configuration lacks.
    fn source_types_needed(&self) -> Vec<&infra::issue_types::IssueType> {
        let mut seen = BTreeSet::new();
        self.moving
            .iter()
            .filter(|i| seen.insert(i.type_id))
            .filter(|i| !self.target_content.types.iter().any(|t| t.item_id == i.type_id))
            .filter_map(|i| self.source_content.types.iter().find(|t| t.item_id == i.type_id))
            .collect()
    }

    /// The new type of each moving ticket with `types`, if it has one.
    fn landing_type(
        &self,
        issue: &Issue,
        types: &BTreeMap<u128, u128>,
    ) -> Option<&infra::issue_types::IssueType> {
        let id = types
            .get(&issue.type_id.as_u128())
            .copied()
            .unwrap_or(issue.type_id.as_u128());
        self.target_content
            .types
            .iter()
            .find(|t| t.item_id.as_u128() == id)
    }

    /// The statuses of moving tickets not in the workflow of their new type
    /// (with `types`), with the source types of the tickets in them.
    fn statuses_needed(&self, types: &BTreeMap<u128, u128>) -> Vec<(&infra::workflows::Status, Vec<Uuid>)> {
        let mut needed: Vec<(&infra::workflows::Status, Vec<Uuid>)> = Vec::new();
        for issue in &self.moving {
            let in_workflow = self
                .landing_type(issue, types)
                .and_then(|t| t.workflow_id)
                .is_some_and(|w| {
                    self.target_content
                        .workflows
                        .statuses_of(w)
                        .any(|s| s.item_id == issue.status_id)
                });
            if in_workflow {
                continue;
            }
            let Some(status) = self
                .source_content
                .workflows
                .statuses
                .iter()
                .find(|s| s.item_id == issue.status_id)
            else {
                continue;
            };
            match needed.iter_mut().find(|(s, _)| s.item_id == status.item_id) {
                Some((_, type_ids)) if !type_ids.contains(&issue.type_id) => type_ids.push(issue.type_id),
                Some(_) => {}
                None => needed.push((status, vec![issue.type_id])),
            }
        }
        needed
    }

    /// The status suggested for `status`, among those of the workflows of
    /// the new types of its tickets.
    fn suggested_status(
        &self,
        status: &infra::workflows::Status,
        types: &BTreeMap<u128, u128>,
    ) -> Option<u128> {
        let workflows: BTreeSet<Uuid> = self
            .moving
            .iter()
            .filter(|i| i.status_id == status.item_id)
            .filter_map(|i| self.landing_type(i, types)?.workflow_id)
            .collect();
        let options: Vec<StatusOption<'_>> = self
            .target_content
            .workflows
            .statuses
            .iter()
            .filter(|s| workflows.contains(&s.workflow_id))
            .map(|s| StatusOption {
                id: s.item_id.as_u128(),
                key: &s.key,
                category: Category::parse(&s.category).unwrap_or(Category::Todo),
            })
            .collect();
        suggest_status(
            &status.key,
            Category::parse(&status.category).unwrap_or(Category::Todo),
            &options,
        )
    }

    /// Where the moving tickets land with `types` and `statuses`, or what
    /// the move refuses.
    fn landings(
        &self,
        types: &BTreeMap<u128, u128>,
        statuses: &BTreeMap<u128, u128>,
    ) -> Result<Vec<Landing>, Vec<FieldError>> {
        let moving: Vec<Moving> = self
            .moving
            .iter()
            .map(|i| Moving {
                id: i.id.as_u128(),
                type_id: i.type_id.as_u128(),
                status_id: i.status_id.as_u128(),
            })
            .collect();
        plan_move(&moving, &self.target_types(), types, statuses)
    }

    /// The values each moving ticket keeps at its landing, and the fields
    /// dropped (any ticket), in the order of the source configuration.
    fn kept(&self, landings: &[Landing]) -> (Vec<serde_json::Value>, Vec<Uuid>) {
        let type_ids = type_ids_of(&self.target_content.fields);
        let defs = field_defs(&self.target_content.fields, &type_ids);
        let mut dropped = BTreeSet::new();
        let kept = self
            .moving
            .iter()
            .zip(landings)
            .map(|(issue, landing)| {
                let values = stored_values(&issue.fields);
                let (_, gone) = kept_values(&values, landing.type_id, &defs);
                dropped.extend(gone.iter().copied());
                let mut object = issue.fields.as_object().cloned().unwrap_or_default();
                object.retain(|id, _| Uuid::parse_str(id).is_ok_and(|id| !gone.contains(&id.as_u128())));
                serde_json::Value::Object(object)
            })
            .collect();
        let order = self
            .source_content
            .fields
            .iter()
            .map(|f| f.item_id)
            .filter(|id| dropped.contains(&id.as_u128()));
        (kept, order.collect())
    }
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
    let source_content = Content::of(tx, source.id).await?;
    if source_content
        .types
        .iter()
        .any(|t| t.item_id == issue.type_id && t.is_subtask)
    {
        return Err(Problem::new(ErrorCode::IssueSubtaskMoves));
    }
    let target_content = Content::of(tx, target.id).await?;
    let same_configuration =
        infra::configs::of_project(tx, source.id).await? == infra::configs::of_project(tx, target.id).await?;
    let children = repo::children(tx, issue.id).await?;
    let mut moving = vec![issue];
    moving.extend(children);
    Ok(Prepared {
        target,
        source_content,
        target_content,
        same_configuration,
        moving,
    })
}

fn mappings(body: &[MappingBody]) -> BTreeMap<u128, u128> {
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
    let (kept, dropped) = prepared.kept(&landings);
    let target = &prepared.target;
    let target_key = target.key.as_str();
    let actor = tx.account_id();
    let mut moved_root = None;
    for (index, ((issue, landing), fields)) in prepared.moving.iter().zip(&landings).zip(&kept).enumerate() {
        // The ticket asked for leaves the one it sat under; its sub-tasks stay under it.
        let parent_id = if index == 0 { None } else { issue.parent_id };
        let moved = repo::move_to(
            &mut tx,
            &repo::Move {
                id: issue.id,
                project_id: target.id,
                project_key: target_key,
                type_id: Uuid::from_u128(landing.type_id),
                status_id: Uuid::from_u128(landing.status_id),
                parent_id,
                fields,
            },
        )
        .await?;
        let mut entries = vec![issue_history::moved(&issue.key, &moved.key)];
        entries.extend(issue_history::changes(
            &issue_history::snapshot(issue, &prepared.source_content),
            &issue_history::snapshot(&moved, &prepared.target_content),
        ));
        issue_history::record(&mut tx, moved.id, &entries).await?;
        infra::outbox::record(
            &mut tx,
            &infra::outbox::Event {
                kind: "issue.moved",
                aggregate_type: "issue",
                aggregate_id: moved.id,
                payload: serde_json::json!({
                    "key": moved.key,
                    "formerKey": issue.key,
                    "projectId": target.id,
                    "formerProjectId": source.id,
                    "typeId": moved.type_id,
                    "statusId": moved.status_id,
                    "droppedFieldIds": dropped,
                }),
                actor_id: Some(actor),
            },
        )
        .await?;
        tracing::info!(target: "audit", actor = %actor, issue = %moved.id, from = %issue.key, to = %moved.key, "ticket moved");
        if index == 0 {
            moved_root = Some(moved);
        }
    }
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
        assert_eq!(body.start_date, Some(NaiveDate::from_ymd_opt(2026, 12, 1)));
        assert_eq!(body.due_date, None);
        assert_eq!(body.description, None);
        assert!(serde_json::from_value::<ChangesBody>(serde_json::json!({"type": "x"})).is_err());
    }

    #[test]
    fn only_changed_values_are_reported() {
        assert_eq!(change("summary", "a".into(), "a".into()), None);
        assert_eq!(
            change("priority", "medium".into(), "high".into()),
            Some(serde_json::json!({"field": "priority", "old": "medium", "new": "high"}))
        );
    }
}
