//! SQL of the workflows of a configuration version, with their statuses
//! and transitions (L04-US04). Row-level security scopes every query to
//! the current tenant.
//!
//! Workflows, statuses and transitions are addressed by their `item_id`,
//! which they keep from one version to the next.

use std::collections::{BTreeMap, HashMap};

use domain::transitions::Rules;
use domain::workflows::{StatusNode, TransitionEdge};
use sqlx::postgres::PgConnection;
use sqlx::types::Json;
use uuid::Uuid;

use crate::transition_rules::RulesDoc;

type Labels = BTreeMap<String, String>;

/// A workflow as stored in a version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workflow {
    pub item_id: Uuid,
    pub key: String,
    pub label_translations: Labels,
    pub position: i32,
    pub version: i64,
}

/// A status of a workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub item_id: Uuid,
    pub workflow_id: Uuid,
    pub key: String,
    pub label_translations: Labels,
    pub portal_label_translations: Labels,
    pub category: String,
    pub is_initial: bool,
    pub position: i32,
    pub version: i64,
}

impl Status {
    pub fn node(&self) -> StatusNode {
        StatusNode {
            id: self.item_id.as_u128(),
            is_initial: self.is_initial,
        }
    }
}

/// A transition of a workflow; `from_status_id` none: from any status.
#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    pub item_id: Uuid,
    pub workflow_id: Uuid,
    pub name_translations: Labels,
    pub from_status_id: Option<Uuid>,
    pub to_status_id: Uuid,
    /// Who may take it, the fields it requires, what it does (L04-US05).
    pub rules: Rules,
    pub position: i32,
    pub version: i64,
}

impl Transition {
    pub fn edge(&self) -> TransitionEdge {
        TransitionEdge {
            id: self.item_id.as_u128(),
            from: self.from_status_id.as_ref().map(Uuid::as_u128),
            to: self.to_status_id.as_u128(),
        }
    }
}

/// Everything of the workflows of a version.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Workflows {
    pub workflows: Vec<Workflow>,
    pub statuses: Vec<Status>,
    pub transitions: Vec<Transition>,
}

impl Workflows {
    pub fn statuses_of(&self, workflow: Uuid) -> impl Iterator<Item = &Status> {
        self.statuses.iter().filter(move |s| s.workflow_id == workflow)
    }

    pub fn transitions_of(&self, workflow: Uuid) -> impl Iterator<Item = &Transition> {
        self.transitions.iter().filter(move |t| t.workflow_id == workflow)
    }
}

struct WorkflowRow {
    item_id: Uuid,
    key: String,
    label: Json<Labels>,
    position: i32,
    version: i64,
}

struct StatusRow {
    item_id: Uuid,
    workflow_id: Uuid,
    key: String,
    label: Json<Labels>,
    portal_label: Json<Labels>,
    category: String,
    is_initial: bool,
    position: i32,
    version: i64,
}

struct TransitionRow {
    item_id: Uuid,
    workflow_id: Uuid,
    name: Json<Labels>,
    from_status_id: Option<Uuid>,
    to_status_id: Uuid,
    rules: Json<RulesDoc>,
    position: i32,
    version: i64,
}

/// What a status is made of; its key is set once, when it is added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusFields<'a> {
    pub label_translations: &'a Labels,
    pub portal_label_translations: &'a Labels,
    pub category: &'a str,
    pub is_initial: bool,
}

/// What a transition is made of; its rules stay as they are when none.
#[derive(Debug, Clone, PartialEq)]
pub struct TransitionFields<'a> {
    pub name_translations: &'a Labels,
    pub from_status_id: Option<Uuid>,
    pub to_status_id: Uuid,
    pub rules: Option<&'a Rules>,
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    NotFound,
    VersionMismatch,
    /// Another workflow of the version, or status of the workflow, has
    /// this key.
    KeyTaken,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

/// Labels trimmed, empty ones dropped.
fn trimmed(labels: &Labels) -> Labels {
    labels
        .iter()
        .map(|(locale, label)| (locale.clone(), label.trim().to_owned()))
        .filter(|(_, label)| !label.is_empty())
        .collect()
}

