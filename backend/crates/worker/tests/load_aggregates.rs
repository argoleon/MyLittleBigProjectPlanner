//! Integration tests: the weekly and monthly load kept by the worker
//! (L08-US06), against PostgreSQL with row-level security. An assignment
//! written, then an absence declared: after the jobs run, the stored sums
//! of the week and the month follow, and nothing is left to recompute.

#![allow(clippy::unwrap_used)]

use domain::absences::AbsenceKind;
use domain::assignments::Mode;
use domain::hierarchy::NodeType;
use domain::resources::Kind;
use infra::absences::{self, AbsenceInput};
use infra::accounts::TenantRole;
use infra::assignments::{self, AssignmentInput, StoredSegment};
use infra::capacity::local_date;
use infra::db::begin_tenant_tx;
use infra::email::Mailer;
use infra::id::new_id;
use infra::issues::{self, NewIssue};
use infra::load_aggregates::{self, Sums};
use infra::nodes::{self, NewNode, NodeFields};
use infra::resources::{self, ResourceInput};
use infra::uuid::Uuid;
use serde_json::json;
use testkit::TestDb;
use time::{Date, Duration, OffsetDateTime};
use worker::Batch;
use worker::jobs::{JobRunner, Jobs};

async fn drain(db: &TestDb) {
    let smtp = infra::config::Secret::new(testkit::smtp_url());
    let (valkey, prefix) = testkit::valkey().await;
    let jobs = Jobs {
        mailer: Mailer::new(&smtp, "Planner <noreply@example.org>").unwrap(),
        exports: infra::exports::ExportStore::new(valkey, prefix),
        storage: None,
        clamav: None,
        public_url: None,
        events: None,
    };
    let runner = JobRunner::new(db.app_pool.clone());
    // The aggregates job waits a moment after the change: run it now.
    for _ in 0..3 {
        let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
        infra::sqlx::query("UPDATE job SET run_at = now() WHERE run_at > now()")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        while runner.next_batch(&jobs).await == Batch::Processed {}
    }
}

/// A Monday from four weeks on, of a week without a French public holiday.
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

/// The planned load of `project` in the hierarchy's aggregates (L08-US07).
async fn planned(db: &TestDb, project: Uuid) -> u64 {
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let found = infra::aggregates::find(&mut tx, &[project]).await.unwrap();
    tx.commit().await.unwrap();
    found.get(&project).map_or(0, |a| a.figures.planned_minutes)
}

async fn week(db: &TestDb, resource: Uuid, monday: Date) -> Option<Sums> {
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let found = load_aggregates::stored(&mut tx, &[resource], "week", &[monday])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    found.get(&(resource, monday)).copied()
}

#[tokio::test]
async fn the_worker_keeps_the_weekly_load_in_step_with_assignments_and_absences() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let admin = db.add_member(a, TenantRole::TenantAdmin).await.account_id;
    let monday = monday_ahead();
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let nathan = resources::insert(
        &mut tx,
        new_id(),
        &ResourceInput {
            kind: Kind::Generic,
            display_name: Some("Nathan Roux"),
            job_title: None,
            account_id: None,
            active: true,
            calendar_id: None,
        },
    )
    .await
    .unwrap()
    .unwrap()
    .id;
    let project = nodes::insert(
        &mut tx,
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
    let issue = issues::create(
        &mut tx,
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
    .id;
    let input = AssignmentInput {
        provisional: false,
        segments: vec![StoredSegment {
            start: monday,
            end: monday + Duration::days(4),
            mode: Mode::Percentage,
            value: 50,
        }],
    };
    assignments::insert(&mut tx, new_id(), issue, nathan, &input, None)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    drain(&db).await;
    assert_eq!(
        week(&db, nathan, monday).await,
        Some(Sums {
            load: 1050,
            provisional: 0,
            capacity: 2100,
            scheduled: 2100,
        })
    );
    // AC4: the project's aggregate follows within seconds (L08-US07).
    assert_eq!(planned(&db, project).await, 1050);

    // A day of leave: 50% of the days left, the capacity a day less.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
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
    tx.commit().await.unwrap();
    drain(&db).await;
    assert_eq!(
        week(&db, nathan, monday).await,
        Some(Sums {
            load: 840,
            provisional: 0,
            capacity: 1680,
            scheduled: 2100,
        })
    );
    assert_eq!(planned(&db, project).await, 840);
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let left: i64 = infra::sqlx::query_scalar("SELECT count(*) FROM load_aggregate_stale")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(left, 0, "nothing left to recompute");
}
