//! Links between tickets, their parent, and finding a ticket (specification
//! §5, L05-US03).
//!
//! A link joins two tickets of the tenant, across projects; adding or
//! removing one takes `edit` on both tickets' projects. It shows only to
//! those who see both tickets. A ticket's parent is changed within its
//! project, the type hierarchy kept (`domain::links::validate_parent`).
//! Each change writes its event to the outbox: `issue.linked`,
//! `issue.unlinked`, `issue.updated` (parent).

use std::collections::BTreeMap;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use domain::links::{Link, LinkError, NewParent, ParentError, Relation, validate_link, validate_parent};
use domain::validation::FieldError;
use infra::issues::{self as repo, Issue};
use infra::links::{self as link_repo, StoredLink};
use infra::nodes::Node;
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::etag::require_if_match;
use crate::extract::{AcceptLanguage, ApiJson, ApiPath, ApiQuery};
use crate::issue_changes::{locked, present, require_edit, ticket_response};
use crate::issues::{
    Content, IssueView, ProjectView, StatusView, TAG, TypeView, issue_view, project_of_key, project_view,
    visible_issue,
};
use crate::nodes::{ensure_changeable, visible_node};
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::tenant::{AppState, TenantTx};

/// Most tickets a search returns.
const SEARCH_LIMIT: usize = 10;

/// How a link reads from a ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = IssueRelation)]
pub enum RelationView {
    Blocks,
    BlockedBy,
    DependsOn,
    RequiredBy,
    Duplicates,
    DuplicatedBy,
    RelatesTo,
}

impl From<RelationView> for Relation {
    fn from(view: RelationView) -> Self {
        match view {
            RelationView::Blocks => Self::Blocks,
            RelationView::BlockedBy => Self::BlockedBy,
            RelationView::DependsOn => Self::DependsOn,
            RelationView::RequiredBy => Self::RequiredBy,
            RelationView::Duplicates => Self::Duplicates,
            RelationView::DuplicatedBy => Self::DuplicatedBy,
            RelationView::RelatesTo => Self::RelatesTo,
        }
    }
}

impl From<Relation> for RelationView {
    fn from(relation: Relation) -> Self {
        match relation {
            Relation::Blocks => Self::Blocks,
            Relation::BlockedBy => Self::BlockedBy,
            Relation::DependsOn => Self::DependsOn,
            Relation::RequiredBy => Self::RequiredBy,
            Relation::Duplicates => Self::Duplicates,
            Relation::DuplicatedBy => Self::DuplicatedBy,
            Relation::RelatesTo => Self::RelatesTo,
        }
    }
}

/// Another ticket, with its project, type and status.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueRelated)]
pub struct RelatedView {
    id: Uuid,
    #[schema(example = "CRM-224")]
    key: String,
    summary: String,
    project: ProjectView,
    #[serde(rename = "type")]
    issue_type: TypeView,
    status: StatusView,
}

/// A link of a ticket, as it reads from it.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueLink)]
pub struct LinkView {
    id: Uuid,
    /// How it reads from this ticket: `blocks`, `blocked_by`…
    relation: RelationView,
    /// The other ticket.
    issue: RelatedView,
}

/// A link to add.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = NewIssueLink, example = json!({"relation": "blocks", "issue": "CRM-224"}))]
pub struct NewLinkBody {
    /// How the link reads from this ticket.
    relation: RelationView,
    /// Id, key or former key of the other ticket.
    issue: String,
}

/// The parent of a ticket.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(as = IssueParent, example = json!({"parentId": null}))]
pub struct ParentBody {
    /// A ticket of the same project; `null`: at the top (never for a
    /// sub-task).
    #[serde(deserialize_with = "present")]
    #[schema(value_type = Option<Uuid>, required = true)]
    parent_id: Option<Option<Uuid>>,
}

/// `?search=&projectKey=` of tickets.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub struct SearchParams {
    /// A key or former key (`PLAT-12`), or part of a summary, any case.
    pub search: Option<String>,
    /// Only the tickets of this project.
    pub project_key: Option<String>,
}

