//! Integration tests on a real PostgreSQL: the daily load of resources
//! (L08-US02) — rows written with the assignment in its transaction, equal
//! to the domain computation after a creation, a change and a deletion;
//! monthly partitions created as needed and pruned on date ranges;
//! recomputations idempotent; tenant isolation.

#![allow(clippy::unwrap_used)]

use domain::assignments::Mode;
use domain::hierarchy::NodeType;
use domain::resources::Kind;
use infra::accounts::TenantRole;
use infra::assignments::{self, AssignmentInput, StoredSegment};
use infra::day_load::{self, Recompute, Scope};
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

/// A ticket of `tenant`, in a new project.
async fn any_issue(db: &TestDb, tenant: &SeedTenant, key: &str) -> Uuid {
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
    issue.id
}

fn segment(start: Date, end: Date, mode: Mode, value: u32) -> StoredSegment {
    StoredSegment {
        start,
        end,
        mode,
        value,
    }
}

/// The load rows of `resource`, day by day: (day, minutes, provisional).
async fn load(db: &TestDb, tenant: &SeedTenant, resource: Uuid) -> Vec<(Date, u32, bool)> {
    let mut tx = begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let rows = day_load::of_resource(&mut tx, resource, date!(2000 - 01 - 01), date!(2100 - 01 - 01))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    rows.into_iter()
        .map(|r| (r.day, r.minutes, r.provisional))
        .collect()
}

#[tokio::test]
async fn an_assignment_writes_its_daily_load_in_its_own_transaction() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let nathan = resource(&db, a).await;
    let issue = any_issue(&db, a, "PLAT").await;
    let id = new_id();

    // 4 h/day over a week and a weekend, then 50% of two 7 h days.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let input = AssignmentInput {
        provisional: true,
        segments: vec![
            segment(
                date!(2026 - 11 - 16),
                date!(2026 - 11 - 22),
                Mode::HoursPerDay,
                240,
            ),
            segment(date!(2026 - 11 - 23), date!(2026 - 11 - 24), Mode::Percentage, 50),
        ],
    };
    let saved = assignments::insert(&mut tx, id, issue, nathan, &input, None)
        .await
        .unwrap()
        .unwrap();
    // The rows match the domain's computation.
    let expected = day_load::rows_of(&mut tx, &[saved]).await.unwrap();
    assert_eq!(expected.len(), 7);
    tx.commit().await.unwrap();
    let rows = load(&db, a, nathan).await;
    assert_eq!(
        rows,
        expected
            .iter()
            .map(|r| (r.day, r.minutes, true))
            .collect::<Vec<_>>()
    );
    assert_eq!(rows[0], (date!(2026 - 11 - 16), 240, true));
    assert_eq!(rows[5], (date!(2026 - 11 - 23), 210, true));

    // A change replaces them.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let changed = AssignmentInput {
        provisional: false,
        segments: vec![segment(
            date!(2026 - 11 - 18),
            date!(2026 - 11 - 19),
            Mode::TotalEffort,
            601,
        )],
    };
    assignments::update(&mut tx, id, None, &changed, None)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        load(&db, a, nathan).await,
        [
            (date!(2026 - 11 - 18), 301, false),
            (date!(2026 - 11 - 19), 300, false)
        ]
    );

    // A rolled back write leaves the rows as they were.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    assignments::update(&mut tx, id, None, &input, None)
        .await
        .unwrap()
        .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(load(&db, a, nathan).await.len(), 2);

    // AC4: deleting the assignment takes its rows along.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    assignments::delete(&mut tx, id, None, None)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    assert!(load(&db, a, nathan).await.is_empty());
}

