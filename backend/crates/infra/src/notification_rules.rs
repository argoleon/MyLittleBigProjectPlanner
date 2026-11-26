//! SQL of the notification rules of a configuration version (L04-US07):
//! one per ticket event, always there. Row-level security scopes every
//! query to the current tenant.

use domain::notifications::{Event, RecipientKind, Rule};
use sqlx::postgres::PgConnection;
use uuid::Uuid;

/// The rule of an event in a version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationRule {
    pub event: Event,
    pub rule: Rule,
    pub version: i64,
}

struct Row {
    event: String,
    recipient_kinds: Vec<String>,
    role_ids: Vec<Uuid>,
    group_ids: Vec<Uuid>,
    account_ids: Vec<Uuid>,
    version: i64,
}

impl Row {
    /// The rule; none for an event the domain no longer knows.
    fn rule(self) -> Option<NotificationRule> {
        let ids = |ids: Vec<Uuid>| ids.iter().map(Uuid::as_u128).collect();
        Some(NotificationRule {
            event: Event::parse(&self.event)?,
            rule: Rule {
                kinds: self
                    .recipient_kinds
                    .iter()
                    .filter_map(|k| RecipientKind::parse(k))
                    .collect(),
                roles: ids(self.role_ids),
                groups: ids(self.group_ids),
                accounts: ids(self.account_ids),
            },
            version: self.version,
        })
    }
}

/// Why a write did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteConflict {
    NotFound,
    VersionMismatch,
}

pub type WriteResult<T> = Result<Result<T, WriteConflict>, sqlx::Error>;

/// The rules of `version`, in the order of the events.
pub async fn list(conn: &mut PgConnection, version: Uuid) -> Result<Vec<NotificationRule>, sqlx::Error> {
    let rows = sqlx::query_as!(
        Row,
        r#"
        SELECT event, recipient_kinds, role_ids, group_ids, account_ids, version
        FROM notification_rule WHERE version_id = $1
        ORDER BY array_position(ARRAY['created', 'assigned', 'status_changed', 'commented', 'mentioned', 'absence_conflict'], event)
        "#,
        version
    )
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().filter_map(Row::rule).collect())
}

/// Gives the event `event` of `version` the rule `rule`, if its version is
/// one of `expected_versions` (`None`: any).
pub async fn update(
    conn: &mut PgConnection,
    version: Uuid,
    event: Event,
    expected_versions: Option<&[i64]>,
    rule: &Rule,
) -> WriteResult<()> {
    let uuids = |ids: &[u128]| ids.iter().map(|id| Uuid::from_u128(*id)).collect::<Vec<_>>();
    let kinds: Vec<&str> = rule.kinds.iter().map(|k| k.as_str()).collect();
    let updated = sqlx::query!(
        r#"
        UPDATE notification_rule
        SET recipient_kinds = $4, role_ids = $5, group_ids = $6, account_ids = $7, version = version + 1
        WHERE version_id = $1 AND event = $2 AND ($3::bigint[] IS NULL OR version = ANY($3))
        "#,
        version,
        event.as_str(),
        expected_versions,
        &kinds as &[&str],
        &uuids(&rule.roles),
        &uuids(&rule.groups),
        &uuids(&rule.accounts)
    )
    .execute(&mut *conn)
    .await?;
    if updated.rows_affected() > 0 {
        return Ok(Ok(()));
    }
    let exists = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM notification_rule WHERE version_id = $1 AND event = $2) AS "exists!""#,
        version,
        event.as_str()
    )
    .fetch_one(conn)
    .await?;
    Ok(Err(if exists {
        WriteConflict::VersionMismatch
    } else {
        WriteConflict::NotFound
    }))
}

/// Copies the rules of the version `from` into `to`.
pub async fn copy(conn: &mut PgConnection, from: Uuid, to: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO notification_rule (id, tenant_id, version_id, event, recipient_kinds, role_ids, group_ids, account_ids)
        SELECT uuidv7(), tenant_id, $2, event, recipient_kinds, role_ids, group_ids, account_ids
        FROM notification_rule WHERE version_id = $1
        "#,
        from,
        to
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// Gives `version` the default rules (`app_seed_default_notifications`).
pub async fn seed_defaults(conn: &mut PgConnection, version: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!("SELECT app_seed_default_notifications($1)", version)
        .execute(conn)
        .await?;
    Ok(())
}
