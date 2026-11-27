//! Integration tests on a real PostgreSQL: the assignments of a resource
//! (L08-US05) — those meeting a period, newest first, page by page, with
//! their tickets and projects; per tenant.

#![allow(clippy::unwrap_used)]

use domain::assignments::Mode;
use domain::hierarchy::NodeType;
use domain::resources::Kind;
use infra::accounts::TenantRole;
use infra::assignments::{self, AssignmentInput, StoredSegment};
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::issues::{self, NewIssue};
use infra::nodes::{self, NewNode, NodeFields};
use infra::resources::{self, ResourceInput};
use infra::sqlx::PgConnection;
use infra::uuid::Uuid;
use serde_json::json;
use testkit::TestDb;
use time::Date;
use time::macros::date;

async fn project(conn: &mut PgConnection, admin: Uuid, key: &str, name: &str) -> Uuid {
    nodes::insert(
        conn,
        &NewNode {
            id: new_id(),
            node_type: NodeType::Project,
            key,
            fields: NodeFields {
                key: None,
                name,
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
    .id
}

async fn ticket(conn: &mut PgConnection, project: Uuid, key: &str, admin: Uuid, summary: &str) -> Uuid {
    let fields = json!({});
    issues::create(
        conn,
        &NewIssue {
            id: new_id(),
            project_id: project,
            project_key: key,
            type_id: Uuid::from_u128(1),
            status_id: Uuid::from_u128(2),
            parent_id: None,
            summary,
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

fn week(monday: Date) -> AssignmentInput {
    AssignmentInput {
        provisional: false,
        segments: vec![StoredSegment {
            start: monday,
            end: monday + time::Duration::days(4),
            mode: Mode::Percentage,
            value: 50,
        }],
    }
}

#[tokio::test]
async fn a_resource_lists_its_assignments_by_period_page_by_page() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let admin = db.add_member(a, TenantRole::TenantAdmin).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let plat = project(&mut tx, admin, "PLAT", "Identity platform").await;
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
    // Three weeks of November 2026, one ticket each.
    let mut ids = Vec::new();
    for (i, monday) in [
        date!(2026 - 11 - 02),
        date!(2026 - 11 - 16),
        date!(2026 - 11 - 30),
    ]
    .into_iter()
    .enumerate()
    {
        let issue = ticket(&mut tx, plat, "PLAT", admin, &format!("Week {i}")).await;
        ids.push(
            assignments::insert(&mut tx, new_id(), issue, nathan, &week(monday), None)
                .await
                .unwrap()
                .unwrap()
                .id,
        );
    }

    // Every one, newest first.
    let all = assignments::of_resource(&mut tx, nathan, None, None, 50, None)
        .await
        .unwrap();
    assert_eq!(
        all.items.iter().map(|a| a.id).collect::<Vec<_>>(),
        [ids[2], ids[1], ids[0]]
    );
    // Those meeting a period: from the 18th, until the 20th.
    let from = assignments::of_resource(&mut tx, nathan, Some(date!(2026 - 11 - 18)), None, 50, None)
        .await
        .unwrap();
    assert_eq!(
        from.items.iter().map(|a| a.id).collect::<Vec<_>>(),
        [ids[2], ids[1]]
    );
    let within = assignments::of_resource(
        &mut tx,
        nathan,
        Some(date!(2026 - 11 - 18)),
        Some(date!(2026 - 11 - 20)),
        50,
        None,
    )
    .await
    .unwrap();
    assert_eq!(within.items.iter().map(|a| a.id).collect::<Vec<_>>(), [ids[1]]);
    // Page by page.
    let first = assignments::of_resource(&mut tx, nathan, None, None, 2, None)
        .await
        .unwrap();
    assert_eq!(first.items.len(), 2);
    let second = assignments::of_resource(&mut tx, nathan, None, None, 2, first.next)
        .await
        .unwrap();
    assert_eq!(second.items.iter().map(|a| a.id).collect::<Vec<_>>(), [ids[0]]);
    assert!(second.next.is_none());

    // Their tickets and projects.
    let issue_ids: Vec<Uuid> = all.items.iter().map(|a| a.issue_id).collect();
    let refs = assignments::issue_refs(&mut tx, &issue_ids).await.unwrap();
    let first_ref = &refs[&all.items[2].issue_id];
    assert_eq!(
        (
            first_ref.key.as_str(),
            first_ref.summary.as_str(),
            first_ref.project_key.as_str()
        ),
        ("PLAT-1", "Week 0", "PLAT")
    );
    assert_eq!(
        (
            first_ref.project_id,
            first_ref.project_name.as_str(),
            first_ref.project_archived
        ),
        (plat, "Identity platform", false)
    );
    tx.commit().await.unwrap();

    // Another tenant lists none.
    let mut other = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert!(
        assignments::of_resource(&mut other, nathan, None, None, 50, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    other.commit().await.unwrap();
}
