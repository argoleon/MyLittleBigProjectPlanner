//! Bulk actions on a search result (L06-US08): the action and its report,
//! and what changing one of its tickets does. Row-level security scopes
//! every query to the current tenant; each member reads their own actions.
//!
//! [`run_item`] changes one ticket in the transaction it is given, then
//! records what became of it in that same transaction, so that partial
//! failures never undo the tickets changed. Who may change the ticket is
//! checked for each one, from the database: the member's sight, their
//! permissions on its project, the project's archiving.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use domain::bulk::{Kind, Leaving, Outcome, Pick, follows_parent, pick_transition, reason};
use domain::issues::Priority;
use domain::permissions::{Permission, PermissionSet};
use domain::validation::FieldError;
use serde::{Deserialize, Deserializer, Serialize};
use sqlx::postgres::PgConnection;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::audit::Author;
use crate::tickets::Content;

/// Job changing one ticket of a background bulk action.
pub const ITEM_JOB: &str = "bulk.item";
/// Job deleting a bulk action once expired.
pub const PURGE_JOB: &str = "bulk.purge";
/// The outbox event of a background bulk action done.
pub const DONE: &str = "bulk.done";

/// `Some(None)` for a JSON `null`, `Some(Some(v))` for a value, `None`
/// when absent (with `#[serde(default)]`).
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

/// What Modify sets on every ticket: what is given replaces, `null`
/// clears; custom fields by id (those of one configuration).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModifyParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub assignee_id: Option<Option<Uuid>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub start_date: Option<Option<Date>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub due_date: Option<Option<Date>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<Uuid, serde_json::Value>,
}

/// A type or status mapped for Move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mapping {
    pub from_id: Uuid,
    pub to_id: Uuid,
}

/// What a bulk action does, as stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    Modify(ModifyParams),
    #[serde(rename_all = "camelCase")]
    Transition {
        status_key: String,
    },
    #[serde(rename_all = "camelCase")]
    Move {
        project_key: String,
        #[serde(default)]
        type_mappings: Vec<Mapping>,
        #[serde(default)]
        status_mappings: Vec<Mapping>,
    },
}

impl Action {
    pub const fn kind(&self) -> Kind {
        match self {
            Self::Modify(_) => Kind::Modify,
            Self::Transition { .. } => Kind::Transition,
            Self::Move { .. } => Kind::Move,
        }
    }
}

/// A bulk action.
#[derive(Debug, Clone, PartialEq)]
pub struct BulkAction {
    pub id: Uuid,
    pub account_id: Uuid,
    pub action: Action,
    pub language: String,
    pub background: bool,
    pub total: i32,
    pub remaining: i32,
    pub completed_at: Option<OffsetDateTime>,
    pub expires_at: OffsetDateTime,
    pub created_at: OffsetDateTime,
}

/// A ticket of a bulk action, and what became of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: Uuid,
    pub position: i32,
    pub issue_id: Uuid,
    pub issue_key: String,
    /// `None` while waiting.
    pub outcome: Option<Outcome>,
    pub reason: Option<String>,
    /// The field errors, when refused for its values: `(field, code)`.
    pub errors: Vec<(String, String)>,
    pub new_key: Option<String>,
}

/// A ticket selected: its id and key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selected {
    pub id: Uuid,
    pub key: String,
}

struct Row {
    id: Uuid,
    account_id: Uuid,
    params: serde_json::Value,
    language: String,
    background: bool,
    total: i32,
    remaining: i32,
    completed_at: Option<OffsetDateTime>,
    expires_at: OffsetDateTime,
    created_at: OffsetDateTime,
}

fn action_of(row: Row) -> Option<BulkAction> {
    Some(BulkAction {
        action: serde_json::from_value(row.params).ok()?,
        id: row.id,
        account_id: row.account_id,
        language: row.language,
        background: row.background,
        total: row.total,
        remaining: row.remaining,
        completed_at: row.completed_at,
        expires_at: row.expires_at,
        created_at: row.created_at,
    })
}

/// A bulk action to open.
#[derive(Debug, Clone)]
pub struct NewBulk<'a> {
    pub id: Uuid,
    pub account: Uuid,
    pub action: &'a Action,
    /// The language of its report.
    pub language: &'a str,
    /// Run by the worker, its requester told when done.
    pub background: bool,
    pub selected: &'a [Selected],
    pub expires_at: OffsetDateTime,
}

