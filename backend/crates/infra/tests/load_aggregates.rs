//! Integration tests on a real PostgreSQL: the weekly and monthly load of
//! resources (L08-US06) — marked stale by load and capacity changes within
//! the horizon, one job queued for a burst, recomputed equal to the sums
//! computed on the fly, read back by period; per tenant.

#![allow(clippy::unwrap_used)]

use domain::absences::AbsenceKind;
use domain::assignments::Mode;
use domain::hierarchy::NodeType;
use domain::resources::Kind;
use domain::workload::{Scale, periods};
use infra::absences::{self, AbsenceInput};
use infra::accounts::TenantRole;
use infra::assignments::{self, AssignmentInput, StoredSegment};
use infra::capacity::{local_date, sql_date};
use infra::day_load::{self, Recompute, Scope};
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::issues::{self, NewIssue};
use infra::load_aggregates::{self, AGGREGATES_JOB, Sums};
use infra::nodes::{self, NewNode, NodeFields};
use infra::resources::{self, ResourceInput};
use infra::sqlx::PgConnection;
use infra::uuid::Uuid;
use serde_json::json;
use testkit::TestDb;
use time::{Date, Duration, OffsetDateTime};

/// A Monday from four weeks on, within the horizon, of a week without a
/// French public holiday: whenever the test runs, a plain week.
fn monday_ahead() -> Date {
    let day = OffsetDateTime::now_utc().date() + Duration::weeks(4);
    let mut monday = day - Duration::days(i64::from(day.weekday().number_days_from_monday()));
    let holiday = |date: Date| {
        local_date(date).is_some_and(|d| domain::holidays::HolidaySet::France.holiday_on(d).is_some())
    };
    while (0..5).any(|i| holiday(monday + Duration::days(i))) {
        monday += Duration::weeks(1);
    }
    monday
}

async fn resource(conn: &mut PgConnection, name: &str) -> Uuid {
    resources::insert(
        conn,
        new_id(),
        &ResourceInput {
            kind: Kind::Generic,
            display_name: Some(name),
            job_title: None,
            account_id: None,
            active: true,
            calendar_id: None,
        },
    )
    .await
    .unwrap()
    .unwrap()
    .id
}

async fn ticket(conn: &mut PgConnection, admin: Uuid) -> Uuid {
    let project = nodes::insert(
        conn,
        &NewNode {
            id: new_id(),
            node_type: NodeType::Project,
            key: "PLAT",
            fields: NodeFields {
                key: None,
                name: "Platform",
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
    .unwrap()
    .id;
    let fields = json!({});
    issues::create(
        conn,
        &NewIssue {
            id: new_id(),
            project_id: project,
            project_key: "PLAT",
            type_id: Uuid::from_u128(1),
            status_id: Uuid::from_u128(2),
            parent_id: None,
            summary: "Account migration scripts",
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
    .unwrap()
    .id
}

async fn marks(conn: &mut PgConnection) -> Vec<(Uuid, String, Date)> {
    infra::sqlx::query_as(
        "SELECT resource_id, period_kind, period_start FROM load_aggregate_stale ORDER BY 1, 2, 3",
    )
    .fetch_all(conn)
    .await
    .unwrap()
}

async fn queued(conn: &mut PgConnection) -> i64 {
    infra::sqlx::query_scalar("SELECT count(*) FROM job WHERE kind = $1")
        .bind(AGGREGATES_JOB)
        .fetch_one(conn)
        .await
        .unwrap()
}

#[tokio::test]
async fn load_and_capacity_changes_mark_their_weeks_and_months_and_the_job_stores_them() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let admin = db.add_member(a, TenantRole::TenantAdmin).await.account_id;
    let monday = monday_ahead();
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let nathan = resource(&mut tx, "Nathan Roux").await;
    let idle = resource(&mut tx, "Rust developer").await;
    let issue = ticket(&mut tx, admin).await;

    // 4 h a day for a week, provisional: its week and month are marked.
    let input = AssignmentInput {
        provisional: true,
        segments: vec![StoredSegment {
            start: monday,
            end: monday + Duration::days(4),
            mode: Mode::HoursPerDay,
            value: 240,
        }],
    };
    assignments::insert(&mut tx, new_id(), issue, nathan, &input, None)
        .await
        .unwrap()
        .unwrap();
    let marked = marks(&mut tx).await;
    assert!(
        marked.contains(&(nathan, "week".to_owned(), monday)),
        "{marked:?}"
    );
    assert!(marked.iter().any(|(r, kind, _)| *r == nathan && kind == "month"));
    assert!(marked.iter().all(|(r, _, _)| *r == nathan));
    // A day of leave: the capacity of everyone of the scope changes.
    absences::insert(
        &mut tx,
        new_id(),
        nathan,
        &AbsenceInput {
            kind: AbsenceKind::Leave,
            start: monday,
            starts_at_noon: false,
            end: monday,
            ends_at_noon: false,
            comment: None,
        },
        None,
    )
    .await
    .unwrap()
    .unwrap();
    day_load::run(
        &mut tx,
        &Recompute {
            scope: Scope::All,
            from: Some(monday.to_string()),
            until: Some((monday + Duration::days(1)).to_string()),
        },
    )
    .await
    .unwrap();
    assert!(
        marks(&mut tx).await.contains(&(idle, "week".to_owned(), monday)),
        "a resource without load too"
    );
    assert_eq!(queued(&mut tx).await, 1, "one job for the burst");
    // An unbounded change stays within the horizon.
    load_aggregates::mark_stale(&mut tx, &[idle], None, None)
        .await
        .unwrap();
    let weeks = marks(&mut tx)
        .await
        .iter()
        .filter(|(r, kind, _)| *r == idle && kind == "week")
        .count();
    assert!((156..=163).contains(&weeks), "37 months of weeks: {weeks}");

    // The job stores what the sums computed on the fly give.
    let stored = load_aggregates::recompute(&mut tx).await.unwrap();
    assert!(
        stored > 190,
        "the horizon of one resource, and the other's week and month: {stored}"
    );
    assert!(marks(&mut tx).await.is_empty());
    let week = load_aggregates::stored(&mut tx, &[nathan, idle], "week", &[monday])
        .await
        .unwrap();
    // The Monday off: its 4 h stay planned (a conflict), 4 days of 7 h
    // left out of 5.
    assert_eq!(
        week[&(nathan, monday)],
        Sums {
            load: 1200,
            provisional: 1200,
            capacity: 1680,
            scheduled: 2100,
        }
    );
    assert_eq!(week[&(idle, monday)].capacity, 2100);
    let local = local_date(monday).unwrap();
    let computed = load_aggregates::compute(&mut tx, &[(nathan, None)], &periods(Scale::Week, local, local))
        .await
        .unwrap();
    assert_eq!(computed[&nathan][0], week[&(nathan, monday)]);
    // Days, never stored, come from the same computation.
    let days = load_aggregates::compute(
        &mut tx,
        &[(nathan, None)],
        &periods(Scale::Day, local, local.add_days(1)),
    )
    .await
    .unwrap();
    assert_eq!(days[&nathan][1].load, 240);
    assert_eq!((days[&nathan][0].capacity, days[&nathan][0].scheduled), (0, 420));
    assert!(sql_date(local).is_some());
    tx.commit().await.unwrap();

    // Another tenant reads none.
    let mut other = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert!(
        load_aggregates::stored(&mut other, &[nathan], "week", &[monday])
            .await
            .unwrap()
            .is_empty()
    );
    other.commit().await.unwrap();
}
