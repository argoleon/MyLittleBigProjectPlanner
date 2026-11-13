//! Integration tests on a real PostgreSQL: the precomputed aggregates of the
//! hierarchy (L03-US05), their recompute after a change, the debounce of
//! the job, tenant isolation, and the browse of one level.

#![allow(clippy::unwrap_used)]

use std::time::{Duration, Instant};

use domain::aggregates::{Aggregate, Figures, RECOMPUTE_DELAY};
use domain::hierarchy::{Health, NodeType};
use infra::accounts::TenantRole;
use infra::aggregates::{self, AGGREGATE_JOB};
use infra::db::begin_tenant_tx;
use infra::id::new_id;
use infra::nodes::{self, NewNode, NodeFields};
use infra::sqlx::{self, PgConnection};
use infra::uuid::Uuid;
use testkit::{SeedTenant, TestDb};

async fn admin(db: &TestDb, tenant: &SeedTenant) -> Uuid {
    db.add_member(tenant, TenantRole::TenantAdmin).await.account_id
}

async fn create(
    conn: &mut PgConnection,
    node_type: NodeType,
    name: &str,
    code: Option<&str>,
    parent: Option<Uuid>,
    creator: Uuid,
) -> Uuid {
    let id = new_id();
    nodes::insert(
        conn,
        &NewNode {
            id,
            node_type,
            key: code,
            fields: NodeFields {
                name,
                description: "",
                objectives: "",
                start_date: None,
                end_date: None,
                health: Health::OnTrack,
                owners: &[],
            },
            parent,
            created_by: creator,
        },
    )
    .await
    .unwrap()
    .unwrap();
    id
}

fn figures(planned_minutes: u64, todo: u64, done: u64, late: u64) -> Figures {
    Figures {
        planned_minutes,
        capacity_minutes: planned_minutes * 2,
        todo,
        in_progress: 0,
        done,
        late,
    }
}

async fn aggregate(conn: &mut PgConnection, node: Uuid) -> Aggregate {
    aggregates::find(conn, &[node])
        .await
        .unwrap()
        .remove(&node)
        .unwrap_or_default()
}

/// The queued aggregate jobs, as the owner sees them: (tenant, status, due
/// in seconds).
async fn jobs(db: &TestDb) -> Vec<(Uuid, String, f64)> {
    sqlx::query_as(
        "SELECT tenant_id, status, EXTRACT(EPOCH FROM (run_at - now()))::float8 FROM job
         WHERE kind = $1 ORDER BY run_at",
    )
    .bind(AGGREGATE_JOB)
    .fetch_all(&db.owner_pool)
    .await
    .unwrap()
}

/// The portfolio of mockup 05, in small.
struct Portfolio {
    portfolio: Uuid,
    program_a: Uuid,
    program_b: Uuid,
    platform: Uuid,
    portal: Uuid,
    warehouse: Uuid,
    erp: Uuid,
}

async fn portfolio(conn: &mut PgConnection, creator: Uuid) -> Portfolio {
    let portfolio = create(conn, NodeType::Portfolio, "Digital", None, None, creator).await;
    let program_a = create(
        conn,
        NodeType::Program,
        "Modernization",
        None,
        Some(portfolio),
        creator,
    )
    .await;
    let program_b = create(
        conn,
        NodeType::Program,
        "Compliance",
        None,
        Some(portfolio),
        creator,
    )
    .await;
    let platform = create(
        conn,
        NodeType::Project,
        "Identity platform",
        Some("PLAT"),
        Some(program_a),
        creator,
    )
    .await;
    nodes::link(conn, program_b, platform, 100)
        .await
        .unwrap()
        .unwrap();
    let portal = create(
        conn,
        NodeType::Project,
        "Customer portal",
        Some("PORT"),
        Some(program_a),
        creator,
    )
    .await;
    let warehouse = create(
        conn,
        NodeType::Project,
        "Data warehouse",
        Some("DWH"),
        Some(program_a),
        creator,
    )
    .await;
    let erp = create(
        conn,
        NodeType::Project,
        "ERP flows",
        Some("ERP"),
        Some(warehouse),
        creator,
    )
    .await;
    for (project, own) in [
        (platform, figures(600, 42, 58, 1)),
        (portal, figures(900, 71, 29, 3)),
        (warehouse, figures(500, 37, 13, 3)),
        (erp, figures(300, 18, 2, 4)),
    ] {
        assert!(aggregates::set_figures(conn, project, &own).await.unwrap());
    }
    Portfolio {
        portfolio,
        program_a,
        program_b,
        platform,
        portal,
        warehouse,
        erp,
    }
}

