//! Fields of a project configuration (specification §5, L04-US03): the
//! system fields (Points, Remaining work) and custom ones, with whether
//! each is hidden, visible or required per issue type and workflow step.
//!
//! Fields are read from the draft of the configuration if it has one, else
//! from its published version. Every change goes to the draft, opened on
//! the first one, and counts once per field in its changes. A field keeps
//! its `id` from one version to the next, and its `key` and `kind` for
//! good. A list value of the published version is deactivated, never
//! removed: tickets keep it. System fields keep their label and options;
//! only their types and rules change.

use std::collections::{BTreeMap, BTreeSet};

use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use domain::configuration::{Item, ItemKind};
use domain::fields::{
    FieldInput, Kind, ListValue, Mode, Options, Rule, Step, TypeSteps, Unit, keep_published, validate_field,
    validate_rules,
};
use domain::validation::FieldError;
use infra::configs as configs_repo;
use infra::fields::{self as repo, Field, FieldFields, WriteConflict};
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

/// The type of a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = FieldKind)]
pub enum KindView {
    ShortText,
    RichText,
    Number,
    Checkbox,
    Date,
    DateTime,
    SingleSelect,
    MultiSelect,
    CascadingSelect,
    User,
    Group,
    Labels,
    Url,
    IssueLink,
}

const KINDS: [(KindView, Kind); 14] = [
    (KindView::ShortText, Kind::ShortText),
    (KindView::RichText, Kind::RichText),
    (KindView::Number, Kind::Number),
    (KindView::Checkbox, Kind::Checkbox),
    (KindView::Date, Kind::Date),
    (KindView::DateTime, Kind::DateTime),
    (KindView::SingleSelect, Kind::SingleSelect),
    (KindView::MultiSelect, Kind::MultiSelect),
    (KindView::CascadingSelect, Kind::CascadingSelect),
    (KindView::User, Kind::User),
    (KindView::Group, Kind::Group),
    (KindView::Labels, Kind::Labels),
    (KindView::Url, Kind::Url),
    (KindView::IssueLink, Kind::IssueLink),
];

impl From<KindView> for Kind {
    fn from(view: KindView) -> Self {
        KINDS
            .iter()
            .find(|(v, _)| *v == view)
            .map_or(Self::ShortText, |(_, kind)| *kind)
    }
}

impl From<Kind> for KindView {
    fn from(kind: Kind) -> Self {
        KINDS
            .iter()
            .find(|(_, k)| *k == kind)
            .map_or(Self::ShortText, |(view, _)| *view)
    }
}

/// The unit of a number field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = FieldUnit)]
pub enum UnitView {
    Hours,
    Days,
    Percent,
}

impl From<UnitView> for Unit {
    fn from(view: UnitView) -> Self {
        match view {
            UnitView::Hours => Self::Hours,
            UnitView::Days => Self::Days,
            UnitView::Percent => Self::Percent,
        }
    }
}

impl From<Unit> for UnitView {
    fn from(unit: Unit) -> Self {
        match unit {
            Unit::Hours => Self::Hours,
            Unit::Days => Self::Days,
            Unit::Percent => Self::Percent,
        }
    }
}

/// Whether a field is hidden, visible or required at a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = FieldMode)]
pub enum ModeView {
    Hidden,
    Visible,
    Required,
}

impl From<ModeView> for Mode {
    fn from(view: ModeView) -> Self {
        match view {
            ModeView::Hidden => Self::Hidden,
            ModeView::Visible => Self::Visible,
            ModeView::Required => Self::Required,
        }
    }
}

impl From<Mode> for ModeView {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::Hidden => Self::Hidden,
            Mode::Visible => Self::Visible,
            Mode::Required => Self::Required,
        }
    }
}

/// A value of a list field.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = FieldValue)]
pub struct ValueView {
    /// Its identity, kept for good: tickets hold it.
    id: Uuid,
    /// In the language of the request, else the tenant's default, else any.
    #[schema(example = "Directory")]
    label: Option<String>,
    #[schema(example = json!({"fr": "Annuaire", "en": "Directory"}))]
    label_translations: BTreeMap<String, String>,
    /// False: deactivated, no longer offered; tickets that hold it keep it.
    active: bool,
    /// Whether the published version has it: then removing it deactivates it.
    published: bool,
    /// Cascading select: the values under it.
    #[schema(no_recursion)]
    children: Vec<ValueView>,
}