fn key_taken<T>(result: Result<T, sqlx::Error>) -> WriteResult<T> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(sqlx::Error::Database(err))
            if matches!(
                err.constraint(),
                Some("workflow_key_unique" | "workflow_status_key_unique")
            ) =>
        {
            Ok(Err(WriteConflict::KeyTaken))
        }
        Err(err) => Err(err),
    }
}

/// The workflows of `version` with their statuses and transitions, each in
/// their order.
pub async fn load(conn: &mut PgConnection, version: Uuid) -> Result<Workflows, sqlx::Error> {
    let workflows = sqlx::query_as!(
        WorkflowRow,
        r#"
        SELECT item_id, key, label AS "label: Json<Labels>", position, version
        FROM workflow WHERE version_id = $1 ORDER BY position, key
        "#,
        version
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| Workflow {
        item_id: r.item_id,
        key: r.key,
        label_translations: r.label.0,
        position: r.position,
        version: r.version,
    })
    .collect();
    let statuses = sqlx::query_as!(
        StatusRow,
        r#"
        SELECT item_id, workflow_id, key, label AS "label: Json<Labels>",
               portal_label AS "portal_label: Json<Labels>", category, is_initial, position, version
        FROM workflow_status WHERE version_id = $1 ORDER BY position, key
        "#,
        version
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| Status {
        item_id: r.item_id,
        workflow_id: r.workflow_id,
        key: r.key,
        label_translations: r.label.0,
        portal_label_translations: r.portal_label.0,
        category: r.category,
        is_initial: r.is_initial,
        position: r.position,
        version: r.version,
    })
    .collect();
    let transitions = sqlx::query_as!(
        TransitionRow,
        r#"
        SELECT item_id, workflow_id, name AS "name: Json<Labels>", from_status_id, to_status_id,
               rules AS "rules: Json<RulesDoc>", position, version
        FROM workflow_transition WHERE version_id = $1 ORDER BY position, item_id
        "#,
        version
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| Transition {
        item_id: r.item_id,
        workflow_id: r.workflow_id,
        name_translations: r.name.0,
        from_status_id: r.from_status_id,
        to_status_id: r.to_status_id,
        rules: Rules::from(&r.rules.0),
        position: r.position,
        version: r.version,
    })
    .collect();
    Ok(Workflows {
        workflows,
        statuses,
        transitions,
    })
}

/// Adds the workflow `item` to `version`, last, with no status.
pub async fn insert_workflow(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
    key: &str,
    label: &Labels,
) -> WriteResult<()> {
    key_taken(
        sqlx::query!(
            r#"
            INSERT INTO workflow (id, tenant_id, version_id, item_id, key, label, position)
            SELECT $1, app_current_tenant(), $2, $3, $4, $5,
                   coalesce((SELECT max(position) + 1 FROM workflow WHERE version_id = $2), 0)
            "#,
            crate::id::new_id(),
            version,
            item,
            key,
            Json(trimmed(label)) as _
        )
        .execute(conn)
        .await
        .map(|_| ()),
    )
}

