//! Bulk actions on a search result (specification §6, §5, L06-US08):
//! several tickets modified, moved through their workflow, or moved to
//! another project at once.
//!
//! The selection is tickets ticked (ids) or all the results of a query,
//! the tickets the member sees only. Up to 100, the request changes them,
//! each in its own transaction, and answers the report; beyond, the worker
//! does, one job per ticket, and the member is notified when done. Each
//! ticket is checked on its own: visible, editable by the member, its
//! project active, its version locked; a refused one is reported, never
//! changed, and never undoes the others.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use domain::bulk::{LIFETIME_DAYS, MAX, Outcome, Run, follows_parent, run_of, shared};
use domain::fields::Step;
use domain::issues::Priority;
use domain::validation::FieldError;
use infra::bulk_actions::{self as repo, Action, BulkAction, Context, Item, Mapping, ModifyParams, Selected};
use infra::full_text::Audience;
use infra::issues::Issue;
use infra::search::Context as SearchContext;
use infra::tickets::moves::Prepared;
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use time::{Date, Duration, OffsetDateTime};
use utoipa::ToSchema;

use crate::extract::{AcceptLanguage, ApiJson, ApiPath};
use crate::issue_changes::{
    DroppedFieldView, MappingBody, ProblemView, StatusNeedView, StatusTargetView, TypeNeedView,
    TypeTargetView, mappings, present,
};
use crate::issues::{Content, FormFieldView, PriorityView, ProjectView, fields_shown, project_view};
use crate::nodes::Sight;
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::search::{query_problem, search_problem};
use crate::tenant::{AppState, TenantTx};

pub const TAG: &str = "bulk-actions";

/// The tickets a bulk action takes: those ticked, or all the results of a
/// query.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = BulkSelection)]
pub struct SelectionBody {
    /// The tickets ticked (up to 10,000).
    #[serde(default)]
    issue_ids: Vec<Uuid>,
    /// Or every ticket a query of the language finds.
    #[serde(default)]
    query: Option<String>,
}

/// What a bulk action does.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[schema(as = BulkActionFields)]
pub enum ActionBody {
    /// Sets what is given on every ticket; `null` clears.
    #[serde(rename_all = "camelCase")]
    Modify {
        #[serde(default)]
        priority: Option<PriorityView>,
        #[serde(default, deserialize_with = "present")]
        #[schema(value_type = Option<Uuid>)]
        assignee_id: Option<Option<Uuid>>,
        #[serde(default, deserialize_with = "present")]
        #[schema(value_type = Option<String>, format = Date)]
        start_date: Option<Option<Date>>,
        #[serde(default, deserialize_with = "present")]
        #[schema(value_type = Option<String>, format = Date)]
        due_date: Option<Option<Date>>,
        /// Custom field values by field id (fields the selected tickets
        /// share, see `options`); `null` clears one.
        #[serde(default)]
        fields: BTreeMap<Uuid, serde_json::Value>,
    },
    /// Runs on every ticket the first transition of its workflow leading
    /// to the status `statusKey`.
    #[serde(rename_all = "camelCase")]
    Transition { status_key: String },
    /// Moves every ticket to a project (its sub-tasks follow), types and
    /// statuses mapped once for the batch (see `move-plan`).
    #[serde(rename_all = "camelCase")]
    Move {
        project_key: String,
        #[serde(default)]
        type_mappings: Vec<MappingBody>,
        #[serde(default)]
        status_mappings: Vec<MappingBody>,
    },
}

/// A bulk action to run.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = NewBulkAction, example = json!({
    "selection": {"issueIds": ["01933b2e-7c4a-7b9e-8f3d-2a6c5e1f0b9a"]},
    "action": {"kind": "transition", "statusKey": "done"}
}))]
pub struct BulkBody {
    selection: SelectionBody,
    action: ActionBody,
}

/// A selection to plan a move for.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = BulkMovePlanFields)]
pub struct MovePlanBody {
    selection: SelectionBody,
    project_key: String,
    #[serde(default)]
    type_mappings: Vec<MappingBody>,
    #[serde(default)]
    status_mappings: Vec<MappingBody>,
}

/// A status some transition reaches from a selected ticket's status.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BulkStatusOption)]
pub struct StatusOptionView {
    #[schema(example = "done")]
    key: String,
    label: Option<String>,
    category: crate::workflows::CategoryView,
    /// How many selected tickets a transition leads there from.
    reachable: usize,
}

