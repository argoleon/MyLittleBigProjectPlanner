//! Issue types of a project configuration (specification §5, L04-US02):
//! Epic, Story, Task, Sub-task, Bug… with the types each may sit under.
//!
//! Types are read from the draft of the configuration if it has one, else
//! from its published version. Every change goes to the draft, opened on
//! the first one, and counts once per type in its changes; the projects get
//! it when the draft is published. A type keeps its `id` from one version
//! to the next, and its `key` for good.

use std::collections::BTreeMap;

use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use domain::configuration::{Item, ItemKind};
use domain::issue_types::{
    Color, HierarchyRefusal, RemovalRefusal, TypeInput, TypeNode, hierarchy_refusal, is_permutation, levels,
    removal_refusal, validate_type,
};
use domain::validation::FieldError;
use infra::configs as configs_repo;
use infra::issue_types::{self as repo, IssueType, TypeFields, WriteConflict};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::configs::{TAG, require_config_admin};
use crate::etag::{etag, require_if_match};
use crate::extract::{AcceptLanguage, ApiJson, ApiPath};
use crate::idempotency::{self, Start};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::tenant::TenantTx;

/// The color of a type: one of the type color tokens of the theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = IssueTypeColor)]
pub enum ColorView {
    Epic,
    Story,
    Task,
    Bug,
}

impl From<ColorView> for Color {
    fn from(color: ColorView) -> Self {
        match color {
            ColorView::Epic => Self::Epic,
            ColorView::Story => Self::Story,
            ColorView::Task => Self::Task,
            ColorView::Bug => Self::Bug,
        }
    }
}

impl ColorView {
    /// The stored color; the database only holds these four.
    fn of(raw: &str) -> Self {
        match Color::parse(raw) {
            Some(Color::Epic) => Self::Epic,
            Some(Color::Story) => Self::Story,
            Some(Color::Bug) => Self::Bug,
            Some(Color::Task) | None => Self::Task,
        }
    }
}

/// An issue type.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueType)]
pub struct IssueTypeView {
    /// Its identity, the same in every version of the configuration.
    id: Uuid,
    /// Its stable identifier, set when it is added.
    #[schema(example = "bug")]
    key: String,
    /// In the language of the request, else the tenant's default, else any.
    #[schema(example = "Bug")]
    label: Option<String>,
    #[schema(example = json!({"fr": "Bug", "en": "Bug"}))]
    label_translations: BTreeMap<String, String>,
    /// A Material Symbols name.
    #[schema(example = "bug_report")]
    icon: String,
    color: ColorView,
    /// A sub-task always has a parent, and is never one.
    is_subtask: bool,
    /// The types a ticket of this type may sit under (`id`s); none: only at
    /// the top.
    parent_ids: Vec<Uuid>,
    /// The workflow its tickets follow.
    #[schema(required = true)]
    workflow_id: Option<Uuid>,
    /// 1 for a type with no parent, else one more than its deepest parent.
    #[schema(example = 2)]
    level: u32,
    #[schema(example = 1)]
    version: i64,
}

/// Where the types were read.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = IssueTypeStage)]
pub enum StageView {
    /// The draft, which the projects get once published.
    Draft,
    /// The published version, which the projects use.
    Published,
}

/// The types of a configuration, in their order.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueTypeList)]
pub struct IssueTypeList {
    stage: StageView,
    items: Vec<IssueTypeView>,
}

/// A new issue type.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = NewIssueType,
    example = json!({
        "key": "bug",
        "labelTranslations": {"fr": "Bug", "en": "Bug"},
        "icon": "bug_report",
        "color": "bug",
        "isSubtask": false,
        "parentIds": ["01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9a"]
    })
)]
pub struct NewTypeBody {
    /// A lowercase letter, then up to 39 lowercase letters, digits or
    /// underscores; unique in the configuration, never changed.
    key: String,
    /// One label per language (`fr`, `en`), at least one; 60 characters at
    /// most.
    label_translations: BTreeMap<String, String>,
    /// A Material Symbols name (`bug_report`).
    icon: String,
    color: ColorView,
    #[serde(default)]
    is_subtask: bool,
    /// The types it may sit under; required for a sub-task.
    #[serde(default)]
    parent_ids: Vec<Uuid>,
    /// The workflow its tickets follow; by default the configuration's
    /// first one for a new type, and the one it has for a change.
    #[serde(default)]
    workflow_id: Option<Uuid>,
}

