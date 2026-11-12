//! Integration tests of the hierarchy's nodes (L03-US01) on real PostgreSQL
//! and Valkey: creation at the root or under a parent, project keys, the
//! attachment matrix, changes with `If-Match`, who sees and who changes.
//! Grants on nodes come with L03-US03: a test grant source stands for them.

#![allow(clippy::unwrap_used)]

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use common::{TestApi, json, multi, start_with};
use infra::accounts::TenantRole;
use infra::permissions::{GrantFuture, GrantSource};
use infra::sqlx::PgConnection;
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

/// Grants of roles on given nodes, without inheritance: enough to stand for
/// the grants of L03-US03.
#[derive(Debug, Default)]
struct TestGrants(Mutex<HashMap<(Uuid, Uuid), Vec<Uuid>>>);

impl TestGrants {
    fn grant(&self, account_id: Uuid, node: Uuid, role_id: Uuid) {
        self.0
            .lock()
            .unwrap()
            .entry((account_id, node))
            .or_default()
            .push(role_id);
    }
}

impl GrantSource for TestGrants {
    fn roles_of<'a>(
        &'a self,
        _: &'a mut PgConnection,
        account_id: Uuid,
        node: Option<Uuid>,
    ) -> GrantFuture<'a, Vec<Uuid>> {
        let roles = node
            .and_then(|node| self.0.lock().unwrap().get(&(account_id, node)).cloned())
            .unwrap_or_default();
        Box::pin(async move { Ok(roles) })
    }

    fn grants_of_role<'a>(&'a self, _: &'a mut PgConnection, _: Uuid) -> GrantFuture<'a, u64> {
        Box::pin(async { Ok(0) })
    }
}

async fn start(db: &TestDb, grants: Arc<TestGrants>) -> TestApi {
    start_with(db, multi(), move |state, settings| {
        api::app(state.with_grants(grants), settings)
    })
    .await
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

/// The default "Node admin" role of `tenant`.
async fn node_admin_role(db: &TestDb, tenant: &SeedTenant) -> Uuid {
    infra::sqlx::query_scalar("SELECT id FROM role WHERE tenant_id = $1 AND label ->> 'en' = 'Node admin'")
        .bind(tenant.id)
        .fetch_one(&db.owner_pool)
        .await
        .unwrap()
}

async fn create(api: &TestApi, tenant: &SeedTenant, body: Value) -> (u16, Value) {
    send(admin(api, tenant, Method::POST, "/api/v1/nodes").json(&body)).await
}

#[tokio::test]
async fn a_tenant_admin_creates_a_project_whose_key_is_unique_in_the_tenant() {
    let db = TestDb::new().await;
    let api = start(&db, Arc::default()).await;
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
    let api = start(&db, Arc::default()).await;
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
    let grants = Arc::new(TestGrants::default());
    let api = start(&db, grants.clone()).await;
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

    // Node admin of the program; a grant invalidates the cached permissions,
    // as L03-US03's grants will.
    grants.grant(member, program_id, node_admin_role(&db, &db.tenant_a).await);
    infra::permissions::PermissionCache::new(api.valkey.clone(), api.key_prefix.clone())
        .invalidate(db.tenant_a.id)
        .await
        .unwrap();
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
    let api = start(&db, Arc::default()).await;
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
