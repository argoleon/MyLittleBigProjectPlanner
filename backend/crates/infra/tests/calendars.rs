//! Integration tests on a real PostgreSQL: calendars (L07-US04), the tenant
//! calendar every tenant gets, site calendars and the resources using them,
//! exceptions of calendars and resources, the outbox events telling which
//! days changed, and tenant isolation.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use domain::calendars::ExceptionKind;
use domain::holidays::HolidaySet;
use domain::resources::Kind;
use infra::calendars::{self, CalendarInput, CalendarKind, ExceptionInput, Owner, WriteConflict};
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::resources::{self, ResourceInput};
use infra::sqlx;
use infra::uuid::Uuid;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};
use time::macros::date;

fn named(fr: &str, set: Option<HolidaySet>) -> CalendarInput {
    CalendarInput {
        name_translations: BTreeMap::from([("fr".to_owned(), fr.to_owned())]),
        holiday_set: set,
    }
}

async fn resource(db: &TestDb, tenant: &SeedTenant, calendar: Option<Uuid>) -> Uuid {
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
            calendar_id: calendar,
        },
    )
    .await
    .unwrap()
    .unwrap();
    tx.commit().await.unwrap();
    created.id
}

async fn events(db: &TestDb, tenant: &SeedTenant, kind: &str) -> Vec<Value> {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    sqlx::query_scalar("SELECT payload FROM outbox_event WHERE kind = $1 ORDER BY created_at, id")
        .bind(kind)
        .fetch_all(&mut *tx)
        .await
        .unwrap()
}