#[tokio::test]
async fn rows_go_to_monthly_partitions_made_as_needed_and_ranges_read_only_theirs() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let nathan = resource(&db, a).await;
    let issue = any_issue(&db, a, "PLAT").await;
    // Far beyond the partitions made ahead: from December 2031 to January 2032.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let input = AssignmentInput {
        provisional: false,
        segments: vec![segment(
            date!(2031 - 12 - 29),
            date!(2032 - 01 - 02),
            Mode::HoursPerDay,
            60,
        )],
    };
    assignments::insert(&mut tx, new_id(), issue, nathan, &input, None)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();

    // Read in the tenant: row-level security is forced on the owner too.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let partitions: Vec<(Date, String)> =
        sqlx::query_as("SELECT day, tableoid::regclass::text FROM resource_day_load ORDER BY day")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        partitions,
        [
            (date!(2031 - 12 - 29), "resource_day_load_2031_12".to_owned()),
            (date!(2031 - 12 - 30), "resource_day_load_2031_12".to_owned()),
            (date!(2031 - 12 - 31), "resource_day_load_2031_12".to_owned()),
            // New Year's Day loads nothing.
            (date!(2032 - 01 - 02), "resource_day_load_2032_01".to_owned()),
        ],
        "never the default partition"
    );
    // Asking again creates nothing; the partitions ahead exist.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    assert_eq!(
        day_load::ensure_partitions(&mut tx, date!(2031 - 12 - 01), date!(2032 - 01 - 31))
            .await
            .unwrap(),
        0
    );
    assert_eq!(day_load::ensure_partitions_ahead(&mut tx).await.unwrap(), 0);
    tx.commit().await.unwrap();

    // A range reads only its months' partitions.
    let plan: String = sqlx::query_scalar(
        "EXPLAIN SELECT * FROM resource_day_load WHERE resource_id = $1 AND day BETWEEN '2031-12-01' AND '2031-12-31'",
    )
    .bind(nathan)
    .fetch_all(&db.owner_pool)
    .await
    .unwrap()
    .into_iter()
    .collect::<Vec<String>>()
    .join("\n");
    assert!(plan.contains("resource_day_load_2031_12"), "{plan}");
    assert!(!plan.contains("resource_day_load_2032_01"), "{plan}");
    assert!(!plan.contains("resource_day_load_default"), "{plan}");
}

#[tokio::test]
async fn recomputing_gives_the_same_rows_and_another_tenant_sees_none() {
    let db = TestDb::new().await;
    let a = &db.tenant_a;
    let nathan = resource(&db, a).await;
    let issue = any_issue(&db, a, "PLAT").await;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let input = AssignmentInput {
        provisional: false,
        segments: vec![segment(
            date!(2026 - 12 - 21),
            date!(2026 - 12 - 31),
            Mode::TotalEffort,
            2400,
        )],
    };
    let id = new_id();
    assignments::insert(&mut tx, id, issue, nathan, &input, None)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let before = load(&db, a, nathan).await;
    // 40 h over the 8 working days but Christmas: 300 min each.
    assert_eq!(before.len(), 8);
    assert!(before.iter().all(|(_, minutes, _)| *minutes == 300));

    // AC5: run twice, from scratch too: the same rows.
    let job = Recompute {
        scope: Scope::Resources { ids: vec![nathan] },
        from: Some("2026-12-24".to_owned()),
        until: None,
    };
    for _ in 0..2 {
        let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
        assert_eq!(day_load::run(&mut tx, &job).await.unwrap(), 1);
        tx.commit().await.unwrap();
        assert_eq!(load(&db, a, nathan).await, before);
    }
    sqlx::query("DELETE FROM resource_day_load")
        .execute(&db.owner_pool)
        .await
        .unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    assert_eq!(
        day_load::run(
            &mut tx,
            &Recompute {
                scope: Scope::All,
                from: None,
                until: None
            }
        )
        .await
        .unwrap(),
        1
    );
    tx.commit().await.unwrap();
    assert_eq!(load(&db, a, nathan).await, before);
    // Outside the range: nothing to recompute.
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    let later = Recompute {
        scope: Scope::Resources { ids: vec![nathan] },
        from: Some("2027-01-01".to_owned()),
        until: None,
    };
    assert_eq!(day_load::run(&mut tx, &later).await.unwrap(), 0);
    tx.commit().await.unwrap();

    // Beta sees none of alpha's rows, and recomputes none of them.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    let seen: i64 = sqlx::query_scalar("SELECT count(*) FROM resource_day_load")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(seen, 0);
    assert_eq!(
        day_load::run(
            &mut tx,
            &Recompute {
                scope: Scope::All,
                from: None,
                until: None
            }
        )
        .await
        .unwrap(),
        0
    );
    assert_eq!(day_load::recompute(&mut tx, &[id], None).await.unwrap(), 0);
    tx.commit().await.unwrap();
    assert_eq!(load(&db, a, nathan).await, before);
    // Nor can it write a row for alpha.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    let written = sqlx::query(
        "INSERT INTO resource_day_load (tenant_id, resource_id, day, assignment_id, issue_id, minutes, provisional) \
         VALUES ($1, $2, '2026-12-21', $3, $4, 60, false)",
    )
    .bind(a.id)
    .bind(nathan)
    .bind(id)
    .bind(issue)
    .execute(&mut *tx)
    .await;
    assert!(written.is_err());
}