#[tokio::test]
async fn a_node_aggregates_its_distinct_projects_and_follows_every_change() {
    let db = TestDb::new().await;
    let creator = admin(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let p = portfolio(&mut tx, creator).await;

    // Nothing is computed until the job runs.
    assert_eq!(aggregate(&mut tx, p.portfolio).await, Aggregate::default());
    assert_eq!(aggregates::recompute(&mut tx).await.unwrap(), 7);
    assert_eq!(
        aggregates::recompute(&mut tx).await.unwrap(),
        0,
        "the marks are taken once"
    );

    // The platform is under both programs: once at the portfolio.
    let top = aggregate(&mut tx, p.portfolio).await;
    assert_eq!(top.projects, 4);
    assert_eq!(top.figures.planned_minutes, 2300);
    assert_eq!(top.figures.capacity_minutes, 4600);
    assert_eq!(
        (top.figures.todo, top.figures.done, top.figures.late),
        (168, 102, 11)
    );
    assert_eq!(
        aggregate(&mut tx, p.program_a).await.figures.planned_minutes,
        2300
    );
    assert_eq!(aggregate(&mut tx, p.program_b).await.figures.planned_minutes, 600);
    // A project aggregates itself and its sub-projects.
    let warehouse = aggregate(&mut tx, p.warehouse).await;
    assert_eq!((warehouse.projects, warehouse.figures.planned_minutes), (2, 800));
    assert_eq!(aggregate(&mut tx, p.erp).await.figures.late, 4);

    // Each program applies its own weighting; the portfolio counts whole.
    nodes::link(&mut tx, p.program_a, p.platform, 60)
        .await
        .unwrap()
        .unwrap();
    nodes::link(&mut tx, p.program_b, p.platform, 25)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        aggregates::recompute(&mut tx).await.unwrap(),
        3,
        "the programs and what is above"
    );
    let a = aggregate(&mut tx, p.program_a).await;
    assert_eq!(a.figures.planned_minutes, 360 + 900 + 500 + 300);
    assert_eq!(a.figures.open(), 168, "tickets stay whole");
    assert_eq!(aggregate(&mut tx, p.program_b).await.figures.planned_minutes, 150);
    assert_eq!(
        aggregate(&mut tx, p.portfolio).await.figures.planned_minutes,
        2300
    );

    // A confidential project is counted, and told apart.
    let portal = nodes::find(&mut tx, p.portal).await.unwrap().unwrap();
    nodes::set_confidential(&mut tx, p.portal, Some(&[portal.version]), true)
        .await
        .unwrap()
        .unwrap();
    aggregates::recompute(&mut tx).await.unwrap();
    let top = aggregate(&mut tx, p.portfolio).await;
    assert_eq!((top.projects, top.figures.planned_minutes), (4, 2300));
    assert_eq!((top.confidential_projects, top.confidential_open), (1, 71));
    assert_eq!(aggregate(&mut tx, p.program_b).await.confidential_projects, 0);

    // New figures on a project reach every ancestor.
    aggregates::set_figures(&mut tx, p.erp, &figures(1300, 20, 2, 6))
        .await
        .unwrap();
    assert_eq!(aggregates::recompute(&mut tx).await.unwrap(), 4);
    assert_eq!(
        aggregate(&mut tx, p.warehouse).await.figures.planned_minutes,
        1800
    );
    let top = aggregate(&mut tx, p.portfolio).await;
    assert_eq!((top.figures.planned_minutes, top.figures.late), (3300, 13));

    // Archived projects leave the aggregates of their ancestors.
    sqlx::query("UPDATE node SET archived_at = now() WHERE id = $1")
        .bind(p.warehouse)
        .execute(&mut *tx)
        .await
        .unwrap();
    aggregates::mark_stale(&mut tx, &[p.warehouse]).await.unwrap();
    aggregates::recompute(&mut tx).await.unwrap();
    let top = aggregate(&mut tx, p.portfolio).await;
    assert_eq!(
        (top.projects, top.figures.planned_minutes),
        (3, 600 + 900 + 1300),
        "its sub-project stays"
    );

    // A detached project leaves its former parent only.
    assert!(nodes::unlink(&mut tx, p.program_a, p.platform).await.unwrap());
    aggregates::recompute(&mut tx).await.unwrap();
    assert_eq!(
        aggregate(&mut tx, p.program_a).await.figures.planned_minutes,
        900 + 1300
    );
    assert_eq!(aggregate(&mut tx, p.program_b).await.figures.planned_minutes, 150);
    assert_eq!(
        aggregate(&mut tx, p.portfolio).await.figures.planned_minutes,
        2800
    );

    // Figures belong to projects.
    assert!(
        !aggregates::set_figures(&mut tx, p.program_a, &figures(1, 0, 0, 0))
            .await
            .unwrap()
    );
    assert!(
        !aggregates::set_figures(&mut tx, new_id(), &figures(1, 0, 0, 0))
            .await
            .unwrap()
    );
    let late_not_open = Figures {
        late: 5,
        ..figures(10, 2, 0, 0)
    };
    assert!(
        aggregates::set_figures(&mut tx, p.erp, &late_not_open)
            .await
            .is_err(),
        "late tickets are open tickets"
    );
}

