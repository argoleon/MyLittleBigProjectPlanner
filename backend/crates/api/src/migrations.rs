//! The status and type mapping wizard (specification §5, L04-US08): what a
//! draft removes that tickets use, mapped to what it keeps, then migrated
//! in the background before the draft is published.
//!
//! The plan lists each removed status and type with its tickets, the
//! targets it may map to (the new ones marked) and a suggested one.
//! Starting a migration checks that every removed item is mapped
//! (`mapping_incomplete` otherwise) and queues the job, which publishes the
//! draft once every ticket moved; its progress is read meanwhile. Tickets
//! are counted over the projects of the configuration, archived ones
//! included ([`TicketCounts`]); a ticket in both a removed status and a
//! removed type counts once in the total.

use std::collections::{BTreeMap, BTreeSet};

use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use domain::migration::{
    MappingError, Required, State, StatusOption, TypeOption, percent, suggest_status, suggest_type,
    tickets_to_move, validate_mapping,
};
use domain::validation::FieldError;
use domain::workflows::Category;
use infra::config_migrations::{self as repo, Mapping, Migration, NewMigration};
use infra::configs as configs_repo;
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::configs::{TAG, require_config_admin};
use crate::etag::require_if_match;
use crate::extract::{AcceptLanguage, ApiJson, ApiPath};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::tenant::TenantTx;

type Labels = BTreeMap<String, String>;

/// A status or type the tickets of a removed one may move to.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = MigrationTarget)]
pub struct TargetView {
    id: Uuid,
    #[schema(example = "In acceptance")]
    label: Option<String>,
    /// Added by the draft.
    is_new: bool,
}

/// A removed status or type to map.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = MigrationItem)]
pub struct ItemView {
    id: Uuid,
    key: String,
    #[schema(example = "In test")]
    label: Option<String>,
    /// The workflow of a status; `null` for a type.
    #[schema(required = true)]
    workflow: Option<String>,
    /// The tickets in it, over the projects of the configuration.
    ticket_count: u64,
    targets: Vec<TargetView>,
    /// Same identifier, else same category (or sub-task or not).
    #[schema(required = true)]
    suggested_id: Option<Uuid>,
}

/// What publishing the draft removes that must be mapped.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = MigrationPlan)]
pub struct PlanView {
    statuses: Vec<ItemView>,
    types: Vec<ItemView>,
    /// Every ticket to move, each once (in a removed status and type alike).
    ticket_total: u64,
    /// The configuration's version: send it as `If-Match` to start.
    version: i64,
}

/// A removed status or type and where its tickets go.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = MigrationMapping)]
pub struct MappingBody {
    from_id: Uuid,
    to_id: Uuid,
}

/// The mapping of every removed status and type.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = NewMigration)]
pub struct StartBody {
    #[serde(default)]
    status_mappings: Vec<MappingBody>,
    #[serde(default)]
    type_mappings: Vec<MappingBody>,
}

/// Where a migration stands.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = MigrationState)]
pub enum StateView {
    /// Queued.
    Pending,
    /// Moving tickets.
    Running,
    /// Every ticket moved, the draft published.
    Done,
    /// Stopped for good (`error`); nothing published.
    Failed,
}

impl From<State> for StateView {
    fn from(state: State) -> Self {
        match state {
            State::Pending => Self::Pending,
            State::Running => Self::Running,
            State::Done => Self::Done,
            State::Failed => Self::Failed,
        }
    }
}

/// A migration and its progress.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = Migration)]
pub struct MigrationView {
    id: Uuid,
    state: StateView,
    ticket_total: i64,
    tickets_migrated: i64,
    #[schema(example = 50)]
    percent: u8,
    #[schema(required = true)]
    error: Option<String>,
    created_at: DateTime<Utc>,
    #[schema(required = true)]
    finished_at: Option<DateTime<Utc>>,
}

impl From<Migration> for MigrationView {
    fn from(m: Migration) -> Self {
        Self {
            percent: percent(
                u64::try_from(m.tickets_migrated).unwrap_or(0),
                u64::try_from(m.ticket_total).unwrap_or(0),
            ),
            id: m.id,
            state: m.state.into(),
            ticket_total: m.ticket_total,
            tickets_migrated: m.tickets_migrated,
            error: m.error,
            created_at: m.created_at,
            finished_at: m.finished_at,
        }
    }
}

/// A removed item with its targets, as the plan and the check read it.
struct Removed {
    id: Uuid,
    key: String,
    labels: Labels,
    workflow: Option<Labels>,
    tickets: u64,
    targets: Vec<(Uuid, Labels, bool)>,
    suggested: Option<Uuid>,
}

