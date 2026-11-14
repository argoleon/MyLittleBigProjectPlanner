//! Screens of a project configuration (specification §5, L04-US06): the
//! fields shown, in their order, when a ticket is created, viewed, or moved
//! by a transition.
//!
//! Every configuration has a general creation screen and a general view
//! screen; a type may have its own, which takes over for it. Transition
//! screens are named and attached to transitions (their `rules.screenId`).
//! Screens are read from the draft if the configuration has one; every
//! change goes to the draft. The resolved screen applies the field rules:
//! hidden fields go, required ones missing are added at the end.

use std::collections::BTreeMap;

use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use domain::configuration::{Item, ItemKind};
use domain::fields::{Mode, Rules, Step};
use domain::i18n::validate_translations;
use domain::screens::{
    FieldRules, Kind, RemovalRefusal, SCREEN_NAME_MAX_CHARS, fields_errors, pick, removal_refusal, resolve,
};
use domain::validation::{FieldError, Violations};
use infra::configs as configs_repo;
use infra::screens::{self as repo, Screen, WriteConflict};
use infra::uuid::Uuid;
use infra::workflows::Workflows;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::config_fields::{KindView as FieldKindView, ModeView};
use crate::configs::{TAG, require_config_admin};
use crate::etag::{etag, require_if_match};
use crate::extract::{AcceptLanguage, ApiJson, ApiPath, ApiQuery};
use crate::idempotency::{self, Start};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::tenant::TenantTx;

type Labels = BTreeMap<String, String>;

/// When a screen is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = ScreenKind)]
pub enum KindView {
    /// When a ticket is created.
    Creation,
    /// When a ticket is viewed.
    View,
    /// Before a transition runs.
    Transition,
}

impl From<KindView> for Kind {
    fn from(view: KindView) -> Self {
        match view {
            KindView::Creation => Self::Creation,
            KindView::View => Self::View,
            KindView::Transition => Self::Transition,
        }
    }
}

impl From<Kind> for KindView {
    fn from(kind: Kind) -> Self {
        match kind {
            Kind::Creation => Self::Creation,
            Kind::View => Self::View,
            Kind::Transition => Self::Transition,
        }
    }
}

/// A transition that shows a screen.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ScreenUse)]
pub struct UseView {
    transition_id: Uuid,
    workflow_id: Uuid,
    /// The transition, in the language of the request.
    #[schema(example = "Reject")]
    transition: Option<String>,
    #[schema(example = "Standard workflow")]
    workflow: Option<String>,
}

/// A screen of a configuration.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ConfigScreen)]
pub struct ScreenView {
    /// Its identity, the same in every version of the configuration.
    id: Uuid,
    kind: KindView,
    /// The type it is for; `null`: the general creation or view screen, or
    /// a transition screen.
    #[schema(required = true)]
    issue_type_id: Option<Uuid>,
    /// The general creation and view screens stay.
    is_general: bool,
    #[schema(example = "Rejection reason")]
    name: Option<String>,
    #[schema(example = json!({"fr": "Motif du refus", "en": "Rejection reason"}))]
    name_translations: Labels,
    /// Its fields, in their order.
    field_ids: Vec<Uuid>,
    /// The transitions that show it (transition screens).
    used_by: Vec<UseView>,
    #[schema(example = 1)]
    version: i64,
}

/// Where the screens were read.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = ScreenStage)]
pub enum StageView {
    /// The draft, which the projects get once published.
    Draft,
    /// The published version, which the projects use.
    Published,
}

/// The screens of a configuration: creation, view, then transition screens.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ConfigScreenList)]
pub struct ScreenList {
    stage: StageView,
    items: Vec<ScreenView>,
}

/// A new screen: for a type (creation or view), or a transition screen.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = NewConfigScreen,
    example = json!({
        "kind": "transition",
        "nameTranslations": {"fr": "Motif du refus", "en": "Rejection reason"},
        "fieldIds": ["01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9a"]
    })
)]
pub struct NewScreenBody {
    kind: KindView,
    /// Creation and view screens: the type it is for, which has none of
    /// this kind yet. Transition screens: none.
    #[serde(default)]
    issue_type_id: Option<Uuid>,
    /// One name per language (`fr`, `en`), at least one; 60 characters at
    /// most.
    name_translations: Labels,
    /// Fields of the configuration, each once, in their order.
    #[serde(default)]
    field_ids: Vec<Uuid>,
}

/// What a screen is made of; its kind and type stay.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = ConfigScreenSettings)]
pub struct ScreenBody {
    /// One name per language (`fr`, `en`), at least one; 60 characters at
    /// most.
    name_translations: Labels,
    /// Fields of the configuration, each once, in their order.
    #[serde(default)]
    field_ids: Vec<Uuid>,
}

