//! Integration tests of the hierarchy's nodes (L03-US01, US02) on real
//! PostgreSQL and Valkey: creation at the root or under a parent, project
//! keys, the attachment matrix, changes with `If-Match`, who sees and who
//! changes; several weighted parents, cycles, candidate parents.
//! Roles are granted through the API (L03-US03), as an admin would.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi};
use infra::accounts::TenantRole;
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

async fn start(db: &TestDb) -> TestApi {
    common::start(db, multi()).await
}

fn assert_problem(status: u16, body: &Value, expected_status: u16, code: &str) {
    assert_eq!(status, expected_status, "{body}");
    assert_eq!(body["code"], code, "{body}");
}

fn field_codes(body: &Value) -> Vec<(String, String)> {
    body["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["field"].as_str().unwrap().to_owned(),
                e["code"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

async fn send(request: reqwest::RequestBuilder) -> (u16, Value) {
    json(request.send().await.unwrap()).await
}

/// A request of the seeded tenant admin of `tenant`.
fn admin(api: &TestApi, tenant: &SeedTenant, method: Method, path: &str) -> reqwest::RequestBuilder {
    api.request(method, tenant, path)
}

/// A request of `member` (a session opened for them).
fn as_member(
    api: &TestApi,
    tenant: &SeedTenant,
    session: &(String, String),
    method: Method,
    path: &str,
) -> reqwest::RequestBuilder {
    api.anonymous(method, tenant, path)
        .header("cookie", format!("mlbpp_session={}", session.0))
        .header("x-csrf-token", &session.1)
}

async fn session_of(api: &TestApi, account_id: Uuid, tenant: &SeedTenant) -> (String, String) {
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
    (opened.token, opened.session.csrf_token)
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

/// The default "Node admin" role of `tenant`.
async fn node_admin_role(db: &TestDb, tenant: &SeedTenant) -> Uuid {
    role(db, tenant, "Node admin").await
}

/// Grants `role` to the member `account` on `node`, as the tenant admin.
async fn grant(api: &TestApi, tenant: &SeedTenant, node: impl std::fmt::Display, role: Uuid, account: Uuid) {
    let (status, granted) = send(
        admin(api, tenant, Method::POST, &format!("/api/v1/nodes/{node}/grants"))
            .json(&json!({"roleId": role, "accountId": account})),
    )
    .await;
    assert_eq!(status, 201, "{granted}");
}

/// Creates a node as the tenant admin; its id.
async fn node(api: &TestApi, tenant: &SeedTenant, body: Value) -> String {
    let (status, node) = create(api, tenant, body).await;
    assert_eq!(status, 201, "{node}");
    node["id"].as_str().unwrap().to_owned()
}

/// The `name` of each item of `list`.
fn names(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap().to_owned())
        .collect()
}

async fn create(api: &TestApi, tenant: &SeedTenant, body: Value) -> (u16, Value) {
    send(admin(api, tenant, Method::POST, "/api/v1/nodes").json(&body)).await
}

#[tokio::test]
async fn a_tenant_admin_creates_a_project_whose_key_is_unique_in_the_tenant() {
    let db = TestDb::new().await;
    let api = start(&db).await;
    let body = json!({"type": "project", "name": "Identity platform", "key": "PLAT", "startDate": "2026-01-05", "endDate": "2026-12-18"});

    let res = admin(&api, &db.tenant_a, Method::POST, "/api/v1/nodes")
        .header("idempotency-key", "create-plat")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let location = res.headers()["location"].to_str().unwrap().to_owned();
    assert_eq!(res.headers()["etag"], "\"1\"");
    let (_, plat) = json(res).await;
    assert_eq!(
        (
            plat["type"].as_str(),
            plat["key"].as_str(),
            plat["health"].as_str()
        ),
        (Some("project"), Some("PLAT"), Some("on_track"))
    );
    assert_eq!(
        (plat["startDate"].as_str(), plat["endDate"].as_str()),
        (Some("2026-01-05"), Some("2026-12-18"))
    );
    assert_eq!(plat["parents"], json!([]), "at the root");
    assert_eq!(
        location,
        format!("/api/v1/nodes/{}", plat["id"].as_str().unwrap())
    );

    // The same request again: the same answer, nothing created twice.
    let (status, again) = send(
        admin(&api, &db.tenant_a, Method::POST, "/api/v1/nodes")
            .header("idempotency-key", "create-plat")
            .json(&body),
    )
    .await;
    assert_eq!((status, again["id"].clone()), (201, plat["id"].clone()));

    let (status, problem) = create(
        &api,
        &db.tenant_a,
        json!({"type": "project", "name": "Another", "key": "PLAT"}),
    )
    .await;
    assert_problem(status, &problem, 409, "node.key_taken");
    assert_eq!(field_codes(&problem), [("key".to_owned(), "taken".to_owned())]);

    // Another tenant has its own PLAT, and sees nothing of alpha's.
    let (status, _) = create(
        &api,
        &db.tenant_b,
        json!({"type": "project", "name": "Platform", "key": "PLAT"}),
    )
    .await;
    assert_eq!(status, 201);
    let (status, _) = send(admin(&api, &db.tenant_b, Method::GET, &location)).await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn nodes_are_checked_field_by_field_and_against_the_matrix() {
    let db = TestDb::new().await;
    let api = start(&db).await;
    let stranger = db.add_member(&db.tenant_b, TenantRole::Member).await.account_id;
    let (_, program) = create(
        &api,
        &db.tenant_a,
        json!({"type": "program", "name": "IT Modernization"}),
    )
    .await;
    let (_, project) = create(
        &api,
        &db.tenant_a,
        json!({"type": "project", "name": "Identity", "key": "PLAT"}),
    )
    .await;

    for (body, expected) in [
        (
            json!({"type": "project", "name": "No key"}),
            vec![("key", "required")],
        ),
        (
            json!({"type": "project", "name": "Bad key", "key": "plat"}),
            vec![("key", "format")],
        ),
        (
            json!({"type": "program", "name": "Keyed", "key": "PROG"}),
            vec![("key", "not_allowed")],
        ),
        (
            json!({"type": "portfolio", "name": " "}),
            vec![("name", "required")],
        ),
        (
            json!({"type": "portfolio", "name": "Late", "startDate": "2026-12-18", "endDate": "2026-01-05"}),
            vec![("endDate", "before_start")],
        ),
        (
            json!({"type": "program", "name": "Under a project", "parentId": project["id"]}),
            vec![("parentId", "not_allowed")],
        ),
        (
            json!({"type": "portfolio", "name": "Under a program", "parentId": program["id"]}),
            vec![("parentId", "not_allowed")],
        ),
        (
            json!({"type": "portfolio", "name": "Orphan", "parentId": Uuid::now_v7()}),
            vec![("parentId", "not_found")],
        ),
        (
            json!({"type": "portfolio", "name": "Foreign owner", "ownerIds": [stranger]}),
            vec![("ownerIds", "unknown_member")],
        ),
    ] {
        let (status, problem) = create(&api, &db.tenant_a, body.clone()).await;
        assert_problem(status, &problem, 400, "validation_failed");
        let expected: Vec<(String, String)> = expected
            .into_iter()
            .map(|(f, c)| (f.to_owned(), c.to_owned()))
            .collect();
        assert_eq!(field_codes(&problem), expected, "{body}");
    }
    let (status, _) = create(
        &api,
        &db.tenant_a,
        json!({"type": "project", "name": "Unknown", "key": "UNK", "kind": "x"}),
    )
    .await;
    assert_eq!(status, 400, "unknown fields are refused");

    // A project under a project, a program under a program: allowed.
    let (status, sub) = create(
        &api,
        &db.tenant_a,
        json!({"type": "project", "name": "Sub", "key": "SUB", "parentId": project["id"]}),
    )
    .await;
    assert_eq!(
        (status, sub["parents"][0]["name"].as_str()),
        (201, Some("Identity"))
    );
    let (status, _) = create(
        &api,
        &db.tenant_a,
        json!({"type": "program", "name": "Sub-program", "parentId": program["id"]}),
    )
    .await;
    assert_eq!(status, 201);
}

#[tokio::test]
async fn node_admins_create_children_under_their_nodes_only() {
    let db = TestDb::new().await;
    let api = start(&db).await;
    let (_, program) = create(
        &api,
        &db.tenant_a,
        json!({"type": "program", "name": "IT Modernization"}),
    )
    .await;
    let (_, other) = create(
        &api,
        &db.tenant_a,
        json!({"type": "program", "name": "Compliance"}),
    )
    .await;
    let program_id: Uuid = program["id"].as_str().unwrap().parse().unwrap();
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await.account_id;
    let session = session_of(&api, member, &db.tenant_a).await;
    let create_as =
        |body: Value| as_member(&api, &db.tenant_a, &session, Method::POST, "/api/v1/nodes").json(&body);
    let child = |parent: &Value| json!({"type": "project", "name": "Identity platform", "key": "PLAT", "parentId": parent["id"]});

    // Without "administer node": refused anywhere.
    let (status, problem) = send(create_as(child(&program))).await;
    assert_problem(status, &problem, 403, "forbidden");
    let (status, _) = send(as_member(
        &api,
        &db.tenant_a,
        &session,
        Method::GET,
        &format!("/api/v1/nodes/{program_id}"),
    ))
    .await;
    assert_eq!(status, 404, "nor does the member see it");

    // Node admin of the program: it applies to their next request.
    let node_admin = node_admin_role(&db, &db.tenant_a).await;
    grant(&api, &db.tenant_a, program_id, node_admin, member).await;
    let (status, created) = send(create_as(child(&program))).await;
    assert_eq!(status, 201, "{created}");
    assert_eq!(
        created["parents"],
        json!([{"id": program["id"], "type": "program", "name": "IT Modernization"}])
    );
    let (status, _) = send(create_as(child(&other))).await;
    assert_eq!(status, 403, "not under a node they do not administer");
    let (status, _) = send(create_as(
        json!({"type": "project", "name": "Root", "key": "ROOT"}),
    ))
    .await;
    assert_eq!(status, 403, "not at the root");
    let (status, _) = send(create_as(
        json!({"type": "project", "name": "Ghost", "key": "GHOST", "parentId": Uuid::now_v7()}),
    ))
    .await;
    assert_eq!(status, 403, "an unknown parent is not revealed");

    // They see and change the program, not the other one.
    let (status, seen) = send(as_member(
        &api,
        &db.tenant_a,
        &session,
        Method::GET,
        &format!("/api/v1/nodes/{program_id}"),
    ))
    .await;
    assert_eq!((status, seen["name"].as_str()), (200, Some("IT Modernization")));
    let (_, page) = send(as_member(
        &api,
        &db.tenant_a,
        &session,
        Method::GET,
        "/api/v1/nodes?type=program",
    ))
    .await;
    let names: Vec<&str> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["IT Modernization"]);
    let (status, _) = send(
        as_member(
            &api,
            &db.tenant_a,
            &session,
            Method::PUT,
            &format!("/api/v1/nodes/{program_id}"),
        )
        .header("if-match", "*")
        .json(&json!({"name": "IT Modernization", "health": "at_risk"})),
    )
    .await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn a_portfolio_is_changed_with_the_version_it_was_read_at() {
    let db = TestDb::new().await;
    let api = start(&db).await;
    let marion = db.add_member(&db.tenant_a, TenantRole::Member).await.account_id;
    let (_, portfolio) = create(
        &api,
        &db.tenant_a,
        json!({"type": "portfolio", "name": "Digital transformation"}),
    )
    .await;
    let path = format!("/api/v1/nodes/{}", portfolio["id"].as_str().unwrap());
    let changes = json!({
        "name": "Digital transformation",
        "description": "Every digital project of Acme",
        "objectives": "Cloud first by 2027",
        "startDate": "2026-01-01",
        "endDate": "2027-12-31",
        "health": "at_risk",
        "ownerIds": [marion],
    });

    let (status, problem) = send(admin(&api, &db.tenant_a, Method::PUT, &path).json(&changes)).await;
    assert_problem(status, &problem, 428, "precondition_required");
    let res = admin(&api, &db.tenant_a, Method::PUT, &path)
        .header("if-match", "\"1\"")
        .json(&changes)
        .send()
        .await
        .unwrap();
    assert_eq!(
        (res.status().as_u16(), res.headers()["etag"].to_str().unwrap()),
        (200, "\"2\"")
    );
    let (_, changed) = json(res).await;
    assert_eq!(
        (changed["health"].as_str(), changed["objectives"].as_str()),
        (Some("at_risk"), Some("Cloud first by 2027"))
    );
    assert_eq!(changed["owners"][0]["accountId"], marion.to_string());
    assert_eq!(
        (changed["startDate"].as_str(), changed["endDate"].as_str()),
        (Some("2026-01-01"), Some("2027-12-31"))
    );

    // Read at version 1, changed meanwhile: refused.
    let (status, problem) = send(
        admin(&api, &db.tenant_a, Method::PUT, &path)
            .header("if-match", "\"1\"")
            .json(&changes),
    )
    .await;
    assert_problem(status, &problem, 412, "precondition_failed");
    let res = admin(&api, &db.tenant_a, Method::GET, &path)
        .header("if-none-match", "\"2\"")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 304);

    // A member without rights neither sees nor changes it; another tenant knows nothing of it.
    let member = db.add_member(&db.tenant_a, TenantRole::Member).await.account_id;
    let session = session_of(&api, member, &db.tenant_a).await;
    let (status, _) = send(
        as_member(&api, &db.tenant_a, &session, Method::PUT, &path)
            .header("if-match", "*")
            .json(&changes),
    )
    .await;
    assert_eq!(status, 404);
    let (status, _) = send(
        admin(&api, &db.tenant_b, Method::PUT, &path)
            .header("if-match", "*")
            .json(&changes),
    )
    .await;
    assert_eq!(status, 404);
    let (_, page) = send(as_member(
        &api,
        &db.tenant_a,
        &session,
        Method::GET,
        "/api/v1/nodes",
    ))
    .await;
    assert_eq!(page["items"], json!([]));
}

#[tokio::test]
async fn a_project_gets_a_second_weighted_parent_but_never_a_cycle() {
    let db = TestDb::new().await;
    let api = start(&db).await;
    let t = &db.tenant_a;
    let digital = node(&api, t, json!({"type": "portfolio", "name": "Digital"})).await;
    let modernization = node(
        &api,
        t,
        json!({"type": "program", "name": "IT Modernization", "parentId": digital}),
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
    let cyber = node(
        &api,
        t,
        json!({"type": "project", "name": "Cyber permissions", "key": "CYBP", "parentId": plat}),
    )
    .await;
    let link = |child: &str, parent: &str| {
        admin(
            &api,
            t,
            Method::PUT,
            &format!("/api/v1/nodes/{child}/parents/{parent}"),
        )
    };
    let get = |path: String| admin(&api, t, Method::GET, &path);

    // Compliance as a second parent at 40%: the first link stays at 100%.
    let (status, linked) = send(link(&plat, &compliance).json(&json!({"weightPercent": 40}))).await;
    assert_eq!(status, 201, "{linked}");
    assert_eq!(
        (
            &linked["name"],
            &linked["weightPercent"],
            &linked["parents"][0]["name"]
        ),
        (&json!("Compliance"), &json!(40), &json!("Digital"))
    );
    let (status, parents) = send(get(format!("/api/v1/nodes/{plat}/parents"))).await;
    assert_eq!(status, 200);
    let weights: Vec<(&str, i64)> = parents
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["name"].as_str().unwrap(), l["weightPercent"].as_i64().unwrap()))
        .collect();
    assert_eq!(weights, [("Compliance", 40), ("IT Modernization", 100)]);
    let (_, node_read) = send(get(format!("/api/v1/nodes/{plat}"))).await;
    assert_eq!(names(&node_read["parents"]), ["Compliance", "IT Modernization"]);

    // Putting it again sets the weighting; it stays a percentage.
    let (status, changed) = send(link(&plat, &compliance).json(&json!({"weightPercent": 60}))).await;
    assert_eq!((status, &changed["weightPercent"]), (200, &json!(60)));
    let (status, problem) = send(link(&plat, &compliance).json(&json!({"weightPercent": 101}))).await;
    assert_problem(status, &problem, 400, "validation_failed");
    assert_eq!(
        field_codes(&problem),
        [("weightPercent".to_owned(), "out_of_range".to_owned())]
    );
    let (status, children) = send(get(format!("/api/v1/nodes/{compliance}/children"))).await;
    assert_eq!(status, 200);
    assert_eq!(
        (
            names(&children),
            &children[0]["key"],
            &children[0]["weightPercent"]
        ),
        (vec!["Identity platform".to_owned()], &json!("PLAT"), &json!(60))
    );

    // A program under a project: the matrix refuses it.
    let (status, problem) = send(link(&modernization, &plat).json(&json!({}))).await;
    assert_problem(status, &problem, 409, "node.parent_not_allowed");
    // Its own sub-project as a parent: a cycle.
    let (status, problem) = send(link(&plat, &cyber).json(&json!({}))).await;
    assert_problem(status, &problem, 409, "node.cycle");
    let (status, problem) = send(link(&plat, &plat).json(&json!({}))).await;
    assert_problem(status, &problem, 409, "node.cycle");

    // The parent picker shows why.
    let (status, found) = send(get(format!("/api/v1/nodes/{plat}/parent-candidates?search=c"))).await;
    assert_eq!(status, 200, "{found}");
    let refusals: Vec<(&str, &Value)> = found
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["name"].as_str().unwrap(), &c["refusal"]))
        .collect();
    assert_eq!(
        refusals,
        [
            ("Compliance", &json!("already_parent")),
            ("Cyber permissions", &json!("cycle")),
        ]
    );
    let (_, found) = send(get(format!(
        "/api/v1/nodes/{modernization}/parent-candidates?search=identity"
    )))
    .await;
    assert_eq!(found[0]["refusal"], "not_allowed");
    let (_, found) = send(get(format!(
        "/api/v1/nodes/{cyber}/parent-candidates?search=compl"
    )))
    .await;
    assert_eq!(
        (&found[0]["name"], &found[0]["refusal"]),
        (&json!("Compliance"), &Value::Null)
    );
    let (status, _) = send(get(format!("/api/v1/nodes/{plat}/parent-candidates?limit=51"))).await;
    assert_eq!(status, 400);

    // Descendants come once, however many paths lead to them.
    let (_, page) = send(get(format!("/api/v1/nodes?descendantOf={digital}"))).await;
    let mut below = names(&page["items"]);
    below.sort();
    assert_eq!(
        below,
        [
            "Compliance",
            "Cyber permissions",
            "IT Modernization",
            "Identity platform"
        ]
    );

    // Removing a parent link: the closure follows.
    let unlink = || {
        admin(
            &api,
            t,
            Method::DELETE,
            &format!("/api/v1/nodes/{plat}/parents/{modernization}"),
        )
    };
    let (status, _) = send(unlink()).await;
    assert_eq!(status, 204);
    let (status, _) = send(unlink()).await;
    assert_eq!(status, 404);
    let (_, page) = send(get(format!("/api/v1/nodes?ancestorOf={cyber}"))).await;
    let mut above = names(&page["items"]);
    above.sort();
    assert_eq!(above, ["Compliance", "Digital", "Identity platform"]);
    let (_, page) = send(get(format!("/api/v1/nodes?descendantOf={modernization}"))).await;
    assert_eq!(page["items"], json!([]));

    // Another tenant knows nothing of these nodes.
    let b = &db.tenant_b;
    let other = node(&api, b, json!({"type": "program", "name": "Elsewhere"})).await;
    for (method, path) in [
        (Method::GET, format!("/api/v1/nodes/{plat}/parents")),
        (Method::GET, format!("/api/v1/nodes/{plat}/children")),
        (Method::GET, format!("/api/v1/nodes/{plat}/parent-candidates")),
        (Method::PUT, format!("/api/v1/nodes/{plat}/parents/{other}")),
        (Method::PUT, format!("/api/v1/nodes/{other}/parents/{compliance}")),
        (
            Method::DELETE,
            format!("/api/v1/nodes/{plat}/parents/{compliance}"),
        ),
    ] {
        let (status, _) = send(admin(&api, b, method.clone(), &path).json(&json!({}))).await;
        assert_eq!(status, 404, "{method} {path}");
    }
    let (_, found) = send(
        admin(
            &api,
            b,
            Method::GET,
            &format!("/api/v1/nodes/{other}/parent-candidates"),
        )
        .json(&json!({})),
    )
    .await;
    assert_eq!(found, json!([]));
}