impl Removed {
    fn required(&self) -> Required {
        Required {
            item: self.id.as_u128(),
            targets: self.targets.iter().map(|(id, _, _)| id.as_u128()).collect(),
        }
    }
}

/// The tickets of the projects of a configuration, by status and type.
pub(crate) struct TicketCounts(Vec<(u128, u128, u64)>);

impl TicketCounts {
    pub(crate) async fn of(tx: &mut TenantTx, config: Uuid) -> Result<Self, Problem> {
        let rows = repo::ticket_counts(tx, config).await?;
        Ok(Self(
            rows.into_iter()
                .map(|(status, issue_type, count)| {
                    (
                        status.as_u128(),
                        issue_type.as_u128(),
                        u64::try_from(count).unwrap_or(0),
                    )
                })
                .collect(),
        ))
    }

    /// The tickets in the status `id`.
    pub(crate) fn in_status(&self, id: Uuid) -> u64 {
        self.0
            .iter()
            .filter(|(s, _, _)| *s == id.as_u128())
            .map(|(_, _, n)| n)
            .sum()
    }

    /// The tickets of the type `id`.
    pub(crate) fn of_type(&self, id: Uuid) -> u64 {
        self.0
            .iter()
            .filter(|(_, t, _)| *t == id.as_u128())
            .map(|(_, _, n)| n)
            .sum()
    }

    /// The tickets in one of `statuses` or of one of `types`, each once.
    pub(crate) fn to_move(&self, statuses: &[Uuid], types: &[Uuid]) -> u64 {
        let ids = |items: &[Uuid]| items.iter().map(Uuid::as_u128).collect::<BTreeSet<_>>();
        tickets_to_move(&self.0, &ids(statuses), &ids(types))
    }
}

/// What the draft removes from the published version, with targets.
struct Plan {
    statuses: Vec<Removed>,
    types: Vec<Removed>,
    /// Every ticket to move, each once.
    ticket_total: u64,
}

async fn plan(tx: &mut TenantTx, config: Uuid, published: Uuid, draft: Uuid) -> Result<Plan, Problem> {
    let kept = infra::workflows::load(tx, draft).await?;
    let before = infra::workflows::load(tx, published).await?;
    let is_new_status = |id: Uuid| !before.statuses.iter().any(|s| s.item_id == id);
    let counts = TicketCounts::of(tx, config).await?;
    let statuses: Vec<Removed> = infra::workflows::removed_statuses(tx, published, draft)
        .await?
        .into_iter()
        .map(|(status, workflow)| {
            // The statuses of its workflow if the draft keeps it, else all.
            let same: Vec<_> = kept.statuses_of(status.workflow_id).collect();
            let candidates = if same.is_empty() {
                kept.statuses.iter().collect()
            } else {
                same
            };
            let options: Vec<StatusOption<'_>> = candidates
                .iter()
                .map(|s| StatusOption {
                    id: s.item_id.as_u128(),
                    key: &s.key,
                    category: Category::parse(&s.category).unwrap_or(Category::Todo),
                })
                .collect();
            let category = Category::parse(&status.category).unwrap_or(Category::Todo);
            Removed {
                suggested: suggest_status(&status.key, category, &options).map(Uuid::from_u128),
                targets: candidates
                    .iter()
                    .map(|s| (s.item_id, s.label_translations.clone(), is_new_status(s.item_id)))
                    .collect(),
                id: status.item_id,
                key: status.key,
                labels: status.label_translations,
                workflow: Some(workflow),
                tickets: counts.in_status(status.item_id),
            }
        })
        .collect();
    let kept_types = infra::issue_types::list(tx, draft).await?;
    let before_types = infra::issue_types::list(tx, published).await?;
    let options: Vec<TypeOption<'_>> = kept_types
        .iter()
        .map(|t| TypeOption {
            id: t.item_id.as_u128(),
            key: &t.key,
            is_subtask: t.is_subtask,
        })
        .collect();
    let types = infra::issue_types::removed(tx, published, draft)
        .await?
        .into_iter()
        .map(|t| Removed {
            suggested: suggest_type(&t.key, t.is_subtask, &options).map(Uuid::from_u128),
            targets: kept_types
                .iter()
                .map(|k| {
                    let is_new = !before_types.iter().any(|b| b.item_id == k.item_id);
                    (k.item_id, k.label_translations.clone(), is_new)
                })
                .collect(),
            id: t.item_id,
            key: t.key,
            labels: t.label_translations,
            workflow: None,
            tickets: counts.of_type(t.item_id),
        })
        .collect::<Vec<_>>();
    let ids = |items: &[Removed]| items.iter().map(|r| r.id).collect::<Vec<_>>();
    let ticket_total = counts.to_move(&ids(&statuses), &ids(&types));
    Ok(Plan {
        statuses,
        types,
        ticket_total,
    })
}