/// A field shown on a resolved screen.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ResolvedScreenField)]
pub struct ShownView {
    field_id: Uuid,
    #[schema(example = "application")]
    key: String,
    #[schema(example = "Application")]
    label: Option<String>,
    kind: FieldKindView,
    /// Visible or required.
    mode: ModeView,
    /// Required at this step but missing from the screen: added at the end.
    added: bool,
}

/// The fields a screen shows for a type and a step, the rules applied.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ResolvedScreen)]
pub struct ResolvedView {
    /// The screen used; `null` for a transition without a screen, whose
    /// fields are then only the ones required at its target status.
    #[schema(required = true)]
    screen_id: Option<Uuid>,
    fields: Vec<ShownView>,
}

/// `?kind=&issueTypeId=&statusId=&transitionId=` of the resolved screen.
#[derive(Debug, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub struct ResolveParams {
    /// `creation`, `view` or `transition`.
    pub kind: String,
    /// The type of the ticket.
    pub issue_type_id: Uuid,
    /// `view`: the status of the ticket.
    pub status_id: Option<Uuid>,
    /// `transition`: the transition requested; its target status is the step.
    pub transition_id: Option<Uuid>,
}

fn conflict_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound).with_detail("no such screen"),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
        WriteConflict::Exists => {
            Problem::new(ErrorCode::ScreenExists).with_errors(vec![FieldError::new("issueTypeId", "taken")])
        }
    }
}

fn removal_problem(refusal: &RemovalRefusal, names: &[String]) -> Problem {
    match refusal {
        RemovalRefusal::General => Problem::new(ErrorCode::ScreenGeneral),
        RemovalRefusal::InUse(_) => Problem::new(ErrorCode::ScreenInUse).with_detail(names.join(", ")),
    }
}

/// The name and the fields of a screen, all errors at once.
fn validate(name: &Labels, field_ids: &[Uuid], known: &[Uuid]) -> Result<(), Problem> {
    let mut v = Violations::new();
    if let Err(errors) = validate_translations("nameTranslations", name, SCREEN_NAME_MAX_CHARS) {
        for error in errors {
            v.add(error.field, error.code);
        }
    }
    let ids: Vec<u128> = field_ids.iter().map(Uuid::as_u128).collect();
    let known: Vec<u128> = known.iter().map(Uuid::as_u128).collect();
    for (i, code) in fields_errors(&ids, &known) {
        v.add(format!("fieldIds[{i}]"), code);
    }
    v.into_result().map_err(Problem::validation)
}

/// The transitions that show each screen, by screen.
fn uses(language: &Language, all: &Workflows) -> BTreeMap<Uuid, Vec<UseView>> {
    let mut uses: BTreeMap<Uuid, Vec<UseView>> = BTreeMap::new();
    for transition in &all.transitions {
        if let Some(screen) = transition.screen_id {
            let workflow = all.workflows.iter().find(|w| w.item_id == transition.workflow_id);
            uses.entry(screen).or_default().push(UseView {
                transition_id: transition.item_id,
                workflow_id: transition.workflow_id,
                transition: language.label(&transition.name_translations),
                workflow: workflow.and_then(|w| language.label(&w.label_translations)),
            });
        }
    }
    uses
}

impl Language {
    fn screen(&self, screen: Screen, uses: &mut BTreeMap<Uuid, Vec<UseView>>) -> ScreenView {
        ScreenView {
            is_general: screen.kind != Kind::Transition && screen.issue_type_id.is_none(),
            used_by: uses.remove(&screen.item_id).unwrap_or_default(),
            id: screen.item_id,
            kind: screen.kind.into(),
            issue_type_id: screen.issue_type_id,
            name: self.label(&screen.name_translations),
            name_translations: screen.name_translations,
            field_ids: screen.field_ids,
            version: screen.version,
        }
    }
}

/// The version the screens of `config` are read from: its draft if it has
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

/// Counts the change of the screens `items` in the draft.
pub(crate) async fn screens_changed(tx: &mut TenantTx, draft: Uuid, items: &[Uuid]) -> Result<(), Problem> {
    for item in items {
        let item = Item {
            kind: ItemKind::Screen,
            id: item.as_u128(),
        };
        configs_repo::record_change(tx, draft, item).await?;
    }
    Ok(())
}

async fn field_ids(tx: &mut TenantTx, version: Uuid) -> Result<Vec<Uuid>, Problem> {
    Ok(infra::fields::list(tx, version)
        .await?
        .into_iter()
        .map(|f| f.item_id)
        .collect())
}

