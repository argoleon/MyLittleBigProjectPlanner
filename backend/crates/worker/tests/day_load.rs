//! Integration tests: the daily load kept up to date by the worker
//! (L08-US02), against PostgreSQL with row-level security. Each change —
//! a capacity period, an absence, a site's or the tenant's calendar, the
//! default capacity — queues its recomputation with its event; the worker
//! recomputes the load of the resources it touches, and two workers run
//! each job once.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;

use domain::absences::AbsenceKind;
use domain::assignments::Mode;
use domain::calendars::ExceptionKind;
use domain::capacity::WeekMinutes;
use domain::hierarchy::NodeType;
use domain::resources::Kind;
use infra::absences::{self, AbsenceInput};
use infra::accounts::TenantRole;
use infra::assignments::{self, AssignmentInput, StoredSegment};
use infra::calendars::{self, CalendarInput, ExceptionInput, Owner};
use infra::capacity::{self, PatternInput};
use infra::db::begin_tenant_tx;
use infra::email::Mailer;
use infra::id::new_id;
use infra::issues::{self, NewIssue};
use infra::nodes::{self, NewNode, NodeFields};
use infra::resources::{self, ResourceInput};
use infra::sqlx::PgConnection;
use infra::uuid::Uuid;
use serde_json::json;
use testkit::{SeedTenant, TestDb};
use time::Date;
use time::macros::date;
use worker::Batch;
use worker::jobs::{JobRunner, Jobs};

async fn jobs() -> Jobs {
    let smtp = infra::config::Secret::new(testkit::smtp_url());
    let (valkey, prefix) = testkit::valkey().await;
    Jobs {
        mailer: Mailer::new(&smtp, "Planner <noreply@example.org>").unwrap(),
        exports: infra::exports::ExportStore::new(valkey, prefix),
        storage: None,
        clamav: None,
        public_url: None,
        events: None,
    }
}

/// Runs every due job.
async fn drain(db: &TestDb) {
    let runner = JobRunner::new(db.app_pool.clone());
    let jobs = jobs().await;
    while runner.next_batch(&jobs).await == Batch::Processed {}
}

struct Plan {
    admin: Uuid,
    site: Uuid,
    /// Two resources on the site, one without a site.
    camille: Uuid,
    nathan: Uuid,
    dev: Uuid,
}

async fn resource(conn: &mut PgConnection, name: &str, site: Option<Uuid>) -> Uuid {
    resources::insert(
        conn,
        new_id(),
        &ResourceInput {
            kind: Kind::Generic,
            display_name: Some(name),
            job_title: None,
            account_id: None,
            active: true,
            calendar_id: site,
        },
    )
    .await
    .unwrap()
    .unwrap()
    .id
}