/// Opens the bulk action `new`, each ticket waiting.
pub async fn insert(
    conn: &mut PgConnection,
    new: &NewBulk<'_>,
) -> Result<(BulkAction, Vec<Item>), sqlx::Error> {
    let NewBulk {
        id,
        account,
        action,
        language,
        background,
        selected,
        expires_at,
    } = *new;
    let params = serde_json::to_value(action).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    let total = i32::try_from(selected.len()).unwrap_or(i32::MAX);
    let row = sqlx::query_as!(
        Row,
        r#"
        INSERT INTO bulk_action (id, tenant_id, account_id, kind, params, language, background, total, remaining,
                                 expires_at)
        VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6, $7, $7, $8)
        RETURNING id, account_id, params, language, background, total, remaining, completed_at, expires_at,
                  created_at
        "#,
        id,
        account,
        action.kind().as_str(),
        params,
        language,
        background,
        total,
        expires_at
    )
    .fetch_one(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = selected.iter().map(|_| crate::id::new_id()).collect();
    let issues: Vec<Uuid> = selected.iter().map(|s| s.id).collect();
    let keys: Vec<String> = selected.iter().map(|s| s.key.clone()).collect();
    sqlx::query!(
        r#"
        INSERT INTO bulk_action_item (id, tenant_id, bulk_id, position, issue_id, issue_key)
        SELECT s.id, app_current_tenant(), $1, (s.n - 1)::int, s.issue_id, s.issue_key
        FROM unnest($2::uuid[], $3::uuid[], $4::text[]) WITH ORDINALITY AS s(id, issue_id, issue_key, n)
        "#,
        id,
        &ids,
        &issues,
        &keys
    )
    .execute(&mut *conn)
    .await?;
    let action = action_of(row).ok_or_else(|| sqlx::Error::Protocol("unreadable bulk action".to_owned()))?;
    let items = items_of(conn, id).await?;
    Ok((action, items))
}

/// The bulk action `id`, whoever asked for it (the worker's view).
pub async fn find(conn: &mut PgConnection, id: Uuid) -> Result<Option<BulkAction>, sqlx::Error> {
    let row = sqlx::query_as!(
        Row,
        r#"
        SELECT id, account_id, params, language, background, total, remaining, completed_at, expires_at, created_at
        FROM bulk_action WHERE id = $1
        "#,
        id
    )
    .fetch_optional(conn)
    .await?;
    Ok(row.and_then(action_of))
}

/// The bulk action `id` of `account`, unless expired.
pub async fn find_of(
    conn: &mut PgConnection,
    account: Uuid,
    id: Uuid,
) -> Result<Option<BulkAction>, sqlx::Error> {
    Ok(find(conn, id)
        .await?
        .filter(|b| b.account_id == account && b.expires_at > OffsetDateTime::now_utc()))
}

/// The tickets of the bulk action `bulk`, in their order.
pub async fn items_of(conn: &mut PgConnection, bulk: Uuid) -> Result<Vec<Item>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT id, position, issue_id, issue_key, outcome, reason, errors, new_key
        FROM bulk_action_item WHERE bulk_id = $1 ORDER BY position
        "#,
        bulk
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Item {
            id: r.id,
            position: r.position,
            issue_id: r.issue_id,
            issue_key: r.issue_key,
            outcome: r.outcome.as_deref().and_then(Outcome::parse),
            reason: r.reason,
            errors: r
                .errors
                .and_then(|e| serde_json::from_value::<Vec<(String, String)>>(e).ok())
                .unwrap_or_default(),
            new_key: r.new_key,
        })
        .collect())
}

/// Payload of [`ITEM_JOB`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemJob {
    pub bulk_id: Uuid,
    pub item_id: Uuid,
}

/// Payload of [`PURGE_JOB`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PurgeJob {
    pub bulk_id: Uuid,
}

/// Changes the ticket of the item of `job`, in the job's own transaction,
/// and records what became of it. Nothing when done already, or gone.
pub async fn run_job(conn: &mut PgConnection, job: &ItemJob) -> Result<Option<Outcome>, sqlx::Error> {
    let Some(bulk) = find(conn, job.bulk_id).await? else {
        return Ok(None);
    };
    let item = sqlx::query!(
        "SELECT issue_id, outcome FROM bulk_action_item WHERE id = $1 AND bulk_id = $2",
        job.item_id,
        job.bulk_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(item) = item.filter(|i| i.outcome.is_none()) else {
        return Ok(None);
    };
    let mut ctx = Context::of(conn, &bulk).await?;
    let done = change(conn, &mut ctx, &bulk, item.issue_id).await?;
    record(conn, &bulk, job.item_id, &done).await?;
    Ok(Some(done.outcome))
}

/// What became of a ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    pub outcome: Outcome,
    pub reason: Option<&'static str>,
    pub errors: Vec<FieldError>,
    pub new_key: Option<String>,
}

