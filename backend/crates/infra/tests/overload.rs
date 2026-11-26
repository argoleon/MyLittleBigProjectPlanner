//! Integration tests on a real PostgreSQL: the SQL of the overload check
//! (L08-US03) — the tenant's policy, the per-resource lock, the day loads
//! split between the other assignments and the one written (provisional
//! ones by the policy), the overloads a ticket flags, and forced overloads
//! traced in the history and listed newest first, per tenant.

#![allow(clippy::unwrap_used)]

use domain::assignments::Mode;
use domain::audit::{Action, Origin};
use domain::hierarchy::NodeType;
use domain::overload::{Overload, Policy};
use domain::resources::Kind;
use domain::tenant::OverloadMode;
use infra::accounts::TenantRole;
use infra::assignments::{self, Assignment, AssignmentInput, StoredSegment};
use infra::audit::Author;
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::issues::{self, NewIssue};
use infra::nodes::{self, NewNode, NodeFields};
use infra::overload::{self, Candidate, Forced};
use infra::resources::{self, ResourceInput};
use infra::uuid::Uuid;
use serde_json::json;
use testkit::{SeedTenant, TestDb};
use time::Date;
use time::macros::date;

async fn resource(db: &TestDb, tenant: &SeedTenant, name: &str) -> Uuid {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let created = resources::insert(
        &mut tx,
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
    .unwrap();
    tx.commit().await.unwrap();
    created.id
}

/// A ticket of `tenant` in a new project `key`, and a tenant admin.
async fn any_issue(db: &TestDb, tenant: &SeedTenant, key: &str) -> (Uuid, Uuid) {
    let admin = db.add_member(tenant, TenantRole::TenantAdmin).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let node = nodes::insert(
        &mut tx,
        &NewNode {
            id: new_id(),
            node_type: NodeType::Project,
            key,
            fields: NodeFields {
                key: None,
                name: key,
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
            project_key: key,
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
    tx.commit().await.unwrap();
    (issue.id, admin)
}

fn hours(day: Date, minutes: u32) -> Vec<StoredSegment> {
    vec![StoredSegment {
        start: day,
        end: day,
        mode: Mode::HoursPerDay,
        value: minutes,
    }]
}

async fn assign(
    db: &TestDb,
    tenant: &SeedTenant,
    issue: Uuid,
    resource: Uuid,
    provisional: bool,
    segments: Vec<StoredSegment>,
) -> Assignment {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let input = AssignmentInput {
        provisional,
        segments,
    };
    let saved = assignments::insert(&mut tx, new_id(), issue, resource, &input, None)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    saved
}

const WEDNESDAY: Date = date!(2026 - 11 - 18);

fn local(day: Date) -> domain::dates::Date {
    infra::capacity::local_date(day).unwrap()
}

#[tokio::test]
async fn day_loads_split_the_others_from_the_assignment_written() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let nathan = resource(&db, a, "Nathan Roux").await;
    let (one, _) = any_issue(&db, a, "PLAT").await;
    let (two, _) = any_issue(&db, a, "OPS").await;
    let (three, _) = any_issue(&db, a, "DATA").await;
    let firm = assign(&db, a, one, nathan, false, hours(WEDNESDAY, 240)).await;
    assign(&db, a, two, nathan, true, hours(WEDNESDAY, 120)).await;

    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let policy = overload::policy(&mut tx).await.unwrap();
    assert_eq!(
        policy,
        Policy::default(),
        "no tolerance, blocking, provisional counted"
    );
    // The firm assignment raised to 5 h: its own 4 h are `before`.
    let raised = hours(WEDNESDAY, 300);
    let change = Candidate {
        assignment_id: Some(firm.id),
        issue_id: one,
        resource_id: nathan,
        calendar_id: None,
        provisional: false,
        segments: &raised,
    };
    let days = overload::day_loads(&mut tx, &policy, &change).await.unwrap();
    assert_eq!(days.len(), 1);
    assert_eq!(
        (
            days[0].date,
            days[0].capacity,
            days[0].existing,
            days[0].before,
            days[0].after
        ),
        (local(WEDNESDAY), 420, 120, 240, 300)
    );
    // A new assignment: both are existing load.
    let one_hour = hours(WEDNESDAY, 60);
    let new = Candidate {
        assignment_id: None,
        issue_id: three,
        resource_id: nathan,
        calendar_id: None,
        provisional: false,
        segments: &one_hour,
    };
    let days = overload::day_loads(&mut tx, &policy, &new).await.unwrap();
    assert_eq!((days[0].existing, days[0].before, days[0].after), (360, 0, 60));
    // Provisional ones not counted: neither the existing one nor the new one.
    let ignored = Policy {
        count_provisional: false,
        ..policy
    };
    let days = overload::day_loads(&mut tx, &ignored, &new).await.unwrap();
    assert_eq!(days[0].existing, 240);
    let days = overload::day_loads(
        &mut tx,
        &ignored,
        &Candidate {
            provisional: true,
            ..new
        },
    )
    .await
    .unwrap();
    assert_eq!(days[0].after, 0);
    // Another tenant sees nothing of it.
    tx.commit().await.unwrap();
    let mut other = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    let days = overload::day_loads(&mut other, &policy, &new).await.unwrap();
    assert_eq!(days[0].existing, 0);
    other.commit().await.unwrap();
}

#[tokio::test]
async fn a_ticket_flags_the_days_its_assignments_overload() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let nathan = resource(&db, a, "Nathan Roux").await;
    let (one, _) = any_issue(&db, a, "PLAT").await;
    let (two, _) = any_issue(&db, a, "OPS").await;
    let firm = assign(&db, a, one, nathan, false, hours(WEDNESDAY, 300)).await;
    let tentative = assign(&db, a, two, nathan, true, hours(WEDNESDAY, 180)).await;

    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let counted = Policy::default();
    let flagged = overload::flagged(&mut tx, &counted, &[&firm, &tentative])
        .await
        .unwrap();
    let expected = Overload {
        date: local(WEDNESDAY),
        planned: 480,
        available: 420,
        limit: 420,
    };
    assert_eq!(flagged[&firm.id], vec![expected]);
    assert_eq!(flagged[&tentative.id], vec![expected]);
    // Provisional ones not counted: 5 h fit in 7 h.
    let ignored = Policy {
        count_provisional: false,
        ..counted
    };
    assert!(
        overload::flagged(&mut tx, &ignored, &[&firm, &tentative])
            .await
            .unwrap()
            .is_empty()
    );
    // A tolerance of 15% lets 8 h 03 through.
    let tolerant = Policy {
        tolerance_percent: 15,
        mode: OverloadMode::Warning,
        ..counted
    };
    assert!(
        overload::flagged(&mut tx, &tolerant, &[&firm])
            .await
            .unwrap()
            .is_empty()
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn the_resource_lock_holds_until_the_end_of_the_transaction() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let nathan = resource(&db, a, "Nathan Roux").await;
    let other = resource(&db, a, "Rust developer").await;

    let mut first = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    overload::lock_resource(&mut first, nathan).await.unwrap();
    // A second writer on the same resource waits: here it gives up.
    let mut second = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    infra::sqlx::query("SET LOCAL lock_timeout = '100ms'")
        .execute(&mut *second)
        .await
        .unwrap();
    assert!(overload::lock_resource(&mut second, nathan).await.is_err());
    second.rollback().await.unwrap();
    // Another resource is free.
    let mut third = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    overload::lock_resource(&mut third, other).await.unwrap();
    third.commit().await.unwrap();
    // Released at commit.
    first.commit().await.unwrap();
    let mut fourth = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    infra::sqlx::query("SET LOCAL lock_timeout = '100ms'")
        .execute(&mut *fourth)
        .await
        .unwrap();
    overload::lock_resource(&mut fourth, nathan).await.unwrap();
    fourth.commit().await.unwrap();
}

#[tokio::test]
async fn forced_overloads_are_traced_and_listed_newest_first() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let nathan = resource(&db, a, "Nathan Roux").await;
    let (issue, admin) = any_issue(&db, a, "PLAT").await;
    let assignment = assign(&db, a, issue, nathan, false, hours(WEDNESDAY, 480)).await;
    let author = Author {
        actor_id: Some(admin),
        origin: Origin::Api,
    };
    let days = [Overload {
        date: local(WEDNESDAY),
        planned: 480,
        available: 420,
        limit: 420,
    }];
    let forced = Forced {
        assignment_id: assignment.id,
        issue_id: issue,
        resource_id: nathan,
        resource_name: "Nathan Roux",
        overloads: &days,
    };
    for _ in 0..2 {
        let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
        overload::record_forced(&mut tx, author, &forced).await.unwrap();
        tx.commit().await.unwrap();
    }

    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let first = overload::list_forced(&mut tx, 1, None).await.unwrap();
    assert_eq!(first.items.len(), 1);
    let item = &first.items[0];
    assert_eq!(
        (
            item.issue_key.as_deref(),
            item.resource_name.as_deref(),
            item.actor_id,
            item.origin.as_str()
        ),
        (Some("PLAT-1"), Some("Nathan Roux"), admin, "api")
    );
    assert_eq!(item.days, days);
    let second = overload::list_forced(&mut tx, 1, first.next).await.unwrap();
    assert_eq!(second.items.len(), 1);
    assert!(second.items[0].created_at <= item.created_at);
    assert_ne!(second.items[0].id, item.id);
    assert_eq!(overload::list_forced(&mut tx, 50, None).await.unwrap().next, None);
    // In the ticket's history too.
    let history = infra::audit::of_issue(&mut tx, issue, 10, None).await.unwrap();
    let entry = history
        .items
        .iter()
        .find(|e| e.action == Action::OverloadForced)
        .unwrap();
    assert_eq!(entry.new.as_ref().unwrap()["resourceName"], "Nathan Roux");
    assert_eq!(entry.new.as_ref().unwrap()["days"][0]["date"], "2026-11-18");
    tx.commit().await.unwrap();

    // Kept when the assignment goes; another tenant lists nothing.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    assignments::delete(&mut tx, assignment.id, None, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        overload::list_forced(&mut tx, 50, None)
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    tx.commit().await.unwrap();
    let mut other = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert!(
        overload::list_forced(&mut other, 50, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    other.commit().await.unwrap();
}
