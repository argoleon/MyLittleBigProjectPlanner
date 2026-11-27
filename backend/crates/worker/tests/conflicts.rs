//! Integration tests: absence conflicts kept up to date by the worker and
//! told to the project manager (L08-US04), against PostgreSQL with
//! row-level security. A leave on hours planned per day opens a conflict
//! and notifies the ticket's assignee and the project's Project manager
//! role holders (the default rule); a percentage never conflicts; a segment
//! moved off the leave resolves it, without a new notification.

#![allow(clippy::unwrap_used)]

use domain::absences::AbsenceKind;
use domain::assignments::Mode;
use domain::hierarchy::NodeType;
use domain::resources::Kind;
use infra::absences::{self, AbsenceInput};
use infra::accounts::TenantRole;
use infra::assignments::{self, AssignmentInput, StoredSegment};
use infra::db::begin_tenant_tx;
use infra::email::Mailer;
use infra::grants::{self, Grantee, NewGrant};
use infra::id::new_id;
use infra::issues::{self, NewIssue};
use infra::nodes::{self, NewNode, NodeFields};
use infra::resources::{self, ResourceInput};
use infra::uuid::Uuid;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};
use time::Date;
use time::macros::date;
use worker::Batch;
use worker::jobs::{JobRunner, Jobs};

/// Days far enough ahead to be rescheduled whenever the test runs: a week
/// of March 2031, no public holiday.
const MONDAY: Date = date!(2031 - 03 - 03);
const WEDNESDAY: Date = date!(2031 - 03 - 05);
const FRIDAY: Date = date!(2031 - 03 - 07);

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
    assignee: Uuid,
    manager: Uuid,
    nathan: Uuid,
    /// 7 h/day from Monday to Friday (50% over the same week on PLAT-2).
    hours: Uuid,
}

async fn ticket(
    conn: &mut infra::sqlx::PgConnection,
    project: Uuid,
    admin: Uuid,
    assignee: Option<Uuid>,
    summary: &str,
) -> Uuid {
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
            summary,
            description: None,
            priority: "medium",
            reporter_id: admin,
            assignee_id: assignee,
            start_date: None,
            due_date: None,
            fields: &fields,
        },
    )
    .await
    .unwrap()
    .id
}

/// The project PLAT: its assignee reads it, a member manages it (the
/// default Project manager role); Nathan works 7 h/day on PLAT-1 and 50%
/// on PLAT-2 the week of 3 March 2031.
async fn plan(db: &TestDb, tenant: &SeedTenant) -> Plan {
    let admin = db.add_member(tenant, TenantRole::TenantAdmin).await.account_id;
    let assignee = db.add_member(tenant, TenantRole::Member).await.account_id;
    let manager = db.add_member(tenant, TenantRole::Member).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
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
    // The default configuration: its rules say whom to notify.
    let config = infra::configs::default_id(&mut tx).await.unwrap().unwrap();
    assert!(infra::configs::assign(&mut tx, project, config).await.unwrap());
    for (account, label) in [(assignee, "Contributor"), (manager, "Project manager")] {
        let role: Uuid = infra::sqlx::query_scalar("SELECT id FROM role WHERE label ->> 'en' = $1")
            .bind(label)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        grants::insert(
            &mut tx,
            &NewGrant {
                id: new_id(),
                node: project,
                role,
                grantee: Grantee::Member(account),
                created_by: admin,
            },
        )
        .await
        .unwrap()
        .unwrap();
    }
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
    let first = ticket(
        &mut tx,
        project,
        admin,
        Some(assignee),
        "Account migration scripts",
    )
    .await;
    let second = ticket(&mut tx, project, admin, Some(assignee), "Directory sync").await;
    let mut ids = Vec::new();
    for (issue, mode, value) in [(first, Mode::HoursPerDay, 420), (second, Mode::Percentage, 50)] {
        let input = AssignmentInput {
            provisional: false,
            segments: vec![StoredSegment {
                start: MONDAY,
                end: FRIDAY,
                mode,
                value,
            }],
        };
        ids.push(
            assignments::insert(&mut tx, new_id(), issue, nathan, &input, Some(admin))
                .await
                .unwrap()
                .unwrap()
                .id,
        );
    }
    tx.commit().await.unwrap();
    drain(db).await;
    Plan {
        admin,
        assignee,
        manager,
        nathan,
        hours: ids[0],
    }
}

