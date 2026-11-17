//! Notifications (`notification`, `notification_settings`, L05-US07).
//!
//! Each outbox event queues a [`DISPATCH_JOB`] in its own transaction
//! (`crate::outbox::record`). The worker runs [`dispatch`] in the tenant's
//! transaction: the change becomes its events (`domain::notify`), the rules
//! of the project's published configuration name the recipients
//! (`domain::notifications::resolve`), those who do not see the ticket are
//! left out, the preferences of the others choose the channels; the
//! notifications are written, the emails queued, the event marked
//! processed. Running it twice does nothing more.
//!
//! A member reads the notifications of all their organizations through
//! `app_my_notifications` and the functions next to it, which answer for
//! the signed-in account only. The daily digest gathers them across
//! organizations too ([`due_digests`]).

use std::collections::{BTreeMap, BTreeSet};

use domain::account::TenantRole;
use domain::notifications::{Event, TicketPeople, resolve};
use domain::notify::{Change, Choice, Preferences, events_of, one_per_recipient};
use domain::permissions::Permission;
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgConnection;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::email::{EMAIL_JOB, EmailJob, EmailMessage, NotificationLine};
use crate::pagination::{Keyset, Page, page_from_rows};

/// Job turning an outbox event into notifications.
pub const DISPATCH_JOB: &str = "outbox.dispatch";
/// Months of partitions kept ready ahead of the current one.
pub const MONTHS_AHEAD: i32 = 3;
/// Longest comment excerpt a notification keeps.
const EXCERPT_CHARS: usize = 200;

/// Payload of [`DISPATCH_JOB`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchJob {
    pub event_id: Uuid,
}

/// Creates the partitions of the current month and of the next
/// [`MONTHS_AHEAD`] if missing; the number created.
pub async fn ensure_partitions(conn: &mut PgConnection) -> Result<i32, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT app_ensure_notification_partitions($1) AS "created!""#,
        MONTHS_AHEAD
    )
    .fetch_one(conn)
    .await
}

// --- From an outbox event to notifications ---------------------------------

/// The ids an outbox payload lists under `name`.
fn ids_in(payload: &serde_json::Value, name: &str) -> BTreeSet<u128> {
    payload
        .get(name)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|id| id.as_str().and_then(|s| Uuid::parse_str(s).ok()))
        .map(|id| id.as_u128())
        .collect()
}

/// The new assignee an outbox payload's `changes` set, if any.
fn assigned_in(payload: &serde_json::Value) -> Option<u128> {
    payload
        .get("changes")?
        .as_array()?
        .iter()
        .find(|change| change.get("field").and_then(serde_json::Value::as_str) == Some("assigneeId"))?
        .get("new")?
        .as_str()
        .and_then(|id| Uuid::parse_str(id).ok())
        .map(|id| id.as_u128())
}

/// The change an outbox event of `kind` tells; none when it notifies
/// nobody. As of the event: the worker may dispatch it after later changes.
pub fn change_of(kind: &str, payload: &serde_json::Value) -> Option<Change> {
    let change = match kind {
        "issue.created" => Change {
            created: true,
            assigned_to: payload
                .get("assigneeId")
                .and_then(serde_json::Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())
                .map(|id| id.as_u128()),
            ..Change::default()
        },
        "issue.updated" => Change {
            assigned_to: assigned_in(payload),
            ..Change::default()
        },
        "issue.transitioned" => Change {
            assigned_to: assigned_in(payload),
            status_changed: true,
            ..Change::default()
        },
        "comment.created" => Change {
            commented: true,
            mentioned: ids_in(payload, "mentions"),
            ..Change::default()
        },
        "comment.updated" => Change {
            mentioned: ids_in(payload, "addedMentions"),
            ..Change::default()
        },
        _ => return None,
    };
    Some(change)
}

