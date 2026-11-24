//! Integration tests on a real PostgreSQL: what the capacity of resources
//! depends on, loaded for many resources at once (L07-US06) — calendars of
//! every level, capacity periods, absences — and tenant isolation.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use domain::absences::AbsenceKind;
use domain::calendars::{ExceptionKind, Source};
use domain::capacity::WeekMinutes;
use domain::dates::Date as LocalDate;
use domain::holidays::HolidaySet;
use domain::resources::Kind;
use infra::absences::{self, AbsenceInput};
use infra::calendars::{self, CalendarInput, ExceptionInput, Owner};
use infra::capacity::{self, PatternInput};
use infra::day_capacity::{FALLBACK_WEEK, load};
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::resources::{self, ResourceInput};
use infra::uuid::Uuid;
use testkit::{SeedTenant, TestDb};
use time::Date;
use time::macros::date;

fn local(text: &str) -> LocalDate {
    LocalDate::parse(text).unwrap()
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

async fn site(db: &TestDb, tenant: &SeedTenant, name: &str, set: Option<HolidaySet>) -> Uuid {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let input = CalendarInput {
        name_translations: BTreeMap::from([("fr".to_owned(), name.to_owned())]),
        holiday_set: set,
    };
    let created = calendars::insert_site(&mut tx, new_id(), &input, None)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    created.id
}

async fn closed(db: &TestDb, tenant: &SeedTenant, owner: Owner, day: Date) {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    calendars::insert_exception(
        &mut tx,
        new_id(),
        owner,
        &ExceptionInput {
            day,
            kind: ExceptionKind::NonWorking,
            label: None,
        },
        None,
    )
    .await
    .unwrap()
    .unwrap();
    tx.commit().await.unwrap();
}

async fn period(db: &TestDb, tenant: &SeedTenant, resource_id: Uuid, from: Date, week: [u16; 7]) {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    capacity::insert(
        &mut tx,
        new_id(),
        resource_id,
        &PatternInput {
            effective_from: from,
            week: WeekMinutes(week),
            label: None,
        },
        None,
    )
    .await
    .unwrap()
    .unwrap();
    tx.commit().await.unwrap();
}

async fn leave(
    db: &TestDb,
    tenant: &SeedTenant,
    resource_id: Uuid,
    start: Date,
    end: Date,
    ends_at_noon: bool,
) {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    absences::insert(
        &mut tx,
        new_id(),
        resource_id,
        &AbsenceInput {
            kind: AbsenceKind::Leave,
            start,
            starts_at_noon: false,
            end,
            ends_at_noon,
            comment: None,
        },
        None,
    )
    .await
    .unwrap()
    .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn a_resource_calendar_gathers_its_own_its_site_and_the_tenant_exceptions() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let strasbourg = site(&db, a, "Strasbourg", Some(HolidaySet::FranceAlsaceMoselle)).await;
    let id = resource(&db, a, Some(strasbourg)).await;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let tenant = calendars::tenant_calendar(&mut tx).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    closed(&db, a, Owner::Calendar(tenant.id), date!(2026 - 12 - 24)).await;
    closed(&db, a, Owner::Calendar(strasbourg), date!(2026 - 12 - 31)).await;
    closed(&db, a, Owner::Resource(id), date!(2026 - 12 - 23)).await;
    closed(&db, a, Owner::Resource(id), date!(2027 - 01 - 15)).await;

    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let inputs = load(
        &mut tx,
        &[(id, Some(strasbourg))],
        date!(2026 - 12 - 01),
        date!(2026 - 12 - 31),
    )
    .await
    .unwrap();
    let levels = inputs.levels(id);
    assert_eq!(
        levels.iter().map(|(source, _)| *source).collect::<Vec<_>>(),
        [Source::Resource, Source::Site, Source::Tenant]
    );
    assert_eq!(levels[0].1.exceptions.len(), 1, "the January one is outside");
    assert_eq!(
        (levels[1].1.holidays, levels[1].1.exceptions.len()),
        (Some(HolidaySet::FranceAlsaceMoselle), 1)
    );
    assert_eq!(
        (levels[2].1.holidays, levels[2].1.exceptions.len()),
        (Some(HolidaySet::France), 1)
    );
    // 23 own closure, 25 and 26 Alsace holidays, 31 site closure. The site
    // has its own holiday set, so it decides the days it has no exception
    // for (§7, the closest level decides): the tenant's 24 is not reached.
    let calendar = inputs.calendar(id, &levels).unwrap();
    let minutes = calendar.minutes(local("2026-12-21"), local("2026-12-31"));
    assert_eq!(minutes, [420, 420, 0, 420, 0, 0, 0, 420, 420, 420, 0]);
}

#[tokio::test]
async fn periods_before_the_range_and_absences_across_it_are_loaded_for_each_resource() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let paris = site(&db, a, "Paris", None).await;
    let camille = resource(&db, a, Some(paris)).await;
    let generic = resource(&db, a, None).await;
    let untouched = resource(&db, a, None).await;
    // In force since January, a part-time period from 2 March 2027 on.
    period(
        &db,
        a,
        camille,
        date!(2026 - 01 - 01),
        [420, 420, 420, 420, 420, 0, 0],
    )
    .await;
    period(
        &db,
        a,
        camille,
        date!(2027 - 03 - 02),
        [420, 420, 0, 420, 420, 0, 0],
    )
    .await;
    period(&db, a, camille, date!(2027 - 06 - 01), [60; 7]).await;
    period(
        &db,
        a,
        generic,
        date!(2026 - 01 - 01),
        [240, 240, 240, 240, 240, 0, 0],
    )
    .await;
    // From the Friday before the range to Monday 1 March, morning only.
    leave(
        &db,
        a,
        camille,
        date!(2027 - 02 - 26),
        date!(2027 - 03 - 01),
        true,
    )
    .await;
    leave(
        &db,
        a,
        generic,
        date!(2027 - 04 - 01),
        date!(2027 - 04 - 02),
        false,
    )
    .await;

    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let selected = [(camille, Some(paris)), (generic, None), (untouched, None)];
    let inputs = load(&mut tx, &selected, date!(2027 - 03 - 01), date!(2027 - 03 - 07))
        .await
        .unwrap();
    assert_eq!(inputs.default, FALLBACK_WEEK);
    assert_eq!(inputs.resources[&camille].periods.len(), 2, "not the June one");
    assert_eq!(inputs.resources[&camille].absences.len(), 1);
    assert!(inputs.resources[&generic].absences.is_empty(), "April is outside");
    assert_eq!(inputs.resources[&untouched], Default::default());
    // Paris uses the tenant's holidays.
    assert_eq!(inputs.sites[&paris].holidays, None);

    let week = |id: Uuid| {
        let levels = inputs.levels(id);
        inputs
            .calendar(id, &levels)
            .unwrap()
            .minutes(local("2027-03-01"), local("2027-03-07"))
    };
    assert_eq!(week(camille), [210, 420, 0, 420, 420, 0, 0]);
    assert_eq!(week(generic), [240, 240, 240, 240, 240, 0, 0]);
    assert_eq!(
        week(untouched),
        [420, 420, 420, 420, 420, 0, 0],
        "the tenant default"
    );
    // A resource not asked for has nothing.
    assert!(inputs.levels(new_id()).is_empty());
    assert!(inputs.calendar(new_id(), &[]).is_none());
}

