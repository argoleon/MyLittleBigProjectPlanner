//! Integration tests of the archiving of projects (L03-US06) on real
//! PostgreSQL and Valkey: who archives and restores, the read-only guard on
//! every write, the archived list, the browse and the aggregates, the order
//! of sub-projects, and tenant isolation.

#![allow(clippy::unwrap_used)]

mod common;

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

fn assert_problem(status: u16, body: &Value, expected_status: u16, code: &str) {
    assert_eq!(status, expected_status, "{body}");
    assert_eq!(body["code"], code, "{body}");
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

/// Grants `role` to `account` on `node`, as the tenant admin.
async fn grant(api: &TestApi, tenant: &SeedTenant, node: &str, role: Uuid, account: Uuid) {
    let path = format!("/api/v1/nodes/{node}/grants");
    let body = json!({"roleId": role, "accountId": account});
    let (status, granted) = send(admin(api, tenant, Method::POST, &path).json(&body)).await;
    assert_eq!(status, 201, "{granted}");
}

/// Creates a node as the tenant admin; its id.
async fn node(api: &TestApi, tenant: &SeedTenant, body: Value) -> String {
    let (status, node) = send(admin(api, tenant, Method::POST, "/api/v1/nodes").json(&body)).await;
    assert_eq!(status, 201, "{node}");
    node["id"].as_str().unwrap().to_owned()
}

fn archiving(api: &TestApi, tenant: &SeedTenant, node: &str, archived: bool) -> reqwest::RequestBuilder {
    admin(
        api,
        tenant,
        Method::PUT,
        &format!("/api/v1/nodes/{node}/archiving"),
    )
    .header("if-match", "*")
    .json(&json!({"archived": archived}))
}

/// The names of a page of nodes.
fn names(page: &Value) -> Vec<&str> {
    let mut names: Vec<&str> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    names
}

/// Runs the recompute of `tenant`, as the worker's job does.
async fn recompute(db: &TestDb, tenant: &SeedTenant) {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    infra::aggregates::recompute(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn an_archived_project_is_read_only_and_out_of_the_active_lists_until_restored() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let program = node(&api, t, json!({"type": "program", "name": "IT Modernization"})).await;
    let compliance = node(&api, t, json!({"type": "program", "name": "Compliance"})).await;
    let plat = node(
        &api,
        t,
        json!({"type": "project", "name": "Identity platform", "key": "PLAT", "parentId": program}),
    )
    .await;
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, t.id).await.unwrap();
    let figures = Figures {
        planned_minutes: 600,
        todo: 12,
        ..Figures::default()
    };
    infra::aggregates::set_figures(&mut tx, plat.parse().unwrap(), &figures)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    recompute(&db, t).await;
    let aggregate_path = format!("/api/v1/nodes/{program}/aggregate");
    let (_, aggregate) = send(admin(&api, t, Method::GET, &aggregate_path)).await;
    assert_eq!(aggregate["projects"], 1);

    // Who archives: those who administer it; only projects; with If-Match.
    let (reader_id, reader_session) = member(&db, &api, t).await;
    let (manager_id, manager_session) = member(&db, &api, t).await;
    grant(&api, t, &program, role(&db, t, "Reader").await, reader_id).await;
    grant(&api, t, &plat, role(&db, t, "Node admin").await, manager_id).await;
    let path = format!("/api/v1/nodes/{plat}/archiving");
    let (status, problem) = send(
        as_member(&api, t, &reader_session, Method::PUT, &path)
            .header("if-match", "*")
            .json(&json!({"archived": true})),
    )
    .await;
    assert_problem(status, &problem, 403, "forbidden");
    let (status, problem) = send(archiving(&api, t, &program, true)).await;
    assert_problem(status, &problem, 400, "validation_failed");
    assert_eq!(problem["errors"][0]["field"], "archived");
    assert_eq!(problem["errors"][0]["code"], "not_allowed");
    let (status, problem) = send(admin(&api, t, Method::PUT, &path).json(&json!({"archived": true}))).await;
    assert_problem(status, &problem, 428, "precondition_required");
    let (status, problem) = send(
        admin(&api, t, Method::PUT, &path)
            .header("if-match", "\"99\"")
            .json(&json!({"archived": true})),
    )
    .await;
    assert_problem(status, &problem, 412, "precondition_failed");

    // The project's own administrator archives it.
    let res = as_member(&api, t, &manager_session, Method::PUT, &path)
        .header("if-match", "*")
        .json(&json!({"archived": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 200);
    let etag = res.headers()["etag"].to_str().unwrap().to_owned();
    let archived: Value = res.json().await.unwrap();
    assert_eq!(archived["archived"], true);
    assert_eq!(etag, format!("\"{}\"", archived["version"]));

    // Read-only: nothing changes it, nor its links, nor what is under it.
    let node_path = format!("/api/v1/nodes/{plat}");
    let (status, current) = send(admin(&api, t, Method::GET, &node_path)).await;
    assert_eq!(
        (status, &current["archived"]),
        (200, &json!(true)),
        "still readable"
    );
    let refused = [
        admin(&api, t, Method::PUT, &node_path)
            .header("if-match", "*")
            .json(&json!({"name": "Renamed", "health": "on_track", "ownerIds": []})),
        admin(&api, t, Method::PUT, &format!("{node_path}/confidentiality"))
            .header("if-match", "*")
            .json(&json!({"confidential": true})),
        admin(&api, t, Method::PUT, &format!("{node_path}/parents/{compliance}"))
            .json(&json!({"weightPercent": 50})),
        admin(&api, t, Method::DELETE, &format!("{node_path}/parents/{program}")),
        admin(&api, t, Method::POST, "/api/v1/nodes")
            .json(&json!({"type": "project", "name": "Sub", "key": "SUB", "parentId": plat})),
    ];
    for request in refused {
        let (status, problem) = send(request).await;
        assert_problem(status, &problem, 409, "node.archived");
    }
    let (_, current) = send(admin(&api, t, Method::GET, &node_path)).await;
    assert_eq!(current["name"], "Identity platform");

    // Out of the active lists and of the aggregates; in the archived list.
    let (_, active) = send(admin(&api, t, Method::GET, "/api/v1/nodes")).await;
    assert_eq!(names(&active), ["Compliance", "IT Modernization"]);
    let (status, archived_list) = send(admin(&api, t, Method::GET, "/api/v1/nodes?archived=true")).await;
    assert_eq!(status, 200, "{archived_list}");
    assert_eq!(names(&archived_list), ["Identity platform"]);
    let (_, children) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/nodes/{program}/children"),
    ))
    .await;
    assert_eq!(children, json!([]));
    let (_, rows) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/hierarchy?under={program}"),
    ))
    .await;
    assert_eq!(rows, json!([]));
    recompute(&db, t).await;
    let (_, aggregate) = send(admin(&api, t, Method::GET, &aggregate_path)).await;
    assert_eq!(
        (&aggregate["projects"], &aggregate["openTickets"]),
        (&json!(0), &json!(0))
    );
    // A reader of the program finds it in the archived list too.
    let (_, seen) = send(as_member(
        &api,
        t,
        &reader_session,
        Method::GET,
        "/api/v1/nodes?archived=true",
    ))
    .await;
    assert_eq!(names(&seen), ["Identity platform"]);

    // Restored: editable, listed, counted again.
    let (status, restored) = send(archiving(&api, t, &plat, false)).await;
    assert_eq!((status, &restored["archived"]), (200, &json!(false)));
    let (status, renamed) = send(
        admin(&api, t, Method::PUT, &node_path)
            .header("if-match", "*")
            .json(&json!({"name": "Identity platform 2", "health": "on_track", "ownerIds": []})),
    )
    .await;
    assert_eq!((status, &renamed["name"]), (200, &json!("Identity platform 2")));
    let (_, active) = send(admin(&api, t, Method::GET, "/api/v1/nodes")).await;
    assert_eq!(
        names(&active),
        ["Compliance", "IT Modernization", "Identity platform 2"]
    );
    let (_, archived_list) = send(admin(&api, t, Method::GET, "/api/v1/nodes?archived=true")).await;
    assert_eq!(archived_list["items"], json!([]));
    recompute(&db, t).await;
    let (_, aggregate) = send(admin(&api, t, Method::GET, &aggregate_path)).await;
    assert_eq!(
        (&aggregate["projects"], &aggregate["openTickets"]),
        (&json!(1), &json!(12))
    );
}