/// A value of a list field, as an admin sets it.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = FieldValueInput)]
pub struct ValueBody {
    /// Its identity; none for a new value.
    #[serde(default)]
    id: Option<Uuid>,
    /// One label per language (`fr`, `en`), at least one; 80 characters at
    /// most.
    label_translations: BTreeMap<String, String>,
    /// False: deactivated, no longer offered.
    #[serde(default = "active_by_default")]
    active: bool,
    /// Cascading select only: the values under it (one level).
    #[serde(default)]
    #[schema(no_recursion)]
    children: Vec<ValueBody>,
}

const fn active_by_default() -> bool {
    true
}

impl ValueBody {
    fn value(&self) -> ListValue {
        ListValue {
            id: self.id.unwrap_or_else(infra::id::new_id).as_u128(),
            labels: self.label_translations.clone(),
            active: self.active,
            children: self.children.iter().map(Self::value).collect(),
        }
    }
}

/// The options of a field; each type takes its own (`not_applicable`
/// otherwise).
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = FieldOptions)]
pub struct OptionsView {
    /// Short text: longest text, 1 to 255 characters (255 by default).
    #[serde(skip_serializing_if = "Option::is_none")]
    max_length: Option<u32>,
    /// Number: decimals allowed, 0 to 4 (0 by default).
    #[serde(skip_serializing_if = "Option::is_none")]
    decimals: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unit: Option<UnitView>,
    /// User: several users.
    #[serde(skip_serializing_if = "Option::is_none")]
    multiple: Option<bool>,
    /// Single, multi and cascading select: the values, in their order.
    #[serde(skip_serializing_if = "Option::is_none")]
    values: Option<Vec<ValueView>>,
}

/// The options of a field, as an admin sets them; each type takes its own
/// (`not_applicable` otherwise).
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = FieldOptionsInput)]
pub struct OptionsBody {
    /// Short text: longest text, 1 to 255 characters (255 by default).
    #[serde(default)]
    max_length: Option<u32>,
    /// Number: decimals allowed, 0 to 4 (0 by default).
    #[serde(default)]
    decimals: Option<u8>,
    #[serde(default)]
    min: Option<f64>,
    #[serde(default)]
    max: Option<f64>,
    #[serde(default)]
    unit: Option<UnitView>,
    /// User: several users.
    #[serde(default)]
    multiple: Option<bool>,
    /// Single, multi and cascading select: the values, in their order, at
    /// least one. A value of the published version left out is kept,
    /// deactivated.
    #[serde(default)]
    values: Option<Vec<ValueBody>>,
}

impl OptionsBody {
    fn options(&self) -> Options {
        Options {
            max_length: self.max_length,
            decimals: self.decimals,
            min: self.min,
            max: self.max,
            unit: self.unit.map(Unit::from),
            multiple: self.multiple,
            values: self
                .values
                .as_ref()
                .map(|values| values.iter().map(ValueBody::value).collect()),
        }
    }
}

/// One cell of the rules grid that is not Visible: a cell nobody set is
/// Visible.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = FieldRule)]
pub struct RuleView {
    /// A type the field applies to.
    issue_type_id: Uuid,
    /// A status of the type's workflow; null: at creation.
    #[schema(required = true)]
    #[serde(default)]
    status_id: Option<Uuid>,
    mode: ModeView,
}

impl RuleView {
    fn of(rule: &Rule) -> Self {
        Self {
            issue_type_id: Uuid::from_u128(rule.issue_type),
            status_id: match rule.step {
                Step::Creation => None,
                Step::Status(status) => Some(Uuid::from_u128(status)),
            },
            mode: rule.mode.into(),
        }
    }

    fn rule(&self) -> Rule {
        Rule {
            issue_type: self.issue_type_id.as_u128(),
            step: self
                .status_id
                .map_or(Step::Creation, |s| Step::Status(s.as_u128())),
            mode: self.mode.into(),
        }
    }
}

