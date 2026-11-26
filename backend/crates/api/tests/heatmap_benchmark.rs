//! Read performance of the workload heatmap (L08-US06, specification §19:
//! 200 resources over 3 months in under a second): by week from the
//! precomputed sums, by week computed on the fly (nothing stored yet), and
//! by day. Each resource has a capacity period, an absence and two
//! assignments over the range. Alone in its test binary, so that cargo
//! runs it after the others. The build is unoptimized: a pass here is a
//! conservative one.

#![allow(clippy::unwrap_used)]

mod common;

use std::time::{Duration, Instant};

use common::{TestApi, json, multi};
use domain::absences::AbsenceKind;
use domain::assignments::Mode;
use domain::capacity::WeekMinutes;
use domain::hierarchy::NodeType;
use domain::resources::Kind;
use infra::absences::{self, AbsenceInput};
use infra::accounts::TenantRole;
use infra::assignments::{self, AssignmentInput, StoredSegment};
use infra::capacity::{self, PatternInput};
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::issues::{self, NewIssue};
use infra::nodes::{self, NewNode, NodeFields};
use infra::resources::{self, ResourceInput};
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::json;
use testkit::TestDb;
use time::Duration as Days;
use time::macros::date;

const RESOURCES: usize = 200;
const RUNS: usize = 20;
const TARGET: Duration = Duration::from_secs(1);

async fn seed(db: &TestDb) {
    let a = &db.tenant_a;
    let admin = db.add_member(a, TenantRole::TenantAdmin).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
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
    let mut tickets = Vec::new();
    for i in 0..2 {
        let issue = issues::create(
            &mut tx,
            &NewIssue {
                id: new_id(),
                project_id: project,
                project_key: "PLAT",
                type_id: Uuid::from_u128(1),
                status_id: Uuid::from_u128(2),
                parent_id: None,
                summary: if i == 0 { "Build" } else { "Run" },
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
        tickets.push(issue.id);
    }
    for index in 0..RESOURCES {
        let id = resources::insert(
            &mut tx,
            new_id(),
            &ResourceInput {
                kind: Kind::Generic,
                display_name: Some(&format!("Resource {index:03}")),
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
        let input = PatternInput {
            effective_from: date!(2026 - 01 - 01),
            week: WeekMinutes([420, 420, 420, 420, 210, 0, 0]),
            label: None,
        };
        capacity::insert(&mut tx, new_id(), id, &input, None)
            .await
            .unwrap()
            .unwrap();
        let offset = Days::days(i64::try_from(index % 20).unwrap());
        let input = AbsenceInput {
            kind: AbsenceKind::Leave,
            start: date!(2027 - 01 - 11) + offset,
            starts_at_noon: false,
            end: date!(2027 - 01 - 12) + offset,
            ends_at_noon: false,
            comment: None,
        };
        absences::insert(&mut tx, new_id(), id, &input, None)
            .await
            .unwrap()
            .unwrap();
        for (issue, mode, value) in [
            (tickets[0], Mode::Percentage, 50),
            (tickets[1], Mode::HoursPerDay, 120),
        ] {
            let input = AssignmentInput {
                provisional: index % 5 == 0,
                segments: vec![StoredSegment {
                    start: date!(2026 - 12 - 01),
                    end: date!(2027 - 02 - 26),
                    mode,
                    value,
                }],
            };
            assignments::insert(&mut tx, new_id(), issue, id, &input, None)
                .await
                .unwrap()
                .unwrap();
        }
    }
    tx.commit().await.unwrap();
}

/// The 95th percentile of `RUNS` reads of `path`, after a warm-up.
async fn p95(api: &TestApi, db: &TestDb, path: &str, columns: usize) -> Duration {
    for _ in 0..3 {
        let (status, heatmap) =
            json(api.request(Method::GET, &db.tenant_a, path).send().await.unwrap()).await;
        assert_eq!(status, 200, "{heatmap}");
        assert_eq!(heatmap["rows"].as_array().unwrap().len(), RESOURCES);
        assert_eq!(heatmap["rows"][0]["load"].as_array().unwrap().len(), columns);
    }
    let mut timings = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let started = Instant::now();
        let response = api.request(Method::GET, &db.tenant_a, path).send().await.unwrap();
        assert_eq!(response.status(), 200);
        response.bytes().await.unwrap();
        timings.push(started.elapsed());
    }
    timings.sort();
    let p95 = timings[RUNS * 95 / 100 - 1];
    println!("{path}: median {:?}, p95 {p95:?}", timings[RUNS / 2]);
    p95
}

#[tokio::test]
async fn the_heatmap_of_200_resources_over_3_months_reads_within_a_second() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    seed(&db).await;
    let weeks = "/api/v1/workload/heatmap?scale=week&from=2026-12-01&to=2027-02-28";
    let days = "/api/v1/workload/heatmap?scale=day&from=2026-12-01&to=2027-02-28";

    // Nothing stored yet: every week computed on the fly.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    infra::sqlx::query("DELETE FROM load_aggregate")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let computed = p95(&api, &db, weeks, 13).await;
    // Then stored by the worker.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert!(infra::load_aggregates::recompute(&mut tx).await.unwrap() > 0);
    tx.commit().await.unwrap();
    let stored = p95(&api, &db, weeks, 13).await;
    let by_day = p95(&api, &db, days, 90).await;
    for (what, p95) in [
        ("computed weeks", computed),
        ("stored weeks", stored),
        ("days", by_day),
    ] {
        assert!(p95 < TARGET, "{what}: p95 {p95:?} ≥ {TARGET:?}");
    }
}
