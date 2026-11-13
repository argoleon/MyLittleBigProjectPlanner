//! Integration tests of the hierarchy browse and of the aggregates
//! (L03-US05) on real PostgreSQL and Valkey: the rows of each level, the
//! aggregates over distinct projects with the weighting of each link, what
//! each member sees, veiled confidential projects, tenant isolation, and the
//! response time on a seeded tenant.

#![allow(clippy::unwrap_used)]

mod common;

use std::time::{Duration, Instant};

use common::{TestApi, json, multi};
use domain::aggregates::Figures;
use infra::accounts::TenantRole;
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

type Session = (String, String);

async fn send(request: reqwest::RequestBuilder) -> (u16, Value) {
    json(request.send().await.unwrap()).await
}

/// A request of the seeded tenant admin of `tenant`.
fn admin(api: &TestApi, tenant: &SeedTenant, method: Method, path: &str) -> reqwest::RequestBuilder {
    api.request(method, tenant, path)
}

/// A request of the member whose session is `session`.
fn as_member(
    api: &TestApi,
    tenant: &SeedTenant,
    session: &Session,
    method: Method,
    path: &str,
) -> reqwest::RequestBuilder {
    api.anonymous(method, tenant, path)
        .header("cookie", format!("mlbpp_session={}", session.0))
        .header("x-csrf-token", &session.1)
}

/// A new member of `tenant`, with a session opened for them.
async fn member(db: &TestDb, api: &TestApi, tenant: &SeedTenant) -> (Uuid, Session) {
    let account_id = db.add_member(tenant, TenantRole::Member).await.account_id;
    let settings = common::session_settings();
    let opened = infra::sessions::SessionStore::new(
        api.valkey.clone(),
        api.key_prefix.clone(),
        infra::sessions::SessionLifetime {
            idle: settings.idle,
            max_age: settings.max_age,
        },
    )
    .create(account_id, tenant.id)
    .await
    .unwrap();
    (account_id, (opened.token, opened.session.csrf_token))
}

/// The default role labelled `label` (English) of `tenant`.
async fn role(db: &TestDb, tenant: &SeedTenant, label: &str) -> Uuid {
    infra::sqlx::query_scalar("SELECT id FROM role WHERE tenant_id = $1 AND label ->> 'en' = $2")
        .bind(tenant.id)
        .bind(label)
        .fetch_one(&db.owner_pool)
        .await
        .unwrap()
}

/// Creates a node as the tenant admin; its id.
async fn node(api: &TestApi, tenant: &SeedTenant, body: Value) -> String {
    let (status, node) = send(admin(api, tenant, Method::POST, "/api/v1/nodes").json(&body)).await;
    assert_eq!(status, 201, "{node}");
    node["id"].as_str().unwrap().to_owned()
}

/// Grants `role` to `account` on `node`, as the tenant admin.
async fn grant(api: &TestApi, tenant: &SeedTenant, node: &str, role: Uuid, account: Uuid) {
    let path = format!("/api/v1/nodes/{node}/grants");
    let body = json!({"roleId": role, "accountId": account});
    let (status, granted) = send(admin(api, tenant, Method::POST, &path).json(&body)).await;
    assert_eq!(status, 201, "{granted}");
}

/// What tickets and workload will measure on a project (L05, L08).
async fn measure(db: &TestDb, tenant: &SeedTenant, project: &str, figures: Figures) {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let project: Uuid = project.parse().unwrap();
    assert!(
        infra::aggregates::set_figures(&mut tx, project, &figures)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
}

/// Runs the recompute of `tenant`, as the worker's job does.
async fn recompute(db: &TestDb, tenant: &SeedTenant) -> usize {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let computed = infra::aggregates::recompute(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    computed
}

/// The queued aggregate jobs of `tenant`.
async fn queued_jobs(db: &TestDb, tenant: &SeedTenant) -> i64 {
    infra::sqlx::query_scalar(
        "SELECT count(*) FROM job WHERE tenant_id = $1 AND kind = 'node.aggregates' AND status = 'queued'",
    )
    .bind(tenant.id)
    .fetch_one(&db.owner_pool)
    .await
    .unwrap()
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

/// The names of `rows`; a veiled one shows as `veiled`.
fn names(rows: &Value) -> Vec<String> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|row| {
            row["name"]
                .as_str()
                .map_or_else(|| "veiled".to_owned(), ToOwned::to_owned)
        })
        .collect()
}

fn row<'a>(rows: &'a Value, name: &str) -> &'a Value {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == name)
        .unwrap_or_else(|| panic!("no row {name} in {rows}"))
}

