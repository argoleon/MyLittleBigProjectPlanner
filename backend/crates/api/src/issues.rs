//! Tickets (specification §5, §15, L05-US01): created in a project with the
//! creation screen of its type, read by id or key.
//!
//! A ticket follows the published version of its project's configuration:
//! its type, the initial status of the type's workflow, the custom fields
//! and their rules at creation (`domain::issues::validate_new`). It gets
//! the next key of its project (`PLAT-12`), unique in the tenant; creation
//! writes `issue.created` to the outbox in the same transaction. A ticket
//! of a project the member does not see answers 404, like one of another
//! tenant.

use std::collections::{BTreeMap, BTreeSet};

use axum::extract::State;
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, NaiveDate, Utc};
use domain::fields::{Kind, ListValue, Mode, Rules, Step, Value};
use domain::issue_types::TypeNode;
use domain::issues::{FieldDef, NewIssue, Parent, Priority, parse_key, validate_new};
use domain::permissions::Permission;
use domain::screens::{FieldRules, Kind as ScreenKind, pick, resolve};
use domain::validation::FieldError;
use infra::fields::Field;
use infra::issue_types::IssueType;
use infra::issues::{self as repo, Issue};
use infra::nodes::Node;
use infra::uuid::Uuid;
use infra::workflows::Workflows;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::config_fields::{KindView, ModeView};
use crate::etag::etag;
use crate::extract::{AcceptLanguage, ApiJson, ApiPath, ApiQuery};
use crate::idempotency::{self, Start};
use crate::nodes::{ensure_changeable, is_tenant_admin, visible_node};
use crate::pagination::{PageParams, encode_cursor};
use crate::permissions;
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::tenant::{AppState, TenantTx};

pub const TAG: &str = "issues";

type Labels = BTreeMap<String, String>;

/// The priority of a ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = IssuePriority)]
pub enum PriorityView {
    Highest,
    High,
    Medium,
    Low,
    Lowest,
}

impl From<PriorityView> for Priority {
    fn from(view: PriorityView) -> Self {
        match view {
            PriorityView::Highest => Self::Highest,
            PriorityView::High => Self::High,
            PriorityView::Medium => Self::Medium,
            PriorityView::Low => Self::Low,
            PriorityView::Lowest => Self::Lowest,
        }
    }
}

impl From<Priority> for PriorityView {
    fn from(priority: Priority) -> Self {
        match priority {
            Priority::Highest => Self::Highest,
            Priority::High => Self::High,
            Priority::Medium => Self::Medium,
            Priority::Low => Self::Low,
            Priority::Lowest => Self::Lowest,
        }
    }
}

/// A project, by its key and name.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueProject)]
pub struct ProjectView {
    id: Uuid,
    #[schema(example = "PLAT")]
    key: String,
    #[schema(example = "Identity platform")]
    name: String,
}

/// The type of a ticket.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueTypeRef)]
pub struct TypeView {
    id: Uuid,
    key: String,
    #[schema(example = "Story")]
    label: Option<String>,
    label_translations: Labels,
    icon: String,
    /// One of the type color tokens: `epic`, `story`, `task`, `bug`.
    color: String,
    is_subtask: bool,
    /// The types it may sit under.
    parent_ids: Vec<Uuid>,
}

/// The status of a ticket.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueStatusRef)]
pub struct StatusView {
    id: Uuid,
    key: String,
    #[schema(example = "In progress")]
    label: Option<String>,
    category: crate::workflows::CategoryView,
}

/// A person of a ticket.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssuePerson)]
pub struct PersonView {
    id: Uuid,
    #[schema(example = "Camille Martin")]
    name: String,
}

/// The value of a custom field on a ticket.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueFieldValue)]
pub struct FieldValueView {
    field_id: Uuid,
    key: String,
    #[schema(example = "Application")]
    label: Option<String>,
    kind: KindView,
    /// As sent: text, number, boolean, identifiers…
    value: serde_json::Value,
    /// What to show, in the language of the request: the labels of list
    /// values, the names of users and groups, the keys of tickets (of
    /// projects the member sees); `null` when the value shows as is.
    #[schema(required = true)]
    display: Option<String>,
    /// The members, groups or tickets the value names, each with its name
    /// (a ticket: its key), in the order of the value; empty for other
    /// kinds. Those that no longer exist, or tickets of projects the
    /// member does not see, are left out.
    references: Vec<PersonView>,
}

/// Another ticket, in a few words.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueRef)]
pub struct IssueRefView {
    id: Uuid,
    #[schema(example = "PLAT-100")]
    key: String,
    summary: String,
}

