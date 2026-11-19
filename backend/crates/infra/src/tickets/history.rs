//! The entries a change of a ticket writes to its history (L05-US06): the
//! values of its fields before and after, a move, a link, a comment, a file.

use std::collections::BTreeMap;

use domain::audit::{Action, Change, Field as AuditField, Snapshot, diff};
use domain::links::Relation;
use serde_json::{Value as Json, json};
use uuid::Uuid;

use super::content::Content;
use crate::audit::Entry;
use crate::issues::Issue;

/// A type or a status as stored: its id and its labels at the time.
fn labelled(id: Uuid, labels: Option<&BTreeMap<String, String>>) -> Json {
    json!({"id": id, "label": labels.cloned().unwrap_or_default()})
}

/// The values of `issue`'s fields, its configuration being `content`.
pub fn snapshot(issue: &Issue, content: &Content) -> Snapshot<Json> {
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
pub fn field_name(field: AuditField) -> String {
    match field {
        AuditField::Custom(id) => format!("fields.{}", Uuid::from_u128(id)),
        system => system.system_name().unwrap_or_default().to_owned(),
    }
}

/// The entries of the fields changed between `before` and `after`.
pub fn changes(before: &Snapshot<Json>, after: &Snapshot<Json>) -> Vec<Entry> {
    diff(before, after)
        .into_iter()
        .map(|Change { field, old, new }| Entry::field(field_name(field), old, new))
        .collect()
}

/// The entry of a ticket moved from `from` to `to` (keys).
pub fn moved(from: &str, to: &str) -> Entry {
    Entry {
        old: Some(json!({"key": from})),
        new: Some(json!({"key": to})),
        ..Entry::action(Action::Moved)
    }
}

/// The entry of a link added or removed, as it reads from the ticket whose
/// history it goes to.
pub fn link(added: bool, relation: Relation, other: Uuid) -> Entry {
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
pub fn comment(action: Action, id: Uuid, old: Option<&Json>, new: Option<&Json>) -> Entry {
    let value = |content: Option<&Json>| content.map(|c| json!({"commentId": id, "content": c}));
    Entry {
        old: value(old).or_else(|| (action != Action::CommentCreated).then(|| json!({"commentId": id}))),
        new: value(new),
        ..Entry::action(action)
    }
}

/// The entry of a file attached or removed: its id and name.
pub fn attachment(added: bool, id: Uuid, name: &str) -> Entry {
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