impl NewTypeBody {
    fn fields(&self) -> TypeBody {
        TypeBody {
            label_translations: self.label_translations.clone(),
            icon: self.icon.clone(),
            color: self.color,
            is_subtask: self.is_subtask,
            parent_ids: self.parent_ids.clone(),
            workflow_id: self.workflow_id,
        }
    }
}

/// What an issue type is made of; its `key` stays.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = IssueTypeFields,
    example = json!({
        "labelTranslations": {"fr": "Anomalie", "en": "Bug"},
        "icon": "bug_report",
        "color": "bug",
        "isSubtask": false,
        "parentIds": []
    })
)]
pub struct TypeBody {
    /// One label per language (`fr`, `en`), at least one; 60 characters at
    /// most.
    label_translations: BTreeMap<String, String>,
    /// A Material Symbols name (`bug_report`).
    icon: String,
    color: ColorView,
    #[serde(default)]
    is_subtask: bool,
    /// The types it may sit under; required for a sub-task.
    #[serde(default)]
    parent_ids: Vec<Uuid>,
    /// The workflow its tickets follow; by default the configuration's
    /// first one for a new type, and the one it has for a change.
    #[serde(default)]
    workflow_id: Option<Uuid>,
}

/// The order of the types.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = IssueTypeOrder)]
pub struct OrderBody {
    /// Every type of the configuration once, in the new order.
    ids: Vec<Uuid>,
}

impl TypeBody {
    fn validate(&self, key: Option<&str>) -> Result<(), Problem> {
        validate_type(&TypeInput {
            key,
            label_translations: &self.label_translations,
            icon: &self.icon,
        })
        .map_err(Problem::validation)
    }

    fn fields(&self, workflow_id: Uuid) -> TypeFields<'_> {
        TypeFields {
            workflow_id,
            label_translations: &self.label_translations,
            icon: &self.icon,
            color: Color::from(self.color).as_str(),
            is_subtask: self.is_subtask,
            parent_ids: &self.parent_ids,
        }
    }

    fn node(&self, id: Uuid) -> TypeNode {
        TypeNode {
            id: id.as_u128(),
            is_subtask: self.is_subtask,
            parents: self.parent_ids.iter().map(Uuid::as_u128).collect(),
        }
    }
}

fn views(language: &Language, types: Vec<IssueType>) -> Vec<IssueTypeView> {
    let nodes: Vec<TypeNode> = types.iter().map(IssueType::node).collect();
    let levels = levels(&nodes);
    types
        .into_iter()
        .map(|t| IssueTypeView {
            level: levels.get(&t.item_id.as_u128()).copied().unwrap_or(1),
            id: t.item_id,
            key: t.key,
            label: language.label(&t.label_translations),
            label_translations: t.label_translations,
            icon: t.icon,
            color: ColorView::of(&t.color),
            is_subtask: t.is_subtask,
            parent_ids: t.parent_ids,
            workflow_id: t.workflow_id,
            version: t.version,
        })
        .collect()
}

/// The view of the type `item` among `types`, with its ETag.
fn type_response(language: &Language, types: Vec<IssueType>, item: Uuid) -> Result<Response, Problem> {
    let view = views(language, types)
        .into_iter()
        .find(|t| t.id == item)
        .ok_or(Problem::new(ErrorCode::NotFound))?;
    let version = view.version;
    let mut response = ApiJson(view).into_response();
    response.headers_mut().insert(header::ETAG, etag(version));
    language.headers(&mut response);
    Ok(response)
}

fn conflict_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound).with_detail("no such type"),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
        WriteConflict::KeyTaken => {
            Problem::new(ErrorCode::IssueTypeKeyTaken).with_errors(vec![FieldError::new("key", "taken")])
        }
    }
}

fn hierarchy_problem(refusal: HierarchyRefusal) -> Problem {
    match refusal {
        HierarchyRefusal::UnknownParent(_) => {
            Problem::validation(vec![FieldError::new("parentIds", "not_found")])
        }
        HierarchyRefusal::SubtaskWithoutParent(_) => {
            Problem::validation(vec![FieldError::new("parentIds", "required")])
        }
        HierarchyRefusal::OwnAncestor(_) => Problem::new(ErrorCode::IssueTypeCycle),
        HierarchyRefusal::SubtaskAsParent(_) => Problem::new(ErrorCode::IssueTypeSubtaskParent),
    }
}

