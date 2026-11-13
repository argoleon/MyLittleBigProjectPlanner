//! Project configurations (specification §5, L04-US01): what a project
//! works with, shared by any number of projects. Tenant admins and
//! configuration admins manage them; the others get 403.
//!
//! - Each tenant has a default configuration, which new projects get; a
//!   project moves to another one with `PUT /nodes/{id}/configuration`.
//! - Edits go to a draft, shared by the admins (`POST …/draft`); publishing
//!   it (`POST …/publish`) applies it to every project at once, after
//!   `GET …/publish-impact` has listed them.
//! - A project detaches from a shared configuration by duplicating it
//!   (`POST …/duplicate` with its `projectId`).
//!
//! A project the member does not see shows veiled in the lists of a
//! configuration: counted, without name or real id.

use std::collections::{BTreeMap, BTreeSet};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use domain::configuration::{
    DeletionRefusal, deletion_refusal, manages_configurations, publish_impact, validate_name,
};
use domain::hierarchy::NodeType;
use domain::validation::FieldError;
use infra::configs::{self as repo, Config, ConfigProject, WriteConflict};
use infra::nodes::Node;
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::etag::{etag, if_none_match, require_if_match};
use crate::extract::{AcceptLanguage, ApiJson, ApiPath, ApiQuery};
use crate::idempotency::{self, Start};
use crate::nodes::{Sight, ensure_changeable, visible_node};
use crate::pagination::{PageParams, encode_cursor};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::tenant::{AppState, TenantTx};

pub const TAG: &str = "configurations";

const PATH: &str = "/api/v1/project-configs";

/// A project configuration.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ProjectConfig)]
pub struct ConfigView {
    id: Uuid,
    /// In the language of the request (`Accept-Language`), else the
    /// tenant's default, else any.
    #[schema(example = "Software development")]
    name: Option<String>,
    #[schema(example = json!({"fr": "Développement logiciel", "en": "Software development"}))]
    name_translations: BTreeMap<String, String>,
    /// The configuration new projects get; it cannot be deleted.
    is_default: bool,
    /// Projects using it, archived ones included.
    #[schema(example = 6)]
    project_count: i64,
    published: PublishedView,
    /// The unpublished draft, `null` when there is none.
    #[schema(required = true)]
    draft: Option<DraftView>,
    #[schema(example = 3)]
    version: i64,
}

/// The published version of a configuration, used by its projects.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ProjectConfigPublished)]
pub struct PublishedView {
    /// 1, then one more per publication.
    #[schema(example = 2)]
    number: i32,
    published_at: DateTime<Utc>,
}

/// The draft of a configuration, shared by its admins.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ProjectConfigDraft)]
pub struct DraftView {
    #[schema(example = 3)]
    number: i32,
    /// Items it changed, each once however many times it was edited.
    #[schema(example = 2)]
    change_count: i64,
    created_at: DateTime<Utc>,
}

/// A page of configurations.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ProjectConfigPage)]
pub struct ConfigPage {
    items: Vec<ConfigView>,
    /// Cursor of the next page, `null` on the last page.
    #[schema(required = true)]
    next_cursor: Option<String>,
}

/// `?limit=&cursor=` of the configurations list.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ConfigListParams {
    /// Page size, from 1 to 200 (default 50).
    #[param(minimum = 1, maximum = 200)]
    pub limit: Option<usize>,
    /// `nextCursor` of the previous page.
    pub cursor: Option<String>,
}

/// The name of a configuration.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = ProjectConfigName,
    example = json!({"nameTranslations": {"fr": "Développement logiciel", "en": "Software development"}})
)]
pub struct NameBody {
    /// One name per language (`fr`, `en`), at least one; 100 characters at
    /// most, unique in the tenant whatever the case.
    name_translations: BTreeMap<String, String>,
}

/// A copy of a configuration, which a project may move to.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = ProjectConfigCopy,
    example = json!({
        "nameTranslations": {"fr": "Développement logiciel (PLAT)", "en": "Software development (PLAT)"},
        "projectId": "01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9a"
    })
)]
pub struct DuplicateBody {
    /// Name of the copy, as for a new configuration.
    name_translations: BTreeMap<String, String>,
    /// A project using the configuration, which moves to the copy: further
    /// changes of the original no longer affect it.
    #[serde(default)]
    project_id: Option<Uuid>,
}

