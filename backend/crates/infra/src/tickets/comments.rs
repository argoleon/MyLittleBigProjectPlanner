//! A comment written on a ticket (L05-US04), by a member or by the
//! post-function of a transition: its author follows the ticket, the
//! history and the outbox tell it.

use domain::audit::Action;
use domain::comments::Visibility;
use domain::rich_text::Doc;
use sqlx::postgres::PgConnection;
use uuid::Uuid;

use crate::audit::Author;
use crate::comments::{self as repo, Comment, Source, Written};
use crate::issues::Issue;

/// Writes `doc` on `issue` as `account` (`author` in the history): the
/// comment, its history entry and its `comment.created` event.
pub async fn write(
    conn: &mut PgConnection,
    author: &Author,
    account: Uuid,
    issue: &Issue,
    source: Source,
    visibility: Visibility,
    doc: &Doc,
) -> Result<Comment, sqlx::Error> {
    // Those who comment follow the ticket.
    crate::watchers::watch(conn, issue.id, account).await?;
    let content = crate::rich_text::to_json(doc);
    let html = doc.to_html();
    let mentions: Vec<Uuid> = doc.mentions().into_iter().map(Uuid::from_u128).collect();
    let comment = repo::insert(
        conn,
        crate::id::new_id(),
        issue.id,
        account,
        source,
        &Written {
            visibility,
            content: &content,
            content_html: &html,
            mentions: &mentions,
        },
    )
    .await?;
    crate::audit::record(
        conn,
        issue.id,
        *author,
        &[super::history::comment(
            Action::CommentCreated,
            comment.id,
            None,
            Some(&content),
        )],
    )
    .await?;
    crate::outbox::record(
        conn,
        &crate::outbox::Event {
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
            actor_id: Some(account),
        },
    )
    .await?;
    Ok(comment)
}
