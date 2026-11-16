//! Integration tests on a real PostgreSQL: the history of tickets
//! (L05-US06). Monthly partitions created once and protected like their
//! table; entries of one change listed in their order, newest change first,
//! page by page; kept in their tenant; authors shown under their pseudonym
//! once erased; the content of an erased member's comments purged on
//! request.

#![allow(clippy::unwrap_used)]

use domain::audit::{Action, Origin};
use domain::comments::Visibility;
use domain::hierarchy::NodeType;
use infra::accounts::TenantRole;
use infra::audit::{self, Author, Entry};
use infra::comments::{self, Source, Written};
use infra::db::{begin_tenant_tx, set_account};
use infra::erasure::{self, ErasedMember};
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
            key: "PLAT",
            fields: NodeFields {
                key: None,
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

fn by(actor: Uuid) -> Author {
    Author {
        actor_id: Some(actor),
        origin: Origin::Interface,
    }
}

#[tokio::test]
async fn history_partitions_are_created_once_and_take_the_entries_of_their_month() {
    let db = TestDb::new().await;
    let (issue, admin) = ticket(&db).await;

    // The migration made them; the worker's check, as the application role,
    // finds nothing to do.
    let mut conn = db.app_pool.acquire().await.unwrap();
    assert_eq!(audit::ensure_partitions(&mut conn).await.unwrap(), 0);
    drop(conn);
    db.exec_as_owner(
        "DO $$ BEGIN EXECUTE format('DROP TABLE audit_log_%s', \
         to_char(date_trunc('month', now()) + interval '3 months', 'YYYY_MM')); END $$",
    )
    .await;
    let mut conn = db.app_pool.acquire().await.unwrap();
    assert_eq!(
        audit::ensure_partitions(&mut conn).await.unwrap(),
        1,
        "the last month again"
    );
    assert_eq!(audit::ensure_partitions(&mut conn).await.unwrap(), 0);
    drop(conn);

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    audit::record(&mut tx, issue.id, by(admin), &[Entry::action(Action::Created)])
        .await
        .unwrap();
    let (partition, current): (String, String) = sqlx::query_as(
        "SELECT tableoid::regclass::text, 'audit_log_' || to_char(now(), 'YYYY_MM') FROM audit_log",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(partition, current);
    tx.commit().await.unwrap();

    let forced: Vec<(String, bool)> = sqlx::query_as(
        "SELECT c.relname::text, c.relforcerowsecurity FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid
         WHERE i.inhparent = 'audit_log'::regclass ORDER BY 1",
    )
    .fetch_all(&db.owner_pool)
    .await
    .unwrap();
    assert_eq!(forced.len(), 5, "default, current and three ahead: {forced:?}");
    assert!(forced.iter().all(|(_, force)| *force), "{forced:?}");
}

#[tokio::test]
async fn entries_list_newest_change_first_in_their_order_page_by_page_in_their_tenant() {
    let db = TestDb::new().await;
    let (issue, admin) = ticket(&db).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let created = audit::record(&mut tx, issue.id, by(admin), &[Entry::action(Action::Created)])
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let edited = audit::record(
        &mut tx,
        issue.id,
        Author {
            actor_id: None,
            origin: Origin::Mcp,
        },
        &[
            Entry::field("status", json!({"id": "s1"}), json!({"id": "s2"})),
            Entry::field("assignee", json!(null), json!(admin)),
            Entry::field("dueDate", json!("2026-11-30"), json!(null)),
        ],
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        audit::record(&mut tx, issue.id, by(admin), &[]).await.unwrap(),
        None,
        "nothing to record, no change"
    );
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let first = audit::of_issue(&mut tx, issue.id, 2, None).await.unwrap();
    let fields: Vec<_> = first.items.iter().map(|e| e.field.as_deref()).collect();
    assert_eq!(fields, [Some("status"), Some("assignee")]);
    assert!(first.items.iter().all(|e| e.change_id == edited));
    assert!(
        first
            .items
            .iter()
            .all(|e| e.origin == Origin::Mcp && e.actor_id.is_none())
    );
    assert_eq!(first.items[1].old, None, "JSON null is stored as nothing");
    assert_eq!(first.items[1].new, Some(json!(admin)));
    let second = audit::of_issue(&mut tx, issue.id, 2, first.next).await.unwrap();
    let rest: Vec<_> = second.items.iter().map(|e| (e.action, e.field.clone())).collect();
    assert_eq!(
        rest,
        [
            (Action::Updated, Some("dueDate".to_owned())),
            (Action::Created, None)
        ]
    );
    assert_eq!(second.items[1].change_id, created);
    assert_eq!(second.items[1].actor_id, Some(admin));
    assert_eq!(second.items[1].actor_name.as_deref(), Some("Test Member"));
    assert_eq!(second.next, None);
    let index: (String,) =
        sqlx::query_as("SELECT indexdef FROM pg_indexes WHERE indexname = 'audit_log_entity_idx'")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(
        index.0.contains("(tenant_id, entity_id, created_at)"),
        "{}",
        index.0
    );
    tx.commit().await.unwrap();

    // Tenant B sees none of it and cannot write into tenant A.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert!(
        audit::of_issue(&mut tx, issue.id, 10, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let refused = sqlx::query(
        "INSERT INTO audit_log (id, tenant_id, entity_type, entity_id, change_id, action, origin)
         VALUES ($1, $2, 'issue', $3, $1, 'created', 'system')",
    )
    .bind(new_id())
    .bind(db.tenant_a.id)
    .bind(issue.id)
    .execute(&mut *tx)
    .await;
    assert!(
        refused.is_err(),
        "row-level security refuses another tenant's rows"
    );
}

#[tokio::test]
async fn the_database_refuses_an_update_without_its_field_and_unknown_origins() {
    let db = TestDb::new().await;
    let (issue, _) = ticket(&db).await;
    for (action, field, origin) in [
        ("updated", None, "interface"),
        ("created", Some("status"), "interface"),
        ("created", None, "web"),
    ] {
        let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
        let refused = sqlx::query(
            "INSERT INTO audit_log (id, tenant_id, entity_type, entity_id, change_id, action, field, origin)
             VALUES ($1, app_current_tenant(), 'issue', $2, $1, $3, $4, $5)",
        )
        .bind(new_id())
        .bind(issue.id)
        .bind(action)
        .bind(field)
        .bind(origin)
        .execute(&mut *tx)
        .await;
        assert!(refused.is_err(), "{action} {field:?} {origin}");
    }
}

#[tokio::test]
async fn erased_members_show_under_their_pseudonym_and_their_comments_can_be_purged() {
    let db = TestDb::new().await;
    let (issue, admin) = ticket(&db).await;
    let lucas = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let content = json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "Mine"}]}]});
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let comment = comments::insert(
        &mut tx,
        new_id(),
        issue.id,
        lucas.account_id,
        Source::Member,
        &Written {
            visibility: Visibility::Internal,
            content: &content,
            content_html: "<p>Mine</p>",
            mentions: &[],
        },
    )
    .await
    .unwrap();
    audit::record(
        &mut tx,
        issue.id,
        by(lucas.account_id),
        &[Entry {
            new: Some(json!({"commentId": comment.id, "content": content})),
            ..Entry::action(Action::CommentCreated)
        }],
    )
    .await
    .unwrap();
    audit::record(
        &mut tx,
        issue.id,
        by(lucas.account_id),
        &[Entry::field("summary", json!("Ticket"), json!("Ticket 2"))],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // Erased from this tenant: the account keeps its name elsewhere, the
    // history here shows the pseudonym.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    set_account(&mut tx, admin).await.unwrap();
    let number = erasure::erase_in_tenant(&mut tx, admin, lucas.account_id, &lucas.email, true)
        .await
        .unwrap();
    infra::members::remove(&mut tx, lucas.account_id, None)
        .await
        .unwrap()
        .unwrap();
    let pseudonym = format!("Deleted user #{number}");
    let page = audit::of_issue(&mut tx, issue.id, 10, None).await.unwrap();
    assert!(
        page.items
            .iter()
            .all(|e| e.actor_name.as_deref() == Some(pseudonym.as_str()))
    );
    let names = audit::names(&mut tx, &[lucas.account_id, admin, new_id()])
        .await
        .unwrap();
    assert_eq!(names.get(&lucas.account_id), Some(&pseudonym));
    assert_eq!(names.get(&admin).map(String::as_str), Some("Test Member"));
    assert_eq!(names.len(), 2, "unknown accounts are left out");
    let comment_now = comments::find(&mut tx, comment.id).await.unwrap().unwrap();
    assert_eq!(comment_now.author_name.as_deref(), Some(pseudonym.as_str()));

    // The worker's steps: the comment emptied, its content gone from the
    // history, the entry itself kept.
    let member = ErasedMember {
        account_id: lucas.account_id,
        pseudonym_number: number,
        purge_comments: true,
    };
    let done = erasure::anonymize(&mut tx, &member, &erasure::steps())
        .await
        .unwrap();
    assert_eq!(
        done,
        [
            ("invitations_sent", 0),
            ("comments", 1),
            ("history", 1),
            ("notifications", 0)
        ]
    );
    let again = erasure::anonymize(&mut tx, &member, &erasure::steps())
        .await
        .unwrap();
    assert_eq!(
        again,
        [
            ("invitations_sent", 0),
            ("comments", 0),
            ("history", 0),
            ("notifications", 0)
        ],
        "idempotent"
    );
    let comment_now = comments::find(&mut tx, comment.id).await.unwrap().unwrap();
    assert!(comment_now.deleted_at.is_some() && comment_now.content.is_none());
    let entries = audit::of_issue(&mut tx, issue.id, 10, None).await.unwrap().items;
    let written = entries
        .iter()
        .find(|e| e.action == Action::CommentCreated)
        .unwrap();
    assert_eq!(written.new, Some(json!({"commentId": comment.id})));
    let renamed = entries.iter().find(|e| e.action == Action::Updated).unwrap();
    assert_eq!(renamed.new, Some(json!("Ticket 2")), "other changes are kept");
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn kept_comments_stay_in_the_history_when_not_purged() {
    let db = TestDb::new().await;
    let (issue, _) = ticket(&db).await;
    let lucas = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let comment_id = new_id();
    audit::record(
        &mut tx,
        issue.id,
        by(lucas.account_id),
        &[Entry {
            new: Some(json!({"commentId": comment_id, "content": {"type": "doc"}})),
            ..Entry::action(Action::CommentCreated)
        }],
    )
    .await
    .unwrap();
    let member = ErasedMember {
        account_id: lucas.account_id,
        pseudonym_number: 1,
        purge_comments: false,
    };
    let done = erasure::anonymize(&mut tx, &member, &erasure::steps())
        .await
        .unwrap();
    assert_eq!(
        done,
        [
            ("invitations_sent", 0),
            ("comments", 0),
            ("history", 0),
            ("notifications", 0)
        ]
    );
    let entries = audit::of_issue(&mut tx, issue.id, 10, None).await.unwrap().items;
    assert_eq!(
        entries[0].new,
        Some(json!({"commentId": comment_id, "content": {"type": "doc"}}))
    );
}
