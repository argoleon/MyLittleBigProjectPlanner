//! Integration tests on a real PostgreSQL: comments (L05-US04). Written,
//! listed (public ones only, as the portal will), edited, deleted to a
//! placeholder the database keeps consistent, and kept in their tenant.

#![allow(clippy::unwrap_used)]

use domain::comments::Visibility;
use domain::hierarchy::NodeType;
use infra::accounts::TenantRole;
use infra::comments::{self, Source, Written};
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::issues::{self, Issue, NewIssue};
use infra::nodes::{self, NewNode, NodeFields};
use infra::sqlx;
use infra::uuid::Uuid;
use serde_json::json;
use testkit::TestDb;

/// A ticket of a new project of tenant A, and the admin who made it.
async fn ticket(db: &TestDb) -> (Issue, Uuid) {
    let admin = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let node = nodes::insert(
        &mut tx,
        &NewNode {
            id: new_id(),
            node_type: NodeType::Project,
            key: Some("PLAT"),
            fields: NodeFields {
                name: "PLAT",
                description: "",
                objectives: "",
                start_date: None,
                end_date: None,
                health: domain::hierarchy::Health::OnTrack,
                owners: &[],
            },
            parent: None,
            created_by: admin,
        },
    )
    .await
    .unwrap()
    .unwrap();
    let fields = json!({});
    let issue = issues::create(
        &mut tx,
        &NewIssue {
            id: new_id(),
            project_id: node.id,
            project_key: "PLAT",
            type_id: Uuid::from_u128(1),
            status_id: Uuid::from_u128(2),
            parent_id: None,
            summary: "Ticket",
            description: None,
            priority: "medium",
            reporter_id: admin,
            assignee_id: None,
            start_date: None,
            due_date: None,
            fields: &fields,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (issue, admin)
}

#[tokio::test]
async fn comments_are_written_listed_edited_and_deleted_to_a_placeholder() {
    let db = TestDb::new().await;
    let (issue, admin) = ticket(&db).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let content = json!({"type": "doc", "content": []});
    let written = |visibility, html| Written {
        visibility,
        content: &content,
        content_html: html,
        mentions: &[],
    };
    let internal = comments::insert(
        &mut tx,
        new_id(),
        issue.id,
        admin,
        Source::Member,
        &written(Visibility::Internal, "<p>a</p>"),
    )
    .await
    .unwrap();
    let public = comments::insert(
        &mut tx,
        new_id(),
        issue.id,
        admin,
        Source::Transition,
        &written(Visibility::Public, "<p>b</p>"),
    )
    .await
    .unwrap();
    assert_eq!(
        (internal.version, internal.source, &internal.author_id),
        (1, Source::Member, &Some(admin))
    );
    assert!(internal.author_name.is_some());
    assert_eq!(public.source, Source::Transition);

    // All for members; public ones only for the portal.
    let ids = |list: Vec<comments::Comment>| list.into_iter().map(|c| c.id).collect::<Vec<_>>();
    assert_eq!(
        ids(comments::of_issue(&mut tx, issue.id, false).await.unwrap()),
        [internal.id, public.id]
    );
    assert_eq!(
        ids(comments::of_issue(&mut tx, issue.id, true).await.unwrap()),
        [public.id]
    );

    let mentioned = [admin];
    let edited = comments::update(
        &mut tx,
        internal.id,
        &Written {
            visibility: Visibility::Public,
            content: &content,
            content_html: "<p>c</p>",
            mentions: &mentioned,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (edited.version, edited.visibility, edited.mentions.as_slice()),
        (2, Visibility::Public, &mentioned[..])
    );
    assert!(edited.edited_at.is_some());
    assert_eq!(comments::lock(&mut tx, internal.id).await.unwrap(), Some(2));

    let deleted = comments::delete(&mut tx, internal.id, admin).await.unwrap();
    assert_eq!(
        (
            deleted.content,
            deleted.content_html,
            deleted.mentions,
            deleted.version
        ),
        (None, None, Vec::new(), 3)
    );
    assert!(deleted.deleted_at.is_some());
    tx.commit().await.unwrap();

    // The database refuses a comment without content that is not deleted.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let refused = sqlx::query("UPDATE comment SET content = NULL WHERE id = $1")
        .bind(public.id)
        .execute(&mut *tx)
        .await;
    assert!(refused.is_err(), "comment_content_or_deleted");
}

#[tokio::test]
async fn comments_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let (issue, admin) = ticket(&db).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let content = json!({"type": "doc", "content": []});
    let comment = comments::insert(
        &mut tx,
        new_id(),
        issue.id,
        admin,
        Source::Member,
        &Written {
            visibility: Visibility::Public,
            content: &content,
            content_html: "<p>a</p>",
            mentions: &[],
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let mut other = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert_eq!(comments::find(&mut other, comment.id).await.unwrap(), None);
    assert_eq!(comments::of_issue(&mut other, issue.id, true).await.unwrap(), []);
    assert_eq!(comments::lock(&mut other, comment.id).await.unwrap(), None);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM comment")
        .fetch_one(&mut *other)
        .await
        .unwrap();
    assert_eq!(count, 0);
    // Nor can it comment on tenant A's ticket.
    let written = Written {
        visibility: Visibility::Internal,
        content: &content,
        content_html: "<p>x</p>",
        mentions: &[],
    };
    assert!(
        comments::insert(&mut other, new_id(), issue.id, admin, Source::Member, &written)
            .await
            .is_err()
    );
}