/// The beginning of a comment, for a notification.
pub fn excerpt(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= EXCERPT_CHARS {
        text
    } else {
        format!(
            "{}…",
            text.chars().take(EXCERPT_CHARS).collect::<String>().trim_end()
        )
    }
}

/// A recipient, as the worker needs them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient {
    pub account_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub locale: String,
    pub preferences: Preferences,
}

/// The preferences stored as `{"mentioned": {"inApp": true, "email": true}}`.
pub fn preferences_of(stored: &serde_json::Value, daily_digest: bool) -> Preferences {
    let chosen = stored
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(event, choice)| {
            let event = Event::parse(event)?;
            let flag = |name: &str| choice.get(name).and_then(serde_json::Value::as_bool);
            let default = Preferences::default().choice(event);
            Some((
                event,
                Choice {
                    in_app: flag("inApp").unwrap_or(default.in_app),
                    email: flag("email").unwrap_or(default.email),
                },
            ))
        });
    Preferences::with(chosen.collect::<Vec<_>>(), daily_digest)
}

/// `preferences` as stored.
pub fn stored_preferences(preferences: &Preferences) -> serde_json::Value {
    preferences
        .choices()
        .into_iter()
        .map(|(event, choice)| {
            (
                event.as_str().to_owned(),
                serde_json::json!({"inApp": choice.in_app, "email": choice.email}),
            )
        })
        .collect::<serde_json::Map<_, _>>()
        .into()
}

/// The active members of the current tenant among `accounts`, with their
/// preferences.
pub async fn recipients(conn: &mut PgConnection, accounts: &[Uuid]) -> Result<Vec<Recipient>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT account_id AS "account_id!", email AS "email!", display_name AS "display_name!",
               locale AS "locale!", preferences AS "preferences!", daily_digest AS "daily_digest!"
        FROM app_notification_recipients($1)
        "#,
        accounts
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Recipient {
            preferences: preferences_of(&r.preferences, r.daily_digest),
            account_id: r.account_id,
            email: r.email,
            display_name: r.display_name,
            locale: r.locale,
        })
        .collect())
}

/// Whether `account` sees the project `project`: a tenant admin, or a role
/// with `view` reaching it.
async fn sees(conn: &mut PgConnection, account: Uuid, project: Uuid) -> Result<bool, sqlx::Error> {
    let Some(member) = crate::accounts::find(conn, account).await? else {
        return Ok(false);
    };
    if member.tenant_role == TenantRole::TenantAdmin {
        return Ok(true);
    }
    let roles = crate::grants::roles_reaching(conn, account, project).await?;
    Ok(crate::roles::permissions_of(conn, &roles)
        .await?
        .contains(Permission::View))
}

/// The accounts holding `role` on `project` (granted there or above),
/// directly or through a group.
async fn role_holders(
    conn: &mut PgConnection,
    project: Uuid,
    role: Uuid,
) -> Result<BTreeSet<u128>, sqlx::Error> {
    let mut holders = BTreeSet::new();
    for access in crate::grants::access(conn, project).await? {
        if access.role_id != role {
            continue;
        }
        match access.holder {
            crate::grants::Holder::Member { account_id, .. } => {
                holders.insert(account_id.as_u128());
            }
            crate::grants::Holder::Group { id, .. } => {
                holders.extend(group_members(conn, id).await?);
            }
        }
    }
    Ok(holders)
}

async fn group_members(conn: &mut PgConnection, group: Uuid) -> Result<BTreeSet<u128>, sqlx::Error> {
    Ok(crate::groups::members(conn, group)
        .await?
        .into_iter()
        .map(|m| m.account_id.as_u128())
        .collect())
}

/// An outbox event to dispatch.
struct Pending {
    kind: String,
    aggregate_id: Uuid,
    payload: serde_json::Value,
    actor_id: Option<Uuid>,
}

/// What came of dispatching an event.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dispatched {
    /// Notifications written.
    pub notified: usize,
    /// Emails queued.
    pub emailed: usize,
    /// The members who see a new notification in the application.
    pub in_app: Vec<Uuid>,
    /// The real-time signals queued (`crate::events`).
    pub signals: usize,
}

