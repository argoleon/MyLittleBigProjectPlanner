//! Workflows of a project configuration (specification §5, L04-US04):
//! their statuses, each in a category, and the transitions between them.
//! Issue types follow them (`workflowId` of a type).
//!
//! Workflows are read from the draft of the configuration if it has one,
//! else from its published version. Every change goes to the draft, opened
//! on the first one, and counts once per workflow in its changes. A draft
//! may hold an incomplete workflow: each one lists its `problems`, and
//! publishing refuses them (`workflow.invalid`).

use std::collections::BTreeMap;

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use domain::configuration::{Item, ItemKind};
use domain::i18n::validate_translations;
use domain::issue_types::is_key;
use domain::validation::{FieldError, Violations};
use domain::workflows::{
    Category, Problem as WorkflowProblem, RemovalRefusal, WORKFLOW_LABEL_MAX_CHARS, problems, removal_refusal,
};
use infra::configs as configs_repo;
use infra::uuid::Uuid;
use infra::workflows::{self as repo, StatusFields, TransitionFields, Workflows, WriteConflict};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::configs::{TAG, require_config_admin};
use crate::etag::require_if_match;
use crate::extract::{AcceptLanguage, ApiJson, ApiPath};
use crate::idempotency::{self, Start};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::tenant::TenantTx;
use crate::transition_rules::{ReferencesView, RulesBody, checked, references};

type Labels = BTreeMap<String, String>;

/// The category of a status: what aggregates, boards and burndown read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = WorkflowCategory)]
pub enum CategoryView {
    Todo,
    InProgress,
    Done,
}

impl From<CategoryView> for Category {
    fn from(category: CategoryView) -> Self {
        match category {
            CategoryView::Todo => Self::Todo,
            CategoryView::InProgress => Self::InProgress,
            CategoryView::Done => Self::Done,
        }
    }
}

impl CategoryView {
    /// The stored category; the database only holds these three.
    fn of(raw: &str) -> Self {
        match Category::parse(raw) {
            Some(Category::Todo) | None => Self::Todo,
            Some(Category::InProgress) => Self::InProgress,
            Some(Category::Done) => Self::Done,
        }
    }
}

/// A status of a workflow.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = WorkflowStatus)]
pub struct StatusView {
    /// Its identity, the same in every version of the configuration.
    id: Uuid,
    #[schema(example = "in_review")]
    key: String,
    #[schema(example = "In review")]
    label: Option<String>,
    label_translations: Labels,
    /// What requesters see on the portal; `null`: the label.
    #[schema(example = "Being processed")]
    portal_label: Option<String>,
    portal_label_translations: Labels,
    category: CategoryView,
    /// The status of a new ticket.
    is_initial: bool,
    #[schema(example = 1)]
    version: i64,
}

/// A transition of a workflow.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = WorkflowTransition)]
pub struct TransitionView {
    id: Uuid,
    #[schema(example = "Start")]
    name: Option<String>,
    name_translations: Labels,
    /// The status it leaves; `null`: from any status.
    #[schema(required = true)]
    from_id: Option<Uuid>,
    to_id: Uuid,
    /// Who may take it, the fields it requires, what it does.
    rules: RulesBody,
    #[schema(example = 1)]
    version: i64,
}

/// What keeps a workflow from being published.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = WorkflowProblem)]
pub struct ProblemView {
    /// `no_status`, `no_initial`, `unreachable` or `duplicate_transition`.
    #[schema(example = "unreachable")]
    code: String,
    /// The status nothing leads to (`unreachable`).
    #[schema(required = true)]
    status_id: Option<Uuid>,
    /// The transition repeating another (`duplicate_transition`).
    #[schema(required = true)]
    transition_id: Option<Uuid>,
}

/// A workflow with its statuses and transitions.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Workflow)]
pub struct WorkflowView {
    /// Its identity, the same in every version of the configuration.
    id: Uuid,
    #[schema(example = "standard")]
    key: String,
    #[schema(example = "Standard workflow")]
    label: Option<String>,
    label_translations: Labels,
    /// The issue types that follow it.
    issue_type_ids: Vec<Uuid>,
    statuses: Vec<StatusView>,
    transitions: Vec<TransitionView>,
    /// None: it can be published.
    problems: Vec<ProblemView>,
    #[schema(example = 1)]
    version: i64,
}