/// A ticket.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Issue)]
pub struct IssueView {
    id: Uuid,
    #[schema(example = "PLAT-142")]
    key: String,
    project: ProjectView,
    #[serde(rename = "type")]
    issue_type: TypeView,
    status: StatusView,
    #[schema(example = "Migrate the authentication to OIDC")]
    summary: String,
    /// The rich text document, as the editor reads it; `null`: none.
    #[schema(required = true)]
    description: Option<serde_json::Value>,
    /// The description as sanitized HTML, to show.
    #[schema(required = true)]
    description_html: Option<String>,
    priority: PriorityView,
    #[schema(required = true)]
    reporter: Option<PersonView>,
    #[schema(required = true)]
    assignee: Option<PersonView>,
    #[schema(required = true)]
    start_date: Option<NaiveDate>,
    #[schema(required = true)]
    due_date: Option<NaiveDate>,
    /// The custom fields that hold a value, in the order of the
    /// configuration.
    fields: Vec<FieldValueView>,
    /// `expand=parent`: the ticket it sits under.
    #[serde(skip_serializing_if = "Option::is_none")]
    parent: Option<IssueRefView>,
    /// The ticket it sits under; `null`: at the top.
    #[schema(required = true)]
    parent_id: Option<Uuid>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[schema(example = 1)]
    version: i64,
    /// `expand=children`: its sub-tasks, oldest first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) children: Option<Vec<crate::issue_links::RelatedView>>,
    /// `expand=links`: its links to tickets the member sees, oldest first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) links: Option<Vec<crate::issue_links::LinkView>>,
}

/// A ticket in a list.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueSummary)]
pub struct SummaryView {
    id: Uuid,
    key: String,
    summary: String,
    project: ProjectView,
    #[serde(rename = "type")]
    issue_type: TypeView,
    status: StatusView,
    priority: PriorityView,
    #[schema(required = true)]
    assignee: Option<PersonView>,
    updated_at: DateTime<Utc>,
}

/// A page of tickets.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssuePage)]
pub struct IssuePage {
    items: Vec<SummaryView>,
    /// Cursor of the next page, `null` on the last page.
    #[schema(required = true)]
    next_cursor: Option<String>,
}

/// A new ticket.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = NewIssue,
    example = json!({
        "typeId": "01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9a",
        "summary": "Migrate the authentication to OIDC",
        "description": {"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Replace the local accounts."}]}]},
        "priority": "high",
        "fields": {}
    })
)]
pub struct NewIssueBody {
    /// A type of the project's configuration.
    type_id: Uuid,
    /// 1 to 255 characters.
    summary: String,
    /// A rich text document (Tiptap JSON): paragraphs, headings, lists,
    /// quotes, code, links (http, https, mailto) and bold, italic, strike,
    /// code marks.
    #[serde(default)]
    description: Option<serde_json::Value>,
    /// `medium` by default.
    #[serde(default)]
    priority: Option<PriorityView>,
    /// The ticket it sits under, in the same project; required for a
    /// sub-task.
    #[serde(default)]
    parent_id: Option<Uuid>,
    /// An active member of the organization.
    #[serde(default)]
    assignee_id: Option<Uuid>,
    #[serde(default)]
    start_date: Option<NaiveDate>,
    #[serde(default)]
    due_date: Option<NaiveDate>,
    /// Custom field values, by field id, as the field takes them.
    #[serde(default)]
    fields: BTreeMap<Uuid, serde_json::Value>,
}

/// `?expand=` of a ticket.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ReadParams {
    /// Comma-separated: `parent` (the ticket it sits under, in a few
    /// words), `children` (its sub-tasks), `links` (its links to tickets
    /// the member sees).
    pub expand: Option<String>,
}

/// `?typeId=` of the creation form.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub struct FormParams {
    /// The type chosen; by default the first that is no sub-task.
    pub type_id: Option<Uuid>,
}

/// Who `GET …/people` searches.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = PeopleKind)]
pub enum PeopleKind {
    /// Active members.
    #[default]
    Member,
    /// Groups, for group fields.
    Group,
}

/// `?search=&kind=` of the people.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PeopleParams {
    /// Part of the name (or email of a member), any case.
    pub search: Option<String>,
    /// `member` (default) or `group`.
    #[serde(default)]
    #[param(inline)]
    pub kind: PeopleKind,
}

/// `?limit=&cursor=` of a list.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListParams {
    /// Page size, from 1 to 200 (default 50).
    #[param(minimum = 1, maximum = 200)]
    pub limit: Option<usize>,
    /// `nextCursor` of the previous page.
    pub cursor: Option<String>,
}

/// A list value offered by a field, active only.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueFormValue)]
pub struct FormValueView {
    id: Uuid,
    label: Option<String>,
    #[schema(no_recursion)]
    children: Vec<FormValueView>,
}

/// A field of the creation screen.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueFormField)]
pub struct FormFieldView {
    field_id: Uuid,
    key: String,
    label: Option<String>,
    kind: KindView,
    /// Visible or required.
    mode: ModeView,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_length: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decimals: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max: Option<f64>,
    /// `hours`, `days` or `percent`.
    #[serde(skip_serializing_if = "Option::is_none")]
    unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    multiple: Option<bool>,
    /// The values to pick from, for a list field.
    #[serde(skip_serializing_if = "Option::is_none")]
    values: Option<Vec<FormValueView>>,
}

/// What creating a ticket in a project offers.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueForm)]
pub struct FormView {
    project: ProjectView,
    /// The types of the project's configuration, in their order.
    types: Vec<TypeView>,
    /// The type the fields are for.
    type_id: Uuid,
    /// The fields of its creation screen, the rules applied.
    fields: Vec<FormFieldView>,
}

/// The published configuration of a project, as tickets read it.
pub(crate) struct Content {
    pub(crate) types: Vec<IssueType>,
    pub(crate) workflows: Workflows,
    pub(crate) fields: Vec<Field>,
    pub(crate) screens: Vec<infra::screens::Screen>,
}

