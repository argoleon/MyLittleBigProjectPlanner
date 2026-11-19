//! Moving a ticket through its workflow (L05-US09): the transitions that
//! leave its status, the fields a transition takes, and what running one
//! does: its condition, the rules of the target status, its validators,
//! its post-functions in order, its comments, history and event.

use std::collections::{BTreeMap, BTreeSet};

use domain::comments::Visibility;
use domain::fields::{Step, Value};
use domain::issues::{Current, IssueChanges, missing_required, validate_changes};
use domain::rich_text::{Block, Doc, Inline};
use domain::screens::{FieldRules, resolve};
use domain::transitions::{Actor, Change, Condition, Refusal, Rules, Ticket, allowed, apply, evaluate};
use domain::validation::FieldError;
use sqlx::postgres::PgConnection;
use uuid::Uuid;

use super::content::{Content, field_defs, read_fields, stored_values, type_ids_of, unknown_references};
use crate::audit::Author;
use crate::comments::Source;
use crate::issues::{self as repo, Issue};
use crate::workflows::Transition;

/// Who runs transitions on a ticket of `project`: their account, roles
/// reaching the project, groups.
pub async fn actor_of(conn: &mut PgConnection, account: Uuid, project: Uuid) -> Result<Actor, sqlx::Error> {
    let roles = crate::grants::roles_reaching(conn, account, project).await?;
    let groups = crate::groups::of_members(conn, &[account]).await?;
    Ok(Actor {
        account: account.as_u128(),
        roles: roles.iter().map(Uuid::as_u128).collect(),
        groups: groups.iter().map(|(_, g)| g.id.as_u128()).collect(),
    })
}

/// The rules of `transition` as the actor meets them: a tenant admin meets
/// every condition.
pub fn rules_for(transition: &Transition, admin: bool) -> Rules {
    if admin {
        Rules {
            condition: Condition::default(),
            ..transition.rules.clone()
        }
    } else {
        transition.rules.clone()
    }
}

/// The transitions of the workflow of `issue`'s type that leave its status
/// (or any status).
pub fn leaving<'a>(content: &'a Content, issue: &Issue) -> Vec<&'a Transition> {
    let Some(workflow) = content
        .types
        .iter()
        .find(|t| t.item_id == issue.type_id)
        .and_then(|t| t.workflow_id)
    else {
        return Vec::new();
    };
    content
        .workflows
        .transitions_of(workflow)
        .filter(|t| t.from_status_id.is_none_or(|from| from == issue.status_id))
        .collect()
}

/// The fields `transition` lists: those of its transition screen, then its
/// validators.
pub fn wanted(content: &Content, transition: &Transition) -> Vec<u128> {
    let mut wanted: Vec<u128> = transition
        .screen_id
        .and_then(|screen| content.screens.iter().find(|s| s.item_id == screen))
        .map(|s| s.field_ids.iter().map(Uuid::as_u128).collect())
        .unwrap_or_default();
    for validator in &transition.rules.validators {
        if !wanted.contains(validator) {
            wanted.push(*validator);
        }
    }
    wanted
}

/// The fields `transition` takes for `issue`: those it lists and those
/// required at the target status, the rules of the target status applied.
pub fn screen_ids(content: &Content, transition: &Transition, issue: &Issue) -> BTreeSet<u128> {
    let type_ids = type_ids_of(&content.fields);
    let rules: Vec<FieldRules<'_>> = content
        .fields
        .iter()
        .zip(&type_ids)
        .map(|(f, types)| FieldRules {
            id: f.item_id.as_u128(),
            rules: domain::fields::Rules {
                all_types: f.all_types,
                issue_types: types,
                rules: &f.rules,
            },
        })
        .collect();
    resolve(
        &wanted(content, transition),
        &rules,
        issue.type_id.as_u128(),
        Step::Status(transition.to_status_id.as_u128()),
    )
    .into_iter()
    .map(|shown| shown.field)
    .collect()
}

/// Why a transition did not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Its condition is not met.
    Restricted,
    /// Field errors: a value refused, a validator's field empty, a field
    /// required at the target status missing.
    Invalid(Vec<FieldError>),
}

/// A transition about to run on a ticket of `project`.
#[derive(Debug, Clone, Copy)]
pub struct Run<'a> {
    pub project: Uuid,
    pub content: &'a Content,
    pub transition: &'a Transition,
    pub actor: &'a Actor,
    /// A tenant admin: every condition met.
    pub admin: bool,
}

