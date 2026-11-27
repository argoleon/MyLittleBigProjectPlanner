//! Integration tests on a real PostgreSQL: replacing a generic resource
//! with a person (L08-US08) — assignments cut at today, past days left on
//! the generic resource, a total effort shared by working days, segments
//! joining the person's assignment of the same ticket, overlaps refused,
//! archived projects left alone, the history of each ticket, the person's
//! overloads, and other tenants untouched.

#![allow(clippy::unwrap_used)]

use domain::assignments::Mode;
use domain::audit::{Action, Origin};
use domain::hierarchy::NodeType;
use domain::resources::Kind;
use infra::accounts::TenantRole;
use infra::assignments::{self, Assignment, AssignmentInput, StoredSegment};
use infra::audit::Author;
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::issues::{self, NewIssue};
use infra::nodes::{self, NewNode, NodeFields};
use infra::replacement::{self, Replacement, Side};
use infra::resources::{self, ResourceInput};
use infra::uuid::Uuid;
use serde_json::json;
use testkit::{SeedTenant, TestDb};
use time::Date;
use time::macros::date;

// A week without public holidays: Monday 16 to Friday 20 November 2026.
const MONDAY: Date = date!(2026 - 11 - 16);
const TUESDAY: Date = date!(2026 - 11 - 17);
const WEDNESDAY: Date = date!(2026 - 11 - 18);
const THURSDAY: Date = date!(2026 - 11 - 19);
const FRIDAY: Date = date!(2026 - 11 - 20);
const NEXT_MONDAY: Date = date!(2026 - 11 - 23);
const NEXT_TUESDAY: Date = date!(2026 - 11 - 24);

