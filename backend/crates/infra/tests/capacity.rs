//! Integration tests on a real PostgreSQL: capacity periods (L07-US03),
//! one per resource and date, the outbox events telling which days changed,
//! the tenant's workload settings changed by its admins only, and tenant
//! isolation.

#![allow(clippy::unwrap_used)]

use domain::capacity::WeekMinutes;
use domain::resources::Kind;
use infra::accounts::TenantRole;
use infra::capacity::{self, PatternInput, WorkloadSettings, WriteConflict};
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::resources::{self, ResourceInput};
use infra::sqlx;
use infra::uuid::Uuid;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};
use time::Date;
use time::macros::date;

const FULL: WeekMinutes = WeekMinutes([420, 420, 420, 420, 420, 0, 0]);
const NO_WEDNESDAY: WeekMinutes = WeekMinutes([420, 420, 0, 420, 420, 0, 0]);

fn period(from: Date, week: WeekMinutes) -> PatternInput<'static> {
    PatternInput {
        effective_from: from,
        week,
        label: None,
    }
}

async fn resource(db: &TestDb, tenant: &SeedTenant) -> Uuid {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let created = resources::insert(
        &mut tx,
        new_id(),
        &ResourceInput {
            kind: Kind::Generic,
            display_name: Some("Rust developer"),
            job_title: None,
            account_id: None,
            active: true,
            calendar_id: None,
        },
    )
    .await
    .unwrap()
    .unwrap();
    tx.commit().await.unwrap();
    created.id
}

/// The payloads of the capacity events of `tenant`, oldest first.
async fn events(db: &TestDb, tenant: &SeedTenant, kind: &str) -> Vec<Value> {
    // The outbox is under row-level security, the owner's role included.
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    sqlx::query_scalar("SELECT payload FROM outbox_event WHERE kind = $1 ORDER BY created_at, id")
        .bind(kind)
        .fetch_all(&mut *tx)
        .await
        .unwrap()
}

#[tokio::test]
async fn periods_are_kept_by_date_one_per_day_each_change_telling_its_days() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let id = resource(&db, a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let january = capacity::insert(&mut tx, new_id(), id, &period(date!(2026 - 01 - 01), FULL), None)
        .await
        .unwrap()
        .unwrap();
    let march = capacity::insert(
        &mut tx,
        new_id(),
        id,
        &PatternInput {
            label: Some("Part time 80%"),
            ..period(date!(2026 - 03 - 01), NO_WEDNESDAY)
        },
        None,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        (march.label.as_deref(), march.week),
        (Some("Part time 80%"), NO_WEDNESDAY)
    );
    assert_eq!(
        capacity::insert(&mut tx, new_id(), id, &period(date!(2026 - 03 - 01), FULL), None)
            .await
            .unwrap(),
        Err(WriteConflict::DateTaken)
    );
    tx.rollback().await.unwrap();

    // Again, committed this time.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let january = capacity::insert(
        &mut tx,
        january.id,
        id,
        &period(date!(2026 - 01 - 01), FULL),
        None,
    )
    .await
    .unwrap()
    .unwrap();
    let march = capacity::insert(
        &mut tx,
        march.id,
        id,
        &period(date!(2026 - 03 - 01), NO_WEDNESDAY),
        None,
    )
    .await
    .unwrap()
    .unwrap();
    let listed = capacity::list(&mut tx, id).await.unwrap();
    assert_eq!(
        listed.iter().map(|p| p.id).collect::<Vec<_>>(),
        [january.id, march.id]
    );

    // Moved to June, then deleted; a stale version is refused.
    let june = capacity::update(
        &mut tx,
        id,
        march.id,
        Some(&[march.version]),
        &period(date!(2026 - 06 - 01), NO_WEDNESDAY),
        None,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(june.version, march.version + 1);
    assert_eq!(
        capacity::update(
            &mut tx,
            id,
            march.id,
            Some(&[march.version]),
            &period(date!(2026 - 06 - 01), FULL),
            None
        )
        .await
        .unwrap(),
        Err(WriteConflict::VersionMismatch)
    );
    capacity::delete(&mut tx, id, june.id, Some(&[june.version]), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        capacity::delete(&mut tx, id, june.id, None, None).await.unwrap(),
        Err(WriteConflict::NotFound)
    );
    assert_eq!(
        capacity::insert(
            &mut tx,
            new_id(),
            new_id(),
            &period(date!(2026 - 01 - 01), FULL),
            None
        )
        .await
        .unwrap(),
        Err(WriteConflict::NoResource)
    );
    tx.commit().await.unwrap();

    // Onto another period's date (a unique violation ends its transaction).
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    capacity::insert(&mut tx, new_id(), id, &period(date!(2026 - 09 - 01), FULL), None)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    assert_eq!(
        capacity::update(
            &mut tx,
            id,
            january.id,
            None,
            &period(date!(2026 - 09 - 01), FULL),
            None
        )
        .await
        .unwrap(),
        Err(WriteConflict::DateTaken)
    );
    tx.rollback().await.unwrap();

    assert_eq!(
        events(&db, a, capacity::RESOURCE_CHANGED).await,
        [
            json!({"resourceId": id, "from": "2026-01-01", "until": null}),
            json!({"resourceId": id, "from": "2026-03-01", "until": null}),
            // March → June: from March, the January period applies again.
            json!({"resourceId": id, "from": "2026-03-01", "until": null}),
            json!({"resourceId": id, "from": "2026-06-01", "until": null}),
            json!({"resourceId": id, "from": "2026-09-01", "until": null}),
        ]
    );
}

