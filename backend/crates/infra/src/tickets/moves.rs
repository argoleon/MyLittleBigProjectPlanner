//! Moving a ticket to another project (L05-US02): its sub-tasks follow,
//! types and statuses the target configuration lacks are mapped, values of
//! fields the new types lack are dropped; each ticket gets a key of its new
//! project and keeps its former one as an alias.

use std::collections::{BTreeMap, BTreeSet};

use domain::issues::{Landing, Moving, TargetType, kept_values, plan_move};
use domain::migration::{StatusOption, TypeOption, suggest_status, suggest_type};
use domain::validation::FieldError;
use domain::workflows::Category;
use sqlx::postgres::PgConnection;
use uuid::Uuid;

use super::content::{Content, ContentError, field_defs, stored_values, type_ids_of};
use crate::audit::Author;
use crate::issues::{self as repo, Issue};
use crate::nodes::Node;

/// A move checked and ready: where to, both configurations, the tickets.
/// A move, checked: the tickets that move and where to.
#[derive(Debug)]
pub struct Prepared {
    pub target: Node,
    pub source_content: std::sync::Arc<Content>,
    pub target_content: std::sync::Arc<Content>,
    pub same_configuration: bool,
    /// The ticket asked for, then its sub-tasks.
    pub moving: Vec<Issue>,
}