impl Content {
    pub(crate) async fn of(tx: &mut TenantTx, project: Uuid) -> Result<Self, Problem> {
        let config = infra::configs::of_project(tx, project)
            .await?
            .ok_or(Problem::new(ErrorCode::NotFound))?;
        Self::of_config(tx, config).await
    }

    /// The published content of configuration `config`.
    pub(crate) async fn of_config(tx: &mut TenantTx, config: Uuid) -> Result<Self, Problem> {
        let version = infra::configs::find(tx, config)
            .await?
            .ok_or(Problem::new(ErrorCode::NotFound))?
            .published
            .id;
        Ok(Self {
            types: infra::issue_types::list(tx, version).await?,
            workflows: infra::workflows::load(tx, version).await?,
            fields: infra::fields::list(tx, version).await?,
            screens: infra::screens::list(tx, version).await?,
        })
    }

    pub(crate) fn type_nodes(&self) -> Vec<TypeNode> {
        self.types.iter().map(IssueType::node).collect()
    }
}

/// The fields with their rules, borrowing `fields` and `types`.
pub(crate) fn field_defs<'a>(fields: &'a [Field], type_ids: &'a [Vec<u128>]) -> Vec<FieldDef<'a>> {
    fields
        .iter()
        .zip(type_ids)
        .map(|(f, types)| FieldDef {
            id: f.item_id.as_u128(),
            kind: f.kind,
            options: &f.options,
            rules: Rules {
                all_types: f.all_types,
                issue_types: types,
                rules: &f.rules,
            },
        })
        .collect()
}

pub(crate) fn type_ids_of(fields: &[Field]) -> Vec<Vec<u128>> {
    fields
        .iter()
        .map(|f| f.issue_type_ids.iter().map(Uuid::as_u128).collect())
        .collect()
}

pub(crate) fn project_view(node: &Node) -> ProjectView {
    project_ref(node.id, node.key.clone(), node.name.clone())
}

pub(crate) const fn project_ref(id: Uuid, key: String, name: String) -> ProjectView {
    ProjectView { id, key, name }
}

impl Language {
    pub(crate) fn type_view(&self, t: &IssueType) -> TypeView {
        TypeView {
            id: t.item_id,
            key: t.key.clone(),
            label: self.label(&t.label_translations),
            label_translations: t.label_translations.clone(),
            icon: t.icon.clone(),
            color: t.color.clone(),
            is_subtask: t.is_subtask,
            parent_ids: t.parent_ids.clone(),
        }
    }

    pub(crate) fn status_view(&self, content: &Content, id: Uuid) -> StatusView {
        let status = content.workflows.statuses.iter().find(|s| s.item_id == id);
        StatusView {
            id,
            key: status.map(|s| s.key.clone()).unwrap_or_default(),
            label: status.and_then(|s| self.label(&s.label_translations)),
            category: crate::workflows::CategoryView::of(status.map_or("todo", |s| s.category.as_str())),
        }
    }

    pub(crate) fn type_of(&self, content: &Content, id: Uuid) -> TypeView {
        content.types.iter().find(|t| t.item_id == id).map_or_else(
            || TypeView {
                id,
                key: String::new(),
                label: None,
                label_translations: Labels::new(),
                icon: "help".to_owned(),
                color: "task".to_owned(),
                is_subtask: false,
                parent_ids: Vec::new(),
            },
            |t| self.type_view(t),
        )
    }

    /// The labels of the list values `ids` of `values`, joined.
    fn value_labels(&self, values: &[ListValue], ids: &[u128], separator: &str) -> Option<String> {
        let all: Vec<&ListValue> = values
            .iter()
            .flat_map(|v| std::iter::once(v).chain(&v.children))
            .collect();
        let labels: Vec<String> = ids
            .iter()
            .filter_map(|id| all.iter().find(|v| v.id == *id))
            .filter_map(|v| self.label(&v.labels))
            .collect();
        (!labels.is_empty()).then(|| labels.join(separator))
    }

    pub(crate) fn display(
        &self,
        field: &Field,
        value: &Value,
        people: &BTreeMap<Uuid, String>,
    ) -> Option<String> {
        let ids = |value: &Value| ids_in(value).iter().map(Uuid::as_u128).collect::<Vec<_>>();
        let values = field.options.values.as_deref().unwrap_or_default();
        match field.kind {
            Kind::SingleSelect | Kind::MultiSelect => self.value_labels(values, &ids(value), ", "),
            Kind::CascadingSelect => self.value_labels(values, &ids(value), " › "),
            Kind::User | Kind::Group | Kind::IssueLink => {
                let names: Vec<&str> = ids(value)
                    .into_iter()
                    .filter_map(|id| people.get(&Uuid::from_u128(id)).map(String::as_str))
                    .collect();
                (!names.is_empty()).then(|| names.join(", "))
            }
            _ => None,
        }
    }
}

/// The identifiers a value holds: list values, users, groups, tickets.
pub(crate) fn ids_in(value: &Value) -> Vec<Uuid> {
    let one = |value: &Value| match value {
        Value::Text(text) => domain::fields::parse_uuid(text).map(Uuid::from_u128),
        _ => None,
    };
    match value {
        Value::List(items) => items.iter().filter_map(one).collect(),
        other => one(other).into_iter().collect(),
    }
}

