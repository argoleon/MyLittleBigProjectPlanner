//! The history of a ticket (specification §12, §18, L05-US06).
//!
//! Every ticket write records its entries in `audit_log` in its own
//! transaction ([`record`]): the fields it changed, compared before and
//! after ([`snapshot`], `domain::audit::diff`), one entry each with the old
//! and new values; or its action (creation, move, link, comment,
//! attachment). Who wrote
//! and from where come from the request ([`TenantTx::author`]).
//!
//! Values are stored as the ticket holds them, except types and statuses,
//! stored with their labels of the time: the configuration may change, the
//! ticket may move. `GET /issues/{idOrKey}/activity` reads them back in the
//! member's language, newest first.

use std::collections::{BTreeMap, BTreeSet};

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use domain::audit::{Action, Change, Field as AuditField, Origin, Snapshot, diff};
use domain::fields::Kind;
use domain::links::Relation;
use infra::audit::{Entry, Recorded};
use infra::fields::Field;
use infra::issues::{self as repo, Issue};
use infra::uuid::Uuid;
use serde::Serialize;
use serde_json::{Value as Json, json};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::extract::{AcceptLanguage, ApiJson, ApiPath, ApiQuery};
use crate::issue_comments::acting;
use crate::issue_links::RelationView;
use crate::issues::{Content, ListParams, PersonView, TAG, ids_in, person, visible_issue};
use crate::nodes::visible_node;
use crate::pagination::{PageParams, encode_cursor};
use crate::problem::{PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::tenant::{AppState, TenantTx};

// --- Writing -----------------------------------------------------------------

/// A type or a status as stored: its id and its labels at the time.
fn labelled(id: Uuid, labels: Option<&BTreeMap<String, String>>) -> Json {
    json!({"id": id, "label": labels.cloned().unwrap_or_default()})
}

/// The values of `issue`'s fields, its configuration being `content`.
pub(crate) fn snapshot(issue: &Issue, content: &Content) -> Snapshot<Json> {
    let issue_type = content.types.iter().find(|t| t.item_id == issue.type_id);
    let status = content
        .workflows
        .statuses
        .iter()
        .find(|s| s.item_id == issue.status_id);
    let mut values = Snapshot::from([
        (
            AuditField::Type,
            labelled(issue.type_id, issue_type.map(|t| &t.label_translations)),
        ),
        (
            AuditField::Status,
            labelled(issue.status_id, status.map(|s| &s.label_translations)),
        ),
        (AuditField::Summary, json!(issue.summary)),
        (
            AuditField::Description,
            issue.description.clone().unwrap_or_default(),
        ),
        (AuditField::Priority, json!(issue.priority)),
        (AuditField::Reporter, json!(issue.reporter_id)),
        (AuditField::Assignee, json!(issue.assignee_id)),
        (AuditField::StartDate, json!(issue.start_date)),
        (AuditField::DueDate, json!(issue.due_date)),
        (AuditField::Parent, json!(issue.parent_id)),
    ]);
    for (id, value) in issue.fields.as_object().into_iter().flatten() {
        if let Ok(id) = Uuid::parse_str(id) {
            values.insert(AuditField::Custom(id.as_u128()), value.clone());
        }
    }
    values
}

/// The name a field is stored under: `status`, `fields.{id}`…
pub(crate) fn field_name(field: AuditField) -> String {
    match field {
        AuditField::Custom(id) => format!("fields.{}", Uuid::from_u128(id)),
        system => system.system_name().unwrap_or_default().to_owned(),
    }
}

/// The entries of the fields changed between `before` and `after`.
pub(crate) fn changes(before: &Snapshot<Json>, after: &Snapshot<Json>) -> Vec<Entry> {
    diff(before, after)
        .into_iter()
        .map(|Change { field, old, new }| Entry::field(field_name(field), old, new))
        .collect()
}

/// The entry of a ticket moved from `from` to `to` (keys).
pub(crate) fn moved(from: &str, to: &str) -> Entry {
    Entry {
        old: Some(json!({"key": from})),
        new: Some(json!({"key": to})),
        ..Entry::action(Action::Moved)
    }
}

/// The entry of a link added or removed, as it reads from the ticket whose
/// history it goes to.
pub(crate) fn link(added: bool, relation: Relation, other: Uuid) -> Entry {
    let value = json!({"relation": relation.as_str(), "issueId": other});
    if added {
        Entry {
            new: Some(value),
            ..Entry::action(Action::LinkAdded)
        }
    } else {
        Entry {
            old: Some(value),
            ..Entry::action(Action::LinkRemoved)
        }
    }
}

/// The entry of a comment written, edited or deleted: its id, and its
/// content before and after.
pub(crate) fn comment(action: Action, id: Uuid, old: Option<&Json>, new: Option<&Json>) -> Entry {
    let value = |content: Option<&Json>| content.map(|c| json!({"commentId": id, "content": c}));
    Entry {
        old: value(old).or_else(|| (action != Action::CommentCreated).then(|| json!({"commentId": id}))),
        new: value(new),
        ..Entry::action(action)
    }
}

/// The entry of a file attached or removed: its id and name.
pub(crate) fn attachment(added: bool, id: Uuid, name: &str) -> Entry {
    let value = json!({"attachmentId": id, "name": name});
    if added {
        Entry {
            new: Some(value),
            ..Entry::action(Action::AttachmentAdded)
        }
    } else {
        Entry {
            old: Some(value),
            ..Entry::action(Action::AttachmentRemoved)
        }
    }
}

/// Records `entries` in the history of the ticket `issue`, as the member
/// of `tx` from where they write.
pub(crate) async fn record(tx: &mut TenantTx, issue: Uuid, entries: &[Entry]) -> Result<(), Problem> {
    let author = tx.author();
    infra::audit::record(tx, issue, author, entries).await?;
    Ok(())
}

// --- Reading -----------------------------------------------------------------

/// What an entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = ActivityAction)]
pub enum ActionView {
    Created,
    Updated,
    Moved,
    LinkAdded,
    LinkRemoved,
    CommentCreated,
    CommentUpdated,
    CommentDeleted,
    AttachmentAdded,
    AttachmentRemoved,
}

impl From<Action> for ActionView {
    fn from(action: Action) -> Self {
        match action {
            Action::Created => Self::Created,
            Action::Updated => Self::Updated,
            Action::Moved => Self::Moved,
            Action::LinkAdded => Self::LinkAdded,
            Action::LinkRemoved => Self::LinkRemoved,
            Action::CommentCreated => Self::CommentCreated,
            Action::CommentUpdated => Self::CommentUpdated,
            Action::CommentDeleted => Self::CommentDeleted,
            Action::AttachmentAdded => Self::AttachmentAdded,
            Action::AttachmentRemoved => Self::AttachmentRemoved,
        }
    }
}

/// Where a change came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = ActivityOrigin)]
pub enum OriginView {
    Interface,
    Api,
    Mcp,
    Import,
    System,
}

impl From<Origin> for OriginView {
    fn from(origin: Origin) -> Self {
        match origin {
            Origin::Interface => Self::Interface,
            Origin::Api => Self::Api,
            Origin::Mcp => Self::Mcp,
            Origin::Import => Self::Import,
            Origin::System => Self::System,
        }
    }
}

/// The field an `updated` entry changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ActivityField)]
pub enum FieldView {
    Type,
    Status,
    Summary,
    Description,
    Priority,
    Reporter,
    Assignee,
    StartDate,
    DueDate,
    Parent,
    /// A custom field: see `fieldLabel`.
    Custom,
}

impl From<AuditField> for FieldView {
    fn from(field: AuditField) -> Self {
        match field {
            AuditField::Type => Self::Type,
            AuditField::Status => Self::Status,
            AuditField::Summary => Self::Summary,
            AuditField::Description => Self::Description,
            AuditField::Priority => Self::Priority,
            AuditField::Reporter => Self::Reporter,
            AuditField::Assignee => Self::Assignee,
            AuditField::StartDate => Self::StartDate,
            AuditField::DueDate => Self::DueDate,
            AuditField::Parent => Self::Parent,
            AuditField::Custom(_) => Self::Custom,
        }
    }
}

/// How to show a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ActivityValueKind)]
pub enum ValueKind {
    /// `text` as is: a label, a summary, list values…
    Text,
    /// `html`: sanitized rich text, shown on demand.
    RichText,
    /// `text` is a date (`2026-11-30`), to format in the reader's locale.
    Date,
    /// `text` is an instant (RFC 3339), to format in the reader's time zone.
    DateTime,
    /// `text` is a priority (`highest`…), to translate.
    Priority,
    /// `text` is `true` or `false`.
    Boolean,
    /// `text` is a number.
    Number,
    /// `text` is a person's name; `null`: no longer a member.
    Person,
    /// `text` is a ticket's key; `null`: a ticket no longer there.
    Issue,
}

/// A value of an entry.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ActivityValue)]
pub struct ValueView {
    kind: ValueKind,
    #[schema(required = true)]
    text: Option<String>,
    /// Sanitized HTML of a rich text value.
    #[schema(required = true)]
    html: Option<String>,
}

impl ValueView {
    fn text(kind: ValueKind, text: Option<String>) -> Self {
        Self {
            kind,
            text,
            html: None,
        }
    }

    fn rich_text(json: &Json) -> Self {
        Self {
            kind: ValueKind::RichText,
            text: None,
            html: infra::rich_text::parse(json).ok().map(|doc| doc.to_html()),
        }
    }
}

/// An entry of the history of a ticket.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ActivityEntry)]
pub struct EntryView {
    id: Uuid,
    /// Entries written by one change share it.
    change_id: Uuid,
    action: ActionView,
    /// The field an `updated` entry changed.
    #[schema(required = true)]
    field: Option<FieldView>,
    /// The label of a custom field, in the request's language; `null` for
    /// system fields, or a field no longer in the ticket's configuration.
    #[schema(required = true)]
    field_label: Option<String>,
    /// How a link added or removed reads from this ticket.
    #[schema(required = true)]
    relation: Option<RelationView>,
    /// The value before; `null`: nothing, or not shown (comments).
    #[schema(required = true)]
    old: Option<ValueView>,
    #[schema(required = true)]
    new: Option<ValueView>,
    /// Who made the change; `null`: the system (`origin` `system`) or an
    /// account no longer a member. Erased members show as "Deleted user #N".
    #[schema(required = true)]
    actor: Option<PersonView>,
    origin: OriginView,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

/// A page of the history of a ticket.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ActivityPage)]
pub struct ActivityPage {
    items: Vec<EntryView>,
    /// Cursor of the next (older) page, `null` on the last page.
    #[schema(required = true)]
    next_cursor: Option<String>,
}

/// What reading the entries needs: the ticket's configuration, the names
/// of the people, groups and tickets the values name.
struct Reading<'a> {
    language: &'a Language,
    fields: BTreeMap<u128, &'a Field>,
    /// Names of members and groups, keys of the tickets the member sees.
    names: BTreeMap<Uuid, String>,
    /// The member may see deleted comments (moderation).
    moderates: bool,
}

/// The field an entry stored under `name` changed.
fn parse_field(name: &str) -> Option<AuditField> {
    match name.strip_prefix("fields.") {
        Some(id) => Uuid::parse_str(id)
            .ok()
            .map(|id| AuditField::Custom(id.as_u128())),
        None => AuditField::parse_system(name),
    }
}

/// A value as text, whatever it holds: list items joined.
fn plain(json: &Json) -> Option<String> {
    match json {
        Json::Null => None,
        Json::String(text) => Some(text.clone()),
        Json::Array(items) => Some(items.iter().filter_map(plain).collect::<Vec<_>>().join(", ")),
        other => Some(other.to_string()),
    }
}

/// The ids a stored value names (users, groups, tickets).
fn ids(json: &Json) -> Vec<Uuid> {
    infra::fields::value_of(json)
        .map(|v| ids_in(&v))
        .unwrap_or_default()
}

impl Reading<'_> {
    fn name_of(&self, json: &Json) -> Option<String> {
        json.as_str()
            .and_then(|id| Uuid::parse_str(id).ok())
            .and_then(|id| self.names.get(&id).cloned())
    }

    /// `json`, stored for `field`, as shown.
    fn value(&self, field: AuditField, json: Option<&Json>) -> Option<ValueView> {
        let json = json.filter(|j| !j.is_null())?;
        Some(match field {
            AuditField::Type | AuditField::Status => {
                let labels: BTreeMap<String, String> =
                    serde_json::from_value(json.get("label").cloned().unwrap_or_default())
                        .unwrap_or_default();
                ValueView::text(ValueKind::Text, self.language.label(&labels))
            }
            AuditField::Summary => ValueView::text(ValueKind::Text, plain(json)),
            AuditField::Description => ValueView::rich_text(json),
            AuditField::Priority => ValueView::text(ValueKind::Priority, plain(json)),
            AuditField::Reporter | AuditField::Assignee => {
                ValueView::text(ValueKind::Person, self.name_of(json))
            }
            AuditField::StartDate | AuditField::DueDate => ValueView::text(ValueKind::Date, plain(json)),
            AuditField::Parent => ValueView::text(ValueKind::Issue, self.name_of(json)),
            AuditField::Custom(id) => match self.fields.get(&id) {
                None => ValueView::text(ValueKind::Text, plain(json)),
                Some(def) => self.custom(def, json),
            },
        })
    }

    fn custom(&self, field: &Field, json: &Json) -> ValueView {
        let kind = match field.kind {
            Kind::RichText => return ValueView::rich_text(json),
            Kind::Date => ValueKind::Date,
            Kind::DateTime => ValueKind::DateTime,
            Kind::Checkbox => ValueKind::Boolean,
            Kind::Number => ValueKind::Number,
            _ => ValueKind::Text,
        };
        let shown =
            infra::fields::value_of(json).and_then(|value| self.language.display(field, &value, &self.names));
        ValueView::text(kind, shown.or_else(|| plain(json)))
    }

    /// The view of `entry`; none when it names a ticket the member does
    /// not see (a link).
    fn entry(&self, entry: &Recorded) -> Option<EntryView> {
        let field = entry.field.as_deref().and_then(parse_field);
        let (mut old, mut new, mut relation) = (None, None, None);
        match entry.action {
            Action::Updated => {
                let field = field?;
                old = self.value(field, entry.old.as_ref());
                new = self.value(field, entry.new.as_ref());
            }
            Action::Moved => {
                let key = |json: Option<&Json>| json.and_then(|j| j.get("key")).and_then(plain);
                old = Some(ValueView::text(ValueKind::Issue, key(entry.old.as_ref())));
                new = Some(ValueView::text(ValueKind::Issue, key(entry.new.as_ref())));
            }
            Action::LinkAdded | Action::LinkRemoved => {
                let stored = entry.new.as_ref().or(entry.old.as_ref())?;
                relation = stored
                    .get("relation")
                    .and_then(Json::as_str)
                    .and_then(Relation::parse)
                    .map(RelationView::from);
                let other = ValueView::text(ValueKind::Issue, Some(self.name_of(stored.get("issueId")?)?));
                if entry.action == Action::LinkAdded {
                    new = Some(other);
                } else {
                    old = Some(other);
                }
            }
            Action::CommentDeleted if domain::audit::shows_deleted_comment(self.moderates) => {
                old = entry
                    .old
                    .as_ref()
                    .and_then(|j| j.get("content"))
                    .map(ValueView::rich_text);
            }
            Action::AttachmentAdded | Action::AttachmentRemoved => {
                let name = |json: Option<&Json>| {
                    json.and_then(|j| j.get("name"))
                        .and_then(plain)
                        .map(|name| ValueView::text(ValueKind::Text, Some(name)))
                };
                old = name(entry.old.as_ref());
                new = name(entry.new.as_ref());
            }
            Action::Created | Action::CommentCreated | Action::CommentUpdated | Action::CommentDeleted => {}
        }
        Some(EntryView {
            field_label: match field {
                Some(AuditField::Custom(id)) => self
                    .fields
                    .get(&id)
                    .and_then(|f| self.language.label(&f.label_translations)),
                _ => None,
            },
            field: field.map(FieldView::from),
            relation,
            old,
            new,
            actor: entry
                .actor_name
                .as_ref()
                .and_then(|name| person(entry.actor_id, Some(name))),
            origin: entry.origin.into(),
            action: entry.action.into(),
            id: entry.id,
            change_id: entry.change_id,
            created_at: entry.created_at,
        })
    }
}