impl Done {
    const fn plain(outcome: Outcome) -> Self {
        Self {
            outcome,
            reason: None,
            errors: Vec::new(),
            new_key: None,
        }
    }

    const fn refused(reason: &'static str) -> Self {
        Self {
            outcome: Outcome::Refused,
            reason: Some(reason),
            errors: Vec::new(),
            new_key: None,
        }
    }

    fn invalid(reason: &'static str, errors: Vec<FieldError>) -> Self {
        Self {
            errors,
            ..Self::refused(reason)
        }
    }
}

/// Records `done` for the item `item` of `bulk`, unless recorded already;
/// the tickets still waiting then. When none is left, the action is
/// complete, and a background one tells its requester (`bulk.done`).
pub async fn record(
    conn: &mut PgConnection,
    bulk: &BulkAction,
    item: Uuid,
    done: &Done,
) -> Result<Option<i32>, sqlx::Error> {
    let errors: Option<serde_json::Value> = (!done.errors.is_empty()).then(|| {
        serde_json::Value::Array(
            done.errors
                .iter()
                .map(|e| serde_json::json!([e.field, e.code]))
                .collect(),
        )
    });
    let recorded = sqlx::query!(
        r#"
        UPDATE bulk_action_item SET outcome = $2, reason = $3, errors = $4, new_key = $5, done_at = now()
        WHERE id = $1 AND outcome IS NULL
        "#,
        item,
        done.outcome.as_str(),
        done.reason,
        errors,
        done.new_key
    )
    .execute(&mut *conn)
    .await?;
    if recorded.rows_affected() == 0 {
        return Ok(None);
    }
    let remaining = sqlx::query_scalar!(
        r#"
        UPDATE bulk_action SET remaining = remaining - 1,
               completed_at = CASE WHEN remaining = 1 THEN now() END, version = version + 1
        WHERE id = $1
        RETURNING remaining
        "#,
        bulk.id
    )
    .fetch_one(&mut *conn)
    .await?;
    if remaining == 0 && bulk.background {
        let counts = sqlx::query!(
            r#"
            SELECT count(*) FILTER (WHERE outcome IN ('changed', 'with_parent')) AS "changed!",
                   count(*) FILTER (WHERE outcome = 'unchanged') AS "unchanged!",
                   count(*) FILTER (WHERE outcome = 'refused') AS "refused!"
            FROM bulk_action_item WHERE bulk_id = $1
            "#,
            bulk.id
        )
        .fetch_one(&mut *conn)
        .await?;
        crate::outbox::record(
            conn,
            &crate::outbox::Event {
                kind: DONE,
                aggregate_type: "bulk_action",
                aggregate_id: bulk.id,
                payload: serde_json::json!({
                    "accountId": bulk.account_id,
                    "kind": bulk.action.kind().as_str(),
                    "total": bulk.total,
                    "changed": counts.changed,
                    "unchanged": counts.unchanged,
                    "refused": counts.refused,
                }),
                actor_id: Some(bulk.account_id),
            },
        )
        .await?;
    }
    Ok(Some(remaining))
}

/// Deletes the bulk action `id` and its report.
pub async fn delete(conn: &mut PgConnection, id: Uuid) -> Result<bool, sqlx::Error> {
    let deleted = sqlx::query!("DELETE FROM bulk_action WHERE id = $1", id)
        .execute(conn)
        .await?;
    Ok(deleted.rows_affected() == 1)
}

// --- Changing one ticket ---------------------------------------------------

/// What every ticket of a run shares: who acts, what they see, what they
/// may do on each project, the configurations read.
#[derive(Debug)]
pub struct Context {
    pub account: Uuid,
    pub admin: bool,
    /// The nodes the member sees (`None`: all, a tenant admin).
    pub sight: Option<BTreeSet<Uuid>>,
    /// The tickets selected, for sub-tasks moving with their parent.
    pub selected: BTreeSet<u128>,
    permissions: HashMap<Uuid, PermissionSet>,
    contents: HashMap<Uuid, std::sync::Arc<Content>>,
}