impl Language {
    fn item(&self, removed: &Removed) -> ItemView {
        ItemView {
            id: removed.id,
            key: removed.key.clone(),
            label: self.label(&removed.labels),
            workflow: removed.workflow.as_ref().and_then(|w| self.label(w)),
            ticket_count: removed.tickets,
            targets: removed
                .targets
                .iter()
                .map(|(id, labels, is_new)| TargetView {
                    id: *id,
                    label: self.label(labels),
                    is_new: *is_new,
                })
                .collect(),
            suggested_id: removed.suggested,
        }
    }
}

/// The errors of a mapping, under `statusMappings` or `typeMappings`, by
/// the removed item they name.
fn mapping_errors(path: &str, errors: Vec<MappingError>) -> Vec<FieldError> {
    errors
        .into_iter()
        .map(|e| FieldError::new(format!("{path}.{}", Uuid::from_u128(e.item())), e.code()))
        .collect()
}

fn pairs(mappings: &[MappingBody]) -> Vec<(u128, u128)> {
    mappings
        .iter()
        .map(|m| (m.from_id.as_u128(), m.to_id.as_u128()))
        .collect()
}

/// The configuration `id` and its draft, or the problem.
async fn with_draft(tx: &mut TenantTx, id: Uuid) -> Result<(configs_repo::Config, Uuid), Problem> {
    let config = configs_repo::find(tx, id)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such configuration"))?;
    let draft = config
        .draft
        .as_ref()
        .map(|d| d.id)
        .ok_or(Problem::new(ErrorCode::ConfigNoDraft))?;
    Ok((config, draft))
}

