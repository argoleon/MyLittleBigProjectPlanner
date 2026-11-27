//! Notification rules of a project configuration (specification §5 and
//! §12, L04-US07): for each ticket event, whom it notifies on the projects
//! using the configuration.
//!
//! Every configuration has one rule per event (none of its recipients: the
//! event notifies nobody). Rules are read from the draft if the
//! configuration has one; a change goes to the draft. The author of a
//! change is never notified of it; tickets dispatch from L05 on
//! (domain::notifications::resolve).

use std::collections::BTreeSet;

use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use domain::configuration::{Item, ItemKind};
use domain::notifications::{Event, RecipientKind, References, Rule, validate_rule};
use infra::configs as configs_repo;
use infra::notification_rules::{self as repo, NotificationRule, WriteConflict};
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::configs::{TAG, require_config_admin};
use crate::etag::require_if_match;
use crate::extract::{AcceptLanguage, ApiJson, ApiPath};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::tenant::TenantTx;
use crate::transition_rules::{ReferencesView, named_references};

/// A ticket event that notifies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = NotificationEvent)]
pub enum EventView {
    Created,
    Assigned,
    StatusChanged,
    Commented,
    /// Someone is mentioned in a description or a comment.
    Mentioned,
    /// Hours of the ticket fall on an absence of their resource.
    AbsenceConflict,
}

const EVENTS: [(EventView, Event); 6] = [
    (EventView::Created, Event::Created),
    (EventView::Assigned, Event::Assigned),
    (EventView::StatusChanged, Event::StatusChanged),
    (EventView::Commented, Event::Commented),
    (EventView::Mentioned, Event::Mentioned),
    (EventView::AbsenceConflict, Event::AbsenceConflict),
];

impl From<Event> for EventView {
    fn from(event: Event) -> Self {
        EVENTS
            .iter()
            .find(|(_, e)| *e == event)
            .map_or(Self::Created, |(view, _)| *view)
    }
}

impl From<EventView> for Event {
    fn from(view: EventView) -> Self {
        EVENTS
            .iter()
            .find(|(v, _)| *v == view)
            .map_or(Self::Created, |(_, event)| *event)
    }
}

/// People of the ticket a rule notifies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = NotificationRecipientKind)]
pub enum KindView {
    Reporter,
    /// The assignee; the new one for `assigned`.
    Assignee,
    Watchers,
    /// The people mentioned: `commented` and `mentioned` only.
    Mentioned,
}

impl From<KindView> for RecipientKind {
    fn from(view: KindView) -> Self {
        match view {
            KindView::Reporter => Self::Reporter,
            KindView::Assignee => Self::Assignee,
            KindView::Watchers => Self::Watchers,
            KindView::Mentioned => Self::Mentioned,
        }
    }
}

impl From<RecipientKind> for KindView {
    fn from(kind: RecipientKind) -> Self {
        match kind {
            RecipientKind::Reporter => Self::Reporter,
            RecipientKind::Assignee => Self::Assignee,
            RecipientKind::Watchers => Self::Watchers,
            RecipientKind::Mentioned => Self::Mentioned,
        }
    }
}

/// Whom an event notifies; nobody when every list is empty. The author of
/// the change never is.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = NotificationRecipients)]
pub struct RecipientsBody {
    #[serde(default)]
    recipient_kinds: Vec<KindView>,
    /// Roles of the tenant: their holders on the ticket's project, granted
    /// there or above.
    #[serde(default)]
    role_ids: Vec<Uuid>,
    #[serde(default)]
    group_ids: Vec<Uuid>,
    /// Members chosen by name.
    #[serde(default)]
    account_ids: Vec<Uuid>,
}

impl RecipientsBody {
    fn of(rule: &Rule) -> Self {
        let ids = |ids: &[u128]| ids.iter().map(|id| Uuid::from_u128(*id)).collect();
        Self {
            recipient_kinds: rule.kinds.iter().map(|k| KindView::from(*k)).collect(),
            role_ids: ids(&rule.roles),
            group_ids: ids(&rule.groups),
            account_ids: ids(&rule.accounts),
        }
    }