impl Context {
    /// The context of `bulk`'s member running it.
    pub async fn of(conn: &mut PgConnection, bulk: &BulkAction) -> Result<Self, sqlx::Error> {
        let account = bulk.account_id;
        let admin = crate::accounts::find(conn, account)
            .await?
            .is_some_and(|m| m.tenant_role == crate::accounts::TenantRole::TenantAdmin);
        let sight = if admin {
            None
        } else {
            Some(
                crate::grants::sight(conn, account)
                    .await?
                    .visible
                    .into_iter()
                    .collect(),
            )
        };
        // Only a move needs the others (sub-tasks follow their parent).
        let selected = if bulk.action.kind() == Kind::Move {
            sqlx::query_scalar!(
                "SELECT issue_id FROM bulk_action_item WHERE bulk_id = $1",
                bulk.id
            )
            .fetch_all(conn)
            .await?
            .into_iter()
            .map(|id| id.as_u128())
            .collect()
        } else {
            BTreeSet::new()
        };
        Ok(Self {
            account,
            admin,
            sight,
            selected,
            permissions: HashMap::new(),
            contents: HashMap::new(),
        })
    }

    fn sees(&self, node: Uuid) -> bool {
        self.sight.as_ref().is_none_or(|s| s.contains(&node))
    }

    async fn may(
        &mut self,
        conn: &mut PgConnection,
        node: Uuid,
        permission: Permission,
    ) -> Result<bool, sqlx::Error> {
        if self.admin {
            return Ok(true);
        }
        if let Some(set) = self.permissions.get(&node) {
            return Ok(set.contains(permission));
        }
        let roles = crate::grants::roles_reaching(conn, self.account, node).await?;
        let set = crate::roles::permissions_of(conn, &roles).await?;
        self.permissions.insert(node, set);
        Ok(set.contains(permission))
    }

    async fn content(
        &mut self,
        conn: &mut PgConnection,
        project: Uuid,
    ) -> Result<Option<std::sync::Arc<Content>>, sqlx::Error> {
        if let Some(content) = self.contents.get(&project) {
            return Ok(Some(content.clone()));
        }
        match Content::of(conn, project).await {
            Ok(content) => {
                let content = std::sync::Arc::new(content);
                self.contents.insert(project, content.clone());
                Ok(Some(content))
            }
            Err(crate::tickets::ContentError::NotFound) => Ok(None),
            Err(crate::tickets::ContentError::Database(e)) => Err(e),
        }
    }
}

