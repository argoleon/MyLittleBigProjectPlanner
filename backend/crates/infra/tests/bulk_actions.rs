//! Integration tests of bulk actions' SQL (L06-US08) on a real PostgreSQL:
//! row-level security keeps each tenant's actions and items to itself,
//! an item's outcome is recorded once, the action counts down and, run in
//! the background, tells its requester when done; a member reads their own.

#![allow(clippy::unwrap_used)]

use domain::bulk::Outcome;
use infra::accounts::TenantRole;
use infra::bulk_actions::{self, Action, Done, ModifyParams, NewBulk, Selected};
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::sqlx;
use infra::uuid::Uuid;
use testkit::TestDb;
use time::{Duration, OffsetDateTime};

fn selection(n: usize) -> Vec<Selected> {
    (0..n)
        .map(|i| Selected {
            id: new_id(),
            key: format!("PLAT-{}", i + 1),
        })
        .collect()
}

fn modify() -> Action {
    Action::Modify(ModifyParams {
        priority: Some("high".to_owned()),
        ..ModifyParams::default()
    })
}

fn done(outcome: Outcome) -> Done {
    Done {
        outcome,
        reason: None,
        errors: Vec::new(),
        new_key: None,
    }
}

async fn done_events(tx: &mut sqlx::PgConnection) -> Vec<serde_json::Value> {
    sqlx::query_scalar("SELECT payload FROM outbox_event WHERE kind = 'bulk.done' ORDER BY created_at")
        .fetch_all(tx)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_background_action_counts_down_and_tells_its_requester_once_done() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let lucas = db.add_member(a, TenantRole::Member).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let expires = OffsetDateTime::now_utc() + Duration::days(7);
    let (bulk, items) = bulk_actions::insert(
        &mut tx,
        &NewBulk {
            id: new_id(),
            account: lucas,
            action: &modify(),
            language: "en",
            background: true,
            selected: &selection(3),
            expires_at: expires,
        },
    )
    .await
    .unwrap();
    assert_eq!((bulk.total, bulk.remaining, items.len()), (3, 3, 3));
    assert_eq!(
        items
            .iter()
            .map(|i| (i.position, i.issue_key.as_str(), i.outcome))
            .collect::<Vec<_>>(),
        [(0, "PLAT-1", None), (1, "PLAT-2", None), (2, "PLAT-3", None)]
    );
    assert_eq!(bulk.action, modify(), "stored as asked");

    assert_eq!(
        bulk_actions::record(&mut tx, &bulk, items[0].id, &done(Outcome::Changed))
            .await
            .unwrap(),
        Some(2)
    );
    // Recorded once: a job run twice does nothing more.
    assert_eq!(
        bulk_actions::record(&mut tx, &bulk, items[0].id, &done(Outcome::Changed))
            .await
            .unwrap(),
        None
    );
    let refused = Done {
        outcome: Outcome::Refused,
        reason: Some(domain::bulk::reason::PERMISSION_DENIED),
        errors: vec![domain::validation::FieldError::new("fields.x", "required")],
        new_key: None,
    };
    assert_eq!(
        bulk_actions::record(&mut tx, &bulk, items[1].id, &refused)
            .await
            .unwrap(),
        Some(1)
    );
    assert!(done_events(&mut tx).await.is_empty(), "not done yet");
    assert_eq!(
        bulk_actions::record(&mut tx, &bulk, items[2].id, &done(Outcome::Unchanged))
            .await
            .unwrap(),
        Some(0)
    );
    assert_eq!(
        done_events(&mut tx).await,
        [
            serde_json::json!({"accountId": lucas, "kind": "modify", "total": 3, "changed": 1, "unchanged": 1, "refused": 1})
        ]
    );
    let read = bulk_actions::find_of(&mut tx, lucas, bulk.id)
        .await
        .unwrap()
        .unwrap();
    assert!(read.completed_at.is_some());
    let items = bulk_actions::items_of(&mut tx, bulk.id).await.unwrap();
    assert_eq!(items[1].reason.as_deref(), Some("permission.denied"));
    assert_eq!(items[1].errors, [("fields.x".to_owned(), "required".to_owned())]);
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn an_action_run_within_the_request_tells_nobody() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let lucas = db.add_member(a, TenantRole::Member).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let expires = OffsetDateTime::now_utc() + Duration::days(7);
    let (bulk, items) = bulk_actions::insert(
        &mut tx,
        &NewBulk {
            id: new_id(),
            account: lucas,
            action: &modify(),
            language: "en",
            background: false,
            selected: &selection(1),
            expires_at: expires,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        bulk_actions::record(&mut tx, &bulk, items[0].id, &done(Outcome::Changed))
            .await
            .unwrap(),
        Some(0)
    );
    assert!(done_events(&mut tx).await.is_empty());
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn bulk_actions_stay_in_their_tenant_and_with_their_member() {
    let db = TestDb::new().await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let lucas = db.add_member(a, TenantRole::Member).await.account_id;
    let hugo = db.add_member(a, TenantRole::Member).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let expires = OffsetDateTime::now_utc() + Duration::days(7);
    let (bulk, _) = bulk_actions::insert(
        &mut tx,
        &NewBulk {
            id: new_id(),
            account: lucas,
            action: &modify(),
            language: "en",
            background: false,
            selected: &selection(2),
            expires_at: expires,
        },
    )
    .await
    .unwrap();
    let (old, _) = bulk_actions::insert(
        &mut tx,
        &NewBulk {
            id: new_id(),
            account: lucas,
            action: &modify(),
            language: "en",
            background: false,
            selected: &selection(1),
            expires_at: OffsetDateTime::now_utc() - Duration::seconds(1),
        },
    )
    .await
    .unwrap();
    assert!(
        bulk_actions::find_of(&mut tx, lucas, bulk.id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        bulk_actions::find_of(&mut tx, hugo, bulk.id)
            .await
            .unwrap()
            .is_none(),
        "another member's"
    );
    assert!(
        bulk_actions::find_of(&mut tx, lucas, old.id)
            .await
            .unwrap()
            .is_none(),
        "expired"
    );
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, b.id).await.unwrap();
    assert!(bulk_actions::find(&mut tx, bulk.id).await.unwrap().is_none());
    assert!(bulk_actions::items_of(&mut tx, bulk.id).await.unwrap().is_empty());
    assert!(!bulk_actions::delete(&mut tx, bulk.id).await.unwrap());
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    assert!(bulk_actions::delete(&mut tx, bulk.id).await.unwrap());
    let items: i64 = sqlx::query_scalar("SELECT count(*) FROM bulk_action_item WHERE bulk_id = $1")
        .bind(bulk.id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(items, 0, "its report goes with it");
    tx.commit().await.unwrap();
    let _: Uuid = old.id;
}