/// Job appending real-time signals to the tenant's event log, once the
/// dispatch committed (`crate::events`).
pub const PUBLISH_JOB: &str = "events.publish";

/// Payload of [`PUBLISH_JOB`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishJob {
    pub signals: Vec<crate::events::Signal>,
}

/// The tickets an outbox event concerns: its aggregate, and the other end
/// of a link.
fn issues_of(kind: &str, aggregate: Uuid, payload: &serde_json::Value) -> Vec<Uuid> {
    let mut issues = vec![aggregate];
    if kind == "issue.linked" || kind == "issue.unlinked" {
        for side in ["sourceId", "targetId"] {
            if let Some(id) = payload
                .get(side)
                .and_then(serde_json::Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())
                && !issues.contains(&id)
            {
                issues.push(id);
            }
        }
    }
    issues
}

/// Builds the address of a ticket's page: from the tenant's slug and the
/// ticket's key, if the public address is known.
pub type LinkOf<'a> = &'a (dyn Fn(&str, &str) -> Option<String> + Sync);

/// Dispatches the outbox event `event_id` of the current tenant: writes
/// its notifications and queues their emails, then marks it processed.
/// Nothing when it was already.
pub async fn dispatch(
    conn: &mut PgConnection,
    event_id: Uuid,
    link_of: LinkOf<'_>,
) -> Result<Dispatched, sqlx::Error> {
    let pending = sqlx::query_as!(
        Pending,
        r#"
        SELECT kind, aggregate_id, payload, actor_id FROM outbox_event
        WHERE id = $1 AND processed_at IS NULL
        FOR UPDATE
        "#,
        event_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(pending) = pending else {
        return Ok(Dispatched::default());
    };
    let mut dispatched = notify(conn, event_id, &pending, link_of).await?;
    // Real time: the tickets changed, and who has new notifications.
    let mut signals = Vec::new();
    if pending.kind.starts_with("issue.")
        || pending.kind.starts_with("comment.")
        || pending.kind.starts_with("attachment.")
    {
        for id in issues_of(&pending.kind, pending.aggregate_id, &pending.payload) {
            if let Some(issue) = crate::issues::find(conn, id).await? {
                signals.push(crate::events::Signal {
                    topic: crate::events::Topic::Issue,
                    kind: Some(pending.kind.clone()),
                    issue_id: Some(issue.id),
                    issue_key: Some(issue.key),
                    project_id: Some(issue.project_id),
                    actor_id: pending.actor_id,
                    recipients: Vec::new(),
                });
            }
        }
    }
    if !dispatched.in_app.is_empty() {
        signals.push(crate::events::Signal {
            topic: crate::events::Topic::Notifications,
            kind: None,
            issue_id: None,
            issue_key: None,
            project_id: None,
            actor_id: None,
            recipients: dispatched.in_app.clone(),
        });
    }
    if !signals.is_empty() {
        dispatched.signals = signals.len();
        let payload =
            serde_json::to_value(PublishJob { signals }).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
        crate::jobs::enqueue(conn, PUBLISH_JOB, &payload).await?;
    }
    sqlx::query!(
        "UPDATE outbox_event SET processed_at = now() WHERE id = $1",
        event_id
    )
    .execute(&mut *conn)
    .await?;
    Ok(dispatched)
}

async fn notify(
    conn: &mut PgConnection,
    event_id: Uuid,
    pending: &Pending,
    link_of: LinkOf<'_>,
) -> Result<Dispatched, sqlx::Error> {
    if pending.aggregate_type_is_not_issue() {
        return Ok(Dispatched::default());
    }
    let Some(issue) = crate::issues::find(conn, pending.aggregate_id).await? else {
        return Ok(Dispatched::default());
    };
    let Some(change) = change_of(&pending.kind, &pending.payload) else {
        return Ok(Dispatched::default());
    };
    let events = events_of(&change);
    if events.is_empty() {
        return Ok(Dispatched::default());
    }
    let Some(config) = crate::configs::of_project(conn, issue.project_id).await? else {
        return Ok(Dispatched::default());
    };
    let Some(version) = crate::configs::find(conn, config).await?.map(|c| c.published.id) else {
        return Ok(Dispatched::default());
    };
    let rules = crate::notification_rules::list(conn, version).await?;

    // Who each event notifies, by the configuration's rules.
    let people = TicketPeople {
        reporter: issue.reporter_id.map(|id| id.as_u128()),
        assignee: issue.assignee_id.map(|id| id.as_u128()),
        watchers: crate::watchers::of_issue(conn, issue.id)
            .await?
            .into_iter()
            .map(|id| id.as_u128())
            .collect(),
        mentioned: change.mentioned.clone(),
    };
    let mut holders: BTreeMap<u128, BTreeSet<u128>> = BTreeMap::new();
    let mut members: BTreeMap<u128, BTreeSet<u128>> = BTreeMap::new();
    for rule in rules.iter().filter(|r| events.contains(&r.event)) {
        for role in &rule.rule.roles {
            if !holders.contains_key(role) {
                let found = role_holders(conn, issue.project_id, Uuid::from_u128(*role)).await?;
                holders.insert(*role, found);
            }
        }
        for group in &rule.rule.groups {
            if !members.contains_key(group) {
                let found = group_members(conn, Uuid::from_u128(*group)).await?;
                members.insert(*group, found);
            }
        }
    }
    let author = pending.actor_id.map_or(0, |id| id.as_u128());
    let by_event: Vec<(Event, BTreeSet<u128>)> = events
        .iter()
        .filter_map(|event| {
            let rule = &rules.iter().find(|r| r.event == *event)?.rule;
            let mut people = people.clone();
            if *event != Event::Mentioned && *event != Event::Commented {
                people.mentioned.clear();
            }
            // As of the event: the assignee it set; at creation, nobody
            // watched yet but its reporter and assignee.
            if matches!(event, Event::Created | Event::Assigned) {
                people.assignee = change.assigned_to;
            }
            if *event == Event::Created {
                people.watchers.clear();
            }
            let recipients = resolve(
                rule,
                &people,
                author,
                &|role| holders.get(&role).cloned().unwrap_or_default(),
                &|group| members.get(&group).cloned().unwrap_or_default(),
            );
            Some((*event, recipients))
        })
        .collect();
    let chosen = one_per_recipient(&by_event);
    if chosen.is_empty() {
        return Ok(Dispatched::default());
    }

    // Those who see the ticket, as their preferences say.
    let accounts: Vec<Uuid> = chosen.keys().map(|id| Uuid::from_u128(*id)).collect();
    let mut recipients = Vec::new();
    for recipient in self::recipients(conn, &accounts).await? {
        if sees(conn, recipient.account_id, issue.project_id).await? {
            recipients.push(recipient);
        }
    }
    let tenant = sqlx::query!("SELECT slug, name FROM tenant WHERE id = app_current_tenant()")
        .fetch_one(&mut *conn)
        .await?;
    let actor_name = match pending.actor_id {
        Some(actor) => crate::audit::names(conn, &[actor]).await?.remove(&actor),
        None => None,
    }
    .unwrap_or_default();
    let status = if change.status_changed {
        crate::workflows::load(conn, version)
            .await?
            .statuses
            .into_iter()
            .find(|s| s.item_id == issue.status_id)
            .map(|s| s.label_translations)
            .unwrap_or_default()
    } else {
        BTreeMap::new()
    };
    let excerpt = pending
        .payload
        .get("text")
        .and_then(serde_json::Value::as_str)
        .map(excerpt)
        .filter(|text| !text.is_empty());
    let link = link_of(&tenant.slug, &issue.key);

    let mut dispatched = Dispatched::default();
    for recipient in recipients {
        let Some(event) = chosen.get(&recipient.account_id.as_u128()).copied() else {
            continue;
        };
        let preferences = &recipient.preferences;
        if !preferences.records(event) {
            continue;
        }
        let details = serde_json::json!({
            "key": issue.key,
            "summary": issue.summary,
            "status": status,
            "excerpt": excerpt,
        });
        let emails_now = preferences.emails_now(event);
        sqlx::query!(
            r#"
            INSERT INTO notification (id, tenant_id, account_id, event, issue_id, actor_id, details, outbox_event_id,
                                      in_app, email_wanted, emailed_at)
            VALUES ($1, app_current_tenant(), $2, $3, $4, $5, $6, $7, $8, $9,
                    CASE WHEN $10 THEN now() END)
            "#,
            crate::id::new_id(),
            recipient.account_id,
            event.as_str(),
            issue.id,
            pending.actor_id,
            details,
            event_id,
            preferences.wants(event, domain::notify::Channel::InApp),
            preferences.wants(event, domain::notify::Channel::Email),
            emails_now
        )
        .execute(&mut *conn)
        .await?;
        dispatched.notified += 1;
        if preferences.wants(event, domain::notify::Channel::InApp) {
            dispatched.in_app.push(recipient.account_id);
        }
        if emails_now {
            let job = EmailJob {
                to: recipient.email.clone(),
                locale: recipient.locale.clone(),
                message: EmailMessage::Notification {
                    tenant_name: tenant.name.clone(),
                    line: NotificationLine {
                        event: event.as_str().to_owned(),
                        actor: actor_name.clone(),
                        key: issue.key.clone(),
                        summary: issue.summary.clone(),
                        status: status.clone(),
                        excerpt: excerpt.clone(),
                    },
                    link: link.clone(),
                },
            };
            let payload = serde_json::to_value(&job).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
            crate::jobs::enqueue(conn, EMAIL_JOB, &payload).await?;
            dispatched.emailed += 1;
        }
    }
    Ok(dispatched)
}

impl Pending {
    fn aggregate_type_is_not_issue(&self) -> bool {
        !(self.kind.starts_with("issue.") || self.kind.starts_with("comment."))
    }
}

// --- What a member reads -----------------------------------------------------

/// A notification as its recipient reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub tenant_slug: String,
    pub tenant_name: String,
    pub event: Event,
    pub issue_id: Uuid,
    pub actor_id: Option<Uuid>,
    pub actor_name: Option<String>,
    pub details: serde_json::Value,
    pub read_at: Option<OffsetDateTime>,
    pub created_at: OffsetDateTime,
}