#[tokio::test]
async fn a_change_between_two_periods_ends_at_the_next_one() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let id = resource(&db, a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    for from in [date!(2026 - 01 - 01), date!(2026 - 06 - 01)] {
        capacity::insert(&mut tx, new_id(), id, &period(from, FULL), None)
            .await
            .unwrap()
            .unwrap();
    }
    capacity::insert(
        &mut tx,
        new_id(),
        id,
        &period(date!(2026 - 03 - 01), NO_WEDNESDAY),
        None,
    )
    .await
    .unwrap()
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        events(&db, a, capacity::RESOURCE_CHANGED).await.last(),
        Some(&json!({"resourceId": id, "from": "2026-03-01", "until": "2026-06-01"}))
    );
}

#[tokio::test]
async fn only_a_tenant_admin_changes_the_workload_settings() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let admin = db.add_member(a, TenantRole::TenantAdmin).await.account_id;
    let manager = db.add_member(a, TenantRole::ResourceManager).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let before = capacity::workload_settings(&mut tx).await.unwrap().unwrap();
    assert_eq!(
        (
            before.default_capacity,
            before.overload_tolerance_percent,
            before.overload_mode.as_str()
        ),
        (FULL, 0, "blocking")
    );
    let wanted = WorkloadSettings {
        default_capacity: WeekMinutes([480, 480, 480, 480, 240, 0, 0]),
        overload_tolerance_percent: 10,
        overload_mode: "warning".to_owned(),
        version: 0,
    };
    let after = capacity::update_workload_settings(&mut tx, admin, Some(&[before.version]), &wanted)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            after.default_capacity,
            after.overload_tolerance_percent,
            after.version
        ),
        (wanted.default_capacity, 10, before.version + 1)
    );
    assert_eq!(
        capacity::update_workload_settings(&mut tx, admin, Some(&[before.version]), &wanted)
            .await
            .unwrap(),
        None,
        "stale version"
    );
    // Same capacity: no event.
    capacity::update_workload_settings(
        &mut tx,
        admin,
        None,
        &WorkloadSettings {
            overload_tolerance_percent: 5,
            ..wanted.clone()
        },
    )
    .await
    .unwrap()
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        events(&db, a, capacity::TENANT_CHANGED).await,
        [json!({"defaultCapacityMinutes": [480, 480, 480, 480, 240, 0, 0]})]
    );

    // A resource manager, or a member of another tenant, is refused by the database.
    for actor in [
        manager,
        db.add_member(&db.tenant_b, TenantRole::TenantAdmin)
            .await
            .account_id,
    ] {
        let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
        let refused = capacity::update_workload_settings(&mut tx, actor, None, &wanted).await;
        assert!(refused.is_err(), "{actor}");
    }
}

#[tokio::test]
async fn a_tenant_sees_and_writes_only_its_own_periods() {
    let db = TestDb::new().await;
    let of_alpha = resource(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let pattern = capacity::insert(
        &mut tx,
        new_id(),
        of_alpha,
        &period(date!(2026 - 01 - 01), FULL),
        None,
    )
    .await
    .unwrap()
    .unwrap();
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert!(capacity::list(&mut tx, of_alpha).await.unwrap().is_empty());
    assert_eq!(
        capacity::insert(
            &mut tx,
            new_id(),
            of_alpha,
            &period(date!(2026 - 02 - 01), FULL),
            None
        )
        .await
        .unwrap(),
        Err(WriteConflict::NoResource)
    );
    assert_eq!(
        capacity::delete(&mut tx, of_alpha, pattern.id, None, None)
            .await
            .unwrap(),
        Err(WriteConflict::NotFound)
    );
    // The database refuses a period of another tenant's resource outright.
    let refused = sqlx::query(
        "INSERT INTO capacity_pattern (id, tenant_id, resource_id, effective_from, minutes)
         VALUES ($1, app_current_tenant(), $2, '2026-02-01', '{0,0,0,0,0,0,0}')",
    )
    .bind(new_id())
    .bind(of_alpha)
    .execute(&mut *tx)
    .await;
    assert!(refused.is_err());
}