#[tokio::test]
async fn every_tenant_has_its_calendar_with_french_holidays_never_deleted() {
    let db = TestDb::new().await;
    for tenant in [&db.tenant_a, &db.tenant_b] {
        let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
        let all = calendars::list(&mut tx).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(
            (all[0].kind, all[0].holiday_set),
            (CalendarKind::Tenant, Some(HolidaySet::France))
        );
        assert_eq!(all[0].name_translations["en"], "Organization");
    }
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let tenant = calendars::tenant_calendar(&mut tx).await.unwrap().unwrap();
    assert_eq!(
        calendars::delete_site(&mut tx, tenant.id, None, None)
            .await
            .unwrap(),
        Err(WriteConflict::TenantCalendar)
    );
    assert_eq!(
        calendars::update(&mut tx, tenant.id, None, &named("Organisation", None), None)
            .await
            .unwrap(),
        Err(WriteConflict::TenantCalendar),
        "the tenant calendar keeps a holiday set"
    );
    let changed = calendars::update(
        &mut tx,
        tenant.id,
        Some(&[tenant.version]),
        &named("Acme", Some(HolidaySet::FranceAlsaceMoselle)),
        None,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(changed.holiday_set, Some(HolidaySet::FranceAlsaceMoselle));
    // A second tenant calendar is refused by the database.
    let refused = sqlx::query(
        "INSERT INTO calendar (id, tenant_id, kind, name, holiday_set) VALUES ($1, app_current_tenant(), 'tenant', '{\"fr\": \"Bis\"}', 'fr')",
    )
    .bind(new_id())
    .execute(&mut *tx)
    .await;
    assert!(refused.is_err());
}

#[tokio::test]
async fn a_site_holds_its_resources_and_falls_back_to_the_tenant_once_deleted() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let paris = calendars::insert_site(&mut tx, new_id(), &named("Paris", None), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((paris.kind, paris.holiday_set), (CalendarKind::Site, None));
    assert_eq!(
        calendars::insert_site(&mut tx, new_id(), &named("PARIS", None), None)
            .await
            .unwrap(),
        Err(WriteConflict::NameTaken)
    );
    tx.rollback().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let paris = calendars::insert_site(&mut tx, paris.id, &named("Paris", None), None)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();

    let id = resource(&db, a, Some(paris.id)).await;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    assert_eq!(
        resources::find(&mut tx, id).await.unwrap().unwrap().calendar_id,
        Some(paris.id)
    );
    assert_eq!(
        calendars::find(&mut tx, paris.id)
            .await
            .unwrap()
            .unwrap()
            .resource_count,
        1
    );
    // The tenant calendar is no site.
    let tenant = calendars::tenant_calendar(&mut tx).await.unwrap().unwrap();
    let not_a_site = resources::insert(
        &mut tx,
        new_id(),
        &ResourceInput {
            kind: Kind::Generic,
            display_name: Some("X"),
            job_title: None,
            account_id: None,
            active: true,
            calendar_id: Some(tenant.id),
        },
    )
    .await
    .unwrap();
    assert_eq!(not_a_site, Err(resources::WriteConflict::NotASite));
    calendars::delete_site(&mut tx, paris.id, Some(&[paris.version]), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        resources::find(&mut tx, id).await.unwrap().unwrap().calendar_id,
        None
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn exceptions_of_calendars_and_resources_one_per_day_each_change_recorded() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let id = resource(&db, a, None).await;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let tenant = calendars::tenant_calendar(&mut tx).await.unwrap().unwrap();
    let bridge = ExceptionInput {
        day: date!(2026 - 05 - 15),
        kind: ExceptionKind::NonWorking,
        label: Some("Bridge day"),
    };
    let closure = calendars::insert_exception(&mut tx, new_id(), Owner::Calendar(tenant.id), &bridge, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (closure.kind, closure.label.as_deref()),
        (ExceptionKind::NonWorking, Some("Bridge day"))
    );
    let worked = calendars::insert_exception(
        &mut tx,
        new_id(),
        Owner::Resource(id),
        &ExceptionInput {
            day: date!(2026 - 05 - 25),
            kind: ExceptionKind::Working,
            label: None,
        },
        None,
    )
    .await
    .unwrap()
    .unwrap();
    // Changed, then deleted.
    let moved = calendars::update_exception(
        &mut tx,
        Owner::Resource(id),
        worked.id,
        Some(&[worked.version]),
        &ExceptionInput {
            day: date!(2026 - 05 - 26),
            kind: ExceptionKind::NonWorking,
            label: Some("Training"),
        },
        None,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(moved.version, worked.version + 1);
    // Another owner's exception is not found through this one.
    assert_eq!(
        calendars::delete_exception(&mut tx, Owner::Calendar(tenant.id), moved.id, None, None)
            .await
            .unwrap(),
        Err(WriteConflict::NotFound)
    );
    calendars::delete_exception(
        &mut tx,
        Owner::Resource(id),
        moved.id,
        Some(&[moved.version]),
        None,
    )
    .await
    .unwrap()
    .unwrap();
    let listed = calendars::exceptions(
        &mut tx,
        Owner::Calendar(tenant.id),
        date!(2026 - 01 - 01),
        date!(2026 - 12 - 31),
    )
    .await
    .unwrap();
    assert_eq!(listed.iter().map(|e| e.id).collect::<Vec<_>>(), [closure.id]);
    tx.commit().await.unwrap();

    // One exception per owner and day (it ends its transaction).
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    assert_eq!(
        calendars::insert_exception(&mut tx, new_id(), Owner::Calendar(tenant.id), &bridge, None)
            .await
            .unwrap(),
        Err(WriteConflict::DayTaken)
    );
    tx.rollback().await.unwrap();

    assert_eq!(
        events(&db, a, calendars::CALENDAR_CHANGED).await,
        [json!({"calendarId": tenant.id, "days": ["2026-05-15"]})]
    );
    assert_eq!(
        events(&db, a, infra::capacity::RESOURCE_CHANGED).await,
        [
            json!({"resourceId": id, "from": "2026-05-25", "until": "2026-05-26"}),
            json!({"resourceId": id, "from": "2026-05-25", "until": "2026-05-27"}),
            json!({"resourceId": id, "from": "2026-05-26", "until": "2026-05-27"}),
        ]
    );
}

#[tokio::test]
async fn a_tenant_sees_and_writes_only_its_own_calendars() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let paris = calendars::insert_site(&mut tx, new_id(), &named("Paris", None), None)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert_eq!(calendars::find(&mut tx, paris.id).await.unwrap(), None);
    assert_eq!(
        calendars::list(&mut tx).await.unwrap().len(),
        1,
        "beta's own tenant calendar"
    );
    assert_eq!(
        calendars::insert_exception(
            &mut tx,
            new_id(),
            Owner::Calendar(paris.id),
            &ExceptionInput {
                day: date!(2026 - 12 - 24),
                kind: ExceptionKind::NonWorking,
                label: None
            },
            None
        )
        .await
        .unwrap(),
        Err(WriteConflict::NotFound)
    );
    // A resource of beta cannot use alpha's site.
    let refused = resources::insert(
        &mut tx,
        new_id(),
        &ResourceInput {
            kind: Kind::Generic,
            display_name: Some("X"),
            job_title: None,
            account_id: None,
            active: true,
            calendar_id: Some(paris.id),
        },
    )
    .await
    .unwrap();
    assert_eq!(refused, Err(resources::WriteConflict::NotASite));
}