/// The view of the screen `item` of `version`, with its version.
async fn screen_view(
    tx: &mut TenantTx,
    language: &Language,
    version: Uuid,
    item: Uuid,
) -> Result<(ScreenView, i64), Problem> {
    let screen = repo::find(tx, version, item)
        .await?
        .ok_or_else(|| conflict_problem(WriteConflict::NotFound))?;
    let mut uses = uses(language, &infra::workflows::load(tx, version).await?);
    let view = language.screen(screen, &mut uses);
    let version = view.version;
    Ok((view, version))
}

/// `GET /api/v1/project-configs/{id}/screens`.
#[utoipa::path(
    get,
    path = "/project-configs/{id}/screens",
    tag = TAG,
    operation_id = "listScreens",
    summary = "List the screens of a configuration",
    description = "Tenant admins and configuration admins. Creation screens (the general one, then \
                   those of the types), view screens, then transition screens with the transitions \
                   that show them; from the draft if the configuration has one.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the names"),
    ),
    responses(
        (status = 200, description = "Its screens", body = ScreenList),
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
    let screens = repo::list(&mut tx, version).await?;
    let language = Language::of(&tx, &accept);
    let mut uses = uses(&language, &infra::workflows::load(&mut tx, version).await?);
    tx.commit().await?;
    let items = screens
        .into_iter()
        .map(|s| language.screen(s, &mut uses))
        .collect();
    let mut response = ApiJson(ScreenList { stage, items }).into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `POST /api/v1/project-configs/{id}/screens`: idempotent with
/// `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/project-configs/{id}/screens",
    tag = TAG,
    operation_id = "createScreen",
    summary = "Add a screen to the draft of a configuration",
    description = "Tenant admins and configuration admins. A creation or view screen for a type, \
                   which then uses it instead of the general one, or a transition screen.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the names"),
    ),
    request_body = NewScreenBody,
    responses(
        (status = 201, description = "Added", body = ScreenView, headers(
            ("ETag" = String, description = "Version of the screen"),
            ("Location" = String, description = "URL of the screens"),
        )),
        (status = 400, description = "`validation_failed`: the name, `fieldIds[…]` `not_found` or `duplicate`, `issueTypeId` `required`, `not_found` or `not_applicable`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`screen.exists`: the type already has one of this kind; or `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<NewScreenBody>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let path = format!("/api/v1/project-configs/{id}/screens");
    let key = match idempotency::start(&mut tx, &headers, &format!("POST {path}"), &body).await? {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    let draft = draft(&mut tx, id).await?;
    let kind = Kind::from(body.kind);
    match (kind, body.issue_type_id) {
        (Kind::Transition, Some(_)) => {
            return Err(Problem::validation(vec![FieldError::new(
                "issueTypeId",
                "not_applicable",
            )]));
        }
        (Kind::Creation | Kind::View, None) => {
            return Err(Problem::validation(vec![FieldError::new(
                "issueTypeId",
                "required",
            )]));
        }
        (Kind::Creation | Kind::View, Some(issue_type)) => {
            if infra::issue_types::find(&mut tx, draft, issue_type)
                .await?
                .is_none()
            {
                return Err(Problem::validation(vec![FieldError::new(
                    "issueTypeId",
                    "not_found",
                )]));
            }
        }
        (Kind::Transition, None) => {}
    }
    let known = field_ids(&mut tx, draft).await?;
    validate(&body.name_translations, &body.field_ids, &known)?;
    let item = infra::id::new_id();
    repo::insert(
        &mut tx,
        draft,
        item,
        kind,
        body.issue_type_id,
        &body.name_translations,
        &body.field_ids,
    )
    .await?
    .map_err(conflict_problem)?;
    screens_changed(&mut tx, draft, &[item]).await?;
    let language = Language::of(&tx, &accept);
    let (view, version) = screen_view(&mut tx, &language, draft, item).await?;
    let mut response = idempotency::created(&mut tx, key.as_deref(), &view, path, version).await?;
    tx.commit().await?;
    language.headers(&mut response);
    Ok(response)
}

/// `PUT /api/v1/project-configs/{id}/screens/{screenId}`: requires
/// `If-Match`.
#[utoipa::path(
    put,
    path = "/project-configs/{id}/screens/{screenId}",
    tag = TAG,
    operation_id = "updateScreen",
    summary = "Change a screen in the draft of a configuration",
    description = "Tenant admins and configuration admins. Its name and its fields in their order; \
                   its kind and type stay.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("screenId" = Uuid, Path, description = "Id of the screen"),
        ("If-Match" = String, Header, description = "ETag of the screen, or `*`"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the names"),
    ),
    request_body = ScreenBody,
    responses(
        (status = 200, description = "The screen", body = ScreenView, headers(
            ("ETag" = String, description = "New version of the screen"),
        )),
        (status = 400, description = "`validation_failed`: the name, `fieldIds[…]` `not_found` or `duplicate`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration or screen", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The screen changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath((id, item)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<ScreenBody>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let draft = draft(&mut tx, id).await?;
    let known = field_ids(&mut tx, draft).await?;
    validate(&body.name_translations, &body.field_ids, &known)?;
    repo::update(
        &mut tx,
        draft,
        item,
        if_match.versions(),
        &body.name_translations,
        &body.field_ids,
    )
    .await?
    .map_err(conflict_problem)?;
    screens_changed(&mut tx, draft, &[item]).await?;
    let language = Language::of(&tx, &accept);
    let (view, version) = screen_view(&mut tx, &language, draft, item).await?;
    tx.commit().await?;
    let mut response = ApiJson(view).into_response();
    response.headers_mut().insert(header::ETAG, etag(version));
    language.headers(&mut response);
    Ok(response)
}

/// `DELETE /api/v1/project-configs/{id}/screens/{screenId}`: requires
/// `If-Match`.
#[utoipa::path(
    delete,
    path = "/project-configs/{id}/screens/{screenId}",
    tag = TAG,
    operation_id = "deleteScreen",
    summary = "Remove a screen from the draft of a configuration",
    description = "Tenant admins and configuration admins. The general creation and view screens \
                   stay (`screen.general`); a transition screen stays while transitions show it \
                   (`screen.in_use`, naming them in `detail`).",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("screenId" = Uuid, Path, description = "Id of the screen"),
        ("If-Match" = String, Header, description = "ETag of the screen, or `*`"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the transitions named"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration or screen", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`screen.general` or `screen.in_use`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The screen changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath((id, item)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, Problem> {
    require_config_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let draft = draft(&mut tx, id).await?;
    let screen = repo::find(&mut tx, draft, item)
        .await?
        .ok_or_else(|| conflict_problem(WriteConflict::NotFound))?;
    let language = Language::of(&tx, &accept);
    let used_by = uses(&language, &infra::workflows::load(&mut tx, draft).await?)
        .remove(&item)
        .unwrap_or_default();
    let ids: Vec<u128> = used_by.iter().map(|u| u.transition_id.as_u128()).collect();
    if let Some(refusal) = removal_refusal(&screen.domain(), &ids) {
        let names: Vec<String> = used_by
            .iter()
            .map(|u| {
                format!(
                    "{} ({})",
                    u.transition.clone().unwrap_or_default(),
                    u.workflow.clone().unwrap_or_default()
                )
            })
            .collect();
        return Err(removal_problem(&refusal, &names));
    }
    repo::delete(&mut tx, draft, item, if_match.versions())
        .await?
        .map_err(conflict_problem)?;
    screens_changed(&mut tx, draft, &[item]).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/project-configs/{id}/resolved-screen`.
#[utoipa::path(
    get,
    path = "/project-configs/{id}/resolved-screen",
    tag = TAG,
    operation_id = "resolveScreen",
    summary = "Resolve the screen of a type at a step",
    description = "Tenant admins and configuration admins; from the draft if the configuration has \
                   one. The fields a ticket of `issueTypeId` shows: at creation (`kind=creation`), \
                   in a status (`kind=view&statusId=`) or before a transition \
                   (`kind=transition&transitionId=`, at its target status). Its own screen if the \
                   type has one, else the general one; the field rules win: hidden fields go, \
                   required ones missing are added at the end. Tickets (L05) and portal forms \
                   (L13) resolve the published version the same way.",
    params(("id" = Uuid, Path, description = "Id of the configuration"), ResolveParams,
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels")),
    responses(
        (status = 200, description = "The fields shown", body = ResolvedView),
        (status = 400, description = "`invalid_query`: the kind, or its status or transition missing", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration, type, status or transition", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn resolved(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(params): ApiQuery<ResolveParams>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let kind = Kind::parse(&params.kind).ok_or_else(|| {
        Problem::new(ErrorCode::InvalidQuery).with_detail("kind: creation, view or transition")
    })?;
    let (version, _) = read_version(&mut tx, id).await?;
    let issue_type = infra::issue_types::find(&mut tx, version, params.issue_type_id)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such type"))?;
    let all = infra::workflows::load(&mut tx, version).await?;
    let screens = repo::list(&mut tx, version).await?;
    let domain_screens: Vec<domain::screens::Screen> = screens.iter().map(Screen::domain).collect();
    let type_id = issue_type.item_id.as_u128();
    let (screen, step) = match kind {
        Kind::Creation => (pick(&domain_screens, kind, type_id).map(|s| s.id), Step::Creation),
        Kind::View => {
            let status = params.status_id.ok_or_else(|| {
                Problem::new(ErrorCode::InvalidQuery).with_detail("statusId: required for view")
            })?;
            if !all
                .statuses
                .iter()
                .any(|s| s.item_id == status && Some(s.workflow_id) == issue_type.workflow_id)
            {
                return Err(Problem::new(ErrorCode::NotFound).with_detail("no such status for this type"));
            }
            (
                pick(&domain_screens, kind, type_id).map(|s| s.id),
                Step::Status(status.as_u128()),
            )
        }
        Kind::Transition => {
            let transition = params.transition_id.ok_or_else(|| {
                Problem::new(ErrorCode::InvalidQuery).with_detail("transitionId: required for transition")
            })?;
            let found = all
                .transitions
                .iter()
                .find(|t| t.item_id == transition && Some(t.workflow_id) == issue_type.workflow_id)
                .ok_or_else(|| {
                    Problem::new(ErrorCode::NotFound).with_detail("no such transition for this type")
                })?;
            (
                found.screen_id.map(|s| s.as_u128()),
                Step::Status(found.to_status_id.as_u128()),
            )
        }
    };
    let fields = infra::fields::list(&mut tx, version).await?;
    let language = Language::of(&tx, &accept);
    tx.commit().await?;
    let type_ids: Vec<Vec<u128>> = fields
        .iter()
        .map(|f| f.issue_type_ids.iter().map(Uuid::as_u128).collect())
        .collect();
    let rules: Vec<FieldRules<'_>> = fields
        .iter()
        .zip(&type_ids)
        .map(|(f, types)| FieldRules {
            id: f.item_id.as_u128(),
            rules: Rules {
                all_types: f.all_types,
                issue_types: types,
                rules: &f.rules,
            },
        })
        .collect();
    let on_screen: Vec<u128> = screen
        .and_then(|s| domain_screens.iter().find(|d| d.id == s))
        .map(|s| s.fields.clone())
        .unwrap_or_default();
    let shown = resolve(&on_screen, &rules, type_id, step)
        .into_iter()
        .filter_map(|shown| {
            let field = fields.iter().find(|f| f.item_id.as_u128() == shown.field)?;
            Some(ShownView {
                field_id: field.item_id,
                key: field.key.clone(),
                label: language.label(&field.label_translations),
                kind: field.kind.into(),
                mode: if shown.mode == Mode::Required {
                    ModeView::Required
                } else {
                    ModeView::Visible
                },
                added: shown.added,
            })
        })
        .collect();
    let mut response = ApiJson(ResolvedView {
        screen_id: screen.map(Uuid::from_u128),
        fields: shown,
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
    fn refusals_and_conflicts_have_their_own_codes() {
        assert_eq!(
            code(&removal_problem(&RemovalRefusal::General, &[])),
            (409, "screen.general")
        );
        let in_use = removal_problem(
            &RemovalRefusal::InUse(vec![1]),
            &["Reject (Standard workflow)".to_owned()],
        );
        assert_eq!(code(&in_use), (409, "screen.in_use"));
        assert_eq!(in_use.detail.as_deref(), Some("Reject (Standard workflow)"));
        assert_eq!(
            code(&conflict_problem(WriteConflict::Exists)),
            (409, "screen.exists")
        );
        assert_eq!(
            code(&conflict_problem(WriteConflict::VersionMismatch)),
            (412, "precondition_failed")
        );
    }

    #[test]
    fn a_screen_names_fields_of_the_configuration_once() {
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let name: Labels = [("en".to_owned(), "Rejection reason".to_owned())].into();
        assert!(validate(&name, &[a, b], &[a, b]).is_ok());
        let refused = validate(&Labels::new(), &[a, a, Uuid::from_u128(9)], &[a]).unwrap_err();
        assert_eq!(
            refused.errors,
            [
                FieldError::new("nameTranslations", "required"),
                FieldError::new("fieldIds[1]", "duplicate"),
                FieldError::new("fieldIds[2]", "not_found"),
            ]
        );
    }

    #[test]
    fn kinds_map_both_ways() {
        for kind in Kind::ALL {
            assert_eq!(Kind::from(KindView::from(kind)), kind);
        }
    }
}