/// Field errors for the users, groups and tickets that `values` name and
/// that do not exist (`validate_value` checks their form only). Fields
/// already in error are left alone.
pub(crate) async fn unknown_references(
    tx: &mut TenantTx,
    fields: &[Field],
    values: &BTreeMap<u128, Value>,
    errors: &[FieldError],
) -> Result<Vec<FieldError>, Problem> {
    let mut unknown = Vec::new();
    for field in fields {
        let path = format!("fields.{}", field.item_id);
        let Some(value) = values.get(&field.item_id.as_u128()) else {
            continue;
        };
        if errors.iter().any(|e| e.field == path) {
            continue;
        }
        let ids = ids_in(value);
        if ids.is_empty() {
            continue;
        }
        let (found, code) = match field.kind {
            Kind::User => (
                infra::rule_references::members(tx, &ids).await?.len(),
                "unknown_member",
            ),
            Kind::Group => (
                infra::rule_references::groups(tx, &ids).await?.len(),
                "unknown_group",
            ),
            Kind::IssueLink => {
                let mut found = 0;
                for id in &ids {
                    if repo::find(tx, *id).await?.is_some() {
                        found += 1;
                    }
                }
                (found, "unknown_issue")
            }
            _ => continue,
        };
        if found < ids.iter().collect::<BTreeSet<_>>().len() {
            unknown.push(FieldError::new(path, code));
        }
    }
    Ok(unknown)
}

/// The custom field values of `issue`, in the order of the configuration.
pub(crate) fn field_values(
    language: &Language,
    content: &Content,
    issue: &Issue,
    people: &BTreeMap<Uuid, String>,
) -> Vec<FieldValueView> {
    content
        .fields
        .iter()
        .filter_map(|field| {
            let json = issue.fields.get(field.item_id.to_string())?;
            let value = infra::fields::value_of(json)?;
            if value.is_empty() {
                return None;
            }
            let references = match field.kind {
                Kind::User | Kind::Group | Kind::IssueLink => ids_in(&value)
                    .into_iter()
                    .filter_map(|id| {
                        people.get(&id).map(|name| PersonView {
                            id,
                            name: name.clone(),
                        })
                    })
                    .collect(),
                _ => Vec::new(),
            };
            Some(FieldValueView {
                field_id: field.item_id,
                key: field.key.clone(),
                label: language.label(&field.label_translations),
                kind: field.kind.into(),
                display: language.display(field, &value, people),
                value: json.clone(),
                references,
            })
        })
        .collect()
}

/// The identifiers that fields of `kind` of `issue` hold.
pub(crate) fn ids_of_kind(content: &Content, issue: &Issue, kind: Kind) -> Vec<Uuid> {
    content
        .fields
        .iter()
        .filter(|f| f.kind == kind)
        .filter_map(|f| issue.fields.get(f.item_id.to_string()))
        .flat_map(|json| match json {
            serde_json::Value::String(s) => vec![s.clone()],
            serde_json::Value::Array(items) => items
                .iter()
                .filter_map(|i| i.as_str().map(str::to_owned))
                .collect(),
            _ => Vec::new(),
        })
        .filter_map(|s| Uuid::parse_str(&s).ok())
        .collect()
}

/// What the user, group and ticket fields of `issue` name, by id: names of
/// members and groups, keys of the tickets whose project the member sees.
pub(crate) async fn names_of(
    state: &AppState,
    tx: &mut TenantTx,
    content: &Content,
    issue: &Issue,
) -> Result<BTreeMap<Uuid, String>, Problem> {
    let mut names: BTreeMap<Uuid, String> =
        infra::rule_references::members(tx, &ids_of_kind(content, issue, Kind::User))
            .await?
            .into_iter()
            .collect();
    names.extend(infra::rule_references::groups(tx, &ids_of_kind(content, issue, Kind::Group)).await?);
    for id in ids_of_kind(content, issue, Kind::IssueLink) {
        if let Some(linked) = repo::find(tx, id).await?
            && visible_node(state, tx, linked.project_id, "").await.is_ok()
        {
            names.insert(id, linked.key);
        }
    }
    Ok(names)
}

pub(crate) fn person(id: Option<Uuid>, name: Option<&String>) -> Option<PersonView> {
    id.map(|id| PersonView {
        id,
        name: name.cloned().unwrap_or_default(),
    })
}

impl FormFieldView {
    pub(crate) const fn field_id(&self) -> Uuid {
        self.field_id
    }

    /// Marks it required (a transition's validator).
    pub(crate) fn require(&mut self) {
        self.mode = ModeView::Required;
    }
}

impl IssueView {
    pub(crate) const fn version(&self) -> i64 {
        self.version
    }
}

/// `issue` in a few words.
pub(crate) fn issue_ref(issue: &Issue) -> IssueRefView {
    IssueRefView {
        id: issue.id,
        key: issue.key.clone(),
        summary: issue.summary.clone(),
    }
}

