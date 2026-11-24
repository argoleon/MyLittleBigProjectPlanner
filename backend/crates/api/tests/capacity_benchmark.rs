//! Read performance of the bulk capacity (L07-US06, specification §19: API
//! reads under 200 ms at the 95th percentile): 200 resources over 3 months
//! on the reference dataset below. Alone in its test binary, so that cargo
//! runs it after the others, not alongside them. The build is unoptimized:
//! a pass here is a conservative one.

#![allow(clippy::unwrap_used)]

mod common;

use std::time::{Duration, Instant};

use common::{json, multi};
use domain::absences::AbsenceKind;
use domain::calendars::ExceptionKind;
use domain::capacity::WeekMinutes;
use domain::holidays::HolidaySet;
use domain::resources::Kind;
use infra::absences::{self, AbsenceInput};
use infra::calendars::{self, CalendarInput, ExceptionInput, Owner};
use infra::capacity::{self, PatternInput};
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::resources::{self, ResourceInput};
use reqwest::Method;
use testkit::TestDb;
use time::macros::date;
use time::{Date, Duration as Days};

const RESOURCES: usize = 200;
const RUNS: usize = 20;
const TARGET: Duration = Duration::from_millis(200);

/// The reference dataset: 3 sites (one in Alsace) with closures, 200
/// resources spread over them, each with 2 capacity periods (one switching
/// to part time in the range), a closure of its own and 3 absences in the
/// range, one of them ending at noon.
async fn seed(db: &TestDb) {
    let a = &db.tenant_a;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let mut sites = Vec::new();
    for (name, holidays) in [
        ("Paris", None),
        ("Lyon", Some(HolidaySet::France)),
        ("Strasbourg", Some(HolidaySet::FranceAlsaceMoselle)),
    ] {
        let input = CalendarInput {
            name_translations: [("fr".to_owned(), name.to_owned())].into(),
            holiday_set: holidays,
        };
        let site = calendars::insert_site(&mut tx, new_id(), &input, None)
            .await
            .unwrap()
            .unwrap();
        for day in [date!(2027 - 01 - 04), date!(2027 - 02 - 15)] {
            exception(&mut tx, Owner::Calendar(site.id), day).await;
        }
        sites.push(site.id);
    }
    for index in 0..RESOURCES {
        let created = resources::insert(
            &mut tx,
            new_id(),
            &ResourceInput {
                kind: Kind::Generic,
                display_name: Some(&format!("Resource {index:03}")),
                job_title: None,
                account_id: None,
                active: true,
                calendar_id: (index % 4 != 3).then(|| sites[index % 3]),
            },
        )
        .await
        .unwrap()
        .unwrap();
        let id = created.id;
        for (from, week) in [
            (date!(2026 - 01 - 01), [420, 420, 420, 420, 420, 0, 0]),
            (date!(2027 - 01 - 18), [420, 420, 0, 420, 210, 0, 0]),
        ] {
            let input = PatternInput {
                effective_from: from,
                week: WeekMinutes(week),
                label: None,
            };
            capacity::insert(&mut tx, new_id(), id, &input, None)
                .await
                .unwrap()
                .unwrap();
        }
        let offset = i64::try_from(index % 20).unwrap();
        exception(
            &mut tx,
            Owner::Resource(id),
            date!(2027 - 01 - 06) + Days::days(offset),
        )
        .await;
        for (start, length, ends_at_noon) in [
            (date!(2026 - 12 - 07), 4, false),
            (date!(2027 - 01 - 25), 2, true),
            (date!(2027 - 02 - 08), 0, false),
        ] {
            let start = start + Days::days(offset);
            let input = AbsenceInput {
                kind: AbsenceKind::Leave,
                start,
                starts_at_noon: false,
                end: start + Days::days(length),
                ends_at_noon,
                comment: None,
            };
            absences::insert(&mut tx, new_id(), id, &input, None)
                .await
                .unwrap()
                .unwrap();
        }
    }
    tx.commit().await.unwrap();
}

async fn exception(tx: &mut infra::sqlx::PgConnection, owner: Owner, day: Date) {
    let input = ExceptionInput {
        day,
        kind: ExceptionKind::NonWorking,
        label: None,
    };
    calendars::insert_exception(tx, new_id(), owner, &input, None)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn capacity_of_200_resources_over_3_months_is_read_within_200_ms_at_p95() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    seed(&db).await;
    let path = "/api/v1/capacity?from=2026-12-01&to=2027-02-28";

    // Warm up the pools and the plans.
    for _ in 0..3 {
        let (status, page) = json(api.request(Method::GET, &db.tenant_a, path).send().await.unwrap()).await;
        assert_eq!(status, 200, "{page}");
        assert_eq!(page["items"].as_array().unwrap().len(), RESOURCES);
        assert_eq!(page["items"][0]["minutes"].as_array().unwrap().len(), 90);
    }
    let mut timings = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let started = Instant::now();
        let response = api.request(Method::GET, &db.tenant_a, path).send().await.unwrap();
        assert_eq!(response.status(), 200);
        response.bytes().await.unwrap();
        timings.push(started.elapsed());
    }
    timings.sort();
    // The 19th of 20: 95% of the calls are at most this long.
    let p95 = timings[RUNS * 95 / 100 - 1];
    println!(
        "capacity of {RESOURCES} resources × 90 days: median {:?}, p95 {p95:?}",
        timings[RUNS / 2]
    );
    assert!(p95 < TARGET, "p95 {p95:?} ≥ {TARGET:?}: {timings:?}");
}
