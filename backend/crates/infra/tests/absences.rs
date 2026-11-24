//! Integration tests on a real PostgreSQL: absences (L07-US05), never
//! overlapping half day by half day, the outbox events telling which days
//! changed, their comments erased with the person, and tenant isolation.

#![allow(clippy::unwrap_used)]

use domain::absences::AbsenceKind;
use domain::resources::Kind;
use infra::absences::{self, AbsenceInput, WriteConflict};
use infra::accounts::TenantRole;
use infra::db::{begin_tenant_tx, set_account};
use infra::erasure::{self, AccountErasure};
use infra::id::new_id;
use infra::resources::{self, ResourceInput};
use infra::sqlx;
use infra::uuid::Uuid;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};
use time::Date;
use time::macros::date;

fn day_off(start: Date, starts_at_noon: bool, end: Date, ends_at_noon: bool) -> AbsenceInput<'static> {
    AbsenceInput {
        kind: AbsenceKind::Leave,
        start,
        starts_at_noon,
        end,
        ends_at_noon,
        comment: Some("Family"),
    }
}

async fn resource(db: &TestDb, tenant: &SeedTenant, account: Option<Uuid>) -> Uuid {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let created = resources::insert(
        &mut tx,
        new_id(),
        &ResourceInput {
            kind: Kind::Person,
            display_name: Some("Camille Martin"),
            job_title: None,
            account_id: account,
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

async fn events(db: &TestDb, tenant: &SeedTenant) -> Vec<Value> {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    sqlx::query_scalar("SELECT payload FROM outbox_event WHERE kind = $1 ORDER BY created_at, id")
        .bind(absences::ABSENCE_CHANGED)
        .fetch_all(&mut *tx)
        .await
        .unwrap()
}

#[tokio::test]
async fn absences_never_take_a_same_half_day_and_each_change_is_recorded() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let id = resource(&db, a, None).await;
    let declarer = db.add_member(a, TenantRole::ResourceManager).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let morning = absences::insert(
        &mut tx,
        new_id(),
        id,
        &day_off(date!(2026 - 12 - 07), false, date!(2026 - 12 - 07), true),
        Some(declarer),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        (morning.declared_by, morning.declared_by_name.as_deref()),
        (Some(declarer), Some("Test Member"))
    );
    // The afternoon of the same day fits.
    let afternoon = absences::insert(
        &mut tx,
        new_id(),
        id,
        &AbsenceInput {
            kind: AbsenceKind::Training,
            ..day_off(date!(2026 - 12 - 07), true, date!(2026 - 12 - 08), false)
        },
        None,
    )
    .await
    .unwrap()
    .unwrap();
    // A whole day over either does not.
    assert_eq!(
        absences::insert(
            &mut tx,
            new_id(),
            id,
            &day_off(date!(2026 - 12 - 08), false, date!(2026 - 12 - 09), false),
            None
        )
        .await
        .unwrap(),
        Err(WriteConflict::Overlap)
    );
    // Changed onto itself is no overlap; onto the morning one is.
    let moved = absences::update(
        &mut tx,
        id,
        afternoon.id,
        Some(&[afternoon.version]),
        &day_off(date!(2026 - 12 - 07), true, date!(2026 - 12 - 09), false),
        None,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(moved.version, afternoon.version + 1);
    assert_eq!(
        absences::update(
            &mut tx,
            id,
            moved.id,
            None,
            &day_off(date!(2026 - 12 - 07), false, date!(2026 - 12 - 09), false),
            None
        )
        .await
        .unwrap(),
        Err(WriteConflict::Overlap)
    );
    assert_eq!(
        absences::update(
            &mut tx,
            id,
            moved.id,
            Some(&[1]),
            &day_off(date!(2026 - 12 - 10), false, date!(2026 - 12 - 10), false),
            None
        )
        .await
        .unwrap(),
        Err(WriteConflict::VersionMismatch)
    );
    absences::delete(&mut tx, id, morning.id, None, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        absences::delete(&mut tx, id, morning.id, None, None)
            .await
            .unwrap(),
        Err(WriteConflict::NotFound)
    );
    let listed = absences::list(&mut tx, id, date!(2026 - 12 - 01), date!(2026 - 12 - 31))
        .await
        .unwrap();
    assert_eq!(listed.iter().map(|a| a.id).collect::<Vec<_>>(), [moved.id]);
    assert!(
        absences::list(&mut tx, id, date!(2026 - 12 - 10), date!(2026 - 12 - 31))
            .await
            .unwrap()
            .is_empty()
    );
    tx.commit().await.unwrap();

    let morning_id = morning.id.to_string();
    let afternoon_id = afternoon.id.to_string();
    assert_eq!(
        events(&db, a).await,
        [
            json!({"resourceId": id, "absenceId": morning_id, "from": "2026-12-07", "until": "2026-12-08"}),
            json!({"resourceId": id, "absenceId": afternoon_id, "from": "2026-12-07", "until": "2026-12-09"}),
            json!({"resourceId": id, "absenceId": afternoon_id, "from": "2026-12-07", "until": "2026-12-10"}),
            json!({"resourceId": id, "absenceId": morning_id, "from": "2026-12-07", "until": "2026-12-08"}),
        ]
    );
}

#[tokio::test]
async fn the_database_refuses_an_absence_ending_before_it_starts_or_from_noon_to_noon() {
    let db = TestDb::new().await;
    let id = resource(&db, &db.tenant_a, None).await;
    for (start, starts_at_noon, end, ends_at_noon) in [
        (date!(2026 - 12 - 08), false, date!(2026 - 12 - 07), false),
        (date!(2026 - 12 - 08), true, date!(2026 - 12 - 08), true),
    ] {
        let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
        let refused = absences::insert(
            &mut tx,
            new_id(),
            id,
            &day_off(start, starts_at_noon, end, ends_at_noon),
            None,
        )
        .await;
        assert!(refused.is_err(), "{start} {end}");
    }
}

#[tokio::test]
async fn an_erased_person_leaves_absences_without_their_comment() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let admin = db.add_member(a, TenantRole::TenantAdmin).await;
    let in_tenant = db.add_member(a, TenantRole::Member).await;
    let everywhere = db.add_member(a, TenantRole::Member).await;
    let first = resource(&db, a, Some(in_tenant.account_id)).await;
    let second = resource(&db, a, Some(everywhere.account_id)).await;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    for id in [first, second] {
        absences::insert(
            &mut tx,
            new_id(),
            id,
            &day_off(date!(2026 - 12 - 04), false, date!(2026 - 12 - 04), false),
            None,
        )
        .await
        .unwrap()
        .unwrap();
    }
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    set_account(&mut tx, admin.account_id).await.unwrap();
    erasure::erase_in_tenant(
        &mut tx,
        admin.account_id,
        in_tenant.account_id,
        &in_tenant.email,
        false,
    )
    .await
    .unwrap();
    infra::members::remove(&mut tx, in_tenant.account_id, None)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    set_account(&mut tx, everywhere.account_id).await.unwrap();
    assert!(matches!(
        erasure::erase_account(&mut tx, everywhere.account_id, everywhere.account_id, false)
            .await
            .unwrap(),
        AccountErasure::Erased(_)
    ));
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    for id in [first, second] {
        let kept = absences::list(&mut tx, id, date!(2026 - 12 - 01), date!(2026 - 12 - 31))
            .await
            .unwrap();
        assert_eq!(kept.len(), 1, "the dates stay for the load");
        assert_eq!(kept[0].comment, None, "the comment goes");
    }
}

#[tokio::test]
async fn a_tenant_sees_and_writes_only_its_own_absences() {
    let db = TestDb::new().await;
    let of_alpha = resource(&db, &db.tenant_a, None).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let absence = absences::insert(
        &mut tx,
        new_id(),
        of_alpha,
        &day_off(date!(2026 - 12 - 04), false, date!(2026 - 12 - 04), false),
        None,
    )
    .await
    .unwrap()
    .unwrap();
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert!(
        absences::list(&mut tx, of_alpha, date!(2026 - 12 - 01), date!(2026 - 12 - 31))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(absences::person_of(&mut tx, of_alpha).await.unwrap(), None);
    assert_eq!(
        absences::insert(
            &mut tx,
            new_id(),
            of_alpha,
            &day_off(date!(2026 - 12 - 05), false, date!(2026 - 12 - 05), false),
            None
        )
        .await
        .unwrap(),
        Err(WriteConflict::NotFound)
    );
    assert_eq!(
        absences::delete(&mut tx, of_alpha, absence.id, None, None)
            .await
            .unwrap(),
        Err(WriteConflict::NotFound)
    );
}