#[tokio::test]
async fn a_portfolio_shows_its_tree_and_aggregates_its_distinct_projects() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let digital = node(&api, t, json!({"type": "portfolio", "name": "Digital"})).await;
    assert_eq!(queued_jobs(&db, t).await, 1, "a change queues the recompute");
    let modernization = node(
        &api,
        t,
        json!({"type": "program", "name": "Modernization", "parentId": digital}),
    )
    .await;
    let compliance = node(
        &api,
        t,
        json!({"type": "program", "name": "Compliance", "parentId": digital}),
    )
    .await;
    let plat = node(
        &api,
        t,
        json!({"type": "project", "name": "Identity platform", "key": "PLAT", "parentId": modernization}),
    )
    .await;
    let portal = node(
        &api,
        t,
        json!({"type": "project", "name": "Customer portal", "key": "PORT", "parentId": modernization}),
    )
    .await;
    let secret = node(
        &api,
        t,
        json!({"type": "project", "name": "Merger", "key": "MERG", "parentId": digital}),
    )
    .await;
    let lone = node(
        &api,
        t,
        json!({"type": "project", "name": "Alone", "key": "ALONE"}),
    )
    .await;
    // The platform is also under Compliance, for a quarter of its budget.
    let path = format!("/api/v1/nodes/{plat}/parents/{compliance}");
    let (status, _) = send(admin(&api, t, Method::PUT, &path).json(&json!({"weightPercent": 25}))).await;
    assert_eq!(status, 201);
    let path = format!("/api/v1/nodes/{secret}/confidentiality");
    let (status, _) = send(
        admin(&api, t, Method::PUT, &path)
            .header("if-match", "*")
            .json(&json!({"confidential": true})),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(queued_jobs(&db, t).await, 1, "changes in a burst share one job");

    // Before the job has run: the tree is there, the figures are not.
    let (status, top) = send(admin(&api, t, Method::GET, "/api/v1/hierarchy")).await;
    assert_eq!(status, 200, "{top}");
    assert_eq!(
        names(&top),
        ["Digital", "Alone"],
        "the root nodes, portfolios first"
    );
    assert_eq!(row(&top, "Digital")["aggregate"]["projects"], 0);

    measure(&db, t, &plat, figures(600, 42, 58, 1)).await;
    measure(&db, t, &portal, figures(900, 71, 29, 3)).await;
    measure(&db, t, &secret, figures(300, 38, 2, 2)).await;
    measure(&db, t, &lone, figures(60, 1, 0, 0)).await;
    assert_eq!(recompute(&db, t).await, 7);

    // The portfolio: three distinct projects, the platform once and whole.
    let (status, aggregate) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/nodes/{digital}/aggregate"),
    ))
    .await;
    assert_eq!(status, 200, "{aggregate}");
    assert_eq!(
        aggregate,
        json!({
            "projects": 3,
            "confidentialProjects": 1,
            "plannedMinutes": 1800,
            "capacityMinutes": 3600,
            "capacityUsedPercent": 50,
            "openTickets": 151,
            "lateTickets": 6,
            "progressPercent": 37,
            "confidentialOpenTickets": 38,
        })
    );

    // Its rows: programs, then projects, each with its own aggregate.
    let under_digital = format!("/api/v1/hierarchy?under={digital}");
    let (status, rows) = send(admin(&api, t, Method::GET, &under_digital)).await;
    assert_eq!(status, 200, "{rows}");
    assert_eq!(names(&rows), ["Compliance", "Modernization", "Merger"]);
    let program = row(&rows, "Compliance");
    assert_eq!(
        (
            &program["type"],
            &program["hasChildren"],
            &program["weightPercent"]
        ),
        (&json!("program"), &json!(true), &json!(100))
    );
    assert_eq!(program["health"], "on_track");
    assert_eq!(program["otherParents"], json!([]));
    // Compliance weighs the platform at 25 %; its tickets stay whole.
    assert_eq!(program["aggregate"]["plannedMinutes"], 150);
    assert_eq!(program["aggregate"]["openTickets"], 42);
    assert_eq!(row(&rows, "Modernization")["aggregate"]["plannedMinutes"], 1500);
    let merger = row(&rows, "Merger");
    assert_eq!(
        (&merger["key"], &merger["hasChildren"]),
        (&json!("MERG"), &json!(false))
    );
    assert_eq!(merger["aggregate"]["confidentialProjects"], 1);

    // The next level, with the other parents of each project.
    let (_, rows) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/hierarchy?under={modernization}"),
    ))
    .await;
    assert_eq!(names(&rows), ["Customer portal", "Identity platform"]);
    let platform = row(&rows, "Identity platform");
    assert_eq!(platform["id"], json!(plat));
    assert_eq!(
        platform["otherParents"],
        json!([{"id": compliance, "type": "program", "name": "Compliance"}])
    );
    assert_eq!(
        platform["aggregate"]["plannedMinutes"], 600,
        "its own load, unweighted"
    );
    assert_eq!(platform["aggregate"]["progressPercent"], 58);
    let (_, rows) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/hierarchy?under={lone}"),
    ))
    .await;
    assert_eq!(rows, json!([]), "no child: the page invites to add one");

    // Lucas reads the portfolio: he sees its totals, confidential project
    // included, and that project veiled.
    let (lucas, lucas_session) = member(&db, &api, t).await;
    let reader = role(&db, t, "Reader").await;
    grant(&api, t, &digital, reader, lucas).await;
    let get = |session: &Session, path: &str| as_member(&api, t, session, Method::GET, path);
    let (status, top) = send(get(&lucas_session, "/api/v1/hierarchy")).await;
    assert_eq!(status, 200, "{top}");
    assert_eq!(names(&top), ["Digital"], "not the project he has no role on");
    assert_eq!(row(&top, "Digital")["aggregate"], aggregate);
    let (_, rows) = send(get(&lucas_session, &under_digital)).await;
    assert_eq!(names(&rows), ["Compliance", "Modernization", "veiled"]);
    let veiled = &rows[2];
    let mut fields: Vec<&String> = veiled.as_object().unwrap().keys().collect();
    fields.sort();
    assert_eq!(
        fields,
        ["id", "plannedMinutes", "type", "veiled", "weightPercent"],
        "{veiled}"
    );
    assert_eq!(veiled["plannedMinutes"], 300);
    assert_ne!(veiled["id"], json!(secret));
    let text = rows.to_string();
    assert!(!text.contains("Merger") && !text.contains("MERG") && !text.contains(&secret));
    let (status, _) = send(get(&lucas_session, &format!("/api/v1/nodes/{secret}/aggregate"))).await;
    assert_eq!(status, 404);
    let (status, _) = send(get(&lucas_session, &format!("/api/v1/hierarchy?under={secret}"))).await;
    assert_eq!(status, 404);

    // Inès reads Compliance only: it is the top of what she sees.
    let (ines, ines_session) = member(&db, &api, t).await;
    grant(&api, t, &compliance, reader, ines).await;
    let (_, top) = send(get(&ines_session, "/api/v1/hierarchy")).await;
    assert_eq!(names(&top), ["Compliance"]);
    assert_eq!(row(&top, "Compliance")["weightPercent"], json!(null));
    assert_eq!(row(&top, "Compliance")["otherParents"], json!([]));
    let (_, rows) = send(get(
        &ines_session,
        &format!("/api/v1/hierarchy?under={compliance}"),
    ))
    .await;
    assert_eq!(names(&rows), ["Identity platform"]);
    assert_eq!(rows[0]["weightPercent"], 25);
    assert_eq!(
        rows[0]["otherParents"],
        json!([]),
        "Modernization is not hers to see"
    );
    for path in [
        under_digital.clone(),
        format!("/api/v1/nodes/{digital}/aggregate"),
        format!("/api/v1/hierarchy?under={modernization}"),
    ] {
        let (status, problem) = send(get(&ines_session, &path)).await;
        assert_eq!((status, &problem["code"]), (404, &json!("not_found")), "{path}");
    }
    let (status, own) = send(get(
        &ines_session,
        &format!("/api/v1/nodes/{compliance}/aggregate"),
    ))
    .await;
    assert_eq!((status, &own["plannedMinutes"]), (200, &json!(150)));

    // A member without any role finds an empty hierarchy.
    let (_, nobody_session) = member(&db, &api, t).await;
    let (status, top) = send(get(&nobody_session, "/api/v1/hierarchy")).await;
    assert_eq!((status, top), (200, json!([])));

    // Archived projects leave the rows and, once recomputed, the aggregates.
    infra::sqlx::query("UPDATE node SET archived_at = now() WHERE id = $1::uuid")
        .bind(&portal)
        .execute(&db.owner_pool)
        .await
        .unwrap();
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, t.id).await.unwrap();
    infra::aggregates::mark_stale(&mut tx, &[portal.parse().unwrap()])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    recompute(&db, t).await;
    let (_, rows) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/hierarchy?under={modernization}"),
    ))
    .await;
    assert_eq!(names(&rows), ["Identity platform"]);
    let (_, aggregate) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/nodes/{digital}/aggregate"),
    ))
    .await;
    assert_eq!(
        (&aggregate["projects"], &aggregate["plannedMinutes"]),
        (&json!(2), &json!(900))
    );

    // Requests that cannot be served.
    let (status, problem) = send(admin(&api, t, Method::GET, "/api/v1/hierarchy?under=nope")).await;
    assert_eq!(status, 400, "{problem}");
    let res = api
        .anonymous(Method::GET, t, "/api/v1/hierarchy")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 401);
    let unknown = format!("/api/v1/nodes/{}/aggregate", Uuid::nil());
    let (status, _) = send(admin(&api, t, Method::GET, &unknown)).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn the_hierarchy_and_its_aggregates_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let digital = node(&api, a, json!({"type": "portfolio", "name": "Digital"})).await;
    let plat = node(
        &api,
        a,
        json!({"type": "project", "name": "Identity platform", "key": "PLAT", "parentId": digital}),
    )
    .await;
    measure(&db, a, &plat, figures(600, 42, 58, 1)).await;
    recompute(&db, a).await;
    let other = node(&api, b, json!({"type": "portfolio", "name": "Elsewhere"})).await;
    assert_eq!(recompute(&db, b).await, 1, "each tenant recomputes its own nodes");

    let (_, top) = send(admin(&api, b, Method::GET, "/api/v1/hierarchy")).await;
    assert_eq!(names(&top), ["Elsewhere"]);
    for path in [
        format!("/api/v1/hierarchy?under={digital}"),
        format!("/api/v1/nodes/{digital}/aggregate"),
        format!("/api/v1/nodes/{plat}/aggregate"),
    ] {
        let (status, problem) = send(admin(&api, b, Method::GET, &path)).await;
        assert_eq!((status, &problem["code"]), (404, &json!("not_found")), "{path}");
    }
    let (_, top) = send(admin(&api, a, Method::GET, "/api/v1/hierarchy")).await;
    assert_eq!(names(&top), ["Digital"]);
    assert_eq!(row(&top, "Digital")["aggregate"]["plannedMinutes"], 600);
    let (status, _) = send(admin(
        &api,
        a,
        Method::GET,
        &format!("/api/v1/hierarchy?under={other}"),
    ))
    .await;
    assert_eq!(status, 404);
}