/// What a bulk action can do on a selection.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BulkOptions)]
pub struct OptionsView {
    /// The tickets selected that the member sees.
    total: usize,
    /// Changed within the request (up to 100), else in the background.
    background: bool,
    /// The custom fields every selected ticket has (one configuration).
    fields: Vec<FormFieldView>,
    /// The statuses a transition reaches from some selected ticket.
    statuses: Vec<StatusOptionView>,
}

/// What moving a selection to a project takes.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BulkMovePlan)]
pub struct MovePlanView {
    project: ProjectView,
    /// Every ticket's configuration is the target's: nothing to map.
    same_configuration: bool,
    /// The tickets selected that move (their sub-tasks follow them).
    tickets: usize,
    /// Sub-tasks selected without their parent: they stay.
    subtasks_alone: usize,
    /// The types of the target configuration.
    targets: Vec<TypeTargetView>,
    /// The statuses of the target configuration.
    status_targets: Vec<StatusTargetView>,
    /// The types to map, with a suggestion, across the selection.
    types: Vec<TypeNeedView>,
    /// The statuses to map, with a suggestion, across the selection.
    statuses: Vec<StatusNeedView>,
    /// With the mappings given (else the suggestions): the fields whose
    /// values are dropped.
    dropped_fields: Vec<DroppedFieldView>,
    /// With the mappings given (else the suggestions): what the move would
    /// refuse.
    problems: Vec<ProblemView>,
}

/// Where a bulk action stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = BulkStatus)]
pub enum StatusView {
    Running,
    Done,
}

/// What became of a ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = BulkOutcome)]
pub enum OutcomeView {
    Changed,
    Unchanged,
    Refused,
    /// Moved with its parent, selected too.
    WithParent,
}

impl From<Outcome> for OutcomeView {
    fn from(outcome: Outcome) -> Self {
        match outcome {
            Outcome::Changed => Self::Changed,
            Outcome::Unchanged => Self::Unchanged,
            Outcome::Refused => Self::Refused,
            Outcome::WithParent => Self::WithParent,
        }
    }
}

/// A field error of a ticket refused for its values.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BulkItemError)]
pub struct ItemErrorView {
    field: String,
    code: String,
}

/// A ticket of a bulk action.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BulkItem)]
pub struct ItemView {
    issue_id: Uuid,
    /// Its key when selected.
    #[schema(example = "PLAT-142")]
    key: String,
    /// Its key once moved.
    new_key: Option<String>,
    /// `null` while waiting.
    outcome: Option<OutcomeView>,
    /// Why it was refused: `not_found`, `permission.denied`,
    /// `node.archived`, `validation_failed`, `transition.none`,
    /// `transition.restricted`, `issue.subtask_moves`, `move.not_mapped`.
    reason: Option<String>,
    errors: Vec<ItemErrorView>,
}

/// The counts of a bulk action's outcomes.
#[derive(Debug, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BulkCounts)]
pub struct CountsView {
    changed: usize,
    unchanged: usize,
    refused: usize,
    with_parent: usize,
    waiting: usize,
}

/// A bulk action and its report.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = BulkAction)]
pub struct BulkView {
    id: Uuid,
    /// `modify`, `transition` or `move`.
    #[schema(example = "transition")]
    kind: String,
    status: StatusView,
    background: bool,
    total: i32,
    counts: CountsView,
    items: Vec<ItemView>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    completed_at: Option<OffsetDateTime>,
    /// When the report is deleted.
    #[serde(with = "time::serde::rfc3339")]
    expires_at: OffsetDateTime,
}