/// `GET /api/v1/project-configs/{id}/migration-plan`.
#[utoipa::path(
    get,
    path = "/project-configs/{id}/migration-plan",
    tag = TAG,
    operation_id = "getMigrationPlan",
    summary = "List what publishing the draft removes, to map",
    description = "Tenant admins and configuration admins. Each status and type the draft removes, \
                   with its tickets over the projects of the configuration, the targets it may map to (the new \
                   ones marked) and a suggested one: the same identifier, else the same category \
                   (or a sub-task for a sub-task).",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    responses(
        (status = 200, description = "The plan", body = PlanView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`config.no_draft`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn plan_of(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let (config, draft) = with_draft(&mut tx, id).await?;
    let plan = plan(&mut tx, id, config.published.id, draft).await?;
    let language = Language::of(&tx, &accept);
    tx.commit().await?;
    let view = PlanView {
        ticket_total: plan.ticket_total,
        statuses: plan.statuses.iter().map(|r| language.item(r)).collect(),
        types: plan.types.iter().map(|r| language.item(r)).collect(),
        version: config.version,
    };
    let mut response = ApiJson(view).into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `POST /api/v1/project-configs/{id}/migrations`: requires `If-Match`.
#[utoipa::path(
    post,
    path = "/project-configs/{id}/migrations",
    tag = TAG,
    operation_id = "startMigration",
    summary = "Map what the draft removes and migrate its tickets, then publish",
    description = "Tenant admins and configuration admins. Every removed status and type of the \
                   plan mapped to one of its targets (`mapping_incomplete` otherwise, naming each \
                   in `errors`: `statusMappings.{id}` / `typeMappings.{id}` `required`, \
                   `not_a_target` or `unexpected`). The tickets move in the background; the \
                   draft is published once all moved. Send the plan's `version` as `If-Match`.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("If-Match" = String, Header, description = "Version of the configuration the plan was read on"),
    ),
    request_body = StartBody,
    responses(
        (status = 202, description = "Started", body = MigrationView, headers(
            ("Location" = String, description = "URL of its progress"),
        )),
        (status = 400, description = "`mapping_incomplete`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`config.no_draft`, `workflow.invalid`, or `config.migration_running`: one is under way", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The configuration changed since the plan", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn start(
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<StartBody>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let (config, draft) = with_draft(&mut tx, id).await?;
    if if_match.versions().is_some_and(|v| !v.contains(&config.version)) {
        return Err(Problem::new(ErrorCode::PreconditionFailed));
    }
    let problems = crate::workflows::publish_errors(&infra::workflows::load(&mut tx, draft).await?);
    if !problems.is_empty() {
        return Err(Problem::new(ErrorCode::WorkflowInvalid).with_errors(problems));
    }
    let plan = plan(&mut tx, id, config.published.id, draft).await?;
    let required = |items: &[Removed]| items.iter().map(Removed::required).collect::<Vec<_>>();
    let mut errors = Vec::new();
    if let Err(e) = validate_mapping(&required(&plan.statuses), &pairs(&body.status_mappings)) {
        errors.extend(mapping_errors("statusMappings", e));
    }
    if let Err(e) = validate_mapping(&required(&plan.types), &pairs(&body.type_mappings)) {
        errors.extend(mapping_errors("typeMappings", e));
    }
    if !errors.is_empty() {
        return Err(Problem::new(ErrorCode::MappingIncomplete).with_errors(errors));
    }
    let mappings = |bodies: &[MappingBody]| {
        bodies
            .iter()
            .map(|m| Mapping {
                from: m.from_id,
                to: m.to_id,
            })
            .collect::<Vec<_>>()
    };
    let total = plan.ticket_total;
    let migration_id = infra::id::new_id();
    let actor = tx.account_id();
    let started = repo::start(
        &mut tx,
        &NewMigration {
            id: migration_id,
            config_id: id,
            draft_id: draft,
            status_mappings: &mappings(&body.status_mappings),
            type_mappings: &mappings(&body.type_mappings),
            ticket_total: i64::try_from(total).unwrap_or(i64::MAX),
            created_by: actor,
        },
    )
    .await?;
    if !started {
        return Err(Problem::new(ErrorCode::ConfigMigrationRunning));
    }
    let migration = repo::find(&mut tx, id, migration_id)
        .await?
        .ok_or(Problem::new(ErrorCode::NotFound))?;
    tracing::info!(
        target: "audit",
        actor = %tx.account_id(),
        config = %id,
        migration = %migration_id,
        tickets = total,
        "configuration migration started"
    );
    tx.commit().await?;
    let mut response = (StatusCode::ACCEPTED, ApiJson(MigrationView::from(migration))).into_response();
    if let Ok(location) =
        HeaderValue::from_str(&format!("/api/v1/project-configs/{id}/migrations/{migration_id}"))
    {
        response.headers_mut().insert(header::LOCATION, location);
    }
    Ok(response)
}

/// `GET /api/v1/project-configs/{id}/migrations/{migrationId}`.
#[utoipa::path(
    get,
    path = "/project-configs/{id}/migrations/{migrationId}",
    tag = TAG,
    operation_id = "getMigration",
    summary = "Follow a migration",
    description = "Tenant admins and configuration admins. Its state and the tickets moved so far.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("migrationId" = Uuid, Path, description = "Id of the migration"),
    ),
    responses(
        (status = 200, description = "The migration", body = MigrationView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration or migration", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn progress(
    mut tx: TenantTx,
    ApiPath((id, migration_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<ApiJson<MigrationView>, Problem> {
    require_config_admin(&tx)?;
    let migration = repo::find(&mut tx, id, migration_id)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such migration"))?;
    tx.commit().await?;
    Ok(ApiJson(migration.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_errors_name_the_item_under_their_list() {
        let item = Uuid::from_u128(9);
        let errors = mapping_errors(
            "statusMappings",
            vec![MappingError::Missing(9), MappingError::NotATarget(10)],
        );
        assert_eq!(
            errors,
            [
                FieldError::new(format!("statusMappings.{item}"), "required"),
                FieldError::new(format!("statusMappings.{}", Uuid::from_u128(10)), "not_a_target"),
            ]
        );
        let problem = Problem::new(ErrorCode::MappingIncomplete);
        assert_eq!(
            (problem.code.status().as_u16(), problem.code.as_str()),
            (400, "mapping_incomplete")
        );
        let running = Problem::new(ErrorCode::ConfigMigrationRunning);
        assert_eq!(
            (running.code.status().as_u16(), running.code.as_str()),
            (409, "config.migration_running")
        );
    }

    #[test]
    fn progress_is_done_when_nothing_moves() {
        let migration = Migration {
            id: Uuid::from_u128(1),
            config_id: Uuid::from_u128(2),
            draft_id: Uuid::from_u128(3),
            state: State::Running,
            status_mappings: Vec::new(),
            type_mappings: Vec::new(),
            ticket_total: 2172,
            tickets_migrated: 1086,
            error: None,
            created_at: Utc::now(),
            finished_at: None,
        };
        assert_eq!(MigrationView::from(migration.clone()).percent, 50);
        let empty = Migration {
            ticket_total: 0,
            tickets_migrated: 0,
            ..migration
        };
        assert_eq!(MigrationView::from(empty).percent, 100);
    }
}