/// The view of `issue`, with its parent if `expand` asks for it.
pub(crate) async fn issue_view(
    state: &AppState,
    tx: &mut TenantTx,
    language: &Language,
    issue: Issue,
    project: &Node,
    expand_parent: bool,
) -> Result<IssueView, Problem> {
    let content = Content::of(tx, issue.project_id).await?;
    let people = names_of(state, tx, &content, &issue).await?;
    let parent = match (expand_parent, issue.parent_id) {
        (true, Some(parent)) => repo::find(tx, parent).await?.map(|p| IssueRefView {
            id: p.id,
            key: p.key,
            summary: p.summary,
        }),
        _ => None,
    };
    let description = issue
        .description
        .as_ref()
        .and_then(|json| infra::rich_text::parse(json).ok());
    Ok(IssueView {
        fields: field_values(language, &content, &issue, &people),
        issue_type: language.type_of(&content, issue.type_id),
        status: language.status_view(&content, issue.status_id),
        project: project_view(project),
        description_html: description.as_ref().map(domain::rich_text::Doc::to_html),
        description: issue.description,
        priority: Priority::parse(&issue.priority).unwrap_or_default().into(),
        reporter: person(issue.reporter_id, issue.reporter_name.as_ref()),
        assignee: person(issue.assignee_id, issue.assignee_name.as_ref()),
        start_date: issue.start_date,
        due_date: issue.due_date,
        parent,
        parent_id: issue.parent_id,
        id: issue.id,
        key: issue.key,
        summary: issue.summary,
        created_at: issue.created_at,
        updated_at: issue.updated_at,
        version: issue.version,
        children: None,
        links: None,
    })
}

/// The project `key`, if the member sees it; else 404.
pub(crate) async fn project_of_key(state: &AppState, tx: &mut TenantTx, key: &str) -> Result<Node, Problem> {
    let not_found = || Problem::new(ErrorCode::NotFound).with_detail("no such project");
    let id = infra::nodes::project_of_key(tx, key)
        .await?
        .ok_or_else(not_found)?;
    visible_node(state, tx, id, "no such project").await
}

/// 403 `permission.denied` unless the member may create tickets in `project`.
pub(crate) async fn require_create(
    state: &AppState,
    tx: &mut TenantTx,
    project: Uuid,
) -> Result<(), Problem> {
    if is_tenant_admin(tx) {
        return Ok(());
    }
    let permissions = permissions::effective(state, tx, Some(project)).await?;
    permissions::check(permissions, Permission::CreateTicket)
}

/// The ticket `id_or_key` (a key, a former key, or an id), if the member
/// sees its project; else 404.
pub(crate) async fn visible_issue(
    state: &AppState,
    tx: &mut TenantTx,
    id_or_key: &str,
) -> Result<(Issue, Node), Problem> {
    let not_found = || Problem::new(ErrorCode::NotFound).with_detail("no such ticket");
    let id = match Uuid::parse_str(id_or_key) {
        Ok(id) => Some(id),
        Err(_) if parse_key(id_or_key).is_some() => repo::id_of_key(tx, id_or_key).await?,
        Err(_) => None,
    };
    let issue = match id {
        Some(id) => repo::find(tx, id).await?,
        None => None,
    }
    .ok_or_else(not_found)?;
    let project = visible_node(state, tx, issue.project_id, "no such ticket").await?;
    Ok((issue, project))
}

/// The custom field values of `body`, read as the fields take them; the
/// field errors otherwise.
pub(crate) fn read_fields(
    fields: &BTreeMap<Uuid, serde_json::Value>,
) -> Result<BTreeMap<u128, Value>, Vec<FieldError>> {
    let mut values = BTreeMap::new();
    let mut errors = Vec::new();
    for (id, json) in fields {
        match infra::fields::value_of(json) {
            Some(value) => {
                values.insert(id.as_u128(), value);
            }
            None => errors.push(FieldError::new(format!("fields.{id}"), "wrong_type")),
        }
    }
    if errors.is_empty() {
        Ok(values)
    } else {
        Err(errors)
    }
}