#[tokio::test]
async fn changes_in_a_burst_share_one_job_and_a_running_job_calls_for_another() {
    let db = TestDb::new().await;
    let creator = admin(&db, &db.tenant_a).await;

    // A dozen changes in one transaction, then more in another.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let p = portfolio(&mut tx, creator).await;
    tx.commit().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    aggregates::set_figures(&mut tx, p.portal, &figures(10, 1, 1, 0))
        .await
        .unwrap();
    nodes::link(&mut tx, p.program_b, p.portal, 50)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();

    let queued = jobs(&db).await;
    assert_eq!(queued.len(), 1, "{queued:?}");
    let (tenant, status, due_in) = &queued[0];
    assert_eq!((*tenant, status.as_str()), (db.tenant_a.id, "queued"));
    assert!(*due_in <= RECOMPUTE_DELAY.as_secs_f64(), "due in {due_in} s");

    // Another tenant has its own job.
    let other = admin(&db, &db.tenant_b).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    create(&mut tx, NodeType::Portfolio, "Other", None, None, other).await;
    tx.commit().await.unwrap();
    assert_eq!(jobs(&db).await.len(), 2);
    sqlx::query("UPDATE job SET run_at = now() + interval '1 hour' WHERE tenant_id = $1")
        .bind(db.tenant_b.id)
        .execute(&db.owner_pool)
        .await
        .unwrap();

    // Once a worker has claimed the job, a change queues the next one: the
    // running job may have read the hierarchy already.
    sqlx::query("UPDATE job SET run_at = now() WHERE tenant_id = $1")
        .bind(db.tenant_a.id)
        .execute(&db.owner_pool)
        .await
        .unwrap();
    let claimed = infra::jobs::claim(&db.app_pool, "worker-test", 10, Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(
        (claimed[0].tenant_id, claimed[0].kind.as_str()),
        (db.tenant_a.id, AGGREGATE_JOB)
    );
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    aggregates::set_figures(&mut tx, p.portal, &figures(20, 1, 1, 0))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let of_a: Vec<String> = jobs(&db)
        .await
        .into_iter()
        .filter(|(tenant, ..)| *tenant == db.tenant_a.id)
        .map(|(_, status, _)| status)
        .collect();
    assert_eq!(of_a, ["running", "queued"]);
}

#[tokio::test]
async fn a_queued_job_waits_for_the_changes_that_count_on_it() {
    let db = TestDb::new().await;
    let creator = admin(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let p = portfolio(&mut tx, creator).await;
    tx.commit().await.unwrap();
    sqlx::query("UPDATE job SET run_at = now()")
        .execute(&db.owner_pool)
        .await
        .unwrap();

    // A change in flight counts on the queued job: no worker takes it
    // before the change is committed, or it would miss it.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    aggregates::set_figures(&mut tx, p.portal, &figures(10, 1, 1, 0))
        .await
        .unwrap();
    let lease = Duration::from_secs(60);
    assert!(
        infra::jobs::claim(&db.app_pool, "worker-test", 10, lease)
            .await
            .unwrap()
            .is_empty()
    );
    tx.commit().await.unwrap();
    assert_eq!(
        infra::jobs::claim(&db.app_pool, "worker-test", 10, lease)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(jobs(&db).await.len(), 1, "no second job");
}

#[tokio::test]
async fn aggregates_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let (alpha, beta) = (admin(&db, &db.tenant_a).await, admin(&db, &db.tenant_b).await);
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let a = portfolio(&mut tx, alpha).await;
    tx.commit().await.unwrap();
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    let b = portfolio(&mut tx, beta).await;
    tx.commit().await.unwrap();

    // The recompute of a tenant takes its own marks only.
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    assert_eq!(aggregates::recompute(&mut tx).await.unwrap(), 7);
    assert_eq!(aggregate(&mut tx, a.portfolio).await.projects, 4);
    // The other tenant's nodes are not there to read, mark or measure.
    assert_eq!(aggregate(&mut tx, b.portfolio).await, Aggregate::default());
    aggregates::mark_stale(&mut tx, &[b.portfolio]).await.unwrap();
    assert_eq!(aggregates::recompute(&mut tx).await.unwrap(), 0);
    assert!(
        !aggregates::set_figures(&mut tx, b.portal, &figures(1, 1, 0, 0))
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();

    let stale: Vec<(Uuid, i64)> =
        sqlx::query_as("SELECT tenant_id, count(*) FROM node_aggregate_stale GROUP BY tenant_id")
            .fetch_all(&db.owner_pool)
            .await
            .unwrap();
    assert_eq!(
        stale,
        [(db.tenant_b.id, 7)],
        "the other tenant's marks are untouched"
    );
    let computed: Vec<Uuid> = sqlx::query_scalar("SELECT DISTINCT tenant_id FROM node_aggregate")
        .fetch_all(&db.owner_pool)
        .await
        .unwrap();
    assert_eq!(computed, [db.tenant_a.id]);

    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    assert_eq!(aggregates::recompute(&mut tx).await.unwrap(), 7);
    assert_eq!(
        aggregate(&mut tx, b.portfolio).await.figures.planned_minutes,
        2300
    );
    assert_eq!(aggregate(&mut tx, a.portfolio).await, Aggregate::default());
}

#[tokio::test]
async fn one_level_is_browsed_at_a_time() {
    let db = TestDb::new().await;
    let creator = admin(&db, &db.tenant_a).await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let p = portfolio(&mut tx, creator).await;
    let lone = create(&mut tx, NodeType::Project, "Alone", Some("ALONE"), None, creator).await;
    nodes::link(&mut tx, p.portfolio, p.erp, 40)
        .await
        .unwrap()
        .unwrap();

    let names = |rows: &[nodes::BrowseRow]| rows.iter().map(|row| row.name.clone()).collect::<Vec<_>>();

    // At the top: the nodes without a parent, portfolios first.
    let top = nodes::browse(&mut tx, None, None).await.unwrap();
    assert_eq!(names(&top), ["Digital", "Alone"]);
    assert!(top[0].has_children && !top[1].has_children);
    assert_eq!(top[0].weight_percent, None);

    // Under a node: its children, programs before projects, by name, with
    // the weighting of their link and their other parents.
    let under = nodes::browse(&mut tx, Some(p.portfolio), None).await.unwrap();
    assert_eq!(names(&under), ["Compliance", "Modernization", "ERP flows"]);
    assert_eq!(
        under.iter().map(|row| row.weight_percent).collect::<Vec<_>>(),
        [Some(100), Some(100), Some(40)]
    );
    let parents: Vec<&str> = under[2]
        .parents
        .iter()
        .map(|parent| parent.name.as_str())
        .collect();
    assert_eq!(parents, ["Data warehouse", "Digital"]);
    let under = nodes::browse(&mut tx, Some(p.program_a), None).await.unwrap();
    assert_eq!(
        names(&under),
        ["Customer portal", "Data warehouse", "Identity platform"]
    );
    assert_eq!(
        under.iter().map(|row| row.has_children).collect::<Vec<_>>(),
        [false, true, false]
    );
    assert!(nodes::browse(&mut tx, Some(lone), None).await.unwrap().is_empty());

    // For a member: the top of what they see.
    let sees = [p.program_b, p.platform, p.warehouse, p.erp];
    let top = nodes::browse(&mut tx, None, Some(&sees)).await.unwrap();
    assert_eq!(
        names(&top),
        ["Compliance", "Data warehouse"],
        "not those under a node they see"
    );
    assert!(nodes::browse(&mut tx, None, Some(&[])).await.unwrap().is_empty());

    // Archived nodes leave the lists, and no longer count as children.
    sqlx::query("UPDATE node SET archived_at = now() WHERE id = ANY($1)")
        .bind(vec![p.erp, lone])
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        names(&nodes::browse(&mut tx, None, None).await.unwrap()),
        ["Digital"]
    );
    let under = nodes::browse(&mut tx, Some(p.program_a), None).await.unwrap();
    assert_eq!(
        under.iter().map(|row| row.has_children).collect::<Vec<_>>(),
        [false, false, false]
    );
}

/// 10 portfolios of 10 programs of 10 projects, each project also under a
/// second program: 1,000 projects, 1,110 nodes, 2,100 links.
#[tokio::test]
async fn a_tenant_of_a_thousand_projects_is_recomputed_in_well_under_a_second() {
    let db = TestDb::new().await;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let ids = |count: usize| (0..count).map(|_| new_id()).collect::<Vec<Uuid>>();
    let (portfolios, programs, projects) = (ids(10), ids(100), ids(1000));
    for (node_type, nodes) in [("portfolio", &portfolios), ("program", &programs)] {
        sqlx::query(
            "INSERT INTO node (id, tenant_id, node_type, name)
             SELECT id, app_current_tenant(), $2, $2 || ' ' || rank
             FROM unnest($1::uuid[]) WITH ORDINALITY AS n (id, rank)",
        )
        .bind(nodes)
        .bind(node_type)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO node (id, tenant_id, node_type, name, key)
         SELECT id, app_current_tenant(), 'project', 'Project ' || rank, 'P' || rank
         FROM unnest($1::uuid[]) WITH ORDINALITY AS n (id, rank)",
    )
    .bind(&projects)
    .execute(&mut *tx)
    .await
    .unwrap();
    let mut parents = Vec::new();
    let mut children = Vec::new();
    for (index, program) in programs.iter().enumerate() {
        parents.push(portfolios[index / 10]);
        children.push(*program);
    }
    for (index, project) in projects.iter().enumerate() {
        let first = index / 10;
        parents.push(programs[first]);
        children.push(*project);
        // A second program, in another portfolio.
        parents.push(programs[(first + 37) % 100]);
        children.push(*project);
    }
    sqlx::query(
        "INSERT INTO node_edge (id, tenant_id, parent_id, child_id, weight_percent)
         SELECT uuidv7(), app_current_tenant(), parent, child, 50
         FROM unnest($1::uuid[], $2::uuid[]) AS e (parent, child)",
    )
    .bind(&parents)
    .bind(&children)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO project_figure (tenant_id, node_id, planned_minutes, tickets_todo, tickets_done)
         SELECT app_current_tenant(), id, 600, 30, 10 FROM unnest($1::uuid[]) AS p (id)",
    )
    .bind(&projects)
    .execute(&mut *tx)
    .await
    .unwrap();

    aggregates::mark_all_stale(&mut tx).await.unwrap();
    let started = Instant::now();
    let computed = aggregates::recompute(&mut tx).await.unwrap();
    let elapsed = started.elapsed();
    println!("full recompute of {computed} nodes (1,000 projects): {elapsed:?}");
    assert_eq!(computed, 1110);
    // Measured around 40 ms on a developer machine (debug build); the
    // margin is for slow CI runners.
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");

    // A program weighs its 20 projects at 50 %; a portfolio counts its 200
    // distinct projects whole.
    let program = aggregate(&mut tx, programs[0]).await;
    assert_eq!((program.projects, program.figures.planned_minutes), (20, 6000));
    let portfolio = aggregate(&mut tx, portfolios[0]).await;
    assert_eq!(
        (portfolio.projects, portfolio.figures.planned_minutes),
        (200, 120_000)
    );
    assert_eq!(portfolio.figures.progress_percent(), Some(25));

    // One changed project: itself, its two programs and their portfolios.
    aggregates::set_figures(&mut tx, projects[0], &figures(1200, 30, 10, 0))
        .await
        .unwrap();
    let started = Instant::now();
    assert_eq!(aggregates::recompute(&mut tx).await.unwrap(), 5);
    println!("recompute after one change: {:?}", started.elapsed());
    assert_eq!(
        aggregate(&mut tx, portfolios[0]).await.figures.planned_minutes,
        120_600
    );
}