/// A project using a configuration.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ConfigProject)]
pub struct ProjectView {
    id: Uuid,
    #[schema(example = "Identity platform")]
    name: String,
    #[schema(example = "PLAT")]
    key: String,
    /// Read-only until restored; it still uses the configuration.
    archived: bool,
}

/// A project the member does not see: counted, without name or detail.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = VeiledProject)]
pub struct VeiledProjectView {
    /// Always `true`: tells a veiled project from a project.
    #[schema(example = true)]
    veiled: bool,
    /// An opaque identifier: stable, but not the project's id.
    id: Uuid,
}

/// A project of a configuration, seen or veiled.
#[derive(Debug, Serialize, ToSchema)]
#[serde(untagged)]
#[schema(as = ConfigProjectItem)]
pub enum ProjectItemView {
    Project(ProjectView),
    Veiled(VeiledProjectView),
}

fn project_items(projects: Vec<ConfigProject>, sight: &Sight) -> Vec<ProjectItemView> {
    projects
        .into_iter()
        .map(|project| {
            if sight.sees(project.id) {
                ProjectItemView::Project(ProjectView {
                    id: project.id,
                    name: project.name,
                    key: project.key,
                    archived: project.archived,
                })
            } else {
                ProjectItemView::Veiled(VeiledProjectView {
                    veiled: true,
                    id: infra::nodes::opaque_id(project.id),
                })
            }
        })
        .collect()
}

/// The projects using a configuration, by name.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ConfigProjectList)]
pub struct ProjectList {
    items: Vec<ProjectItemView>,
}

/// What publishing the draft does.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ProjectConfigPublishImpact)]
pub struct ImpactView {
    /// Every project that gets the new version, archived ones included.
    projects: Vec<ProjectItemView>,
    /// Whether tickets must be mapped to other types or statuses first
    /// (the migration wizard of L04-US08).
    needs_migration: bool,
    /// Version of the configuration this impact was computed on: publish
    /// with it as `If-Match`, so that nothing changed in between.
    #[schema(example = 3)]
    version: i64,
}

/// The configuration of a project.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = NodeConfiguration)]
pub struct NodeConfigView {
    id: Uuid,
    #[schema(example = "Software development")]
    name: Option<String>,
    /// Projects sharing it, this one included.
    #[schema(example = 6)]
    project_count: i64,
    is_default: bool,
}

/// The configuration a project moves to.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = NodeConfigurationChange, example = json!({"configId": "01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9a"}))]
pub struct AssignBody {
    config_id: Uuid,
}

impl Language {
    fn config(&self, config: Config) -> ConfigView {
        ConfigView {
            id: config.id,
            name: self.label(&config.name_translations),
            name_translations: config.name_translations,
            is_default: config.is_default,
            project_count: config.project_count,
            published: PublishedView {
                number: config.published.number,
                published_at: config.published.published_at,
            },
            draft: config.draft.map(|draft| DraftView {
                number: draft.number,
                change_count: draft.change_count,
                created_at: draft.created_at,
            }),
            version: config.version,
        }
    }

    fn node_config(&self, config: &Config) -> NodeConfigView {
        NodeConfigView {
            id: config.id,
            name: self.label(&config.name_translations),
            project_count: config.project_count,
            is_default: config.is_default,
        }
    }
}

/// Refuses whoever does not manage configurations: `forbidden`.
pub(crate) fn require_config_admin(tx: &TenantTx) -> Result<(), Problem> {
    if manages_configurations(tx.member().tenant_role) {
        Ok(())
    } else {
        Err(Problem::new(ErrorCode::Forbidden))
    }
}

fn conflict_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
        WriteConflict::NameTaken => Problem::new(ErrorCode::ConfigNameTaken)
            .with_errors(vec![FieldError::new("nameTranslations", "taken")]),
        WriteConflict::NoDraft => Problem::new(ErrorCode::ConfigNoDraft),
    }
}

fn deletion_problem(refusal: DeletionRefusal) -> Problem {
    Problem::new(match refusal {
        DeletionRefusal::IsDefault => ErrorCode::ConfigDefault,
        DeletionRefusal::InUse => ErrorCode::ConfigInUse,
    })
}

/// The response of a configuration, with its ETag and language.
fn config_response(language: &Language, config: Config) -> Response {
    let version = config.version;
    let mut response = ApiJson(language.config(config)).into_response();
    response.headers_mut().insert(header::ETAG, etag(version));
    language.headers(&mut response);
    response
}