/// The views of `issues` whose project the member sees, in their order.
pub(crate) async fn related_views(
    state: &AppState,
    tx: &mut TenantTx,
    language: &Language,
    issues: Vec<Issue>,
) -> Result<Vec<RelatedView>, Problem> {
    let mut projects: BTreeMap<Uuid, Option<(Node, Content)>> = BTreeMap::new();
    let mut views = Vec::new();
    for issue in issues {
        if let std::collections::btree_map::Entry::Vacant(entry) = projects.entry(issue.project_id) {
            let seen = match visible_node(state, tx, issue.project_id, "").await {
                Ok(node) => Some((node, Content::of(tx, issue.project_id).await?)),
                Err(problem) if problem.code == ErrorCode::NotFound => None,
                Err(problem) => return Err(problem),
            };
            entry.insert(seen);
        }
        let Some(Some((node, content))) = projects.get(&issue.project_id) else {
            continue;
        };
        views.push(RelatedView {
            issue_type: language.type_of(content, issue.type_id),
            status: language.status_view(content, issue.status_id),
            project: project_view(node),
            id: issue.id,
            key: issue.key,
            summary: issue.summary,
        });
    }
    Ok(views)
}

/// The links of the ticket `issue` to tickets the member sees.
pub(crate) async fn link_views(
    state: &AppState,
    tx: &mut TenantTx,
    language: &Language,
    issue: Uuid,
) -> Result<Vec<LinkView>, Problem> {
    let links = link_repo::of_issue(tx, issue).await?;
    let others: Vec<Uuid> = links.iter().map(|l| l.other(issue)).collect();
    let found = repo::find_many(tx, &others).await?;
    // In the order of the links.
    let ordered: Vec<Issue> = others
        .iter()
        .filter_map(|id| found.iter().find(|i| i.id == *id).cloned())
        .collect();
    let related: BTreeMap<Uuid, RelatedView> = related_views(state, tx, language, ordered)
        .await?
        .into_iter()
        .map(|v| (v.id, v))
        .collect();
    Ok(links
        .iter()
        .filter_map(|link| {
            // Two tickets may have several links: each gets the other ticket.
            let other = related.get(&link.other(issue))?.clone();
            Some(LinkView {
                id: link.id,
                relation: Relation::of(link.link_type, link.source_id == issue).into(),
                issue: other,
            })
        })
        .collect())
}

/// The view of one link of `issue`, the other ticket being `other`.
async fn link_view(
    state: &AppState,
    tx: &mut TenantTx,
    language: &Language,
    issue: Uuid,
    link: &StoredLink,
    other: Issue,
) -> Result<LinkView, Problem> {
    let related = related_views(state, tx, language, vec![other])
        .await?
        .pop()
        .ok_or_else(|| Problem::new(ErrorCode::NotFound))?;
    Ok(LinkView {
        id: link.id,
        relation: Relation::of(link.link_type, link.source_id == issue).into(),
        issue: related,
    })
}

/// `edit` on both projects, both active.
async fn require_edit_both(
    state: &AppState,
    tx: &mut TenantTx,
    one: &Node,
    other: &Node,
) -> Result<(), Problem> {
    require_edit(state, tx, one.id).await?;
    ensure_changeable(one)?;
    if other.id != one.id {
        require_edit(state, tx, other.id).await?;
        ensure_changeable(other)?;
    }
    Ok(())
}