fn bulk_view(bulk: &BulkAction, items: Vec<Item>) -> BulkView {
    let mut counts = CountsView::default();
    for item in &items {
        match item.outcome {
            Some(Outcome::Changed) => counts.changed += 1,
            Some(Outcome::Unchanged) => counts.unchanged += 1,
            Some(Outcome::Refused) => counts.refused += 1,
            Some(Outcome::WithParent) => counts.with_parent += 1,
            None => counts.waiting += 1,
        }
    }
    BulkView {
        id: bulk.id,
        kind: bulk.action.kind().as_str().to_owned(),
        status: if bulk.remaining == 0 {
            StatusView::Done
        } else {
            StatusView::Running
        },
        background: bulk.background,
        total: bulk.total,
        counts,
        items: items
            .into_iter()
            .map(|i| ItemView {
                issue_id: i.issue_id,
                key: i.issue_key,
                new_key: i.new_key,
                outcome: i.outcome.map(Into::into),
                reason: i.reason,
                errors: i
                    .errors
                    .into_iter()
                    .map(|(field, code)| ItemErrorView { field, code })
                    .collect(),
            })
            .collect(),
        created_at: bulk.created_at,
        completed_at: bulk.completed_at,
        expires_at: bulk.expires_at,
    }
}

/// The tickets of `selection` the member sees, in its order (ids) or the
/// query's; up to [`MAX`] + 1, to tell when there are too many.
async fn resolve(tx: &mut TenantTx, selection: &SelectionBody) -> Result<Vec<Issue>, Problem> {
    let account = tx.account_id();
    let sight = Sight::of(tx).await?;
    let visible = sight.visible();
    match (&selection.query, selection.issue_ids.is_empty()) {
        (Some(query), true) => {
            let query = domain::query::parse(query).map_err(|e| query_problem(&e))?;
            let time_zone = infra::search::time_zone_of(tx, account).await?;
            let context = SearchContext {
                account_id: account,
                time_zone: &time_zone,
                visible_projects: visible.as_deref(),
                audience: Audience::Members,
            };
            Ok(infra::search::search(tx, &query, &context, None, MAX + 1, false)
                .await
                .map_err(search_problem)?
                .items)
        }
        (None, false) => {
            if selection.issue_ids.len() > MAX {
                return Err(too_many(selection.issue_ids.len()));
            }
            let found = infra::issues::find_many(tx, &selection.issue_ids).await?;
            let by_id: HashMap<Uuid, Issue> = found
                .into_iter()
                .filter(|i| sight.sees(i.project_id))
                .map(|i| (i.id, i))
                .collect();
            let mut seen = BTreeSet::new();
            Ok(selection
                .issue_ids
                .iter()
                .filter(|id| seen.insert(**id))
                .filter_map(|id| by_id.get(id).cloned())
                .collect())
        }
        _ => Err(Problem::validation(vec![FieldError::new(
            "selection",
            "required",
        )])),
    }
}

fn too_many(count: usize) -> Problem {
    Problem::new(ErrorCode::BulkTooMany).with_detail(format!(
        "{count} tickets selected; a bulk action takes {MAX} at most"
    ))
}