/// The signed-in account's notifications, of every organization or of
/// `tenant`, newest first, `limit` after `before`.
pub async fn mine(
    conn: &mut PgConnection,
    tenant: Option<Uuid>,
    unread_only: bool,
    before: Option<Keyset>,
    limit: usize,
) -> Result<Page<Notification>, sqlx::Error> {
    let fetch = i32::try_from(limit.saturating_add(1)).unwrap_or(i32::MAX);
    let rows = sqlx::query!(
        r#"
        SELECT id AS "id!", tenant_id AS "tenant_id!", tenant_slug AS "tenant_slug!", tenant_name AS "tenant_name!",
               event AS "event!", issue_id AS "issue_id!", actor_id, actor_name, details AS "details!", read_at,
               created_at AS "created_at!"
        FROM app_my_notifications($1, $2, $3, $4, $5)
        "#,
        tenant,
        unread_only,
        before.map(|k| k.created_at),
        before.map(|k| k.id),
        fetch
    )
    .fetch_all(conn)
    .await?;
    let items: Vec<Notification> = rows
        .into_iter()
        .filter_map(|r| {
            Some(Notification {
                event: Event::parse(&r.event)?,
                id: r.id,
                tenant_id: r.tenant_id,
                tenant_slug: r.tenant_slug,
                tenant_name: r.tenant_name,
                issue_id: r.issue_id,
                actor_id: r.actor_id,
                actor_name: r.actor_name,
                details: r.details,
                read_at: r.read_at,
                created_at: r.created_at,
            })
        })
        .collect();
    Ok(page_from_rows(items, limit, |n| Keyset {
        created_at: n.created_at,
        id: n.id,
    }))
}