/// `POST /api/v1/issues/{idOrKey}/links`.
#[utoipa::path(
    post,
    path = "/issues/{idOrKey}/links",
    tag = TAG,
    operation_id = "createIssueLink",
    summary = "Link a ticket to another",
    description = "Members with `edit` on both tickets' projects (tenant admins always), across \
                   projects of the organization. The link reads one way from each ticket \
                   (`blocks` / `blocked_by`…); two tickets have each kind once. Writes \
                   `issue.linked` to the event outbox.",
    params(
        ("idOrKey" = String, Path, description = "Id, key or former key of the ticket"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    request_body = NewLinkBody,
    responses(
        (status = 201, description = "The link, as it reads from this ticket", body = LinkView),
        (status = 400, description = "`validation_failed`: `issue` `not_found` (no such ticket the member sees) or `self`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`issue_link.exists`, or `node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id_or_key): ApiPath<String>,
    ApiJson(body): ApiJson<NewLinkBody>,
) -> Result<Response, Problem> {
    let (issue, project) = visible_issue(&state, &mut tx, &id_or_key).await?;
    let (other, other_project) = match visible_issue(&state, &mut tx, &body.issue).await {
        Ok(found) => found,
        Err(problem) if problem.code == ErrorCode::NotFound => {
            return Err(Problem::validation(vec![FieldError::new("issue", "not_found")]));
        }
        Err(problem) => return Err(problem),
    };
    let link = Link::new(issue.id.as_u128(), body.relation.into(), other.id.as_u128());
    if let Err(LinkError::ToItself) = validate_link(&link, &[]) {
        return Err(Problem::validation(vec![FieldError::new(
            "issue",
            LinkError::ToItself.code(),
        )]));
    }
    require_edit_both(&state, &mut tx, &project, &other_project).await?;
    let existing: Vec<Link> = link_repo::of_issue(&mut tx, issue.id)
        .await?
        .iter()
        .map(StoredLink::domain)
        .collect();
    if validate_link(&link, &existing).is_err() {
        return Err(Problem::new(ErrorCode::IssueLinkExists));
    }
    let actor = tx.account_id();
    let stored = link_repo::insert(&mut tx, infra::id::new_id(), &link, actor)
        .await?
        .map_err(|_| Problem::new(ErrorCode::IssueLinkExists))?;
    let (source_key, target_key) = if stored.source_id == issue.id {
        (&issue.key, &other.key)
    } else {
        (&other.key, &issue.key)
    };
    infra::outbox::record(
        &mut tx,
        &infra::outbox::Event {
            kind: "issue.linked",
            aggregate_type: "issue",
            aggregate_id: stored.source_id,
            payload: serde_json::json!({
                "linkId": stored.id,
                "linkType": stored.link_type.as_str(),
                "sourceId": stored.source_id,
                "targetId": stored.target_id,
                "sourceKey": source_key,
                "targetKey": target_key,
            }),
            actor_id: Some(actor),
        },
    )
    .await?;
    let from_source = stored.source_id == issue.id;
    crate::issue_history::record(
        &mut tx,
        issue.id,
        &[crate::issue_history::link(
            true,
            Relation::of(stored.link_type, from_source),
            other.id,
        )],
    )
    .await?;
    crate::issue_history::record(
        &mut tx,
        other.id,
        &[crate::issue_history::link(
            true,
            Relation::of(stored.link_type, !from_source),
            issue.id,
        )],
    )
    .await?;
    tracing::info!(target: "audit", actor = %actor, link = %stored.id, source = %source_key, target = %target_key, "tickets linked");
    let language = Language::of(&tx, &accept);
    let view = link_view(&state, &mut tx, &language, issue.id, &stored, other).await?;
    tx.commit().await?;
    let mut response = (StatusCode::CREATED, ApiJson(view)).into_response();
    language.headers(&mut response);
    Ok(response)
}

/// `DELETE /api/v1/issues/{idOrKey}/links/{linkId}`.
#[utoipa::path(
    delete,
    path = "/issues/{idOrKey}/links/{linkId}",
    tag = TAG,
    operation_id = "deleteIssueLink",
    summary = "Remove a link of a ticket",
    description = "Members with `edit` on both tickets' projects. Writes `issue.unlinked` to the \
                   event outbox.",
    params(
        ("idOrKey" = String, Path, description = "Id, key or former key of the ticket"),
        ("linkId" = Uuid, Path, description = "Id of the link"),
    ),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member, or no such link of it", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath((id_or_key, link_id)): ApiPath<(String, Uuid)>,
) -> Result<StatusCode, Problem> {
    let (issue, project) = visible_issue(&state, &mut tx, &id_or_key).await?;
    let not_found = || Problem::new(ErrorCode::NotFound).with_detail("no such link");
    let link = link_repo::find(&mut tx, link_id)
        .await?
        .filter(|l| l.source_id == issue.id || l.target_id == issue.id)
        .ok_or_else(not_found)?;
    let other = repo::find(&mut tx, link.other(issue.id))
        .await?
        .ok_or_else(not_found)?;
    let other_project = match visible_node(&state, &mut tx, other.project_id, "no such link").await {
        Ok(node) => node,
        Err(problem) if problem.code == ErrorCode::NotFound => return Err(not_found()),
        Err(problem) => return Err(problem),
    };
    require_edit_both(&state, &mut tx, &project, &other_project).await?;
    link_repo::delete(&mut tx, link.id).await?;
    let actor = tx.account_id();
    infra::outbox::record(
        &mut tx,
        &infra::outbox::Event {
            kind: "issue.unlinked",
            aggregate_type: "issue",
            aggregate_id: link.source_id,
            payload: serde_json::json!({
                "linkId": link.id,
                "linkType": link.link_type.as_str(),
                "sourceId": link.source_id,
                "targetId": link.target_id,
            }),
            actor_id: Some(actor),
        },
    )
    .await?;
    let from_source = link.source_id == issue.id;
    crate::issue_history::record(
        &mut tx,
        issue.id,
        &[crate::issue_history::link(
            false,
            Relation::of(link.link_type, from_source),
            other.id,
        )],
    )
    .await?;
    crate::issue_history::record(
        &mut tx,
        other.id,
        &[crate::issue_history::link(
            false,
            Relation::of(link.link_type, !from_source),
            issue.id,
        )],
    )
    .await?;
    tracing::info!(target: "audit", actor = %actor, link = %link.id, "tickets unlinked");
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /api/v1/issues/{idOrKey}/parent`: requires `If-Match`.
#[utoipa::path(
    put,
    path = "/issues/{idOrKey}/parent",
    tag = TAG,
    operation_id = "setIssueParent",
    summary = "Put a ticket under another, or at the top",
    description = "Members with `edit` on its project. The parent is a ticket of the same project, \
                   of a type the ticket's type may sit under, never the ticket itself or one below \
                   it; a sub-task always keeps one. Writes `issue.updated` to the event outbox.",
    params(
        ("idOrKey" = String, Path, description = "Id, key or former key of the ticket"),
        ("If-Match" = String, Header, description = "ETag of the version changed, or `*`"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    request_body = ParentBody,
    responses(
        (status = 200, description = "The ticket", body = IssueView, headers(
            ("ETag" = String, description = "Version of the ticket"),
        )),
        (status = 400, description = "`validation_failed`: `parentId` (`required`, `not_found`, `not_allowed`, `other_project`, `cycle`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "`precondition_failed`: the ticket changed since this version", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`precondition_required`: `If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn set_parent(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    headers: HeaderMap,
    ApiPath(id_or_key): ApiPath<String>,
    ApiJson(body): ApiJson<ParentBody>,
) -> Result<Response, Problem> {
    let if_match = require_if_match(&headers)?;
    let (issue, project) = visible_issue(&state, &mut tx, &id_or_key).await?;
    require_edit(&state, &mut tx, project.id).await?;
    ensure_changeable(&project)?;
    let issue = locked(&mut tx, issue.id, if_match.versions()).await?;
    let wanted = body.parent_id.flatten();
    let refused = |error: ParentError| Problem::validation(vec![FieldError::new("parentId", error.code())]);
    let parent = match wanted {
        None => None,
        Some(id) => {
            let parent = repo::find(&mut tx, id)
                .await?
                .ok_or_else(|| Problem::validation(vec![FieldError::new("parentId", "not_found")]))?;
            Some(NewParent {
                id: parent.id.as_u128(),
                type_id: parent.type_id.as_u128(),
                same_project: parent.project_id == issue.project_id,
                is_below_ticket: repo::is_below(&mut tx, issue.id, parent.id).await?,
            })
        }
    };
    let content = Content::of(&mut tx, project.id).await?;
    validate_parent(
        issue.id.as_u128(),
        issue.type_id.as_u128(),
        parent.as_ref(),
        &content.type_nodes(),
    )
    .map_err(refused)?;
    let language = Language::of(&tx, &accept);
    if wanted == issue.parent_id {
        let view = issue_view(&state, &mut tx, &language, issue, &project, true).await?;
        tx.commit().await?;
        return Ok(ticket_response(view, &language));
    }
    let former = issue.parent_id;
    let updated = repo::set_parent(&mut tx, issue.id, wanted).await?;
    let actor = tx.account_id();
    infra::outbox::record(
        &mut tx,
        &infra::outbox::Event {
            kind: "issue.updated",
            aggregate_type: "issue",
            aggregate_id: updated.id,
            payload: serde_json::json!({
                "key": updated.key,
                "projectId": project.id,
                "changes": [{"field": "parentId", "old": former, "new": wanted}],
            }),
            actor_id: Some(actor),
        },
    )
    .await?;
    crate::issue_history::record(
        &mut tx,
        updated.id,
        &[infra::audit::Entry::field(
            "parent",
            serde_json::json!(former),
            serde_json::json!(wanted),
        )],
    )
    .await?;
    tracing::info!(target: "audit", actor = %actor, issue = %updated.id, key = %updated.key, "ticket parent changed");
    let view = issue_view(&state, &mut tx, &language, updated, &project, true).await?;
    tx.commit().await?;
    Ok(ticket_response(view, &language))
}

/// `GET /api/v1/issues?search=`.
#[utoipa::path(
    get,
    path = "/issues",
    tag = TAG,
    operation_id = "searchIssues",
    summary = "Find tickets by key or summary",
    description = "Whoever is signed in: the tickets of projects they see whose key or former key \
                   is `search` (first), or whose summary holds it, most recently updated first; ten \
                   at most. Empty without `search`. The query language of search (L06) will go \
                   further.",
    params(SearchParams, ("Accept-Language" = Option<String>, Header, description = "Language of the labels")),
    responses(
        (status = 200, description = "The tickets", body = Vec<RelatedView>),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "`projectKey`: no such project visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn search(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiQuery(params): ApiQuery<SearchParams>,
) -> Result<Response, Problem> {
    let project = match params.project_key.as_deref() {
        Some(key) => Some(project_of_key(&state, &mut tx, key).await?.id),
        None => None,
    };
    let language = Language::of(&tx, &accept);
    let text = params.search.as_deref().map(str::trim).unwrap_or_default();
    let mut found = Vec::new();
    if !text.is_empty() {
        // More than needed: some may be in projects the member does not see.
        let candidates = repo::search(&mut tx, text, project, SEARCH_LIMIT * 5).await?;
        found = related_views(&state, &mut tx, &language, candidates).await?;
        found.truncate(SEARCH_LIMIT);
    }
    tx.commit().await?;
    let mut response = ApiJson(found).into_response();
    language.headers(&mut response);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relations_map_both_ways() {
        for relation in Relation::ALL {
            assert_eq!(Relation::from(RelationView::from(relation)), relation);
            assert_eq!(
                serde_json::to_value(RelationView::from(relation)).unwrap(),
                relation.as_str()
            );
        }
    }

    #[test]
    fn a_parent_is_given_even_when_null() {
        let top: ParentBody = serde_json::from_value(serde_json::json!({"parentId": null})).unwrap();
        assert_eq!(top.parent_id, Some(None));
        assert!(serde_json::from_value::<ParentBody>(serde_json::json!({})).is_err());
    }
}