/// A field of a configuration.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ConfigField)]
pub struct FieldView {
    /// Its identity, the same in every version of the configuration.
    id: Uuid,
    /// Its stable identifier, set when it is added.
    #[schema(example = "application")]
    key: String,
    /// In the language of the request, else the tenant's default, else any.
    #[schema(example = "Application")]
    label: Option<String>,
    #[schema(example = json!({"fr": "Application", "en": "Application"}))]
    label_translations: BTreeMap<String, String>,
    /// Set when it is added.
    kind: KindView,
    options: OptionsView,
    /// A system field cannot be removed; only its types and rules change.
    is_system: bool,
    /// Applies to every type, including the ones added later.
    all_types: bool,
    /// The types it applies to, when not all.
    issue_type_ids: Vec<Uuid>,
    /// Its cells that are not Visible.
    rules: Vec<RuleView>,
    #[schema(example = 1)]
    version: i64,
}

/// Where the fields were read.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = FieldStage)]
pub enum StageView {
    /// The draft, which the projects get once published.
    Draft,
    /// The published version, which the projects use.
    Published,
}

/// The fields of a configuration, in their order.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ConfigFieldList)]
pub struct FieldList {
    stage: StageView,
    items: Vec<FieldView>,
}

/// A new custom field.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = NewConfigField,
    example = json!({
        "key": "application",
        "labelTranslations": {"fr": "Application", "en": "Application"},
        "kind": "single_select",
        "options": {"values": [
            {"labelTranslations": {"fr": "Annuaire", "en": "Directory"}},
            {"labelTranslations": {"fr": "Messagerie", "en": "Mail"}}
        ]},
        "issueTypeIds": ["01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9a"],
        "rules": [{"issueTypeId": "01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9a", "statusId": null, "mode": "required"}]
    })
)]
pub struct NewFieldBody {
    /// A lowercase letter, then up to 39 lowercase letters, digits or
    /// underscores; unique in the configuration, never changed.
    key: String,
    /// Never changed.
    kind: KindView,
    /// One label per language (`fr`, `en`), at least one; 80 characters at
    /// most.
    label_translations: BTreeMap<String, String>,
    #[serde(default)]
    options: OptionsBody,
    /// Every type, including the ones added later.
    #[serde(default)]
    all_types: bool,
    /// The types it applies to, when not all.
    #[serde(default)]
    issue_type_ids: Vec<Uuid>,
    /// The cells that are not Visible; Visible ones may be sent, and are
    /// not kept. A status must belong to the workflow of the type.
    #[serde(default)]
    rules: Vec<RuleView>,
}

impl NewFieldBody {
    fn settings(&self) -> FieldBody {
        FieldBody {
            label_translations: self.label_translations.clone(),
            options: self.options.clone(),
            all_types: self.all_types,
            issue_type_ids: self.issue_type_ids.clone(),
            rules: self.rules.clone(),
        }
    }
}

/// What a field is made of; its `key` and `kind` stay.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = ConfigFieldSettings,
    example = json!({
        "labelTranslations": {"fr": "Application", "en": "Application"},
        "options": {"values": [
            {"id": "01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b01", "labelTranslations": {"fr": "Annuaire", "en": "Directory"}},
            {"labelTranslations": {"fr": "ERP", "en": "ERP"}}
        ]},
        "allTypes": false,
        "issueTypeIds": ["01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9a"],
        "rules": []
    })
)]
pub struct FieldBody {
    /// One label per language (`fr`, `en`), at least one; 80 characters at
    /// most. A system field keeps its own.
    label_translations: BTreeMap<String, String>,
    /// A system field keeps its own.
    #[serde(default)]
    options: OptionsBody,
    /// Every type, including the ones added later.
    #[serde(default)]
    all_types: bool,
    /// The types it applies to, when not all.
    #[serde(default)]
    issue_type_ids: Vec<Uuid>,
    /// The cells that are not Visible; Visible ones may be sent, and are
    /// not kept. A status must belong to the workflow of the type.
    #[serde(default)]
    rules: Vec<RuleView>,
}

impl FieldBody {
    /// Its types, each once, in their order.
    fn issue_type_ids(&self) -> Vec<Uuid> {
        let mut seen = BTreeSet::new();
        self.issue_type_ids
            .iter()
            .copied()
            .filter(|id| seen.insert(*id))
            .collect()
    }

    fn rules(&self) -> Vec<Rule> {
        self.rules.iter().map(RuleView::rule).collect()
    }
}

fn conflict_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound).with_detail("no such field"),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
        WriteConflict::KeyTaken => {
            Problem::new(ErrorCode::FieldKeyTaken).with_errors(vec![FieldError::new("key", "taken")])
        }
    }
}

