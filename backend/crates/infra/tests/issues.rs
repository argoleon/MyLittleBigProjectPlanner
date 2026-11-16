//! Integration tests on a real PostgreSQL: tickets (L05-US01). Keys
//! allocated in sequence per project, never twice under concurrent
//! creations; former keys found again; tickets, their aliases, their
//! external identifiers and outbox events kept in their tenant; the monthly
//! outbox partitions created once, by the application role.

#![allow(clippy::unwrap_used)]

use domain::hierarchy::NodeType;
use infra::accounts::TenantRole;
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::issues::{self, Issue, NewIssue};
use infra::nodes::{self, NewNode, NodeFields};
use infra::outbox::{self, Event};
use infra::sqlx::{self, PgConnection};
use infra::uuid::Uuid;
use serde_json::json;
use testkit::{SeedTenant, TestDb};

/// A project `key` of `tenant` created by a new tenant admin; its id and
/// that admin.
async fn project(db: &TestDb, tenant: &SeedTenant, key: &str) -> (Uuid, Uuid) {
    let admin = db.add_member(tenant, TenantRole::TenantAdmin).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let node = nodes::insert(
        &mut tx,
        &NewNode {
            id: new_id(),
            node_type: NodeType::Project,
            key: Some(key),
            fields: NodeFields {
                name: key,
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

async fn create(conn: &mut PgConnection, project: Uuid, key: &str, reporter: Uuid, summary: &str) -> Issue {
    let fields = json!({});
    issues::create(
        conn,
        &NewIssue {
            id: new_id(),
            project_id: project,
            project_key: key,
            type_id: Uuid::from_u128(1),
            status_id: Uuid::from_u128(2),
            parent_id: None,
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

async fn count(conn: &mut PgConnection, table: &str) -> i64 {
    // `table` is one of the test's own names.
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(conn)
        .await
        .unwrap()
}

#[tokio::test]
async fn concurrent_creations_in_a_project_never_share_a_key() {
    let db = TestDb::new().await;
    let (plat, admin) = project(&db, &db.tenant_a, "PLAT").await;
    let (mail, _) = project(&db, &db.tenant_a, "MAIL").await;

    let mut creations = Vec::new();
    for i in 0..20 {
        let pool = db.app_pool.clone();
        let tenant = db.tenant_a.id;
        creations.push(tokio::spawn(async move {
            let mut tx = begin_tenant_tx(&pool, tenant).await.unwrap();
            let issue = create(&mut tx, plat, "PLAT", admin, &format!("Ticket {i}")).await;
            tx.commit().await.unwrap();
            issue.key_number
        }));
    }
    let mut numbers = Vec::new();
    for creation in creations {
        numbers.push(creation.await.unwrap());
    }
    numbers.sort_unstable();
    assert_eq!(numbers, (1..=20).collect::<Vec<i32>>());

    // Each project counts its own; a rolled-back creation leaves a gap,
    // never a duplicate.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let first = create(&mut tx, mail, "MAIL", admin, "  First  ").await;
    assert_eq!((first.key.as_str(), first.summary.as_str()), ("MAIL-1", "First"));
    assert_eq!(first.reporter_id, Some(admin));
    assert!(first.reporter_name.is_some());
    assert_eq!(first.version, 1);
    tx.commit().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    create(&mut tx, mail, "MAIL", admin, "Rolled back").await;
    tx.rollback().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(create(&mut tx, mail, "MAIL", admin, "Next").await.key, "MAIL-2");
}

#[tokio::test]
async fn a_former_key_finds_its_ticket_and_tickets_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let (plat, admin) = project(&db, a, "PLAT").await;
    project(&db, b, "PLAT").await;

    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let issue = create(&mut tx, plat, "PLAT", admin, "Mine").await;
    sqlx::query("INSERT INTO issue_key_alias (id, tenant_id, key, issue_id) VALUES ($1, $2, 'OLD-7', $3)")
        .bind(new_id())
        .bind(a.id)
        .bind(issue.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO external_ref (id, tenant_id, issue_id, system, external_id) VALUES ($1, $2, $3, 'jira', 'ID-42')",
    )
    .bind(new_id())
    .bind(a.id)
    .bind(issue.id)
    .execute(&mut *tx)
    .await
    .unwrap();
    outbox::record(
        &mut tx,
        &Event {
            kind: "issue.created",
            aggregate_type: "issue",
            aggregate_id: issue.id,
            payload: json!({"key": "PLAT-1"}),
            actor_id: Some(admin),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        issues::id_of_key(&mut tx, "PLAT-1").await.unwrap(),
        Some(issue.id)
    );
    assert_eq!(issues::id_of_key(&mut tx, "OLD-7").await.unwrap(), Some(issue.id));
    assert_eq!(issues::id_of_key(&mut tx, "PLAT-2").await.unwrap(), None);
    let mine = issues::of_account(&mut tx, admin, None, 10).await.unwrap();
    assert_eq!(mine.items, std::slice::from_ref(&issue));
    tx.commit().await.unwrap();

    // Tenant B sees none of it, even under the same key, and cannot write
    // into tenant A.
    let mut tx = begin_tenant_tx(&db.app_pool, b.id).await.unwrap();
    assert_eq!(issues::find(&mut tx, issue.id).await.unwrap(), None);
    assert_eq!(issues::id_of_key(&mut tx, "PLAT-1").await.unwrap(), None);
    assert_eq!(issues::id_of_key(&mut tx, "OLD-7").await.unwrap(), None);
    for table in [
        "issue",
        "issue_key_alias",
        "external_ref",
        "outbox_event",
        "project_issue_counter",
    ] {
        assert_eq!(count(&mut tx, table).await, 0, "{table}");
    }
    let refused =
        sqlx::query("INSERT INTO issue_key_alias (id, tenant_id, key, issue_id) VALUES ($1, $2, 'X-1', $3)")
            .bind(new_id())
            .bind(a.id)
            .bind(issue.id)
            .execute(&mut *tx)
            .await;
    assert!(
        refused.is_err(),
        "row-level security refuses another tenant's rows"
    );
}

#[tokio::test]
async fn outbox_partitions_are_created_once_and_take_the_events_of_their_month() {
    let db = TestDb::new().await;
    let (plat, admin) = project(&db, &db.tenant_a, "PLAT").await;

    // The migration made them; the worker's check, as the application role,
    // finds nothing to do.
    let mut conn = db.app_pool.acquire().await.unwrap();
    assert_eq!(outbox::ensure_partitions(&mut conn).await.unwrap(), 0);
    drop(conn);
    db.exec_as_owner(
        "DO $$ BEGIN EXECUTE format('DROP TABLE outbox_event_%s', \
         to_char(date_trunc('month', now()) + interval '3 months', 'YYYY_MM')); END $$",
    )
    .await;
    let mut conn = db.app_pool.acquire().await.unwrap();
    assert_eq!(
        outbox::ensure_partitions(&mut conn).await.unwrap(),
        1,
        "the last month again"
    );
    assert_eq!(outbox::ensure_partitions(&mut conn).await.unwrap(), 0);
    drop(conn);

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let issue = create(&mut tx, plat, "PLAT", admin, "Mine").await;
    outbox::record(
        &mut tx,
        &Event {
            kind: "issue.created",
            aggregate_type: "issue",
            aggregate_id: issue.id,
            payload: json!({}),
            actor_id: None,
        },
    )
    .await
    .unwrap();
    let (partition, current): (String, String) = sqlx::query_as(
        "SELECT tableoid::regclass::text, 'outbox_event_' || to_char(now(), 'YYYY_MM') FROM outbox_event",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(partition, current);
    tx.commit().await.unwrap();

    // A new partition is protected like its table.
    let forced: Vec<(String, bool)> = sqlx::query_as(
        "SELECT c.relname::text, c.relforcerowsecurity FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid
         WHERE i.inhparent = 'outbox_event'::regclass ORDER BY 1",
    )
    .fetch_all(&db.owner_pool)
    .await
    .unwrap();
    assert_eq!(forced.len(), 5, "default, current and three ahead: {forced:?}");
    assert!(forced.iter().all(|(_, force)| *force), "{forced:?}");
}

#[tokio::test]
async fn an_edit_bumps_the_version_and_a_lock_holds_others_until_it_commits() {
    let db = TestDb::new().await;
    let (plat, admin) = project(&db, &db.tenant_a, "PLAT").await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let issue = create(&mut tx, plat, "PLAT", admin, "Draft").await;
    tx.commit().await.unwrap();

    // The first edit holds the lock; the second waits for it, then reads
    // the version it left.
    let mut first = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(issues::lock(&mut first, issue.id).await.unwrap(), Some(1));
    let pool = db.app_pool.clone();
    let tenant = db.tenant_a.id;
    let second = tokio::spawn(async move {
        let mut tx = begin_tenant_tx(&pool, tenant).await.unwrap();
        let version = issues::lock(&mut tx, issue.id).await.unwrap();
        tx.rollback().await.unwrap();
        version
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!second.is_finished(), "the second lock waits for the first");
    let fields = json!({"a": 1});
    let edited = issues::update(
        &mut first,
        issue.id,
        &issues::Edited {
            summary: "  Final  ",
            description: None,
            priority: "high",
            assignee_id: Some(admin),
            start_date: None,
            due_date: None,
            fields: &fields,
        },
    )
    .await
    .unwrap();
    first.commit().await.unwrap();
    assert_eq!(
        (
            edited.summary.as_str(),
            edited.priority.as_str(),
            edited.version,
            edited.assignee_id
        ),
        ("Final", "high", 2, Some(admin))
    );
    assert!(edited.updated_at > issue.updated_at);
    assert_eq!(second.await.unwrap(), Some(2));

    // Another tenant locks nothing.
    let mut other = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert_eq!(issues::lock(&mut other, issue.id).await.unwrap(), None);
}

#[tokio::test]
async fn a_move_takes_the_next_key_of_its_project_and_keeps_the_former_one() {
    let db = TestDb::new().await;
    let (plat, admin) = project(&db, &db.tenant_a, "PLAT").await;
    let (crm, _) = project(&db, &db.tenant_a, "CRM").await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let parent = create(&mut tx, plat, "PLAT", admin, "Parent").await;
    create(&mut tx, crm, "CRM", admin, "Already there").await;
    let fields = json!({});
    let sub = issues::create(
        &mut tx,
        &NewIssue {
            id: new_id(),
            project_id: plat,
            project_key: "PLAT",
            type_id: Uuid::from_u128(3),
            status_id: Uuid::from_u128(2),
            parent_id: Some(parent.id),
            summary: "Child",
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
    assert_eq!(
        issues::children(&mut tx, parent.id).await.unwrap(),
        std::slice::from_ref(&sub)
    );

    let kept = json!({"f": "v"});
    let moved = issues::move_to(
        &mut tx,
        &issues::Move {
            id: parent.id,
            project_id: crm,
            project_key: "CRM",
            type_id: Uuid::from_u128(7),
            status_id: Uuid::from_u128(8),
            parent_id: None,
            fields: &kept,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        (
            moved.key.as_str(),
            moved.key_number,
            moved.project_id,
            moved.version
        ),
        ("CRM-2", 2, crm, 2)
    );
    assert_eq!(
        (moved.type_id, moved.status_id, &moved.fields),
        (Uuid::from_u128(7), Uuid::from_u128(8), &kept)
    );
    assert_eq!(
        issues::id_of_key(&mut tx, "PLAT-1").await.unwrap(),
        Some(parent.id)
    );
    assert_eq!(
        issues::id_of_key(&mut tx, "CRM-2").await.unwrap(),
        Some(parent.id)
    );
    // The child stays under its parent; PLAT's numbers are never reused.
    assert_eq!(issues::children(&mut tx, parent.id).await.unwrap()[0].id, sub.id);
    assert_eq!(create(&mut tx, plat, "PLAT", admin, "Next").await.key, "PLAT-3");
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn a_transition_writes_the_status_assignee_and_values_and_bumps_the_version() {
    let db = TestDb::new().await;
    let (plat, admin) = project(&db, &db.tenant_a, "PLAT").await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let issue = create(&mut tx, plat, "PLAT", admin, "Draft").await;
    let fields = json!({"f": 3});
    let moved = issues::transition(&mut tx, issue.id, Uuid::from_u128(9), Some(admin), &fields)
        .await
        .unwrap();
    assert_eq!(
        (moved.status_id, moved.assignee_id, &moved.fields, moved.version),
        (Uuid::from_u128(9), Some(admin), &fields, 2)
    );
    let unassigned = issues::transition(&mut tx, issue.id, Uuid::from_u128(10), None, &fields)
        .await
        .unwrap();
    assert_eq!((unassigned.assignee_id, unassigned.version), (None, 3));
    tx.commit().await.unwrap();
}