/// The 95th percentile of `times`.
fn p95(mut times: Vec<Duration>) -> Duration {
    times.sort();
    let rank = (times.len() * 95).div_ceil(100);
    times[rank.saturating_sub(1)]
}

/// One portfolio of 20 programs of 50 projects: 1,000 projects.
async fn seed_volume(db: &TestDb, tenant: &SeedTenant, portfolio: &str) -> Vec<Uuid> {
    let portfolio: Uuid = portfolio.parse().unwrap();
    let ids = |count: usize| (0..count).map(|_| infra::id::new_id()).collect::<Vec<Uuid>>();
    let (programs, projects) = (ids(20), ids(1000));
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    infra::sqlx::query(
        "INSERT INTO node (id, tenant_id, node_type, name)
         SELECT id, app_current_tenant(), 'program', 'Program ' || lpad(rank::text, 2, '0')
         FROM unnest($1::uuid[]) WITH ORDINALITY AS n (id, rank)",
    )
    .bind(&programs)
    .execute(&mut *tx)
    .await
    .unwrap();
    infra::sqlx::query(
        "INSERT INTO node (id, tenant_id, node_type, name, key)
         SELECT id, app_current_tenant(), 'project', 'Project ' || rank, 'P' || rank
         FROM unnest($1::uuid[]) WITH ORDINALITY AS n (id, rank)",
    )
    .bind(&projects)
    .execute(&mut *tx)
    .await
    .unwrap();
    let mut parents = vec![portfolio; programs.len()];
    let mut children = programs.clone();
    for (index, project) in projects.iter().enumerate() {
        parents.push(programs[index / 50]);
        children.push(*project);
    }
    infra::sqlx::query(
        "INSERT INTO node_edge (id, tenant_id, parent_id, child_id)
         SELECT uuidv7(), app_current_tenant(), parent, child
         FROM unnest($1::uuid[], $2::uuid[]) AS e (parent, child)",
    )
    .bind(&parents)
    .bind(&children)
    .execute(&mut *tx)
    .await
    .unwrap();
    infra::sqlx::query(
        "INSERT INTO project_figure (tenant_id, node_id, planned_minutes, tickets_todo, tickets_done)
         SELECT app_current_tenant(), id, 600, 30, 10 FROM unnest($1::uuid[]) AS p (id)",
    )
    .bind(&projects)
    .execute(&mut *tx)
    .await
    .unwrap();
    infra::aggregates::mark_all_stale(&mut tx).await.unwrap();
    infra::aggregates::recompute(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    programs
}

#[tokio::test]
async fn browsing_a_thousand_projects_answers_within_200_ms_at_the_95th_percentile() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let portfolio = node(&api, t, json!({"type": "portfolio", "name": "Digital"})).await;
    let programs = seed_volume(&db, t, &portfolio).await;
    // A member's requests also compute what they see of the hierarchy.
    let (lucas, session) = member(&db, &api, t).await;
    grant(&api, t, &portfolio, role(&db, t, "Reader").await, lucas).await;

    let paths = [
        "/api/v1/hierarchy".to_owned(),
        format!("/api/v1/hierarchy?under={portfolio}"),
        format!("/api/v1/hierarchy?under={}", programs[7]),
        format!("/api/v1/nodes/{portfolio}/aggregate"),
    ];
    let (_, rows) = send(as_member(&api, t, &session, Method::GET, &paths[2])).await;
    assert_eq!(rows.as_array().unwrap().len(), 50);
    let (_, aggregate) = send(as_member(&api, t, &session, Method::GET, &paths[3])).await;
    assert_eq!(
        (&aggregate["projects"], &aggregate["plannedMinutes"]),
        (&json!(1000), &json!(600_000))
    );

    for path in &paths {
        for (who, as_admin) in [("admin", true), ("member", false)] {
            let mut times = Vec::new();
            for _ in 0..40 {
                let request = if as_admin {
                    admin(&api, t, Method::GET, path)
                } else {
                    as_member(&api, t, &session, Method::GET, path)
                };
                let started = Instant::now();
                let res = request.send().await.unwrap();
                assert_eq!(res.status().as_u16(), 200);
                res.bytes().await.unwrap();
                times.push(started.elapsed());
            }
            let p95 = p95(times);
            println!("{who} GET {path}: p95 {p95:?}");
            assert!(p95 < Duration::from_millis(200), "{who} GET {path}: p95 {p95:?}");
        }
    }
}
