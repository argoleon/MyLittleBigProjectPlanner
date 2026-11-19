//! Editing a ticket (L05-US02): what is given replaces its value, checked
//! against the rules of its type and status; its history, watchers and
//! `issue.updated` event follow.

use std::collections::BTreeMap;

use domain::fields::Value;
use domain::issues::{Current, IssueChanges, Priority, validate_changes};
use domain::validation::FieldError;
use sqlx::postgres::PgConnection;
use time::Date;
use uuid::Uuid;

use super::content::{
    Content, field_defs, read_description, read_fields, stored_values, type_ids_of, unknown_references,
};
use crate::audit::Author;
use crate::issues::{self as repo, Issue};

/// What an edit changes: each given value replaces the ticket's, `Some(None)`
/// clears it.
#[derive(Debug, Clone, Default)]
pub struct Edit<'a> {
    pub summary: Option<&'a str>,
    /// A rich text document as sent; `Some(None)` removes the description.
    pub description: Option<Option<serde_json::Value>>,
    pub priority: Option<Priority>,
    pub assignee: Option<Option<Uuid>>,
    pub start_date: Option<Option<Date>>,
    pub due_date: Option<Option<Date>>,
    /// Custom field values by field id; JSON `null` clears one.
    pub fields: BTreeMap<Uuid, serde_json::Value>,
}

/// The ticket after an edit, and whether anything changed.
#[derive(Debug, Clone)]
pub struct Edited {
    pub issue: Issue,
    pub changed: bool,
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

/// Applies `edit` to `issue` (locked, of `project`, its configuration
/// `content`) as `account`: the field errors if any, else the ticket as
/// stored, with its history, watchers and `issue.updated` event when
/// anything changed.
pub async fn apply(
    conn: &mut PgConnection,
    author: Author,
    account: Uuid,
    project: Uuid,
    issue: Issue,
    content: &Content,
    edit: &Edit<'_>,
) -> Result<Result<Edited, Vec<FieldError>>, sqlx::Error> {
    let mut errors = Vec::new();
    let changed = read_fields(&edit.fields).unwrap_or_else(|e| {
        errors.extend(e);
        BTreeMap::new()
    });
    let description = match &edit.description {
        None => issue.description.clone(),
        Some(None) => None,
        Some(Some(json)) => read_description(json).unwrap_or_else(|e| {
            errors.push(e);
            None
        }),
    };
    let assignee = edit.assignee.unwrap_or(issue.assignee_id);
    if let Some(Some(member)) = edit.assignee
        && crate::rule_references::members(conn, &[member]).await?.is_empty()
    {
        errors.push(FieldError::new("assigneeId", "unknown_member"));
    }
    let start_date = edit.start_date.unwrap_or(issue.start_date);
    let due_date = edit.due_date.unwrap_or(issue.due_date);
    let dates_in_order = match (start_date, due_date) {
        (Some(start), Some(due)) => start <= due,
        _ => true,
    };
    let current_values = stored_values(&issue.fields);
    let type_ids = type_ids_of(&content.fields);
    let defs = field_defs(&content.fields, &type_ids);
    let changes = IssueChanges {
        summary: edit.summary,
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
    let unknown = unknown_references(conn, &content.fields, &changed, &errors).await?;
    errors.extend(unknown);
    if !errors.is_empty() {
        return Ok(Err(errors));
    }

    // The values once changed, each a field given replacing or clearing it.
    let mut stored = issue.fields.as_object().cloned().unwrap_or_default();
    for (id, json) in &edit.fields {
        if changed.get(&id.as_u128()).is_some_and(Value::is_empty) {
            stored.remove(&id.to_string());
        } else {
            stored.insert(id.to_string(), json.clone());
        }
    }
    let summary = edit.summary.map_or(issue.summary.as_str(), str::trim);
    let priority = edit
        .priority
        .map_or(issue.priority.clone(), |p| p.as_str().to_owned());
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
    for id in edit.fields.keys() {
        let field = id.to_string();
        let old = issue.fields.get(&field).cloned().unwrap_or_default();
        let new = stored.get(&field).cloned().unwrap_or_default();
        diff.extend(change(&format!("fields.{id}"), old, new));
    }
    if diff.is_empty() {
        return Ok(Ok(Edited {
            issue,
            changed: false,
        }));
    }
    let stored = serde_json::Value::Object(stored);
    let updated = repo::update(
        conn,
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
    let entries = super::history::changes(
        &super::history::snapshot(&issue, content),
        &super::history::snapshot(&updated, content),
    );
    crate::audit::record(conn, updated.id, author, &entries).await?;
    // A new assignee follows the ticket.
    if let Some(assignee) = updated.assignee_id.filter(|a| Some(*a) != issue.assignee_id) {
        crate::watchers::watch(conn, updated.id, assignee).await?;
    }
    crate::outbox::record(
        conn,
        &crate::outbox::Event {
            kind: "issue.updated",
            aggregate_type: "issue",
            aggregate_id: updated.id,
            payload: serde_json::json!({"key": updated.key, "projectId": project, "changes": diff}),
            actor_id: Some(account),
        },
    )
    .await?;
    tracing::info!(target: "audit", actor = %account, issue = %updated.id, key = %updated.key, changes = diff.len(), "ticket edited");
    Ok(Ok(Edited {
        issue: updated,
        changed: true,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_changed_values_are_reported() {
        assert_eq!(change("summary", "a".into(), "a".into()), None);
        assert_eq!(
            change("priority", "medium".into(), "high".into()),
            Some(serde_json::json!({"field": "priority", "old": "medium", "new": "high"}))
        );
    }
}