    fn rule(&self) -> Rule {
        let ids = |ids: &[Uuid]| ids.iter().map(Uuid::as_u128).collect();
        Rule {
            kinds: self
                .recipient_kinds
                .iter()
                .map(|k| RecipientKind::from(*k))
                .collect(),
            roles: ids(&self.role_ids),
            groups: ids(&self.group_ids),
            accounts: ids(&self.account_ids),
        }
    }
}

/// The rule of an event.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = NotificationRule)]
pub struct RuleView {
    event: EventView,
    #[serde(flatten)]
    recipients: RecipientsBody,
    #[schema(example = 1)]
    version: i64,
}

/// Where the rules were read.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = NotificationRuleStage)]
pub enum StageView {
    /// The draft, which the projects get once published.
    Draft,
    /// The published version, which the projects use.
    Published,
}

/// The rules of a configuration, one per event, with the names of the
/// roles, groups and members they name.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = NotificationRuleList)]
pub struct RuleList {
    stage: StageView,
    items: Vec<RuleView>,
    references: ReferencesView,
}

fn view(rule: &NotificationRule) -> RuleView {
    RuleView {
        event: rule.event.into(),
        recipients: RecipientsBody::of(&rule.rule),
        version: rule.version,
    }
}

fn conflict_problem(conflict: WriteConflict) -> Problem {
    match conflict {
        WriteConflict::NotFound => Problem::new(ErrorCode::NotFound).with_detail("no such event"),
        WriteConflict::VersionMismatch => Problem::new(ErrorCode::PreconditionFailed),
    }
}

/// The rules of `version`, with the names they refer to.
async fn rule_list(
    tx: &mut TenantTx,
    language: &Language,
    version: Uuid,
    stage: StageView,
) -> Result<RuleList, Problem> {
    let rules = repo::list(tx, version).await?;
    let uuids = |ids: &[u128]| ids.iter().map(|id| Uuid::from_u128(*id)).collect::<Vec<_>>();
    let mut roles = BTreeSet::new();
    let mut groups = BTreeSet::new();
    let mut members = BTreeSet::new();
    for rule in &rules {
        roles.extend(uuids(&rule.rule.roles));
        groups.extend(uuids(&rule.rule.groups));
        members.extend(uuids(&rule.rule.accounts));
    }
    let references = named_references(tx, language, roles, groups, members).await?;
    Ok(RuleList {
        stage,
        items: rules.iter().map(view).collect(),
        references,
    })
}