/// Where the workflows were read.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = WorkflowStage)]
pub enum StageView {
    Draft,
    Published,
}

/// The workflows of a configuration, in their order.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = WorkflowList)]
pub struct WorkflowList {
    stage: StageView,
    items: Vec<WorkflowView>,
    /// The roles, groups and members the rules of the transitions name.
    references: ReferencesView,
}

/// A new workflow, empty or a copy of another one.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = NewWorkflow,
    example = json!({"key": "bug", "labelTranslations": {"fr": "Workflow bug", "en": "Bug workflow"}, "copyOf": null})
)]
pub struct NewWorkflowBody {
    /// A lowercase letter, then up to 39 lowercase letters, digits or
    /// underscores; unique in the configuration.
    key: String,
    /// One label per language, at least one; 60 characters at most.
    label_translations: Labels,
    /// A workflow whose statuses and transitions it starts with.
    #[serde(default)]
    copy_of: Option<Uuid>,
}

/// The label of a workflow.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = WorkflowFields)]
pub struct WorkflowBody {
    label_translations: Labels,
}

/// A new status.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = NewWorkflowStatus,
    example = json!({
        "key": "in_review",
        "labelTranslations": {"fr": "En revue", "en": "In review"},
        "portalLabelTranslations": {"fr": "En cours de traitement", "en": "Being processed"},
        "category": "in_progress",
        "isInitial": false
    })
)]
pub struct NewStatusBody {
    /// Unique in its workflow, never changed.
    key: String,
    label_translations: Labels,
    #[serde(default)]
    portal_label_translations: Labels,
    category: CategoryView,
    #[serde(default)]
    is_initial: bool,
}

/// What a status is made of; its `key` stays. Made initial, it takes the
/// place of the former initial status.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = WorkflowStatusFields)]
pub struct StatusBody {
    label_translations: Labels,
    #[serde(default)]
    portal_label_translations: Labels,
    category: CategoryView,
    #[serde(default)]
    is_initial: bool,
}

/// A transition.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = WorkflowTransitionFields,
    example = json!({"nameTranslations": {"fr": "Démarrer", "en": "Start"}, "fromId": null, "toId": "01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9a"})
)]
pub struct TransitionBody {
    name_translations: Labels,
    /// The status it leaves; `null`: from any status of the workflow.
    #[serde(default)]
    from_id: Option<Uuid>,
    to_id: Uuid,
    /// Its conditions, validators and post-functions; left out, they stay as
    /// they are (none for a new transition).
    #[serde(default)]
    rules: Option<RulesBody>,
}

fn labels_errors(v: &mut Violations, field: &str, labels: &Labels, required: bool) {
    if !required && labels.values().all(|l| l.trim().is_empty()) {
        return;
    }
    if let Err(errors) = validate_translations(field, labels, WORKFLOW_LABEL_MAX_CHARS) {
        for error in errors {
            v.add(error.field, error.code);
        }
    }
}

fn key_error(v: &mut Violations, key: &str) {
    if !is_key(key) {
        v.add("key", "format");
    }
}

impl Language {
    fn workflow(
        &self,
        all: &Workflows,
        workflow: &repo::Workflow,
        types: &[(Uuid, Option<Uuid>)],
    ) -> WorkflowView {
        let statuses: Vec<&repo::Status> = all.statuses_of(workflow.item_id).collect();
        let transitions: Vec<&repo::Transition> = all.transitions_of(workflow.item_id).collect();
        let nodes: Vec<_> = statuses.iter().map(|s| s.node()).collect();
        let edges: Vec<_> = transitions.iter().map(|t| t.edge()).collect();
        WorkflowView {
            id: workflow.item_id,
            key: workflow.key.clone(),
            label: self.label(&workflow.label_translations),
            label_translations: workflow.label_translations.clone(),
            issue_type_ids: types
                .iter()
                .filter(|(_, w)| *w == Some(workflow.item_id))
                .map(|(t, _)| *t)
                .collect(),
            statuses: statuses
                .iter()
                .map(|s| StatusView {
                    id: s.item_id,
                    key: s.key.clone(),
                    label: self.label(&s.label_translations),
                    label_translations: s.label_translations.clone(),
                    portal_label: self.label(&s.portal_label_translations),
                    portal_label_translations: s.portal_label_translations.clone(),
                    category: CategoryView::of(&s.category),
                    is_initial: s.is_initial,
                    version: s.version,
                })
                .collect(),
            transitions: transitions
                .iter()
                .map(|t| TransitionView {
                    id: t.item_id,
                    name: self.label(&t.name_translations),
                    name_translations: t.name_translations.clone(),
                    from_id: t.from_status_id,
                    to_id: t.to_status_id,
                    rules: RulesBody::of(&t.rules),
                    version: t.version,
                })
                .collect(),
            problems: problems(&nodes, &edges).into_iter().map(problem_view).collect(),
            version: workflow.version,
        }
    }
}

