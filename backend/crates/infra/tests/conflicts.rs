//! Integration tests on a real PostgreSQL: the SQL of absence conflicts
//! (L08-US04) — kept in step with the daily load in the assignment's
//! transaction, opened, resolved and reopened; told through the outbox for
//! the days still to come only; read by assignment, by resource and day,
//! and listed with filters and the projects a member sees; per tenant.

#![allow(clippy::unwrap_used)]

use domain::absences::AbsenceKind;
use domain::assignments::Mode;
use domain::hierarchy::NodeType;
use domain::resources::Kind;
use infra::absences::{self, AbsenceInput};
use infra::accounts::TenantRole;
use infra::assignments::{self, AssignmentInput, StoredSegment};
use infra::conflicts::{self, CONFLICTS_OPENED, ConflictDay, Filter};
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::issues::{self, NewIssue};
use infra::nodes::{self, NewNode, NodeFields};
use infra::resources::{self, ResourceInput};
use infra::sqlx::PgConnection;
use infra::uuid::Uuid;
use serde_json::{Value, json};
use testkit::TestDb;
use time::Date;
use time::macros::date;

/// Days far enough ahead to be told whenever the test runs.
const WEDNESDAY: Date = date!(2031 - 03 - 05);
/// A day long past: stored, never told.
const PAST: Date = date!(2020 - 03 - 04);