/// Adds the workflow `item` to `version` as a copy of the statuses and
/// transitions of `source`, with identities of their own.
pub async fn duplicate_workflow(
    conn: &mut PgConnection,
    version: Uuid,
    source: Uuid,
    item: Uuid,
    key: &str,
    label: &Labels,
) -> WriteResult<()> {
    if let Err(conflict) = insert_workflow(conn, version, item, key, label).await? {
        return Ok(Err(conflict));
    }
    let all = load(conn, version).await?;
    let mut ids: HashMap<Uuid, Uuid> = HashMap::new();
    for status in all.statuses_of(source) {
        let copy = crate::id::new_id();
        ids.insert(status.item_id, copy);
        sqlx::query!(
            r#"
            INSERT INTO workflow_status
                (id, tenant_id, version_id, item_id, workflow_id, key, label, portal_label, category, is_initial, position)
            VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
            crate::id::new_id(),
            version,
            copy,
            item,
            status.key,
            Json(&status.label_translations) as _,
            Json(&status.portal_label_translations) as _,
            status.category,
            status.is_initial,
            status.position
        )
        .execute(&mut *conn)
        .await?;
    }
    for transition in all.transitions_of(source) {
        let (Some(to), from) = (
            ids.get(&transition.to_status_id).copied(),
            transition.from_status_id.and_then(|f| ids.get(&f).copied()),
        ) else {
            continue;
        };
        sqlx::query!(
            r#"
            INSERT INTO workflow_transition
                (id, tenant_id, version_id, item_id, workflow_id, name, from_status_id, to_status_id, rules, position)
            VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
            crate::id::new_id(),
            version,
            crate::id::new_id(),
            item,
            Json(&transition.name_translations) as _,
            from,
            to,
            Json(RulesDoc::from(&transition.rules)) as _,
            transition.position
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(Ok(()))
}

/// Renames the workflow `item`, if its version is one of
/// `expected_versions` (`None`: any).
pub async fn update_workflow(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
    expected_versions: Option<&[i64]>,
    label: &Labels,
) -> WriteResult<()> {
    let updated = sqlx::query!(
        r#"
        UPDATE workflow SET label = $4, version = version + 1
        WHERE version_id = $1 AND item_id = $2 AND ($3::bigint[] IS NULL OR version = ANY($3))
        "#,
        version,
        item,
        expected_versions,
        Json(trimmed(label)) as _
    )
    .execute(&mut *conn)
    .await?;
    written(conn, Table::Workflow, version, item, updated.rows_affected()).await
}

/// Removes the workflow `item` with its statuses and transitions, if its
/// version is expected. The caller checks first that no type follows it.
pub async fn delete_workflow(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
    expected_versions: Option<&[i64]>,
) -> WriteResult<()> {
    let deleted = sqlx::query!(
        "DELETE FROM workflow WHERE version_id = $1 AND item_id = $2 AND ($3::bigint[] IS NULL OR version = ANY($3))",
        version,
        item,
        expected_versions
    )
    .execute(&mut *conn)
    .await?;
    written(conn, Table::Workflow, version, item, deleted.rows_affected()).await
}

/// Adds the status `item` to the workflow `workflow`, last. If it is the
/// initial one, the former initial status no longer is.
pub async fn insert_status(
    conn: &mut PgConnection,
    version: Uuid,
    workflow: Uuid,
    item: Uuid,
    key: &str,
    fields: &StatusFields<'_>,
) -> WriteResult<()> {
    if fields.is_initial {
        not_initial(conn, version, workflow, item).await?;
    }
    key_taken(
        sqlx::query!(
            r#"
            INSERT INTO workflow_status
                (id, tenant_id, version_id, item_id, workflow_id, key, label, portal_label, category, is_initial, position)
            SELECT $1, app_current_tenant(), $2, $3, $4, $5, $6, $7, $8, $9,
                   coalesce((SELECT max(position) + 1 FROM workflow_status WHERE version_id = $2 AND workflow_id = $4), 0)
            "#,
            crate::id::new_id(),
            version,
            item,
            workflow,
            key,
            Json(trimmed(fields.label_translations)) as _,
            Json(trimmed(fields.portal_label_translations)) as _,
            fields.category,
            fields.is_initial
        )
        .execute(conn)
        .await
        .map(|_| ()),
    )
}

/// Changes the status `item`, if its version is expected. If it becomes
/// the initial one, the former initial status no longer is.
pub async fn update_status(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
    expected_versions: Option<&[i64]>,
    fields: &StatusFields<'_>,
) -> WriteResult<()> {
    let workflow = sqlx::query_scalar!(
        "SELECT workflow_id FROM workflow_status WHERE version_id = $1 AND item_id = $2",
        version,
        item
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(workflow) = workflow else {
        return Ok(Err(WriteConflict::NotFound));
    };
    let stale = sqlx::query_scalar!(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM workflow_status
            WHERE version_id = $1 AND item_id = $2 AND NOT ($3::bigint[] IS NULL OR version = ANY($3))
        ) AS "stale!"
        "#,
        version,
        item,
        expected_versions
    )
    .fetch_one(&mut *conn)
    .await?;
    if stale {
        return Ok(Err(WriteConflict::VersionMismatch));
    }
    if fields.is_initial {
        not_initial(conn, version, workflow, item).await?;
    }
    let updated = sqlx::query!(
        r#"
        UPDATE workflow_status
        SET label = $3, portal_label = $4, category = $5, is_initial = $6, version = version + 1
        WHERE version_id = $1 AND item_id = $2
        "#,
        version,
        item,
        Json(trimmed(fields.label_translations)) as _,
        Json(trimmed(fields.portal_label_translations)) as _,
        fields.category,
        fields.is_initial
    )
    .execute(&mut *conn)
    .await?;
    written(conn, Table::Status, version, item, updated.rows_affected()).await
}

/// The statuses of `workflow` other than `except` are no longer initial.
async fn not_initial(
    conn: &mut PgConnection,
    version: Uuid,
    workflow: Uuid,
    except: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        UPDATE workflow_status SET is_initial = false, version = version + 1
        WHERE version_id = $1 AND workflow_id = $2 AND item_id <> $3 AND is_initial
        "#,
        version,
        workflow,
        except
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Removes the status `item` with the transitions from and to it, if its
/// version is expected.
pub async fn delete_status(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
    expected_versions: Option<&[i64]>,
) -> WriteResult<()> {
    let deleted = sqlx::query!(
        "DELETE FROM workflow_status WHERE version_id = $1 AND item_id = $2 AND ($3::bigint[] IS NULL OR version = ANY($3))",
        version,
        item,
        expected_versions
    )
    .execute(&mut *conn)
    .await?;
    written(conn, Table::Status, version, item, deleted.rows_affected()).await
}

/// Adds the transition `item` to the workflow `workflow`, last. The caller
/// checks that its statuses belong to the workflow.
pub async fn insert_transition(
    conn: &mut PgConnection,
    version: Uuid,
    workflow: Uuid,
    item: Uuid,
    fields: &TransitionFields<'_>,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO workflow_transition
            (id, tenant_id, version_id, item_id, workflow_id, name, from_status_id, to_status_id, rules, position)
        SELECT $1, app_current_tenant(), $2, $3, $4, $5, $6, $7, $8,
               coalesce((SELECT max(position) + 1 FROM workflow_transition WHERE version_id = $2 AND workflow_id = $4), 0)
        "#,
        crate::id::new_id(),
        version,
        item,
        workflow,
        Json(trimmed(fields.name_translations)) as _,
        fields.from_status_id,
        fields.to_status_id,
        Json(fields.rules.map(RulesDoc::from).unwrap_or_default()) as _
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Changes the transition `item`, if its version is expected.
pub async fn update_transition(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
    expected_versions: Option<&[i64]>,
    fields: &TransitionFields<'_>,
) -> WriteResult<()> {
    let updated = sqlx::query!(
        r#"
        UPDATE workflow_transition
        SET name = $4, from_status_id = $5, to_status_id = $6, rules = coalesce($7, rules), version = version + 1
        WHERE version_id = $1 AND item_id = $2 AND ($3::bigint[] IS NULL OR version = ANY($3))
        "#,
        version,
        item,
        expected_versions,
        Json(trimmed(fields.name_translations)) as _,
        fields.from_status_id,
        fields.to_status_id,
        fields.rules.map(|rules| Json(RulesDoc::from(rules))) as _
    )
    .execute(&mut *conn)
    .await?;
    written(conn, Table::Transition, version, item, updated.rows_affected()).await
}

/// Removes the transition `item`, if its version is expected.
pub async fn delete_transition(
    conn: &mut PgConnection,
    version: Uuid,
    item: Uuid,
    expected_versions: Option<&[i64]>,
) -> WriteResult<()> {
    let deleted = sqlx::query!(
        "DELETE FROM workflow_transition WHERE version_id = $1 AND item_id = $2 AND ($3::bigint[] IS NULL OR version = ANY($3))",
        version,
        item,
        expected_versions
    )
    .execute(&mut *conn)
    .await?;
    written(conn, Table::Transition, version, item, deleted.rows_affected()).await
}

/// Takes the field `field`, removed from `version`, out of the rules of its
/// transitions; the workflows that changed.
pub async fn forget_field(
    conn: &mut PgConnection,
    version: Uuid,
    field: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let all = load(conn, version).await?;
    let mut changed = Vec::new();
    for transition in all
        .transitions
        .iter()
        .filter(|t| t.rules.names_field(field.as_u128()))
    {
        let rules = transition.rules.without_field(field.as_u128());
        sqlx::query!(
            "UPDATE workflow_transition SET rules = $3, version = version + 1 WHERE version_id = $1 AND item_id = $2",
            version,
            transition.item_id,
            Json(RulesDoc::from(&rules)) as _
        )
        .execute(&mut *conn)
        .await?;
        if !changed.contains(&transition.workflow_id) {
            changed.push(transition.workflow_id);
        }
    }
    Ok(changed)
}

/// Copies the workflows of the version `from`, with their statuses and
/// transitions, into `to`, keeping their identities.
pub async fn copy(conn: &mut PgConnection, from: Uuid, to: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO workflow (id, tenant_id, version_id, item_id, key, label, position)
        SELECT uuidv7(), tenant_id, $2, item_id, key, label, position FROM workflow WHERE version_id = $1
        "#,
        from,
        to
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        r#"
        INSERT INTO workflow_status
            (id, tenant_id, version_id, item_id, workflow_id, key, label, portal_label, category, is_initial, position)
        SELECT uuidv7(), tenant_id, $2, item_id, workflow_id, key, label, portal_label, category, is_initial, position
        FROM workflow_status WHERE version_id = $1
        "#,
        from,
        to
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        r#"
        INSERT INTO workflow_transition
            (id, tenant_id, version_id, item_id, workflow_id, name, from_status_id, to_status_id, rules, position)
        SELECT uuidv7(), tenant_id, $2, item_id, workflow_id, name, from_status_id, to_status_id, rules, position
        FROM workflow_transition WHERE version_id = $1
        "#,
        from,
        to
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Gives `version` the standard workflow, followed by its types that have
/// none (`app_seed_default_workflow`).
pub async fn seed_default(conn: &mut PgConnection, version: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!("SELECT app_seed_default_workflow($1)", version)
        .execute(conn)
        .await?;
    Ok(())
}

/// The statuses of `published` that `draft` no longer has, with the label
/// of their workflow in the published version.
pub async fn removed_statuses(
    conn: &mut PgConnection,
    published: Uuid,
    draft: Uuid,
) -> Result<Vec<(Status, Labels)>, sqlx::Error> {
    let before = load(conn, published).await?;
    let after = load(conn, draft).await?;
    let kept: std::collections::HashSet<Uuid> = after.statuses.iter().map(|s| s.item_id).collect();
    let labels: HashMap<Uuid, &Labels> = before
        .workflows
        .iter()
        .map(|w| (w.item_id, &w.label_translations))
        .collect();
    Ok(before
        .statuses
        .iter()
        .filter(|s| !kept.contains(&s.item_id))
        .map(|s| {
            (
                s.clone(),
                labels
                    .get(&s.workflow_id)
                    .map(|l| (*l).clone())
                    .unwrap_or_default(),
            )
        })
        .collect())
}

/// The tables of a workflow.
#[derive(Debug, Clone, Copy)]
enum Table {
    Workflow,
    Status,
    Transition,
}

/// `Ok` if `rows` were written, else whether `item` is gone or stale.
async fn written(
    conn: &mut PgConnection,
    table: Table,
    version: Uuid,
    item: Uuid,
    rows: u64,
) -> WriteResult<()> {
    if rows > 0 {
        return Ok(Ok(()));
    }
    let exists = match table {
        Table::Workflow => sqlx::query_scalar!(
            r#"SELECT EXISTS (SELECT 1 FROM workflow WHERE version_id = $1 AND item_id = $2) AS "exists!""#,
            version,
            item
        )
        .fetch_one(conn)
        .await?,
        Table::Status => sqlx::query_scalar!(
            r#"SELECT EXISTS (SELECT 1 FROM workflow_status WHERE version_id = $1 AND item_id = $2) AS "exists!""#,
            version,
            item
        )
        .fetch_one(conn)
        .await?,
        Table::Transition => sqlx::query_scalar!(
            r#"SELECT EXISTS (SELECT 1 FROM workflow_transition WHERE version_id = $1 AND item_id = $2) AS "exists!""#,
            version,
            item
        )
        .fetch_one(conn)
        .await?,
    };
    Ok(Err(if exists {
        WriteConflict::VersionMismatch
    } else {
        WriteConflict::NotFound
    }))
}
