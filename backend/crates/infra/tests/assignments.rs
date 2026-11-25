//! Integration tests on a real PostgreSQL: assignments and their segments
//! (L08-US01) — the exclusion constraint keeping segments of an assignment
//! apart, one assignment per resource and ticket, and tenant isolation.

#![allow(clippy::unwrap_used)]

use domain::assignments::Mode;
use domain::hierarchy::NodeType;
use domain::resources::Kind;
use infra::accounts::TenantRole;
use infra::assignments::{self, AssignmentInput, StoredSegment, WriteConflict};
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::issues::{self, NewIssue};
use infra::nodes::{self, NewNode, NodeFields};
use infra::resources::{self, ResourceInput};
use infra::sqlx;
use infra::uuid::Uuid;
use serde_json::json;
use testkit::{SeedTenant, TestDb};
use time::Date;
use time::macros::date;

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

/// A ticket of `tenant`, in a new project PLAT.
async fn any_issue(db: &TestDb, tenant: &SeedTenant) -> Uuid {
    let admin = db.add_member(tenant, TenantRole::TenantAdmin).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let node = nodes::insert(
        &mut tx,
        &NewNode {
            id: new_id(),
            node_type: NodeType::Project,
            key: "PLAT",
            fields: NodeFields {
                key: None,
                name: "PLAT",
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
    tx.commit().await.unwrap();
    issue.id
}

fn segment(start: Date, end: Date) -> StoredSegment {
    StoredSegment {
        start,
        end,
        mode: Mode::HoursPerDay,
        value: 240,
    }
}

#[tokio::test]
async fn segments_never_overlap_and_a_resource_is_assigned_once_per_ticket() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let nathan = resource(&db, a).await;
    let issue = any_issue(&db, a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let input = AssignmentInput {
        provisional: false,
        segments: vec![
            segment(date!(2026 - 11 - 23), date!(2026 - 11 - 27)),
            segment(date!(2026 - 11 - 16), date!(2026 - 11 - 20)),
        ],
    };
    let id = new_id();
    let saved = assignments::insert(&mut tx, id, issue, nathan, &input, None)
        .await
        .unwrap()
        .unwrap();
    // Segments keep the order given.
    assert_eq!(saved.segments, input.segments);
    assert_eq!(saved.span(), Some((date!(2026 - 11 - 16), date!(2026 - 11 - 27))));
    assert_eq!(
        assignments::insert(&mut tx, new_id(), issue, nathan, &input, None)
            .await
            .unwrap(),
        Err(WriteConflict::ResourceTaken)
    );
    tx.commit().await.unwrap();

    // The database refuses overlapping segments on its own.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let overlapping = AssignmentInput {
        provisional: true,
        segments: vec![
            segment(date!(2026 - 11 - 16), date!(2026 - 11 - 20)),
            segment(date!(2026 - 11 - 20), date!(2026 - 11 - 24)),
        ],
    };
    assert_eq!(
        assignments::update(&mut tx, id, None, &overlapping, None)
            .await
            .unwrap(),
        Err(WriteConflict::Overlap)
    );
    tx.rollback().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    assert_eq!(
        assignments::update(&mut tx, id, Some(&[5]), &input, None)
            .await
            .unwrap(),
        Err(WriteConflict::VersionMismatch)
    );
    let changed = AssignmentInput {
        provisional: true,
        segments: vec![segment(date!(2026 - 11 - 16), date!(2026 - 11 - 18))],
    };
    let updated = assignments::update(&mut tx, id, Some(&[1]), &changed, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (updated.version, updated.provisional, updated.segments.len()),
        (2, true, 1)
    );
    assert_eq!(assignments::of_issue(&mut tx, issue).await.unwrap().len(), 1);
    assert_eq!(
        assignments::delete(&mut tx, id, Some(&[2]), None).await.unwrap(),
        Ok(())
    );
    assert_eq!(assignments::find(&mut tx, id).await.unwrap(), None);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM assignment_segment")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(left, 0, "segments go with their assignment");
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn another_tenant_never_sees_nor_writes_an_assignment() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let nathan = resource(&db, a).await;
    let issue = any_issue(&db, a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let input = AssignmentInput {
        provisional: false,
        segments: vec![segment(date!(2026 - 11 - 16), date!(2026 - 11 - 20))],
    };
    let id = new_id();
    assignments::insert(&mut tx, id, issue, nathan, &input, None)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert_eq!(assignments::find(&mut tx, id).await.unwrap(), None);
    assert!(assignments::of_issue(&mut tx, issue).await.unwrap().is_empty());
    assert_eq!(
        assignments::update(&mut tx, id, None, &input, None)
            .await
            .unwrap(),
        Err(WriteConflict::NotFound)
    );
    assert_eq!(
        assignments::delete(&mut tx, id, None, None).await.unwrap(),
        Err(WriteConflict::NotFound)
    );
    let segments: i64 = sqlx::query_scalar("SELECT count(*) FROM assignment_segment")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(segments, 0, "row-level security hides alpha's segments");
    // Beta cannot assign alpha's resource to alpha's ticket.
    assert!(
        assignments::insert(&mut tx, new_id(), issue, nathan, &input, None)
            .await
            .is_err()
    );
}