impl Language {
    fn value_view(&self, value: &ListValue, published: &BTreeSet<u128>) -> ValueView {
        ValueView {
            id: Uuid::from_u128(value.id),
            label: self.label(&value.labels),
            label_translations: value.labels.clone(),
            active: value.active,
            published: published.contains(&value.id),
            children: value
                .children
                .iter()
                .map(|child| self.value_view(child, published))
                .collect(),
        }
    }

    fn field_view(&self, field: Field, published: &BTreeSet<u128>) -> FieldView {
        let options = &field.options;
        FieldView {
            id: field.item_id,
            label: self.label(&field.label_translations),
            options: OptionsView {
                max_length: options.max_length,
                decimals: options.decimals,
                min: options.min,
                max: options.max,
                unit: options.unit.map(UnitView::from),
                multiple: options.multiple,
                values: options
                    .values
                    .as_ref()
                    .map(|values| values.iter().map(|v| self.value_view(v, published)).collect()),
            },
            rules: field.rules.iter().map(RuleView::of).collect(),
            key: field.key,
            label_translations: field.label_translations,
            kind: field.kind.into(),
            is_system: field.is_system,
            all_types: field.all_types,
            issue_type_ids: field.issue_type_ids,
            version: field.version,
        }
    }
}

/// The versions of `config`: the one the fields are read from (its draft
/// if it has one), how it stands, and the published one.
async fn versions(tx: &mut TenantTx, config: Uuid) -> Result<(Uuid, StageView, Uuid), Problem> {
    let config = configs_repo::find(tx, config)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such configuration"))?;
    let published = config.published.id;
    Ok(match config.draft {
        Some(draft) => (draft.id, StageView::Draft, published),
        None => (published, StageView::Published, published),
    })
}

/// The list values of the published version `published`: removing one of
/// them deactivates it.
async fn published_values(tx: &mut TenantTx, published: Uuid) -> Result<BTreeSet<u128>, Problem> {
    Ok(repo::list(tx, published)
        .await?
        .iter()
        .flat_map(|f| {
            f.options
                .all_values()
                .into_iter()
                .map(|v| v.id)
                .collect::<Vec<_>>()
        })
        .collect())
}

/// The draft of `config`, opened if needed, and its published version.
async fn draft(tx: &mut TenantTx, config: Uuid) -> Result<(Uuid, Uuid), Problem> {
    let actor = tx.account_id();
    let opened = configs_repo::open_draft(tx, config, Some(actor))
        .await?
        .map_err(|_| Problem::new(ErrorCode::NotFound).with_detail("no such configuration"))?;
    let draft = opened
        .draft
        .map(|d| d.id)
        .ok_or_else(|| Problem::new(ErrorCode::ConfigNoDraft))?;
    Ok((draft, opened.published.id))
}

/// The types of the draft, each with the statuses of its workflow.
async fn type_steps(tx: &mut TenantTx, draft: Uuid) -> Result<Vec<TypeSteps>, Problem> {
    let types = infra::issue_types::list(tx, draft).await?;
    let workflows = infra::workflows::load(tx, draft).await?;
    Ok(types
        .iter()
        .map(|t| TypeSteps {
            id: t.item_id.as_u128(),
            statuses: t
                .workflow_id
                .map(|w| workflows.statuses_of(w).map(|s| s.item_id.as_u128()).collect())
                .unwrap_or_default(),
        })
        .collect())
}

/// The field as it will be stored, checked: its options (with the
/// published values it leaves out, deactivated) and its rules.
struct Checked {
    options: Options,
    issue_type_ids: Vec<Uuid>,
    rules: Vec<Rule>,
}

async fn check(
    tx: &mut TenantTx,
    draft: Uuid,
    published: Uuid,
    item: Uuid,
    key: Option<&str>,
    kind: Kind,
    body: &FieldBody,
) -> Result<Checked, Problem> {
    let mut options = body.options.options();
    let issue_type_ids = body.issue_type_ids();
    let ids: Vec<u128> = issue_type_ids.iter().map(Uuid::as_u128).collect();
    let mut errors = Vec::new();
    if let Err(found) = validate_field(&FieldInput {
        key,
        label_translations: &body.label_translations,
        kind,
        options: &options,
    }) {
        errors.extend(found);
    }
    let types = type_steps(tx, draft).await?;
    let rules = match validate_rules(body.all_types, &ids, &body.rules(), &types) {
        Ok(rules) => rules,
        Err(found) => {
            errors.extend(found);
            Vec::new()
        }
    };
    if !errors.is_empty() {
        return Err(Problem::validation(errors));
    }
    if let Some(values) = options.values.take() {
        let published = repo::find(tx, published, item).await?;
        let published_values = published.and_then(|f| f.options.values).unwrap_or_default();
        options.values = Some(keep_published(values, &published_values));
    }
    Ok(Checked {
        options,
        issue_type_ids,
        rules,
    })
}