fn problem_view(problem: WorkflowProblem) -> ProblemView {
    let (status_id, transition_id) = match problem {
        WorkflowProblem::Unreachable(id) => (Some(Uuid::from_u128(id)), None),
        WorkflowProblem::Duplicate(id) => (None, Some(Uuid::from_u128(id))),
        WorkflowProblem::NoStatus | WorkflowProblem::NoInitial => (None, None),
    };
    ProblemView {
        code: problem.code().to_owned(),
        status_id,
        transition_id,
    }
}

/// Every problem of the workflows of a version, as field errors naming the
/// workflow and its status (`workflows.standard.statuses.in_review`); none:
/// they can be published.
pub(crate) fn publish_errors(all: &Workflows) -> Vec<FieldError> {
    let mut errors = Vec::new();
    for workflow in &all.workflows {
        let statuses: Vec<&repo::Status> = all.statuses_of(workflow.item_id).collect();
        let nodes: Vec<_> = statuses.iter().map(|s| s.node()).collect();
        let edges: Vec<_> = all
            .transitions_of(workflow.item_id)
            .map(repo::Transition::edge)
            .collect();
        for problem in problems(&nodes, &edges) {
            let field = match problem {
                WorkflowProblem::Unreachable(id) => {
                    let key = statuses
                        .iter()
                        .find(|s| s.item_id.as_u128() == id)
                        .map_or("?", |s| s.key.as_str());
                    format!("workflows.{}.statuses.{key}", workflow.key)
                }
                WorkflowProblem::Duplicate(id) => {
                    format!("workflows.{}.transitions.{}", workflow.key, Uuid::from_u128(id))
                }
                WorkflowProblem::NoStatus | WorkflowProblem::NoInitial => {
                    format!("workflows.{}", workflow.key)
                }
            };
            errors.push(FieldError::new(field, problem.code()));
        }
    }
    errors
}

fn conflict_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
        WriteConflict::KeyTaken => {
            Problem::new(ErrorCode::WorkflowKeyTaken).with_errors(vec![FieldError::new("key", "taken")])
        }
    }
}

fn removal_problem(refusal: RemovalRefusal) -> Problem {
    Problem::new(match refusal {
        RemovalRefusal::InUse => ErrorCode::WorkflowInUse,
        RemovalRefusal::Last => ErrorCode::WorkflowLast,
    })
}

fn not_found(what: &'static str) -> Problem {
    Problem::new(ErrorCode::NotFound).with_detail(what)
}

/// The draft of `config`, opened if needed: every change goes there.
pub(crate) async fn draft(tx: &mut TenantTx, config: Uuid) -> Result<Uuid, Problem> {
    let actor = tx.account_id();
    let opened = configs_repo::open_draft(tx, config, Some(actor))
        .await?
        .map_err(|_| not_found("no such configuration"))?;
    opened
        .draft
        .map(|d| d.id)
        .ok_or_else(|| Problem::new(ErrorCode::ConfigNoDraft))
}

/// Counts the change of the workflow `workflow` in the draft.
async fn changed(tx: &mut TenantTx, draft: Uuid, workflow: Uuid) -> Result<(), Problem> {
    let item = Item {
        kind: ItemKind::Workflow,
        id: workflow.as_u128(),
    };
    configs_repo::record_change(tx, draft, item).await?;
    Ok(())
}

/// The types of `version` with the workflow each follows.
async fn types_of(tx: &mut TenantTx, version: Uuid) -> Result<Vec<(Uuid, Option<Uuid>)>, Problem> {
    Ok(infra::issue_types::list(tx, version)
        .await?
        .into_iter()
        .map(|t| (t.item_id, t.workflow_id))
        .collect())
}