/// The signed-in account's unread notifications, per organization.
pub async fn unread(conn: &mut PgConnection) -> Result<BTreeMap<Uuid, i64>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"SELECT tenant_id AS "tenant_id!", unread AS "unread!" FROM app_my_unread_notifications()"#
    )
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|r| (r.tenant_id, r.unread)).collect())
}

/// Marks read the signed-in account's notifications `ids` (none: all, of
/// `tenant` if given); how many.
pub async fn mark_read(
    conn: &mut PgConnection,
    ids: Option<&[Uuid]>,
    tenant: Option<Uuid>,
) -> Result<i32, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT app_mark_my_notifications_read($1, $2) AS "marked!""#,
        ids,
        tenant
    )
    .fetch_one(conn)
    .await
}

/// The signed-in account's preferences.
pub async fn my_preferences(conn: &mut PgConnection, account: Uuid) -> Result<Preferences, sqlx::Error> {
    let row = sqlx::query!(
        "SELECT preferences, daily_digest FROM notification_settings WHERE account_id = $1",
        account
    )
    .fetch_optional(conn)
    .await?;
    Ok(row.map_or_else(Preferences::default, |r| {
        preferences_of(&r.preferences, r.daily_digest)
    }))
}

/// Replaces the signed-in account's preferences.
pub async fn set_my_preferences(
    conn: &mut PgConnection,
    account: Uuid,
    preferences: &Preferences,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO notification_settings (account_id, preferences, daily_digest)
        VALUES ($1, $2, $3)
        ON CONFLICT (account_id) DO UPDATE SET preferences = EXCLUDED.preferences, daily_digest = EXCLUDED.daily_digest
        "#,
        account,
        stored_preferences(preferences),
        preferences.daily_digest
    )
    .execute(conn)
    .await?;
    Ok(())
}

