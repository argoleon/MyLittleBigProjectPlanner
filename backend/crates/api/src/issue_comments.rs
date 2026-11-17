//! Comments of a ticket (specification §12, L05-US04).
//!
//! A comment is a rich text document (the allow-list of
//! `domain::rich_text`, with `@person` mentions and ticket references),
//! stored with its sanitized HTML, internal or public. Writing one takes
//! `comment` on the project; its author edits and deletes it, those who
//! administer the project delete anyone's (`domain::comments`). A deleted
//! comment stays as a placeholder. Each change writes its event to the
//! outbox, with the accounts mentioned (L05-US07 notifies them) and, for an
//! edit or a deletion, the text before it; and its entry to the ticket's
//! history (L05-US06), with the content before and after.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use domain::audit::Action;
use domain::comments::{Acting, Visibility, can_delete, can_edit};
use domain::permissions::Permission;
use domain::rich_text::Doc;
use domain::validation::FieldError;
use infra::comments::{self as repo, Comment, Source, Written};
use infra::issues::Issue;
use infra::nodes::Node;
use infra::uuid::Uuid;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::etag::{etag, require_if_match};
use crate::extract::{AcceptLanguage, ApiJson, ApiPath};
use crate::issues::{PersonView, TAG, person, visible_issue};
use crate::nodes::{ensure_changeable, is_tenant_admin};
use crate::permissions;
use crate::problem::{ErrorCode, PROBLEM_JSON, Problem, ProblemBody};
use crate::roles::Language;
use crate::tenant::{AppState, TenantTx};

/// Who sees a comment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = CommentVisibility)]
pub enum VisibilityView {
    /// The members who see the ticket.
    #[default]
    Internal,
    /// Also the requester, on the portal.
    Public,
}

impl From<VisibilityView> for Visibility {
    fn from(view: VisibilityView) -> Self {
        match view {
            VisibilityView::Internal => Self::Internal,
            VisibilityView::Public => Self::Public,
        }
    }
}

impl From<Visibility> for VisibilityView {
    fn from(visibility: Visibility) -> Self {
        match visibility {
            Visibility::Internal => Self::Internal,
            Visibility::Public => Self::Public,
        }
    }
}

/// A comment of a ticket.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = IssueComment)]
pub struct CommentView {
    id: Uuid,
    /// None: an erased account.
    #[schema(required = true)]
    author: Option<PersonView>,
    visibility: VisibilityView,
    /// Written by a workflow transition (its "add a comment").
    by_transition: bool,
    /// The rich text document, as the editor reads it; `null` once deleted.
    #[schema(required = true)]
    content: Option<serde_json::Value>,
    /// Sanitized HTML to show; `null` once deleted.
    #[schema(required = true)]
    content_html: Option<String>,
    deleted: bool,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[schema(required = true)]
    #[serde(with = "time::serde::rfc3339::option")]
    edited_at: Option<OffsetDateTime>,
    version: i64,
    /// The member reading may edit it.
    can_edit: bool,
    /// The member reading may delete it.
    can_delete: bool,
}

/// A comment to write, or what an edit makes of it.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schema(
    as = CommentBody,
    example = json!({"content": {"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Done in PLAT-120"}]}]}, "visibility": "internal"})
)]
pub struct CommentBody {
    /// A rich text document (Tiptap JSON): the description's nodes and
    /// marks, `mention` (`attrs.id`: an active member) and `issueRef`
    /// (`attrs.id`: a ticket key). Ticket keys typed in text become links.
    content: serde_json::Value,
    /// `internal` by default.
    #[serde(default)]
    visibility: VisibilityView,
}

/// What the member acting may do with comments on `project`.
pub(crate) async fn acting(state: &AppState, tx: &mut TenantTx, project: &Node) -> Result<Acting, Problem> {
    let account = tx.account_id().as_u128();
    if is_tenant_admin(tx) {
        return Ok(Acting {
            account,
            comments: true,
            moderates: true,
        });
    }
    let permissions = permissions::effective(state, tx, Some(project.id)).await?;
    Ok(Acting {
        account,
        comments: permissions.contains(Permission::Comment),
        moderates: permissions.contains(Permission::AdministerNode),
    })
}

fn view(comment: Comment, acting: &Acting) -> CommentView {
    let author = comment.author_id.map(|id| id.as_u128());
    let deleted = comment.deleted_at.is_some();
    CommentView {
        can_edit: can_edit(author, deleted, acting),
        can_delete: can_delete(author, deleted, acting),
        author: person(comment.author_id, comment.author_name.as_ref()),
        visibility: comment.visibility.into(),
        by_transition: comment.source == Source::Transition,
        content: comment.content,
        content_html: comment.content_html,
        deleted,
        id: comment.id,
        created_at: comment.created_at,
        edited_at: comment.edited_at,
        version: comment.version,
    }
}