#[tokio::test]
async fn sub_projects_are_archived_first_and_restored_after() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = node(
        &api,
        t,
        json!({"type": "project", "name": "Identity platform", "key": "PLAT"}),
    )
    .await;
    let cyber = node(
        &api,
        t,
        json!({"type": "project", "name": "Cyber permissions", "key": "CYBP", "parentId": plat}),
    )
    .await;

    let (status, problem) = send(archiving(&api, t, &plat, true)).await;
    assert_problem(status, &problem, 409, "node.active_children");
    let (status, _) = send(archiving(&api, t, &cyber, true)).await;
    assert_eq!(status, 200);
    let (status, _) = send(archiving(&api, t, &plat, true)).await;
    assert_eq!(status, 200);
    // Its archived sub-project still shows under it.
    let (_, children) = send(admin(
        &api,
        t,
        Method::GET,
        &format!("/api/v1/nodes/{plat}/children"),
    ))
    .await;
    assert_eq!(children[0]["name"], "Cyber permissions");

    let (status, problem) = send(archiving(&api, t, &cyber, false)).await;
    assert_problem(status, &problem, 409, "node.parent_archived");
    let (status, _) = send(archiving(&api, t, &plat, false)).await;
    assert_eq!(status, 200);
    let (status, _) = send(archiving(&api, t, &cyber, false)).await;
    assert_eq!(status, 200);

    // Asking again for what is already so changes nothing and refuses nothing.
    let (status, again) = send(archiving(&api, t, &plat, false)).await;
    assert_eq!((status, &again["archived"]), (200, &json!(false)));
}

#[tokio::test]
async fn archiving_stays_in_its_tenant() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let plat = node(
        &api,
        a,
        json!({"type": "project", "name": "Identity platform", "key": "PLAT"}),
    )
    .await;

    let (status, problem) = send(archiving(&api, b, &plat, true)).await;
    assert_problem(status, &problem, 404, "not_found");
    let (_, other) = send(admin(&api, b, Method::GET, "/api/v1/nodes?archived=true")).await;
    assert_eq!(other["items"], json!([]));
    let (status, _) = send(archiving(&api, a, &plat, true)).await;
    assert_eq!(status, 200);
    let (_, other) = send(admin(&api, b, Method::GET, "/api/v1/nodes?archived=true")).await;
    assert_eq!(
        other["items"],
        json!([]),
        "the other tenant's archived projects stay there"
    );

    let res = api
        .anonymous(Method::PUT, a, &format!("/api/v1/nodes/{plat}/archiving"))
        .header("if-match", "*")
        .json(&json!({"archived": false}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 401);
}