// --- The daily digest ----------------------------------------------------------

/// A member whose digest may be due.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestCandidate {
    pub account_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub locale: String,
    pub local_date: Date,
    /// 1 Monday … 7 Sunday.
    pub local_weekday: u8,
    pub local_hour: u8,
    pub sent_today: bool,
}

/// The members with a digest asked for and notifications waiting for it;
/// `domain::notify::digest_due` tells whose is due.
pub async fn digest_candidates(conn: &mut PgConnection) -> Result<Vec<DigestCandidate>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT account_id AS "account_id!", email AS "email!", display_name AS "display_name!", locale AS "locale!",
               local_date AS "local_date!", local_weekday AS "local_weekday!", local_hour AS "local_hour!",
               sent_today AS "sent_today!"
        FROM app_digest_candidates()
        "#
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| DigestCandidate {
            account_id: r.account_id,
            email: r.email,
            display_name: r.display_name,
            locale: r.locale,
            local_date: r.local_date,
            local_weekday: u8::try_from(r.local_weekday).unwrap_or(0),
            local_hour: u8::try_from(r.local_hour).unwrap_or(0),
            sent_today: r.sent_today,
        })
        .collect())
}

/// A notification waiting for a digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiting {
    pub id: Uuid,
    pub tenant_slug: String,
    pub tenant_name: String,
    pub event: String,
    pub actor_name: Option<String>,
    pub details: serde_json::Value,
}