/// `GET /api/v1/project-configs/{id}/notification-rules`.
#[utoipa::path(
    get,
    path = "/project-configs/{id}/notification-rules",
    tag = TAG,
    operation_id = "listNotificationRules",
    summary = "List the notification rules of a configuration",
    description = "Tenant admins and configuration admins. One per event, in their order, with the \
                   names of the roles, groups and members they name (one deleted since is left \
                   out of `references`); from the draft if the configuration has one.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the role labels"),
    ),
    responses(
        (status = 200, description = "Its rules", body = RuleList),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let config = configs_repo::find(&mut tx, id)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such configuration"))?;
    let (version, stage) = match config.draft {
        Some(draft) => (draft.id, StageView::Draft),
        None => (config.published.id, StageView::Published),
    };
    let language = Language::of(&tx, &accept);
    let list = rule_list(&mut tx, &language, version, stage).await?;
    tx.commit().await?;
    let mut response = ApiJson(list).into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `PUT /api/v1/project-configs/{id}/notification-rules/{event}`: requires
/// `If-Match`.
#[utoipa::path(
    put,
    path = "/project-configs/{id}/notification-rules/{event}",
    tag = TAG,
    operation_id = "updateNotificationRule",
    summary = "Change whom an event notifies, in the draft of a configuration",
    description = "Tenant admins and configuration admins. In the draft, opened if needed. Each \
                   recipient once; the people mentioned for `commented` and `mentioned` only; \
                   roles, groups and active members of the tenant.",
    params(
        ("id" = Uuid, Path, description = "Id of the configuration"),
        ("event" = EventView, Path, description = "The event"),
        ("If-Match" = String, Header, description = "ETag of the rule (its `version`), or `*`"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the role labels"),
    ),
    request_body = RecipientsBody,
    responses(
        (status = 200, description = "The rules", body = RuleList),
        (status = 400, description = "`validation_failed`: `recipientKinds[…]` `duplicate` or `not_applicable`, `roleIds[…]` / `groupIds[…]` `not_found` or `duplicate`, `accountIds[…]` `unknown_member` or `duplicate`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such configuration in this tenant", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "The rule changed since this ETag", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath((id, event)): ApiPath<(Uuid, EventView)>,
    ApiJson(body): ApiJson<RecipientsBody>,
) -> Result<Response, Problem> {
    require_config_admin(&tx)?;
    let if_match = require_if_match(&headers)?;
    let event = Event::from(event);
    let rule = body.rule();
    let uuids = |ids: &[u128]| ids.iter().map(|id| Uuid::from_u128(*id)).collect::<Vec<_>>();
    let found = |items: Vec<Uuid>| {
        items
            .into_iter()
            .map(|id| id.as_u128())
            .collect::<BTreeSet<u128>>()
    };
    let refs = References {
        roles: found(
            infra::rule_references::roles(&mut tx, &uuids(&rule.roles))
                .await?
                .into_iter()
                .map(|(id, _)| id)
                .collect(),
        ),
        groups: found(
            infra::rule_references::groups(&mut tx, &uuids(&rule.groups))
                .await?
                .into_iter()
                .map(|(id, _)| id)
                .collect(),
        ),
        members: found(
            infra::rule_references::members(&mut tx, &uuids(&rule.accounts))
                .await?
                .into_iter()
                .map(|(id, _)| id)
                .collect(),
        ),
    };
    validate_rule(event, &rule, &refs).map_err(Problem::validation)?;
    let actor = tx.account_id();
    let draft = configs_repo::open_draft(&mut tx, id, Some(actor))
        .await?
        .map_err(|_| Problem::new(ErrorCode::NotFound).with_detail("no such configuration"))?
        .draft
        .map(|d| d.id)
        .ok_or_else(|| Problem::new(ErrorCode::ConfigNoDraft))?;
    repo::update(&mut tx, draft, event, if_match.versions(), &rule)
        .await?
        .map_err(conflict_problem)?;
    let item = Item {
        kind: ItemKind::Notification,
        id: event.index(),
    };
    configs_repo::record_change(&mut tx, draft, item).await?;
    let language = Language::of(&tx, &accept);
    let list = rule_list(&mut tx, &language, draft, StageView::Draft).await?;
    tx.commit().await?;
    let mut response = ApiJson(list).into_response();
    language.headers(&mut response);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_and_kinds_map_both_ways() {
        for event in Event::ALL {
            assert_eq!(Event::from(EventView::from(event)), event);
        }
        for kind in RecipientKind::ALL {
            assert_eq!(RecipientKind::from(KindView::from(kind)), kind);
        }
        assert_eq!(
            serde_json::to_value(EventView::StatusChanged).unwrap(),
            Event::StatusChanged.as_str()
        );
    }

    #[test]
    fn recipients_read_back_as_sent() {
        let body: RecipientsBody = serde_json::from_value(serde_json::json!({
            "recipientKinds": ["reporter", "watchers"],
            "roleIds": [Uuid::from_u128(3)],
        }))
        .unwrap();
        let rule = body.rule();
        assert_eq!(rule.kinds, [RecipientKind::Reporter, RecipientKind::Watchers]);
        assert_eq!(rule.roles, [3]);
        assert_eq!(
            serde_json::to_value(RecipientsBody::of(&rule)).unwrap(),
            serde_json::json!({
                "recipientKinds": ["reporter", "watchers"],
                "roleIds": [Uuid::from_u128(3)],
                "groupIds": [],
                "accountIds": []
            })
        );
        assert_eq!(
            conflict_problem(WriteConflict::VersionMismatch).code.as_str(),
            "precondition_failed"
        );
    }
}