impl Prepared {
    pub fn target_types(&self) -> Vec<TargetType> {
        self.target_content
            .types
            .iter()
            .map(|t| TargetType {
                node: t.node(),
                statuses: t
                    .workflow_id
                    .map(|w| {
                        self.target_content
                            .workflows
                            .statuses_of(w)
                            .map(|s| s.item_id.as_u128())
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect()
    }

    /// The type each type the target lacks is suggested to become.
    pub fn suggested_types(&self) -> BTreeMap<u128, u128> {
        let options: Vec<TypeOption<'_>> = self
            .target_content
            .types
            .iter()
            .map(|t| TypeOption {
                id: t.item_id.as_u128(),
                key: &t.key,
                is_subtask: t.is_subtask,
            })
            .collect();
        self.source_types_needed()
            .into_iter()
            .filter_map(|t| Some((t.item_id.as_u128(), suggest_type(&t.key, t.is_subtask, &options)?)))
            .collect()
    }

    /// The types of moving tickets the target configuration lacks.
    pub fn source_types_needed(&self) -> Vec<&crate::issue_types::IssueType> {
        let mut seen = BTreeSet::new();
        self.moving
            .iter()
            .filter(|i| seen.insert(i.type_id))
            .filter(|i| !self.target_content.types.iter().any(|t| t.item_id == i.type_id))
            .filter_map(|i| self.source_content.types.iter().find(|t| t.item_id == i.type_id))
            .collect()
    }

    /// The new type of each moving ticket with `types`, if it has one.
    pub fn landing_type(
        &self,
        issue: &Issue,
        types: &BTreeMap<u128, u128>,
    ) -> Option<&crate::issue_types::IssueType> {
        let id = types
            .get(&issue.type_id.as_u128())
            .copied()
            .unwrap_or(issue.type_id.as_u128());
        self.target_content
            .types
            .iter()
            .find(|t| t.item_id.as_u128() == id)
    }

    /// The statuses of moving tickets not in the workflow of their new type
    /// (with `types`), with the source types of the tickets in them.
    pub fn statuses_needed(
        &self,
        types: &BTreeMap<u128, u128>,
    ) -> Vec<(&crate::workflows::Status, Vec<Uuid>)> {
        let mut needed: Vec<(&crate::workflows::Status, Vec<Uuid>)> = Vec::new();
        for issue in &self.moving {
            let in_workflow = self
                .landing_type(issue, types)
                .and_then(|t| t.workflow_id)
                .is_some_and(|w| {
                    self.target_content
                        .workflows
                        .statuses_of(w)
                        .any(|s| s.item_id == issue.status_id)
                });
            if in_workflow {
                continue;
            }
            let Some(status) = self
                .source_content
                .workflows
                .statuses
                .iter()
                .find(|s| s.item_id == issue.status_id)
            else {
                continue;
            };
            match needed.iter_mut().find(|(s, _)| s.item_id == status.item_id) {
                Some((_, type_ids)) if !type_ids.contains(&issue.type_id) => type_ids.push(issue.type_id),
                Some(_) => {}
                None => needed.push((status, vec![issue.type_id])),
            }
        }
        needed
    }

    /// The status suggested for `status`, among those of the workflows of
    /// the new types of its tickets.
    pub fn suggested_status(
        &self,
        status: &crate::workflows::Status,
        types: &BTreeMap<u128, u128>,
    ) -> Option<u128> {
        let workflows: BTreeSet<Uuid> = self
            .moving
            .iter()
            .filter(|i| i.status_id == status.item_id)
            .filter_map(|i| self.landing_type(i, types)?.workflow_id)
            .collect();
        let options: Vec<StatusOption<'_>> = self
            .target_content
            .workflows
            .statuses
            .iter()
            .filter(|s| workflows.contains(&s.workflow_id))
            .map(|s| StatusOption {
                id: s.item_id.as_u128(),
                key: &s.key,
                category: Category::parse(&s.category).unwrap_or(Category::Todo),
            })
            .collect();
        suggest_status(
            &status.key,
            Category::parse(&status.category).unwrap_or(Category::Todo),
            &options,
        )
    }

    /// Where the moving tickets land with `types` and `statuses`, or what
    /// the move refuses.
    pub fn landings(
        &self,
        types: &BTreeMap<u128, u128>,
        statuses: &BTreeMap<u128, u128>,
    ) -> Result<Vec<Landing>, Vec<FieldError>> {
        let moving: Vec<Moving> = self
            .moving
            .iter()
            .map(|i| Moving {
                id: i.id.as_u128(),
                type_id: i.type_id.as_u128(),
                status_id: i.status_id.as_u128(),
            })
            .collect();
        plan_move(&moving, &self.target_types(), types, statuses)
    }

    /// The values each moving ticket keeps at its landing, and the fields
    /// dropped (any ticket), in the order of the source configuration.
    pub fn kept(&self, landings: &[Landing]) -> (Vec<serde_json::Value>, Vec<Uuid>) {
        let type_ids = type_ids_of(&self.target_content.fields);
        let defs = field_defs(&self.target_content.fields, &type_ids);
        let mut dropped = BTreeSet::new();
        let kept = self
            .moving
            .iter()
            .zip(landings)
            .map(|(issue, landing)| {
                let values = stored_values(&issue.fields);
                let (_, gone) = kept_values(&values, landing.type_id, &defs);
                dropped.extend(gone.iter().copied());
                let mut object = issue.fields.as_object().cloned().unwrap_or_default();
                object.retain(|id, _| Uuid::parse_str(id).is_ok_and(|id| !gone.contains(&id.as_u128())));
                serde_json::Value::Object(object)
            })
            .collect();
        let order = self
            .source_content
            .fields
            .iter()
            .map(|f| f.item_id)
            .filter(|id| dropped.contains(&id.as_u128()));
        (kept, order.collect())
    }
}

/// Why a move cannot even be planned.
#[derive(Debug)]
pub enum PrepareError {
    /// A sub-task moves with its parent only.
    Subtask,
    Content(ContentError),
}

impl From<ContentError> for PrepareError {
    fn from(err: ContentError) -> Self {
        Self::Content(err)
    }
}

impl From<sqlx::Error> for PrepareError {
    fn from(err: sqlx::Error) -> Self {
        Self::Content(ContentError::Database(err))
    }
}

/// Reads what moving `issue` from the project `source` to `target` takes:
/// both configurations, and its sub-tasks, which follow it. The caller
/// checks who moves it and that both projects take changes.
pub async fn prepare(
    conn: &mut PgConnection,
    issue: Issue,
    source: Uuid,
    target: Node,
) -> Result<Prepared, PrepareError> {
    let source_content = Content::of(conn, source).await?;
    if source_content
        .types
        .iter()
        .any(|t| t.item_id == issue.type_id && t.is_subtask)
    {
        return Err(PrepareError::Subtask);
    }
    let target_content = Content::of(conn, target.id).await?;
    let same_configuration = crate::configs::of_project(conn, source).await?
        == crate::configs::of_project(conn, target.id).await?;
    let children = repo::children(conn, issue.id).await?;
    let mut moving = vec![issue];
    moving.extend(children);
    Ok(Prepared {
        target,
        source_content: std::sync::Arc::new(source_content),
        target_content: std::sync::Arc::new(target_content),
        same_configuration,
        moving,
    })
}

/// A ticket moved: its former key, and itself in its new project.
#[derive(Debug, Clone)]
pub struct Moved {
    pub former_key: String,
    pub issue: Issue,
}

/// Moves the tickets of `prepared` from the project `source` where
/// `landings` say, as `account`: keys, types, statuses and kept values,
/// each with its history and `issue.moved` event. The ticket asked for
/// first.
pub async fn execute(
    conn: &mut PgConnection,
    author: Author,
    account: Uuid,
    source: Uuid,
    prepared: &Prepared,
    landings: &[Landing],
) -> Result<Vec<Moved>, sqlx::Error> {
    let (kept, dropped) = prepared.kept(landings);
    let target = &prepared.target;
    let mut moved_all = Vec::with_capacity(prepared.moving.len());
    for (index, ((issue, landing), fields)) in prepared.moving.iter().zip(landings).zip(&kept).enumerate() {
        // The ticket asked for leaves the one it sat under; its sub-tasks stay under it.
        let parent_id = if index == 0 { None } else { issue.parent_id };
        let moved = repo::move_to(
            conn,
            &repo::Move {
                id: issue.id,
                project_id: target.id,
                project_key: &target.key,
                type_id: Uuid::from_u128(landing.type_id),
                status_id: Uuid::from_u128(landing.status_id),
                parent_id,
                fields,
            },
        )
        .await?;
        let mut entries = vec![super::history::moved(&issue.key, &moved.key)];
        entries.extend(super::history::changes(
            &super::history::snapshot(issue, &prepared.source_content),
            &super::history::snapshot(&moved, &prepared.target_content),
        ));
        crate::audit::record(conn, moved.id, author, &entries).await?;
        crate::outbox::record(
            conn,
            &crate::outbox::Event {
                kind: "issue.moved",
                aggregate_type: "issue",
                aggregate_id: moved.id,
                payload: serde_json::json!({
                    "key": moved.key,
                    "formerKey": issue.key,
                    "projectId": target.id,
                    "formerProjectId": source,
                    "typeId": moved.type_id,
                    "statusId": moved.status_id,
                    "droppedFieldIds": dropped,
                }),
                actor_id: Some(account),
            },
        )
        .await?;
        tracing::info!(target: "audit", actor = %account, issue = %moved.id, from = %issue.key, to = %moved.key, "ticket moved");
        moved_all.push(Moved {
            former_key: issue.key.clone(),
            issue: moved,
        });
    }
    Ok(moved_all)
}