/// The workflow `workflow` of the draft, as the response of a change.
async fn workflow_response(
    tx: &mut TenantTx,
    language: &Language,
    draft: Uuid,
    workflow: Uuid,
) -> Result<WorkflowView, Problem> {
    let all = repo::load(tx, draft).await?;
    let types = types_of(tx, draft).await?;
    let found = all
        .workflows
        .iter()
        .find(|w| w.item_id == workflow)
        .ok_or_else(|| not_found("no such workflow"))?;
    Ok(language.workflow(&all, found, &types))
}

/// The workflow a status or a transition belongs to, in `all`.
fn owner(all: &Workflows, item: Uuid) -> Option<Uuid> {
    all.statuses
        .iter()
        .find(|s| s.item_id == item)
        .map(|s| s.workflow_id)
        .or_else(|| {
            all.transitions
                .iter()
                .find(|t| t.item_id == item)
                .map(|t| t.workflow_id)
        })
}

/// `fromId` and `toId` name statuses of `workflow`.
fn check_ends(all: &Workflows, workflow: Uuid, body: &TransitionBody) -> Result<(), Problem> {
    let mut v = Violations::new();
    let of_workflow = |id: Uuid| all.statuses_of(workflow).any(|s| s.item_id == id);
    if body.from_id.is_some_and(|from| !of_workflow(from)) {
        v.add("fromId", "not_found");
    }
    if !of_workflow(body.to_id) {
        v.add("toId", "not_found");
    }
    v.into_result().map_err(Problem::validation)
}

fn path(config: Uuid) -> String {
    format!("/api/v1/project-configs/{config}/workflows")
}

/// `GET /api/v1/project-configs/{id}/workflows`.
#[utoipa::path(
    get,
    path = "/project-configs/{id}/workflows",
    tag = TAG,
    operation_id = "listWorkflows",
    summary = "List the workflows of a configuration",
    description = "Tenant admins and configuration admins. In their order, with their statuses, \
                   transitions, the types that follow them and what keeps them from being published; \
                   from the draft if the configuration has one (`stage: draft`), else from its \
                   published version.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    responses(
        (status = 200, description = "Its workflows", body = WorkflowList),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let config = configs_repo::find(&mut tx, id)
        .await?
        .ok_or_else(|| not_found("no such configuration"))?;
    let (version, stage) = match &config.draft {
        Some(draft) => (draft.id, StageView::Draft),
        None => (config.published.id, StageView::Published),
    };
    let all = repo::load(&mut tx, version).await?;
    let types = types_of(&mut tx, version).await?;
    let language = Language::of(&tx, &accept);
    let references = references(&mut tx, &language, all.transitions.iter().map(|t| &t.rules)).await?;
    tx.commit().await?;
    let items = all
        .workflows
        .iter()
        .map(|w| language.workflow(&all, w, &types))
        .collect();
    let mut response = ApiJson(WorkflowList {
        stage,
        items,
        references,
    })
    .into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `POST /api/v1/project-configs/{id}/workflows`: idempotent with