async fn resource(db: &TestDb, tenant: &SeedTenant, kind: Kind, name: &str) -> Uuid {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let created = resources::insert(
        &mut tx,
        new_id(),
        &ResourceInput {
            kind,
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

/// A ticket of `tenant` in a new project `key`, and the project.
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
    (issue.id, node.id)
}

fn segment(start: Date, end: Date, mode: Mode, value: u32) -> StoredSegment {
    StoredSegment { start, end, mode, value }
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
    let input = AssignmentInput { provisional, segments };
    let saved = assignments::insert(&mut tx, new_id(), issue, resource, &input, None)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    saved
}

fn author() -> Author {
    Author {
        actor_id: None,
        origin: Origin::Interface,
    }
}

/// Replaces `generic` with `person` on `today`, committed when `commit`.
async fn replace(
    db: &TestDb,
    tenant: &SeedTenant,
    generic: Uuid,
    person: Uuid,
    today: Date,
    commit: bool,
) -> Replacement {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let policy = infra::overload::policy(&mut tx).await.unwrap();
    let done = replacement::replace(
        &mut tx,
        Side {
            id: generic,
            calendar_id: None,
            name: "Backend developer",
        },
        Side {
            id: person,
            calendar_id: None,
            name: "Nathan Roux",
        },
        today,
        &policy,
        author(),
    )
    .await
    .unwrap();
    if commit {
        tx.commit().await.unwrap();
    }
    done
}

/// The assignments of `resource`, by ticket.
async fn of(db: &TestDb, tenant: &SeedTenant, issue: Uuid, resource: Uuid) -> Option<Assignment> {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    assignments::of_issue(&mut tx, issue)
        .await
        .unwrap()
        .into_iter()
        .find(|a| a.resource.id == resource)
}

/// Minutes `resource` has planned on `day`, as stored.
async fn day_load(db: &TestDb, tenant: &SeedTenant, resource: Uuid, day: Date) -> i64 {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    sqlx::query_scalar::<_, Option<i64>>(
        "SELECT sum(minutes)::bigint FROM resource_day_load WHERE resource_id = $1 AND day = $2",
    )
    .bind(resource)
    .bind(day)
    .fetch_one(&mut *tx)
    .await
    .unwrap()
    .unwrap_or(0)
}

#[tokio::test]
async fn assignments_move_from_today_on_and_past_days_stay() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let generic = resource(&db, a, Kind::Generic, "Backend developer").await;
    let person = resource(&db, a, Kind::Person, "Nathan Roux").await;
    let (across, _) = any_issue(&db, a, "PLAT").await;
    let (past, _) = any_issue(&db, a, "OPS").await;
    let (ahead, _) = any_issue(&db, a, "DATA").await;
    // 10 h over five working days, cut on Wednesday: 4 h stay, 6 h move.
    assign(&db, a, across, generic, false, vec![segment(MONDAY, FRIDAY, Mode::TotalEffort, 600)]).await;
    assign(&db, a, past, generic, false, vec![segment(MONDAY, TUESDAY, Mode::HoursPerDay, 120)]).await;
    let upcoming = assign(
        &db,
        a,
        ahead,
        generic,
        true,
        vec![segment(NEXT_MONDAY, NEXT_TUESDAY, Mode::Percentage, 50)],
    )
    .await;

    let done = replace(&db, a, generic, person, WEDNESDAY, true).await;

    assert_eq!(done.moved.len(), 2);
    assert!(done.moved.iter().all(|m| !m.merged));
    assert!(done.overlaps.is_empty() && done.archived.is_empty() && done.overloads.is_empty());
    let across_moved = done.moved.iter().find(|m| m.ticket.issue_id == across).unwrap();
    assert_eq!((across_moved.first, across_moved.last), (WEDNESDAY, FRIDAY));
    assert!(across_moved.ticket.key.starts_with("PLAT-"));

    let kept = of(&db, a, across, generic).await.unwrap();
    assert_eq!(kept.segments, vec![segment(MONDAY, TUESDAY, Mode::TotalEffort, 240)]);
    let taken = of(&db, a, across, person).await.unwrap();
    assert_eq!(taken.segments, vec![segment(WEDNESDAY, FRIDAY, Mode::TotalEffort, 360)]);
    assert_eq!(taken.id, across_moved.assignment_id);
    // Wholly past: left alone; wholly ahead: moved, flag kept, generic one gone.
    assert_eq!(of(&db, a, past, generic).await.unwrap().segments.len(), 1);
    assert!(of(&db, a, past, person).await.is_none());
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    assert!(assignments::find(&mut tx, upcoming.id).await.unwrap().is_none());
    drop(tx);
    let moved_ahead = of(&db, a, ahead, person).await.unwrap();
    assert!(moved_ahead.provisional);
    assert_eq!(moved_ahead.segments, upcoming.segments);

    // The daily load follows: 2 h a day before, on the person after.
    assert_eq!(day_load(&db, a, generic, MONDAY).await, 120 + 120);
    assert_eq!(day_load(&db, a, generic, WEDNESDAY).await, 0);
    assert_eq!(day_load(&db, a, person, WEDNESDAY).await, 120);

    // Each ticket moved says so in its history.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let history = infra::audit::of_issue(&mut tx, across, 10, None).await.unwrap();
    let entry = history
        .items
        .iter()
        .find(|e| e.action == Action::ResourceReplaced)
        .unwrap();
    assert_eq!(entry.old.as_ref().unwrap()["name"], "Backend developer");
    assert_eq!(entry.new.as_ref().unwrap()["name"], "Nathan Roux");
    assert_eq!(entry.new.as_ref().unwrap()["from"], "2026-11-18");
    let untouched = infra::audit::of_issue(&mut tx, past, 10, None).await.unwrap();
    assert!(untouched.items.iter().all(|e| e.action != Action::ResourceReplaced));
}

#[tokio::test]
async fn segments_join_the_persons_assignment_and_overlaps_are_refused() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let generic = resource(&db, a, Kind::Generic, "Backend developer").await;
    let person = resource(&db, a, Kind::Person, "Nathan Roux").await;
    let (joined, _) = any_issue(&db, a, "PLAT").await;
    let (clashing, _) = any_issue(&db, a, "OPS").await;
    assign(&db, a, joined, generic, false, vec![segment(WEDNESDAY, FRIDAY, Mode::HoursPerDay, 120)]).await;
    assign(&db, a, joined, person, false, vec![segment(NEXT_MONDAY, NEXT_TUESDAY, Mode::HoursPerDay, 60)]).await;
    assign(&db, a, clashing, generic, false, vec![segment(THURSDAY, FRIDAY, Mode::HoursPerDay, 60)]).await;
    assign(&db, a, clashing, person, false, vec![segment(FRIDAY, NEXT_MONDAY, Mode::HoursPerDay, 60)]).await;

    let done = replace(&db, a, generic, person, MONDAY, true).await;

    assert_eq!(done.moved.len(), 1);
    assert!(done.moved[0].merged);
    assert_eq!(done.overlaps.len(), 1);
    assert_eq!(done.overlaps[0].issue_id, clashing);
    let merged = of(&db, a, joined, person).await.unwrap();
    assert_eq!(
        merged.segments,
        vec![
            segment(WEDNESDAY, FRIDAY, Mode::HoursPerDay, 120),
            segment(NEXT_MONDAY, NEXT_TUESDAY, Mode::HoursPerDay, 60),
        ]
    );
    assert!(of(&db, a, joined, generic).await.is_none());
    // Nothing written for the clashing ticket.
    assert_eq!(of(&db, a, clashing, generic).await.unwrap().segments.len(), 1);
    assert_eq!(of(&db, a, clashing, person).await.unwrap().segments.len(), 1);
}