/// Counts the change of the field `item` in the draft.
async fn changed(tx: &mut TenantTx, draft: Uuid, item: Uuid) -> Result<(), Problem> {
    let item = Item {
        kind: ItemKind::Field,
        id: item.as_u128(),
    };
    configs_repo::record_change(tx, draft, item).await?;
    Ok(())
}

/// Counts the changes of the fields `items` in the draft.
pub(crate) async fn fields_changed(tx: &mut TenantTx, draft: Uuid, items: &[Uuid]) -> Result<(), Problem> {
    for item in items {
        changed(tx, draft, *item).await?;
    }
    Ok(())
}

/// The view of the field `item` of `version`, with its ETag.
async fn field_response(
    tx: &mut TenantTx,
    language: &Language,
    version: Uuid,
    published: Uuid,
    item: Uuid,
) -> Result<(FieldView, i64), Problem> {
    let field = repo::find(tx, version, item)
        .await?
        .ok_or_else(|| conflict_problem(WriteConflict::NotFound))?;
    let published = published_values(tx, published).await?;
    let view = language.field_view(field, &published);
    let version = view.version;
    Ok((view, version))
}

fn with_etag(language: &Language, view: FieldView, version: i64) -> Response {
    let mut response = ApiJson(view).into_response();
    response.headers_mut().insert(header::ETAG, etag(version));
    language.headers(&mut response);
    response
}

/// `GET /api/v1/project-configs/{id}/fields`.
#[utoipa::path(
    get,
    path = "/project-configs/{id}/fields",
    tag = TAG,
    operation_id = "listFields",
    summary = "List the fields of a configuration",
    description = "Tenant admins and configuration admins. System fields and custom fields, in \
                   their order, with their rules; from the draft if the configuration has one \
                   (`stage: draft`), else from its published version.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `label`"),
    ),
    responses(
        (status = 200, description = "Its fields", body = FieldList),
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
    let (version, stage, published) = versions(&mut tx, id).await?;
    let fields = repo::list(&mut tx, version).await?;
    let published = published_values(&mut tx, published).await?;
    let language = Language::of(&tx, &accept);
    tx.commit().await?;
    let items = fields
        .into_iter()
        .map(|f| language.field_view(f, &published))
        .collect();
    let mut response = ApiJson(FieldList { stage, items }).into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `GET /api/v1/project-configs/{id}/fields/{fieldId}`.
#[utoipa::path(
    get,
    path = "/project-configs/{id}/fields/{fieldId}",
    tag = TAG,
    operation_id = "getField",
    summary = "Read a field",
    description = "Tenant admins and configuration admins. From the draft if the configuration has \
                   one, else from its published version.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("fieldId" = Uuid, Path, description = "Id of the field"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `label`"),
    ),
    responses(
        (status = 200, description = "The field", body = FieldView, headers(
            ("ETag" = String, description = "Version of the field"),
        )),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration or field", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn read(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath((id, item)): ApiPath<(Uuid, Uuid)>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let (version, _, published) = versions(&mut tx, id).await?;
    let language = Language::of(&tx, &accept);
    let (view, version) = field_response(&mut tx, &language, version, published, item).await?;
    tx.commit().await?;
    Ok(with_etag(&language, view, version))
}