/// The conflicts of the tenant: (assignment, day, minutes, status).
async fn conflicts(db: &TestDb, tenant: &SeedTenant) -> Vec<(Uuid, Date, i32, String)> {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let rows = infra::sqlx::query_as(
        "SELECT assignment_id, day, minutes, status FROM workload_conflict ORDER BY assignment_id, day",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    rows
}

/// The absence conflict notifications of the tenant: (account, details).
async fn notified(db: &TestDb, tenant: &SeedTenant) -> Vec<(Uuid, Value)> {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let rows = infra::sqlx::query_as(
        "SELECT account_id, details FROM notification WHERE event = 'absence_conflict' ORDER BY account_id",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    rows
}

#[tokio::test]
async fn a_leave_on_planned_hours_opens_a_conflict_and_tells_the_project_manager() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let plan = plan(&db, a).await;
    assert!(conflicts(&db, a).await.is_empty());

    // AC1: a leave on Wednesday, declared by the admin.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    absences::insert(
        &mut tx,
        new_id(),
        plan.nathan,
        &AbsenceInput {
            kind: AbsenceKind::Leave,
            start: WEDNESDAY,
            starts_at_noon: false,
            end: WEDNESDAY,
            ends_at_noon: false,
            comment: None,
        },
        Some(plan.admin),
    )
    .await
    .unwrap()
    .unwrap();
    tx.commit().await.unwrap();
    drain(&db).await;

    // 7 h to reschedule on PLAT-1; AC2: none on the percentage.
    assert_eq!(
        conflicts(&db, a).await,
        [(plan.hours, WEDNESDAY, 420, "open".to_owned())]
    );
    let notes = notified(&db, a).await;
    let mut expected = vec![plan.assignee, plan.manager];
    expected.sort_unstable();
    assert_eq!(
        notes.iter().map(|(account, _)| *account).collect::<Vec<_>>(),
        expected
    );
    assert_eq!(notes[0].1["key"], "PLAT-1");
    assert_eq!(notes[0].1["conflict"]["resourceName"], "Nathan Roux");
    assert_eq!(notes[0].1["conflict"]["minutes"], 420);
    assert_eq!(notes[0].1["conflict"]["days"][0]["date"], "2031-03-05");

    // AC3: the segment moved off the leave resolves it, nobody told again.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let input = AssignmentInput {
        provisional: false,
        segments: vec![
            StoredSegment {
                start: MONDAY,
                end: date!(2031 - 03 - 04),
                mode: Mode::HoursPerDay,
                value: 420,
            },
            StoredSegment {
                start: date!(2031 - 03 - 06),
                end: FRIDAY,
                mode: Mode::HoursPerDay,
                value: 420,
            },
        ],
    };
    assignments::update(&mut tx, plan.hours, None, &input, Some(plan.admin))
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    drain(&db).await;
    assert_eq!(
        conflicts(&db, a).await,
        [(plan.hours, WEDNESDAY, 420, "resolved".to_owned())]
    );
    assert_eq!(notified(&db, a).await.len(), 2);

    // Back on the leave: reopened, and told again.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let input = AssignmentInput {
        provisional: false,
        segments: vec![StoredSegment {
            start: MONDAY,
            end: FRIDAY,
            mode: Mode::HoursPerDay,
            value: 300,
        }],
    };
    assignments::update(&mut tx, plan.hours, None, &input, Some(plan.admin))
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    drain(&db).await;
    assert_eq!(
        conflicts(&db, a).await,
        [(plan.hours, WEDNESDAY, 300, "open".to_owned())]
    );
    // The admin made the change: the rule never notifies its author.
    assert_eq!(notified(&db, a).await.len(), 4);

    // Another tenant sees none of it.
    assert!(conflicts(&db, &db.tenant_b).await.is_empty());
}