fn removal_problem(refusal: RemovalRefusal) -> Problem {
    Problem::new(match refusal {
        RemovalRefusal::LastType => ErrorCode::IssueTypeLastType,
        RemovalRefusal::OrphanSubtask => ErrorCode::IssueTypeOrphanSubtask,
    })
}

/// The version the types of `config` are read from: its draft if it has
/// one, else its published version.
async fn read_version(tx: &mut TenantTx, config: Uuid) -> Result<(Uuid, StageView), Problem> {
    let config = configs_repo::find(tx, config)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such configuration"))?;
    Ok(match config.draft {
        Some(draft) => (draft.id, StageView::Draft),
        None => (config.published.id, StageView::Published),
    })
}

/// The draft of `config`, opened if needed: every change goes there.
async fn draft(tx: &mut TenantTx, config: Uuid) -> Result<Uuid, Problem> {
    let actor = tx.account_id();
    let opened = configs_repo::open_draft(tx, config, Some(actor))
        .await?
        .map_err(|_| Problem::new(ErrorCode::NotFound).with_detail("no such configuration"))?;
    opened
        .draft
        .map(|d| d.id)
        .ok_or_else(|| Problem::new(ErrorCode::ConfigNoDraft))
}

/// The workflow `wanted` of the draft, which must be one of its workflows
/// (`workflowId` `not_found`); none: its first workflow.
async fn workflow_of(tx: &mut TenantTx, draft: Uuid, wanted: Option<Uuid>) -> Result<Uuid, Problem> {
    let workflows = infra::workflows::load(tx, draft).await?.workflows;
    let found = match wanted {
        Some(id) => workflows.iter().find(|w| w.item_id == id),
        None => workflows.first(),
    };
    found
        .map(|w| w.item_id)
        .ok_or_else(|| Problem::validation(vec![FieldError::new("workflowId", "not_found")]))
}

/// Counts the change of the type `item` in the draft.
async fn changed(tx: &mut TenantTx, draft: Uuid, item: Uuid) -> Result<(), Problem> {
    let item = Item {
        kind: ItemKind::IssueType,
        id: item.as_u128(),
    };
    configs_repo::record_change(tx, draft, item).await?;
    Ok(())
}

/// The hierarchy of `types` with `changed` in place of (or added to) its own.
fn with(types: &[IssueType], changed: TypeNode) -> Vec<TypeNode> {
    let mut nodes: Vec<TypeNode> = types
        .iter()
        .filter(|t| t.item_id.as_u128() != changed.id)
        .map(IssueType::node)
        .collect();
    nodes.push(changed);
    nodes
}

/// `GET /api/v1/project-configs/{id}/issue-types`.
#[utoipa::path(
    get,
    path = "/project-configs/{id}/issue-types",
    tag = TAG,
    operation_id = "listIssueTypes",
    summary = "List the issue types of a configuration",
    description = "Tenant admins and configuration admins. In their order; from the draft if the \
                   configuration has one (`stage: draft`), else from its published version.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `label`"),
    ),
    responses(
        (status = 200, description = "Its types", body = IssueTypeList),
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
    let (version, stage) = read_version(&mut tx, id).await?;
    let types = repo::list(&mut tx, version).await?;
    let language = Language::of(&tx, &accept);
    tx.commit().await?;
    let mut response = ApiJson(IssueTypeList {
        stage,
        items: views(&language, types),
    })
    .into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `GET /api/v1/project-configs/{id}/issue-types/{typeId}`.
#[utoipa::path(
    get,
    path = "/project-configs/{id}/issue-types/{typeId}",
    tag = TAG,
    operation_id = "getIssueType",
    summary = "Read an issue type",
    description = "Tenant admins and configuration admins. From the draft if the configuration has \
                   one, else from its published version.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("typeId" = Uuid, Path, description = "Id of the type"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `label`"),
    ),
    responses(
        (status = 200, description = "The type", body = IssueTypeView, headers(
            ("ETag" = String, description = "Version of the type"),
        )),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration or type", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn read(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath((id, item)): ApiPath<(Uuid, Uuid)>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let (version, _) = read_version(&mut tx, id).await?;
    let types = repo::list(&mut tx, version).await?;
    let language = Language::of(&tx, &accept);
    tx.commit().await?;
    type_response(&language, types, item)
}