/// The document of `body`, checked: the allow-list, its limits, not empty,
/// its mentions active members; else the field errors.
async fn read_content(tx: &mut TenantTx, json: &serde_json::Value) -> Result<Doc, Problem> {
    let refused = |code| Problem::validation(vec![FieldError::new("content", code)]);
    let doc = infra::rich_text::parse(json).map_err(|_| refused("invalid"))?;
    doc.validate().map_err(|refusal| refused(refusal.code()))?;
    if doc.is_empty() {
        return Err(refused("required"));
    }
    let mentioned: Vec<Uuid> = doc.mentions().into_iter().map(Uuid::from_u128).collect();
    if !mentioned.is_empty() && infra::rule_references::members(tx, &mentioned).await?.len() < mentioned.len()
    {
        return Err(refused("unknown_member"));
    }
    Ok(doc)
}

/// The comment `id` of `issue`; 404 otherwise (another ticket's included).
async fn comment_of(tx: &mut TenantTx, issue: &Issue, id: Uuid) -> Result<Comment, Problem> {
    repo::find(tx, id)
        .await?
        .filter(|c| c.issue_id == issue.id)
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such comment"))
}

fn plain(content: Option<&serde_json::Value>) -> String {
    content
        .and_then(|json| infra::rich_text::parse(json).ok())
        .map(|doc| doc.plain_text())
        .unwrap_or_default()
}

/// Writes `doc` as a comment of `issue` and its `comment.created` event; it
/// as stored. Used by members and by transitions' "add a comment".
pub(crate) async fn write(
    tx: &mut TenantTx,
    issue: &Issue,
    source: Source,
    visibility: Visibility,
    doc: &Doc,
) -> Result<Comment, Problem> {
    let author = tx.account_id();
    // Those who comment follow the ticket.
    infra::watchers::watch(tx, issue.id, author).await?;
    let content = infra::rich_text::to_json(doc);
    let html = doc.to_html();
    let mentions: Vec<Uuid> = doc.mentions().into_iter().map(Uuid::from_u128).collect();
    let comment = repo::insert(
        tx,
        infra::id::new_id(),
        issue.id,
        author,
        source,
        &Written {
            visibility,
            content: &content,
            content_html: &html,
            mentions: &mentions,
        },
    )
    .await?;
    crate::issue_history::record(
        tx,
        issue.id,
        &[crate::issue_history::comment(
            Action::CommentCreated,
            comment.id,
            None,
            Some(&content),
        )],
    )
    .await?;
    infra::outbox::record(
        tx,
        &infra::outbox::Event {
            kind: "comment.created",
            aggregate_type: "issue",
            aggregate_id: issue.id,
            payload: serde_json::json!({
                "commentId": comment.id,
                "key": issue.key,
                "projectId": issue.project_id,
                "visibility": visibility.as_str(),
                "source": source.as_str(),
                "mentions": mentions,
                "issueRefs": doc.issue_refs(),
                "text": doc.plain_text(),
            }),
            actor_id: Some(author),
        },
    )
    .await?;
    Ok(comment)
}

/// `GET /api/v1/issues/{idOrKey}/comments`.
#[utoipa::path(
    get,
    path = "/issues/{idOrKey}/comments",
    tag = TAG,
    operation_id = "listIssueComments",
    summary = "The comments of a ticket",
    description = "Whoever sees its project: internal and public comments, oldest first, deleted \
                   ones as placeholders. The portal (L13) will only ever return public ones.",
    params(("idOrKey" = String, Path, description = "Id, key or former key of the ticket")),
    responses(
        (status = 200, description = "The comments", body = Vec<CommentView>),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath(id_or_key): ApiPath<String>,
) -> Result<ApiJson<Vec<CommentView>>, Problem> {
    let (issue, project) = visible_issue(&state, &mut tx, &id_or_key).await?;
    let acting = acting(&state, &mut tx, &project).await?;
    let comments = repo::of_issue(&mut tx, issue.id, false).await?;
    tx.commit().await?;
    Ok(ApiJson(comments.into_iter().map(|c| view(c, &acting)).collect()))
}

