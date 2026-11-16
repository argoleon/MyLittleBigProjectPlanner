//! Integration tests on a real PostgreSQL: notifications (L05-US07).
//! Monthly partitions created once and protected; preferences kept per
//! account, out of other accounts' reach; a member's notifications read
//! across their organizations, only theirs, only where they are still an
//! active member, only those shown in the application; the digest's local
//! time; an outbox event dispatched once.

#![allow(clippy::unwrap_used)]

use domain::hierarchy::NodeType;
use domain::notifications::Event;
use domain::notify::{Choice, Preferences};
use infra::accounts::{AccountType, NewMember, TenantRole};
use infra::db::{begin_tenant_tx, set_account};
use infra::id::new_id;
use infra::issues::{self, Issue, NewIssue};
use infra::nodes::{self, NewNode, NodeFields};
use infra::notifications;
use infra::outbox::{self, Event as OutboxEvent};
use infra::sqlx;
use infra::uuid::Uuid;
use serde_json::json;
use testkit::{SeedTenant, TestDb};

/// A ticket of a new project (without configuration) of `tenant`, and its admin.
async fn ticket(db: &TestDb, tenant: &SeedTenant) -> (Issue, Uuid) {
    let admin = db.add_member(tenant, TenantRole::TenantAdmin).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
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

/// A notification of `account` about `issue`, as the worker writes it.
async fn notify(
    db: &TestDb,
    tenant: Uuid,
    account: Uuid,
    issue: &Issue,
    in_app: bool,
    email_wanted: bool,
) -> Uuid {
    let id = new_id();
    sqlx::query(
        "INSERT INTO notification (id, tenant_id, account_id, event, issue_id, details, outbox_event_id, in_app, email_wanted)
         VALUES ($1, $2, $3, 'assigned', $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(tenant)
    .bind(account)
    .bind(issue.id)
    .bind(json!({"key": issue.key, "summary": issue.summary}))
    .bind(new_id())
    .bind(in_app)
    .bind(email_wanted)
    .execute(&db.owner_pool)
    .await
    .unwrap();
    id
}

#[tokio::test]
async fn notification_partitions_are_created_once_and_protected() {
    let db = TestDb::new().await;
    let mut conn = db.app_pool.acquire().await.unwrap();
    assert_eq!(notifications::ensure_partitions(&mut conn).await.unwrap(), 0);
    drop(conn);
    db.exec_as_owner(
        "DO $$ BEGIN EXECUTE format('DROP TABLE notification_%s', \
         to_char(date_trunc('month', now()) + interval '3 months', 'YYYY_MM')); END $$",
    )
    .await;
    let mut conn = db.app_pool.acquire().await.unwrap();
    assert_eq!(notifications::ensure_partitions(&mut conn).await.unwrap(), 1);
    let forced: Vec<(String, bool)> = sqlx::query_as(
        "SELECT c.relname::text, c.relforcerowsecurity FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid
         WHERE i.inhparent = 'notification'::regclass ORDER BY 1",
    )
    .fetch_all(&db.owner_pool)
    .await
    .unwrap();
    assert_eq!(forced.len(), 5, "{forced:?}");
    assert!(forced.iter().all(|(_, force)| *force));
}

#[tokio::test]
async fn preferences_belong_to_their_account_only() {
    let db = TestDb::new().await;
    let lucas = db.add_member(&db.tenant_a, TenantRole::Member).await.account_id;
    let camille = db.add_member(&db.tenant_a, TenantRole::Member).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    set_account(&mut tx, lucas).await.unwrap();
    assert_eq!(
        notifications::my_preferences(&mut tx, lucas).await.unwrap(),
        Preferences::default()
    );
    let mine = Preferences::with(
        [(
            Event::Commented,
            Choice {
                in_app: false,
                email: false,
            },
        )],
        true,
    );
    notifications::set_my_preferences(&mut tx, lucas, &mine)
        .await
        .unwrap();
    assert_eq!(notifications::my_preferences(&mut tx, lucas).await.unwrap(), mine);
    // Camille's, from Lucas's transaction: invisible, and not writable.
    assert_eq!(
        notifications::my_preferences(&mut tx, camille).await.unwrap(),
        Preferences::default()
    );
    assert!(
        notifications::set_my_preferences(&mut tx, camille, &mine)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();

    // The worker reads them for the active members of its tenant only.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    set_account(&mut tx, lucas).await.unwrap();
    notifications::set_my_preferences(&mut tx, lucas, &mine)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let found = notifications::recipients(&mut tx, &[lucas, camille])
        .await
        .unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(
        found.iter().find(|r| r.account_id == lucas).unwrap().preferences,
        mine
    );
    tx.commit().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert!(
        notifications::recipients(&mut tx, &[lucas])
            .await
            .unwrap()
            .is_empty(),
        "not a member of beta"
    );
}

#[tokio::test]
async fn a_member_reads_their_own_notifications_of_their_active_memberships() {
    let db = TestDb::new().await;
    let (alpha_issue, _) = ticket(&db, &db.tenant_a).await;
    let (beta_issue, _) = ticket(&db, &db.tenant_b).await;
    let lucas = db.add_member(&db.tenant_a, TenantRole::Member).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    infra::accounts::add_member(
        &mut tx,
        &NewMember {
            email: &lucas.email,
            display_name: "Test Member",
            password_hash: None,
            account_type: AccountType::Internal,
            tenant_role: TenantRole::Member,
            local_account: false,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let other = db.add_member(&db.tenant_a, TenantRole::Member).await.account_id;
    let in_alpha = notify(&db, db.tenant_a.id, lucas.account_id, &alpha_issue, true, true).await;
    let in_beta = notify(&db, db.tenant_b.id, lucas.account_id, &beta_issue, true, false).await;
    notify(&db, db.tenant_a.id, lucas.account_id, &alpha_issue, false, true).await;
    notify(&db, db.tenant_a.id, other, &alpha_issue, true, true).await;

    let read_as = |account: Uuid| {
        let pool = db.app_pool.clone();
        let tenant = db.tenant_a.id;
        async move {
            let mut tx = begin_tenant_tx(&pool, tenant).await.unwrap();
            set_account(&mut tx, account).await.unwrap();
            tx
        }
    };
    let mut tx = read_as(lucas.account_id).await;
    let page = notifications::mine(&mut tx, None, false, None, 10).await.unwrap();
    let mut ids: Vec<Uuid> = page.items.iter().map(|n| n.id).collect();
    ids.sort();
    let mut expected = vec![in_alpha, in_beta];
    expected.sort();
    assert_eq!(
        ids, expected,
        "theirs, shown in the application, of both organizations"
    );
    let only_beta = notifications::mine(&mut tx, Some(db.tenant_b.id), false, None, 10)
        .await
        .unwrap();
    assert_eq!(
        only_beta.items.iter().map(|n| n.id).collect::<Vec<_>>(),
        [in_beta]
    );
    assert_eq!(only_beta.items[0].tenant_slug, db.tenant_b.slug);
    assert_eq!(
        notifications::unread(&mut tx)
            .await
            .unwrap()
            .values()
            .sum::<i64>(),
        2
    );
    // Ids of others are ignored.
    let others: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM notification WHERE account_id = $1")
        .bind(other)
        .fetch_all(&db.owner_pool)
        .await
        .unwrap();
    assert_eq!(
        notifications::mark_read(&mut tx, Some(&others), None)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        notifications::mark_read(&mut tx, Some(&[in_alpha]), None)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        notifications::unread(&mut tx)
            .await
            .unwrap()
            .values()
            .sum::<i64>(),
        1
    );
    tx.commit().await.unwrap();

    // Suspended in beta: its notifications are no longer read.
    sqlx::query("UPDATE membership SET status = 'suspended' WHERE tenant_id = $1 AND account_id = $2")
        .bind(db.tenant_b.id)
        .bind(lucas.account_id)
        .execute(&db.owner_pool)
        .await
        .unwrap();
    let mut tx = read_as(lucas.account_id).await;
    let page = notifications::mine(&mut tx, None, false, None, 10).await.unwrap();
    assert_eq!(page.items.iter().map(|n| n.id).collect::<Vec<_>>(), [in_alpha]);
    assert_eq!(
        notifications::mark_read(&mut tx, None, Some(db.tenant_b.id))
            .await
            .unwrap(),
        0
    );

    // The application role cannot read them past its tenant directly.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let direct: i64 = sqlx::query_scalar("SELECT count(*) FROM notification WHERE tenant_id = $1")
        .bind(db.tenant_b.id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(direct, 0);
}

#[tokio::test]
async fn the_digest_counts_in_the_members_time_zone_or_their_organizations() {
    let db = TestDb::new().await;
    let (issue, _) = ticket(&db, &db.tenant_a).await;
    let lucas = db.add_member(&db.tenant_a, TenantRole::Member).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    set_account(&mut tx, lucas).await.unwrap();
    notifications::set_my_preferences(&mut tx, lucas, &Preferences::with([], true))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut conn = db.app_pool.acquire().await.unwrap();
    assert!(
        notifications::digest_candidates(&mut conn)
            .await
            .unwrap()
            .is_empty(),
        "nothing waits"
    );
    let waiting = notify(&db, db.tenant_a.id, lucas, &issue, true, true).await;
    notify(&db, db.tenant_a.id, lucas, &issue, true, false).await;

    let expected = |zone: &str| {
        let pool = db.owner_pool.clone();
        let zone = zone.to_owned();
        async move {
            sqlx::query_as::<_, (i32, i32)>(
                "SELECT extract(isodow FROM now() AT TIME ZONE $1)::int, extract(hour FROM now() AT TIME ZONE $1)::int",
            )
            .bind(zone)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let tenant_zone: String = sqlx::query_scalar("SELECT time_zone FROM tenant WHERE id = $1")
        .bind(db.tenant_a.id)
        .fetch_one(&db.owner_pool)
        .await
        .unwrap();
    let candidate = notifications::digest_candidates(&mut conn)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(candidate.account_id, lucas);
    let (weekday, hour) = expected(&tenant_zone).await;
    assert_eq!(
        (
            i32::from(candidate.local_weekday),
            i32::from(candidate.local_hour)
        ),
        (weekday, hour)
    );
    sqlx::query("UPDATE account SET time_zone = 'Pacific/Kiritimati' WHERE id = $1")
        .bind(lucas)
        .execute(&db.owner_pool)
        .await
        .unwrap();
    let candidate = notifications::digest_candidates(&mut conn)
        .await
        .unwrap()
        .remove(0);
    let (weekday, hour) = expected("Pacific/Kiritimati").await;
    assert_eq!(
        (
            i32::from(candidate.local_weekday),
            i32::from(candidate.local_hour)
        ),
        (weekday, hour)
    );
    assert!(!candidate.sent_today);
    let gathered = notifications::waiting_for_digest(&mut conn, lucas).await.unwrap();
    assert_eq!(
        gathered.iter().map(|w| w.id).collect::<Vec<_>>(),
        [waiting],
        "only what goes by email"
    );
    notifications::digest_sent(&mut conn, lucas, candidate.local_date, &[waiting])
        .await
        .unwrap();
    assert!(
        notifications::digest_candidates(&mut conn)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn an_outbox_event_is_dispatched_once_and_queues_its_dispatch() {
    let db = TestDb::new().await;
    let (issue, admin) = ticket(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let event = outbox::record(
        &mut tx,
        &OutboxEvent {
            kind: "issue.updated",
            aggregate_type: "issue",
            aggregate_id: issue.id,
            payload: json!({"changes": []}),
            actor_id: Some(admin),
        },
    )
    .await
    .unwrap();
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM job WHERE kind = 'outbox.dispatch' AND payload ->> 'eventId' = $1",
    )
    .bind(event.to_string())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(queued, 1, "in the transaction of the event");
    let none = |_: &str, _: &str| None;
    // Nobody to notify (no configuration), but the ticket changed: one
    // real-time signal (L05-US08).
    assert_eq!(
        notifications::dispatch(&mut tx, event, &none).await.unwrap(),
        notifications::Dispatched {
            signals: 1,
            ..notifications::Dispatched::default()
        }
    );
    let published: i64 = sqlx::query_scalar("SELECT count(*) FROM job WHERE kind = 'events.publish'")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(published, 1);
    let processed: bool =
        sqlx::query_scalar("SELECT processed_at IS NOT NULL FROM outbox_event WHERE id = $1")
            .bind(event)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert!(processed);
    assert_eq!(
        notifications::dispatch(&mut tx, event, &none).await.unwrap(),
        notifications::Dispatched::default(),
        "processed: nothing again"
    );
}