/// `POST /api/v1/project-configs/{id}/issue-types`: idempotent with
/// `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/project-configs/{id}/issue-types",
    tag = TAG,
    operation_id = "createIssueType",
    summary = "Add an issue type to the draft of a configuration",
    description = "Tenant admins and configuration admins. It goes last, in the draft, opened if \
                   needed; the projects get it once the draft is published.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `label`"),
    ),
    request_body = NewTypeBody,
    responses(
        (status = 201, description = "Added", body = IssueTypeView, headers(
            ("ETag" = String, description = "Version of the type"),
            ("Location" = String, description = "URL of the type"),
        )),
        (status = 400, description = "`validation_failed`: fields, or `parentIds` `not_found` / `required` (a sub-task)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`issue_type.key_taken`, `issue_type.subtask_parent`, or `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<NewTypeBody>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let path = format!("/api/v1/project-configs/{id}/issue-types");
    let key = match idempotency::start(&mut tx, &headers, &format!("POST {path}"), &body).await? {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    let fields = body.fields();
    fields.validate(Some(&body.key))?;
    let draft = draft(&mut tx, id).await?;
    let item = infra::id::new_id();
    let types = repo::list(&mut tx, draft).await?;
    if let Some(refusal) = hierarchy_refusal(&with(&types, fields.node(item)), Some(item.as_u128())) {
        return Err(hierarchy_problem(refusal));
    }
    let workflow = workflow_of(&mut tx, draft, fields.workflow_id).await?;
    repo::insert(&mut tx, draft, item, &body.key, &fields.fields(workflow))
        .await?
        .map_err(conflict_problem)?;
    changed(&mut tx, draft, item).await?;
    let language = Language::of(&tx, &accept);
    let types = repo::list(&mut tx, draft).await?;
    let view = views(&language, types)
        .into_iter()
        .find(|t| t.id == item)
        .ok_or(Problem::new(ErrorCode::NotFound))?;
    let version = view.version;
    let mut response =
        idempotency::created(&mut tx, key.as_deref(), &view, format!("{path}/{item}"), version).await?;
    tx.commit().await?;
    language.headers(&mut response);
    Ok(response)
}

/// `PUT /api/v1/project-configs/{id}/issue-types/{typeId}`: requires
/// `If-Match`.
#[utoipa::path(
    put,
    path = "/project-configs/{id}/issue-types/{typeId}",
    tag = TAG,
    operation_id = "updateIssueType",
    summary = "Change an issue type in the draft of a configuration",
    description = "Tenant admins and configuration admins. In the draft, opened if needed; its `key` \
                   stays. A type is never its own ancestor (`issue_type.cycle`).",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("typeId" = Uuid, Path, description = "Id of the type"),
        ("If-Match" = String, Header, description = "ETag of the type, or `*`"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `label`"),
    ),
    request_body = TypeBody,
    responses(
        (status = 200, description = "The type", body = IssueTypeView, headers(
            ("ETag" = String, description = "New version of the type"),
        )),
        (status = 400, description = "`validation_failed`: fields, or `parentIds` `not_found` / `required` (a sub-task)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration or type", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`issue_type.cycle` or `issue_type.subtask_parent`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The type changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath((id, item)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<TypeBody>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    body.validate(None)?;
    let draft = draft(&mut tx, id).await?;
    let types = repo::list(&mut tx, draft).await?;
    let Some(current) = types.iter().find(|t| t.item_id == item) else {
        return Err(conflict_problem(WriteConflict::NotFound));
    };
    if let Some(refusal) = hierarchy_refusal(&with(&types, body.node(item)), Some(item.as_u128())) {
        return Err(hierarchy_problem(refusal));
    }
    let workflow = workflow_of(&mut tx, draft, body.workflow_id.or(current.workflow_id)).await?;
    repo::update(&mut tx, draft, item, if_match.versions(), &body.fields(workflow))
        .await?
        .map_err(conflict_problem)?;
    changed(&mut tx, draft, item).await?;
    let types = repo::list(&mut tx, draft).await?;
    let language = Language::of(&tx, &accept);
    tx.commit().await?;
    type_response(&language, types, item)
}

/// `DELETE /api/v1/project-configs/{id}/issue-types/{typeId}`: requires
/// `If-Match`.
#[utoipa::path(
    delete,
    path = "/project-configs/{id}/issue-types/{typeId}",
    tag = TAG,
    operation_id = "deleteIssueType",
    summary = "Remove an issue type from the draft of a configuration",
    description = "Tenant admins and configuration admins. From the draft, opened if needed, and \
                   from the parents of the other types. If tickets use it, publishing needs them \
                   moved to another type first (`needsMigration` of the publish impact).",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("typeId" = Uuid, Path, description = "Id of the type"),
        ("If-Match" = String, Header, description = "ETag of the type, or `*`"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration or type", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`issue_type.last_type` or `issue_type.orphan_subtask`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The type changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath((id, item)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, Problem> {
    require_config_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let draft = draft(&mut tx, id).await?;
    let types = repo::list(&mut tx, draft).await?;
    if !types.iter().any(|t| t.item_id == item) {
        return Err(conflict_problem(WriteConflict::NotFound));
    }
    let nodes: Vec<TypeNode> = types.iter().map(IssueType::node).collect();
    if let Some(refusal) = removal_refusal(&nodes, item.as_u128()) {
        return Err(removal_problem(refusal));
    }
    repo::delete(&mut tx, draft, item, if_match.versions())
        .await?
        .map_err(conflict_problem)?;
    changed(&mut tx, draft, item).await?;
    for other in types.iter().filter(|t| t.parent_ids.contains(&item)) {
        changed(&mut tx, draft, other.item_id).await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/v1/project-configs/{id}/issue-types/order`.
#[utoipa::path(
    put,
    path = "/project-configs/{id}/issue-types/order",
    tag = TAG,
    operation_id = "reorderIssueTypes",
    summary = "Order the issue types in the draft of a configuration",
    description = "Tenant admins and configuration admins. Every type once, in the new order; each \
                   type that moves counts as a change of the draft.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `label`"),
    ),
    request_body = OrderBody,
    responses(
        (status = 200, description = "The types, in their new order", body = IssueTypeList),
        (status = 400, description = "`validation_failed`: `ids` `not_a_permutation`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn reorder(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<OrderBody>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let draft = draft(&mut tx, id).await?;
    let types = repo::list(&mut tx, draft).await?;
    let ids: Vec<u128> = types.iter().map(|t| t.item_id.as_u128()).collect();
    let order: Vec<u128> = body.ids.iter().map(Uuid::as_u128).collect();
    if !is_permutation(&order, &ids) {
        return Err(Problem::validation(vec![FieldError::new(
            "ids",
            "not_a_permutation",
        )]));
    }
    for moved in repo::reorder(&mut tx, draft, &body.ids).await? {
        changed(&mut tx, draft, moved).await?;
    }
    let types = repo::list(&mut tx, draft).await?;
    let language = Language::of(&tx, &accept);
    tx.commit().await?;
    let mut response = ApiJson(IssueTypeList {
        stage: StageView::Draft,
        items: views(&language, types),
    })
    .into_response();
    language.headers(&mut response);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(problem: &Problem) -> (u16, &'static str) {
        (problem.code.status().as_u16(), problem.code.as_str())
    }

    #[test]
    fn hierarchy_refusals_have_their_own_codes() {
        let cycle = hierarchy_problem(HierarchyRefusal::OwnAncestor(1));
        assert_eq!(code(&cycle), (409, "issue_type.cycle"));
        let subtask = hierarchy_problem(HierarchyRefusal::SubtaskAsParent(1));
        assert_eq!(code(&subtask), (409, "issue_type.subtask_parent"));
        for (refusal, field_code) in [
            (HierarchyRefusal::UnknownParent(1), "not_found"),
            (HierarchyRefusal::SubtaskWithoutParent(1), "required"),
        ] {
            let problem = hierarchy_problem(refusal);
            assert_eq!(code(&problem), (400, "validation_failed"));
            assert_eq!(problem.errors, vec![FieldError::new("parentIds", field_code)]);
        }
    }

    #[test]
    fn removal_refusals_and_conflicts_have_their_own_codes() {
        assert_eq!(
            code(&removal_problem(RemovalRefusal::LastType)),
            (409, "issue_type.last_type")
        );
        assert_eq!(
            code(&removal_problem(RemovalRefusal::OrphanSubtask)),
            (409, "issue_type.orphan_subtask")
        );
        let taken = conflict_problem(WriteConflict::KeyTaken);
        assert_eq!(code(&taken), (409, "issue_type.key_taken"));
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
    fn stored_colors_map_to_the_api() {
        for (raw, view) in [
            ("epic", ColorView::Epic),
            ("story", ColorView::Story),
            ("task", ColorView::Task),
            ("bug", ColorView::Bug),
        ] {
            assert_eq!(ColorView::of(raw), view);
            assert_eq!(Color::from(view).as_str(), raw);
        }
    }
}
