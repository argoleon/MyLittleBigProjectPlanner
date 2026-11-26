//! Notifications of the signed-in member (specification §12, L05-US07):
//! those of all their organizations at once, with a tenant filter, read
//! and marked read; their preferences, which apply in every organization;
//! and the watchers of tickets.
//!
//! The worker writes notifications from the outbox
//! (`infra::notifications::dispatch`). The reads here go through the
//! database functions answering for the signed-in account only
//! (`app_my_notifications`…), across its organizations.

use axum::extract::State;
use domain::notifications::Event;
use domain::notify::{Choice, Notice, Preferences};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};

use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::issues::visible_issue;
use crate::pagination::{PageParams, encode_cursor};
use crate::problem::{PROBLEM_JSON, Problem, ProblemBody};
use crate::tenant::{AppState, TenantTx};

const TAG: &str = "notifications";

/// What notifies a member: an event of a ticket, or something of their own
/// (an export ready, a bulk action done).
/// (The rules of a configuration name the ticket events only:
/// `NotificationEvent`.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = MemberNotificationEvent)]
pub enum EventView {
    Created,
    Assigned,
    StatusChanged,
    Commented,
    Mentioned,
    /// Hours of the ticket fall on an absence of their resource.
    AbsenceConflict,
    ExportReady,
    BulkDone,
}

impl From<Notice> for EventView {
    fn from(notice: Notice) -> Self {
        match notice {
            Notice::Ticket(Event::Created) => Self::Created,
            Notice::Ticket(Event::Assigned) => Self::Assigned,
            Notice::Ticket(Event::StatusChanged) => Self::StatusChanged,
            Notice::Ticket(Event::Commented) => Self::Commented,
            Notice::Ticket(Event::Mentioned) => Self::Mentioned,
            Notice::Ticket(Event::AbsenceConflict) => Self::AbsenceConflict,
            Notice::ExportReady => Self::ExportReady,
            Notice::BulkDone => Self::BulkDone,
        }
    }
}

impl From<EventView> for Notice {
    fn from(view: EventView) -> Self {
        match view {
            EventView::Created => Self::Ticket(Event::Created),
            EventView::Assigned => Self::Ticket(Event::Assigned),
            EventView::StatusChanged => Self::Ticket(Event::StatusChanged),
            EventView::Commented => Self::Ticket(Event::Commented),
            EventView::Mentioned => Self::Ticket(Event::Mentioned),
            EventView::AbsenceConflict => Self::Ticket(Event::AbsenceConflict),
            EventView::ExportReady => Self::ExportReady,
            EventView::BulkDone => Self::BulkDone,
        }
    }
}

/// An organization a notification comes from.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = NotificationTenant)]
pub struct TenantView {
    id: Uuid,
    slug: String,
    name: String,
}

/// A notification of the signed-in member.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = MemberNotification)]
pub struct NotificationView {
    id: Uuid,
    tenant: TenantView,
    event: EventView,
    /// The ticket; `null` for `export_ready`.
    #[schema(required = true)]
    issue_id: Option<Uuid>,
    #[schema(example = "PLAT-151", required = true)]
    issue_key: Option<String>,
    #[schema(required = true)]
    issue_summary: Option<String>,
    /// The export ready (`export_ready`).
    #[schema(required = true)]
    export: Option<ExportRefView>,
    /// The bulk action done (`bulk_done`).
    #[schema(required = true)]
    bulk: Option<BulkRefView>,
    /// Who made the change; "Deleted user #N" once erased.
    #[schema(required = true)]
    actor_name: Option<String>,
    /// The status reached (`status_changed`), in the language of the request.
    #[schema(required = true)]
    status: Option<String>,
    /// The beginning of the comment (`commented`, `mentioned`).
    #[schema(required = true)]
    excerpt: Option<String>,
    /// The hours to reschedule (`absence_conflict`).
    #[schema(required = true)]
    conflict: Option<ConflictRefView>,
    read: bool,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

/// Hours of a ticket an absence leaves no room for.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = NotificationConflict)]
pub struct ConflictRefView {
    #[schema(example = "Nathan Roux")]
    resource_name: String,
    /// Minutes to reschedule, over the days newly in conflict.
    #[schema(example = 420)]
    minutes: u32,
}

/// An export a notification is about.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = NotificationExport)]
pub struct ExportRefView {
    id: Uuid,
    #[schema(example = "search-2026-11-18.csv")]
    file_name: String,
    rows: i64,
}

/// A bulk action a notification is about.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = NotificationBulk)]
pub struct BulkRefView {
    id: Uuid,
    /// `modify`, `transition` or `move`.
    kind: String,
    total: i64,
    changed: i64,
    unchanged: i64,
    refused: i64,
}

/// A page of notifications.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = NotificationPage)]
pub struct NotificationPage {
    items: Vec<NotificationView>,
    #[schema(required = true)]
    next_cursor: Option<String>,
}

/// The unread notifications of each organization.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = UnreadNotifications)]
pub struct UnreadView {
    total: i64,
    by_tenant: Vec<TenantUnread>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = TenantUnread)]