/// `POST /api/v1/projects/{key}/issues`: idempotent with `Idempotency-Key`.
#[utoipa::path(
    post,
    path = "/projects/{key}/issues",
    tag = TAG,
    operation_id = "createIssue",
    summary = "Create a ticket in a project",
    description = "Members with `create_ticket` on the project (tenant admins always). The ticket \
                   gets the next key of its project and the initial status of its type's workflow; \
                   its custom fields follow the rules of its type at creation. Writes \
                   `issue.created` to the event outbox.",
    params(
        ("key" = String, Path, description = "Key of the project (`PLAT`)"),
        ("Idempotency-Key" = Option<String>, Header, description = "Unique key of this creation, 1 to 255 visible ASCII characters"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    request_body = NewIssueBody,
    responses(
        (status = 201, description = "Created", body = IssueView, headers(
            ("ETag" = String, description = "Version of the ticket"),
            ("Location" = String, description = "URL of the ticket, by its key"),
        )),
        (status = 400, description = "`validation_failed`: `summary`, `typeId`, `parentId` (`required`, `not_found`, `not_allowed`, `other_project`), `assigneeId` `unknown_member`, `dueDate` `before_start`, `description` `invalid`, `fields.{id}` (`required`, `not_applicable`, `not_found`, a value code, or `unknown_member`, `unknown_group`, `unknown_issue`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such project visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`node.archived`, or `idempotency_key.in_flight`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`Idempotency-Key` already used for another request", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath(key): ApiPath<String>,
    ApiJson(body): ApiJson<NewIssueBody>,
) -> Result<Response, Problem> {
    let request = format!("POST /api/v1/projects/{key}/issues");
    let idempotency_key = match idempotency::start(&mut tx, &headers, &request, &body).await? {
        Start::Proceed(key) => key,
        Start::Replay(response) => return Ok(response),
    };
    let project = project_of_key(&state, &mut tx, &key).await?;
    require_create(&state, &mut tx, project.id).await?;
    ensure_changeable(&project)?;
    let content = Content::of(&mut tx, project.id).await?;

    let mut errors = Vec::new();
    let values = read_fields(&body.fields).unwrap_or_else(|e| {
        errors.extend(e);
        BTreeMap::new()
    });
    let parent = match body.parent_id {
        None => None,
        Some(id) => match repo::find(&mut tx, id).await? {
            Some(p) => Some(Parent {
                type_id: p.type_id.as_u128(),
                same_project: p.project_id == project.id,
            }),
            None => {
                errors.push(FieldError::new("parentId", "not_found"));
                None
            }
        },
    };
    let description = match &body.description {
        None => None,
        Some(json) => match infra::rich_text::parse(json) {
            Ok(doc) => match doc.validate() {
                Ok(()) if doc.is_empty() => None,
                Ok(()) => Some(infra::rich_text::to_json(&doc)),
                Err(refusal) => {
                    errors.push(FieldError::new("description", refusal.code()));
                    None
                }
            },
            Err(_) => {
                errors.push(FieldError::new("description", "invalid"));
                None
            }
        },
    };
    if let Some(assignee) = body.assignee_id
        && infra::rule_references::members(&mut tx, &[assignee])
            .await?
            .is_empty()
    {
        errors.push(FieldError::new("assigneeId", "unknown_member"));
    }
    let type_ids = type_ids_of(&content.fields);
    let defs = field_defs(&content.fields, &type_ids);
    let dates_in_order = match (body.start_date, body.due_date) {
        (Some(start), Some(due)) => start <= due,
        _ => true,
    };
    let new = NewIssue {
        type_id: body.type_id.as_u128(),
        summary: &body.summary,
        parent,
        dates_in_order,
        fields: &values,
    };
    if let Err(e) = validate_new(&new, &content.type_nodes(), &defs) {
        // An unknown parent is already named; it is not missing too.
        let parent_named = errors.iter().any(|e| e.field == "parentId");
        errors.extend(e.into_iter().filter(|e| !(parent_named && e.field == "parentId")));
    }
    let unknown = unknown_references(&mut tx, &content.fields, &values, &errors).await?;
    errors.extend(unknown);
    if !errors.is_empty() {
        return Err(Problem::validation(errors));
    }
    let issue_type = content
        .types
        .iter()
        .find(|t| t.item_id == body.type_id)
        .ok_or_else(|| Problem::validation(vec![FieldError::new("typeId", "not_found")]))?;
    let status = issue_type
        .workflow_id
        .and_then(|w| content.workflows.statuses_of(w).find(|s| s.is_initial))
        .ok_or(Problem::new(ErrorCode::WorkflowInvalid))?
        .item_id;
    // Only the values given, by field id.
    let stored: serde_json::Map<String, serde_json::Value> = body
        .fields
        .iter()
        .filter(|(id, _)| values.get(&id.as_u128()).is_some_and(|v| !v.is_empty()))
        .map(|(id, json)| (id.to_string(), json.clone()))
        .collect();
    let stored = serde_json::Value::Object(stored);
    let actor = tx.account_id();
    let priority = body.priority.map_or(Priority::default(), Priority::from);
    let issue = repo::create(
        &mut tx,
        &repo::NewIssue {
            id: infra::id::new_id(),
            project_id: project.id,
            project_key: &project.key,
            type_id: body.type_id,
            status_id: status,
            parent_id: body.parent_id,
            summary: &body.summary,
            description: description.as_ref(),
            priority: priority.as_str(),
            reporter_id: actor,
            assignee_id: body.assignee_id,
            start_date: body.start_date,
            due_date: body.due_date,
            fields: &stored,
        },
    )
    .await?;
    infra::outbox::record(
        &mut tx,
        &infra::outbox::Event {
            kind: "issue.created",
            aggregate_type: "issue",
            aggregate_id: issue.id,
            payload: serde_json::json!({
                "key": issue.key,
                "projectId": project.id,
                "typeId": issue.type_id,
                "assigneeId": issue.assignee_id,
            }),
            actor_id: Some(actor),
        },
    )
    .await?;
    crate::issue_history::record(
        &mut tx,
        issue.id,
        &[infra::audit::Entry::action(domain::audit::Action::Created)],
    )
    .await?;
    // Its reporter and its assignee follow it.
    infra::watchers::watch(&mut tx, issue.id, actor).await?;
    if let Some(assignee) = issue.assignee_id {
        infra::watchers::watch(&mut tx, issue.id, assignee).await?;
    }
    tracing::info!(target: "audit", actor = %actor, issue = %issue.id, key = %issue.key, "ticket created");
    let language = Language::of(&tx, &accept);
    let location = format!("/api/v1/issues/{}", issue.key);
    let view = issue_view(&state, &mut tx, &language, issue, &project, false).await?;
    let version = view.version;
    let mut response =
        idempotency::created(&mut tx, idempotency_key.as_deref(), &view, location, version).await?;
    tx.commit().await?;
    language.headers(&mut response);
    Ok(response)
}

/// `GET /api/v1/issues/{idOrKey}`.
#[utoipa::path(
    get,
    path = "/issues/{idOrKey}",
    tag = TAG,
    operation_id = "getIssue",
    summary = "Read a ticket",
    description = "Whoever sees its project. By id, key (`PLAT-142`) or former key.",
    params(
        ("idOrKey" = String, Path, description = "Id or key of the ticket"),
        ReadParams,
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    responses(
        (status = 200, description = "The ticket", body = IssueView, headers(
            ("ETag" = String, description = "Version of the ticket"),
        )),
        (status = 400, description = "`invalid_query`: an unknown `expand`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn read(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id_or_key): ApiPath<String>,
    ApiQuery(params): ApiQuery<ReadParams>,
) -> Result<Response, Problem> {
    let expand: BTreeSet<&str> = params
        .expand
        .as_deref()
        .map(|e| e.split(',').map(str::trim).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    if let Some(unknown) = expand
        .iter()
        .find(|e| !["parent", "children", "links"].contains(e))
    {
        return Err(Problem::new(ErrorCode::InvalidQuery).with_detail(format!("unknown expand: {unknown}")));
    }
    let (issue, project) = visible_issue(&state, &mut tx, &id_or_key).await?;
    let language = Language::of(&tx, &accept);
    let mut view = issue_view(
        &state,
        &mut tx,
        &language,
        issue,
        &project,
        expand.contains("parent"),
    )
    .await?;
    let id = view.id;
    if expand.contains("children") {
        let children = repo::children(&mut tx, id).await?;
        view.children = Some(crate::issue_links::related_views(&state, &mut tx, &language, children).await?);
    }
    if expand.contains("links") {
        view.links = Some(crate::issue_links::link_views(&state, &mut tx, &language, id).await?);
    }
    tx.commit().await?;
    let version = view.version;
    let mut response = ApiJson(view).into_response();
    response.headers_mut().insert(header::ETAG, etag(version));
    language.headers(&mut response);
    Ok(response)
}

/// `GET /api/v1/projects/{key}/issue-form`.
#[utoipa::path(
    get,
    path = "/projects/{key}/issue-form",
    tag = TAG,
    operation_id = "getIssueForm",
    summary = "What creating a ticket in a project offers",
    description = "Members with `create_ticket` on the project. The types of its configuration, \
                   and for the type chosen the fields of its creation screen, the rules applied: \
                   hidden ones left out, required ones marked (added if the screen lacks them).",
    params(
        ("key" = String, Path, description = "Key of the project"),
        FormParams,
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    responses(
        (status = 200, description = "The form", body = FormView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such project visible to the member, or type", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn form(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(key): ApiPath<String>,
    ApiQuery(params): ApiQuery<FormParams>,
) -> Result<Response, Problem> {
    let project = project_of_key(&state, &mut tx, &key).await?;
    require_create(&state, &mut tx, project.id).await?;
    let content = Content::of(&mut tx, project.id).await?;
    let language = Language::of(&tx, &accept);
    tx.commit().await?;
    let chosen = match params.type_id {
        Some(id) => content.types.iter().find(|t| t.item_id == id),
        None => content
            .types
            .iter()
            .find(|t| !t.is_subtask)
            .or(content.types.first()),
    }
    .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such type"))?;
    let fields = screen_fields(
        &language,
        &content,
        ScreenKind::Creation,
        chosen.item_id.as_u128(),
        Step::Creation,
        &BTreeSet::new(),
    );
    let view = FormView {
        project: project_view(&project),
        types: content.types.iter().map(|t| language.type_view(t)).collect(),
        type_id: chosen.item_id,
        fields,
    };
    let mut response = ApiJson(view).into_response();
    language.headers(&mut response);
    Ok(response)
}

/// The fields of the screen of `kind` for a ticket of `type_id` at `step`,
/// the rules applied: hidden ones left out, required ones marked (added if
/// the screen lacks them). List fields offer their active values, and
/// those of `held` (the ticket's own) even if deactivated.
pub(crate) fn screen_fields(
    language: &Language,
    content: &Content,
    kind: ScreenKind,
    type_id: u128,
    step: Step,
    held: &BTreeSet<u128>,
) -> Vec<FormFieldView> {
    let screens: Vec<domain::screens::Screen> = content
        .screens
        .iter()
        .map(infra::screens::Screen::domain)
        .collect();
    let on_screen = pick(&screens, kind, type_id)
        .map(|s| s.fields.clone())
        .unwrap_or_default();
    fields_shown(language, content, &on_screen, type_id, step, held)
}

/// The fields `on_screen` for a ticket of `type_id` at `step`, the rules
/// applied (see [`screen_fields`]).
pub(crate) fn fields_shown(
    language: &Language,
    content: &Content,
    on_screen: &[u128],
    type_id: u128,
    step: Step,
    held: &BTreeSet<u128>,
) -> Vec<FormFieldView> {
    let type_ids = type_ids_of(&content.fields);
    let rules: Vec<FieldRules<'_>> = content
        .fields
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
    let offered = |v: &ListValue| v.active || held.contains(&v.id);
    let value_view = |v: &ListValue| FormValueView {
        id: Uuid::from_u128(v.id),
        label: language.label(&v.labels),
        children: v
            .children
            .iter()
            .filter(|c| offered(c))
            .map(|c| FormValueView {
                id: Uuid::from_u128(c.id),
                label: language.label(&c.labels),
                children: Vec::new(),
            })
            .collect(),
    };
    resolve(on_screen, &rules, type_id, step)
        .into_iter()
        .filter_map(|shown| {
            let field = content
                .fields
                .iter()
                .find(|f| f.item_id.as_u128() == shown.field)?;
            let o = &field.options;
            Some(FormFieldView {
                field_id: field.item_id,
                key: field.key.clone(),
                label: language.label(&field.label_translations),
                kind: field.kind.into(),
                mode: if shown.mode == Mode::Required {
                    ModeView::Required
                } else {
                    ModeView::Visible
                },
                max_length: o.max_length,
                decimals: o.decimals,
                min: o.min,
                max: o.max,
                unit: o.unit.map(|u| u.as_str().to_owned()),
                multiple: o.multiple,
                values: o
                    .values
                    .as_ref()
                    .map(|values| values.iter().filter(|v| offered(v)).map(value_view).collect()),
            })
        })
        .collect()
}

/// `GET /api/v1/me/issues`.
#[utoipa::path(
    get,
    path = "/me/issues",
    tag = TAG,
    operation_id = "listMyIssues",
    summary = "List the tickets assigned to the signed-in member or reported by them",
    description = "Newest first; only those of projects they see.",
    params(ListParams, ("Accept-Language" = Option<String>, Header, description = "Language of the labels")),
    responses(
        (status = 200, description = "A page of tickets", body = IssuePage),
        (status = 400, description = "Invalid `limit` or `cursor`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn mine(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiQuery(params): ApiQuery<ListParams>,
) -> Result<Response, Problem> {
    let (limit, after) = PageParams {
        limit: params.limit,
        cursor: params.cursor,
    }
    .resolve()?;
    let account = tx.account_id();
    let page = repo::of_account(&mut tx, account, after, limit).await?;
    let language = Language::of(&tx, &accept);
    let mut projects: BTreeMap<Uuid, Option<(Node, Content)>> = BTreeMap::new();
    let mut items = Vec::new();
    for issue in page.items {
        if let std::collections::btree_map::Entry::Vacant(entry) = projects.entry(issue.project_id) {
            let seen = match visible_node(&state, &mut tx, issue.project_id, "").await {
                Ok(node) => Some((node, Content::of(&mut tx, issue.project_id).await?)),
                Err(_) => None,
            };
            entry.insert(seen);
        }
        let Some(Some((node, content))) = projects.get(&issue.project_id) else {
            continue;
        };
        items.push(SummaryView {
            issue_type: language.type_of(content, issue.type_id),
            status: language.status_view(content, issue.status_id),
            project: project_view(node),
            priority: Priority::parse(&issue.priority).unwrap_or_default().into(),
            assignee: person(issue.assignee_id, issue.assignee_name.as_ref()),
            id: issue.id,
            key: issue.key,
            summary: issue.summary,
            updated_at: issue.updated_at,
        });
    }
    tx.commit().await?;
    let mut response = ApiJson(IssuePage {
        items,
        next_cursor: page.next.map(encode_cursor),
    })
    .into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `GET /api/v1/projects/{key}/people`.
#[utoipa::path(
    get,
    path = "/projects/{key}/people",
    tag = TAG,
    operation_id = "listProjectPeople",
    summary = "Search the members or groups a ticket of a project can name",
    description = "Whoever sees the project. The active members of the organization whose name \
                   or email holds `search` (the assignee, user fields), or with `kind=group` its \
                   groups (group fields); by name, ten at most.",
    params(("key" = String, Path, description = "Key of the project"), PeopleParams),
    responses(
        (status = 200, description = "The members or groups", body = Vec<PersonView>),
        (status = 400, description = "`invalid_query`: an unknown `kind`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such project visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn people(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(key): ApiPath<String>,
    ApiQuery(params): ApiQuery<PeopleParams>,
) -> Result<ApiJson<Vec<PersonView>>, Problem> {
    project_of_key(&state, &mut tx, &key).await?;
    let search = params.search.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let found = infra::grants::candidates(&mut tx, search, 10).await?;
    tx.commit().await?;
    Ok(ApiJson(
        found
            .into_iter()
            .filter_map(|c| match c {
                infra::grants::Candidate::Member {
                    account_id,
                    display_name,
                    ..
                } if params.kind == PeopleKind::Member => Some(PersonView {
                    id: account_id,
                    name: display_name,
                }),
                infra::grants::Candidate::Group { id, name } if params.kind == PeopleKind::Group => {
                    Some(PersonView { id, name })
                }
                _ => None,
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priorities_map_both_ways() {
        for priority in Priority::ALL {
            assert_eq!(Priority::from(PriorityView::from(priority)), priority);
        }
        assert_eq!(serde_json::to_value(PriorityView::Highest).unwrap(), "highest");
    }

    #[test]
    fn field_values_are_read_as_their_fields_take_them() {
        let id = Uuid::from_u128(7);
        let fine: BTreeMap<Uuid, serde_json::Value> = [(id, serde_json::json!(["a", 1.5]))].into();
        assert_eq!(
            read_fields(&fine).unwrap()[&7],
            Value::List(vec![Value::Text("a".into()), Value::Number(1.5)])
        );
        let object: BTreeMap<Uuid, serde_json::Value> = [(id, serde_json::json!({"a": 1}))].into();
        assert_eq!(
            read_fields(&object).unwrap_err(),
            [FieldError::new(format!("fields.{id}"), "wrong_type")]
        );
    }
}