async fn find(tx: &mut TenantTx, id: Uuid) -> Result<Config, Problem> {
    repo::find(tx, id)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such configuration"))
}

/// `GET /api/v1/project-configs`, in creation order.
#[utoipa::path(
    get,
    path = "/project-configs",
    tag = TAG,
    operation_id = "listProjectConfigs",
    summary = "List the project configurations",
    description = "Tenant admins and configuration admins. In creation order, the default \
                   configuration first; follow `nextCursor` until it is `null`.",
    params(
        ConfigListParams,
        ("Accept-Language" = Option<String>, Header, description = "Language of `name`"),
    ),
    responses(
        (status = 200, description = "A page of configurations", body = ConfigPage, headers(
            ("Content-Language" = String, description = "Language of `name`"),
        )),
        (status = 400, description = "Invalid `limit` or `cursor`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: neither a tenant admin nor a configuration admin", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiQuery(params): ApiQuery<ConfigListParams>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let (limit, after) = PageParams {
        limit: params.limit,
        cursor: params.cursor,
    }
    .resolve()?;
    let language = Language::of(&tx, &accept);
    let page = repo::list(&mut tx, after, limit).await?;
    tx.commit().await?;
    let mut response = ApiJson(ConfigPage {
        items: page.items.into_iter().map(|c| language.config(c)).collect(),
        next_cursor: page.next.map(encode_cursor),
    })
    .into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `POST /api/v1/project-configs`: idempotent with `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/project-configs",
    tag = TAG,
    operation_id = "createProjectConfig",
    summary = "Create a project configuration",
    description = "Tenant admins and configuration admins. It starts with a published version 1, \
                   and no project.",
    params(
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `name`"),
    ),
    request_body = NameBody,
    responses(
        (status = 201, description = "Created", body = ConfigView, headers(
            ("ETag" = String, description = "Version of the configuration"),
            ("Location" = String, description = "URL of the configuration"),
        )),
        (status = 400, description = "`validation_failed`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`config.name_taken`, or `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiJson(body): ApiJson<NameBody>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let key = match idempotency::start(&mut tx, &headers, &format!("POST {PATH}"), &body).await? {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    validate_name(&body.name_translations).map_err(Problem::validation)?;
    let actor = tx.account_id();
    let config = repo::insert(&mut tx, infra::id::new_id(), &body.name_translations, Some(actor))
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %actor, config = %config.id, "project configuration created");
    let language = Language::of(&tx, &accept);
    let (id, version) = (config.id, config.version);
    let mut response = idempotency::created(
        &mut tx,
        key.as_deref(),
        &language.config(config),
        format!("{PATH}/{id}"),
        version,
    )
    .await?;
    tx.commit().await?;
    language.headers(&mut response);
    Ok(response)
}

/// `GET /api/v1/project-configs/{id}`.
#[utoipa::path(
    get,
    path = "/project-configs/{id}",
    tag = TAG,
    operation_id = "getProjectConfig",
    summary = "Read a project configuration",
    description = "Tenant admins and configuration admins.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("If-None-Match" = Option<String>, Header, description = "ETag already held: 304 if still current"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `name`"),
    ),
    responses(
        (status = 200, description = "The configuration", body = ConfigView, headers(
            ("ETag" = String, description = "Version of the configuration"),
            ("Content-Language" = String, description = "Language of `name`"),
        )),
        (status = 304, description = "Not modified since the `If-None-Match` ETag"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn read(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let language = Language::of(&tx, &accept);
    let config = find(&mut tx, id).await?;
    tx.commit().await?;
    if if_none_match(&headers, config.version) {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        response.headers_mut().insert(header::ETAG, etag(config.version));
        return Ok(response);
    }
    Ok(config_response(&language, config))
}

/// `PUT /api/v1/project-configs/{id}`: requires `If-Match`.
#[utoipa::path(
    put,
    path = "/project-configs/{id}",
    tag = TAG,
    operation_id = "renameProjectConfig",
    summary = "Rename a project configuration",
    description = "Tenant admins and configuration admins. The name applies at once: it is no \
                   content of the configuration, which goes through its draft.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("If-Match" = String, Header, description = "ETag of the version being changed, or `*`"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `name`"),
    ),
    request_body = NameBody,
    responses(
        (status = 200, description = "The configuration", body = ConfigView, headers(
            ("ETag" = String, description = "New version of the configuration"),
        )),
        (status = 400, description = "`validation_failed`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`config.name_taken`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The configuration changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn rename(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<NameBody>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    validate_name(&body.name_translations).map_err(Problem::validation)?;
    let language = Language::of(&tx, &accept);
    let config = repo::rename(&mut tx, id, if_match.versions(), &body.name_translations)
        .await?
        .map_err(conflict_problem)?;
    tx.commit().await?;
    Ok(config_response(&language, config))
}

/// `DELETE /api/v1/project-configs/{id}`: requires `If-Match`.
#[utoipa::path(
    delete,
    path = "/project-configs/{id}",
    tag = TAG,
    operation_id = "deleteProjectConfig",
    summary = "Delete a project configuration",
    description = "Tenant admins and configuration admins. Refused for the default configuration \
                   (`config.default`) and while projects use it (`config.in_use`).",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("If-Match" = String, Header, description = "ETag of the configuration, or `*`"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`config.default` or `config.in_use`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The configuration changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, Problem> {
    require_config_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let config = find(&mut tx, id).await?;
    let used = u64::try_from(config.project_count).unwrap_or(u64::MAX);
    if let Some(refusal) = deletion_refusal(config.is_default, used) {
        return Err(deletion_problem(refusal));
    }
    repo::delete(&mut tx, id, if_match.versions())
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(target: "audit", actor = %tx.account_id(), config = %id, "project configuration deleted");
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/project-configs/{id}/projects`.
#[utoipa::path(
    get,
    path = "/project-configs/{id}/projects",
    tag = TAG,
    operation_id = "listProjectConfigProjects",
    summary = "List the projects using a configuration",
    description = "Tenant admins and configuration admins. By name, archived projects included. \
                   A project the member does not see comes veiled (`veiled: true`).",
    params(("id" = Uuid, Path, description = "Id of the configuration")),
    responses(
        (status = 200, description = "Its projects", body = ProjectList),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn projects(mut tx: TenantTx, ApiPath(id): ApiPath<Uuid>) -> Result<ApiJson<ProjectList>, Problem> {
    require_config_admin(&tx)?;
    find(&mut tx, id).await?;
    let projects = repo::projects(&mut tx, id).await?;
    let sight = Sight::of(&mut tx).await?;
    tx.commit().await?;
    Ok(ApiJson(ProjectList {
        items: project_items(projects, &sight),
    }))
}

/// `POST /api/v1/project-configs/{id}/duplicate`: idempotent with
/// `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/project-configs/{id}/duplicate",
    tag = TAG,
    operation_id = "duplicateProjectConfig",
    summary = "Duplicate a project configuration, or detach a project from it",
    description = "Tenant admins and configuration admins. The copy starts from the published \
                   version of the original (its draft stays with it). With `projectId`, that \
                   project moves to the copy: further changes of the original no longer affect \
                   it.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration to copy"),
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `name`"),
    ),
    request_body = DuplicateBody,
    responses(
        (status = 201, description = "The copy", body = ConfigView, headers(
            ("ETag" = String, description = "Version of the copy"),
            ("Location" = String, description = "URL of the copy"),
        )),
        (status = 400, description = "`validation_failed`; `projectId` `not_using` when the project uses another configuration", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration, or no such project visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`config.name_taken`, `node.archived`, or `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn duplicate(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<DuplicateBody>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let key =
        match idempotency::start(&mut tx, &headers, &format!("POST {PATH}/{id}/duplicate"), &body).await? {
            Start::Proceed(key) => key,
            Start::Replay(response) => return Ok(response),
        };
    validate_name(&body.name_translations).map_err(Problem::validation)?;
    find(&mut tx, id).await?;
    if let Some(project) = body.project_id {
        let node = project_node(&state, &mut tx, project).await?;
        ensure_changeable(&node)?;
        if repo::of_project(&mut tx, project).await? != Some(id) {
            return Err(Problem::validation(vec![FieldError::new(
                "projectId",
                "not_using",
            )]));
        }
    }
    let actor = tx.account_id();
    let copy = repo::duplicate(
        &mut tx,
        id,
        infra::id::new_id(),
        &body.name_translations,
        body.project_id,
        Some(actor),
    )
    .await?
    .map_err(conflict_problem)?;
    tracing::info!(
        target: "audit",
        actor = %actor,
        config = %copy.id,
        source = %id,
        project = ?body.project_id,
        "project configuration duplicated"
    );
    let language = Language::of(&tx, &accept);
    let (copy_id, version) = (copy.id, copy.version);
    let mut response = idempotency::created(
        &mut tx,
        key.as_deref(),
        &language.config(copy),
        format!("{PATH}/{copy_id}"),
        version,
    )
    .await?;
    tx.commit().await?;
    language.headers(&mut response);
    Ok(response)
}

/// `POST /api/v1/project-configs/{id}/draft`.
#[utoipa::path(
    post,
    path = "/project-configs/{id}/draft",
    tag = TAG,
    operation_id = "openProjectConfigDraft",
    summary = "Open the draft of a project configuration",
    description = "Tenant admins and configuration admins. The draft starts as a copy of the \
                   published version; its admins share it. If it is open already, it is returned \
                   as it is.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `name`"),
    ),
    responses(
        (status = 200, description = "The configuration, with its draft", body = ConfigView, headers(
            ("ETag" = String, description = "Version of the configuration"),
        )),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn open_draft(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let actor = tx.account_id();
    let config = repo::open_draft(&mut tx, id, Some(actor))
        .await?
        .map_err(conflict_problem)?;
    let language = Language::of(&tx, &accept);
    tx.commit().await?;
    Ok(config_response(&language, config))
}

/// `DELETE /api/v1/project-configs/{id}/draft`: requires `If-Match`.
#[utoipa::path(
    delete,
    path = "/project-configs/{id}/draft",
    tag = TAG,
    operation_id = "discardProjectConfigDraft",
    summary = "Discard the draft of a project configuration",
    description = "Tenant admins and configuration admins. The published version stays as it is.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("If-Match" = String, Header, description = "ETag of the configuration, or `*`"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `name`"),
    ),
    responses(
        (status = 200, description = "The configuration, without draft", body = ConfigView, headers(
            ("ETag" = String, description = "New version of the configuration"),
        )),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`config.no_draft`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The configuration changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn discard_draft(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let config = repo::discard_draft(&mut tx, id, if_match.versions())
        .await?
        .map_err(conflict_problem)?;
    let language = Language::of(&tx, &accept);
    tx.commit().await?;
    Ok(config_response(&language, config))
}

/// `GET /api/v1/project-configs/{id}/publish-impact`.
#[utoipa::path(
    get,
    path = "/project-configs/{id}/publish-impact",
    tag = TAG,
    operation_id = "getProjectConfigPublishImpact",
    summary = "What publishing the draft of a configuration does",
    description = "Tenant admins and configuration admins. The projects that get the new version, \
                   to confirm before publishing, and whether their tickets must be migrated \
                   first. A project the member does not see comes veiled.",
    params(("id" = Uuid, Path, description = "Id of the configuration")),
    responses(
        (status = 200, description = "The impact", body = ImpactView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`config.no_draft`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn impact(mut tx: TenantTx, ApiPath(id): ApiPath<Uuid>) -> Result<ApiJson<ImpactView>, Problem> {
    require_config_admin(&tx)?;
    let config = find(&mut tx, id).await?;
    if config.draft.is_none() {
        return Err(Problem::new(ErrorCode::ConfigNoDraft));
    }
    let projects = repo::projects(&mut tx, id).await?;
    let sight = Sight::of(&mut tx).await?;
    tx.commit().await?;
    // Nothing is removed yet: types (L04-US02) and workflows (L04-US04)
    // will tell what the draft removes.
    let impact = publish_impact(projects, &BTreeSet::new());
    Ok(ApiJson(ImpactView {
        projects: project_items(impact.projects, &sight),
        needs_migration: impact.needs_migration,
        version: config.version,
    }))
}

/// `POST /api/v1/project-configs/{id}/publish`: requires `If-Match`.
#[utoipa::path(
    post,
    path = "/project-configs/{id}/publish",
    tag = TAG,
    operation_id = "publishProjectConfig",
    summary = "Publish the draft of a project configuration",
    description = "Tenant admins and configuration admins. The draft becomes the published \
                   version of every project of the configuration at once; the former one is \
                   retired. Send the `version` of the publish impact the user confirmed as \
                   `If-Match`: if the configuration changed since, 412.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("If-Match" = String, Header, description = "ETag of the configuration the impact was read on"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `name`"),
    ),
    responses(
        (status = 200, description = "The configuration, published", body = ConfigView, headers(
            ("ETag" = String, description = "New version of the configuration"),
        )),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`config.no_draft`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The configuration changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn publish(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let config = repo::publish(&mut tx, id, if_match.versions())
        .await?
        .map_err(conflict_problem)?;
    tracing::info!(
        target: "audit",
        actor = %tx.account_id(),
        config = %id,
        number = config.published.number,
        projects = config.project_count,
        "project configuration published"
    );
    let language = Language::of(&tx, &accept);
    tx.commit().await?;
    Ok(config_response(&language, config))
}

/// The project `id` if the member sees it, else 404; 404 too for a
/// portfolio or a program, which have no configuration.
async fn project_node(state: &AppState, tx: &mut TenantTx, id: Uuid) -> Result<Node, Problem> {
    let node = visible_node(state, tx, id, "no such project").await?;
    if node.node_type == NodeType::Project {
        Ok(node)
    } else {
        Err(Problem::new(ErrorCode::NotFound).with_detail("only projects have a configuration"))
    }
}

/// `GET /api/v1/nodes/{id}/configuration`.
#[utoipa::path(
    get,
    path = "/nodes/{id}/configuration",
    tag = TAG,
    operation_id = "getNodeConfiguration",
    summary = "Read the configuration of a project",
    description = "Whoever sees the project.",
    params(
        ("id" = Uuid, Path, description = "Id of the project"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `name`"),
    ),
    responses(
        (status = 200, description = "Its configuration", body = NodeConfigView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such project visible to the member, or a portfolio or program", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn node_config(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    project_node(&state, &mut tx, id).await?;
    let config_id = repo::of_project(&mut tx, id)
        .await?
        .ok_or(Problem::new(ErrorCode::NotFound))?;
    let config = find(&mut tx, config_id).await?;
    let language = Language::of(&tx, &accept);
    tx.commit().await?;
    let mut response = ApiJson(language.node_config(&config)).into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `PUT /api/v1/nodes/{id}/configuration`.
#[utoipa::path(
    put,
    path = "/nodes/{id}/configuration",
    tag = TAG,
    operation_id = "setNodeConfiguration",
    summary = "Move a project to another configuration",
    description = "Tenant admins and configuration admins, on a project they see. It uses the \
                   published version of its new configuration at once.",
    params(
        ("id" = Uuid, Path, description = "Id of the project"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `name`"),
    ),
    request_body = AssignBody,
    responses(
        (status = 200, description = "Its configuration", body = NodeConfigView),
        (status = 400, description = "`validation_failed`: `configId` `not_found`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such project visible to the member, or a portfolio or program", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`node.archived`: an archived project is read-only", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn set_node_config(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<AssignBody>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let node = project_node(&state, &mut tx, id).await?;
    ensure_changeable(&node)?;
    if repo::find(&mut tx, body.config_id).await?.is_none() {
        return Err(Problem::validation(vec![FieldError::new(
            "configId",
            "not_found",
        )]));
    }
    let before = repo::of_project(&mut tx, id).await?;
    repo::assign(&mut tx, id, body.config_id).await?;
    if before != Some(body.config_id) {
        tracing::info!(
            target: "audit",
            actor = %tx.account_id(),
            node = %id,
            config = %body.config_id,
            "project configuration changed"
        );
    }
    let config = find(&mut tx, body.config_id).await?;
    let language = Language::of(&tx, &accept);
    tx.commit().await?;
    let mut response = ApiJson(language.node_config(&config)).into_response();
    language.headers(&mut response);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_conflicts_have_their_own_codes() {
        let cases = [
            (WriteConflict::NotFound, 404, "not_found"),
            (WriteConflict::VersionMismatch, 412, "precondition_failed"),
            (WriteConflict::NameTaken, 409, "config.name_taken"),
            (WriteConflict::NoDraft, 409, "config.no_draft"),
        ];
        for (conflict, status, code) in cases {
            let problem = conflict_problem(conflict);
            assert_eq!(
                (problem.code.status().as_u16(), problem.code.as_str()),
                (status, code),
                "{conflict:?}"
            );
        }
    }

    #[test]
    fn deletion_refusals_have_their_own_codes() {
        let cases = [
            (DeletionRefusal::IsDefault, "config.default"),
            (DeletionRefusal::InUse, "config.in_use"),
        ];
        for (refusal, code) in cases {
            let problem = deletion_problem(refusal);
            assert_eq!(
                (problem.code.status().as_u16(), problem.code.as_str()),
                (409, code),
                "{refusal:?}"
            );
        }
    }
}