/// `POST /api/v1/issues/{idOrKey}/comments`.
#[utoipa::path(
    post,
    path = "/issues/{idOrKey}/comments",
    tag = TAG,
    operation_id = "createIssueComment",
    summary = "Comment on a ticket",
    description = "Members with `comment` on its project (tenant admins always). Writes \
                   `comment.created` to the event outbox, with the accounts mentioned.",
    params(
        ("idOrKey" = String, Path, description = "Id, key or former key of the ticket"),
        ("Accept-Language" = Option<String>, Header, description = "Language of the labels"),
    ),
    request_body = CommentBody,
    responses(
        (status = 201, description = "The comment", body = CommentView, headers(
            ("ETag" = String, description = "Version of the comment"),
        )),
        (status = 400, description = "`validation_failed`: `content` (`required`, `invalid`, `too_long`, `too_deep`, `unknown_member`)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    mut tx: TenantTx,
    accept: AcceptLanguage,
    ApiPath(id_or_key): ApiPath<String>,
    ApiJson(body): ApiJson<CommentBody>,
) -> Result<Response, Problem> {
    let (issue, project) = visible_issue(&state, &mut tx, &id_or_key).await?;
    let acting = acting(&state, &mut tx, &project).await?;
    if !acting.comments {
        return Err(Problem::new(ErrorCode::PermissionDenied).with_detail("missing permission: comment"));
    }
    ensure_changeable(&project)?;
    let doc = read_content(&mut tx, &body.content).await?;
    let comment = write(&mut tx, &issue, Source::Member, body.visibility.into(), &doc).await?;
    tracing::info!(target: "audit", actor = %tx.account_id(), issue = %issue.id, comment = %comment.id, "comment written");
    let language = Language::of(&tx, &accept);
    tx.commit().await?;
    let version = comment.version;
    let mut response = (StatusCode::CREATED, ApiJson(view(comment, &acting))).into_response();
    response.headers_mut().insert(header::ETAG, etag(version));
    language.headers(&mut response);
    Ok(response)
}

/// `PUT /api/v1/issues/{idOrKey}/comments/{commentId}`: requires `If-Match`.
#[utoipa::path(
    put,
    path = "/issues/{idOrKey}/comments/{commentId}",
    tag = TAG,
    operation_id = "updateIssueComment",
    summary = "Edit a comment",
    description = "Its author, while they hold `comment` on the project. Writes `comment.updated` \
                   to the event outbox with the text before and after, and the accounts mentioned.",
    params(
        ("idOrKey" = String, Path, description = "Id, key or former key of the ticket"),
        ("commentId" = Uuid, Path, description = "Id of the comment"),
        ("If-Match" = String, Header, description = "ETag of the version edited, or `*`"),
    ),
    request_body = CommentBody,
    responses(
        (status = 200, description = "The comment", body = CommentView, headers(
            ("ETag" = String, description = "Version of the comment"),
        )),
        (status = 400, description = "`validation_failed`: `content`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`: not its author, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member, or comment of it (deleted included)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 412, description = "`precondition_failed`: the comment changed since this version", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 428, description = "`precondition_required`: `If-Match` is missing", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn update(
    State(state): State<AppState>,
    mut tx: TenantTx,
    headers: HeaderMap,
    ApiPath((id_or_key, comment_id)): ApiPath<(String, Uuid)>,
    ApiJson(body): ApiJson<CommentBody>,
) -> Result<Response, Problem> {
    let if_match = require_if_match(&headers)?;
    let (issue, project) = visible_issue(&state, &mut tx, &id_or_key).await?;
    let acting = acting(&state, &mut tx, &project).await?;
    let comment = locked(&mut tx, &issue, comment_id, if_match.versions()).await?;
    if comment.deleted_at.is_some() {
        return Err(Problem::new(ErrorCode::NotFound).with_detail("no such comment"));
    }
    if !can_edit(comment.author_id.map(|id| id.as_u128()), false, &acting) {
        return Err(Problem::new(ErrorCode::PermissionDenied).with_detail("only its author edits a comment"));
    }
    ensure_changeable(&project)?;
    let doc = read_content(&mut tx, &body.content).await?;
    let before = plain(comment.content.as_ref());
    let former_mentions = comment.mentions.clone();
    let content = infra::rich_text::to_json(&doc);
    let html = doc.to_html();
    let mentions: Vec<Uuid> = doc.mentions().into_iter().map(Uuid::from_u128).collect();
    let visibility: Visibility = body.visibility.into();
    let edited = repo::update(
        &mut tx,
        comment.id,
        &Written {
            visibility,
            content: &content,
            content_html: &html,
            mentions: &mentions,
        },
    )
    .await?;
    let added: Vec<&Uuid> = mentions.iter().filter(|m| !former_mentions.contains(m)).collect();
    let actor = tx.account_id();
    infra::outbox::record(
        &mut tx,
        &infra::outbox::Event {
            kind: "comment.updated",
            aggregate_type: "issue",
            aggregate_id: issue.id,
            payload: serde_json::json!({
                "commentId": edited.id,
                "key": issue.key,
                "projectId": issue.project_id,
                "visibility": visibility.as_str(),
                "formerVisibility": comment.visibility.as_str(),
                "mentions": mentions,
                "addedMentions": added,
                "issueRefs": doc.issue_refs(),
                "before": before,
                "text": doc.plain_text(),
            }),
            actor_id: Some(actor),
        },
    )
    .await?;
    crate::issue_history::record(
        &mut tx,
        issue.id,
        &[crate::issue_history::comment(
            Action::CommentUpdated,
            edited.id,
            comment.content.as_ref(),
            Some(&content),
        )],
    )
    .await?;
    tracing::info!(target: "audit", actor = %actor, issue = %issue.id, comment = %edited.id, "comment edited");
    tx.commit().await?;
    let version = edited.version;
    let mut response = ApiJson(view(edited, &acting)).into_response();
    response.headers_mut().insert(header::ETAG, etag(version));
    Ok(response)
}

/// The comment `id` of `issue`, locked, if its version is one of
/// `versions`; else 412.
async fn locked(
    tx: &mut TenantTx,
    issue: &Issue,
    id: Uuid,
    versions: Option<&[i64]>,
) -> Result<Comment, Problem> {
    let comment = comment_of(tx, issue, id).await?;
    let version = repo::lock(tx, comment.id)
        .await?
        .ok_or_else(|| Problem::new(ErrorCode::NotFound).with_detail("no such comment"))?;
    if versions.is_some_and(|v| !v.contains(&version)) {
        return Err(
            Problem::new(ErrorCode::PreconditionFailed).with_detail("the comment changed since this version")
        );
    }
    comment_of(tx, issue, id).await
}

/// `DELETE /api/v1/issues/{idOrKey}/comments/{commentId}`.
#[utoipa::path(
    delete,
    path = "/issues/{idOrKey}/comments/{commentId}",
    tag = TAG,
    operation_id = "deleteIssueComment",
    summary = "Delete a comment",
    description = "Its author while they hold `comment`, or those who administer the project \
                   (moderation). Its content goes; a placeholder stays. Writes `comment.deleted` to \
                   the event outbox with the text it had.",
    params(
        ("idOrKey" = String, Path, description = "Id, key or former key of the ticket"),
        ("commentId" = Uuid, Path, description = "Id of the comment"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`permission.denied`, or `csrf.invalid`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 404, description = "No such ticket visible to the member, or comment of it (deleted included)", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 409, description = "`node.archived`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn delete(
    State(state): State<AppState>,
    mut tx: TenantTx,
    ApiPath((id_or_key, comment_id)): ApiPath<(String, Uuid)>,
) -> Result<StatusCode, Problem> {
    let (issue, project) = visible_issue(&state, &mut tx, &id_or_key).await?;
    let acting = acting(&state, &mut tx, &project).await?;
    let comment = locked(&mut tx, &issue, comment_id, None).await?;
    if comment.deleted_at.is_some() {
        return Err(Problem::new(ErrorCode::NotFound).with_detail("no such comment"));
    }
    if !can_delete(comment.author_id.map(|id| id.as_u128()), false, &acting) {
        return Err(Problem::new(ErrorCode::PermissionDenied)
            .with_detail("only its author or a moderator deletes a comment"));
    }
    ensure_changeable(&project)?;
    let actor = tx.account_id();
    let before = plain(comment.content.as_ref());
    repo::delete(&mut tx, comment.id, actor).await?;
    infra::outbox::record(
        &mut tx,
        &infra::outbox::Event {
            kind: "comment.deleted",
            aggregate_type: "issue",
            aggregate_id: issue.id,
            payload: serde_json::json!({
                "commentId": comment.id,
                "key": issue.key,
                "projectId": issue.project_id,
                "authorId": comment.author_id,
                "before": before,
            }),
            actor_id: Some(actor),
        },
    )
    .await?;
    crate::issue_history::record(
        &mut tx,
        issue.id,
        &[crate::issue_history::comment(
            Action::CommentDeleted,
            comment.id,
            comment.content.as_ref(),
            None,
        )],
    )
    .await?;
    tracing::info!(target: "audit", actor = %actor, issue = %issue.id, comment = %comment.id, "comment deleted");
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibilities_map_both_ways() {
        for visibility in Visibility::ALL {
            assert_eq!(Visibility::from(VisibilityView::from(visibility)), visibility);
        }
        assert_eq!(
            serde_json::to_value(VisibilityView::default()).unwrap(),
            "internal"
        );
    }
}