async fn project(conn: &mut PgConnection, admin: Uuid, key: &str) -> Uuid {
    nodes::insert(
        conn,
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
    .unwrap()
    .id
}

async fn ticket(conn: &mut PgConnection, project: Uuid, key: &str, admin: Uuid) -> Uuid {
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

async fn leave(conn: &mut PgConnection, resource: Uuid, day: Date) {
    absences::insert(
        conn,
        new_id(),
        resource,
        &AbsenceInput {
            kind: AbsenceKind::Leave,
            start: day,
            starts_at_noon: false,
            end: day,
            ends_at_noon: false,
            comment: None,
        },
        None,
    )
    .await
    .unwrap()
    .unwrap();
}

fn hours(day: Date, minutes: u32) -> AssignmentInput {
    AssignmentInput {
        provisional: false,
        segments: vec![StoredSegment {
            start: day,
            end: day,
            mode: Mode::HoursPerDay,
            value: minutes,
        }],
    }
}

/// The payloads of the conflict events of the tenant.
async fn told(conn: &mut PgConnection) -> Vec<Value> {
    infra::sqlx::query_scalar("SELECT payload FROM outbox_event WHERE kind = $1 ORDER BY created_at, id")
        .bind(CONFLICTS_OPENED)
        .fetch_all(conn)
        .await
        .unwrap()
}

#[tokio::test]
async fn conflicts_follow_the_assignment_in_its_transaction() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let admin = db.add_member(a, TenantRole::TenantAdmin).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let plat = project(&mut tx, admin, "PLAT").await;
    let issue = ticket(&mut tx, plat, "PLAT", admin).await;
    let nathan = resource(&mut tx, "Nathan Roux").await;
    leave(&mut tx, nathan, WEDNESDAY).await;
    leave(&mut tx, nathan, PAST).await;

    // Planned on the leave (the overload check aside): opened and told.
    let id = new_id();
    assignments::insert(&mut tx, id, issue, nathan, &hours(WEDNESDAY, 240), Some(admin))
        .await
        .unwrap()
        .unwrap();
    let open = conflicts::open_of(&mut tx, &[id]).await.unwrap();
    assert_eq!(
        open[&id],
        vec![ConflictDay {
            day: WEDNESDAY,
            minutes: 240
        }]
    );
    let events = told(&mut tx).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["conflict"]["minutes"], 240);
    assert_eq!(events[0]["conflict"]["resourceName"], "Nathan Roux");
    let actor: Option<Uuid> = infra::sqlx::query_scalar("SELECT actor_id FROM outbox_event WHERE kind = $1")
        .bind(CONFLICTS_OPENED)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(actor, Some(admin), "the author of the change");

    // More hours: changed, not told again.
    assignments::update(&mut tx, id, None, &hours(WEDNESDAY, 300), Some(admin))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        conflicts::open_of(&mut tx, &[id]).await.unwrap()[&id][0].minutes,
        300
    );
    assert_eq!(told(&mut tx).await.len(), 1);
    let by_day = conflicts::minutes_by_day(&mut tx, nathan, WEDNESDAY, WEDNESDAY)
        .await
        .unwrap();
    assert_eq!(by_day.get(&WEDNESDAY), Some(&300));

    // Moved off the leave: resolved, kept with its last minutes.
    let thursday = date!(2031 - 03 - 06);
    assignments::update(&mut tx, id, None, &hours(thursday, 300), Some(admin))
        .await
        .unwrap()
        .unwrap();
    assert!(conflicts::open_of(&mut tx, &[id]).await.unwrap().is_empty());
    let (status, minutes): (String, i32) =
        infra::sqlx::query_as("SELECT status, minutes FROM workload_conflict WHERE assignment_id = $1")
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!((status.as_str(), minutes), ("resolved", 300));
    assert!(
        conflicts::minutes_by_day(&mut tx, nathan, WEDNESDAY, WEDNESDAY)
            .await
            .unwrap()
            .is_empty()
    );

    // A day long past: stored, never told.
    let old = new_id();
    let other = ticket(&mut tx, plat, "PLAT", admin).await;
    assignments::insert(&mut tx, old, other, nathan, &hours(PAST, 420), Some(admin))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        conflicts::open_of(&mut tx, &[old]).await.unwrap()[&old][0].day,
        PAST
    );
    assert_eq!(told(&mut tx).await.len(), 1);
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn conflicts_list_by_resource_project_status_and_the_projects_seen() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let admin = db.add_member(a, TenantRole::TenantAdmin).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let plat = project(&mut tx, admin, "PLAT").await;
    let ops = project(&mut tx, admin, "OPS").await;
    let nathan = resource(&mut tx, "Nathan Roux").await;
    let camille = resource(&mut tx, "Camille Martin").await;
    leave(&mut tx, nathan, WEDNESDAY).await;
    leave(&mut tx, camille, WEDNESDAY).await;
    for (project, key, who) in [
        (plat, "PLAT", nathan),
        (ops, "OPS", nathan),
        (plat, "PLAT", camille),
    ] {
        let issue = ticket(&mut tx, project, key, admin).await;
        assignments::insert(&mut tx, new_id(), issue, who, &hours(WEDNESDAY, 420), Some(admin))
            .await
            .unwrap()
            .unwrap();
    }

    let all = conflicts::list(&mut tx, &Filter::default(), 50, None)
        .await
        .unwrap();
    assert_eq!(all.items.len(), 3);
    assert!(all.next.is_none());
    let mine = Filter {
        resource_id: Some(nathan),
        ..Filter::default()
    };
    assert_eq!(
        conflicts::list(&mut tx, &mine, 50, None)
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    let in_plat = Filter {
        project_id: Some(plat),
        ..Filter::default()
    };
    let listed = conflicts::list(&mut tx, &in_plat, 50, None).await.unwrap();
    assert_eq!(listed.items.len(), 2);
    assert!(
        listed
            .items
            .iter()
            .all(|c| c.issue_key.starts_with("PLAT-") && c.status == "open")
    );
    // Only the projects seen.
    let seen = [ops];
    let visible = Filter {
        visible_projects: Some(&seen),
        ..Filter::default()
    };
    let listed = conflicts::list(&mut tx, &visible, 50, None).await.unwrap();
    assert_eq!(listed.items.len(), 1);
    assert_eq!(
        (listed.items[0].resource_name.as_str(), listed.items[0].minutes),
        ("Nathan Roux", 420)
    );
    let resolved = Filter {
        status: Some("resolved"),
        ..Filter::default()
    };
    assert!(
        conflicts::list(&mut tx, &resolved, 50, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    // Page by page.
    let first = conflicts::list(&mut tx, &Filter::default(), 2, None)
        .await
        .unwrap();
    assert_eq!(first.items.len(), 2);
    let second = conflicts::list(&mut tx, &Filter::default(), 2, first.next)
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    tx.commit().await.unwrap();

    // Another tenant lists none.
    let mut other = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert!(
        conflicts::list(&mut other, &Filter::default(), 50, None)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    other.commit().await.unwrap();
}