/// `POST /api/v1/project-configs/{id}/fields`: idempotent with
/// `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/project-configs/{id}/fields",
    tag = TAG,
    operation_id = "createField",
    summary = "Add a custom field to the draft of a configuration",
    description = "Tenant admins and configuration admins. It goes last, in the draft, opened if \
                   needed, with its rules; the projects get it once the draft is published.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `label`"),
    ),
    request_body = NewFieldBody,
    responses(
        (status = 201, description = "Added", body = FieldView, headers(
            ("ETag" = String, description = "Version of the field"),
            ("Location" = String, description = "URL of the field"),
        )),
        (status = 400, description = "`validation_failed`: fields, options (`not_applicable` to the type), `issueTypeIds` or `rules` (`not_found`, `not_applicable`, `duplicate`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`config_field.key_taken`, or `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<NewFieldBody>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let path = format!("/api/v1/project-configs/{id}/fields");
    let key = match idempotency::start(&mut tx, &headers, &format!("POST {path}"), &body).await? {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    let (draft, published) = draft(&mut tx, id).await?;
    let item = infra::id::new_id();
    let kind = Kind::from(body.kind);
    let settings = body.settings();
    let checked = check(&mut tx, draft, published, item, Some(&body.key), kind, &settings).await?;
    let fields = FieldFields {
        label_translations: &settings.label_translations,
        options: &checked.options,
        all_types: settings.all_types,
        issue_type_ids: &checked.issue_type_ids,
    };
    repo::insert(&mut tx, draft, item, &body.key, kind, &fields)
        .await?
        .map_err(conflict_problem)?;
    repo::set_rules(&mut tx, draft, item, &checked.rules).await?;
    changed(&mut tx, draft, item).await?;
    // At the end of the general creation and view screens.
    let screens = infra::screens::append_field(&mut tx, draft, item).await?;
    crate::screens::screens_changed(&mut tx, draft, &screens).await?;
    let language = Language::of(&tx, &accept);
    let (view, version) = field_response(&mut tx, &language, draft, published, item).await?;
    let mut response =
        idempotency::created(&mut tx, key.as_deref(), &view, format!("{path}/{item}"), version).await?;
    tx.commit().await?;
    language.headers(&mut response);
    Ok(response)
}

/// `PUT /api/v1/project-configs/{id}/fields/{fieldId}`: requires
/// `If-Match`.
#[utoipa::path(
    put,
    path = "/project-configs/{id}/fields/{fieldId}",
    tag = TAG,
    operation_id = "updateField",
    summary = "Change a field in the draft of a configuration",
    description = "Tenant admins and configuration admins. In the draft, opened if needed, with its \
                   rules; its `key` and `kind` stay. A list value of the published version left out \
                   is kept, deactivated. A system field keeps its label and options \
                   (`config_field.system`): only its types and rules change.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("fieldId" = Uuid, Path, description = "Id of the field"),
        ("If-Match" = String, Header, description = "ETag of the field, or `*`"),
        ("Accept-Language" = Option<String>, Header, description = "Language of `label`"),
    ),
    request_body = FieldBody,
    responses(
        (status = 200, description = "The field", body = FieldView, headers(
            ("ETag" = String, description = "New version of the field"),
        )),
        (status = 400, description = "`validation_failed`: fields, options (`not_applicable` to the type), `issueTypeIds` or `rules` (`not_found`, `not_applicable`, `duplicate`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration or field", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`config_field.system`: a system field with another label or options", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The field changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath((id, item)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<FieldBody>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let (draft, published) = draft(&mut tx, id).await?;
    let current = repo::find(&mut tx, draft, item)
        .await?
        .ok_or_else(|| conflict_problem(WriteConflict::NotFound))?;
    let mut checked = check(&mut tx, draft, published, item, None, current.kind, &body).await?;
    if current.is_system {
        if trimmed(&body.label_translations) != current.label_translations
            || checked.options != current.options
        {
            return Err(Problem::new(ErrorCode::FieldSystem));
        }
        checked.options = current.options.clone();
    }
    let fields = FieldFields {
        label_translations: &body.label_translations,
        options: &checked.options,
        all_types: body.all_types,
        issue_type_ids: &checked.issue_type_ids,
    };
    repo::update(&mut tx, draft, item, if_match.versions(), &fields)
        .await?
        .map_err(conflict_problem)?;
    repo::set_rules(&mut tx, draft, item, &checked.rules).await?;
    changed(&mut tx, draft, item).await?;
    let language = Language::of(&tx, &accept);
    let (view, version) = field_response(&mut tx, &language, draft, published, item).await?;
    tx.commit().await?;
    Ok(with_etag(&language, view, version))
}

/// Labels trimmed, empty ones dropped, as they are stored.
fn trimmed(labels: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    labels
        .iter()
        .map(|(locale, label)| (locale.clone(), label.trim().to_owned()))
        .filter(|(_, label)| !label.is_empty())
        .collect()
}

/// `DELETE /api/v1/project-configs/{id}/fields/{fieldId}`: requires
/// `If-Match`.
#[utoipa::path(
    delete,
    path = "/project-configs/{id}/fields/{fieldId}",
    tag = TAG,
    operation_id = "deleteField",
    summary = "Remove a custom field from the draft of a configuration",
    description = "Tenant admins and configuration admins. From the draft, opened if needed, with \
                   its rules, and from the validators and post-functions of the transitions. System \
                   fields stay (`config_field.system`).",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("fieldId" = Uuid, Path, description = "Id of the field"),
        ("If-Match" = String, Header, description = "ETag of the field, or `*`"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration or field", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`config_field.system`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The field changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
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
    let (draft, _) = draft(&mut tx, id).await?;
    let current = repo::find(&mut tx, draft, item)
        .await?
        .ok_or_else(|| conflict_problem(WriteConflict::NotFound))?;
    if current.is_system {
        return Err(Problem::new(ErrorCode::FieldSystem));
    }
    repo::delete(&mut tx, draft, item, if_match.versions())
        .await?
        .map_err(conflict_problem)?;
    changed(&mut tx, draft, item).await?;
    // The transition rules that named it no longer do.
    for workflow in infra::workflows::forget_field(&mut tx, draft, item).await? {
        let workflow = Item {
            kind: ItemKind::Workflow,
            id: workflow.as_u128(),
        };
        configs_repo::record_change(&mut tx, draft, workflow).await?;
    }
    // And off the screens.
    let screens = infra::screens::forget_field(&mut tx, draft, item).await?;
    crate::screens::screens_changed(&mut tx, draft, &screens).await?;
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
    fn conflicts_have_their_own_codes() {
        let taken = conflict_problem(WriteConflict::KeyTaken);
        assert_eq!(code(&taken), (409, "config_field.key_taken"));
        assert_eq!(taken.errors, vec![FieldError::new("key", "taken")]);
        assert_eq!(
            code(&conflict_problem(WriteConflict::VersionMismatch)),
            (412, "precondition_failed")
        );
        assert_eq!(
            code(&conflict_problem(WriteConflict::NotFound)),
            (404, "not_found")
        );
        assert_eq!(
            code(&Problem::new(ErrorCode::FieldSystem)),
            (409, "config_field.system")
        );
    }

    #[test]
    fn kinds_units_and_modes_map_both_ways() {
        for kind in Kind::ALL {
            assert_eq!(Kind::from(KindView::from(kind)), kind);
        }
        for unit in Unit::ALL {
            assert_eq!(Unit::from(UnitView::from(unit)), unit);
        }
        for mode in Mode::ALL {
            assert_eq!(Mode::from(ModeView::from(mode)), mode);
        }
        let json = serde_json::to_value(KindView::CascadingSelect).unwrap();
        assert_eq!(json, Kind::CascadingSelect.as_str());
    }

    #[test]
    fn new_values_get_an_identity_and_keep_theirs() {
        let kept = Uuid::from_u128(7);
        let body: ValueBody = serde_json::from_value(serde_json::json!({
            "id": kept,
            "labelTranslations": {"fr": "ERP"},
            "children": [{"labelTranslations": {"fr": "Finance"}}]
        }))
        .unwrap();
        let value = body.value();
        assert_eq!(value.id, 7);
        assert!(value.active, "active unless said otherwise");
        assert_ne!(value.children[0].id, 0);
    }

    #[test]
    fn a_field_lists_each_type_once_and_its_rules() {
        let body: FieldBody = serde_json::from_value(serde_json::json!({
            "labelTranslations": {"en": "Application"},
            "issueTypeIds": [Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(1)],
            "rules": [
                {"issueTypeId": Uuid::from_u128(1), "statusId": null, "mode": "required"},
                {"issueTypeId": Uuid::from_u128(2), "statusId": Uuid::from_u128(9), "mode": "hidden"}
            ]
        }))
        .unwrap();
        assert_eq!(body.issue_type_ids(), [Uuid::from_u128(1), Uuid::from_u128(2)]);
        assert_eq!(
            body.rules(),
            [
                Rule {
                    issue_type: 1,
                    step: Step::Creation,
                    mode: Mode::Required
                },
                Rule {
                    issue_type: 2,
                    step: Step::Status(9),
                    mode: Mode::Hidden
                },
            ]
        );
        assert_eq!(RuleView::of(&body.rules()[1]).status_id, Some(Uuid::from_u128(9)));
    }
}