/// The ids of the members, groups and tickets `entries` name.
fn named(entries: &[Recorded], fields: &BTreeMap<u128, &Field>) -> (Vec<Uuid>, Vec<Uuid>, Vec<Uuid>) {
    let (mut members, mut groups, mut issues) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for entry in entries {
        let values = [entry.old.as_ref(), entry.new.as_ref()];
        let field = entry.field.as_deref().and_then(parse_field);
        for json in values.into_iter().flatten() {
            match (entry.action, field) {
                (Action::Updated, Some(AuditField::Reporter | AuditField::Assignee)) => {
                    members.extend(ids(json));
                }
                (Action::Updated, Some(AuditField::Parent)) => issues.extend(ids(json)),
                (Action::Updated, Some(AuditField::Custom(id))) => match fields.get(&id).map(|f| f.kind) {
                    Some(Kind::User) => members.extend(ids(json)),
                    Some(Kind::Group) => groups.extend(ids(json)),
                    Some(Kind::IssueLink) => issues.extend(ids(json)),
                    _ => {}
                },
                (Action::LinkAdded | Action::LinkRemoved, _) => {
                    issues.extend(json.get("issueId").map(ids).unwrap_or_default());
                }
                _ => {}
            }
        }
    }
    (
        members.into_iter().collect(),
        groups.into_iter().collect(),
        issues.into_iter().collect(),
    )
}