#[tokio::test]
async fn archived_projects_stay_and_overloads_of_the_person_are_told() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let generic = resource(&db, a, Kind::Generic, "Backend developer").await;
    let person = resource(&db, a, Kind::Person, "Nathan Roux").await;
    let (busy, _) = any_issue(&db, a, "PLAT").await;
    let (moving, _) = any_issue(&db, a, "OPS").await;
    let (frozen, frozen_project) = any_issue(&db, a, "OLD").await;
    // Wednesday has 7 h: the person's 5 h and the generic's 4 h make 9 h.
    assign(&db, a, busy, person, false, vec![segment(WEDNESDAY, WEDNESDAY, Mode::HoursPerDay, 300)]).await;
    assign(&db, a, moving, generic, false, vec![segment(WEDNESDAY, THURSDAY, Mode::HoursPerDay, 240)]).await;
    assign(&db, a, frozen, generic, false, vec![segment(WEDNESDAY, WEDNESDAY, Mode::HoursPerDay, 60)]).await;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    nodes::set_archived(&mut tx, frozen_project, None, true).await.unwrap().unwrap();
    tx.commit().await.unwrap();

    // Previewed: told, then rolled back.
    let preview = replace(&db, a, generic, person, MONDAY, false).await;
    assert_eq!(preview.moved.len(), 1);
    assert_eq!(preview.archived.len(), 1);
    assert_eq!(preview.archived[0].issue_id, frozen);
    assert_eq!(preview.overloads.len(), 1, "Wednesday only: Thursday has 4 h of 7 h");
    let wednesday = &preview.overloads[0];
    assert_eq!(wednesday.date, infra::capacity::local_date(WEDNESDAY).unwrap());
    assert_eq!((wednesday.planned, wednesday.available), (540, 420));
    assert!(of(&db, a, moving, person).await.is_none(), "a preview writes nothing");
    assert_eq!(day_load(&db, a, person, WEDNESDAY).await, 300);
    assert!(of(&db, a, frozen, generic).await.is_some());

    // Other tenants find nothing to move.
    let b = &db.tenant_b;
    let other_person = resource(&db, b, Kind::Person, "Iris Morel").await;
    let foreign = replace(&db, b, generic, other_person, MONDAY, true).await;
    assert!(foreign.moved.is_empty());
    assert!(of(&db, a, moving, generic).await.is_some());
}
