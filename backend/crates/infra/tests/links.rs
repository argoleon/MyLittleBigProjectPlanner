//! Integration tests on a real PostgreSQL: links between tickets, parents
//! and the ticket search (L05-US03). Each link once, "relates to" once
//! whatever its direction, never to itself (the database refuses it too);
//! links kept in their tenant; what sits below a ticket; tickets found by
//! key, former key or summary.

#![allow(clippy::unwrap_used)]

use domain::hierarchy::NodeType;
use domain::links::{Link, LinkType, Relation};
use infra::accounts::TenantRole;
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::issues::{self, Issue, NewIssue};
use infra::links::{self, Exists};
use infra::nodes::{self, NewNode, NodeFields};
use infra::sqlx::{self, PgConnection};
use infra::uuid::Uuid;
use serde_json::json;
use testkit::{SeedTenant, TestDb};

async fn project(db: &TestDb, tenant: &SeedTenant, code: &str) -> (Uuid, Uuid) {
    let admin = db.add_member(tenant, TenantRole::TenantAdmin).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let node = nodes::insert(
        &mut tx,
        &NewNode {
            id: new_id(),
            node_type: NodeType::Project,
            key: code,
            fields: NodeFields {
                key: None,
                name: code,
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
    tx.commit().await.unwrap();
    (node.id, admin)
}

async fn create(
    conn: &mut PgConnection,
    project: Uuid,
    code: &str,
    reporter: Uuid,
    summary: &str,
    parent: Option<Uuid>,
) -> Issue {
    let fields = json!({});
    issues::create(
        conn,
        &NewIssue {
            id: new_id(),
            project_id: project,
            project_key: code,
            type_id: Uuid::from_u128(1),
            status_id: Uuid::from_u128(2),
            parent_id: parent,
            summary,
            description: None,
            priority: "medium",
            reporter_id: reporter,
            assignee_id: None,
            start_date: None,
            due_date: None,
            fields: &fields,
        },
    )
    .await
    .unwrap()
}

fn between(one: &Issue, relation: Relation, other: &Issue) -> Link {
    Link::new(one.id.as_u128(), relation, other.id.as_u128())
}

#[tokio::test]
async fn each_link_is_stored_once_and_never_to_its_own_ticket() {
    let db = TestDb::new().await;
    let (plat, admin) = project(&db, &db.tenant_a, "PLAT").await;
    let (crm, _) = project(&db, &db.tenant_a, "CRM").await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let a = create(&mut tx, plat, "PLAT", admin, "A", None).await;
    let b = create(&mut tx, crm, "CRM", admin, "B", None).await;

    let blocks = links::insert(&mut tx, new_id(), &between(&a, Relation::Blocks, &b), admin)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (blocks.source_id, blocks.target_id, blocks.link_type),
        (a.id, b.id, LinkType::Blocks)
    );
    assert_eq!(blocks.other(a.id), b.id);
    assert_eq!(blocks.other(b.id), a.id);
    links::insert(&mut tx, new_id(), &between(&a, Relation::RelatesTo, &b), admin)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    // The same, even asked from the other side, is refused by the database;
    // "relates to" once, whatever its direction. (A refusal aborts its
    // transaction: one each.)
    for again in [
        between(&b, Relation::BlockedBy, &a),
        between(&b, Relation::RelatesTo, &a),
    ] {
        let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
        assert_eq!(
            links::insert(&mut tx, new_id(), &again, admin).await.unwrap(),
            Err(Exists)
        );
    }
    // Both sides list them; removed, gone.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(links::of_issue(&mut tx, b.id).await.unwrap().len(), 2);
    assert_eq!(
        links::find(&mut tx, blocks.id).await.unwrap(),
        Some(blocks.clone())
    );
    assert!(links::delete(&mut tx, blocks.id).await.unwrap());
    assert!(!links::delete(&mut tx, blocks.id).await.unwrap());
    assert_eq!(links::of_issue(&mut tx, a.id).await.unwrap().len(), 1);
    tx.commit().await.unwrap();

    // A ticket linked to itself: the database refuses it.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let to_itself = Link {
        source: a.id.as_u128(),
        target: a.id.as_u128(),
        link_type: LinkType::Blocks,
    };
    assert!(links::insert(&mut tx, new_id(), &to_itself, admin).await.is_err());
}

#[tokio::test]
async fn links_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let (plat, admin) = project(&db, &db.tenant_a, "PLAT").await;
    project(&db, &db.tenant_b, "PLAT").await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let a = create(&mut tx, plat, "PLAT", admin, "A", None).await;
    let b = create(&mut tx, plat, "PLAT", admin, "B", None).await;
    let link = links::insert(&mut tx, new_id(), &between(&a, Relation::DependsOn, &b), admin)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert_eq!(links::of_issue(&mut tx, a.id).await.unwrap(), []);
    assert_eq!(links::find(&mut tx, link.id).await.unwrap(), None);
    assert!(!links::delete(&mut tx, link.id).await.unwrap());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM issue_link")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 0);
    // Tenant B cannot link tenant A's tickets either.
    assert!(
        links::insert(&mut tx, new_id(), &between(&b, Relation::Blocks, &a), admin)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_parent_is_changed_and_what_sits_below_a_ticket_is_known() {
    let db = TestDb::new().await;
    let (plat, admin) = project(&db, &db.tenant_a, "PLAT").await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let epic = create(&mut tx, plat, "PLAT", admin, "Epic", None).await;
    let story = create(&mut tx, plat, "PLAT", admin, "Story", Some(epic.id)).await;
    let sub = create(&mut tx, plat, "PLAT", admin, "Sub", Some(story.id)).await;
    let other = create(&mut tx, plat, "PLAT", admin, "Other", None).await;

    assert!(issues::is_below(&mut tx, epic.id, sub.id).await.unwrap());
    assert!(issues::is_below(&mut tx, story.id, sub.id).await.unwrap());
    assert!(!issues::is_below(&mut tx, sub.id, epic.id).await.unwrap());
    assert!(!issues::is_below(&mut tx, other.id, sub.id).await.unwrap());
    assert!(!issues::is_below(&mut tx, epic.id, epic.id).await.unwrap());

    let moved = issues::set_parent(&mut tx, story.id, Some(other.id))
        .await
        .unwrap();
    assert_eq!((moved.parent_id, moved.version), (Some(other.id), 2));
    assert!(issues::is_below(&mut tx, other.id, sub.id).await.unwrap());
    assert!(!issues::is_below(&mut tx, epic.id, sub.id).await.unwrap());
    let top = issues::set_parent(&mut tx, story.id, None).await.unwrap();
    assert_eq!((top.parent_id, top.version), (None, 3));
    assert_eq!(
        issues::find_many(&mut tx, &[sub.id, epic.id, Uuid::nil()])
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn tickets_are_found_by_key_former_key_or_summary() {
    let db = TestDb::new().await;
    let (plat, admin) = project(&db, &db.tenant_a, "PLAT").await;
    let (crm, _) = project(&db, &db.tenant_a, "CRM").await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let oidc = create(&mut tx, plat, "PLAT", admin, "Migrate to OIDC", None).await;
    let percent = create(&mut tx, plat, "PLAT", admin, "Reach 100% of 50_mailboxes", None).await;
    tx.commit().await.unwrap();
    // Updated later: found first.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    create(&mut tx, crm, "CRM", admin, "OIDC client", None).await;
    tx.commit().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let keys = |found: Vec<Issue>| found.into_iter().map(|i| i.key).collect::<Vec<_>>();

    assert_eq!(
        keys(issues::search(&mut tx, "oidc", None, 10).await.unwrap()),
        ["CRM-1", "PLAT-1"]
    );
    assert_eq!(
        keys(issues::search(&mut tx, "OIDC", Some(plat), 10).await.unwrap()),
        ["PLAT-1"]
    );
    assert_eq!(
        keys(issues::search(&mut tx, "oidc", None, 1).await.unwrap()),
        ["CRM-1"]
    );
    // % and _ are text, not patterns.
    assert_eq!(
        keys(issues::search(&mut tx, "100%", None, 10).await.unwrap()),
        ["PLAT-2"]
    );
    assert_eq!(
        keys(issues::search(&mut tx, "0_m", None, 10).await.unwrap()),
        ["PLAT-2"]
    );
    assert_eq!(
        keys(issues::search(&mut tx, "x_y", None, 10).await.unwrap()),
        Vec::<String>::new()
    );
    // A key first, whatever its case; a former key finds the ticket.
    sqlx::query("INSERT INTO issue_key_alias (id, tenant_id, key, issue_id) VALUES ($1, $2, 'OLD-9', $3)")
        .bind(new_id())
        .bind(db.tenant_a.id)
        .bind(percent.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        keys(issues::search(&mut tx, " plat-1 ", None, 10).await.unwrap()),
        ["PLAT-1"]
    );
    assert_eq!(
        keys(issues::search(&mut tx, "old-9", None, 10).await.unwrap()),
        ["PLAT-2"]
    );
    assert_eq!(oidc.key, "PLAT-1");
    tx.commit().await.unwrap();

    let mut other = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert_eq!(issues::search(&mut other, "oidc", None, 10).await.unwrap(), []);
}
