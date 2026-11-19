//! Integration tests of list view exports' SQL (L06-US06) on a real
//! PostgreSQL: row-level security keeps each tenant's exports to itself,
//! a member reads only their own unexpired ones, a run marks an export once;
//! notifications are about a ticket or an export, never both nor neither;
//! the "export ready" preference is kept.

#![allow(clippy::unwrap_used)]

use domain::notify::{Choice, Notice, Preferences};
use infra::accounts::TenantRole;
use infra::db::{begin_tenant_tx, set_account};
use infra::id::new_id;
use infra::list_exports::{self, NewExport};
use infra::sqlx;
use infra::uuid::Uuid;
use testkit::TestDb;
use time::{Duration, OffsetDateTime};

async fn open(
    conn: &mut sqlx::PgConnection,
    account: Uuid,
    expires_in: Duration,
) -> list_exports::ListExport {
    let columns = vec!["key".to_owned(), "summary".to_owned()];
    list_exports::insert(
        conn,
        &NewExport {
            id: new_id(),
            account_id: account,
            file_name: "search-2026-11-18.csv",
            format: "csv",
            query: "project = PLAT",
            columns: &columns,
            language: "en",
            expires_at: OffsetDateTime::now_utc() + expires_in,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn exports_stay_in_their_tenant_and_with_their_member() {
    let db = TestDb::new().await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let lucas = db.add_member(a, TenantRole::Member).await.account_id;
    let hugo = db.add_member(a, TenantRole::Member).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let mine = open(&mut tx, lucas, Duration::hours(24)).await;
    let expired = open(&mut tx, lucas, Duration::seconds(-1)).await;
    assert_eq!((mine.status.as_str(), mine.columns.len()), ("queued", 2));
    assert_eq!(
        list_exports::find_of(&mut tx, lucas, mine.id)
            .await
            .unwrap()
            .map(|e| e.id),
        Some(mine.id)
    );
    assert_eq!(
        list_exports::find_of(&mut tx, hugo, mine.id).await.unwrap(),
        None,
        "another member's"
    );
    assert_eq!(
        list_exports::find_of(&mut tx, lucas, expired.id).await.unwrap(),
        None,
        "expired"
    );
    let listed: Vec<Uuid> = list_exports::list_of(&mut tx, lucas, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.id)
        .collect();
    assert_eq!(listed, [mine.id]);
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, b.id).await.unwrap();
    assert_eq!(list_exports::find(&mut tx, mine.id).await.unwrap(), None);
    assert!(!list_exports::complete(&mut tx, mine.id, "x", 1, 1).await.unwrap());
    assert_eq!(list_exports::delete(&mut tx, mine.id).await.unwrap(), None);
    tx.commit().await.unwrap();

    // Marked once: ready, then neither failed nor ready again.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let key = list_exports::object_key(a.id, mine.id, "csv");
    assert_eq!(key, format!("{}/exports/{}.csv", a.id, mine.id));
    assert!(
        list_exports::complete(&mut tx, mine.id, &key, 2, 120)
            .await
            .unwrap()
    );
    assert!(
        !list_exports::fail(&mut tx, mine.id, "query.invalid")
            .await
            .unwrap()
    );
    assert!(
        !list_exports::complete(&mut tx, mine.id, &key, 3, 130)
            .await
            .unwrap()
    );
    let ready = list_exports::find(&mut tx, mine.id).await.unwrap().unwrap();
    assert_eq!(
        (
            ready.status.as_str(),
            ready.row_count,
            ready.size_bytes,
            ready.completed_at.is_some()
        ),
        ("ready", Some(2), Some(120), true)
    );
    assert_eq!(
        list_exports::delete(&mut tx, mine.id).await.unwrap(),
        Some(Some(key))
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn a_ready_export_has_a_file_and_only_then() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let lucas = db.add_member(a, TenantRole::Member).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let export = open(&mut tx, lucas, Duration::hours(24)).await;
    let refused = sqlx::query("UPDATE list_export SET status = 'ready' WHERE id = $1")
        .bind(export.id)
        .execute(&mut *tx)
        .await;
    assert!(refused.is_err(), "ready without a file");
}

#[tokio::test]
async fn a_notification_is_about_a_ticket_or_an_export() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let lucas = db.add_member(a, TenantRole::Member).await.account_id;
    let insert = |event: &'static str, issue: Option<Uuid>| {
        let pool = db.app_pool.clone();
        let tenant = a.id;
        async move {
            let mut tx = begin_tenant_tx(&pool, tenant).await.unwrap();
            let outcome = sqlx::query(
                "INSERT INTO notification (id, tenant_id, account_id, event, issue_id, details, outbox_event_id, \
                 in_app, email_wanted) VALUES ($1, app_current_tenant(), $2, $3, $4, '{}', $5, true, false)",
            )
            .bind(new_id())
            .bind(lucas)
            .bind(event)
            .bind(issue)
            .bind(new_id())
            .execute(&mut *tx)
            .await;
            tx.rollback().await.unwrap();
            outcome.map(|_| ())
        }
    };
    assert!(insert("export_ready", None).await.is_ok());
    assert!(
        insert("export_ready", Some(new_id())).await.is_err(),
        "an export is no ticket"
    );
    assert!(
        insert("mentioned", None).await.is_err(),
        "a ticket event needs its ticket"
    );
    assert!(insert("archived", None).await.is_err(), "unknown event");
}

#[tokio::test]
async fn the_export_ready_preference_is_kept() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let lucas = db.add_member(a, TenantRole::Member).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    set_account(&mut tx, lucas).await.unwrap();
    let chosen = Preferences::with(
        [(
            Notice::ExportReady,
            Choice {
                in_app: false,
                email: true,
            },
        )],
        false,
    );
    infra::notifications::set_my_preferences(&mut tx, lucas, &chosen)
        .await
        .unwrap();
    let read = infra::notifications::my_preferences(&mut tx, lucas)
        .await
        .unwrap();
    assert_eq!(read, chosen);
    assert_eq!(
        read.choice(Notice::ExportReady),
        Choice {
            in_app: false,
            email: true
        }
    );
    tx.commit().await.unwrap();
}