pub struct TenantUnread {
    tenant_id: Uuid,
    unread: i64,
}

/// `?tenantId=&unread=&limit=&cursor=`.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct ListParams {
    /// Only this organization's.
    tenant_id: Option<Uuid>,
    /// Only the unread ones.
    #[serde(default)]
    unread: bool,
    /// Page size, from 1 to 200 (default 50).
    #[param(minimum = 1, maximum = 200)]
    limit: Option<usize>,
    /// `nextCursor` of the previous page.
    cursor: Option<String>,
}

/// Which notifications to mark read.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = MarkReadBody)]
pub struct MarkReadBody {
    /// These; absent: all of them (of `tenantId` if given).
    #[serde(default)]
    ids: Option<Vec<Uuid>>,
    #[serde(default)]
    tenant_id: Option<Uuid>,
}

/// One event's choice.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = NotificationChoice)]
pub struct ChoiceView {
    event: EventView,
    in_app: bool,
    email: bool,
}

/// The signed-in member's preferences, for every organization.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = NotificationPreferences)]
pub struct PreferencesView {
    /// Every event, with its channels.
    events: Vec<ChoiceView>,
    /// One email each working day at 8:00 instead of one per notification.
    daily_digest: bool,
}

fn preferences_view(preferences: &Preferences) -> PreferencesView {
    PreferencesView {
        events: preferences
            .choices()
            .into_iter()
            .map(|(event, choice)| ChoiceView {
                event: event.into(),
                in_app: choice.in_app,
                email: choice.email,
            })
            .collect(),
        daily_digest: preferences.daily_digest,
    }
}

fn view(notification: infra::notifications::Notification, locale: &str) -> NotificationView {
    let details = &notification.details;
    let text = |name: &str| {
        details
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };
    let status = details
        .get("status")
        .and_then(serde_json::Value::as_object)
        .and_then(|labels| labels.get(locale).or_else(|| labels.values().next()))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    NotificationView {
        tenant: TenantView {
            id: notification.tenant_id,
            slug: notification.tenant_slug,
            name: notification.tenant_name,
        },
        event: notification.event.into(),
        issue_id: notification.issue_id,
        issue_key: notification.issue_id.and(text("key")),
        issue_summary: notification.issue_id.and(text("summary")),
        export: (notification.event == Notice::ExportReady)
            .then(|| {
                Some(ExportRefView {
                    id: text("exportId")?.parse().ok()?,
                    file_name: text("fileName").unwrap_or_default(),
                    rows: details
                        .get("rows")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or_default(),
                })
            })
            .flatten(),
        bulk: (notification.event == Notice::BulkDone)
            .then(|| {
                let count = |name: &str| {
                    details
                        .get(name)
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or_default()
                };
                Some(BulkRefView {
                    id: text("bulkId")?.parse().ok()?,
                    kind: text("kind").unwrap_or_default(),
                    total: count("total"),
                    changed: count("changed"),
                    unchanged: count("unchanged"),
                    refused: count("refused"),
                })
            })
            .flatten(),
        excerpt: text("excerpt"),
        conflict: details.get("conflict").and_then(|c| {
            Some(ConflictRefView {
                resource_name: c.get("resourceName")?.as_str()?.to_owned(),
                minutes: u32::try_from(c.get("minutes")?.as_u64()?).ok()?,
            })
        }),
        status,
        actor_name: notification.actor_name,
        read: notification.read_at.is_some(),
        id: notification.id,
        created_at: notification.created_at,
    }
}

/// The language of the labels of the request.
fn locale_of(tx: &TenantTx, accept: &crate::extract::AcceptLanguage) -> &'static str {
    domain::i18n::negotiate(accept.0.as_deref(), tx.default_locale())
}