/// Runs `run.transition` on `issue` (locked) as `account`, with the values
/// `fields` of its transition screen: the ticket in its new status, with
/// its comments, history and `issue.transitioned` event; or why not.
pub async fn perform(
    conn: &mut PgConnection,
    author: Author,
    account: Uuid,
    run: &Run<'_>,
    issue: Issue,
    fields: &BTreeMap<Uuid, serde_json::Value>,
) -> Result<Result<Issue, Refused>, sqlx::Error> {
    let Run {
        project,
        content,
        transition,
        actor,
        admin,
    } = *run;
    let rules = rules_for(transition, admin);
    if !allowed(&rules.condition, actor) {
        return Ok(Err(Refused::Restricted));
    }
    let type_id = issue.type_id.as_u128();
    let to = transition.to_status_id;

    // The values it takes: the fields it lists only, at the target status.
    let mut errors = Vec::new();
    let given = read_fields(fields).unwrap_or_else(|e| {
        errors.extend(e);
        BTreeMap::new()
    });
    let on_screen = screen_ids(content, transition, &issue);
    for id in fields.keys() {
        if !on_screen.contains(&id.as_u128()) {
            errors.push(FieldError::new(format!("fields.{id}"), "not_applicable"));
        }
    }
    let current_values = stored_values(&issue.fields);
    let type_ids = type_ids_of(&content.fields);
    let defs = field_defs(&content.fields, &type_ids);
    let changes = IssueChanges {
        summary: None,
        dates_in_order: true,
        fields: &given,
    };
    let target = Current {
        type_id,
        status_id: to.as_u128(),
        fields: &current_values,
    };
    if errors.is_empty()
        && let Err(e) = validate_changes(&changes, &target, &defs)
    {
        errors.extend(e);
    }
    let unknown = unknown_references(conn, &content.fields, &given, &errors).await?;
    errors.extend(unknown);
    if !errors.is_empty() {
        return Ok(Err(Refused::Invalid(errors)));
    }

    // The validators and post-functions, on the ticket with its screen values.
    let mut with_screen = current_values.clone();
    for (id, value) in &given {
        if value.is_empty() {
            with_screen.remove(id);
        } else {
            with_screen.insert(*id, value.clone());
        }
    }
    let ticket = Ticket {
        reporter: issue.reporter_id.map(|id| id.as_u128()),
        assignee: issue.assignee_id.map(|id| id.as_u128()),
        fields: with_screen,
    };
    let post = match evaluate(&rules, &ticket, actor) {
        Ok(post) => post,
        Err(refusals) => {
            if refusals.contains(&Refusal::NotAllowed) {
                return Ok(Err(Refused::Restricted));
            }
            return Ok(Err(Refused::Invalid(
                refusals
                    .iter()
                    .filter_map(|r| match r {
                        Refusal::FieldRequired(field) => Some(FieldError::new(
                            format!("fields.{}", Uuid::from_u128(*field)),
                            "required",
                        )),
                        Refusal::NotAllowed => None,
                    })
                    .collect(),
            )));
        }
    };
    let after = apply(&ticket, &post);
    let missing = missing_required(&defs, type_id, Step::Status(to.as_u128()), &after.fields);
    if !missing.is_empty() {
        return Ok(Err(Refused::Invalid(
            missing
                .iter()
                .map(|field| FieldError::new(format!("fields.{}", Uuid::from_u128(*field)), "required"))
                .collect(),
        )));
    }

    // What is stored: the screen values as given, then the post-functions.
    let mut stored = issue.fields.as_object().cloned().unwrap_or_default();
    for (id, json) in fields {
        if given.get(&id.as_u128()).is_some_and(Value::is_empty) {
            stored.remove(&id.to_string());
        } else {
            stored.insert(id.to_string(), json.clone());
        }
    }
    let mut comments = Vec::new();
    for change in &post {
        match change {
            Change::SetField { field, value } => {
                stored.insert(Uuid::from_u128(*field).to_string(), crate::fields::json_of(value));
            }
            Change::ClearField { field } => {
                stored.remove(&Uuid::from_u128(*field).to_string());
            }
            Change::AddComment { text } => comments.push(text.clone()),
            Change::Assign(_) => {}
        }
    }
    let assignee = after.assignee.map(Uuid::from_u128);
    let mut diff = Vec::new();
    if assignee != issue.assignee_id {
        diff.push(serde_json::json!({"field": "assigneeId", "old": issue.assignee_id, "new": assignee}));
    }
    let ids: BTreeSet<&String> = stored
        .keys()
        .chain(issue.fields.as_object().into_iter().flatten().map(|(k, _)| k))
        .collect();
    for id in ids {
        let old = issue.fields.get(id).cloned().unwrap_or_default();
        let new = stored.get(id).cloned().unwrap_or_default();
        if old != new {
            diff.push(serde_json::json!({"field": format!("fields.{id}"), "old": old, "new": new}));
        }
    }
    let stored = serde_json::Value::Object(stored);
    let from = issue.status_id;
    let moved = repo::transition(conn, issue.id, to, assignee, &stored).await?;
    let entries = super::history::changes(
        &super::history::snapshot(&issue, content),
        &super::history::snapshot(&moved, content),
    );
    crate::audit::record(conn, moved.id, author, &entries).await?;
    if let Some(assignee) = moved.assignee_id.filter(|a| Some(*a) != issue.assignee_id) {
        crate::watchers::watch(conn, moved.id, assignee).await?;
    }
    for text in &comments {
        let doc = Doc(vec![Block::Paragraph(vec![Inline::Text {
            text: text.clone(),
            marks: Vec::new(),
        }])]);
        super::comments::write(
            conn,
            &author,
            account,
            &moved,
            Source::Transition,
            Visibility::Internal,
            &doc,
        )
        .await?;
    }
    crate::outbox::record(
        conn,
        &crate::outbox::Event {
            kind: "issue.transitioned",
            aggregate_type: "issue",
            aggregate_id: moved.id,
            payload: serde_json::json!({
                "key": moved.key,
                "projectId": project,
                "transitionId": transition.item_id,
                "fromStatusId": from,
                "toStatusId": to,
                "changes": diff,
                "comments": comments,
            }),
            actor_id: Some(account),
        },
    )
    .await?;
    tracing::info!(target: "audit", actor = %account, issue = %moved.id, key = %moved.key, transition = %transition.item_id, "ticket transitioned");
    Ok(Ok(moved))
}