/// The notifications the digest of `account` gathers, oldest first.
pub async fn waiting_for_digest(conn: &mut PgConnection, account: Uuid) -> Result<Vec<Waiting>, sqlx::Error> {
    sqlx::query_as!(
        Waiting,
        r#"
        SELECT id AS "id!", tenant_slug AS "tenant_slug!", tenant_name AS "tenant_name!", event AS "event!",
               actor_name, details AS "details!"
        FROM app_digest_notifications($1)
        "#,
        account
    )
    .fetch_all(conn)
    .await
}

/// The digest of `account` for its `local_date` went with `ids`.
pub async fn digest_sent(
    conn: &mut PgConnection,
    account: Uuid,
    local_date: Date,
    ids: &[Uuid],
) -> Result<(), sqlx::Error> {
    sqlx::query!("SELECT app_digest_sent($1, $2, $3)", account, local_date, ids)
        .execute(conn)
        .await?;
    Ok(())
}

/// What a waiting notification tells, for the digest.
pub fn line_of(waiting: &Waiting) -> NotificationLine {
    let text = |name: &str| {
        waiting
            .details
            .get(name)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    NotificationLine {
        event: waiting.event.clone(),
        actor: waiting.actor_name.clone().unwrap_or_default(),
        key: text("key"),
        summary: text("summary"),
        status: serde_json::from_value(waiting.details.get("status").cloned().unwrap_or_default())
            .unwrap_or_default(),
        excerpt: waiting
            .details
            .get("excerpt")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const LUCAS: &str = "018f0000-0000-7000-8000-000000000001";

    #[test]
    fn outbox_events_become_changes() {
        let lucas = Uuid::parse_str(LUCAS).unwrap();
        assert_eq!(
            change_of("issue.created", &json!({"assigneeId": LUCAS})),
            Some(Change {
                created: true,
                assigned_to: Some(lucas.as_u128()),
                ..Change::default()
            })
        );
        let changes = json!({"changes": [{"field": "summary", "old": "a", "new": "b"}, {"field": "assigneeId", "old": null, "new": LUCAS}]});
        assert_eq!(
            change_of("issue.updated", &changes).unwrap().assigned_to,
            Some(lucas.as_u128())
        );
        let cleared = json!({"changes": [{"field": "assigneeId", "old": LUCAS, "new": null}]});
        assert_eq!(change_of("issue.updated", &cleared).unwrap().assigned_to, None);
        assert!(
            change_of("issue.transitioned", &json!({"changes": []}))
                .unwrap()
                .status_changed
        );
        let comment = change_of("comment.created", &json!({"mentions": [LUCAS]})).unwrap();
        assert!(comment.commented && comment.mentioned.contains(&lucas.as_u128()));
        let edited = change_of(
            "comment.updated",
            &json!({"mentions": [LUCAS], "addedMentions": []}),
        )
        .unwrap();
        assert!(edited.mentioned.is_empty(), "only those newly mentioned");
        assert_eq!(change_of("issue.linked", &json!({})), None);
    }

    #[test]
    fn preferences_round_trip_and_unknown_events_are_ignored() {
        let mine = Preferences::with(
            [(
                Event::Commented,
                Choice {
                    in_app: false,
                    email: true,
                },
            )],
            true,
        );
        let stored = stored_preferences(&mine);
        assert_eq!(stored["commented"], json!({"inApp": false, "email": true}));
        assert_eq!(preferences_of(&stored, true), mine);
        let odd = preferences_of(
            &json!({"teleported": {"inApp": false}, "mentioned": {"email": false}}),
            false,
        );
        assert!(!odd.wants(Event::Mentioned, domain::notify::Channel::Email));
        assert!(
            odd.wants(Event::Mentioned, domain::notify::Channel::InApp),
            "the default where not said"
        );
    }

    #[test]
    fn excerpts_are_short_and_on_one_line() {
        assert_eq!(excerpt("  Ready\n\nto ship "), "Ready to ship");
        let long = "word ".repeat(100);
        let cut = excerpt(&long);
        assert!(cut.ends_with('…') && cut.chars().count() <= EXCERPT_CHARS + 1);
    }
}