/// `GET /api/v1/me/notifications`.
#[utoipa::path(
    get,
    path = "/me/notifications",
    tag = TAG,
    operation_id = "listMyNotifications",
    summary = "The signed-in member's notifications",
    description = "Of all their organizations (or `tenantId`), newest first; those they chose to \
                   receive in the application.",
    params(ListParams, ("Accept-Language" = Option<String>, Header, description = "Language of the labels")),
    responses(
        (status = 200, description = "A page of notifications", body = NotificationPage),
        (status = 400, description = "Invalid `limit` or `cursor`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    mut tx: TenantTx,
    accept: crate::extract::AcceptLanguage,
    ApiQuery(params): ApiQuery<ListParams>,
) -> Result<ApiJson<NotificationPage>, Problem> {
    let (limit, before) = PageParams {
        limit: params.limit,
        cursor: params.cursor,
    }
    .resolve()?;
    let page = infra::notifications::mine(&mut tx, params.tenant_id, params.unread, before, limit).await?;
    let locale = locale_of(&tx, &accept);
    tx.commit().await?;
    Ok(ApiJson(NotificationPage {
        items: page.items.into_iter().map(|n| view(n, locale)).collect(),
        next_cursor: page.next.map(encode_cursor),
    }))
}

/// `GET /api/v1/me/notifications/unread`.
#[utoipa::path(
    get,
    path = "/me/notifications/unread",
    tag = TAG,
    operation_id = "countMyUnreadNotifications",
    summary = "How many notifications the signed-in member has not read",
    description = "In total and per organization.",
    responses(
        (status = 200, description = "The counts", body = UnreadView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn unread(mut tx: TenantTx) -> Result<ApiJson<UnreadView>, Problem> {
    let counts = infra::notifications::unread(&mut tx).await?;
    tx.commit().await?;
    Ok(ApiJson(UnreadView {
        total: counts.values().sum(),
        by_tenant: counts
            .into_iter()
            .map(|(tenant_id, unread)| TenantUnread { tenant_id, unread })
            .collect(),
    }))
}

/// `POST /api/v1/me/notifications/read`.
#[utoipa::path(
    post,
    path = "/me/notifications/read",
    tag = TAG,
    operation_id = "markMyNotificationsRead",
    summary = "Mark notifications read",
    description = "The ones given, or all of them (of `tenantId` if given). Only the signed-in \
                   member's: others' ids are ignored.",
    request_body = MarkReadBody,
    responses(
        (status = 200, description = "The unread notifications left", body = UnreadView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn mark_read(
    mut tx: TenantTx,
    ApiJson(body): ApiJson<MarkReadBody>,
) -> Result<ApiJson<UnreadView>, Problem> {
    infra::notifications::mark_read(&mut tx, body.ids.as_deref(), body.tenant_id).await?;
    let counts = infra::notifications::unread(&mut tx).await?;
    tx.commit().await?;
    Ok(ApiJson(UnreadView {
        total: counts.values().sum(),
        by_tenant: counts
            .into_iter()
            .map(|(tenant_id, unread)| TenantUnread { tenant_id, unread })
            .collect(),
    }))
}

/// `GET /api/v1/me/notification-preferences`.
#[utoipa::path(
    get,
    path = "/me/notification-preferences",
    tag = TAG,
    operation_id = "getMyNotificationPreferences",
    summary = "The signed-in member's notification preferences",
    description = "For every organization: per event, in the application and by email (the \
                   defaults until they choose), and the daily digest.",
    responses(
        (status = 200, description = "The preferences", body = PreferencesView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn preferences(mut tx: TenantTx) -> Result<ApiJson<PreferencesView>, Problem> {
    let account = tx.account_id();
    let preferences = infra::notifications::my_preferences(&mut tx, account).await?;
    tx.commit().await?;
    Ok(ApiJson(preferences_view(&preferences)))
}

/// `PUT /api/v1/me/notification-preferences`.
#[utoipa::path(
    put,
    path = "/me/notification-preferences",
    tag = TAG,
    operation_id = "setMyNotificationPreferences",
    summary = "Change the signed-in member's notification preferences",
    description = "For every organization. Events left out keep their current choice.",
    request_body = PreferencesView,
    responses(
        (status = 200, description = "The preferences", body = PreferencesView),
        (status = 400, description = "`validation_failed`: `events[i]` `duplicate`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn set_preferences(
    mut tx: TenantTx,
    ApiJson(body): ApiJson<PreferencesView>,
) -> Result<ApiJson<PreferencesView>, Problem> {
    let mut seen = std::collections::BTreeSet::new();
    let mut errors = Vec::new();
    for (i, choice) in body.events.iter().enumerate() {
        if !seen.insert(choice.event) {
            errors.push(domain::validation::FieldError::new(
                format!("events[{i}]"),
                "duplicate",
            ));
        }
    }
    if !errors.is_empty() {
        return Err(Problem::validation(errors));
    }
    let account = tx.account_id();
    let current = infra::notifications::my_preferences(&mut tx, account).await?;
    let chosen = current.choices().into_iter().chain(body.events.iter().map(|c| {
        (
            Notice::from(c.event),
            Choice {
                in_app: c.in_app,
                email: c.email,
            },
        )
    }));
    let preferences = Preferences::with(chosen.collect::<Vec<_>>(), body.daily_digest);
    infra::notifications::set_my_preferences(&mut tx, account, &preferences).await?;
    tx.commit().await?;
    tracing::info!(account = %account, "notification preferences changed");
    Ok(ApiJson(preferences_view(&preferences)))
}

/// Who watches a ticket.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueWatchers)]
pub struct WatchersView {
    count: i64,
    /// The member reading watches it.
    watching: bool,
}

async fn watchers_view(tx: &mut TenantTx, issue: Uuid) -> Result<WatchersView, Problem> {
    let account = tx.account_id();
    let (count, watching) = infra::watchers::summary(tx, issue, account).await?;
    Ok(WatchersView { count, watching })
}

/// `GET /api/v1/issues/{idOrKey}/watchers`.
#[utoipa::path(
    get,
    path = "/issues/{idOrKey}/watchers",
    tag = TAG,
    operation_id = "getIssueWatchers",
    summary = "Who watches a ticket",
    description = "Whoever sees its project: how many watch it, and whether the member does.",
    params(("idOrKey" = String, Path, description = "Id, key or former key of the ticket")),
    responses(
        (status = 200, description = "The watchers", body = WatchersView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn watchers(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id_or_key): ApiPath<String>,
) -> Result<ApiJson<WatchersView>, Problem> {
    let (issue, _) = visible_issue(&state, &mut tx, &id_or_key).await?;
    let view = watchers_view(&mut tx, issue.id).await?;
    tx.commit().await?;
    Ok(ApiJson(view))
}

/// `PUT /api/v1/issues/{idOrKey}/watch`.
#[utoipa::path(
    put,
    path = "/issues/{idOrKey}/watch",
    tag = TAG,
    operation_id = "watchIssue",
    summary = "Watch a ticket",
    description = "Whoever sees its project: the rules naming \"watchers\" notify them.",
    params(("idOrKey" = String, Path, description = "Id, key or former key of the ticket")),
    responses(
        (status = 200, description = "The watchers", body = WatchersView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn watch(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id_or_key): ApiPath<String>,
) -> Result<ApiJson<WatchersView>, Problem> {
    let (issue, _) = visible_issue(&state, &mut tx, &id_or_key).await?;
    let account = tx.account_id();
    infra::watchers::watch(&mut tx, issue.id, account).await?;
    let view = watchers_view(&mut tx, issue.id).await?;
    tx.commit().await?;
    Ok(ApiJson(view))
}

/// `DELETE /api/v1/issues/{idOrKey}/watch`.
#[utoipa::path(
    delete,
    path = "/issues/{idOrKey}/watch",
    tag = TAG,
    operation_id = "unwatchIssue",
    summary = "Stop watching a ticket",
    params(("idOrKey" = String, Path, description = "Id, key or former key of the ticket")),
    responses(
        (status = 200, description = "The watchers", body = WatchersView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn unwatch(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id_or_key): ApiPath<String>,
) -> Result<ApiJson<WatchersView>, Problem> {
    let (issue, _) = visible_issue(&state, &mut tx, &id_or_key).await?;
    let account = tx.account_id();
    infra::watchers::unwatch(&mut tx, issue.id, account).await?;
    let view = watchers_view(&mut tx, issue.id).await?;
    tx.commit().await?;
    Ok(ApiJson(view))
}