/// A site using the tenant's holidays, three resources, and a ticket each
/// is assigned to at 50% from Monday 14 to Friday 18 December 2026.
async fn plan(db: &TestDb, tenant: &SeedTenant) -> Plan {
    let admin = db.add_member(tenant, TenantRole::TenantAdmin).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let site = calendars::insert_site(
        &mut tx,
        new_id(),
        &CalendarInput {
            name_translations: BTreeMap::from([("fr".to_owned(), "Paris".to_owned())]),
            holiday_set: None,
        },
        None,
    )
    .await
    .unwrap()
    .unwrap()
    .id;
    let camille = resource(&mut tx, "Camille", Some(site)).await;
    let nathan = resource(&mut tx, "Nathan", Some(site)).await;
    let dev = resource(&mut tx, "Rust developer", None).await;
    let node = nodes::insert(
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
    .unwrap();
    for resource in [camille, nathan, dev] {
        let input = AssignmentInput {
            provisional: false,
            segments: vec![StoredSegment {
                start: date!(2026 - 12 - 14),
                end: date!(2026 - 12 - 18),
                mode: Mode::Percentage,
                value: 50,
            }],
        };
        assignments::insert(&mut tx, new_id(), issue.id, resource, &input, None)
            .await
            .unwrap()
            .unwrap();
    }
    tx.commit().await.unwrap();
    drain(db).await;
    Plan {
        admin,
        site,
        camille,
        nathan,
        dev,
    }
}

/// The load of `resource` from Monday 14 to Friday 18 December, a day each
/// (0 when it has no row).
async fn week(db: &TestDb, tenant: &SeedTenant, resource: Uuid) -> [u32; 5] {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let rows = infra::day_load::of_resource(&mut tx, resource, date!(2026 - 12 - 14), date!(2026 - 12 - 18))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let by_day = infra::day_load::by_day(&rows);
    let mut days = [0; 5];
    for (i, day) in days.iter_mut().enumerate() {
        let date: Date = date!(2026 - 12 - 14) + time::Duration::days(i64::try_from(i).unwrap());
        *day = by_day.get(&date).copied().unwrap_or(0);
    }
    days
}

#[tokio::test]
async fn capacity_absence_and_calendar_changes_reach_the_daily_load() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let plan = plan(&db, a).await;
    assert_eq!(week(&db, a, plan.camille).await, [210; 5]);

    // AC1: Camille's Friday goes from 7 h to 3.5 h.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let input = PatternInput {
        effective_from: date!(2026 - 01 - 01),
        week: WeekMinutes([420, 420, 420, 420, 210, 0, 0]),
        label: None,
    };
    capacity::insert(&mut tx, new_id(), plan.camille, &input, Some(plan.admin))
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    drain(&db).await;
    assert_eq!(week(&db, a, plan.camille).await, [210, 210, 210, 210, 105]);
    assert_eq!(
        week(&db, a, plan.nathan).await,
        [210; 5],
        "Nathan's capacity did not change"
    );

    // AC2: an absence on Wednesday takes the percentage to 0.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let leave = AbsenceInput {
        kind: AbsenceKind::Leave,
        start: date!(2026 - 12 - 16),
        starts_at_noon: false,
        end: date!(2026 - 12 - 16),
        ends_at_noon: false,
        comment: None,
    };
    absences::insert(&mut tx, new_id(), plan.camille, &leave, Some(plan.admin))
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    drain(&db).await;
    assert_eq!(week(&db, a, plan.camille).await, [210, 210, 0, 210, 105]);

    // AC3: a holiday added to the site: both its resources, not the third.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let holiday = ExceptionInput {
        day: date!(2026 - 12 - 17),
        kind: ExceptionKind::Holiday,
        label: Some("Site day"),
    };
    calendars::insert_exception(
        &mut tx,
        new_id(),
        Owner::Calendar(plan.site),
        &holiday,
        Some(plan.admin),
    )
    .await
    .unwrap()
    .unwrap();
    tx.commit().await.unwrap();
    drain(&db).await;
    assert_eq!(week(&db, a, plan.camille).await, [210, 210, 0, 0, 105]);
    assert_eq!(week(&db, a, plan.nathan).await, [210, 210, 210, 0, 210]);
    assert_eq!(week(&db, a, plan.dev).await, [210; 5]);

    // A closure of the tenant calendar: every resource (the site uses its holidays).
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let tenant = calendars::tenant_calendar(&mut tx).await.unwrap().unwrap();
    let closure = ExceptionInput {
        day: date!(2026 - 12 - 15),
        kind: ExceptionKind::NonWorking,
        label: None,
    };
    calendars::insert_exception(
        &mut tx,
        new_id(),
        Owner::Calendar(tenant.id),
        &closure,
        Some(plan.admin),
    )
    .await
    .unwrap()
    .unwrap();
    tx.commit().await.unwrap();
    drain(&db).await;
    assert_eq!(week(&db, a, plan.nathan).await, [210, 0, 210, 0, 210]);
    assert_eq!(week(&db, a, plan.dev).await, [210, 0, 210, 210, 210]);

    // The default capacity: the resources without a period follow it.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let mut settings = capacity::workload_settings(&mut tx).await.unwrap().unwrap();
    settings.default_capacity = WeekMinutes([240, 240, 240, 240, 240, 0, 0]);
    capacity::update_workload_settings(&mut tx, plan.admin, None, &settings)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    drain(&db).await;
    assert_eq!(week(&db, a, plan.dev).await, [120, 0, 120, 120, 120]);
    assert_eq!(
        week(&db, a, plan.camille).await,
        [210, 0, 0, 0, 105],
        "Camille has a period of her own"
    );

    // Nothing left to run; beta saw nothing of it.
    let pending: i64 =
        infra::sqlx::query_scalar("SELECT count(*) FROM job WHERE kind = 'workload.recompute'")
            .fetch_one(&db.owner_pool)
            .await
            .unwrap();
    assert_eq!(pending, 0);
    assert_eq!(week(&db, &db.tenant_b, plan.dev).await, [0; 5]);
}