#[tokio::test]
async fn another_tenant_resources_and_calendars_are_never_loaded() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let b = &db.tenant_b;
    let paris = site(&db, a, "Paris", None).await;
    let camille = resource(&db, a, Some(paris)).await;
    period(&db, a, camille, date!(2026 - 01 - 01), [60; 7]).await;
    leave(
        &db,
        a,
        camille,
        date!(2027 - 03 - 01),
        date!(2027 - 03 - 01),
        false,
    )
    .await;
    closed(&db, a, Owner::Resource(camille), date!(2027 - 03 - 02)).await;
    closed(&db, a, Owner::Calendar(paris), date!(2027 - 03 - 03)).await;

    let mut tx = begin_tenant_tx(&db.app_pool, b.id).await.unwrap();
    let inputs = load(
        &mut tx,
        &[(camille, Some(paris))],
        date!(2027 - 03 - 01),
        date!(2027 - 03 - 07),
    )
    .await
    .unwrap();
    assert_eq!(
        inputs.resources[&camille],
        infra::day_capacity::ResourceInputs {
            calendar_id: Some(paris),
            ..Default::default()
        }
    );
    assert!(inputs.sites.is_empty(), "alpha's site is invisible");
    assert!(inputs.tenant.is_some(), "beta's own tenant calendar");
}