/// `POST /api/v1/bulk-actions/options`: changes nothing.
#[utoipa::path(
    post,
    path = "/bulk-actions/options",
    tag = TAG,
    operation_id = "getBulkOptions",
    summary = "What a bulk action can do on a selection",
    description = "The tickets selected that the member sees, whether they change in the request \
                   (up to 100) or in the background, the custom fields they all have (tickets of \
                   one configuration), and the statuses a transition reaches from their statuses.",
    params(("Accept-Language" = Option<String>, Header, description = "Language of the labels")),
    request_body = SelectionBody,
    responses(
        (status = 200, description = "The options", body = OptionsView),
        (status = 400, description = "`validation_failed` (`selection` `required`), or `query.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`bulk.too_many`: more than 10,000 tickets", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn options(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiJson(body): ApiJson<SelectionBody>,
) -> Result<Response, Problem> {
    let issues = resolve(&mut tx, &body).await?;
    if issues.len() > MAX {
        return Err(too_many(issues.len()));
    }
    let language = Language::of(&tx, &accept);
    let mut contents: HashMap<Uuid, Content> = HashMap::new();
    for project in issues.iter().map(|i| i.project_id).collect::<BTreeSet<_>>() {
        contents.insert(project, Content::of(&mut tx, project).await?);
    }
    tx.commit().await?;

    // The fields each ticket's type has; their ids are those of its
    // configuration, so tickets of several configurations share none.
    let each: Vec<BTreeSet<u128>> = issues
        .iter()
        .map(|issue| {
            contents
                .get(&issue.project_id)
                .map_or_else(BTreeSet::new, |content| {
                    content
                        .fields
                        .iter()
                        .filter(|f| f.all_types || f.issue_type_ids.contains(&issue.type_id))
                        .map(|f| f.item_id.as_u128())
                        .collect()
                })
        })
        .collect();
    let shared_ids: Vec<u128> = shared(&each).into_iter().collect();
    let fields = match issues
        .first()
        .and_then(|i| contents.get(&i.project_id).map(|c| (i, c)))
    {
        Some((first, content)) if !shared_ids.is_empty() => {
            let ordered: Vec<u128> = content
                .fields
                .iter()
                .map(|f| f.item_id.as_u128())
                .filter(|id| shared_ids.contains(id))
                .collect();
            fields_shown(
                &language,
                content,
                &ordered,
                first.type_id.as_u128(),
                Step::Status(first.status_id.as_u128()),
                &BTreeSet::new(),
            )
        }
        _ => Vec::new(),
    };
    // The statuses some transition reaches, by key, with how many tickets.
    let mut statuses: BTreeMap<String, (StatusOptionView, usize)> = BTreeMap::new();
    for issue in &issues {
        let Some(content) = contents.get(&issue.project_id) else {
            continue;
        };
        let reached: BTreeSet<Uuid> = infra::tickets::transition::leaving(content, issue)
            .iter()
            .map(|t| t.to_status_id)
            .filter(|s| *s != issue.status_id)
            .collect();
        for status in reached {
            let Some(found) = content.workflows.statuses.iter().find(|s| s.item_id == status) else {
                continue;
            };
            statuses
                .entry(found.key.clone())
                .or_insert_with(|| {
                    (
                        StatusOptionView {
                            key: found.key.clone(),
                            label: language.label(&found.label_translations),
                            category: crate::workflows::CategoryView::of(&found.category),
                            reachable: 0,
                        },
                        0,
                    )
                })
                .1 += 1;
        }
    }
    let statuses = statuses
        .into_values()
        .map(|(view, reachable)| StatusOptionView { reachable, ..view })
        .collect();
    let mut response = ApiJson(OptionsView {
        total: issues.len(),
        background: run_of(issues.len()) == Run::Background,
        fields,
        statuses,
    })
    .into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `POST /api/v1/bulk-actions/move-plan`: changes nothing.
#[utoipa::path(
    post,
    path = "/bulk-actions/move-plan",
    tag = TAG,
    operation_id = "planBulkMove",
    summary = "What moving a selection to a project takes",
    description = "The types and statuses of the selected tickets (and their sub-tasks) the target \
                   configuration lacks, mapped once for the batch with a suggestion each, and, with \
                   the mappings given (else the suggestions), the fields whose values are dropped \
                   and what the move would refuse. Sub-tasks selected without their parent stay.",
    params(("Accept-Language" = Option<String>, Header, description = "Language of the labels")),
    request_body = MovePlanBody,
    responses(
        (status = 200, description = "The plan", body = MovePlanView),
        (status = 400, description = "`validation_failed`: `selection` (`required`), `projectKey` (`not_found`); or `query.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`bulk.too_many`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn move_plan(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiJson(body): ApiJson<MovePlanBody>,
) -> Result<Response, Problem> {
    let issues = resolve(&mut tx, &body.selection).await?;
    if issues.len() > MAX {
        return Err(too_many(issues.len()));
    }
    let sight = Sight::of(&mut tx).await?;
    let target = match infra::nodes::project_of_key(&mut tx, &body.project_key).await? {
        Some(id) => infra::nodes::find(&mut tx, id)
            .await?
            .filter(|n| sight.sees(n.id)),
        None => None,
    }
    .ok_or_else(|| Problem::validation(vec![FieldError::new("projectKey", "not_found")]))?;
    let language = Language::of(&tx, &accept);
    let selected: BTreeSet<u128> = issues.iter().map(|i| i.id.as_u128()).collect();
    let target_content = Content::of(&mut tx, target.id).await?;
    let target_config = infra::configs::of_project(&mut tx, target.id).await?;
    // One plan per ticket moving (its sub-tasks with it), as each moves;
    // the configurations read once.
    let target_content = std::sync::Arc::new(target_content);
    let mut sources: HashMap<Uuid, std::sync::Arc<Content>> = HashMap::new();
    let mut subtasks_alone = 0;
    let mut tickets = 0;
    let mut same_configuration = true;
    let mut plans = Vec::new();
    for issue in issues {
        if issue.project_id == target.id || follows_parent(issue.parent_id.map(|p| p.as_u128()), &selected) {
            continue;
        }
        let source_content = match sources.get(&issue.project_id) {
            Some(content) => content.clone(),
            None => {
                let content = std::sync::Arc::new(Content::of(&mut tx, issue.project_id).await?);
                same_configuration &=
                    infra::configs::of_project(&mut tx, issue.project_id).await? == target_config;
                sources.insert(issue.project_id, content.clone());
                content
            }
        };
        if source_content
            .types
            .iter()
            .any(|t| t.item_id == issue.type_id && t.is_subtask)
        {
            subtasks_alone += 1;
            continue;
        }
        tickets += 1;
        let children = infra::issues::children(&mut tx, issue.id).await?;
        let mut moving = vec![issue];
        moving.extend(children);
        plans.push(Prepared {
            target: target.clone(),
            source_content,
            target_content: target_content.clone(),
            same_configuration,
            moving,
        });
    }
    tx.commit().await?;

    let given_types = mappings(&body.type_mappings);
    let given_statuses = mappings(&body.status_mappings);
    let mut type_needs: BTreeMap<Uuid, TypeNeedView> = BTreeMap::new();
    let mut status_needs: BTreeMap<Uuid, StatusNeedView> = BTreeMap::new();
    let mut dropped: BTreeMap<Uuid, DroppedFieldView> = BTreeMap::new();
    let mut problems: BTreeSet<(String, &'static str)> = BTreeSet::new();
    for plan in &plans {
        let suggested = plan.suggested_types();
        let mut types = suggested.clone();
        types.extend(given_types.iter().map(|(k, v)| (*k, *v)));
        for t in plan.source_types_needed() {
            type_needs.entry(t.item_id).or_insert_with(|| TypeNeedView {
                id: t.item_id,
                key: t.key.clone(),
                label: language.label(&t.label_translations),
                is_subtask: t.is_subtask,
                suggested_id: suggested.get(&t.item_id.as_u128()).map(|id| Uuid::from_u128(*id)),
            });
        }
        let needed = plan.statuses_needed(&types);
        let mut statuses: BTreeMap<u128, u128> = needed
            .iter()
            .filter_map(|(s, _)| Some((s.item_id.as_u128(), plan.suggested_status(s, &types)?)))
            .collect();
        statuses.extend(given_statuses.iter().map(|(k, v)| (*k, *v)));
        for (s, type_ids) in &needed {
            let view = status_needs.entry(s.item_id).or_insert_with(|| StatusNeedView {
                id: s.item_id,
                key: s.key.clone(),
                label: language.label(&s.label_translations),
                category: crate::workflows::CategoryView::of(&s.category),
                type_ids: Vec::new(),
                suggested_id: plan.suggested_status(s, &types).map(Uuid::from_u128),
            });
            for id in type_ids {
                if !view.type_ids.contains(id) {
                    view.type_ids.push(*id);
                }
            }
        }
        match plan.landings(&types, &statuses) {
            Ok(landings) => {
                for id in plan.kept(&landings).1 {
                    dropped.entry(id).or_insert_with(|| DroppedFieldView {
                        field_id: id,
                        label: plan
                            .source_content
                            .fields
                            .iter()
                            .find(|f| f.item_id == id)
                            .and_then(|f| language.label(&f.label_translations)),
                    });
                }
            }
            Err(errors) => problems.extend(errors.into_iter().map(|e| (e.field, e.code))),
        }
    }
    let view = MovePlanView {
        project: project_view(&target),
        same_configuration,
        tickets,
        subtasks_alone,
        targets: target_content
            .types
            .iter()
            .map(|t| TypeTargetView {
                id: t.item_id,
                label: language.label(&t.label_translations),
                is_subtask: t.is_subtask,
                parent_ids: t.parent_ids.clone(),
                status_ids: t
                    .workflow_id
                    .map(|w| {
                        target_content
                            .workflows
                            .statuses_of(w)
                            .map(|s| s.item_id)
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect(),
        status_targets: target_content
            .workflows
            .statuses
            .iter()
            .map(|s| StatusTargetView {
                id: s.item_id,
                label: language.label(&s.label_translations),
                category: crate::workflows::CategoryView::of(&s.category),
            })
            .collect(),
        types: type_needs.into_values().collect(),
        statuses: status_needs.into_values().collect(),
        dropped_fields: dropped.into_values().collect(),
        problems: problems
            .into_iter()
            .map(|(field, code)| ProblemView { field, code })
            .collect(),
    };
    let mut response = ApiJson(view).into_response();
    language.headers(&mut response);
    Ok(response)
}

/// The action as stored, checked as far as it can be before any ticket.
fn action_of(body: &ActionBody) -> Result<Action, Problem> {
    Ok(match body {
        ActionBody::Modify {
            priority,
            assignee_id,
            start_date,
            due_date,
            fields,
        } => {
            if priority.is_none()
                && assignee_id.is_none()
                && start_date.is_none()
                && due_date.is_none()
                && fields.is_empty()
            {
                return Err(Problem::validation(vec![FieldError::new("action", "required")]));
            }
            Action::Modify(ModifyParams {
                priority: priority.map(|p| Priority::from(p).as_str().to_owned()),
                assignee_id: *assignee_id,
                start_date: *start_date,
                due_date: *due_date,
                fields: fields.clone(),
            })
        }
        ActionBody::Transition { status_key } => {
            if status_key.trim().is_empty() {
                return Err(Problem::validation(vec![FieldError::new(
                    "action.statusKey",
                    "required",
                )]));
            }
            Action::Transition {
                status_key: status_key.trim().to_owned(),
            }
        }
        ActionBody::Move {
            project_key,
            type_mappings,
            status_mappings,
        } => {
            let map = |body: &[MappingBody]| {
                body.iter()
                    .map(|m| Mapping {
                        from_id: m.from_id,
                        to_id: m.to_id,
                    })
                    .collect()
            };
            Action::Move {
                project_key: project_key.trim().to_owned(),
                type_mappings: map(type_mappings),
                status_mappings: map(status_mappings),
            }
        }
    })
}

/// `POST /api/v1/bulk-actions`.
#[utoipa::path(
    post,
    path = "/bulk-actions",
    tag = TAG,
    operation_id = "createBulkAction",
    summary = "Change several tickets at once",
    description = "Any member, on the tickets they see. Each ticket is checked and changed on its \
                   own, in its own transaction, as its own edit, transition or move would be: \
                   `edit` on its project (and `create_ticket` on the target of a move), its project \
                   active, its version locked. A refused ticket is reported, never changed, and \
                   never undoes the others. Up to 100 tickets: changed within the request, the \
                   report answered (`200`). Beyond (up to 10,000): changed in the background \
                   (`202`), the member notified when done.",
    params(("Accept-Language" = Option<String>, Header, description = "Language of the report")),
    request_body = BulkBody,
    responses(
        (status = 200, description = "Done: the report", body = BulkView),
        (status = 202, description = "Running in the background", body = BulkView),
        (status = 400, description = "`validation_failed`: `selection` (`required`: nothing selected or visible), `action` (`required`: nothing to modify), `action.statusKey`, `projectKey` (`not_found`); or `query.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 422, description = "`bulk.too_many`: more than 10,000 tickets", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiJson(body): ApiJson<BulkBody>,
) -> Result<Response, Problem> {
    let action = action_of(&body.action)?;
    if let Action::Move { project_key, .. } = &action
        && infra::nodes::project_of_key(&mut tx, project_key)
            .await?
            .is_none()
    {
        return Err(Problem::validation(vec![FieldError::new(
            "projectKey",
            "not_found",
        )]));
    }
    let issues = resolve(&mut tx, &body.selection).await?;
    let background = match run_of(issues.len()) {
        Run::Empty => {
            return Err(Problem::validation(vec![FieldError::new(
                "selection",
                "required",
            )]));
        }
        Run::TooMany => return Err(too_many(issues.len())),
        Run::Now => false,
        Run::Background => true,
    };
    let selected: Vec<Selected> = issues
        .iter()
        .map(|i| Selected {
            id: i.id,
            key: i.key.clone(),
        })
        .collect();
    let account = tx.account_id();
    let tenant = tx.tenant_id();
    let language = Language::of(&tx, &accept);
    let expires_at = OffsetDateTime::now_utc() + Duration::days(LIFETIME_DAYS);
    let (bulk, items) = repo::insert(
        &mut tx,
        &repo::NewBulk {
            id: infra::id::new_id(),
            account,
            action: &action,
            language: language.locale(),
            background,
            selected: &selected,
            expires_at,
        },
    )
    .await?;
    let lifetime = std::time::Duration::from_secs(u64::try_from(LIFETIME_DAYS).unwrap_or(7) * 86_400);
    let purge =
        serde_json::to_value(repo::PurgeJob { bulk_id: bulk.id }).map_err(|e| Problem::internal(&e))?;
    infra::jobs::enqueue_in(&mut tx, repo::PURGE_JOB, &purge, lifetime).await?;
    if background {
        for item in &items {
            let job = serde_json::to_value(repo::ItemJob {
                bulk_id: bulk.id,
                item_id: item.id,
            })
            .map_err(|e| Problem::internal(&e))?;
            infra::jobs::enqueue(&mut tx, repo::ITEM_JOB, &job).await?;
        }
    }
    tracing::info!(target: "audit", actor = %account, bulk = %bulk.id, kind = bulk.action.kind().as_str(), total = bulk.total, background, "bulk action asked");
    tx.commit().await?;
    if background {
        let mut response = (StatusCode::ACCEPTED, ApiJson(bulk_view(&bulk, items))).into_response();
        language.headers(&mut response);
        return Ok(response);
    }

    // Within the request: each ticket in its own transaction.
    let mut setup = infra::db::begin_tenant_tx(&state.pool, tenant).await?;
    let mut ctx = Context::of(&mut setup, &bulk).await?;
    setup.commit().await?;
    for item in &items {
        let mut ticket = infra::db::begin_tenant_tx(&state.pool, tenant).await?;
        repo::run_item(&mut ticket, &mut ctx, &bulk, item).await?;
        ticket.commit().await?;
    }
    let mut read = infra::db::begin_tenant_tx(&state.pool, tenant).await?;
    let bulk = repo::find(&mut read, bulk.id)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::Internal))?;
    let items = repo::items_of(&mut read, bulk.id).await?;
    read.commit().await?;
    let mut response = ApiJson(bulk_view(&bulk, items)).into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `GET /api/v1/bulk-actions/{id}`.
#[utoipa::path(
    get,
    path = "/bulk-actions/{id}",
    tag = TAG,
    operation_id = "getBulkAction",
    summary = "A bulk action and its report",
    description = "Its requester only, for 7 days: what became of each ticket, so far.",
    params(("id" = Uuid, Path, description = "Id of the bulk action")),
    responses(
        (status = 200, description = "The bulk action", body = BulkView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such bulk action of the member, or expired", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn read(mut tx: TenantTx, ApiPath(id): ApiPath<Uuid>) -> Result<ApiJson<BulkView>, Problem> {
    let account = tx.account_id();
    let bulk = repo::find_of(&mut tx, account, id)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such bulk action"))?;
    let items = repo::items_of(&mut tx, id).await?;
    tx.commit().await?;
    Ok(ApiJson(bulk_view(&bulk, items)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(json: serde_json::Value) -> ActionBody {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_modify_action_tells_absent_from_null() {
        let action = action_of(&body(
            serde_json::json!({"kind": "modify", "assigneeId": null, "priority": "high"}),
        ))
        .unwrap();
        let Action::Modify(params) = action else {
            panic!("a modify action");
        };
        assert_eq!(params.assignee_id, Some(None));
        assert_eq!(params.priority.as_deref(), Some("high"));
        assert_eq!(params.due_date, None);
        let nothing = action_of(&body(serde_json::json!({"kind": "modify"}))).unwrap_err();
        assert_eq!(nothing.code, ErrorCode::ValidationFailed);
    }

    #[test]
    fn transitions_and_moves_are_read() {
        assert_eq!(
            action_of(&body(
                serde_json::json!({"kind": "transition", "statusKey": " done "})
            ))
            .unwrap(),
            Action::Transition {
                status_key: "done".to_owned()
            }
        );
        assert!(action_of(&body(serde_json::json!({"kind": "transition", "statusKey": ""}))).is_err());
        let moved = action_of(&body(serde_json::json!({"kind": "move", "projectKey": "CRM"}))).unwrap();
        assert_eq!(moved.kind(), domain::bulk::Kind::Move);
        assert!(serde_json::from_value::<ActionBody>(serde_json::json!({"kind": "delete"})).is_err());
    }
}