/// Changes the ticket of `item` as `bulk` says, as the member of `ctx`, in
/// the transaction `conn`; what became of it. Records nothing: see
/// [`run_item`].
pub async fn change(
    conn: &mut PgConnection,
    ctx: &mut Context,
    bulk: &BulkAction,
    issue_id: Uuid,
) -> Result<Done, sqlx::Error> {
    let Some(issue) = crate::issues::find(conn, issue_id).await? else {
        return Ok(Done::refused(reason::NOT_FOUND));
    };
    let Some(project) = crate::nodes::find(conn, issue.project_id).await? else {
        return Ok(Done::refused(reason::NOT_FOUND));
    };
    if !ctx.sees(project.id) {
        return Ok(Done::refused(reason::NOT_FOUND));
    }
    if !ctx.may(conn, project.id, Permission::Edit).await? {
        return Ok(Done::refused(reason::PERMISSION_DENIED));
    }
    if project.archived {
        return Ok(Done::refused(reason::NODE_ARCHIVED));
    }
    // Its own version: locked until this ticket's transaction ends.
    if crate::issues::lock(conn, issue.id).await?.is_none() {
        return Ok(Done::refused(reason::NOT_FOUND));
    }
    let Some(issue) = crate::issues::find(conn, issue.id).await? else {
        return Ok(Done::refused(reason::NOT_FOUND));
    };
    let author = Author {
        actor_id: Some(ctx.account),
        origin: domain::audit::Origin::Interface,
    };
    let Some(content) = ctx.content(conn, project.id).await? else {
        return Ok(Done::refused(reason::NOT_FOUND));
    };
    match &bulk.action {
        Action::Modify(params) => {
            let edit = crate::tickets::edit::Edit {
                summary: None,
                description: None,
                priority: params.priority.as_deref().and_then(Priority::parse),
                assignee: params.assignee_id,
                start_date: params.start_date,
                due_date: params.due_date,
                fields: params.fields.clone(),
            };
            Ok(
                match crate::tickets::edit::apply(
                    conn,
                    author,
                    ctx.account,
                    project.id,
                    issue,
                    &content,
                    &edit,
                )
                .await?
                {
                    Ok(edited) if edited.changed => Done::plain(Outcome::Changed),
                    Ok(_) => Done::plain(Outcome::Unchanged),
                    Err(errors) => Done::invalid(reason::VALIDATION_FAILED, errors),
                },
            )
        }
        Action::Transition { status_key } => {
            let key_of = |id: Uuid| {
                content
                    .workflows
                    .statuses
                    .iter()
                    .find(|s| s.item_id == id)
                    .map(|s| s.key.clone())
                    .unwrap_or_default()
            };
            let actor = crate::tickets::transition::actor_of(conn, ctx.account, project.id).await?;
            let leaving = crate::tickets::transition::leaving(&content, &issue);
            let candidates: Vec<Leaving> = leaving
                .iter()
                .map(|t| Leaving {
                    to_status: key_of(t.to_status_id),
                    allowed: ctx.admin || domain::transitions::allowed(&t.rules.condition, &actor),
                })
                .collect();
            match pick_transition(&key_of(issue.status_id), status_key, &candidates) {
                Pick::AlreadyThere => Ok(Done::plain(Outcome::Unchanged)),
                Pick::NoTransition => Ok(Done::refused(reason::NO_TRANSITION)),
                Pick::Restricted => Ok(Done::refused(reason::RESTRICTED)),
                Pick::Run(i) => {
                    let run = crate::tickets::transition::Run {
                        project: project.id,
                        content: &content,
                        transition: leaving[i],
                        actor: &actor,
                        admin: ctx.admin,
                    };
                    Ok(
                        match crate::tickets::transition::perform(
                            conn,
                            author,
                            ctx.account,
                            &run,
                            issue,
                            &BTreeMap::new(),
                        )
                        .await?
                        {
                            Ok(_) => Done::plain(Outcome::Changed),
                            Err(crate::tickets::transition::Refused::Restricted) => {
                                Done::refused(reason::RESTRICTED)
                            }
                            Err(crate::tickets::transition::Refused::Invalid(errors)) => {
                                Done::invalid(reason::VALIDATION_FAILED, errors)
                            }
                        },
                    )
                }
            }
        }
        Action::Move {
            project_key,
            type_mappings,
            status_mappings,
        } => {
            if follows_parent(issue.parent_id.map(|p| p.as_u128()), &ctx.selected) {
                return Ok(Done::plain(Outcome::WithParent));
            }
            let target = match crate::nodes::project_of_key(conn, project_key).await? {
                Some(id) => crate::nodes::find(conn, id).await?,
                None => None,
            };
            let Some(target) = target.filter(|t| ctx.sees(t.id)) else {
                return Ok(Done::refused(reason::NOT_FOUND));
            };
            if target.id == project.id {
                return Ok(Done::plain(Outcome::Unchanged));
            }
            if !ctx.may(conn, target.id, Permission::CreateTicket).await? {
                return Ok(Done::refused(reason::PERMISSION_DENIED));
            }
            if target.archived {
                return Ok(Done::refused(reason::NODE_ARCHIVED));
            }
            let prepared = match crate::tickets::moves::prepare(conn, issue, project.id, target).await {
                Ok(prepared) => prepared,
                Err(crate::tickets::moves::PrepareError::Subtask) => {
                    return Ok(Done::refused(reason::SUBTASK_ALONE));
                }
                Err(crate::tickets::moves::PrepareError::Content(crate::tickets::ContentError::NotFound)) => {
                    return Ok(Done::refused(reason::NOT_FOUND));
                }
                Err(crate::tickets::moves::PrepareError::Content(
                    crate::tickets::ContentError::Database(e),
                )) => {
                    return Err(e);
                }
            };
            let map = |mappings: &[Mapping]| -> BTreeMap<u128, u128> {
                mappings
                    .iter()
                    .map(|m| (m.from_id.as_u128(), m.to_id.as_u128()))
                    .collect()
            };
            let landings = match prepared.landings(&map(type_mappings), &map(status_mappings)) {
                Ok(landings) => landings,
                Err(errors) => return Ok(Done::invalid(reason::NOT_MAPPED, errors)),
            };
            let moved =
                crate::tickets::moves::execute(conn, author, ctx.account, project.id, &prepared, &landings)
                    .await?;
            Ok(Done {
                new_key: moved.first().map(|m| m.issue.key.clone()),
                ..Done::plain(Outcome::Changed)
            })
        }
    }
}

/// Changes the ticket of `item` and records what became of it, in the
/// transaction `conn` (the ticket's own): the tickets still waiting.
pub async fn run_item(
    conn: &mut PgConnection,
    ctx: &mut Context,
    bulk: &BulkAction,
    item: &Item,
) -> Result<Option<i32>, sqlx::Error> {
    let done = change(conn, ctx, bulk, item.issue_id).await?;
    record(conn, bulk, item.id, &done).await
}