/// `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/project-configs/{id}/workflows",
    tag = TAG,
    operation_id = "createWorkflow",
    summary = "Add a workflow to the draft of a configuration",
    description = "Tenant admins and configuration admins. Empty, or with `copyOf` a copy of the \
                   statuses and transitions of another workflow; in the draft, opened if needed.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    request_body = NewWorkflowBody,
    responses(
        (status = 201, description = "Added", body = WorkflowView, headers(
            ("Location" = String, description = "URL of the workflows of the configuration"),
        )),
        (status = 400, description = "`validation_failed`; `copyOf` `not_found`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`workflow.key_taken`, or `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<NewWorkflowBody>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let key = match idempotency::start(&mut tx, &headers, &format!("POST {}", path(id)), &body).await? {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    let mut v = Violations::new();
    key_error(&mut v, &body.key);
    labels_errors(&mut v, "labelTranslations", &body.label_translations, true);
    v.into_result().map_err(Problem::validation)?;
    let draft = draft(&mut tx, id).await?;
    let item = infra::id::new_id();
    let written = match body.copy_of {
        Some(source) => {
            let all = repo::load(&mut tx, draft).await?;
            if !all.workflows.iter().any(|w| w.item_id == source) {
                return Err(Problem::validation(vec![FieldError::new("copyOf", "not_found")]));
            }
            repo::duplicate_workflow(&mut tx, draft, source, item, &body.key, &body.label_translations)
                .await?
        }
        None => repo::insert_workflow(&mut tx, draft, item, &body.key, &body.label_translations).await?,
    };
    written.map_err(conflict_problem)?;
    changed(&mut tx, draft, item).await?;
    let language = Language::of(&tx, &accept);
    let view = workflow_response(&mut tx, &language, draft, item).await?;
    let version = view.version;
    let mut response = idempotency::created(&mut tx, key.as_deref(), &view, path(id), version).await?;
    tx.commit().await?;
    language.headers(&mut response);
    Ok(response)
}

/// `PUT /api/v1/project-configs/{id}/workflows/{workflowId}`: requires
/// `If-Match`.
#[utoipa::path(
    put,
    path = "/project-configs/{id}/workflows/{workflowId}",
    tag = TAG,
    operation_id = "updateWorkflow",
    summary = "Rename a workflow in the draft of a configuration",
    description = "Tenant admins and configuration admins. Its `key` stays.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("workflowId" = Uuid, Path, description = "Id of the workflow"),
        ("If-Match" = String, Header, description = "ETag of the workflow (its `version`), or `*`"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    request_body = WorkflowBody,
    responses(
        (status = 200, description = "The workflow", body = WorkflowView),
        (status = 400, description = "`validation_failed`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration or workflow", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The workflow changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath((id, workflow)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<WorkflowBody>,
) -> Result<ApiJson<WorkflowView>, Problem> {
    require_config_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let mut v = Violations::new();
    labels_errors(&mut v, "labelTranslations", &body.label_translations, true);
    v.into_result().map_err(Problem::validation)?;
    let draft = draft(&mut tx, id).await?;
    repo::update_workflow(
        &mut tx,
        draft,
        workflow,
        if_match.versions(),
        &body.label_translations,
    )
    .await?
    .map_err(conflict_problem)?;
    changed(&mut tx, draft, workflow).await?;
    let language = Language::of(&tx, &accept);
    let view = workflow_response(&mut tx, &language, draft, workflow).await?;
    tx.commit().await?;
    Ok(ApiJson(view))
}

/// `DELETE /api/v1/project-configs/{id}/workflows/{workflowId}`: requires
/// `If-Match`.
#[utoipa::path(
    delete,
    path = "/project-configs/{id}/workflows/{workflowId}",
    tag = TAG,
    operation_id = "deleteWorkflow",
    summary = "Remove a workflow from the draft of a configuration",
    description = "Tenant admins and configuration admins. With its statuses and transitions; refused \
                   while issue types follow it (`workflow.in_use`) and for the last one \
                   (`workflow.last`).",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("workflowId" = Uuid, Path, description = "Id of the workflow"),
        ("If-Match" = String, Header, description = "ETag of the workflow, or `*`"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration or workflow", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`workflow.in_use` or `workflow.last`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The workflow changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath((id, workflow)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, Problem> {
    require_config_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let draft = draft(&mut tx, id).await?;
    let all = repo::load(&mut tx, draft).await?;
    if !all.workflows.iter().any(|w| w.item_id == workflow) {
        return Err(not_found("no such workflow"));
    }
    let followers = types_of(&mut tx, draft)
        .await?
        .into_iter()
        .filter(|(_, w)| *w == Some(workflow))
        .count();
    if let Some(refusal) = removal_refusal(all.workflows.len(), followers) {
        return Err(removal_problem(refusal));
    }
    repo::delete_workflow(&mut tx, draft, workflow, if_match.versions())
        .await?
        .map_err(conflict_problem)?;
    changed(&mut tx, draft, workflow).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/project-configs/{id}/workflows/{workflowId}/statuses`:
/// idempotent with `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/project-configs/{id}/workflows/{workflowId}/statuses",
    tag = TAG,
    operation_id = "createWorkflowStatus",
    summary = "Add a status to a workflow in the draft of a configuration",
    description = "Tenant admins and configuration admins. It goes last; made initial, it takes the \
                   place of the former initial status.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("workflowId" = Uuid, Path, description = "Id of the workflow"),
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    request_body = NewStatusBody,
    responses(
        (status = 201, description = "The workflow, with its new status", body = WorkflowView, headers(
            ("Location" = String, description = "URL of the workflows of the configuration"),
        )),
        (status = 400, description = "`validation_failed`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration or workflow", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`workflow.key_taken` (in this workflow), or `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create_status(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath((id, workflow)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<NewStatusBody>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let request = format!("POST {}/{workflow}/statuses", path(id));
    let key = match idempotency::start(&mut tx, &headers, &request, &body).await? {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    let mut v = Violations::new();
    key_error(&mut v, &body.key);
    labels_errors(&mut v, "labelTranslations", &body.label_translations, true);
    labels_errors(
        &mut v,
        "portalLabelTranslations",
        &body.portal_label_translations,
        false,
    );
    v.into_result().map_err(Problem::validation)?;
    let draft = draft(&mut tx, id).await?;
    if !repo::load(&mut tx, draft)
        .await?
        .workflows
        .iter()
        .any(|w| w.item_id == workflow)
    {
        return Err(not_found("no such workflow"));
    }
    let fields = StatusFields {
        label_translations: &body.label_translations,
        portal_label_translations: &body.portal_label_translations,
        category: Category::from(body.category).as_str(),
        is_initial: body.is_initial,
    };
    repo::insert_status(&mut tx, draft, workflow, infra::id::new_id(), &body.key, &fields)
        .await?
        .map_err(conflict_problem)?;
    changed(&mut tx, draft, workflow).await?;
    let language = Language::of(&tx, &accept);
    let view = workflow_response(&mut tx, &language, draft, workflow).await?;
    let version = view.version;
    let mut response = idempotency::created(&mut tx, key.as_deref(), &view, path(id), version).await?;
    tx.commit().await?;
    language.headers(&mut response);
    Ok(response)
}

/// `PUT /api/v1/project-configs/{id}/workflow-statuses/{statusId}`:
/// requires `If-Match`.
#[utoipa::path(
    put,
    path = "/project-configs/{id}/workflow-statuses/{statusId}",
    tag = TAG,
    operation_id = "updateWorkflowStatus",
    summary = "Change a status in the draft of a configuration",
    description = "Tenant admins and configuration admins. Its `key` stays; made initial, it takes the \
                   place of the former initial status.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("statusId" = Uuid, Path, description = "Id of the status"),
        ("If-Match" = String, Header, description = "ETag of the status (its `version`), or `*`"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    request_body = StatusBody,
    responses(
        (status = 200, description = "Its workflow", body = WorkflowView),
        (status = 400, description = "`validation_failed`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration or status", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The status changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update_status(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath((id, status)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<StatusBody>,
) -> Result<ApiJson<WorkflowView>, Problem> {
    require_config_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let mut v = Violations::new();
    labels_errors(&mut v, "labelTranslations", &body.label_translations, true);
    labels_errors(
        &mut v,
        "portalLabelTranslations",
        &body.portal_label_translations,
        false,
    );
    v.into_result().map_err(Problem::validation)?;
    let draft = draft(&mut tx, id).await?;
    let workflow =
        owner(&repo::load(&mut tx, draft).await?, status).ok_or_else(|| not_found("no such status"))?;
    let fields = StatusFields {
        label_translations: &body.label_translations,
        portal_label_translations: &body.portal_label_translations,
        category: Category::from(body.category).as_str(),
        is_initial: body.is_initial,
    };
    repo::update_status(&mut tx, draft, status, if_match.versions(), &fields)
        .await?
        .map_err(conflict_problem)?;
    changed(&mut tx, draft, workflow).await?;
    let language = Language::of(&tx, &accept);
    let view = workflow_response(&mut tx, &language, draft, workflow).await?;
    tx.commit().await?;
    Ok(ApiJson(view))
}

/// `DELETE /api/v1/project-configs/{id}/workflow-statuses/{statusId}`:
/// requires `If-Match`.
#[utoipa::path(
    delete,
    path = "/project-configs/{id}/workflow-statuses/{statusId}",
    tag = TAG,
    operation_id = "deleteWorkflowStatus",
    summary = "Remove a status from the draft of a configuration",
    description = "Tenant admins and configuration admins. With the transitions from and to it. If \
                   tickets are in it, publishing needs them moved first (`needsMigration` of the \
                   publish impact).",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("statusId" = Uuid, Path, description = "Id of the status"),
        ("If-Match" = String, Header, description = "ETag of the status, or `*`"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration or status", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The status changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete_status(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath((id, status)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, Problem> {
    require_config_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let draft = draft(&mut tx, id).await?;
    let workflow =
        owner(&repo::load(&mut tx, draft).await?, status).ok_or_else(|| not_found("no such status"))?;
    repo::delete_status(&mut tx, draft, status, if_match.versions())
        .await?
        .map_err(conflict_problem)?;
    changed(&mut tx, draft, workflow).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/project-configs/{id}/workflows/{workflowId}/transitions`:
/// idempotent with `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/project-configs/{id}/workflows/{workflowId}/transitions",
    tag = TAG,
    operation_id = "createWorkflowTransition",
    summary = "Add a transition to a workflow in the draft of a configuration",
    description = "Tenant admins and configuration admins. From a status of the workflow, or from any \
                   (`fromId` null), to one of its statuses.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("workflowId" = Uuid, Path, description = "Id of the workflow"),
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    request_body = TransitionBody,
    responses(
        (status = 201, description = "The workflow, with its new transition", body = WorkflowView, headers(
            ("Location" = String, description = "URL of the workflows of the configuration"),
        )),
        (status = 400, description = "`validation_failed`; `fromId` or `toId` `not_found` (not a status of the workflow)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration or workflow", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create_transition(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath((id, workflow)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<TransitionBody>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let request = format!("POST {}/{workflow}/transitions", path(id));
    let key = match idempotency::start(&mut tx, &headers, &request, &body).await? {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    let mut v = Violations::new();
    labels_errors(&mut v, "nameTranslations", &body.name_translations, true);
    v.into_result().map_err(Problem::validation)?;
    let draft = draft(&mut tx, id).await?;
    let all = repo::load(&mut tx, draft).await?;
    if !all.workflows.iter().any(|w| w.item_id == workflow) {
        return Err(not_found("no such workflow"));
    }
    check_ends(&all, workflow, &body)?;
    let rules = match &body.rules {
        Some(rules) => Some(checked(&mut tx, draft, rules).await?),
        None => None,
    };
    let fields = TransitionFields {
        name_translations: &body.name_translations,
        from_status_id: body.from_id,
        to_status_id: body.to_id,
        rules: rules.as_ref(),
    };
    repo::insert_transition(&mut tx, draft, workflow, infra::id::new_id(), &fields).await?;
    changed(&mut tx, draft, workflow).await?;
    let language = Language::of(&tx, &accept);
    let view = workflow_response(&mut tx, &language, draft, workflow).await?;
    let version = view.version;
    let mut response = idempotency::created(&mut tx, key.as_deref(), &view, path(id), version).await?;
    tx.commit().await?;
    language.headers(&mut response);
    Ok(response)
}

/// `PUT /api/v1/project-configs/{id}/workflow-transitions/{transitionId}`:
/// requires `If-Match`.
#[utoipa::path(
    put,
    path = "/project-configs/{id}/workflow-transitions/{transitionId}",
    tag = TAG,
    operation_id = "updateWorkflowTransition",
    summary = "Change a transition in the draft of a configuration",
    description = "Tenant admins and configuration admins.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("transitionId" = Uuid, Path, description = "Id of the transition"),
        ("If-Match" = String, Header, description = "ETag of the transition (its `version`), or `*`"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    request_body = TransitionBody,
    responses(
        (status = 200, description = "Its workflow", body = WorkflowView),
        (status = 400, description = "`validation_failed`; `fromId` or `toId` `not_found`; `rules.…` `not_found`, `duplicate`, `required`, `not_applicable`, `unknown_member` or a value code", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration or transition", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The transition changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update_transition(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath((id, transition)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<TransitionBody>,
) -> Result<ApiJson<WorkflowView>, Problem> {
    require_config_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let mut v = Violations::new();
    labels_errors(&mut v, "nameTranslations", &body.name_translations, true);
    v.into_result().map_err(Problem::validation)?;
    let draft = draft(&mut tx, id).await?;
    let all = repo::load(&mut tx, draft).await?;
    let workflow = owner(&all, transition).ok_or_else(|| not_found("no such transition"))?;
    check_ends(&all, workflow, &body)?;
    let rules = match &body.rules {
        Some(rules) => Some(checked(&mut tx, draft, rules).await?),
        None => None,
    };
    let fields = TransitionFields {
        name_translations: &body.name_translations,
        from_status_id: body.from_id,
        to_status_id: body.to_id,
        rules: rules.as_ref(),
    };
    repo::update_transition(&mut tx, draft, transition, if_match.versions(), &fields)
        .await?
        .map_err(conflict_problem)?;
    changed(&mut tx, draft, workflow).await?;
    let language = Language::of(&tx, &accept);
    let view = workflow_response(&mut tx, &language, draft, workflow).await?;
    tx.commit().await?;
    Ok(ApiJson(view))
}

/// `DELETE /api/v1/project-configs/{id}/workflow-transitions/{transitionId}`:
/// requires `If-Match`.
#[utoipa::path(
    delete,
    path = "/project-configs/{id}/workflow-transitions/{transitionId}",
    tag = TAG,
    operation_id = "deleteWorkflowTransition",
    summary = "Remove a transition from the draft of a configuration",
    description = "Tenant admins and configuration admins.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("transitionId" = Uuid, Path, description = "Id of the transition"),
        ("If-Match" = String, Header, description = "ETag of the transition, or `*`"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration or transition", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The transition changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete_transition(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath((id, transition)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, Problem> {
    require_config_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let draft = draft(&mut tx, id).await?;
    let workflow = owner(&repo::load(&mut tx, draft).await?, transition)
        .ok_or_else(|| not_found("no such transition"))?;
    repo::delete_transition(&mut tx, draft, transition, if_match.versions())
        .await?
        .map_err(conflict_problem)?;
    changed(&mut tx, draft, workflow).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(problem: &Problem) -> (u16, &'static str) {
        (problem.code.status().as_u16(), problem.code.as_str())
    }

    #[test]
    fn refusals_and_conflicts_have_their_own_codes() {
        assert_eq!(
            code(&removal_problem(RemovalRefusal::InUse)),
            (409, "workflow.in_use")
        );
        assert_eq!(
            code(&removal_problem(RemovalRefusal::Last)),
            (409, "workflow.last")
        );
        let taken = conflict_problem(WriteConflict::KeyTaken);
        assert_eq!(code(&taken), (409, "workflow.key_taken"));
        assert_eq!(taken.errors, vec![FieldError::new("key", "taken")]);
        assert_eq!(
            code(&conflict_problem(WriteConflict::VersionMismatch)),
            (412, "precondition_failed")
        );
        assert_eq!(
            code(&conflict_problem(WriteConflict::NotFound)),
            (404, "not_found")
        );
    }

    #[test]
    fn problems_name_their_status_or_transition() {
        let unreachable = problem_view(WorkflowProblem::Unreachable(7));
        assert_eq!(
            (
                unreachable.code.as_str(),
                unreachable.status_id,
                unreachable.transition_id
            ),
            ("unreachable", Some(Uuid::from_u128(7)), None)
        );
        let duplicate = problem_view(WorkflowProblem::Duplicate(9));
        assert_eq!(
            (duplicate.status_id, duplicate.transition_id),
            (None, Some(Uuid::from_u128(9)))
        );
        let none = problem_view(WorkflowProblem::NoInitial);
        assert_eq!((none.code.as_str(), none.status_id), ("no_initial", None));
    }

    #[test]
    fn stored_categories_map_to_the_api() {
        for (raw, view) in [
            ("todo", CategoryView::Todo),
            ("in_progress", CategoryView::InProgress),
            ("done", CategoryView::Done),
        ] {
            assert_eq!(CategoryView::of(raw), view);
            assert_eq!(Category::from(view).as_str(), raw);
        }
    }

    #[test]
    fn a_portal_label_may_be_left_empty_but_not_wrong() {
        let mut v = Violations::new();
        labels_errors(&mut v, "portalLabelTranslations", &Labels::new(), false);
        let empty: Labels = [("fr".to_owned(), " ".to_owned())].into();
        labels_errors(&mut v, "portalLabelTranslations", &empty, false);
        assert!(v.into_result().is_ok());
        let mut v = Violations::new();
        let foreign: Labels = [("de".to_owned(), "Erledigt".to_owned())].into();
        labels_errors(&mut v, "portalLabelTranslations", &foreign, false);
        assert!(v.into_result().is_err());
    }
}