#[tokio::test]
async fn two_workers_recompute_each_change_once_and_end_with_the_same_load() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let plan = plan(&db, a).await;
    // Ten changes on the same resources, queued at once.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    for (i, day) in [14, 15, 16, 17, 18].into_iter().enumerate() {
        let day = Date::from_calendar_date(2026, time::Month::December, day).unwrap();
        let input = ExceptionInput {
            day,
            kind: if i % 2 == 0 {
                ExceptionKind::NonWorking
            } else {
                ExceptionKind::Working
            },
            label: None,
        };
        calendars::insert_exception(
            &mut tx,
            new_id(),
            Owner::Resource(plan.camille),
            &input,
            Some(plan.admin),
        )
        .await
        .unwrap()
        .unwrap();
        calendars::insert_exception(
            &mut tx,
            new_id(),
            Owner::Resource(plan.nathan),
            &input,
            Some(plan.admin),
        )
        .await
        .unwrap()
        .unwrap();
    }
    tx.commit().await.unwrap();
    let queued: i64 = infra::sqlx::query_scalar("SELECT count(*) FROM job WHERE kind = 'workload.recompute'")
        .fetch_one(&db.owner_pool)
        .await
        .unwrap();
    assert_eq!(queued, 10);

    let jobs = jobs().await;
    let first = JobRunner::new(db.app_pool.clone());
    let second = JobRunner::new(db.app_pool.clone());
    let run = |runner: &'static JobRunner| {
        let jobs = &jobs;
        async move { while runner.next_batch(jobs).await == Batch::Processed {} }
    };
    let first: &'static JobRunner = Box::leak(Box::new(first));
    let second: &'static JobRunner = Box::leak(Box::new(second));
    tokio::join!(run(first), run(second));

    let left: Vec<(String, String, i32, Option<String>)> =
        infra::sqlx::query_as(
            "SELECT kind, status, attempts, last_error FROM job WHERE kind = 'workload.recompute' OR attempts > 0",
        )
            .fetch_all(&db.owner_pool)
            .await
            .unwrap();
    assert_eq!(left, [], "every job ran, none failed");
    // Closed on Monday, Wednesday and Friday.
    for resource in [plan.camille, plan.nathan] {
        assert_eq!(week(&db, a, resource).await, [0, 210, 0, 210, 0]);
    }
}

#[tokio::test]
async fn assignments_made_before_the_daily_load_get_theirs_from_one_job_per_tenant() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let plan = plan(&db, a).await;
    // As before the migration: assignments without load rows.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    infra::sqlx::query("DELETE FROM resource_day_load")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(week(&db, a, plan.dev).await, [0; 5]);

    // What the migration runs: alpha has assignments, beta none.
    let queued: i32 = infra::sqlx::query_scalar("SELECT app_queue_load_recompute_all()")
        .fetch_one(&db.owner_pool)
        .await
        .unwrap();
    assert_eq!(queued, 1);
    drain(&db).await;
    for resource in [plan.camille, plan.nathan, plan.dev] {
        assert_eq!(week(&db, a, resource).await, [210; 5]);
    }
    // The application cannot queue it.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    assert!(
        infra::sqlx::query_scalar::<_, i32>("SELECT app_queue_load_recompute_all()")
            .fetch_one(&mut *tx)
            .await
            .is_err()
    );
}