#[tokio::test]
async fn linking_needs_both_nodes_administered() {
    let db = TestDb::new().await;
    let api = start(&db).await;
    let t = &db.tenant_a;
    let modernization = node(&api, t, json!({"type": "program", "name": "IT Modernization"})).await;
    let compliance = node(&api, t, json!({"type": "program", "name": "Compliance"})).await;
    let plat = node(
        &api,
        t,
        json!({"type": "project", "name": "Identity platform", "key": "PLAT", "parentId": modernization}),
    )
    .await;
    let member = db.add_member(t, TenantRole::Member).await.account_id;
    let session = session_of(&api, member, t).await;
    let as_lucas = |method: Method, path: String| as_member(&api, t, &session, method, &path);
    let link_path = format!("/api/v1/nodes/{plat}/parents/{compliance}");

    // Node admin of the project, reader of Compliance.
    grant(&api, t, &plat, node_admin_role(&db, t).await, member).await;
    grant(&api, t, &compliance, role(&db, t, "Reader").await, member).await;
    let (_, found) = send(as_lucas(
        Method::GET,
        format!("/api/v1/nodes/{plat}/parent-candidates?search=compliance"),
    ))
    .await;
    assert_eq!(found[0]["refusal"], "forbidden", "{found}");
    let (status, problem) = send(as_lucas(Method::PUT, link_path.clone()).json(&json!({}))).await;
    assert_problem(status, &problem, 403, "forbidden");
    // Parents the member does not see are left out.
    let (_, parents) = send(as_lucas(Method::GET, format!("/api/v1/nodes/{plat}/parents"))).await;
    assert_eq!(parents, json!([]));

    // Node admin of Compliance too: linked, then unlinked.
    grant(&api, t, &compliance, node_admin_role(&db, t).await, member).await;
    let (_, found) = send(as_lucas(
        Method::GET,
        format!("/api/v1/nodes/{plat}/parent-candidates?search=compliance"),
    ))
    .await;
    assert_eq!(found[0]["refusal"], Value::Null, "{found}");
    let (status, linked) =
        send(as_lucas(Method::PUT, link_path.clone()).json(&json!({"weightPercent": 40}))).await;
    assert_eq!(status, 201, "{linked}");
    let (_, parents) = send(as_lucas(Method::GET, format!("/api/v1/nodes/{plat}/parents"))).await;
    assert_eq!(names(&parents), ["Compliance"]);
    // Not the link to IT Modernization, which they do not administer.
    let (status, _) = send(as_lucas(
        Method::DELETE,
        format!("/api/v1/nodes/{plat}/parents/{modernization}"),
    ))
    .await;
    assert_eq!(status, 404, "a parent they do not see is not revealed");
    let (status, _) = send(as_lucas(Method::DELETE, link_path)).await;
    assert_eq!(status, 204);
}