/// `GET /api/v1/issues/{idOrKey}/activity`.
#[utoipa::path(
    get,
    path = "/issues/{idOrKey}/activity",
    tag = TAG,
    operation_id = "listIssueActivity",
    summary = "The history of a ticket",
    description = "Whoever sees its project. Newest first: each field a change set with its old \
                   and new values, creation, moves, links (those to tickets the member sees), \
                   comments written, edited or deleted (the content of a deleted one for \
                   moderators only); who and from where (`origin`).",
    params(
        ("idOrKey" = String, Path, description = "Id, key or former key of the ticket"),
        ListParams,
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    responses(
        (status = 200, description = "A page of entries", body = ActivityPage),
        (status = 400, description = "Invalid `limit` or `cursor`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn activity(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id_or_key): ApiPath<String>,
    ApiQuery(params): ApiQuery<ListParams>,
) -> Result<Response, Problem> {
    let (limit, before) = PageParams {
        limit: params.limit,
        cursor: params.cursor,
    }
    .resolve()?;
    let (issue, project) = visible_issue(&state, &mut tx, &id_or_key).await?;
    let moderates = acting(&state, &mut tx, &project).await?.moderates;
    let content = Content::of(&mut tx, project.id).await?;
    let page = infra::audit::of_issue(&mut tx, issue.id, limit, before).await?;
    let fields: BTreeMap<u128, &Field> = content.fields.iter().map(|f| (f.item_id.as_u128(), f)).collect();
    let (members, groups, issues) = named(&page.items, &fields);
    let mut names = infra::audit::names(&mut tx, &members).await?;
    names.extend(infra::rule_references::groups(&mut tx, &groups).await?);
    for id in issues {
        if let Some(other) = repo::find(&mut tx, id).await?
            && visible_node(&state, &mut tx, other.project_id, "").await.is_ok()
        {
            names.insert(id, other.key);
        }
    }
    let language = Language::of(&tx, &accept);
    let reading = Reading {
        language: &language,
        fields,
        names,
        moderates,
    };
    let items = page.items.iter().filter_map(|e| reading.entry(e)).collect();
    tx.commit().await?;
    let mut response = ApiJson(ActivityPage {
        items,
        next_cursor: page.next.map(encode_cursor),
    })
    .into_response();
    language.headers(&mut response);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_stored_by_name_and_read_back() {
        for field in [AuditField::Status, AuditField::DueDate, AuditField::Custom(42)] {
            assert_eq!(parse_field(&field_name(field)), Some(field));
        }
        assert_eq!(field_name(AuditField::StartDate), "startDate");
        assert_eq!(
            field_name(AuditField::Custom(1)),
            "fields.00000000-0000-0000-0000-000000000001"
        );
        assert_eq!(parse_field("fields.nope"), None);
        assert_eq!(parse_field("colour"), None);
    }

    #[test]
    fn changed_values_become_field_entries() {
        let before = Snapshot::from([
            (AuditField::Summary, json!("Migrate")),
            (AuditField::Assignee, Json::Null),
        ]);
        let after = Snapshot::from([
            (AuditField::Summary, json!("Migrate to OIDC")),
            (
                AuditField::Assignee,
                json!("018f0000-0000-7000-8000-000000000001"),
            ),
        ]);
        let entries = changes(&before, &after);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].field.as_deref(), Some("summary"));
        assert_eq!(entries[1].old, Some(Json::Null));
    }

    #[test]
    fn comment_entries_keep_the_comment_and_its_content() {
        let id = Uuid::from_u128(7);
        let doc = json!({"type": "doc"});
        let created = comment(Action::CommentCreated, id, None, Some(&doc));
        assert_eq!(created.old, None);
        assert_eq!(created.new, Some(json!({"commentId": id, "content": doc})));
        let deleted = comment(Action::CommentDeleted, id, Some(&doc), None);
        assert_eq!(deleted.old, Some(json!({"commentId": id, "content": doc})));
        assert_eq!(deleted.new, None);
    }

    #[test]
    fn values_read_as_text_whatever_they_hold() {
        assert_eq!(plain(&json!(["a", "b"])), Some("a, b".to_owned()));
        assert_eq!(plain(&json!(3.5)), Some("3.5".to_owned()));
        assert_eq!(plain(&Json::Null), None);
    }
}
